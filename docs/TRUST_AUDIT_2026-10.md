# Audit Kepercayaan Tabular — Oktober 2026

Tanggal: 2026-10-26 · Tabular 1.6.1 · commit `521fb42`

Pertanyaan: apa yang harus diperbaiki agar Tabular **dipercaya programmer** — hasil solid, robust,
stabil; fitur konsisten; alur menu dan workflow mudah dipahami?

Metode: audit kode statis (bukan uji runtime) oleh lima jalur paralel — silent failure, keselamatan
data, konkurensi/stabilitas, struktur/dead-weight, dan UX/IA. Setiap temuan diverifikasi dengan
membaca kode pada commit di atas; yang belum pasti ditandai *(belum pasti)*. Nomor baris mengacu ke
commit tersebut dan akan bergeser.

Baseline: `src/` = **323k baris Rust di 365 file** (AGENTS.md menulis 115k — usang), 71k di
`window_egui/`. 1.484 `#[test]`, 8 file integrasi; driver MySQL/PostgreSQL/SQLite/MongoDB **0 tes**.

---

## 0. Ringkasan eksekutif

Fondasinya lebih kuat dari yang terlihat: `unwrap()` di kode non-tes hanya 59 dan semuanya terjaga;
cancel query sampai ke server (`pg_cancel_backend`, `KILL QUERY`, `sqlite3_interrupt`, MsSQL
ATTENTION); autosave sesi tiap 2 detik dengan tulis atomik; panic hook menulis crash log; simpan grid
berjalan satu transaksi dengan cek `expected_rows = 1`; NULL dibedakan dari string `"NULL"`; Redis
dari editor whitelist baca saja.

Yang menggerus kepercayaan bukan crash, tetapi **empat pola**:

1. **"Gagal diam-diam, tampil sukses."** Tambah/edit koneksi yang gagal disimpan jatuh ke memori dan
   hilang saat restart; password jatuh ke plaintext saat keychain gagal; simpan grid jatuh ke mode
   non-transaksional tanpa pemberitahuan.
2. **Baris yang salah bisa ter-update.** Tanpa PK, WHERE memakai heuristik "kolom pertama mengandung
   `id`"; kolom biner ditulis balik sebagai string; identifier tidak di-escape di jalur simpan grid.
3. **UI beku.** Ekspansi node tree dan klik view/procedure menjalankan query jaringan `block_on` di
   thread UI tanpa timeout; `Runtime::new()` + pool baru per panggilan di `metadata/*`.
4. **Tidak ada menu bar.** Semua fitur bersembunyi di popup gear, context menu sidebar, atau palette;
   istilah tidak konsisten; string Bahasa Indonesia bocor ke UI; sample DB hanya di iOS.

Rencana prioritas ada di §7.

---

## 1. Keselamatan data

| # | Sev | Lokasi | Masalah | Fix |
|---|---|---|---|---|
| D1 | **Critical** | `src/spreadsheet.rs:1140-1151` | Tanpa PK, fallback WHERE memakai kolom pertama bila namanya *mengandung* "id" (`user_id`, `valid`, `width`). `expected_rows=1` menyelamatkan bila >1 baris cocok, tetapi bila kebetulan tepat 1 baris lain cocok → baris salah ter-update. | Hapus heuristik. Tanpa PK/unique: SQLite pakai `rowid`, engine lain tolak simpan dengan pesan jelas. |
| D2 | **Critical** | `src/driver_mysql.rs:135-170, 303-330`, `src/driver_mssql.rs:392` | Kolom BINARY/BLOB/BIT ditampilkan sebagai teks/`0x..`/`0b..`, lalu ditulis balik sebagai string literal (`N'0x..'` di MsSQL dikonversi implisit tanpa error). Tidak ada gate "kolom biner read-only" di `data_table/`. | Tandai kolom biner/BIT non-editable, atau emit literal biner per dialek (`X'..'`, `0x..`, `B'..'`). |
| D3 | High | `src/driver_sqlite.rs:125` | BLOB jadi placeholder `<BLOB n bytes>`; tabel tanpa PK → WHERE semua kolom tidak pernah cocok; Duplicate Row menulis placeholder ke kolom BLOB. | Kunci dengan `rowid`; tampilkan hex; larang edit. |
| D4 | High | `src/editor.rs:11066-11087` | `is_unsafe_dml_query` hanya memeriksa statement **pertama**: `SELECT 1; DELETE FROM users` lolos tanpa dialog. `safety_guard::analyze_safety` (semua statement, sadar quote) sudah ada tetapi hanya dipakai agent. | Editor memanggil `analyze_safety`. |
| D5 | High | `src/connection/execute.rs`, `src/editor.rs`, `query_jobs.rs` | Tidak ada konfirmasi untuk DDL destruktif yang diketik (DROP/TRUNCATE/DROP COLUMN). Klasifikasi destruktif ada di `agent/access.rs:119-190` tetapi tidak dipakai GUI. | Satu klasifikasi untuk GUI + agent; dialog konfirmasi, opsi per koneksi. |
| D6 | High | (tidak ada) | Tidak ada mode read-only/safe per koneksi untuk manusia. Label "Production" (`connection_env.rs`) tidak membatasi apa pun. | Toggle per koneksi "Read-only" / "Confirm destructive", default aktif untuk Production. Lihat `TABLEPRO_GAP_CHECKLIST.md` A1. |
| D7 | High | `src/window_egui/query_jobs.rs:535-552` | Bila pool belum ada di `connection_pools`/`shared_connection_pools`, simpan grid jatuh ke `run_query_with_callback`: statement digabung `;`, `expected_rows` diabaikan, gagal ke-3 tidak membatalkan ke-1/2; hanya `log::warn!`. | Tolak simpan sampai pool siap (`ensure_background_pool_creation` → retry), jangan degradasi diam-diam. |
| D8 | Medium | `src/spreadsheet.rs:403-479` | `spreadsheet_quote_ident` tidak menggandakan quote di dalam nama; `quote_table_ident` lolos bila sudah ada quote. Kolom `a"b` → SQL salah. `data_table/filter_sort.rs:127-145` sudah benar. | Pakai satu `quote_identifier`. |
| D9 | Medium | `src/connection/atomic.rs:92-158` | Timeout transaksi MsSQL (BEGIN/COMMIT manual di deadpool) men-drop future → koneksi kembali ke pool dengan transaksi terbuka. | Pada timeout buang koneksi dari pool (pola `session.rs:404`). |
| D10 | Medium | `src/connection/execute.rs` (MsSQL) | `affected_rows: None` — UPDATE/DELETE MsSQL tanpa jumlah baris. | Tambah `rows_affected`. |
| D11 | Medium | `src/driver_postgres.rs:472-486` | Array PG menyisipkan sentinel `"\0NULL"` mentah ke `{...}`; tampil karakter NUL, ditulis balik rusak. | Tulis `NULL` di dalam array. |
| D12 | Medium | `src/driver_sqlite.rs:138-141` | NUMERIC/DECIMAL dibaca f64 dulu → lossy, WHERE tidak cocok. | Baca TEXT dulu. |
| D13 | Medium | `src/dialog_backup_restore.rs:440, 813-856`, `src/data_transfer/transfer.rs:206`, `backup_restore.rs:2275-2313` | Restore dengan DROP/overwrite dan Transfer dengan `DELETE FROM target` hanya teks peringatan, tidak ada konfirmasi kedua. Drop DB dari sidebar ada dialog tetapi tanpa ketik nama. | Konfirmasi dua langkah (ketik nama) untuk semua operasi menimpa. |
| D14 | Medium | `src/editor.rs:326-328, 585, 617, 10349` | Tutup tab manual-commit = ROLLBACK implisit tanpa prompt; `tab.tx_active` di-set saat Execute, bukan dari `session.tx_open` → Commit tampak aktif padahal tidak. | Prompt "uncommitted transaction"; pantulkan `tx_open`. |
| D15 | *(belum pasti)* | `src/data_table/grid_model.rs:135-146` | Escaping `\` MySQL salah bila server `NO_BACKSLASH_ESCAPES`. Grid edit pada hasil MongoDB tidak di-gate. | Verifikasi runtime; gate Mongo. |

Yang sudah benar: MsSQL legacy `OFFSET/FETCH` dan `__tabular_rn` konsisten; `CLIENT_FOUND_ROWS`
aktif di sqlx-mysql; CSV import memperlakukan "NULL" sebagai string kecuali `null_text` diisi.

Matriks per driver (PK-update / transaksi saat simpan / konfirmasi destruktif / read-only):
PG Yes/Yes/Partial/No · MySQL Yes/Yes/Partial/No · SQLite Partial/Yes/Partial/No ·
MsSQL Yes/Yes(rapuh)/Partial/No · Redis N/A/N/A/Yes(whitelist)/de facto · Mongo No/No/Yes(drop)/No.

---

## 2. Silent failure & pelaporan error

Catatan: 942 `unwrap()` / 652 `expect()` di baseline hampir semuanya kode tes. Non-tes: 59, semua
terjaga. Mutex memakai `unwrap_or_else(PoisonError::into_inner)`; panic worker ditangkap
(`init.rs:1800`). Pesan error query ditampilkan utuh (`query_jobs.rs:139-147`) — bagus.

| # | Sev | Lokasi | Masalah | Fix |
|---|---|---|---|---|
| S1 | High | `src/sidebar_database.rs:740-775`, `:1395` | Tambah koneksi: `save_connection_to_database` gagal (error dibuang) → **fallback ke memori** dengan id palsu `max+1`; tampil tersimpan, hilang saat restart, id bisa bertabrakan. | Hapus fallback; `toasts.error`; dialog tetap terbuka. |
| S2 | High | `src/sidebar_database.rs:705-726`, `src/connection/crud.rs:93-95`, `sidebar_tree.rs:1994`, `app_impl.rs:5749` | Edit koneksi gagal → `debug!` lalu "fallback in-memory"; 4 caller mengabaikan nilai balik. | `Result<(), String>` + toast di semua caller. |
| S3 | High | `src/sidebar_database.rs:2092-2120, 1703-1735` | `connections.db` di data dir kustom gagal dibuka (`Other`: cloud drive/USB belum mount, permission) → diam-diam membuat DB kosong di folder lokal. User melihat semua koneksi hilang dan mulai menyimpan ke DB yang salah. | Toast + banner sidebar selama sesi fallback. |
| S4 | High | `src/secrets.rs:57-76` | Backend secret gagal → password/SSH key disimpan **plaintext** di `connections.db`, hanya `warn!`. | Kembalikan `Stored::{Secret, PlaintextFallback}`; toast sekali per sesi. |
| S5 | Medium | `src/secrets.rs:82-90` | Sentinel ada tapi secret hilang → password `""`; user hanya melihat "password authentication failed" dari server. | Tandai `credentials_missing`, hint di dialog edit. |
| S6 | Medium | `src/config.rs:1083, 1230-1248, 1346-1354` | Semua simpan preferensi `let _ =`; dialog menutup seolah sukses. | Toast error pertama. |
| S7 | Medium | `src/export_import_all.rs:905-985, :161` | Restore All: gagal hanya `eprintln!` (66 situs, tak terlihat GUI); `ImportSummary` tanpa `failed`. | `errors: Vec<String>` + tampilkan; `eprintln!` → `log::warn!`. |
| S8 | Medium | `src/sidebar_history.rs:160-185` | History gagal ditulis → `warn!`; tampil di sidebar, hilang saat restart. | Toast sekali per sesi. |
| S9 | Medium | `src/window_egui/app_impl.rs:923-1000, 3521-3529` | DBA monitor / server metrics / user manager mengalamatkan hasil dengan **index tab**; tab ditutup/digeser → hasil mendarat di tab lain atau hilang. Jalur query utama sudah pakai `tab.id` (`query_jobs.rs:69-83`). | Kirim `tab.id`. |
| S10 | Medium | `src/sidebar_database.rs:3545-3600` | `reset_corrupted_sqlite_db`: INSERT di-`let _`, lalu log "restored all" dan `return true`. | Hitung gagal, jangan klaim sukses. |
| S11 | Low | `src/sidebar_database.rs:820-826, :359` | `test_connection_in_progress = true` lalu `let _ = sender.send(..)` → spinner selamanya bila channel tutup *(belum direproduksi)*. | Cek `is_err()`. |
| S12 | Low | `connection/crud.rs:160-172, 855-867`, `connection_mgr.rs:846-870, 1062-1097`, `cache_data.rs:599-608` | Hapus cache di-`let _` → tree basi, user mengira refresh tidak bekerja. | `warn!` + toast. |

Pola sistemik: 75 `let _ = sqlx::query(...)` non-tes (~60% cache, aman; sisanya tidak); 61
`debug!(".. fail ..")` yang tidak tampil di log default; hasil job dialamatkan dengan index tab di
3 receiver.

---

## 3. Stabilitas & konkurensi

| # | Sev | Lokasi | Masalah | Fix |
|---|---|---|---|---|
| C1 | High | `driver_mysql.rs:671-691`, `driver_postgres.rs:304, 382`, `driver_mssql.rs:558, 654`, `driver_mongodb.rs:112, 152`, `driver_sqlite.rs:338`, `driver_redis.rs:429-773`; caller `tree_loader.rs:1241-2005` | Ekspansi node tree saat cache kosong → `block_on` jaringan di thread UI **tanpa timeout** (pool dibatasi 500 ms, `list_*_tables` tidak). Koneksi lambat/VPN putus → window beku puluhan detik. | Jadikan `BackgroundTask` (`FetchFolderItems`) + placeholder "Loading…"; minimal `with_metadata_timeout` 8 s. |
| C2 | High | `src/connection/metadata/columns.rs:12-27`, `metadata/ddl.rs:10, 337, 604`; caller `tree_loader.rs:2162`, `sidebar_tree.rs:647, 1685`, `quick_open.rs:1402`, `diagram.rs:180`, `cache_data.rs:793` | `fetch_columns_from_database` / `fetch_view_definition` / `fetch_procedure_definition` membuat `Runtime::new()` + **pool DB baru (TCP/SSH) per panggilan**, dipanggil dari UI; batch cache → N runtime + N koneksi. | Terima `&DatabasePool`/`Handle` yang ada; jalankan di worker. |
| C3 | Medium | `cache_data.rs:20-37` (17 caller `block_on_cache`), `agent_access_ui.rs:158-164` | Setiap akses `connections.db` dari UI `block_on` dengan `busy_timeout(5 s)`; `pending_approvals` di-poll tiap 1 s di thread UI. Saat `tabular mcp` menulis lama → frame tersendat hingga 5 s. | Polling & baca cache ke worker; `busy_timeout` pool GUI ~500 ms + retry latar. |
| C4 | Medium | `init.rs:120-131, 215, 1139-1515`, `http_send.rs:227`, `crud.rs:222`, `sidebar_database.rs:1629, 3708`, `spreadsheet.rs:670`, `export_import_all.rs:1801, 2092` | **33× `Runtime::new()`**; dua runtime multi-thread utama + runtime sekali-pakai per request HTTP. `Runtime::new()` di dalam konteks tokio panic — saat ini aman hanya karena konvensi. | Satu `OnceLock<Arc<Runtime>>` + helper `block_on_anywhere`; larang `Runtime::new()` di luar `init.rs`/`agent/cli.rs` via clippy `disallowed_methods`. |
| C5 | Medium | `init.rs:1035-1052, 1101-1111` | Startup `initialize_database` sinkron di `update()`; worker tidur-poll 3 s. | State "Preparing local database…". |
| C6 | Medium | `app_impl.rs:4481, 4814-4860`, `update.rs:31` | Simpan prefs & cek update `block_on` di thread UI tiap flush. | Kirim ke thread penulis `session_restore::writer()`. |
| C7 | Low-Med | `session_restore.rs:242-244` | Snapshot sesi tiap 2 s menyalin + hash seluruh isi tab (≤5 MB/tab). | Fingerprint per tab + `dirty` flag. |
| C8 | Low | `session_restore.rs:578-592`, `app_impl.rs:5868-5885` | Quit saat save/query berjalan tidak diperingatkan; `on_exit` tidak menunggu runtime → Save lalu Cmd+Q = perubahan hilang (rollback, tidak korup). | Dialog quit cek `jobs.active`; `on_exit` tunggu save ≤5 s. |
| C9 | Low | `config.rs:686`, `preferences.rs:1033` | Hasil dimuat penuh `Vec<Vec<String>>`, default 50k baris (≈100-200 MB/tab untuk 30 kolom). Dapat diterima; batasi preferensi >~500k. | Dokumentasi + batas. |
| C10 | Low | `app_logging.rs:189-243` | Setelah crash, start berikutnya tidak memberi tahu ada `crash-*.log`. Kehilangan kerja ≤2 s (bagus). | Toast "Tabular crashed last time, report at …". |

Pola yang **benar** dan harus jadi acuan: `spawn_query_job`/`run_grid_save` (`query_jobs.rs:449-612`)
dan `session_restore::writer()` — kerja di runtime/thread latar, hasil via channel, UI hanya `try_recv`.
Anti-pola tersisa: `tabular.get_runtime().block_on(...)` di `fn foo(tabular: &mut Tabular)` yang
dipanggil dari render.

---

## 4. Alur menu, IA, konsistensi

Peta aktual: **tidak ada menu bar**. Satu-satunya menu = popup gear "Settings & Account"
(`app_impl.rs:2471-2852`) yang merangkap File + Tools + Help (Account, Preferences, Export/Import
ZIP, Open File, Open Data File, Transfer, Compare, Decrypt, Check Updates, Logs, Query Insights,
Agent Access, Shortcuts, Diagnostics, About, Sign Out). Sidebar: `DBs | APIs | Git | Team`;
`DBs` → segmen `Cons | Sqls | Hist | Comp`. Palette 38 perintah tanpa kategori Git/Diagram/
Integration/Project/Backup.

| # | Masalah | Lokasi | Fix |
|---|---|---|---|
| U1 | Tidak ada menu bar; gear = File+Tools+Help. | `app_impl.rs:2471-2852` | Menu bar `File / Edit / View / Database / Project / Tools / Help` (native macOS); gear hanya Account + Preferences. |
| U2 | Backup, Restore, Copy DB, Schema Diff, Users & Privileges, DBA Monitor, Diagrams, Replication, Import Data **hanya di context menu** sidebar. | `sidebar_tree.rs` (mis. `:3172`) | Masukkan ke menu `Database`/`Project` + palette. |
| U3 | Label sidebar singkatan "Cons/Sqls/Hist/Comp". | `app_impl.rs:1158-1161` | "Connections / Queries / History / Compare". |
| U4 | Istilah bersaing: Workspace vs Collection vs HTTP Connection vs Project; Remove vs Delete; Group ×3 makna; "Backup & Restore" = ZIP aplikasi (Preferences) **dan** dump DB (context). "MCP" = server Tabular dan klien AI. | `http_collection.rs:288`, `sidebar_collection.rs:405`, `preferences.rs:1253` | Glosarium satu istilah; rename tab Preferences → "Data Directory & App Export". |
| U5 | Sample DB hanya `#[cfg(target_os = "ios")]`; desktop pertama kali = sidebar kosong tanpa getting-started. | `sample_data.rs:14-26` | Seed "Sample Database" saat `connections` kosong; empty state dengan tombol. |
| U6 | String Bahasa Indonesia di UI: Team ("Belum ada member.", "Pilih Team:", "Tambah Member"), error Flexurio ("Gagal membaca…"), deskripsi endpoint. | `sync/ui_teams.rs:221-1446`, `flexurio_import.rs:264-666`, `repo_endpoints.rs:1203, 1233` | Terjemahkan; tes CI grep kata Indonesia pada literal UI. |
| U7 | Dua fitur "Compare" bernama mirip: gear "Compare Data..." (2 tabel) vs sidebar "Comp" (multi-source). | `transfer_compare_ui.rs`, `multi_compare_ui.rs` | "Compare Two Tables…" vs "Compare"; atau satu halaman dua mode. |
| U8 | Preferences: hint "Changes are saved automatically." **dan** tombol "Save"; `query_timeout_secs`, `max_result_rows`, `show_system_objects` global padahal per-koneksi; `ai_panel_width`, `last_update_check_iso` adalah state bukan prefs; Shortcuts & Language tanpa tab. | `preferences.rs:724, 730`, `config.rs` | Hapus tombol Save; "Defaults for new connections"; tab Shortcuts & General. |
| U9 | Konfirmasi tanpa helper: 110 `egui::Window::new` ad hoc, 0 `egui::Modal`; 6 varian judul ("Confirm Delete Connection", "Drop Index?", "Delete suite?", …) dan label aksi. 6 mekanisme notifikasi (`toasts`, `error_message`, `status_message`, `last_error`, `set_pref_feedback`, `os_notify`). | `render_dialogs.rs:1917`, `render_structure.rs:2828`, `integration_test_ui.rs:4211`, `ui_teams.rs:1420`, `user_manager.rs:3393` | `style::confirm_dialog(title, body, danger_label)`; toast untuk non-blocking, modal hanya fatal. |
| U10 | Label menu ≠ judul dialog: "Export All Data (ZIP)..." → "Export All Application Data (ZIP)" (+emoji); "Agent Access (MCP)" → "Agent Access". | `app_impl.rs:2737-2742` | Judul = label, tanpa emoji. |
| U11 | Shortcut tak terdokumentasi (Cmd+H replace, Cmd+D next occurrence, Cmd+G find next, Cmd+B go-to-def hanya di `editor.rs`, 64 cek modifier di luar keymap); 34 hover "Cmd+…" hard-coded tampil di Windows/Linux. Cmd+Shift+T = toggle manual-commit *(bertabrakan konvensi reopen tab, inferensi)*. | `editor.rs:1440-3753`, `app_impl.rs:2389, 2881` | Daftarkan ke `keymap.rs`; `keymap.label(Action)`. |
| U12 | 270 label berawalan emoji bercampur `egui_icons` (gear: `ICON_*` dan emoji berdampingan). Risiko tofu; melanggar aturan glyph proyek. | `window_egui/*`, `sidebar_tree.rs` | Migrasi ke `ICON_*`; tes glyph-coverage. |
| U13 | Elipsis campur: 190 `"..."` vs 282 `"…"`, sering dalam satu menu. | — | Satu bentuk (`…`). |
| U14 | Pintu masuk ganda berbeda perilaku: buka file via gear (file viewer) vs toolbar (sql) vs context "Open .sql File Here…" vs 2 entri palette. `show_settings_window = true` dari 11 call site. | `app_impl.rs:2384`, `quick_open.rs` | Satu "Open File…" yang mendeteksi tipe. |

Fitur tanpa kode / draft yang masih ada di docs: Notebook (`NOTEBOOK_PLAN.md`, nol kode),
Multi-agent chat (draft), Blueprint flow ("sisa uji manual"). Kandidat flag **Labs**: business-process
flow, Collaboration realtime (CRDT), Obsidian, CLI agent profiles, Replication, Flexurio import.

IA target ringkas:

```
File:     New Query · New HTTP Request · Open File… · Open Data File… · Save · Close Tab · Import Data… · Export Results… · Export/Import App Data (ZIP)… · Quit
Edit:     Undo/Redo · Find & Replace · Go to Line · Format SQL · Toggle Comment · Preferences…
View:     Sidebar · AI Assistant · Query Insights · Background Processes · Refresh · Theme
Database: New Connection… · Edit Connection… · Backup… · Restore… · Copy Database… · Schema Diff… · Transfer Tables… · Compare Tables… · Users & Privileges… · Process Monitor… · Server Dashboard…
Project:  New Project… · Switch Project › · Environments… · Diagrams › · Integration Tests… · Repository Setup…
Tools:    Plugins… · Agent Access (MCP Server)… · Decrypt Exported File… · Open Logs Folder · Copy Diagnostics
Help:     Keyboard Shortcuts · Documentation · Check for Updates · About Tabular
Sidebar:  Connections | Queries | History | Compare · APIs (Collections) · Git · Team (setelah Sign In)
Palette:  satu sumber dari menu di atas; keymap terintegrasi
```

---

## 5. Struktur kode & dead weight (repo-scan)

Klasifikasi: kode proyek 12,8 MB (`src/`); vendored `patches/winit/` 3,0 MB (salinan penuh winit
0.30.13 untuk 3 patch kecil — 23% ukuran tree); aset 3,5 MB; artefak yang tertracked padahal ada di
`.gitignore`: `.idea/` dan `*.xcuserstate`; `integrations/raycast/package-lock.json` 108 KB. Pack git
102 MB.

God object & fungsi raksasa: `Tabular` **575 field** (`window_egui/mod.rs:173`); 58 fungsi >300
baris, 7 >1000: `editor.rs:1303 render_advanced_editor` 3.726 · `diagram_view.rs:1130 render_diagram`
3.283 · `sidebar_tree.rs:2140 render_tree_node_with_table_expansion` 2.553 · `app_impl.rs:1663
render_central_panel` 2.351 · `data_table/render_data.rs:14 render_table_data` 2.294 ·
`sidebar_tree.rs:60 render_tree` 2.080 · `app_impl.rs:4498 ui` 1.368.

Duplikasi: pemecah statement SQL **4 implementasi** (`query_tools/statement_parser.rs:11`,
`connection/sql.rs:757`, `editor.rs:10772`, `safety_guard.rs:216`); quote identifier **≥9**
(`schema_objects/sql.rs:7`, `autocomplete/lexer.rs:20`, `http_tests.rs:2164`, `agent/ops.rs:167`,
`driver_api/query.rs:22`, `query_ast/emitter/dialect.rs`, `table_wizard.rs:61`, `pagination.rs:389`,
`data_table/filter_sort.rs:127`, `spreadsheet.rs:403`); raw SQLite FFI (20 unsafe, 0 SAFETY) di
`http_collection.rs:805-925` dan `backup_restore.rs` padahal sqlx sudah ada; 10 file `*_jobs.rs`
dengan 3 pola job berbeda, registry `BackgroundTasks`/`TaskOwner` hanya dipakai 5 dari 10.

Dead code: 42 `allow(dead_code)` (~1.500-2.500 baris), antara lain `pool.rs:1243 create_database_pool`,
`:1809 get_or_create_connection_pool_with_retry`, `editor.rs:8665 open_legacy_command_palette`,
`sidebar_database.rs:2680 initialize_sample_data`, 4 field `EditorSettings` tak dibaca; dep
`colorful` 0 referensi; `editor_autocomplete.rs` shim 2 baris ke `_new`.

Feature flag: `query_ast` tidak berfungsi sebagai opt-out (`sqlparser` dipakai tanpa cfg di
`connection/sql.rs`, `query_diagram/parse.rs`); feature `sqlparser` tidak dirujuk.

Dependensi: 81 unik, **55 pre-1.0** (sqlx 0.9, mssql-client 0.20, eframe 0.36, reqwest 0.13, egui_icons
0.0.1); dua stack TLS (rustls ring **dan** aws-lc-rs; openssl vendored via sqlx native-tls); `md5` +
`md-5`; `chrono` + `jiff`; `deny.toml` `multiple-versions = "warn"`.

Unsafe: 74 situs, 16 komentar `// SAFETY`. 39 FFI objc2 (2 SAFETY), 20+ raw SQLite (0 SAFETY).

Tes & CI: 0 tes di `connection/metadata/` (2.818 baris), `connection/session.rs`,
`driver_{mysql,postgres,sqlite,mongodb}`; satu-satunya tes driver nyata `#[ignore]`. CI: fmt check
**`continue-on-error: true`**, windows `continue-on-error`, deny `continue-on-error`; tidak ada tes
integrasi DB (services), tidak ada coverage. AGENTS.md bilang fmt belum di-enforce — tidak akurat.

Verdict modul (ringkas): Core Asset — `autocomplete/`, `query_diagram/`, `git/`, `agent/`,
`data_transfer/`, `repo_setup/`, `driver_api/`, `schema_objects/`, `query_tools/`, `connection/`.
Extract & Merge — `editor*` (16k, flat), `diagram_*` (25 file/32k, flat), `http_*` (12 file/18,6k,
flat), `data_table/`, `models/structs.rs` (3.350 baris), `user_manager.rs`, `backup_restore.rs`,
`query_ast/`. Rebuild bertahap — `window_egui/` (God object). Rebuild tes — `driver_*.rs`.
Deprecate/plugin — `flexurio_import.rs`. Under-tested — `sync/` (9,6k baris crypto, 16 tes).

---

## 6. Yang sudah baik (jangan dirusak saat refactor)

- Cancel query sampai server; koneksi PG yang di-cancel di-`detach()` (`execute.rs:1907`).
- `connections.db` WAL + `busy_timeout` + retry Busy; Corrupt → backup + toast; `preferences.json`
  rusak di-quarantine bukan ditimpa.
- Simpan grid atomik dengan `expected_rows` (`atomic.rs:43-131`, ada tes) + dialog "Review & Save".
- Autosave sesi 2 s, tulis atomik, dialog quit untuk tx terbuka/unsaved.
- Panic hook + crash log + "Open Logs Folder"; worker `catch_unwind`.
- Grid divirtualisasi; repaint idle hanya saat ada job; git child kill+wait; wasm fuel.
- Pesan error driver ditampilkan utuh dengan "Go to error".

---

## 7. Rencana prioritas

### Gelombang 1 — "tidak ada lagi yang hilang diam-diam" (S, ~1 minggu)
1. Hapus tiga fallback-ke-memori (S1, S2, S10) → toast error, dialog tetap terbuka.
2. Helper `Tabular::report_persist_error(ctx, e)` (= `error!` + toast dedup per sesi); terapkan ke
   prefs (S6), history (S8), secrets fallback (S4/S5 via enum balik), fallback DB lokal (S3).
3. Simpan grid: tolak bila pool belum siap (D7); alamat hasil job dengan `tab.id` (S9).
4. `ImportSummary.errors` + `eprintln!` → `log::warn!` (S7).

### Gelombang 2 — "tidak ada baris salah" (M, 1-2 minggu)
5. Hapus heuristik "id" (D1); SQLite pakai `rowid`; tanpa kunci → tolak (D3). Tes: tabel tanpa PK,
   kolom berquote.
6. Satukan `quote_identifier` (D8 + 9 duplikat) dan `split_statements` (4 duplikat) di
   `schema_objects`/`query_tools`.
7. Editor memakai `safety_guard::analyze_safety` + `agent::access` is_destructive (D4, D5); toggle per
   koneksi Read-only / Confirm destructive, default aktif untuk Production (D6).
8. Kolom biner/BIT/array non-editable atau literal per dialek; `NULL` dalam array PG; SQLite NUMERIC
   via TEXT (D2, D11, D12). MsSQL: buang koneksi saat timeout tx, `rows_affected` (D9, D10).
9. Konfirmasi ketik-nama untuk restore/transfer/drop DB yang menimpa (D13); prompt tx saat tutup tab
   (D14).

### Gelombang 3 — "tidak pernah beku" (M)
10. `FetchFolderItems`/`FetchObjectDdl` sebagai `BackgroundTask` + "Loading…" di tree (C1, C2);
    `metadata/*` menerima pool yang ada.
11. Satu runtime `OnceLock<Arc<Runtime>>`; clippy `disallowed_methods` untuk `Runtime::new` dan
    `thread::spawn` di luar whitelist (C4).
12. Akses `connections.db` dari UI ke worker; polling approval ke worker (C3); prefs ke writer thread
    (C6); dialog quit cek `jobs.active` (C8); toast crash-log di startup (C10).
13. Lint CI: `grep block_on src/window_egui/*.rs src/driver_*.rs` harus nol kecuali `*_jobs.rs`/`init.rs`.

### Gelombang 4 — "mudah dipahami" (M-L)
14. Menu bar `File/Edit/View/Database/Project/Tools/Help`; gear hanya Account+Preferences (U1, U2);
    palette dibangun dari menu yang sama.
15. Label sidebar penuh (U3); glosarium istilah (U4, U7); judul dialog = label menu (U10).
16. Sample DB di desktop + empty state aktif (U5); terjemahkan string Indonesia + tes CI (U6).
17. `style::confirm_dialog`, kurangi 6 mekanisme notifikasi menjadi toast + modal fatal (U9).
18. Shortcut ke `keymap.rs`, label per platform (U11); emoji → `ICON_*` + tes glyph (U12); elipsis
    seragam (U13); Preferences dirapikan (U8).

### Gelombang 5 — struktur & CI (L, berkelanjutan)
19. CI: hapus `continue-on-error` di fmt/deny/windows; GitHub services (postgres, mysql, redis,
    mongo, mssql) untuk tes driver nyata; tes `connection/metadata/`.
20. Pecah `Tabular` (575 field) menjadi sub-state per fitur (pola `GitUiState`, `HttpRepoUi` sudah
    ada); pecah 7 fungsi >1000 baris mulai dari `render_advanced_editor`, `render_diagram`,
    `render_tree_node_with_table_expansion`.
21. Reorganisasi `diagram_*` → `src/diagram/`, `http_*` → `src/http/`, `editor*` → `src/editor/`;
    pecah `models/structs.rs`. Pertimbangkan workspace multi-crate (core headless vs UI) agar aturan
    "agent tidak depend ke window_egui" ditegakkan compiler.
22. Ganti raw SQLite FFI dengan sqlx (−20 unsafe); `// SAFETY` untuk 39 situs objc2; bersihkan 42
    `allow(dead_code)`, `colorful`, `.idea/`, `.xcuserstate`; arsipkan 3 `*_PLAN.md` tanpa kode;
    perbaiki "115k lines" di AGENTS.md; `flexurio_import` → plugin.

---

## 8. Ukuran keberhasilan

- Nol jalur "simpan gagal tapi UI tampil sukses" (grep `Fallback to in-memory` = 0).
- Nol `block_on` di `window_egui/*.rs` dan `driver_*.rs` di luar whitelist (lint CI).
- Nol `Runtime::new()` di luar `init.rs`/`agent/cli.rs`.
- Satu `quote_identifier`, satu `split_statements`.
- Tes integrasi driver untuk 5 engine jaringan hijau di CI; `continue-on-error` = 0.
- Semua fitur utama dapat dicapai dari menu bar dan palette; nol string Bahasa Indonesia di UI; nol
  emoji sebagai ikon.

---

## 9. Status implementasi (2026-10-26)

Dikerjakan per gelombang, satu commit per gelombang di `main`.

| Gelombang | Commit | Hasil |
|---|---|---|
| 1 — tidak ada yang hilang diam-diam | `7380879` | S1–S12 selesai. Tambahan: restore `connections.db` korup tidak lagi menulis password plaintext dan kolom SSL/jump host tidak hilang. |
| 2 — tidak ada baris salah | `b513cee` | D1–D14 selesai; D15 sebagian (grid MongoDB/Redis dinonaktifkan; `NO_BACKSLASH_ESCAPES` MySQL belum diverifikasi). Tambahan: generator SQL grid menolak, tidak lagi melewati edit diam-diam. Mode Safety per koneksi (`connection_safety.rs`). Satu lexer statement SQL. |
| 3 — tidak pernah beku | `0cffefc` + gelombang 5 | C1, C3 (polling approval), C4, C6, C7, C8 selesai; C2: runtime per panggilan dihapus dan klik view/procedure kini async (gelombang 5). C10 ternyata sudah ada sebelumnya (`init.rs`). |
| 4 — mudah dipahami | `06a8c65` | U1–U3, U5–U14 selesai; U4 sebagian (Workspace→Collection, Remove→Delete). |
| 5 — struktur & CI | commit gelombang 5 | fmt CI wajib lolos (tree diformat), `cargo deny` wajib lolos, job `drivers` (PostgreSQL + MySQL), advisory keamanan diperbaiki (rustls, rustls-webpki, webpki-roots/certs, ctutils, zip 9 via calamine 0.37 / rust_xlsxwriter 0.101), ~1.100 baris kode mati dihapus, FFI SQLite mentah di `http_collection.rs` diganti sqlx, SAFETY untuk FFI backup, `colorful` dihapus, file IDE pribadi dilepas dari git, AGENTS.md diperbarui. |

Belum dikerjakan (disengaja, perlu keputusan atau terlalu berisiko dikerjakan paralel dengan
agent lain yang sedang mengubah repo):

- Memecah `Tabular` (575 field), memecah 7 fungsi >1000 baris, memindah `diagram_*`/`http_*`/
  `editor*` ke subdirektori, workspace multi-crate. Ini refactor besar yang menyentuh hampir
  semua file UI; sebaiknya dikerjakan bertahap di branch tersendiri saat tidak ada pekerjaan
  paralel.
- C3 sisa: `cache_data::block_on_cache` (17 pembacaan cache dari UI) masih sinkron; C5 init
  DB fallback di `update()`; lint CI "nol `block_on` di render" belum bisa dipasang karena masih
  ada pemakaian lama.
- Komentar SAFETY untuk 39 blok `unsafe` objc2 (platform macOS/iOS).
- Job Windows CI masih `continue-on-error` (belum diverifikasi di mesin Windows).
- Emoji lain yang ter-render benar (tercakup NotoEmoji) masih dipakai di label; hanya glyph yang
  terbukti kotak yang diganti.
- `flexurio_import.rs` → plugin; dokumen `*_PLAN.md` dibiarkan (keputusan pemilik).
- D12 ternyata sebagian keliru: SQLite menyimpan desimal presisi tinggi sebagai REAL, bukan
  TEXT; yang diperbaiki adalah INTEGER > 2^53 yang dulu dibaca lewat f64.
