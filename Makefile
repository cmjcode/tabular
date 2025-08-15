# Makefile for Tabular - Cross-platform SQL Editor
# Builds universal binaries for macOS, Linux, and Windows

.PHONY: all clean install-deps build-macos build-linux build-windows bundle-macos bundle-linux bundle-windows release help

# Default target
all: help

# Variables
APP_NAME = Tabular
VERSION = $(shell grep '^version' Cargo.toml | head -n1 | cut -d'"' -f2)
RUST_VERSION = stable

# macOS targets
MACOS_X86_TARGET = x86_64-apple-darwin
MACOS_ARM_TARGET = aarch64-apple-darwin
MACOS_UNIVERSAL_TARGET = universal-apple-darwin

# Linux targets
LINUX_X86_TARGET = x86_64-unknown-linux-gnu
LINUX_ARM_TARGET = aarch64-unknown-linux-gnu

# Windows targets
WINDOWS_X86_TARGET = x86_64-pc-windows-gnu
WINDOWS_ARM_TARGET = aarch64-pc-windows-gnu

# Output directories
BUILD_DIR = target
DIST_DIR = dist
MACOS_DIR = $(DIST_DIR)/macos
LINUX_DIR = $(DIST_DIR)/linux
WINDOWS_DIR = $(DIST_DIR)/windows

help:
	@echo "🛠️  Tabular Build System"
	@echo "======================="
	@echo ""
	@echo "Available targets:"
	@echo "  install-deps    - Install required build dependencies"
	@echo "  build-macos     - Build universal macOS binary"
	@echo "  build-linux     - Build Linux binaries (x86_64 + aarch64)"
	@echo "  build-windows   - Build Windows binaries (x86_64 + aarch64)"
	@echo "  bundle-macos    - Create macOS .app bundle"
	@echo "  bundle-linux    - Create Linux AppImage/tarball"
	@echo "  bundle-windows  - Create Windows installer"
	@echo "  release         - Build and bundle all platforms"
	@echo "  clean           - Clean build artifacts"
	@echo ""

# Install build dependencies
install-deps:
	@echo "📦 Installing build dependencies..."
	# Install Rust targets
	rustup target add $(MACOS_X86_TARGET)
	rustup target add $(MACOS_ARM_TARGET)
	rustup target add $(LINUX_X86_TARGET)
	rustup target add $(LINUX_ARM_TARGET)
	rustup target add $(WINDOWS_X86_TARGET)
	rustup target add $(WINDOWS_ARM_TARGET)
	
	# Install cargo-bundle for macOS app creation
	cargo install cargo-bundle
	
	# Install cross for cross-compilation
	cargo install cross
	
	@echo "✅ Dependencies installed successfully!"

# Clean build artifacts
clean:
	@echo "🧹 Cleaning build artifacts..."
	cargo clean
	rm -rf $(DIST_DIR)
	@echo "✅ Clean completed!"

# Create distribution directories
create-dirs:
	@mkdir -p $(MACOS_DIR)
	@mkdir -p $(LINUX_DIR)
	@mkdir -p $(WINDOWS_DIR)

# Build macOS universal binary
build-macos: create-dirs
	@echo "🍎 Building macOS universal binary..."
	
	# Build for x86_64
	@echo "Building for x86_64-apple-darwin..."
	cargo build --release --target $(MACOS_X86_TARGET)
	
	# Build for aarch64 (Apple Silicon)
	@echo "Building for aarch64-apple-darwin..."
	cargo build --release --target $(MACOS_ARM_TARGET)
	
	# Create universal binary using lipo
	@echo "Creating universal binary..."
	@mkdir -p $(BUILD_DIR)/universal-apple-darwin/release

	# gabungkan kedua arsitektur menjadi satu binary universal 	
	lipo -create \
		$(BUILD_DIR)/$(MACOS_X86_TARGET)/release/tabular \
		$(BUILD_DIR)/$(MACOS_ARM_TARGET)/release/tabular \
		-output $(BUILD_DIR)/universal-apple-darwin/release/tabular
	
	# Copy to dist directory
	cp $(BUILD_DIR)/universal-apple-darwin/release/tabular $(MACOS_DIR)/
	
	@echo "✅ macOS universal binary built successfully!"

# Build Linux binaries
build-linux: create-dirs
	@echo "🐧 Building Linux binaries..."
	
	# Build for x86_64
	@echo "Building for x86_64-unknown-linux-gnu..."
	cargo build --release --target $(LINUX_X86_TARGET)
	cp $(BUILD_DIR)/$(LINUX_X86_TARGET)/release/tabular $(LINUX_DIR)/tabular-x86_64
	
	@echo "✅ Linux binaries built successfully!"

# Build Windows binaries
build-windows: create-dirs
	@echo "🪟 Building Windows binaries..."
	
	# Build for x86_64
	@echo "Building for x86_64-pc-windows-gnu..."
	cargo build --release --target $(WINDOWS_X86_TARGET)
	cp $(BUILD_DIR)/$(WINDOWS_X86_TARGET)/release/tabular.exe $(WINDOWS_DIR)/tabular-x86_64.exe
	

# Bundle macOS app
bundle-macos: build-macos
	@echo "📱 Creating macOS .app bundle..."
	
	# Use cargo-bundle to create .app
	cargo bundle --release --target $(MACOS_ARM_TARGET)
	
	# Copy the bundle to dist directory
	cp -r $(BUILD_DIR)/$(MACOS_ARM_TARGET)/release/bundle/osx/$(APP_NAME).app $(MACOS_DIR)/
	
	# Replace the binary with universal binary
	cp $(BUILD_DIR)/universal-apple-darwin/release/tabular $(MACOS_DIR)/$(APP_NAME).app/Contents/MacOS/tabular
	
	# Create DMG (if hdiutil is available)
	@if command -v hdiutil >/dev/null 2>&1; then \
		echo "Creating DMG..."; \
		hdiutil create -volname "$(APP_NAME)" -srcfolder $(MACOS_DIR)/$(APP_NAME).app -ov -format UDZO $(MACOS_DIR)/$(APP_NAME)-$(VERSION).dmg; \
	else \
		echo "hdiutil not found, skipping DMG creation"; \
	fi
	
	@echo "✅ macOS app bundle created successfully!"

# Bundle Linux packages
bundle-linux: build-linux
	@echo "📦 Creating Linux packages..."
	
	# Create tarball for x86_64
	@echo "Creating x86_64 tarball..."
	cd $(LINUX_DIR) && tar -czf tabular-$(VERSION)-linux-x86_64.tar.gz tabular-x86_64
	
	# Create tarball for aarch64
	@echo "Creating aarch64 tarball..."
	cd $(LINUX_DIR) && tar -czf tabular-$(VERSION)-linux-aarch64.tar.gz tabular-aarch64
	
	# Create AppDir structure for AppImage (x86_64)
	@mkdir -p $(LINUX_DIR)/AppDir/usr/bin
	@mkdir -p $(LINUX_DIR)/AppDir/usr/share/applications
	@mkdir -p $(LINUX_DIR)/AppDir/usr/share/icons/hicolor/512x512/apps
	
	# Copy binary
	cp $(LINUX_DIR)/tabular-x86_64 $(LINUX_DIR)/AppDir/usr/bin/tabular
	
	# Create desktop file
	@echo "[Desktop Entry]" > $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Type=Application" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Name=$(APP_NAME)" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Comment=Your SQL Editor, Forged with Rust: Fast, Safe, Efficient." >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Exec=tabular" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Icon=tabular" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Terminal=false" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Categories=Development;Database;" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	
	# Copy icon if exists
	@if [ -f assets/logo.png ]; then \
		cp assets/logo.png $(LINUX_DIR)/AppDir/usr/share/icons/hicolor/512x512/apps/tabular.png; \
	fi
	
	@echo "✅ Linux packages created successfully!"

# Bundle Windows installer
bundle-windows: build-windows
	@echo "🪟 Creating Windows packages..."
	
	# Create zip files
	@echo "Creating x86_64 zip..."
	cd $(WINDOWS_DIR) && zip -r tabular-$(VERSION)-windows-x86_64.zip tabular-x86_64.exe
	
	@echo "Creating aarch64 zip..."
	cd $(WINDOWS_DIR) && zip -r tabular-$(VERSION)-windows-aarch64.zip tabular-aarch64.exe
	
	@echo "✅ Windows packages created successfully!"

# Build and bundle everything
release: clean install-deps bundle-macos bundle-linux bundle-windows
	@echo ""
	@echo "🎉 Release build completed successfully!"
	@echo ""
	@echo "📦 Generated packages:"
	@echo "macOS:"
	@find $(MACOS_DIR) -name "*.dmg" -o -name "*.app" | sed 's/^/  /'
	@echo ""
	@echo "Linux:"
	@find $(LINUX_DIR) -name "*.tar.gz" | sed 's/^/  /'
	@echo ""
	@echo "Windows:"
	@find $(WINDOWS_DIR) -name "*.zip" | sed 's/^/  /'
	@echo ""
	@echo "✨ All packages are ready for distribution!"

# Quick build for current platform only
build:
	@echo "🚀 Building for current platform..."
	cargo build --release
	@echo "✅ Build completed!"

# Run the application
run:
	@echo "🚀 Running Tabular..."
	cargo run

# Development build with debug symbols
dev:
	@echo "🔧 Building development version..."
	cargo build
	@echo "✅ Development build completed!"

# Test the application
test:
	@echo "🧪 Running tests..."
	cargo test
	@echo "✅ Tests completed!"

# Check code formatting and linting
check:
	@echo "🔍 Checking code..."
	cargo fmt --check
	cargo clippy -- -D warnings
	@echo "✅ Code check completed!"

# Format code
fmt:
	@echo "✨ Formatting code..."
	cargo fmt
	@echo "✅ Code formatted!"

# Show project information
info:
	@echo "📋 Project Information"
	@echo "===================="
	@echo "Name: $(APP_NAME)"
	@echo "Version: $(VERSION) (from Cargo.toml)"
	@echo "Rust Version: $(RUST_VERSION)"
	@echo ""
	@echo "🎯 Build Targets:"
	@echo "macOS: $(MACOS_X86_TARGET), $(MACOS_ARM_TARGET)"
	@echo "Linux: $(LINUX_X86_TARGET), $(LINUX_ARM_TARGET)"
	@echo "Windows: $(WINDOWS_X86_TARGET), $(WINDOWS_ARM_TARGET)"
	@echo ""
	@echo "📁 Output Directories:"
	@echo "Build: $(BUILD_DIR)"
	@echo "Distribution: $(DIST_DIR)"
