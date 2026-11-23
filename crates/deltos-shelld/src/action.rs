//! The closed list (privilege boundary rule 5): every action `deltos-shelld`
//! knows, read from a request into a fixed shape (rule 6). A name that is
//! not here is refused; there is no "run this command" action.
//!
//! Each family keeps its own type, its own checks and its own
//! `Authorized`, in the crate that owns what it changes. This module only
//! reads requests into them and names what each needs.

use serde::Deserialize;
use serde_json::Value;

use crate::addons;
use deltos_backup::Passphrase;
use deltos_net::{Secret, Target};

/// Every action on the list, in its owning crate's type.
#[derive(Debug, Clone)]
pub enum Action {
    Net(deltos_net::Action),
    Backup(deltos_backup::Action),
    Addon(addons::Action),
}

/// Built and dispatched by this shelld (privilege boundary §8). The rest of
/// §8 is designed and not built; each is refused by name with
/// [`NOT_BUILT`], so a caller can tell "not yet" from "never".
pub const BUILT: &[&str] = &[
    "approve_name",
    "remove_name",
    "set_address_reservation",
    "remove_address_reservation",
    "set_uplink_sharing",
    "configure_ap",
    "start_backup",
    "review_backup",
    "restore_backup",
    "install_addon",
    "update_addon",
    "roll_back_addon",
    "remove_addon",
    "set_addon_running",
];

/// On privilege boundary §8, not built yet (`docs/shelld.md` §6 says why each waits).
pub const NOT_BUILT: &[&str] = &[
    "install_pack",
    "remove_pack",
    "roll_back_pack",
    "mount_removable",
    "eject_removable",
    "convert_zim_from_drive",
    "stage_upload",
    "join_network",
    "restart_service",
    "pair_box",
    "apply_update",
    "roll_back_update",
    "power_box",
    "set_clock",
    "set_language",
    "set_home_screen",
    "save_support_file",
    "export_activity",
];

fn args<T: for<'de> Deserialize<'de>>(v: &Value) -> Result<T, String> {
    serde_json::from_value(v.clone()).map_err(|e| e.to_string())
}

// Argument shapes. `deny_unknown_fields`: an argument that is not in the
// shape is an error, never ignored (rule 6).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveName {
    name: String,
    /// `"box"`, or a private IPv4 address on the box's network.
    target: String,
    requested_by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    mac: String,
    address: std::net::Ipv4Addr,
    label: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mac {
    mac: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct On {
    on: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ap {
    ssid: String,
    passphrase: String,
    country: String,
    channel: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartBackup {
    drive: String,
    passphrase: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewBackup {
    drive: String,
    backup_id: String,
    passphrase: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreBackup {
    review_id: String,
    review_digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    source: String,
    id: String,
    version: String,
    #[serde(default)]
    accept_unmeasured: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdVersion {
    id: String,
    version: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    id: String,
    keep_data: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Running {
    id: String,
    on: bool,
}

fn pass(s: String) -> Result<Passphrase, String> {
    Passphrase::new(s).map_err(|e| e.to_string())
}

impl Action {
    /// Read one request's action name and arguments into its fixed shape.
    pub fn parse(name: &str, a: &Value) -> Result<Action, String> {
        use deltos_backup::Action as B;
        use deltos_net::Action as N;
        Ok(match name {
            "approve_name" => {
                let x: ApproveName = args(a)?;
                let target = if x.target == "box" {
                    Target::Box
                } else {
                    Target::Address(x.target.parse().map_err(|_| format!("target: {:?} is not \"box\" or an IPv4 address", x.target))?)
                };
                Action::Net(N::ApproveName { name: x.name, target, requested_by: x.requested_by })
            }
            "remove_name" => Action::Net(N::RemoveName { name: args::<Name>(a)?.name }),
            "set_address_reservation" => {
                let x: Reservation = args(a)?;
                Action::Net(N::SetAddressReservation { mac: x.mac, address: x.address, label: x.label })
            }
            "remove_address_reservation" => Action::Net(N::RemoveAddressReservation { mac: args::<Mac>(a)?.mac }),
            "set_uplink_sharing" => Action::Net(N::SetUplinkSharing { on: args::<On>(a)?.on }),
            "configure_ap" => {
                let x: Ap = args(a)?;
                Action::Net(N::ConfigureAp { ssid: x.ssid, passphrase: Secret::new(x.passphrase), country: x.country, channel: x.channel })
            }
            "start_backup" => {
                let x: StartBackup = args(a)?;
                Action::Backup(B::StartBackup { drive: x.drive, passphrase: pass(x.passphrase)? })
            }
            "review_backup" => {
                let x: ReviewBackup = args(a)?;
                Action::Backup(B::ReviewBackup { drive: x.drive, backup_id: x.backup_id, passphrase: pass(x.passphrase)? })
            }
            "restore_backup" => {
                let x: RestoreBackup = args(a)?;
                Action::Backup(B::RestoreBackup { review_id: x.review_id, review_digest: x.review_digest })
            }
            "install_addon" | "update_addon" => {
                let x: Install = args(a)?;
                let (source, id, version, accept_unmeasured) = (x.source, x.id, x.version, x.accept_unmeasured);
                Action::Addon(if name == "install_addon" {
                    addons::Action::InstallAddon { source, id, version, accept_unmeasured }
                } else {
                    addons::Action::UpdateAddon { source, id, version, accept_unmeasured }
                })
            }
            "roll_back_addon" => {
                let x: IdVersion = args(a)?;
                Action::Addon(addons::Action::RollBackAddon { id: x.id, version: x.version })
            }
            "remove_addon" => {
                let x: Remove = args(a)?;
                Action::Addon(addons::Action::RemoveAddon { id: x.id, keep_data: x.keep_data })
            }
            "set_addon_running" => {
                let x: Running = args(a)?;
                Action::Addon(addons::Action::SetAddonRunning { id: x.id, on: x.on })
            }
            n if NOT_BUILT.contains(&n) => return Err(format!("{n} is on the action list but not built yet")),
            n => return Err(format!("{n:?} is not an action")),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Action::Net(a) => a.name(),
            Action::Backup(a) => a.name(),
            Action::Addon(a) => a.name(),
        }
    }

    pub fn permission(&self) -> &'static str {
        match self {
            Action::Net(_) => deltos_net::actions::PERMISSION,
            Action::Backup(_) => deltos_backup::actions::PERMISSION,
            Action::Addon(a) => a.permission(),
        }
    }

    pub fn needs_reconfirm(&self) -> bool {
        match self {
            Action::Net(a) => a.needs_reconfirm(),
            Action::Backup(a) => a.needs_reconfirm(),
            Action::Addon(a) => a.needs_reconfirm(),
        }
    }

    /// What a confirmation ticket for this exact request is bound to
    /// (privilege boundary §6). deltd asks for it with `describe` before it
    /// has `identityd` issue the ticket, so both sides digest the same bytes.
    pub fn args_digest(&self) -> String {
        match self {
            Action::Net(a) => a.args_digest(),
            Action::Backup(a) => a.args_digest(),
            Action::Addon(a) => a.args_digest(),
        }
    }

    /// Long actions return a job (privilege boundary §8).
    pub fn is_long(&self) -> bool {
        match self {
            Action::Net(_) => false,
            Action::Backup(_) => true,
            Action::Addon(a) => a.is_long(),
        }
    }

    /// The log line's arguments, secrets as `[redacted]` (rule 12).
    pub fn summary(&self) -> String {
        use deltos_net::Action as N;
        match self {
            Action::Net(N::ConfigureAp { ssid, country, channel, .. }) => {
                format!("configure_ap ssid={ssid:?} passphrase=[redacted] country={country} channel={channel}")
            }
            // nothing secret in the other network actions
            Action::Net(a) => format!("{} {}", a.name(), a.canonical_args().replace('\n', " ")),
            Action::Backup(a) => a.summary(),
            Action::Addon(a) => a.summary(),
        }
    }
}
