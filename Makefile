############################################
# Tabular Makefile
# Cross-platform build (macOS / Linux / Windows)
# Includes macOS codesign, notarization, App Store pkg
############################################

.PHONY: all help install-deps clean create-dirs \
        build-macos build-linux build-windows \
        bundle-macos bundle-linux bundle-windows pkg-macos-store \
        release build run dev test check fmt info

all: help

APP_NAME = Tabular
VERSION  = $(shell grep '^version' Cargo.toml | head -n1 | cut -d'"' -f2)
RUST_VERSION = stable

# Targets
MACOS_X86_TARGET = x86_64-apple-darwin
MACOS_ARM_TARGET = aarch64-apple-darwin
MACOS_UNIVERSAL_TARGET = universal-apple-darwin
LINUX_X86_TARGET = x86_64-unknown-linux-gnu
LINUX_ARM_TARGET = aarch64-unknown-linux-gnu
WINDOWS_X86_TARGET = x86_64-pc-windows-gnu
WINDOWS_ARM_TARGET = aarch64-pc-windows-gnu

# Dirs
BUILD_DIR = target
DIST_DIR  = dist
MACOS_DIR = $(DIST_DIR)/macos
LINUX_DIR = $(DIST_DIR)/linux
WINDOWS_DIR = $(DIST_DIR)/windows

help:
	@echo "🛠️  Tabular Build System"
	@echo "================================"
	@echo "Version: $(VERSION)"
	@echo ""
	@echo "Core Targets:"
	@echo "  install-deps       Install all Rust targets + tools"
	@echo "  build-macos        Build macOS universal binary"
	@echo "  build-linux        Build Linux (x86_64 + aarch64)"
	@echo "  build-windows      Build Windows (x86_64 + aarch64)"
	@echo "  bundle-macos       Create .app (codesign/notarize optional)"
	@echo "  pkg-macos-store    Create signed .pkg (App Store / distribution)"
	@echo "  bundle-linux       Create tarballs + basic AppDir"
	@echo "  bundle-windows     Create zipped binaries"
	@echo "  release            Clean + deps + all bundles"
	@echo "Dev Helpers:"
	@echo "  run / dev / test / check / fmt / info"
	@echo "Environment (macOS signing/notarization):"
	@echo "  APPLE_IDENTITY='Developer ID Application: Name (TEAMID)'"
	@echo "  APPLE_BUNDLE_ID='id.tabular.data'"
	@echo "  NOTARIZE=1 APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID"
	@echo "  PROVISIONING_PROFILE=path/to/AppStore.provisionprofile (for pkg)"
	@echo ""

install-deps:
	@echo "📦 Installing build dependencies..."
	rustup target add $(MACOS_X86_TARGET) $(MACOS_ARM_TARGET) \
		$(LINUX_X86_TARGET) $(LINUX_ARM_TARGET) \
		$(WINDOWS_X86_TARGET) $(WINDOWS_ARM_TARGET)
	cargo install cargo-bundle || true
	cargo install cross || true
	@echo "✅ Dependencies installed."

clean:
	@echo "🧹 Cleaning..."
	cargo clean
	rm -rf $(DIST_DIR)
	@echo "✅ Clean done."

create-dirs:
	@mkdir -p $(MACOS_DIR) $(LINUX_DIR) $(WINDOWS_DIR)

build-macos: create-dirs
	@echo "🍎 Build macOS universal binary"
	cargo build --release --target $(MACOS_X86_TARGET)
	cargo build --release --target $(MACOS_ARM_TARGET)
	@mkdir -p $(BUILD_DIR)/$(MACOS_UNIVERSAL_TARGET)/release
	lipo -create \
		$(BUILD_DIR)/$(MACOS_X86_TARGET)/release/tabular \
		$(BUILD_DIR)/$(MACOS_ARM_TARGET)/release/tabular \
		-output $(BUILD_DIR)/$(MACOS_UNIVERSAL_TARGET)/release/tabular
	cp $(BUILD_DIR)/$(MACOS_UNIVERSAL_TARGET)/release/tabular $(MACOS_DIR)/
	@echo "✅ macOS universal binary ready."

build-linux: create-dirs
	@echo "🐧 Build Linux (x86_64 + aarch64)"
	cross build --release --target $(LINUX_X86_TARGET)
	cross build --release --target $(LINUX_ARM_TARGET)
	cp $(BUILD_DIR)/$(LINUX_X86_TARGET)/release/tabular $(LINUX_DIR)/tabular-x86_64
	cp $(BUILD_DIR)/$(LINUX_ARM_TARGET)/release/tabular $(LINUX_DIR)/tabular-aarch64
	@echo "✅ Linux builds ready."

build-windows: create-dirs
	@echo "🪟 Build Windows (x86_64 + aarch64)"
	cargo build --release --target $(WINDOWS_X86_TARGET)
	cargo build --release --target $(WINDOWS_ARM_TARGET)
	cp $(BUILD_DIR)/$(WINDOWS_X86_TARGET)/release/tabular.exe $(WINDOWS_DIR)/tabular-x86_64.exe
	cp $(BUILD_DIR)/$(WINDOWS_ARM_TARGET)/release/tabular.exe $(WINDOWS_DIR)/tabular-aarch64.exe
	@echo "✅ Windows builds ready."

bundle-macos: build-macos
	@echo "📱 Create macOS .app bundle"
	cargo bundle --release --target $(MACOS_ARM_TARGET)
	cp -R $(BUILD_DIR)/$(MACOS_ARM_TARGET)/release/bundle/osx/$(APP_NAME).app $(MACOS_DIR)/
	cp $(BUILD_DIR)/$(MACOS_UNIVERSAL_TARGET)/release/tabular $(MACOS_DIR)/$(APP_NAME).app/Contents/MacOS/tabular
	@if [ -n "$$APPLE_IDENTITY" ]; then \
		echo "🔏 Codesign (binary + app)..."; \
		codesign --force --timestamp --options runtime --entitlements macos/Tabular.entitlements -s "$$APPLE_IDENTITY" $(MACOS_DIR)/$(APP_NAME).app/Contents/MacOS/tabular; \
		codesign --force --timestamp --options runtime --entitlements macos/Tabular.entitlements -s "$$APPLE_IDENTITY" -v $(MACOS_DIR)/$(APP_NAME).app; \
	else \
		echo "⚠️  Skipping codesign (set APPLE_IDENTITY)."; \
	fi
	@if [ -n "$$APPLE_IDENTITY" ]; then codesign --verify --deep --strict --verbose=2 $(MACOS_DIR)/$(APP_NAME).app || true; fi
	@if command -v hdiutil >/dev/null 2>&1; then \
		echo "💿 Create DMG"; \
		hdiutil create -volname "$(APP_NAME)" -srcfolder $(MACOS_DIR)/$(APP_NAME).app -ov -format UDZO $(MACOS_DIR)/$(APP_NAME)-$(VERSION).dmg; \
	fi
	@if [ -n "$$NOTARIZE" ] && [ -n "$$APPLE_ID" ] && [ -n "$$APPLE_PASSWORD" ] && [ -n "$$APPLE_TEAM_ID" ]; then \
		echo "📤 Notarize DMG"; \
		xcrun notarytool submit $(MACOS_DIR)/$(APP_NAME)-$(VERSION).dmg --apple-id $$APPLE_ID --team-id $$APPLE_TEAM_ID --password $$APPLE_PASSWORD --wait || echo "Notarization failed"; \
	fi
	@echo "✅ macOS bundle done."

pkg-macos-store: bundle-macos
	@echo "📦 Build signed .pkg (App Store / distribution)"
	@if [ -z "$$APPLE_IDENTITY" ]; then echo "❌ APPLE_IDENTITY required (Apple Distribution: Name (TEAMID))"; exit 1; fi
	@if [ -z "$$APPLE_BUNDLE_ID" ]; then echo "❌ APPLE_BUNDLE_ID required"; exit 1; fi
	APP_PATH=$(MACOS_DIR)/$(APP_NAME).app; \
	if [ -n "$$PROVISIONING_PROFILE" ] && [ -f "$$PROVISIONING_PROFILE" ]; then \
		echo "🔗 Embed provisioning profile"; cp "$$PROVISIONING_PROFILE" $$APP_PATH/Contents/embedded.provisionprofile; \
	else echo "ℹ️  No provisioning profile (set PROVISIONING_PROFILE)"; fi; \
	codesign --force --timestamp --options runtime --entitlements macos/Tabular.entitlements -s "$$APPLE_IDENTITY" $$APP_PATH/Contents/MacOS/tabular; \
	codesign --force --timestamp --options runtime --entitlements macos/Tabular.entitlements -s "$$APPLE_IDENTITY" -v $$APP_PATH; \
	productbuild --component $$APP_PATH /Applications $(MACOS_DIR)/$(APP_NAME)-$(VERSION).pkg --sign "$$APPLE_IDENTITY" --identifier $$APPLE_BUNDLE_ID; \
	if [ -n "$$NOTARIZE" ] && [ -n "$$APPLE_ID" ] && [ -n "$$APPLE_PASSWORD" ] && [ -n "$$APPLE_TEAM_ID" ]; then \
		xcrun notarytool submit $(MACOS_DIR)/$(APP_NAME)-$(VERSION).pkg --apple-id $$APPLE_ID --team-id $$APPLE_TEAM_ID --password $$APPLE_PASSWORD --wait && xcrun stapler staple $(MACOS_DIR)/$(APP_NAME)-$(VERSION).pkg || true; \
	else echo "ℹ️  Notarization skipped for pkg"; fi
	@echo "✅ pkg created. Upload via Transporter for App Store."

bundle-linux: build-linux
	@echo "📦 Package Linux"
	cd $(LINUX_DIR) && tar -czf tabular-$(VERSION)-linux-x86_64.tar.gz tabular-x86_64
	cd $(LINUX_DIR) && tar -czf tabular-$(VERSION)-linux-aarch64.tar.gz tabular-aarch64
	@mkdir -p $(LINUX_DIR)/AppDir/usr/bin \
		$(LINUX_DIR)/AppDir/usr/share/applications \
		$(LINUX_DIR)/AppDir/usr/share/icons/hicolor/512x512/apps
	cp $(LINUX_DIR)/tabular-x86_64 $(LINUX_DIR)/AppDir/usr/bin/tabular
	@echo "[Desktop Entry]" >  $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Type=Application" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Name=$(APP_NAME)" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Comment=Your SQL Editor, Forged with Rust: Fast, Safe, Efficient." >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Exec=tabular" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Icon=tabular" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Terminal=false" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@echo "Categories=Development;Database;" >> $(LINUX_DIR)/AppDir/usr/share/applications/tabular.desktop
	@if [ -f assets/logo.png ]; then cp assets/logo.png $(LINUX_DIR)/AppDir/usr/share/icons/hicolor/512x512/apps/tabular.png; fi
	@echo "✅ Linux packaging done."

bundle-windows: build-windows
	@echo "🪟 Package Windows"
	cd $(WINDOWS_DIR) && zip -r tabular-$(VERSION)-windows-x86_64.zip tabular-x86_64.exe
	cd $(WINDOWS_DIR) && zip -r tabular-$(VERSION)-windows-aarch64.zip tabular-aarch64.exe
	@echo "✅ Windows packaging done."

release: clean install-deps bundle-macos bundle-linux bundle-windows
	@echo "🎉 Release completed"
	@echo "macOS artifacts:"; find $(MACOS_DIR) -maxdepth 1 -name '*.dmg' -o -name '*.pkg' -o -name '*.app' | sed 's/^/  /' || true
	@echo "Linux artifacts:"; find $(LINUX_DIR) -maxdepth 1 -name '*.tar.gz' | sed 's/^/  /' || true
	@echo "Windows artifacts:"; find $(WINDOWS_DIR) -maxdepth 1 -name '*.zip' | sed 's/^/  /' || true

build:
	cargo build --release

run:
	cargo run

dev:
	cargo build

test:
	cargo test

check:
	cargo fmt --check
	cargo clippy -- -D warnings

fmt:
	cargo fmt

info:
	@echo "Name: $(APP_NAME)"
	@echo "Version: $(VERSION)"
	@echo "Rust: $(RUST_VERSION)"
	@echo "Targets: macOS(universal), Linux(x86_64,aarch64), Windows(x86_64,aarch64)"
	@echo "Dist dir: $(DIST_DIR)"
