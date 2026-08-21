# Walkthrough: iPad / iPadOS Build & Publish Configuration for Tabular

Kami telah menyelesaikan konfigurasi, perbaikan arsitektur, dan pengujian build untuk mendukung target **iPad / iPadOS** di `tabular-client`.

---

## 🚀 Ringkasan Solusi & Implementasi

### 1. Kompatibilitas Rust Staticlib untuk iOS / iPadOS
- **`crate-type`**: Dikonfigurasi menjadi `["rlib", "staticlib"]` di [Cargo.toml](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/Cargo.toml).
- **Target OS Gating**: Menambahkan dependency `apple-native-keyring-store` khusus iOS dan memisahkan dialog picker `rfd` dengan stub fallback di [src/lib.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/lib.rs).
- **Dependency Isolation**: Menyesuaikan modul file dialog di 6 file sumber: [sidebar_collection.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/sidebar_collection.rs), [window_egui/init.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/window_egui/init.rs), [window_egui/settings.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/window_egui/settings.rs), [dialog.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/dialog.rs), [diagram_view.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/diagram_view.rs), dan [export.rs](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/src/export.rs).

### 2. Pembuatan Universal `Tabular.xcframework`
- Dikelola oleh [build-rust-xcframework.sh](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/ios/Xcode/scripts/build-rust-xcframework.sh).
- Menghasilkan framework universal di `ios/Xcode/TabulariOS/Generated/Tabular.xcframework` yang memuat:
  - Slice perangkat iOS (`aarch64-apple-ios`)
  - Slice simulator Apple Silicon (`aarch64-apple-ios-sim`)
  - Header FFI C/Obj-C (`TabularFFI.h`)

### 3. Konfigurasi Xcode Project untuk iPadOS
- **Device Family**: `TARGETED_DEVICE_FAMILY = "1,2"` (iPhone + iPad).
- **Orientasi**: Mendukung Portrait, Portrait Upside Down, Landscape Left, dan Landscape Right di iPad.
- **Linker Frameworks**: Menambahkan linking framework Apple yang dibutuhkan Rust runtime: `SystemConfiguration`, `Security`, `CoreFoundation`, `Network`.
- **Swift Bridging Header**: Dihubungkan langsung ke `Sources/FFI/TabularFFI.h`.
- **Sandbox & Script Phase Optimization**: Mencegah proses cargo berulang atau deadlock di dalam subshell Xcode saat `Tabular.xcframework` sudah tersedia.

### 4. Skrip Otomasi Build & Publishing
- [build_ipad.sh](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/build_ipad.sh): Skrip komprehensif untuk iPadOS yang mendukung perintah `xcframework`, `build`, `sim`, `archive`, `export`, `publish`, dan `all`.
- [ExportOptions-AppStore.plist](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/ios/Xcode/ExportOptions-AppStore.plist) & [ExportOptions-Development.plist](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/ios/Xcode/ExportOptions-Development.plist).
- [TabulariOS.entitlements](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/ios/Xcode/TabulariOS/TabulariOS.entitlements).
- Integrasi ke [Makefile](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/Makefile) (`make build-ipad`, `make archive-ipad`, `make ipa-ipad`, `make publish-ipad`).
- Integrasi ke [build_apple.sh](file:///Users/jayuda/Documents/PROJECT/TABULAR/tabular-client/build_apple.sh).

---

## 🛠️ Cara Menggunakan

### 1. Build untuk iPad Simulator
```bash
# Menggunakan Makefile
make build-ipad

# Atau langsung dengan skrip
bash build_ipad.sh sim
```

### 2. Build & Archive untuk Perangkat Asli / App Store
```bash
# Archive .xcarchive untuk iPad
bash build_ipad.sh archive
# atau
make archive-ipad
```

### 3. Export IPA
```bash
# Export IPA untuk App Store / TestFlight
bash build_ipad.sh export

# Atau untuk Development / Ad-Hoc
bash build_ipad.sh export --dev
```

### 4. Publish ke App Store Connect / TestFlight
Set environment variable kredensial App Store:
```bash
export APPLE_TEAM_ID="YD4J5Z6A4G"
export APPLE_ID="nunung.pamungkas@vneu.co.id"
export APPLE_PASSWORD="<app-specific-password>"

# Upload IPA
bash build_ipad.sh publish
# atau
make publish-ipad
```

---

## ✅ Hasil Verifikasi

1. **XCFramework Universal**:
   - `ios/Xcode/TabulariOS/Generated/Tabular.xcframework` berhasil di-generate dengan deployment target iOS 16.0.
2. **iPad Simulator Build**:
   - `xcodebuild -scheme TabulariOS -destination "generic/platform=iOS Simulator,name=iPad" Debug build` -> **`** BUILD SUCCEEDED **`**.
3. **iPad Device Build & Archive**:
   - `xcodebuild -scheme TabulariOS -destination "generic/platform=iOS" Release archive` -> **`** ARCHIVE SUCCEEDED **`**.
4. **End-to-End Automation Script**:
   - `bash build_ipad.sh build` -> **`[SUCCESS] iPad Device build completed.`**
