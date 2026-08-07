use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Represents a contiguous run of blocks on the DATA device
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Pod, Zeroable)]
pub struct DnpfsExtent {
    pub start_block: u64,
    pub block_count: u64,
}

impl DnpfsExtent {
    pub fn new(start_block: u64, block_count: u64) -> Self {
        Self {
            start_block,
            block_count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.block_count == 0
    }
}
