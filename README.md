# 繁化姬 GPUI

[繁化姬](https://zhconvert.org) desktop app built with [GPUI](https://gpui.rs): a native
Rust rewrite of [fanhuaji-tauri](https://github.com/fanhuaji-desktop/fanhuaji-tauri) with no
WebView. It converts Chinese text files (txt, srt, ass, lrc, vtt, csv, json, xml, html, md,
epub) between Traditional and Simplified variants through the zhconvert.org API.

The API client, chunking, encoding detection, EPUB handling and backups come from the
Tauri repo's `fanhuaji-core` crate, so both apps share one backend.

## Build

```bash
cargo run                       # debug build
cargo test                      # unit tests
cargo clippy --all-targets -- -D warnings
./scripts/bundle-macos.sh       # target/release/bundle/Fanhuaji.app (unsigned)
```

Full Xcode is not required: Metal shaders compile at launch (`runtime_shaders`).

`fanhuaji-core` is a git dependency on the Tauri repo. To develop against a local checkout,
put this in `.cargo/config.toml` (not committed):

```toml
[patch."https://github.com/fanhuaji-desktop/fanhuaji-tauri"]
fanhuaji-core = { path = "../fanhuaji-tauri/src-tauri/core" }
```

## Layout

- `src/main.rs` — the window: file list, preview, settings, menus, conversion queue wiring
- `src/queue.rs` — file queue and batch state (port of `queue.ts`)
- `src/diff.rs` — line + inline diff for the preview (port of `diff.ts`)
- `src/settings.rs` — persisted settings, a JSON file in the config dir
- `src/i18n.rs`, `locales/` — UI strings and error-code translation (port of `i18n.ts`)
- `src/updater.rs` — notify-only check against GitHub Releases
- `src/theme.rs` — light and dark palettes

The pure modules were ported test-first from the Tauri app's vitest suites.

## Differences from the Tauri app

- Updates: notify-only (link to the release); no self-install yet.
- Packaging: macOS `.app` only, unsigned; Windows and Linux builds are untested.
- The file dialog has no type filter (GPUI's prompt doesn't take one); unsupported files
  are skipped when added.
