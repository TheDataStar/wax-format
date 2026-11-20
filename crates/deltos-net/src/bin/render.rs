//! `deltos-net-render` — write the settings files the registry implies.
//!
//! Run by the box's supervisor at boot, before `dnsmasq` starts. It changes
//! nothing in the registry: every change goes through `deltos-shelld`'s
//! checked actions (`deltos_net::actions`), never through a command line.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(registry), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: deltos-net-render <registry.sqlite> <settings-dir>");
        std::process::exit(2);
    };
    let reg = match deltos_net::Registry::open(&PathBuf::from(registry)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("deltos-net-render: {e}");
            std::process::exit(1);
        }
    };
    match deltos_net::write(&reg, &deltos_net::Layout::new(out)) {
        Ok(r) => println!("restart_dnsmasq={} reload_firewall={}", r.restart_dnsmasq, r.reload_firewall),
        Err(e) => {
            eprintln!("deltos-net-render: {e}");
            std::process::exit(1);
        }
    }
}
