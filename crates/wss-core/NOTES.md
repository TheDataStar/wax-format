# Wax Service Shell engine (`wss-core`, `wss`) — notes

First build, 2026-11-20. The engine and command line of the Wax Service Shell
(WSS): the tool that turns sources into `.wax` packs and keeps them current.
The desktop app and SaaS are later front ends over this crate; there is no UI
here by design.

## In plain words

You point `wss` at something you have — a Wikipedia ZIM, a folder of web
pages or worksheets, an EPUB book, a PDF, or a website address — and it makes
a pack a DeltOS box can serve and search. Later, when the source changes, `wss
refresh` makes an *update* that holds only what changed, so boxes download a
small file instead of the whole pack again. Anything it could not carry is
listed in a report, never dropped quietly.

## What is built

| Area | State |
|---|---|
| ZIM | Delegated whole to `zim2wax`, at zstd level 19. A ZIM converted by `wss` and by `zim2wax` with the same options is byte-identical (tested), so the box's own USB conversion matches. |
| Folder (HTML site, documents, built web app) | Tree kept as is, so relative links work. Pages searchable and cut into passages. PDFs inside get a text page each. |
| EPUB | Book files kept at their paths; generated contents page as the start page; Contents/Previous/Next bar per chapter; chapters searchable. Metadata (title, creator, rights, language, PNG cover) used. DRM refused; obfuscated fonts carried and reported. |
| PDF | Original kept byte for byte; a generated text page with one heading per PDF page, so search lands on the right page and phones without a PDF viewer can read it. Scans (no text layer) carried and reported; no OCR. |
| Website | Same-site crawl from a start URL, under a path prefix, obeying robots.txt and `Crawl-delay`, with a named User-Agent (`wss/…`), page and depth limits, request spacing. Captured links rewritten to pack paths; other sites never contacted; redirects become aliases; query pages get their own files. |
| Editions | `refresh` re-converts with the pack's identity and carried dictionary, diffs by content hash, appends new/changed entries, tombstones for removed ones, and search rows/passages for changed pages, and the new manifest (with a newer version) when it changed. `rollback` undoes it byte for byte. `gc` compacts into a new base edition. |
| Management | `inspect`, `diff`, `update` (append a candidate built elsewhere), `sign`, `verify`. |
| Recipe | Every choice that shapes a pack, as TOML (`wss.toml`). `--save-recipe` writes it; `--recipe` rebuilds the same bytes (tested). |

Build settings follow ADR-0003 (PR #3): level 19, the 110 KiB dictionary
trained by zim2wax's sampling rule, entropy-coded media verbatim, TTF/OTF as
plain zstd. Search uses `wax-bigram-16` and passages use `html-headings-1`
(SPEC v0.13, PR #6).

## The API a front end uses

```rust
let recipe = wss_core::Recipe::from_toml(&text)?;      // or build one in code
let kind = wss_core::detect(source)?;                  // what is this?
let out = wss_core::convert(source, &pack, &recipe)?;  // ConvertOutcome
let upd = wss_core::refresh(&pack, source, &recipe)?;  // RefreshOutcome
wss_core::manage::{inspect, diff, update, rollback, gc, sign, verify}
```

Every result is `serde::Serialize`; `wss --json <command>` prints exactly what
a GUI would receive.

## Files written beside a pack

* `<pack>.build-report.json` — the Contract §11 report. Only closed-list
  codes ever appear in it (tested), including the four this PR adds.
* `<pack>.wss-report.json` — WSS's own plain-language account: every item not
  carried and why, and for websites the URL → pack path map. Never read by a
  box.
* `<pack>.minisig` — when signed.

## Decisions made here (reversible, stated so they can be disagreed with)

* **Source-stated license and attribution win; the recipe fills gaps only**,
  and an operator-supplied license is always reviewed (Contract §11, same as
  zim2wax). **Name and languages are the other way round**: a person naming
  their pack means it, so the recipe wins over a page `<title>`.
* **No default for category, resources, license, attribution or version.**
  Missing ones stop the build with a question (the project rule). A ZIM states
  its own name/version/languages/start page, so a recipe setting them for a
  ZIM is refused rather than silently ignored.
* **Generated pages live under `_assets/wss/`**, the builder-reserved prefix,
  so they can never collide with a source file. Contents pages are not
  indexed (they would match every chapter title twice); PDF text pages are.
* **Root-relative links** in generated and rewritten pages, as zim2wax does,
  because a pack is served at its own hostname's root (contract §8).
* **PDF and EPUB files are stored verbatim** (`none`): both are mostly
  compressed already and the gain is unmeasured. Measure before changing.
* **Crawl defaults** (1000 pages, depth 8, 1 s between requests, 30 s
  timeout, 256 MiB per response) are tool settings in the recipe, not
  contract values.
* **Generated pages use the locked light tokens** (`surface`, `ink`,
  `accent-ink`, focus ring per design-language §5). No dark tokens are
  defined, so dark mode uses the browser's system colours rather than
  invented values.

## Defects and gaps found (reported, not worked around)

1. **An appended update records its edition's version: closed.** SPEC v0.13
   §5.6.1 (PR #6) lets an appended segment replace the manifest. When the
   re-converted source's manifest differs, `wss refresh` / `wss update` write
   it into the update, and refuse unless its CalVer version is newer than the
   pack's (a changed manifest is a newer edition). The new icon and entry
   point must resolve in the merged pack. `rollback` restores the previous
   manifest with the previous segment.
2. **Warning codes for the new importers: proposed in this PR** as a
   contract §11 amendment: `uncaptured_page`, `external_reference`,
   `no_searchable_text`, `kept_unopened`. Every note kind now maps to a closed
   code, so the build report counts everything the import report names. The
   spec's `external_request`, `transcode_failed` and `unsupported_source_item`
   wait for the importers that detect them.
3. **Where a website's URL map lives: decided in the same amendment.** Beside
   the archive in the builder's own report, never in the pack or the build
   report; intake and boxes never read it.
4. **Two zim2wax bugs fixed in this PR** (pre-existing): the build report's
   `archive_filename` named `<pack>.partial`, and a signed conversion left its
   signature at `<pack>.partial.minisig`, beside a file that no longer exists.
5. **Found by the end-to-end check with real inputs, fixed (2026-11-21,
   `docs/e2e-check-2026-11-21.md`):** EPUB file names are read as UTF-8 even
   without the zip UTF-8 flag (OCF requires UTF-8; a Japanese book had lost
   its chapters and its search); `pdf-extract` 0.7 → 0.10 so CJK PDFs with
   predefined CMaps (`UniJIS-UCS2-H`) yield text; `wss refresh` defaults
   category, resources, arch, gpu, name, license and attribution from the
   pack's manifest. In `wax-core`, search text no longer breaks words at
   inline tags and drops ruby readings, so Japanese phrases match.

## Shared-crate changes in this PR (minimal)

* `wax-core`: `WaxWriter::zstd_level`, `StreamingWriter::set_zstd_level`,
  `check_zstd_level`; levels 1–19 only (20–22 break SPEC §3's 8 MiB window
  bound). Default stays 3, so every existing build is byte-identical. New
  test: level-19 frames stay within the bound, ultra levels are refused.
* `wax-builder`: `PackStream::set_zstd_level`.
* `zim2wax`: `ConvertOptions::zstd_level` (`None` = unchanged behaviour), and
  the two fixes above.

## Not yet

Publishing targets (box, USB shelf — layout undefined, WSS spec open question
2 — catalog), video/audio transcoding, Kolibri and IIAB migration, Markdown
and office documents, re-crawls that ask the server what changed, the
headless-browser check for web apps that call a server, exporting the update
as a separate delta file, and the desktop GUI. ZIM conversion and
`wax-builder` still default to level 3 when run directly (ADR-0003 follow-up);
`wss` always passes 19.
