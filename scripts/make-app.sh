#!/bin/sh
# Build target/Lumos.app, ad-hoc signed, so macOS can grant it Screen Recording, Camera and Microphone.
# An ad-hoc signature changes on every rebuild, so macOS forgets the grants; rebuild the bundle only when needed.
# Distribution needs a Developer ID signature and notarization instead of `-s -`.
set -eu
cd "$(dirname "$0")/.."
cargo build -p lumos-app ${RELEASE:+--release}
BIN=target/${RELEASE:+release}${RELEASE:-debug}/lumos-app
APP=target/Lumos.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/Lumos"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.lumos.app</string>
  <key>CFBundleName</key><string>Lumos</string>
  <key>CFBundleExecutable</key><string>Lumos</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSCameraUsageDescription</key><string>Lumos shows your camera in the face bubble and records it with your screen.</string>
  <key>NSMicrophoneUsageDescription</key><string>Lumos records your voice with the screen recording.</string>
</dict>
</plist>
PLIST
codesign --force --deep -s - "$APP"
echo "Built $APP"
