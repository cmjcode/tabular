#!/usr/bin/env bash
set -euo pipefail

# Upload Debian source package artifacts to Launchpad using dput.
# Relies on the source package produced by build_deb_source_docker.sh.

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PKG_NAME=${PKG_NAME:-tabular}
CHANGES_FILE=${CHANGES_FILE:-}

if [[ -z "$CHANGES_FILE" ]]; then
    PACKAGE_VERSION=$(sed -n "s/^${PKG_NAME} (\([^)]*\)).*/\1/p" "${PROJECT_ROOT}/debian/changelog" | head -n1)
    if [[ -z "$PACKAGE_VERSION" ]]; then
        echo "Error: could not determine package version from debian/changelog." >&2
        exit 1
    fi
    CHANGES_FILE="${PROJECT_ROOT}/../${PKG_NAME}_${PACKAGE_VERSION}_source.changes"
fi

if [[ ! -f "$CHANGES_FILE" ]]; then
    echo "Error: .changes file not found at: $CHANGES_FILE" >&2
    echo "Hint: run ./build_deb_source_docker.sh first or set CHANGES_FILE explicitly." >&2
    exit 1
fi

if ! command -v dput >/dev/null 2>&1; then
    echo "Error: dput command not found. Install the devscripts package." >&2
    exit 1
fi

echo "Uploading $CHANGES_FILE to ppa:yulius-jayuda/tabular..."
dput ppa:yulius-jayuda/tabular "$CHANGES_FILE"