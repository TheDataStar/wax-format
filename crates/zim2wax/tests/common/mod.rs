//! Test support: a minimal ZIM *writer* so the suite can build synthetic
//! archives with exactly the shapes real fixtures lack (redirect cycles,
//! dangling redirects, audio entries, missing illustration, empty Creator).
//!
//! Writes format 6.1 (new namespace scheme) or 5.0 (legacy), one uncompressed
//! cluster, no checksum. Just enough for `zim` 0.5 to read it back.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[derive(Debug, Clone)]
pub enum Body {
    Content { mime: String, bytes: Vec<u8> },
    /// Redirect to the dirent with this `(namespace, url)`.
    Redirect { to_ns: u8, to_url: String },
}

#[derive(Debug, Clone)]
pub struct Dirent {
    pub ns: u8,
    pub url: String,
    pub title: String,
    pub body: Body,
}

pub struct ZimBuilder {
    pub major: u16,
    pub minor: u16,
    pub dirents: Vec<Dirent>,
    pub main_page: Option<(u8, String)>,
}

impl ZimBuilder {
    /// New-namespace (6.1) archive.
    pub fn new() -> Self {
        ZimBuilder {
            major: 6,
            minor: 1,
            dirents: Vec::new(),
            main_page: None,
        }
    }
    /// Legacy-namespace (5.0) archive.
    pub fn legacy() -> Self {
        ZimBuilder {
            major: 5,
            minor: 0,
            ..Self::new()
        }
    }

    pub fn content(mut self, ns: u8, url: &str, title: &str, mime: &str, bytes: impl AsRef<[u8]>) -> Self {
        self.dirents.push(Dirent {
            ns,
            url: url.to_string(),
            title: title.to_string(),
            body: Body::Content {
                mime: mime.to_string(),
                bytes: bytes.as_ref().to_vec(),
            },
        });
        self
    }
    pub fn html(self, ns: u8, url: &str, title: &str, html: &str) -> Self {
        self.content(ns, url, title, "text/html", html)
    }
    pub fn redirect(mut self, ns: u8, url: &str, title: &str, to_ns: u8, to_url: &str) -> Self {
        self.dirents.push(Dirent {
            ns,
            url: url.to_string(),
            title: title.to_string(),
            body: Body::Redirect {
                to_ns,
                to_url: to_url.to_string(),
            },
        });
        self
    }
    /// Set (or replace) a metadata entry — later calls override earlier ones,
    /// so `standard_meta().meta("Date", …)` does what it reads like.
    pub fn meta(mut self, key: &str, value: &str) -> Self {
        self.dirents.retain(|d| !(d.ns == b'M' && d.url == key));
        self.content(b'M', key, "", "text/plain", value)
    }
    /// Remove a metadata entry.
    pub fn without_meta(mut self, key: &str) -> Self {
        self.dirents.retain(|d| !(d.ns == b'M' && d.url == key));
        self
    }
    pub fn main_page(mut self, ns: u8, url: &str) -> Self {
        self.main_page = Some((ns, url.to_string()));
        self
    }
    /// The usual metadata a real ZIM carries. Tests override what they need.
    pub fn standard_meta(self) -> Self {
        self.meta("Title", "Synthetic Test Wiki")
            .meta("Date", "2026-09-05")
            .meta("Creator", "Test Creator")
            .meta("Publisher", "Test Publisher")
            .meta("Language", "eng")
            .meta("License", "CC-BY-SA-4.0")
            .meta("Description", "synthetic")
    }

    pub fn write_to(&self, path: &Path) {
        std::fs::write(path, self.build()).unwrap();
    }

    pub fn build(&self) -> Vec<u8> {
        // ---- sort dirents by (ns, url): the url pointer list must be ordered
        let mut order: Vec<usize> = (0..self.dirents.len()).collect();
        order.sort_by(|&a, &b| {
            let da = &self.dirents[a];
            let db = &self.dirents[b];
            (da.ns, da.url.as_str()).cmp(&(db.ns, db.url.as_str()))
        });
        let sorted: Vec<&Dirent> = order.iter().map(|&i| &self.dirents[i]).collect();
        let index_of = |ns: u8, url: &str| -> u32 {
            sorted
                .iter()
                .position(|d| d.ns == ns && d.url == url)
                .unwrap_or_else(|| panic!("redirect/main target {}/{} not in archive", ns as char, url))
                as u32
        };

        // ---- mime list
        let mut mimes: Vec<String> = Vec::new();
        for d in &sorted {
            if let Body::Content { mime, .. } = &d.body {
                if !mimes.contains(mime) {
                    mimes.push(mime.clone());
                }
            }
        }
        let mime_id = |m: &str| mimes.iter().position(|x| x == m).unwrap() as u16;

        // ---- one cluster holding every content blob, uncompressed
        let mut blobs: Vec<&[u8]> = Vec::new();
        let mut blob_index: BTreeMap<usize, u32> = BTreeMap::new(); // sorted idx -> blob no
        for (i, d) in sorted.iter().enumerate() {
            if let Body::Content { bytes, .. } = &d.body {
                blob_index.insert(i, blobs.len() as u32);
                blobs.push(bytes);
            }
        }
        let mut cluster: Vec<u8> = vec![0x01]; // compression = none, not extended
        let n = blobs.len() as u32;
        let offsets_len = 4 * (n + 1);
        let mut off = offsets_len;
        for b in &blobs {
            cluster.extend_from_slice(&off.to_le_bytes());
            off += b.len() as u32;
        }
        cluster.extend_from_slice(&off.to_le_bytes());
        for b in &blobs {
            cluster.extend_from_slice(b);
        }

        // ---- dirents
        let mut dirent_bytes: Vec<Vec<u8>> = Vec::new();
        for (i, d) in sorted.iter().enumerate() {
            let mut b = Vec::new();
            match &d.body {
                Body::Content { mime, .. } => {
                    b.extend_from_slice(&mime_id(mime).to_le_bytes());
                    b.push(0); // parameter len
                    b.push(d.ns);
                    b.extend_from_slice(&0u32.to_le_bytes()); // revision
                    b.extend_from_slice(&0u32.to_le_bytes()); // cluster 0
                    b.extend_from_slice(&blob_index[&i].to_le_bytes());
                }
                Body::Redirect { to_ns, to_url } => {
                    b.extend_from_slice(&0xFFFFu16.to_le_bytes());
                    b.push(0);
                    b.push(d.ns);
                    b.extend_from_slice(&0u32.to_le_bytes());
                    b.extend_from_slice(&index_of(*to_ns, to_url).to_le_bytes());
                }
            }
            b.extend_from_slice(d.url.as_bytes());
            b.push(0);
            b.extend_from_slice(d.title.as_bytes());
            b.push(0);
            dirent_bytes.push(b);
        }

        // ---- layout: header | mime list | dirents | url ptrs | title ptrs | cluster ptrs | cluster
        let header_len = 80usize;
        let mut mime_list = Vec::new();
        for m in &mimes {
            mime_list.extend_from_slice(m.as_bytes());
            mime_list.push(0);
        }
        mime_list.push(0);

        let mime_list_pos = header_len as u64;
        let dirents_pos = mime_list_pos + mime_list.len() as u64;
        let mut dirent_offsets: Vec<u64> = Vec::new();
        let mut cursor = dirents_pos;
        for b in &dirent_bytes {
            dirent_offsets.push(cursor);
            cursor += b.len() as u64;
        }
        let url_ptr_pos = cursor;
        cursor += 8 * sorted.len() as u64;
        let title_ptr_pos = cursor;
        cursor += 4 * sorted.len() as u64;
        let cluster_ptr_pos = cursor;
        cursor += 8;
        let cluster_pos = cursor;
        cursor += cluster.len() as u64;
        let checksum_pos = cursor;

        // title pointer list: indices into url list, sorted by (ns, title-or-url)
        let mut title_order: Vec<u32> = (0..sorted.len() as u32).collect();
        title_order.sort_by(|&a, &b| {
            let da = sorted[a as usize];
            let db = sorted[b as usize];
            let ta = if da.title.is_empty() { &da.url } else { &da.title };
            let tb = if db.title.is_empty() { &db.url } else { &db.title };
            (da.ns, ta.as_str()).cmp(&(db.ns, tb.as_str()))
        });

        let main = self
            .main_page
            .as_ref()
            .map(|(ns, url)| index_of(*ns, url))
            .unwrap_or(0xFFFF_FFFF);

        let mut out = Vec::new();
        out.extend_from_slice(&0x044D_495Au32.to_le_bytes());
        out.extend_from_slice(&self.major.to_le_bytes());
        out.extend_from_slice(&self.minor.to_le_bytes());
        out.extend_from_slice(b"SYNTHETIC-ZIM-UU");
        out.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // cluster count
        out.extend_from_slice(&url_ptr_pos.to_le_bytes());
        out.extend_from_slice(&title_ptr_pos.to_le_bytes());
        out.extend_from_slice(&cluster_ptr_pos.to_le_bytes());
        out.extend_from_slice(&mime_list_pos.to_le_bytes());
        out.extend_from_slice(&main.to_le_bytes());
        out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // layout page
        out.extend_from_slice(&checksum_pos.to_le_bytes());
        assert_eq!(out.len(), header_len);
        out.extend_from_slice(&mime_list);
        for b in &dirent_bytes {
            out.extend_from_slice(b);
        }
        for o in &dirent_offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        for t in &title_order {
            out.extend_from_slice(&t.to_le_bytes());
        }
        out.extend_from_slice(&cluster_pos.to_le_bytes());
        out.extend_from_slice(&cluster);
        // 16-byte MD5 slot at checksumPos (the reader requires the slot to
        // exist; verifying it is opt-in and never done here)
        assert_eq!(out.len() as u64, checksum_pos);
        out.extend_from_slice(&[0u8; 16]);
        out
    }
}

impl Default for ZimBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A scratch dir with a ZIM written into it.
pub struct Fx {
    pub dir: TempDir,
    pub zim: PathBuf,
    pub wax: PathBuf,
}

pub fn fixture(b: &ZimBuilder) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let zim = dir.path().join("in.zim");
    let wax = dir.path().join("out.wax");
    b.write_to(&zim);
    Fx { dir, zim, wax }
}

pub fn committed(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

pub fn opts() -> zim2wax::ConvertOptions {
    zim2wax::ConvertOptions {
        category: "reference".into(),
        min_hw_tier: "pi_zero_2w".into(),
        license_if_absent: None,
        attribution_if_absent: None,
        created_at: Some(1_789_000_000),
        archive_uuid: Some([
            0x4a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x46, 0x07, 0x8a, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ]),
        sign_key: None,
    }
}

pub fn report_json(wax: &Path) -> serde_json::Value {
    let p = wax_builder::BuildReport::path_for(wax);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

pub fn warning_count(v: &serde_json::Value, code: &str) -> u64 {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == code)
        .map(|w| w["count"].as_u64().unwrap())
        .unwrap_or(0)
}

/// The Contract §11 closed warning vocabulary. Every code in a report must be
/// one of these.
pub const CONTRACT_WARNING_CODES: [&str; 8] = [
    "unsupported_mimetype",
    "redirect_cycle",
    "redirect_dangling",
    "invalid_path",
    "reserved_prefix_collision",
    "license_operator_supplied",
    "attribution_operator_supplied",
    "icon_generated",
];

pub fn assert_only_contract_codes(v: &serde_json::Value) {
    for w in v["warnings"].as_array().unwrap() {
        let code = w["code"].as_str().unwrap();
        assert!(
            CONTRACT_WARNING_CODES.contains(&code),
            "report carries {code:?}, which is not in Contract §11's closed vocabulary"
        );
    }
}
