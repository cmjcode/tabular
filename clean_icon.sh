#!/bin/bash
# ==============================================================================
# Tabular App Icon Transparency Cleaner
# ==============================================================================
# Removes the alpha channel from icon images and fills transparent backgrounds
# with a solid dark color (e.g., #111318 or #1a1a1a) to comply with Apple App
# Store requirements (which reject any AppIcon with alpha channels).
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Detect if running from workspace root or tabular-client
if [ -d "$SCRIPT_DIR/tabular-client" ]; then
    CLIENT_DIR="$SCRIPT_DIR/tabular-client"
else
    CLIENT_DIR="$SCRIPT_DIR"
fi

BG_COLOR="${1:-#111318}"

echo "=================================================="
echo "🎨 [Tabular Icon Cleaner] Processing App Icons"
echo "  Background Color: $BG_COLOR"
echo "  Client Dir      : $CLIENT_DIR"
echo "=================================================="

# Check for ImageMagick
if ! command -v magick &>/dev/null; then
    echo "❌ Error: 'magick' (ImageMagick) is required. Install via 'brew install imagemagick'." >&2
    exit 1
fi

# 1. Regenerate Xcode Asset Catalog and AppIcon.icns from clean transparent assets
echo "🔄 Regenerating Xcode Assets & AppIcon..."
(cd "$CLIENT_DIR" && make xcode-assets)

# 2. Keep assets/icon.png and assets/logo-512.png in sync with alpha
cp "$CLIENT_DIR/assets/logo.png" "$CLIENT_DIR/assets/icon.png"
sips -z 512 512 "$CLIENT_DIR/assets/logo.png" --out "$CLIENT_DIR/assets/logo-512.png" &>/dev/null

# 3. For iOS App Store compliance (iOS App Store rejects alpha in 1024x1024 marketing icon),
# only strip alpha from the iOS-specific icon-1024.png if explicitly requested,
# while preserving alpha for all macOS icons and master assets.
if [ "$1" = "--ios-store" ] || [ "$2" = "--ios-store" ]; then
    echo "🧹 Removing alpha channel from iOS icon-1024.png for App Store submission..."
    IOS_ICON="$CLIENT_DIR/apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png"
    if [ -f "$IOS_ICON" ]; then
        magick "$IOS_ICON" -background "$BG_COLOR" -alpha remove -alpha off "$IOS_ICON"
    fi
fi

# 5. Verify AppIcon properties
echo "🔍 Verifying AppIcon properties:"
sips -g all "$CLIENT_DIR/apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png" | grep -E "hasAlpha|samplesPerPixel|pixelWidth|pixelHeight"

echo ""
echo "✅ [SUCCESS] App Icon cleaned and assets catalog generated successfully!"
