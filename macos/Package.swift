// swift-tools-version: 6.0
// The macOS app. Build it with `macos/build.sh`, which first builds the Rust core
// (crates/rf-mac) and generates its Swift bindings into Sources/.

import Foundation
import PackageDescription

let repoRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path

let package = Package(
    name: "RustyFiles",
    platforms: [.macOS("26.0")],
    targets: [
        // The C header UniFFI generates for the Rust core (copied in by build.sh).
        .target(name: "rf_macFFI", path: "Sources/rf_macFFI"),
        .executableTarget(
            name: "RustyFiles",
            dependencies: ["rf_macFFI"],
            path: "Sources/RustyFiles",
            linkerSettings: [
                // The static library by full path, so the linker never picks the .dylib next to it.
                .unsafeFlags(["\(repoRoot)/target/mac/release/librf_mac.a"]),
                .linkedFramework("AppKit"),
                .linkedFramework("Quartz"),
                .linkedFramework("CoreWLAN"),
                .linkedFramework("CoreFoundation"),
                .linkedLibrary("iconv"),
            ]
        ),
    ],
    swiftLanguageModes: [.v5]
)
