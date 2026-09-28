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

# Prepare opaque iOS master icon (Apple App Store requires opaque large icon with NO alpha channel)
IOS_MASTER_TMP="$(mktemp -d)"
IOS_MASTER_ICON="$IOS_MASTER_TMP/icon-ios-master.png"

echo "[INFO] Creating opaque master icon for iOS (stripping alpha channel and filling background with #FFFFFF)..."
if command -v magick &>/dev/null; then
    magick "$SOURCE_ICON" -resize 1024x1024 -background "#FFFFFF" -alpha remove -alpha off "$IOS_MASTER_ICON"
else
    swift - "$SOURCE_ICON" "$IOS_MASTER_ICON" << 'SWIFT_EOF'
import CoreGraphics
import ImageIO
import Foundation

let src = CommandLine.arguments[1]
let dst = CommandLine.arguments[2]
guard let srcURL = CFURLCreateWithFileSystemPath(nil, src as CFString, .cfurlposixPathStyle, false),
      let isrc = CGImageSourceCreateWithURL(srcURL, nil),
      let img = CGImageSourceCreateImageAtIndex(isrc, 0, nil) else {
    exit(1)
}

let w = 1024, h = 1024
let cs = CGColorSpaceCreateDeviceRGB()
let info = CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.noneSkipLast.rawValue
guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: cs, bitmapInfo: info) else {
    exit(1)
}

ctx.setFillColor(red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0)
ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))

guard let res = ctx.makeImage(),
      let dstURL = CFURLCreateWithFileSystemPath(nil, dst as CFString, .cfurlposixPathStyle, false),
      let dest = CGImageDestinationCreateWithURL(dstURL, "public.png" as CFString, 1, nil) else {
    exit(1)
}

CGImageDestinationAddImage(dest, res, nil)
guard CGImageDestinationFinalize(dest) else { exit(1) }
SWIFT_EOF
fi

# Generate iOS icon sizes (from opaque master icon, guaranteed no alpha channel)
sips -z 1024 1024 "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-1024.png" &>/dev/null
sips -z 167 167   "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-83.5@2x.png" &>/dev/null
sips -z 152 152   "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-76@2x.png" &>/dev/null
sips -z 76 76     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-76@1x.png" &>/dev/null
sips -z 120 120   "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-60@2x.png" &>/dev/null
sips -z 180 180   "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-60@3x.png" &>/dev/null
sips -z 80 80     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-40@2x.png" &>/dev/null
sips -z 120 120   "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-40@3x.png" &>/dev/null
sips -z 40 40     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-40@1x.png" &>/dev/null
sips -z 58 58     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-29@2x.png" &>/dev/null
sips -z 87 87     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-29@3x.png" &>/dev/null
sips -z 29 29     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-29@1x.png" &>/dev/null
sips -z 40 40     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-20@2x.png" &>/dev/null
sips -z 60 60     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-20@3x.png" &>/dev/null
sips -z 20 20     "$IOS_MASTER_ICON" --out "$APPICON_DIR/icon-20@1x.png" &>/dev/null

rm -rf "$IOS_MASTER_TMP"

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

# Generate assets/AppIcon.icns for macOS desktop builds
if command -v iconutil &>/dev/null; then
    ICONSET_TMP="$(mktemp -d)"
    ICONSET_DIR="$ICONSET_TMP/AppIcon.iconset"
    mkdir -p "$ICONSET_DIR"
    cp "$APPICON_DIR/icon-16@1x.png" "$ICONSET_DIR/icon_16x16.png"
    cp "$APPICON_DIR/icon-16@2x.png" "$ICONSET_DIR/icon_16x16@2x.png"
    cp "$APPICON_DIR/icon-32@1x.png" "$ICONSET_DIR/icon_32x32.png"
    cp "$APPICON_DIR/icon-32@2x.png" "$ICONSET_DIR/icon_32x32@2x.png"
    cp "$APPICON_DIR/icon-128@1x.png" "$ICONSET_DIR/icon_128x128.png"
    cp "$APPICON_DIR/icon-128@2x.png" "$ICONSET_DIR/icon_128x128@2x.png"
    cp "$APPICON_DIR/icon-256@1x.png" "$ICONSET_DIR/icon_256x256.png"
    cp "$APPICON_DIR/icon-256@2x.png" "$ICONSET_DIR/icon_256x256@2x.png"
    cp "$APPICON_DIR/icon-512@1x.png" "$ICONSET_DIR/icon_512x512.png"
    cp "$APPICON_DIR/icon-512@2x.png" "$ICONSET_DIR/icon_512x512@2x.png"
    iconutil -c icns "$ICONSET_DIR" -o "$CLIENT_DIR/assets/AppIcon.icns"
    rm -rf "$ICONSET_TMP"
    echo "[SUCCESS] AppIcon.icns generated at $CLIENT_DIR/assets/AppIcon.icns"
fi

# Sanitize extended attributes from generated assets catalog and master assets
xattr -cr "$ASSETS_DIR" 2>/dev/null || true
xattr -cr "$CLIENT_DIR/assets" 2>/dev/null || true
find "$ASSETS_DIR" "$CLIENT_DIR/assets" -name ".DS_Store" -delete 2>/dev/null || true

# Verify iOS App Store large icon compliance (no alpha channel, 1024x1024)
echo "[INFO] Verifying iOS App Store large app icon compliance..."
if command -v sips &>/dev/null; then
    sips -g all "$APPICON_DIR/icon-1024.png" | grep -E "hasAlpha|samplesPerPixel|pixelWidth|pixelHeight"
fi

