#!/usr/bin/env bash
set -euo pipefail

# Simple Flatpak builder for Tabular (local build).
# Usage:
#   ./build_flatpak.sh               # build into local repo
#   ./build_flatpak.sh --run         # build then run the app
#   ./build_flatpak.sh --clean       # clean build dir first
#
# Requires: flatpak, flatpak-builder and the
# org.freedesktop.Sdk.Extension.rust-stable extension.

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$PROJECT_ROOT"

APP_ID="id.tabular.database"
MANIFEST="$PROJECT_ROOT/flatpak/${APP_ID}.yml"
BUILD_DIR="$PROJECT_ROOT/flatpak/.build-dir"
REPO_DIR="$PROJECT_ROOT/flatpak/repo"
RUN_AFTER_BUILD=false
CLEAN_FIRST=false

print_help() {
    cat <<EOF
Usage: $0 [options]

Options:
  --run           Build and run the Flatpak app.
  --clean         Remove previous Flatpak build directory before building.
  --help          Show this help.
EOF
}

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: required tool '$1' is not installed." >&2
        exit 1
    fi
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --run)
            RUN_AFTER_BUILD=true
            shift
            ;;
        --clean)
            CLEAN_FIRST=true
            shift
            ;;
        --help|-h)
            print_help
            exit 0
            ;;
        *)
            echo "Error: unknown option '$1'." >&2
            print_help
            exit 1
            ;;
    esac
done

require_tool flatpak
require_tool flatpak-builder

if [[ ! -f "$MANIFEST" ]]; then
    echo "Error: Flatpak manifest not found at $MANIFEST" >&2
    exit 1
fi

if [[ "$CLEAN_FIRST" == true ]]; then
    rm -rf "$BUILD_DIR" "$REPO_DIR"
fi

mkdir -p "$BUILD_DIR" "$REPO_DIR"

# Build into a local OSTree repo that can be used for bundling or testing.
flatpak-builder \
  --force-clean \
  --repo="$REPO_DIR" \
  "$BUILD_DIR" \
  "$MANIFEST"

echo "Flatpak build finished. Local repo: $REPO_DIR"

echo "To install locally for this user, run:"
echo "  flatpak --user remote-add --if-not-exists tabular-local file://$REPO_DIR"
echo "  flatpak --user install tabular-local $APP_ID"

if [[ "$RUN_AFTER_BUILD" == true ]]; then
    echo "Running $APP_ID inside Flatpak sandbox..."
    flatpak-builder --run "$BUILD_DIR" "$APP_ID" tabular
fi
