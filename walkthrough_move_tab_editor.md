# Walkthrough: Fitur Export dan Import Seluruh Data Aplikasi (ZIP)

Dokumen ini mendokumentasikan implementasi dan verifikasi fitur **Export & Import Seluruh Data** (Database Connections, Saved Queries, HTTP API Collections, dan Query History) dalam format arsip **ZIP** dengan kemampuan **Restore** lengkap di Tabular.

---

## 1. Ringkasan Fitur

Fitur ini menyediakan mekanisme backup dan migrasi menyeluruh untuk pengguna Tabular:
1. **Pengeksporan Lengkap (Export All to ZIP)**:
   - Mengemas 4 domain data utama ke dalam 1 berkas `.zip`:
     - **Database Connections & Folders**: Semua profil koneksi database (termasuk kredensial, SSL/SSH tunneling, custom views) dan struktur foldernya.
     - **Saved Queries**: Semua berkas `.sql` dan hierarki subdirektorinya dari `{app_data}/query/`.
     - **HTTP API Collections**: Semua workspace (`HttpWorkspace`), folder, saved request, dan environment dari `{app_data}/http_collections/`.
     - **Query Execution History**: Seluruh riwayat eksekusi query dari basis data lokal SQLite (`query_history`).
   - Menyertakan berkas `manifest.json` berisi versi format, timestamp, indikator modul, dan jumlah entri yang diekspor.
2. **Pratinjau & Inspeksi Arsip (Archive Inspection)**:
   - Mampu memeriksa file ZIP sebelum restore dilakukan untuk membaca `manifest.json` atau menghitung entri di dalam arsip secara aman tanpa mengekstraknya ke disk sistem.
3. **Pemulihan & Penanganan Konflik (Restore & Conflict Handling)**:
   - Mendukung 3 strategi konflik:
     - `Merge (Keep Existing)`: Menambahkan data baru tanpa menimpa data yang sudah ada.
     - `Merge (Overwrite Existing)`: Memperbarui data yang telah ada dan menambahkan data baru.
     - `Clean Restore (Replace All)`: Menghapus data lama dan memulihkan data dari arsip secara bersih.
   - Mengamankan kredensial kembali ke backend secrets (`externalize_connection_secrets`).
   - Memetakan relasi `connection_id` pada riwayat query agar integritas Foreign Key tetap terjaga.
   - Melindungi sistem dari serangan keamanan **Zip Slip (Path Traversal)**.
   - Merefresh status in-memory Tabular secara instan tanpa perlu me-restart aplikasi.

---

## 2. Berkas yang Dibuat & Dimodifikasi

| Berkas | Status | Deskripsi |
| :--- | :--- | :--- |
| `src/export_import_all.rs` | **Baru** | Modul logika inti kompresi, inspeksi ZIP, manifest, sanitasi zip slip, serialisasi/deserialisasi 4 domain data, dan mapping relasi database. |
| `src/dialog_export_import_all.rs` | **Baru** | Komponen UI modal `egui` untuk dialog Export All dan dialog Import & Restore All dengan file picker native (`rfd`), status progress, dan laporan ringkasan. |
| `src/lib.rs` | Diubah | Mendaftarkan modul `export_import_all` dan `dialog_export_import_all`. |
| `src/sidebar_database.rs` | Diubah | Mengubah visibilitas `externalize_connection_secrets` menjadi `pub(crate)` agar dapat dipanggil saat restore koneksi. |
| `src/window_egui/mod.rs` | Diubah | Menambahkan field state `show_export_all_dialog`, `show_import_all_dialog`, `export_all_state`, dan `import_all_state` pada struct `Tabular`. |
| `src/window_egui/init.rs` | Diubah | Inisialisasi awal dialog state pada constructor `Tabular::new()`. |
| `src/window_egui/app_impl.rs` | Diubah | Menambahkan menu "Export All Data (ZIP)..." dan "Import All Data (ZIP)..." pada Gear Menu, kartu backup/restore pada Preferences > Data Directory, serta pemanggilan render dialog di loop utama. |
| `src/editor.rs` | Diubah | Menambahkan handler eksekusi command `"Export All Data (ZIP)"` dan `"Import All Data (ZIP)"`. |
| `src/quick_open.rs` | Diubah | Mendaftarkan perintah Export & Import ke Command Palette (Quick Open). |

---

## 3. Struktur Berkas di Dalam ZIP

```
tabular_backup_YYYYMMDD_HHMMSS.zip
├── manifest.json
├── connections/
│   ├── connections.json
│   └── folders.json
├── queries/
│   ├── analytics/
│   │   └── monthly_report.sql
│   └── schema_init.sql
├── http_collections/
│   ├── ws_1710000000_1.json
│   └── ws_1710000000_2.json
└── history/
    └── history.json
```

---

## 4. Hasil Verifikasi & Pengujian

### 4.1 Unit & Integration Tests (`src/export_import_all.rs`)
Dijalankan melalui `cargo test --lib export_import_all`:
```
running 7 tests
test export_import_all::tests::test_conflict_strategy_labels ... ok
test export_import_all::tests::test_manifest_serialization ... ok
test export_import_all::tests::test_zip_slip_detection_in_archive ... ok
test export_import_all::tests::test_zip_options ... ok
test export_import_all::tests::test_zip_slip_rejection ... ok
test export_import_all::tests::test_archive_creation_and_inspection ... ok
test export_import_all::tests::test_roundtrip_export_import ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 180 filtered out
```

Pengujian mencakup:
- **`test_conflict_strategy_labels`**: Memverifikasi teks nama dan deskripsi tiap opsi resolusi konflik.
- **`test_manifest_serialization`**: Memverifikasi serialisasi dan deserialisasi struktur manifest JSON.
- **`test_zip_slip_rejection`**: Memverifikasi penolakan arsip ZIP berbahaya yang berisi path traversal (contoh: `../../etc/malicious.txt`).
- **`test_archive_creation_and_inspection`**: Menguji pembuatan arsip ZIP tiruan dan verifikasi parsing inspeksi manifest sebelum restore.
- **`test_roundtrip_export_import`**: Pengujian end-to-end lengkap yang membuat koneksi, folder, HTTP workspace, dan riwayat query ke database SQLite sementara, mengekspornya ke berkas ZIP, menginspeksi arsip, menghapus memori aplikasi, dan memulihkan kembali seluruh data melalui `import_all_data`.

### 4.2 Keseluruhan Test Suite Workspace
Dijalankan melalui `cargo test`:
- **Library tests**: 187 passed; 0 failed.
- **Editor buffer tests**: 4 passed; 0 failed.
- **Find replace tests**: 6 passed; 0 failed.
- **Query AST tests**: 15 passed; 0 failed.
- **Syntax tree-sitter tests**: 4 passed; 0 failed.
- **Total**: **216 tests passed, 0 failed**.

### 4.3 Verifikasi Kompilasi
Perintah `cargo check` berhasil tanpa error.

---

## 5. Cara Penggunaan Fitur

1. **Melalui Gear Settings Menu**:
   - Klik ikon gerigi (Settings) di pojok kiri bawah.
   - Pilih **📦 Export All Data (ZIP)...** untuk mengekspor data.
   - Pilih **📥 Import All Data (ZIP)...** untuk merestore data dari file ZIP.
2. **Melalui Preferences**:
   - Buka Preferences (`⌘,`) > tab **Data Directory**.
   - Pada bagian **Backup & Restore All Data**, klik tombol "Export All Data (ZIP)..." atau "Import & Restore All Data (ZIP)...".
3. **Melalui Command Palette (`⌘P`)**:
   - Tekan `⌘P` dan ketik `Export All Data (ZIP)` atau `Import All Data (ZIP)` lalu tekan Enter.
