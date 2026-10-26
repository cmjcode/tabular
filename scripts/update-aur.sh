#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# Update AUR packaging metadata and push it to the external AUR repository.
# Usage:
#   scripts/update-aur.sh [tabular|tabular-bin|all]
#
# Environment variables:
#   TARGET_REPO        Override destination repo directory (for single package)
#   AUR_REPO_URL       Override AUR remote git URL
#   PKGVER             Override pkgver (defaults to version from Cargo.toml)
#   PKGREL             Override pkgrel (default: 1)
#   GIT_AUTHOR_NAME    Override git author name (default: Tabular CI or existing)
#   GIT_AUTHOR_EMAIL   Override git author email (default: support@tabular.id or existing)
# ==============================================================================

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_PKG="${1:-tabular}"

# Extract version from Cargo.toml using Python's tomllib with fallback to grep/sed
get_cargo_version() {
  local ver=""
  if command -v python3 >/dev/null 2>&1; then
    ver="$(python3 - <<'PY' 2>/dev/null || true
import pathlib, sys
try:
    import tomllib
except ModuleNotFoundError:
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        sys.exit(1)
cargo_toml = pathlib.Path("Cargo.toml")
if cargo_toml.exists():
    data = tomllib.loads(cargo_toml.read_text())
    print(data.get("package", {}).get("version", ""))
PY
)"
  fi
  if [[ -z "${ver}" ]]; then
    ver="$(grep -m1 '^version\s*=' "${ROOT_DIR}/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/' || true)"
  fi
  echo "${ver}"
}

CARGO_VERSION="$(get_cargo_version)"
if [[ -z "${CARGO_VERSION}" ]]; then
  echo "Failed to determine version from Cargo.toml" >&2
  exit 1
fi

VERSION="${PKGVER:-${CARGO_VERSION}}"
VERSION="${VERSION#v}" # remove leading 'v' if present
REL="${PKGREL:-1}"

# Helper to run makepkg --printsrcinfo safely (even if running as root in container/CI)
run_makepkg_srcinfo() {
  local dir="$1"
  (
    cd "${dir}"
    if [[ "$(id -u)" -eq 0 ]]; then
      # makepkg prohibits running as root even for --printsrcinfo.
      if ! id -u builder >/dev/null 2>&1; then
        useradd -m builder 2>/dev/null || true
      fi
      chmod -R a+r "${dir}" 2>/dev/null || true
      su builder -c "cd '${dir}' && makepkg --printsrcinfo" > .SRCINFO
    else
      makepkg --printsrcinfo > .SRCINFO
    fi
  )
}

update_single_package() {
  local package="$1"
  local pkgbuild_dir=""
  local default_target_repo=""

  case "${package}" in
    tabular)
      pkgbuild_dir="${ROOT_DIR}"
      default_target_repo="${HOME}/Documents/PROJECT/TABULAR/tabular-aur"
      ;;
    tabular-bin)
      pkgbuild_dir="${ROOT_DIR}/aur/tabular-bin"
      default_target_repo="${HOME}/Documents/PROJECT/TABULAR/tabular-bin-aur"
      ;;
    *)
      echo "Unknown package variant: ${package}" >&2
      echo "Usage: $0 [tabular|tabular-bin|all]" >&2
      exit 1
      ;;
  esac

  # Determine target repository directory
  local target_repo="${TARGET_REPO:-}"
  if [[ -z "${target_repo}" ]]; then
    if [[ -d "${default_target_repo}" ]]; then
      target_repo="${default_target_repo}"
    else
      target_repo="${ROOT_DIR}/dist/aur/${package}"
    fi
  fi

  local aur_url="${AUR_REPO_URL:-ssh://aur@aur.archlinux.org/${package}.git}"

  # If target repo is not a git repo, clone it
  if [[ ! -d "${target_repo}/.git" ]]; then
    echo "Cloning ${aur_url} into ${target_repo}..."
    mkdir -p "$(dirname "${target_repo}")"
    if ! git clone "${aur_url}" "${target_repo}"; then
      echo "Warning: Could not clone from ${aur_url}." >&2
    fi
  fi

  echo "Updating PKGBUILD (${package}) to version ${VERSION}-${REL}..."
  sed -i -e "s/^pkgver=.*/pkgver=${VERSION}/" "${pkgbuild_dir}/PKGBUILD"
  sed -i -e "s/^pkgrel=.*/pkgrel=${REL}/" "${pkgbuild_dir}/PKGBUILD"

  echo "Regenerating .SRCINFO for ${package}..."
  run_makepkg_srcinfo "${pkgbuild_dir}"

  echo "Copying packaging files to ${target_repo}..."
  mkdir -p "${target_repo}"
  cp "${pkgbuild_dir}/PKGBUILD" "${target_repo}/PKGBUILD"
  cp "${pkgbuild_dir}/.SRCINFO" "${target_repo}/.SRCINFO"

  if [[ -f "${pkgbuild_dir}/tabular-bin.install" ]]; then
    cp "${pkgbuild_dir}/tabular-bin.install" "${target_repo}/tabular-bin.install"
  fi

  if [[ -d "${target_repo}/.git" ]]; then
    echo "Committing and pushing changes for ${package} in ${target_repo}..."
    (
      cd "${target_repo}"
      git config --global --add safe.directory "${target_repo}" 2>/dev/null || true

      if ! git config user.name >/dev/null 2>&1; then
        git config user.name "${GIT_AUTHOR_NAME:-Tabular CI}"
      fi
      if ! git config user.email >/dev/null 2>&1; then
        git config user.email "${GIT_AUTHOR_EMAIL:-support@tabular.id}"
      fi

      FILES_TO_ADD=("PKGBUILD" ".SRCINFO")
      if [[ -f "tabular-bin.install" ]]; then
        FILES_TO_ADD+=("tabular-bin.install")
      fi

      git add "${FILES_TO_ADD[@]}"
      if git diff --cached --quiet; then
        echo "No changes to commit for ${package}."
      else
        git commit -m "${package} ${VERSION}"
        git push origin master || git push
        echo "Successfully committed and pushed ${package} ${VERSION} to AUR."
      fi
    )
  else
    echo "Warning: ${target_repo} is not a git repository. Skipping commit/push."
  fi
}

case "${TARGET_PKG}" in
  all)
    update_single_package "tabular"
    update_single_package "tabular-bin"
    ;;
  tabular|tabular-bin)
    update_single_package "${TARGET_PKG}"
    ;;
  *)
    echo "Unknown package variant: ${TARGET_PKG}" >&2
    echo "Usage: $0 [tabular|tabular-bin|all]" >&2
    exit 1
    ;;
esac

echo "AUR packaging update complete."
