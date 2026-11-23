//! The app manifest format (docs/base-addons-admin.md §5).

use deltos_apps::{parse, ReadError};
use std::path::Path;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

const MINIMAL: &str = r#"
schema = 1
id = "notes-demo"
name = "Demo"
version = "1.0.0"
kind = "addon"
summary = "A demo."
licence = "MIT"
min_profile = "classroom"
depends = ["platform"]
[resources]
evidence = "measured"
min_ram_bytes = 1000
min_storage_bytes = 1000
arch = "any"
basis = "measured on an aarch64 2 GB box"
[run]
form = "compose"
entry = "compose.yaml"
listens = [{ port = 9000, proto = "tcp", bind = "loopback" }]
[launcher]
group = "create"
icon = "icon.svg"
[reach]
host = "demo"
requires = ["demo.use"]
[[defines]]
name = "demo.use"
summary = "Use the demo"
seed_bundles = ["student"]
[sign_in]
method = "adapter_api"
[health]
probe = "compose"
target = "demo"
[backup]
paths = ["/var/lib/deltos/apps/notes-demo/"]
"#;

fn problems(text: &str) -> Vec<String> {
    match parse(text) {
        Ok(_) => vec![],
        Err(ReadError::Invalid(p)) => p.into_iter().map(|p| p.to_string()).collect(),
        Err(e) => vec![e.to_string()],
    }
}

fn with(old: &str, new: &str) -> String {
    assert!(MINIMAL.contains(old), "fixture lacks {old:?}");
    MINIMAL.replacen(old, new, 1)
}

fn assert_problem(text: &str, needle: &str) {
    let p = problems(text);
    assert!(
        p.iter().any(|m| m.contains(needle)),
        "expected a problem containing {needle:?}, got {p:#?}"
    );
}

#[test]
fn every_shipped_manifest_is_valid() {
    let mut n = 0;
    for dir in ["system/apps", "crates/deltos-apps/addons"] {
        for e in std::fs::read_dir(root().join(dir)).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "toml") {
                let text = std::fs::read_to_string(&p).unwrap();
                parse(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                n += 1;
            }
        }
    }
    assert!(n >= 7, "found only {n} manifests");
}

#[test]
fn minimal_manifest_is_valid() {
    assert_eq!(problems(MINIMAL), Vec::<String>::new());
}

#[test]
fn unknown_key_is_an_error_not_a_pass_through() {
    let p = problems(&with(
        "summary = \"A demo.\"",
        "summary = \"A demo.\"\ncolour = \"red\"",
    ));
    assert!(p.iter().any(|m| m.contains("unknown field")), "{p:?}");
}

#[test]
fn an_open_floor_may_not_carry_a_number() {
    assert_problem(
        &with("evidence = \"measured\"", "evidence = \"open\""),
        "an unmeasured number is an invented one",
    );
}

#[test]
fn a_measured_floor_needs_numbers_and_a_basis() {
    let t = with("min_ram_bytes = 1000\n", "")
        .replace("basis = \"measured on an aarch64 2 GB box\"\n", "");
    assert_problem(&t, "resources.min_ram_bytes");
    assert_problem(&t, "resources.basis");
}

#[test]
fn omission_is_not_zero() {
    assert_problem(
        &with("min_ram_bytes = 1000", "min_ram_bytes = 0"),
        "omission is not zero",
    );
}

#[test]
fn add_ons_stay_on_loopback_and_get_no_capabilities() {
    assert_problem(
        &with("bind = \"loopback\"", "bind = \"lan\""),
        "listens on loopback only",
    );
    assert_problem(
        &with(
            "entry = \"compose.yaml\"",
            "entry = \"compose.yaml\"\ncapabilities = [\"CAP_NET_ADMIN\"]",
        ),
        "no Linux capabilities",
    );
    assert_problem(
        &with(
            "form = \"compose\"\nentry = \"compose.yaml\"",
            "form = \"systemd\"\nentry = \"demo.service\"",
        ),
        "only base services are systemd units",
    );
}

#[test]
fn compose_add_ons_need_the_platform() {
    assert_problem(
        &with("depends = [\"platform\"]", "depends = []"),
        "needs the 'platform'",
    );
}

#[test]
fn add_ons_never_run_on_kiosk() {
    assert_problem(
        &with("min_profile = \"classroom\"", "min_profile = \"kiosk\""),
        "Kiosk runs no Track G or Track H",
    );
}

#[test]
fn permissions_are_checked() {
    assert_problem(
        &with("seed_bundles = [\"student\"]", "seed_bundles = [\"admin\"]"),
        "admin holds every permission",
    );
    assert_problem(
        &with(
            "seed_bundles = [\"student\"]",
            "seed_bundles = [\"librarian\"]",
        ),
        "not a shipped bundle",
    );
    assert_problem(
        &with(
            "seed_bundles = [\"student\"]",
            "secure_only = true\nseed_bundles = [\"guest\"]",
        ),
        "guests never have a secure session",
    );
    assert_problem(
        &with("requires = [\"demo.use\"]", "requires = []"),
        "no open door",
    );
    assert_problem(
        &with("requires = [\"demo.use\"]", "requires = [\"Demo Use\"]"),
        "not a permission name",
    );
    assert_problem(
        &with("name = \"demo.use\"", "name = \"demo\""),
        "not a permission name",
    );
}

#[test]
fn hosts_are_checked() {
    assert_problem(&with("host = \"demo\"", "host = \"home\""), "reserved");
    assert_problem(
        &with("host = \"demo\"", "host = \"Demo.Site\""),
        "not a valid name",
    );
    assert_problem(
        &with("method = \"adapter_api\"", "method = \"webos\""),
        "host-only cookie",
    );
}

#[test]
fn backups_stay_in_the_state_tree() {
    assert_problem(
        &with(
            "paths = [\"/var/lib/deltos/apps/notes-demo/\"]",
            "paths = [\"/home/demo\"]",
        ),
        "one state tree",
    );
    assert_problem(
        &with(
            "paths = [\"/var/lib/deltos/apps/notes-demo/\"]",
            "paths = [\"/var/lib/deltos/../etc\"]",
        ),
        "one state tree",
    );
}

#[test]
fn ids_versions_and_schema() {
    assert_problem(&with("id = \"notes-demo\"", "id = \"Notes\""), "id:");
    assert_problem(&with("id = \"notes-demo\"", "id = \"notes-\""), "id:");
    assert_problem(
        &with("version = \"1.0.0\"", "version = \"1.0\""),
        "MAJOR.MINOR.PATCH",
    );
    assert_problem(&with("schema = 1", "schema = 2"), "schema 1");
    assert_problem(&with("licence = \"MIT\"", "licence = \" \""), "no licence");
}

#[test]
fn gpu_device_needs_a_gpu_declaration() {
    assert_problem(
        &with(
            "entry = \"compose.yaml\"",
            "entry = \"compose.yaml\"\ndevices = [\"gpu\"]",
        ),
        "declare gpu",
    );
}

#[test]
fn the_one_licence_check_decides_review() {
    let m = parse(MINIMAL).unwrap();
    assert!(!m.licence_review_required());
    let m = parse(&with("licence = \"MIT\"", "licence = \"AGPL-3.0-only\"")).unwrap();
    assert!(
        m.licence_review_required(),
        "AGPL is not on the contract §11 allowlist, so it goes to review"
    );
}
