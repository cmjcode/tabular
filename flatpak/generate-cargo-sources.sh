#!/usr/bin/env bash
set -euo pipefail

# Generate vendored cargo dependencies for offline Flatpak build.
# This uses 'cargo vendor' instead of flatpak-cargo-generator for simplicity.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PROJECT_ROOT="$SCRIPT_DIR/.."
cd "$PROJECT_ROOT"

CARGO_LOCK="$PROJECT_ROOT/Cargo.lock"
VENDOR_DIR="$SCRIPT_DIR/cargo-vendor"

if [[ ! -f "$CARGO_LOCK" ]]; then
    echo "Error: Cargo.lock not found. Run 'cargo build' first to generate it." >&2
    exit 1
fi

echo "Vendoring cargo dependencies..."
rm -rf "$VENDOR_DIR"
mkdir -p "$VENDOR_DIR"

# Vendor all dependencies
cargo vendor "$VENDOR_DIR"

echo "Dependencies vendored to: $VENDOR_DIR"
echo "Now you can build with: sh flatpak_build.sh"
