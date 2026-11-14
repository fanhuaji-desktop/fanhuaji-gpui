# 繁化姬 GPUI

[繁化姬](https://zhconvert.org) desktop app built with [GPUI](https://gpui.rs): a native
Rust rewrite of [fanhuaji-tauri](https://github.com/fanhuaji-desktop/fanhuaji-tauri) with no
WebView. It converts Chinese text files (txt, srt, ass, lrc, vtt, csv, json, xml, html, md,
epub) between Traditional and Simplified variants through the zhconvert.org API.

The API client, chunking, encoding detection, EPUB handling and backups come from the
Tauri repo's `fanhuaji-core` crate, so both apps share one backend.

> This is a third-party client, not an official Fanhuaji product. It is an early port; the
> [Tauri edition](https://github.com/fanhuaji-desktop/fanhuaji-tauri) is the stable one.

## Download

Get the latest version from [Releases](https://github.com/fanhuaji-desktop/fanhuaji-gpui/releases):

| Platform | Architecture | File |
|----------|--------------|------|
| macOS | Apple Silicon | `Fanhuaji-macos-aarch64.zip` |
| macOS | Intel | `Fanhuaji-macos-x86_64.zip` |
| Windows | x86_64 | `Fanhuaji-windows-x86_64.zip` |
| Linux | x86_64 | `Fanhuaji-linux-x86_64.zip` |
| Linux | ARM64 | `Fanhuaji-linux-aarch64.zip` |

There is no installer: unzip it and run `Fanhuaji.app` (macOS), `fanhuaji-gpui.exe`
(Windows) or `fanhuaji-gpui` (Linux). Keep it somewhere you can write to, such as
`/Applications` or a folder in your home directory: the app updates itself by replacing
that file when you accept a new version.

### Opening it for the first time

The app is not signed, so the system warns before the first launch.

- **macOS** says it cannot be opened. Click **Done**, go to **System Settings → Privacy &
  Security**, and click **Open Anyway** next to "Fanhuaji was blocked". Or run
  `xattr -cr /Applications/Fanhuaji.app` in Terminal.
- **Windows** SmartScreen says it protected your PC. Click **More info → Run anyway**.

## Build

```bash
cargo run                       # debug build
cargo test                      # unit tests
cargo clippy --all-targets -- -D warnings
./scripts/bundle-macos.sh       # target/release/bundle/Fanhuaji.app (unsigned)
```

Full Xcode is not required: Metal shaders compile at launch (`runtime_shaders`).
Linux needs `libxkbcommon-x11-dev libwayland-dev libvulkan-dev libfontconfig-dev
libx11-xcb-dev libzstd-dev` (see `.github/workflows/ci.yml`).

`fanhuaji-core` is a git dependency on the Tauri repo. To develop against a local checkout,
put this in `.cargo/config.toml` (not committed):

```toml
[patch."https://github.com/fanhuaji-desktop/fanhuaji-tauri"]
fanhuaji-core = { path = "../fanhuaji-tauri/src-tauri/core" }
```

## Release

Bump `version` in `Cargo.toml`, then push the tag `v<version>`. `release.yml` builds
macOS (arm64, x86_64), Windows (x86_64) and Linux (x86_64, arm64) zips, signs each with
minisign and publishes the GitHub Release. It needs the `MINISIGN_SECRET_KEY` secret; its
public half is `PUBLIC_KEY` in `src/updater.rs`, and changing the key strands every
installed copy on its current version.

## Layout

- `src/main.rs` — app state and behavior: adding files, converting, Undo, updates, menus
- `src/view.rs` — rendering: top bar, file list, preview, settings drawer, popovers
- `src/files.rs` — expands dropped or picked folders into the files to queue
- `src/queue.rs` — file queue and batch state (port of `queue.ts`)
- `src/diff.rs` — line + inline diff for the preview (port of `diff.ts`)
- `src/settings.rs` — persisted settings, a JSON file in the config dir
- `src/i18n.rs`, `locales/` — UI strings and error-code translation (port of `i18n.ts`)
- `src/updater.rs` — GitHub Releases check, signed download and in-place install
- `src/theme.rs` — light and dark palettes
- `build.rs` — embeds the icon in the Windows executable

The pure modules were ported test-first from the Tauri app's vitest suites.

## Differences from the Tauri app

- Updates: the release zip replaces the installed `.app` or executable in place (needs write
  access to its folder; otherwise the error shows and the release page is a click away).
  There is no installer: Windows and Linux ship a bare executable in a zip.
- Packaging: unsigned, so macOS Gatekeeper and Windows SmartScreen warn on first launch.
- The file dialog has no type filter (GPUI's prompt doesn't take one); unsupported files
  are skipped when added.
- Dropped or picked folders are expanded to their supported files, subfolders included
  (hidden entries skipped, at most 1000 files per drop). The Tauri app queues the folder
  itself, which then fails to convert.

## License

The source code is released under the [MIT](LICENSE) license.

This app uses the [Fanhuaji](https://docs.zhconvert.org/) API; use of the API is subject to
Fanhuaji's [terms of service](https://docs.zhconvert.org/license/).
