use bytemuck::{bytes_of, from_bytes};
use clap::Parser;
use dnpfs_core::*;
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEntry, ReplyWrite, Request,
};
use libc::{EIO, ENOENT, ENOSPC};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(1);

#[derive(Parser, Debug)]
#[command(author, version, about = "DNPFS FUSE Driver", long_about = None)]
struct Args {
    /// Path to metadata volume/device (DNPFS_META)
    #[arg(short, long)]
    meta: PathBuf,

    /// Path to data volume/device (DNPFS_DATA)
    #[arg(short, long)]
    data: PathBuf,

    /// Mount point directory
    mountpoint: PathBuf,
}

struct DnpfsFsInner {
    meta_file: File,
    data_file: File,
    superblock: Superblock,
    data_header: DataHeader,
    next_inode_id: u64,
    next_data_block: u64,
}

impl DnpfsFsInner {
    fn read_inode(&mut self, ino: u64) -> Result<Inode, i32> {
        let inode_table_offset = 64 * 1024u64;
        let inode_offset = inode_table_offset + ((ino - 1) * INODE_SIZE as u64);
        self.meta_file
            .seek(SeekFrom::Start(inode_offset))
            .map_err(|_| EIO)?;
        let mut buf = [0u8; INODE_SIZE];
        self.meta_file.read_exact(&mut buf).map_err(|_| EIO)?;
        let inode: Inode = *from_bytes(&buf);
        if inode.inode_id == 0 || !inode.is_valid() {
            Err(ENOENT)
        } else {
            Ok(inode)
        }
    }

    fn write_inode(&mut self, inode: &mut Inode) -> Result<(), i32> {
        inode.checksum = inode.compute_checksum();
        let inode_table_offset = 64 * 1024u64;
        let inode_offset = inode_table_offset + ((inode.inode_id - 1) * INODE_SIZE as u64);
        self.meta_file
            .seek(SeekFrom::Start(inode_offset))
            .map_err(|_| EIO)?;
        self.meta_file
            .write_all(bytes_of(inode))
            .map_err(|_| EIO)?;
        self.meta_file.flush().map_err(|_| EIO)?;
        Ok(())
    }

    fn inode_to_attr(&self, inode: &Inode) -> FileAttr {
        let kind = if inode.is_dir() {
            FileType::Directory
        } else {
            FileType::RegularFile
        };

        FileAttr {
            ino: inode.inode_id,
            size: inode.file_size,
            blocks: inode.block_count,
            atime: UNIX_EPOCH + Duration::from_secs(inode.modified),
            mtime: UNIX_EPOCH + Duration::from_secs(inode.modified),
            ctime: UNIX_EPOCH + Duration::from_secs(inode.created),
            crtime: UNIX_EPOCH + Duration::from_secs(inode.created),
            kind,
            perm: inode.permissions as u16,
            nlink: inode.link_count,
            uid: inode.owner,
            gid: 1000,
            rdev: 0,
            blksize: BLOCK_SIZE as u32,
            flags: inode.flags,
        }
    }
}

struct DnpfsFs {
    inner: Mutex<DnpfsFsInner>,
}

impl Filesystem for DnpfsFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut inner = self.inner.lock().unwrap();
        let parent_inode = match inner.read_inode(parent) {
            Ok(ino) => ino,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        if !parent_inode.is_dir() {
            reply.error(ENOENT);
            return;
        }

        let dir_block_offset = inner.superblock.transaction_region_offset
            + (parent_inode.inode_id * BLOCK_SIZE as u64);
        if inner.meta_file.seek(SeekFrom::Start(dir_block_offset)).is_err() {
            reply.error(EIO);
            return;
        }

        let mut buf = [0u8; BLOCK_SIZE];
        if inner.meta_file.read_exact(&mut buf).is_err() {
            reply.error(ENOENT);
            return;
        }

        let entry_size = std::mem::size_of::<DirEntry>();
        let count = BLOCK_SIZE / entry_size;
        for i in 0..count {
            let start = i * entry_size;
            let entry_buf = &buf[start..start + entry_size];
            let entry: &DirEntry = from_bytes(entry_buf);
            if entry.inode_id > 0 && entry.name_as_str() == name_str {
                if let Ok(target_inode) = inner.read_inode(entry.inode_id) {
                    let attr = inner.inode_to_attr(&target_inode);
                    reply.entry(&TTL, &attr, 0);
                    return;
                }
            }
        }

        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let mut inner = self.inner.lock().unwrap();
        match inner.read_inode(ino) {
            Ok(inode) => {
                let attr = inner.inode_to_attr(&inode);
                reply.attr(&TTL, &attr);
            }
            Err(err) => reply.error(err),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let inode = match inner.read_inode(ino) {
            Ok(i) => i,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        if !inode.is_dir() {
            reply.error(ENOENT);
            return;
        }

        let mut entries: Vec<(u64, FileType, String)> = Vec::new();
        entries.push((ino, FileType::Directory, ".".to_string()));
        entries.push((1, FileType::Directory, "..".to_string()));

        let dir_block_offset =
            inner.superblock.transaction_region_offset + (inode.inode_id * BLOCK_SIZE as u64);
        if inner
            .meta_file
            .seek(SeekFrom::Start(dir_block_offset))
            .is_ok()
        {
            let mut buf = [0u8; BLOCK_SIZE];
            if inner.meta_file.read_exact(&mut buf).is_ok() {
                let entry_size = std::mem::size_of::<DirEntry>();
                let count = BLOCK_SIZE / entry_size;
                for i in 0..count {
                    let start = i * entry_size;
                    let entry_buf = &buf[start..start + entry_size];
                    let entry: &DirEntry = from_bytes(entry_buf);
                    if entry.inode_id > 0 {
                        let ftype = if entry.file_type == 2 {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        };
                        entries.push((entry.inode_id, ftype, entry.name_as_str().to_string()));
                    }
                }
            }
        }

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut inner = self.inner.lock().unwrap();
        let new_ino = inner.next_inode_id;
        inner.next_inode_id += 1;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut dir_inode = Inode {
            inode_id: new_ino,
            file_size: 0,
            block_count: 0,
            link_count: 2,
            mode_flags: (libc::S_IFDIR as u32) | (mode & 0o777),
            payload: InodePayload { data: [0u8; 128] },
            created: now,
            modified: now,
            permissions: mode & 0o777,
            owner: 1000,
            flags: 0,
            reserved0: 0,
            fallback_path_offset: 0,
            reserved: [0u8; 48],
            checksum: 0,
        };

        if inner.write_inode(&mut dir_inode).is_err() {
            reply.error(EIO);
            return;
        }

        let dir_block_offset =
            inner.superblock.transaction_region_offset + (parent * BLOCK_SIZE as u64);
        let mut buf = [0u8; BLOCK_SIZE];
        let _ = inner.meta_file.seek(SeekFrom::Start(dir_block_offset));
        let _ = inner.meta_file.read_exact(&mut buf);

        let entry_size = std::mem::size_of::<DirEntry>();
        let count = BLOCK_SIZE / entry_size;
        let mut added = false;

        for i in 0..count {
            let start = i * entry_size;
            let entry: &DirEntry = from_bytes(&buf[start..start + entry_size]);
            if entry.inode_id == 0 {
                let new_entry = DirEntry::new(new_ino, 2, name_str);
                buf[start..start + entry_size].copy_from_slice(bytes_of(&new_entry));
                added = true;
                break;
            }
        }

        if !added {
            reply.error(ENOSPC);
            return;
        }

        let _ = inner.meta_file.seek(SeekFrom::Start(dir_block_offset));
        let _ = inner.meta_file.write_all(&buf);
        let _ = inner.meta_file.flush();

        let attr = inner.inode_to_attr(&dir_inode);
        reply.entry(&TTL, &attr, 0);
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut inner = self.inner.lock().unwrap();
        let new_ino = inner.next_inode_id;
        inner.next_inode_id += 1;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut file_inode = Inode {
            inode_id: new_ino,
            file_size: 0,
            block_count: 0,
            link_count: 1,
            mode_flags: (libc::S_IFREG as u32) | (mode & 0o777),
            payload: InodePayload { data: [0u8; 128] },
            created: now,
            modified: now,
            permissions: mode & 0o777,
            owner: 1000,
            flags: INODE_FLAG_PENDING_COMMIT,
            reserved0: 0,
            fallback_path_offset: 0,
            reserved: [0u8; 48],
            checksum: 0,
        };

        if inner.write_inode(&mut file_inode).is_err() {
            reply.error(EIO);
            return;
        }

        let dir_block_offset =
            inner.superblock.transaction_region_offset + (parent * BLOCK_SIZE as u64);
        let mut buf = [0u8; BLOCK_SIZE];
        let _ = inner.meta_file.seek(SeekFrom::Start(dir_block_offset));
        let _ = inner.meta_file.read_exact(&mut buf);

        let entry_size = std::mem::size_of::<DirEntry>();
        let count = BLOCK_SIZE / entry_size;
        let mut added = false;

        for i in 0..count {
            let start = i * entry_size;
            let entry: &DirEntry = from_bytes(&buf[start..start + entry_size]);
            if entry.inode_id == 0 {
                let new_entry = DirEntry::new(new_ino, 1, name_str);
                buf[start..start + entry_size].copy_from_slice(bytes_of(&new_entry));
                added = true;
                break;
            }
        }

        if !added {
            reply.error(ENOSPC);
            return;
        }

        let _ = inner.meta_file.seek(SeekFrom::Start(dir_block_offset));
        let _ = inner.meta_file.write_all(&buf);
        let _ = inner.meta_file.flush();

        let attr = inner.inode_to_attr(&file_inode);
        reply.created(&TTL, &attr, 0, 0, 0);
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let mut inode = match inner.read_inode(ino) {
            Ok(i) => i,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        let (mut extents, _) = inode.payload.get_extents();
        let required_blocks = (data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE;

        let _start_data_block = if extents[0].is_empty() {
            let blk = inner.next_data_block;
            inner.next_data_block += required_blocks as u64;
            extents[0] = DnpfsExtent::new(blk, required_blocks as u64);
            blk
        } else {
            let blk = extents[0].start_block + extents[0].block_count;
            extents[0].block_count += required_blocks as u64;
            inner.next_data_block = inner.next_data_block.max(blk + required_blocks as u64);
            blk
        };

        // Write raw data bytes to DATA device
        let data_write_offset = (extents[0].start_block * BLOCK_SIZE as u64) + offset as u64;
        if inner
            .data_file
            .seek(SeekFrom::Start(data_write_offset))
            .is_err()
        {
            reply.error(EIO);
            return;
        }

        if inner.data_file.write_all(data).is_err() {
            reply.error(EIO);
            return;
        }
        let _ = inner.data_file.flush();

        // Write Level-1 Checksum table entries
        let entry_size = std::mem::size_of::<BlockChecksumEntry>() as u64;
        let now_sec = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        for b in 0..required_blocks {
            let block_offset = b * BLOCK_SIZE;
            let block_end = (block_offset + BLOCK_SIZE).min(data.len());
            let chunk = &data[block_offset..block_end];
            let xxhash = compute_block_checksum(chunk);

            let current_blk_idx = (offset as u64 / BLOCK_SIZE as u64) + b as u64;
            let target_blk = extents[0].start_block + current_blk_idx;

            let entry = BlockChecksumEntry::new(xxhash, now_sec);
            let off = inner.superblock.checksum_table_offset + (target_blk * entry_size);
            let _ = inner.meta_file.seek(SeekFrom::Start(off));
            let _ = inner.meta_file.write_all(bytes_of(&entry));
        }

        inode.payload.set_extents(&extents, 0);
        inode.file_size = (offset as u64 + data.len() as u64).max(inode.file_size);
        inode.block_count = extents[0].block_count;
        inode.flags &= !INODE_FLAG_PENDING_COMMIT;

        if inner.write_inode(&mut inode).is_err() {
            reply.error(EIO);
            return;
        }

        reply.written(data.len() as u32);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let inode = match inner.read_inode(ino) {
            Ok(i) => i,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        if offset as u64 >= inode.file_size {
            reply.data(&[]);
            return;
        }

        let (extents, _) = inode.payload.get_extents();
        let primary_extent = extents[0];
        if primary_extent.is_empty() {
            reply.data(&[]);
            return;
        }

        let read_size = (size as u64).min(inode.file_size - offset as u64) as usize;
        let data_read_offset = (primary_extent.start_block * BLOCK_SIZE as u64) + offset as u64;

        if inner
            .data_file
            .seek(SeekFrom::Start(data_read_offset))
            .is_err()
        {
            reply.error(EIO);
            return;
        }

        let mut buf = vec![0u8; read_size];
        if inner.data_file.read_exact(&mut buf).is_err() {
            reply.error(EIO);
            return;
        }

        let block_idx = primary_extent.start_block + (offset as u64 / BLOCK_SIZE as u64);
        let entry_size = std::mem::size_of::<BlockChecksumEntry>() as u64;
        let checksum_entry_offset =
            inner.superblock.checksum_table_offset + (block_idx * entry_size);
        if inner
            .meta_file
            .seek(SeekFrom::Start(checksum_entry_offset))
            .is_ok()
        {
            let mut entry_buf = [0u8; std::mem::size_of::<BlockChecksumEntry>()];
            if inner.meta_file.read_exact(&mut entry_buf).is_ok() {
                let entry: &BlockChecksumEntry = from_bytes(&entry_buf);
                let check_len = buf.len().min(BLOCK_SIZE);
                let computed = compute_block_checksum(&buf[..check_len]);
                if entry.checksum != 0 && computed != entry.checksum {
                    println!("[DNPFS CORRUPTION DETECTED] Read checksum mismatch for inode {}!", ino);
                    reply.error(EIO);
                    return;
                }
            }
        }

        reply.data(&buf);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("==================================================");
    println!("DNPFS — FUSE Driver v0.1");
    println!("==================================================");
    println!("Metadata Volume : {:?}", args.meta);
    println!("Data Volume     : {:?}", args.data);
    println!("Mount Point     : {:?}", args.mountpoint);

    let mut meta_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&args.meta)?;
    let mut data_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&args.data)?;

    meta_file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET))?;
    let mut sb_buf = [0u8; std::mem::size_of::<Superblock>()];
    meta_file.read_exact(&mut sb_buf)?;
    let sb: Superblock = *from_bytes(&sb_buf);

    if !sb.is_valid() {
        return Err("Invalid Superblock magic or checksum on metadata volume".into());
    }

    data_file.seek(SeekFrom::Start(0))?;
    let mut dh_buf = [0u8; std::mem::size_of::<DataHeader>()];
    data_file.read_exact(&mut dh_buf)?;
    let dh: DataHeader = *from_bytes(&dh_buf);

    if !dh.is_valid() {
        return Err("Invalid DataHeader magic or checksum on data volume".into());
    }

    if sb.uuid_data != dh.uuid_data || sb.uuid_meta != dh.uuid_meta {
        return Err("Metadata and Data volume UUID pairing mismatch!".into());
    }

    println!("Volume pairing verified successfully!");

    let fs = DnpfsFs {
        inner: Mutex::new(DnpfsFsInner {
            meta_file,
            data_file,
            superblock: sb,
            data_header: dh,
            next_inode_id: 2,
            next_data_block: 1,
        }),
    };

    let options = vec![
        MountOption::RW,
        MountOption::FSName("dnpfs".to_string()),
    ];

    println!("Mounting DNPFS at {:?}...", args.mountpoint);
    fuser::mount2(fs, &args.mountpoint, &options)?;

    Ok(())
}
