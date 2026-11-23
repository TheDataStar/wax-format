//! How many people at once can the fed WebOS serve? (`docs/base-addons-admin.md` §4.)
//!
//! The visitor's own device draws the whole DeltOS screen, windows included,
//! so the box only answers requests. Four things can run out, and the
//! smallest wins:
//!
//! 1. **Radio**: how many devices the box's Wi-Fi lets join, and how much data
//!    it can carry.
//! 2. **CPU**: the box's time to answer pages and searches.
//! 3. **RAM**: what is left after every installed app's floor.
//! 4. **Sign-in bursts**: password checks cost memory each (argon2), so a room
//!    of teachers signing in at once is its own limit.
//!
//! Every input is `Option`: a number nobody measured stays `None`, and the
//! limit it drives is reported as not measured, never estimated.

use serde::{Deserialize, Serialize};

/// What the box was measured to do. Each figure carries the machine it came from.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    /// Where these figures were measured, in words.
    pub machine: String,
    /// CPU cores the box gives to answering requests.
    pub cpu_cores: Option<f64>,
    /// CPU seconds one core spends on one page (median page of a real pack).
    pub page_cpu_seconds: Option<f64>,
    /// CPU seconds one core spends on one search across the installed packs.
    pub search_cpu_seconds: Option<f64>,
    /// Mean bytes sent per page view, including its images and styles.
    pub page_bytes: Option<f64>,
    /// Bytes a device fetches the first time: the WebOS shell plus a pack's
    /// shared styles and scripts. Ordinary HTTP caching covers later visits.
    pub first_visit_bytes: Option<f64>,
    /// Devices the radio lets join at once.
    pub radio_max_stations: Option<u32>,
    /// Data the radio carried in practice, bits per second, all devices together.
    pub radio_bits_per_second: Option<f64>,
    /// RAM free for people after every installed floor (box RAM minus the sum).
    pub ram_free_bytes: Option<f64>,
    /// Extra RAM for each person active at once (sum of the apps' `load`).
    pub ram_bytes_per_active_user: Option<f64>,
    /// RAM one password check holds while it runs (argon2 memory cost).
    pub sign_in_ram_bytes: Option<f64>,
}

/// How people use the box: a scenario to compare, not a measured fact.
/// The dashboard replaces it with the box's own counts (F14, aggregate only)
/// once a box has run for a while.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    /// Page views per active person per minute.
    pub pages_per_minute: f64,
    /// Searches per active person per minute.
    pub searches_per_minute: f64,
    /// Share of active people opening DeltOS for the first time in the same minute.
    pub first_visit_share: f64,
    /// Share of the CPU and the radio the box may use before it counts as full
    /// (leaves room for bursts and for everything else the box does).
    pub ceiling: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    RadioStations,
    RadioData,
    Cpu,
    Ram,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitResult {
    pub limit: Limit,
    /// People active at once this limit allows, or `None` when an input is missing.
    pub people: Option<u32>,
    /// Which inputs were missing.
    pub missing: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Estimate {
    pub machine: String,
    pub scenario: String,
    pub limits: Vec<LimitResult>,
    /// The smallest known limit. `complete` is false when some limit is unmeasured,
    /// so the true answer may be lower still.
    pub people: Option<u32>,
    pub limited_by: Option<Limit>,
    pub complete: bool,
    /// Password checks the box can run at once without running out of RAM.
    pub sign_ins_at_once: Option<u32>,
}

fn floor_u32(x: f64) -> u32 {
    if x.is_finite() && x > 0.0 {
        x.floor().min(u32::MAX as f64) as u32
    } else {
        0
    }
}

pub fn estimate(m: &Measured, s: &Scenario) -> Estimate {
    let mut limits = Vec::new();

    limits.push(LimitResult {
        limit: Limit::RadioStations,
        people: m.radio_max_stations,
        missing: if m.radio_max_stations.is_none() {
            vec!["radio_max_stations"]
        } else {
            vec![]
        },
    });

    // Bits per active person per second.
    let data = {
        let mut missing = vec![];
        if m.radio_bits_per_second.is_none() {
            missing.push("radio_bits_per_second");
        }
        if m.page_bytes.is_none() {
            missing.push("page_bytes");
        }
        if m.first_visit_bytes.is_none() && s.first_visit_share > 0.0 {
            missing.push("first_visit_bytes");
        }
        let people = if missing.is_empty() {
            let per_person = 8.0
                * (m.page_bytes.unwrap() * s.pages_per_minute
                    + m.first_visit_bytes.unwrap_or(0.0) * s.first_visit_share)
                / 60.0;
            Some(if per_person <= 0.0 {
                u32::MAX
            } else {
                floor_u32(m.radio_bits_per_second.unwrap() * s.ceiling / per_person)
            })
        } else {
            None
        };
        LimitResult {
            limit: Limit::RadioData,
            people,
            missing,
        }
    };
    limits.push(data);

    let cpu = {
        let mut missing = vec![];
        if m.cpu_cores.is_none() {
            missing.push("cpu_cores");
        }
        if m.page_cpu_seconds.is_none() {
            missing.push("page_cpu_seconds");
        }
        if m.search_cpu_seconds.is_none() && s.searches_per_minute > 0.0 {
            missing.push("search_cpu_seconds");
        }
        let people = if missing.is_empty() {
            let per_person = (m.page_cpu_seconds.unwrap() * s.pages_per_minute
                + m.search_cpu_seconds.unwrap_or(0.0) * s.searches_per_minute)
                / 60.0;
            Some(if per_person <= 0.0 {
                u32::MAX
            } else {
                floor_u32(m.cpu_cores.unwrap() * s.ceiling / per_person)
            })
        } else {
            None
        };
        LimitResult {
            limit: Limit::Cpu,
            people,
            missing,
        }
    };
    limits.push(cpu);

    let ram = {
        let mut missing = vec![];
        if m.ram_free_bytes.is_none() {
            missing.push("ram_free_bytes");
        }
        if m.ram_bytes_per_active_user.is_none() {
            missing.push("ram_bytes_per_active_user");
        }
        let people = if missing.is_empty() {
            let per = m.ram_bytes_per_active_user.unwrap();
            Some(if per <= 0.0 {
                u32::MAX
            } else {
                floor_u32(m.ram_free_bytes.unwrap() / per)
            })
        } else {
            None
        };
        LimitResult {
            limit: Limit::Ram,
            people,
            missing,
        }
    };
    limits.push(ram);

    let known: Vec<&LimitResult> = limits.iter().filter(|l| l.people.is_some()).collect();
    let best = known.iter().min_by_key(|l| l.people.unwrap());
    let sign_ins_at_once = match (m.ram_free_bytes, m.sign_in_ram_bytes) {
        (Some(f), Some(p)) if p > 0.0 => Some(floor_u32(f / p)),
        _ => None,
    };
    Estimate {
        machine: m.machine.clone(),
        scenario: s.name.clone(),
        people: best.map(|l| l.people.unwrap()),
        limited_by: best.map(|l| l.limit.clone()),
        complete: known.len() == limits.len(),
        limits,
        sign_ins_at_once,
    }
}
