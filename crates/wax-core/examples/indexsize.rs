//! Directive 07 §3 gate: how big is a real FTS5 index over real pack content,
//! and does it fit inside the margin WAX currently has over ZIM?
//!
//! Measured before implementing, because the answer changes the design. The
//! index segment is raw SQLite inside the pack — it is not compressed the way
//! ZIM compresses its Xapian index into clusters — so the comparison is not
//! obviously winnable and is worth knowing early.
//!
//! usage: indexsize <pack.wax> [--limit N]

use wax_core::WaxReader;

fn is_text(mime: Option<&str>) -> bool {
    matches!(mime, Some(m) if m.starts_with("text/html") || m.starts_with("application/xhtml"))
}

/// Strip tags crudely. The real builder will index rendered text; this is close
/// enough to size an index and deliberately not a parser.
fn detag(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut depth = 0u32;
    for c in html.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pack = args.first().ok_or("usage: indexsize <pack.wax>")?;
    let limit: usize = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);

    let r = WaxReader::open(pack)?;

    let variants: [(&str, &str); 3] = [
        ("contentless, detail=full", "CREATE VIRTUAL TABLE s USING fts5(title, body, content='', tokenize='unicode61')"),
        ("contentless, detail=column", "CREATE VIRTUAL TABLE s USING fts5(title, body, content='', detail=column, tokenize='unicode61')"),
        ("contentless, detail=none", "CREATE VIRTUAL TABLE s USING fts5(title, body, content='', detail=none, tokenize='unicode61')"),
    ];

    let mut text_bytes = 0u64;
    let mut docs = 0u64;

    for (name, ddl) in variants {
        let f = tempfile::NamedTempFile::new()?;
        let c = rusqlite::Connection::open(f.path())?;
        c.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;")?;
        c.execute_batch(ddl)?;
        // The rowid -> path map a contentless index needs to answer with paths.
        c.execute_batch("CREATE TABLE d(id INTEGER PRIMARY KEY, path TEXT NOT NULL)")?;
        c.execute_batch("BEGIN")?;
        {
            let mut ins = c.prepare("INSERT INTO s(rowid,title,body) VALUES (?1,?2,?3)")?;
            let mut insd = c.prepare("INSERT INTO d(id,path) VALUES (?1,?2)")?;
            let mut n = 0u64;
            for e in r.entries() {
                let e = e?;
                if e.is_redirect() || !is_text(e.mime.as_deref()) {
                    continue;
                }
                if n as usize >= limit {
                    break;
                }
                let body = match r.read(&e.path) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let txt = detag(&String::from_utf8_lossy(&body));
                n += 1;
                if text_bytes == 0 || docs < n {
                    // only tally once, on the first variant
                }
                ins.execute(rusqlite::params![n as i64, e.title.clone().unwrap_or_default(), txt])?;
                insd.execute(rusqlite::params![n as i64, e.path])?;
                if name == variants[0].0 {
                    text_bytes += txt.len() as u64;
                }
            }
            if name == variants[0].0 {
                docs = n;
            }
        }
        c.execute_batch("COMMIT")?;
        c.execute_batch("INSERT INTO s(s) VALUES('optimize')").ok();
        c.cache_flush()?;
        drop(c);
        let sz = std::fs::metadata(f.path())?.len();
        println!(
            "  {name:<26} {sz:>12} bytes   {:.3}x indexed text",
            sz as f64 / text_bytes.max(1) as f64
        );
    }
    println!("  docs indexed: {docs}, plain text: {text_bytes} bytes");
    Ok(())
}
