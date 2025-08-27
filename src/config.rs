use dirs::home_dir;
use log::info;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite, sqlite::SqlitePoolOptions};
use std::fs;
use std::path::PathBuf;

/// File name to store the current data directory location
const CONFIG_LOCATION_FILE: &str = "config_location.txt";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppPreferences {
    pub is_dark_mode: bool,
    pub link_editor_theme: bool,
    pub editor_theme: String,
    pub font_size: f32,
    pub word_wrap: bool,
    pub data_directory: Option<String>,
    pub auto_check_updates: bool,
    pub use_server_pagination: bool,
    // RFC3339 timestamp of the last time we checked GitHub releases (persisted)
    pub last_update_check_iso: Option<String>,
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

        log::info!("Attempting to create/open database at: {}", url);

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
                        log::info!("Config store initialized successfully with SQLite");
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
                is_dark_mode: true,
                link_editor_theme: true,
                editor_theme: "GITHUB_DARK".into(),
                font_size: 14.0,
                word_wrap: true,
                data_directory: None,
                auto_check_updates: true,
                use_server_pagination: true, // Default to true for better performance
                last_update_check_iso: None,
            };

            if let Ok(rows) = sqlx::query("SELECT key, value FROM preferences")
                .fetch_all(pool)
                .await
            {
                for row in rows {
                    let k: String = row.get(0);
                    let v: String = row.get(1);
                    match k.as_str() {
                        "is_dark_mode" => prefs.is_dark_mode = v == "1",
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
                        _ => {}
                    }
                }
            }

            info!(
                "Loaded prefs from SQLite: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}, use_server_pagination={}",
                prefs.is_dark_mode,
                prefs.link_editor_theme,
                prefs.editor_theme,
                prefs.font_size,
                prefs.word_wrap,
                prefs.data_directory,
                prefs.auto_check_updates,
                prefs.use_server_pagination
            );
            return prefs;
        }

        AppPreferences::default()
    }

    pub async fn save(&self, prefs: &AppPreferences) {
        if self.use_json_fallback {
            let _ = self.save_to_json(prefs);
            info!(
                "Saved prefs to JSON: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}, use_server_pagination={}",
                prefs.is_dark_mode,
                prefs.link_editor_theme,
                prefs.editor_theme,
                prefs.font_size,
                prefs.word_wrap,
                prefs.data_directory,
                prefs.auto_check_updates,
                prefs.use_server_pagination
            );
            return;
        }

        if let Some(ref pool) = self.pool {
            let font_size_string = prefs.font_size.to_string();
            let entries: [(&str, &str); 8] = [
                ("is_dark_mode", if prefs.is_dark_mode { "1" } else { "0" }),
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

            info!(
                "Saved prefs to SQLite: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}",
                prefs.is_dark_mode,
                prefs.link_editor_theme,
                prefs.editor_theme,
                prefs.font_size,
                prefs.word_wrap,
                prefs.data_directory,
                prefs.auto_check_updates
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
        info!(
            "Loaded prefs from JSON: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}, data_directory={:?}, auto_check_updates={}",
            prefs.is_dark_mode,
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
            && let Ok(row) = sqlx::query_as::<_, (String,)>(
                "SELECT value FROM preferences WHERE key = ?",
            )
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

    log::info!(
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
                    log::info!(
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
        log::info!("Using saved config location: {}", saved_location);
        unsafe {
            std::env::set_var("TABULAR_DATA_DIR", &saved_location);
        }
        return;
    }

    // If no saved location, check environment variable
    if let Ok(env_dir) = std::env::var("TABULAR_DATA_DIR") {
        log::info!("Using environment variable TABULAR_DATA_DIR: {}", env_dir);
        return;
    }

    // Otherwise use default ~/.tabular
    let default_dir = get_default_tabular_dir();
    log::info!("Using default data directory: {}", default_dir.display());
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

    log::info!("Data directory changed to: {}", new_path);
    Ok(())
}
