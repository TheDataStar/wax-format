use crate::{WaxHeader, WAX_MAGIC};
use rusqlite::{Connection, OptionalExtension};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tempfile::NamedTempFile;
use thiserror::Error;
use zerocopy::FromBytes;

#[derive(Error, Debug)]
pub enum WaxError {
    #[error("IO/Compression Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Index Error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("Invalid Archive: Magic bytes mismatch")]
    InvalidMagic,
    #[error("File not found: {0}")]
    FileNotFound(String),
}

// A simple struct to return file metadata
#[derive(Debug)]
pub struct WaxEntry {
    pub path: String,
    pub mime_type: String,
    pub size: u64,
}

pub struct WaxReader {
    archive_file: File,
    index_conn: Connection,
    _temp_index: NamedTempFile,
}

impl WaxReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, WaxError> {
        let mut file = File::open(path)?;

        // Read Header
        let mut header_buffer = [0u8; 64];
        file.read_exact(&mut header_buffer)?;
        
        let header = WaxHeader::read_from(&header_buffer[..])
            .ok_or(std::io::Error::new(std::io::ErrorKind::InvalidData, "Header too short"))?;

        if header.magic != WAX_MAGIC {
            return Err(WaxError::InvalidMagic);
        }

        // Extract Index
        file.seek(SeekFrom::Start(header.index_offset))?;
        
        let mut temp_index = NamedTempFile::new()?;
        let mut index_reader = file.try_clone()?.take(header.index_length);
        std::io::copy(&mut index_reader, &mut temp_index)?;

        let conn = Connection::open(temp_index.path())?;

        Ok(Self {
            archive_file: file,
            index_conn: conn,
            _temp_index: temp_index,
        })
    }

    pub fn get_file_data(&mut self, path: &str) -> Result<Vec<u8>, WaxError> {
        let mut stmt = self.index_conn.prepare(
            "SELECT blob_offset, blob_length FROM files WHERE path = ?1"
        )?;

        let result: Option<(u64, u64)> = stmt.query_row([path], |row| {
            let off: u64 = row.get(0)?;
            let len: u64 = row.get(1)?;
            Ok((off, len))
        }).optional()?;

        let (offset, length) = match result {
            Some(r) => r,
            None => return Err(WaxError::FileNotFound(path.to_string())),
        };

        self.archive_file.seek(SeekFrom::Start(offset))?;

        let mut compressed_buffer = vec![0u8; length as usize];
        self.archive_file.read_exact(&mut compressed_buffer)?;

        let decompressed = zstd::stream::decode_all(&compressed_buffer[..])?;

        Ok(decompressed)
    }
    
    pub fn get_mime_type(&self, path: &str) -> Result<String, WaxError> {
        let mut stmt = self.index_conn.prepare(
            "SELECT mime_type FROM files WHERE path = ?1"
        )?;
        
        let mime_result: Option<Option<String>> = stmt.query_row([path], |row| {
            let m: Option<String> = row.get(0)?;
            Ok(m)
        }).optional()?; 

        let mime = mime_result.flatten().unwrap_or_else(|| "application/octet-stream".to_string());
        
        Ok(mime)
    }

    // NEW: Function to list all files
    pub fn list_files(&self) -> Result<Vec<WaxEntry>, WaxError> {
        let mut stmt = self.index_conn.prepare(
            "SELECT path, mime_type, original_size FROM files ORDER BY path ASC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(WaxEntry {
                path: row.get(0)?,
                mime_type: row.get::<_, Option<String>>(1)?.unwrap_or("unknown".to_string()),
                size: row.get(2)?,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }
}