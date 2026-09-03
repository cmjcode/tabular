#!/bin/bash
# ==============================================================================
# Tabular Xcode Cargo Build Bridge
# ==============================================================================
# This script is called by Xcode's Run Script Phase to build the Rust binary
# for the selected SDK, architecture, and configuration, then copies the binary
# and dSYM into the Xcode build products folder for seamless archiving and
# App Store Connect upload.
# ==============================================================================

set -e

# Prepend standard cargo & toolchain paths
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPLE_DIR="$(dirname "$SCRIPT_DIR")"
CLIENT_DIR="$(dirname "$APPLE_DIR")"
cd "$CLIENT_DIR"

echo "=================================================="
echo "🚀 [Tabular Xcode Bridge] Starting Cargo Build Phase"
echo "  Configuration : ${CONFIGURATION:-Release}"
echo "  Platform      : ${PLATFORM_NAME:-macosx}"
echo "  Architectures : ${ARCHS:-arm64}"
echo "  Action        : ${ACTION:-build}"
echo "  Built Products: ${BUILT_PRODUCTS_DIR:-dist}"
echo "=================================================="

# 1. Verify Rust & Cargo
if ! command -v cargo &>/dev/null; then
    echo "error: Cargo not found in PATH ($PATH). Please ensure Rust is installed." >&2
    exit 1
fi

# 2. Determine Rust target
RUST_TARGET=""
case "${PLATFORM_NAME:-macosx}" in
    iphoneos)
        RUST_TARGET="aarch64-apple-ios"
        ;;
    iphonesimulator)
        if [[ "${ARCHS:-arm64}" == *"arm64"* ]]; then
            RUST_TARGET="aarch64-apple-ios-sim"
        else
            RUST_TARGET="x86_64-apple-ios"
        fi
        ;;
    macosx)
        if [[ "${ARCHS:-arm64}" == *"arm64"* ]]; then
            RUST_TARGET="aarch64-apple-darwin"
        else
            RUST_TARGET="x86_64-apple-darwin"
        fi
        ;;
    *)
        echo "warning: Unknown platform ${PLATFORM_NAME}, defaulting to host arch" >&2
        RUST_TARGET="aarch64-apple-darwin"
        ;;
esac

echo "🎯 Selected Rust Target: $RUST_TARGET"

# 3. Run Cargo Build
CARGO_FLAGS="--target $RUST_TARGET --bin tabular"
BUILD_SUBDIR="release"

if [ "${CONFIGURATION}" = "Debug" ] && [ "${ACTION}" != "install" ]; then
    BUILD_SUBDIR="debug"
else
    CARGO_FLAGS="$CARGO_FLAGS --release"
fi

echo "📦 Running: cargo build $CARGO_FLAGS"
cargo build $CARGO_FLAGS

SOURCE_BIN="$CLIENT_DIR/target/$RUST_TARGET/$BUILD_SUBDIR/tabular"
if [ ! -f "$SOURCE_BIN" ]; then
    echo "error: Compiled binary not found at $SOURCE_BIN" >&2
    exit 1
fi

# 4. Copy Binary to Xcode's Built Products Destination
if [ -n "$BUILT_PRODUCTS_DIR" ] && [ -n "$EXECUTABLE_PATH" ]; then
    DEST_BIN="$BUILT_PRODUCTS_DIR/$EXECUTABLE_PATH"
    echo "📋 Copying binary to $DEST_BIN"
    mkdir -p "$(dirname "$DEST_BIN")"
    cp "$SOURCE_BIN" "$DEST_BIN"
    chmod +x "$DEST_BIN"
    
    # 5. Generate dSYM for App Store Crash Symbolication
    if [ -n "$DWARF_DSYM_FOLDER_PATH" ] && [ -n "$DWARF_DSYM_FILE_NAME" ] && command -v dsymutil &>/dev/null; then
        echo "🔍 Generating dSYM at $DWARF_DSYM_FOLDER_PATH/$DWARF_DSYM_FILE_NAME"
        mkdir -p "$DWARF_DSYM_FOLDER_PATH"
        dsymutil "$SOURCE_BIN" -o "$DWARF_DSYM_FOLDER_PATH/$DWARF_DSYM_FILE_NAME" 2>/dev/null || true
    fi

    # 6. Copy application assets if present
    APP_DIR="$(dirname "$DEST_BIN")"
    if [[ "$PLATFORM_NAME" == "macosx" ]]; then
        APP_DIR="$(dirname "$APP_DIR")/Resources"
    fi
    mkdir -p "$APP_DIR"
    if [ -d "$CLIENT_DIR/assets" ]; then
        cp -r "$CLIENT_DIR/assets" "$APP_DIR/assets" 2>/dev/null || true
    fi
fi

echo "✅ [Tabular Xcode Bridge] Cargo build phase finished successfully!"
