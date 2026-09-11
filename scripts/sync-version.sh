#!/usr/bin/env bash
# ==============================================================================
# Tabular Version & Packaging Metadata Synchronization Script
# ==============================================================================
# Usage:
#   ./scripts/sync-version.sh [NEW_VERSION]
#
# Examples:
#   ./scripts/sync-version.sh            # Syncs all metadata to current version in Cargo.toml
#   ./scripts/sync-version.sh 0.16.3     # Bumps Cargo.toml to 0.16.3 and syncs all files
#
# Synchronized targets:
#   1. Cargo.toml (package.version)
#   2. id.tabular.database.metainfo.xml (AppStream release history & date)
#   3. flatpak/id.tabular.database.flathub.yml (git tag)
#   4. PKGBUILD (root Arch package)
#   5. .SRCINFO (root Arch package metadata)
#   6. aur/tabular-bin/PKGBUILD (AUR binary package)
#   7. aur/tabular-bin/.SRCINFO (AUR binary metadata)
#   8. Tabular.xcodeproj/project.pbxproj (MARKETING_VERSION & CURRENT_PROJECT_VERSION)
#   9. SECURITY.md (Supported versions table)
# ==============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

print_info() {
    echo -e "${BLUE}==>${NC} ${BOLD}$1${NC}"
}

print_success() {
    echo -e "${GREEN}==>${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}WARNING:${NC} $1"
}

print_error() {
    echo -e "${RED}ERROR:${NC} $1" >&2
}

# Require Python 3 for safe, portable multi-file regex replacement
if ! command -v python3 >/dev/null 2>&1; then
    print_error "python3 is required to run sync-version.sh"
    exit 1
fi

TARGET_VERSION="${1:-}"

# Run Python helper to parse / update
python3 - "${TARGET_VERSION}" << 'PY'
import sys
import os
import re
import datetime
import pathlib

root = pathlib.Path(".").resolve()
cargo_toml = root / "Cargo.toml"

if not cargo_toml.exists():
    print(f"Error: Cargo.toml not found at {cargo_toml}", file=sys.stderr)
    sys.exit(1)

cargo_content = cargo_toml.read_text(encoding="utf-8")
current_version_match = re.search(r'^version\s*=\s*"([^"]+)"', cargo_content, re.MULTILINE)
if not current_version_match:
    print("Error: Could not extract current version from Cargo.toml", file=sys.stderr)
    sys.exit(1)

current_version = current_version_match.group(1)
requested_version = sys.argv[1].strip() if len(sys.argv) > 1 and sys.argv[1].strip() else None

target_version = requested_version if requested_version else current_version

# Validate SemVer format
if not re.match(r'^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$', target_version):
    print(f"Error: Invalid version format '{target_version}'. Expected SemVer like '0.16.2'", file=sys.stderr)
    sys.exit(1)

print(f"\033[1mSyncing Tabular release metadata to version: \033[32m{target_version}\033[0m")
if requested_version and requested_version != current_version:
    print(f"  (Bumping from {current_version} -> {target_version})")

today_str = datetime.date.today().strftime("%Y-%m-%d")

# Derive Xcode project build version (e.g. 0.16.2 -> 162, 0.17.0 -> 170)
parts = target_version.split("-")[0].split(".")
try:
    major, minor, patch = int(parts[0]), int(parts[1]), int(parts[2])
    build_version = f"{major * 1000 + minor * 10 + patch}" if major > 0 else f"{minor * 10 + patch}"
except Exception:
    build_version = "1"

updated_files = []

# 1. Cargo.toml
if requested_version and requested_version != current_version:
    new_cargo = re.sub(r'^version\s*=\s*"[^"]+"', f'version = "{target_version}"', cargo_content, count=1, flags=re.MULTILINE)
    cargo_toml.write_text(new_cargo, encoding="utf-8")
    updated_files.append("Cargo.toml")

# 2. PKGBUILD (root)
pkgbuild_file = root / "PKGBUILD"
if pkgbuild_file.exists():
    content = pkgbuild_file.read_text(encoding="utf-8")
    new_content = re.sub(r'^pkgver=.*$', f'pkgver={target_version}', content, flags=re.MULTILINE)
    new_content = re.sub(r'^pkgrel=.*$', 'pkgrel=1', new_content, flags=re.MULTILINE)
    if new_content != content:
        pkgbuild_file.write_text(new_content, encoding="utf-8")
        updated_files.append("PKGBUILD")

# 3. .SRCINFO (root)
srcinfo_file = root / ".SRCINFO"
if srcinfo_file.exists():
    content = srcinfo_file.read_text(encoding="utf-8")
    new_content = re.sub(r'^\s*pkgver\s*=\s*.*$', f'\tpkgver = {target_version}', content, flags=re.MULTILINE)
    new_content = re.sub(r'^\s*pkgrel\s*=\s*.*$', '\tpkgrel = 1', new_content, flags=re.MULTILINE)
    new_content = re.sub(r'tabular-[0-9.]+\.tar\.gz::https://github\.com/tabular-id/tabular/archive/refs/tags/v[0-9.]+\.tar\.gz',
                         f'tabular-{target_version}.tar.gz::https://github.com/tabular-id/tabular/archive/refs/tags/v{target_version}.tar.gz',
                         new_content)
    if new_content != content:
        srcinfo_file.write_text(new_content, encoding="utf-8")
        updated_files.append(".SRCINFO")

# 4. aur/tabular-bin/PKGBUILD
aur_pkgbuild = root / "aur" / "tabular-bin" / "PKGBUILD"
if aur_pkgbuild.exists():
    content = aur_pkgbuild.read_text(encoding="utf-8")
    new_content = re.sub(r'^pkgver=.*$', f'pkgver={target_version}', content, flags=re.MULTILINE)
    new_content = re.sub(r'^pkgrel=.*$', 'pkgrel=1', new_content, flags=re.MULTILINE)
    if new_content != content:
        aur_pkgbuild.write_text(new_content, encoding="utf-8")
        updated_files.append("aur/tabular-bin/PKGBUILD")

# 5. aur/tabular-bin/.SRCINFO
aur_srcinfo = root / "aur" / "tabular-bin" / ".SRCINFO"
if aur_srcinfo.exists():
    content = aur_srcinfo.read_text(encoding="utf-8")
    new_content = re.sub(r'^\s*pkgver\s*=\s*.*$', f'\tpkgver = {target_version}', content, flags=re.MULTILINE)
    new_content = re.sub(r'^\s*pkgrel\s*=\s*.*$', '\tpkgrel = 1', new_content, flags=re.MULTILINE)
    new_content = re.sub(r'/v[0-9.]+(?=/|\.tar\.gz)', f'/v{target_version}', new_content)
    if new_content != content:
        aur_srcinfo.write_text(new_content, encoding="utf-8")
        updated_files.append("aur/tabular-bin/.SRCINFO")

# 6. flatpak/id.tabular.database.flathub.yml
flatpak_flathub = root / "flatpak" / "id.tabular.database.flathub.yml"
if flatpak_flathub.exists():
    content = flatpak_flathub.read_text(encoding="utf-8")
    new_content = re.sub(r'tag:\s*v[0-9.]+', f'tag: v{target_version}', content)
    if new_content != content:
        flatpak_flathub.write_text(new_content, encoding="utf-8")
        updated_files.append("flatpak/id.tabular.database.flathub.yml")

# 7. id.tabular.database.metainfo.xml
metainfo_file = root / "id.tabular.database.metainfo.xml"
if metainfo_file.exists():
    content = metainfo_file.read_text(encoding="utf-8")
    if f'<release version="{target_version}"' not in content:
        # Insert new release entry at top of <releases>
        new_release = f'''  <releases>
    <release version="{target_version}" date="{today_str}">
      <description>
        <p>Tabular v{target_version} release with stability, performance, and UI enhancements.</p>
        <ul>
          <li>Performance improvements and bug fixes</li>
        </ul>
      </description>
    </release>'''
        new_content = content.replace("  <releases>", new_release, 1)
        if new_content != content:
            metainfo_file.write_text(new_content, encoding="utf-8")
            updated_files.append("id.tabular.database.metainfo.xml")

# 8. Tabular.xcodeproj/project.pbxproj
pbxproj_file = root / "Tabular.xcodeproj" / "project.pbxproj"
if pbxproj_file.exists():
    content = pbxproj_file.read_text(encoding="utf-8")
    new_content = re.sub(r'MARKETING_VERSION = [^;]+;', f'MARKETING_VERSION = {target_version};', content)
    new_content = re.sub(r'CURRENT_PROJECT_VERSION = [^;]+;', f'CURRENT_PROJECT_VERSION = {build_version};', new_content)
    if new_content != content:
        pbxproj_file.write_text(new_content, encoding="utf-8")
        updated_files.append("Tabular.xcodeproj/project.pbxproj")

# 9. SECURITY.md
security_file = root / "SECURITY.md"
if security_file.exists():
    content = security_file.read_text(encoding="utf-8")
    series = f"{parts[0]}.{parts[1]}.x"
    prev_series = f"< {parts[0]}.{parts[1]}.0"
    table_pattern = r'(\|\s*Version\s*\|\s*Supported\s*\|\n\|\s*[-]+\s*\|\s*[-]+\s*\|\n)(\|\s*[^\|]+\|\s*[^\|]+\|\n)+'
    new_table = f"\\1| {series:<6} | :white_check_mark: |\n| {prev_series:<7} | :x:                |\n"
    new_content = re.sub(table_pattern, new_table, content)
    if new_content != content:
        security_file.write_text(new_content, encoding="utf-8")
        updated_files.append("SECURITY.md")

if updated_files:
    print(f"\n\033[32m✔ Successfully updated {len(updated_files)} file(s):\033[0m")
    for f in updated_files:
        print(f"  • {f}")
else:
    print("\n\033[36mℹ All packaging and metadata files are already synchronized with version " + target_version + "\033[0m")
PY

print_success "Version sync verification completed!"
