#!/bin/sh
# Build driver.wasm dan siapkan folder plugin yang bisa di-install dari
# Plugins -> Database Drivers -> "Install from Folder...".
set -e
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown
mkdir -p dist
cp target/wasm32-unknown-unknown/release/tabular_driver_clickhouse.wasm dist/driver.wasm
cp manifest.json dist/manifest.json
echo "Plugin folder ready: $(pwd)/dist"
