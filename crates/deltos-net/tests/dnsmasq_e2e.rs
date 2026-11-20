//! The generated settings, run by a real `dnsmasq`, with a fake phone on a
//! virtual network: the phone gets an address and the box as its DNS, the
//! box's names resolve, an approved name and a reservation work without a
//! restart, and nothing else resolves while offline.
//!
//! Needs root, `dnsmasq`, `ip`, `dig` and `busybox` (for `udhcpc`, a DHCP
//! client). Without them it skips loudly; with `DELTOS_REQUIRE_NET_E2E=1` a
//! missing tool is a failure (the repo's usual rule, CLAUDE.md).
//! No real radio and no real Pi: a virtual ethernet pair stands in for Wi-Fi.

use std::net::Ipv4Addr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use deltos_net::actions::{authorize, Action, Caller, IdentityCheck};
use deltos_net::{apply, Layout, Mode, Registry, Settings, Target};

const NS: &str = "deltos-e2e-phone";
const BOX_IF: &str = "dlt-e2e-box";
const PHONE_IF: &str = "dlt-e2e-ph";

fn have(cmd: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {cmd}")).stdout(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn ready() -> bool {
    let root = Command::new("id").arg("-u").output().map(|o| o.stdout == b"0\n").unwrap_or(false);
    let missing: Vec<&str> = ["dnsmasq", "ip", "dig", "busybox"].into_iter().filter(|c| !have(c)).collect();
    if root && missing.is_empty() {
        return true;
    }
    let why = if root { format!("missing {}", missing.join(", ")) } else { "not root".to_string() };
    if std::env::var("DELTOS_REQUIRE_NET_E2E").as_deref() == Ok("1") {
        panic!("DELTOS_REQUIRE_NET_E2E=1 but {why}");
    }
    eprintln!("SKIPPED (dnsmasq end-to-end: {why}; set DELTOS_REQUIRE_NET_E2E=1 to make this a failure)");
    false
}

fn sh(cmd: &str) -> String {
    let o = Command::new("sh").arg("-c").arg(cmd).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Tears the virtual network and dnsmasq down however the test ends.
struct Net {
    dnsmasq: Option<Child>,
}

impl Drop for Net {
    fn drop(&mut self) {
        if let Some(mut c) = self.dnsmasq.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = Command::new("ip").args(["link", "del", BOX_IF]).status();
        let _ = Command::new("ip").args(["netns", "del", NS]).status();
    }
}

struct Id;
impl IdentityCheck for Id {
    fn session(&self, _: &str) -> Option<(String, bool)> {
        Some(("admin".into(), true))
    }
    fn has_permission(&self, _: &str, p: &str) -> bool {
        p == "network.configure"
    }
    fn redeem_ticket(&self, _: &str, _: &str, _: &str, _: &str) -> bool {
        false
    }
}

/// Ask for an address with `udhcpc` as `mac`; returns (ip, dns, router).
fn dhcp(dir: &Path, mac: &str) -> (String, String, String) {
    let script = dir.join("udhcpc.sh");
    let out = dir.join("lease.env");
    std::fs::write(&script, format!("#!/bin/sh\n[ \"$1\" = bound ] && echo \"$ip $dns $router\" > {}\nexit 0\n", out.display())).unwrap();
    sh(&format!("chmod +x {}", script.display()));
    let _ = std::fs::remove_file(&out);
    sh(&format!("ip netns exec {NS} ip link set {PHONE_IF} down"));
    sh(&format!("ip netns exec {NS} ip link set {PHONE_IF} address {mac}"));
    sh(&format!("ip netns exec {NS} ip link set {PHONE_IF} up"));
    sh(&format!(
        "ip netns exec {NS} busybox udhcpc -i {PHONE_IF} -n -q -f -t 5 -T 1 -s {}",
        script.display()
    ));
    let v = std::fs::read_to_string(&out).unwrap();
    let f: Vec<String> = v.split_whitespace().map(str::to_string).collect();
    (f[0].clone(), f[1].clone(), f[2].clone())
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_phone_on_the_hotspot_gets_an_address_and_the_boxs_names() {
    if !ready() {
        return;
    }
    for args in [vec!["link", "del", BOX_IF], vec!["netns", "del", NS]] {
        let _ = Command::new("ip").args(args).stderr(Stdio::null()).status();
    }
    let mut net = Net { dnsmasq: None };
    sh(&format!("ip netns add {NS}"));
    sh(&format!("ip link add {BOX_IF} type veth peer name {PHONE_IF}"));
    sh(&format!("ip link set {PHONE_IF} netns {NS}"));
    sh(&format!("ip addr add 10.42.0.1/24 dev {BOX_IF} && ip link set {BOX_IF} up"));
    sh(&format!("ip netns exec {NS} ip link set lo up"));

    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("net"));
    let mut reg = Registry::open(&dir.path().join("registry.sqlite")).unwrap();
    reg.set_settings(&Settings {
        mode: Mode::Hotspot { interface: BOX_IF.into(), box_address: Ipv4Addr::new(10, 42, 0, 1), prefix_len: 24 },
        uplink_interface: None,
        upstream_dns: vec![],
        uplink_sharing: false,
    })
    .unwrap();
    deltos_net::write(&reg, &layout).unwrap();
    let conf = layout.dnsmasq_conf();
    sh(&format!("dnsmasq --test --conf-file={}", conf.display()));
    let log = std::fs::File::create(dir.path().join("dnsmasq.log")).unwrap();
    net.dnsmasq = Some(
        Command::new("dnsmasq")
            .arg("--keep-in-foreground")
            .arg(format!("--conf-file={}", conf.display()))
            .arg(format!("--pid-file={}", dir.path().join("dnsmasq.pid").display()))
            .arg("--user=root")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(500));

    // DHCP: an address in the pool, the box as DNS and router
    let (ip, dns, router) = dhcp(dir.path(), "02:00:00:00:00:01");
    let ip_v: Ipv4Addr = ip.parse().unwrap();
    assert!(ip_v.octets()[..3] == [10, 42, 0] && ip_v != Ipv4Addr::new(10, 42, 0, 1), "{ip}");
    assert_eq!((dns.as_str(), router.as_str()), ("10.42.0.1", "10.42.0.1"));
    sh(&format!("ip netns exec {NS} ip addr add {ip}/24 dev {PHONE_IF}"));
    let digp = |n: &str| sh(&format!("ip netns exec {NS} dig @10.42.0.1 +short +time=2 +tries=1 {n} A"));

    // the box's zones: every name is the box, pack hostnames included
    assert_eq!(digp("library.deltos.lan").trim(), "10.42.0.1");
    assert_eq!(digp("pack-12-g3-0123456789abcdef.deltos-packs.lan").trim(), "10.42.0.1");
    // phones' internet checks come to the box; Windows' DNS check gets its fixed answer
    assert_eq!(digp("connectivitycheck.gstatic.com").trim(), "10.42.0.1");
    assert_eq!(digp("dns.msftncsi.com").trim(), "131.107.255.255");
    // offline: anything else is "no such name", at once
    let st = sh(&format!("ip netns exec {NS} dig @10.42.0.1 +time=2 +tries=1 example.com A"));
    assert!(st.contains("status: NXDOMAIN"), "{st}");

    // an approved name pointing at another device: resolves without a restart
    let pid_before = std::fs::read_to_string(dir.path().join("dnsmasq.pid")).unwrap();
    let a = authorize(
        Action::ApproveName { name: "printer.deltos.lan".into(), target: Target::Address(Ipv4Addr::new(10, 42, 0, 30)), requested_by: "t".into() },
        &Caller { session_ref: "s", over_https: true, confirmation_ticket: None },
        &Id,
    )
    .unwrap();
    let reload = apply(&mut reg, a, &layout).unwrap();
    assert!(!reload.restart_dnsmasq);
    until("the approved name", || digp("printer.deltos.lan").trim() == "10.42.0.30");

    // a reservation: the device gets its fixed address, again without a restart
    let a = authorize(
        Action::SetAddressReservation { mac: "02:00:00:00:00:02".into(), address: Ipv4Addr::new(10, 42, 0, 77), label: "projector".into() },
        &Caller { session_ref: "s", over_https: true, confirmation_ticket: None },
        &Id,
    )
    .unwrap();
    assert!(!apply(&mut reg, a, &layout).unwrap().restart_dnsmasq);
    std::thread::sleep(Duration::from_millis(300));
    sh(&format!("ip netns exec {NS} ip addr flush dev {PHONE_IF}"));
    let (ip2, _, _) = dhcp(dir.path(), "02:00:00:00:00:02");
    assert_eq!(ip2, "10.42.0.77");
    assert_eq!(std::fs::read_to_string(dir.path().join("dnsmasq.pid")).unwrap(), pid_before, "never restarted");
    assert!(net.dnsmasq.as_mut().unwrap().try_wait().unwrap().is_none(), "dnsmasq still running");

    // leases are visible to an admin, and the log holds no queries
    let leases = deltos_net::leases::read(&layout.leases(), 0);
    assert!(leases.iter().any(|l| l.address == Ipv4Addr::new(10, 42, 0, 77)), "{leases:?}");
    let log = std::fs::read_to_string(dir.path().join("dnsmasq.log")).unwrap();
    assert!(!log.contains("printer.deltos.lan") && !log.contains("query["), "no query log:\n{log}");

    // the firewall file is valid nftables, if nft is here to check it
    if have("nft") {
        sh(&format!("nft -c -f {}", layout.nftables().display()));
    }
}
