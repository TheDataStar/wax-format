//! TEST ONLY: stands in for `deltos-shelld` (not built yet) so the VM test
//! (`system/tests/vm/`) can run a real `configure_ap` through
//! [`deltos_net::authorize`] and [`deltos_net::apply`] on a booted box and
//! watch the box react. It is an example, never built into a box image and
//! never installed: its identity check says yes to one fixed session, which
//! is exactly what a real box must never do.
//!
//! Usage: configure_ap_standin <registry.sqlite> <settings-dir> <ssid> <passphrase> <country> <channel>

use deltos_net::actions::{authorize, Action, Caller, IdentityCheck};
use deltos_net::{apply, Layout, Registry, Secret};

struct Yes;
impl IdentityCheck for Yes {
    fn session(&self, s: &str) -> Option<(String, bool)> {
        (s == "vm-test").then(|| ("vm-test-admin".into(), true))
    }
    fn has_permission(&self, _: &str, perm: &str) -> bool {
        perm == "network.configure"
    }
    fn redeem_ticket(&self, t: &str, _: &str, _: &str, _: &str) -> bool {
        t == "vm-test-ticket"
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [reg, dir, ssid, pass, country, channel] = a.as_slice() else {
        eprintln!("usage: configure_ap_standin <registry> <settings-dir> <ssid> <passphrase> <country> <channel>");
        std::process::exit(2);
    };
    let action = Action::ConfigureAp {
        ssid: ssid.clone(),
        passphrase: Secret::new(pass.clone()),
        country: country.clone(),
        channel: channel.parse().expect("channel"),
    };
    let caller = Caller { session_ref: "vm-test", over_https: true, confirmation_ticket: Some("vm-test-ticket") };
    let ok = authorize(action, &caller, &Yes).unwrap_or_else(|e| {
        eprintln!("refused: {e}");
        std::process::exit(1)
    });
    let mut r = Registry::open(std::path::Path::new(reg)).expect("registry");
    let reload = apply(&mut r, ok, &Layout::new(dir)).expect("apply");
    println!("{reload:?}");
}
