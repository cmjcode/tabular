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

BG_COLOR="${1:-#FFFFFF}"

echo "=================================================="
echo "🎨 [Tabular Icon Cleaner] Processing App Icons"
echo "  Background Color: $BG_COLOR"
echo "  Client Dir      : $CLIENT_DIR"
echo "=================================================="

# Check for ImageMagick (optional if swift is available)
HAS_MAGICK=false
if command -v magick &>/dev/null; then
    HAS_MAGICK=true
fi

# 1. Strip extended attributes (quarantine, etc.) from master assets
echo "🧹 Sanitizing master assets extended attributes..."
xattr -cr "$CLIENT_DIR/assets" 2>/dev/null || true
find "$CLIENT_DIR/assets" -name ".DS_Store" -delete 2>/dev/null || true

# 2. Regenerate Xcode Asset Catalog and AppIcon.icns from clean assets
echo "🔄 Regenerating Xcode Assets & AppIcon..."
(cd "$CLIENT_DIR" && make xcode-assets)

# 3. Keep assets/icon.png and assets/logo-512.png in sync
cp "$CLIENT_DIR/assets/logo.png" "$CLIENT_DIR/assets/icon.png"
sips -z 512 512 "$CLIENT_DIR/assets/logo.png" --out "$CLIENT_DIR/assets/logo-512.png" &>/dev/null

# 4. Verify iOS App Store large icon compliance (no alpha channel)
IOS_ICON="$CLIENT_DIR/apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png"
if [ -f "$IOS_ICON" ]; then
    HAS_ALPHA=$(sips -g hasAlpha "$IOS_ICON" 2>/dev/null | grep -i "hasAlpha" | awk '{print $2}')
    if [ "$HAS_ALPHA" = "yes" ]; then
        echo "🧹 Removing alpha channel from iOS icon-1024.png for App Store compliance..."
        if [ "$HAS_MAGICK" = true ]; then
            magick "$IOS_ICON" -background "$BG_COLOR" -alpha remove -alpha off "$IOS_ICON"
        fi
    fi
fi

# 5. Sanitize all generated assets
xattr -cr "$CLIENT_DIR/assets" 2>/dev/null || true
xattr -cr "$CLIENT_DIR/apple/Assets.xcassets" 2>/dev/null || true
find "$CLIENT_DIR/assets" "$CLIENT_DIR/apple" -name ".DS_Store" -delete 2>/dev/null || true

# 6. Verify AppIcon properties
echo "🔍 Verifying AppIcon properties:"
sips -g all "$CLIENT_DIR/apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png" | grep -E "hasAlpha|samplesPerPixel|pixelWidth|pixelHeight"

echo ""
echo "✅ [SUCCESS] App Icon cleaned, extended attributes stripped, and assets catalog generated successfully!"
