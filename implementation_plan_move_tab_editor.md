# Rencana Teknis Implementasi: Export & Import Seluruh Data (ZIP)

Dokumen ini menjelaskan rencana teknis menyeluruh untuk menambahkan fitur **Export dan Import seluruh data Tabular** (Database Connections, Saved Queries, HTTP API Collections, dan Query History) dalam format file **ZIP**, serta mekanisme **Restore** data ke dalam sistem.

---

## 1. Analisis Kebutuhan (Requirements Analysis)

### 1.1 Latar Belakang & Tujuan
Saat ini pengguna Tabular dapat melakukan backup pada level database engine tertentu (seperti `pg_dump` atau `mysqldump`), serta import koleksi HTTP individual (seperti Postman atau Yaak). Namun, belum ada mekanisme terpadu untuk:
1. Mengekspor **seluruh konfigurasi dan workspace pengguna** sekaligus ke dalam 1 file arsip portable (ZIP).
2. Memindahkan atau memulihkan seluruh data aplikasi (migrasi perangkat atau pemulihan bencana).
3. Mengimpor kembali file ZIP tersebut sehingga seluruh data (**Connection DB**, **Query**, **HTTP API**, dan **History**) ter-restore dengan aman, konsisten, dan langsung aktif di antarmuka aplikasi.

### 1.2 Cakupan Data (4 Domain Utama)
1. **Connection DB (Koneksi Database)**:
   - Menyimpan seluruh profil koneksi (`ConnectionConfig`): nama koneksi, host, port, username, password, database default, tipe database (`MySQL`, `PostgreSQL`, `SQLite`, `Redis`, `MsSQL`, `MongoDB`, dll.), SSL settings, SSH tunneling & credentials, custom views, dan replication settings.
   - Menyimpan struktur pengelompokan folder koneksi (`connection_folders`).
   - Penanganan kredensial: saat diekspor, kredensial diambil dari state memori/secret store; saat diimpor, kredensial disimpan ulang ke SQLite dan di-externalize ke secrets store (`externalize_connection_secrets`).

2. **Saved Queries (Koleksi Query SQL)**:
   - Seluruh file query SQL (`.sql`) beserta subfoldernya yang tersimpan di direktori aplikasi `{app_data}/query/`.
   - Mempertahankan header metadata Tabular seperti `-- tabular:connection_id=...`, `-- tabular:database=...`, dan hierarki foldernya.

3. **HTTP API (Koleksi & Workspace HTTP)**:
   - Seluruh workspace HTTP (`HttpWorkspace`), subfolder (`HttpFolder`), saved requests (`SavedRequest`), dan variabel environment (`YaakEnvironment`) dari direktori `{app_data}/http_collections/`.

4. **History (Riwayat Eksekusi Query)**:
   - Seluruh log riwayat query dari tabel `query_history` di SQLite (`id`, `query_text`, `connection_id`, `connection_name`, `executed_at`).
   - Penanganan relasi Foreign Key: penyesuaian `connection_id` dengan ID baru jika koneksi diimpor ke basis data target yang memiliki ID berbeda (berdasarkan pencocokan `connection_name`).

### 1.3 Format Struktur Arsip ZIP
Format arsip ZIP didesain modular, aman, dan mudah dibaca secara terstruktur:

```
tabular_backup_YYYYMMDD_HHMMSS.zip
├── manifest.json
├── connections/
│   └── connections.json
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

- **`manifest.json`**:
  ```json
  {
    "version": "1.0",
    "app": "Tabular",
    "exported_at": "2026-09-09T14:30:00Z",
    "counts": {
      "connections": 5,
      "connection_folders": 2,
      "queries": 14,
      "http_workspaces": 3,
      "history_items": 100
    },
    "includes": {
      "connections": true,
      "queries": true,
      "http_api": true,
      "history": true
    }
  }
  ```

---

## 2. Desain Arsitektur & Rencana Perubahan Berkas

### 2.1 Modul Baru

#### A. `src/export_import_all.rs` (Core Logic Module)
Modul independen untuk operasi kompresi, dekompresi, serialisasi, validasi, dan persistensi database:
- **Tipe Data & Model**:
  - `ExportAllManifest`: Metadata arsip.
  - `ExportAllOptions`: Opsi export (pilihan kategori yang disertakan).
  - `ImportAllOptions`: Opsi import (pilihan kategori yang ingin di-restore, serta strategi konflik: `MergeKeepExisting`, `MergeOverwrite`, atau `CleanRestore`).
  - `ExportSummary` & `ImportSummary`: Laporan jumlah data yang berhasil diproses.
  - `ExportImportError`: Error handling komprehensif menggunakan `thiserror`.
- **Fungsi Inti**:
  - `pub fn export_all_data(tabular: &Tabular, target_path: &Path, options: &ExportAllOptions) -> Result<ExportSummary, ExportImportError>`
    - Mengumpulkan data dari in-memory state dan file system.
    - Menulis ke ZIP menggunakan `zip::ZipWriter` dengan kompresi Deflate.
  - `pub fn inspect_archive(archive_path: &Path) -> Result<ExportAllManifest, ExportImportError>`
    - Membaca `manifest.json` dan menghitung preview entri sebelum proses restore dijalankan.
  - `pub fn import_all_data(tabular: &mut Tabular, archive_path: &Path, options: &ImportAllOptions) -> Result<ImportSummary, ExportImportError>`
    - Membuka ZIP dengan `zip::ZipArchive`.
    - Memvalidasi path entri (mencegah Zip Slip vulnerability).
    - Memulihkan koneksi database & folder koneksi ke SQLite serta mendaftarkan secret credentials.
    - Mengekstrak file query ke `{app_data}/query/`.
    - Mengekstrak file workspace HTTP ke `{app_data}/http_collections/`.
    - Menyimpan history ke tabel `query_history` dengan mapping ID koneksi.
    - Merefresh in-memory state Tabular (`load_connection_folders`, `load_queries_from_directory`, `load_workspaces`, `load_query_history`, dan trigger `needs_refresh`).

#### B. `src/dialog_export_import_all.rs` (UI Dialog Module)
Komponen dialog berbasis `egui`:
- **`ExportAllDialogState`**:
  - Pilihan kategori data (checkboxes: Connections, Queries, HTTP API, History).
  - Target path file ZIP default (misal: `~/Downloads/tabular_backup_YYYYMMDD_HHMMSS.zip`).
  - Status eksekusi (Idle, InProgress, Completed, Error).
  - Ringkasan hasil ekspor.
- **`ImportAllDialogState`**:
  - File picker untuk memilih file `.zip`.
  - Preview manifest hasil inspeksi (jumlah item yang ditemukan di dalam file ZIP).
  - Checkbox pilihan data yang ingin di-restore.
  - Opsi penanganan duplikasi (Merge / Overwrite).
  - Tombol aksi "Restore Now" dan progress bar / status banner.

---

### 2.2 Berkas yang Dimodifikasi

1. **`src/main.rs` / `src/lib.rs`**:
   - Daftarkan modul baru:
     ```rust
     pub mod export_import_all;
     pub mod dialog_export_import_all;
     ```

2. **`src/window_egui/mod.rs`**:
   - Tambahkan state flag & dialog state di struct `Tabular`:
     ```rust
     pub show_export_all_dialog: bool,
     pub show_import_all_dialog: bool,
     pub export_all_state: Option<crate::dialog_export_import_all::ExportAllDialogState>,
     pub import_all_state: Option<crate::dialog_export_import_all::ImportAllDialogState>,
     ```

3. **`src/window_egui/init.rs`**:
   - Inisialisasi field baru tersebut dengan `false` dan `None`.

4. **`src/window_egui/app_impl.rs`**:
   - **Gear Settings Context Menu** (baris ~2535):
     - Tambahkan item menu:
       - `📦 Export All Data (.zip)...` -> membuka `show_export_all_dialog = true`
       - `📥 Import & Restore All Data (.zip)...` -> membuka `show_import_all_dialog = true`
   - **Settings Window - Data Directory Tab (`PrefTab::DataDirectory`)** (baris ~437):
     - Tambahkan kartu UI "Backup & Restore Application Data" dengan tombol "Export All to ZIP" dan "Import from ZIP".
   - **Render Loop** (baris ~4995):
     - Render `render_export_all_dialog(self, ctx)` saat `self.show_export_all_dialog == true`.
     - Render `render_import_all_dialog(self, ctx)` saat `self.show_import_all_dialog == true`.

5. **`src/quick_open.rs`**:
   - Daftarkan perintah ke Command Palette:
     - `Export All Data (ZIP)`
     - `Import All Data (ZIP)`

---

## 3. Analisis Potensi Risiko & Strategi Mitigasi

| Risiko | Dampak | Strategi Mitigasi |
| :--- | :--- | :--- |
| **Zip Slip / Path Traversal Attack** | File jahat di dalam ZIP dapat menimpa berkas sistem sembarang (`../../etc/shadow`). | Wajib menggunakan `file.enclosed_name()` dari crate `zip` dan membatasi ekstraksi hanya di dalam subdirektori tujuan yang sah (`query/` dan `http_collections/`). |
| **Foreign Key Constraint pada `query_history`** | `query_history` memiliki relasi `connection_id -> connections(id) ON DELETE CASCADE`. Jika `connection_id` lama tidak ditemukan, query insert history gagal. | Buat tabel mapping ID lama ke ID baru berdasarkan kesamaan nama koneksi (`connection_name`). Jika koneksi belum ada, buat koneksi terlebih dahulu atau kaitkan ke koneksi default yang valid. |
| **Penanganan Kredensial & Secrets** | Password atau SSH key tersimpan sebagai sentinel di SQLite dan data asli di keychain/secret store. | Saat ekspor, ambil data dari `tabular.connections` (yang sudah ter-dekripsi di RAM). Saat import, panggil `externalize_connection_secrets` agar kredensial tersimpan aman di database dan secret backend sistem target. |
| **UI Freeze saat Arsip Besar** | Aplikasi tidak responsif selama kompresi/ekstraksi I/O. | Jalankan proses kompresi dan dekompresi ZIP di background thread / tokio runtime async dengan `mpsc` channel untuk mengirim progress dan hasil ke UI thread. |
| **In-Memory State Stale setelah Restore** | Data sudah masuk ke SQLite/disk tetapi tampilan sidebar tidak terupdate. | Panggil fungsi reload: `sidebar_database::load_connection_folders()`, `sidebar_query::load_queries_from_directory()`, `http_collection::load_workspaces()`, dan `sidebar_history::load_query_history()`, serta set `tabular.needs_refresh = true`. |

---

## 4. Tahapan Verifikasi & Pengujian

1. **Uji Kompilasi (`cargo check`)**:
   - Memastikan tidak ada compile error, type mismatch, atau broken references.
2. **Automated Unit Tests**:
   - Buat unit test komprehensif di `src/export_import_all.rs`:
     - Test pembuatan mock connection, query file, HTTP workspace, dan history item.
     - Test ekspor ke buffer/file ZIP sementara dan verifikasi integritas ZIP serta isi `manifest.json`.
     - Test pembacaan dan validasi isi arsip (`inspect_archive`).
     - Test impor ke direktori sementara dan verifikasi bahwa data koneksi, queries, HTTP collection, dan history berhasil dipulihkan secara identik.
     - Test proteksi Zip Slip (path traversal rejected).
     - Test pemulihan relasi `query_history` saat connection ID berubah.
3. **Uji Integrasi UI**:
   - Verifikasi pembukaan dialog via Gear Menu dan Tab Settings Data Directory.
   - Verifikasi pemilihan file picker (`rfd::FileDialog`).
   - Verifikasi feedback visual, progress bar, dan notifikasi keberhasilan pemulihan data.

---

## 5. Kesimpulan & Batasan Tahap Ini (Stage 1 Scope)

Sesuai instruksi tugas, pengerjaan pada **Tahap 1 (Planning Phase)** dibatasi hanya pada penyusunan dokumen perencanaan teknis ini di file `implementation_plan.md`. Tidak ada kode aplikasi yang diubah pada tahap ini sebelum rencana ini ditinjau dan disetujui oleh pengguna.
