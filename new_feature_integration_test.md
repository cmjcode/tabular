# Plan: Integration Test untuk Bug Data (Concurrency, DB Assertion, Data Check, MCP)

**Source**: Kasus nyata VOLTUNES 2026-10-29 (penerimaan stok dobel, saldo tidak sinkron)
**Complexity**: Large (multi-session, bisa dikerjakan per fase)

## Summary

Integration Testing Tabular sekarang hanya bisa mengirim request HTTP **berurutan**
dan memeriksa **respons HTTP**. Bug data yang paling mahal (double submit, race
condition, saldo tidak sinkron, data yatim) tidak terlihat dari respons dan hanya
muncul saat dua request berjalan bersamaan. Plan ini menambah empat kemampuan:

1. **Concurrent step**: kirim request yang sama N kali serentak, lalu assertion atas
   kumpulan responsnya.
2. **DB assertion**: langkah yang menjalankan query baca ke koneksi database lalu
   memeriksa hasilnya (termasuk menunggu proses async selesai).
3. **Data check**: kumpulan query invariant per project ("harus kosong") yang bisa
   dijalankan dari GUI, CLI, jadwal, dan MCP untuk mendeteksi data rusak di
   production tanpa menulis apa pun.
4. **MCP test tools**: agent bisa menjalankan suite dan data check, lalu membaca
   hasilnya.

## Latar belakang: kasus yang memicu

Audit DB VOLTUNES (`voltunes_chick`) menemukan 5 masalah yang lolos dari unit test
(sqlmock) dan dari suite Tabular:

| # | Masalah | Kenapa tidak tertangkap |
|---|---|---|
| 1 | Satu surat jalan diterima 2x dalam < 1 detik (4.750 → 9.500) | Hanya terjadi saat 2 request bersamaan; runner Tabular berurutan |
| 2 | Saldo ringkasan ≠ jumlah transaksi (pola 2x) | Ditulis goroutine async setelah respons; tidak ada assertion ke DB |
| 3 | Hapus penerimaan DOC meninggalkan stock-out aktif | Respons DELETE 200 OK; kerusakan hanya terlihat di tabel lain |
| 4 | Penerimaan terpakai melebihi isinya | Race antar pengeluaran + baca dari replica |
| 5 | Saldo negatif | Akibat #3 dan #4; hanya terlihat lewat query agregat |

Pola umumnya: **respons HTTP terlihat benar, datanya salah.** Untuk bug seperti ini
yang dibutuhkan adalah (a) beban serentak dan (b) pemeriksaan isi database, dan
keduanya belum ada di Tabular.

## Keterbatasan saat ini (dari kode)

| Area | Lokasi | Keadaan |
|---|---|---|
| Runner | `tabular-client/src/http_tests.rs` `run_suite` | Berurutan: Setup → Test → Cleanup, satu request per langkah |
| Assertion target | `http_tests.rs` `Assertion`, `read_target` | Hanya `status`, `time_ms`, `body`, `header:<n>`, `json:<path>` |
| Akses DB | `http_tests.rs` `check_db_residue` | Hanya cek sisa data berisi `run_id` setelah run (`--no-db-check`) |
| CLI | `agent/cli.rs` | `tabular test list`, `tabular test run` saja |
| MCP | `agent/mcp.rs` | Tool database & schema; **tidak ada** tool suite/test |
| Akses koneksi | `agent/access.rs` | Per koneksi `read_only` / `ask` / `edit` / `agent`; dipakai untuk gerbang baca |

## Goals

- Reproduksi race condition (double submit) dari suite, hasilnya deterministik
  (PASS/FAIL), bisa jalan di CI.
- Assertion ke isi database setelah request, termasuk efek async.
- Deteksi data rusak di production secara rutin, **read-only**.
- Agent (Claude via MCP) bisa menjalankan dan membaca hasil test tanpa GUI.

## Non-goals

- Load/performance testing (throughput, p99). Concurrent step dibatasi kecil (≤ 50).
- Menulis/memperbaiki data dari suite. DB assertion dan data check selalu baca saja.
- Mengganti unit test di repo aplikasi.

---

## F1. Concurrent step

### Model

Field baru opsional di `HttpTestStep`:

```rust
/// Kirim request langkah ini beberapa kali serentak.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub concurrency: Option<Concurrency>,

pub struct Concurrency {
    /// Jumlah request serentak, 2..=50.
    pub count: u32,
    /// Semua thread menunggu di barrier lalu mengirim bersamaan
    /// (default true). false = dikirim berurutan secepatnya.
    #[serde(default = "default_true")]
    pub barrier: bool,
    /// Ulangi seluruh burst sekian kali untuk menangkap race yang jarang.
    #[serde(default = "one")]
    pub rounds: u32,
}
```

### Assertion agregat

Target baru, dievaluasi atas seluruh respons burst:

| Target | Arti |
|---|---|
| `count:status=2xx` | jumlah respons berstatus 2xx (juga `count:status=409`, `count:status=4xx`) |
| `count:json:$.status=success` | jumlah respons yang path JSON-nya bernilai tertentu |
| `any:<target>` | lulus bila minimal satu respons memenuhi assertion biasa |
| `all:<target>` | lulus bila semua respons memenuhi |

Assertion biasa (tanpa prefix) pada langkah concurrent = `all:`.
`extract` diambil dari respons sukses pertama (urut waktu selesai).

### Runner

- `run_suite` menerima `send: &dyn Fn(RequestSpec) -> HttpClientResponse`. Untuk
  burst, `send` harus `Sync` (atau buat sender per thread). Pakai `std::thread::scope`
  + `std::sync::Barrier` agar semua request dilepas bersamaan.
- `StepResult` menyimpan daftar respons per instance (status, waktu, ringkasan body)
  untuk laporan; assertion agregat dievaluasi setelah semua selesai.
- Batas: `count ≤ 50`, `rounds ≤ 20`, timeout per request mengikuti setting yang ada.
- Cleanup: langkah Cleanup untuk langkah concurrent dijalankan untuk **setiap**
  data yang berhasil dibuat (extract per instance → `{{var#1}}`, `{{var#2}}`, …), atau
  cukup satu cleanup berbasis query bila tidak ada id.

### Contoh (kasus VOLTUNES #1)

```json
{
  "id": "terima_sj_dobel",
  "name": "Terima surat jalan yang sama 5x serentak",
  "phase": "test",
  "request": { "method": "POST", "url": "{{base_url}}/kandang-stock-ins",
               "body_text": "{\"warehouse_stock_out_detail_id\": {{sj_detail_id}}, \"total_in\": {{qty_kirim}}, ...}" },
  "concurrency": { "count": 5, "barrier": true, "rounds": 3 },
  "assertions": [
    { "target": "count:status=2xx", "op": "equals", "value": "1" },
    { "target": "any:json:$.message", "op": "contains", "value": "already been received" }
  ]
}
```

---

## F2. DB assertion step

### Model

Jenis langkah baru. `HttpTestStep` mendapat field `kind` (default `http`, kompatibel
dengan suite lama):

```rust
#[serde(default)]
pub kind: StepKind,            // Http | DbQuery

#[serde(default, skip_serializing_if = "Option::is_none")]
pub db: Option<DbQueryStep>,

pub struct DbQueryStep {
    /// Nama variabel environment yang berisi connection id, mis. "db_main".
    /// Bukan id literal, agar suite yang sama jalan di Dev/Staging.
    pub connection_var: String,
    #[serde(default)]
    pub database: Option<String>,
    /// SELECT saja. Variabel {{x}} diikat sebagai parameter, bukan disambung ke teks.
    pub sql: String,
    /// Ulangi query sampai semua assertion lulus atau timeout
    /// (untuk efek async seperti goroutine/queue setelah commit).
    #[serde(default)]
    pub wait_until_ms: Option<u64>,
    #[serde(default = "default_poll")]
    pub poll_ms: u64,           // default 250
}
```

### Assertion target untuk hasil query

| Target | Arti |
|---|---|
| `row_count` | jumlah baris |
| `scalar` | kolom pertama baris pertama |
| `cell:<row>.<kolom>` | mis. `cell:0.qty_balance` |
| `column:<kolom>` | semua nilai kolom digabung koma (untuk `contains`) |

`extract` juga berlaku (`{ "var": "saldo_awal", "from": "scalar" }`), sehingga langkah
berikutnya bisa membandingkan sebelum/sesudah.

Operator baru yang dibutuhkan: `approx` (selisih ≤ toleransi, untuk decimal/float),
dan perbandingan dengan variabel (`"value": "{{saldo_awal}} + {{qty_kirim}}"` →
evaluasi aritmetika sederhana `+ - * /`).

### Keamanan (wajib)

- Query lewat jalur baca yang sama dengan `check_db_residue` (HeadlessSession +
  gerbang `agent/access.rs`). Statement non-SELECT ditolak **walaupun** koneksi
  berakses `edit`/`agent`.
- Variabel `{{x}}` diubah menjadi placeholder parameter (`?` / `$1`) lalu di-bind.
  Tidak ada substitusi teks ke SQL (mencegah SQL injection lewat nilai respons).
- `LIMIT` otomatis (mis. 1000 baris) dan timeout query.
- Koneksi berlabel `environment: production` ditolak kecuali run memakai
  `--allow-production` (aturan yang sama dengan suite HTTP).

### Contoh (kasus VOLTUNES #2 dan #3)

```json
[
  { "id": "saldo_awal", "kind": "db_query", "phase": "setup",
    "db": { "connection_var": "db_main",
            "sql": "SELECT qty_balance FROM kandang_stocks WHERE kandang_project_id = {{project_id}} AND product_id = {{product_id}} AND deleted_at IS NULL" },
    "extract": [ { "var": "saldo_awal", "from": "scalar" } ] },

  { "id": "saldo_setelah_terima", "kind": "db_query", "phase": "test",
    "db": { "connection_var": "db_main", "wait_until_ms": 5000,
            "sql": "SELECT qty_balance FROM kandang_stocks WHERE kandang_project_id = {{project_id}} AND product_id = {{product_id}} AND deleted_at IS NULL" },
    "assertions": [ { "target": "scalar", "op": "approx", "value": "{{saldo_awal}} + {{qty_kirim}}" } ] },

  { "id": "tidak_ada_stock_out_yatim", "kind": "db_query", "phase": "test",
    "db": { "connection_var": "db_main",
            "sql": "SELECT so.id FROM kandang_stock_outs so JOIN kandang_stock_ins si ON si.id = so.kandang_stock_in_id WHERE si.id = {{stock_in_id}} AND si.deleted_at IS NOT NULL AND so.deleted_at IS NULL" },
    "assertions": [ { "target": "row_count", "op": "equals", "value": "0" } ] }
]
```

### Mapping environment → koneksi

Environment project sudah punya variabel (`base_url`, dll.). Tambahkan variabel
bertipe koneksi, mis. `db_main = <connection id>` per environment (Dev → koneksi dev,
Staging → koneksi staging). UI environment menampilkan pemilih koneksi untuk variabel
ini. Preflight run memeriksa koneksi ada dan bisa dijangkau sebelum langkah pertama.

---

## F3. Data check (monitor invariant)

Kumpulan query per project yang **seharusnya mengembalikan 0 baris**. Bila ada baris,
itulah bukti data rusak. Ini melengkapi suite: suite menguji kode di Staging, data
check menjaga data di Production.

### Model

File `{app_data}/data_checks/<project_id>.json`:

```rust
pub struct DataCheck {
    pub id: String,
    pub name: String,            // "Penerimaan dobel per dokumen"
    pub description: String,     // aturan bisnis yang dijaga
    pub connection_var: String,  // resolve per environment, sama seperti F2
    pub database: Option<String>,
    pub sql: String,             // SELECT; hasil kosong = sehat
    pub severity: Severity,      // Critical | Warning | Info
    pub max_rows: Option<u32>,   // toleransi; default 0
    pub enabled: bool,
}
```

### Eksekusi

- GUI: panel "Data Health" per project: daftar check, status terakhir, jumlah baris
  bermasalah, tombol buka hasil di grid (re-run query yang sama).
- CLI: `tabular check run --project VOLTUNES --env Production [--report out.html] [--junit out.xml]`.
  Exit code 1 bila ada check Critical yang gagal.
- Jadwal: opsional, interval per project (mis. harian 06:00), notifikasi bila status
  berubah dari sehat ke gagal.
- Riwayat: simpan jumlah baris per run agar tren terlihat (naik = bug masih aktif).
- Selalu read-only, aman untuk Production (tidak memerlukan `--allow-production`
  karena tidak menulis), tetap dengan LIMIT dan timeout.

### Contoh check VOLTUNES (siap dipakai)

Kelima query bukti dari audit 2026-10-29 menjadi check awal:

| Check | Severity |
|---|---|
| Penerimaan dobel per dokumen (SJ gudang / mutasi) sejak 30 hari | Critical |
| Saldo `kandang_stocks` ≠ jumlah transaksi (stock-switch dikecualikan) | Critical |
| Stock-out aktif dengan stock-in sumber terhapus | Critical |
| Stock-in terpakai melebihi qty masuk (project aktif) | Warning |
| Saldo negatif per kandang/project/produk (project aktif) | Warning |

Generate with AI: dari deskripsi aturan bisnis ("satu surat jalan hanya boleh
diterima sekali") + skema (`table_context`, virtual relations di diagram), AI
mengusulkan SQL check; user menyetujui sebelum disimpan.

---

## F4. MCP tools

Tambahkan ke `agent/mcp.rs` (semua menghormati akses koneksi dan proteksi production):

| Tool | Input | Output |
|---|---|---|
| `list_test_suites` | `project` | id, nama, jumlah langkah, validasi terakhir |
| `run_test_suite` | `suite`, `env`, `groups?`, `allow_production?` | verdict, ringkasan per langkah, path laporan |
| `get_test_run` | `run_id` | detail langkah: request (header disensor), respons, assertion gagal |
| `list_data_checks` | `project` | daftar check + status terakhir |
| `run_data_checks` | `project`, `env`, `check_ids?` | per check: jumlah baris, sampel ≤ 20 baris |
| `save_data_check` | definisi check | id; perlu persetujuan user di GUI (seperti write yang `ask`) |

`run_test_suite` membuat data di server target, jadi diperlakukan seperti write:
memerlukan persetujuan user kecuali environment ditandai aman (Dev/Staging) di
project. `run_data_checks` hanya baca.

Dengan ini alur agent menjadi: perbaiki kode → `run_test_suite` di Staging →
`run_data_checks` di Production → laporkan.

---

## F5. Laporan

`http_test_report` (HTML/MD/JSON/JUnit) ditambah:

- Langkah concurrent: tabel instance (no, status, waktu, potongan body) + hasil
  assertion agregat.
- Langkah DB: SQL (dengan nilai parameter), jumlah baris, sampel hasil, berapa kali
  polling sebelum lulus/timeout.
- Data check: daftar check, severity, jumlah baris, tren dibanding run sebelumnya.

---

## Patterns to mirror

| Category | Source | Pattern |
|---|---|---|
| Model suite | `tabular-client/src/http_tests.rs` | Field baru `#[serde(default, skip_serializing_if = ...)]` agar suite lama tetap terbaca |
| Assertion | `http_tests.rs` `AssertOp::parse`, `read_target`, `evaluate` | Tambah target/op di tempat yang sama; `parse` toleran alias |
| Akses DB | `http_tests.rs` `check_db_residue`, `agent/access.rs` | Lewat `HeadlessSession`, gerbang read-only |
| Runner | `http_tests.rs` `run_suite`, `spawn_run` | Event `TestEvent` per langkah; cleanup selalu jalan |
| Validasi AI | `tabular-client/src/http_test_validate.rs` | Prompt + parser AI perlu mengenal `kind`, `concurrency`, target baru |
| UI | `window_egui/integration_test_ui.rs` | Editor langkah: pemilih kind, editor SQL, field concurrency |
| CLI | `agent/cli.rs` | Subcommand baru `check` di samping `test` |
| MCP | `agent/mcp.rs`, `agent/mcp_resources.rs` | Pola tool yang ada (`run_query`, `describe_table`) |

## Fase pengerjaan

1. **F2 DB assertion** (nilai paling tinggi, risiko rendah): model + runner + target
   `row_count/scalar/cell` + bind parameter + `wait_until_ms` + UI editor sederhana.
2. **F3 Data check**: model, panel Data Health, CLI `tabular check run`, 5 check VOLTUNES.
3. **F1 Concurrent step**: barrier, assertion agregat, laporan per instance.
4. **F4 MCP tools**: `run_data_checks` dulu (read-only), lalu `run_test_suite` dengan persetujuan.
5. **F5 Laporan** menyusul per fase (setiap fase menambah bagiannya sendiri).
6. AI: Generate with AI & Validate & fix memahami `kind`, `concurrency`, target baru.

## Acceptance criteria

- Suite lama (tanpa field baru) terbaca dan berjalan identik; file yang disimpan
  ulang tidak berubah selain field yang diedit.
- Suite double submit VOLTUNES: **FAIL** terhadap build api-trans-stock sebelum
  perbaikan 2026-10-29 (lebih dari satu 2xx) dan **PASS** sesudahnya, minimal
  3 run berturut-turut.
- DB assertion dengan nilai `{{x}}` berisi `' OR 1=1 --` tetap diikat sebagai
  parameter (tes unit).
- Statement non-SELECT di langkah DB/data check ditolak pada semua level akses.
- `tabular check run` terhadap VOLTUNES Production menghasilkan jumlah baris yang
  sama dengan query bukti audit, tanpa satu pun statement tulis di query history.
- MCP `run_data_checks` mengembalikan hasil yang sama dengan CLI.
- Unit test: evaluasi assertion agregat, polling `wait_until_ms` (lulus di
  percobaan ke-n, timeout), mapping `connection_var` per environment, proteksi
  production.

## Risiko & keputusan terbuka

- **`send` belum `Sync`**: perlu dicek apakah klien HTTP yang dipakai bisa dibagi
  antar thread; bila tidak, buat klien per thread di dalam burst.
- **Replica lag**: DB assertion ke replica bisa membaca data lama. Rekomendasi:
  `connection_var` menunjuk primary, atau andalkan `wait_until_ms`.
- **Data check di production** membebani DB bila query berat: wajib timeout,
  jadwal di luar jam sibuk, dan peringatan bila query tidak memakai index
  (`explain_query` sudah ada).
- **Keputusan**: apakah `run_test_suite` via MCP boleh tanpa persetujuan untuk
  environment Dev/Staging (diusulkan: boleh, bisa diatur per project).
