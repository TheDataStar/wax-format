//! The capacity estimate (docs/base-addons-admin.md §4).

use deltos_apps::capacity::{estimate, Limit, Measured, Scenario};

fn measured() -> Measured {
    Measured {
        machine: "test".into(),
        cpu_cores: Some(1.0),
        page_cpu_seconds: Some(0.001),
        search_cpu_seconds: Some(0.002),
        page_bytes: Some(10_000.0),
        first_visit_bytes: Some(0.0),
        radio_max_stations: Some(8),
        radio_bits_per_second: Some(8_000_000.0),
        ram_free_bytes: Some(1_000_000_000.0),
        ram_bytes_per_active_user: Some(1_000_000.0),
        sign_in_ram_bytes: Some(20_000_000.0),
    }
}

fn scenario() -> Scenario {
    Scenario {
        name: "t".into(),
        pages_per_minute: 6.0,
        searches_per_minute: 3.0,
        first_visit_share: 0.0,
        ceiling: 0.5,
    }
}

#[test]
fn the_smallest_limit_wins() {
    let e = estimate(&measured(), &scenario());
    // radio stations: 8
    // radio data: 8e6*0.5 / (8*10000*6/60 = 8000 b/s) = 500
    // cpu: 1*0.5 / ((0.006+0.006)/60 = 0.0002) = 2500
    // ram: 1e9/1e6 = 1000
    let by = |l: Limit| e.limits.iter().find(|r| r.limit == l).unwrap().people;
    assert_eq!(by(Limit::RadioStations), Some(8));
    assert_eq!(by(Limit::RadioData), Some(500));
    assert_eq!(by(Limit::Cpu), Some(2500));
    assert_eq!(by(Limit::Ram), Some(1000));
    assert_eq!(e.people, Some(8));
    assert_eq!(e.limited_by, Some(Limit::RadioStations));
    assert!(e.complete);
    assert_eq!(e.sign_ins_at_once, Some(50));
}

#[test]
fn a_missing_measurement_is_reported_not_guessed() {
    let mut m = measured();
    m.radio_max_stations = None;
    m.cpu_cores = None;
    let e = estimate(&m, &scenario());
    assert!(!e.complete);
    assert_eq!(e.people, Some(500), "smallest of the limits that are known");
    let st = e
        .limits
        .iter()
        .find(|r| r.limit == Limit::RadioStations)
        .unwrap();
    assert_eq!(st.people, None);
    assert_eq!(st.missing, vec!["radio_max_stations"]);
    let cpu = e.limits.iter().find(|r| r.limit == Limit::Cpu).unwrap();
    assert_eq!(cpu.missing, vec!["cpu_cores"]);
}

#[test]
fn first_visits_cost_radio_data() {
    let mut m = measured();
    m.first_visit_bytes = Some(1_000_000.0);
    let mut s = scenario();
    s.first_visit_share = 0.1;
    // per person: 8*(10000*6 + 1e6*0.1)/60 = 8*160000/60 = 21333 b/s -> 4e6/21333 = 187
    let e = estimate(&m, &s);
    assert_eq!(
        e.limits
            .iter()
            .find(|r| r.limit == Limit::RadioData)
            .unwrap()
            .people,
        Some(187)
    );
}

#[test]
fn the_shipped_measurement_files_parse() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/capacity"));
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let text = std::fs::read_to_string(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with("measured-") {
            toml::from_str::<Measured>(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            n += 1;
        } else if name.starts_with("scenario-") {
            toml::from_str::<Scenario>(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            n += 1;
        }
    }
    assert!(n >= 3);
}
