#!/usr/bin/env bash
set -euo pipefail

# Build Debian source package for Launchpad inside an Ubuntu Noble Docker container.
# Requires Docker (or Podman with alias) available on the host Arch system.

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
IMAGE="${DEB_DOCKER_IMAGE:-tabular-deb-builder:noble}"
DOCKERFILE_PATH="${PROJECT_ROOT}/docker/deb-builder/Dockerfile"
FORCE_REBUILD="${FORCE_REBUILD:-0}"
DEBFULLNAME="${DEBFULLNAME:-Tabular Team}"
DEBEMAIL="${DEBEMAIL:-support@tabular.id}"

if ! command -v docker >/dev/null 2>&1; then
    echo "Error: docker command not found. Install Docker or set up a docker-compatible alias." >&2
    exit 1
fi

if [ "${FORCE_REBUILD}" = "1" ] || ! docker image inspect "${IMAGE}" >/dev/null 2>&1; then
    if [ ! -f "${DOCKERFILE_PATH}" ]; then
        echo "Error: Dockerfile not found at ${DOCKERFILE_PATH}." >&2
        exit 1
    fi
    echo "Building Docker image ${IMAGE} ..."
    docker build -f "${DOCKERFILE_PATH}" -t "${IMAGE}" "${PROJECT_ROOT}"/docker/deb-builder
fi

# Launch container with the prebuilt toolchain and run dpkg-buildpackage.
# Build artifacts (.dsc, .changes, .orig.tar.gz, .debian.tar.xz) will be written to PROJECT_ROOT/.. as usual.
docker run --rm \
    -e DEBFULLNAME="${DEBFULLNAME}" \
    -e DEBEMAIL="${DEBEMAIL}" \
    -v "${PROJECT_ROOT}:/build" \
    -w /build \
    "${IMAGE}" \
    bash -ceu '
        export DEBIAN_FRONTEND=noninteractive

        # Create an upstream orig tarball so dpkg-source (3.0 (quilt)) can find it.
        # dpkg-source expects tabular_<upstream-version>.orig.tar.* in the parent dir
        # of the source directory. /build is the mounted project; create the tar
        # in / (the parent) and exclude debian/ so it's a clean upstream tarball.
        PACKAGE_VERSION=$(sed -n 's/^tabular (\([^)]*\)).*/\1/p' /build/debian/changelog | head -n1)
        UPSTREAM_VERSION=${PACKAGE_VERSION%%-*}
        cd /
        tar -czf /tabular_${UPSTREAM_VERSION}.orig.tar.gz -C /build --exclude=debian .
        cd /build

        dpkg-buildpackage -S -sa -us -uc
    '

# Display where the files ended up.
PACKAGE_VERSION=$(sed -n 's/^tabular (\([^)]*\)).*/\1/p' "${PROJECT_ROOT}/debian/changelog" | head -n1)
UPSTREAM_VERSION=${PACKAGE_VERSION%%-*}

printf "\nDebian source package artifacts generated in: %s\n" "${PROJECT_ROOT}/.."
ls -1 "${PROJECT_ROOT}/.."/tabular_${UPSTREAM_VERSION}* 2>/dev/null || true
ls -1 "${PROJECT_ROOT}/.."/tabular_${PACKAGE_VERSION}_* 2>/dev/null || true
