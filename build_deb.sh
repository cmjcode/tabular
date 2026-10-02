#!/usr/bin/env bash
set -euo pipefail

# Simple Debian package builder for Tabular.
# Usage: ./build_deb.sh [--target <triple>] [--skip-build]

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$PROJECT_ROOT"

PACKAGE="tabular"
DESKTOP_FILE="$PROJECT_ROOT/tabular.desktop"
ICON_FILE="$PROJECT_ROOT/assets/logo.png"
BUILD_DIR="$PROJECT_ROOT/dist/deb"
TARGET_TRIPLE=""
SKIP_BUILD=false

print_help() {
    cat <<EOF
Usage: $0 [options]

Options:
  --target <triple>   Build for the given Rust target triple.
  --skip-build        Repackage using existing release binary.
  --help              Show this help.
EOF
}

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: required tool '$1' is not installed." >&2
        exit 1
    fi
}

map_to_deb_arch() {
    case "$1" in
        *x86_64*) echo "amd64" ;;
        *aarch64*|*arm64*) echo "arm64" ;;
        *armv7*|*armhf*) echo "armhf" ;;
        *armv6*|*armel*) echo "armel" ;;
        *i686*|*i386*) echo "i386" ;;
        *) return 1 ;;
    esac
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            [[ $# -lt 2 ]] && { echo "Error: --target expects a value." >&2; exit 1; }
            TARGET_TRIPLE="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=true
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

require_tool cargo
require_tool dpkg-deb
require_tool install

VERSION=$(grep '^version' Cargo.toml | head -n1 | cut -d '"' -f2)
if [[ -z "$VERSION" ]]; then
    echo "Error: unable to detect version from Cargo.toml." >&2
    exit 1
fi

if [[ -z "$TARGET_TRIPLE" ]]; then
    if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
        TARGET_TRIPLE="$CARGO_BUILD_TARGET"
    elif [[ -n "${TARGET:-}" ]]; then
        TARGET_TRIPLE="$TARGET"
    else
        TARGET_TRIPLE=$(rustc -Vv | awk '/host/ {print $2}')
    fi
fi

if [[ -z "$TARGET_TRIPLE" ]]; then
    echo "Error: failed to determine build target. Pass --target explicitly." >&2
    exit 1
fi

if [[ "$TARGET_TRIPLE" != *"linux"* ]]; then
    echo "Error: target '$TARGET_TRIPLE' is not a Linux triple. Use --target <*-unknown-linux-gnu>." >&2
    exit 1
fi

if [[ "$SKIP_BUILD" == false ]]; then
    if [[ -n "$TARGET_TRIPLE" ]]; then
        cargo build --release --target "$TARGET_TRIPLE"
    fi
fi

CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-target}
if [[ -n "$TARGET_TRIPLE" ]]; then
    BIN_PATH="$CARGO_TARGET_DIR/$TARGET_TRIPLE/release/tabular"
fi

if [[ ! -f "$BIN_PATH" ]]; then
    echo "Error: binary not found at $BIN_PATH." >&2
    exit 1
fi

if [[ ! -f "$DESKTOP_FILE" ]]; then
    echo "Error: desktop entry not found at $DESKTOP_FILE." >&2
    exit 1
fi

if [[ ! -f "$ICON_FILE" ]]; then
    echo "Error: icon not found at $ICON_FILE." >&2
    exit 1
fi

DEB_ARCH=""
if [[ -n "$TARGET_TRIPLE" ]]; then
    DEB_ARCH=$(map_to_deb_arch "$TARGET_TRIPLE") || true
fi

if [[ -z "$DEB_ARCH" ]]; then
    MACHINE=$(uname -m)
    DEB_ARCH=$(map_to_deb_arch "$MACHINE") || true
fi

if [[ -z "$DEB_ARCH" ]]; then
    echo "Error: unsupported architecture. Provide --target explicitly." >&2
    exit 1
fi

PKG_ROOT="$BUILD_DIR/${PACKAGE}_${VERSION}_${DEB_ARCH}"
rm -rf "$PKG_ROOT"
mkdir -p "$PKG_ROOT/DEBIAN"
mkdir -p "$PKG_ROOT/usr/bin"
mkdir -p "$PKG_ROOT/usr/share/applications"
mkdir -p "$PKG_ROOT/usr/share/icons/hicolor/512x512/apps"
mkdir -p "$PKG_ROOT/usr/share/doc/$PACKAGE"

install -m755 "$BIN_PATH" "$PKG_ROOT/usr/bin/tabular"
install -m644 "$DESKTOP_FILE" "$PKG_ROOT/usr/share/applications/tabular.desktop"
install -m644 "$ICON_FILE" "$PKG_ROOT/usr/share/icons/hicolor/512x512/apps/tabular.png"
install -m644 LICENSE "$PKG_ROOT/usr/share/doc/$PACKAGE/LICENSE"
if [[ -f LICENSE-AGPL ]]; then
    install -m644 LICENSE-AGPL "$PKG_ROOT/usr/share/doc/$PACKAGE/LICENSE-AGPL"
fi

INSTALLED_SIZE=$(du -ks "$PKG_ROOT/usr" | cut -f1)
cat >"$PKG_ROOT/DEBIAN/control" <<EOF
Package: tabular
Version: $VERSION
Section: utils
Priority: optional
Architecture: $DEB_ARCH
Depends: libc6, libssl3, libgtk-3-0, libxcb1, libxkbcommon0, libglib2.0-0
Maintainer: Tabular Team <support@tabular.id>
Homepage: https://tabular.id
Installed-Size: $INSTALLED_SIZE
Description: Tabular database client
 Tabular helps you explore and query SQL and NoSQL databases from a single desktop app.
EOF

cat >"$PKG_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q /usr/share/icons/hicolor || true
fi
exit 0
EOF

cat >"$PKG_ROOT/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q /usr/share/icons/hicolor || true
fi
exit 0
EOF

chmod 755 "$PKG_ROOT/DEBIAN/postinst" "$PKG_ROOT/DEBIAN/postrm"

mkdir -p "$BUILD_DIR"
DEB_FILE="$BUILD_DIR/${PACKAGE}_${VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build "$PKG_ROOT" "$DEB_FILE"

echo "Debian package created at $DEB_FILE"
