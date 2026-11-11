#!/usr/bin/env bash
# Build target/release/bundle/Fanhuaji.app (with TARGET=<triple>: target/<triple>/release/...).
# shortcut: unsigned and not notarized, so Gatekeeper warns on first launch; codesign it once
# there is an Apple Developer ID.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release ${TARGET:+--target "$TARGET"}
version=$(cargo pkgid | sed 's/.*[#@]//')
out=target/${TARGET:+$TARGET/}release
app=$out/bundle/Fanhuaji.app

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$out/fanhuaji-gpui" "$app/Contents/MacOS/"
cp assets/icon.icns "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Fanhuaji</string>
  <key>CFBundleDisplayName</key><string>繁化姬</string>
  <key>CFBundleIdentifier</key><string>org.zhconvert.fanhuaji.gpui</string>
  <key>CFBundleExecutable</key><string>fanhuaji-gpui</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>CFBundleDevelopmentRegion</key><string>zh_TW</string>
  <key>CFBundleLocalizations</key>
  <array><string>zh_TW</string><string>zh_CN</string><string>en</string></array>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "$app"
