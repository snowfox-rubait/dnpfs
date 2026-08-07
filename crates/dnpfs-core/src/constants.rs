/// DNPFS Core Constants matching ARCHITECTURE.md v0.2 spec

/// Superblock Magic Number: "DNPF" (0x444E5046)
pub const SUPERBLOCK_MAGIC: u32 = 0x444E5046;

/// Data Device Header Magic Number: "DNDT" (0x444E4454)
pub const DATA_HEADER_MAGIC: u32 = 0x444E4454;

/// Default Block Size (4 KB)
pub const BLOCK_SIZE: usize = 4096;

/// Fixed Inode Struct Size (256 bytes)
pub const INODE_SIZE: usize = 256;

/// Sector Size (512 bytes)
pub const SECTOR_SIZE: usize = 512;

/// Fixed Superblock Offset on META device (0x1000 = 4096 bytes)
pub const SUPERBLOCK_OFFSET: u64 = 0x1000;

/// Number of blocks covered per Level-2 Group Checksum
pub const CHECKSUM_GROUP_SIZE: usize = 100;

/// Inode flags: File is pending transaction commit
pub const INODE_FLAG_PENDING_COMMIT: u32 = 0x0001;

/// Maximum Inline Extents per Inode
pub const INODE_INLINE_EXTENTS: usize = 4;

/// Name max limit
pub const NAME_MAX: usize = 255;
