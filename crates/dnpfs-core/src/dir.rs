use bytemuck::{Pod, Zeroable};

/// Directory Entry structure stored in directory metadata blocks
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub inode_id: u64,
    pub file_type: u8,
    pub reserved0: u8,
    pub name_len: u16,
    pub reserved: u32,
    pub name: [u8; 256],
}

unsafe impl Zeroable for DirEntry {}
unsafe impl Pod for DirEntry {}

impl DirEntry {
    pub fn new(inode_id: u64, file_type: u8, name_str: &str) -> Self {
        let mut name = [0u8; 256];
        let bytes = name_str.as_bytes();
        let len = bytes.len().min(255);
        name[..len].copy_from_slice(&bytes[..len]);
        Self {
            inode_id,
            file_type,
            reserved0: 0,
            name_len: len as u16,
            reserved: 0,
            name,
        }
    }

    pub fn name_as_str(&self) -> &str {
        let len = (self.name_len as usize).min(256);
        std::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}
