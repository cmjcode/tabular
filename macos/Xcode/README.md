# Xcode Project Integration

This folder contains the Xcode project scaffolding so you can:

1. Open the project in Xcode (UI, signing, archives)
2. Build & Archive a macOS App Store ready binary
3. Reuse the existing Rust sources as the core executable

## Overview
We treat Rust as the core (built via Cargo) and wrap it with a minimal Xcode project whose build phases invoke Cargo, then embed the produced universal binary inside the app bundle before code signing & archiving.

## Project Layout
```
macos/Xcode/Tabular.xcodeproj      # Xcode project
macos/Xcode/TabularApp/            # Wrapper app sources (very small Swift main)
macos/Xcode/scripts/               # Helper build scripts
```

## Build Flow (Release / Archive)
1. Pre-build script runs `cargo build --release` (or universal via lipo) producing `target/universal-apple-darwin/release/tabular`.
2. The Swift wrapper target copies Info.plist & resources.
3. A build phase replaces the generated `TabularApp` binary with the Rust binary renamed to `Tabular` (or you can keep a thin Swift stub that launches Rust binary if you prefer).
4. Codesign & Archive through Xcode Organizer.

## Universal Binary
The provided script builds both archs and uses `lipo` similar to the Makefile target.

## Steps to Use
1. Open `macos/Xcode/Tabular.xcodeproj` in Xcode.
2. Set your Team and Signing (Bundle ID: `id.tabular.database`).
3. Product > Archive.
4. Distribute via Organizer (App Store or Notarize Developer ID).

## Keep Versions in Sync
The script reads version from Cargo.toml and propagates CFBundleShortVersionString / CFBundleVersion.

## Updating
If dependencies change or you add resources, adjust the copy phases inside the Xcode project.

