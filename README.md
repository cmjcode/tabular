# TABULAR
Simple & Powerfull SQL Client

![Screenshot Halaman Utama](screenshots/halaman-utama.jpg)


## BUILD ON ARCH LINUX
### Requirements
```bash
sudo pacman -Syu clang llvm
sudo pacman -Syu base-devel
```
Export Environment :
```
export LIBCLANG_PATH=/usr/lib
```

Build multi architecture : 
```
cargo install cross
```

## BUILD ON UBUNTU
### Requirements
```bash
sudo apt update
sudo apt install clang libclang-dev
```

Build multi architecture : 
```
cargo install cross
```


## DEP
1. egui
2. sqlx
3. tiberias
4. redis

## macOS (App Store / Notarized) Build

1. Ensure you have an Apple Developer account and installed Xcode command line tools.
2. Set environment variables (example):
```
export APPLE_ID="your-apple-id@example.com"
export APPLE_PASSWORD="app-specific-password"   # Use app-specific password
export APPLE_TEAM_ID="TEAMID"
export APPLE_BUNDLE_ID="id.tabular.data"
export APPLE_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export NOTARIZE=1
```
3. Build:
```
./build.sh macos --deps
```
4. (Optional) After notarization success, staple the ticket:
```
xcrun stapler staple dist/macos/Tabular-<version>.dmg
```
5. Validate:
```
spctl -a -vv dist/macos/Tabular.app
codesign --verify --deep --strict --verbose=2 dist/macos/Tabular.app
```

Entitlements file: `macos/Tabular.entitlements` (sandbox + network + user file access).

### Create Signed .pkg (Distribution / App Store)
```
export APPLE_IDENTITY="Apple Distribution: Your Name (TEAMID)"   # or Developer ID for outside store
export APPLE_BUNDLE_ID="id.tabular.data"
export PROVISIONING_PROFILE="/path/to/Tabular_AppStore.provisionprofile" # App Store profile
make pkg-macos-store
```
If NOTARIZE=1 and credentials set, the pkg will be notarized & stapled (outside Mac App Store flow). For Mac App Store, upload the signed .pkg via Transporter.
