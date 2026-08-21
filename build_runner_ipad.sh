#!/bin/bash
# =============================================================================
# Build & Publish script for Tabular iPad / iPadOS
# Usage: ./build_ipad.sh [COMMAND] [OPTIONS]
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

APP_NAME="Tabular"
SCHEME_NAME="TabulariOS"
PROJECT_PATH="ios/Xcode/TabulariOS/TabulariOS.xcodeproj"
DIST_DIR="dist/ios"
ARCHIVE_PATH="$DIST_DIR/${APP_NAME}iOS.xcarchive"
IPA_PATH="$DIST_DIR/${APP_NAME}iOS.ipa"

# Colors for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

print_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
print_warning() { echo -e "${YELLOW}[WARNING]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

show_help() {
    echo -e "${CYAN}📱 Tabular iPad (iPadOS) Build & Publish Tool${NC}"
    echo "================================================"
    echo ""
    echo "Usage: $0 [COMMAND] [OPTIONS]"
    echo ""
    echo "Commands:"
    echo "  xcframework  - Build Rust universal XCFramework (device + simulator)"
    echo "  build        - Build XCFramework & compile Xcode iOS app"
    echo "  sim          - Build & compile for iPad Simulator"
    echo "  archive      - Create ${APP_NAME}iOS.xcarchive"
    echo "  export       - Export signed .ipa from archive (App Store / Ad-Hoc)"
    echo "  publish      - Upload .ipa to App Store Connect / TestFlight"
    echo "  all          - Run xcframework -> archive -> export"
    echo ""
    echo "Options:"
    echo "  --team-id TEAM_ID        Apple Developer Team ID"
    echo "  --apple-id EMAIL         Apple ID email for App Store submission"
    echo "  --dev                    Export with Development profile instead of App Store"
    echo "  --clean                  Clean previous build artifacts before running"
    echo "  --help                   Show this help message"
    echo ""
    echo "Environment Variables (Alternative to flags):"
    echo "  APPLE_TEAM_ID            (e.g., 'YD4J5Z6A4G')"
    echo "  APPLE_ID                 (e.g., 'developer@example.com')"
    echo "  APPLE_PASSWORD           (App-specific password for App Store Connect)"
    echo "  APPLE_BUNDLE_ID          (Default: 'id.tabular.database.ios')"
    echo ""
}

COMMAND="all"
USE_DEV_EXPORT=false
CLEAN_FIRST=false

while [[ $# -gt 0 ]]; do
    case $1 in
        xcframework|build|sim|archive|export|publish|all)
            COMMAND="$1"
            shift
            ;;
        --team-id)
            export APPLE_TEAM_ID="$2"
            shift 2
            ;;
        --apple-id)
            export APPLE_ID="$2"
            shift 2
            ;;
        --dev)
            USE_DEV_EXPORT=true
            shift
            ;;
        --clean)
            CLEAN_FIRST=true
            shift
            ;;
        --help|-h)
            show_help
            exit 0
            ;;
        *)
            print_error "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

check_prerequisites() {
    print_info "Checking prerequisites..."
    if ! command -v cargo &> /dev/null; then
        print_error "Cargo is not installed. Please install Rust."
        exit 1
    fi
    if ! command -v xcodebuild &> /dev/null; then
        print_error "xcodebuild not found. Please install Xcode and Command Line Tools."
        exit 1
    fi
    print_success "Prerequisites available."
}

clean_build() {
    print_info "Cleaning iOS build artifacts..."
    rm -rf "$DIST_DIR"
    rm -rf "ios/Xcode/TabulariOS/Generated"
    rm -rf "ios/Xcode/TabulariOS/build"
    print_success "Clean completed."
}

build_xcframework() {
    print_info "Building Rust XCFramework for iOS/iPadOS..."
    bash "ios/Xcode/scripts/build-rust-xcframework.sh"
    print_success "Rust XCFramework generated successfully."
}

build_sim() {
    build_xcframework
    print_info "Compiling Tabular iPad app for Simulator..."
    xcodebuild \
        -project "$PROJECT_PATH" \
        -scheme "$SCHEME_NAME" \
        -destination "generic/platform=iOS Simulator" \
        -configuration Debug \
        build CODE_SIGNING_ALLOWED=NO
    print_success "iPad Simulator build completed."
}

build_app() {
    build_xcframework
    print_info "Compiling Tabular iPad app for Generic iOS Device..."
    xcodebuild \
        -project "$PROJECT_PATH" \
        -scheme "$SCHEME_NAME" \
        -destination "generic/platform=iOS" \
        -configuration Release \
        build CODE_SIGNING_ALLOWED=NO
    print_success "iPad Device build completed."
}

archive_app() {
    build_xcframework
    mkdir -p "$DIST_DIR"
    print_info "Archiving Tabular iPad app to $ARCHIVE_PATH..."

    ARCHIVE_CMD=(
        xcodebuild archive
        -project "$PROJECT_PATH"
        -scheme "$SCHEME_NAME"
        -destination "generic/platform=iOS"
        -archivePath "$ARCHIVE_PATH"
        -configuration Release
    )

    if [ -n "${APPLE_TEAM_ID:-}" ]; then
        print_info "Using DEVELOPMENT_TEAM: $APPLE_TEAM_ID"
        ARCHIVE_CMD+=(DEVELOPMENT_TEAM="$APPLE_TEAM_ID")
    fi

    "${ARCHIVE_CMD[@]}"
    print_success "Archive created at $ARCHIVE_PATH"
}

export_ipa() {
    if [ ! -d "$ARCHIVE_PATH" ]; then
        print_warning "Archive not found. Running archive first..."
        archive_app
    fi

    EXPORT_PLIST="ios/Xcode/ExportOptions-AppStore.plist"
    if [ "$USE_DEV_EXPORT" = true ]; then
        EXPORT_PLIST="ios/Xcode/ExportOptions-Development.plist"
        print_info "Using Development ExportOptions: $EXPORT_PLIST"
    else
        print_info "Using App Store ExportOptions: $EXPORT_PLIST"
    fi

    print_info "Exporting IPA to $DIST_DIR..."
    xcodebuild -exportArchive \
        -archivePath "$ARCHIVE_PATH" \
        -exportOptionsPlist "$EXPORT_PLIST" \
        -exportPath "$DIST_DIR" \
        -allowProvisioningUpdates

    print_success "IPA exported to $DIST_DIR"
}

publish_app() {
    print_info "Publishing to App Store Connect / TestFlight..."

    # Find the generated IPA
    IPA_FOUND=$(find "$DIST_DIR" -maxdepth 1 -name "*.ipa" | head -n1 || true)
    if [ -z "$IPA_FOUND" ]; then
        print_error "No .ipa file found in $DIST_DIR. Run export first."
        exit 1
    fi

    print_info "Target IPA: $IPA_FOUND"

    if [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_PASSWORD:-}" ]; then
        print_warning "APPLE_ID and/or APPLE_PASSWORD not set."
        echo "To upload automatically, export your credentials:"
        echo "  export APPLE_ID='developer@example.com'"
        echo "  export APPLE_PASSWORD='app-specific-password'"
        echo ""
        echo "Or use Transporter App / Xcode Organizer with archive:"
        echo "  $ARCHIVE_PATH"
        exit 1
    fi

    print_info "Uploading to App Store Connect..."
    xcrun altool --upload-app \
        -f "$IPA_FOUND" \
        -t ios \
        -u "$APPLE_ID" \
        -p "$APPLE_PASSWORD"

    print_success "🎉 Upload to TestFlight / App Store completed successfully!"
}

main() {
    check_prerequisites

    if [ "$CLEAN_FIRST" = true ]; then
        clean_build
    fi

    case $COMMAND in
        xcframework)
            build_xcframework
            ;;
        build)
            build_app
            ;;
        sim)
            build_sim
            ;;
        archive)
            archive_app
            ;;
        export)
            export_ipa
            ;;
        publish)
            publish_app
            ;;
        all)
            archive_app
            export_ipa
            ;;
    esac

    echo ""
    print_success "🎉 All operations completed for iPad target!"
    if [ -d "$DIST_DIR" ]; then
        print_info "📦 Generated files:"
        find "$DIST_DIR" -maxdepth 2 \( -name "*.ipa" -o -name "*.xcarchive" \) | while read -r f; do
            size=$(ls -lh -d "$f" | awk '{print $5}')
            echo "  📁 $f ($size)"
        done
    fi
}

main
