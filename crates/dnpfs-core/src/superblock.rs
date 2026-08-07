use crate::constants::SUPERBLOCK_MAGIC;
use bytemuck::{bytes_of, Pod, Zeroable};
use xxhash_rust::xxh3::xxh3_64;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub magic: u32,
    pub version: u32,
    pub compat_flags: u32,
    pub incompat_flags: u32,
    pub ro_compat_flags: u32,
    pub reserved0: u32,
    pub uuid_meta: [u8; 16],
    pub uuid_data: [u8; 16],
    pub block_size: u32,
    pub reserved1: u32,
    pub total_data_blocks: u64,
    pub total_inodes: u64,
    pub free_data_blocks: u64,
    pub free_inodes: u64,
    pub root_inode: u64,
    pub transaction_region_offset: u64,
    pub last_mount_time: u64,
    pub last_write_time: u64,
    pub journal_offset: u64,
    pub checksum_table_offset: u64,
    pub bad_block_map_offset: u64,
    pub reservation_table_offset: u64,
    pub superblock_checksum: u64,
}

unsafe impl Zeroable for Superblock {}
unsafe impl Pod for Superblock {}

impl Superblock {
    /// Calculate xxHash3_64 over all superblock fields except the checksum field itself
    pub fn compute_checksum(&self) -> u64 {
        let bytes = bytes_of(self);
        let data_slice = &bytes[0..bytes.len() - 8];
        xxh3_64(data_slice)
    }

    pub fn is_valid(&self) -> bool {
        self.magic == SUPERBLOCK_MAGIC && self.superblock_checksum == self.compute_checksum()
    }
}
