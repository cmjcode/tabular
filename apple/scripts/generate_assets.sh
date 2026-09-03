#!/bin/bash
# ==============================================================================
# Tabular Asset Catalog Generator (Icons & Colors for iOS & macOS)
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPLE_DIR="$(dirname "$SCRIPT_DIR")"
CLIENT_DIR="$(dirname "$APPLE_DIR")"

SOURCE_ICON="$CLIENT_DIR/assets/logo.png"
if [ ! -f "$SOURCE_ICON" ]; then
    SOURCE_ICON="$CLIENT_DIR/assets/icon.png"
fi

if [ ! -f "$SOURCE_ICON" ]; then
    echo "[ERROR] Source icon not found at assets/logo.png or assets/icon.png" >&2
    exit 1
fi

ASSETS_DIR="$APPLE_DIR/Assets.xcassets"
APPICON_DIR="$ASSETS_DIR/AppIcon.appiconset"
ACCENT_DIR="$ASSETS_DIR/AccentColor.colorset"

mkdir -p "$APPICON_DIR"
mkdir -p "$ACCENT_DIR"

echo "[INFO] Generating AppIcon images from $SOURCE_ICON..."

# Generate iOS & macOS icon sizes
sips -z 1024 1024 "$SOURCE_ICON" --out "$APPICON_DIR/icon-1024.png" &>/dev/null
sips -z 167 167   "$SOURCE_ICON" --out "$APPICON_DIR/icon-83.5@2x.png" &>/dev/null
sips -z 152 152   "$SOURCE_ICON" --out "$APPICON_DIR/icon-76@2x.png" &>/dev/null
sips -z 76 76     "$SOURCE_ICON" --out "$APPICON_DIR/icon-76@1x.png" &>/dev/null
sips -z 120 120   "$SOURCE_ICON" --out "$APPICON_DIR/icon-60@2x.png" &>/dev/null
sips -z 180 180   "$SOURCE_ICON" --out "$APPICON_DIR/icon-60@3x.png" &>/dev/null
sips -z 80 80     "$SOURCE_ICON" --out "$APPICON_DIR/icon-40@2x.png" &>/dev/null
sips -z 120 120   "$SOURCE_ICON" --out "$APPICON_DIR/icon-40@3x.png" &>/dev/null
sips -z 40 40     "$SOURCE_ICON" --out "$APPICON_DIR/icon-40@1x.png" &>/dev/null
sips -z 58 58     "$SOURCE_ICON" --out "$APPICON_DIR/icon-29@2x.png" &>/dev/null
sips -z 87 87     "$SOURCE_ICON" --out "$APPICON_DIR/icon-29@3x.png" &>/dev/null
sips -z 29 29     "$SOURCE_ICON" --out "$APPICON_DIR/icon-29@1x.png" &>/dev/null
sips -z 40 40     "$SOURCE_ICON" --out "$APPICON_DIR/icon-20@2x.png" &>/dev/null
sips -z 60 60     "$SOURCE_ICON" --out "$APPICON_DIR/icon-20@3x.png" &>/dev/null
sips -z 20 20     "$SOURCE_ICON" --out "$APPICON_DIR/icon-20@1x.png" &>/dev/null

# macOS specific sizes
sips -z 16 16     "$SOURCE_ICON" --out "$APPICON_DIR/icon-16@1x.png" &>/dev/null
sips -z 32 32     "$SOURCE_ICON" --out "$APPICON_DIR/icon-16@2x.png" &>/dev/null
sips -z 32 32     "$SOURCE_ICON" --out "$APPICON_DIR/icon-32@1x.png" &>/dev/null
sips -z 64 64     "$SOURCE_ICON" --out "$APPICON_DIR/icon-32@2x.png" &>/dev/null
sips -z 128 128   "$SOURCE_ICON" --out "$APPICON_DIR/icon-128@1x.png" &>/dev/null
sips -z 256 256   "$SOURCE_ICON" --out "$APPICON_DIR/icon-128@2x.png" &>/dev/null
sips -z 256 256   "$SOURCE_ICON" --out "$APPICON_DIR/icon-256@1x.png" &>/dev/null
sips -z 512 512   "$SOURCE_ICON" --out "$APPICON_DIR/icon-256@2x.png" &>/dev/null
sips -z 512 512   "$SOURCE_ICON" --out "$APPICON_DIR/icon-512@1x.png" &>/dev/null
sips -z 1024 1024 "$SOURCE_ICON" --out "$APPICON_DIR/icon-512@2x.png" &>/dev/null

# Assets.xcassets root Contents.json
cat << 'EOF' > "$ASSETS_DIR/Contents.json"
{
  "info" : {
    "author" : "xcode",
    "version" : 1
  }
}
EOF

# AppIcon Contents.json (Universal iOS & macOS)
cat << 'EOF' > "$APPICON_DIR/Contents.json"
{
  "images" : [
    {
      "idiom" : "universal",
      "platform" : "ios",
      "size" : "1024x1024",
      "filename" : "icon-1024.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "2x",
      "size" : "83.5x83.5",
      "filename" : "icon-83.5@2x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "2x",
      "size" : "76x76",
      "filename" : "icon-76@2x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "1x",
      "size" : "76x76",
      "filename" : "icon-76@1x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "2x",
      "size" : "40x40",
      "filename" : "icon-40@2x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "1x",
      "size" : "40x40",
      "filename" : "icon-40@1x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "2x",
      "size" : "29x29",
      "filename" : "icon-29@2x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "1x",
      "size" : "29x29",
      "filename" : "icon-29@1x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "2x",
      "size" : "20x20",
      "filename" : "icon-20@2x.png"
    },
    {
      "idiom" : "ipad",
      "scale" : "1x",
      "size" : "20x20",
      "filename" : "icon-20@1x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "2x",
      "size" : "60x60",
      "filename" : "icon-60@2x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "3x",
      "size" : "60x60",
      "filename" : "icon-60@3x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "2x",
      "size" : "40x40",
      "filename" : "icon-40@2x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "3x",
      "size" : "40x40",
      "filename" : "icon-40@3x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "2x",
      "size" : "29x29",
      "filename" : "icon-29@2x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "3x",
      "size" : "29x29",
      "filename" : "icon-29@3x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "2x",
      "size" : "20x20",
      "filename" : "icon-20@2x.png"
    },
    {
      "idiom" : "iphone",
      "scale" : "3x",
      "size" : "20x20",
      "filename" : "icon-20@3x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "1x",
      "size" : "16x16",
      "filename" : "icon-16@1x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "2x",
      "size" : "16x16",
      "filename" : "icon-16@2x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "1x",
      "size" : "32x32",
      "filename" : "icon-32@1x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "2x",
      "size" : "32x32",
      "filename" : "icon-32@2x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "1x",
      "size" : "128x128",
      "filename" : "icon-128@1x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "2x",
      "size" : "128x128",
      "filename" : "icon-128@2x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "1x",
      "size" : "256x256",
      "filename" : "icon-256@1x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "2x",
      "size" : "256x256",
      "filename" : "icon-256@2x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "1x",
      "size" : "512x512",
      "filename" : "icon-512@1x.png"
    },
    {
      "idiom" : "mac",
      "scale" : "2x",
      "size" : "512x512",
      "filename" : "icon-512@2x.png"
    }
  ],
  "info" : {
    "author" : "xcode",
    "version" : 1
  }
}
EOF

# AccentColor Contents.json
cat << 'EOF' > "$ACCENT_DIR/Contents.json"
{
  "colors" : [
    {
      "color" : {
        "color-space" : "srgb",
        "components" : {
          "alpha" : "1.000",
          "blue" : "0.950",
          "green" : "0.550",
          "red" : "0.100"
        }
      },
      "idiom" : "universal"
    },
    {
      "appearances" : [
        {
          "appearance" : "luminosity",
          "value" : "dark"
        }
      ],
      "color" : {
        "color-space" : "srgb",
        "components" : {
          "alpha" : "1.000",
          "blue" : "1.000",
          "green" : "0.650",
          "red" : "0.200"
        }
      },
      "idiom" : "universal"
    }
  ],
  "info" : {
    "author" : "xcode",
    "version" : 1
  }
}
EOF

echo "[SUCCESS] Asset catalog generated at $ASSETS_DIR"
