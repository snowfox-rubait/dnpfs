use bytemuck::{Pod, Zeroable};
use xxhash_rust::xxh3::xxh3_64;

/// Level 1 Block Checksum Entry in checksum table on Metadata SSD
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct BlockChecksumEntry {
    pub checksum: u64,
    pub last_verified: u64,
    pub status: u32,
    pub reserved: u32,
}

impl BlockChecksumEntry {
    pub fn new(checksum: u64, timestamp: u64) -> Self {
        Self {
            checksum,
            last_verified: timestamp,
            status: 0, // 0 = Good
            reserved: 0,
        }
    }
}

/// Compute Level 1 xxHash3_64 checksum for a 4KB data block
pub fn compute_block_checksum(block_data: &[u8]) -> u64 {
    xxh3_64(block_data)
}

/// Compute Level 2 Group Checksum over a span of 100 Level-1 block checksums
pub fn compute_group_checksum(level1_checksums: &[u64]) -> u64 {
    let bytes = bytemuck::cast_slice(level1_checksums);
    xxh3_64(bytes)
}
