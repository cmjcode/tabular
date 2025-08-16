#!/bin/bash
# Simple notarization script without jq dependency
# Usage: ./notarize-simple.sh

set -e

APP_NAME="Tabular"
VERSION=$(grep '^version' Cargo.toml | head -n1 | cut -d'"' -f2)
DIST_DIR="dist/macos"

# ⚠️ IMPORTANT: Replace with your actual app-specific password
# Get it from: https://appleid.apple.com → Sign-In and Security → App-Specific Passwords
APPLE_ID='nunung.pamungkas@vneu.co.id'
APPLE_TEAM_ID='YD4J5Z6A4G'
APPLE_PASSWORD='wcnh-nvcd-nxia-ghfg'  # Format: abcd-efgh-ijkl-mnop

echo "🔐 Simple Notarization for $APP_NAME v$VERSION"
echo "=============================================="
echo "📧 Apple ID: $APPLE_ID"
echo "🏢 Team ID: $APPLE_TEAM_ID"
echo "🔑 Using app-specific password (required for 2FA)"

# Check environment
if [ "$APPLE_PASSWORD" = "REPLACE-WITH-APP-SPECIFIC-PASSWORD" ]; then
    echo "❌ Please replace APPLE_PASSWORD with your actual app-specific password!"
    echo ""
    echo "📝 Steps to get app-specific password:"
    echo "1. Go to: https://appleid.apple.com"
    echo "2. Sign in with: $APPLE_ID"
    echo "3. Go to 'Sign-In and Security' → 'App-Specific Passwords'"
    echo "4. Click '+ Generate an app-specific password'"
    echo "5. Label: 'Tabular Notarization'"
    echo "6. Copy the generated password (format: abcd-efgh-ijkl-mnop)"
    echo "7. Replace APPLE_PASSWORD in this script"
    echo ""
    echo "⚠️ NOTE: This is NOT your regular Apple ID password!"
    echo "   For 2FA accounts, you MUST use app-specific passwords."
    exit 1
fi

if [ -z "$APPLE_ID" ] || [ -z "$APPLE_TEAM_ID" ] || [ -z "$APPLE_PASSWORD" ]; then
    echo "❌ Missing environment variables."
    exit 1
fi

echo "✅ Environment variables set"


# Notarize DMG
if [ -f "$DIST_DIR/$APP_NAME-$VERSION.dmg" ]; then
    echo "💿 Notarizing DMG..."
    
    xcrun notarytool submit "$DIST_DIR/$APP_NAME-$VERSION.dmg" \
        --apple-id "$APPLE_ID" \
        --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_PASSWORD" \
        --wait
    
    if [ $? -eq 0 ]; then
        echo "✅ DMG notarized successfully"
        echo ""
        echo "🎉 Notarization completed!"
        echo "📦 Test with:"
        spctl -a -t open --context context:primary-signature -v "$DIST_DIR/$APP_NAME-$VERSION.dmg"
        xcrun stapler staple "$DIST_DIR/$APP_NAME-$VERSION.dmg"
        echo "✅ DMG stapled"
    else
        echo "❌ DMG notarization failed"
    fi
fi




# Notarize PKG
if [ -f "$DIST_DIR/$APP_NAME-$VERSION.pkg" ]; then
    echo "💿 Notarizing PKG..."

    xcrun notarytool submit "$DIST_DIR/$APP_NAME-$VERSION.pkg" \
        --apple-id "$APPLE_ID" \
        --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_PASSWORD" \
        --wait
    
    if [ $? -eq 0 ]; then
        echo "✅ PKG notarized successfully"
        xcrun stapler staple "$DIST_DIR/$APP_NAME-$VERSION.pkg"
        echo "✅ PKG stapled"
    else
        echo "❌ PKG notarization failed"
    fi
fi

