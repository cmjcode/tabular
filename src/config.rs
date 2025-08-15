use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite, Row};
use log::info;
use dirs::home_dir;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppPreferences {
    pub is_dark_mode: bool,
    pub link_editor_theme: bool,
    pub editor_theme: String,
    pub font_size: f32,
    pub word_wrap: bool,
}

impl AppPreferences {
    pub fn editor_theme_enum(&self) -> String { self.editor_theme.clone() }
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
            log::error!("Failed to create config directory {}: {}", path.display(), e);
            // Use JSON fallback if directory creation fails
            return Ok(Self { pool: None, use_json_fallback: true });
        }
        
        path.push("preferences.db");
        
        // Try to create the file first if it doesn't exist
        if !path.exists() {
            if let Err(e) = std::fs::File::create(&path) {
                log::error!("Failed to create database file {}: {}, using JSON fallback", path.display(), e);
                return Ok(Self { pool: None, use_json_fallback: true });
            }
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
                word_wrap: true 
            };
            
            if let Ok(rows) = sqlx::query("SELECT key, value FROM preferences").fetch_all(pool).await {
                for row in rows {
                    let k: String = row.get(0);
                    let v: String = row.get(1);
                    match k.as_str() {
                        "is_dark_mode" => prefs.is_dark_mode = v == "1",
                        "link_editor_theme" => prefs.link_editor_theme = v == "1",
                        "editor_theme" => prefs.editor_theme = v,
                        "font_size" => prefs.font_size = v.parse().unwrap_or(14.0),
                        "word_wrap" => prefs.word_wrap = v == "1",
                        _ => {}
                    }
                }
            }
            
            info!("Loaded prefs from SQLite: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}", 
                  prefs.is_dark_mode, prefs.link_editor_theme, prefs.editor_theme, prefs.font_size, prefs.word_wrap);
            return prefs;
        }
        
        AppPreferences::default()
    }

    pub async fn save(&self, prefs: &AppPreferences) {
        if self.use_json_fallback {
            let _ = self.save_to_json(prefs);
            info!("Saved prefs to JSON: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}", 
                  prefs.is_dark_mode, prefs.link_editor_theme, prefs.editor_theme, prefs.font_size, prefs.word_wrap);
            return;
        }
        
        if let Some(ref pool) = self.pool {
            let font_size_string = prefs.font_size.to_string();
            let entries: [(&str,&str);5] = [
                ("is_dark_mode", if prefs.is_dark_mode {"1"} else {"0"}),
                ("link_editor_theme", if prefs.link_editor_theme {"1"} else {"0"}),
                ("editor_theme", prefs.editor_theme.as_str()),
                ("font_size", &font_size_string),
                ("word_wrap", if prefs.word_wrap {"1"} else {"0"}),
            ];
            
            for (k,v) in entries.iter() {
                let _ = sqlx::query("REPLACE INTO preferences (key,value) VALUES (?,?)")
                    .bind(k).bind(v).execute(pool).await;
            }
            
            info!("Saved prefs to SQLite: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}", 
                  prefs.is_dark_mode, prefs.link_editor_theme, prefs.editor_theme, prefs.font_size, prefs.word_wrap);
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
        info!("Loaded prefs from JSON: is_dark_mode={}, link_editor_theme={}, editor_theme={}, font_size={}, word_wrap={}", 
              prefs.is_dark_mode, prefs.link_editor_theme, prefs.editor_theme, prefs.font_size, prefs.word_wrap);
        Ok(prefs)
    }

    fn save_to_json(&self, prefs: &AppPreferences) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::json_path();
        let content = serde_json::to_string_pretty(prefs)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

fn config_dir() -> PathBuf {
    if let Some(mut hd) = home_dir() {
        hd.push(".tabular");
        return hd
    }
    PathBuf::from(".")
}
