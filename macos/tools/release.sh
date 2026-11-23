#!/bin/sh
# Builds the Mac release into build/Releases, as release.yml does for Windows:
#   1. build.sh (the version comes from the workspace Cargo.toml);
#   2. vpk download: the previous osx release, so the new one gets a delta package;
#   3. vpk pack, channel osx: the app with Velopack's updater inside, signed with the
#      Developer ID, notarized (profile "rustyfiles"), as a .nupkg, releases.osx.json and
#      rustyfiles-osx-Portable.zip;
#   4. make-dmg.sh: that app in rustyfiles-mac-arm64.dmg, signed and notarized too.
# Upload afterwards, once the Windows workflow has made the release:
#   gh release upload vX.Y.Z build/Releases/* build/rustyfiles-mac-arm64.dmg
# Needs vpk 1.2.161, the version release.yml uses: dotnet tool install -g vpk --version 1.2.161
set -eu
cd "$(dirname "$0")/.."
REPO=https://github.com/gelenmedya/rustyfiles
OUT=build/Releases
export PATH="$PATH:$HOME/.dotnet/tools"
command -v vpk >/dev/null || { echo "vpk not found: dotnet tool install -g vpk --version 1.2.161" >&2; exit 1; }

./build.sh
VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' build/rustyfiles.app/Contents/Info.plist)"
ID="$(security find-identity -v -p codesigning | awk '/"Developer ID Application: / { print $2; exit }')"
[ -n "$ID" ] || { echo "no Developer ID Application certificate in the keychain" >&2; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT"
# (none yet for the first Mac release: then there is simply no delta)
vpk download github --repoUrl "$REPO" -c osx -o "$OUT" || true

# no Developer ID Installer certificate: no .pkg, the .dmg below is the installer
vpk pack -u rustyfiles -v "$VERSION" -c osx -o "$OUT" -p build/rustyfiles.app -e RustyFiles \
    --packTitle rustyfiles --packAuthors "Özer Güzel" -i target/mac/AppIcon.icns \
    --signAppIdentity "$ID" --notaryProfile rustyfiles --noInst

# the disk image carries the app vpk made (it has the updater in it)
rm -rf build/vpk-app
mkdir -p build/vpk-app
ditto -x -k "$OUT/rustyfiles-osx-Portable.zip" build/vpk-app
APP="$(find build/vpk-app -maxdepth 1 -name '*.app' | head -1)" tools/make-dmg.sh
ls -la "$OUT" build/rustyfiles-mac-arm64.dmg
