#!/bin/sh
# Packs build/rustyfiles.app into build/rustyfiles-mac-arm64.dmg: the app and a link to
# /Applications to drag it onto. Run ./build.sh first. The name has no version in it, so
# README can link releases/latest/download/rustyfiles-mac-arm64.dmg.
set -eu
cd "$(dirname "$0")/.."
APP=build/rustyfiles.app
OUT=build/rustyfiles-mac-arm64.dmg
[ -d "$APP" ] || { echo "no $APP: run ./build.sh first" >&2; exit 1; }
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/rustyfiles.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
hdiutil create -volname rustyfiles -srcfolder "$STAGE" -fs APFS -format ULFO -quiet "$OUT"
echo "$OUT ($(du -h "$OUT" | cut -f1 | tr -d ' '), version $(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist"))"
