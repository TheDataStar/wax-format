//! What does a Kiwix ZIM spend on its own full-text index?
//!
//! Directive 07 §3 gates on WAX+index vs ZIM+index. The ZIM totals we have
//! already include Xapian, and zim2wax drops those entries by design, so the
//! budget for a WAX index is exactly what ZIM spends on its own.
use zim::{Namespace, Target, Zim};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: zimindexsize <file.zim>")?;
    let z = Zim::new(&path)?;
    let file_size = std::fs::metadata(&path)?.len();

    let mut idx_bytes = 0u64;
    let mut idx_count = 0u64;
    let mut content_bytes = 0u64;
    let mut content_count = 0u64;
    let mut cur: Option<(u32, zim::Cluster<'_>)> = None;

    let mut order: Vec<(u32, u32, u32, bool)> = Vec::new();
    for i in 0..z.header.article_count {
        let e = match z.get_by_url_index(i) { Ok(e) => e, Err(_) => continue };
        if let Some(Target::Cluster(c, b)) = e.target {
            order.push((c, b, i, matches!(e.namespace, Namespace::FulltextIndex)));
        }
    }
    order.sort_unstable();
    for &(c, b, _i, is_idx) in &order {
        if cur.as_ref().map(|(x, _)| *x) != Some(c) {
            cur = Some((c, z.get_cluster(c)?));
        }
        let cl = &cur.as_ref().unwrap().1;
        let g = cl.read()?;
        let n = g.blob(b)?.len() as u64;
        if is_idx { idx_bytes += n; idx_count += 1; } else { content_bytes += n; content_count += 1; }
    }
    println!("## {path}");
    println!("  zim file                : {file_size}");
    println!("  fulltext-index entries  : {idx_count}  ({idx_bytes} bytes uncompressed)");
    println!("  content entries         : {content_count}  ({content_bytes} bytes uncompressed)");
    println!("  index as share of content: {:.1}%", 100.0 * idx_bytes as f64 / content_bytes.max(1) as f64);
    Ok(())
}
