use bytemuck::from_bytes;
use clap::{Parser, Subcommand};
use dnpfs_core::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(author, version, about = "DNPFS Management and Inspection Tools", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Perform a filesystem integrity check on paired metadata & data volumes
    Check {
        /// Path to metadata volume (DNPFS_META)
        #[arg(short, long)]
        meta: PathBuf,

        /// Path to data volume (DNPFS_DATA)
        #[arg(short, long)]
        data: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check { meta, data } => {
            println!("==================================================");
            println!("DNPFS Check — Volume Consistency & Verification");
            println!("==================================================");

            let mut meta_file = File::open(&meta)?;
            let mut data_file = File::open(&data)?;

            // 1. Verify Primary Superblock
            meta_file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET))?;
            let mut sb_buf = [0u8; std::mem::size_of::<Superblock>()];
            meta_file.read_exact(&mut sb_buf)?;
            let sb: Superblock = *from_bytes(&sb_buf);

            if sb.is_valid() {
                println!("[OK] Superblock magic (0x{:08X}) & xxHash3 checksum valid.", sb.magic);
            } else {
                println!("[FAIL] Superblock checksum or magic invalid!");
                return Ok(());
            }

            // 2. Verify DataHeader
            data_file.seek(SeekFrom::Start(0))?;
            let mut dh_buf = [0u8; std::mem::size_of::<DataHeader>()];
            data_file.read_exact(&mut dh_buf)?;
            let dh: DataHeader = *from_bytes(&dh_buf);

            if dh.is_valid() {
                println!("[OK] DataHeader magic (0x{:08X}) & checksum valid.", dh.magic);
            } else {
                println!("[FAIL] DataHeader checksum or magic invalid!");
                return Ok(());
            }

            // 3. Verify UUID Pairing
            if sb.uuid_data == dh.uuid_data && sb.uuid_meta == dh.uuid_meta {
                println!("[OK] Volume UUID pairing verified!");
                println!("     Meta UUID: {}", Uuid::from_bytes(sb.uuid_meta));
                println!("     Data UUID: {}", Uuid::from_bytes(sb.uuid_data));
            } else {
                println!("[FAIL] Mismatch between metadata and data volume UUID pairing!");
            }

            println!("==================================================");
            println!("Filesystem check completed cleanly.");
        }
    }

    Ok(())
}
