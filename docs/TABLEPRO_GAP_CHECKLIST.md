# Checklist Gap Fitur: Tabular vs TablePro

Tanggal: 2026-09-29 · Tabular 1.1.3 · TablePro 0.76.1 (28 Sep 2026)

Sumber TablePro: https://tablepro.app, https://docs.tablepro.app (indeks `llms.txt`, changelog),
https://github.com/TableProApp/TablePro. Sisi Tabular diverifikasi dari README, `docs/`, dan
pencarian kode `src/` (bukan uji runtime).

## Cara pakai

- Centang `[x]` fitur yang ingin diterapkan. Fitur yang tidak dicentang dilewati.
- Kolom **P** = prioritas usulan (1 = paling berdampak). **Usaha**: S (< 2 hari), M (2-5 hari),
  L (1-2 minggu), XL (> 2 minggu).
- **Status**: Belum = tidak ada jejak di kode; Sebagian = ada tapi lebih sempit dari TablePro.
- Bagian **Pilihan saya** di bawah untuk catatan urutan atau batasan tambahan.

---

## A. Keamanan eksekusi

- [ ] **A1. Safe Mode per koneksi** · P1 · M · Belum
  Level: Silent, Alert, Alert Full, Safe Mode, Safe Mode Full, Read-Only. Auto-elevasi bila nama
  koneksi mengandung `prod`. Konfirmasi ketik "I understand" untuk statement destruktif.
  Fondasi: `src/safety_guard.rs`, `src/agent/classify.rs`, enforce di `src/connection/execute.rs`,
  field baru di `ConnectionConfig` (`src/models/structs.rs`).
- [ ] **A2. Execution log lokal hash-chained** · P4 · S · Sebagian
  Tabular baru mencatat query agent di history. Tambah digest per operasi (backup, drop, kill).
- [ ] **A3. Managed Safe Mode via configuration profile (fleet)** · P5 · S · Belum
  Baca file kebijakan dari `TABULAR_DATA_DIR` atau MDM; nilai per koneksi tidak bisa diturunkan.

## B. Data grid & hasil query

Status 2026-09-29: diterapkan di `src/data_table/grid_{model,prefs,state,ui}.rs` + kait di
`render_data.rs`, `spreadsheet.rs`. Belum diuji manual terhadap database sungguhan.

- [x] **B1. Change tracking + review SQL sebelum simpan** · P1 · M · Selesai
  Sel diubah ditandai kuning (tooltip nilai asli), baris baru hijau, baris hapus merah + coret.
  ⌘S / "Review & Save" membuka dialog SQL; Discard/Esc membatalkan. SQL yang ditampilkan adalah
  SQL yang dieksekusi (nilai di-escape inline, bukan bind parameter). Mode manual commit: dialog
  memberi peringatan bahwa simpan grid berjalan di koneksi terpisah (ADR 0001).
- [x] **B2. Undo/redo di grid** · P2 · S · Selesai
  ⌘Z / ⌘⇧Z saat grid fokus, tombol di action bar. Commit sukses mengosongkan riwayat.
- [x] **B3. Data Rewind / Restore Previous Values setelah commit** · P3 · M · Selesai (sesi)
  Riwayat commit per sesi (maks 50) + SQL pembalik yang direview sebelum dijalankan. Insert dengan
  kunci auto-generated tidak bisa dibalik (dicatat di catatan entri).
- [x] **B4. Find in results + Search All Rows (server-side)** · P1 · S · Selesai
- [x] **B5. Filter by cell (context menu)** · P2 · S · Selesai
- [x] **B6. Saved filters per tabel** · P3 · S · Selesai
  Tabel `grid_table_prefs` di `connections.db`; filter bertanda default diterapkan saat tabel dibuka.
- [x] **B7. Highlight rules (warna baris/sel berdasar nilai)** · P2 · M · Selesai
- [x] **B8. Invisible characters indicator** · P4 · S · Selesai
- [x] **B9. Hide column + Jump to Column (fuzzy)** · P3 · S · Selesai
  Hide per tab (sesi); Jump ⌘J dengan tipe + posisi.
- [x] **B10. Row as JSON dengan FK drilling (5 level)** · P3 · M · Selesai
  Butuh FK di cache (`foreign_key_cache`).
- [x] **B11. FK value picker saat mengedit kolom FK** · P3 · M · Selesai
  Lewat klik kanan sel FK -> "Pick from ...".
- [x] **B12. Default value menu saat insert** · P4 · S · Selesai
  NULL, '', DEFAULT, NOW(), UUID v4 (dibuat klien). Baris baru default ke DEFAULT.
- [ ] **B13. Column reorder (drag)** · P4 · M · Sebagian
  Drag header mengubah urutan tampilan (per tab, sesi). Urutan fisik: MySQL/MariaDB lewat
  `MODIFY COLUMN ... AFTER` dengan review. PostgreSQL/SQLite/SQL Server (rebuild tabel) tidak
  dikerjakan karena risiko kehilangan constraint/trigger/FK.

## C. Chart & visualisasi

- [x] **C1. Result charts (bar, line, area, scatter)** · P1 · M · Selesai
  Tab "Chart" di bar paginasi hasil, `egui_plot`; pilih X/Y (multi seri), agregasi
  None/Sum/Avg/Count/Min/Max, deteksi kolom numerik/tanggal/teks. `src/result_chart.rs`.
- [x] **C2. EXPLAIN bar chart (self cost, time, rows)** · P3 · S · Selesai
  View "Metrics" di profiler; klik bar membuka node di graph. `src/query_profiler/compare_view.rs`.
- [x] **C3. EXPLAIN Compare (diff dengan run sebelumnya, pin plan)** · P3 · M · Selesai
  Tabel `explain_plan_history` di `connections.db` per (koneksi, fingerprint query), 20 plan
  tak di-pin per query; view "Compare" dengan delta cost/time/rows per node dan deteksi
  perubahan bentuk plan. `src/query_profiler/{compare,history}.rs`.
- [x] **C4. Map view untuk kolom geometry/PostGIS** · P5 · L · Selesai
  Tab "Map" muncul bila ada kolom geometry (EWKB PostGIS, WKB MySQL, WKT/EWKT, GeoJSON);
  `walkers` + basemap OpenStreetMap (bisa dimatikan, cache di `TABULAR_DATA_DIR/tile_cache`),
  SRID 3857 dikonversi, SRID proyeksi lain digambar planar. `src/geo_map/`.
  Belum: geometry native SQL Server (format biner CLR), pakai `.STAsText()`.

## D. Editor SQL

- [ ] **D1. Named result tabs dari komentar leading** · P2 · S · Belum
  `-- Total per bulan` di atas statement jadi judul tab hasil.
- [ ] **D2. Result chooser di status bar + Pin/Close result** · P2 · S · Sebagian
- [ ] **D3. Run split-button: Run All / Run Without Limit / Stop** · P2 · S · Sebagian
- [ ] **D4. Statement navigation (Ctrl+Cmd+Left/Right) + run-and-advance** · P2 · S · Belum
  Memakai `src/query_tools/statement_parser.rs`.
- [ ] **D5. Code folding (statement, CTE, subquery, BEGIN)** · P3 · L · Belum
  Editor custom `src/editor.rs`; perlu region map + hidden lines di galley.
- [ ] **D6. Vim mode** · P4 · XL · Belum
  Normal/Insert/Visual/Visual Line/Replace/Command, motions, text objects, register, marks, macro.
- [ ] **D7. Query parameters sebagai prepared statement** · P4 · M · Sebagian
  Saat ini substitusi teks (`substitute_query_parameters`). Kirim sebagai bind param per driver.
- [ ] **D8. SQL Files: buka .sql eksternal sebagai tab dengan deteksi perubahan luar** · P3 · S · Sebagian
- [ ] **D9. Row cap dinamis (default 10.000, maks 500.000) + stop fetch dini** · P3 · S · Sebagian
  `max_result_rows` ada; tambah UI per tab dan early termination stream.

## E. Tab, window, navigasi

- [ ] **E1. Recent tabs (Ctrl+Tab) + reopen closed tab** · P2 · S · Belum
  Simpan riwayat di `src/session_restore.rs`.
- [ ] **E2. Multi-window (drag tab keluar jadi window baru)** · P3 · L · Belum
  egui viewports; state `Tabular` harus bisa dibagi per viewport.
- [ ] **E3. Split panes editor/hasil vertikal/horizontal** · P3 · M · Belum
- [ ] **E4. Tab rows (multi-baris tab strip)** · P4 · S · Belum
- [ ] **E5. Favorites: star database/tabel, pin di sidebar** · P2 · S · Belum
- [ ] **E6. Recent connections + clear** · P3 · S · Belum
- [ ] **E7. Focus navigation antar pane via keyboard (View > Focus)** · P4 · S · Belum
- [ ] **E8. What's New window setelah update** · P4 · S · Belum
- [ ] **E9. Contextual tips (Open Quickly, preview tab)** · P5 · S · Belum

## F. Koneksi, tunnel, kredensial

- [ ] **F1. Credential profiles (username/password reusable)** · P1 · M · Belum
  Tabel baru di `connections.db` (`src/sidebar_database.rs`), secret tetap di `src/secrets.rs`.
- [ ] **F2. SSH profiles reusable** · P1 · S · Belum
  Bergantung F1 untuk pola penyimpanan.
- [ ] **F3. Import koneksi dari klien lain** · P1 · M · Belum
  TablePlus (Keychain), Sequel Ace, DBeaver (`data-sources.json`), DataGrip, Beekeeper (`app.db`),
  Navicat `.ncx`. Password mungkin tidak bisa diambil tanpa entitlement; minta isi ulang.
- [ ] **F4. Scan `.env` / `DATABASE_URL` / project folder jadi koneksi** · P2 · S · Belum
  Bisa memakai `src/repo_scan.rs`.
- [ ] **F5. SOCKS5 proxy dengan remote DNS** · P3 · M · Belum
- [ ] **F6. Tunnel Command (kubectl port-forward, AWS SSM, custom)** · P3 · M · Belum
  Tambah transport di `src/ssh_tunnel.rs`.
- [ ] **F7. Cloudflare Tunnel (cloudflared access)** · P4 · S · Belum
- [ ] **F8. Cloud SQL Auth Proxy** · P4 · S · Belum
- [ ] **F9. AWS IAM auth untuk RDS/Aurora + import instance dari AWS** · P3 · M · Belum
- [ ] **F10. Resolusi kredensial eksternal: 1Password, HashiCorp Vault, AWS Secrets Manager, shell command** · P3 · M · Sebagian
  Sekarang OS keychain + encrypted file.
- [ ] **F11. Identity file + SSH agent IdentitiesOnly, TOTP prompt** · P4 · S · Sebagian
- [ ] **F12. Connection URL paste (postgres://..., mysql://...) di form koneksi** · P2 · S · Belum
- [ ] **F13. Check connection (ping) opsional saat dipakai** · P4 · S · Belum
- [ ] **F14. Transport activity (throughput SSH/SOCKS di toolbar)** · P5 · S · Belum
- [ ] **F15. Redis Sentinel mode** · P3 · S · Belum
  Standalone + cluster sudah ada.
- [ ] **F16. SQLite over SSH (remote file) + load extension saat connect** · P4 · M · Belum

## G. Skema & objek database

- [ ] **G1. Structure editor lengkap: FK, trigger, check constraint, generated column, DDL** · P2 · L · Sebagian
  Kolom + index sudah ada di `src/data_table/render_structure.rs`.
- [ ] **G2. Rename tabel/database/schema dari sidebar** · P2 · S · Belum
- [ ] **G3. Edit comment tabel/kolom** · P3 · S · Belum
- [ ] **G4. User-defined types (enum, composite, domain, range) di sidebar + inline edit** · P3 · M · Belum
- [ ] **G5. Materialized view: sidebar, DDL, refresh (concurrently)** · P3 · S · Belum
- [ ] **G6. Partition bounds + row count di sidebar** · P4 · S · Sebagian
- [ ] **G7. Toggle system databases/schemas** · P4 · S · Belum
- [ ] **G8. Table operations: maintenance (VACUUM/ANALYZE/OPTIMIZE), view management** · P3 · S · Sebagian
- [ ] **G9. PostgreSQL schema create/edit dengan ownership & privileges** · P4 · S · Belum
- [ ] **G10. Routine source viewer + export untuk semua engine** · P4 · S · Sebagian
- [ ] **G11. SQL Server: GO batch separator, PRINT output, per-result tabs** · P3 · M · Belum

## H. Import, export, transfer

- [ ] **H1. Data Files: buka CSV/TSV/JSON/XLSX/Parquet/compressed sebagai tabel tanpa import** · P1 · L · Belum
  Engine in-memory DataFusion (keputusan 2 di `docs/NOTEBOOK_PLAN.md`). Hindari DuckDB karena
  konflik `libsqlite3-sys` 0.37.
- [ ] **H2. Export file: Markdown, HTML, XML, NDJSON, Parquet native** · P2 · S · Sebagian
  `src/export.rs`; Parquet saat ini via Wasm plugin.
- [ ] **H3. Encrypted export (AES-256-GCM)** · P3 · S · Belum
  Pakai primitif `src/sync/vault_crypto.rs`.
- [ ] **H4. Import JSON dan XLSX (perluas CSV wizard)** · P2 · M · Sebagian
  `src/dialog.rs` (CSV Import Wizard).
- [ ] **H5. Export encoding picker + BOM; import UTF-16/Windows-1252** · P4 · S · Belum
- [ ] **H6. Max INSERT size untuk SQL export; index di post-data** · P4 · S · Belum
- [ ] **H7. Export any object (view, routine, trigger, type, privilege) dalam pohon** · P3 · M · Sebagian
- [ ] **H8. Transfer To: salin baris langsung antar koneksi terbuka** · P2 · M · Belum
- [ ] **H9. Copy To lintas engine dengan aproksimasi tipe** · P3 · L · Sebagian
  `src/dialog_copy_database.rs` sekarang satu engine.
- [ ] **H10. Backup/restore: mongodump, sqlpackage (MSSQL)** · P3 · M · Sebagian
  `src/backup_restore.rs` sudah pg_dump, mysqldump, sqlite.
- [ ] **H11. Data Compare & Sync (per tabel, filter, row limit, highlight, apply script)** · P3 · L · Belum
  Schema sync sudah ada di diagram.
- [ ] **H12. Saved comparisons (pasangan endpoint + opsi)** · P4 · S · Belum
  Bergantung H11.

## I. Query history, insight, monitoring

- [ ] **I1. Query Insights: most-run, slowest, increasingly slow** · P2 · M · Sebagian
  Data `query_history` + `src/autocomplete/usage.rs` sudah menyimpan hitungan; tambah durasi dan panel.
- [ ] **I2. Table load history 7 hari + query time breakdown (server/first row/transfer)** · P3 · S · Belum
- [ ] **I3. Server dashboard: metrik real-time (QPS, koneksi, buffer) + slow query** · P3 · M · Sebagian
  DBA monitor sudah processlist, lock tree, kill.
- [ ] **I4. Notifikasi OS untuk operasi panjang dengan threshold** · P3 · S · Sebagian
  Toast in-app ada (`src/window_egui/notifications.rs`).

## J. Saved queries & kolaborasi

- [ ] **J1. Version history saved query** · P2 · M · Belum
  Simpan revisi di `connections.db` atau sebagai file `.sql.N`.
- [ ] **J2. Git integration untuk folder query** · P3 · M · Belum
- [ ] **J3. Linked folders (folder .sql eksternal tampil di sidebar, watch perubahan)** · P2 · S · Belum
- [ ] **J4. Global folders lintas koneksi** · P4 · S · Sebagian
- [ ] **J5. Environment variables di query/koneksi (`{{var}}` per environment)** · P3 · M · Belum
  HTTP client mungkin sudah punya pola env; unifikasi.
- [ ] **J6. Team Catalog / Team Library (koneksi & query bersama)** · P4 · M · Sebagian
  Teams + vault E2E sudah ada; tambah katalog terkurasi.

## K. AI & MCP

- [ ] **K1. MCP permission level Ask / Edit / Agent per koneksi** · P1 · M · Sebagian
  Saat ini read-only total. `src/agent/mcp.rs`, `src/agent/classify.rs`.
- [ ] **K2. MCP scoped token + pairing PKCE + revocation + activity log** · P2 · L · Belum
  Diperlukan bila transport HTTP/SSE ditambahkan; stdio sekarang tanpa token.
- [ ] **K3. MCP resources (connections, schema, tables, history) + prompts (8 template) + subscriptions** · P2 · M · Belum
- [ ] **K4. Per-connection allowlist untuk klien MCP + "Forget"** · P3 · S · Belum
- [ ] **K5. Outside MCP servers: AI chat memanggil MCP eksternal dengan allowlist** · P3 · L · Belum
- [ ] **K6. Agent Mode: satu sesi AI kelola window + session history** · P3 · L · Sebagian
  Live edit tab sudah ada.
- [ ] **K7. Inline ghost-text suggestions di editor** · P2 · M · Belum
- [ ] **K8. Review with AI (dari editor bar / context menu)** · P3 · S · Belum
- [ ] **K9. Fix failed query sebagai diff** · P3 · S · Belum
- [ ] **K10. Provider tambahan: Gemini API, xAI, OpenRouter, Ollama, llama.cpp, MLX** · P3 · S · Sebagian
  OpenAI-compatible endpoint sudah mencakup sebagian (Ollama, OpenRouter).
- [ ] **K11. Perluasan MCP tools ke 40+ (write/DDL terkontrol, backup, kill, dsb.)** · P3 · L · Sebagian
  Bergantung K1.

## L. Engine database baru

Catatan: `libsqlite3-sys` dipin 0.37 (bundled); crate yang membawa SQLite sendiri bentrok.
Gunakan feature flag per driver seperti `collab`.

- [ ] **L1. ClickHouse (HTTP interface)** · P2 · M · Belum
- [ ] **L2. MariaDB/TiDB/OceanBase/Databend sebagai tipe koneksi di atas driver MySQL** · P3 · S · Sebagian
- [ ] **L3. Redshift/CockroachDB/PGlite/Turso sebagai tipe di atas driver PostgreSQL/SQLite** · P3 · S · Sebagian
- [ ] **L4. Cassandra / ScyllaDB (CQL)** · P3 · L · Belum
- [ ] **L5. Elasticsearch (REST, Query DSL console)** · P3 · M · Belum
- [ ] **L6. DuckDB (embedded)** · P3 · L · Belum · risiko konflik SQLite
- [ ] **L7. Oracle Database** · P4 · XL · Belum · butuh Instant Client / native protocol
- [ ] **L8. Snowflake** · P4 · L · Belum
- [ ] **L9. Google BigQuery** · P4 · L · Belum
- [ ] **L10. Amazon DynamoDB (PartiQL)** · P4 · L · Belum
- [ ] **L11. Kafka (topic browser, consumer lag, produce)** · P4 · L · Belum
- [ ] **L12. etcd** · P5 · M · Belum
- [ ] **L13. SurrealDB** · P5 · M · Belum
- [ ] **L14. libSQL / Turso remote** · P4 · M · Belum
- [ ] **L15. Cloudflare D1 / R2 SQL** · P5 · M · Belum
- [ ] **L16. Trino** · P5 · M · Belum
- [ ] **L17. Teradata, SAP HANA, Dameng DM8, Spanner, Typesense, Weaviate, Beancount** · P5 · XL · Belum
- [ ] **L18. Driver plugin API di Wasm runtime + plugin registry** · P3 · XL · Belum
  Memungkinkan komunitas menambah engine tanpa rebuild.
- [ ] **L19. MongoDB: mongosh JS shell, Extended JSON editor, nested filters, field rename di Structure** · P3 · L · Sebagian

## M. Platform, integrasi OS, lain-lain

- [ ] **M1. URL scheme `tabular://` (open connection, run query, import)** · P3 · M · Belum
- [ ] **M2. CLI opener `tabular open <url>` + integrasi ddev** · P3 · S · Belum
- [ ] **M3. Raycast extension** · P5 · M · Belum
- [ ] **M4. AppleScript dictionary** · P5 · L · Belum · macOS only
- [ ] **M5. iOS Shortcuts + Handoff** · P5 · L · Belum · Apple only
- [ ] **M6. Localization (KO, TR, VI, ZH, ID)** · P3 · L · Belum
  Perlu sistem string (mis. `fluent`) di seluruh UI.
- [ ] **M7. Touch ID / biometrik untuk membuka vault** · P4 · S · Belum
- [ ] **M8. Background update download + deferred install** · P4 · S · Sebagian
  Self-update sudah ada (`src/self_update.rs`).
- [ ] **M9. Managed updates via configuration profile** · P5 · S · Belum
- [ ] **M10. Connection colors di tab/toolbar per environment** · P3 · S · Belum
- [ ] **M11. iPad layout side-by-side stabil** · P3 · L · Sebagian
- [ ] **M12. Privacy page: daftar semua outbound request + toggle** · P4 · S · Belum

---

## Keunggulan Tabular yang tidak ada di TablePro (dipertahankan)

Windows/Linux native · HTTP client + AI + code export · E2E zero-knowledge sync lintas platform
dengan teams dan CRDT collab · Diagram dengan groups, notes, repo linking, Mermaid, LOD ·
Obsidian vault sebagai knowledge · Multi CLI agent chat (agy, claude, gemini) · Wasm plugin
exporter/ORM generator · Query data-flow diagram · Index check badge · Semantic history search ·
AST query optimizer · Two-way schema sync.

---

## Pilihan saya

Tulis urutan, batasan, atau catatan di sini, misalnya:

```
Mulai dari: A1, B1, B4, C1, F1, F2, F3
Tunda: L*, M4, M5
Catatan: ...
```
