use crate::extent::DnpfsExtent;
use bytemuck::{bytes_of, Pod, Zeroable};
use xxhash_rust::xxh3::xxh3_64;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InodePayload {
    pub data: [u8; 128],
}

unsafe impl Zeroable for InodePayload {}
unsafe impl Pod for InodePayload {}

impl InodePayload {
    pub fn get_extents(&self) -> ([DnpfsExtent; 4], u64) {
        let extents_bytes = &self.data[0..64];
        let extents: [DnpfsExtent; 4] = *bytemuck::from_bytes(extents_bytes);
        let mut indirect_bytes = [0u8; 8];
        indirect_bytes.copy_from_slice(&self.data[64..72]);
        let indirect_ptr = u64::from_le_bytes(indirect_bytes);
        (extents, indirect_ptr)
    }

    pub fn set_extents(&mut self, extents: &[DnpfsExtent; 4], indirect_ptr: u64) {
        let extents_bytes = bytemuck::bytes_of(extents);
        self.data[0..64].copy_from_slice(extents_bytes);
        self.data[64..72].copy_from_slice(&indirect_ptr.to_le_bytes());
    }
}

/// Fixed 256-byte Inode structure on DNPFS Metadata Device
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    pub inode_id: u64,
    pub file_size: u64,
    pub block_count: u64,
    pub link_count: u32,
    pub mode_flags: u32,
    pub payload: InodePayload,
    pub created: u64,
    pub modified: u64,
    pub permissions: u32,
    pub owner: u32,
    pub flags: u32,
    pub reserved0: u32,
    pub fallback_path_offset: u64,
    pub reserved: [u8; 48],
    pub checksum: u64,
}

unsafe impl Zeroable for Inode {}
unsafe impl Pod for Inode {}

impl Inode {
    pub fn compute_checksum(&self) -> u64 {
        let bytes = bytes_of(self);
        let data_slice = &bytes[0..bytes.len() - 8];
        xxh3_64(data_slice)
    }

    pub fn is_valid(&self) -> bool {
        self.checksum == self.compute_checksum()
    }

    pub fn is_dir(&self) -> bool {
        (self.mode_flags & (libc::S_IFMT as u32)) == (libc::S_IFDIR as u32)
    }

    pub fn is_reg(&self) -> bool {
        (self.mode_flags & (libc::S_IFMT as u32)) == (libc::S_IFREG as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inode_size() {
        assert_eq!(std::mem::size_of::<Inode>(), 256);
    }
}
