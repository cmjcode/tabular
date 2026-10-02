# Plan: Multi-Agent AI Chat (pilih agent per pertanyaan)

Status: **draft, siap dikerjakan**
Cakupan: `tabular-client` saja. `tabular-server` tidak tersentuh.
Bahasa: komentar kode Bahasa Indonesia, string UI dan dokumentasi user Bahasa Inggris
(lihat `AGENTS.md`).

---

## 1. Tujuan

Saat ini panel AI Assistant hanya bisa memakai **satu** backend yang dipilih di Settings:
HTTP API *atau* satu CLI agent (`agy` / `claude` / `gemini` / custom). Memilih agent lain di
Settings menghapus path, model, dan effort agent sebelumnya.

Target setelah perubahan ini:

1. Semua CLI agent bisa dikonfigurasi **bersamaan** (profil per agent, masing-masing punya
   path/model/effort/extra args sendiri, dan flag enabled).
2. Di panel chat ada **picker** untuk memilih siapa yang ditanya pada giliran ini: HTTP API
   atau salah satu agent yang enabled. Pindah agent di tengah percakapan **tidak** menghapus
   riwayat; agent baru menerima ringkasan percakapan lewat prompt.
3. Setiap balasan di transkrip menampilkan nama agent yang menjawab.
4. Fitur non-chat (blok inline `--AI`, bantuan AI di HTTP client) memakai satu **default
   target** yang diatur di Settings.
5. Preferensi lama bermigrasi otomatis; user yang upgrade tidak kehilangan konfigurasi.

Tidak termasuk (out of scope): beberapa profil untuk kind yang sama (mis. dua Custom),
riwayat chat terpisah per agent, bertanya ke beberapa agent sekaligus secara paralel.

---

## 2. Peta kode saat ini (yang harus dipahami sebelum mulai)

| Lapisan | File | Yang relevan |
|---|---|---|
| Tipe & persistensi prefs | `src/config.rs` | `AiBackend` (baris ~205), `CliAgentKind` (~240), `AppPreferences` field `ai_backend`, `ai_cli_kind`, `ai_cli_bin`, `ai_cli_model`, `ai_cli_effort`, `ai_cli_extra_args` (~346-359). Load: `match` key-value (~620-632). Save: daftar tuple `(key, value)` (~746-753). Prefs disimpan sebagai **baris key-value string** di SQLite, bukan JSON utuh. |
| State runtime | `src/window_egui/mod.rs` (~595-632) | Field `ai_*` di struct `Tabular`; `ai_session_id`, `ai_cli_mcp_registered`, `ai_cli_mcp_receiver`, `ai_cli_mcp_message`, buffer `ai_settings_cli_*_input`. |
| Load prefs ke state | `src/window_egui/init.rs` (~83-95, ~600-626) | Penyalinan field prefs → `Tabular`. Ada test di ~1607-1633 yang meng-assert field lama. |
| Simpan state ke prefs | `src/window_egui/app_impl.rs` (~4091-4096) | Membangun `AppPreferences` dari `Tabular`. |
| Settings UI | `src/window_egui/ai_cli_settings.rs` | `ai_cli_config()`, `ensure_ai_mcp_check()`, `start_ai_mcp_register()`, `start_ai_cli_test()`, `poll_ai_cli_background()`, `render_ai_backend_settings()`, `render_ai_cli_agent_rows()` (radio Agent di ~219-245), `render_ai_cli_mcp_status()`, `render_ai_cli_test()`. Dipanggil dari `preferences.rs` ~1246-1250. |
| Snapshot backend untuk thread | `src/ai_assistant.rs` | `ChatBackend` (~331), `chat_backend()` (~354), `backend_label()` (~383), `backend_ready()` (~407), `start_chat()` (~433), `request_text()` (~516), `build_chat_prompts()` (~833). |
| Panel chat | `src/editor.rs` | Kirim giliran (~5281-5345: `backend_ready` → `chat_backend` → `build_chat_prompts` → `start_chat`), handler event `AgentEvent::Session` (~5132), `ai_new_chat` (~5350), header/chip backend (~6165-6190), peringatan MCP (~6985), blok inline `--AI` (~4438-4463). |
| Konsumen lain | `src/http_client.rs` (~192, 245, 344, 2037), `src/http_ai.rs` (~568), `src/window_egui/app_impl.rs` (~3032-3053) | Memakai `ChatBackend` untuk bantuan AI di HTTP client. |
| Harness CLI | `src/agent/harness.rs` | `CliAgentConfig`, `AgentRequest`, `build_args`, `StreamParser`, `spawn_stream`, `check_mcp_registered`, `register_mcp`, `test_connection`. **Sudah stateless per request; tidak perlu diubah** kecuali satu `impl From`. |
| Model pesan | `src/models/structs.rs` (~1190) | `AiChatMessage` (derive `Default`). |

Aturan penting dari `AGENTS.md`: `src/agent/` tidak boleh bergantung pada `window_egui`;
fungsi headless menerima data biasa, bukan `&mut Tabular`.

---

## 3. Keputusan arsitektur

| # | Keputusan | Alasan |
|---|---|---|
| A1 | **Satu profil per `CliAgentKind`** (4 slot tetap: agy, claude, gemini, custom), bukan daftar profil dengan ID bebas. | Tidak perlu ID unik, pemetaan sesi/MCP/UI tab tetap keyed by `CliAgentKind`. Perubahan minimal terhadap harness. |
| A2 | Tipe baru `ChatTarget { Api, Cli(CliAgentKind) }` sebagai identitas "siapa yang ditanya". `AiBackend` tetap ada sebagai nilai turunan (`target.backend()`) supaya `ChatBackend.backend` dan `http_client` tidak banyak berubah. | Menyatukan API dan CLI dalam satu picker. |
| A3 | Prefs baru disimpan sebagai **satu key JSON** `ai_cli_profiles` plus dua key string `ai_default_target` dan `ai_chat_target`. | Tabel prefs adalah key-value; JSON menghindari 20+ key baru. |
| A4 | Migrasi dilakukan **saat load** dari key legacy bila `ai_cli_profiles` belum ada. Key legacy tidak ditulis lagi, tapi juga tidak dihapus dari DB. | Aman untuk upgrade dan downgrade. |
| A5 | Sesi CLI disimpan sebagai `AgentSession { kind, id }`. Sesi hanya dipakai bila `kind` sama dengan target giliran ini. Bila tidak sama, giliran tersebut menyertakan `history_prefix` walaupun agent mendukung resume. | Inilah mekanisme "pindah agent tanpa kehilangan konteks". |
| A6 | Status MCP (registered/receiver/message) menjadi map per `CliAgentKind`. Hasil Connection Test tetap satu, mengikuti tab yang sedang dibuka di Settings. | agy dan gemini masing-masing perlu registrasi global sendiri. |
| A7 | Buffer input Settings (`ai_settings_cli_*_input`) dihapus; TextEdit langsung mengedit profil, penyimpanan tetap dipicu oleh Apply / lost focus. | Mengurangi state ganda. |
| A8 | Default `enabled = false` untuk profil baru; migrasi mengaktifkan kind yang sebelumnya dipilih. | Picker tidak menampilkan agent yang belum pernah dikonfigurasi. |

---

## 4. Desain detail

### 4.1 `src/config.rs`

#### Tipe baru

```rust
impl CliAgentKind {
    /// Urutan tampil di Settings dan picker chat.
    pub const ALL: [CliAgentKind; 4] = [
        CliAgentKind::Antigravity,
        CliAgentKind::ClaudeCode,
        CliAgentKind::GeminiCli,
        CliAgentKind::Custom,
    ];
}
// Tambahkan derive `PartialOrd, Ord, Hash` pada `CliAgentKind` (dipakai sebagai key map).

/// Profil satu CLI agent. Satu slot tetap per `CliAgentKind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliAgentProfile {
    pub kind: CliAgentKind,
    /// Tampil di picker panel chat dan boleh dipakai sebagai default target.
    #[serde(default)]
    pub enabled: bool,
    /// Path binary; kosong berarti cari `kind.default_binary()` di PATH.
    #[serde(default)]
    pub bin: String,
    #[serde(default)]
    pub model: String,
    /// `low` | `medium` | `high`; kosong berarti default CLI.
    #[serde(default)]
    pub effort: String,
    /// Argumen tambahan; untuk `Custom` adalah template dengan placeholder.
    #[serde(default)]
    pub extra_args: String,
}

impl CliAgentProfile {
    pub fn new(kind: CliAgentKind) -> Self { /* enabled=false, string kosong */ }
    /// Empat profil default, urutan `CliAgentKind::ALL`.
    pub fn defaults() -> Vec<CliAgentProfile> { … }
}

/// Siapa yang menjawab: HTTP API (provider di prefs) atau salah satu CLI agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatTarget {
    Api,
    Cli(CliAgentKind),
}

impl Default for ChatTarget { fn default() -> Self { ChatTarget::Api } }

impl ChatTarget {
    pub fn backend(self) -> AiBackend { /* Api → AiBackend::Api, Cli(_) → AiBackend::Cli */ }
    /// Bentuk string untuk tabel prefs: `"API"` atau `"CLI:AGY"`, `"CLI:CLAUDE"`, …
    pub fn as_string(self) -> String { … }
}
impl std::str::FromStr for ChatTarget { /* kebalikan `as_string`; input tak dikenal → Api */ }
```

`AiBackend` **dipertahankan** (dipakai `ChatBackend.backend`, `http_client`), tapi tidak lagi
menjadi field prefs.

#### `AppPreferences`

Hapus: `ai_backend`, `ai_cli_kind`, `ai_cli_bin`, `ai_cli_model`, `ai_cli_effort`,
`ai_cli_extra_args`.

Tambah:

```rust
/// Target untuk fitur non-chat (blok inline `--AI`, HTTP client) dan nilai awal picker.
#[serde(default)]
pub ai_default_target: ChatTarget,
/// Pilihan terakhir di picker panel chat; `None` berarti ikut `ai_default_target`.
#[serde(default)]
pub ai_chat_target: Option<ChatTarget>,
/// Profil semua CLI agent; selalu 4 entri (satu per `CliAgentKind`).
#[serde(default = "CliAgentProfile::defaults")]
pub ai_cli_profiles: Vec<CliAgentProfile>,
```

Pastikan `AppPreferences` tidak memakai `deny_unknown_fields` (agar JSON/export lama yang
masih membawa field legacy tetap bisa di-deserialize). Perbarui kedua konstruktor default
(`Default` di ~425 dan nilai awal di ~554).

#### Load (fungsi yang berisi `match` key di ~620)

1. Tambah variabel lokal sebelum loop:
   ```rust
   let mut legacy = LegacyCliPrefs::default(); // backend, kind, bin, model, effort, extra_args
   let mut saw_profiles = false;
   ```
2. Di `match`:
   - `"ai_backend" | "ai_cli_kind" | "ai_cli_bin" | "ai_cli_model" | "ai_cli_effort" | "ai_cli_extra_args"` → isi `legacy`.
   - `"ai_cli_profiles"` → `serde_json::from_str::<Vec<CliAgentProfile>>(&v)`; bila `Ok`, set `saw_profiles = true`. Bila `Err`, `log::warn!("[PREFS] …")` dan biarkan default.
   - `"ai_default_target"` → `v.parse().unwrap_or_default()`.
   - `"ai_chat_target"` → kosong = `None`, selain itu `Some(v.parse().unwrap_or_default())`.
3. Setelah loop: `if !saw_profiles { let (profiles, target) = migrate_legacy_cli_prefs(&legacy); prefs.ai_cli_profiles = profiles; prefs.ai_default_target = target; }`
4. Selalu jalankan `normalize_profiles(&mut prefs.ai_cli_profiles)`: pastikan tepat 4 entri, satu per kind, urutan `CliAgentKind::ALL` (tambah yang hilang, buang duplikat, urutkan).

#### Fungsi migrasi (murni, unit-testable)

```rust
#[derive(Debug, Default, Clone)]
pub struct LegacyCliPrefs {
    pub backend: Option<AiBackend>,
    pub kind: Option<CliAgentKind>,
    pub bin: String,
    pub model: String,
    pub effort: String,
    pub extra_args: String,
}

/// Bangun profil dari preferensi format lama (satu agent aktif).
pub fn migrate_legacy_cli_prefs(legacy: &LegacyCliPrefs) -> (Vec<CliAgentProfile>, ChatTarget) {
    let mut profiles = CliAgentProfile::defaults();
    let kind = legacy.kind.unwrap_or_default();
    // Hanya aktifkan bila user memang pernah memakai CLI atau pernah mengisi sesuatu.
    let had_cli = legacy.backend == Some(AiBackend::Cli)
        || !legacy.bin.trim().is_empty() || !legacy.model.trim().is_empty()
        || !legacy.effort.trim().is_empty() || !legacy.extra_args.trim().is_empty();
    if had_cli {
        let p = profiles.iter_mut().find(|p| p.kind == kind).expect("defaults() memuat semua kind");
        p.enabled = true;
        p.bin = legacy.bin.clone(); p.model = legacy.model.clone();
        p.effort = legacy.effort.clone(); p.extra_args = legacy.extra_args.clone();
    }
    let target = if legacy.backend == Some(AiBackend::Cli) { ChatTarget::Cli(kind) } else { ChatTarget::Api };
    (profiles, target)
}
```

#### Save (daftar tuple di ~746)

Hapus enam tuple legacy. Tambah:

```rust
("ai_default_target", default_target_str.as_str()),   // ChatTarget::as_string()
("ai_chat_target", chat_target_str.as_str()),          // "" bila None
("ai_cli_profiles", profiles_json.as_str()),            // serde_json::to_string(&prefs.ai_cli_profiles)
```

`serde_json::to_string` pada `Vec<CliAgentProfile>` tidak bisa gagal untuk tipe ini; bila
ingin tetap defensif, `unwrap_or_default()` dan `log::error!`.

### 4.2 `src/agent/harness.rs` (satu tambahan kecil)

```rust
impl From<&crate::config::CliAgentProfile> for CliAgentConfig {
    fn from(p: &CliAgentProfile) -> Self {
        CliAgentConfig { kind: p.kind, bin: p.bin.clone(), model: p.model.clone(),
                         effort: p.effort.clone(), extra_args: p.extra_args.clone() }
    }
}
```

`harness.rs` sudah mengimpor `crate::config::CliAgentKind`, jadi arah dependensi tidak
berubah. Tidak ada perubahan lain di `src/agent/`.

### 4.3 State runtime: `src/window_egui/mod.rs`, `init.rs`, `app_impl.rs`

#### Field `Tabular` — hapus

`ai_backend`, `ai_cli_kind`, `ai_cli_bin`, `ai_cli_model`, `ai_cli_effort`,
`ai_cli_extra_args`, `ai_session_id`, `ai_settings_cli_bin_input`,
`ai_settings_cli_model_input`, `ai_settings_cli_extra_args_input`, `ai_cli_mcp_registered`,
`ai_cli_mcp_receiver`, `ai_cli_mcp_message`.

#### Field `Tabular` — tambah

```rust
/// Profil semua CLI agent, keyed by kind (selalu 4 entri).
pub ai_cli_profiles: std::collections::BTreeMap<crate::config::CliAgentKind, crate::config::CliAgentProfile>,
/// Target untuk fitur non-chat; juga fallback picker.
pub ai_default_target: crate::config::ChatTarget,
/// Pilihan picker di panel chat (dipersist sebagai `ai_chat_target`).
pub ai_chat_target: crate::config::ChatTarget,
/// Target yang dipakai giliran yang sedang berjalan; menentukan `kind` saat
/// `AgentEvent::Session` diterima.
pub ai_turn_target: Option<crate::config::ChatTarget>,
/// Sesi CLI aktif; hanya dipakai bila `kind` sama dengan target giliran berikutnya.
pub ai_session: Option<AgentSession>,
/// Tab agent yang sedang diedit di Settings (tidak dipersist).
pub ai_settings_cli_tab: crate::config::CliAgentKind,
/// Status registrasi MCP global per agent (agy, gemini).
pub ai_cli_mcp: std::collections::HashMap<crate::config::CliAgentKind, McpStatus>,
```

dengan tipe pendukung (letakkan di `models/structs.rs` atau `window_egui/mod.rs`):

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession { pub kind: CliAgentKind, pub id: String }

#[derive(Default)]
pub struct McpStatus {
    /// None = belum diperiksa; Some(true) = MCP Tabular terdaftar di CLI global.
    pub registered: Option<bool>,
    pub message: Option<String>,
    pub receiver: Option<std::sync::mpsc::Receiver<Result<bool, String>>>,
}
```

`ai_cli_test_receiver` dan `ai_cli_test_result` tetap (satu, mengikuti `ai_settings_cli_tab`;
di-reset saat tab berganti).

#### `init.rs`

- Penyalinan prefs → state (~83-95): isi `ai_cli_profiles` dari `prefs.ai_cli_profiles`
  (`.into_iter().map(|p| (p.kind, p)).collect()`), `ai_default_target`,
  `ai_chat_target = prefs.ai_chat_target.unwrap_or(prefs.ai_default_target)`.
- Nilai awal struct (~600-626): map dari `CliAgentProfile::defaults()`, target `Api`,
  `ai_turn_target: None`, `ai_session: None`, `ai_settings_cli_tab: Antigravity`,
  `ai_cli_mcp: HashMap::new()`.
- Test di ~1607-1633: ganti menjadi membangun `AppPreferences` dengan `ai_cli_profiles`
  yang memuat profil ClaudeCode `{enabled: true, bin: "/opt/bin/claude", model: "opus",
  effort: "high", extra_args: "--verbose"}` dan `ai_default_target: Cli(ClaudeCode)`; assert
  `tabular.ai_cli_profiles[&ClaudeCode]` dan `tabular.ai_chat_target`.

#### `app_impl.rs` (~4091-4096)

Ganti enam field dengan:

```rust
ai_default_target: self.ai_default_target,
ai_chat_target: Some(self.ai_chat_target),
ai_cli_profiles: self.ai_cli_profiles.values().cloned().collect(),
```

Di ~3032-3053 (HTTP client): gunakan `ai_default_target` (lihat 4.7).

### 4.4 Helper di `impl Tabular` (`src/window_egui/ai_cli_settings.rs`)

```rust
/// Profil untuk `kind`; selalu ada karena map diisi lengkap saat init.
pub(crate) fn cli_profile(&self, kind: CliAgentKind) -> &CliAgentProfile
pub(crate) fn cli_profile_mut(&mut self, kind: CliAgentKind) -> &mut CliAgentProfile
   // gunakan `entry(kind).or_insert_with(|| CliAgentProfile::new(kind))`

pub(crate) fn ai_cli_config_for(&self, kind: CliAgentKind) -> CliAgentConfig
   // CliAgentConfig::from(self.cli_profile(kind))

/// Daftar untuk picker: `Api` dulu, lalu kind enabled sesuai `CliAgentKind::ALL`.
/// Di build sandbox/mobile hanya `Api`.
pub(crate) fn enabled_chat_targets(&self) -> Vec<ChatTarget>

/// Target picker yang masih valid: `ai_chat_target` bila enabled, kalau tidak
/// `ai_default_target` bila enabled, kalau tidak `Api`.
pub(crate) fn effective_chat_target(&self) -> ChatTarget
pub(crate) fn effective_default_target(&self) -> ChatTarget   // aturan yang sama tanpa langkah pertama

pub(crate) fn target_enabled(&self, t: ChatTarget) -> bool
   // Api → true; Cli(k) → profile enabled && cli_backend_available()

pub(crate) fn mcp_registered(&self, kind: CliAgentKind) -> Option<bool>
```

Ganti `ai_cli_config()` yang lama dengan `ai_cli_config_for(self.ai_settings_cli_tab)` di
pemanggil Settings.

#### Pekerjaan latar

- `ensure_ai_mcp_check()`: loop `CliAgentKind::ALL`; untuk setiap kind yang
  `needs_global_mcp_registration()` **dan** profilnya enabled **dan** `ai_cli_mcp[kind]`
  belum punya `registered` maupun `receiver`, spawn thread `check_mcp_registered` dan simpan
  receiver di `ai_cli_mcp.entry(kind)`.
- `start_ai_mcp_register(kind)`: sama seperti sekarang tapi menerima `kind` dan menulis ke
  `ai_cli_mcp[kind]`.
- `start_ai_cli_test()`: memakai `ai_cli_config_for(self.ai_settings_cli_tab)`.
- `poll_ai_cli_background()`: iterasi semua entri `ai_cli_mcp` (kumpulkan kind dulu ke
  `Vec` supaya tidak meminjam map saat memutasi), logika per entri sama dengan yang sekarang.

### 4.5 Settings UI (`ai_cli_settings.rs` + `preferences.rs`)

Tata letak baru tab **AI Assistant**:

```
API PROVIDER              ← bagian API yang sudah ada, selalu tampil
  Provider / API key / Model / Base URL

CLI AGENTS                ← disembunyikan bila !cli_backend_available() (tampilkan callout sandbox)
  [ Antigravity (agy) ] [ Claude Code (claude) ] [ Gemini CLI (gemini) ] [ Custom command ]
  Enabled          [x] Show this agent in the chat picker
  Command / path   [ … ] Apply Detect
  Model            [ … ] Apply Default   + Quick pick chips
  Reasoning effort [ combo ]
  Extra args       [ … ] Apply
  Database Access  (hanya kind yang needs_global_mcp_registration)
  Connection Test  [Test]

DEFAULT TARGET
  Default target   [ combo: HTTP API (OpenAI) | Antigravity (agy) | … hanya yang enabled ]
                   "Used by inline --AI blocks and the HTTP client. The chat panel has its own picker."

MEMORY                    ← render_ai_memory_settings, selalu tampil
```

Perubahan konkret:

1. `render_ai_backend_settings()`: hapus radio Backend (API vs CLI). Bagian sandbox tetap
   menampilkan callout; blok "kembalikan ke API" diganti: bila sandbox, set
   `ai_default_target = Api`, `ai_chat_target = Api`, simpan.
2. `preferences.rs` ~1246-1250: selalu render API settings, lalu CLI section, lalu default
   target, lalu memory. Hilangkan `if self.ai_backend != AiBackend::Api { … return; }`.
3. `render_ai_cli_agent_rows()`: baris radio "Agent" diganti baris tab
   (`ui.selectable_value(&mut tab, kind, kind.display_name())` untuk setiap
   `CliAgentKind::ALL`). Saat tab berganti: set `ai_settings_cli_tab`, reset
   `ai_cli_test_result`. **Jangan** menghapus nilai profil apa pun.
4. Tambah baris `Enabled` (checkbox) di bawah tab. Saat dimatikan dan kind itu sedang
   menjadi `ai_default_target` / `ai_chat_target`, fallback lewat
   `effective_*_target()` sudah menangani; tidak perlu logika khusus. Simpan prefs.
5. Semua TextEdit mengedit `self.cli_profile_mut(tab).bin / .model / .extra_args` langsung
   (perhatikan borrow: ambil `tab` ke variabel lokal `Copy` dulu). Apply / lost focus →
   `save_ai_prefs()`; untuk `bin` juga reset `ai_cli_mcp[tab].registered = None`.
6. Quick pick, Default, Detect, effort combo: sama, tapi menulis ke profil tab.
7. `render_ai_cli_mcp_status()` / tombol Register / Re-check: membaca `ai_cli_mcp[tab]`
   dan memanggil `start_ai_mcp_register(tab)`.
8. Bagian baru `render_ai_default_target()` dengan `egui::ComboBox` berisi
   `enabled_chat_targets()`; label lewat `backend_label_for(self, t)` (4.6). Simpan saat
   berubah.

### 4.6 `src/ai_assistant.rs`

```rust
pub struct ChatBackend {
    /// Target yang dipilih pemanggil; `backend` adalah turunan `target.backend()`.
    pub target: ChatTarget,
    pub backend: AiBackend,
    … (field lain tetap)
}

pub fn chat_backend_for(tabular: &Tabular, target: ChatTarget) -> ChatBackend
   // Cli(kind): cli = tabular.ai_cli_config_for(kind);
   //   mcp_available = match kind { ClaudeCode => true, Custom => false,
   //                                _ => tabular.mcp_registered(kind) == Some(true) }
   // Api: cli = CliAgentConfig::default(), mcp_available = false

pub fn backend_label_for(tabular: &Tabular, target: ChatTarget) -> String
   // Api → provider.display_name(); Cli(kind) → "<bin_name>" atau "<bin_name> · <model>"
   // (logika sama dengan `backend_label` sekarang, membaca profil `kind`)

pub fn backend_ready_for(tabular: &Tabular, target: ChatTarget) -> Result<(), String>
   // Api: cek API key (pesan sama).
   // Cli(kind): sandbox → SANDBOX_UNAVAILABLE_MESSAGE;
   //   !profile.enabled → "Agent {display_name} is disabled. Enable it in Settings → AI Assistant.";
   //   Custom && bin kosong → pesan yang ada sekarang.

/// Id sesi yang boleh dipakai untuk `target`: hanya bila sesi berasal dari kind yang sama.
pub fn session_for(session: Option<&AgentSession>, target: ChatTarget) -> Option<&str>
   // match (session, target) { (Some(s), ChatTarget::Cli(k)) if s.kind == k && k.supports_resume() => Some(&s.id), _ => None }

pub fn build_chat_prompts(tabular: &Tabular, cfg: &ChatBackend, user_text: &str,
                          has_native_session: bool) -> (String, String)
   // ganti `if !cfg.keeps_history_natively()` menjadi `if !has_native_session`
```

`start_chat()` dan `request_text()` tidak berubah signature-nya; `start_chat` sudah
menerima `session_id: Option<String>` dan sudah memeriksa `supports_resume()`.
`keeps_history_natively()` boleh tetap ada, tapi tidak lagi dipakai untuk memutuskan
history (hapus bila jadi dead code, clippy tidak menandai method publik tapi jaga
kebersihan).

Hapus `chat_backend()`, `backend_label()`, `backend_ready()` lama setelah semua pemanggil
pindah.

### 4.7 Panel chat dan konsumen lain (`src/editor.rs`, `http_*`)

#### Kirim giliran (`editor.rs` ~5281-5345)

```rust
let target = tabular.effective_chat_target();
if let Err(e) = crate::ai_assistant::backend_ready_for(tabular, target) { /* seperti sekarang */ }
let cfg = crate::ai_assistant::chat_backend_for(tabular, target);
let session = crate::ai_assistant::session_for(tabular.ai_session.as_ref(), target).map(str::to_string);
let (system, user) = crate::ai_assistant::build_chat_prompts(tabular, &cfg, &text, session.is_some());
// initial_step: teks "Starting {} agent…" sudah memakai cfg.cli.kind — tetap.
tabular.ai_chat.push(AiChatMessage { role: Assistant, streaming: true,
    agent_label: Some(crate::ai_assistant::backend_label_for(tabular, target)), … });
tabular.ai_turn_target = Some(target);
match start_chat(&cfg, system, user, session) { … }
```

#### Handler event (~5132)

```rust
AgentEvent::Session(id) => {
    if let Some(ChatTarget::Cli(kind)) = tabular.ai_turn_target {
        tabular.ai_session = Some(AgentSession { kind, id });
    }
    false
}
```

Saat giliran selesai (`ai_finish_turn`): `ai_turn_target = None`.
`ai_new_chat`: `ai_session = None` (menggantikan `ai_session_id = None`).

#### Picker di header (~6165-6190)

Ganti chip backend statis dengan:

```rust
let targets = tabular.enabled_chat_targets();
let mut current = tabular.effective_chat_target();
ui.add_enabled_ui(!tabular.ai_is_loading, |ui| {
    egui::ComboBox::from_id_salt("ai_chat_target_picker")
        .selected_text(format!("{icon} {}", backend_label_for(tabular, current)))
        .show_ui(ui, |ui| {
            for t in &targets {
                ui.selectable_value(&mut current, *t, backend_label_for(tabular, *t));
            }
        });
});
if current != tabular.effective_chat_target() {
    tabular.ai_chat_target = current;
    tabular.save_ai_prefs();          // riwayat & sesi TIDAK disentuh
}
```

Ikon: `ICON_CLOUD` untuk Api, `ICON_TERMINAL` untuk Cli. Tooltip menjelaskan
"Switching agents keeps this conversation; the new agent receives a summary of it."
Bila `targets.len() == 1` (hanya API), tampilkan chip seperti sekarang tanpa combo.

Baris ringkasan ("Backend", `backend_label`) di ~6055 → `backend_label_for(tabular,
effective_chat_target())`.

#### Peringatan MCP (~6985)

```rust
if let ChatTarget::Cli(kind) = tabular.effective_chat_target()
    && kind.needs_global_mcp_registration()
    && tabular.mcp_registered(kind) == Some(false)
{ … tombol Register memanggil start_ai_mcp_register(kind) … }
```

#### Label agent per pesan

`models/structs.rs`: tambah `pub agent_label: Option<String>` pada `AiChatMessage`
(dokumentasi: "Nama backend/agent yang menjawab; hanya diisi untuk role Assistant").
Di render gelembung assistant, tampilkan sebagai teks kecil muted di baris yang sama dengan
`usage` (cari tempat `msg.usage` digambar). `ai_export_chat` ikut menuliskan label
(`**Assistant (claude · opus):**`).

#### Konsumen non-chat

- `editor.rs` ~4438-4463 (blok inline `--AI`): `let target = tabular.effective_default_target();`
  lalu `backend_ready_for` / `chat_backend_for`.
- `app_impl.rs` ~3032-3053 (HTTP client): idem.
- `http_client.rs` `ai_backend_label()` dan `http_ai.rs`: tetap membaca `backend.backend` /
  `backend.cli.kind`; tidak perlu diubah selain kompilasi.

### 4.8 Dokumentasi

- `README.md` baris ~139-141: jelaskan bahwa beberapa CLI agent bisa aktif bersamaan dan
  dipilih per pertanyaan di panel chat; default target untuk fitur lain.
- `docs/MCP.md` tabel ~118-119: kolom "Register" per agent, tidak tergantung agent aktif.

---

## 5. Urutan implementasi

Setiap fase harus berakhir dengan `cargo check` bersih. Fase 1 dan 2 boleh digabung dalam
satu commit bila kompilasi menuntut.

| Fase | Pekerjaan | File | Gate |
|---|---|---|---|
| 1 | Tipe baru, prefs, migrasi, save/load, `From<&CliAgentProfile>` | `config.rs`, `agent/harness.rs` | `cargo test --lib config::` hijau, termasuk test baru (6.1) |
| 2 | Field `Tabular`, init, save prefs, helper `impl Tabular`, pekerjaan latar per kind | `window_egui/mod.rs`, `init.rs`, `app_impl.rs`, `ai_cli_settings.rs` (bagian non-render) | `cargo check`; test `init.rs` diperbarui |
| 3 | `ai_assistant.rs`: fungsi `*_for`, `session_for`, `build_chat_prompts(has_native_session)` | `ai_assistant.rs` | test 6.2 |
| 4 | Settings UI: tab per agent, checkbox Enabled, default target combo, hapus radio Backend | `ai_cli_settings.rs`, `preferences.rs` | manual |
| 5 | Panel chat: picker, sesi per kind, `agent_label`, peringatan MCP, konsumen non-chat | `editor.rs`, `models/structs.rs`, `http_client.rs`, `http_ai.rs`, `app_impl.rs` | manual 6.3 |
| 6 | Hapus fungsi/field lama yang tersisa, dokumentasi | semua | `cargo clippy --all-targets -- -D warnings`, `cargo test` |

Jalankan juga `cargo clippy --all-targets --features collab -- -D warnings` di akhir
(CI menjalankannya). `cargo fmt` hanya pada file yang disentuh.

---

## 6. Pengujian

### 6.1 Unit test `config.rs`

- `chat_target_roundtrip`: setiap varian `as_string()` → `parse()` kembali sama; string
  tak dikenal → `Api`.
- `profiles_json_roundtrip`: `CliAgentProfile::defaults()` → JSON → kembali sama.
- `migrate_legacy_cli_enables_selected_kind`: legacy `{backend: Cli, kind: ClaudeCode,
  bin: "/x/claude", model: "opus"}` → profil ClaudeCode enabled dengan nilai itu, tiga
  lainnya disabled kosong, target `Cli(ClaudeCode)`.
- `migrate_legacy_api_backend_keeps_target_api`: legacy `{backend: Api, kind: Antigravity,
  bin: ""}` → semua disabled, target `Api`.
- `migrate_legacy_api_with_filled_bin_enables_profile`: backend Api tapi bin terisi →
  profil enabled, target tetap `Api`.
- `normalize_profiles_fills_missing_and_dedups`: input `[Claude, Claude]` → 4 entri urutan
  `ALL`, satu Claude.
- `load_prefers_new_key_over_legacy`: bila DB memuat `ai_cli_profiles` **dan** key legacy,
  hasil load = isi `ai_cli_profiles`. (Gunakan fixture load yang sudah ada di modul test
  config, atau uji fungsi parsing yang dipisah dari SQLite.)

### 6.2 Unit test `ai_assistant.rs`

- `session_for_matches_only_same_kind`: sesi `{kind: Antigravity}` + target
  `Cli(ClaudeCode)` → `None`; target `Cli(Antigravity)` → `Some(id)`; target `Api` → `None`;
  target `Cli(GeminiCli)` dengan sesi Gemini → `None` (tidak `supports_resume`).
- `build_chat_prompts_includes_history_without_native_session`: dengan `has_native_session
  = false` dan `ai_chat` berisi 2 pesan, `user` prompt memuat "## Conversation so far";
  dengan `true` tidak memuat. (Bangun `Tabular` lewat konstruktor test yang sudah dipakai
  test `init.rs`.)
- `backend_ready_for_rejects_disabled_profile`: profil ClaudeCode `enabled=false` → `Err`
  yang menyebut "disabled".
- `enabled_chat_targets_order`: enable Gemini dan agy → `[Api, Cli(Antigravity),
  Cli(GeminiCli)]`.

### 6.3 Uji manual (mesin dev ini punya `agy`, `claude`, `gemini` di PATH)

1. **Migrasi**: jalankan build lama dengan Claude Code dipilih + model `opus`, tutup, buka
   build baru → tab Claude Code enabled dengan model `opus`, picker chat menunjukkan Claude.
2. **Konfigurasi bersamaan**: isi model berbeda di tab agy dan claude, pindah tab bolak
   balik → nilai tidak hilang; restart → tetap.
3. **Pindah agent di tengah chat**: tanya ke agy "list tabel di database X"; ganti picker ke
   Claude; tanya "buat query join dari dua tabel pertama yang tadi kamu sebut" → Claude
   menjawab merujuk tabel yang disebut agy (bukti `history_prefix` masuk). Label pesan
   menunjukkan `agy` lalu `claude`.
4. **Sesi kembali**: ganti ke agy lagi, tanya lanjutan → log `[AGENT]` menunjukkan
   `--conversation <id lama>` (sesi agy masih dipakai).
5. **New chat**: sesi hilang, picker tidak berubah.
6. **Disable saat dipilih**: matikan Enabled untuk agent yang sedang dipilih di picker →
   picker jatuh ke default target / API tanpa panic; kirim pesan tetap berjalan.
7. **MCP per agent**: status Database Access untuk agy dan gemini independen; Register di
   satu tab tidak mengubah tab lain.
8. **Picker saat streaming**: combo nonaktif selama `ai_is_loading`.
9. **Inline `--AI` dan HTTP client** memakai default target, bukan pilihan picker.
10. Sandbox/mobile: bagian CLI tersembunyi, picker hanya API (cukup dicek lewat
    `cli_backend_available()` di-mock bila ada, atau review kode).

---

## 7. Kriteria selesai

- [ ] Semua item 6.1 dan 6.2 ada dan hijau; `cargo test` penuh hijau.
- [ ] `cargo clippy --all-targets -- -D warnings` dan varian `--features collab` bersih.
- [ ] Tidak ada lagi referensi ke `ai_cli_kind`, `ai_cli_bin`, `ai_cli_model`,
      `ai_cli_effort`, `ai_cli_extra_args`, `ai_session_id`, `ai_cli_mcp_registered`,
      `chat_backend(`, `backend_label(`, `backend_ready(` di `src/` (cek dengan `grep`).
- [ ] `src/agent/` tidak mengimpor apa pun dari `window_egui`.
- [ ] Skenario manual 6.3 nomor 1, 3, 4, 6 lulus.
- [ ] README dan `docs/MCP.md` diperbarui.

---

## 8. Catatan untuk implementor

- Ikuti `AGENTS.md`: komentar Bahasa Indonesia, UI Inggris, `log::warn!("[AGENT] …")` /
  `"[PREFS] …"`, jangan `unwrap()` di jalur I/O.
- Borrow checker di egui: ambil `let tab = self.ai_settings_cli_tab;` sebelum closure
  `row(ui, …, |ui| { … self.cli_profile_mut(tab) … })`. Untuk `poll_ai_cli_background`,
  kumpulkan key `ai_cli_mcp` ke `Vec<CliAgentKind>` dulu sebelum memutasi entri.
- `CliAgentKind` perlu `Ord` + `Hash` untuk `BTreeMap`/`HashMap`; derive saja.
- Jangan mengubah `build_args` / `StreamParser` / `spawn_stream`. Bila tergoda, berarti ada
  yang salah di lapisan atas.
- Hapus kode lama di fase terakhir, bukan di awal, supaya tiap fase tetap bisa
  dikompilasi dan di-review terpisah.
