//! A classroom signing in at once: N password sign-ins arrive together while
//! the box keeps checking other people's sessions. Measures how long the
//! burst takes, how long a session check waits during it, and the process's
//! peak memory.
//!
//! ```text
//! cargo run --release -p deltos-identity --example signin_burst -- 40 shared
//! cargo run --release -p deltos-identity --example signin_burst -- 40 separate
//! ```
//! `shared`: one `Client` for all, as `deltd` holds it. `separate`: one
//! connection each, the most parallel any caller could be.
//! Run each mode in its own process: peak memory is per process.

use std::os::unix::net::UnixListener;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use deltos_identity::protocol::{self, Callers};
use deltos_identity::{Client, Register};
use serde_json::json;

fn hwm_kib() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn main() {
    let mut a = std::env::args().skip(1);
    let n: usize = a.next().and_then(|v| v.parse().ok()).unwrap_or(40);
    let mode = a.next().unwrap_or_else(|| "shared".into());

    let dir = tempfile::tempdir().unwrap();
    let reg = Arc::new(Register::open(&dir.path().join("reg"), "home.deltos.lan").unwrap());
    let (_, pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    let sock = dir.path().join("identityd.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let me = unsafe { libc::getuid() };
    std::thread::spawn(move || protocol::serve(l, reg, Callers { deltd: vec![me], shelld: vec![] }));

    let shared = Arc::new(Client::new(&sock));
    let ctx = json!({ "https": true, "peer": "10.42.0.2" });
    let first = shared
        .call(&json!({ "call": "sign_in_password", "ctx": ctx, "sign_in_name": "admin", "password": pw }))
        .unwrap();
    let token = first["session_token"].as_str().unwrap().to_string();
    let before = hwm_kib();

    let go = Arc::new(Barrier::new(n + 2));
    let t0 = Instant::now();
    let mut hs = Vec::new();
    for i in 0..n {
        let (go, pw, sock, shared, mode) = (go.clone(), pw.clone(), sock.clone(), shared.clone(), mode.clone());
        hs.push(std::thread::spawn(move || {
            let own;
            let c: &Client = if mode == "shared" {
                &shared
            } else {
                own = Client::new(&sock);
                &own
            };
            let ctx = json!({ "https": true, "peer": format!("10.42.1.{i}") });
            go.wait();
            let t = Instant::now();
            c.call(&json!({ "call": "sign_in_password", "ctx": ctx, "sign_in_name": "admin", "password": pw }))
                .unwrap();
            t.elapsed()
        }));
    }
    // someone already signed in keeps reading while the class signs in
    let probe = {
        let (go, shared, token) = (go.clone(), shared.clone(), token.clone());
        std::thread::spawn(move || {
            go.wait();
            std::thread::sleep(Duration::from_millis(5));
            let t = Instant::now();
            shared.call(&json!({ "call": "session", "token": token })).unwrap();
            t.elapsed()
        })
    };
    go.wait();
    let mut each: Vec<Duration> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    let all = t0.elapsed();
    let probe = probe.join().unwrap();
    each.sort();
    println!(
        "{mode} n={n} cores={} burst_ms={:.0} slowest_sign_in_ms={:.0} median_sign_in_ms={:.0} \
         session_check_during_burst_ms={:.1} peak_rss_mib_before={:.1} peak_rss_mib_after={:.1}",
        std::thread::available_parallelism().map(|p| p.get()).unwrap_or(1),
        all.as_secs_f64() * 1e3,
        each.last().unwrap().as_secs_f64() * 1e3,
        each[each.len() / 2].as_secs_f64() * 1e3,
        probe.as_secs_f64() * 1e3,
        before as f64 / 1024.0,
        hwm_kib() as f64 / 1024.0,
    );
}
