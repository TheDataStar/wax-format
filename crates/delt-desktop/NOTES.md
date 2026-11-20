# Delt desktop: decisions, findings, open items

The Delt desktop app is the Kiwix-Desktop equivalent: read `.wax` packs on
your own computer. Two crates:

| Crate | What | In the workspace? |
|---|---|---|
| `crates/delt-desktop` | The core: library, reading, search, bookmarks, private notes, history, settings, themes, edition updates. All tested here. | Yes |
| `crates/delt-desktop-app` | The window (Tauri 2) and its page (`ui/`). A thin layer of commands over the core. | **No.** It needs the platform web view to build; the format, the box and CI do not. |

## Why Tauri

* **Pack pages are web pages**, so the app needs a real web engine. Tauri
  uses the one the computer already has (WebView2 on Windows, WKWebView on
  macOS, WebKitGTK on Linux), so the app stays small: no bundled Chromium as
  with Electron. A native Rust toolkit (egui, iced, Slint) cannot render a
  pack's HTML at all.
* **It is Rust**, so `deltlib` runs inside the app as a library: no second
  implementation, no server process, no network port.
* **It can answer a private URL scheme** (`deltpack:`) in-process, which is
  how pack pages reach the pack zone without anything listening on the
  network.
* Tauri also targets phones; that is not used (box-only policy: no mobile apps).

## Reuse, not a fork

* Library: `deltlib::Library` over the library folder, `deltlib::Catalog`
  for pack addresses.
* The window's library, search and completion calls go through
  `deltlib::api::Api::handle` (deltd API v1), the code `deltd` runs, as the
  desktop's one signed-in person (`LocalIdentity`).
* Pack pages go through `deltlib::packzone::serve`, unchanged. The desktop
  changes one header on the way out: `frame-ancestors` names the desktop
  window instead of the WebOS host.
* **No change to `deltlib` or `wax-core`.** The workspace gained one member;
  the app crate is excluded.

## Decisions taken (recommended option, per Commander's delegation)

1. **One host per pack**, `pack-<index>-g<generation>.localhost` on the
   `deltpack:` scheme (Windows: `http://deltpack.pack-….localhost`, Tauri's
   mapping). Converted pages link root-relative (`/A/Foo`), so each pack
   needs its own root, exactly as on the box.
2. **Pack pages run in `<iframe sandbox="allow-scripts allow-forms
   allow-same-origin">`.** `allow-same-origin` is needed: without it the
   page gets an opaque origin, and the pack zone's
   `Cross-Origin-Resource-Policy: same-origin` (rightly) blocks its own
   stylesheets and images (seen in WebKitGTK). It does not let a page reach
   the window: the page's origin is its own pack host, never the window's,
   and Tauri gives commands to the window's origin only.
3. **Adding a pack copies it** into the library folder; the library folder
   can instead be pointed at any folder (a drive of packs), like Kiwix's
   "open folder".
4. **Edition updates**:
   * an **appended edition** (its first new segment's `prev_state_digest`
     equals this copy's digest) is applied in place by SPEC §7.4's
     procedure, using the new edition file as the delta. Only the new part
     is written. The result is checked against the new edition's digest, and
     undone (SPEC §7.5) if it differs. **Undo** is offered whenever the
     newest segment carries `prev_header`.
   * a **rebuilt edition** replaces the file only when its manifest
     `version` (CalVer, contract §11) is newer. Without versions on both,
     the update is refused: which is newer is unknown, not guessed.
5. **Removing a pack deletes its file** (after a confirmation in the
   window). Bookmarks and notes about it are kept.
6. **Private data** lives in `user.sqlite` in the OS's per-user app-data
   folder; schema-versioned, a newer version is refused. Notes export to
   Markdown on request; nothing is ever sent anywhere.
7. **Snippets.** deltd's `/search` returns `snippet: null`; the desktop fills
   it from the best matching passage (SPEC §5.10) for the hits it shows.
   Worth moving into deltlib so the WebOS gets the same; left alone here to
   keep shared crates unchanged.

## Findings for other threads

* **Format (v0.13 thread): an appended update cannot change the manifest.**
  SPEC §5.6 reads the manifest from the base segment only, so an edition
  shipped as an append still says the old `version`, `description` and so
  on. The desktop (and the box) cannot show which edition is installed; the
  window describes such an update as "2026.09.1 with 1 more update" instead.
  A per-segment manifest overlay (newest wins, like entries) would fix it.
* **Signatures are carried, not checked.** Which publisher keys a desktop
  trusts is undecided (the box's equivalent is architecture Q2, contract
  §13.2). The window says so in plain words when a pack has a sidecar.
* **No delta file format exists.** SPEC §7.4 defines what a delta *is*
  (the new edition's bytes past the old logical end, plus its header) but
  not a file to ship one in, and Track A's A6 delta engine is unbuilt. The
  desktop therefore applies updates from a whole newer edition file. When a
  delta file is specified, `updates::append_tail` is the place to accept it.
* **Media is handed to the web view whole per request** (up to 512 MiB):
  Tauri's scheme handler takes a byte buffer, not a stream. Web views ask for
  video in ranges, so in practice each answer is a range; a whole-file
  request for a huge entry would be held in memory.

## Proposed for the design language's owner

* **Dark palette** (`theme.rs` `DARK`): Commander chose light and dark
  modes (2026-11-15), which overrides design-language §4's "one light
  theme"; §4 has no dark values, so these are a proposal. Every pairing is
  measured in tests against `bg`, `surface` and `raised` to the light
  palette's own bar (ink ≥ 7, text tokens ≥ 4.5, `accent` ≥ 3 as UI only).
* **Colour-blind mode**: `success` moves to blue (`#1F5F99` light,
  `#7FB8F0` dark), so success and error differ for red/green colour
  blindness. State always carries a word too (§4).
* **Faces**: §5.1 says three bundled faces but does not name them, so the
  window uses the computer's own faces until it does.
* **The app icon** is a placeholder (the three-segment rule on the light
  ground): the icon set is still open (contract §13.2).

## Not built yet

* Opening `.zim` files directly (the box converts ZIMs; on the desktop that
  is the Wax Service Shell's job).
* Export of articles to HTML/EPUB/PDF (product decision 2026-11-15) — not
  part of this task's list; it belongs in deltlib so the box gets it too.
* Text-to-speech, reader mode and the magnifier from design-language §12.
* Packaging and signing installers (`cargo tauri build`) and auto-update of
  the app itself.
