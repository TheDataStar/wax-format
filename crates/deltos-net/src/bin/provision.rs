//! `deltos-net-provision` — give a new box its first network settings.
//!
//! **Only on a box that has none yet.** Once the registry holds settings,
//! every change goes through `deltos-shelld`'s checked actions
//! (`deltos_net::actions`) and this tool refuses to run, so it is never a
//! side door around the privilege boundary. It is what Track E's
//! provisioning (E7) and the installable-stack installer call once, as
//! root, with a settings file the person setting up the box wrote.
//!
//! There are no defaults. Every value the box needs is in the file, or the
//! tool refuses (docs/box-radio-and-boot.md §4):
//!
//! ```text
//! # the box runs its own Wi-Fi (box-network-plan §3.1)
//! mode=hotspot
//! interface=wlan0
//! address=10.42.0.1/24
//! uplink_interface=eth0        # optional: the interface with internet
//! upstream_dns=192.168.1.1     # optional, comma-separated
//! ssid=Library
//! passphrase=...
//! country=KE
//! channel=6
//! ```
//!
//! or, joined to an existing network (plan §3.2): `mode=joined`,
//! `interface`, `address` (the box's fixed address there, no prefix) and
//! optional `upstream_dns`. Internet sharing always starts off: only an
//! admin turns it on, through `set_uplink_sharing` (contract §16).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::exit;

use deltos_net::{Mode, Radio, Registry, Secret, Settings};

const KEYS: &[&str] = &[
    "mode", "interface", "address", "uplink_interface", "upstream_dns", "ssid", "passphrase", "country", "channel",
];

fn fail(code: i32, msg: impl std::fmt::Display) -> ! {
    eprintln!("deltos-net-provision: {msg}");
    exit(code)
}

fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut m = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.trim_end_matches('\r');
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        let (k, v) = l.split_once('=').ok_or_else(|| format!("line {}: expected key=value", i + 1))?;
        let k = k.trim();
        if !KEYS.contains(&k) {
            return Err(format!("line {}: unknown setting {k:?}", i + 1));
        }
        // the password is taken exactly as written; other values are trimmed
        let v = if k == "passphrase" { v.to_string() } else { v.trim().to_string() };
        if m.insert(k.to_string(), v).is_some() {
            return Err(format!("line {}: {k} given twice", i + 1));
        }
    }
    Ok(m)
}

fn build(mut m: BTreeMap<String, String>) -> Result<(Settings, Option<Radio>), String> {
    let mut take = |k: &str| m.remove(k);
    let need = |v: Option<String>, k: &str| v.filter(|s| !s.is_empty()).ok_or_else(|| format!("{k} is required"));
    let mode = need(take("mode"), "mode")?;
    let interface = need(take("interface"), "interface")?;
    let address = need(take("address"), "address")?;
    let upstream_dns = match take("upstream_dns") {
        Some(s) if !s.is_empty() => s
            .split(',')
            .map(|a| a.trim().parse::<Ipv4Addr>().map_err(|_| format!("upstream_dns: {a:?} is not an IPv4 address")))
            .collect::<Result<Vec<_>, _>>()?,
        _ => vec![],
    };
    let uplink_interface = take("uplink_interface").filter(|s| !s.is_empty());
    let radio_keys = ["ssid", "passphrase", "country", "channel"].map(&mut take);
    match mode.as_str() {
        "hotspot" => {
            let (a, p) = address.split_once('/').ok_or("address must be address/prefix for a hotspot, e.g. 10.42.0.1/24")?;
            let box_address: Ipv4Addr = a.parse().map_err(|_| format!("address: {a:?} is not an IPv4 address"))?;
            let prefix_len: u8 = p.parse().map_err(|_| format!("address: bad prefix {p:?}"))?;
            let [ssid, passphrase, country, channel] = radio_keys;
            let radio = Radio {
                ssid: need(ssid, "ssid")?,
                passphrase: Secret::new(need(passphrase, "passphrase")?),
                country: need(country, "country")?,
                channel: need(channel, "channel")?.parse().map_err(|_| "channel must be a number".to_string())?,
            };
            let s = Settings { mode: Mode::Hotspot { interface, box_address, prefix_len }, uplink_interface, upstream_dns, uplink_sharing: false };
            Ok((s, Some(radio)))
        }
        "joined" => {
            if radio_keys.iter().any(Option::is_some) {
                return Err("ssid, passphrase, country and channel are for a hotspot; this box joins another network".into());
            }
            let box_address: Ipv4Addr = address.parse().map_err(|_| format!("address: {address:?} is not an IPv4 address"))?;
            let s = Settings { mode: Mode::Joined { interface, box_address }, uplink_interface, upstream_dns, uplink_sharing: false };
            Ok((s, None))
        }
        other => Err(format!("mode must be hotspot or joined, not {other:?}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [registry, seed] = args.as_slice() else {
        fail(2, "usage: deltos-net-provision <registry.sqlite> <settings-file>");
    };
    let text = std::fs::read_to_string(seed).unwrap_or_else(|e| fail(1, format!("{seed}: {e}")));
    let (settings, radio) = parse(&text).and_then(build).unwrap_or_else(|e| fail(1, format!("{seed}: {e}")));
    settings.validate().unwrap_or_else(|e| fail(1, e));
    if let Some(r) = &radio {
        r.validate().unwrap_or_else(|e| fail(1, e));
    }
    let mut reg = Registry::open(&PathBuf::from(registry)).unwrap_or_else(|e| fail(1, e));
    match reg.settings() {
        Err(deltos_net::NetError::NotConfigured) => {}
        Ok(_) => fail(3, "this box already has network settings; change them through the admin console (network.configure)"),
        Err(e) => fail(1, e),
    }
    if let Some(r) = &radio {
        reg.set_radio(r).unwrap_or_else(|e| fail(1, e));
    }
    reg.set_settings(&settings).unwrap_or_else(|e| fail(1, e));
    match &settings.mode {
        Mode::Hotspot { interface, box_address, prefix_len } => {
            println!("provisioned: hotspot on {interface}, box at {box_address}/{prefix_len}, internet sharing off")
        }
        Mode::Joined { interface, box_address } => println!("provisioned: joined on {interface}, box at {box_address}, no DHCP"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotspot_file_parses_and_needs_every_radio_value() {
        let f = "mode=hotspot\ninterface=wlan0\naddress=10.42.0.1/24\nssid=Library\npassphrase= spaces kept \ncountry=KE\nchannel=6\n";
        let (s, r) = build(parse(f).unwrap()).unwrap();
        assert!(!s.uplink_sharing);
        let r = r.unwrap();
        assert_eq!(r.passphrase.expose(), " spaces kept ");
        assert!(!format!("{r:?}").contains("spaces"), "password never prints");
        for k in ["ssid", "passphrase", "country", "channel"] {
            let without: String = f.lines().filter(|l| !l.starts_with(k)).map(|l| format!("{l}\n")).collect();
            assert!(build(parse(&without).unwrap()).unwrap_err().contains(k), "{k} is required");
        }
    }

    #[test]
    fn unknown_and_repeated_keys_are_refused() {
        assert!(parse("uplink_sharing=1\n").unwrap_err().contains("unknown"));
        assert!(parse("mode=hotspot\nmode=joined\n").unwrap_err().contains("twice"));
    }

    #[test]
    fn joined_takes_no_radio() {
        let (s, r) = build(parse("mode=joined\ninterface=eth0\naddress=192.168.1.50\n").unwrap()).unwrap();
        assert!(r.is_none() && matches!(s.mode, Mode::Joined { .. }));
        assert!(build(parse("mode=joined\ninterface=eth0\naddress=192.168.1.50\nssid=x\n").unwrap()).is_err());
    }
}
