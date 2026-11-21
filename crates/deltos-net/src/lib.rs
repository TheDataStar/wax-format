//! `deltos-net` — E10, the box's own network: the name and address
//! registry, and the `dnsmasq` and `nftables` settings generated from it.
//! Design: `docs/box-network-plan.md`.
//!
//! * [`registry`] — settings, approved names, reservations (SQLite, versioned).
//! * [`render`] — the generated files; names and reservations take effect
//!   without a restart.
//! * [`actions`] — the closed list of network changes, only through the
//!   privilege boundary's checks.
//! * [`radio`] — the hotspot's Wi-Fi settings and the `hostapd` file (E3).
//! * [`leases`] — reading `dnsmasq`'s lease file.
//! * [`connectivity`] — the closed list of phones' internet checks `deltd`
//!   answers.

pub mod actions;
pub mod connectivity;
pub mod leases;
pub mod radio;
pub mod registry;
pub mod render;

pub use actions::{apply, authorize, Action, Authorized, Caller, IdentityCheck, Refusal};
pub use radio::{Radio, Secret};
pub use registry::{Mode, NetError, Registry, Settings, Target, WEBOS_HOST};
pub use render::{render, write, Layout, Reload, Rendered};
