//! A pack built before the measured-resource migration must still open.
//!
//! The fixtures here are **real artifacts**, not hand-written test data: both
//! were produced by `wax-builder build` as it stood immediately before the
//! migration, so they carry a genuine `min_hw_tier` row written by the code
//! that owned that field. Nothing built before this change may become
//! unopenable (Directive 03 §2).

use wax_builder::config::{
    resolve_hw_requirement, MIN_SPEC_RAM, MIN_SPEC_STORAGE,
};
use wax_core::WaxReader;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn a_pack_built_before_the_migration_still_opens() {
    for name in ["legacy-tier-pi_zero_2w.wax", "legacy-tier-pi_5.wax"] {
        let r = WaxReader::open(fixture(name)).unwrap_or_else(|e| {
            panic!("{name} must still open after the migration, got: {e}")
        });
        // and its content is still readable, not merely its header
        let ep = r.manifest().get("entry_point").cloned().unwrap();
        let html = String::from_utf8(r.read(&ep).unwrap()).unwrap();
        assert!(html.contains("Legacy pack"), "{name}: entry point should read back");
    }
}

#[test]
fn a_legacy_tier_resolves_to_a_supported_floor() {
    let r = WaxReader::open(fixture("legacy-tier-pi_zero_2w.wax")).unwrap();
    let m = r.manifest();

    // The pack declares the retired field and none of the new ones.
    assert_eq!(m.get("min_hw_tier").map(String::as_str), Some("pi_zero_2w"));
    assert!(m.get("min_ram_bytes").is_none(), "a legacy pack declares no resources");

    let hw = resolve_hw_requirement(m).expect("a legacy tier must resolve");
    assert_eq!(hw.from_legacy_tier.as_deref(), Some("pi_zero_2w"));
    // The board is retired as a target; the pack does not become unrunnable
    // because the device it named is gone (Directive 03 §3).
    assert_eq!(hw.min_ram_bytes, MIN_SPEC_RAM);
    assert_eq!(hw.min_storage_bytes, MIN_SPEC_STORAGE);
    assert_eq!(hw.arch, "any", "only the numbers were ever the requirement");
    assert!(!hw.weakened, "pi_zero_2w mapped up to the floor, so nothing was lost");
}

#[test]
fn a_legacy_pi_5_reports_that_the_mapping_lost_a_guarantee() {
    let r = WaxReader::open(fixture("legacy-tier-pi_5.wax")).unwrap();
    let hw = resolve_hw_requirement(r.manifest()).expect("a legacy tier must resolve");

    assert_eq!(hw.from_legacy_tier.as_deref(), Some("pi_5"));
    assert_eq!(hw.min_ram_bytes, MIN_SPEC_RAM);
    // The point of this test: the resolution is a WIDENING, and it says so.
    // `pi_5` guaranteed more than the minimum spec, and the resource point it
    // carried is stated in no current document — so the loss is surfaced
    // rather than silently absorbed.
    assert!(
        hw.weakened,
        "mapping pi_5 down to the minimum spec loses a guarantee and must be reported"
    );
}

#[test]
fn a_declared_requirement_wins_over_any_legacy_field() {
    // Belt and braces: if both are somehow present, the declared fields are
    // authored and the tier is derived, so the declared ones win.
    let mut m = std::collections::BTreeMap::new();
    m.insert("min_hw_tier".to_string(), "mini_pc".to_string());
    m.insert("min_ram_bytes".to_string(), "1073741824".to_string());
    m.insert("min_storage_bytes".to_string(), "2147483648".to_string());
    m.insert("arch".to_string(), "aarch64".to_string());

    let hw = resolve_hw_requirement(&m).unwrap();
    assert_eq!(hw.min_ram_bytes, 1_073_741_824);
    assert_eq!(hw.arch, "aarch64");
    assert!(hw.from_legacy_tier.is_none(), "declared beats derived");
}
