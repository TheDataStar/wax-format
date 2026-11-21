//! E3: the hotspot's radio settings, the `configure_ap` action, and the
//! files the boot units read (docs/box-radio-and-boot.md).

use std::cell::RefCell;
use std::collections::HashSet;
use std::net::Ipv4Addr;

use deltos_net::actions::{authorize, Action, Caller, IdentityCheck, Refusal};
use deltos_net::{apply, render, Layout, Mode, NetError, Radio, Registry, Secret, Settings};

const ADMIN: &str = "admin-session";

struct Id {
    tickets: RefCell<HashSet<(String, String, String, String)>>,
}
impl Id {
    fn new() -> Self {
        Id { tickets: RefCell::new(HashSet::new()) }
    }
    fn issue(&self, t: &str, a: &Action) {
        self.tickets.borrow_mut().insert((t.into(), ADMIN.into(), a.name().into(), a.args_digest()));
    }
}
impl IdentityCheck for Id {
    fn session(&self, s: &str) -> Option<(String, bool)> {
        (s == ADMIN).then(|| ("admin".into(), true))
    }
    fn has_permission(&self, p: &str, perm: &str) -> bool {
        p == "admin" && perm == "network.configure"
    }
    fn redeem_ticket(&self, t: &str, s: &str, a: &str, d: &str) -> bool {
        self.tickets.borrow_mut().remove(&(t.into(), s.into(), a.into(), d.into()))
    }
}

fn hotspot(sharing: bool) -> Settings {
    Settings {
        mode: Mode::Hotspot { interface: "wlan0".into(), box_address: Ipv4Addr::new(10, 42, 0, 1), prefix_len: 24 },
        uplink_interface: Some("eth0".into()),
        upstream_dns: vec![],
        uplink_sharing: sharing,
    }
}

fn radio(ssid: &str, pass: &str) -> Radio {
    Radio { ssid: ssid.into(), passphrase: Secret::new(pass), country: "KE".into(), channel: 6 }
}

fn configure(ssid: &str, pass: &str) -> Action {
    Action::ConfigureAp { ssid: ssid.into(), passphrase: Secret::new(pass), country: "ke".into(), channel: 6 }
}

fn with_ticket(t: &'static str) -> Caller<'static> {
    Caller { session_ref: ADMIN, over_https: true, confirmation_ticket: Some(t) }
}

#[test]
fn radio_values_are_checked_and_none_is_defaulted() {
    assert!(radio("Library", "correct horse").validate().is_ok());
    assert!(radio("", "correct horse").validate().is_err(), "empty name");
    assert!(radio(&"x".repeat(33), "correct horse").validate().is_err(), "name over 32 bytes");
    assert!(radio("Library", "short").validate().is_err(), "password under 8");
    assert!(radio("Library", &"p".repeat(64)).validate().is_err(), "password over 63");
    assert!(radio("Library", "pass\u{e9}word").validate().is_err(), "password not ASCII");
    assert!(radio("Lib\nrary", "correct horse").validate().is_err(), "control character in the name");
    for c in ["", "K", "ke", "KEN", "00"] {
        let mut r = radio("Library", "correct horse");
        r.country = c.into();
        assert!(r.validate().is_err(), "country {c:?}");
    }
    for ch in [0, 14, 36] {
        let mut r = radio("Library", "correct horse");
        r.channel = ch;
        assert!(r.validate().is_err(), "channel {ch}");
    }
    // a fresh registry has no radio, so no hostapd.conf and no hotspot
    let mut reg = Registry::in_memory().unwrap();
    reg.set_settings(&hotspot(false)).unwrap();
    assert!(reg.radio().unwrap().is_none());
    let dir = tempfile::tempdir().unwrap();
    assert!(render(&reg, &Layout::new(dir.path())).unwrap().hostapd.is_none());
}

#[test]
fn the_password_never_prints() {
    let a = configure("Library", "hunter2hunter2");
    assert!(!format!("{a:?}").contains("hunter2"));
    assert!(!a.canonical_args().contains("hunter2"), "the ticket binds a digest, not the password");
    assert!(!format!("{:?}", radio("Library", "hunter2hunter2")).contains("hunter2"));
}

#[test]
fn configure_ap_is_secure_asks_again_and_binds_the_exact_password() {
    let id = Id::new();
    let plain = Caller { session_ref: ADMIN, over_https: false, confirmation_ticket: None };
    assert_eq!(authorize(configure("Library", "correct horse"), &plain, &id).unwrap_err(), Refusal::SecureConnectionRequired);
    let secure = Caller { session_ref: ADMIN, over_https: true, confirmation_ticket: None };
    assert_eq!(authorize(configure("Library", "correct horse"), &secure, &id).unwrap_err(), Refusal::ConfirmationRequired);
    // a ticket confirmed for another password does not change this one
    id.issue("t1", &configure("Library", "other password").normalised_for_test());
    assert_eq!(authorize(configure("Library", "correct horse"), &with_ticket("t1"), &id).unwrap_err(), Refusal::ConfirmationRequired);
    // an invalid value is refused before the ticket is spent
    id.issue("t2", &configure("Library", "short").normalised_for_test());
    assert!(matches!(authorize(configure("Library", "short"), &with_ticket("t2"), &id).unwrap_err(), Refusal::Invalid(_)));
    assert_eq!(id.tickets.borrow().len(), 2);
    id.issue("t3", &configure("Library", "correct horse").normalised_for_test());
    assert!(authorize(configure("Library", "correct horse"), &with_ticket("t3"), &id).is_ok());
}

/// The test helper above: the digest is over the normalised arguments
/// (country upper-cased), as shelld would show them for confirmation.
trait Norm {
    fn normalised_for_test(self) -> Action;
}
impl Norm for Action {
    fn normalised_for_test(self) -> Action {
        match self {
            Action::ConfigureAp { ssid, passphrase, country, channel } => {
                Action::ConfigureAp { ssid, passphrase, country: country.to_ascii_uppercase(), channel }
            }
            a => a,
        }
    }
}

#[test]
fn configure_ap_writes_a_root_only_hostapd_file_and_says_to_restart_it() {
    let id = Id::new();
    let mut reg = Registry::in_memory().unwrap();
    reg.set_settings(&hotspot(false)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("net"));
    deltos_net::write(&reg, &layout).unwrap();
    assert!(!layout.hostapd().exists());

    let a = configure("Café Library \"A\"", "correct horse");
    id.issue("t", &a.clone().normalised_for_test());
    let ok = authorize(a, &with_ticket("t"), &id).unwrap();
    let reload = apply(&mut reg, ok, &layout).unwrap();
    assert!(reload.restart_hostapd && !reload.restart_dnsmasq && !reload.restart_deltd);

    let conf = std::fs::read_to_string(layout.hostapd()).unwrap();
    assert!(conf.contains("interface=wlan0\n"));
    assert!(conf.contains("country_code=KE\n"));
    assert!(conf.contains("channel=6\n"));
    assert!(conf.contains("wpa=2\n") && conf.contains("rsn_pairwise=CCMP\n") && !conf.contains("TKIP"));
    assert!(conf.contains("wpa_passphrase=correct horse\n"));
    assert!(!conf.contains("ctrl_interface"), "no control socket");
    // any name, quotes included, goes in as hex
    let hex: String = "Café Library \"A\"".bytes().map(|b| format!("{b:02x}")).collect();
    assert!(conf.contains(&format!("ssid2={hex}\n")), "{conf}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(layout.hostapd()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the Wi-Fi password is root's only");
    }
    // the same settings again change nothing
    assert!(!deltos_net::write(&reg, &layout).unwrap().restart_hostapd);
}

#[test]
fn configure_ap_needs_a_hotspot() {
    let id = Id::new();
    let mut reg = Registry::in_memory().unwrap();
    reg.set_settings(&Settings {
        mode: Mode::Joined { interface: "eth0".into(), box_address: Ipv4Addr::new(192, 168, 1, 50) },
        uplink_interface: None,
        upstream_dns: vec![],
        uplink_sharing: false,
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = configure("Library", "correct horse");
    id.issue("t", &a.clone().normalised_for_test());
    let ok = authorize(a, &with_ticket("t"), &id).unwrap();
    assert!(matches!(apply(&mut reg, ok, &Layout::new(dir.path())), Err(NetError::Invalid(_))));
    // and a joined box never gets a hostapd.conf, even with a stored radio
    reg.set_radio(&radio("Library", "correct horse")).unwrap();
    let r = render(&reg, &Layout::new(dir.path())).unwrap();
    assert!(r.hostapd.is_none());
    assert!(!r.hotspot_env.contains("DELTOS_HOTSPOT_INTERFACE"), "{}", r.hotspot_env);
}

#[test]
fn boot_files_follow_the_registry() {
    let mut reg = Registry::in_memory().unwrap();
    reg.set_settings(&hotspot(false)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let r = render(&reg, &layout).unwrap();
    assert!(r.hotspot_env.contains("DELTOS_HOTSPOT_INTERFACE=wlan0\n"));
    assert!(r.hotspot_env.contains("DELTOS_HOTSPOT_ADDRESS=10.42.0.1/24\n"));
    assert!(r.hotspot_env.contains("DELTOS_UPLINK_SHARING=0\n"));
    // offline hotspot: DNS sends phones' checks to the box, so deltd answers them
    assert!(r.deltd_env.contains("DELTD_CONNECTIVITY_ARGS=--answer-connectivity-checks\n"));
    assert!(r.dnsmasq_conf.contains("connectivitycheck.gstatic.com"));

    // sharing on: the checks reach the real hosts, and deltd stops answering
    reg.set_settings(&hotspot(true)).unwrap();
    let r = render(&reg, &layout).unwrap();
    assert!(r.hotspot_env.contains("DELTOS_UPLINK_SHARING=1\n"));
    assert!(r.deltd_env.contains("DELTD_CONNECTIVITY_ARGS=\n"));
    assert!(!r.dnsmasq_conf.contains("connectivitycheck.gstatic.com"));

    // and the write says which services must follow
    deltos_net::write(&reg, &layout).unwrap();
    reg.set_settings(&hotspot(false)).unwrap();
    let reload = deltos_net::write(&reg, &layout).unwrap();
    assert!(reload.reload_firewall && reload.restart_deltd && reload.restart_dnsmasq && !reload.restart_hostapd);
}

#[test]
fn a_version_1_registry_is_upgraded_and_a_newer_one_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("registry.sqlite");
    {
        let c = rusqlite_conn(&p);
        c.execute_batch(
            "CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1), mode TEXT NOT NULL, interface TEXT NOT NULL,
               box_address TEXT NOT NULL, prefix_len INTEGER, uplink_interface TEXT, upstream_dns TEXT NOT NULL DEFAULT '',
               uplink_sharing INTEGER NOT NULL DEFAULT 0);
             INSERT INTO settings VALUES (1, 'hotspot', 'wlan0', '10.42.0.1', 24, NULL, '', 0);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let mut reg = Registry::open(&p).unwrap();
    assert_eq!(reg.settings().unwrap(), Settings { uplink_interface: None, ..hotspot(false) });
    reg.set_radio(&radio("Library", "correct horse")).unwrap();
    drop(reg);
    assert_eq!(Registry::open(&p).unwrap().radio().unwrap().unwrap().ssid, "Library");
    rusqlite_conn(&p).execute_batch("PRAGMA user_version = 3;").unwrap();
    assert!(matches!(Registry::open(&p), Err(NetError::NewerSchema(3))));
}

fn rusqlite_conn(p: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(p).unwrap()
}

#[test]
fn the_webos_name_cannot_be_given_away() {
    let id = Id::new();
    let caller = Caller { session_ref: ADMIN, over_https: true, confirmation_ticket: None };
    for n in ["home.deltos.lan", "HOME.deltos.lan."] {
        let a = Action::ApproveName { name: n.into(), target: deltos_net::Target::Address(Ipv4Addr::new(10, 42, 0, 9)), requested_by: "student-7".into() };
        assert!(matches!(authorize(a, &caller, &id), Err(Refusal::Invalid(_))), "{n}");
    }
    assert_eq!(deltos_net::WEBOS_HOST, "home.deltos.lan");
}
