//! The box's name and address registry (docs/box-network-plan.md §4).
//!
//! One small SQLite table set, carrying a schema version that a newer one is
//! refused against (contract §10). It holds what the box decides about its
//! own network; leases are `dnsmasq`'s and are only read ([`crate::leases`]).

use std::net::Ipv4Addr;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::radio::{Radio, Secret};

/// Schema version, in `PRAGMA user_version`. 2 added the hotspot's radio
/// settings (E3); a version-1 registry is upgraded in place on open.
pub const REGISTRY_SCHEMA_VERSION: i64 = 2;

/// The zone service names live in (contract §8).
pub const SERVICE_ZONE: &str = "deltos.lan";
/// The pack zone (contract §8). The registry never holds a name in it:
/// pack hostnames are issued by `identityd` and answered by the wildcard.
pub const PACK_ZONE: &str = "deltos-packs.lan";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetError {
    #[error("registry schema version {0} is newer than this build understands ({REGISTRY_SCHEMA_VERSION})")]
    NewerSchema(i64),
    #[error("the registry has no network settings yet")]
    NotConfigured,
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0} does not exist")]
    Missing(String),
    #[error("registry database: {0}")]
    Sql(String),
    #[error("writing settings: {0}")]
    Io(String),
}

impl From<rusqlite::Error> for NetError {
    fn from(e: rusqlite::Error) -> Self {
        NetError::Sql(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, NetError>;

/// How the box meets phones (plan §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// The box runs its own Wi-Fi: DNS and DHCP on `interface` (plan §3.1).
    Hotspot { interface: String, box_address: Ipv4Addr, prefix_len: u8 },
    /// The box joined someone else's network: DNS only, never DHCP (plan §3.2).
    Joined { interface: String, box_address: Ipv4Addr },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub mode: Mode,
    /// The interface with internet, if any.
    pub uplink_interface: Option<String>,
    /// Upstream resolvers learned from the uplink. Empty: the box is offline
    /// and answers every other name "no such name" at once (plan §2).
    pub upstream_dns: Vec<Ipv4Addr>,
    /// Whether hotspot devices reach the internet through the box (plan §3.1).
    /// Off unless an admin turned it on (contract §16).
    pub uplink_sharing: bool,
}

impl Settings {
    pub fn box_address(&self) -> Ipv4Addr {
        match &self.mode {
            Mode::Hotspot { box_address, .. } | Mode::Joined { box_address, .. } => *box_address,
        }
    }

    /// The hotspot's network and prefix, when the box runs one.
    pub fn hotspot_net(&self) -> Option<(Ipv4Addr, u8)> {
        match &self.mode {
            Mode::Hotspot { box_address, prefix_len, .. } => {
                let mask = mask(*prefix_len);
                Some((Ipv4Addr::from(u32::from(*box_address) & mask), *prefix_len))
            }
            Mode::Joined { .. } => None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let iface_ok = |s: &str| !s.is_empty() && s.len() <= 15 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
        let (iface, addr) = match &self.mode {
            Mode::Hotspot { interface, box_address, prefix_len } => {
                if !(16..=30).contains(prefix_len) {
                    return Err(NetError::Invalid(format!("hotspot prefix /{prefix_len} must be /16 to /30")));
                }
                if !box_address.is_private() {
                    return Err(NetError::Invalid(format!("hotspot address {box_address} must be a private (RFC 1918) address")));
                }
                let host = u32::from(*box_address) & !mask(*prefix_len);
                if host == 0 || host == !mask(*prefix_len) {
                    return Err(NetError::Invalid(format!("{box_address} is the network or broadcast address")));
                }
                (interface, box_address)
            }
            Mode::Joined { interface, box_address } => (interface, box_address),
        };
        if !iface_ok(iface) {
            return Err(NetError::Invalid(format!("interface name {iface:?}")));
        }
        if addr.is_unspecified() || addr.is_loopback() || addr.is_multicast() || addr.is_broadcast() {
            return Err(NetError::Invalid(format!("box address {addr}")));
        }
        if let Some(u) = &self.uplink_interface {
            if !iface_ok(u) {
                return Err(NetError::Invalid(format!("uplink interface name {u:?}")));
            }
        }
        if self.uplink_sharing && (self.uplink_interface.is_none() || !matches!(self.mode, Mode::Hotspot { .. })) {
            return Err(NetError::Invalid("uplink sharing needs a hotspot and an uplink interface".into()));
        }
        Ok(())
    }
}

pub(crate) fn mask(prefix: u8) -> u32 {
    if prefix == 0 { 0 } else { u32::MAX << (32 - prefix as u32) }
}

/// Where an approved name points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The box itself. Already answered by the wildcard; recorded so the
    /// box's services know the name is theirs.
    Box,
    /// Another device on the box's network: a second box, a printer.
    Address(Ipv4Addr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedName {
    /// Full name, `<label>.deltos.lan`.
    pub name: String,
    pub target: Target,
    pub requested_by: String,
    pub approved_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    /// Lowercase `aa:bb:cc:dd:ee:ff`.
    pub mac: String,
    pub address: Ipv4Addr,
    pub label: String,
}

/// A service name: exactly one RFC 1123 label under `deltos.lan` (contract
/// §8: `<service>.deltos.lan`). Returns the normalised full name.
pub fn check_service_name(name: &str) -> Result<String> {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    let label = n
        .strip_suffix(&format!(".{SERVICE_ZONE}"))
        .ok_or_else(|| NetError::Invalid(format!("{name:?} is not a name under {SERVICE_ZONE}")))?;
    check_label(label)?;
    Ok(n)
}

/// One RFC 1123 hostname label.
pub fn check_label(label: &str) -> Result<()> {
    let ok = !label.is_empty()
        && label.len() <= 63
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-');
    if ok { Ok(()) } else { Err(NetError::Invalid(format!("{label:?} is not a valid name (a–z, 0–9, '-', up to 63)"))) }
}

pub fn check_mac(mac: &str) -> Result<String> {
    let m = mac.trim().to_ascii_lowercase();
    let parts: Vec<&str> = m.split(':').collect();
    let ok = parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()));
    if !ok {
        return Err(NetError::Invalid(format!("{mac:?} is not a MAC address")));
    }
    let first = u8::from_str_radix(parts[0], 16).unwrap();
    if first & 1 == 1 {
        return Err(NetError::Invalid(format!("{mac} is a multicast address")));
    }
    Ok(m)
}

pub struct Registry {
    conn: Connection,
}

impl Registry {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v > REGISTRY_SCHEMA_VERSION {
            return Err(NetError::NewerSchema(v));
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 mode TEXT NOT NULL CHECK (mode IN ('hotspot','joined')),
                 interface TEXT NOT NULL,
                 box_address TEXT NOT NULL,
                 prefix_len INTEGER,
                 uplink_interface TEXT,
                 upstream_dns TEXT NOT NULL DEFAULT '',
                 uplink_sharing INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS names (
                 name TEXT PRIMARY KEY,
                 target TEXT NOT NULL,          -- 'box' or a dotted IPv4 address
                 requested_by TEXT NOT NULL,
                 approved_by TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS reservations (
                 mac TEXT PRIMARY KEY,
                 address TEXT NOT NULL UNIQUE,
                 label TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS radio (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 ssid TEXT NOT NULL,
                 passphrase TEXT NOT NULL,
                 country TEXT NOT NULL,
                 channel INTEGER NOT NULL
             );",
        )?;
        conn.execute_batch(&format!("PRAGMA user_version = {REGISTRY_SCHEMA_VERSION};"))?;
        Ok(Registry { conn })
    }

    pub fn settings(&self) -> Result<Settings> {
        let row = self
            .conn
            .query_row(
                "SELECT mode, interface, box_address, prefix_len, uplink_interface, upstream_dns, uplink_sharing FROM settings WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or(NetError::NotConfigured)?;
        let addr: Ipv4Addr = row.2.parse().map_err(|_| NetError::Sql(format!("bad stored address {:?}", row.2)))?;
        let mode = match row.0.as_str() {
            "hotspot" => Mode::Hotspot {
                interface: row.1,
                box_address: addr,
                prefix_len: row.3.ok_or_else(|| NetError::Sql("hotspot without a prefix length".into()))? as u8,
            },
            _ => Mode::Joined { interface: row.1, box_address: addr },
        };
        let upstream_dns = row
            .5
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(|_| NetError::Sql(format!("bad stored resolver {s:?}"))))
            .collect::<Result<_>>()?;
        Ok(Settings { mode, uplink_interface: row.4, upstream_dns, uplink_sharing: row.6 != 0 })
    }

    /// Store the settings. Provisioning (Track E, E7) and the
    /// `configure_ap` / `join_network` actions write these.
    pub fn set_settings(&mut self, s: &Settings) -> Result<()> {
        s.validate()?;
        let (mode, iface, addr, prefix) = match &s.mode {
            Mode::Hotspot { interface, box_address, prefix_len } => ("hotspot", interface, box_address, Some(*prefix_len as i64)),
            Mode::Joined { interface, box_address } => ("joined", interface, box_address, None),
        };
        let up: Vec<String> = s.upstream_dns.iter().map(|a| a.to_string()).collect();
        self.conn.execute(
            "INSERT INTO settings (id, mode, interface, box_address, prefix_len, uplink_interface, upstream_dns, uplink_sharing)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET mode=?1, interface=?2, box_address=?3, prefix_len=?4,
                 uplink_interface=?5, upstream_dns=?6, uplink_sharing=?7",
            params![mode, iface, addr.to_string(), prefix, s.uplink_interface, up.join(","), s.uplink_sharing as i64],
        )?;
        Ok(())
    }

    pub fn names(&self) -> Result<Vec<ApprovedName>> {
        let mut st = self.conn.prepare("SELECT name, target, requested_by, approved_by FROM names ORDER BY name")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (name, t, requested_by, approved_by) = r?;
            let target = if t == "box" {
                Target::Box
            } else {
                Target::Address(t.parse().map_err(|_| NetError::Sql(format!("bad stored target {t:?}")))?)
            };
            out.push(ApprovedName { name, target, requested_by, approved_by });
        }
        Ok(out)
    }

    pub(crate) fn add_name(&mut self, n: &ApprovedName) -> Result<()> {
        let t = match n.target {
            Target::Box => "box".to_string(),
            Target::Address(a) => a.to_string(),
        };
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO names VALUES (?1, ?2, ?3, ?4)",
            params![n.name, t, n.requested_by, n.approved_by],
        )?;
        if added == 0 {
            return Err(NetError::Exists(n.name.clone()));
        }
        Ok(())
    }

    pub(crate) fn remove_name(&mut self, name: &str) -> Result<()> {
        if self.conn.execute("DELETE FROM names WHERE name = ?1", [name])? == 0 {
            return Err(NetError::Missing(name.to_string()));
        }
        Ok(())
    }

    pub fn reservations(&self) -> Result<Vec<Reservation>> {
        let mut st = self.conn.prepare("SELECT mac, address, label FROM reservations ORDER BY address")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (mac, a, label) = r?;
            out.push(Reservation { mac, address: a.parse().map_err(|_| NetError::Sql(format!("bad stored address {a:?}")))?, label });
        }
        out.sort_by_key(|r| u32::from(r.address));
        Ok(out)
    }

    pub(crate) fn set_reservation(&mut self, r: &Reservation) -> Result<()> {
        let s = self.settings()?;
        let (net, prefix) = s
            .hotspot_net()
            .ok_or_else(|| NetError::Invalid("reservations need the box's own hotspot; the box never hands out addresses on another network".into()))?;
        let m = mask(prefix);
        let a = u32::from(r.address);
        if a & m != u32::from(net) || a & !m == 0 || a & !m == !m || r.address == s.box_address() {
            return Err(NetError::Invalid(format!("{} is not a free host address in {net}/{prefix}", r.address)));
        }
        let taken: Option<String> = self
            .conn
            .query_row("SELECT mac FROM reservations WHERE address = ?1 AND mac != ?2", params![r.address.to_string(), r.mac], |x| x.get(0))
            .optional()?;
        if let Some(other) = taken {
            return Err(NetError::Exists(format!("{} (reserved for {other})", r.address)));
        }
        self.conn.execute(
            "INSERT INTO reservations VALUES (?1, ?2, ?3)
             ON CONFLICT(mac) DO UPDATE SET address = ?2, label = ?3",
            params![r.mac, r.address.to_string(), r.label],
        )?;
        Ok(())
    }

    pub(crate) fn remove_reservation(&mut self, mac: &str) -> Result<()> {
        if self.conn.execute("DELETE FROM reservations WHERE mac = ?1", [mac])? == 0 {
            return Err(NetError::Missing(mac.to_string()));
        }
        Ok(())
    }

    pub(crate) fn set_uplink_sharing(&mut self, on: bool) -> Result<()> {
        let mut s = self.settings()?;
        s.uplink_sharing = on;
        self.set_settings(&s)
    }

    /// The hotspot's radio settings, if any were set.
    pub fn radio(&self) -> Result<Option<Radio>> {
        let r = self
            .conn
            .query_row("SELECT ssid, passphrase, country, channel FROM radio WHERE id = 1", [], |r| {
                Ok(Radio { ssid: r.get(0)?, passphrase: Secret::new(r.get::<_, String>(1)?), country: r.get(2)?, channel: r.get::<_, i64>(3)? as u8 })
            })
            .optional()?;
        Ok(r)
    }

    /// Store the radio settings. Provisioning (Track E, E7) and the
    /// `configure_ap` action write these. The registry file therefore holds
    /// the Wi-Fi password: it lives in a directory only root can read
    /// (`system/tmpfiles.d/deltos.conf`).
    pub fn set_radio(&mut self, r: &Radio) -> Result<()> {
        r.validate()?;
        self.conn.execute(
            "INSERT INTO radio (id, ssid, passphrase, country, channel) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET ssid = ?1, passphrase = ?2, country = ?3, channel = ?4",
            params![r.ssid, r.passphrase.expose(), r.country, r.channel as i64],
        )?;
        Ok(())
    }
}
