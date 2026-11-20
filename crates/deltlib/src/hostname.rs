//! Pack hostnames (contract §8):
//! `pack-<pack_index>-g<pack_generation>-<pack_partition>.deltos-packs.lan`.
//!
//! Parsing is strict, because the hostname is the pack zone's only input
//! besides the path: a name that is not exactly in this form is refused
//! before any lookup (deltd API §4.1, privilege boundary rule 16).

use std::fmt;

/// The zone every pack hostname lives in (contract §8). Shares no cookie
/// scope with `deltos.lan`.
pub const PACK_ZONE: &str = "deltos-packs.lan";

/// Highest `pack_index` (contract §8: an integer 0–511).
pub const MAX_PACK_INDEX: u16 = 511;

/// Length of a `pack_partition`: 16 lowercase hex characters, 64 random bits.
pub const PARTITION_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackHost {
    pub pack_index: u16,
    pub pack_generation: u64,
    pub pack_partition: String,
}

impl PackHost {
    /// Parse a `Host` value (already lowercased, port removed) in the pack
    /// zone. `None` for anything not exactly a pack hostname.
    pub fn parse(host: &str) -> Option<PackHost> {
        let label = host.strip_suffix(PACK_ZONE)?.strip_suffix('.')?;
        let rest = label.strip_prefix("pack-")?;
        let mut parts = rest.split('-');
        let (idx, gen, part) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let pack_index = canonical_uint(idx)?;
        if pack_index > MAX_PACK_INDEX as u64 {
            return None;
        }
        let pack_generation = canonical_uint(gen.strip_prefix('g')?)?;
        if pack_generation == 0 {
            return None; // generations start at 1
        }
        if !is_partition(part) {
            return None;
        }
        Some(PackHost {
            pack_index: pack_index as u16,
            pack_generation,
            pack_partition: part.to_string(),
        })
    }
}

impl fmt::Display for PackHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pack-{}-g{}-{}.{}",
            self.pack_index, self.pack_generation, self.pack_partition, PACK_ZONE
        )
    }
}

/// True when `s` is a well-formed `pack_partition`.
pub fn is_partition(s: &str) -> bool {
    s.len() == PARTITION_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A decimal integer written without leading zeros (contract §8), so every
/// pack has exactly one hostname per partition.
fn canonical_uint(s: &str) -> Option<u64> {
    if s.is_empty() || s.len() > 19 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

/// Normalise a `Host` header value: lowercase, port and one trailing dot
/// removed. `None` when it is empty or holds characters no hostname can.
pub fn normalize_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let host = match raw.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) if raw.starts_with('[') => return None, // IPv6 literal: never a box name
        _ => raw,
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return None;
    }
    Some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let h = PackHost {
            pack_index: 12,
            pack_generation: 3,
            pack_partition: "0123456789abcdef".into(),
        };
        let s = h.to_string();
        assert_eq!(s, "pack-12-g3-0123456789abcdef.deltos-packs.lan");
        assert_eq!(PackHost::parse(&s), Some(h));
    }

    #[test]
    fn refuses_everything_else() {
        for bad in [
            "pack-12-g3-0123456789abcdef.deltos.lan",
            "pack-012-g3-0123456789abcdef.deltos-packs.lan",
            "pack-12-g03-0123456789abcdef.deltos-packs.lan",
            "pack-12-g0-0123456789abcdef.deltos-packs.lan",
            "pack-512-g1-0123456789abcdef.deltos-packs.lan",
            "pack-12-g1-0123456789ABCDEF.deltos-packs.lan",
            "pack-12-g1-0123456789abcde.deltos-packs.lan",
            "pack-12-g1-0123456789abcdef-x.deltos-packs.lan",
            "x.pack-12-g1-0123456789abcdef.deltos-packs.lan",
            "pack-12-1-0123456789abcdef.deltos-packs.lan",
            "deltos-packs.lan",
            "pack--g1-0123456789abcdef.deltos-packs.lan",
        ] {
            assert_eq!(PackHost::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn host_normalisation() {
        assert_eq!(normalize_host("Library.Deltos.LAN:8080").as_deref(), Some("library.deltos.lan"));
        assert_eq!(normalize_host("a.b.").as_deref(), Some("a.b"));
        assert_eq!(normalize_host("[::1]:80"), None);
        assert_eq!(normalize_host("evil.com/x"), None);
        assert_eq!(normalize_host(""), None);
    }
}
