//! A1b measurement harness: what does it cost to open a pack and serve from it?
//!
//! ```text
//! cargo run --release -p wax-core --example readbench -- <pack.wax> [--open-only]
//!     [--reads N] [--sample K] [--no-cache] [--hot] [--seed S]
//! ```
//!
//! Prints open latency (and its parts), iteration throughput, per-lookup and
//! per-read latency with the SQLite page cache cold and warm, lookup-cache hit
//! rates, and the number of index page reads each phase issued through the
//! VFS — the figure that predicts SD-card-class latency, where every page read
//! is a random I/O. Peak memory is measured from outside (see zim2wax's
//! README for the method); this binary only does the work.

use std::time::{Duration, Instant};
use wax_core::header::WaxHeader;
use wax_core::reader::ReadOptions;
use wax_core::segment::Segment;
use wax_core::{vfs, WaxReader, HEADER_LEN};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(pack) = args.first() else {
        eprintln!("usage: readbench <pack.wax> [--open-only] [--reads N] [--sample K] [--no-cache] [--hot] [--seed S]");
        std::process::exit(2);
    };
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str, default: usize| -> usize {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let open_only = flag("--open-only");
    let reads = value("--reads", 20_000);
    let sample = value("--sample", 4_096);
    let mut seed = value("--seed", 0x9E37_79B9) as u64;
    let opts = ReadOptions {
        verify_checksums: true,
        lookup_cache_entries: if flag("--no-cache") { 0 } else { ReadOptions::default().lookup_cache_entries },
    };

    // --- open, in parts ---------------------------------------------------
    let header = {
        let mut f = std::fs::File::open(pack).expect("open");
        let mut h = [0u8; HEADER_LEN];
        std::io::Read::read_exact(&mut f, &mut h).expect("header bytes");
        WaxHeader::parse(&h).expect("header")
    };

    let r0 = vfs::read_stats();
    let t = Instant::now();
    let seg = Segment::open_at(pack.as_ref(), header.index_offset, header.index_length).expect("segment");
    let t_seg = t.elapsed();
    let t = Instant::now();
    let vol = seg.first_nonzero_volume().expect("volume scan");
    let t_vol = t.elapsed();
    let t = Instant::now();
    let manifest_rows = seg.manifest().expect("manifest").len();
    let t_man = t.elapsed();
    let r1 = vfs::read_stats();
    drop(seg);

    let t = Instant::now();
    let reader = WaxReader::open_with(pack, opts).expect("open");
    let t_open = t.elapsed();
    let r2 = vfs::read_stats();

    println!(
        "open: total={} segments={} index_bytes={} | segment+meta={} volume_scan={} ({} rows read through {} page reads) manifest={} ({} rows)",
        ms(t_open),
        reader.segment_count(),
        header.index_length,
        ms(t_seg),
        ms(t_vol),
        if vol.is_some() { "nonzero" } else { "all-zero" },
        r1.0 - r0.0,
        ms(t_man),
        manifest_rows,
    );
    println!(
        "open: page_reads={} bytes={} (the second open, {} reads, is what a warm process pays)",
        r1.0 - r0.0,
        r1.1 - r0.1,
        r2.0 - r1.0
    );
    if open_only {
        return;
    }

    // --- one streaming pass: count + reservoir-sample paths ----------------
    let r0 = vfs::read_stats();
    let t = Instant::now();
    let mut reservoir: Vec<String> = Vec::with_capacity(sample);
    let mut n = 0usize;
    let mut redirects = 0usize;
    for e in reader.entries() {
        let e = e.expect("iterate");
        if e.redirect_to.is_some() {
            redirects += 1;
        }
        if reservoir.len() < sample {
            reservoir.push(e.path);
        } else {
            let j = (next(&mut seed) % (n as u64 + 1)) as usize;
            if j < sample {
                reservoir[j] = e.path;
            }
        }
        n += 1;
    }
    let t_iter = t.elapsed();
    let r1 = vfs::read_stats();
    println!(
        "iterate: entries={} redirects={} wall={} ({:.0} entries/s) page_reads={}",
        n,
        redirects,
        ms(t_iter),
        n as f64 / t_iter.as_secs_f64(),
        r1.0 - r0.0
    );

    // --- lookups: fresh readers ⇒ empty SQLite page caches -----------------
    // uniform over the sample, or skewed (80% of requests to a 64-path hot
    // set) when --hot is given: the shape a served pack actually sees.
    let hot = flag("--hot");
    let hot_set = 64.min(reservoir.len());
    let picks: Vec<&str> = (0..reads)
        .map(|_| {
            let r = next(&mut seed);
            let i = if hot && r % 10 < 8 {
                (r / 10 % hot_set as u64) as usize
            } else {
                (r % reservoir.len() as u64) as usize
            };
            reservoir[i].as_str()
        })
        .collect();
    println!(
        "workload: {} lookups, {}",
        reads,
        if hot {
            format!("80% to a {hot_set}-path hot set, 20% uniform over {}", reservoir.len())
        } else {
            format!("uniform over {} sampled paths", reservoir.len())
        }
    );

    let cold = WaxReader::open_with(pack, ReadOptions { lookup_cache_entries: 0, ..opts }).expect("open");
    let r0 = vfs::read_stats();
    let mut lat = Vec::with_capacity(reads);
    for p in &picks {
        let t = Instant::now();
        cold.entry(p).expect("entry");
        lat.push(t.elapsed());
    }
    let r1 = vfs::read_stats();
    report("lookup, no lookup cache, fresh SQLite cache", &lat, r1.0 - r0.0);

    let r0 = vfs::read_stats();
    let mut lat = Vec::with_capacity(reads);
    for p in &picks {
        let t = Instant::now();
        cold.entry(p).expect("entry");
        lat.push(t.elapsed());
    }
    let r1 = vfs::read_stats();
    report("lookup, no lookup cache, same SQLite cache again", &lat, r1.0 - r0.0);

    if opts.lookup_cache_entries > 0 {
        let cached = WaxReader::open_with(pack, opts).expect("open");
        let r0 = vfs::read_stats();
        let mut lat = Vec::with_capacity(reads);
        for p in &picks {
            let t = Instant::now();
            cached.entry(p).expect("entry");
            lat.push(t.elapsed());
        }
        let r1 = vfs::read_stats();
        let (h, m) = cached.cache_stats();
        report(
            &format!(
                "lookup, lookup cache of {} ({} hits / {} misses), fresh SQLite cache",
                opts.lookup_cache_entries, h, m
            ),
            &lat,
            r1.0 - r0.0,
        );
    }

    // --- full reads: resolve + blob + decompress + sha256 ------------------
    let r0 = vfs::read_stats();
    let mut lat = Vec::with_capacity(reads);
    let mut bytes = 0u64;
    for p in &picks {
        let t = Instant::now();
        bytes += cold.read(p).expect("read").len() as u64;
        lat.push(t.elapsed());
    }
    let r1 = vfs::read_stats();
    report(
        &format!("read (verify_checksums=on, {:.1} KiB mean)", bytes as f64 / reads as f64 / 1024.0),
        &lat,
        r1.0 - r0.0,
    );
}

fn report(label: &str, lat: &[Duration], page_reads: u64) {
    let mut v: Vec<u64> = lat.iter().map(|d| d.as_nanos() as u64).collect();
    v.sort_unstable();
    let pct = |p: f64| v[((v.len() - 1) as f64 * p) as usize] as f64 / 1000.0;
    let mean = v.iter().sum::<u64>() as f64 / v.len() as f64 / 1000.0;
    println!(
        "{label}: n={} mean={mean:.1}µs p50={:.1}µs p99={:.1}µs max={:.1}µs page_reads={page_reads} ({:.2}/op)",
        v.len(),
        pct(0.5),
        pct(0.99),
        pct(1.0),
        page_reads as f64 / v.len() as f64
    );
}

fn ms(d: Duration) -> String {
    format!("{:.2}ms", d.as_secs_f64() * 1000.0)
}

fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
