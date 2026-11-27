#!/bin/sh
# Builds the Mac release into build/Releases, as release.yml does for Windows:
#   1. build.sh, for Apple silicon and Intel in one app (the version comes from the
#      workspace Cargo.toml);
#   2. vpk download: the previous osx release, so the new one gets a delta package;
#   3. vpk pack, channel osx: the app with Velopack's updater inside, signed with the
#      Developer ID, notarized (profile "rustyfiles"), as a .nupkg, releases.osx.json and
#      rustyfiles-osx-Portable.zip;
#   4. make-dmg.sh: that app in rustyfiles-mac.dmg, signed and notarized too, and the same
#      file as rustyfiles-mac-arm64.dmg, the name older links point to.
# Upload afterwards, once the Windows workflow has made the release:
#   cd build/Releases && gh release upload vX.Y.Z rustyfiles-X.Y.Z-osx-*.nupkg releases.osx.json \
#       assets.osx.json RELEASES-osx rustyfiles-osx-Portable.zip ../rustyfiles-mac.dmg ../rustyfiles-mac-arm64.dmg
# (not the previous version's full package, which vpk download fetched to make the delta)
# Needs vpk 1.2.161, the version release.yml uses: dotnet tool install -g vpk --version 1.2.161
set -eu
cd "$(dirname "$0")/.."
REPO=https://github.com/gelenmedya/rustyfiles
OUT=build/Releases
export PATH="$PATH:$HOME/.dotnet/tools"
# Homebrew's dotnet@8 is keg-only: vpk needs to be told where the runtime is
[ -d /opt/homebrew/opt/dotnet@8/libexec ] && export DOTNET_ROOT="${DOTNET_ROOT:-/opt/homebrew/opt/dotnet@8/libexec}"
command -v vpk >/dev/null || { echo "vpk not found: dotnet tool install -g vpk --version 1.2.161" >&2; exit 1; }

RF_UNIVERSAL=1 ./build.sh
VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' build/rustyfiles.app/Contents/Info.plist)"
ID="$(security find-identity -v -p codesigning | awk '/"Developer ID Application: / { print $2; exit }')"
[ -n "$ID" ] || { echo "no Developer ID Application certificate in the keychain" >&2; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT"
# (none yet for the first Mac release: then there is simply no delta)
vpk download github --repoUrl "$REPO" -c osx -o "$OUT" || true

# no Developer ID Installer certificate: no .pkg, the .dmg below is the installer
vpk pack -u rustyfiles -v "$VERSION" -c osx -o "$OUT" -p build/rustyfiles.app -e RustyFiles \
    --packTitle rustyfiles --packAuthors "Özer Güzel" -i build/rustyfiles.app/Contents/Resources/AppIcon.icns \
    --signAppIdentity "$ID" --signEntitlements rustyfiles.entitlements --notaryProfile rustyfiles --noInst

# the disk image carries the app vpk made (it has the updater in it)
rm -rf build/vpk-app
mkdir -p build/vpk-app
ditto -x -k "$OUT/rustyfiles-osx-Portable.zip" build/vpk-app
APP="$(find build/vpk-app -maxdepth 1 -name '*.app' | head -1)" tools/make-dmg.sh
cp build/rustyfiles-mac.dmg build/rustyfiles-mac-arm64.dmg
ls -la "$OUT" build/rustyfiles-mac.dmg build/rustyfiles-mac-arm64.dmg
