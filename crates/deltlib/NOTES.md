# deltlib / deltd — build notes (2026-11-20)

First build of `deltd` against `docs/deltd-api-v1.md` §11 steps 1 and 2.

## What is built

| API doc | State |
|---|---|
| §2 hostname split, rule 16 (unknown hosts refused) | Built. One listener, routed by `Host`. |
| §4.1 every row | Built: 200/206, 301 alias, 410 tombstone, 404, 302 `/`, 500 on hash mismatch or unknown codec, 404 for unissued hostnames before lookup, 405. |
| §4.2 headers | Built: ETag = entry sha256, `-zst` suffix for the zstd bytes, `Vary`, ranges on plain responses, the pack CSP with `frame-ancestors` = WebOS origin, CORP, nosniff, Referrer-Policy, no cookies ever. |
| §4.3 compression | Built: zstd passthrough only for plain-zstd entries to clients that send `zstd`, no range; dictionary entries always decoded on the box. |
| §4.4 issued, live hostnames | Built: current generation, partition live via `identityd`, Guest only on guest-accessible packs. |
| §6.2 Library | `GET /packs`, `/packs/{uuid}` (with this person's pack hostname), `/packs/{uuid}/pages`. Not yet: `/random` (waits on format ask F6), `/catalog`, `/updates`, the `outdated` filter. |
| §6.3 Search | `GET /search`, `/search/packs/{uuid}`, `/suggest`. Snippets are off (`snippet: null`), as §11 asks until their speed is measured. |
| `/status`, `/openapi.json` | Built. |
| §6.1 sessions, §6.4–§6.8, §8 progress | Not built. `identityd` is the `Identity` trait; `StaticIdentity` is a dev-only fixed table. |
| HTTPS | Not built. `deltd` listens on plain HTTP; Secure endpoints arrive with TLS. |
| §9 rate limits | Not built: the numbers are to be measured (API §10 #9). |

Packs are found in `/var/lib/deltos/packs/` (contract §10) and rescanned every few seconds. A pack whose header changed is reopened, so an appended edition (and its search index) is served without a restart. `pack_index`/`pack_generation` live in a small table under `/var/lib/deltos/b4/` until B4 exists.

## Measured (x86_64 dev container, not the target)

On `wikipedia_en_climate_change_mini_2024-06` (8.98 MB ZIM, 3,821 articles):

| | |
|---|---|
| Search index added to the pack | 8.21 MB → 9.55 MB (+16%; pack is 1.06× the ZIM). wax-core still writes zstd level 3 (ADR-0003 follow-up). |
| Conversion time with index | 3.6 s vs 3.1 s without |
| Rebuild determinism | Byte-identical |
| Page, plain HTTP | p50 0.53 ms |
| Search, two words | p50 1.7 ms |
| Title completion | p50 1.0 ms |
| 40 concurrent clients | ~2,100 pages/s, p95 26 ms |
| deltd resident memory | ~12 MB with one pack open |

None of this is the 2 GB ARM64 floor; the API doc requires those numbers before release.

### gzip for browsers (2026-11-22)

Every plain-HTTP visitor's browser takes gzip but not zstd, so until now
they got every page uncompressed. `deltd` now gzips a page when the browser
asks for gzip and not zstd, the pack stored the entry compressed (media
stays verbatim), the type is text-like, the whole entry is asked for (no
range), and it is at least `GZIP_MIN_BYTES` (1,024: below one packet
compressing gains nothing). Tag `"<sha256>-gz"`, `Vary: Accept-Encoding`.
zstd passthrough still wins where offered.

Measured on the climate mini pack, all 3,821 article pages in turn, 10
clients, `deltd` pinned to one x86_64 core, 15 s per row:

| Encoding to the browser | Mean bytes per page | CPU per page | Pages/s on one core |
|---|---|---|---|
| None (before) | 7,546 | 0.085 ms | 11,750 |
| gzip level 1, miniz_oxide | 2,792 | 0.179 ms | 5,525 |
| gzip level 6, miniz_oxide | 2,476 | 0.226 ms | 4,409 |
| gzip level 1, zlib-rs | 3,278 | 0.162 ms | 6,132 |
| **gzip level 6, zlib-rs (chosen)** | **2,448** | **0.211 ms** | **4,713** |

Chosen: level 6 on the pure-Rust `zlib-rs` backend. It sends **3.1× fewer
bytes** for 2.5× the CPU. The box's limit is the radio, not the processor
(`docs/base-addons-admin.md` §4): 4,700 pages a second on one core is far
beyond what any Wi-Fi radio carries, while every byte saved is airtime. No
cache of gzipped bytes: not needed at these figures; revisit if the ARM64
measurement shows CPU becoming the limit.

## Findings for the format-gaps work

1. *(Closed by SPEC v0.13 §5.7.2: `wax-bigram-16`; zim2wax now indexes CJK packs and deltd queries them through `wax_core::tokenize`.)* **CJK search cannot work as SPEC §5.7 stands.** With `detail=none`, SQLite's `trigram` tokenizer refuses any term longer than three characters ("phrase queries are not supported"), and a two-character word, which is most Chinese words, can never match. Because every later edition must keep the base's tokenizer, `zim2wax` withholds the index for `zh`/`ja`/`ko` packs rather than lock them into it. `deltd` still queries any `trigram` pack by splitting long words into three-character windows. Needs a format decision (for example `detail=full` for CJK only, or a bigram tokenizer).
2. **Ranking is weak under `detail=none`.** The index stores neither columns nor term counts, so bm25 favours short pages that merely mention a word. `deltd` puts exact and leading title matches first (up to 5 per pack), which fixed the obvious cases on real Wikipedia ("Kyoto Protocol" now finds *Kyoto Protocol* first). A better fix belongs to the format.
3. *(Closed for new packs by `wax-bigram-16`, which folds with `nfkc-cf-16` first: `ß`/`ss` and ligatures fold; `œ`/`oe` still does not, as case folding does not map it.)* **`unicode61` does not fold `ß`/`ss`, `œ`/`oe` or ligatures** (it removes accents only). Title completion folds them (`nfkc-cf-16`), full-text search does not.
4. **zstd passthrough almost never applies to real packs.** `zim2wax` stores all text against the shared dictionary, which a browser cannot use, so the box decodes every page (as ADR-0003 already notes for plain HTTP).
5. **Media byte ranges cannot be checked.** `sha256` covers a whole entry, so a range of a video is served unverified; a whole-entry read is verified as it streams and cut off on mismatch. Per-chunk hashes would be a format addition.

## Not decided here (still open in the contract)

The WebOS hostname (`--webos-host` is required and has no default), session lengths, rate-limit numbers, translated error pages for the pack zone.

## Step 3: personal data and reading (2026-11-27)

Built against deltd API §6.4 and §6.5 and §11 step 3.

| API doc | State |
|---|---|
| §6.5 `/me/bookmarks`, `/me/highlights`, `/me/lists` | Built: `GET` (newest first, cursor pages, `pack`/`path` filters for bookmarks and highlights), `POST`, `PATCH /{id}`, `DELETE /{id}`. Pages are checked when added (the pack is one the person may see, the path is a page, aliases followed to the page itself); titles come from the page. |
| §6.5 `/me/history` | Built: `GET` and `DELETE`. Filled by `deltd` itself when it serves a page (HTML only, never media or style sheets) on a person's pack hostname whose partition is live, as `entry_viewed` will be (§8.2). Guest records nothing. 500 pages per person, the desktop app's number. |
| §6.5 `/me/needs-attention` | Built. A highlight stores the pack's header hash when its anchor was checked; when the pack has changed, the next read of the person's highlights re-finds each quote on the page (visible text, whitespace-collapsed, the old section tried first). Found: the anchor follows it to its new section. Not found, or the page deleted: **flagged and left where it was**, never moved. A removed pack's highlights are left alone, so they come back if it is installed again. |
| §6.5 `/me/export` | Built: one JSON file (`deltos-personal-export`, version 1) with everything above and the names of the packs it mentions, as an attachment. |
| §6.4 `outline`, `reader` | Built from the page's passages (SPEC v0.13 §5.10): a section is a passage, its id the path plus ordinal, so it is the same id a highlight anchors to. `reader` gives each section's paragraphs (block boundaries become breaks; scripts, styles and comments are not text) and its images resolved to paths inside the pack; images on other sites and `data:` URLs are left out (contract §16). A page without passages (packs before v0.13) is one section. |
| §6.4 `speech` | Not built: needs a text-to-speech engine measured on the minimum spec. |
| §6.4 `progress-events`, §8 | Not built: where `identityd` keeps progress events (contract §5.2, §10 names `identity/`) and its calls for them come first. |

**Where it lives.** `/var/lib/deltos/deltd/personal.sqlite` (`--state-dir`), answered 2026-11-27 in contract §10 on the API doc's own recommendation (§10 #2). `deltos-deltd.service` makes the directory (`StateDirectory=`). Schema 1 in `PRAGMA user_version`; a newer one is refused at start and on restore (`deltos-backup`).

**Privacy.** No endpoint takes a person argument; every row is keyed by the session's `profile_id`. Ids are 64 random bits, so an id says nothing about how many rows anyone has, and another person's id answers `item_not_found` exactly like one that never existed. Every `/me` response is `no-store`.

**Bounds** (implementation limits, not contract values, `personal.rs`): titles 300 characters, quotes 2,000, notes 10,000, 100 pages per list, all inside the API's 16 KiB request body; reader mode and highlight checks decode at most 8 MiB of one page.

**Not done, and why.**
* The Delt desktop app still keeps its own store (`delt-desktop/src/store.rs`, one person, notes without passages). Moving it onto `deltlib::personal` changes what is on people's disks, so it is its own step.
* A highlight check decodes the page twice (once for the text, once inside `passages()`); pages are small and this runs only after an update. A `wax-core` call that returns both would remove it.
* History is one SQLite write per page read. Not yet measured on an SD card; if it shows, writes can be batched.
