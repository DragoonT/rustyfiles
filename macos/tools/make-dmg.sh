#!/bin/sh
# Packs build/rustyfiles.app into build/rustyfiles-mac.dmg: the app and a link to
# /Applications to drag it onto. Run ./build.sh first. The name has no version in it, so
# README can link releases/latest/download/rustyfiles-mac.dmg. (The app in it runs on Apple
# silicon and Intel; a copy named rustyfiles-mac-arm64.dmg, as it was called when it was
# Apple silicon only, keeps the links made then working.)
#
# Signing and notarization happen by themselves when this Mac is set up for them:
#   - a "Developer ID Application" certificate in the keychain (Xcode › Settings › Accounts
#     › Manage Certificates), or SIGN_ID set to the identity to use;
#   - a notarytool profile named "rustyfiles" (or NOTARY_PROFILE), made once with
#     xcrun notarytool store-credentials rustyfiles --apple-id … --team-id …
# Without the certificate the app keeps build.sh's ad-hoc signature and macOS asks the
# person who downloads it to allow it once (Privacy & Security › Open Anyway).
set -eu
cd "$(dirname "$0")/.."
# (APP: another bundle to pack, such as the one vpk made in release.sh)
APP="${APP:-build/rustyfiles.app}"
OUT=build/rustyfiles-mac.dmg
PROFILE="${NOTARY_PROFILE:-rustyfiles}"
[ -d "$APP" ] || { echo "no $APP: run ./build.sh first" >&2; exit 1; }

# (by its SHA-1: a name with letters such as "Ö" does not survive the trip through the shell)
ID="${SIGN_ID:-$(security find-identity -v -p codesigning | awk '/"Developer ID Application: / { print $2; exit }')}"

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/rustyfiles.app"

if [ -n "$ID" ] && codesign -dv --verbose=2 "$STAGE/rustyfiles.app" 2>&1 | grep -q "Authority=Developer ID Application"; then
    echo "the app is already signed with a Developer ID (by vpk): kept as it is"
elif [ -n "$ID" ]; then
    # the hardened runtime and a secure timestamp are what notarization asks for; the app
    # needs no entitlements (it only receives Apple Events, and Ferry's local network use is
    # declared in Info.plist)
    echo "signing with $ID"
    codesign --force --options runtime --timestamp --sign "$ID" "$STAGE/rustyfiles.app"
    codesign --verify --strict --deep "$STAGE/rustyfiles.app"
else
    echo "no Developer ID certificate: the app stays ad-hoc signed and is not notarized" >&2
fi

ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
hdiutil create -volname rustyfiles -srcfolder "$STAGE" -fs APFS -format ULFO -quiet "$OUT"

if [ -n "$ID" ]; then
    codesign --force --timestamp --sign "$ID" "$OUT"
    if xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1; then
        echo "notarizing (usually a few minutes)…"
        xcrun notarytool submit "$OUT" --keychain-profile "$PROFILE" --wait
        # the ticket goes into the image, so it opens without a network check too
        xcrun stapler staple "$OUT"
        xcrun stapler validate "$OUT"
        spctl --assess --type open --context context:primary-signature -v "$OUT"
    else
        echo "no notarytool profile \"$PROFILE\": signed but not notarized" >&2
    fi
fi

echo "$OUT ($(du -h "$OUT" | cut -f1 | tr -d ' '), version $(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist"))"
