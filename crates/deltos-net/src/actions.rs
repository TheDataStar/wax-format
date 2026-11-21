//! Changing the network, behind the privilege boundary.
//!
//! These are `deltos-shelld` actions (docs/privilege-boundary.md §5, §8),
//! and this module is the shape `deltos-shelld` will call. The rule it
//! enforces in the type system: **the registry cannot be changed except
//! through [`apply`], and `apply` takes only an [`Authorized`], which only
//! [`authorize`] can make**, after shelld's own checks against `identityd`:
//!
//! 1. the session is live **and secure** (made over HTTPS; rule 9);
//! 2. its person holds `network.configure` (rule 4, the second of the two checks);
//! 3. for an action that asks again, a single-use confirmation ticket bound to
//!    this session, this action and a digest of these exact arguments is
//!    redeemed with `identityd` (§6).
//!
//! The list is closed (rule 5) and every argument has a fixed, validated
//! shape (rule 6). There is no "run this command" action, and no command-line
//! tool that changes anything: `deltos-net-render` only writes the files the
//! registry already implies.

use std::net::Ipv4Addr;

use sha2::{Digest, Sha256};

use crate::leases::{self, Lease};
use crate::radio::{Radio, Secret};
use crate::registry::{check_label, check_mac, check_service_name, ApprovedName, NetError, Registry, Reservation, Target};
use crate::render::{self, Layout, Reload};

/// The permission every network action needs (contract §4).
pub const PERMISSION: &str = "network.configure";

/// The closed list of network actions this crate performs. `configure_ap`
/// (privilege boundary §8) is E3's hotspot radio. `join_network` (joining
/// another Wi-Fi network as a client) is not built yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A requested name is approved (Track E §23.3). `requested_by` is the
    /// profile that asked; the approver is the caller.
    ApproveName { name: String, target: Target, requested_by: String },
    RemoveName { name: String },
    SetAddressReservation { mac: String, address: Ipv4Addr, label: String },
    RemoveAddressReservation { mac: String },
    /// Hotspot devices reach the internet through the box, or stop.
    SetUplinkSharing { on: bool },
    /// Change the box's own hotspot: its name, password, country and channel
    /// (privilege boundary §8, Track E §6). The password never prints.
    ConfigureAp { ssid: String, passphrase: Secret, country: String, channel: u8 },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::ApproveName { .. } => "approve_name",
            Action::RemoveName { .. } => "remove_name",
            Action::SetAddressReservation { .. } => "set_address_reservation",
            Action::RemoveAddressReservation { .. } => "remove_address_reservation",
            Action::SetUplinkSharing { .. } => "set_uplink_sharing",
            Action::ConfigureAp { .. } => "configure_ap",
        }
    }

    /// Asks for the password again (plan §7): turning sharing on changes
    /// what leaves the box; changing the hotspot can cut every phone off it
    /// (privilege boundary §8).
    pub fn needs_reconfirm(&self) -> bool {
        matches!(self, Action::SetUplinkSharing { .. } | Action::ConfigureAp { .. })
    }

    /// The canonical argument string a confirmation ticket is bound to.
    pub fn canonical_args(&self) -> String {
        match self {
            Action::ApproveName { name, target, requested_by } => {
                let t = match target {
                    Target::Box => "box".to_string(),
                    Target::Address(a) => a.to_string(),
                };
                format!("name={name}\ntarget={t}\nrequested_by={requested_by}")
            }
            Action::RemoveName { name } => format!("name={name}"),
            Action::SetAddressReservation { mac, address, label } => format!("mac={mac}\naddress={address}\nlabel={label}"),
            Action::RemoveAddressReservation { mac } => format!("mac={mac}"),
            Action::SetUplinkSharing { on } => format!("on={on}"),
            // the password is bound by its digest, so the ticket binds the
            // exact password without the argument string ever holding it
            Action::ConfigureAp { ssid, passphrase, country, channel } => {
                let pd: String = Sha256::digest(passphrase.expose().as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
                format!("ssid={ssid}\npassphrase_sha256={pd}\ncountry={country}\nchannel={channel}")
            }
        }
    }

    /// SHA-256 of `name ‖ "\n" ‖ canonical_args`, hex.
    pub fn args_digest(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.name().as_bytes());
        h.update(b"\n");
        h.update(self.canonical_args().as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Fixed argument shapes (rule 6), normalised.
    fn normalised(self) -> Result<Action, NetError> {
        Ok(match self {
            Action::ApproveName { name, target, requested_by } => {
                if requested_by.is_empty() || requested_by.len() > 128 {
                    return Err(NetError::Invalid("requested_by".into()));
                }
                if let Target::Address(a) = target {
                    if !a.is_private() {
                        return Err(NetError::Invalid(format!("{a}: a name may only point at a device on the box's own network")));
                    }
                }
                Action::ApproveName { name: check_service_name(&name)?, target, requested_by }
            }
            Action::RemoveName { name } => Action::RemoveName { name: check_service_name(&name)? },
            Action::SetAddressReservation { mac, address, label } => {
                let label = label.trim().to_ascii_lowercase();
                check_label(&label)?;
                Action::SetAddressReservation { mac: check_mac(&mac)?, address, label }
            }
            Action::RemoveAddressReservation { mac } => Action::RemoveAddressReservation { mac: check_mac(&mac)? },
            a @ Action::SetUplinkSharing { .. } => a,
            Action::ConfigureAp { ssid, passphrase, country, channel } => {
                let r = Radio { ssid, passphrase, country: country.trim().to_ascii_uppercase(), channel };
                r.validate()?;
                Action::ConfigureAp { ssid: r.ssid, passphrase: r.passphrase, country: r.country, channel: r.channel }
            }
        })
    }
}

/// What `deltos-shelld` asks `identityd` (privilege boundary §5 step 6, §6).
pub trait IdentityCheck {
    /// The live session's person and whether it was made over HTTPS.
    fn session(&self, session_ref: &str) -> Option<(String, bool)>;
    fn has_permission(&self, profile_id: &str, permission: &str) -> bool;
    /// Redeem a single-use ticket bound to this session, action and digest.
    fn redeem_ticket(&self, ticket: &str, session_ref: &str, action: &str, args_digest: &str) -> bool;
}

/// What the request carried to `deltos-shelld`.
pub struct Caller<'a> {
    pub session_ref: &'a str,
    /// The request reached the box over HTTPS (privilege boundary §4.1).
    pub over_https: bool,
    pub confirmation_ticket: Option<&'a str>,
}

/// Why an action was refused, in deltd API §7's codes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("not_signed_in")]
    NotSignedIn,
    #[error("secure_connection_required")]
    SecureConnectionRequired,
    #[error("permission_denied")]
    PermissionDenied,
    #[error("confirmation_required")]
    ConfirmationRequired,
    #[error("invalid_request: {0}")]
    Invalid(String),
}

/// An action that passed every check. Only [`authorize`] makes one.
#[derive(Debug)]
pub struct Authorized {
    action: Action,
    profile_id: String,
}

impl Authorized {
    pub fn action(&self) -> &Action {
        &self.action
    }
    /// Who to name in the action log (rule 12).
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
}

fn check_caller(caller: &Caller, id: &dyn IdentityCheck) -> Result<String, Refusal> {
    let (profile, secure) = id.session(caller.session_ref).ok_or(Refusal::NotSignedIn)?;
    if !caller.over_https || !secure {
        return Err(Refusal::SecureConnectionRequired);
    }
    if !id.has_permission(&profile, PERMISSION) {
        return Err(Refusal::PermissionDenied);
    }
    Ok(profile)
}

/// shelld's checks for one action. The ticket is redeemed only after every
/// other check passed, so a refused request never spends it.
pub fn authorize(action: Action, caller: &Caller, id: &dyn IdentityCheck) -> Result<Authorized, Refusal> {
    let profile = check_caller(caller, id)?;
    let action = action.normalised().map_err(|e| Refusal::Invalid(e.to_string()))?;
    if action.needs_reconfirm() {
        let t = caller.confirmation_ticket.ok_or(Refusal::ConfirmationRequired)?;
        if !id.redeem_ticket(t, caller.session_ref, action.name(), &action.args_digest()) {
            return Err(Refusal::ConfirmationRequired);
        }
    }
    Ok(Authorized { action, profile_id: profile })
}

/// Perform an authorized action and regenerate the settings files.
pub fn apply(reg: &mut Registry, a: Authorized, layout: &Layout) -> Result<Reload, NetError> {
    match a.action {
        Action::ApproveName { name, target, requested_by } => {
            reg.add_name(&ApprovedName { name, target, requested_by, approved_by: a.profile_id })?
        }
        Action::RemoveName { name } => reg.remove_name(&name)?,
        Action::SetAddressReservation { mac, address, label } => reg.set_reservation(&Reservation { mac, address, label })?,
        Action::RemoveAddressReservation { mac } => reg.remove_reservation(&mac)?,
        Action::SetUplinkSharing { on } => reg.set_uplink_sharing(on)?,
        Action::ConfigureAp { ssid, passphrase, country, channel } => {
            if !matches!(reg.settings()?.mode, crate::Mode::Hotspot { .. }) {
                return Err(NetError::Invalid("the box is joined to another network and runs no hotspot".into()));
            }
            reg.set_radio(&Radio { ssid, passphrase, country, channel })?
        }
    }
    render::write(reg, layout)
}

/// The hotspot's current leases (`GET /network/leases`): the same checks
/// as an action, without re-confirmation. Leases are personal data (plan §4).
pub fn list_leases(caller: &Caller, id: &dyn IdentityCheck, layout: &Layout, now_unix: u64) -> Result<Vec<Lease>, Refusal> {
    check_caller(caller, id)?;
    Ok(leases::read(&layout.leases(), now_unix))
}
