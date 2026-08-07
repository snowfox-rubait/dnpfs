use crate::constants::DATA_HEADER_MAGIC;
use bytemuck::{bytes_of, Pod, Zeroable};
use xxhash_rust::xxh3::xxh3_64;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DataHeader {
    pub magic: u32,
    pub version: u32,
    pub uuid_data: [u8; 16],
    pub uuid_meta: [u8; 16],
    pub total_blocks: u64,
    pub block_size: u32,
    pub reserved: [u8; 452],
    pub checksum: u64,
}

unsafe impl Zeroable for DataHeader {}
unsafe impl Pod for DataHeader {}

impl DataHeader {
    pub fn compute_checksum(&self) -> u64 {
        let bytes = bytes_of(self);
        let data_slice = &bytes[0..bytes.len() - 8];
        xxh3_64(data_slice)
    }

    pub fn is_valid(&self) -> bool {
        self.magic == DATA_HEADER_MAGIC && self.checksum == self.compute_checksum()
    }
}
