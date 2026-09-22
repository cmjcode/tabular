use dirs::home_dir;
use log::debug;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite, sqlite::SqlitePoolOptions};
use std::fs;
use std::path::PathBuf;

/// File name to store the current data directory location
const CONFIG_LOCATION_FILE: &str = "config_location.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AppTheme {
    #[default]
    Dark,
    Light,
    LightSoft,
}

impl AppTheme {
    pub fn is_dark(self) -> bool {
        self == AppTheme::Dark
    }
    pub fn as_str(self) -> &'static str {
        match self {
            AppTheme::Dark => "DARK",
            AppTheme::Light => "LIGHT",
            AppTheme::LightSoft => "LIGHT_SOFT",
        }
    }
}

impl std::str::FromStr for AppTheme {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "LIGHT" => AppTheme::Light,
            "LIGHT_SOFT" => AppTheme::LightSoft,
            _ => AppTheme::Dark,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiModePreference {
    #[default]
    Auto,
    Desktop,
    TouchTablet,
}

impl UiModePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            UiModePreference::Auto => "AUTO",
            UiModePreference::Desktop => "DESKTOP",
            UiModePreference::TouchTablet => "TOUCH_TABLET",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            UiModePreference::Auto => "Automatic (screen / device)",
            UiModePreference::Desktop => "Desktop (Kompak & Mouse)",
            UiModePreference::TouchTablet => "Tablet / Touch (Area Sentuh Nyaman)",
        }
    }
}

impl std::str::FromStr for UiModePreference {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "DESKTOP" => UiModePreference::Desktop,
            "TOUCH_TABLET" | "TABLET" | "TOUCH" => UiModePreference::TouchTablet,
            _ => UiModePreference::Auto,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AiProvider {
    #[default]
    OpenAI,
    Anthropic,
    Groq,
    GitHub,
    Custom,
}

impl AiProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "OPENAI",
            AiProvider::Anthropic => "ANTHROPIC",
            AiProvider::Groq => "GROQ",
            AiProvider::GitHub => "GITHUB",
            AiProvider::Custom => "CUSTOM",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "OpenAI (ChatGPT)",
            AiProvider::Anthropic => "Anthropic (Claude)",
            AiProvider::Groq => "Groq",
            AiProvider::GitHub => "GitHub (Copilot/Models)",
            AiProvider::Custom => "Custom (OpenAI-compatible)",
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "gpt-4o-mini",
            AiProvider::Anthropic => "claude-3-haiku-20240307",
            AiProvider::Groq => "llama3-70b-8192",
            AiProvider::GitHub => "gpt-4o-mini",
            AiProvider::Custom => "gpt-4o-mini",
        }
    }
    pub fn preset_models(self) -> &'static [&'static str] {
        match self {
            AiProvider::OpenAI => &[
                "gpt-4o-mini",
                "gpt-4o",
                "gpt-4-turbo",
                "gpt-4",
                "gpt-3.5-turbo",
                "o1-mini",
                "o1",
                "o3-mini",
            ],
            AiProvider::Anthropic => &[
                "claude-3-haiku-20240307",
                "claude-3-sonnet-20240229",
                "claude-3-opus-20240229",
                "claude-3-5-sonnet-20241022",
                "claude-3-5-haiku-20241022",
            ],
            AiProvider::Groq => &[
                "llama3-70b-8192",
                "llama3-8b-8192",
                "llama-3.1-70b-versatile",
                "llama-3.3-70b-versatile",
                "mixtral-8x7b-32768",
                "gemma2-9b-it",
            ],
            AiProvider::GitHub => &[
                "gpt-4o-mini",
                "gpt-4o",
                "o1-mini",
                "o1",
                "Meta-Llama-3.1-70B-Instruct",
                "Meta-Llama-3.1-8B-Instruct",
                "Mistral-large",
                "Mistral-small",
                "Phi-3.5-mini-instruct",
                "Phi-3.5-MoE-instruct",
                "Cohere-command-r-plus",
            ],
            AiProvider::Custom => &[
                "gpt-4o-mini",
                "gpt-4o",
                "llama3",
                "mistral",
                "deepseek-coder",
            ],
        }
    }
    pub fn default_base_url(self) -> &'static str {
        match self {
            AiProvider::OpenAI => "https://api.openai.com/v1",
            AiProvider::Anthropic => "https://api.anthropic.com/v1",
            AiProvider::Groq => "https://api.groq.com/openai/v1",
            AiProvider::GitHub => "https://models.inference.ai.azure.com",
            AiProvider::Custom => "https://api.openai.com/v1",
        }
    }
    pub fn api_key_hint(self) -> &'static str {
        match self {
            AiProvider::GitHub => {
                "GitHub PAT (Settings → Developer settings → Personal access tokens)"
            }
            AiProvider::OpenAI => "sk-… (platform.openai.com/api-keys)",
            AiProvider::Anthropic => "sk-ant-… (console.anthropic.com/settings/keys)",
            AiProvider::Groq => "gsk_… (console.groq.com/keys)",
            AiProvider::Custom => "API key for your custom endpoint",
        }
    }
}

impl std::str::FromStr for AiProvider {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ANTHROPIC" => AiProvider::Anthropic,
            "GROQ" => AiProvider::Groq,
            "GITHUB" => AiProvider::GitHub,
            "CUSTOM" => AiProvider::Custom,
            _ => AiProvider::OpenAI,
        })
    }
}

/// Cara panel AI Assistant menjangkau model: lewat HTTP API langsung
/// (perilaku lama, butuh API key) atau lewat CLI agent lokal seperti
/// Antigravity (`agy`) / Claude Code yang sudah login di mesin user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AiBackend {
    #[default]
    Api,
    Cli,
}

impl AiBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            AiBackend::Api => "API",
            AiBackend::Cli => "CLI",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            AiBackend::Api => "HTTP API (API key)",
            AiBackend::Cli => "CLI Agent (agy / Claude Code / …)",
        }
    }
}

impl std::str::FromStr for AiBackend {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "CLI" => AiBackend::Cli,
            _ => AiBackend::Api,
        })
    }
}

/// Jenis CLI agent yang dipakai bila [`AiBackend::Cli`]. Menentukan argumen
/// baris perintah dan parser output stream-json (lihat `agent::harness`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub enum CliAgentKind {
    #[default]
    Antigravity,
    ClaudeCode,
    GeminiCli,
    Custom,
}

impl CliAgentKind {
    /// Urutan tampil di Settings dan picker chat.
    pub const ALL: [CliAgentKind; 4] = [
        CliAgentKind::Antigravity,
        CliAgentKind::ClaudeCode,
        CliAgentKind::GeminiCli,
        CliAgentKind::Custom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CliAgentKind::Antigravity => "AGY",
            CliAgentKind::ClaudeCode => "CLAUDE",
            CliAgentKind::GeminiCli => "GEMINI",
            CliAgentKind::Custom => "CUSTOM",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            CliAgentKind::Antigravity => "Antigravity (agy)",
            CliAgentKind::ClaudeCode => "Claude Code (claude)",
            CliAgentKind::GeminiCli => "Gemini CLI (gemini)",
            CliAgentKind::Custom => "Custom command",
        }
    }
    /// Nama binary yang dicari di PATH bila user tidak mengisi path manual.
    pub fn default_binary(self) -> &'static str {
        match self {
            CliAgentKind::Antigravity => "agy",
            CliAgentKind::ClaudeCode => "claude",
            CliAgentKind::GeminiCli => "gemini",
            CliAgentKind::Custom => "",
        }
    }
    /// Model kosong berarti biarkan CLI memakai default akunnya sendiri.
    pub fn preset_models(self) -> &'static [&'static str] {
        match self {
            CliAgentKind::Antigravity => &[
                "gemini-3.8-flash-medium",
                "gemini-3.8-flash-high",
                "gemini-3.1-pro-high",
                "claude-sonnet-4-6",
                "claude-opus-4-6-thinking",
            ],
            CliAgentKind::ClaudeCode => &["sonnet", "opus", "haiku"],
            CliAgentKind::GeminiCli => &["gemini-2.5-pro", "gemini-2.5-flash"],
            CliAgentKind::Custom => &[],
        }
    }
    /// Apakah CLI menerima flag `--effort`.
    pub fn supports_effort(self) -> bool {
        matches!(self, CliAgentKind::Antigravity | CliAgentKind::ClaudeCode)
    }
    /// Apakah percakapan bisa dilanjutkan lewat id sesi (`--conversation` /
    /// `--resume`). Gemini CLI hanya menerima index sesi, jadi tiap giliran
    /// dikirim sebagai percakapan baru.
    pub fn supports_resume(self) -> bool {
        matches!(self, CliAgentKind::Antigravity | CliAgentKind::ClaudeCode)
    }
    /// Apakah MCP server Tabular harus didaftarkan di konfigurasi global CLI
    /// (tidak bisa dikirim per-invocation seperti `claude --mcp-config`).
    pub fn needs_global_mcp_registration(self) -> bool {
        matches!(self, CliAgentKind::Antigravity | CliAgentKind::GeminiCli)
    }
}

impl std::str::FromStr for CliAgentKind {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "CLAUDE" => CliAgentKind::ClaudeCode,
            "GEMINI" => CliAgentKind::GeminiCli,
            "CUSTOM" => CliAgentKind::Custom,
            _ => CliAgentKind::Antigravity,
        })
    }
}

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
    pub fn new(kind: CliAgentKind) -> Self {
        Self {
            kind,
            enabled: false,
            bin: String::new(),
            model: String::new(),
            effort: String::new(),
            extra_args: String::new(),
        }
    }

    /// Empat profil default, urutan `CliAgentKind::ALL`.
    pub fn defaults() -> Vec<CliAgentProfile> {
        CliAgentKind::ALL.into_iter().map(Self::new).collect()
    }
}

/// Siapa yang menjawab: HTTP API (provider di prefs) atau salah satu CLI agent.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChatTarget {
    #[default]
    Api,
    Cli(CliAgentKind),
}

impl ChatTarget {
    pub fn backend(self) -> AiBackend {
        match self {
            ChatTarget::Api => AiBackend::Api,
            ChatTarget::Cli(_) => AiBackend::Cli,
        }
    }

    /// Bentuk string untuk tabel prefs: `"API"` atau `"CLI:AGY"`, `"CLI:CLAUDE"`, …
    pub fn as_string(self) -> String {
        match self {
            ChatTarget::Api => "API".to_string(),
            ChatTarget::Cli(kind) => format!("CLI:{}", kind.as_str()),
        }
    }
}

impl std::str::FromStr for ChatTarget {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "CLI:AGY" => ChatTarget::Cli(CliAgentKind::Antigravity),
            "CLI:CLAUDE" => ChatTarget::Cli(CliAgentKind::ClaudeCode),
            "CLI:GEMINI" => ChatTarget::Cli(CliAgentKind::GeminiCli),
            "CLI:CUSTOM" => ChatTarget::Cli(CliAgentKind::Custom),
            _ => ChatTarget::Api,
        })
    }
}

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
        || !legacy.bin.trim().is_empty()
        || !legacy.model.trim().is_empty()
        || !legacy.effort.trim().is_empty()
        || !legacy.extra_args.trim().is_empty();
    if had_cli {
        if let Some(p) = profiles.iter_mut().find(|p| p.kind == kind) {
            p.enabled = true;
            p.bin = legacy.bin.clone();
            p.model = legacy.model.clone();
            p.effort = legacy.effort.clone();
            p.extra_args = legacy.extra_args.clone();
        }
    }
    let target = if legacy.backend == Some(AiBackend::Cli) {
        ChatTarget::Cli(kind)
    } else {
        ChatTarget::Api
    };
    (profiles, target)
}

/// Pastikan tepat 4 entri profil, satu per kind, urutan `CliAgentKind::ALL`.
pub fn normalize_profiles(profiles: &mut Vec<CliAgentProfile>) {
    let mut normalized = Vec::with_capacity(CliAgentKind::ALL.len());
    for kind in CliAgentKind::ALL {
        if let Some(pos) = profiles.iter().position(|p| p.kind == kind) {
            normalized.push(profiles.remove(pos));
        } else {
            normalized.push(CliAgentProfile::new(kind));
        }
    }
    *profiles = normalized;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPreferences {
    #[serde(default)]
    pub theme: AppTheme,
    #[serde(default)]
    pub ui_mode: UiModePreference,
    pub link_editor_theme: bool,
    pub editor_theme: String,
    pub font_size: f32,
    pub word_wrap: bool,
    pub data_directory: Option<String>,
    pub auto_check_updates: bool,
    pub use_server_pagination: bool,
    // RFC3339 timestamp of the last time we checked GitHub releases (persisted)
    pub last_update_check_iso: Option<String>,
    #[serde(default)]
    pub enable_debug_logging: bool,
    // AI Assistant settings
    #[serde(default)]
    pub ai_api_key: String,
    #[serde(default)]
    pub ai_model: String,
    #[serde(default)]
    pub ai_provider: AiProvider,
    #[serde(default)]
    pub ai_base_url: String,
    /// Target untuk fitur non-chat (blok inline `--AI`, HTTP client) dan nilai awal picker.
    #[serde(default)]
    pub ai_default_target: ChatTarget,
    /// Pilihan terakhir di picker panel chat; `None` berarti ikut `ai_default_target`.
    #[serde(default)]
    pub ai_chat_target: Option<ChatTarget>,
    /// Profil semua CLI agent; selalu 4 entri (satu per `CliAgentKind`).
    #[serde(default = "CliAgentProfile::defaults")]
    pub ai_cli_profiles: Vec<CliAgentProfile>,
    /// Tulis blok `sql tabular:tab=…` dari agent langsung ke editor saat streaming.
    #[serde(default = "default_true")]
    pub ai_cli_auto_apply_edits: bool,
    /// Folder vault Obsidian yang dipakai sebagai memory AI; kosong berarti
    /// belum dipilih. Path lokal per mesin, tidak ikut sync.
    #[serde(default)]
    pub ai_obsidian_vault_path: String,
    /// Sertakan catatan vault yang relevan di prompt dan buka tool notes MCP.
    #[serde(default)]
    pub ai_obsidian_enabled: bool,
    /// Izinkan AI menulis catatan baru ke `<vault>/Tabular Memory/`.
    #[serde(default)]
    pub ai_obsidian_allow_write: bool,
    #[serde(default = "default_redis_browser_auto_refresh_seconds")]
    pub redis_browser_auto_refresh_seconds: u32,
    #[serde(default)]
    pub sync_server_url: Option<String>,
    /// Timeout query per statement dalam detik; 0 berarti tanpa batas.
    #[serde(default)]
    pub query_timeout_secs: u32,
    /// Jumlah baris maksimum yang disimpan dari satu result set tanpa paginasi.
    #[serde(default = "default_max_result_rows")]
    pub max_result_rows: u32,
    /// Buka kembali tab query dari sesi sebelumnya (termasuk draft yang belum disimpan).
    #[serde(default = "default_true")]
    pub restore_session: bool,
    /// Lebar panel AI Assistant di sebelah kanan (pixel).
    #[serde(default = "default_ai_panel_width")]
    pub ai_panel_width: f32,
}

fn default_ai_panel_width() -> f32 {
    350.0
}

fn default_redis_browser_auto_refresh_seconds() -> u32 {
    5
}

pub const DEFAULT_MAX_RESULT_ROWS: u32 = 50_000;

fn default_max_result_rows() -> u32 {
    DEFAULT_MAX_RESULT_ROWS
}

fn default_true() -> bool {
    true
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            theme: AppTheme::Dark,
            ui_mode: UiModePreference::Auto,
            link_editor_theme: true,
            editor_theme: "GITHUB_DARK".into(),
            font_size: 14.0,
            word_wrap: true,
            data_directory: None,
            auto_check_updates: true,
            use_server_pagination: true,
            last_update_check_iso: None,
            enable_debug_logging: false,
            ai_api_key: String::new(),
            ai_model: String::new(),
            ai_provider: AiProvider::OpenAI,
            ai_base_url: String::new(),
            ai_default_target: ChatTarget::Api,
            ai_chat_target: None,
            ai_cli_profiles: CliAgentProfile::defaults(),
            ai_cli_auto_apply_edits: true,
            ai_obsidian_vault_path: String::new(),
            ai_obsidian_enabled: false,
            ai_obsidian_allow_write: false,
            redis_browser_auto_refresh_seconds: default_redis_browser_auto_refresh_seconds(),
            sync_server_url: Some("https://api.tabular.id".to_string()),
            query_timeout_secs: 0,
            max_result_rows: DEFAULT_MAX_RESULT_ROWS,
            restore_session: true,
            ai_panel_width: default_ai_panel_width(),
        }
    }
}

pub(crate) fn apply_kv_pair(
    prefs: &mut AppPreferences,
    legacy: &mut LegacyCliPrefs,
    saw_profiles: &mut bool,
    ai_key_rewrite: &mut Option<String>,
    k: &str,
    v: &str,
) {
    match k {
        "theme" => prefs.theme = v.parse().unwrap_or(AppTheme::Dark),
        "ui_mode" => prefs.ui_mode = v.parse().unwrap_or(UiModePreference::Auto),
        // Legacy migration: old boolean flags
        "is_dark_mode" => {
            if v != "1" {
                prefs.theme = AppTheme::Light;
            }
        }
        "is_light_soft" => {
            if v == "1" {
                prefs.theme = AppTheme::LightSoft;
            }
        }
        "link_editor_theme" => prefs.link_editor_theme = v == "1",
        "editor_theme" => prefs.editor_theme = v.to_string(),
        "font_size" => prefs.font_size = v.parse().unwrap_or(14.0),
        "word_wrap" => prefs.word_wrap = v == "1",
        "data_directory" => {
            prefs.data_directory = if v.is_empty() { None } else { Some(v.to_string()) }
        }
        "auto_check_updates" => prefs.auto_check_updates = v == "1",
        "use_server_pagination" => prefs.use_server_pagination = v == "1",
        "last_update_check_iso" => {
            prefs.last_update_check_iso = if v.is_empty() { None } else { Some(v.to_string()) }
        }
        "enable_debug_logging" => prefs.enable_debug_logging = v == "1",
        "ai_api_key" => {
            let (real, rewrite) =
                crate::secrets::resolve_stored("pref:ai_api_key", v);
            prefs.ai_api_key = real;
            *ai_key_rewrite = rewrite;
        }
        "ai_model" => prefs.ai_model = v.to_string(),
        "ai_provider" => {
            prefs.ai_provider = v.parse().unwrap_or(AiProvider::OpenAI)
        }
        "ai_base_url" => prefs.ai_base_url = v.to_string(),
        "ai_backend" => legacy.backend = Some(v.parse().unwrap_or(AiBackend::Api)),
        "ai_cli_kind" => {
            legacy.kind = Some(v.parse().unwrap_or(CliAgentKind::Antigravity))
        }
        "ai_cli_bin" => legacy.bin = v.to_string(),
        "ai_cli_model" => legacy.model = v.to_string(),
        "ai_cli_effort" => legacy.effort = v.to_string(),
        "ai_cli_extra_args" => legacy.extra_args = v.to_string(),
        "ai_cli_profiles" => {
            match serde_json::from_str::<Vec<CliAgentProfile>>(v) {
                Ok(parsed) => {
                    prefs.ai_cli_profiles = parsed;
                    *saw_profiles = true;
                }
                Err(e) => {
                    log::warn!("[PREFS] Failed to parse ai_cli_profiles JSON: {e}");
                }
            }
        }
        "ai_default_target" => {
            prefs.ai_default_target = v.parse().unwrap_or_default();
        }
        "ai_chat_target" => {
            prefs.ai_chat_target = if v.trim().is_empty() {
                None
            } else {
                Some(v.parse().unwrap_or_default())
            };
        }
        "ai_cli_auto_apply_edits" => prefs.ai_cli_auto_apply_edits = v == "1",
        "ai_obsidian_vault_path" => prefs.ai_obsidian_vault_path = v.to_string(),
        "ai_obsidian_enabled" => prefs.ai_obsidian_enabled = v == "1",
        "ai_obsidian_allow_write" => prefs.ai_obsidian_allow_write = v == "1",
        "redis_browser_auto_refresh_seconds" => {
            prefs.redis_browser_auto_refresh_seconds = v
                .parse()
                .unwrap_or(default_redis_browser_auto_refresh_seconds())
        }
        "sync_server_url" => {
            prefs.sync_server_url = if v.is_empty() { None } else { Some(v.to_string()) }
        }
        "query_timeout_secs" => prefs.query_timeout_secs = v.parse().unwrap_or(0),
        "max_result_rows" => {
            prefs.max_result_rows = v.parse().unwrap_or(DEFAULT_MAX_RESULT_ROWS)
        }
        "restore_session" => prefs.restore_session = v == "1",
        "ai_panel_width" => {
            prefs.ai_panel_width = v
                .parse()
                .unwrap_or_else(|_| default_ai_panel_width())
                .clamp(280.0, 800.0);
        }
        _ => {}
    }
}

pub fn preferences_from_kv<'a>(rows: impl IntoIterator<Item = (&'a str, &'a str)>) -> AppPreferences {
    let mut prefs = AppPreferences::default();
    let mut legacy = LegacyCliPrefs::default();
    let mut saw_profiles = false;
    let mut rewrite = None;
    for (k, v) in rows {
        apply_kv_pair(&mut prefs, &mut legacy, &mut saw_profiles, &mut rewrite, k, v);
    }
    if !saw_profiles {
        let (profiles, target) = migrate_legacy_cli_prefs(&legacy);
        prefs.ai_cli_profiles = profiles;
        prefs.ai_default_target = target;
    }
    normalize_profiles(&mut prefs.ai_cli_profiles);
    prefs
}

pub struct ConfigStore {
    pub pool: Option<Pool<Sqlite>>,
    use_json_fallback: bool,
}

impl ConfigStore {
    pub async fn new() -> Result<Self, sqlx::Error> {
        let mut path = config_dir();

        // Create directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&path) {
            log::error!(
                "Failed to create config directory {}: {}",
                path.display(),
                e
            );
            // Use JSON fallback if directory creation fails
            return Ok(Self {
                pool: None,
                use_json_fallback: true,
            });
        }

        path.push("preferences.db");

        // Try to create the file first if it doesn't exist
        if !path.exists()
            && let Err(e) = std::fs::File::create(&path)
        {
            log::error!(
                "Failed to create database file {}: {}, using JSON fallback",
                path.display(),
                e
            );
            return Ok(Self {
                pool: None,
                use_json_fallback: true,
            });
        }

        // Use file:// protocol with absolute path
        let url = format!("sqlite://{}?mode=rwc", path.to_string_lossy());

        log::debug!("Attempting to create/open database at: {}", url);

        let connect_opts =
            match <sqlx::sqlite::SqliteConnectOptions as std::str::FromStr>::from_str(&url) {
                Ok(opts) => opts
                    .create_if_missing(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
                    .busy_timeout(std::time::Duration::from_secs(5)),
                Err(e) => {
                    log::warn!("Invalid SQLite URL ({}), using JSON storage instead", e);
                    return Ok(Self {
                        pool: None,
                        use_json_fallback: true,
                    });
                }
            };

        match SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts)
            .await
        {
            Ok(pool) => {
                match sqlx::query("CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
                    .execute(&pool)
                    .await
                {
                    Ok(_) => {
                        log::debug!("Config store initialized successfully with SQLite");
                        Ok(Self { pool: Some(pool), use_json_fallback: false })
                    }
                    Err(e) => {
                        log::error!("Failed to create table: {}, falling back to JSON", e);
                        Ok(Self { pool: None, use_json_fallback: true })
                    }
                }
            }
            Err(e) => {
                log::warn!("SQLite unavailable ({}), using JSON storage instead", e);
                Ok(Self { pool: None, use_json_fallback: true })
            }
        }
    }

    pub async fn load(&self) -> AppPreferences {
        if self.use_json_fallback {
            return self.load_from_json().unwrap_or_default();
        }

        if let Some(ref pool) = self.pool {
            let mut prefs = AppPreferences {
                theme: AppTheme::Dark,
                link_editor_theme: true,
                editor_theme: "GITHUB_DARK".into(),
                font_size: 14.0,
                word_wrap: true,
                data_directory: None,
                auto_check_updates: true,
                use_server_pagination: true, // Default to true for better performance
                last_update_check_iso: None,
                enable_debug_logging: false,
                ai_api_key: String::new(),
                ai_model: String::new(),
                ai_provider: AiProvider::OpenAI,
                ai_base_url: String::new(),
                ai_default_target: ChatTarget::Api,
                ai_chat_target: None,
                ai_cli_profiles: CliAgentProfile::defaults(),
                ai_cli_auto_apply_edits: true,
                ai_obsidian_vault_path: String::new(),
                ai_obsidian_enabled: false,
                ai_obsidian_allow_write: false,
                redis_browser_auto_refresh_seconds: default_redis_browser_auto_refresh_seconds(),
                sync_server_url: Some("https://api.tabular.id".to_string()),
                ui_mode: UiModePreference::Auto,
                query_timeout_secs: 0,
                max_result_rows: DEFAULT_MAX_RESULT_ROWS,
                restore_session: true,
                ai_panel_width: default_ai_panel_width(),
            };

            // Set when a legacy plaintext AI key was migrated to the secret
            // store during this load; the row is rewritten below.
            let mut ai_key_rewrite: Option<String> = None;
            let mut legacy = LegacyCliPrefs::default();
            let mut saw_profiles = false;

            if let Ok(rows) = sqlx::query("SELECT key, value FROM preferences")
                .fetch_all(pool)
                .await
            {
                for row in rows {
                    let k: String = row.get(0);
                    let v: String = row.get(1);
                    apply_kv_pair(&mut prefs, &mut legacy, &mut saw_profiles, &mut ai_key_rewrite, &k, &v);
                }
            }

            if !saw_profiles {
                let (profiles, target) = migrate_legacy_cli_prefs(&legacy);
                prefs.ai_cli_profiles = profiles;
                prefs.ai_default_target = target;
            }
            normalize_profiles(&mut prefs.ai_cli_profiles);

            if let Some(value) = ai_key_rewrite {
                let _ = sqlx::query("REPLACE INTO preferences (key,value) VALUES (?,?)")
                    .bind("ai_api_key")
                    .bind(value)
                    .execute(pool)
                    .await;
            }

            debug!(
                "Loaded prefs from SQLite: theme={:?}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}, use_server_pagination={}, enable_debug_logging={}",
                prefs.theme,
                prefs.link_editor_theme,
                prefs.editor_theme,
                prefs.font_size,
                prefs.word_wrap,
                prefs.data_directory,
                prefs.auto_check_updates,
                prefs.use_server_pagination,
                prefs.enable_debug_logging
            );
            return prefs;
        }

        // Fallback to JSON
        let path = Self::json_path();
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(mut prefs) = serde_json::from_str::<AppPreferences>(&content)
        {
            // Resolve AI API key from keyring/file if stored as sentinel
            let (real, rewrite) =
                crate::secrets::resolve_stored("pref:ai_api_key", &prefs.ai_api_key);
            prefs.ai_api_key = real;
            if rewrite.is_some() {
                let _ = self.save_to_json(&prefs);
            }
            normalize_profiles(&mut prefs.ai_cli_profiles);
            return prefs;
        }
        AppPreferences::default()
    }

    pub async fn save(&self, prefs: &AppPreferences) {
        if self.use_json_fallback {
            let _ = self.save_to_json(prefs);
            debug!(
                "Saved prefs to JSON: theme={:?}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}, use_server_pagination={}, enable_debug_logging={}",
                prefs.theme,
                prefs.link_editor_theme,
                prefs.editor_theme,
                prefs.font_size,
                prefs.word_wrap,
                prefs.data_directory,
                prefs.auto_check_updates,
                prefs.use_server_pagination,
                prefs.enable_debug_logging
            );
            return;
        }

        if let Some(ref pool) = self.pool {
            let font_size_string = prefs.font_size.to_string();
            let redis_browser_auto_refresh_seconds =
                prefs.redis_browser_auto_refresh_seconds.to_string();
            // The key goes to the OS keychain; the row keeps only a sentinel.
            let ai_api_key_stored =
                crate::secrets::store_or_keep("pref:ai_api_key", &prefs.ai_api_key);
            let query_timeout_secs = prefs.query_timeout_secs.to_string();
            let max_result_rows = prefs.max_result_rows.to_string();
            let ai_default_target_str = prefs.ai_default_target.as_string();
            let ai_chat_target_str = prefs
                .ai_chat_target
                .map(|t| t.as_string())
                .unwrap_or_default();
            let ai_cli_profiles_json = match serde_json::to_string(&prefs.ai_cli_profiles) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("[PREFS] Failed to serialize ai_cli_profiles: {e}");
                    String::new()
                }
            };
            let ai_panel_width_str = prefs.ai_panel_width.to_string();
            let entries: [(&str, &str); 27] = [
                ("theme", prefs.theme.as_str()),
                ("ui_mode", prefs.ui_mode.as_str()),
                (
                    "link_editor_theme",
                    if prefs.link_editor_theme { "1" } else { "0" },
                ),
                ("editor_theme", prefs.editor_theme.as_str()),
                ("font_size", &font_size_string),
                ("word_wrap", if prefs.word_wrap { "1" } else { "0" }),
                (
                    "data_directory",
                    prefs.data_directory.as_deref().unwrap_or(""),
                ),
                (
                    "auto_check_updates",
                    if prefs.auto_check_updates { "1" } else { "0" },
                ),
                (
                    "use_server_pagination",
                    if prefs.use_server_pagination {
                        "1"
                    } else {
                        "0"
                    },
                ),
                (
                    "enable_debug_logging",
                    if prefs.enable_debug_logging { "1" } else { "0" },
                ),
                ("ai_api_key", ai_api_key_stored.as_str()),
                ("ai_model", prefs.ai_model.as_str()),
                ("ai_provider", prefs.ai_provider.as_str()),
                ("ai_base_url", prefs.ai_base_url.as_str()),
                ("ai_default_target", ai_default_target_str.as_str()),
                ("ai_chat_target", ai_chat_target_str.as_str()),
                ("ai_cli_profiles", ai_cli_profiles_json.as_str()),
                (
                    "ai_cli_auto_apply_edits",
                    if prefs.ai_cli_auto_apply_edits {
                        "1"
                    } else {
                        "0"
                    },
                ),
                (
                    "ai_obsidian_vault_path",
                    prefs.ai_obsidian_vault_path.as_str(),
                ),
                (
                    "ai_obsidian_enabled",
                    if prefs.ai_obsidian_enabled { "1" } else { "0" },
                ),
                (
                    "ai_obsidian_allow_write",
                    if prefs.ai_obsidian_allow_write {
                        "1"
                    } else {
                        "0"
                    },
                ),
                (
                    "redis_browser_auto_refresh_seconds",
                    &redis_browser_auto_refresh_seconds,
                ),
                (
                    "sync_server_url",
                    prefs.sync_server_url.as_deref().unwrap_or(""),
                ),
                ("query_timeout_secs", &query_timeout_secs),
                ("max_result_rows", &max_result_rows),
                (
                    "restore_session",
                    if prefs.restore_session { "1" } else { "0" },
                ),
                ("ai_panel_width", &ai_panel_width_str),
            ];

            for (k, v) in entries.iter() {
                let _ = sqlx::query("REPLACE INTO preferences (key,value) VALUES (?,?)")
                    .bind(k)
                    .bind(v)
                    .execute(pool)
                    .await;
            }

            // Persist last_update_check_iso if present so it isn't lost on preference save
            if let Some(ref iso) = prefs.last_update_check_iso {
                let _ = sqlx::query("REPLACE INTO preferences (key,value) VALUES (?,?)")
                    .bind("last_update_check_iso")
                    .bind(iso)
                    .execute(pool)
                    .await;
            }

            // Mirror to JSON for fast-path startup loading
            let _ = self.save_to_json(prefs);

            debug!(
                "Saved prefs to SQLite: theme={:?}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}, enable_debug_logging={}",
                prefs.theme,
                prefs.link_editor_theme,
                prefs.editor_theme,
                prefs.font_size,
                prefs.word_wrap,
                prefs.data_directory,
                prefs.auto_check_updates,
                prefs.enable_debug_logging
            );
        }
    }

    pub fn json_path() -> PathBuf {
        let mut path = config_dir();
        path.push("preferences.json");
        path
    }

    fn load_from_json(&self) -> Result<AppPreferences, Box<dyn std::error::Error>> {
        let path = Self::json_path();
        let content = std::fs::read_to_string(&path)?;
        let mut prefs: AppPreferences = serde_json::from_str(&content)?;
        let (real, rewrite) = crate::secrets::resolve_stored("pref:ai_api_key", &prefs.ai_api_key);
        if let Some(value) = rewrite {
            // Legacy plaintext key migrated to the secret store: rewrite the
            // JSON file so it only holds the sentinel.
            let mut sanitized = prefs.clone();
            sanitized.ai_api_key = value;
            if let Ok(json) = serde_json::to_string_pretty(&sanitized) {
                let _ = std::fs::write(&path, json);
            }
        }
        prefs.ai_api_key = real;
        debug!(
            "Loaded prefs from JSON: theme={:?}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}",
            prefs.theme,
            prefs.link_editor_theme,
            prefs.editor_theme,
            prefs.font_size,
            prefs.word_wrap,
            prefs.data_directory,
            prefs.auto_check_updates
        );
        Ok(prefs)
    }

    fn save_to_json(&self, prefs: &AppPreferences) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::json_path();
        // The key goes to the OS keychain; the file keeps only a sentinel.
        let mut sanitized = prefs.clone();
        sanitized.ai_api_key = crate::secrets::store_or_keep("pref:ai_api_key", &prefs.ai_api_key);
        let content = serde_json::to_string_pretty(&sanitized)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get the RFC3339 string of last update check, if any
    pub async fn get_last_update_check(&self) -> Option<String> {
        if self.use_json_fallback {
            if let Ok(p) = self.load_from_json() {
                return p.last_update_check_iso;
            }
            return None;
        }
        if let Some(ref pool) = self.pool
            && let Ok(row) =
                sqlx::query_as::<_, (String,)>("SELECT value FROM preferences WHERE key = ?")
                    .bind("last_update_check_iso")
                    .fetch_optional(pool)
                    .await
        {
            return row.map(|(v,)| v).filter(|s| !s.is_empty());
        }
        None
    }

    /// Set last update check to now (UTC) and persist
    pub async fn set_last_update_check_now(&self) {
        let now = chrono::Utc::now().to_rfc3339();
        if self.use_json_fallback {
            // Load, update, then save back to JSON
            let mut prefs = self.load_from_json().unwrap_or_default();
            prefs.last_update_check_iso = Some(now);
            let _ = self.save_to_json(&prefs);
            return;
        }
        if let Some(ref pool) = self.pool {
            let _ = sqlx::query("REPLACE INTO preferences (key,value) VALUES (?,?)")
                .bind("last_update_check_iso")
                .bind(now)
                .execute(pool)
                .await;
        }
    }
}

/// Pengaturan vault Obsidian yang dibutuhkan proses headless (`tabular mcp`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObsidianSettings {
    pub vault_path: String,
    pub enabled: bool,
    pub allow_write: bool,
}

impl ObsidianSettings {
    /// Root vault bila fitur aktif dan folder sudah dipilih.
    pub fn active_root(&self) -> Option<PathBuf> {
        (self.enabled && !self.vault_path.trim().is_empty())
            .then(|| PathBuf::from(self.vault_path.trim()))
    }

    fn from_json(content: &str) -> Self {
        let value: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
        Self {
            vault_path: value["ai_obsidian_vault_path"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            enabled: value["ai_obsidian_enabled"].as_bool().unwrap_or(false),
            allow_write: value["ai_obsidian_allow_write"].as_bool().unwrap_or(false),
        }
    }

    /// Baca dari `preferences.json` (cermin yang ditulis GUI tiap kali
    /// preferensi disimpan). Sengaja tidak lewat [`ConfigStore::load`] supaya
    /// proses headless tidak menyentuh keychain, dan dibaca ulang tiap
    /// pemanggilan supaya perubahan toggle di GUI langsung berlaku.
    pub fn load_headless() -> Self {
        std::fs::read_to_string(ConfigStore::json_path())
            .map(|content| Self::from_json(&content))
            .unwrap_or_default()
    }
}

/// Local config directory (~/.tabular) — never the custom data dir.
///
/// Use this for files that must NOT be cloud-synced (e.g. `secrets.key`).
/// If the custom data dir points to Google Drive / Dropbox / etc., using
/// `get_data_dir()` for key material causes sync conflicts and key loss.
pub fn get_local_data_dir() -> PathBuf {
    get_default_tabular_dir()
}

/// Get the default tabular directory in home folder (or Documents on iOS)
fn get_default_tabular_dir() -> PathBuf {
    #[cfg(target_os = "ios")]
    {
        if let Some(doc) = dirs::document_dir() {
            return doc.join(".tabular");
        }
    }

    if let Some(mut hd) = home_dir() {
        hd.push(".tabular");
        hd
    } else {
        PathBuf::from(".tabular")
    }
}

/// Save the current data directory location to ~/.tabular/config_location.txt
fn save_config_location(data_dir: &str) -> Result<(), String> {
    let default_dir = get_default_tabular_dir();

    // Create ~/.tabular directory if it doesn't exist
    if let Err(e) = fs::create_dir_all(&default_dir) {
        return Err(format!(
            "Cannot create default directory {}: {}",
            default_dir.display(),
            e
        ));
    }

    let config_file = default_dir.join(CONFIG_LOCATION_FILE);

    // Write the new data directory location
    if let Err(e) = fs::write(&config_file, data_dir) {
        return Err(format!("Cannot write config location file: {}", e));
    }

    log::debug!(
        "Saved config location: {} -> {}",
        config_file.display(),
        data_dir
    );
    Ok(())
}

/// Load the saved data directory location from ~/.tabular/config_location.txt
fn load_config_location() -> Option<String> {
    let default_dir = get_default_tabular_dir();
    let config_file = default_dir.join(CONFIG_LOCATION_FILE);

    if config_file.exists() {
        match fs::read_to_string(&config_file) {
            Ok(content) => {
                let path_str = content.trim();
                let path = PathBuf::from(path_str);
                if !path_str.is_empty() && path.exists() {
                    // Check if path is actually accessible and writable.
                    // In sandboxed environments (TestFlight / App Store), accessing paths
                    // outside the sandbox container (e.g. Google Drive, external volumes)
                    // fails with EPERM / PermissionDenied.
                    let test_data = path.join("data");
                    if fs::create_dir_all(&test_data).is_err() {
                        log::warn!(
                            "Config location path is not writable (App Sandbox restriction?): {}",
                            path_str
                        );
                        return None;
                    }

                    log::debug!(
                        "Loaded config location from {}: {}",
                        config_file.display(),
                        path_str
                    );
                    return Some(path_str.to_string());
                } else {
                    log::warn!("Config location file contains invalid path: {}", path_str);
                    // Remove invalid config file
                    let _ = fs::remove_file(&config_file);
                }
            }
            Err(e) => {
                log::error!(
                    "Failed to read config location file {}: {}",
                    config_file.display(),
                    e
                );
            }
        }
    }
    None
}

/// Initialize data directory from saved config or environment variable
pub fn init_data_dir() {
    // First check if there's a saved config location
    if let Some(saved_location) = load_config_location() {
        log::debug!("Using saved config location: {}", saved_location);
        unsafe {
            std::env::set_var("TABULAR_DATA_DIR", &saved_location);
        }
        return;
    }

    // If no saved location, check environment variable
    if let Ok(env_dir) = std::env::var("TABULAR_DATA_DIR") {
        log::debug!("Using environment variable TABULAR_DATA_DIR: {}", env_dir);
        return;
    }

    // Otherwise use default ~/.tabular
    let default_dir = get_default_tabular_dir();
    log::debug!("Using default data directory: {}", default_dir.display());
}

fn config_dir() -> PathBuf {
    get_data_dir()
}

pub fn get_data_dir() -> PathBuf {
    // Try to get custom data directory from environment variable first
    if let Ok(custom_dir) = std::env::var("TABULAR_DATA_DIR") {
        let path = PathBuf::from(custom_dir);
        if path.is_absolute() {
            return path;
        }
    }

    // Default to ~/.tabular (or Documents/.tabular on iOS)
    get_default_tabular_dir()
}

pub fn set_data_dir(new_path: &str) -> Result<(), String> {
    let path = PathBuf::from(new_path);

    // Validate that the path is absolute and accessible
    if !path.is_absolute() {
        return Err("Path must be absolute".to_string());
    }

    // Try to create the directory if it doesn't exist
    if let Err(e) = std::fs::create_dir_all(&path) {
        return Err(format!("Cannot create directory: {}", e));
    }

    // Check if we can write to the directory
    let test_file = path.join(".test_write");
    if let Err(e) = std::fs::write(&test_file, "test") {
        return Err(format!("Cannot write to directory: {}", e));
    }

    // Clean up test file
    let _ = std::fs::remove_file(&test_file);

    // Save the location persistently to ~/.tabular/config_location.txt
    if let Err(e) = save_config_location(new_path) {
        log::error!("Failed to save config location: {}", e);
        // Continue anyway, at least set environment variable
    }

    // Set environment variable for this session
    unsafe {
        std::env::set_var("TABULAR_DATA_DIR", new_path);
    }

    log::debug!("Data directory changed to: {}", new_path);
    Ok(())
}

/// Load preferences quickly on startup without initializing Tokio runtime or SQLite pool.
pub fn load_fast_preferences() -> AppPreferences {
    let path = ConfigStore::json_path();
    if let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(mut prefs) = serde_json::from_str::<AppPreferences>(&content)
    {
        // Quickly resolve AI key if present
        if prefs.ai_api_key == crate::secrets::SECRET_SENTINEL {
            prefs.ai_api_key = crate::secrets::get_secret("pref:ai_api_key").unwrap_or_default();
        }
        return prefs;
    }
    AppPreferences::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidian_settings_read_from_prefs_json_mirror() {
        let prefs = AppPreferences {
            ai_obsidian_vault_path: "/vaults/work".into(),
            ai_obsidian_enabled: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&prefs).expect("serialize prefs");
        let settings = ObsidianSettings::from_json(&json);
        assert_eq!(settings.vault_path, "/vaults/work");
        assert!(settings.enabled && !settings.allow_write);
        assert_eq!(settings.active_root(), Some(PathBuf::from("/vaults/work")));

        // Mati, belum dipilih, atau file rusak -> tidak ada vault aktif.
        let off = ObsidianSettings {
            enabled: false,
            ..settings.clone()
        };
        assert_eq!(off.active_root(), None);
        assert_eq!(ObsidianSettings::from_json("{}").active_root(), None);
        assert_eq!(
            ObsidianSettings::from_json("not json"),
            ObsidianSettings::default()
        );
    }

    #[test]
    fn chat_target_roundtrip() {
        let targets = [
            ChatTarget::Api,
            ChatTarget::Cli(CliAgentKind::Antigravity),
            ChatTarget::Cli(CliAgentKind::ClaudeCode),
            ChatTarget::Cli(CliAgentKind::GeminiCli),
            ChatTarget::Cli(CliAgentKind::Custom),
        ];
        for t in targets {
            let s = t.as_string();
            let parsed: ChatTarget = s.parse().unwrap();
            assert_eq!(t, parsed);
        }
        // Unknown strings fallback to Api
        assert_eq!("".parse::<ChatTarget>().unwrap(), ChatTarget::Api);
        assert_eq!("UNKNOWN".parse::<ChatTarget>().unwrap(), ChatTarget::Api);
        assert_eq!("CLI:UNKNOWN".parse::<ChatTarget>().unwrap(), ChatTarget::Api);
    }

    #[test]
    fn profiles_json_roundtrip() {
        let defaults = CliAgentProfile::defaults();
        let json = serde_json::to_string(&defaults).expect("serialize defaults");
        let restored: Vec<CliAgentProfile> =
            serde_json::from_str(&json).expect("deserialize defaults");
        assert_eq!(defaults, restored);
    }

    #[test]
    fn migrate_legacy_cli_enables_selected_kind() {
        let legacy = LegacyCliPrefs {
            backend: Some(AiBackend::Cli),
            kind: Some(CliAgentKind::ClaudeCode),
            bin: "/x/claude".into(),
            model: "opus".into(),
            effort: "high".into(),
            extra_args: "--verbose".into(),
        };
        let (profiles, target) = migrate_legacy_cli_prefs(&legacy);
        assert_eq!(target, ChatTarget::Cli(CliAgentKind::ClaudeCode));
        assert_eq!(profiles.len(), 4);
        let claude = profiles
            .iter()
            .find(|p| p.kind == CliAgentKind::ClaudeCode)
            .unwrap();
        assert!(claude.enabled);
        assert_eq!(claude.bin, "/x/claude");
        assert_eq!(claude.model, "opus");
        assert_eq!(claude.effort, "high");
        assert_eq!(claude.extra_args, "--verbose");

        for p in profiles.iter().filter(|p| p.kind != CliAgentKind::ClaudeCode) {
            assert!(!p.enabled);
            assert!(p.bin.is_empty());
        }
    }

    #[test]
    fn migrate_legacy_api_backend_keeps_target_api() {
        let legacy = LegacyCliPrefs {
            backend: Some(AiBackend::Api),
            kind: Some(CliAgentKind::Antigravity),
            bin: String::new(),
            model: String::new(),
            effort: String::new(),
            extra_args: String::new(),
        };
        let (profiles, target) = migrate_legacy_cli_prefs(&legacy);
        assert_eq!(target, ChatTarget::Api);
        for p in profiles {
            assert!(!p.enabled);
        }
    }

    #[test]
    fn migrate_legacy_api_with_filled_bin_enables_profile() {
        let legacy = LegacyCliPrefs {
            backend: Some(AiBackend::Api),
            kind: Some(CliAgentKind::Antigravity),
            bin: "/usr/local/bin/agy".into(),
            model: String::new(),
            effort: String::new(),
            extra_args: String::new(),
        };
        let (profiles, target) = migrate_legacy_cli_prefs(&legacy);
        assert_eq!(target, ChatTarget::Api);
        let agy = profiles
            .iter()
            .find(|p| p.kind == CliAgentKind::Antigravity)
            .unwrap();
        assert!(agy.enabled);
        assert_eq!(agy.bin, "/usr/local/bin/agy");
    }

    #[test]
    fn normalize_profiles_fills_missing_and_dedups() {
        let mut profiles = vec![
            CliAgentProfile {
                kind: CliAgentKind::ClaudeCode,
                enabled: true,
                bin: "claude1".into(),
                ..CliAgentProfile::new(CliAgentKind::ClaudeCode)
            },
            CliAgentProfile {
                kind: CliAgentKind::ClaudeCode,
                enabled: false,
                bin: "claude2".into(),
                ..CliAgentProfile::new(CliAgentKind::ClaudeCode)
            },
        ];
        normalize_profiles(&mut profiles);
        assert_eq!(profiles.len(), 4);
        assert_eq!(profiles[0].kind, CliAgentKind::Antigravity);
        assert_eq!(profiles[1].kind, CliAgentKind::ClaudeCode);
        assert_eq!(profiles[2].kind, CliAgentKind::GeminiCli);
        assert_eq!(profiles[3].kind, CliAgentKind::Custom);
        assert!(profiles[1].enabled);
        assert_eq!(profiles[1].bin, "claude1");
    }

    #[test]
    fn load_prefers_new_key_over_legacy() {
        let mut custom_profile = CliAgentProfile::new(CliAgentKind::Custom);
        custom_profile.enabled = true;
        custom_profile.bin = "my-ai".into();
        let mut expected_profiles = CliAgentProfile::defaults();
        if let Some(p) = expected_profiles
            .iter_mut()
            .find(|p| p.kind == CliAgentKind::Custom)
        {
            *p = custom_profile;
        }
        let profiles_json = serde_json::to_string(&expected_profiles).unwrap();

        let rows = [
            ("ai_backend", "CLI"),
            ("ai_cli_kind", "CLAUDE"),
            ("ai_cli_bin", "/old/claude"),
            ("ai_cli_profiles", profiles_json.as_str()),
            ("ai_default_target", "CLI:CUSTOM"),
        ];

        let prefs = preferences_from_kv(rows);
        assert_eq!(prefs.ai_default_target, ChatTarget::Cli(CliAgentKind::Custom));
        let custom = prefs
            .ai_cli_profiles
            .iter()
            .find(|p| p.kind == CliAgentKind::Custom)
            .unwrap();
        assert!(custom.enabled);
        assert_eq!(custom.bin, "my-ai");
        let claude = prefs
            .ai_cli_profiles
            .iter()
            .find(|p| p.kind == CliAgentKind::ClaudeCode)
            .unwrap();
        assert!(!claude.enabled);
    }
}
