//! The capability record (contract §9): measured from a made-up `/proc` and
//! `/sys`, written once with a chosen profile, measured again at boot.

use deltos_apps::fit::{BoxArch, Capability};
use deltos_apps::manifest::Profile;
use deltos_apps::measure::{self, Error, Machine, GB, GIB};
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const IW_AP: &str =
    "Wiphy phy0\n\tSupported interface modes:\n\t\t * managed\n\t\t * AP\n\tBand 1:\n";

struct Fake {
    _t: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

/// A Pi-class board: 2 GB sold (1.8 GiB seen), a 32 GB card, one radio,
/// a v3d render node, no clock.
fn pi(mem_total_kb: u64, card_bytes: u64) -> Fake {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("root");
    let state = t.path().join("var/lib/deltos");
    fs::create_dir_all(&state).unwrap();
    meminfo(&root, mem_total_kb, mem_total_kb - 1_500_000);
    w(&root.join("proc/sys/kernel/arch"), "aarch64\n");
    // the state tree's real device, pointed at a made-up card partition
    let dev = fs::metadata(&state).unwrap().dev();
    let disk = root.join("sys/devices/platform/mmc/mmcblk0");
    w(&disk.join("size"), &format!("{}\n", card_bytes / 512));
    w(&disk.join("mmcblk0p2/partition"), "2\n");
    w(&disk.join("mmcblk0p2/size"), "100\n");
    fs::create_dir_all(root.join("sys/dev/block")).unwrap();
    symlink(
        disk.join("mmcblk0p2"),
        root.join(format!(
            "sys/dev/block/{}:{}",
            libc::major(dev),
            libc::minor(dev)
        )),
    )
    .unwrap();
    fs::create_dir_all(root.join("sys/bus/drivers/v3d")).unwrap();
    fs::create_dir_all(root.join("sys/class/drm/renderD128/device")).unwrap();
    fs::create_dir_all(root.join("sys/class/drm/card0/device")).unwrap();
    symlink(
        root.join("sys/bus/drivers/v3d"),
        root.join("sys/class/drm/renderD128/device/driver"),
    )
    .unwrap();
    fs::create_dir_all(root.join("sys/class/net/wlan0/phy80211")).unwrap();
    fs::create_dir_all(root.join("sys/class/net/eth0")).unwrap();
    fs::create_dir_all(root.join("proc/1")).unwrap();
    w(&root.join("proc/1/comm"), "systemd\n");
    Fake { _t: t, root, state }
}

fn meminfo(root: &Path, total_kb: u64, avail_kb: u64) {
    w(
        &root.join("proc/meminfo"),
        &format!(
            "MemTotal:       {total_kb} kB\nMemFree:        1 kB\nMemAvailable:   {avail_kb} kB\n"
        ),
    );
}

fn machine(f: &Fake, iw: Option<&'static str>) -> Machine {
    Machine {
        root: f.root.clone(),
        state: f.state.clone(),
        iw_phy: Box::new(move || iw.map(String::from)),
    }
}

#[test]
fn a_box_measures_itself_and_the_profile_is_chosen_once() {
    let f = pi(1_849_000, 31_914_983_424);
    let m = machine(&f, Some(IW_AP));
    let rec = f.state.join("capability.json");

    // below the floor: nothing written
    let e = measure::provision(&m, &rec, Profile::CommunityHub).unwrap_err();
    assert!(matches!(e, Error::BelowFloor { .. }), "{e}");
    assert!(e.to_string().contains("16 GiB of memory"), "{e}");
    assert!(!rec.exists());

    let (c, running) = measure::provision(&m, &rec, Profile::Classroom).unwrap();
    assert!(running.is_empty());
    assert_eq!(c.profile, Profile::Classroom);
    assert_eq!(c.ram_bytes, 2 * GIB, "a 2 GB board, as sold");
    assert_eq!(
        c.storage_bytes, 31_914_983_424,
        "the whole card, not the partition"
    );
    assert_eq!(c.arch, BoxArch::Aarch64);
    assert_eq!(c.gpu, "v3d");
    assert!(c.has_wifi && c.can_ap && !c.has_rtc);
    // what Linux cannot hand out: 2 GiB less MemAvailable (349,000 kB available)
    assert_eq!(c.os_ram_bytes, Some(2 * GIB - 349_000 * 1024));

    let mode = fs::metadata(&rec).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
    let text = fs::read_to_string(&rec).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["profile"], "classroom");
    // what shelld and the fit check read
    let read: Capability = serde_json::from_str(&text).unwrap();
    assert_eq!(read, c);

    let e = measure::provision(&m, &rec, Profile::Kiosk).unwrap_err();
    assert!(matches!(e, Error::Exists(_)), "{e}");
    assert_eq!(
        fs::read_to_string(&rec).unwrap(),
        text,
        "a second provision changes nothing"
    );
}

#[test]
fn every_boot_measures_again_and_keeps_the_profile() {
    let f = pi(1_849_000, 31_914_983_424);
    let m = machine(&f, Some(IW_AP));
    let rec = f.state.join("capability.json");
    assert!(matches!(
        measure::remeasure(&m, &rec).unwrap_err(),
        Error::Missing(_)
    ));
    measure::provision(&m, &rec, Profile::Kiosk).unwrap();
    let before = fs::metadata(&rec).unwrap().mtime_nsec();

    // nothing changed: nothing rewritten
    let (c, _, below) = measure::remeasure(&m, &rec).unwrap();
    assert!(below.is_none());
    assert_eq!(fs::metadata(&rec).unwrap().mtime_nsec(), before);
    let os = c.os_ram_bytes;

    // a service is running: its memory is not the operating system's
    fs::create_dir_all(f.root.join("proc/812")).unwrap();
    w(&f.root.join("proc/812/comm"), "deltd\n");
    meminfo(&f.root, 1_849_000, 100_000);
    let (c, running, _) = measure::remeasure(&m, &rec).unwrap();
    assert_eq!(running, ["deltd"]);
    assert_eq!(c.os_ram_bytes, os, "kept from the last time nothing ran");
    fs::remove_dir_all(f.root.join("proc/812")).unwrap();

    // the card moved to a board with less memory: recorded as it is, and said
    meminfo(&f.root, 900_000, 600_000);
    let (c, _, below) = measure::remeasure(&m, &rec).unwrap();
    assert_eq!(c.profile, Profile::Kiosk);
    assert_eq!(c.ram_bytes, GIB);
    assert!(matches!(below, Some(Error::BelowFloor { .. })));
    let read: Capability = serde_json::from_str(&fs::read_to_string(&rec).unwrap()).unwrap();
    assert_eq!(read.ram_bytes, GIB);
}

#[test]
fn what_cannot_be_measured_is_refused_not_guessed() {
    let f = pi(1_849_000, 31_914_983_424);
    let rec = f.state.join("capability.json");

    // a radio, and no iw to ask it
    let e = measure::provision(&machine(&f, None), &rec, Profile::Kiosk).unwrap_err();
    assert!(matches!(e, Error::NoIw), "{e}");
    // no radio: no iw needed, and no hotspot
    fs::remove_dir_all(f.root.join("sys/class/net/wlan0")).unwrap();
    let (c, _) = measure::provision(&machine(&f, None), &rec, Profile::Kiosk).unwrap();
    assert!(!c.has_wifi && !c.can_ap);
    fs::remove_file(&rec).unwrap();

    w(&f.root.join("proc/sys/kernel/arch"), "riscv64\n");
    let e = measure::provision(&machine(&f, None), &rec, Profile::Kiosk).unwrap_err();
    assert!(matches!(e, Error::Arch { .. }), "{e}");
    w(&f.root.join("proc/sys/kernel/arch"), "aarch64\n");

    // a 32 GB card a little under 32 GB as sold is under the floor
    let small = pi(1_849_000, 15_931_539_456);
    let e = measure::provision(
        &machine(&small, Some(IW_AP)),
        &small.state.join("capability.json"),
        Profile::Kiosk,
    )
    .unwrap_err();
    assert!(e.to_string().contains("32 GB drive"), "{e}");

    fs::remove_file(f.root.join("proc/meminfo")).unwrap();
    assert!(matches!(
        measure::provision(&machine(&f, None), &rec, Profile::Kiosk).unwrap_err(),
        Error::Read(_)
    ));
}

#[test]
fn a_newer_record_is_refused_and_no_gpu_says_none() {
    let f = pi(16_200_000, 512_110_190_592);
    fs::remove_dir_all(f.root.join("sys/class/drm")).unwrap();
    let rec = f.state.join("capability.json");
    let (c, _) =
        measure::provision(&machine(&f, Some(IW_AP)), &rec, Profile::CommunityHub).unwrap();
    assert_eq!(c.gpu, "none");
    assert_eq!(c.ram_bytes, 16 * GIB);
    assert!(c.storage_bytes >= 256 * GB);

    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&rec).unwrap()).unwrap();
    v["schema"] = 2.into();
    fs::write(&rec, v.to_string()).unwrap();
    assert!(matches!(
        measure::remeasure(&machine(&f, Some(IW_AP)), &rec).unwrap_err(),
        Error::Newer(_, 2)
    ));
}

#[test]
fn this_machine_measures() {
    let t = tempfile::tempdir().unwrap();
    let m = Machine::this(t.path());
    let (total, avail) = m.meminfo().unwrap();
    assert!(total > 0 && avail <= total);
    assert!(m.storage_bytes().unwrap() > 0);
    m.arch().unwrap();
    let _ = m.gpu();
}
