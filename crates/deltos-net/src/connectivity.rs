//! Phones' "is there internet?" checks (docs/box-network-plan.md §6).
//!
//! When hotspot devices have no internet through the box, the box's DNS
//! sends these hosts to the box, and `deltd` answers exactly these paths the
//! way each operating system expects from a working connection. Phones then
//! keep using the box's Wi-Fi and DNS, and open no stray sign-in window.
//!
//! This is a **closed list**, like the privilege boundary's: a host here is
//! answered only on its listed paths, only for `GET`/`HEAD`, with a fixed
//! body and no cookie. Everything else on these hosts is refused.
//!
//! Each row is the URL and expected answer the vendor's own software uses.
//! **Not yet tested on real handsets** (plan §6; hardware testing deferred).

/// One check: host, path, status, content type, body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub host: &'static str,
    pub path: &'static str,
    pub status: u16,
    pub content_type: &'static str,
    pub body: &'static str,
}

const APPLE_SUCCESS: &str = "<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>";

pub const CHECKS: &[Check] = &[
    // Android and ChromeOS
    Check { host: "connectivitycheck.gstatic.com", path: "/generate_204", status: 204, content_type: "", body: "" },
    Check { host: "connectivitycheck.android.com", path: "/generate_204", status: 204, content_type: "", body: "" },
    Check { host: "clients3.google.com", path: "/generate_204", status: 204, content_type: "", body: "" },
    Check { host: "www.google.com", path: "/gen_204", status: 204, content_type: "", body: "" },
    // Apple
    Check { host: "captive.apple.com", path: "/hotspot-detect.html", status: 200, content_type: "text/html", body: APPLE_SUCCESS },
    Check { host: "www.apple.com", path: "/library/test/success.html", status: 200, content_type: "text/html", body: APPLE_SUCCESS },
    // Windows
    Check { host: "www.msftconnecttest.com", path: "/connecttest.txt", status: 200, content_type: "text/plain", body: "Microsoft Connect Test" },
    Check { host: "www.msftncsi.com", path: "/ncsi.txt", status: 200, content_type: "text/plain", body: "Microsoft NCSI" },
    // Firefox
    Check { host: "detectportal.firefox.com", path: "/success.txt", status: 200, content_type: "text/plain", body: "success\n" },
];

/// Windows also checks that this name resolves to this fixed address, so
/// the box's DNS answers it with that address, not the box's.
pub const WINDOWS_DNS_CHECK: (&str, &str) = ("dns.msftncsi.com", "131.107.255.255");

/// Every host on the list, each once, in list order.
pub fn hosts() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for c in CHECKS {
        if !v.contains(&c.host) {
            v.push(c.host);
        }
    }
    v
}

pub fn is_check_host(host: &str) -> bool {
    CHECKS.iter().any(|c| c.host == host)
}

/// The answer for `host` + `path`, if the pair is on the list.
pub fn lookup(host: &str, path: &str) -> Option<&'static Check> {
    CHECKS.iter().find(|c| c.host == host && c.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_list() {
        assert_eq!(lookup("connectivitycheck.gstatic.com", "/generate_204").unwrap().status, 204);
        assert!(lookup("connectivitycheck.gstatic.com", "/anything-else").is_none());
        assert!(lookup("example.com", "/generate_204").is_none());
        assert!(hosts().len() < CHECKS.len() + 1);
        assert!(!is_check_host("library.deltos.lan"));
    }
}
