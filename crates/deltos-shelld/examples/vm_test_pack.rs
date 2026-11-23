//! TEST ONLY, for the emulated box (`system/tests/vm`): make a throwaway
//! pack signing key and two signed editions of a small pack, so the VM can
//! install, update, roll back and remove a pack through shelld as a real
//! box will. Never part of an image: a real box trusts only the project's
//! key (Track F §8), which nobody has made yet.
//!
//!   vm_test_pack OUTDIR
//!     OUTDIR/pack.pub            the throwaway public key
//!     OUTDIR/atlas.wax(.minisig) edition 2026.09.1
//!     OUTDIR/atlas-2.wax(.minisig) edition 2026.10.1, appended to the first
//!     OUTDIR/unsigned.wax        another pack, with no signature

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wax_core::writer::{EntryMeta, WaxWriter};

pub const ATLAS: [u8; 16] = [0x20, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2];
pub const OTHER: [u8; 16] = [0x30, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 3];

fn manifest(version: &str) -> BTreeMap<String, String> {
    [
        ("name", "Atlas"),
        ("icon", "icon.svg"),
        ("category", "reference"),
        ("license", "CC0-1.0"),
        ("attribution", "DeltOS VM test"),
        ("version", version),
        ("min_ram_bytes", "67108864"),
        ("min_storage_bytes", "1048576"),
        ("arch", "any"),
        ("entry_point", "index.html"),
        ("guest_accessible", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn sign(p: &Path, key: &minisign::KeyPair) {
    let (digest, uuid, created_at) = wax_core::reader::signable_digest_of(p).unwrap();
    let u = deltos_shelld::packs::uuid_text(&uuid);
    let comment = format!("wax archive_uuid={u} created_at={created_at}");
    let sig = minisign::sign(Some(&key.pk), &key.sk, std::io::Cursor::new(digest.to_vec()), Some(&comment), None).unwrap();
    std::fs::write(format!("{}.minisig", p.display()), sig.into_string()).unwrap();
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("OUTDIR"));
    std::fs::create_dir_all(&out).unwrap();
    let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    std::fs::write(out.join("pack.pub"), key.pk.to_box().unwrap().into_string()).unwrap();

    let one = out.join("atlas.wax");
    let mut sw = WaxWriter::new(ATLAS).created_at(1_790_000_000).create(&one, &manifest("2026.09.1")).unwrap();
    for (p, b) in [("index.html", "<title>Atlas</title><p>Rivers and mountains.</p>"), ("icon.svg", "<svg/>")] {
        sw.add_entry(EntryMeta::new(p).mime("text/html"), &mut b.as_bytes()).unwrap();
    }
    sw.finish().unwrap();
    sign(&one, &key);

    let two = out.join("atlas-2.wax");
    std::fs::copy(&one, &two).unwrap();
    let mut sw = WaxWriter::new(ATLAS).created_at(1_790_100_000).open_append(&two).unwrap();
    sw.set_manifest(&manifest("2026.10.1"));
    sw.add_entry(EntryMeta::new("Lakes.html").mime("text/html"), &mut &b"<p>Lakes.</p>"[..]).unwrap();
    sw.finish().unwrap();
    sign(&two, &key);

    let other = out.join("unsigned.wax");
    let mut sw = WaxWriter::new(OTHER).created_at(1_790_000_000).create(&other, &manifest("2026.09.1")).unwrap();
    sw.add_entry(EntryMeta::new("index.html").mime("text/html"), &mut &b"<p>Unsigned.</p>"[..]).unwrap();
    sw.finish().unwrap();
}
