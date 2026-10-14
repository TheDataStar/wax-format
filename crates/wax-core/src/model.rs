//! Shared value types for entries and content (SPEC §3, §5.2).

use crate::{Result, WaxError};

/// Compression codec for an entry blob (SPEC §3, §12.6). An unrecognised
/// string in an archive is *not* mapped here; it is surfaced as
/// [`WaxError::UnknownCompression`] when the entry is read.
///
/// The codec is **per entry**, so one pack may mix all three — a v0.10 writer
/// is free to leave already-compressed media as `Zstd` or `None` and spend the
/// dictionary only where it pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Stored verbatim; `length == uncompressed_length`.
    None,
    /// A single complete Zstandard frame (RFC 8878).
    Zstd,
    /// A single Zstandard frame compressed against the pack's shared
    /// dictionary (SPEC v0.10 §3.1). Decoding requires that dictionary, which
    /// is stored once in segment 0.
    ///
    /// New in v0.10. A v0.9 reader encountering this value reports
    /// [`WaxError::UnknownCompression`] for that entry, which is the correct
    /// outcome: it genuinely cannot decode the bytes. Opening and listing such
    /// a pack still works, because §5.4's version tolerance is about the index,
    /// not the blobs.
    ZstdDict,
}

impl Compression {
    pub fn as_str(self) -> &'static str {
        match self {
            Compression::None => "none",
            Compression::Zstd => "zstd",
            Compression::ZstdDict => "zstd-dict",
        }
    }

    /// True when decoding this entry needs the pack's shared dictionary.
    pub fn needs_dictionary(self) -> bool {
        matches!(self, Compression::ZstdDict)
    }

    /// Parse the on-disk `compression` string for `path`. Unknown → error.
    pub fn parse(path: &str, value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Compression::None),
            "zstd" => Ok(Compression::Zstd),
            "zstd-dict" => Ok(Compression::ZstdDict),
            other => Err(WaxError::UnknownCompression {
                path: path.to_string(),
                value: other.to_string(),
            }),
        }
    }
}

/// One row of the `entries` table, exactly as stored in a single segment
/// (SPEC §5.2). This is the raw, pre-merge shape; the reader merges these
/// across the segment chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub title: Option<String>,
    pub offset: u64,
    pub length: u64,
    pub uncompressed_length: u64,
    pub mime: Option<String>,
    /// Raw string from disk; validated against [`Compression`] only at read time.
    pub compression: String,
    pub sha256: Option<[u8; 32]>,
    pub volume_id: i64,
    pub redirect_to: Option<String>,
}

impl Entry {
    pub fn is_redirect(&self) -> bool {
        self.redirect_to.is_some()
    }
}

/// A fully resolved entry (redirects followed at most one hop) plus the path it
/// was reached through. Returned by [`crate::reader::WaxReader::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The path the caller asked for.
    pub requested: String,
    /// The canonical entry that actually holds the bytes.
    pub entry: Entry,
}

// ---------------------------------------------------------------------------
// Writer-side input types
// ---------------------------------------------------------------------------

/// Content for one entry being written.
#[derive(Debug, Clone)]
pub enum EntryContent {
    /// Literal bytes; the writer applies `compression`.
    Data {
        bytes: Vec<u8>,
        compression: Compression,
    },
    /// An alias to another path. The writer flattens redirect chains so that
    /// `to` always names a non-redirect entry (SPEC §6.2).
    Redirect { to: String },
}

/// One entry handed to [`crate::writer::WaxWriter`].
#[derive(Debug, Clone)]
pub struct EntryInput {
    pub path: String,
    pub title: Option<String>,
    pub mime: Option<String>,
    pub content: EntryContent,
}

impl EntryInput {
    pub fn data(path: impl Into<String>, bytes: impl Into<Vec<u8>>, compression: Compression) -> Self {
        EntryInput {
            path: path.into(),
            title: None,
            mime: None,
            content: EntryContent::Data {
                bytes: bytes.into(),
                compression,
            },
        }
    }

    pub fn redirect(path: impl Into<String>, to: impl Into<String>) -> Self {
        EntryInput {
            path: path.into(),
            title: None,
            mime: None,
            content: EntryContent::Redirect { to: to.into() },
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }
}
