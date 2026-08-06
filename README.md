# DNPFS — Definitely Not Paranoid File System

> *"It's not paranoia if the disk is actually dying."*

DNPFS is an open source Linux filesystem that separates raw data storage from all filesystem structures across two physical devices. Your large drive stores only raw data blocks. A small, fast secondary drive stores everything else — metadata, journal, checksums, bad block maps, and bookkeeping. Nothing else lives on either drive.

This is not a RAID setup. This is not a cache layer. This is a ground-up rethink of where filesystem structures should physically live.

---

## Why

On a traditional single-drive filesystem, every read and write involves the drive jumping between data regions and metadata regions constantly. The head seeks. Performance suffers. Metadata and data compete for the same I/O queue, the same cache, the same physical medium.

DNPFS eliminates that entirely.

- The data drive does one thing: store data sequentially. No seeking for metadata. No journal writes interrupting data writes.
- The metadata drive is small and fast (SSD recommended). Lookups are near-instant. You get a precise pointer, then go straight to the right sector on the data drive.
- Metadata is small enough to back up completely, redundantly, and frequently. Your filesystem map is protected independently from your data.
- Per-block checksums stored on the metadata drive catch silent write failures — cases where a drive lies and says a write succeeded but wrote garbage instead.

---

## Key Features

**Dry run before every write.** Every operation is simulated first. An `allocation.dry` manifest (binary `struct` on disk, rendered as YAML by userspace tools) is generated and committed before anything touches either device. You know exactly where data will land before it lands there.

**Kernel-managed block reservation.** Target extents are reserved during an operation so nothing else can overwrite them. Reservations are held in RAM strictly by the kernel's transaction coordinator for the duration of the write and do not expire based on wall-clock time, eliminating mid-runtime double-allocation races. On crash recovery, dangling reservations from interrupted writes are automatically released.

**Forensic recovery manifest.** If a write is interrupted by power loss or disconnection, the manifest tells you exactly which blocks were written and which were not. Recovery is precise, not a guess.

**Transactional isolation via Inode flags.** Files being written/copied are immediately visible in the directory structure but carry the `INODE_PENDING_COMMIT` flag. Any concurrent read or write requests to pending files block (or return `EBUSY` if `O_NONBLOCK` is set) until the transaction commits, preventing access to unwritten or partial disk blocks. *(Live-Migration Symlink Fallback is planned for V2).*

**Two-level Merkle checksumming.** At write time, data blocks are verified using 64-bit xxHash3 checksums with a static 100-block group layout stored on the metadata device. Routine integrity checks verify group checksums first, dropping to individual block checksums only to pinpoint corruption. *(Dynamic size-based grouping and deferred checksumming are planned for V2).*

**Accidental format protection.** While mounted, DNPFS opens the data device exclusively (`O_EXCL`), causing formatting utilities (`mkfs`, `fdisk`) to fail with a device-busy error. When unmounted, a Sector 0 Data Signature Header provides advisory identification for `blkid` and `wipefs`.

**Bad sector tracking.** A bad block map lives on the metadata drive. Bad sectors detected during write verification or read scrubbing are logged and permanently avoided. Write failures dynamically allocate alternate blocks; read-time at-rest corruption triggers bad-block logging and reports `EIO`.

**S.M.A.R.T. integration.** Both drives are monitored continuously. Rising remap counts, pending sectors, uncorrectable errors — all surfaced with recommended actions before data loss occurs.

**Metadata backup system.** The metadata drive can be imaged completely. Backups are triggered automatically on large operations, on S.M.A.R.T. warnings, or on a schedule. If the metadata drive dies, restore from backup to a new drive and continue.

**Block-layer Encryption via LUKS.** Encryption is applied at the standard Linux block layer (`dm-crypt`/LUKS) beneath DNPFS. Devices are unlocked via standard OS utilities (`cryptsetup`) prior to mounting. *(Native driver-managed key chaining is planned for V2).*

**RAID compatible.** DNPFS works on top of any mdadm or dm-raid array transparently. RAID 1 on the metadata device is strongly recommended for high-availability setups and significantly reduces metadata hardware failure risk.

**Copy-On-Write (COW) modifications.** To maintain strict transactional safety, random writes (`pwrite`) and truncations (`ftruncate`) use Copy-On-Write rather than in-place block updates. Edits allocate new block extents, keeping the old data intact until the write successfully verifies and commits. *(Note: COW random-access writes incur metadata extent growth and checksum group write churn; DNPFS is structurally optimized for bulk sequential storage rather than random write-heavy database workloads).*

**Unified transaction model.** Writes, deletes, copies, and renames all follow the same lifecycle: plan → dry run → reserve → execute → verify → confirm. Every operation is recoverable at every stage. Direct cross-device moves are banned and replaced by a secure Copy-Verify-Delete pipeline. Because writes are tracked by fixed `inode_id` mappings rather than file paths, pending copies can be renamed or deleted (aborting the transaction) on the fly without disrupting the active copy worker. Non-conflicting writes execute in parallel, and metadata commits are coalesced via **Group Commits** on the metadata SSD.

> [!IMPORTANT]
> **Contributor Invariant:** The source file deletion phase **must never** execute in parallel with the copy phase or before verification has fully succeeded. Any attempts to "optimize" this by running copy and delete operations concurrently are strictly prohibited. This is a core architectural decision for data safety and must not be changed.

---

## Architecture

See [ARCHITECTURE.md](./ARCHITECTURE.md) for the full technical specification including:

- Physical device layout
- Storage limits and why they don't constrain you
- All core data structures with field-level detail
- Two-level checksum strategy (grouped Merkle-style + per-block)
- The `allocation.dry` manifest format
- Complete transaction lifecycle (6 phases)
- Driver architecture (kernel module, userspace daemon, tools)
- Cache coherency and FUA write ordering strategy
- Power loss and crash recovery procedure
- Bad sector handling and silent failure detection
- S.M.A.R.T. integration thresholds
- Metadata backup and restore
- Encryption modes (LUKS, 4 options, chained unlock)
- RAID compatibility and recommended configurations
- TRIM/discard coordination
- Concurrent operation handling
- Defragmentation policy
- Recovery tooling specifications
- Known limitations (honestly documented)
- Known unsolvable problems (also honestly documented)

---

## Project Status

**Pre-implementation. Architecture and design phase.**

The design is complete enough to begin implementation. The codebase does not exist yet. This repository is being opened now specifically to invite community review of the architecture before a single line of kernel code is written — because fixing a design flaw at this stage costs nothing, and fixing it after implementation costs everything.

If you see a problem with the architecture, that is the most valuable thing you can contribute right now.

---

## Planned Components & Implementation Strategy

To eliminate kernel-space development risks and iterate safely, implementation follows a two-stage roadmap: **Phase 1 (FUSE Prototype & Tooling)** followed by **Phase 2 (Production Kernel Module)**.

- `dnpfs-fuse` — userspace FUSE prototype to validate transaction lifecycles, dry-runs, and crash-injection recovery in userspace before kernel porting
- `dnpfs-format` — formats a paired meta + data volume
- `dnpfs.ko` — production Linux kernel module implementing the native VFS interface
- `dnpfsd` — userspace daemon for S.M.A.R.T. polling, backup scheduling, and orphan transaction monitoring
- `dnpfs-check` — two-device filesystem check tool
- `dnpfs-recover` — forensic recovery using `allocation.dry` manifests
- `dnpfs-backup` — metadata backup and restore
- `dnpfs-smart` — S.M.A.R.T. status report for both devices
- `dnpfs-dry` — inspect, confirm, or forcefully abort pending dry run manifests

---

## Hardware Requirements

| Role | Recommended | Minimum |
|---|---|---|
| Metadata device | SSD: ≥1.5% to 2.3% of Data capacity (e.g., 32GB–48GB SSD for 2TB HDD) | SSD: ≥1.0% of Data capacity (e.g., 16GB SSD for 1.6TB HDD) |
| Data device | Any HDD or SSD | Any block device |
| Metadata device interface | SATA SSD or NVMe | USB flash (not recommended for production) |

*Note on Metadata Sizing:* Level-1 64-bit checksums, dynamically growable inodes, and active transaction manifests consume metadata space proportional to the block count of the data device. A ratio of 1.0% to 1.5% of the data drive's capacity is required for standard workloads, while dense small-file workloads (e.g., 16KB per inode) require up to ~2.3% metadata capacity.

The metadata device is your most critical component. If it fails without a backup, the data device becomes a pile of unaddressed raw blocks. Treat it accordingly. Use reliable hardware, keep backups, and consider RAID 1 on the metadata device for zero-downtime failure handling.

**Encryption:** LUKS encryption is supported on either or both devices. An SSD metadata device with LUKS adds negligible latency. A passphrase is required at mount time if encryption is enabled — factor this into automated mount setups.

---

## Contributing

Contributions, suggestions, and design feedback are open to everyone.

**Right now the most useful contributions are:**

- Identifying flaws or gaps in the architecture (see ARCHITECTURE.md)
- Suggesting better solutions to the problems documented there
- Sharing experience with similar split-device or external-journal setups
- Pointing to prior art, research papers, or existing implementations we should be aware of

**When implementation begins:**

- Kernel module development (Linux VFS, block layer, I/O scheduling)
- Userspace tooling (C, Rust, or Python — to be decided)
- Test infrastructure (especially power loss simulation and bad sector injection)
- Documentation

To contribute, open an issue or pull request. Design discussions belong in issues. No contribution is too small — even catching a typo in the architecture doc means one less misunderstanding for someone reading it.

If you have tried something similar and failed (or quietly succeeded), please share it. Real-world failure data is more valuable than theoretical analysis.

---

## License

DNPFS is open source and dual-licensed under **GPL-2.0-only** and **MIT**.

This licensing model ensures full compatibility with the Linux kernel (allowing clean upstreaming into the mainline kernel without licensing conflicts or taints) while providing maximum flexibility for userspace components.

*(Formal COPYING and LICENSE files to be added before first release.)*

---

## Name

DNPFS stands for **Definitely Not Paranoid File System**.

It is paranoid. That is the point. Every write is treated as a potential failure. Every confirmation is verified. Every operation leaves a recoverable trail. The name is a reminder that in storage, paranoia is not a personality trait — it is a design requirement.

The name is also a nod to the open source tradition of developers who name their projects with a straight face and a raised eyebrow. You know who you are.

---

*Designed in public. Built in the open. Contributions welcome.*
