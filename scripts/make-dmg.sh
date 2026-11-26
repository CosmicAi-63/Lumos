#!/bin/sh
# Build a release Lumos.app and wrap it in dist/Lumos-macos-arm64.dmg for download.
# The DMG holds the app and an Applications shortcut to drag it onto.
set -eu
cd "$(dirname "$0")/.."
ARCH=$(uname -m)
DMG="dist/Lumos-macos-$ARCH.dmg"
STAGE=$(mktemp -d)
RELEASE=1 DEST="$STAGE" scripts/make-app.sh
ln -s /Applications "$STAGE/Applications"
mkdir -p dist
rm -f "$DMG"
hdiutil create -volname Lumos -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
rm -rf "$STAGE"
echo "Built $DMG"
