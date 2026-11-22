//! The fit check (docs/base-addons-admin.md §6).

use deltos_apps::fit::{check, Blocker, Capability, Sum, Verdict};
use deltos_apps::{parse, read_dir, AppManifest};
use std::path::Path;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn cap(json: &str) -> Capability {
    serde_json::from_str(json).unwrap()
}

/// The contract §9 example record, at the minimum spec.
fn min_spec() -> Capability {
    cap(
        r#"{ "profile": "classroom", "ram_bytes": 2147483648, "storage_bytes": 34359738368,
             "arch": "aarch64", "gpu": "none", "has_wifi": true, "can_ap": true, "has_rtc": false,
             "os_ram_bytes": 268435456 }"#,
    )
}

/// The record exactly as contract §9 shows it today: no OS measurement.
fn min_spec_unmeasured_os() -> Capability {
    cap(
        r#"{ "profile": "classroom", "ram_bytes": 2147483648, "storage_bytes": 34359738368,
             "arch": "aarch64", "gpu": "none", "has_wifi": true, "can_ap": true, "has_rtc": false }"#,
    )
}

fn base() -> Vec<AppManifest> {
    read_dir(&root().join("system/apps")).unwrap()
}

fn addon(name: &str) -> AppManifest {
    parse(&std::fs::read_to_string(root().join("crates/deltos-apps/addons").join(name)).unwrap())
        .unwrap()
}

/// A base set whose floors are all known, for the arithmetic.
fn measured_base(ram: u64) -> Vec<AppManifest> {
    vec![parse(&format!(
        r#"schema = 1
id = "home"
name = "Home"
version = "0.1.0"
kind = "base"
summary = "Test."
licence = "MIT"
min_profile = "kiosk"
[resources]
evidence = "measured"
min_ram_bytes = {ram}
min_storage_bytes = 1
arch = "any"
basis = "test"
[run]
form = "systemd"
entry = "home.service"
listens = [{{ port = 80, proto = "tcp", bind = "lan" }}]
[sign_in]
method = "webos"
[health]
probe = "systemd"
target = "home.service"
[backup]
paths = []
"#
    ))
    .unwrap()]
}

const GIB: u64 = 1 << 30;

#[test]
fn open_base_floors_mean_cannot_tell() {
    let f = check(&min_spec(), &base(), &[addon("platform.toml")], 20 * GIB);
    assert_eq!(f.verdict, Verdict::CannotTell, "{f:#?}");
    match &f.ram {
        Sum::Unknown { unmeasured, .. } => {
            assert!(
                unmeasured.contains(&"dns-dhcp".to_string())
                    && unmeasured.contains(&"hotspot".to_string())
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn provisional_floors_fit_provisionally() {
    let f = check(
        &min_spec(),
        &measured_base(400 << 20),
        &[addon("platform.toml")],
        20 * GIB,
    );
    assert_eq!(f.verdict, Verdict::FitsProvisionally, "{f:#?}");
    assert_eq!(
        f.ram,
        Sum::Known {
            needed: (256 << 20) + (400 << 20) + (768 << 20),
            have: 2 * GIB,
            trust: deltos_apps::fit::Trust::Provisional
        }
    );
}

#[test]
fn an_unmeasured_operating_system_is_named() {
    let f = check(
        &min_spec_unmeasured_os(),
        &measured_base(400 << 20),
        &[addon("platform.toml")],
        20 * GIB,
    );
    assert_eq!(f.verdict, Verdict::CannotTell);
    assert!(
        matches!(&f.ram, Sum::Unknown { unmeasured, .. } if unmeasured == &vec![deltos_apps::fit::OS.to_string()]),
        "{:?}",
        f.ram
    );
}

#[test]
fn over_budget_is_refused_even_with_open_floors() {
    // 256 MiB OS, 1,100 MiB of measured base and the 768 MiB platform is already over 2 GiB,
    // whatever the open floors turn out to be.
    let mut inst = measured_base(1100 << 20);
    inst.push(base().into_iter().find(|a| a.id == "hotspot").unwrap());
    let f = check(&min_spec(), &inst, &[addon("platform.toml")], 20 * GIB);
    assert_eq!(f.verdict, Verdict::DoesNotFit);
    assert!(f.blockers.contains(&Blocker::Ram));
}

#[test]
fn storage_counts_the_set_against_free_space() {
    let f = check(
        &min_spec(),
        &measured_base(1 << 20),
        &[addon("platform.toml")],
        4 * GIB,
    );
    assert!(f.blockers.contains(&Blocker::Storage), "{f:#?}");
}

#[test]
fn a_missing_dependency_blocks_and_installing_it_together_does_not() {
    let f = check(
        &min_spec(),
        &measured_base(1 << 20),
        &[addon("media-hub.toml")],
        20 * GIB,
    );
    assert!(f.blockers.contains(&Blocker::MissingDependency {
        app: "media-hub".into(),
        needs: "platform".into()
    }));
    let f = check(
        &min_spec(),
        &measured_base(1 << 20),
        &[addon("platform.toml"), addon("media-hub.toml")],
        20 * GIB,
    );
    assert!(f.blockers.is_empty(), "{f:#?}");
    assert_eq!(f.verdict, Verdict::CannotTell, "media-hub's floor is open");
}

#[test]
fn profile_arch_and_gpu_are_compared() {
    let kiosk = cap(
        r#"{ "profile": "kiosk", "ram_bytes": 2147483648, "storage_bytes": 34359738368, "arch": "aarch64", "gpu": "none" }"#,
    );
    let f = check(
        &kiosk,
        &measured_base(1 << 20),
        &[addon("platform.toml")],
        20 * GIB,
    );
    assert!(matches!(f.blockers[0], Blocker::Profile { .. }), "{f:#?}");

    let mail = addon("mail.toml");
    let f = check(
        &min_spec(),
        &measured_base(1 << 20),
        &[addon("platform.toml"), mail],
        20 * GIB,
    );
    assert!(
        f.blockers
            .iter()
            .any(|b| matches!(b, Blocker::Profile { app, .. } if app == "mail")),
        "mail needs community_hub"
    );

    let mut x86 = addon("platform.toml");
    x86.resources.arch = deltos_apps::manifest::Arch::X86_64;
    let f = check(&min_spec(), &measured_base(1 << 20), &[x86], 20 * GIB);
    assert!(f.blockers.iter().any(|b| matches!(b, Blocker::Arch { .. })));

    let mut gpu = addon("platform.toml");
    gpu.resources.gpu = Some(deltos_apps::manifest::Gpu::Required);
    let f = check(
        &min_spec(),
        &measured_base(1 << 20),
        &[gpu.clone()],
        20 * GIB,
    );
    assert!(f.blockers.contains(&Blocker::Gpu {
        app: "platform".into()
    }));
    // `preferred` never gates (contract §2.3).
    gpu.resources.gpu = Some(deltos_apps::manifest::Gpu::Preferred);
    let f = check(&min_spec(), &measured_base(1 << 20), &[gpu], 20 * GIB);
    assert!(f.blockers.is_empty());
}

#[test]
fn collisions_are_found() {
    let mut a = addon("platform.toml");
    a.run.listens[0].port = 80; // the base's own port
    let f = check(&min_spec(), &measured_base(1 << 20), &[a], 20 * GIB);
    assert!(
        f.blockers
            .iter()
            .any(|b| matches!(b, Blocker::Port { port: 80, .. })),
        "{f:#?}"
    );

    let mut twin = addon("media-hub.toml");
    twin.id = "media-two".into();
    let f = check(
        &min_spec(),
        &measured_base(1 << 20),
        &[addon("platform.toml"), addon("media-hub.toml"), twin],
        20 * GIB,
    );
    assert!(f
        .blockers
        .iter()
        .any(|b| matches!(b, Blocker::Host { host, .. } if host == "media")));
    assert!(f
        .blockers
        .iter()
        .any(|b| matches!(b, Blocker::Permission { name, .. } if name == "media.watch")));
    assert!(f
        .blockers
        .iter()
        .any(|b| matches!(b, Blocker::Port { port: 8096, .. })));
}

#[test]
fn base_services_and_reinstalls_are_refused() {
    let home = base().into_iter().find(|a| a.id == "home").unwrap();
    let f = check(&min_spec(), &base(), &[home], 20 * GIB);
    assert!(f
        .blockers
        .contains(&Blocker::BaseNotInstallable { app: "home".into() }));
    assert!(f
        .blockers
        .contains(&Blocker::AlreadyInstalled { app: "home".into() }));
}

#[test]
fn licence_review_is_reported() {
    let mut p = addon("platform.toml");
    p.licence = "AGPL-3.0-only".into();
    let f = check(&min_spec(), &measured_base(1 << 20), &[p], 20 * GIB);
    assert_eq!(f.licence_review, vec!["platform".to_string()]);
}
