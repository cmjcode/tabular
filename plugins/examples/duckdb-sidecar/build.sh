#!/bin/sh
# Build sidecar dan siapkan folder plugin untuk "Install from Folder...".
# Build pertama mengompilasi DuckDB (C++) dan bisa memakan waktu lama.
set -e
cd "$(dirname "$0")"
cargo build --release
mkdir -p dist/bin
EXE=tabular-duckdb-sidecar
[ -f "target/release/$EXE.exe" ] && EXE="$EXE.exe"
cp "target/release/$EXE" "dist/bin/$EXE"
cp manifest.json dist/manifest.json
echo "Plugin folder ready: $(pwd)/dist"
