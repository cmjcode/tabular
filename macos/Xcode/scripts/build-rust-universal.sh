#!/bin/bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT_DIR"

ARCHS=("x86_64-apple-darwin" "aarch64-apple-darwin")
APP_NAME="Tabular"
CARGO_BIN="tabular"
UNIVERSAL_DIR="target/universal-apple-darwin/release"

VERSION=$(grep '^version' Cargo.toml | head -n1 | cut -d '"' -f2)

echo "[Rust] Building universal binary for $APP_NAME v$VERSION"
for TARGET in "${ARCHS[@]}"; do
  echo "[Rust] cargo build --release --target $TARGET"
  cargo build --release --target "$TARGET"
  if [ ! -f "target/$TARGET/release/$CARGO_BIN" ]; then
    echo "[Error] Missing built binary for $TARGET" >&2
    exit 1
  fi
done
mkdir -p "$UNIVERSAL_DIR"
UNIVERSAL_BIN="$UNIVERSAL_DIR/$CARGO_BIN"

echo "[lipo] Creating universal binary -> $UNIVERSAL_BIN"
lipo -create \
  target/x86_64-apple-darwin/release/$CARGO_BIN \
  target/aarch64-apple-darwin/release/$CARGO_BIN \
  -output "$UNIVERSAL_BIN"
chmod +x "$UNIVERSAL_BIN"

# Export variables for later Xcode build phases
echo "EXPORT_VERSION=$VERSION" > "$UNIVERSAL_DIR/xc-env.txt"
echo "EXPORT_BINARY=$UNIVERSAL_BIN" >> "$UNIVERSAL_DIR/xc-env.txt"

echo "[Done] Universal Rust binary ready: $UNIVERSAL_BIN"
