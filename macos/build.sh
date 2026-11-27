#!/bin/bash
# Builds the Rust core, generates its Swift bindings, builds the Swift app and wraps it in
# macos/build/rustyfiles.app (ad-hoc signed). Usage: macos/build.sh [debug|release]
set -euo pipefail
cd "$(dirname "$0")/.."

CONFIG="${1:-release}"
# The app runs on macOS 26 and later; the Rust core's C parts (zstd, …) must be built for it too.
export MACOSX_DEPLOYMENT_TARGET=26.0
SWIFT_SRC=macos/Sources

# rf-mac is a workspace of its own (the root workspace is the Windows app's); its build goes
# to target/mac so it never disturbs the Windows build's files.
RF_MAC=(--manifest-path crates/rf-mac/Cargo.toml --target-dir target/mac)

echo "==> Rust core (rf-mac)"
cargo build "${RF_MAC[@]}" --lib --release
# RF_UNIVERSAL=1 (release.sh): Apple silicon and Intel in one app. The core is built for
# both and joined into one library; Package.swift links that one (RF_RUST_LIB).
SWIFT_ARCH=()
if [ "${RF_UNIVERSAL:-0}" = 1 ]; then
    cargo build "${RF_MAC[@]}" --lib --release --target aarch64-apple-darwin
    cargo build "${RF_MAC[@]}" --lib --release --target x86_64-apple-darwin
    mkdir -p target/mac/universal
    lipo -create target/mac/aarch64-apple-darwin/release/librf_mac.a target/mac/x86_64-apple-darwin/release/librf_mac.a \
        -output target/mac/universal/librf_mac.a
    export RF_RUST_LIB="$PWD/target/mac/universal/librf_mac.a"
    SWIFT_ARCH=(--arch arm64 --arch x86_64)
fi

echo "==> Swift bindings"
cargo run -q --release "${RF_MAC[@]}" --bin uniffi-bindgen -- generate \
    --library target/mac/release/librf_mac.dylib --language swift --out-dir target/mac/uniffi-swift
mkdir -p "$SWIFT_SRC/RustyFiles/Generated" "$SWIFT_SRC/rf_macFFI/include"
cp target/mac/uniffi-swift/rf_mac.swift "$SWIFT_SRC/RustyFiles/Generated/RustCore.swift"
cp target/mac/uniffi-swift/rf_macFFI.h "$SWIFT_SRC/rf_macFFI/include/rf_macFFI.h"
cp target/mac/uniffi-swift/rf_macFFI.modulemap "$SWIFT_SRC/rf_macFFI/include/module.modulemap"

echo "==> Swift app ($CONFIG)"
swift build --package-path macos -c "$CONFIG" ${SWIFT_ARCH[@]+"${SWIFT_ARCH[@]}"}
BIN="$(swift build --package-path macos -c "$CONFIG" ${SWIFT_ARCH[@]+"${SWIFT_ARCH[@]}"} --show-bin-path)"

echo "==> Bundle"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
APP=macos/build/rustyfiles.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/RustyFiles" "$APP/Contents/MacOS/RustyFiles"
sed "s/__VERSION__/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"

# The app icon, from the logo the Windows icon is made from (made again only when it changes).
LOGO=crates/rf-app/assets/rustyfiles.svg
ICNS=target/mac/AppIcon.icns
if [ ! -f "$ICNS" ] || [ "$LOGO" -nt "$ICNS" ] || [ macos/tools/make_icon.swift -nt "$ICNS" ]; then
    swift macos/tools/make_icon.swift "$LOGO" "$ICNS"
fi
cp "$ICNS" "$APP/Contents/Resources/AppIcon.icns"

# The Lucide icons the interface draws (the set the Windows app uses, from gpui-kit-assets).
LUCIDE="$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/gpui-kit-assets-0.9.0/assets/icons | head -1)"
mkdir -p "$APP/Contents/Resources/icons"
grep -v '^#' macos/icons.txt | while read -r name; do
    [ -n "$name" ] && cp "$LUCIDE/$name.svg" "$APP/Contents/Resources/icons/"
done
cp "$LUCIDE/../../LICENSE-LUCIDE" "$APP/Contents/Resources/icons/"

# The interface in other languages (macos/Localization/<lang>.json)
python3 macos/tools/l10n.py macos/Sources/RustyFiles macos/Localization "$APP/Contents/Resources"
codesign --force --sign - "$APP" >/dev/null
echo "$APP"
