#!/usr/bin/env bash
set -euo pipefail

# Build Debian source package for Launchpad inside an Ubuntu Noble Docker container.
# Requires Docker (or Podman with alias) available on the host Arch system.

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
IMAGE="${DEB_DOCKER_IMAGE:-ubuntu:noble}"
DEBFULLNAME="${DEBFULLNAME:-Tabular Team}"
DEBEMAIL="${DEBEMAIL:-support@tabular.id}"

if ! command -v docker >/dev/null 2>&1; then
    echo "Error: docker command not found. Install Docker or set up a docker-compatible alias." >&2
    exit 1
fi

# Launch container, install build deps, and run dpkg-buildpackage.
# Build artifacts (.dsc, .changes, .orig.tar.gz, .debian.tar.xz) will be written to PROJECT_ROOT/.. as usual.
docker run --rm \
    -e DEBFULLNAME="${DEBFULLNAME}" \
    -e DEBEMAIL="${DEBEMAIL}" \
    -v "${PROJECT_ROOT}:/build" \
    -w /build \
    "${IMAGE}" \
    bash -ceu '
        export DEBIAN_FRONTEND=noninteractive
        apt-get update
        apt-get install -y --no-install-recommends \
            build-essential \
            debhelper \
            devscripts \
            cargo \
            rustc \
            pkg-config \
            libssl-dev \
            libgtk-3-dev \
            libxcb1-dev \
            libxkbcommon-dev \
            libglib2.0-dev \
            libudev-dev \
            libpango1.0-dev \
            libatk1.0-dev
        dpkg-buildpackage -S -sa -us -uc
    '

# Display where the files ended up.
PACKAGE_VERSION=$(sed -n 's/^tabular (\([^)]*\)).*/\1/p' "${PROJECT_ROOT}/debian/changelog" | head -n1)
UPSTREAM_VERSION=${PACKAGE_VERSION%%-*}

printf "\nDebian source package artifacts generated in: %s\n" "${PROJECT_ROOT}/.."
ls -1 "${PROJECT_ROOT}/.."/tabular_${UPSTREAM_VERSION}* 2>/dev/null || true
ls -1 "${PROJECT_ROOT}/.."/tabular_${PACKAGE_VERSION}_* 2>/dev/null || true
