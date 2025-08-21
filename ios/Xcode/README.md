# Tabular iOS (Experimental)

This folder contains a minimal iOS Xcode project that links against the Rust static library build of Tabular.

Status: ONLY exposes FFI version + a stub run call. The full egui window isn't integrated (eframe on iOS experimental). Use this as base to expand a native iPad UI calling Rust logic.

## Layout
```
ios/Xcode/TabulariOS/TabulariOS.xcodeproj   # Xcode iOS project (manual pbxproj)
ios/Xcode/TabulariOS/Sources/               # Swift sources
ios/Xcode/scripts/                          # Build helper scripts
```

## Build Flow
1. Pre-build script compiles Rust for both iOS device & simulator targets (`aarch64-apple-ios` and `aarch64-apple-ios-sim` + optional `x86_64-apple-ios` if needed for older Simulators).
2. Creates an XCFramework aggregating the static libraries.
3. Swift target links the XCFramework.

## Using
1. Open `ios/Xcode/TabulariOS/TabulariOS.xcodeproj` in Xcode.
2. Select a simulator (iPad) or connected device.
3. Build & Run. UI shows version fetched from Rust.

## Extend FFI
Add new functions in `src/lib.rs` with `#[no_mangle] extern "C"` and rebuild.

