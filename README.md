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

**Dry run before every write.** Every operation is simulated first. An `allocation.dry` manifest is generated and committed before anything touches either device. You know exactly where data will land before it lands there.

**Block reservation with lease renewal.** Target extents are leased during an operation so nothing else can overwrite them. Expiry is calculated dynamically based on the estimated write time, and active long-running writes renew their lease using a heartbeat mechanism. If aborted, reservations expire automatically to prevent ghost block pileups.

**Forensic recovery manifest.** If a write is interrupted by power loss or disconnection, the manifest tells you exactly which blocks were written and which were not. Recovery is precise, not a guess.

**Transactional isolation via Inode flags.** Files being written/copied are immediately visible in the directory structure but carry the `INODE_PENDING_COMMIT` flag. Any concurrent read requests to these files are either transparently redirected (using the Live-Migration Symlink Fallback) or blocked, preventing processes from reading stale/unwritten disk blocks.

**Two-level deferred checksumming.** At copy/write time, blocks are grouped (dynamic sizing based on file size) to write a single group checksum, minimizing write latency and SSD wear. Individual file/block SHA-256 checksums are generated in the background when the drive is idle. Routine integrity checks verify group checksums first, dropping to individual checksums only to pinpoint corruption.

**Accidental format protection.** When mounted, DNPFS opens the data device exclusively (`O_EXCL`), causing formatting and partitioning utilities (`mkfs`, `fdisk`) to fail with a write-protect/busy error. This is coupled with a minimal Data Signature Header on Sector 0 (and backed up on the last sector) so standard tools identify it as DNPFS.

**Bad sector tracking.** A bad block map lives on the metadata drive. Bad sectors on the data drive are detected, logged, and permanently avoided. The data drive's dying state is tracked in detail — even as it degrades.

**S.M.A.R.T. integration.** Both drives are monitored continuously. Rising remap counts, pending sectors, uncorrectable errors — all surfaced with recommended actions before data loss occurs.

**Metadata backup system.** The metadata drive can be imaged completely. Backups are triggered automatically on large operations, on S.M.A.R.T. warnings, or on a schedule. If the metadata drive dies, restore from backup to a new drive and continue.

**Encryption via LUKS.** Four modes: encrypt the metadata device only (recommended default), encrypt the data device only, encrypt both, or no encryption. Mode 3 uses a single passphrase that unlocks both devices in sequence — one unlock, full protection.

**RAID compatible.** DNPFS works on top of any mdadm or dm-raid array transparently. RAID 1 on the metadata device is strongly recommended for high-availability setups and eliminates the metadata single point of failure entirely.

**Copy-On-Write (COW) modifications.** To maintain strict transactional safety, random writes (`pwrite`) and truncations (`ftruncate`) use Copy-On-Write rather than in-place block updates. Edits allocate new block extents, keeping the old data intact until the write successfully verifies and commits.

**Unified transaction model.** Writes, deletes, copies, and renames all follow the same lifecycle: plan → dry run → reserve → execute → verify → confirm. Every operation is recoverable at every stage. Direct cross-device moves are banned and replaced by a secure Copy-Verify-Delete pipeline, featuring a Live-Migration Symlink Fallback to guarantee zero-downtime read redirection during copying. Because writes are tracked by fixed `inode_id` mappings rather than file paths, pending copies can be renamed or deleted (aborting the transaction) on the fly without disrupting the active copy worker. Non-conflicting writes execute in parallel, and metadata commits are coalesced via **Group Commits** on the metadata SSD.

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

## Planned Components

- `dnpfs.ko` — Linux kernel module implementing the VFS interface
- `dnpfsd` — userspace daemon for S.M.A.R.T. polling, backup scheduling, reservation expiry
- `dnpfs-format` — formats a paired meta + data volume
- `dnpfs-check` — two-device filesystem check tool
- `dnpfs-recover` — forensic recovery using `allocation.dry` manifests
- `dnpfs-backup` — metadata backup and restore
- `dnpfs-smart` — S.M.A.R.T. status report for both devices
- `dnpfs-dry` — inspect, confirm, or cancel pending dry run manifests

---

## Hardware Requirements

| Role | Recommended | Minimum |
|---|---|---|
| Metadata device | SSD: ≥1.5% of Data capacity (e.g., 32GB SSD for 2TB HDD) | SSD: ≥1.0% of Data capacity (e.g., 16GB SSD for 1.6TB HDD) |
| Data device | Any HDD or SSD | Any block device |
| Metadata device interface | SATA SSD or NVMe | USB flash (not recommended for production) |

*Note on Metadata Sizing:* Level-1 64-bit checksums, dynamically growable inodes, and active transaction manifests consume metadata space proportional to the block count of the data device. A ratio of 1.0% to 1.5% of the data drive's capacity is required for optimal operation.

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
