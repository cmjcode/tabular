#!/bin/bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT_DIR"

CRATE_NAME=tabular
IOS_DEVICE=aarch64-apple-ios
ios_sim_targets=(aarch64-apple-ios-sim)
# Add x86_64-apple-ios-sim if you still target Intel simulator: rustup target add x86_64-apple-ios

echo "[Rust][iOS] Building device staticlib"
cargo build --release --target $IOS_DEVICE

device_lib=target/$IOS_DEVICE/release/lib${CRATE_NAME}.a
if [ ! -f "$device_lib" ]; then echo "Missing device lib"; exit 1; fi

SIM_LIBS=()
for T in "${ios_sim_targets[@]}"; do
  echo "[Rust][iOS] Building sim target $T"
  rustup target add "$T" >/dev/null 2>&1 || true
  cargo build --release --target "$T"
  sim_lib=target/$T/release/lib${CRATE_NAME}.a
  if [ ! -f "$sim_lib" ]; then echo "Missing sim lib for $T"; exit 1; fi
  SIM_LIBS+=("$sim_lib")
done

OUT_DIR=ios/Xcode/TabulariOS/Generated
mkdir -p "$OUT_DIR"
XCFRAMEWORK="$OUT_DIR/Tabular.xcframework"
rm -rf "$XCFRAMEWORK"

# Create lipo for simulator slice if multiple archs (currently single arch list)
if [ ${#SIM_LIBS[@]} -gt 1 ]; then
  lipo -create "${SIM_LIBS[@]}" -output "$OUT_DIR/lib${CRATE_NAME}-sim.a"
  SIM_FAT="$OUT_DIR/lib${CRATE_NAME}-sim.a"
else
  SIM_FAT="${SIM_LIBS[0]}"
fi

xcodebuild -create-xcframework \
  -library "$device_lib" -headers ios/Xcode/TabulariOS/Sources/FFI \
  -library "$SIM_FAT" -headers ios/Xcode/TabulariOS/Sources/FFI \
  -output "$XCFRAMEWORK"

echo "[OK] XCFramework: $XCFRAMEWORK"
