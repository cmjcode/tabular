//! Environment koneksi + warna (M10).
//!
//! Setiap koneksi bisa ditandai Production / Staging / Development / Testing /
//! Local. Warnanya dipakai sebagai strip di tab query dan toolbar editor, supaya
//! jelas sedang bekerja di server mana. Tanpa tanda eksplisit, environment
//! ditebak dari nama koneksi (mis. "Orders PROD" → Production).
//!
//! Disimpan di tabel terpisah `connection_environment` di `connections.db`
//! (bukan kolom `connections`) agar tidak menyentuh skema yang dipakai sync
//! dan import/export; konsekuensinya tanda ini lokal per mesin.

use std::collections::HashMap;

use eframe::egui::Color32;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Environment {
    Production,
    Staging,
    Development,
    Testing,
    Local,
}

impl Environment {
    pub const ALL: [Environment; 5] = [
        Environment::Production,
        Environment::Staging,
        Environment::Development,
        Environment::Testing,
        Environment::Local,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Environment::Production => "production",
            Environment::Staging => "staging",
            Environment::Development => "development",
            Environment::Testing => "testing",
            Environment::Local => "local",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|e| e.key().eq_ignore_ascii_case(key.trim()))
    }

    /// Label terlokalisasi.
    pub fn label(self) -> &'static str {
        use crate::i18n::tr;
        match self {
            Environment::Production => tr("Production"),
            Environment::Staging => tr("Staging"),
            Environment::Development => tr("Development"),
            Environment::Testing => tr("Testing"),
            Environment::Local => tr("Local"),
        }
    }

    /// Singkatan untuk badge sempit.
    pub fn short(self) -> &'static str {
        match self {
            Environment::Production => "PROD",
            Environment::Staging => "STG",
            Environment::Development => "DEV",
            Environment::Testing => "TEST",
            Environment::Local => "LOCAL",
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Environment::Production => Color32::from_rgb(0xE5, 0x48, 0x4D),
            Environment::Staging => Color32::from_rgb(0xF5, 0x9E, 0x0B),
            Environment::Development => Color32::from_rgb(0x3B, 0x82, 0xF6),
            Environment::Testing => Color32::from_rgb(0x8B, 0x5C, 0xF6),
            Environment::Local => Color32::from_rgb(0x22, 0xA0, 0x6B),
        }
    }
}

/// Tebak environment dari nama koneksi berdasarkan token utuh, supaya
/// "device" tidak terbaca sebagai "dev".
pub fn detect_from_name(name: &str) -> Option<Environment> {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let has = |words: &[&str]| tokens.iter().any(|t| words.contains(t));
    // Urutan penting: "preprod" harus menjadi Staging, bukan Production.
    if has(&["staging", "stage", "stg", "uat", "preprod"]) {
        Some(Environment::Staging)
    } else if has(&["prod", "production", "prd", "live"]) {
        Some(Environment::Production)
    } else if has(&["test", "testing", "qa"]) {
        Some(Environment::Testing)
    } else if has(&["dev", "develop", "development"]) {
        Some(Environment::Development)
    } else if has(&["local", "localhost"]) {
        Some(Environment::Local)
    } else {
        None
    }
}

/// Environment efektif: tanda eksplisit, atau tebakan dari nama.
pub fn effective(explicit: Option<Environment>, connection_name: &str) -> Option<Environment> {
    explicit.or_else(|| detect_from_name(connection_name))
}

async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS connection_environment (\
            connection_id INTEGER PRIMARY KEY, \
            environment TEXT NOT NULL)",
    )
    .execute(pool)
    .await
    .map(|_| ())
}

/// Semua tanda eksplisit, per id koneksi.
pub async fn load_all(pool: &SqlitePool) -> Result<HashMap<i64, Environment>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT connection_id, environment FROM connection_environment")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, key)| Environment::parse(&key).map(|e| (id, e)))
        .collect())
}

/// Simpan atau hapus (`None`) tanda eksplisit.
pub async fn set(
    pool: &SqlitePool,
    connection_id: i64,
    env: Option<Environment>,
) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    match env {
        Some(e) => {
            sqlx::query(
                "INSERT INTO connection_environment (connection_id, environment) VALUES (?, ?) \
                 ON CONFLICT(connection_id) DO UPDATE SET environment = excluded.environment",
            )
            .bind(connection_id)
            .bind(e.key())
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM connection_environment WHERE connection_id = ?")
                .bind(connection_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_environment_from_name_tokens() {
        assert_eq!(detect_from_name("Orders PROD"), Some(Environment::Production));
        assert_eq!(detect_from_name("orders-preprod"), Some(Environment::Staging));
        assert_eq!(detect_from_name("api_dev"), Some(Environment::Development));
        assert_eq!(detect_from_name("device registry"), None);
        assert_eq!(detect_from_name("QA replica"), Some(Environment::Testing));
        assert_eq!(detect_from_name("Local MySQL"), Some(Environment::Local));
        assert_eq!(
            effective(Some(Environment::Local), "orders prod"),
            Some(Environment::Local)
        );
    }

    #[test]
    fn keys_roundtrip() {
        for e in Environment::ALL {
            assert_eq!(Environment::parse(e.key()), Some(e));
        }
        assert_eq!(Environment::parse("PRODUCTION"), Some(Environment::Production));
        assert_eq!(Environment::parse("nope"), None);
    }

    #[tokio::test]
    async fn persists_and_clears_in_sqlite() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        set(&pool, 3, Some(Environment::Staging)).await.unwrap();
        set(&pool, 3, Some(Environment::Production)).await.unwrap();
        set(&pool, 4, Some(Environment::Local)).await.unwrap();
        let all = load_all(&pool).await.unwrap();
        assert_eq!(all.get(&3), Some(&Environment::Production));
        assert_eq!(all.len(), 2);
        set(&pool, 4, None).await.unwrap();
        assert_eq!(load_all(&pool).await.unwrap().len(), 1);
    }
}
