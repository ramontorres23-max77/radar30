#!/bin/bash
# Build the native SwiftUI Radar30.app around the Rust CLI.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${2:-$ROOT/dist/Radar30.app}"
BIN="${1:-$ROOT/target/release/radar30}"

if [[ ! -x "$BIN" ]]; then
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi

VERSION="$(grep -m1 '^version = ' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
swift "$ROOT/macos/make_icon.swift" "$ICONSET"
iconutil -c icns "$ICONSET" -o "$ICONSET/../AppIcon.icns"

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp "$BIN" "$OUT/Contents/MacOS/radar30-cli"
cp "$ICONSET/../AppIcon.icns" "$OUT/Contents/Resources/AppIcon.icns"
swiftc -O -swift-version 5 -parse-as-library "$ROOT/macos/Radar30App.swift" -o "$OUT/Contents/MacOS/Radar30"
rm -rf "$ICONSET"

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Radar30</string>
  <key>CFBundleDisplayName</key><string>radar30</string>
  <key>CFBundleIdentifier</key><string>com.radar30.app</string>
  <key>CFBundleExecutable</key><string>Radar30</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$OUT"
echo "UI app: $OUT"
