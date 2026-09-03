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

# 1. Clean logo.png
if [ -f "$CLIENT_DIR/assets/logo.png" ]; then
    echo "🧹 Removing alpha channel from $CLIENT_DIR/assets/logo.png..."
    magick "$CLIENT_DIR/assets/logo.png" -background "$BG_COLOR" -alpha remove -alpha off "$CLIENT_DIR/assets/logo.png"
fi

# 2. Also keep assets/icon.png in sync
if [ -f "$CLIENT_DIR/assets/logo.png" ]; then
    cp "$CLIENT_DIR/assets/logo.png" "$CLIENT_DIR/assets/icon.png"
fi

# 3. Clean logo-512.png if present
if [ -f "$CLIENT_DIR/assets/logo-512.png" ]; then
    echo "🧹 Removing alpha channel from $CLIENT_DIR/assets/logo-512.png..."
    magick "$CLIENT_DIR/assets/logo-512.png" -background "$BG_COLOR" -alpha remove -alpha off "$CLIENT_DIR/assets/logo-512.png"
fi

# 4. Regenerate Xcode Asset Catalog
echo "🔄 Regenerating Xcode Assets..."
(cd "$CLIENT_DIR" && make xcode-assets)

# 5. Verify AppIcon properties
echo "🔍 Verifying AppIcon properties:"
sips -g all "$CLIENT_DIR/apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png" | grep -E "hasAlpha|samplesPerPixel|pixelWidth|pixelHeight"

echo ""
echo "✅ [SUCCESS] App Icon cleaned and assets catalog generated successfully!"
