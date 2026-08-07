# Proposed Architecture Enhancements & Specification Amendments
## Document Version: 0.2 (Draft Proposal for Integration into ARCHITECTURE.md)

> **Context:** This document gathers proposed fixes for design contradictions, legal/linker constraints, and jargon simplification. These amendments will be integrated into the primary `ARCHITECTURE.md` file after review. All previously approved modifications (segmented layouts, capacity alerts, and bootstrap partition changes) have already been merged into `ARCHITECTURE.md` and are removed from this draft list.

---

## 1. HDD Read-Back Verification Performance Fix

### Problem Statement
The v0.2 transaction lifecycle specified a synchronous read-back verification step for every written data block (Phase 4, Step 3). On spinning hard drives (HDDs), reading back blocks immediately after writing them forces the drive head to seek backward and wait for platter rotation. This introduces severe rotational latency and completely negates DNPFS's primary performance advantage: sequential, seek-free writes on the data drive.

### Proposed Specification Change: Deferred Background Verification
- **Write Path Default Behavior:** Synchronous read-back verification is **disabled by default on spinning data HDDs** during Phase 4 (Execution). Data blocks are written sequentially, and their Level-1 checksums are committed directly to the SSD metadata table.
- **Asynchronous Scrubbing Delegation:** Read-back integrity verification is delegated to `dnpfsd`'s background scrubbing engine during idle periods (or via periodic filesystem scrubs). 
- **Optional Paranoid Mode:** Users can explicitly enable synchronous read-back verification via a mount option (`verify=sync_readback`) on high-speed arrays or SSDs where seek latency is negligible.

---

## 2. Kernel Module Licensing Compliance

### Problem Statement
The specification proposed a dual-license model (GPL/MIT) for the entire DNPFS project. However, the production Linux kernel module (`dnpfs.ko`) must link against core virtual filesystem (VFS) symbols exported by the kernel, many of which are marked as `GPL_ONLY`. Distributing `dnpfs.ko` under a non-GPL compatible license creates a legal and linker conflict with the Linux kernel license boundaries.

### Proposed Specification Change: Distinct Licensing Boundaries
- **Kernel Module (`dnpfs.ko`):** Explicitly licensed under **GPL-2.0-only** to ensure full compliance with the Linux kernel VFS subsystem requirements and symbol exports.
- **Userspace Components:** The userspace prototype driver (`dnpfs-fuse`), formatter (`dnpfs-format`), and administration utility tools (`dnpfs-tools`) remain dual-licensed under **GPL-2.0-only** and **MIT**.

---

## 3. Specification Jargon Simplification

### Problem Statement
Several sections of the specification use proprietary-sounding terminology for standard, well-established computer science techniques. This increases the cognitive load for external reviewers and obscures the simplicity of the underlying design.

### Proposed Specification Change: Standard Terminology Alignment
- **"Strict Ascending Extent Locking Invariant"** is renamed to: **Lock Ordering by Block Address** (standard VFS practice to prevent AB-BA circular wait deadlocks).
- **"RANGE_PRIORITY_INHERITANCE" / "Priority Aging"** is renamed to: **Starvation Prevention via Priority Inheritance** (standard range-lock starvation resolution technique).

---

## Summary of Proposed Fixes

| Feature / Mismatch | Proposed Fix | Affected Specification Section |
|---|---|---|
| **HDD Write Throughput Bottleneck** | Disable read-back verify by default; delegate to background scrub | Section 8 (Transaction Lifecycle) & Section 12 (Bad Sector Tracking) |
| **GPL/MIT Symbol Linker Conflict** | License `dnpfs.ko` strictly under `GPL-2.0-only` | Section 9 (Driver Architecture) & Section 23 (Contributing) |
| **Over-complex Terminology** | Map custom locking names to standard VFS terms | Section 18 (Concurrent Operation Handling) |
