#!/bin/bash
# Quick build script for Tabular
# Usage: ./build.sh [platform]
# Platforms: macos, linux, windows, all

set -e

APP_NAME="Tabular"
# Ambil versi dari Cargo.toml supaya sinkron
VERSION=$(grep '^version' Cargo.toml | head -n1 | cut -d '"' -f2)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if required tools are installed
check_dependencies() {
    print_status "Checking dependencies..."
    
    if ! command -v cargo &> /dev/null; then
        print_error "Cargo is not installed. Please install Rust first."
        exit 1
    fi
    
    if ! command -v make &> /dev/null; then
        print_error "Make is not installed. Please install make."
        exit 1
    fi
    
    print_success "All dependencies are available!"
}

# Show help
show_help() {
    echo "🛠️  Tabular Build Script"
    echo "======================="
    echo ""
    echo "Usage: $0 [PLATFORM] [OPTIONS]"
    echo ""
    echo "Platforms:"
    echo "  macos        - Build macOS universal + .app (DMG optional)"
    echo "  macos-pkg    - Build macOS .app lalu signed .pkg (App Store / distribusi)"
    echo "  linux        - Build + package Linux (x86_64 + aarch64)"
    echo "  windows      - Build + package Windows (x86_64 + aarch64)"
    echo "  all          - Release build semua platform"
    echo ""
    echo "Options:"
    echo "  --deps    - Install build dependencies first"
    echo "  --clean   - Clean before building"
    echo "  --help    - Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 macos          # Build macOS only"
    echo "  $0 linux --clean  # Clean and build Linux"
    echo "  $0 all --deps     # Install deps and build all"
    echo ""
}

# Parse command line arguments
PLATFORM="all"
BUILD_PKG=false
INSTALL_DEPS=false
CLEAN_FIRST=false

while [[ $# -gt 0 ]]; do
    case $1 in
    macos|macos-pkg|linux|windows|all)
            PLATFORM="$1"
            shift
            ;;
        --deps)
            INSTALL_DEPS=true
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

# Main build function
main() {
    print_status "Starting build for platform: $PLATFORM"
    
    check_dependencies
    
    # Install dependencies if requested
    if [ "$INSTALL_DEPS" = true ]; then
        print_status "Installing build dependencies..."
        make install-deps
    fi
    
    # Clean if requested
    if [ "$CLEAN_FIRST" = true ]; then
        print_status "Cleaning previous builds..."
        make clean
    fi
    
    # Build based on platform
    case $PLATFORM in
        macos)
            print_status "Building macOS universal binary and App Store ready bundle..."
            print_status "Setting up environment variables for code signing..."
            
            # Export environment variables for code signing
            export APPLE_ID='nunung.pamungkas@vneu.co.id'
            export APPLE_TEAM_ID='YD4J5Z6A4G'
            export APPLE_BUNDLE_ID='id.tabular.database'
            export PASSWORD='Simbok21AMIDAMA'
            export APPLE_IDENTITY='Developer ID Application: PT. VNEU TEKNOLOGI INDONESIA (YD4J5Z6A4G)'
            export APPLE_IDENTITY_INS='Developer ID Installer: PT. VNEU TEKNOLOGI INDONESIA (YD4J5Z6A4G)'
            export APPLE_APP_IDENTITY='3rd Party Mac Developer Application: PT. VNEU TEKNOLOGI INDONESIA (YD4J5Z6A4G)'
            
            # Note: APPLE_PASSWORD dan NOTARIZE=1 perlu di-set manual jika mau notarize
            echo "✅ Environment variables set for code signing"
            echo "📝 Note: For notarization, manually set APPLE_PASSWORD and NOTARIZE=1"
            
            make clean
            make bundle-macos
            make pkg-macos-store
            sh notarize.sh
            print_success "macOS build completed!"
            ;;
        macos-pkg)

            # Export environment variables for code signing
            export APPLE_ID='nunung.pamungkas@vneu.co.id'
            export APPLE_TEAM_ID='YD4J5Z6A4G'
            export APPLE_BUNDLE_ID='id.tabular.database'
            export PASSWORD='Simbok21AMIDAMA'
            # export APPLE_IDENTITY='Developer ID Application: PT. VNEU TEKNOLOGI INDONESIA (YD4J5Z6A4G)'
            export APPLE_IDENTITY='Developer ID Installer: PT. VNEU TEKNOLOGI INDONESIA (YD4J5Z6A4G)'
            print_status "Building macOS .app + signed .pkg"
            if [ -z "$APPLE_IDENTITY" ]; then
                print_warning "APPLE_IDENTITY belum diset. Contoh: export APPLE_IDENTITY='Apple Distribution: Nama (TEAMID)'"
            fi
            if [ -z "$APPLE_BUNDLE_ID" ]; then
                print_warning "APPLE_BUNDLE_ID belum diset (contoh: id.tabular.data)"
            fi
            make pkg-macos-store || {
                print_error "Gagal membuat pkg. Pastikan env & provisioning profile benar."
                exit 1
            }
            print_success "macOS pkg build completed!"
            ;;
        linux)
            print_status "Building Linux binaries..."
            make bundle-linux
            print_success "Linux build completed!"
            ;;
        windows)
            print_status "Building Windows binaries..."
            make bundle-windows
            print_success "Windows build completed!"
            ;;
        all)
            print_status "Building for all platforms..."
            make release
            print_success "All platform builds completed!"
            ;;
        *)
            print_error "Unknown platform: $PLATFORM"
            exit 1
            ;;
    esac
    
    # Show build results
    echo ""
    print_success "🎉 Build completed successfully!"
    echo ""
    print_status "📦 Generated files:"
    
    if [ -d "dist" ]; then
        find dist -type f \( -name "*.dmg" -o -name "*.pkg" -o -name "*.app" -o -name "*.tar.gz" -o -name "*.zip" \) | while read -r file; do
            size=$(ls -lh "$file" | awk '{print $5}')
            echo "  📁 $file ($size)"
        done
    else
        print_warning "No distribution files found. Build may have failed."
    fi
    
    echo ""
    print_status "✨ Ready for distribution!"
}

# Run main function
main "$@"
