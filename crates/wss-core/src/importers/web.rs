//! Website capture: one start URL → pack (WSS spec importer B2, first cut).
//!
//! A polite, bounded, same-site crawl:
//!
//! * **Scope.** Pages are followed only on the start URL's own scheme, host
//!   and port, under its path prefix (by default the start URL's folder).
//!   Files a captured page needs to render (images, stylesheets, scripts,
//!   fonts) are fetched from anywhere on that same site. Other sites are never
//!   contacted; references to them are left as they are and listed.
//! * **Manners.** Every request names WSS in its `User-Agent`; `robots.txt`
//!   is read first and obeyed (its `Crawl-delay` can only slow the crawl
//!   down); requests are spaced by the recipe's delay.
//! * **Limits.** `max_pages` URLs and `max_depth` links from the start page.
//!   What a limit leaves out is reported.
//! * **Offline result.** After the crawl, every reference to a captured URL
//!   is rewritten to its pack path (root-relative, like zim2wax, since a pack
//!   is served at its own hostname's root, contract §8). The URL → pack path
//!   map goes in the import report.
//!
//! Not yet: re-crawls that ask the server what changed (B8), pages built by
//! scripts at view time (a headless browser), logins.

use super::{bare_mime, html_title, is_html_mime, Staged};
use crate::recipe::{CrawlSettings, Recipe};
use crate::refs::{self, RefKind};
use crate::report::NoteKind;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Read;
use std::time::{Duration, Instant};
use url::Url;

/// What every request says it is. Names the tool so a site owner can find
/// out what visited and block it in robots.txt with `User-agent: wss`.
pub const USER_AGENT: &str = concat!("wss/", env!("CARGO_PKG_VERSION"), " (DeltOS Wax Service Shell; offline archive builder)");
/// The robots.txt group WSS answers to, besides `*`.
pub const ROBOTS_TOKEN: &str = "wss";
/// Largest response read; anything bigger is reported, not held in memory.
pub const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;

/// Parsed robots.txt rules for WSS.
#[derive(Debug, Default, Clone)]
pub struct Robots {
    /// (allow, path prefix). Longest match wins; allow wins ties.
    rules: Vec<(bool, String)>,
    pub crawl_delay: Option<Duration>,
}

impl Robots {
    /// RFC 9309: use the most specific group naming WSS, else `*`.
    pub fn parse(text: &str) -> Robots {
        #[derive(Default)]
        struct Group {
            agents: Vec<String>,
            rules: Vec<(bool, String)>,
            delay: Option<Duration>,
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut cur: Option<Group> = None;
        let mut last_was_agent = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once(':') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "user-agent" => {
                    if !last_was_agent {
                        if let Some(g) = cur.take() {
                            groups.push(g);
                        }
                        cur = Some(Group::default());
                    }
                    cur.get_or_insert_with(Group::default).agents.push(v.to_ascii_lowercase());
                    last_was_agent = true;
                }
                "allow" | "disallow" => {
                    last_was_agent = false;
                    if let Some(g) = cur.as_mut() {
                        if !v.is_empty() {
                            g.rules.push((k == "allow", v.to_string()));
                        }
                    }
                }
                "crawl-delay" => {
                    last_was_agent = false;
                    if let (Some(g), Ok(s)) = (cur.as_mut(), v.parse::<f64>()) {
                        if s.is_finite() && s >= 0.0 {
                            g.delay = Some(Duration::from_secs_f64(s.min(3600.0)));
                        }
                    }
                }
                _ => last_was_agent = false,
            }
        }
        if let Some(g) = cur.take() {
            groups.push(g);
        }
        let pick = groups
            .iter()
            .find(|g| g.agents.iter().any(|a| a == ROBOTS_TOKEN))
            .or_else(|| groups.iter().find(|g| g.agents.iter().any(|a| a == "*")));
        match pick {
            Some(g) => Robots { rules: g.rules.clone(), crawl_delay: g.delay },
            None => Robots::default(),
        }
    }

    pub fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, prefix) in &self.rules {
            let p = prefix.trim_end_matches('$');
            let matched = if prefix.ends_with('$') { path == p } else { path.starts_with(p) };
            if matched {
                let len = p.len();
                best = match best {
                    Some((l, a)) if l > len || (l == len && a) => Some((l, a)),
                    _ => Some((len, *allow)),
                };
            }
        }
        best.is_none_or(|(_, a)| a)
    }
}

/// The pack path a captured URL is stored at: its path without the leading
/// `/`, a folder URL as its `index.html`, and a query folded into the name
/// (an 8-hex hash) so `?page=2` is its own file.
pub fn pack_path(u: &Url) -> String {
    let mut p = zim2wax::paths::percent_decode(u.path().trim_start_matches('/'));
    if p.is_empty() || p.ends_with('/') {
        p.push_str("index.html");
    }
    if let Some(q) = u.query() {
        let h = Sha256::digest(q.as_bytes());
        let tag = format!("__q{:02x}{:02x}{:02x}{:02x}", h[0], h[1], h[2], h[3]);
        let (dir, file) = p.rsplit_once('/').map(|(d, f)| (format!("{d}/"), f.to_string())).unwrap_or((String::new(), p.clone()));
        p = match file.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => format!("{dir}{stem}{tag}.{ext}"),
            _ => format!("{dir}{file}{tag}"),
        };
    }
    p
}

/// The URL with its fragment removed: what is fetched and remembered.
fn key(u: &Url) -> Url {
    let mut k = u.clone();
    k.set_fragment(None);
    k
}

fn same_site(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default()
}

/// HTTP access: through the environment's proxy, except for this machine.
struct Http {
    direct: ureq::Agent,
    proxied: ureq::Agent,
}

impl Http {
    fn new(timeout: Duration) -> Http {
        let base = || ureq::AgentBuilder::new().user_agent(USER_AGENT).timeout(timeout).redirects(5);
        Http { direct: base().build(), proxied: base().try_proxy_from_env(true).build() }
    }
    fn get(&self, u: &Url) -> std::result::Result<ureq::Response, String> {
        let local = matches!(u.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]") | Some("::1"));
        let agent = if local { &self.direct } else { &self.proxied };
        match agent.get(u.as_str()).call() {
            Ok(r) => Ok(r),
            Err(ureq::Error::Status(code, _)) => Err(format!("the server answered {code}")),
            Err(e) => Err(format!("could not be fetched: {e}")),
        }
    }
}

pub fn import(start: &str, recipe: &Recipe) -> Result<Staged> {
    let start = Url::parse(start).with_context(|| format!("{start:?} is not a URL"))?;
    if !matches!(start.scheme(), "http" | "https") {
        bail!("only http and https URLs can be captured, not {}", start.scheme());
    }
    let c: &CrawlSettings = &recipe.crawl;
    let prefix = c.path_prefix.clone().unwrap_or_else(|| match start.path().rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/"),
        None => "/".into(),
    });
    let http = Http::new(Duration::from_secs(c.timeout_secs.max(1)));
    let mut staged = Staged::new("web")?;

    let robots = {
        let mut ru = start.clone();
        ru.set_path("/robots.txt");
        ru.set_query(None);
        ru.set_fragment(None);
        match http.get(&ru) {
            Ok(r) => {
                let mut s = String::new();
                r.into_reader().take(512 * 1024).read_to_string(&mut s).ok();
                Robots::parse(&s)
            }
            Err(_) => Robots::default(), // no robots.txt: nothing is disallowed
        }
    };
    let delay = Duration::from_millis(c.delay_ms).max(robots.crawl_delay.unwrap_or_default());

    let mut queue: VecDeque<(Url, usize, RefKind)> = VecDeque::from([(key(&start), 0, RefKind::Link)]);
    let mut seen: HashSet<String> = HashSet::from([key(&start).to_string()]);
    let mut external: BTreeMap<String, u64> = BTreeMap::new();
    let mut fetched = 0usize;
    let mut last: Option<Instant> = None;
    let mut entry: Option<String> = None;
    // pack path → the URL it was fetched from, for the rewrite pass
    let mut origin_of: BTreeMap<String, Url> = BTreeMap::new();

    while let Some((url, depth, _)) = queue.pop_front() {
        if fetched >= c.max_pages {
            let left = queue.len() + 1;
            staged.notes.add(
                NoteKind::UncapturedPage,
                url.as_str(),
                format!("the page limit ({}) was reached; {left} queued URLs were not fetched", c.max_pages),
            );
            break;
        }
        if !robots.allows(url.path()) {
            staged.notes.add(NoteKind::UncapturedPage, url.as_str(), "robots.txt asks crawlers not to fetch it");
            continue;
        }
        if let Some(t) = last {
            let since = t.elapsed();
            if since < delay {
                std::thread::sleep(delay - since);
            }
        }
        last = Some(Instant::now());
        fetched += 1;
        let resp = match http.get(&url) {
            Ok(r) => r,
            Err(why) => {
                staged.notes.add(NoteKind::UncapturedPage, url.as_str(), why);
                continue;
            }
        };
        // Redirects were followed; store under where the content really is.
        let final_url = Url::parse(resp.get_url()).map(|u| key(&u)).unwrap_or_else(|_| url.clone());
        let mime = bare_mime(resp.header("content-type").unwrap_or(""));
        let mime = if mime.is_empty() { super::guess_mime(final_url.path()) } else { mime };
        let mut bytes = Vec::new();
        resp.into_reader().take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut bytes).ok();
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            staged.notes.add(NoteKind::UncapturedPage, url.as_str(), "larger than the 256 MiB a single capture may hold");
            continue;
        }
        if !same_site(&final_url, &start) {
            staged.notes.add(NoteKind::UncapturedPage, url.as_str(), format!("redirects off the site, to {final_url}"));
            continue;
        }
        let path = pack_path(&final_url);
        let from = url.to_string();
        if final_url != url {
            let alias = pack_path(&url);
            if alias != path && staged.check_path(&from, &alias).is_some() {
                staged.staging.add_redirect(&alias, &path, None)?;
            }
            seen.insert(final_url.to_string());
        }
        if staged.staging.contains(&path)? {
            staged.url_map.insert(from, path);
            continue;
        }
        let title = is_html_mime(&mime).then(|| html_title(&String::from_utf8_lossy(&bytes))).flatten();
        if !staged.stage_file(&from, &path, &mime, title.as_deref(), &bytes)? {
            continue;
        }
        staged.url_map.insert(from.clone(), path.clone());
        if final_url.as_str() != from {
            staged.url_map.insert(final_url.to_string(), path.clone());
        }
        origin_of.insert(path.clone(), final_url.clone());
        if entry.is_none() {
            entry = Some(path.clone());
            staged.facts.name = title.clone();
            if is_html_mime(&mime) {
                staged.facts.languages =
                    super::html_lang(&String::from_utf8_lossy(&bytes)).filter(|l| super::valid_language(l));
            }
        }

        // Follow what this capture references.
        let found: Vec<(RefKind, String)> = if is_html_mime(&mime) {
            refs::collect_html(&String::from_utf8_lossy(&bytes))
        } else if mime == "text/css" {
            refs::collect_css(&String::from_utf8_lossy(&bytes)).into_iter().map(|u| (RefKind::Resource, u)).collect()
        } else {
            vec![]
        };
        for (kind, raw) in found {
            let raw = raw.trim();
            if raw.is_empty() || raw.starts_with('#') || raw.starts_with("data:") || raw.starts_with("javascript:") || raw.starts_with("mailto:") {
                continue;
            }
            let Ok(target) = final_url.join(raw) else { continue };
            if !matches!(target.scheme(), "http" | "https") {
                continue;
            }
            if !same_site(&target, &start) {
                *external.entry(target.origin().ascii_serialization()).or_default() += 1;
                continue;
            }
            let target = key(&target);
            let in_scope = match kind {
                RefKind::Link => target.path().starts_with(&prefix) && depth < c.max_depth,
                RefKind::Resource => true,
            };
            if !in_scope || c.exclude.iter().any(|x| target.path().contains(x.as_str())) {
                continue;
            }
            if seen.insert(target.to_string()) {
                let next_depth = if kind == RefKind::Link { depth + 1 } else { depth };
                queue.push_back((target, next_depth, kind));
            }
        }
    }

    if entry.is_none() {
        bail!("nothing was captured from {start}; see the notes for why");
    }
    for (origin, n) in &external {
        staged.notes.add(
            NoteKind::ExternalReference,
            origin.as_str(),
            format!("{n} references to another site were left as they are; WSS does not contact other sites"),
        );
    }

    // Point every captured reference at its pack path.
    let captured: BTreeMap<String, String> =
        staged.url_map.iter().filter(|(_, p)| !p.is_empty()).map(|(u, p)| (u.clone(), p.clone())).collect();
    for (path, base) in &origin_of {
        let Some(item) = staged.staging.get(path)? else { continue };
        let crate::stage::Body::Data { bytes, .. } = item.body else { continue };
        let mime = item.mime.unwrap_or_default();
        let mut f = |_: RefKind, raw: &str| -> Option<String> {
            let t = base.join(raw.trim()).ok()?;
            let frag = t.fragment().map(|f| format!("#{f}")).unwrap_or_default();
            let p = captured.get(key(&t).as_str())?;
            Some(format!("{}{frag}", zim2wax::paths::href_for(p)))
        };
        let new = if is_html_mime(&mime) {
            refs::rewrite_html(&String::from_utf8_lossy(&bytes), &mut f)
        } else if mime == "text/css" {
            refs::rewrite_css(&String::from_utf8_lossy(&bytes), &mut f)
        } else {
            continue;
        };
        if new.as_bytes() != bytes.as_slice() {
            staged.staging.replace_bytes(path, new.as_bytes())?;
        }
    }
    staged.url_map.retain(|_, p| !p.is_empty());
    staged.facts.entry_point = entry;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_groups_and_longest_match() {
        let r = Robots::parse("User-agent: *\nDisallow: /\n\nUser-agent: wss\nUser-agent: other\nDisallow: /private/\nAllow: /private/ok\nCrawl-delay: 2\n");
        assert!(r.allows("/docs/a"));
        assert!(!r.allows("/private/x"));
        assert!(r.allows("/private/ok.html"));
        assert_eq!(r.crawl_delay, Some(Duration::from_secs(2)));
        let star = Robots::parse("User-agent: *\nDisallow: /tmp\n");
        assert!(!star.allows("/tmp/x") && star.allows("/a"));
        assert!(Robots::parse("").allows("/anything"));
    }

    #[test]
    fn urls_map_to_pack_paths() {
        let p = |s: &str| pack_path(&Url::parse(s).unwrap());
        assert_eq!(p("http://h/"), "index.html");
        assert_eq!(p("http://h/docs/"), "docs/index.html");
        assert_eq!(p("http://h/docs/a%20b.html"), "docs/a b.html");
        assert_eq!(p("http://h/about"), "about");
        let q = p("http://h/list.html?page=2");
        assert!(q.starts_with("list__q") && q.ends_with(".html"), "{q}");
        assert_ne!(q, p("http://h/list.html?page=3"));
    }
}
