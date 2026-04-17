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
            AiProvider::GitHub => "GitHub PAT (Settings → Developer settings → Personal access tokens)",
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPreferences {
    #[serde(default)]
    pub theme: AppTheme,
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
    #[serde(default = "default_redis_browser_auto_refresh_seconds")]
    pub redis_browser_auto_refresh_seconds: u32,
}

fn default_redis_browser_auto_refresh_seconds() -> u32 {
    5
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            theme: AppTheme::Dark,
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
            redis_browser_auto_refresh_seconds: default_redis_browser_auto_refresh_seconds(),
        }
    }
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

        match SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
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
                redis_browser_auto_refresh_seconds: default_redis_browser_auto_refresh_seconds(),
            };

            if let Ok(rows) = sqlx::query("SELECT key, value FROM preferences")
                .fetch_all(pool)
                .await
            {
                for row in rows {
                    let k: String = row.get(0);
                    let v: String = row.get(1);
                    match k.as_str() {
                        "theme" => prefs.theme = v.parse().unwrap_or(AppTheme::Dark),
                        // Legacy migration: old boolean flags
                        "is_dark_mode" => if v != "1" { prefs.theme = AppTheme::Light; },
                        "is_light_soft" => if v == "1" { prefs.theme = AppTheme::LightSoft; },
                        "link_editor_theme" => prefs.link_editor_theme = v == "1",
                        "editor_theme" => prefs.editor_theme = v,
                        "font_size" => prefs.font_size = v.parse().unwrap_or(14.0),
                        "word_wrap" => prefs.word_wrap = v == "1",
                        "data_directory" => {
                            prefs.data_directory = if v.is_empty() { None } else { Some(v) }
                        }
                        "auto_check_updates" => prefs.auto_check_updates = v == "1",
                        "use_server_pagination" => prefs.use_server_pagination = v == "1",
                        "last_update_check_iso" => {
                            prefs.last_update_check_iso = if v.is_empty() { None } else { Some(v) }
                        }
                        "enable_debug_logging" => prefs.enable_debug_logging = v == "1",
                        "ai_api_key" => prefs.ai_api_key = v,
                        "ai_model" => prefs.ai_model = v,
                        "ai_provider" => prefs.ai_provider = v.parse().unwrap_or(AiProvider::OpenAI),
                        "ai_base_url" => prefs.ai_base_url = v,
                        "redis_browser_auto_refresh_seconds" => {
                            prefs.redis_browser_auto_refresh_seconds = v.parse().unwrap_or(default_redis_browser_auto_refresh_seconds())
                        }
                        _ => {}
                    }
                }
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
            let redis_browser_auto_refresh_seconds = prefs.redis_browser_auto_refresh_seconds.to_string();
            let entries: [(&str, &str); 14] = [
                ("theme", prefs.theme.as_str()),
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
                ("ai_api_key", prefs.ai_api_key.as_str()),
                ("ai_model", prefs.ai_model.as_str()),
                ("ai_provider", prefs.ai_provider.as_str()),
                ("ai_base_url", prefs.ai_base_url.as_str()),
                ("redis_browser_auto_refresh_seconds", &redis_browser_auto_refresh_seconds),
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

    fn json_path() -> PathBuf {
        let mut path = config_dir();
        path.push("preferences.json");
        path
    }

    fn load_from_json(&self) -> Result<AppPreferences, Box<dyn std::error::Error>> {
        let path = Self::json_path();
        let content = std::fs::read_to_string(path)?;
        let prefs: AppPreferences = serde_json::from_str(&content)?;
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
        let content = serde_json::to_string_pretty(prefs)?;
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

/// Get the default tabular directory in home folder
fn get_default_tabular_dir() -> PathBuf {
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
                let path = content.trim();
                if !path.is_empty() && PathBuf::from(path).exists() {
                    log::debug!(
                        "Loaded config location from {}: {}",
                        config_file.display(),
                        path
                    );
                    return Some(path.to_string());
                } else {
                    log::warn!("Config location file contains invalid path: {}", path);
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

    // Default to ~/.tabular
    if let Some(mut hd) = home_dir() {
        hd.push(".tabular");
        return hd;
    }
    PathBuf::from(".")
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
