# Rencana Implementasi Teknis: Peningkatan Kualitas SQL Editor Tabular

Dokumen ini menyusun rencana arsitektur dan implementasi teknis mendalam untuk meningkatkan pengalaman pengembang (*developer experience / DX*) pada fitur SQL Editor di Tabular, menjadikannya cepat, ergonomis, cerdas, dan disukai oleh para *software engineer* dan *database administrator* (DBA).

---

## 1. Analisis Kebutuhan & Evaluasi Fitur Eksisting

### 1.1 Evaluasi Editor Eksisting Tabular
Tabular saat ini memiliki fondasi editor berbasis Rust dan `egui`:
- **Buffer & Text Handling (`src/editor_buffer.rs`)**: Menggunakan `EditorBuffer` berbasis String dengan riwayat undo/redo (`EditRecord`), pemetaan offset baris/kolom binary search (`compute_line_starts`), serta integrasi multi-caret seleksi (`src/editor_selection.rs`).
- **Autocomplete (`src/editor_autocomplete_new.rs`)**: Menyediakan saran kata kunci, tabel, dan kolom dasar dengan cache in-memory (`autocomplete_cols_mem`, `autocomplete_tables_mem`, `autocomplete_fks_mem`).
- **Pewarnaan Sintaks (`src/syntax_ts.rs`)**: Mendukung integrasi *tree-sitter* SQL dengan fallback ke *heuristic colorizer*.
- **Query Tools (`src/query_tools/mod.rs`)**: Menyediakan integrasi `sqlformat`, linter berbasis aturan ringan, dan beberapa *template snippet* dasar.
- **Konfigurasi Editor (`src/models/structs.rs` -> `AdvancedEditor`)**: Mengatur tema (Github Dark, Monokai, Dracula, dll.), ukuran font, pencarian/penggantian teks (regex, whole word, match case), dan word wrap.

### 1.2 Kebutuhan & Ekspektasi Programmer Modern
Programmer sangat produktif saat SQL Editor memiliki kapabilitas setara alat profesional (DataGrip, TablePlus, DBeaver, VS Code):
1. **IntelliSense Kontekstual & Cerdas**:
   - Pengenalan alias tabel (contoh: `FROM users u WHERE u.` langsung menyarankan kolom milik `users`).
   - Auto-complete klausa `JOIN ... ON` secara otomatis mendeteksi relasi Foreign Key yang ada.
   - Preferensi huruf besar/kecil (UPPERCASE vs lowercase untuk kata kunci SQL).
   - Penanda jenis ikon yang jelas (Keyword ⚡, Table 📦, Column 🏷️, Function 🧩, Snippet 📄).
2. **Ergonomi & Shortcut Produktivitas**:
   - **Execute Current Statement (`Ctrl+Enter` / `Cmd+Enter`)**: Menjalankan hanya blok statement SQL tempat kursor berada tanpa harus memblok seluruh teks secara manual.
   - **Format Query Cepat (`Ctrl+Shift+F` / `Cmd+Shift+F`)**: Memformat query secara rapi dan instan.
   - **Toggle Comment Baris/Blok (`Ctrl+/` / `Cmd+/`)**: Memberi atau mencabut komentar `-- ` pada baris kursor atau seleksi multi-baris.
   - **Manipulasi Baris Cepat**: Duplicate line (`Shift+Alt+Down` / `Ctrl+D`) dan Move line (`Alt+Up` / `Alt+Down`).
3. **Safety Guardrails & Multi-Statement Management**:
   - Pemisah statement pintar yang memperhitungkan string quote literal (`'...'`) dan komentar (`--`, `/* */`).
   - Deteksi eksekusi bahaya (*Unsafe DML Guard*) jika `UPDATE` / `DELETE` tidak memiliki klausa `WHERE`.
4. **Utilitas Output & Result Set**:
   - Salin hasil query ke *clipboard* dengan format fleksibel: CSV, JSON, Markdown Table, dan SQL `INSERT INTO`.
   - Indikator durasi eksekusi dan jumlah baris yang presisi pada status bar editor.

---

## 2. Rancangan Perubahan Arsitektur & Berkas

Perubahan dirancang secara modular dan mematuhi prinsip *immutability* serta minim efek samping.

### 2.1 File yang Akan Diubah / Dibuat

```
src/
├── query_tools/
│   ├── mod.rs               <- Penambahan extractor statement di kursor, ekspansi snippets & format config
│   ├── statement_parser.rs  <- [BARU] Parser pemisah statement cerdas (delimiter ';', literal-safe)
│   └── text_actions.rs      <- [BARU] Helper murni untuk comment toggle, duplicate line, move line
├── editor_autocomplete_new.rs <- Peningkatan resolusi alias tabel, JOIN FK completion, & keyword casing
├── editor.rs                <- Integrasi shortcut baru (Ctrl+/, Alt+Up/Down, Ctrl+D, Ctrl+Shift+F) & statement run
├── models/
│   ├── structs.rs           <- Penambahan opsi AdvancedEditor (keyword casing preference, highlight active line)
│   └── enums.rs             <- Enum untuk KeywordCasing (Upper, Lower, Preserve)
└── data_table/
    └── export_clipboard.rs  <- [BARU] Utilitas salin cepat (Copy as Markdown, JSON, CSV, SQL Inserts)
```

### 2.2 Rincian Modul Baru & Peningkatan

#### A. Statement Boundary Extractor (`src/query_tools/statement_parser.rs`)
- **Tujuan**: Memungkinkan eksekusi *Current Statement* di posisi kursor tanpa mewajibkan pengguna memilih teks dengan mouse.
- **Fungsi Utama**:
  ```rust
  pub struct SqlStatementSpan {
      pub text: String,
      pub range: std::ops::Range<usize>,
      pub line_range: (usize, usize),
  }
  
  pub fn split_statements(sql: &str) -> Vec<SqlStatementSpan>;
  pub fn find_statement_at_cursor(sql: &str, cursor_pos: usize) -> Option<SqlStatementSpan>;
  ```
- **Logika**: State machine ringan yang mengenali:
  - Single-line comment (`-- ...\n`)
  - Multi-line comment (`/* ... */`)
  - String quotes tunggal (`'...'`) dan escape (`''`)
  - Delimiter semicolon (`;`)
  Mengembalikan statement tunggal yang melingkupi posisi kursor saat ini.

#### B. Ergonomic Text Actions (`src/query_tools/text_actions.rs`)
- **Tujuan**: Operasi baris teks berkecepatan tinggi yang murni (*pure functions*) untuk kemudahan pengujian unit test.
- **Fungsi Utama**:
  ```rust
  /// Menambahkan atau menghapus komentar `-- ` pada baris-baris yang terpilih
  pub fn toggle_line_comments(text: &str, selection_start: usize, selection_end: usize) -> (String, usize, usize);
  
  /// Menggandakan baris kursor / seleksi ke baris bawahnya
  pub fn duplicate_lines(text: &str, start_pos: usize, end_pos: usize) -> (String, usize, usize);
  
  /// Memindahkan baris ke atas atau ke bawah (Alt+Up / Alt+Down)
  pub fn move_lines(text: &str, start_pos: usize, end_pos: usize, move_up: bool) -> (String, usize, usize);
  ```

#### C. IntelliSense 2.0 (`src/editor_autocomplete_new.rs`)
- **Alias Resolution**:
  - Memindai klausa `FROM <table_name> <alias>` atau `FROM <table_name> AS <alias>`.
  - Memetakan alias ke nama tabel sebenarnya dalam in-memory metadata (`autocomplete_cols_mem`).
  - Saat programmer mengetik `u.`, sistem mencari kolom dari tabel yang diasosiasikan dengan alias `u`.
- **Foreign Key Auto-Join**:
  - Saat kursor berada setelah kata `JOIN <table_name> ON `, secara otomatis menyarankan kondisi relasi dari `autocomplete_fks_mem` (contoh: `orders.user_id = users.id`).
- **Keyword Casing**:
  - Mengikuti preferensi pengguna di pengaturan `AdvancedEditor` (Uppercase: `SELECT`, `FROM`, `WHERE` vs Lowercase: `select`, `from`, `where`).

#### D. Quick Export Utilities (`src/data_table/export_clipboard.rs`)
- **Fungsi Utama**:
  ```rust
  pub fn format_as_markdown_table(headers: &[String], rows: &[Vec<String>]) -> String;
  pub fn format_as_json(headers: &[String], rows: &[Vec<String>]) -> String;
  pub fn format_as_csv(headers: &[String], rows: &[Vec<String>]) -> String;
  pub fn format_as_sql_inserts(table_name: &str, headers: &[String], rows: &[Vec<String>]) -> String;
  ```

---

## 3. Prinsip Desain & Kepatuhan Batasan

1. **Zero New Dependencies**:
   - Seluruh fungsionalitas memanfaatkan library yang telah terdefinisi di `Cargo.toml`: `sqlformat`, `unicode-segmentation`, `serde_json`, `regex`, `tokio`, `sqlx`, dan `eframe/egui`.
2. **Prinsip Immutability & Safety**:
   - Setiap fungsi transformasi teks menerima slice `&str` dan menghasilkan `(String, usize, usize)` baru untuk teks serta posisi kursor yang disesuaikan.
   - Tidak ada mutasi in-place yang rawan race-condition pada buffer data query.
3. **Performa 60 FPS**:
   - Statement parser dan comment toggling bekerja dalam skala sub-milidetik (O(N) berbasis karakter tanpa alokasi berlebih).
   - Autocomplete tetap menerapkan *debounce* dan *background metadata pre-warming* agar rendering GUI tidak pernah stutter/lag.

---

## 4. Potensi Risiko & Strategi Mitigasi

| Potensi Risiko | Dampak | Strategi Mitigasi |
|---|---|---|
| **Pemisahan Statement Terpotong Pada String/Komentar** | Query rusak saat ada semicolon di dalam teks literal atau komentar (misal: `'hello; world'`). | Parser menggunakan lexer state-machine sederhana yang secara akurat mengabaikan semicolon di dalam string quote dan block comment. |
| **Pergeseran Kursor Setelah Toggle Comment / Move Line** | Kursor melompat ke posisi acak di dalam dokumen. | Fungsi manipulasi teks mengembalikan koordinat offset kursor baru yang telah dikompensasi secara presisi. |
| **Cache Miss Alias Autocomplete** | Tabel alias tidak terdeteksi jika sintaks query kompleks (nested subqueries). | Fallback secara aman ke daftar kata kunci dan seluruh tabel/kolom global tanpa menyebabkan error atau crash. |
| **Ukuran Hasil Query Ekspor Terlalu Besar di Clipboard** | UI membeku (*freeze*) jika mencoba menyalin 100.000 baris ke JSON di clipboard. | Batasi ekspor clipboard pada baris yang tampak/halaman aktif (maksimum 1.000 baris pertama dengan peringatan jika data terpotong). |

---

## 5. Tahapan Verifikasi & Rencana Pengujian

### 5.1 Unit Testing (`tests/` atau modul internal `#[cfg(test)]`)
1. **Test Statement Splitter & Cursor Finder**:
   - Verifikasi pemisahan query multiline: `SELECT 1;\n\nSELECT 2;`.
   - Verifikasi ketahanan terhadap semicolon di dalam string: `SELECT 'foo;bar' AS x; SELECT 2;`.
   - Verifikasi penemuan statement yang benar saat kursor berada di baris pertama, tengah, dan akhir statement.
2. **Test Text Actions**:
   - Toggle comment pada satu baris teks kosong, berindentasi, dan baris normal.
   - Toggle comment pada seleksi multi-baris (menghapus tanda komentar jika semua baris terkomentar, atau menambahkan jika belum).
   - Duplicate line dan pertahankan konsistensi newline.
   - Move line ke atas dan ke bawah pada batas awal dokumen dan akhir dokumen.
3. **Test Format Exports**:
   - Verifikasi output Markdown table, JSON array, CSV escaped quotes, dan SQL INSERT statements.

### 5.2 Pengujian Integrasi & Manual UI
1. **Shortcut Verification**:
   - Tekan `Ctrl+Enter` / `Cmd+Enter` pada editor berisikan multi-statement: pastikan hanya query tempat kursor aktif yang dieksekusi.
   - Tekan `Ctrl+/` / `Cmd+/`: pastikan baris aktif terkomentari secara rapi.
   - Tekan `Ctrl+Shift+F` / `Cmd+Shift+F`: pastikan query diformat sesuai standar format SQL.
2. **Autocomplete Verification**:
   - Uji pengetikan alias: `SELECT u. FROM users u` -> verifikasi daftar kolom `users` muncul di popup.
   - Uji casing kata kunci: verifikasi kata kunci `SELECT`, `FROM`, `WHERE` ditampilkan sesuai preferensi pengaturan editor.
3. **Regresi & Kompilasi**:
   - Verifikasi seluruh kode lolos kompilasi (`cargo check`) dan semua tes lolos (`cargo test`).
