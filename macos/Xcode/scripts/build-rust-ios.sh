#!/bin/bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT_DIR"

TARGET="aarch64-apple-ios"
echo "[Rust][iOS] building target $TARGET (release)"
cargo build --target $TARGET --release

OUT="target/$TARGET/release/libtabular.a"
if [ ! -f "$OUT" ]; then
  echo "[ERR] static lib not produced at $OUT" >&2
  exit 1
fi

echo "[OK] iOS static library ready: $OUT"
