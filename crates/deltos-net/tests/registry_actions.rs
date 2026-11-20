//! The registry, the generated settings, and the privilege-boundary checks
//! every network change passes through.

use std::cell::RefCell;
use std::collections::HashSet;
use std::net::Ipv4Addr;

use deltos_net::actions::{authorize, list_leases, Action, Caller, IdentityCheck, Refusal};
use deltos_net::{apply, render, Layout, Mode, NetError, Registry, Settings, Target};

const ADMIN: &str = "admin-session";
const TEACHER: &str = "teacher-session";
const PLAIN: &str = "plain-admin-session";

/// `identityd`, as shelld sees it: two secure sessions, one plain one, one
/// permission holder, and single-use tickets.
struct Id {
    tickets: RefCell<HashSet<(String, String, String, String)>>,
}

impl Id {
    fn new() -> Self {
        Id { tickets: RefCell::new(HashSet::new()) }
    }
    fn issue(&self, ticket: &str, session: &str, a: &Action) {
        self.tickets.borrow_mut().insert((ticket.into(), session.into(), a.name().into(), a.args_digest()));
    }
}

impl IdentityCheck for Id {
    fn session(&self, s: &str) -> Option<(String, bool)> {
        match s {
            ADMIN => Some(("admin".into(), true)),
            TEACHER => Some(("teacher".into(), true)),
            PLAIN => Some(("admin".into(), false)),
            _ => None,
        }
    }
    fn has_permission(&self, p: &str, perm: &str) -> bool {
        p == "admin" && perm == "network.configure"
    }
    fn redeem_ticket(&self, t: &str, s: &str, a: &str, d: &str) -> bool {
        self.tickets.borrow_mut().remove(&(t.into(), s.into(), a.into(), d.into()))
    }
}

fn hotspot(sharing: bool, upstream: bool) -> Settings {
    Settings {
        mode: Mode::Hotspot { interface: "wlan0".into(), box_address: Ipv4Addr::new(10, 42, 0, 1), prefix_len: 24 },
        uplink_interface: Some("eth0".into()),
        upstream_dns: if upstream { vec![Ipv4Addr::new(192, 168, 1, 1)] } else { vec![] },
        uplink_sharing: sharing,
    }
}

fn setup() -> (Registry, tempfile::TempDir, Layout) {
    let mut reg = Registry::in_memory().unwrap();
    reg.set_settings(&hotspot(false, false)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("net"));
    deltos_net::write(&reg, &layout).unwrap();
    (reg, dir, layout)
}

fn admin() -> Caller<'static> {
    Caller { session_ref: ADMIN, over_https: true, confirmation_ticket: None }
}

fn approve(name: &str, target: Target) -> Action {
    Action::ApproveName { name: name.into(), target, requested_by: "student-7".into() }
}

// --- the boundary --------------------------------------------------------------

#[test]
fn every_change_needs_a_live_secure_session_with_the_permission() {
    let id = Id::new();
    let a = || approve("wiki.deltos.lan", Target::Box);
    let refuse = |c: Caller| authorize(a(), &c, &id).unwrap_err();
    assert_eq!(refuse(Caller { session_ref: "nobody", over_https: true, confirmation_ticket: None }), Refusal::NotSignedIn);
    assert_eq!(refuse(Caller { session_ref: ADMIN, over_https: false, confirmation_ticket: None }), Refusal::SecureConnectionRequired);
    assert_eq!(refuse(Caller { session_ref: PLAIN, over_https: true, confirmation_ticket: None }), Refusal::SecureConnectionRequired, "a session made over plain HTTP");
    assert_eq!(refuse(Caller { session_ref: TEACHER, over_https: true, confirmation_ticket: None }), Refusal::PermissionDenied);
    let ok = authorize(a(), &admin(), &id).unwrap();
    assert_eq!(ok.profile_id(), "admin");
}

#[test]
fn sharing_asks_again_with_a_ticket_bound_to_the_exact_arguments() {
    let id = Id::new();
    let on = Action::SetUplinkSharing { on: true };
    assert_eq!(authorize(on.clone(), &admin(), &id).unwrap_err(), Refusal::ConfirmationRequired);
    // a ticket for the opposite arguments does not work
    id.issue("t1", ADMIN, &Action::SetUplinkSharing { on: false });
    let with = |t: &'static str| Caller { session_ref: ADMIN, over_https: true, confirmation_ticket: Some(t) };
    assert_eq!(authorize(on.clone(), &with("t1"), &id).unwrap_err(), Refusal::ConfirmationRequired);
    // a ticket for another session does not work
    id.issue("t2", TEACHER, &on);
    assert_eq!(authorize(on.clone(), &with("t2"), &id).unwrap_err(), Refusal::ConfirmationRequired);
    // the right one works once
    id.issue("t3", ADMIN, &on);
    assert!(authorize(on.clone(), &with("t3"), &id).is_ok());
    assert_eq!(authorize(on, &with("t3"), &id).unwrap_err(), Refusal::ConfirmationRequired, "single use");
    // a refused request never spends a ticket
    let id = Id::new();
    let on = Action::SetUplinkSharing { on: true };
    id.issue("t4", PLAIN, &on);
    let plain = Caller { session_ref: PLAIN, over_https: true, confirmation_ticket: Some("t4") };
    assert_eq!(authorize(on.clone(), &plain, &id).unwrap_err(), Refusal::SecureConnectionRequired);
    assert_eq!(id.tickets.borrow().len(), 1);
}

#[test]
fn arguments_have_fixed_shapes() {
    let id = Id::new();
    for bad in [
        approve("wiki.example.com", Target::Box),
        approve("a.b.deltos.lan", Target::Box),
        approve("pack-1-g1-0123456789abcdef.deltos-packs.lan", Target::Box),
        approve("-x.deltos.lan", Target::Box),
        approve("under_score.deltos.lan", Target::Box),
        approve("ok.deltos.lan", Target::Address(Ipv4Addr::new(8, 8, 8, 8))),
        Action::SetAddressReservation { mac: "01:00:5e:00:00:01".into(), address: Ipv4Addr::new(10, 42, 0, 20), label: "x".into() },
        Action::SetAddressReservation { mac: "zz".into(), address: Ipv4Addr::new(10, 42, 0, 20), label: "x".into() },
        Action::SetAddressReservation { mac: "aa:bb:cc:dd:ee:ff".into(), address: Ipv4Addr::new(10, 42, 0, 20), label: "bad label!".into() },
    ] {
        assert!(matches!(authorize(bad.clone(), &admin(), &id), Err(Refusal::Invalid(_))), "{bad:?}");
    }
    // normalised on the way in
    let a = authorize(approve("Wiki.DELTOS.lan.", Target::Box), &admin(), &id).unwrap();
    assert_eq!(a.action(), &approve("wiki.deltos.lan", Target::Box));
}

// --- what the actions do ---------------------------------------------------------

#[test]
fn names_and_reservations_take_effect_without_a_restart() {
    let (mut reg, _d, layout) = setup();
    let id = Id::new();
    let r = apply(&mut reg, authorize(approve("printer.deltos.lan", Target::Address(Ipv4Addr::new(10, 42, 0, 30))), &admin(), &id).unwrap(), &layout).unwrap();
    assert!(!r.restart_dnsmasq && !r.reload_firewall, "{r:?}");
    let names = std::fs::read_to_string(layout.names_dir().join("names")).unwrap();
    assert!(names.contains("10.42.0.30 printer.deltos.lan"), "{names}");
    assert_eq!(reg.names().unwrap()[0].approved_by, "admin");
    assert!(matches!(
        apply(&mut reg, authorize(approve("printer.deltos.lan", Target::Box), &admin(), &id).unwrap(), &layout),
        Err(NetError::Exists(_))
    ));

    let res = Action::SetAddressReservation { mac: "AA:BB:CC:00:00:01".into(), address: Ipv4Addr::new(10, 42, 0, 40), label: "projector".into() };
    let r = apply(&mut reg, authorize(res, &admin(), &id).unwrap(), &layout).unwrap();
    assert!(!r.restart_dnsmasq);
    let hosts = std::fs::read_to_string(layout.dhcp_hosts_dir().join("reservations")).unwrap();
    assert!(hosts.contains("aa:bb:cc:00:00:01,10.42.0.40,projector"), "{hosts}");

    for (addr, why) in [((10, 42, 0, 1), "the box"), ((10, 42, 1, 5), "outside the subnet"), ((10, 42, 0, 255), "broadcast"), ((10, 42, 0, 40), "taken")] {
        let a = Action::SetAddressReservation { mac: "aa:bb:cc:00:00:02".into(), address: Ipv4Addr::new(addr.0, addr.1, addr.2, addr.3), label: "x".into() };
        assert!(apply(&mut reg, authorize(a, &admin(), &id).unwrap(), &layout).is_err(), "{why}");
    }
    apply(&mut reg, authorize(Action::RemoveAddressReservation { mac: "aa:bb:cc:00:00:01".into() }, &admin(), &id).unwrap(), &layout).unwrap();
    assert!(reg.reservations().unwrap().is_empty());
}

#[test]
fn sharing_changes_the_firewall_and_who_answers_internet_checks() {
    let (mut reg, _d, layout) = setup();
    let before = render(&reg, &layout).unwrap();
    assert!(before.dnsmasq_conf.contains("address=/connectivitycheck.gstatic.com/10.42.0.1"));
    assert!(before.dnsmasq_conf.contains("address=/dns.msftncsi.com/131.107.255.255"));
    assert!(before.nftables.contains("policy drop") && !before.nftables.contains("masquerade"));

    let id = Id::new();
    let on = Action::SetUplinkSharing { on: true };
    id.issue("t", ADMIN, &on);
    let c = Caller { session_ref: ADMIN, over_https: true, confirmation_ticket: Some("t") };
    let r = apply(&mut reg, authorize(on, &c, &id).unwrap(), &layout).unwrap();
    assert!(r.restart_dnsmasq && r.reload_firewall);
    let after = render(&reg, &layout).unwrap();
    assert!(!after.dnsmasq_conf.contains("connectivitycheck"), "with internet, checks reach the real hosts");
    assert!(after.nftables.contains("iifname \"wlan0\" oifname \"eth0\" accept"));
    assert!(after.nftables.contains("oifname \"eth0\" masquerade"));
}

#[test]
fn generated_dnsmasq_settings_keep_the_rules() {
    let (reg, _d, layout) = setup();
    let r = render(&reg, &layout).unwrap();
    let c = &r.dnsmasq_conf;
    assert!(!c.contains("log-queries"), "no query log, ever");
    for want in [
        "address=/deltos.lan/10.42.0.1",
        "address=/deltos-packs.lan/10.42.0.1",
        "local=/deltos.lan/",
        "local=/deltos-packs.lan/",
        "address=/#/",
        "dhcp-range=10.42.0.2,10.42.0.254,255.255.255.0",
        "dhcp-option=option:dns-server,10.42.0.1",
        "dhcp-authoritative",
        "interface=wlan0",
    ] {
        assert!(c.contains(want), "missing {want:?} in\n{c}");
    }
    assert_eq!(render(&reg, &layout).unwrap(), r, "deterministic");

    // with an upstream: forwarded, not blackholed
    let mut up = Registry::in_memory().unwrap();
    up.set_settings(&hotspot(false, true)).unwrap();
    let c = render(&up, &layout).unwrap().dnsmasq_conf;
    assert!(c.contains("server=192.168.1.1") && !c.contains("address=/#/"));

    // joined to a school network: DNS only, no DHCP, no reservations
    let mut j = Registry::in_memory().unwrap();
    j.set_settings(&Settings {
        mode: Mode::Joined { interface: "eth0".into(), box_address: Ipv4Addr::new(192, 168, 1, 50) },
        uplink_interface: Some("eth0".into()),
        upstream_dns: vec![Ipv4Addr::new(192, 168, 1, 1)],
        uplink_sharing: false,
    })
    .unwrap();
    let c = render(&j, &layout).unwrap().dnsmasq_conf;
    assert!(!c.contains("dhcp-range") && !c.contains("dhcp-authoritative"), "{c}");
    assert!(c.contains("address=/deltos.lan/192.168.1.50"));
    let id = Id::new();
    let res = Action::SetAddressReservation { mac: "aa:bb:cc:00:00:01".into(), address: Ipv4Addr::new(192, 168, 1, 60), label: "x".into() };
    assert!(matches!(apply(&mut j, authorize(res, &admin(), &id).unwrap(), &layout), Err(NetError::Invalid(_))));
}

#[test]
fn settings_are_validated_and_the_schema_is_versioned() {
    let mut reg = Registry::in_memory().unwrap();
    assert_eq!(reg.settings().unwrap_err(), NetError::NotConfigured);
    let mut s = hotspot(false, false);
    s.mode = Mode::Hotspot { interface: "wlan0".into(), box_address: Ipv4Addr::new(8, 8, 8, 1), prefix_len: 24 };
    assert!(reg.set_settings(&s).is_err(), "public address");
    s.mode = Mode::Hotspot { interface: "wlan0".into(), box_address: Ipv4Addr::new(10, 42, 0, 0), prefix_len: 24 };
    assert!(reg.set_settings(&s).is_err(), "network address");
    let mut s = hotspot(true, false);
    s.uplink_interface = None;
    assert!(reg.set_settings(&s).is_err(), "sharing with no uplink");

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.sqlite");
    drop(Registry::open(&p).unwrap());
    rusqlite_set_version(&p, 9);
    assert!(matches!(Registry::open(&p), Err(NetError::NewerSchema(9))));
}

fn rusqlite_set_version(p: &std::path::Path, v: i64) {
    // SQLite keeps user_version in the file header, offset 60, big-endian
    let mut b = std::fs::read(p).unwrap();
    b[60..64].copy_from_slice(&(v as u32).to_be_bytes());
    std::fs::write(p, b).unwrap();
}
#[test]
fn leases_need_the_same_checks() {
    let (_reg, _d, layout) = setup();
    std::fs::write(layout.leases(), "4000000000 aa:bb:cc:dd:ee:01 10.42.0.20 phone *\n").unwrap();
    let id = Id::new();
    let teacher = Caller { session_ref: TEACHER, over_https: true, confirmation_ticket: None };
    assert_eq!(list_leases(&teacher, &id, &layout, 1).unwrap_err(), Refusal::PermissionDenied, "teachers never see devices");
    let plain = Caller { session_ref: ADMIN, over_https: false, confirmation_ticket: None };
    assert_eq!(list_leases(&plain, &id, &layout, 1).unwrap_err(), Refusal::SecureConnectionRequired);
    assert_eq!(list_leases(&admin(), &id, &layout, 1).unwrap()[0].device_name.as_deref(), Some("phone"));
}
