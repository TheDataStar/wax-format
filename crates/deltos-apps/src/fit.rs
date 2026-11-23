//! The fit check: may this box run these apps, given what it measured about
//! itself and what is already installed? (`docs/base-addons-admin.md` §6.)
//!
//! The budget is **the sum of declared floors over the installed set**
//! (contract §6, §15.2), compared with the box's measured resources (§2.2,
//! §9). It is a comparison of numbers and nothing else (§2.3). Where a floor
//! is `open`, the check says it cannot tell, rather than counting it as zero.

use crate::manifest::{AppManifest, Arch, Bind, Evidence, Gpu, Kind, Profile, Proto};
use serde::{Deserialize, Serialize};

/// Contract §9's capability record, as written to
/// `/var/lib/deltos/capability.json`. Unknown fields are kept out of the way
/// so a newer record still reads.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Capability {
    pub profile: Profile,
    pub ram_bytes: u64,
    pub storage_bytes: u64,
    pub arch: BoxArch,
    pub gpu: String,
    #[serde(default)]
    pub has_wifi: bool,
    #[serde(default)]
    pub can_ap: bool,
    #[serde(default)]
    pub has_rtc: bool,
    /// **[Proposed 2026-11-22]** RAM the operating system itself uses with no
    /// DeltOS service running, measured at first boot (`docs/base-addons-admin.md`
    /// §6). Absent until a box measures it; the RAM sum is then unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_ram_bytes: Option<u64>,
}

/// A box is one architecture; `any` is only for what runs on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxArch {
    Aarch64,
    X86_64,
}

impl BoxArch {
    fn runs(self, a: Arch) -> bool {
        matches!(
            (self, a),
            (_, Arch::Any) | (BoxArch::Aarch64, Arch::Aarch64) | (BoxArch::X86_64, Arch::X86_64)
        )
    }
}

/// How sure the answer for one resource is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Sum {
    /// Every floor in the sum is known. `trust` is the weakest evidence in it.
    Known {
        needed: u64,
        have: u64,
        trust: Trust,
    },
    /// At least one floor is open: these apps have not been measured.
    Unknown {
        known_part: u64,
        have: u64,
        unmeasured: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    Provisional,
    Measured,
}

impl Sum {
    pub fn fits(&self) -> Option<bool> {
        match self {
            Sum::Known { needed, have, .. } => Some(needed <= have),
            Sum::Unknown {
                known_part, have, ..
            } => {
                if known_part > have {
                    Some(false) // already over, whatever the rest is
                } else {
                    None
                }
            }
        }
    }
}

/// A reason the set cannot be installed as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Blocker {
    /// Contract §3: the box's profile is below what the app needs.
    Profile {
        app: String,
        needs: String,
        have: String,
    },
    /// Contract §2.3: wrong architecture.
    Arch {
        app: String,
        needs: String,
        have: String,
    },
    /// Contract §2.3: `gpu: required` and the box measured none.
    Gpu {
        app: String,
    },
    /// A dependency is neither installed nor in the set.
    MissingDependency {
        app: String,
        needs: String,
    },
    /// Two apps want the same port.
    Port {
        port: u16,
        apps: Vec<String>,
    },
    /// Two apps want the same name under deltos.lan.
    Host {
        host: String,
        apps: Vec<String>,
    },
    /// Two apps define the same permission.
    Permission {
        name: String,
        apps: Vec<String>,
    },
    /// Already installed.
    AlreadyInstalled {
        app: String,
    },
    /// A base service is installed with the system, not from the store.
    BaseNotInstallable {
        app: String,
    },
    Ram,
    Storage,
}

/// The answer the dashboard and the store show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every check passes and every floor in the sums was measured.
    Fits,
    /// Every check passes, but some floor is only provisional.
    FitsProvisionally,
    /// Nothing blocks, but an open floor means the RAM or storage sum is unknown.
    CannotTell,
    /// At least one blocker.
    DoesNotFit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fit {
    pub verdict: Verdict,
    pub ram: Sum,
    pub storage: Sum,
    pub blockers: Vec<Blocker>,
    /// Ids of apps in the set whose licence goes to human review (contract §11).
    pub licence_review: Vec<String>,
}

/// The name the RAM sum uses for the operating system's own share.
pub const OS: &str = "operating system";

fn sum(apps: &[&AppManifest], have: u64, field: impl Fn(&AppManifest) -> Option<u64>) -> Sum {
    sum_from(0, Vec::new(), apps, have, field)
}

fn sum_from(
    start: u64,
    unmeasured: Vec<String>,
    apps: &[&AppManifest],
    have: u64,
    field: impl Fn(&AppManifest) -> Option<u64>,
) -> Sum {
    let mut total: u64 = start;
    let mut unmeasured = unmeasured;
    let mut trust = Trust::Measured;
    for a in apps {
        match (a.resources.evidence, field(a)) {
            (Evidence::Open, _) | (_, None) => unmeasured.push(a.id.clone()),
            (e, Some(n)) => {
                total = total.saturating_add(n);
                if e == Evidence::Provisional {
                    trust = Trust::Provisional;
                }
            }
        }
    }
    if unmeasured.is_empty() {
        Sum::Known {
            needed: total,
            have,
            trust,
        }
    } else {
        Sum::Unknown {
            known_part: total,
            have,
            unmeasured,
        }
    }
}

/// Check installing `set` (one app, or an app plus the dependencies it needs)
/// on a box with `cap` that already runs `installed`.
///
/// `free_storage_bytes` is measured now on the state filesystem (statvfs), not
/// taken from the capability record, which holds the total. Installed apps'
/// `min_storage_bytes` are already counted inside what is used, so storage
/// compares only the set's own needs with what is free.
pub fn check(
    cap: &Capability,
    installed: &[AppManifest],
    set: &[AppManifest],
    free_storage_bytes: u64,
) -> Fit {
    let mut blockers = Vec::new();

    for a in set {
        if a.kind == Kind::Base {
            blockers.push(Blocker::BaseNotInstallable { app: a.id.clone() });
        }
        if installed.iter().any(|i| i.id == a.id) {
            blockers.push(Blocker::AlreadyInstalled { app: a.id.clone() });
        }
        if a.min_profile > cap.profile {
            blockers.push(Blocker::Profile {
                app: a.id.clone(),
                needs: a.min_profile.wire().into(),
                have: cap.profile.wire().into(),
            });
        }
        if !cap.arch.runs(a.resources.arch) {
            blockers.push(Blocker::Arch {
                app: a.id.clone(),
                needs: a.resources.arch.wire().into(),
                have: match cap.arch {
                    BoxArch::Aarch64 => "aarch64".into(),
                    BoxArch::X86_64 => "x86_64".into(),
                },
            });
        }
        if a.resources.gpu == Some(Gpu::Required) && cap.gpu == "none" {
            blockers.push(Blocker::Gpu { app: a.id.clone() });
        }
        for d in &a.depends {
            if !installed.iter().any(|i| &i.id == d) && !set.iter().any(|s| &s.id == d) {
                blockers.push(Blocker::MissingDependency {
                    app: a.id.clone(),
                    needs: d.clone(),
                });
            }
        }
    }

    // Collisions across everything that would then be on the box.
    let all: Vec<&AppManifest> = installed.iter().chain(set.iter()).collect();
    let mut ports: Vec<((u16, Proto, Bind), Vec<String>)> = Vec::new();
    let mut hosts: Vec<(String, Vec<String>)> = Vec::new();
    let mut perms: Vec<(String, Vec<String>)> = Vec::new();
    for a in &all {
        for l in &a.run.listens {
            // A loopback and a LAN listener on the same port still collide on 0.0.0.0.
            let key = (l.port, l.proto, Bind::Loopback);
            match ports.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(a.id.clone()),
                None => ports.push((key, vec![a.id.clone()])),
            }
        }
        if let Some(h) = a.reach.as_ref().and_then(|r| r.host.clone()) {
            match hosts.iter_mut().find(|(k, _)| *k == h) {
                Some((_, v)) => v.push(a.id.clone()),
                None => hosts.push((h, vec![a.id.clone()])),
            }
        }
        for p in &a.defines {
            match perms.iter_mut().find(|(k, _)| *k == p.name) {
                Some((_, v)) => v.push(a.id.clone()),
                None => perms.push((p.name.clone(), vec![a.id.clone()])),
            }
        }
    }
    let touches_set = |apps: &Vec<String>| apps.iter().any(|id| set.iter().any(|s| &s.id == id));
    for ((port, _, _), apps) in ports {
        if apps.len() > 1 && touches_set(&apps) {
            blockers.push(Blocker::Port { port, apps });
        }
    }
    for (host, apps) in hosts {
        if apps.len() > 1 && touches_set(&apps) {
            blockers.push(Blocker::Host { host, apps });
        }
    }
    for (name, apps) in perms {
        if apps.len() > 1 && touches_set(&apps) {
            blockers.push(Blocker::Permission { name, apps });
        }
    }

    let ram = match cap.os_ram_bytes {
        Some(os) => sum_from(os, Vec::new(), &all, cap.ram_bytes, |a| {
            a.resources.min_ram_bytes
        }),
        None => sum_from(0, vec![OS.to_string()], &all, cap.ram_bytes, |a| {
            a.resources.min_ram_bytes
        }),
    };
    let set_refs: Vec<&AppManifest> = set.iter().collect();
    let storage = sum(&set_refs, free_storage_bytes, |a| {
        match (a.resources.min_storage_bytes, a.resources.install_bytes) {
            (Some(s), i) => Some(s.saturating_add(i.unwrap_or(0))),
            (None, _) => None,
        }
    });
    if ram.fits() == Some(false) {
        blockers.push(Blocker::Ram);
    }
    if storage.fits() == Some(false) {
        blockers.push(Blocker::Storage);
    }

    let verdict = if !blockers.is_empty() {
        Verdict::DoesNotFit
    } else if ram.fits().is_none() || storage.fits().is_none() {
        Verdict::CannotTell
    } else if matches!(
        ram,
        Sum::Known {
            trust: Trust::Provisional,
            ..
        }
    ) || matches!(
        storage,
        Sum::Known {
            trust: Trust::Provisional,
            ..
        }
    ) {
        Verdict::FitsProvisionally
    } else {
        Verdict::Fits
    };
    let licence_review = set
        .iter()
        .filter(|a| a.licence_review_required())
        .map(|a| a.id.clone())
        .collect();
    Fit {
        verdict,
        ram,
        storage,
        blockers,
        licence_review,
    }
}
