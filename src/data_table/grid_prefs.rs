//! Preferensi grid per tabel di `connections.db`: saved filter (B6) dan
//! highlight rule (B7).
//!
//! Tabel `grid_table_prefs` dibuat lazily sehingga tidak perlu migrasi
//! terpusat. Fungsi di sini headless (hanya butuh `&SqlitePool`) agar bisa
//! diuji dengan pool in-memory.

use crate::models::structs::{FilterCondition, FilterGroup};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const KIND_FILTER: &str = "filter";
pub const KIND_HIGHLIGHT: &str = "highlight";
/// Nama tetap untuk satu set highlight rule per tabel.
pub const HIGHLIGHT_RULES_NAME: &str = "rules";

/// Identitas tabel yang dipakai sebagai kunci preferensi.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct TableKey {
    pub connection_id: i64,
    pub database: String,
    pub table: String,
}

/// Isi saved filter: kondisi visual filter plus teks WHERE bebas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SavedFilterPayload {
    #[serde(default)]
    pub conditions: Vec<FilterCondition>,
    #[serde(default = "default_true")]
    pub match_all: bool,
    #[serde(default)]
    pub group: FilterGroup,
    #[serde(default)]
    pub where_text: String,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedFilter {
    pub name: String,
    pub payload: SavedFilterPayload,
    pub is_default: bool,
}

pub async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS grid_table_prefs (
            connection_id INTEGER NOT NULL,
            database_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            is_default INTEGER NOT NULL DEFAULT 0,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (connection_id, database_name, table_name, kind, name)
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Semua entri sebuah jenis untuk satu tabel: (nama, payload JSON, default).
pub async fn list_entries(
    pool: &SqlitePool,
    key: &TableKey,
    kind: &str,
) -> Result<Vec<(String, String, bool)>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT name, payload_json, is_default FROM grid_table_prefs
         WHERE connection_id = ? AND database_name = ? AND table_name = ? AND kind = ?
         ORDER BY name COLLATE NOCASE",
    )
    .bind(key.connection_id)
    .bind(&key.database)
    .bind(&key.table)
    .bind(kind)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, payload, is_default)| (name, payload, is_default != 0))
        .collect())
}

/// Simpan (insert/replace) satu entri. Bila `is_default`, entri lain dengan
/// jenis yang sama untuk tabel itu kehilangan status default.
pub async fn upsert_entry(
    pool: &SqlitePool,
    key: &TableKey,
    kind: &str,
    name: &str,
    payload_json: &str,
    is_default: bool,
) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    let mut tx = pool.begin().await?;
    if is_default {
        sqlx::query(
            "UPDATE grid_table_prefs SET is_default = 0
             WHERE connection_id = ? AND database_name = ? AND table_name = ? AND kind = ?",
        )
        .bind(key.connection_id)
        .bind(&key.database)
        .bind(&key.table)
        .bind(kind)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO grid_table_prefs
            (connection_id, database_name, table_name, kind, name, payload_json, is_default, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(connection_id, database_name, table_name, kind, name)
         DO UPDATE SET payload_json = excluded.payload_json,
                       is_default = excluded.is_default,
                       updated_at = CURRENT_TIMESTAMP",
    )
    .bind(key.connection_id)
    .bind(&key.database)
    .bind(&key.table)
    .bind(kind)
    .bind(name)
    .bind(payload_json)
    .bind(is_default as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn remove_entry(
    pool: &SqlitePool,
    key: &TableKey,
    kind: &str,
    name: &str,
) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query(
        "DELETE FROM grid_table_prefs
         WHERE connection_id = ? AND database_name = ? AND table_name = ? AND kind = ? AND name = ?",
    )
    .bind(key.connection_id)
    .bind(&key.database)
    .bind(&key.table)
    .bind(kind)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}

/// Daftar saved filter terurai; entri dengan JSON rusak dilewati.
pub async fn load_saved_filters(
    pool: &SqlitePool,
    key: &TableKey,
) -> Result<Vec<SavedFilter>, sqlx::Error> {
    let entries = list_entries(pool, key, KIND_FILTER).await?;
    Ok(entries
        .into_iter()
        .filter_map(
            |(name, json, is_default)| match serde_json::from_str(&json) {
                Ok(payload) => Some(SavedFilter {
                    name,
                    payload,
                    is_default,
                }),
                Err(e) => {
                    log::warn!("[GRID] saved filter '{}' tidak valid: {}", name, e);
                    None
                }
            },
        )
        .collect())
}

pub async fn load_highlight_rules(
    pool: &SqlitePool,
    key: &TableKey,
) -> Result<Vec<super::grid_model::HighlightRule>, sqlx::Error> {
    let entries = list_entries(pool, key, KIND_HIGHLIGHT).await?;
    Ok(entries
        .into_iter()
        .find(|(name, _, _)| name == HIGHLIGHT_RULES_NAME)
        .and_then(|(_, json, _)| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

pub async fn save_highlight_rules(
    pool: &SqlitePool,
    key: &TableKey,
    rules: &[super::grid_model::HighlightRule],
) -> Result<(), sqlx::Error> {
    if rules.is_empty() {
        return remove_entry(pool, key, KIND_HIGHLIGHT, HIGHLIGHT_RULES_NAME).await;
    }
    let json = serde_json::to_string(rules).unwrap_or_else(|_| "[]".to_string());
    upsert_entry(
        pool,
        key,
        KIND_HIGHLIGHT,
        HIGHLIGHT_RULES_NAME,
        &json,
        false,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_table::grid_model::{HighlightColor, HighlightRule};
    use crate::models::structs::FilterOperator;

    async fn pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    fn key() -> TableKey {
        TableKey {
            connection_id: 1,
            database: "shop".into(),
            table: "orders".into(),
        }
    }

    #[tokio::test]
    async fn saved_filter_simpan_default_dan_hapus() {
        let pool = pool().await;
        let payload = SavedFilterPayload {
            conditions: vec![FilterCondition::new(
                "status",
                FilterOperator::Equal,
                "paid",
            )],
            match_all: true,
            group: FilterGroup::And,
            where_text: "\"status\" = 'paid'".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        upsert_entry(&pool, &key(), KIND_FILTER, "Paid", &json, true)
            .await
            .unwrap();
        upsert_entry(&pool, &key(), KIND_FILTER, "All", "{}", true)
            .await
            .unwrap();

        let filters = load_saved_filters(&pool, &key()).await.unwrap();
        assert_eq!(filters.len(), 2);
        // Default hanya boleh satu: yang terakhir disimpan.
        assert!(filters.iter().find(|f| f.name == "All").unwrap().is_default);
        let paid = filters.iter().find(|f| f.name == "Paid").unwrap();
        assert!(!paid.is_default);
        assert_eq!(paid.payload, payload);

        // Tabel lain tidak ikut terlihat.
        let other = TableKey {
            table: "users".into(),
            ..key()
        };
        assert!(load_saved_filters(&pool, &other).await.unwrap().is_empty());

        remove_entry(&pool, &key(), KIND_FILTER, "Paid")
            .await
            .unwrap();
        assert_eq!(load_saved_filters(&pool, &key()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn highlight_rules_round_trip() {
        let pool = pool().await;
        assert!(
            load_highlight_rules(&pool, &key())
                .await
                .unwrap()
                .is_empty()
        );
        let mut rule = HighlightRule::new("status");
        rule.color = HighlightColor::Red;
        save_highlight_rules(&pool, &key(), std::slice::from_ref(&rule))
            .await
            .unwrap();
        assert_eq!(
            load_highlight_rules(&pool, &key()).await.unwrap(),
            vec![rule]
        );
        save_highlight_rules(&pool, &key(), &[]).await.unwrap();
        assert!(
            load_highlight_rules(&pool, &key())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
