# Proposed Architecture Enhancements & Specification Amendments
## Document Version: 0.1 (Draft Proposal for Integration into ARCHITECTURE.md)

> **Context:** This document collects proposed architectural refinements, layout modifications, and tooling additions identified during architectural review (referenced in `notes.md`). These proposals address volume resizing, metadata expansion, compression design boundaries, and metadata SSD capacity monitoring without altering the core v0.2 `ARCHITECTURE.md` file until reviewed and approved.

---

## 1. Dynamic Volume Resizing (HDD & SSD Expansion)

### Problem Statement
In the original v0.2 specification, the Level-1 Checksum Table and Block Allocation Bitmap on the metadata SSD (`DNPFS_META`) were described as contiguous flat arrays written sequentially at format time. If metadata structures are packed tightly without gaps (e.g., `Checksum Table` followed immediately by `Bad Block Map` and `Journal`), expanding `total_data_blocks` (e.g. growing the data HDD from 1 TB to 2 TB) would require growing the checksum table *in place*, which is blocked by adjacent metadata structures.

### Proposed Specification Change: Segmented Checksum & Bitmap Layout

Instead of storing the Checksum Table and Allocation Bitmap as contiguous monolithic arrays:

1. **Segmented Checksum Table:**
   - The Checksum Table is divided into **1 MB Checksum Segments** on the metadata SSD.
   - Each 1 MB segment holds 43,690 `BlockChecksumEntry` records (24 bytes each), covering a **~170.6 MB span** of 4KB data blocks.
   - Looking up a checksum entry for data block $B$ remains a constant-time $O(1)$ arithmetic operation:
     $$\text{Segment Index} = \left\lfloor \frac{B}{\text{BLOCKS\_PER\_SEGMENT}} \right\rfloor$$
     $$\text{Offset in Segment} = (B \pmod{\text{BLOCKS\_PER\_SEGMENT}}) \times 24\text{ bytes}$$
   - Segment pointers are managed in a **Master Checksum Index Table** on the metadata SSD. To prevent this index table itself from becoming a contiguous growth bottleneck, it is organized as a **chained metadata block structure** (a linked list of 4KB index blocks, where each block stores up to 510 segment pointers and a `next_index_block` pointer to the next block). As capacity grows, new index blocks are dynamically allocated from the general metadata pool.

2. **HDD Expansion Procedure (`dnpfs-resize --data`):**
   - **Data Device (`DNPFS_DATA`):** Contains zero filesystem structures (only raw 4KB blocks and Sector 0 header).
   - When the underlying HDD partition is enlarged (e.g., via `parted` or `lvextend`), `dnpfs-resize` queries `ioctl(BLKGETSIZE64)`:
     1. Relocates the **Backup Data Signature Header** from the old last sector to the newly expanded last sector of `DNPFS_DATA`.
     2. Updates `Superblock.total_data_blocks` and `Superblock.free_data_blocks` on `DNPFS_META`.
     3. Allocates new 1 MB Checksum Segments from `DNPFS_META`'s free metadata pool to cover the new block range.
     4. Appends the new blocks to the free block allocation pool.
   - **Data Safety:** No existing data blocks or inodes are moved or shifted.

3. **SSD Expansion Procedure (`dnpfs-resize --meta`):**
   - When the metadata SSD partition is enlarged (e.g., upgrading from a 16 GB SSD to a 120 GB or 500 GB SSD, or resizing `/dev/nvme0n1p2`):
     1. Updates `Superblock.total_metadata_blocks` and free metadata block bitmaps to claim the new SSD space.
     2. Instantly expands capacity for additional Inode blocks, WAL journal entries, and Checksum Segments.

4. **New Tool Specification (`dnpfs-resize`):**
   - Add `dnpfs-resize` to Section 9 (Userspace Tools):
     ```bash
     dnpfs-resize --meta /dev/nvme0n1p2 --data /dev/sdb1
     ```
   - Automatically detects block device expansion and updates metadata structures online or offline.

---

## 2. Metadata Capacity Monitoring & Backup Expansion

### Problem Statement
1. **Capacity Monitoring Gap:** Section 13 (S.M.A.R.T. Integration) monitors hardware drive *health* (remapped sectors, wear level), but does not monitor metadata SSD *capacity utilization*. Running out of metadata space currently results in an unannounced `ENOSPC` error on write operations (Section 5).
2. **Sector-for-Sector Backup Limitation:** Section 14 (Metadata Backup System) specifies `dnpfs-backup` and `dnpfs-restore` as sector-for-sector image copies. Restoring a 16 GB metadata backup image onto a 120 GB replacement SSD would leave 104 GB unallocated and unusable.

### Proposed Specification Change:

1. **Metadata Utilization Monitoring in `dnpfsd`:**
   - `dnpfsd` will monitor metadata capacity utilization (inodes used vs. free, metadata blocks used vs. free).
   - **Threshold Escalation Rules:**
     - **Warning Alert (80% utilization):** Issue a system notification advising administrator of rising metadata usage.
     - **Critical Alert (90% utilization):** Advise metadata volume expansion (`dnpfs-resize`) or file pruning.
     - **Emergency Protection (95% utilization):** Trigger an automatic incremental metadata backup to prevent data loss.

2. **Expanded Backup Restore (`dnpfs-restore --expand`):**
   - Update Section 14 restore procedure:
     ```bash
     dnpfs-backup --restore backup.img --target /dev/nvme0n1p2 --expand
     ```
   - When `--expand` is passed, `dnpfs-restore` writes the image and immediately executes an in-line metadata space extension to claim the full capacity of the target device.

---

## 3. Compression Architecture Clarification

### Problem Statement
Questions were raised regarding bit-level Run-Length Encoding (RLE) vs. extent-level LZ4/ZSTD compression during write operations.

### Proposed Specification Change & Clarification:

1. **Why Bit-Level RLE is Excluded:**
   - DNPFS's core architecture relies on a deterministic **fixed 4KB physical block indexing model** ($1 \text{ logical block} = 1 \text{ physical 4KB block}$).
   - Bit-level or byte-level RLE produces variable-length bit runs. This breaks direct $O(1)$ block-number indexing for the Checksum Table, corrupts 100-block Level-2 Merkle group checksum boundaries, and complicates Bad Block Remapping (where 1 bad physical sector maps 1-to-1 to a clean sector).
   - Real-world media, binary, and backup workloads targeted by DNPFS contain negligible runs of identical single bits.

2. **Sparse Files (Native Zero-Fill):**
   - Large runs of repeated zeros (sparse regions) are **already handled natively at 0 storage cost** via sparse file extents (`DnpfsExtent.block_count > 0` with zero physical allocation), requiring no disk writes or metadata overhead.

3. **Extent-Level LZ4/ZSTD Compression (V2 Standard):**
   - Transparent compression in DNPFS (deferred to V2) operates strictly at the **extent / 4KB page boundary layer**.
   - Data blocks are compressed in memory using LZ4 or ZSTD prior to disk submission. Compression flags and compressed sizes are tracked in metadata extent descriptors without altering the fixed LBA indexing model.

---

## 4. Documentation Clarification on Metadata SSD Sizing

### Problem Statement
Mentions of "16GB SSD" in `README.md` (line 114) and `ARCHITECTURE.md` (line 38) could be misinterpreted as a hardcoded format limit rather than a minimum illustrative ratio.

### Proposed Specification Change:

1. **Explicit Clarification in Section 3 & Section 4:**
   - Add explicit wording clarifying that **16 GB is an illustrative ratio example** ($1.0\%$ minimum guideline based on a 1.6 TB data HDD), NOT a format ceiling.
   - Any SSD capacity (120 GB, 256 GB, 500 GB, 2 TB+) is natively supported by `dnpfs-format` and `dnpfs.ko` without format modifications.
   - Over-provisioning the metadata SSD (e.g., using a 120 GB SSD for a 2 TB volume) is explicitly recommended as a zero-cost mitigation against metadata space exhaustion.

---

## 5. DNPFS_BOOTSTRAP Design, Auto-Update Security, and FUSE Integration

### Problem Statement
The `DNPFS_BOOTSTRAP` partition is a 512MB FAT32 partition on the metadata SSD designed to enable zero-download mounting on any machine. However, the original specification does not address:
1. **Driver Staleness:** A static compiled binary will drift out of sync with subsequent volume upgrades/format version changes.
2. **Supply-Chain / Update Hijacking (CVE Risk):** Downloading and running replacement binaries on arbitrary hosts exposes a critical execution vulnerability.
3. **Cross-Platform Limits:** FUSE drivers on Windows and macOS have system-level library dependencies that are not preinstalled on untrusted hosts.

### Proposed Specification Change:

1. **Auto-Update with Cryptographic Signature Verification:**
   - Instead of naive "latest release" fetching, the auto-updater queries releases matching the volume's active on-disk format (`incompat_flags`).
   - **Key Rotation and Verification-Before-Promotion (A/B Partition Layout):**
     - The bootstrap partition contains a write-protected primary slot holding the factory-shipped binary and a pinned **Root Public Key**.
     - To allow key rotation without hardcoding vulnerabilities (e.g. key compromise or algorithm deprecation), DNPFS uses a **Key Rotation Certificate Chain**: the active signing key for new binaries is verified against a short-lived key-rotation certificate signed by the offline Master Root private key.
     - Fetched updates are written to a secondary slot. The updated binary is only promoted to active after its certificate chain successfully validates back to the immutable Root Public Key.
     - If the update fails to boot, verify, or mount, the system automatically falls back to the read-only primary slot.
     - Write-back of updates to the bootstrap partition is strictly gated behind an explicit administrator opt-in prompt.

2. **Cross-Platform FUSE Integration & Limitations:**
   - **Linux:** Native `dnpfs-fuse` compiled for x86_64 and AArch64 ships inside the bootstrap partition, mounting seamlessly with zero extra dependencies.
   - **Windows & macOS:** The bootstrap partition ships portable `dnpfs-fuse` wrappers targeting userspace FUSE frameworks (**WinFsp** on Windows and **macFUSE** on macOS). 
   - **Disclaimer:** The installer will detect if WinFsp/macFUSE is missing on the host OS and prompt the user to install the dependency first, preserving userspace filesystem access without requiring kernel-level driver certificates.
   - **Performance Ceiling:** The documentation must clarify that userspace FUSE mounting is optimized for portability and rescue recovery. Production deployments should default to the native `dnpfs.ko` kernel module to avoid double userspace-kernel context crossing.

---

## Summary of Proposed Changes Checklist

| Feature / Issue | Proposed Fix | New/Updated Tool |
|---|---|---|
| HDD Data Expansion | Move backup header to new tail sector; append free blocks | `dnpfs-resize --data` |
| SSD Meta Expansion | Segmented 1 MB Checksum Segments; expand free meta bitmap | `dnpfs-resize --meta` |
| Metadata Utilization Alerts | Add 80%/90%/95% utilization monitoring in background daemon | `dnpfsd` alert subsystem |
| Restoring to Larger SSD | Add `--expand` flag to metadata restore tool | `dnpfs-backup --restore --expand` |
| Compression Strategy | Maintain fixed 4KB page indexing; LZ4/ZSTD at extent level in V2 | Documentation clarification |
| Metadata SSD Capacity Text | Clarify 16 GB is illustrative minimum ratio, not a cap | `README.md` & `ARCHITECTURE.md` text fix |
| Bootstrap Auto-Updates | Cryptographically verified A/B updates, locked public keys, FUSE limits | `DNPFS_BOOTSTRAP` spec update |


