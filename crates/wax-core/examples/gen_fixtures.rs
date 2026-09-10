//! Regenerate the committed A4 fixtures and fuzz seed corpora.
//!
//!   cargo run -p wax-core --example gen_fixtures
//!
//! Fixtures land in `crates/wax-core/tests/fixtures/`, seeds in `fuzz/corpus/`.
//! They are checked in so the conformance suite and `cargo fuzz` have stable
//! inputs without a build step; this binary just refreshes them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wax_core::writer::{build_segment_db_with_meta, SegRow};
use wax_core::{Compression, EntryInput, WaxWriter, HEADER_LEN};

const UUID: [u8; 16] = *b"WAXFIXTURE-0.9\0\0";
const T: u64 = 1_700_000_000;

fn main() -> std::io::Result<()> {
    let root = workspace_root();
    let fx = root.join("crates/wax-core/tests/fixtures");
    let corpus = root.join("fuzz/corpus");
    std::fs::create_dir_all(&fx)?;

    // --- fixture 1: minimal valid archive (one zero-byte entry) ------------
    let p = fx.join("minimal.wax");
    WaxWriter::new(UUID)
        .created_at(T)
        .build(&p, vec![EntryInput::data("index.html", b"<h1>ok</h1>".to_vec(), Compression::None)], &BTreeMap::new())
        .unwrap();
    println!("wrote {}", p.display());

    // --- fixture 2: redirect-chain archive (flattened at build) -----------
    let p = fx.join("redirect-chain.wax");
    let mut manifest = BTreeMap::new();
    manifest.insert("title".into(), "Redirect Fixture".into());
    WaxWriter::new(UUID)
        .created_at(T)
        .build(
            &p,
            vec![
                EntryInput::data("articles/canonical.html", b"CANON".to_vec(), Compression::Zstd)
                    .with_title("Canonical"),
                EntryInput::redirect("articles/old-name.html", "articles/canonical.html"),
                EntryInput::redirect("articles/older-name.html", "articles/old-name.html"),
                EntryInput::redirect("index.html", "articles/canonical.html"),
            ],
            &manifest,
        )
        .unwrap();
    println!("wrote {}", p.display());

    // --- fixture 3: multi-segment archive (base + 2 appends) -------------
    let p = fx.join("multi-segment.wax");
    let w = WaxWriter::new(UUID).created_at(T);
    w.build(
        &p,
        vec![
            EntryInput::data("a.txt", b"a-base".to_vec(), Compression::None),
            EntryInput::data("shared.txt", b"v1".to_vec(), Compression::None),
        ],
        &BTreeMap::new(),
    )
    .unwrap();
    w.append(&p, vec![EntryInput::data("shared.txt", b"v2".to_vec(), Compression::None)])
        .unwrap();
    w.append(&p, vec![EntryInput::data("b.txt", b"b-append".to_vec(), Compression::Zstd)])
        .unwrap();
    println!("wrote {} (3 segments)", p.display());

    // --- fixture 4: corrupt-signature archive ---------------------------
    // A valid archive plus a deliberately bogus detached minisign sidecar.
    // A7's verify path (not yet implemented) must reject this; today the
    // conformance suite asserts the archive's signable digest does not match
    // whatever the sidecar claims.
    let p = fx.join("corrupt-signature.wax");
    WaxWriter::new(UUID)
        .created_at(T)
        .build(&p, vec![EntryInput::data("page.html", b"signed content".to_vec(), Compression::None)], &BTreeMap::new())
        .unwrap();
    let sig = p.with_file_name("corrupt-signature.wax.minisig");
    std::fs::write(
        &sig,
        "untrusted comment: signature from a WAX fixture (DELIBERATELY INVALID)\n\
         RWTinvalidbase64signaturedatadefinitelynotrealAAAA==\n\
         trusted comment: uuid=5741584649585455 corrupted=yes\n\
         AAAAinvalidglobalsigAAAA==\n",
    )?;
    println!("wrote {} + {}", p.display(), sig.display());

    // --- fuzz seed corpora -----------------------------------------------
    // header_parse: a valid header, plus near-miss mutations.
    let hp = corpus.join("header-parse");
    std::fs::create_dir_all(&hp)?;
    let valid_header = {
        let mut b = vec![0u8; HEADER_LEN];
        b[..4].copy_from_slice(b"WAX1");
        b[5] = 9;
        b[32..40].copy_from_slice(&(HEADER_LEN as u64 + 16).to_le_bytes()); // index_offset
        b[40..48].copy_from_slice(&4096u64.to_le_bytes()); // index_length
        b[48..56].copy_from_slice(&16u64.to_le_bytes()); // blob_section_length
        b
    };
    write_seed(&hp, "valid", &valid_header)?;
    write_seed(&hp, "bad_magic", b"WAX2\x00\x09")?;
    write_seed(&hp, "short", &[0u8; 40])?;
    write_seed(&hp, "major1", {
        let mut b = valid_header.clone();
        b[4] = 1;
        &b.clone()
    })?;

    // index_loader: a real segment db + a truncation.
    let il = corpus.join("index-loader");
    std::fs::create_dir_all(&il)?;
    let seg = build_segment_db_with_meta(
        &[SegRow {
            path: "x".into(),
            title: None,
            offset: 128,
            length: 1,
            uncompressed_length: 1,
            mime: None,
            compression: "none".into(),
            sha256: None,
            volume_id: 0,
            redirect_to: None,
        }],
        &[
            ("format".into(), "wax-index-segment".into()),
            ("segment_index".into(), "0".into()),
            ("blob_region_offset".into(), "128".into()),
            ("blob_region_length".into(), "1".into()),
            ("created_at".into(), "0".into()),
        ],
        None,
    )
    .unwrap();
    write_seed(&il, "real_segment", &seg)?;
    write_seed(&il, "truncated_segment", &seg[..seg.len() / 3])?;
    write_seed(&il, "sqlite_header_only", b"SQLite format 3\x00")?;

    // segment_merge: compact model scripts (see wax_core::fuzz).
    let sm = corpus.join("segment-merge");
    std::fs::create_dir_all(&sm)?;
    write_seed(&sm, "two_segments_overlap", &[2, 1, b'a', 0, 1, b'a', 0])?;
    write_seed(&sm, "redirect", &[1, 1, b'a', 1, b'b'])?;
    write_seed(&sm, "self_redirect", &[1, 1, b'a', 1, b'a'])?;
    write_seed(&sm, "empty", &[0])?;

    println!("done");
    Ok(())
}

fn write_seed(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let p = dir.join(name);
    std::fs::write(&p, bytes)?;
    println!("  seed {}", p.display());
    Ok(())
}

fn workspace_root() -> PathBuf {
    // examples run with CWD = crate dir or workspace root depending on invocation;
    // CARGO_MANIFEST_DIR is crates/wax-core.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
