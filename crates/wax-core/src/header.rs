//! The fixed 128-byte WAX header (SPEC §2).
//!
//! All multi-byte integers are little-endian (SPEC §0.1, §12.1). Parsing is
//! manual (no `repr(C)` transmute) so there are no alignment/padding concerns
//! and no way for malformed input to do anything but return an error.

use crate::{Result, WaxError, HEADER_LEN, MIN_SQLITE_LEN, WAX_MAGIC};

/// `flags` bit positions (SPEC §2.1).
pub mod flag {
    pub const HAS_SEARCH_INDEX: u16 = 1 << 0;
    pub const HAS_DELTA_BASE: u16 = 1 << 1;
    pub const IS_MULTI_VOLUME: u16 = 1 << 2;
    pub const IS_SIGNED: u16 = 1 << 3;
    /// Bits the reader must ignore but the writer must keep zero.
    pub const RESERVED_MASK: u16 = !(HAS_SEARCH_INDEX | HAS_DELTA_BASE | IS_MULTI_VOLUME | IS_SIGNED);
}

/// Parsed header. Field order and offsets match SPEC §2 exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaxHeader {
    pub version_major: u8,
    pub version_minor: u8,
    pub flags: u16,
    pub archive_uuid: [u8; 16],
    pub created_at: u64,
    pub index_offset: u64,
    pub index_length: u64,
    pub blob_section_length: u64,
    pub search_index_offset: u64,
    pub search_index_length: u64,
}

impl Default for WaxHeader {
    fn default() -> Self {
        WaxHeader {
            version_major: crate::FORMAT_VERSION_MAJOR,
            version_minor: crate::FORMAT_VERSION_MINOR,
            flags: 0,
            archive_uuid: [0; 16],
            created_at: 0,
            index_offset: 0,
            index_length: 0,
            blob_section_length: 0,
            search_index_offset: 0,
            search_index_length: 0,
        }
    }
}

impl WaxHeader {
    /// Parse a header from the first [`HEADER_LEN`] bytes of `buf`.
    ///
    /// Checks only the two things that make the byte range *un-parseable*:
    /// length and magic. Semantic validation (versions, bounds) is
    /// [`WaxHeader::validate`], done once the file size is known.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_LEN {
            return Err(WaxError::TruncatedHeader {
                found: buf.len() as u64,
            });
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&buf[0..4]);
        if magic != WAX_MAGIC {
            return Err(WaxError::BadMagic { found: magic });
        }

        let u64_at = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[off..off + 8]);
            u64::from_le_bytes(b)
        };
        let mut archive_uuid = [0u8; 16];
        archive_uuid.copy_from_slice(&buf[8..24]);

        Ok(WaxHeader {
            version_major: buf[4],
            version_minor: buf[5],
            flags: u16::from_le_bytes([buf[6], buf[7]]),
            archive_uuid,
            created_at: u64_at(24),
            index_offset: u64_at(32),
            index_length: u64_at(40),
            blob_section_length: u64_at(48),
            search_index_offset: u64_at(56),
            search_index_length: u64_at(64),
        })
    }

    /// Semantic validation against the real file size (SPEC §2.2 steps 3–6).
    /// Step 7 (blob-section consistency) needs the segment chain and lives in
    /// the reader.
    pub fn validate(&self, file_size: u64) -> Result<()> {
        if file_size < HEADER_LEN as u64 {
            return Err(WaxError::TruncatedHeader { found: file_size });
        }
        if self.version_major != crate::FORMAT_VERSION_MAJOR {
            return Err(WaxError::UnsupportedMajorVersion {
                found: self.version_major,
            });
        }
        if self.index_length < MIN_SQLITE_LEN {
            return Err(WaxError::IndexTooSmall {
                found: self.index_length,
            });
        }
        if self.index_offset < HEADER_LEN as u64 {
            return Err(WaxError::IndexOffsetInHeader {
                found: self.index_offset,
            });
        }
        match self.index_offset.checked_add(self.index_length) {
            Some(end) if end <= file_size => {}
            _ => {
                return Err(WaxError::IndexOutOfBounds {
                    offset: self.index_offset,
                    length: self.index_length,
                    file_size,
                })
            }
        }
        Ok(())
    }

    /// Serialize to the canonical 128-byte form (SPEC §2). `reserved` is
    /// zero-filled; unknown `flags` bits are cleared.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&WAX_MAGIC);
        out[4] = self.version_major;
        out[5] = self.version_minor;
        out[6..8].copy_from_slice(&(self.flags & !flag::RESERVED_MASK).to_le_bytes());
        out[8..24].copy_from_slice(&self.archive_uuid);
        out[24..32].copy_from_slice(&self.created_at.to_le_bytes());
        out[32..40].copy_from_slice(&self.index_offset.to_le_bytes());
        out[40..48].copy_from_slice(&self.index_length.to_le_bytes());
        out[48..56].copy_from_slice(&self.blob_section_length.to_le_bytes());
        out[56..64].copy_from_slice(&self.search_index_offset.to_le_bytes());
        out[64..72].copy_from_slice(&self.search_index_length.to_le_bytes());
        // 72..128 stay zero (reserved).
        out
    }

    pub fn has_flag(&self, bit: u16) -> bool {
        self.flags & bit != 0
    }
}
