use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use wax_core::{reader::WaxReader, WaxHeader, WAX_MAGIC};
use rusqlite::Connection;
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use walkdir::WalkDir;
use zerocopy::AsBytes;
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Parser, Debug)]
#[command(author = "Neon Digital Systems", version = "1.1.0", about = "High-performance WAX Archive Builder")]
struct Args {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Compress a directory into a .wax archive
    Build {
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Read a specific file out of a .wax archive
    Read {
        #[arg(short, long)]
        archive: PathBuf,
        #[arg(short, long)]
        file: String,
    },
    /// List all files inside an archive
    Ls {
        #[arg(short, long)]
        archive: PathBuf,
    },
    /// Inspect archive metadata
    Inspect {
        #[arg(short, long)]
        archive: PathBuf,
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.cmd {
        Commands::Build { input, output } => build_archive(input, output),
        Commands::Read { archive, file } => read_file(archive, file),
        Commands::Ls { archive } => list_archive(archive),
        Commands::Inspect { archive } => inspect_archive(archive),
    }
}

fn build_archive(input: PathBuf, output: PathBuf) -> Result<()> {
    println!("Init: {:?}", output);

    let mut output_file = File::create(&output).context("Failed to create output file")?;
    output_file.write_all(&[0u8; 64])?; 

    let temp_db_path = output.with_extension("db.temp");
    if temp_db_path.exists() { fs::remove_file(&temp_db_path)?; }
    
    let conn = Connection::open(&temp_db_path)?;
    conn.execute(
        "CREATE TABLE files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            mime_type TEXT,
            blob_offset INTEGER,
            blob_length INTEGER,
            original_size INTEGER
        )",
        [],
    )?;

    println!("Scanning files...");
    let mut files_to_process = Vec::new();
    for entry in WalkDir::new(&input) {
        let entry = entry?;
        if entry.path().is_file() {
            files_to_process.push(entry.into_path());
        }
    }

    let bar = ProgressBar::new(files_to_process.len() as u64);
    bar.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    let mut stmt = conn.prepare(
        "INSERT INTO files (path, mime_type, blob_offset, blob_length, original_size) 
         VALUES (?1, ?2, ?3, ?4, ?5)"
    )?;

    let mut current_offset = 64u64;

    for path in files_to_process {
        let relative_path = path.strip_prefix(&input)?.to_string_lossy().to_string();
        let normalized_path = relative_path.replace("\\", "/");

        let mime = mime_guess::from_path(&path).first_or_octet_stream();
        let raw_data = fs::read(&path)?;
        let original_size = raw_data.len() as u64;

        let compressed_data = zstd::stream::encode_all(&raw_data[..], 3)?;
        let blob_length = compressed_data.len() as u64;

        output_file.write_all(&compressed_data)?;

        stmt.execute((
            normalized_path,
            mime.as_ref(),
            current_offset,
            blob_length,
            original_size,
        ))?;

        current_offset += blob_length;
        bar.inc(1);
    }
    bar.finish_with_message("Compression Complete");
    drop(stmt);

    println!("Finalizing Index...");
    let index_start_offset = current_offset;
    conn.close().map_err(|(_, e)| e)?;

    let mut db_file = File::open(&temp_db_path)?;
    let index_length = std::io::copy(&mut db_file, &mut output_file)?;

    output_file.seek(SeekFrom::Start(0))?;
    let header = WaxHeader {
        magic: WAX_MAGIC,
        version: 1,
        uuid: [0; 16],
        index_offset: index_start_offset,
        index_length: index_length,
        compression_type: 1,
        padding: [0; 23],
    };

    output_file.write_all(header.as_bytes())?;
    fs::remove_file(temp_db_path)?;
    
    println!("Success! Archive Ready.");
    Ok(())
}

fn read_file(archive: PathBuf, file_path: String) -> Result<()> {
    let mut reader = WaxReader::open(&archive)?;
    
    match reader.get_file_data(&file_path) {
        Ok(data) => {
            let mime = reader.get_mime_type(&file_path)?;
            println!("Found: {} ({} bytes, {})", file_path, data.len(), mime);
            
            if let Ok(text) = String::from_utf8(data) {
                println!("\n{}", text);
            } else {
                println!("(Binary Data)");
            }
        }
        Err(e) => println!("Error: {}", e),
    }
    Ok(())
}

fn list_archive(archive: PathBuf) -> Result<()> {
    let reader = WaxReader::open(&archive)?;
    let files = reader.list_files()?;
    
    println!("{:<50} | {:<20} | {:<10}", "PATH", "MIME", "SIZE");
    println!("{:-<50}-|-{:-<20}-|-{:-<10}", "", "", "");
    
    for file in files {
        println!("{:<50} | {:<20} | {:<10}", file.path, file.mime_type, file.size);
    }
    Ok(())
}

fn inspect_archive(archive: PathBuf) -> Result<()> {
    // Just opening it is a validity check
    let _reader = WaxReader::open(&archive)?;
    println!("Status: VALID WAX Archive");
    Ok(())
}