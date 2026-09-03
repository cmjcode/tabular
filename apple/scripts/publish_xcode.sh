#!/bin/bash
# ==============================================================================
# Tabular Xcode Archive & App Store Connect Publisher (CLI Helper)
# ==============================================================================
# Usage:
#   ./apple/scripts/publish_xcode.sh ios [archive|upload]
#   ./apple/scripts/publish_xcode.sh macos [archive|upload]
#   ./apple/scripts/publish_xcode.sh all
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPLE_DIR="$(dirname "$SCRIPT_DIR")"
CLIENT_DIR="$(dirname "$APPLE_DIR")"
ROOT_DIR="$(dirname "$CLIENT_DIR")"

# Load .env credentials if available
if [ -f "$ROOT_DIR/.env" ]; then
    set -a; source "$ROOT_DIR/.env"; set +a
elif [ -f "$CLIENT_DIR/.env" ]; then
    set -a; source "$CLIENT_DIR/.env"; set +a
fi

cd "$CLIENT_DIR"

PROJECT_FILE="Tabular.xcodeproj"
DIST_DIR="$CLIENT_DIR/dist/xcode"
mkdir -p "$DIST_DIR"

PLATFORM="${1:-ios}"
ACTION="${2:-upload}"

show_help() {
    echo "Usage: $0 [ios|macos|all] [archive|upload]"
    echo ""
    echo "Actions:"
    echo "  archive  - Build .xcarchive only (ready for Xcode Organizer)"
    echo "  upload   - Build .xcarchive, export .ipa / .pkg, and upload to App Store Connect"
    echo ""
}

if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
    show_help
    exit 0
fi

archive_ios() {
    echo "📱 [Xcode Build] Archiving Tabular-iOS (iPadOS)..."
    local archive_path="$DIST_DIR/Tabular-iOS.xcarchive"
    rm -rf "$archive_path"
    
    xcodebuild archive \
        -project "$PROJECT_FILE" \
        -scheme "Tabular-iOS" \
        -destination "generic/platform=iOS" \
        -archivePath "$archive_path" \
        -configuration Release
        
    echo "✅ iOS Archive created at: $archive_path"
}

archive_macos() {
    echo "💻 [Xcode Build] Archiving Tabular-macOS..."
    local archive_path="$DIST_DIR/Tabular-macOS.xcarchive"
    rm -rf "$archive_path"
    
    xcodebuild archive \
        -project "$PROJECT_FILE" \
        -scheme "Tabular-macOS" \
        -destination "generic/platform=macOS" \
        -archivePath "$archive_path" \
        -configuration Release
        
    echo "✅ macOS Archive created at: $archive_path"
}

upload_ios() {
    archive_ios
    local archive_path="$DIST_DIR/Tabular-iOS.xcarchive"
    local ipa_dir="$DIST_DIR/ios-export"
    mkdir -p "$ipa_dir"
    
    # Create standard exportOptions for App Store
    cat << EOF > "$DIST_DIR/ExportOptions-iOS.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store</string>
    <key>uploadSymbols</key>
    <true/>
    <key>manageAppVersionAndBuildNumber</key>
    <false/>
</dict>
</plist>
EOF

    echo "📤 Exporting & Uploading iOS IPA via xcodebuild..."
    xcodebuild -exportArchive \
        -archivePath "$archive_path" \
        -exportOptionsPlist "$DIST_DIR/ExportOptions-iOS.plist" \
        -exportPath "$ipa_dir" \
        -allowProvisioningUpdates || {
            echo "⚠️ xcodebuild auto-upload requires Xcode accounts. Archive is ready in Organizer: $archive_path"
            echo "ℹ️  You can open Xcode -> Window -> Organizer -> Distribute App to upload with 1 click."
        }
}

upload_macos() {
    archive_macos
    local archive_path="$DIST_DIR/Tabular-macOS.xcarchive"
    local pkg_dir="$DIST_DIR/macos-export"
    mkdir -p "$pkg_dir"
    
    cat << EOF > "$DIST_DIR/ExportOptions-macOS.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store</string>
    <key>uploadSymbols</key>
    <true/>
</dict>
</plist>
EOF

    echo "📤 Exporting & Uploading macOS PKG via xcodebuild..."
    xcodebuild -exportArchive \
        -archivePath "$archive_path" \
        -exportOptionsPlist "$DIST_DIR/ExportOptions-macOS.plist" \
        -exportPath "$pkg_dir" \
        -allowProvisioningUpdates || {
            echo "⚠️ Archive is ready in Organizer: $archive_path"
            echo "ℹ️  You can open Xcode -> Window -> Organizer -> Distribute App to upload with 1 click."
        }
}

case "$PLATFORM" in
    ios)
        if [ "$ACTION" = "archive" ]; then
            archive_ios
        else
            upload_ios
        fi
        ;;
    macos)
        if [ "$ACTION" = "archive" ]; then
            archive_macos
        else
            upload_macos
        fi
        ;;
    all)
        upload_ios
        upload_macos
        ;;
    *)
        echo "Unknown platform: $PLATFORM"
        show_help
        exit 1
        ;;
esac

echo ""
echo "🎉 Process complete! Archives and exports stored in $DIST_DIR"
