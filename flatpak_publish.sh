#!/usr/bin/env bash
set -euo pipefail

# Build + bundle Tabular Flatpak for distribution.
# This script creates a single .flatpak bundle file that you can
# upload to a web server or attach to releases. For Flathub, you
# typically submit the manifest to the Flathub repo instead.
#
# Usage:
#   ./publish_flatpak.sh            # build + bundle 'stable' branch
#   CHANNEL=beta ./publish_flatpak.sh  # use a different branch name

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$PROJECT_ROOT"

APP_ID="id.tabular.database"
CHANNEL="${CHANNEL:-stable}"
MANIFEST="$PROJECT_ROOT/flatpak/${APP_ID}.yml"
BUILD_DIR="$PROJECT_ROOT/flatpak/.build-dir"
REPO_DIR="$PROJECT_ROOT/flatpak/repo"
BUNDLE_DIR="$PROJECT_ROOT/flatpak/dist"

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: required tool '$1' is not installed." >&2
        exit 1
    fi
}

require_tool flatpak
require_tool flatpak-builder

if [[ ! -f "$MANIFEST" ]]; then
    echo "Error: Flatpak manifest not found at $MANIFEST" >&2
    exit 1
fi

mkdir -p "$BUILD_DIR" "$REPO_DIR" "$BUNDLE_DIR"

# Build into local repo
flatpak-builder \
  --force-clean \
  --disable-rofiles-fuse \
  --repo="$REPO_DIR" \
  "$BUILD_DIR" \
  "$MANIFEST"

BUNDLE_FILE="$BUNDLE_DIR/${APP_ID}-${CHANNEL}.flatpak"

# Create a single-file bundle from the repo
flatpak build-bundle "$REPO_DIR" "$BUNDLE_FILE" "$APP_ID" "$CHANNEL"

echo "Flatpak bundle created: $BUNDLE_FILE"
echo "You can distribute this file directly, or use it for testing via:"
echo "  flatpak install --user --bundle $BUNDLE_FILE"