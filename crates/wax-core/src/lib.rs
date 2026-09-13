//! `wax-core` — reader/writer for the WAX archive format.
//!
//! The on-disk contract is [`SPEC.md`](../../../SPEC.md) (WAX format **v0.9**).
//! This crate implements:
//!
//! * [`header`] — the fixed 128-byte header (parse / validate / serialize).
//! * [`segment`] — one embedded SQLite index segment (`entries`, `segment_meta`, …).
//! * [`reader`] — [`reader::WaxReader`]: open an archive, walk the segment chain,
//!   merge entries (last-segment-wins), resolve one-hop redirects, verify content.
//! * [`writer`] — [`writer::WaxWriter`]: assemble a valid archive; append a segment
//!   following the spec's append-commit protocol (§7). v0.9 builders emit a single
//!   segment; append exists for the general shape and for the A4 conformance suite.
//! * [`fuzz`] — panic-free entry points for the three A4 fuzz targets, also run
//!   as ordinary tests where libFuzzer is unavailable (SPEC §11.3, §12.15).
//!
//! Nothing here ever panics on malformed archive input; every failure path is a
//! [`WaxError`].

pub mod fuzz;
pub mod header;
pub mod model;
pub mod reader;
pub mod segment;
pub mod writer;

pub use header::WaxHeader;
pub use model::{Compression, Entry, EntryContent, EntryInput, Resolved};
pub use reader::WaxReader;
pub use writer::{EntryMeta, EntryStats, FinishStats, StreamingWriter, WaxWriter};

/// Magic bytes at offset 0: ASCII `"WAX1"`.
pub const WAX_MAGIC: [u8; 4] = *b"WAX1";

/// Fixed header size in bytes (SPEC §2).
pub const HEADER_LEN: usize = 128;

/// Format version implemented by this crate (SPEC §10).
pub const FORMAT_VERSION_MAJOR: u8 = 0;
/// Format minor version implemented by this crate.
pub const FORMAT_VERSION_MINOR: u8 = 9;

/// Smallest possible SQLite database (one 512-byte page). Used as the lower
/// bound for `index_length` and `prev_segment_length` (SPEC §2.2, §5.1).
pub const MIN_SQLITE_LEN: u64 = 512;

/// Hard cap on segment-chain length before the reader gives up (SPEC §5.1).
pub const MAX_SEGMENTS: usize = 64;

/// Error groups mirror SPEC §9. The conformance suite asserts on the *variant*,
/// not on the `Display` string.
#[derive(Debug, thiserror::Error)]
pub enum WaxError {
    // --- Header (rejected at open) -----------------------------------------
    #[error("bad magic: expected 'WAX1', found {found:02x?}")]
    BadMagic { found: [u8; 4] },
    #[error("truncated header: need 128 bytes, file has {found}")]
    TruncatedHeader { found: u64 },
    #[error("unsupported major version {found} (this build implements format major 0)")]
    UnsupportedMajorVersion { found: u8 },
    #[error("index_length {found} is smaller than the minimum SQLite database (512)")]
    IndexTooSmall { found: u64 },
    #[error("header index_offset {found} is inside the header region (< 128)")]
    IndexOffsetInHeader { found: u64 },
    #[error("index segment [{offset}, {offset}+{length}) lies outside the file ({file_size} bytes)")]
    IndexOutOfBounds {
        offset: u64,
        length: u64,
        file_size: u64,
    },

    // --- Blob section (rejected at open) ----------------------------------
    #[error("blob_section_length mismatch: {detail}")]
    BlobSectionLengthMismatch { detail: String },

    // --- Segment / chain (rejected at open) ------------------------------
    #[error("segment at offset {offset} is not a WAX index segment ({reason})")]
    NotAnIndexSegment { offset: u64, reason: String },
    #[error("prev_segment [{offset}, {offset}+{length}) is out of bounds for the chain")]
    PrevSegmentOutOfBounds { offset: u64, length: u64 },
    #[error("segment chain contains a cycle at offset {offset}")]
    SegmentChainCycle { offset: u64 },
    #[error("segment chain longer than the 64-segment limit")]
    TooManySegments,
    #[error("broken segment chain: {detail}")]
    BrokenSegmentChain { detail: String },

    // --- Schema (rejected at open) --------------------------------------
    #[error("index segment schema error: {detail}")]
    Schema { detail: String },
    #[error("entry {path:?} has volume_id {found}, but v0.9 archives are single-volume (must be 0)")]
    UnexpectedVolumeId { path: String, found: i64 },

    // --- Lookup (returned at get / resolve) ----------------------------
    #[error("entry not found: {0:?}")]
    EntryNotFound(String),
    #[error("redirect from {from:?} points at {to:?}, which does not exist")]
    DanglingRedirect { from: String, to: String },
    #[error("redirect from {from:?} via {via:?} is deeper than one hop (chain not flattened)")]
    RedirectChainTooDeep { from: String, via: String },

    // --- Content (returned at read) -----------------------------------
    #[error("entry {path:?} uses unknown compression {value:?}")]
    UnknownCompression { path: String, value: String },
    #[error("entry {path:?} failed to decompress: {detail}")]
    Decompress { path: String, detail: String },
    #[error("entry {path:?} content does not match its recorded sha256")]
    ChecksumMismatch { path: String },

    // --- Signature (returned by verify path) --------------------------
    #[error("signature sidecar is missing")]
    SignatureMissing,
    #[error("signature does not verify")]
    SignatureInvalid,
    #[error("signature sidecar is for a different archive (uuid mismatch)")]
    SignatureArchiveMismatch,

    // --- Plumbing ---------------------------------------------------
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, WaxError>;
