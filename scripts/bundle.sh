#!/usr/bin/env bash
# Build Qur.app (M7): release binary, Info.plist, Qur.icns, ad-hoc signature.
# usage: scripts/bundle.sh [--zip]      -> target/release/Qur.app (and Qur.zip)
# Developer ID signing and notarization: docs/signing.md (SIGN_ID=... here).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
VERSION=$(sed -nE 's/^version = "([^"]+)"/\1/p' Cargo.toml | head -1)
source "$HOME/.cargo/env" 2>/dev/null || true
cargo build --release -p qur_app

APP=target/release/Qur.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/qur "$APP/Contents/MacOS/qur"

TMP=$(mktemp -d)
xcrun swift scripts/make_icns.swift assets/brand "$TMP/Qur.iconset"
iconutil -c icns "$TMP/Qur.iconset" -o "$APP/Contents/Resources/Qur.icns"
rm -rf "$TMP"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Qur</string>
  <key>CFBundleDisplayName</key><string>Qur</string>
  <key>CFBundleIdentifier</key><string>ph.peur.qur</string>
  <key>CFBundleExecutable</key><string>qur</string>
  <key>CFBundleIconFile</key><string>Qur</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>© PEUR Technologies. GPL-3.0-or-later.</string>
</dict>
</plist>
PLIST

# Ad-hoc by default (runs on this Mac); SIGN_ID="Developer ID Application: …" for release.
codesign --force --options runtime --timestamp=none --sign "${SIGN_ID:--}" "$APP" >/dev/null
codesign --verify --strict "$APP"
echo "built $APP ($(du -sh "$APP" | cut -f1), version $VERSION, signed: ${SIGN_ID:-ad-hoc})"
if [ "${1:-}" = "--zip" ]; then
  ditto -c -k --keepParent "$APP" target/release/Qur.zip
  echo "zipped target/release/Qur.zip ($(du -sh target/release/Qur.zip | cut -f1))"
fi
