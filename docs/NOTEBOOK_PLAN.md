# Rencana: Notebook & Data Science di Tabular

Status: **DRAFT, menunggu keputusan.** Belum ada kode yang ditulis.
Tanggal: 2026-09-18 · Branch saat diskusi: `improve/stability-dx`

Tujuan: Tabular bisa menyimpan pekerjaan sebagai query (`.sql`, sudah ada) atau notebook
(`.ipynb`), dan menjadi tool data science yang **sangat cepat** dan **mudah dipakai**, di
desktop, iPad, dan tablet Android.

---

## 1. Keputusan yang perlu diambil

Ini yang perlu dipikirkan. Sisa dokumen adalah bahan pertimbangannya.

| # | Keputusan | Rekomendasi | Alternatif | Bagian |
|---|---|---|---|---|
| 1 | Posisi produk | **SQL-first notebook, kompatibel `.ipynb`**; kernel Python/lainnya sebagai lapisan tambahan | Frontend Jupyter penuh (tidak realistis di egui: tidak ada webview untuk output HTML/JS/widget) | 2 |
| 2 | Engine analitik lokal | **DataFusion di semua platform** | DuckDB, dengan syarat lolos spike build iOS + Android; jika gagal kembali ke DataFusion | 4.A |
| 3 | Python di tablet | **Remote kernel lewat WebSocket** (Jupyter Server milik user dulu, gateway `tabular-server` belakangan) | Embed CPython di app (+50-100 MB, wheel numpy/pandas mobile terbatas, perlu estimasi terpisah) | 5 |
| 4 | Mulai dari mana | **Fase 0: tiga spike** (2-3 hari) sebelum investasi besar | Langsung Fase 1 | 7 |
| 5 | Urutan setelah core | Notebook UI lalu engine lalu profiler/chart lalu kernel | Profiler + chart di grid biasa dulu (paling cepat rilis, risiko terkecil, tidak bergantung pada notebook) | 7 |

---

## 2. Posisi produk

Kekuatan Tabular: native, cepat, koneksi langsung ke database. Posisi yang realistis lebih dekat
ke DataGrip / Hex / DuckDB UI daripada JupyterLab:

- Bahasa utama notebook adalah **SQL**. Hasil langsung masuk grid, chart, dan profil kolom.
- File tetap `.ipynb` standar (nbformat 4.5), jadi bisa dibuka di Jupyter atau VS Code. Itu
  jalan keluar bagi user yang butuh ekosistem penuh.
- Sel SQL **tidak lewat kernel**. Tetap memakai `QueryJob`, pool, SSH tunnel, dan
  `safety_guard` yang sudah ada, sehingga jalan di semua platform termasuk iPad.
- Kernel (Python, Rust/evcxr, R, Julia) hanya untuk sel non-SQL.

Batasan yang harus dikomunikasikan ke user: output HTML, JavaScript, dan ipywidgets tidak bisa
dirender. Yang didukung: `text/plain`, `text/markdown`, `image/png`, tabel, error/traceback.
Label fitur: "SQL Notebook (.ipynb compatible)".

---

## 3. Temuan codebase yang menentukan desain

| Temuan | Sumber | Dampak |
|---|---|---|
| Hasil query adalah `Vec<Vec<String>>`, semua string | `src/connection/types.rs:94-114`, `src/models/structs.rs:719-721` | **Hambatan terbesar untuk kecepatan.** Statistik, chart numerik, dan transfer ke Python butuh kolom bertipe |
| Grid sudah memvirtualisasi baris (top spacer + `skip(first_row)`) | `src/data_table/render_data.rs:660-670` | Cocok dengan Arrow: hanya baris terlihat yang diformat ke string |
| Query tersimpan berupa file `.sql` dengan metadata `-- tabular: connection_id=` | `src/sidebar_query.rs:14-37` | Notebook mengikuti pola sama: file di query dir, metadata koneksi di `metadata.tabular` |
| `QueryTab` mengasumsikan satu editor dan satu result set | `src/models/structs.rs:704` | Perlu tipe tab baru `NotebookTab`, jangan dipaksakan ke `QueryTab` |
| `editor.rs` 10,5k baris, mengasumsikan satu instance | `src/editor.rs` | Risiko terbesar UI. Solusi: hanya sel aktif yang memakai editor hidup |
| Bridge async ke UI sudah ada: `std::sync::mpsc` + `ctx.request_repaint()` | `src/connection/execute.rs:192`, `src/window_egui/app_impl.rs` | Bridge kernel meniru pola ini |
| Eksekusi headless lewat `QueryJob` | `src/connection/execute.rs` | Eksekusi sel memakai pipeline ini, ditambah `cell_id` |
| `egui_commonmark` sudah ada | `Cargo.toml` | Sel markdown tanpa dependency baru |
| Belum ada library chart | grep `egui_plot` kosong | Perlu `egui_plot` yang cocok dengan egui 0.36 (terbaru di crates.io 0.37) |
| Lapisan MCP headless | `src/agent/` | Tool notebook untuk agent AI |
| Plugin wasm via `wasmi` (interpreter) | `src/plugin_runtime/` | Pyodide/WASM Python tidak layak: terlalu lambat |
| Build iPad sudah matang; metrik sentuh tersedia | `build_ipad.md`, `src/window_egui/device_profile.rs` | UI notebook bisa touch-first sejak awal |
| **Build Android belum ada** (hanya beberapa baris `cfg(target_os = "android")`) | grep di `src/` | Prasyarat terpisah, bukan bagian pekerjaan notebook |
| `tokio-tungstenite` sudah di tree (fitur `collab`); `tabular-server` memakai axum + `ws` | `Cargo.toml` kedua repo | Transport WebSocket kernel dan gateway kernel punya pijakan |

### Pola yang diikuti

| Kategori | Sumber | Pola |
|---|---|---|
| Penamaan | `src/obsidian.rs`, `src/sidebar_query.rs` | Modul headless `src/notebook/` (`model.rs`, `ipynb.rs`, `exec.rs`, `kernel/`); UI di `src/window_egui/notebook_view.rs` |
| Error | `QueryExecutionError`, `AgentError` | Enum `thiserror` baru `NotebookError`; tanpa `unwrap()` di jalur I/O |
| Logging | `log::warn!("[AGENT] ...")` | Tag `[NOTEBOOK]`, `[KERNEL]` |
| Akses data | `export_all_data_payload`, `src/agent/core.rs` | Fungsi headless menerima `&Path` / `&ConnectionConfig`, tidak pernah `&mut Tabular` |
| Test | test di `src/connection/execute.rs` | Inline `#[cfg(test)]`, pool SQLite in-memory; fixture ipynb di `tests/fixtures/` |
| Komentar | `AGENTS.md` | Komentar kode Bahasa Indonesia, teks UI English |

---

## 4. Pilihan teknologi: pros dan cons

Versi crate dicek di crates.io pada 2026-09-18.

### A. Engine analitik lokal

Gunanya: query hasil sel lain tanpa bolak-balik ke server (`SELECT ... FROM sales` di mana
`sales` adalah hasil sel 1), query file CSV/Parquet/JSON, dan join lintas sumber (hasil sel
Postgres digabung dengan hasil sel MySQL).

| | **DataFusion** 55.x | **DuckDB** 1.x (bundled) | **Polars** 0.55 |
|---|---|---|---|
| Kecepatan | Sangat cepat; sedikit di bawah DuckDB pada join/agregasi kompleks | Tercepat, optimizer paling matang | Sangat cepat untuk operasi DataFrame |
| SQL | Baik, mirip Postgres, kurang kaya | Paling lengkap (PIVOT, ASOF JOIN, `read_parquet('*.parquet')`) | SQL terbatas; API utamanya DataFrame |
| Build | **Rust murni**: jalan di semua target termasuk iOS dan Android | **C++**: compile lama, belum terbukti lewat `duckdb-rs` untuk xcframework iOS dan NDK, berat di 6 target packaging desktop | Rust murni, compile paling lama |
| Ukuran binary | +15-25 MB | +25-35 MB per slice | +20-30 MB |
| Arrow | Native, memory model-nya memang Arrow | Zero-copy lewat C Data Interface | Kompatibel, perlu konversi |
| Dikenal user data science | Rendah | Tinggi | Tinggi (API DataFrame) |
| Stabilitas API | Major baru kira-kira tiap bulan, perlu di-pin | Stabil | Pre-1.0, sering berubah |

**Rekomendasi: DataFusion di semua platform.** Alasan utamanya konsistensi notebook antar
perangkat, bukan sekadar build. Jika desktop memakai DuckDB dan tablet memakai DataFusion,
ada dua dialek SQL: notebook yang ditulis di Mac bisa gagal saat dibuka di iPad. Satu engine
berarti notebook yang sama jalan sama di mana pun.

Yang dikorbankan: fitur SQL khas DuckDB dan keakrabannya di kalangan data science. Mitigasi
sebagian: di desktop user tetap bisa `import duckdb` dari sel Python.

Jika tetap ingin DuckDB: jadikan spike sebagai gerbang. DuckDB harus berhasil build untuk
`aarch64-apple-ios`, simulator, dan `aarch64-linux-android`. Satu saja gagal, pakai DataFusion.

Polars tidak direkomendasikan sebagai engine karena Tabular adalah tool SQL.

### B. Klien kernel

| | **`jupyter-zmq-client` + `zeromq`** | **`jupyter-websocket-client`** | Helper stdio custom | Pyodide/WASM |
|---|---|---|---|---|
| Fungsi | Kernel lokal lewat ZeroMQ | Kernel remote lewat Jupyter Server | Subprocess Python sendiri | Python di wasm |
| Platform | Desktop non-MAS | **Semua**, termasuk iPad/Android | Desktop non-MAS | Semua |
| Pros | Semua kernel (ipykernel, evcxr, IRkernel, Julia), Rust murni tanpa libzmq, dipakai Zed, interrupt/completion sudah ada di protokol | Sama, plus bisa menjalankan notebook di mesin GPU atau server | Sederhana | Tanpa instalasi |
| Cons | Siklus hidup proses kernel diurus sendiri (spawn, connection file, 5 port, kill saat tab ditutup atau app crash); interrupt di Windows perlu diuji; zmq.rs belum sematang libzmq | Butuh server; perlu reconnect saat app di-suspend | Hanya Python, semua dibangun sendiri | `wasmi` interpreter, terlalu lambat, tanpa numpy native |
| Keputusan | **Pakai (desktop)** | **Pakai (semua platform)** | Tidak | Tidak |

Catatan: crate `runtimelib` sudah **deprecated**, diganti nama menjadi `jupyter-zmq-client`
(1.0.1). Keluarga yang sama: `jupyter-protocol` 2.0.2 (tipe pesan), `jupyter-websocket-client`
2.0.0, `nbformat` 3.0.0. Pin versi minor seperti `rmcp` dan `mssql-client`.

### C. Transfer data antara Tabular dan kernel

| | **File Arrow IPC di cache dir** | Parquet | CSV | Shared memory / Arrow Flight |
|---|---|---|---|---|
| Kecepatan | Sangat cepat, bisa mmap, tanpa parsing | Cepat, encode/decode makan CPU | Lambat, tipe hilang | Tercepat |
| Kompleksitas | Rendah | Rendah | Terendah | Tinggi, spesifik platform |
| Keputusan | **Pakai** (kernel lokal) | Untuk export user; untuk kernel remote data dikirim lewat upload | Tidak | Berlebihan untuk sekarang |

Untuk kernel remote, file lokal tidak terlihat oleh kernel. Data dikirim lewat Contents API
Jupyter Server atau di-inline untuk hasil kecil. Perlu batas ukuran.

### D. Format notebook

| | **Crate `nbformat`** | serde tulis tangan |
|---|---|---|
| Pros | Bertipe, dirawat tim runtimed, dukung cell id v4.5 | Kontrol penuh, passthrough field tak dikenal terjamin |
| Cons | Harus dipastikan round-trip tidak membuang metadata asing | Lebih banyak kode dan edge case |
| Keputusan | **Coba dulu**; jika test round-trip fixture gagal, pakai passthrough `serde_json::Value` | Cadangan |

### E. Chart

| | **`egui_plot`** | plotters ke texture | matplotlib lewat kernel |
|---|---|---|---|
| Pros | Native, pan/zoom 60fps, tooltip hover, tanpa proses, jalan di tablet | Tipe chart statis lebih kaya | Apa pun yang user mau |
| Cons | Tipe chart terbatas (heatmap/box digambar sendiri); **versi harus cocok dengan egui 0.36** | Tidak interaktif | Butuh kernel, output PNG statis |
| Keputusan | **Utama**. Downsample (LTTB) jika lebih dari 50k titik | Tidak perlu | Otomatis tersedia |

### F. Bridge UI dan worker

| | **tokio unbounded mpsc + `try_recv` + `ctx.request_repaint()`** | `Arc<Mutex<State>>` bersama |
|---|---|---|
| Pros | Sama dengan pola yang sudah ada, tanpa lock di render thread | Sederhana |
| Cons | Perlu coalescing | Lock contention menyebabkan frame drop |
| Keputusan | **Pakai** | Tidak |

Aturan bridge:
- UI ke worker: `UnboundedSender<KernelCommand>` (`Execute{cell_id, code}`, `Interrupt`,
  `Restart`, `Shutdown`). `send()` tidak memblok, aman dipanggil dari frame egui.
- Worker ke UI: `try_recv()` di `update()`. **Jangan pernah** `blocking_recv` di render thread.
- Worker memegang clone `egui::Context` dan memanggil `request_repaint()` tiap pesan iopub
  datang. Tanpa itu output streaming baru muncul saat user menggerakkan mouse.
- Coalescing: jika kernel mencetak 10k baris, drain semua per frame dan batasi output per sel.
- Satu task tokio per kernel; `select!` atas iopub, shell reply, dan channel perintah. Output
  dicocokkan ke sel lewat `parent_header.msg_id`.

---

## 5. Arsitektur

```
NotebookTab (UI, window_egui/notebook_view.rs)
   │
   ├─ src/notebook/model.rs     Notebook, Cell, CellOutput
   ├─ src/notebook/ipynb.rs     baca/tulis nbformat 4.5
   │
   └─ trait CellExecutor
        ├─ SqlExecutor       QueryJob native                 semua platform
        ├─ LocalExecutor     DataFusion atas hasil sel/file  semua platform
        └─ KernelExecutor
             └─ trait KernelTransport
                  ├─ LocalZmq   jupyter-zmq-client           desktop non-MAS
                  └─ RemoteWs   jupyter-websocket-client     semua platform
```

Hasil sel disimpan sebagai Arrow `RecordBatch` (hanya di jalur notebook dan engine lokal;
jalur string lama untuk tab biasa tidak dibongkar).

Jembatan SQL ke Python: hasil sel bernama (`-- @name: sales`) ditulis ke Arrow IPC, lalu
Tabular mengirim `execute_request` senyap (`sales = pl.read_ipc(...)`) ke kernel.

### Matriks fitur per platform

| Fitur | Desktop | Desktop MAS | iPad | Android |
|---|---|---|---|---|
| Notebook SQL + Markdown, `.ipynb` | Ya | Ya | Ya | Ya* |
| Engine lokal (referensi antar sel, CSV/Parquet) | Ya | Ya | Ya | Ya* |
| Chart, profiler, parameter, cache | Ya | Ya | Ya | Ya* |
| Kernel **remote** (WebSocket) | Ya | Ya | Ya | Ya* |
| Kernel **lokal** (ZMQ) | Ya | Tidak | Tidak | Tidak |
| "Set up Python" lewat `uv` | Ya | Tidak | Tidak | Tidak |

\* setelah build Android ada (prasyarat terpisah).

Kenapa kernel lokal tidak bisa di tablet: sandbox iOS melarang `fork/exec`; Android tidak
punya Python sistem, dan kebijakan App Store (2.5.2) serta Play Store melarang mengunduh lalu
menjalankan executable.

Sumber kernel remote untuk tablet:
1. **Jupyter Server / JupyterHub milik user** (URL + token). Langsung bisa, tanpa pekerjaan server.
2. **`tabular-server` sebagai gateway kernel.** Paling cocok sebagai produk (login di iPad,
   Python langsung jalan), tapi besar dan sensitif: eksekusi kode arbitrer di server berarti
   isolasi container dan kuota per user. Fase terpisah setelah opsi 1 terbukti.

---

## 6. Kunci kecepatan dan kemudahan

### Kecepatan (urut dampak)
1. **Arrow kolumnar** sebagai format hasil sel. Menghindari jutaan alokasi `String`; profiler
   dan chart membaca buffer numerik langsung.
2. **Engine lokal jadi fondasi**, bukan fitur belakangan. Referensi antar sel jadi instan.
3. **Cache hasil per sel.** Kunci = hash(query + parameter + koneksi + database), disimpan
   sebagai Arrow IPC. Buka ulang notebook langsung tampil. Wajib ada badge "stale" dan tombol
   refresh supaya user tidak tertipu data lama.
4. **Prewarm kernel.** Start kernel di background saat notebook dibuka (startup ipykernel
   sekitar 1-2 detik), plus import senyap library umum.
5. **Virtualisasi sel.** Sel di luar viewport hanya `allocate_space` dengan tinggi terakhir.
   Sel tidak aktif dirender sebagai galley ter-cache; hanya satu editor hidup. Cache
   `egui_commonmark` dan texture chart.
6. **Downsample chart** dan batas baris output per sel.

### Kemudahan
- **"Set up Python" satu tombol** (desktop non-MAS): unduh `uv` dengan verifikasi checksum,
  lalu `uv venv` + `uv pip install ipykernel pyarrow polars` di app data dir. Opt-in. Kernel
  conda/venv yang sudah ada tetap terdeteksi lewat kernelspec.
- Seret tabel dari sidebar ke notebook: sel `SELECT * ... LIMIT 100` dibuat dan dijalankan.
  Di layar sentuh: tekan lama, lalu "Insert into notebook".
- Tombol "Chart" dan "Profile" di tiap hasil membuat sel baru tanpa konfigurasi manual.
- Pemilih bahasa per sel (SQL / Python / Markdown); tanpa magic `%%sql`.
- Variable explorer: daftar hasil bernama (`sales: 120k baris x 8 kolom`) dan variabel kernel.
- Aksi notebook masuk ke command palette yang sudah ada (`src/quick_open.rs`).
- Shortcut bawaan Jupyter (Shift+Enter, A/B, DD, M/Y), plus Run All dan Run Above.
- Parameter `{{start_date}}` dengan form input kecil di atas notebook.
- Konversi `.sql` ke notebook; export notebook ke `.sql` / `.md` / `.html`.
- Pesan error kernel tidak ditemukan mengarah langsung ke tombol setup.

### Khusus tablet
| Topik | Penanganan |
|---|---|
| Memori | iPadOS mematikan app yang melewati batas (jetsam). DataFusion diberi memory pool terbatas dengan spill ke disk; batas per perangkat lewat `DeviceUiMetrics`; batas baris per sel lebih kecil |
| Backgrounding | WebSocket kernel putus saat app di-suspend: auto-reconnect, `kernel_info`, sinkron ulang output |
| Sentuh | Tombol Run, tambah sel, pemilih bahasa di tiap sel dengan ukuran `min_touch_size`; urut ulang sel lewat handle atau tombol naik/turun |
| Keyboard layar | Sel aktif auto-scroll ke atas keyboard; pakai ulang penanganan di editor query |
| File | `rfd` tidak ada di iOS. Notebook di direktori Documents app, tampil di app Files (`UIFileSharingEnabled`, `LSSupportsOpeningDocumentsInPlace`), daftarkan tipe dokumen `.ipynb` di Info.plist. Android lewat Storage Access Framework |
| Sync | Tulis di Mac, buka di iPad lewat vault sync yang sudah ada. Output dibuang sebelum sync secara default |

---

## 7. Fase implementasi

| # | Fase | Isi | Estimasi |
|---|---|---|---|
| 0 | **Spike** | (a) round-trip `nbformat` dengan fixture Jupyter asli; (b) build DataFusion untuk iOS + uji batas memori di iPad fisik; (c) kernel lewat WebSocket ke Jupyter Server, `print` dalam loop ter-stream ke label egui, interrupt jalan. Tiap spike di branch terpisah | 2-3 hari |
| 1 | Core | `src/notebook/model.rs`, `ipynb.rs`; Arrow sebagai format hasil sel; koneksi disimpan dengan **nama** (bukan id, bukan config); batas baris output tersimpan (default 200) | 3-4 hari |
| 2 | Notebook UI | `NotebookTab`, ikon di sidebar Queries, satu editor hidup + galley ter-cache, grid mini per sel (pakai ulang `data_table/render_data.rs`), eksekusi lewat `QueryJob` dengan `(tab_id, cell_id)`, `safety_guard` per sel, session restore, **touch-first sejak awal** | 8-12 hari |
| 3 | Engine lokal | DataFusion, referensi antar sel, query CSV/Parquet/JSON, cache hasil, batas memori per perangkat | 5-8 hari |
| 4 | Profiler + chart | Profil kolom (count, null %, distinct, min/max/mean/median/stddev, top-k, histogram mini), opsi profil seluruh tabel lewat SQL agregat per dialek; sel chart `egui_plot`; parameter; helper sampling. Berlaku juga untuk tab query biasa | 5-7 hari |
| 5a | Kernel remote | Trait `KernelTransport` + `RemoteWs`; pemilihan MIME `image/png` lalu `text/markdown` lalu `text/plain`; prompt "trust notebook" | 5-7 hari |
| 5b | Kernel lokal | `LocalZmq`, deteksi kernelspec, siklus hidup proses, setup `uv`, prewarm, jembatan Arrow IPC. Gating `#[cfg(not(target_os = "ios"))]` + cek runtime MAS (pola `ai_cli_settings.rs`) | 5-7 hari |
| 6 | MCP / AI / sync | Tool MCP `notebook_list`, `notebook_read`, `notebook_add_cell`, `notebook_run_cell` (lewat `classify.rs` + safety guard); AI "explain result", "generate chart", "tulis sel berikutnya"; ringkasan ke vault Obsidian; registrasi tipe dokumen iOS | 3-4 hari |
| 7 | (opsional, besar) | Gateway kernel di `tabular-server` | estimasi terpisah |
| — | Prasyarat terpisah | Build Android | estimasi terpisah |

Fase 1-4 sudah menjadi rilis yang utuh di semua platform ("SQL Notebook + engine lokal +
profiler + chart") tanpa kernel sama sekali.

Semua dependency berat (Arrow, DataFusion, klien kernel) di belakang fitur cargo `notebook`.

---

## 8. Risiko

| Risiko | Tingkat | Mitigasi |
|---|---|---|
| `editor.rs` belum multi-instance | **TINGGI** | Satu editor hidup di sel aktif; sel lain teks ter-highlight read-only |
| Output tersimpan membocorkan data produksi/PII ke git atau sync | **TINGGI** | Batas baris, toggle "Clear outputs on save", output dibuang saat sync secara default |
| Run All menjalankan DML/DDL | **TINGGI** | Safety guard per sel; Run All berhenti dan minta konfirmasi pada statement destruktif |
| Membuka notebook orang lain berarti eksekusi kode arbitrer | SEDANG | Tidak pernah auto-run; prompt "trust notebook" |
| Round-trip merusak notebook Python milik user | SEDANG | Pertahankan field tak dikenal; test fixture |
| Memori di tablet (jetsam) | SEDANG | Memory pool terbatas + spill; batas per perangkat |
| Siklus hidup kernel lokal, kernel zombie, interrupt Windows | SEDANG | Uji di CI Windows; kill saat tab ditutup; pembersihan saat start |
| zmq.rs belum sematang libzmq | SEDANG | Hanya kernel lokal di `127.0.0.1`; kernel remote lewat WebSocket |
| Waktu compile dan ukuran binary naik (perkiraan +30-40 MB) | SEDANG | Fitur `notebook`; pantau di CI |
| Unduhan `uv` | SEDANG | Opt-in, verifikasi checksum, nonaktif di MAS dan mobile |
| API DataFusion dan crate Jupyter cepat berubah | RENDAH-SEDANG | Pin versi minor |
| User mengira "dukungan Jupyter" berarti HTML dan widget | RENDAH | Penamaan fitur dan dokumentasi batasan |

---

## 9. Ide lanjutan (setelah fase di atas)

- Snapshot hasil dan diff antar dua eksekusi query yang sama.
- Eksekusi notebook terjadwal dengan export laporan HTML/PDF.
- Data dictionary: deskripsi kolom dari vault Obsidian tampil di profiler.
- Pivot table di grid; matriks korelasi dan penanda outlier di profiler.
- ADBC / Arrow Flight untuk warehouse kolumnar (BigQuery, Snowflake, ClickHouse).
- Embed CPython untuk Python offline di tablet, jika permintaannya terbukti tinggi.
