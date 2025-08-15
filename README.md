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


# Fitur Pengaturan Lokasi Data Directory

## Deskripsi

Fitur ini memungkinkan pengguna untuk mengubah lokasi penyimpanan data aplikasi Tabular dari lokasi default `~/.tabular` ke folder yang diinginkan pengguna melalui native file dialog sistem operasi.

## Perubahan yang Dilakukan

### 1. Model Data (config.rs)

- **Menambahkan field `data_directory`** ke struct `AppPreferences`:
  ```rust
  pub struct AppPreferences {
      // ... field lainnya
      pub data_directory: Option<String>,
  }
  ```

- **Menambahkan fungsi helper**:
  - `get_data_dir()`: Mendapatkan lokasi data directory saat ini
  - `set_data_dir(new_path: &str)`: Mengubah lokasi data directory dengan validasi

- **Memperbarui fungsi `config_dir()`** untuk menggunakan environment variable `TABULAR_DATA_DIR` jika tersedia

### 2. UI Settings (window_egui.rs)

- **Menambahkan field baru** ke struct `Tabular`:
  - `data_directory: String`: Lokasi data directory saat ini
  - `temp_data_directory: String`: Temporary input untuk lokasi baru
  - `show_directory_picker: bool`: Flag untuk menampilkan dialog pemilih directory (tidak lagi digunakan)
  - `directory_picker_result: Option<Receiver<String>>`: Channel untuk menerima hasil dari native file dialog

- **Menambahkan bagian "Data Directory"** di preferences window dengan:
  - Tampilan lokasi saat ini
  - Input field untuk lokasi baru
  - Tombol "Browse" yang membuka **native file dialog**
  - Tombol "Apply Changes" untuk menerapkan perubahan
  - Tombol "Reset to Default" untuk kembali ke default
  - Peringatan bahwa perubahan memerlukan restart aplikasi

- **Implementasi async directory picker** menggunakan:
  - `rfd::FileDialog` untuk native OS file dialog
  - Thread terpisah untuk mencegah UI blocking
  - Channel communication untuk hasil selection

### 3. Persistence & Loading

- **Loading preferences**: Aplikasi memuat custom data directory dari preferences dan mengaplikasikannya saat startup
- **Saving preferences**: Perubahan data directory disimpan ke preferences database/JSON

## Cara Menggunakan

### Mengubah Lokasi Data Directory

1. **Buka Settings**: Klik icon gear (⚙️) di pojok kanan atas, lalu pilih "Preferences"

2. **Navigasi ke Data Directory**: Scroll ke bagian "Data Directory" 

3. **Pilih Lokasi Baru**:
   - **Manual**: Ketik path absolut di field "New location"
   - **Browse**: Klik tombol "📁 Browse" untuk membuka **native file dialog**
     - Dialog akan menggunakan file picker sistem operasi (Finder di macOS, File Explorer di Windows, dll)
     - Mulai dari lokasi data directory saat ini
     - Hanya memungkinkan pemilihan folder, bukan file

4. **Apply Changes**: Klik tombol "Apply Changes" jika path valid

5. **Restart**: Restart aplikasi untuk menerapkan perubahan sepenuhnya

### Keunggulan Native File Dialog

- **User Experience yang Familiar**: Menggunakan dialog yang sama dengan aplikasi lain di sistem operasi
- **Keyboard Shortcuts**: Mendukung shortcut keyboard standar OS (Cmd+Shift+G di macOS, dll)
- **Bookmarks & Favorites**: Akses ke bookmark dan lokasi favorit sistem
- **Network Drives**: Mendukung pemilihan network drives dan external storage
- **Recent Locations**: Akses ke lokasi yang baru-baru ini diakses
- **No Blocking UI**: Dialog berjalan di thread terpisah sehingga tidak memblokir UI utama

### Lokasi Default

- **macOS/Linux**: `~/.tabular` (contoh: `/Users/username/.tabular`)
- **Windows**: `C:\Users\username\.tabular`

### Lokasi Alternatif yang Disarankan

- **Documents**: `~/Documents/Tabular`
- **Desktop**: `~/Desktop/Tabular`
- **External Drive**: `/Volumes/ExternalDrive/Tabular` (macOS) atau `D:\Tabular` (Windows)
- **Network Drive**: `\\server\share\Tabular` (Windows) atau `/mnt/share/Tabular` (Linux)

## Validasi & Error Handling

- **Path harus absolut**: Relative path ditolak
- **Directory harus writable**: Aplikasi test kemampuan write sebelum apply
- **Auto-create**: Directory dibuat otomatis jika belum ada
- **Rollback**: Jika gagal, kembali ke lokasi sebelumnya
- **Thread-safe**: File dialog berjalan di thread terpisah dengan channel communication

## Environment Variable

Aplikasi juga mendukung environment variable `TABULAR_DATA_DIR`:

```bash
export TABULAR_DATA_DIR="/custom/path/to/tabular/data"
./tabular
```

Environment variable memiliki prioritas tertinggi dan akan override setting di preferences.

## Migration Data

⚠️ **PENTING**: Fitur ini tidak otomatis memindahkan data yang sudah ada. Jika ingin memindahkan data:

1. Tutup aplikasi Tabular
2. Copy folder `~/.tabular` ke lokasi baru
3. Buka aplikasi dan ubah setting data directory
4. Restart aplikasi

## File yang Tersimpan

Directory data berisi:
- **preferences.db** atau **preferences.json**: Konfigurasi aplikasi
- **cache.db**: Cache koneksi dan metadata database  
- **queries/**: Folder berisi saved queries
- **history/**: History query yang telah dijalankan

## Technical Implementation

### Dependencies
- **rfd**: Rust File Dialog untuk native OS file picker
- **std::sync::mpsc**: Channel untuk komunikasi antar thread
- **std::thread**: Thread untuk non-blocking file dialog

### Flow Diagram
```
User clicks Browse
    ↓
spawn_thread(rfd::FileDialog)
    ↓
User selects folder in native dialog
    ↓
send(selected_path) → channel
    ↓
UI thread receives → update temp_data_directory
    ↓
User clicks Apply → validate & set_data_dir()
```

### Error Handling
- Network drive disconnection handling
- Permission denied graceful fallback
- Invalid path format detection
- Disk space validation before directory creation

## Platform Support

- **macOS**: Menggunakan Cocoa NSOpenPanel
- **Windows**: Menggunakan Windows Shell Common Dialog
- **Linux**: Menggunakan GTK file chooser atau KDE file dialog (tergantung desktop environment)

Fitur ini memberikan pengalaman yang native dan familiar bagi pengguna di setiap platform.
