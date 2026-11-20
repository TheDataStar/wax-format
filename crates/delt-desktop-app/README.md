# Delt for the desktop (the window)

The Delt reader for a computer of one's own: add `.wax` packs, read them,
search across all of them, keep bookmarks and private notes, choose light or
dark, and bring packs up to a newer edition. Everything it does lives in
[`crates/delt-desktop`](../delt-desktop) (tested there); this crate is the
Tauri window and its page in `ui/`. Decisions and open items:
[`crates/delt-desktop/NOTES.md`](../delt-desktop/NOTES.md).

It is **not** a workspace member, so `cargo test --workspace` never needs a
web view. Build it from this folder.

## Build and run

Needs Rust stable, a C toolchain, and the platform web view:

| Platform | Web view | Extra packages |
|---|---|---|
| Windows 10/11 | WebView2 (preinstalled on 11) | none |
| macOS 11+ | WKWebView | Xcode command-line tools |
| Linux | WebKitGTK 4.1 | Debian/Ubuntu: `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev` |

```bash
cd crates/delt-desktop-app
cargo run                         # debug window
cargo build --release             # ./target/release/delt
```

Installers (`.msi`, `.dmg`, `.deb`/`.AppImage`) come from the Tauri CLI,
`cargo install tauri-cli --version '^2'` then `cargo tauri build`; not set up
in CI yet.

Try it with demo packs:

```bash
cargo run -p delt-desktop --example demo_packs -- /tmp/delt-demo   # from the repo root
```

then **Add a pack…** for `science.wax` and `history.wax`, and **Find updates
in a folder…** on `/tmp/delt-demo/updates` to apply (and then undo) an
edition update.

## Where things are kept

The operating system's per-user app-data folder for `lan.deltos.delt`
(Linux `~/.local/share/lan.deltos.delt`, macOS `~/Library/Application
Support/lan.deltos.delt`, Windows `%APPDATA%\lan.deltos.delt`):
`user.sqlite` (bookmarks, notes, history, settings), `catalog.sqlite` (pack
addresses) and `packs/` (the library folder, unless moved in the window).

## How the window is put together

* `src/main.rs`: the `deltpack:` scheme (pack pages, via deltlib's pack
  zone), the commands the page calls, and the file pickers.
* `ui/index.html`, `ui/app.css`, `ui/app.js`: no framework and no build
  step. Colours come only from the tokens the core generates
  (`delt_desktop::theme::css`).
* Pack pages load in a sandboxed frame from their own `deltpack:` host and
  cannot call any command (`capabilities/default.json`).

Checked on Linux (WebKitGTK, under Xvfb): the library, opening and reading a
pack with its own stylesheet and links, bookmarks, notes, completion,
search with snippets, and the dark and colour-blind themes. Not yet run on
Windows or macOS.
