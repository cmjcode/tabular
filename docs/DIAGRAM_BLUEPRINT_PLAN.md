# Rencana: Diagram sebagai Blueprint Arsitektur (Business Process per Endpoint)

Status: **Fase 1–6 selesai** (Fase 1 di commit `f56e55c`); sisa: uji manual Fase 5 dan
label `outdated`. Rincian di checklist bagian 12.
Tanggal: 2026-09-30 · Branch saat diskusi: `main`

Tujuan: diagram ERD menjadi blueprint arsitektur software. Tiap proses bisnis tampil sebagai
**card** di kanvas, dengan garis ke tabel dan resource yang disentuhnya, urutan langkahnya, dan
animasi pergerakan data saat card di-double-click. Alur di-generate AI dari kode di repository
git. Versi pertama hanya menangani proses yang dipicu **endpoint HTTP**; modelnya sudah umum
supaya job, queue consumer, cron dan event bisa menyusul tanpa mengubah format file.

---

## 1. Keputusan

| # | Keputusan | Pilihan | Status |
|---|---|---|---|
| 1 | Akar model | **Flow** (proses) dengan **trigger**; endpoint HTTP adalah salah satu jenis trigger | Disetujui |
| 2 | Implementasi pertama | Hanya trigger HTTP, karena pemindai route dan `endpoint_links` sudah ada | Disetujui |
| 3 | Sumber kebenaran "endpoint menyentuh tabel" | `endpoint_links` tetap, formatnya tidak diubah; card dan langkah di field baru | Rekomendasi |
| 4 | Generate alur | Job AI terpisah dari dokumentasi endpoint, batch 6 endpoint | Rekomendasi |
| 5 | Deteksi alur basi | Hash isi file sumber per flow, bukan `git diff` (clone cache ber-depth 1) | Rekomendasi |
| 6 | Garis proses | Hanya untuk card terpilih, di-hover atau diputar; opsi "All" tersedia | Rekomendasi |
| 7 | Resource non-tabel (API eksternal, queue, cache) | Disimpan sebagai target bertipe; v1 digambar sebagai baris di card, node kanvas di fase B1 | Perlu konfirmasi |
| 8 | Badge "API n" di header tabel | Dipertahankan sebagai opsi tampilan; bawaan berpindah ke Cards | Perlu konfirmasi |
| 9 | Urutan | Fase 1–5 dulu, Fase 6 (MCP, Mermaid, docs) menyusul | Perlu konfirmasi |

---

## 2. Konsep

Tiga lapis dalam satu kanvas:

| Lapis | Objek | Asal |
|---|---|---|
| Service | Group diagram yang punya repository | Sudah ada (`DiagramGroup.repo_url`) |
| Proses | **Flow card**: trigger + langkah berurutan | Baru |
| Data | Tabel, kolom, relasi (ERD) | Sudah ada |

Istilah:

- **Flow**: satu proses bisnis, dari pemicu sampai respons.
- **Trigger**: pemicu flow. Jenis: `http`, `job`, `queue`, `cron`, `event`, `cli`. V1 hanya `http`.
- **Step**: satu langkah di dalam flow (auth, validasi, akses database, panggilan eksternal, …).
- **Target**: resource yang disentuh sebuah step: tabel, API eksternal, queue, cache, atau flow lain.

---

## 3. Kode yang dipakai ulang

| Kebutuhan | Yang sudah ada | Lokasi |
|---|---|---|
| Relasi endpoint ke tabel | `EndpointLink`, `apply_endpoint_links`, `prune_endpoint_links` | `src/models/structs.rs:1080`, `src/repo_links.rs:275` |
| Pemindai route deterministik | `scan_routes`, `RouteHit`, `route_key` | `src/repo_endpoints.rs:896`, `:329` |
| Job AI per batch, paralel, bisa dibatalkan | `run_pool`, `acquire_ai_slot`, `batches`, `batch_snippets` | `src/repo_endpoints.rs:1351`–`1575` |
| Menyiapkan repository | `choose_source`, `resolve_repo`, `ResolvedRepo` | `src/repo_scan.rs:232`, `:400` |
| Mode AI (baca repo read-only atau cuplikan) | `ai_workspace`, `ask_ai_with_offset`, `PromptMode` | `src/repo_scan.rs:1429`, `:1516` |
| Event job | `RepoJobEvent<T>`, `step(...)`, `RepoScanError` | `src/repo_scan.rs:1214`, `:1265` |
| Card melayang dan garis ke tabel | `render_note_cards`, `draw_note_links`, `last_card_rects` | `src/diagram_notes_view.rs:495`, `:282` |
| Partikel di kurva bezier | `draw_flow_animation`, `column_curve`, `sample_curve` | `src/diagram_view.rs:4239` |
| Fokus dan peredupan | `focus_table`, `focus_set`, `DIM_OPACITY`, `dim_node` | `src/diagram_view.rs:1106`, `:3953` |
| Tween viewport | `animate_view_to`, `sample_view_anim` | `src/diagram_view.rs:377` |
| LOD dan culling | `lod_for_zoom`, `curve_visible`, `quantize_font` | `src/diagram_lod.rs` |
| Job diagram di app | `DiagramRepoScanJob`, `start_group_table_scan`, `poll_diagram_repo_scan_jobs` | `src/window_egui/diagram.rs:23`, `:346`, `:449` |
| Jendela progress AI | `render_job_progress` | `src/diagram_repo.rs:1115` |
| Warna method HTTP | `method_color` (privat, dijadikan `pub(crate)`), `method_chip` | `src/http_repo.rs:999` |
| Test render tanpa jendela | `render_frame` | `src/diagram_view.rs:8066` |

Penamaan: animasi partikel relasi tabel yang sudah ada bernama `flow_anim` /
`DiagramFlowAnimation`. Supaya tidak rancu, semua yang baru memakai awalan `flow_card` atau
`FlowCard`/`FlowPlayback`, dan komentar `flow_anim` diperjelas sebagai "aliran relasi tabel".

---

## 4. Model data

Tipe baru ditaruh di `src/models/structs.rs` di dekat `EndpointLink`, karena direferensikan
`DiagramState` seperti `DiagramNote`.

```rust
/// Jenis pemicu sebuah flow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowTriggerKind {
    #[default]
    Http,
    Job,
    Queue,
    Cron,
    Event,
    Cli,
    /// Jenis dari versi Tabular yang lebih baru.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTrigger {
    #[serde(default)]
    pub kind: FlowTriggerKind,
    /// Method HTTP huruf besar; kosong untuk trigger non-HTTP.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// Path route (`/users/{id}`), nama job, topik queue, atau ekspresi cron.
    pub target: String,
}

/// Resource yang disentuh sebuah langkah. Ditulis sebagai `{"kind": "table", "id": "users"}`.
/// `Deserialize` ditulis manual: `#[serde(other)]` menolak target asing yang membawa `id`,
/// jadi jenis tak dikenal ditangkap lewat varian untagged dan menjadi `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum FlowTarget {
    /// Id node tabel di diagram.
    Table(String),
    /// Layanan luar, mis. "Stripe API".
    External(String),
    Queue(String),
    Cache(String),
    /// Id `FlowCard` lain (proses bersambung).
    Flow(String),
    /// Jenis target dari versi Tabular yang lebih baru.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowOp {
    Read, Insert, Update, Delete, Upsert, Call, Publish, Consume,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStepKind {
    Auth, Validate, Db, External, Queue, Cache,
    #[default]
    Logic,
    Branch, Respond,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlowStep {
    #[serde(default)]
    pub kind: FlowStepKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<FlowTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<FlowOp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// `path/file:line` di repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Syarat langkah ini dijalankan, mis. "if coupon present".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// Asal-usul alur hasil generate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowMeta {
    /// Commit HEAD saat generate (informasi saja).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// RFC 3339.
    pub generated_at: String,
    pub backend: String,
    /// AI hanya melihat cuplikan, bukan seluruh repository.
    #[serde(default)]
    pub partial: bool,
    /// File yang dibaca AI untuk alur ini (relatif ke root repository).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_files: Vec<String>,
    /// md5 gabungan isi `source_files`; beda berarti alur perlu di-generate ulang.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_hash: String,
}

/// Satu proses bisnis yang tampil sebagai card di kanvas.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlowCard {
    /// Id stabil (`flw_<n>`), dibuat seperti `diagram_notes::new_note_id`.
    pub id: String,
    pub trigger: FlowTrigger,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Id `SavedRequest`; hanya ada di komputer yang punya collection-nya.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Posisi pojok kiri atas (koordinat diagram). `None` = ditata otomatis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 2]>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<FlowStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<FlowMeta>,
}
```

Field baru di `DiagramState`:

| Field | Tipe | Serde | Guna |
|---|---|---|---|
| `flow_cards` | `Vec<FlowCard>` | `default`, skip bila kosong | Card dan langkahnya |
| `endpoint_display` | `EndpointDisplay` (`Rail`, `Badges`, `Both`) | `default` = `Both`, skip bila bawaan; `cards` (mode pita API yang sudah dihapus) dibaca sebagai `Rail` | Cara endpoint ditampilkan; `show_endpoints` yang lama tetap menjadi saklar tampil/sembunyi. Helper `shows_rail()` / `shows_badges()` |
| `selected_flow` | `Option<String>` | `skip` | Card terpilih |
| `focus_flow` | `Option<String>` | `skip` | Card yang sedang difokuskan (meredupkan lainnya) |
| `flow_play` | `Option<FlowPlayback>` | `skip` | Pemutaran animasi |
| `flow_gen` | `Option<FlowGenWindow>` | `skip` | Jendela progress generate (tidak dibuat di Fase 1; menyusul di Fase 5) |

Status Fase 1: semua tipe di atas sudah ada di `src/models/structs.rs`, kecuali `flow_gen`.
Perbedaan dari rancangan awal:

- `FlowTarget` dan `FlowOp` juga punya varian `Unknown`, supaya diagram dari versi yang lebih
  baru tetap terbuka.
- `FlowStep::title`, `FlowTrigger::target`, `FlowMeta::generated_at` dan `FlowMeta::backend`
  ber-`#[serde(default)]`.
- `endpoint_display` dan `flow_lines` tidak ditulis bila bernilai bawaan (`is_default`), jadi
  diagram tanpa card menghasilkan JSON yang sama persis seperti sebelumnya.

```rust
/// Pemutaran animasi langkah sebuah flow card (runtime saja).
#[derive(Clone, Debug, PartialEq)]
pub struct FlowPlayback {
    pub card_id: String,
    /// Posisi waktu di timeline (detik), sudah memperhitungkan kecepatan.
    pub position: f64,
    /// `egui::InputState::time` pada frame terakhir; `None` saat jeda.
    pub last_tick: Option<f64>,
    pub speed: f32,
}
```

### Aturan konsistensi dengan `endpoint_links`

Semua aturan di bawah sudah diimplementasikan di `src/diagram_flow.rs` (Fase 1).

- Identitas logis card HTTP: `(repo_key, route_key(method, path))`. `repo_endpoints::route_key`
  menyamakan huruf besar-kecil dan nama parameter, jadi `/users/{id}` = `/Users/{userId}`.
  Helper: `card_matches_link(card, link)`, `card_for_endpoint(state, repo_key, method, path)`.
- `new_flow_id(existing)`: `flw_<n>` berurutan (nomor terbesar + 1), pendek karena dipakai di
  prompt AI.
- `sync_cards_from_links(state) -> bool`: untuk tiap kelompok link yang belum punya card, buat
  card tanpa langkah, urut path lalu `method_rank`. Card yang sudah ada hanya dilengkapi field
  kosongnya (`summary`, `source`, `request_id`), tidak ditimpa. Ini yang memigrasikan diagram
  lama saat dibuka. Dipanggil di:
  - `diagram_schema::prepare_stored_state` (file lokal dan layout bersama); bila ada card baru,
    `save_requested = true` supaya id card stabil;
  - `load_diagram_from_db_and_apply` (`src/window_egui/diagram.rs`), setelah `flow_cards`,
    `endpoint_display` dan `flow_lines` disalin;
  - `http_repo::link_to_diagrams`, setelah `apply_endpoint_links` (tab terbuka dan file diagram).
- `links_from_steps(state, card_id) -> ApplyStats`: langkah bertarget `Table` yang belum punya
  link menambah `EndpointLink` lewat `apply_endpoint_links`. Link yang sudah ada tidak diubah.
- Tabel sebuah card = gabungan tabel dari link dan dari langkah, tanpa duplikat (`tables_of`).
- `unlink_endpoint(state, link)`: dipakai tombol unlink di panel endpoint
  (`src/diagram_endpoints_view.rs`). Unlink terakhir dari sebuah card tanpa langkah menghapus
  card itu; card yang punya langkah tetap ada.
- `drop_orphan_cards(state) -> bool`: hapus card HTTP tanpa langkah yang tidak punya link lagi,
  lalu lepas `selected_flow`, `focus_flow` dan `flow_play` yang menunjuk card terhapus.
- `prune_flow_cards(state) -> bool`: target `Table` yang tabelnya sudah hilang diubah menjadi
  `None` (langkahnya tetap, teksnya masih berguna), lalu `drop_orphan_cards`. Dipanggil setelah
  `prune_endpoint_links` di `merge_schema`. Tidak memangkas bila `nodes` masih kosong, dan target
  tabel link database (`lnk_…::tabel`) tidak dianggap hilang karena dimuat belakangan.
- `op_direction(op) -> FlowDirection` (`ToTarget`, `FromTarget`, `Both`, `Unknown`):
  `read`/`consume` dari target, operasi tulis/`call`/`publish` ke target. `combine` menggabungkan
  arah beberapa langkah ke tabel yang sama (hasil `Both` = panah dua arah).

### Batas ukuran

| Batas | Nilai |
|---|---|
| Langkah per flow | 25 |
| `title` | 80 karakter |
| `detail` | 300 karakter |
| `columns` per langkah | 12 |
| `source_files` per flow | 20 |

Dipotong di parser dengan `repo_scan::truncate_chars`. Konstantanya ada di `src/diagram_flow.rs`:
`MAX_STEPS`, `MAX_TITLE_CHARS`, `MAX_DETAIL_CHARS`, `MAX_STEP_COLUMNS`, `MAX_SOURCE_FILES`.

### Kompatibilitas

- File lama terbaca karena semua field baru ber-`#[serde(default)]`.
- Versi lama mengabaikan field baru saat membaca. **Bila versi lama menyimpan diagram,
  `flow_cards` hilang** dari file itu. Card tanpa langkah akan dibuat ulang dari
  `endpoint_links`; langkah hasil AI harus di-generate ulang. Lihat Risiko.
- `layout_fingerprint` (`src/diagram_schema.rs`) ikut meng-hash `id` dan `pos` tiap card,
  supaya layout bersama dari `diagram_by_tabular` tidak menimpa card yang baru digeser user.
  Sudah diimplementasikan (Fase 1).

---

## 5. Modul baru

| File | Isi | Bergantung egui |
|---|---|---|
| `src/diagram_flow.rs` | Logika murni: `new_flow_id`, `sync_cards_from_links`, `links_from_steps`, `tables_of`, `prune_flow_cards`, `card_for_endpoint`, `op_direction` | Tidak |
| `src/diagram_flow_gen.rs` | Generator AI: prompt, parser, hash sumber, job background | Tidak |
| `src/diagram_flow_layout.rs` | Geometri: ukuran card per LOD, anchor langkah, penataan pita API, timeline pemutaran | Hanya tipe `egui::Rect`/`Pos2` |
| `src/diagram_flow_view.rs` | Gambar card, garis, menu, panel, kontrol pemutaran | Ya |

Semua didaftarkan di `src/lib.rs`. `diagram_flow_gen.rs` tidak boleh menyentuh `window_egui`.

---

## 6. Generator AI (`src/diagram_flow_gen.rs`)

### Antarmuka

```rust
pub struct FlowScanInput {
    pub repo_path: Option<String>,
    pub repo_url: Option<String>,
    /// Judul group atau folder, untuk prompt dan log.
    pub scope_name: String,
    pub cards: Vec<FlowSeed>,
    /// Nama tabel diagram (tanpa tabel link database).
    pub tables: Vec<String>,
    pub backend: Option<ChatBackend>,
    pub backend_label: String,
    pub cache_root: PathBuf,
    pub parallel: usize,
    /// `false` = lewati card yang `source_hash`-nya masih sama.
    pub force: bool,
}

/// Bahan satu flow untuk AI.
pub struct FlowSeed {
    pub card_id: String,
    pub method: String,
    pub path: String,
    pub source: Option<String>,
    pub known_tables: Vec<String>,
    pub previous: Option<FlowMeta>,
}

pub struct GeneratedFlow {
    pub card_id: String,
    pub summary: String,
    pub steps: Vec<FlowStep>,
    pub meta: FlowMeta,
}

pub struct FlowScanOutcome {
    pub flows: Vec<GeneratedFlow>,
    pub skipped_fresh: usize,
    /// (card_id, pesan)
    pub failed: Vec<(String, String)>,
    pub note: Option<String>,
}

pub type FlowEvent = RepoJobEvent<FlowScanOutcome>;
pub struct FlowScanHandle { pub rx: mpsc::Receiver<FlowEvent>, cancel: Arc<AtomicBool> }
pub fn spawn_flow_scan(input: FlowScanInput) -> FlowScanHandle;
```

Bentuknya mengikuti `spawn_endpoint_scan` (`src/repo_endpoints.rs:1514`): thread sendiri, hasil
`RepoJobEvent::Finished(Result<_, String>)`, "Cancelled" untuk pembatalan. Log memakai tag
`[DIAGRAM_FLOW]`.

### Alur job

| Langkah | Nomor progress | Isi |
|---|---|---|
| 1 | 1 | `choose_source` + `resolve_repo` ("Cloning or updating repository" / "Opening local folder") |
| 2 | 2 | Tentukan card yang perlu di-generate: tanpa `meta`, `partial`, hash berubah, atau `force` |
| 3 | 3 | `ai_workspace` menentukan `PromptMode` dan `ChatWorkspace` |
| 4 | 4.. | Batch 6 card, dikelompokkan per file sumber; `run_pool` + `acquire_ai_slot`; tiap batch memakai `ask_ai_with_offset` dengan offset berbeda |
| 5 | terakhir | Gabungkan hasil, hitung `source_hash`, isi `FlowMeta` |

Agar batas 6 giliran AI tetap satu untuk semua job, `run_pool`, `acquire_ai_slot`,
`MAX_GLOBAL_AI_TURNS` dan `batch_snippets` di `src/repo_endpoints.rs` dijadikan `pub(crate)`.

Satu batch gagal tidak menggagalkan job: card di batch itu masuk `failed`, sisanya tetap
dikembalikan. `RepoScanError::Cancelled` menghentikan semuanya.

### Deteksi alur basi

- Setelah generate, `source_hash` = md5 (crate `md5` sudah dipakai di `repo_scan.rs`) dari isi
  `source_files` yang diurutkan, masing-masing dibatasi `MAX_FILE_BYTES`.
- Pada generate berikutnya, hash dihitung ulang dari repository yang sudah disiapkan. Sama berarti
  dilewati (`skipped_fresh`).
- `git diff <commit>..HEAD` tidak dipakai: clone di cache ber-depth 1, jadi commit lama belum tentu
  ada, dan folder lokal bisa bukan repository git.
- `commit` diisi dari `crate::git::cli::run_text(root, &["rev-parse", "HEAD"])` bila berhasil,
  hanya untuk ditampilkan ("Generated from a1b2c3d").
- Label `outdated` di card hanya dihitung saat generate atau saat user menekan
  **Check for changes**, bukan tiap frame.

### Prompt

System prompt (bahasa Inggris, seperti prompt lain):

```text
You trace what each HTTP endpoint does, step by step, from its server source code, so an
architect can see the business process. Follow the handler into middleware, services,
repositories, ORM models, raw SQL, queue publishers and HTTP clients.
<repo_location(mode, root)>
Rules:
- List steps in execution order, at most 25 per endpoint. Merge trivial lines into one step.
- `kind` is auth, validate, db, external, queue, cache, logic, branch or respond.
- A db step names exactly one table in `table`, with `op` read, insert, update, delete or
  upsert, and the columns it filters or writes in `columns`.
- external, queue and cache steps put the service, topic or key pattern in `resource`, with
  `op` call, publish or consume.
- `condition` says when a step runs if it is not always executed.
- `source` is `path/to/file:line` of the code for that step.
- `files` lists every file you read for this endpoint.
- Use table names from this list when they match: <tables>
- Never include secrets, tokens or personal data. Never create, modify or remove files.
Reply with ONLY one JSON object, no prose and no markdown fence, shaped like this example:
{"flows":[{"id":"flw_3","summary":"Creates an order and reserves stock","files":["src/routes/orders.ts","src/services/order.ts"],"steps":[{"kind":"auth","title":"Verify bearer token","source":"src/middleware/auth.ts:12"},{"kind":"db","op":"read","table":"users","columns":["id","status"],"title":"Load the customer","source":"src/services/order.ts:40"},{"kind":"db","op":"insert","table":"orders","columns":["user_id","total"],"title":"Create the order","source":"src/services/order.ts:58"},{"kind":"queue","op":"publish","resource":"order.created","title":"Publish order.created","source":"src/services/order.ts:71"},{"kind":"respond","title":"Return 201 with the order"}]}]}
```

User prompt: nama scope, lalu per card `- <id>: <METHOD> <path> (<source>) known tables: a, b`,
lalu cuplikan kode pada mode `Snippets`.

### Parser (`parse_flows_reply`)

- Ambil objek JSON dengan `repo_scan::slice_between(text, '{', '}')`, toleran terhadap pagar
  markdown, seperti `parse_endpoints_reply`.
- `id` yang tidak ada di batch dibuang.
- `table` dicocokkan dengan `repo_endpoints::filter_tables`. Tidak cocok: target menjadi `None`
  dan nama aslinya ditambahkan ke `detail`.
- `resource` menjadi `FlowTarget::External`, `Queue` atau `Cache` menurut `kind`.
- `kind` atau `op` tak dikenal jatuh ke `Logic` / `None`.
- Semua batas ukuran di bagian 4 diterapkan di sini.
- Flow tanpa langkah dianggap gagal untuk card itu.

### Tanpa AI atau mode cuplikan

- Tanpa backend AI: tidak ada job. Card tetap tampil dengan garis ke tabel dari `endpoint_links`,
  operasi tidak diketahui, dan teks "No business process yet".
- Mode `Snippets` (backend API): AI hanya menerima file route, maksimal 60 KB per batch, setelah
  `redact_code_secrets`. `meta.partial = true`, dan card menampilkan label "partial".
- Build tanpa proses eksternal (Mac App Store, iOS): menu generate disembunyikan dengan syarat
  yang sama dengan pemindai repository (`#[cfg(not(target_os = "ios"))]`).

---

## 7. Tata letak dan gambar

### Mode Rail (bawaan): API rail + card sorotan

Pita API di kanvas sulit dibaca pada diagram besar: pil endpoint ikut zoom (tak terbaca saat
ERD terlihat utuh), garisnya panjang dan memotong ERD, dan pengelompokan per segmen path
memecah `/panens_filter_*` menjadi tumpukan berisi satu card. Mode `Rail` memisahkan indeks
endpoint dari kanvas:

| Bagian | Modul | Perilaku |
|---|---|---|
| Model rail | `diagram_api_rail.rs` (murni) | `RailModel::build`: section per repository, entitas per tabel utama (`primary_table`: nama tabel cocok dengan resource di path, lalu tabel pertama yang ditulis, lalu langkah terbanyak), baris endpoint dengan huruf CRUD. `filtered`, `table_crud`, `matrix` |
| Panel rail | `diagram_api_rail_view.rs` | `egui::Panel::left` di dalam `render_diagram`, berskala layar, bisa dilipat dan di-resize. Tab **Endpoints** (filter, pohon entitas) dan **CRUD matrix** (jumlah endpoint per tabel dan operasi; klik sel menyaring daftar). State tampilannya di memori egui, tidak disimpan |
| Card sorotan | `diagram_flow_layout.rs` | `FlowFrame::compute` tanpa pita; hanya `spotlight_index` (terpilih, fokus, atau diputar) yang `shown`. `spotlight_pos` mencoba kiri/kanan kotak tabelnya, kiri/kanan tabel pertama, lalu atas/bawah, dan memakai tempat pertama yang tidak menutupi tabel. Card selalu digambar penuh di semua zoom dan bisa digeser; posisi hasil geser disimpan di `DiagramState::flow_card_pos` (runtime saja) |
| Pilih endpoint | `diagram_flow_view::spotlight_card` | Memilih card, menggeser viewport sampai card dan tabelnya terlihat (`focus_card`), memutar prosesnya. Dipakai rail, pencarian, dan panel endpoint tabel |
| Badge tabel | `diagram_endpoints_view::draw_badge` | `API n · CRUD`, huruf dari `RailModel::table_crud` |
| Sorotan dua arah | `render_flow_cards` | Hover langkah menyorot tabelnya; hover tabel menyorot langkah card terpilih yang memakainya |

Bingkai group hanya memuat tabel.

> **Revisi 2026-10-02: mode `Cards` dihapus.** Pilihan tampilan kini `Rail` (panel saja),
> `Badges` (badge saja) dan `Both` (panel + badge, bawaan). Pita API beserta `arrange_bands`,
> `band_sizes`, `group_content_rect`, `flow_lines`/`FlowLineMode`, `flow_show_steps`,
> `FlowCard::pos`/`collapsed`, drag card, `Collapse`/`Expand`, `Reset Position` dan
> **Arrange API cards** sudah tidak ada di kode. File lama yang masih memuat field itu tetap
> terbaca (field diabaikan). Bagian di bawah ini yang membahas pita API, garis `Selected`/`All`
> dan posisi card adalah catatan rancangan lama.

### Ukuran (koordinat diagram, zoom 1.0)

| Konstanta | Nilai | Guna |
|---|---|---|
| `CARD_WIDTH` | 300 | Lebar card |
| `CARD_HEADER_H` | 34 | Chip method + path |
| `CARD_SUMMARY_H` | 22 | Satu baris ringkasan |
| `STEP_ROW_H` | 22 | Satu langkah |
| `CARD_MAX_STEPS_SHOWN` | 8 | Sisanya "+n more"; semua tampil saat card dipilih |
| `BAND_GAP` | 48 | Jarak pita API ke tabel teratas; garis pemisah API/DATA di tengahnya |
| `BAND_LABEL_H` | 26 | Judul lapisan "API" di atas pita |
| `STACK_HEADER_H` | 22 | Judul tumpukan resource (`/panens · 5 endpoints`) |
| `STACK_CARD_GAP` | 8 | Jarak antar card dalam tumpukan |
| `STACK_GAP_Y` | 20 | Jarak antar tumpukan dalam satu kolom |
| `BAND_COLUMN_GAP` | 28 | Jarak antar kolom pita |

Card ringkas secara bawaan: hanya header (method + path + jumlah langkah). Card terpilih
digambar penuh di atas tetangganya tanpa menggeser tata letak. Saklar **Show steps on all
cards** (`DiagramState::flow_show_steps`, disimpan) menampilkan langkah di semua card; pada mode
itu menu card **Collapse/Expand** berlaku.

### LOD

| LOD (`lod_for_zoom`) | Tampilan card |
|---|---|
| `Detail` | Header, ringkasan, daftar langkah bernomor dengan ikon jenis |
| Menengah | Pil method + path, jumlah langkah |
| `Overview` | Titik berwarna method; tanpa teks |

### Penataan otomatis (`arrange_bands`)

Mengikuti diagram arsitektur berlapis (C4/ArchiMate): tiap group adalah satu service dengan
lapisan **API** (card endpoint) di atas lapisan **DATA** (tabel, notasi ERD), di dalam bingkai
group yang sama. Garis card ke tabel mengikuti notasi Data Flow Diagram (proses ke data store,
berlabel operasi CRUD).

1. Card masuk pita group yang memuat tabel-tabelnya. Group dengan repository sama
   (`repo_key` dari `repo_url` bersama) didahulukan, lalu group yang memuat tabel terbanyak, lalu
   urutan group. Card yang tabelnya di luar group tapi repository-nya milik suatu group ikut pita
   group itu.
2. Pita group berada `BAND_GAP` di atas tabel group, rata kiri. Kotak group (render,
   `resolve_group_overlaps`, `compact_blocks`, anchor note) = tabel + pita
   (`group_content_rect`), jadi pita tidak pernah menimpa group lain.
3. Dalam pita, card ditumpuk per resource (`resource_of`: segmen path pertama setelah `api`,
   `vN` dan parameter; trigger non-HTTP per jenisnya). Tumpukan diurutkan menurut rata-rata x
   tabel yang disentuh, supaya garis pendek dan jarang bersilangan. Card dalam tumpukan urut path
   lalu `repo_links::method_rank`.
4. Tumpukan dibagi ke kolom berurutan dengan tinggi seimbang; jumlah kolom terkecil yang membuat
   pita minimal 1,6 kali lebih lebar dari tingginya. Ukuran pita tidak bergantung pada lebar
   tabel, jadi stabil saat group dipisahkan.
5. Card yang tabelnya tidak di group mana pun masuk pita per repository di atas tabelnya, dengan
   bingkai sendiri, dinaikkan bila menabrak tabel atau group. Card tanpa tabel di diagram masuk
   pita "endpoints without tables" di kiri seluruh diagram.
6. Hanya card dengan `pos == None` yang ditata; card yang digeser user tidak dipindah.
   **Arrange API cards** dan **Auto Arrange** mengembalikan semua card ke pitanya.
7. Bila ukuran pita berubah (card baru, generate, saklar langkah, mode tampilan) dan **Prevent
   table & group overlap** aktif, `settle_bands` memisahkan group yang kini bertabrakan (ditunda
   selama pointer ditekan).

### Garis proses

- Satu kurva per pasangan (card, target tabel). Card di pita (di atas tabel): turun dari tepi
  bawah card ke tepi atas tabel, x sejajar card bila masih di dalam lebar tabel. Bila `columns`
  terisi dan LOD `Detail`, masuk dari sisi tabel di baris kolom pertama (`column_anchor_y`).
  Card yang digeser ke samping tabel memakai kurva horizontal seperti sebelumnya.
- Chip garis aktif: huruf CRUD lalu nomor langkah (`CU · 3, 5`), di 80% panjang kurva dekat
  tabel supaya tidak tertutup card lain di pita.
- Legenda flow di pojok kiri bawah (saat ada card di layar): warna garis, arti chip, dan
  ringkasan blueprint (jumlah endpoint, yang sudah punya proses, tabel yang disentuh, tanggal
  dan commit generate terakhir).
- Warna mengikuti operasi:

  | Operasi | Warna | Arah panah |
  |---|---|---|
  | `read` | biru (80, 200, 255) | tabel → card |
  | `insert`, `update`, `upsert` | hijau (80, 220, 140) | card → tabel |
  | `delete` | merah (239, 83, 80) | card → tabel |
  | tidak diketahui | abu-abu | tanpa panah |

  Satu tabel dengan beberapa operasi memakai warna operasi tulis dan panah dua arah.
- Chip nomor langkah di tengah kurva (`draw_chip_label` di `src/diagram_view.rs:5068`, dijadikan
  `pub(crate)`).
- Mode `Selected`: garis hanya untuk card terpilih, di-hover, atau yang sedang diputar. Card lain
  tidak menggambar garis.
- Mode `All`: semua garis, opacity `DENSE_OPACITY` kecuali yang aktif. Di atas `DASH_BUDGET`
  garis, yang tidak aktif tidak digambar.
- Culling dengan `curve_visible`; card di luar `clip` tidak digambar sama sekali.

### Interaksi card

| Aksi | Hasil |
|---|---|
| Klik | Pilih card, garisnya tampil, detail proses muncul di bawah card, dan animasi langsung diputar (sama dengan tombol Play; tidak diulang bila card itu sedang diputar) |
| Drag header | Pindah card; `pos` disimpan saat dilepas (`save_requested`) |
| Double-click | Fokus + putar animasi (bagian 8) |
| Klik baris langkah | Sorot tabel targetnya; tooltip `detail` dan `source` |
| Klik kanan | Menu card |
| `Esc` / klik kanvas kosong | Lepas pilihan, fokus dan pemutaran |
| Mode Hand Tool | Card tidak interaktif, seperti note (`interactive = false`) |
| Tab subset (`scoped_to`) | Read-only: tidak ada drag, generate atau unlink |

### Titik integrasi di `render_diagram` (`src/diagram_view.rs`)

| Lokasi sekarang | Perubahan |
|---|---|
| `:912` menu tampilan | "Show API endpoints" tetap; ditambah `API endpoints as: Cards / Badges / Both` dan `Process lines: Selected / All` |
| `:952` `note_card_rects` | Tambah rect card yang bisa di-scroll, supaya scroll tidak men-zoom kanvas |
| `:1062` validasi `focus_table` | Validasi `focus_flow` dan `selected_flow` (card sudah tidak ada → `None`) |
| `:1109` `focus_set` | Lengan ketiga: `focus_flow` → `diagram_flow::tables_of(card)` |
| `:1126` `endpoint_counts` | Hanya dihitung bila `endpoint_display` menyertakan badge |
| `:1770` setelah `draw_note_links` | `diagram_flow_view::draw_flow_lines(ui, state, &to_screen, clip, hover_pos)` |
| `:2632` double-click tabel | Tidak berubah; double-click card ditangani di view card |
| `:2733` sebelum `render_note_cards` | `diagram_flow_view::render_flow_cards(...) -> FlowCardsOutcome` |
| `:3474` panel endpoint | Baris panel badge mendapat tombol "Show card" |
| `:3487` panel | `render_flow_panel` dan `render_flow_gen_window` |

`subset_with_relations` (`:566`) menyalin card yang punya minimal satu tabel di subset; langkah
ke tabel di luar subset tetap tersimpan tetapi tanpa garis. Sudah diimplementasikan (Fase 1),
termasuk menyalin `endpoint_display` dan `flow_lines`.

Pencarian (`src/diagram_search.rs:148`): `SearchTarget::Endpoint` mendapat `card: Option<String>`;
pada mode Cards hasilnya melompat ke card, bukan ke tabel.

### Ikon

Hanya `egui_icons::icons::ICON_*`; tidak ada glyph Unicode mentah. Sebelum dipakai, tiap konstanta
dicek keberadaannya di crate, lalu dipastikan terender lewat screenshot. Jenis langkah yang tidak
punya ikon yang pas digambar dengan painter (lingkaran bernomor).

---

## 8. Animasi double-click

### Timeline (murni, di `diagram_flow_layout.rs`)

```rust
pub enum PlayPhase {
    /// Request masuk ke card. `t` 0..1.
    Request(f32),
    /// Langkah ke-`index` sedang berjalan. `t` 0..1.
    Step { index: usize, t: f32 },
    /// Respons keluar dari card.
    Response(f32),
    Done,
}

pub const REQUEST_SECS: f64 = 0.6;
pub const STEP_SECS: f64 = 1.2;
pub const STEP_NO_TARGET_SECS: f64 = 0.5;
pub const RESPONSE_SECS: f64 = 0.6;

pub fn play_phase(card: &FlowCard, position: f64) -> PlayPhase;
pub fn play_duration(card: &FlowCard) -> f64;
pub fn step_start(card: &FlowCard, index: usize) -> f64;
```

Langkah tanpa target tabel memakai durasi pendek. Card tanpa langkah memutar satu langkah semu per
tabel dari `endpoint_links`.

### Urutan saat double-click

1. `selected_flow = focus_flow = Some(id)`; `focus_table` dan `focus_group` dikosongkan.
2. Viewport di-tween ke bounding box card + semua tabelnya: zoom yang memuat kotak itu dengan
   margin 60 px, dibatasi `DETAIL_MIN_ZOOM..=FOCUS_ZOOM`, lewat `animate_view_to`.
3. `flow_play = Some(FlowPlayback { position: 0.0, last_tick: None, speed: 1.0, .. })`.
4. Tiap frame: `position += (now - last_tick) * speed`, lalu `play_phase` menentukan gambar.

### Yang digambar per fase

| Fase | Gambar |
|---|---|
| `Request` | Partikel masuk dari kiri ke header card; header berdenyut |
| `Step` dengan target tabel | Baris langkah di card menyala; partikel berekor di kurva sesuai arah operasi; header tabel dan baris `columns` berdenyut dengan warna operasi; caption langkah di kurva |
| `Step` tanpa target | Baris langkah menyala; ikon jenis berdenyut |
| `Response` | Partikel keluar ke kiri card |
| `Done` | Semua garis card tetap tampil, tanpa partikel; fokus tetap sampai user keluar |

Gambar partikel memakai ulang teknik `draw_flow_animation` (ekor 4 titik, `bezier.sample`).

### Kontrol pemutaran

Bilah melayang di bawah card: Play/Pause, Previous step, Next step, kecepatan (0.5×, 1×, 2×),
Replay, Close. Previous/Next memindahkan `position` ke `step_start`. Spasi = Play/Pause bila tidak
ada field teks yang fokus.

### Repaint

- Sedang berjalan dan terlihat: `request_repaint_after(33 ms)`.
- Jeda atau `Done`: tidak meminta repaint.
- LOD bukan `Detail`: partikel tetap digambar di kurva, sorotan kolom dilewati.
- Animasi berhenti sendiri di `Done`; tidak ada loop otomatis.

### Detail proses di dalam card

Saat card dipilih (LOD `Detail`), card memanjang dan bagian bawahnya berisi **Tables involved**:
tiap tabel dengan lencana C / R / U / D dan nomor langkahnya; klik = lompat ke tabel. Isinya
digambar lewat `egui::Area` yang ikut pan/zoom (teks diskalakan dengan zoom 0,5–1,4) tanpa
bingkai sendiri; tingginya terukur tiap frame ke `DiagramState::flow_footer_h` dan ditambahkan
ke `drawn_height`, jadi latar dan tepi card membungkus semuanya sebagai satu card. Tidak ada
jendela terpisah.

Tidak ada tombol di card: klik card langsung memutar prosesnya (pengganti Play), sedangkan Open
Request dan Generate/Regenerate ada di menu klik kanan card. Ringkasan penuh, info generate dan
lokasi route tidak ditampilkan lagi; ringkasan satu baris tetap di card dan tanggal/commit
generate terakhir ada di legenda flow.

Daftar langkah tidak diulang karena sudah tampil di card. Di card terpilih, langkah yang punya
detail memiliki chevron; klik barisnya membuka detail tepat di bawahnya (kolom, kondisi,
keterangan yang dipecah per kata, lokasi kode) dan klik lagi menutupnya
(`DiagramState::flow_open_step`, satu langkah sekaligus, direset saat card lain dipilih). Baris
sesudahnya turun (`step_row_rect` dengan `open`) dan footer ikut di bawah card. Pada zoom kecil (card berupa pil/titik)
detail tidak digambar.

---

## 9. Aksi, menu dan teks UI

`DiagramAction` baru (`src/diagram_view.rs:67`):

```rust
/// Generate alur bisnis untuk card ini (kosong = semua card repository group).
GenerateFlows { group_id: Option<String>, card_ids: Vec<String>, force: bool },
/// Hentikan job generate alur yang berjalan.
CancelFlowGeneration,
```

`GenerateFlows` masuk daftar `modifies_source` di `handle_diagram_action`
(`src/window_egui/diagram.rs:237`), jadi ditolak di tab subset.

Teks UI (bahasa Inggris):

| Tempat | Teks |
|---|---|
| Menu group | `Generate Business Process (AI)` |
| Menu card | `Play Process`, `Open Request`, `Generate Business Process (AI)` / `Regenerate`, `Collapse` / `Expand`, `Reset Position`, `Remove Card` |
| Menu tampilan | `API endpoints as: Rail / Badges / Both` |
| Card kosong | `No business process yet` |
| Label | `partial`, `outdated` |
| Jendela Generate Endpoints | checkbox `Also generate business process` |
| Toast | `Business process generated for {n} endpoint(s); {m} unchanged, {k} failed` |

---

## 10. Job di aplikasi

Mengikuti `DiagramRepoScanJob`:

```rust
/// Generate alur bisnis untuk card sebuah diagram.
pub struct DiagramFlowGenJob {
    conn_id: Option<i64>,
    db_name: Option<String>,
    handle: crate::diagram_flow_gen::FlowScanHandle,
}
```

| File | Perubahan |
|---|---|
| `src/window_egui/diagram.rs` | `DiagramFlowGenJob`, `start_flow_generation`, `poll_diagram_flow_jobs`, penanganan `GenerateFlows` |
| `src/window_egui/mod.rs` (dekat `:801`) | Field `diagram_flow_jobs: Vec<diagram::DiagramFlowGenJob>` |
| `src/window_egui/init.rs` (dekat `:690`) | Inisialisasi `Vec::new()` |
| `src/window_egui/app_impl.rs` (dekat `:5161`) | Panggil `poll_diagram_flow_jobs(ctx)` |

`start_flow_generation`:

1. Backend AI dari `effective_chat_target` + `backend_ready_for`, seperti `start_group_table_scan`.
   Tanpa backend: toast error, tidak ada job.
2. Repository dari group (`local_repo_path`, `repo_url`). Card yang `repo_key`-nya tidak cocok
   dengan group mana pun memakai repository folder HTTP API lewat `RepoIndex`
   (`src/repo_links.rs:50`).
3. Satu job per diagram; permintaan baru untuk diagram yang sama membatalkan yang lama.
4. `state.flow_gen` diisi untuk jendela progress.

`poll_diagram_flow_jobs` menerapkan hasil:

1. Untuk tiap `GeneratedFlow`: isi `summary`, `steps`, `meta` pada card dengan `id` yang sama.
2. `links_from_steps` untuk menambah link tabel baru.
3. `save_requested = true`.
4. Tab ditutup saat job berjalan: job dibatalkan, seperti pemindai repository.

Dari jendela **Generate Endpoints** (`add_endpoints`, `src/http_repo.rs:1755`): bila checkbox
dicentang, setelah `link_to_diagrams` selesai, generate alur dimulai untuk diagram yang sedang
terbuka. Diagram yang tidak terbuka hanya mendapat link; alurnya di-generate lewat menu setelah
diagram dibuka.

---

## 11. Simpan dan sinkronisasi

Tidak ada jalur baru. `flow_cards` ikut `DiagramState`, jadi tersimpan ke file lokal,
`diagram_by_tabular`, vault dan cloud sync.

- `diagram_links::persistable` tidak perlu diubah: card milik diagram host, bukan link database.
- `request_id` dan folder project tetap lokal; yang dibagikan hanya `repo_key`, path dan langkah.
- Tidak ada secret: prompt melarangnya, cuplikan diredaksi, dan parser memotong teks panjang.
  `ConnectionConfig` tidak pernah masuk prompt.

---

## 12. Fase implementasi

Tiap fase diakhiri `cargo clippy --all-targets -- -D warnings`,
`cargo clippy --all-targets --features collab -- -D warnings` dan `cargo test`. Format dengan
`rustfmt` per file yang disentuh.

### Fase 1: Model dan logika murni

- [x] Tipe di `src/models/structs.rs` + field `DiagramState` + `Default`. `flow_gen` /
  `FlowGenWindow` ditunda ke Fase 5 bersama jendela progress.
- [x] `src/diagram_flow.rs` dengan fungsi di bagian 5, plus `unlink_endpoint` dan
  `drop_orphan_cards` (dipakai tombol unlink di panel endpoint dan `prune_flow_cards`).
- [x] `prune_flow_cards` dipanggil setelah `prune_endpoint_links` di `merge_schema`;
  `layout_fingerprint` diperluas.
- [x] `subset_with_relations` menyalin card.
- [x] `sync_cards_from_links` dipanggil di `prepare_stored_state`, di
  `load_diagram_from_db_and_apply`, dan setelah `apply_endpoint_links` di `http_repo.rs`.
- Validasi: `cargo test --lib diagram_flow::` (15 test) dan test serde file lama.

### Fase 2: Generator AI

- [x] `pub(crate)` untuk helper di `src/repo_endpoints.rs` (`run_pool`, `acquire_ai_slot`,
  `MAX_GLOBAL_AI_TURNS`, `repo_location`), plus `source_file`, `file_snippets` dan
  `batches_by_file` generik yang juga dipakai generate endpoint.
- [x] `src/diagram_flow_gen.rs`: prompt, parser, hash, job. Giliran AI disuntikkan (`AskFn`)
  supaya job bisa dites tanpa AI.
- [x] Test parser, batas ukuran, pemilihan card basi, pembatalan, batch gagal.
- [x] Test nyata ber-`#[ignore]` `real_claude_traces_business_process` (Claude Code, haiku).
- Validasi: `cargo test --lib diagram_flow_gen::` (12 test + 1 ignored, lulus; test nyata lulus).

### Fase 3: Card dan garis di kanvas

- [x] `src/diagram_flow_layout.rs`: ukuran, baris langkah, `FlowFrame`, `arrange_lane`.
  Lane dipasangkan dengan group lewat URL repository bersama saja (murni, tanpa membaca
  `.git/config` tiap frame); group tanpa URL memakai bounding box tabel yang disentuh card.
- [x] `src/diagram_flow_view.rs`: `render_flow_cards`, `draw_flow_lines`, `card_curve`,
  `focus_card`, menu card (Open Request, Collapse/Expand, Reset Position, Remove Card).
  Card digeser dari seluruh badannya; card terpilih menampilkan semua langkah tanpa scroll,
  jadi rect scroll card tidak perlu didaftarkan ke kanvas.
- [x] Integrasi di `render_diagram` sesuai tabel bagian 7 (validasi pilihan, lengan
  `focus_flow` di `focus_set`, badge hanya pada mode Badges/Both, Esc dan klik kanvas kosong
  melepas card, tombol "Show card" di panel endpoint).
- [x] Menu tampilan dan pencarian (`SearchTarget::Endpoint { card }`, satu hasil per card).
- [x] Cek visual: example sementara + `ViewportCommand::Screenshot` (Detail, Compact, Overview).
- [x] Revisi tata letak (2026-09-30): lane di kiri group diganti pita API di dalam bingkai group
  (`arrange_bands`), card ringkas bawaan, garis turun ke tabel, chip CRUD, legenda flow, dan Auto
  Arrange ikut menata card. Alasan: lane bertumpuk ke kiri, menjauh dari tabel dan menimpa group
  lain, dan Auto Arrange tidak menyentuh card.
- Validasi: test layout (9), `render_frame` (culling, LOD, mode garis, double-click + Esc),
  pencarian mode Cards.

### Fase 4: Animasi dan panel

- [x] `play_phase` dan kawan-kawan + test murni. Timeline ditaruh di
  `src/diagram_flow_play.rs` (bukan `diagram_flow_layout.rs`) karena Fase 3 dikerjakan
  paralel; `FlowPlayback` mendapat field `playing` untuk membedakan jeda dari belum mulai.
- [x] Gambar partikel, sorotan, caption; kontrol pemutaran; panel "Process"
  (`src/diagram_flow_play_view.rs`). Panel memakai `egui::Window`, seperti panel Relations.
- [x] `focus_flow` di `focus_set` (dikerjakan bersama integrasi Fase 3).
- Validasi: `cargo test --lib diagram_flow` (54 test), termasuk
  `playback_stops_by_itself_and_paused_frames_do_not_repaint`; screenshot langkah baca/tulis.

### Fase 5: Job, menu dan jendela progress

- [x] `DiagramFlowGenJob`, poll, `DiagramAction::GenerateFlows` di
  `src/window_egui/diagram_flow_jobs.rs` (bukan `diagram.rs`, supaya file itu tidak membesar).
  Card dikelompokkan per repository (`plan_flow_generation`); beberapa repository dikerjakan
  berurutan dalam satu job. `CancelFlowGeneration` tidak dibuat: tombol Cancel memakai
  `FlowGenWindow::cancel_requested`, sama seperti jendela saran tabel.
- [x] Jendela progress dengan `render_job_progress` (`src/diagram_flow_gen_view.rs`): Cancel,
  Run in Background (toast saat selesai), ringkasan hasil dan daftar card gagal.
- [x] Menu: group `Generate Business Process (AI)`, card `Generate …` / `Regenerate`, tombol di
  panel Process. Saat job berjalan, item menu menjadi `Show Generation Progress`.
- [x] Checkbox `Also generate business process` di jendela Generate Endpoints.
- Validasi otomatis: 4 test job + 1 test view, clippy default dan `collab` bersih.
- [ ] Uji manual dengan Claude Code dan satu backend API (mode cuplikan).

### Fase 6: Pelengkap

- [x] `src/agent/knowledge.rs`: `FlowInfo`, `FlowStepInfo`, `FlowTableInfo` di field baru
  `DiagramDescription::flows` (plus `total_flows`) dari `describe_diagram`, jadi tersedia
  lewat tool MCP dan resource diagram. Id node tabel diganti nama tabel; `request_id`,
  `pos`, `repo_key` dan `collapsed` tidak dikirim. Dengan `table`/`group`, hanya flow yang
  menyentuh tabel cakupan; maksimal 40 per respons (`MAX_FLOWS`). Deskripsi tool, instruksi
  server MCP dan prompt AI Assistant menyebut flow.
- [x] `src/diagram_mermaid.rs`: `flow_to_mermaid` (`flowchart TD`: trigger, langkah
  berurutan, tabel sebagai silinder, API luar/queue/cache/flow lain dengan bentuk sendiri,
  arah panah dari `op_direction`, langkah bersyarat sebagai garis putus-putus berlabel) dan
  `flows_markdown` (satu blok per card). UI: menu card **Copy as Mermaid** dan menu Export
  **Business processes (Mermaid .md)…**. Tidak ada impor balik.
- [x] `docs/HTTP_API_REPOSITORY.md` (card, generate, pemutaran, panel, ekspor,
  kompatibilitas), `docs/DIAGRAM_GROUP_REPOSITORY.md` (menu group), `docs/MCP.md` (bentuk
  `flows`).
- Validasi: 3 test Mermaid, 1 test `flow_infos` + assert di
  `describe_diagram_filters_and_redacts`; output diparse dengan `mermaid.parse` (Mermaid 11,
  di luar repo); clippy default dan `collab` bersih; `cargo test` lulus.
- Belum dibuat (tidak ada di UI, jadi tidak ditulis di docs): label `outdated` dan tombol
  **Check for changes** dari bagian 6. Alur basi saat ini hanya terdeteksi saat generate
  (card yang hash-nya sama dilewati).

Total: 10–10,5 hari kerja. Fase 1 dan 3 saja sudah menampilkan card dan garis dari data yang ada.

---

## 13. Pengujian

| Jenis | Cakupan |
|---|---|
| Unit, `diagram_flow` (selesai) | `sync_cards_from_links` tidak menduplikasi dan menyamakan route; unlink terakhir menghapus card kosong; `prune_flow_cards` mempertahankan langkah; `tables_of` menggabungkan link dan langkah; `links_from_steps`; `layout_fingerprint`; `op_direction` |
| Unit, serde (selesai) | File lama tanpa `flow_cards` terbaca; state tanpa card tidak menulis field baru; jenis tak dikenal menjadi `Unknown`; round-trip `FlowCard` |
| Unit, parser | JSON berpagar, id asing, tabel tak dikenal, lebih dari 25 langkah, teks terlalu panjang, flow kosong |
| Unit, hash | File berubah → basi; file hilang → basi; urutan `source_files` tidak memengaruhi hash |
| Unit, layout | Pita di atas tabel di dalam group; urutan tumpukan per posisi tabel; kolom lebar untuk banyak resource; pita repository naik melewati tabel; group ber-repository sama menang; card ber-`pos` tidak dipindah; mode ringkas |
| Unit, timeline | Batas fase, `step_start`, card tanpa langkah |
| Render (`render_frame`) | Card di luar layar tanpa geometri; Overview lebih ringan dari Detail; double-click mengisi `focus_flow`; pemutaran berakhir di `Done` |
| Bench | Varian `bench_large_diagram` dengan 300 card: waktu frame tidak naik lebih dari 10% pada mode `Selected` |
| Manual | Repository Express, Laravel dan satu monorepo; backend Claude Code, Gemini CLI, satu API |

---

## 14. Risiko

| Tingkat | Risiko | Mitigasi |
|---|---|---|
| Tinggi | Diagram penuh garis pada API besar | Mode `Selected` bawaan; anggaran garis; card ringkas; pita per resource tepat di atas tabelnya |
| Tinggi | AI keliru menebak alur atau tabel | `source` per langkah; tabel difilter ke daftar diagram; label `partial`; regenerate per card |
| Sedang | Versi lama menyimpan diagram dan membuang `flow_cards` | Card dibuat ulang dari link; langkah bisa di-generate ulang; dicatat di docs dan catatan rilis |
| Sedang | `diagram_view.rs` 8.252 baris bertambah rumit | Kode baru di empat modul; `render_diagram` hanya memanggil |
| Sedang | Agent lain sedang mengubah `src/window_egui/mod.rs` dan `src/git/` | Edit sekecil mungkin di `mod.rs`, `init.rs`, `app_impl.rs`; tidak memakai `git stash`; hanya memanggil `git::cli::run_text` |
| Sedang | Biaya AI | Batch 6, hash untuk melewati yang tidak berubah, slot global 6, bisa dibatalkan |
| Sedang | Payload sync membesar | Batas ukuran bagian 4; perkiraan 1–2 KB per flow |
| Rendah | Mode cuplikan menghasilkan alur dangkal | Label `partial`; docs menyarankan CLI agent |
| Rendah | Ikon menjadi kotak kosong | Hanya `ICON_*`, dicek lewat screenshot |

---

## 15. Setelah v1: menuju blueprint penuh

| Fase | Isi | Prasyarat di model |
|---|---|---|
| B1 | Node kanvas untuk resource non-tabel (eksternal, queue, cache), dipakai bersama antar flow | `FlowTarget::External/Queue/Cache` sudah ada |
| B2 | Sambungan antar flow: endpoint → queue → consumer, service A memanggil service B | `FlowTarget::Flow` sudah ada |
| B3 | Pemindai trigger lain: job, consumer, cron, event, CLI | `FlowTriggerKind` sudah ada |
| B4 | Tampilan level service saat zoom jauh: satu kotak per group dengan jumlah flow dan garis antar service | Group ber-repository sudah ada |
