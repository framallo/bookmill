#!/usr/bin/env bash
# Wrap a built bookmill.app into a plain drag-to-install .dmg using hdiutil.
#
# Tauri's default DMG step (create-dmg) drives Finder via AppleScript to style
# the DMG window; that step fails in headless/CI sessions without GUI-automation
# access. This produces a functional (if unstyled) DMG without AppleScript.
#
# Usage: packaging/make-dmg.sh [path/to/bookmill.app] [out.dmg]
set -euo pipefail

APP="${1:-desktop/src-tauri/target/release/bundle/macos/bookmill.app}"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
ARCH="$(uname -m | sed 's/x86_64/x64/;s/arm64/aarch64/')"
OUT="${2:-desktop/src-tauri/target/release/bundle/dmg/bookmill_${VERSION}_${ARCH}.dmg}"

[ -d "$APP" ] || { echo "app not found: $APP (run: cd desktop/src-tauri && cargo tauri build)"; exit 1; }

STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
mkdir -p "$(dirname "$OUT")"
hdiutil create -volname "bookmill" -srcfolder "$STAGE" -ov -format UDZO "$OUT"
rm -rf "$STAGE"

echo "DMG:    $OUT"
echo "sha256: $(shasum -a 256 "$OUT" | awk '{print $1}')"
