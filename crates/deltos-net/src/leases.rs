//! Reading `dnsmasq`'s lease file. `dnsmasq` owns it; the box only reads it.
//!
//! One lease per line: `<expiry> <mac> <address> <hostname|*> <client-id|*>`,
//! expiry in Unix seconds (`0` = never).

use std::net::Ipv4Addr;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub expires_unix: u64,
    pub mac: String,
    pub address: Ipv4Addr,
    /// The name the device gave, if any. Device-chosen text: shown, never trusted.
    pub device_name: Option<String>,
}

/// Unexpired leases, in address order. A missing or unreadable file is no
/// leases; a malformed line is skipped.
pub fn read(path: &Path, now_unix: u64) -> Vec<Lease> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    parse(&text, now_unix)
}

pub fn parse(text: &str, now_unix: u64) -> Vec<Lease> {
    let mut out: Vec<Lease> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            let expires_unix: u64 = f[0].parse().ok()?;
            if expires_unix != 0 && expires_unix <= now_unix {
                return None;
            }
            let address: Ipv4Addr = f[2].parse().ok()?;
            let name = (f[3] != "*").then(|| f[3].chars().filter(|c| !c.is_control()).take(63).collect());
            Some(Lease { expires_unix, mac: f[1].to_ascii_lowercase(), address, device_name: name })
        })
        .collect();
    out.sort_by_key(|l| u32::from(l.address));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_drops_expired() {
        let t = "2000 aa:bb:cc:dd:ee:01 10.42.0.20 phone-1 01:aa\n500 aa:bb:cc:dd:ee:02 10.42.0.9 old *\n\
                 0 AA:BB:CC:DD:EE:03 10.42.0.3 * *\ngarbage\n";
        let l = parse(t, 1000);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].address, Ipv4Addr::new(10, 42, 0, 3));
        assert_eq!(l[0].device_name, None);
        assert_eq!(l[0].mac, "aa:bb:cc:dd:ee:03");
        assert_eq!(l[1].device_name.as_deref(), Some("phone-1"));
    }
}
