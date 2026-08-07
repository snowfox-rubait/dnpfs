use bytemuck::bytes_of;
use clap::Parser;
use dnpfs_core::*;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(author, version, about = "Format paired DNPFS metadata and data volumes", long_about = None)]
struct Args {
    /// Path to metadata device or image file (DNPFS_META)
    #[arg(short, long)]
    meta: PathBuf,

    /// Path to data device or image file (DNPFS_DATA)
    #[arg(short, long)]
    data: PathBuf,

    /// Force format even if devices already contain data
    #[arg(short, long, default_value_t = false)]
    force: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("==================================================");
    println!("DNPFS — Format Utility");
    println!("==================================================");
    println!("Metadata volume : {:?}", args.meta);
    println!("Data volume     : {:?}", args.data);

    let mut meta_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&args.meta)?;
    let mut data_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&args.data)?;

    let meta_len = meta_file.metadata()?.len();
    let data_len = data_file.metadata()?.len();

    println!("Metadata device size : {} MB", meta_len / (1024 * 1024));
    println!("Data device size     : {} MB", data_len / (1024 * 1024));

    if meta_len < 16 * 1024 * 1024 {
        return Err("Metadata device must be at least 16 MB".into());
    }
    if data_len < 64 * 1024 * 1024 {
        return Err("Data device must be at least 64 MB".into());
    }

    let uuid_meta = Uuid::new_v4();
    let uuid_data = Uuid::new_v4();

    println!("Generated Volume UUIDs:");
    println!("  Meta Device UUID : {}", uuid_meta);
    println!("  Data Device UUID : {}", uuid_data);

    let total_data_blocks = data_len / (BLOCK_SIZE as u64);
    let total_inodes = 4096u64;

    // 1. Format DATA Device (Sector 0 Header & Backup Header)
    println!("[1/2] Formatting DATA volume...");
    let mut data_header = DataHeader {
        magic: DATA_HEADER_MAGIC,
        version: 1,
        uuid_data: *uuid_data.as_bytes(),
        uuid_meta: *uuid_meta.as_bytes(),
        total_blocks: total_data_blocks,
        block_size: BLOCK_SIZE as u32,
        reserved: [0u8; 452],
        checksum: 0,
    };
    data_header.checksum = data_header.compute_checksum();

    data_file.seek(SeekFrom::Start(0))?;
    data_file.write_all(bytes_of(&data_header))?;

    if data_len >= 512 {
        data_file.seek(SeekFrom::Start(data_len - 512))?;
        data_file.write_all(bytes_of(&data_header))?;
    }
    data_file.flush()?;

    // 2. Format META Device
    println!("[2/2] Formatting META volume...");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let superblock_offset = SUPERBLOCK_OFFSET;
    let inode_table_offset = 64 * 1024u64;
    let checksum_table_offset = inode_table_offset + (total_inodes * INODE_SIZE as u64);
    let bad_block_map_offset =
        checksum_table_offset + (total_data_blocks * std::mem::size_of::<BlockChecksumEntry>() as u64);
    let journal_offset = bad_block_map_offset + (64 * 1024);
    let reservation_table_offset = journal_offset + (1024 * 1024);
    let transaction_region_offset = reservation_table_offset + (256 * 1024);

    let mut sb = Superblock {
        magic: SUPERBLOCK_MAGIC,
        version: 1,
        compat_flags: 0,
        incompat_flags: 0,
        ro_compat_flags: 0,
        reserved0: 0,
        uuid_meta: *uuid_meta.as_bytes(),
        uuid_data: *uuid_data.as_bytes(),
        block_size: BLOCK_SIZE as u32,
        reserved1: 0,
        total_data_blocks,
        total_inodes,
        free_data_blocks: total_data_blocks - 1,
        free_inodes: total_inodes - 1,
        root_inode: 1,
        transaction_region_offset,
        last_mount_time: now,
        last_write_time: now,
        journal_offset,
        checksum_table_offset,
        bad_block_map_offset,
        reservation_table_offset,
        superblock_checksum: 0,
    };
    sb.superblock_checksum = sb.compute_checksum();

    meta_file.seek(SeekFrom::Start(superblock_offset))?;
    meta_file.write_all(bytes_of(&sb))?;

    for &block_off in &[1024u64, 8192u64, 32768u64] {
        let byte_off = block_off * BLOCK_SIZE as u64;
        if byte_off + (std::mem::size_of::<Superblock>() as u64) <= meta_len {
            meta_file.seek(SeekFrom::Start(byte_off))?;
            meta_file.write_all(bytes_of(&sb))?;
        }
    }

    // Initialize Root Inode
    let mut root_inode = Inode {
        inode_id: 1,
        file_size: 0,
        block_count: 0,
        link_count: 2,
        mode_flags: (libc::S_IFDIR | 0o755) as u32,
        payload: InodePayload { data: [0u8; 128] },
        created: now,
        modified: now,
        permissions: 0o755,
        owner: 1000,
        flags: 0,
        reserved0: 0,
        fallback_path_offset: 0,
        reserved: [0u8; 48],
        checksum: 0,
    };
    root_inode.checksum = root_inode.compute_checksum();

    let root_inode_offset = inode_table_offset;
    meta_file.seek(SeekFrom::Start(root_inode_offset))?;
    meta_file.write_all(bytes_of(&root_inode))?;

    meta_file.flush()?;

    println!("==================================================");
    println!("DNPFS volume successfully formatted!");
    println!("==================================================");

    Ok(())
}
