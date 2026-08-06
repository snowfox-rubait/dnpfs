# DNPFS — Definitely Not Paranoid File System
## Architecture Specification v0.2

> *"It's not paranoia if the disk is actually dying."*

---

## Table of Contents

1. [Overview](#overview)
2. [Design Philosophy](#design-philosophy)
3. [Storage Limits](#storage-limits)
4. [Physical Layout](#physical-layout)
5. [Core Data Structures](#core-data-structures)
6. [Checksum Strategy — Per-Block and Grouped](#checksum-strategy)
7. [The Allocation Manifest — allocation.dry](#the-allocation-manifest)
8. [Transaction Lifecycle](#transaction-lifecycle)
9. [Driver Architecture](#driver-architecture)
10. [Cache Coherency and Write Ordering](#cache-coherency-and-write-ordering)
11. [Power Loss and Crash Recovery](#power-loss-and-crash-recovery)
12. [Bad Sector Tracking and Silent Failure Detection](#bad-sector-tracking-and-silent-failure-detection)
13. [S.M.A.R.T. Integration](#smart-integration)
14. [Metadata Backup System](#metadata-backup-system)
15. [Encryption](#encryption)
16. [RAID Compatibility](#raid-compatibility)
17. [TRIM/Discard Coordination](#trimdiscard-coordination)
18. [Concurrent Operation Handling](#concurrent-operation-handling)
19. [Defragmentation Strategy](#defragmentation-strategy)
20. [Recovery Tooling](#recovery-tooling)
21. [Known Limitations](#known-limitations)
22. [Known Unsolvable Problems](#known-unsolvable-problems)
23. [Future Work](#future-work)

---

## Overview

DNPFS is a Linux filesystem designed around a single core principle: **separate raw data storage from all filesystem structures**. Every byte of metadata, journaling, checksums, bad block maps, and filesystem bookkeeping lives on a small, fast secondary device (typically a 16GB SSD). The primary device (typically a large HDD) stores only raw data blocks — nothing else.

This separation enables:

- Faster sequential writes on the data drive (no head seeking between data and metadata)
- Near-instant metadata lookups via fast SSD
- Metadata redundancy and backup without touching the data drive
- Precise forensic recovery after partial writes or power loss
- Per-block checksum verification stored independently from data

DNPFS is **not** designed for OS installation. It is a storage filesystem for large files, raw data, media, backups, and bulk content.

---

## Design Philosophy

**Paranoia is a feature.** Every write is treated as a potential failure. Every confirmation is verified. Every operation leaves a recoverable trail.

**Metadata is more valuable than data.** The 2TB data drive dying is survivable with backups. The 16GB metadata drive dying without a backup means the 2TB becomes a pile of unaddressed blocks. The system treats the metadata drive accordingly — multiple redundant backups, checksummed, version-tracked.

**Dry before wet.** No write operation touches either device until a full simulation has been committed, verified, and reserved. The dry run manifest is the source of truth for recovery.

**Explicit over implicit.** Write ordering, flush guarantees, TRIM suppression, and cache coordination are all explicit driver responsibilities — never assumed from the underlying hardware.

---

## Storage Limits

All address and size fields in DNPFS use `u64` — 64-bit unsigned integers. This means:

| Limit | Value |
|---|---|
| Max data device size | ~16 exabytes (2^64 bytes) |
| Max file size | ~16 exabytes (u64 file_size field) |
| Max number of inodes | 2^64 (~18 quintillion files) |
| Max block pointers per file | 2^64 |
| Max volume size | ~16 exabytes |

For practical purposes DNPFS has no meaningful storage limit. 16 exabytes exceeds all hardware that exists today by orders of magnitude. The address space will not be a constraint for the foreseeable future.

**Why FAT32 has limits and DNPFS does not:**

FAT32 uses 32-bit fields for file size — a design decision made in the 1980s when files larger than a few MB were inconceivable. The 4GB per-file limit is a direct consequence of a 32-bit size field (2^32 bytes = 4GB). DNPFS uses 64-bit fields throughout, designed with no artificial ceiling.

**Filename and path length** are explicitly fixed to standard Linux POSIX limits: **255 bytes** per filename component (`NAME_MAX`, matching ext4/btrfs) and **4096 bytes** for total path length (`PATH_MAX`). These values define the fixed buffer allocations in directory entries and inode structures for format v1.

---

## Physical Layout

```
┌─────────────────────────────────┐    ┌──────────────────────────────────────┐
│        META DEVICE              │    │           DATA DEVICE                │
│     (16GB SSD recommended)      │    │        (2TB HDD typical)             │
│                                 │    │                                      │
│  Superblock                     │    │  Raw data blocks only                │
│  Inode table                    │    │  No metadata                         │
│  Directory tree                 │    │  No journal                          │
│  Block allocation bitmap        │    │  No superblock                       │
│  Journal / Write-ahead log      │    │  No directory entries                │
│  Block checksum table           │    │                                      │
│  Bad block map                  │    │  Sequential layout preferred         │
│  S.M.A.R.T. history cache       │    │  Head never seeks for metadata       │
│  Pending reservation table      │    │                                      │
│  allocation.dry manifests       │    │                                      │
│  Operation snapshots            │    │                                      │
│  Backup metadata copies         │    │                                      │
└─────────────────────────────────┘    └──────────────────────────────────────┘
         │                                            │
         └──────────────── DNPFS Driver ──────────────┘
                    (kernel module + userspace daemon)
```

Both devices are identified by **UUID only** — never by `/dev/sdX` names, which can change across reboots. The driver enforces pairing at mount time by verifying UUIDs stored in both device headers. To prevent the data device from appearing as completely blank/unformatted raw space to OS tools (which could lead to accidental partitioning or formatting), the data device contains a minimal 512-byte **DNPFS Data Signature Header** at Sector 0. This header stores the data device's own UUID and its paired metadata device's UUID, allowing `blkid` and `udev` to identify it. To protect against bad sectors on Sector 0, a **Backup Data Signature Header** is written to the last sector of the data device.

To protect against metadata corruption, the metadata device maintains **Redundant Superblocks** at fixed offsets (e.g., primary at `0x1000`, with backups at block offsets 1024, 8192, and 32768).

### Metadata Sizing Ratio Derivation (1.0% – 1.5%)

The required metadata device capacity relative to data device capacity ($1.0\% \text{ to } 1.5\%$) is derived mathematically using exact block allocation calculations:

1. **Level-1 Checksum Table Footprint ($0.49\%$):**
   - Data Block Size: $4096 \text{ bytes}$ ($4\text{ KB}$).
   - Per-block checksum entry: $20 \text{ bytes}$ ($8\text{B xxHash3} + 8\text{B block flags} + 4\text{B allocation metadata}$).
   - Ratio: $\frac{20\text{ bytes}}{4096\text{ bytes}} = 0.0048828125 \approx \mathbf{0.49\%}$ ($0.4883\%$).

2. **Inode Table & Extent Indirection Footprint ($0.39\%$ to $0.78\%$):**
   - Struct inode size: $256 \text{ bytes}$.
   - Default allocation ratio (1 inode per 32KB to 64KB data):
     - At 64KB per inode: $\frac{256\text{B}}{65536\text{B}} = 0.00390625 \approx \mathbf{0.39\%}$ ($0.3906\%$).
     - At 32KB per inode: $\frac{256\text{B}}{32768\text{B}} = 0.0078125 \approx \mathbf{0.78\%}$ ($0.7813\%$).
     - *(Note: For dense small-file workloads at 16KB per inode, $\frac{256\text{B}}{16384\text{B}} = 0.015625 \approx 1.56\%$, which increases the total metadata requirement up to $\approx 2.28\%$).*

3. **Journal WAL, Bad Block Maps, Reservation Tables & Transaction Manifests ($0.12\%$ to $0.23\%$):**
   - Fixed ring buffers, bad sector tracking maps, transaction manifests, and pre-operation snapshots consume $\approx \mathbf{0.12\% \text{ to } 0.23\%}$.

$$\text{Total Metadata Ratio (Standard Workload)} = 0.4883\% + (0.3906\% \text{ to } 0.7813\%) + (0.1211\% \text{ to } 0.2304\%) = \mathbf{1.00\% \text{ to } 1.50\%}$$

---

## Core Data Structures

### Superblock (Meta Device, fixed offset 0x1000)

```
magic:              0x444E5046 ("DNPF")
version:            u32
uuid_meta:          uuid (16 bytes)
uuid_data:          uuid (16 bytes) — paired data device
block_size:         u32
total_data_blocks:  u64
total_inodes:       u64
free_data_blocks:   u64
free_inodes:        u64
root_inode:         u64
last_mount_time:    timestamp
last_write_time:    timestamp
journal_offset:     u64
checksum_table_offset: u64
bad_block_map_offset:  u64
reservation_table_offset: u64
superblock_checksum: u64  — xxhash3_64 of the superblock
```

DNPFS does not store a global dirty flag or active transaction counter in the superblock (which would bottleneck parallel commits and suffer from RAM-disk desync races). Instead, crash recovery is triggered dynamically on mount/load if the `/transactions/` directory on the metadata device contains any uncommitted `allocation_*.dry` manifests.

*Note on Growable Limits:* Like the inode table, the reservation table is **dynamically growable**. The `reservation_table_offset` points to a head block. As concurrent transactions increase, the driver allocates new metadata blocks dynamically (chained via `next_block` pointers), eliminating arbitrary bounds on transaction concurrency.

*Note on Out-Of-Metadata-Space Failure Mode:* Metadata structures allocate space dynamically within the fixed metadata SSD volume formatted during `dnpfs-format`. If an extreme small-file workload exhausts the metadata SSD space (`free_inodes` or free metadata blocks reach 0), Phase 1 (Planning) intercepts all new write/copy allocation requests and rejects them cleanly with `ENOSPC` (No space left on device). Existing files on the volume remain fully intact and readable.

### Inode

```
inode_id:           u64
file_size:          u64
block_count:        u64
extents:            [dnpfs_extent; 4]  — first 4 inline extents, 5th+ via indirect_extent_block
indirect_extent_block: u64 | null      — points to secondary extent block on META device (indirection)
created:            timestamp
modified:           timestamp
permissions:        u32
owner:              u32
flags:              u32                — bits: 0x1 = INODE_PENDING_COMMIT
fallback_path_offset: u64 | null        — offset to source path on META device (Live-Migration Symlink Fallback)
checksum:           u64  — xxhash3_64 of this inode structure
```

*Note on Inode Table Capacity:* The inode table on the metadata SSD is dynamically growable. Unlike ext2/ext3's fixed-size tables formatted at creation time, DNPFS allocates metadata blocks in chained blocks of 512 inodes each as the number of files grows, eliminating the classic "out of inodes" constraint entirely.

**dnpfs_extent structure:**
```
struct dnpfs_extent {
    u64 start_block;      // Offsets into DATA device
    u64 block_count;      // Length of contiguous block run
}
```

**Indirection Spillover & Chained Indirect Blocks:**
DNPFS inodes are fixed-size structures in the metadata device's inode table to enable $O(1)$ lookups. The array capacity for inline extents is capped at 4. If a file is highly fragmented and requires a 5th contiguous block run, the additional extents **spill over** into the 4 KB metadata block pointed to by `indirect_extent_block`.

To prevent overflow in pathologically fragmented files, indirect blocks are chained as a linked list:
* **Indirect Block Structure (4 KB):**
  * `extents: [dnpfs_extent; 255]` — up to 255 extents (4080 bytes)
  * `next_indirect_block: u64` — offset of the next indirect metadata block, or `null` (8 bytes)
  * `padding/reserved: u64` — 8 bytes aligned padding
* If the number of extents exceeds 259 (4 inline + 255 in the first indirect block), a new 4 KB indirect block is allocated on the metadata device and chained via the `next_indirect_block` pointer.
* **In-RAM Extent Index Cache ($O(\log E)$ Lookups):** On disk, indirect extent blocks use chained 4 KB blocks for format v1 simplicity. To prevent $O(N)$ traversal overhead for heavily fragmented COW files in memory, `dnpfs.ko` builds a compact in-RAM **Extent Index Cache** (a sorted red-black tree / dynamic array over file block offsets) when an inode is opened into the VFS inode cache. Reads and writes perform binary search ($O(\log E)$) over this in-RAM index cache rather than walking the on-disk linked list. A B-tree on-disk extent layout is planned for format v2.

---

### Block Checksum Table Entry

The checksum table is stored as a flat array on the metadata SSD, indexed directly by the data device's block number. This eliminates the need to store individual block offsets in each entry.

```
checksum:           u64   — 64-bit xxHash3 or CRC32C checksum (8 bytes)
last_verified:      timestamp (8 bytes)
status:             enum { good, suspect, bad, remapped } (4 bytes packed/padded)
```

### Bad Block Map Entry

```
data_block_offset:  u64
detected:           timestamp
detection_method:   enum { read_error, write_error, checksum_fail, smart_pending }
remap_target:       u64 | null
```

### Pending Reservation Entry

```
reservation_id:     uuid
manifest_path:      path to allocation.dry
blocks_held:        [u64]  — data device block addresses
created:            timestamp
operation_type:     enum { write, delete, copy, rename }
status:             enum { pending, committed, rolled_back, aborted }
```

---

## Checksum Strategy

DNPFS uses a two-level checksum architecture that balances precision with performance. Storing and verifying individual checksums for every block on every health check is expensive — especially for volumes with millions of small files. The solution is a Merkle-tree-inspired grouped checksum system.

### Level 1 — Individual Block Checksums

Every data block has a compact 64-bit xxHash3 (or CRC32C) checksum stored in the checksum table on the metadata device. This provides a low-overhead, precise, per-block verification footprint (exactly 20 bytes per block entry, equivalent to ~0.49% / 0.4883% of the data drive capacity), making it small enough to fit within SSD metadata devices. xxHash3 is used for both individual block validation and complete file-level verification, keeping the entire pipeline non-cryptographic and highly performant.

> [!NOTE]
> **Threat Model Assumption:** DNPFS assumes a non-adversarial threat model (protecting against silent bit rot, cosmic rays, and sector/firmware errors rather than malicious cryptographic tampering). Consequently, non-cryptographic xxHash3 is used exclusively; collision resistance against malicious forgery is an accepted trade-off and is out of scope.

```
Block 400000 → xxhash3: 0xA3F9B2C10D2E4A9F
Block 400001 → xxhash3: 0xC71D44E89B01F2D3
Block 400002 → xxhash3: 0x88EF01A9BC3D4E5F
```

Individual checksums are consulted during writes (read-back verification), during recovery (manifest cross-check), and when a group checksum fails (see below).

### Level 2 — Group Checksums

Blocks are organized into groups of N (default: 100, configurable). A single group checksum covers the combined state of all blocks in the group.

```
Group 0 (blocks 0–99):   xxhash3_64 of [checksum0 + checksum1 + ... + checksum99]
Group 1 (blocks 100–199): xxhash3_64 of [checksum100 + ... + checksum199]
```

**V1 Group Checksum Maintenance under COW, Deletions & Bad-Sector Remaps:**
In V1, whenever a Copy-On-Write write, file deletion, or Branch 1 bad-sector block reallocation (moving block X → Y during Phase 4 write verification) alters block allocations, the Level 2 group checksums for all affected 100-block groups (including both the source bad-sector group and destination reallocated group) are **synchronously updated in Phase 5 (Confirmation)**. Because Level 2 group hashes are calculated in RAM directly from active 64-bit Level 1 block checksums (without needing to re-read data blocks from the physical HDD), synchronous group checksum updates incur zero physical disk I/O overhead. This guarantees that on-disk Level 2 group checksums remain 100% consistent across all COW, deletion, and bad-sector remap operations in V1.

**Routine health check flow (Data Verification / Scrubbing):**

To verify data integrity without redundant metadata reads, the routine health check (scrubber) follows this flow:
```
1. Read G blocks from the data device.
2. Compute individual checksums in RAM.
3. Hash the in-RAM checksums to generate a candidate group checksum.
4. Read the stored group checksum from the metadata device.
5. Compare the candidate group checksum with the stored group checksum:
   → Match: All blocks in the group are healthy. Done. (Saves G-1 metadata reads)
   → Mismatch: Drop to individual checks:
       Read the G stored individual checksums from the metadata device.
       Compare each in-RAM computed checksum with its stored counterpart.
       → Mismatch: Flag bad sector(s) and trigger bad sector handling.
```

### Advanced Checksum Grouping (Planned for V2)

> [!NOTE]
> **V1 Implementation Constraint:** The following dynamic write-time checksum tiering and deferred checksumming features introduce significant state-management complexity for the background daemon. For the initial MVP, DNPFS will strictly use a flat, static 100-block group size for all writes, computing individual checksums immediately in Phase 4. These optimizations are deferred to V2.

<!--
**Write-Time Dynamic Checksum Grouping by File Size**

To optimize performance and metadata overhead during write operations (copy-paste), DNPFS dynamically scales checksum group sizes based on file sizes at write time:
* **File size < 10 KB:** 500 files per checksum group
* **File size < 100 KB:** 100 files per checksum group
* **File size < 1 MB:** 50 files per checksum group
* **File size < 100 MB:** 5 files per checksum group
* **File size < 500 MB:** 2 files per checksum group
* **File size ≥ 500 MB:** Individual checksumming (no grouping)

Once data is written to the drive and committed, block-level grouping remains the primary layout for background management.

**Deferred Checksumming (Write-Time Grouping)**
To achieve high write-performance, DNPFS implements **Deferred Checksumming** during copying/writing:
1. **At Write Time:** Rather than calculating and writing individual checksums for every single file or block, the driver groups them and computes only a single **group checksum** (hashing the combined data blocks). This group checksum is written immediately to the metadata device. No individual block checksums are written, reducing SSD wear and write overhead.
2. **At Idle Time:** Once the system is idle, `dnpfsd` runs a background task to compute the individual file/block checksums and save them to the metadata device.
3. **If a Mismatch Occurs before Idle Checksumming:** The filesystem falls back to the group-level verification and rolls back the entire group to its pre-operation snapshot. If it occurs after idle checksumming has run, the system uses the stored individual checksums to pinpoint the exact corrupted file.

**Composition (How Block-level and File-level Grouping Align):**
* **File-level grouping** is a logical transaction-bundling utility used strictly during Phase 1–3 of writes to group multiple small files into a single contiguous extent range allocation in the manifest, saving write-overhead.
* **Block-level grouping** (the static 100-block table) is the physical layout format on the metadata SSD. Once a file transaction is committed, its allocated blocks are registered within these static 100-block on-disk groups, which are subsequently managed and verified by the background scrubber.
* **Group Checksum Invalidation on Block Release:** When blocks are freed (e.g. during a Copy-On-Write update or file deletion), the affected 100-block group checksum on the SSD is marked as "stale/dirty" by setting its status flag to `suspect`. During the next background scrub or idle period, `dnpfsd` recomputes the group checksum from the remaining active block hashes.
-->

### Checksum Amortization for Small Files

Per-block checksums have a fixed storage overhead (20 bytes per 4KB block, or ~0.49%). For routine background verification (scrubbing), Level 2 group checksums amortize metadata read latency: the scrubber reads a 100-block span from the data device, computes the candidate group hash in RAM, and compares it against a single Level 2 group checksum on the metadata SSD. Individual Level 1 block checksums are evaluated only if a group checksum mismatch is detected.

*(Note: Inline storage for very small files < 4KB on the metadata SSD is planned for V2).*

### Group Checksum Data Structure

```
group_id:           u64
block_range_start:  u64
block_range_end:    u64
group_checksum:     u64  — xxhash3_64 of member block hashes
last_verified:      timestamp
member_count:       u32
```

### RAM Bit-Flip Protection (Double-Checksumming in Transit)

To protect against random DRAM corruption (e.g., from cosmic rays or faulty non-ECC memory modules), DNPFS implements double-checksum validation in transit:
* **Write Path:** When a page is dirty-marked in the kernel page cache by VFS, the driver immediately computes a temporary 64-bit checksum and stores it in the page's VFS private descriptor in RAM. During Phase 4 (Execution), right before sending the block to the disk controller, the driver recomputes the checksum in RAM and verifies it against the dirty-page descriptor. If a mismatch is detected, the transaction aborts, the page is discarded, a kernel warning is issued, and VFS is requested to rewrite the block from cache.
* **Long-Dwelling Dirty Pages:** No active in-RAM polling mechanism is required. The dirty-page checksum window (comparing initial VFS dirtying against pre-disk submission in Phase 4) intrinsically protects data regardless of how long dirty pages dwell in memory before being flushed by standard OS page cache flusher threads (`wb_workfn`).
* **Read Path (Optional):** By default, checksums are verified only when a block is first read from the physical HDD into the kernel page cache. Subsequent cache-hit reads bypass checksum re-evaluation to preserve memory throughput. For environments requiring extreme paranoia, a mount flag (`verify=paranoid_cache`) can be enabled to force the driver to recompute the xxHash3 checksum right before copying data from the page cache to the userspace buffer on *every* `read()` syscall. If a RAM bit-flip is detected, the page is discarded and reloaded from the HDD.
* **Hardware Recommendation:** While transit checksumming protects data under the filesystem's direct control, it cannot protect data once it is copied to the application's private memory. DNPFS strongly recommends **ECC (Error-Correcting Code) RAM** for production deployments.

---

## The Allocation Manifest

The `allocation.dry` file is the cornerstone of DNPFS crash recovery and write verification. It is generated during the dry run phase of every write or delete operation and committed to the meta device before any actual operation begins.

*Note on Binary vs. YAML Representation:* On disk and in kernel memory, `allocation_*.dry` files are serialized as compact C binary structures (`struct dnpfs_allocation_manifest`) by `dnpfs.ko` to maximize hot-path write throughput without kernel-level text parsing overhead. Userspace tools (`dnpfsd`, `dnpfs-dry`) parse these binary manifests and render them as human-readable YAML for administrative inspection.

### Structure (YAML Representation)

```yaml
manifest_version: 1
manifest_id: uuid
operation_type: write | delete | copy
created: timestamp
reservation_id: uuid

source:
  path: /original/file/path
  size_bytes: 1073741824
  checksum_xxhash3: 0xa3f9b2c1...
  cascade_delete_on_confirm: true  # if true, acts as a move/migration (deletes source after verify)
  expected_mtime: 1783492800        # used for source mutation verification
  expected_inode: 289410            # used for source mutation verification

destination:
  device_uuid: <data device uuid>
  extent_map:
    - index: 0
      start_sector: 204800
      block_count: 256  # contiguous block range (extent)
      group_checksum: a1b2c3d4...  # combined checksum for the extent
      status: reserved
    - index: 1
      start_sector: 206000
      block_count: 512
      group_checksum: e5f6a7b8...
      status: reserved

pre_operation_snapshot:
  inode_state: <serialized inode or null if new file>
  affected_directory_entries: [...]

verification:
  bad_sectors_in_range: []
  estimated_write_time_ms: 4320
  space_available: true
  meta_device_healthy: true
  data_device_healthy: true

confirmation:
  status: pending | confirmed | failed | rolled_back
  confirmed_at: null
  blocks_written: 0
  blocks_verified: 0
```

### Lifecycle

```
1. Operation requested
2. Dry run: simulate entire operation
3. Scan target block range for bad sectors
4. Write allocation.dry to meta device
5. Flush meta device (FUA if supported)
6. Copy allocation.dry to RAM
7. Reserve all target blocks (TRIM-immune)
8. Perform actual operation
9. After each block write: verify checksum against manifest
10. On full success: update manifest status to confirmed
11. Update inode table and directory entries
12. Update checksum table
13. Delete manifest from /transactions/ directory
14. Release reservations
15. Optionally trigger meta backup if threshold exceeded
```

### Indefinite Kernel Reservations & Process Abort Handling

Unlike userspace leases, reservations in DNPFS are held strictly in RAM by the kernel's transaction coordinator. They **do not expire based on wall-clock time**, completely eliminating mid-runtime double-allocation races.

**Process Termination & Abort Handling:**
* **Active Writes:** For long-running writes, the kernel driver maintains the reservation in RAM until the write physically completes and commits.
* **Process Kills / Crashes (`SIGKILL`, OOM, abnormal exit):** If the writing process is killed or terminates prematurely while the OS kernel remains running, Linux VFS triggers standard file handle release callbacks (`f_op->release` / `f_op->flush`). The `dnpfs.ko` driver intercepts this callback, issues an **automatic transaction abort signal**, releases all held RAM reservations back to the free block bitmap, and deletes the pending manifest from `/transactions/`.
* **Kernel Worker Deadlock / System Crash:** If an unrecoverable kernel thread deadlock or full OS crash occurs, the reservation remains held until the system reboots, at which point the boot-time recovery loop scans `/transactions/` and safely rolls back the interrupted transaction.
* **Uninterruptible D-State Processes & Admin Override:** If a writing process gets stuck in an uninterruptible sleep state (D-state) due to physical I/O errors on a dying data HDD (preventing standard release callbacks from firing), system administrators can forcefully abort the transaction using the `dnpfs-dry --force-abort <transaction_id>` utility. This issues an explicit driver IOCTL (`DNPFS_IOC_ABORT_TRANSACTION`, requiring `CAP_SYS_ADMIN`), which forcefully revokes held RAM reservations, purges the manifest from `/transactions/`, and wakes up any blocked threads with `EINTR`.

---

## Transaction Lifecycle

Every operation — write, delete, copy, rename — follows the same unified transaction model. Direct cross-device `move` operations are banned (see below).

```
PHASE 1: PLANNING
  Identify operation type
  Compute block requirements
  Check space, device health, bad block map
  → Reject early if any precondition fails

PHASE 2: DRY RUN
  Simulate full operation
  Assign specific block addresses
  Compute expected checksums for all blocks
  Scan target range for bad/suspect sectors
  Generate allocation.dry manifest

PHASE 3: RESERVATION
  Write allocation.dry to meta device
  Flush to physical medium (FUA or write cache disabled)
  Copy allocation.dry to RAM
  Mark all target blocks as reserved
  Suppress TRIM on reserved blocks

PHASE 4: EXECUTION
  Write data blocks to data device first
  After each block: read back and verify checksum (or dirty-page verify for RAM safety)
  On checksum mismatch: flag bad sector, find alternative, retry
  Write metadata second (inode updates, directory entries)
  Flush meta device

PHASE 5: CONFIRMATION
  Cross-reference written blocks against allocation.dry
  Update block checksum table
  Update bad block map if any issues detected
  Update manifest status to confirmed
  Write complete.dry summary
  Delete allocation.dry from metadata /transactions/ directory
  Release reservations
  Release TRIM suppression

PHASE 6: OPTIONAL BACKUP TRIGGER
  If bytes_written > 100MB OR files_written > 100:
    Trigger incremental meta device backup
    Update backup sequence number
```

Delete operations follow the same flow but in reverse — reservation holds the blocks being freed, metadata is updated first to remove pointers, data blocks are released last, then TRIM is issued only after confirmation.

### Random Writes and Truncations (Copy-On-Write)

To achieve VFS compliance and prevent data corruption, DNPFS does not support in-place block overwrites on the primary data device. All random writes (`pwrite`), file truncations (`ftruncate`), and in-place modifications are implemented using **Copy-On-Write (COW)**:
* **Random Writes:** When writing to an arbitrary offset in an existing file, the driver allocates a new extent range for the modified blocks, writes the updated data, verifies its checksum, and then updates the inode's extent mapping table to point to the new blocks. The old blocks are subsequently freed.
* **Truncations:** For file shrinking, the driver updates the file size in the inode and marks the unmapped trailing extents as free in the allocation bitmap. For file expansion, it allocates a zero-filled extent or updates the file size metadata (supporting sparse files).

### Safe Move Operations (Copy-Verify-Delete)

To eliminate data loss risks from partial or interrupted moves, direct metadata-only "move" operations across devices are banned. Move operations are implemented explicitly as a three-phase transaction:
1. **Copy Phase:** All target data blocks are sequentially copied to the DNPFS device using standard write reservations.
2. **Verification Phase:** The driver performs block checksum comparisons (xxHash3/CRC32C) and file-level verification (xxHash3) between the source and destination.
3. **Delete Phase:** Only after verification successfully confirms data identity, the driver issues a secure delete operation to the original source file.

> [!IMPORTANT]
> **Contributor Invariant:** The Delete Phase **must never** execute in parallel with the Copy Phase or before Verification completes. Any attempts to "optimize" this transaction via parallel copying and deleting are strictly prohibited. This is a core architectural decision for data safety and must not be modified.

### Live-Migration Symlink Fallback (Planned for V2)

> [!NOTE]
> **V1 Implementation Constraint:** The Live-Migration Fallback feature involves complex cross-process namespace boundary crossing, kernel pointer lifecycles, and TOCTOU mitigations. To minimize kernel security risks (CVEs) and scope creep for the initial MVP, this feature is strictly deferred to V2. 
> 
> **For V1:** Any `read()` or `open()` attempt on a pending inode simply blocks (if `O_NONBLOCK` is unset) or returns `EBUSY` until the transaction completes.

<!--
To prevent active readers from experiencing read blocks or downtime during a slow copy/move operation, DNPFS implements a metadata-level **Live-Migration Symlink Fallback** redirection:
* **Pending Commit Flag:** When a write/copy begins, the inode is created immediately on the metadata device with the `INODE_PENDING_COMMIT` flag set in its `flags` field.
* **Encapsulated Redirection:** The `fallback_path_offset` field in the inode points to the file path of the original source file on the host OS. This path is stored internally in the metadata table and is **never** exposed to userspace as a literal symlink (e.g., `readlink()` and `ls -l` will report the file as a normal regular file with its eventual size). To bypass mount namespace visibility issues (such as containers), the driver opens the source file once during Phase 1 (Planning) using the initiator's namespace/credentials, obtaining an active kernel `struct file *` reference stored in RAM. The fallback read path directly invokes `kernel_read` on this file reference. To ensure kernel safety, this `struct file *` reference is owned and managed strictly by the global driver superblock context in RAM, not the initiating process. If the initiating process terminates, the file reference is kept alive by the global driver coordinator and is only released (`fput`) when the copy transaction either commits or aborts.
* **TOCTOU Permission Validation:** To prevent Time-Of-Check to Time-Of-Use privilege escalation via the fallback handle, the driver **does not** bypass credentials during reads. Every time a process invokes a `read()` on the pending inode, the driver calls the standard `inode_permission(pending_destination_inode, MAY_READ)` helper to validate the *current calling process's credentials* against the **destination file's own requested permissions** (not the stale permissions of the original source file). If the caller lacks read access, the syscall returns `EACCES` or `EPERM`. Only after the check succeeds does the driver delegate the read to the internal `struct file *` reference.
* **Source Mutation Protection:** During Phase 1 (Planning), the driver records the source file's `mtime`, `size`, and `inode_id` in the `allocation_<write_id>.dry` manifest. Every intercepted read request validates that the source file's current attributes match these records. If a mismatch is detected:
  * The read fails with `ESTALE` (Stale file handle) or `EIO`.
  * The copy transaction is aborted and rolled back.
* **Crash Recovery Persistence:** If a system crash occurs mid-copy, the boot-time recovery manager scans for pending manifests and re-establishes the temporary in-RAM redirection tables, ensuring data availability until the transaction is either rolled back or completed.
* **Syscall Interception:**
  * **Pending with Fallback Source:**
    * `read()` / `open(O_RDONLY)`: The VFS driver intercepts read requests to the pending inode and transparently redirects them to read from the original source file.
    * `write()` / `open(O_WRONLY)`: Any write operations requested by other processes return `EBUSY`.
  * **Pending without Fallback Source (New File Writes):**
    * `read()` / `open(O_RDONLY)`: In blocking mode (default), the read call blocks (putting the thread to sleep in a commit wait-queue) until the transaction completes and the flag is cleared. In non-blocking mode (`O_NONBLOCK`), the call returns `EAGAIN` or `EBUSY` immediately.
    * `write()` / `open(O_WRONLY)`: The initial write worker executes the write; concurrent write requests from other processes return `EBUSY`.
  * `rename()`: **Allowed.** Modifying directory names updates only the SSD directory pointer to the inode's fixed `inode_id`. Write transactions are tracked by `inode_id`, keeping block copies completely unaffected by path renames.
  * `unlink()`: **Allowed (Ordered Abort Sequence):**
    1. **Untrack:** The driver immediately removes the file's directory entry on the SSD, hiding it from userspace (preventing any *new* handles from opening).
    2. **Abort Signal:** Sets an `ABORT_PENDING` flag on the active transaction.
    3. **Worker Stop:** The write worker checks the flag at its next block write-and-verify boundary, stops sequential writing, and cleans up its VFS structures. (Allowing the worker to finish its current block write is a deliberate design choice to prevent torn writes at the disk layer).
    4. **Release Reservations:** Once the worker has safely terminated, the driver releases the blocks back to the free block bitmap, preventing block reuse concurrency races.
    5. **Reader Wakeup:** Wakes any readers sleeping on the transaction's commit wait-queue, immediately returning `ENOENT`.
    6. **POSIX-compliant In-Flight Reads:** If a reader is actively streaming via Live-Migration fallback when `unlink()` occurs, the driver keeps the internal source file descriptor open and continues serving reads from the original source file until the user closes the local file handle.
* **Atomic Promotion:** Once the copy completes and passes checksum validation, the driver atomically clears the `INODE_PENDING_COMMIT` flag and the `fallback_path_offset` field, promoting the file to a standard local DNPFS inode. If a move was requested, the source file deletion is then safely triggered.
-->

---

## Driver Architecture

DNPFS consists of three components:

### 1. Kernel Module (`dnpfs.ko`)

Implements the VFS interface for Linux. Handles:

- Mount / unmount with UUID-based device pairing verification
- Block allocation and inode management
- Write ordering enforcement between meta and data devices
- TRIM suppression for reserved and in-flight blocks
- FUA enforcement on critical writes
- Device health monitoring (detects offline events mid-operation)
- **Accidental Format Protection:** Rather than dynamically toggling read-only states mid-operation (which risks race conditions), DNPFS relies on native kernel exclusive-open claims. When DNPFS mounts the pairing, the driver acquires an exclusive lock on the data block device using the kernel's `bd_holder` claim API (via `blkdev_get_by_path` or `blkdev_get_by_dev`), causing other partitioners and formatters (`mkfs`, `fdisk`) to immediately fail with a device-busy error. This is complemented by standard `udev` rules matching the DNPFS Data Signature Header to warn users of active pairing.

**On device offline detection:**

```
If meta device goes offline mid-write:
  → Halt all I/O to data device immediately
  → RAM copy of allocation.dry is still valid
  → Present user choice:
      A) Wait for meta device to reconnect
      B) Complete operation using RAM allocation.dry
         (requires meta device backup to be available)
      C) Halt and preserve current state

If data device goes offline mid-write:
  → Halt all I/O
  → Roll back meta device to pre-operation snapshot
  → Mark operation as failed in manifest
  → Release reservations
```

**On power loss detection (boot-time):**

```
Scan metadata device /transactions/ directory for uncommitted manifests (allocation_*.dry)
If manifests exist:
  For each pending manifest:
    Read manifest
    For each block in destination extent_map:
      Read block from data device
      Compute xxHash3 checksum in RAM
      Compare computed checksum with expected checksum stored in manifest
      → Match: mark block as committed
      → Mismatch: discard block
    → If all blocks in manifest are committed: confirm transaction, promote inode
    → If any block failed/partially written: roll back meta to pre-op snapshot, discard transaction
    Delete manifest from /transactions/ directory
```

*Note on Recovery Idempotency:* Phase 5 (Confirmation) and boot-time recovery are fully **idempotent**. If a crash occurs *after* a transaction successfully commits data to disk but *before* its manifest is deleted from `/transactions/`, the recovery scan will safely re-hash the blocks, confirm they all match the expected checksums, silently re-promote the inode, and delete the manifest with zero risk of data corruption.

### 2. Userspace Daemon (`dnpfsd`)

Runs as a background service. Handles:

- S.M.A.R.T. polling on both devices (configurable interval, default 1 hour)
- Orphaned transaction monitoring and health logging
- Incremental meta device backups
- Idle-time defragmentation scheduling on meta device
- Bad sector escalation alerts
- **Background Idle Checksumming & Verification:** When the system is idle:
  * **Phase 1 (Generation):** It checksums all files starting from the largest to the smallest (for fully written, non-partial files) and saves the results to the metadata device.
  * **Phase 2 (Scrubbing/Cross-Checking):** Once all files are checksummed, it cross-checks files against stored checksums to verify integrity.
  * *Yielding:* If a user read or write operation is requested, `dnpfsd` immediately pauses its background task and resumes only after the drive becomes idle again.

### 3. Userspace Tools (`dnpfs-tools`)

- `dnpfs-format` — formats both devices as a paired DNPFS volume
- `dnpfs-check` — filesystem check tool, understands two-device layout
- `dnpfs-recover` — forensic recovery tool, imports allocation.dry for partial write analysis
- `dnpfs-backup` — manual meta device backup and restore
- `dnpfs-smart` — S.M.A.R.T. status report for both devices
- `dnpfs-dry` — inspect, confirm, or forcefully abort pending dry run manifests (via DNPFS_IOC_ABORT_TRANSACTION)

---

## Cache Coherency and Write Ordering

This is one of the hardest implementation challenges. Two physically separate devices have two separate I/O queues, two separate drive caches, and no native cross-device atomic operation.

### The Problem

Drive firmware can confirm a write while data is still in the drive's internal volatile cache. If power dies after a fake confirmation, the write is lost with no signal to the OS.

### The Solution

**Force Unit Access (FUA):** For all critical transaction logs (WAL head pointers, superblock metadata updates, and `allocation.dry` state commits), the driver issues writes with the FUA flag set. This instructs the drive to flush its internal cache to non-volatile storage before confirming. Drives that do not support FUA will have write caching disabled in the driver.

**Explicit write ordering (Split-Path Design):**
To reconcile transactional safety with wear mitigation, DNPFS separates log writes from actual structural updates:
* **The Log Path (Synchronous WAL):** The planning manifest (`allocation.dry`) and confirmation markers are written sequentially to the Write-Ahead Log (WAL) on the metadata SSD using synchronous FUA flushes.
* **Single-Threaded WAL Pipelining:** While Group Commits coalesce concurrent multi-threaded writes, single-threaded sequential write streams would otherwise eat 2 FUA flushes per write call. To mitigate this latency for single-threaded bulk writes, `dnpfs.ko` implements **Pipelined WAL Batching**: consecutive Phase 3 dry-run manifests within a single open file stream are batched into a single WAL FUA write, reducing synchronization overhead to 1 FUA flush per batch during active sequential streams.
* **The Structure Path (Asynchronous Checkpoints):** Structural filesystem updates (in-place inode tables, free block bitmaps, and checksum tables) are updated in RAM first. These are lazily written to the metadata device asynchronously during idle periods (checkpointing), amortizing flash wear.

The detailed transactional sequence is:
1. Write the transaction planning manifest to the WAL on the metadata device → issue FUA flush → wait for hardware confirmation.
2. Only after metadata WAL confirms → write the data blocks sequentially to the data device.
3. Read back written data blocks from the data device → verify checksums (optional/configurable; see below).
4. Write the confirmation marker to the metadata WAL → issue FUA flush → wait for confirmation.
5. Inodes and block bitmaps are updated in RAM, and checkpointed to their permanent locations on the SSD asynchronously.

This guarantees that the logged intent on the metadata device always leads the data device. If a crash occurs before Step 4, the metadata checkpoint is stale, but the synchronous WAL manifest allows boot recovery to replay or rollback the state perfectly.

**RAM buffer:** `allocation.dry` in RAM provides a temporary bridge during short metadata device disconnections. The RAM copy is considered authoritative only until the metadata device reconnects and is verified. It is never written to the data device before metadata WAL confirmation.

**Power Loss Protection (PLP) Recommendation:** DNPFS strongly recommends the use of SSDs equipped with physical **Power Loss Protection (PLP)** capacitors (e.g., enterprise/industrial grade SSDs). On PLP-equipped SSDs, FUA commands complete near-instantly with zero physical flash cycle write penalty, as the drive controller can safely guarantee writes cached in volatile controllers.

---

## Power Loss and Crash Recovery

### Detection

Crash recovery is triggered on mount if the `/transactions/` directory on the metadata device contains any uncommitted `allocation_*.dry` manifests.

### Recovery Procedure (Concurrent Multi-Transaction Loop)

```
Boot
→ Read superblock from meta device
→ Scan metadata device /transactions/ directory for uncommitted allocation_*.dry manifests
→ Uncommitted manifests exist?
  → Yes: enter recovery mode
    → For EACH uncommitted manifest (allocation_<write_id>.dry):
        → Read manifest from /transactions/
        → For each block in manifest's extent map:
            Read block from data device
            Compute xxHash3 checksum in RAM
            Compare to expected_checksum in manifest
            → Match: block was written successfully
            → No match or read error: block was not written or is corrupt
        → Determine operation completeness:
            All blocks match → confirm operation, promote inode to committed
            Partial or no match → roll back metadata to pre_operation_snapshot, discard transaction
        → Delete manifest file (allocation_<write_id>.dry) from /transactions/
  → No: clean mount, proceed normally
```

### Pre-operation Snapshots

Before every transaction, the current state of affected inodes and directory entries is serialized into the allocation.dry manifest as `pre_operation_snapshot`. This is the rollback target. It is small (a few KB at most) and does not require snapshotting the data device.

---

## Bad Sector Tracking and Silent Failure Detection

### Per-Block Checksums

Every data block written to the data device has a corresponding xxHash3/CRC32C checksum stored in the checksum table on the meta device. This enables detection of silent write failures — cases where the drive confirms a write but writes garbage.

**On write:** checksum is computed before write, stored in manifest, verified by read-back after write, then stored permanently in the checksum table.

**On read:** checksum is recomputed from the block content and compared to the stored checksum. A mismatch triggers bad sector handling.

### Bad Sector Handling

DNPFS strictly distinguishes between **Write-Time Faults** (recoverable via active RAM buffers) and **Read-Time At-Rest Faults** (unrecoverable silent corruption on disk):

**Branch 1 — Write-Time Fault (Verification Mismatch during Write):**
```
Checksum mismatch detected during Phase 4 read-back verify:
  → Source buffer is still present in RAM
  → Add sector/block X to bad_block_map on metadata device (status=bad)
  → Allocate clean alternative block Y (not in bad_block_map, not reserved)
  → Write RAM buffer to block Y
  → Verify block Y xxHash3 checksum
  → Update destination extent map in allocation.dry manifest from X to Y
  → Log bad sector event with timestamp
  → Escalate warning if S.M.A.R.T. pending sector count is rising
```

**Branch 2 — Read-Time Fault (At-Rest Silent Corruption / Bit Rot):**
```
Checksum mismatch detected during file read or background scrubbing:
  → Source buffer is NOT in RAM; data on disk is corrupted
  → Add sector/block X to bad_block_map on metadata device (status=bad)
  → Mark affected extent/inode as DAMAGED in metadata
  → Return EIO (I/O Error) to calling process
  → Log critical bit-rot alert with filename, offset, and block ID
  → Note: Automatic recovery is impossible without an external backup or secondary mirror
```

**Bad Block Map Sizing & Thresholds:**
Each entry in `bad_block_map` is a compact 16-byte record (`sector_offset` u64 + `flags` u64). Even on a severely degraded HDD with 10,000 bad sectors, the total map consumes only ~160 KB of metadata space. If bad sector accumulation exceeds a configurable threshold (default: 5,000 bad sectors), `dnpfsd` issues a critical drive health warning advising immediate hardware replacement. Exceeding this threshold is strictly advisory; by design, DNPFS does not enforce a hard mount or write block, continuing to permanently record and bypass all defective sectors without evicting records for the lifetime of the paired volume.

### allocation.dry Integration

During dry run, the target block range is cross-referenced against the bad_block_map. Any block in the range that is flagged as bad or suspect is avoided in the allocation plan — the dry run finds clean blocks only. This means bad sectors are handled before the write attempt, not during.

---

## S.M.A.R.T. Integration

The userspace daemon polls both devices using S.M.A.R.T. and caches the results on the meta device. The following attributes are monitored:

| Attribute | Quantitative Condition | Action on threshold |
|---|---|---|
| Reallocated Sector Count rising | Trend: $\Delta > 5$ remapped sectors/hour | Warn user, lower backup trigger thresholds |
| Pending Sector Count > 0 | Absolute: $> 0$ pending sectors | Flag affected blocks in bad_block_map as suspect |
| Uncorrectable Error Count > 0 | Absolute: $> 0$ uncorrectable errors | Urgent warning, recommend immediate backup |
| Reallocated Sector Count high | Absolute: $> 100$ total remapped sectors | Critical warning, data device near end of life |
| SSD Wear Leveling Count low | Absolute: $< 10\%$ remaining life | Warning, meta device approaching end of life |

S.M.A.R.T. data is stored locally on the meta device with timestamps to allow trend analysis — a slowly rising remap count is more dangerous than a stable high count.

---

## Metadata Backup System

The meta device is small, critical, and fully self-contained. It can be imaged completely and restored to a new device. DNPFS supports three backup targets:

- **Local backup** — image stored on the user's main OS partition (small, fast, always available)
- **External backup** — image stored on a separate drive or USB
- **Remote backup** — image uploaded to a backup server or cloud storage (opt-in, user-configured)

### Backup Trigger Conditions

Backup is triggered automatically when any of the following occur:

- A write or delete operation exceeds 100MB total
- A write or delete operation touches more than 100 files
- S.M.A.R.T. reports any degradation on the meta device
- User manually requests backup
- Configurable time interval (default: daily)

### Backup Format

```
dnpfs-meta-backup-{sequence}-{timestamp}.img
```

The image is a complete sector-for-sector copy of the meta device, compressed and checksummed. At least 3 sequential backups are retained. The sequence number allows identification of the most recent consistent backup.

### Restore Procedure

```
dnpfs-backup --restore backup.img --target /dev/sdX
→ Verify backup checksum
→ Write image to new meta device
→ Update UUID pairing if device UUID changed
→ Run dnpfs-check to verify consistency with data device
```

### Temporary Local Safety Copy

A compressed meta image is maintained on the OS partition. It is updated on every backup trigger. It is automatically deleted if both the meta device and data device are simultaneously disconnected — this prevents a stale image from being mistakenly used as authoritative after the volume is moved.

---

## Encryption

DNPFS does not implement its own cryptography. Encryption is provided by **dm-crypt/LUKS**, the standard Linux disk encryption layer, applied beneath DNPFS at the block device level. This is a deliberate design choice — cryptography is notoriously easy to get subtly wrong, and LUKS is battle-tested, audited, and widely understood.

DNPFS sits on top of LUKS volumes. The OS presents decrypted block devices to the DNPFS driver, which operates normally regardless of whether encryption is active underneath.

### Dual-Mode LUKS Chaining (Planned for V2)
 
> [!NOTE]
> **V1 Implementation Constraint:** The following multi-mode encryption and key-chaining logic requires custom userspace unlocking hooks that are too complex for an MVP. For V1, DNPFS simply expects the underlying block devices to be already decrypted by the OS (via standard `cryptsetup`) before the filesystem is mounted. The native driver will not handle LUKS key derivations internally.

<!--
### Four Encryption Modes

Users select an encryption mode during `dnpfs-format`. The mode is stored in the superblock and enforced at mount time.

**Mode 1 — Encrypt metadata device only** *(recommended default)*

The meta device is a LUKS volume. The data device is unencrypted.

Without the meta device passphrase, the data device is structurally inaccessible — no pointers, no inode table, no directory tree. This is not cryptographic protection of the data blocks themselves, but it is a meaningful access barrier with minimal performance overhead.

This mode is recommended because: metadata I/O is already fast on SSD, LUKS overhead on a 16GB device is negligible, and filenames, directory structure, and access patterns are all hidden.

**Mode 2 — Encrypt data device only**

The data device is a LUKS volume. The meta device is unencrypted.

Data blocks are cryptographically protected but directory structure, filenames, file sizes, and access patterns are fully visible to anyone with the meta device. This is a weaker posture than Mode 1 in most threat models. Available as a user choice but not recommended.

**Mode 3 — Encrypt both devices** *(maximum security)*

Both devices are LUKS volumes with a single user passphrase. The passphrase derives two independent keys via separate KDF paths — one for each device.

The unlock flow is seamless:

```
User enters passphrase once
→ Derive key A → unlock LUKS on meta device
→ Read data device key material from meta device
→ Derive key B → unlock LUKS on data device
→ DNPFS mounts normally
```

Single unlock, double protection, no extra user friction. Recommended for users with strong security requirements.

**Mode 4 — No encryption**

Both devices are unencrypted. The structural separation of DNPFS (data unreadable without meta device) provides inconvenience, not security. Users who choose this mode are shown a clear advisory during format.

### Encryption Mode in Superblock

```
encryption_mode:    u8  { 0=none, 1=meta_only, 2=data_only, 3=both }
luks_uuid_meta:     uuid | null
luks_uuid_data:     uuid | null
```

### Key Storage in Mode 3

The data device LUKS key material is stored encrypted on the meta device, protected by the same passphrase. If the meta device is lost, the data device key is also lost — this is by design and is consistent with the principle that the meta device is the authoritative root of the volume.
-->

---

## RAID Compatibility

DNPFS does not implement RAID natively. RAID is handled at the block device layer beneath DNPFS, using Linux's standard `mdadm` or `dm-raid` tools. DNPFS operates identically whether it sits on a single physical drive or an mdadm array — the OS presents the array as a single block device and DNPFS does not need to know the difference.

### Setup Order

Always configure RAID first, then run `dnpfs-format` on the resulting `/dev/mdX` devices. Never run `dnpfs-format` on individual RAID member drives.

```bash
# Example: RAID 1 on metadata device (two SSDs)
mdadm --create /dev/md0 --level=1 --raid-devices=2 /dev/sdb /dev/sdc

# Example: RAID 5 on data device (three HDDs)
mdadm --create /dev/md1 --level=5 --raid-devices=3 /dev/sdd /dev/sde /dev/sdf

# Format DNPFS on the arrays
dnpfs-format --meta /dev/md0 --data /dev/md1
```

### Recommended RAID Configurations

| Component | Recommended RAID | Reason |
|---|---|---|
| Meta device | RAID 1 (mirror) | Meta device is single point of failure; mirroring eliminates it |
| Data device | RAID 1, 5, or 6 | User preference; RAID 6 for large arrays where double failure during rebuild is a real risk |
| Meta device | No RAID (+ backup system) | Acceptable if backup system is configured and tested |

**RAID 1 on the metadata device is the strongest single reliability improvement a user can make.** The backup system already protects against metadata loss, but RAID 1 eliminates the recovery step entirely — the mirror takes over instantly with no downtime and no restore procedure.

### TRIM on RAID Arrays

If the data device is a RAID array of SSDs, TRIM passthrough must be enabled in mdadm. DNPFS's TRIM suppression logic works identically regardless — it issues TRIM to the block device presented by mdadm, which handles passthrough to member drives transparently.

### UUID Handling

mdadm arrays have their own UUIDs. DNPFS uses the UUID of the block device presented at format time — whether that is a physical drive or an mdadm array UUID. No conflicts arise, but UUID pairing must be re-verified if the mdadm array is rebuilt or its UUID changes.

---

## TRIM/Discard Coordination

TRIM tells SSDs that blocks are no longer in use and can be erased internally. Issuing TRIM at the wrong time is a data integrity risk.

### TRIM Suppression Rules

- All blocks listed in a pending reservation are **TRIM-immune** until the reservation is confirmed or rolled back
- All blocks involved in an active transaction are TRIM-immune during the transaction
- Blocks are only eligible for TRIM after they are confirmed free in both the allocation bitmap and the checksum table
- The driver maintains an explicit TRIM suppression list in RAM, updated as reservations are created and released

### TRIM Issuance

For deleted blocks, TRIM is issued as the final step of the delete transaction — after metadata is updated, after the manifest is confirmed, after reservations are released. Never before.

For the meta device, TRIM is only issued for metadata blocks after the corresponding data operation is fully confirmed on both devices.

---

## Concurrent Operation Handling

Multiple processes may attempt operations simultaneously. DNPFS handles this with an ordered reservation-based locking model.

### Rules

- **Parallel Execution:** Transactions that target disjoint (non-overlapping) block ranges and modify non-conflicting directory paths are executed and written in parallel.
- **Group Commits (WAL Coalescing):** Rather than performing a synchronous FUA flush on the metadata SSD for every single transaction (which serializes throughput), `dnpfs.ko` coalesces concurrent metadata commits into a single physical FUA write. Multiple active writes write their metadata blocks to the in-memory log, and a single coordinator thread flushes the WAL head in one operation.
- **Multiple Allocation Manifests:** Rather than a single global file, each active transaction utilizes a dedicated `allocation_<write_id>.dry` manifest. This reduces write contention and allows independent operations to proceed concurrently.
- Two operations may not hold overlapping block reservations.
- Reservations are granted in order of request.
- A new dry run that would require blocks already reserved must wait or fail with a retry signal.
- Directory entry locks are held for the duration of any operation that modifies directory structure.
- Read operations do not require reservations but do check the TRIM suppression list:
  - **For Copy-On-Write Overwrites:** A read of a reserved block returns the pre-reservation data (reads the original, unmodified on-disk block state while the write proceeds).
  - **For Brand-New File Writes:** The destination blocks have no pre-existing data; read requests to the pending inode follow the `INODE_PENDING_COMMIT` rule (blocking on the commit wait-queue or returning `EBUSY` if `O_NONBLOCK` is set).

### Queue & Coordination

The reservation table on the meta device serves as the coordination point. All operations write their individual `allocation_<write_id>.dry` reservation before execution. Conflicts are detected at reservation time, not at execution time — this prevents silent overwrites. Non-conflicting transactions bypass the FIFO queue and execute in parallel.

---

## Defragmentation Strategy

The meta device is an SSD or flash storage. Logical fragmentation on SSDs does not cause seek penalty, but excessive fragmentation can still increase read amplification and lookup times.

### Policy

- Defragmentation is triggered only during idle periods (no active I/O on either device for a configurable idle threshold, default: 5 minutes)
- Defragmentation is limited to the meta device — the data device is treated as a large sequential store and is not defragmented
- If defragmentation is in progress and a new operation arrives, defragmentation pauses immediately, the operation proceeds normally, and defragmentation resumes afterward
- A brief user-visible notification is shown if defragmentation cannot pause fast enough (expected duration: seconds on a 16GB device)

---

## Recovery Tooling

Because DNPFS uses a non-standard two-device layout, no existing recovery tool understands its structure. The following tools are included:

### dnpfs-check

Two-device filesystem check. Unlike standard `fsck`, it cross-references both devices:

```
1. Read superblock from meta device
2. Verify superblock checksum
3. Scan inode table: for each inode, verify all block pointers are valid
4. For each block pointer: verify corresponding checksum exists in checksum table
5. Optionally: read and verify each block against stored checksum (full scan mode)
6. Report inconsistencies with suggested remediation
```

### dnpfs-recover

Forensic recovery tool for partial writes and corruption:

```
dnpfs-recover --manifest allocation.dry --data-device /dev/sdX

1. Parse allocation.dry
2. For each block in manifest:
   Read from data device
   Compute checksum
   Compare to expected_checksum
   Report: written / not written / corrupt
3. Produce recovery report:
   Total blocks planned: N
   Blocks successfully written: M
   Blocks missing: N-M
   Recommended action: rollback / partial accept / manual review
```

This tool can also accept a meta device image in place of a live mount, enabling offline forensic analysis.

### dnpfs-import

For cases where the meta device is destroyed but a backup exists:

```
dnpfs-import --backup meta-backup.img --data-device /dev/sdX

1. Restore meta backup to new meta device
2. Run consistency check
3. Report what data is accessible
4. Report any blocks on data device not referenced by restored metadata
```

---

## Known Limitations

These are real constraints users should understand before adopting DNPFS. They are accepted tradeoffs, not bugs.

**Not portable.** DNPFS requires the kernel module installed on any machine that mounts the volume. FAT32 is understood by every OS on earth. DNPFS requires a custom driver. Until it is merged into the Linux kernel mainline and recognized by standard tools, it is a manual install on every machine.

**Not bootable.** By design. DNPFS is a storage filesystem. The OS cannot boot from it because the kernel module is not loaded at boot time. This is a conscious tradeoff accepted at the design stage.

**Two points of failure instead of one.** A standard filesystem requires one healthy drive. DNPFS requires two healthy drives (or a backup restore procedure for the meta device). The backup system and optional RAID 1 on the meta device mitigate this, but the architectural reality remains.

**Meta device is the critical dependency.** If the meta device fails without a backup, the data device becomes a pile of unaddressed raw blocks. The system is designed with this asymmetry in mind — the meta device receives paranoid protection precisely because of this.

**Small file overhead.** Per-block checksums, inodes, reservations, and manifest entries have a fixed cost per file regardless of file size. For volumes with millions of tiny files, meta device space usage is higher than a traditional filesystem. The grouped checksum system reduces the performance overhead of verification but not the storage cost of individual checksum entries. Inline storage for very small files is planned as a future optimization.

**Extent fragmentation and checksum group churn under Copy-On-Write (COW).** Because all random-access modifications use COW, workloads involving frequent small writes to existing files (e.g., SQLite databases, VM disk images, active system logs) will cause rapid extent list growth. Furthermore, because COW writes allocate new blocks in different physical spans, Phase 5 confirmation must synchronously update 100-block group hashes for both the source and destination groups. This will exhaust the inode's 4 inline and 255 indirect blocks quickly under heavy random write activity, increasing metadata lookup latency and SSD write cycles. DNPFS is structurally optimized for bulk sequential storage and is not recommended for random write-heavy database workloads.


**No existing tool compatibility.** Standard tools (`fsck`, `testdisk`, `photorec`, `blkid`) do not understand DNPFS. All maintenance and recovery requires DNPFS-specific tooling. This improves as adoption grows.

---

## Known Unsolvable Problems

These are fundamental constraints that cannot be engineered away, only mitigated.

### Metadata/Data Time-Gap & Split-Brain Risk

Two physically separate devices cannot achieve native hardware-level atomic synchronization during a crash. There exists a microsecond gap between the metadata device confirmation and the data device write where power loss can occur, introducing a structural **split-brain risk** where the metadata device and data device disagree on state.

DNPFS resolves this by defining strict write-ordering boundaries using WAL transactions. If power is lost during this time-gap, recovery is fully deterministic: the boot-time recovery manager scans `/transactions/` for active manifests and replays or rolls back the metadata to match the physical blocks actually written on the data drive, eliminating split-brain desynchronization.

### Drive Firmware Lies

Even with FUA, some drive firmware reports confirmation before physical write is complete. DNPFS mitigates this with read-back checksum verification but cannot fully compensate for drives that lie about FUA compliance. Use of drives with verified FUA support is recommended.

---

## Future Work

- **Git-based metadata backup** — serialize metadata state as structured text (YAML/CBOR) and commit to a self-hosted git repository (Gitea/Forgejo) on every backup trigger. Provides full version history, delta compression, diff visibility between states, and push to any remote. Planned as a separate subsystem with native support in `dnpfsd`.
- **Inline small file storage** — files below a configurable size threshold (suggested: 4KB) stored entirely on the meta device, eliminating the data device round-trip and block-level checksum overhead for small files
- **Parity blocks for meta device** — store XOR parity of metadata regions to enable single-sector reconstruction without a full backup restore
- **Double parity** — extend to two-sector failure recovery for critical structures (superblock, inode table header)
- **Network meta device** — allow meta device to live on a network share or NAS for high-availability setups
- **Multi-data-device with Size-Based Spanning** — span a single metadata device across multiple data HDDs. Files are sorted at write-time and routed based on size thresholds (e.g., >1GB to HDD 1, 500MB–1GB to HDD 2, 100MB–500MB to HDD 3, etc.) to optimize device wear and capacity.
- **Filesystem adoption** — submit to Linux kernel for inclusion in `fs/` tree; update `fsck`, `blkid`, and `udev` for native DNPFS awareness
- **Transparent Data Compression** — transparent compression of data blocks utilizing fast algorithms like **LZ4** or **ZSTD** (optimizing repeating binary pattern compression) with compressed block sizes and compression state flags tracked directly in the metadata device's block map.

---

## Prior Art & Design Precedents

DNPFS builds upon several established structural paradigms and caching drivers in the Linux filesystem space:
* **ZFS Special VDEV Class:** ZFS allows allocating specific metadata blocks, DDT (deduplication tables), and small files to dedicated SSD devices. DNPFS takes this partition isolation further by mandating physical separation at the device driver layer.
* **Ext4 External Journal:** Ext4 has long supported allocating its journal and transaction logs to a separate fast physical block device (`ext4 -J device=...`), separating synchronous log writes from primary metadata and data.
* **bcache & dm-cache:** These drivers operate as block-level SSD caching layers underneath generic filesystems. DNPFS is a native filesystem rather than a block-level cache, allowing it to leverage semantic filesystem data (such as size-based extent grouping, transaction verification, and live fallbacks) that block-level caches cannot access.

---

## Contributing

DNPFS is open source and dual-licensed under **GPL-2.0-only** and **MIT**. Contributions, design feedback, and alternative solutions to the problems described here are welcome. If you have encountered a similar architecture and have real-world failure data, please open an issue — especially for the cache coherency and FUA compliance sections.

The design is more important than the code at this stage. If you see a flaw in the architecture, that is the most valuable thing to report.

---

*DNPFS Architecture Specification v0.2 — Updated Design Draft*
*Status: Pre-implementation, open for community review*
