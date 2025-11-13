#!/usr/bin/env bash
set -euo pipefail

# Update the AUR packaging metadata and push it to the external AUR repo.
# Usage: update-aur.sh [tabular|tabular-bin]
# - Syncs pkgver with Cargo.toml
# - Regenerates .SRCINFO via makepkg
# - Copies PKGBUILD and .SRCINFO into the destination repo
# - Commits and pushes the changes if there are any

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE="${1:-tabular}"

case "${PACKAGE}" in
  tabular)
    PKGBUILD_DIR="${ROOT_DIR}"
    DEFAULT_TARGET_REPO="${HOME}/Documents/PROJECT/TABULAR/tabular-aur"
    ;;
  tabular-bin)
    PKGBUILD_DIR="${ROOT_DIR}/aur/tabular-bin"
    DEFAULT_TARGET_REPO="${HOME}/Documents/PROJECT/TABULAR/tabular-bin-aur"
    ;;
  *)
    echo "Unknown package variant: ${PACKAGE}" >&2
    echo "Usage: $0 [tabular|tabular-bin]" >&2
    exit 1
    ;;
esac

TARGET_REPO="${TARGET_REPO:-${DEFAULT_TARGET_REPO}}"

if [[ ! -f "${ROOT_DIR}/Cargo.toml" ]]; then
  echo "Cargo.toml not found at repo root: ${ROOT_DIR}" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to extract the version from Cargo.toml" >&2
  exit 1
fi

if ! command -v makepkg >/dev/null 2>&1; then
  echo "makepkg is required to generate .SRCINFO (pacman package base-devel)" >&2
  exit 1
fi

# Extract version from Cargo.toml using Python's tomllib (Python 3.11+).
CARGO_VERSION="$(python3 - <<'PY'
import pathlib
import sys
try:
    import tomllib
except ModuleNotFoundError:
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        sys.exit("Python module tomllib/tomli is required")

cargo_toml = pathlib.Path("Cargo.toml")
data = tomllib.loads(cargo_toml.read_text())
print(data["package"]["version"])
PY
)"

if [[ -z "${CARGO_VERSION}" ]]; then
  echo "Failed to determine version from Cargo.toml" >&2
  exit 1
fi

echo "Updating PKGBUILD (${PACKAGE}) to version ${CARGO_VERSION}";
sed -i -e "s/^pkgver=.*/pkgver=${CARGO_VERSION}/" "${PKGBUILD_DIR}/PKGBUILD"
sed -i -e "s/^pkgrel=.*/pkgrel=1/" "${PKGBUILD_DIR}/PKGBUILD"

echo "Regenerating .SRCINFO"
(
  cd "${PKGBUILD_DIR}"
  makepkg --printsrcinfo > .SRCINFO
)

echo "Copying PKGBUILD and .SRCINFO to ${TARGET_REPO}"
mkdir -p "${TARGET_REPO}"
cp "${PKGBUILD_DIR}/PKGBUILD" "${TARGET_REPO}/PKGBUILD"
cp "${PKGBUILD_DIR}/.SRCINFO" "${TARGET_REPO}/.SRCINFO"

echo "Committing and pushing changes"
(
  cd "${TARGET_REPO}"
  git add PKGBUILD .SRCINFO
  if git diff --cached --quiet; then
    echo "No changes to commit."
  else
    git commit -m "${PACKAGE} ${CARGO_VERSION}"
    git push
  fi
)

echo "AUR packaging update complete."
