#!/bin/sh
# Build Lumos.app (default: ~/Applications/Lumos.app), ad-hoc signed, so macOS can grant it Screen Recording, Camera and Microphone.
# An ad-hoc signature changes on every rebuild, so macOS forgets the grants; rebuild the bundle only when needed.
# Distribution needs a Developer ID signature and notarization instead of `-s -`.
#   RELEASE=1  build with --release
#   DEST=dir   put Lumos.app in dir instead of ~/Applications
set -eu
cd "$(dirname "$0")/.."
cargo build -p lumos-app ${RELEASE:+--release}
PROFILE=${RELEASE:+release}
PROFILE=${PROFILE:-debug}
BIN=target/$PROFILE/lumos-app
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# Outside ~/Documents on purpose: an app that runs from, or reads files in, Documents makes macOS ask for Documents access.
APP="${DEST:-$HOME/Applications}/Lumos.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Lumos"
# The capture helper lives inside the app, so the bundle never reads from the source tree.
HELPER=$(ls -t target/$PROFILE/build/lumos-capture-*/out/lumos-capture-helper 2>/dev/null | head -1)
[ -n "$HELPER" ] || { echo "capture helper was not built" >&2; exit 1; }
cp "$HELPER" "$APP/Contents/MacOS/lumos-capture-helper"
# App icon: an .icns made from the 512px PNG.
ICONSET=$(mktemp -d)/Lumos.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s crates/lumos-app/icons/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  [ $d -le 512 ] && sips -z $d $d crates/lumos-app/icons/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Lumos.icns"
rm -rf "$(dirname "$ICONSET")"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.lumos.app</string>
  <key>CFBundleName</key><string>Lumos</string>
  <key>CFBundleDisplayName</key><string>Lumos</string>
  <key>CFBundleExecutable</key><string>Lumos</string>
  <key>CFBundleIconFile</key><string>Lumos</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSCameraUsageDescription</key><string>Lumos shows your camera in the face bubble and records it with your screen.</string>
  <key>NSMicrophoneUsageDescription</key><string>Lumos records your voice with the screen recording.</string>
</dict>
</plist>
PLIST
# Ad-hoc signing normally ties permissions to the exact build, so every rebuild loses them. This
# requirement matches on the bundle identifier instead, so Screen Recording and Camera grants survive rebuilds.
codesign --force -s - -i dev.lumos.app.helper "$APP/Contents/MacOS/lumos-capture-helper"
codesign --force -s - -r='designated => identifier "dev.lumos.app"' "$APP"
echo "Built $APP"
