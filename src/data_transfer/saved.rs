//! Saved comparisons (H12): pasangan endpoint + opsi Data Compare, disimpan di
//! `connections.db`. Tabel dibuat lazily; fungsi di sini hanya butuh
//! `&SqlitePool` sehingga bisa diuji dengan pool in-memory.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::compare::CompareOptions;
use super::compare_structure::StructureOptions;

/// Mode perbandingan: isi baris data atau definisi struktur kolom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompareMode {
    #[default]
    Data,
    Structure,
}

impl CompareMode {
    pub fn label(self) -> &'static str {
        match self {
            CompareMode::Data => "Data",
            CompareMode::Structure => "Structure",
        }
    }
}

/// Satu sisi perbandingan yang disimpan. Hanya id koneksi yang disimpan,
/// bukan konfigurasi koneksinya.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedEndpoint {
    pub connection_id: i64,
    pub database: Option<String>,
    pub table: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedComparison {
    pub name: String,
    pub source: SavedEndpoint,
    pub target: SavedEndpoint,
    pub mode: CompareMode,
    pub options: CompareOptions,
    pub structure_options: StructureOptions,
    /// Waktu simpan terakhir (UTC, dari SQLite).
    pub updated_at: String,
}

pub async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS data_compare_saved (
            name TEXT PRIMARY KEY COLLATE NOCASE,
            source_connection_id INTEGER NOT NULL,
            source_database TEXT,
            source_table TEXT NOT NULL,
            target_connection_id INTEGER NOT NULL,
            target_database TEXT,
            target_table TEXT NOT NULL,
            options_json TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

type SavedRow = (
    String,
    i64,
    Option<String>,
    String,
    i64,
    Option<String>,
    String,
    String,
    Option<String>,
);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedOptionsPayload {
    #[serde(default)]
    mode: CompareMode,
    #[serde(flatten)]
    data: CompareOptions,
    #[serde(default)]
    structure: StructureOptions,
}

/// Semua perbandingan tersimpan, urut nama.
pub async fn list(pool: &SqlitePool) -> Result<Vec<SavedComparison>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows = sqlx::query_as::<_, SavedRow>(
        "SELECT name, source_connection_id, source_database, source_table,
                target_connection_id, target_database, target_table, options_json,
                CAST(updated_at AS TEXT)
         FROM data_compare_saved ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, sc, sd, st, tc, td, tt, options, updated)| {
            let (mode, opts, struct_opts) =
                match serde_json::from_str::<SavedOptionsPayload>(&options) {
                    Ok(p) => (p.mode, p.data, p.structure),
                    Err(_) => (
                        CompareMode::Data,
                        serde_json::from_str(&options).unwrap_or_default(),
                        StructureOptions::default(),
                    ),
                };
            SavedComparison {
                name,
                source: SavedEndpoint {
                    connection_id: sc,
                    database: sd,
                    table: st,
                },
                target: SavedEndpoint {
                    connection_id: tc,
                    database: td,
                    table: tt,
                },
                mode,
                options: opts,
                structure_options: struct_opts,
                updated_at: updated.unwrap_or_default(),
            }
        })
        .collect())
}

/// Simpan atau timpa perbandingan dengan nama yang sama.
pub async fn save(pool: &SqlitePool, item: &SavedComparison) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    let payload = SavedOptionsPayload {
        mode: item.mode,
        data: item.options.clone(),
        structure: item.structure_options.clone(),
    };
    let options = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    sqlx::query(
        "INSERT INTO data_compare_saved
            (name, source_connection_id, source_database, source_table,
             target_connection_id, target_database, target_table, options_json, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(name) DO UPDATE SET
            source_connection_id = excluded.source_connection_id,
            source_database = excluded.source_database,
            source_table = excluded.source_table,
            target_connection_id = excluded.target_connection_id,
            target_database = excluded.target_database,
            target_table = excluded.target_table,
            options_json = excluded.options_json,
            updated_at = CURRENT_TIMESTAMP",
    )
    .bind(item.name.trim())
    .bind(item.source.connection_id)
    .bind(&item.source.database)
    .bind(&item.source.table)
    .bind(item.target.connection_id)
    .bind(&item.target.database)
    .bind(&item.target.table)
    .bind(options)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, name: &str) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query("DELETE FROM data_compare_saved WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    fn item(name: &str, table: &str) -> SavedComparison {
        SavedComparison {
            name: name.to_string(),
            source: SavedEndpoint {
                connection_id: 1,
                database: Some("shop".to_string()),
                table: table.to_string(),
            },
            target: SavedEndpoint {
                connection_id: 2,
                database: None,
                table: table.to_string(),
            },
            mode: CompareMode::Data,
            options: CompareOptions {
                key_columns: vec!["id".to_string()],
                where_clause: Some("id > 10".to_string()),
                row_limit: Some(500),
                ..Default::default()
            },
            structure_options: StructureOptions::default(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn save_list_overwrite_delete() {
        let pool = pool().await;
        assert!(list(&pool).await.unwrap().is_empty());
        save(&pool, &item("prod vs staging", "orders"))
            .await
            .unwrap();
        save(&pool, &item("alpha", "users")).await.unwrap();
        // Nama sama (beda huruf besar) menimpa, bukan menambah.
        save(&pool, &item("PROD vs staging", "invoices"))
            .await
            .unwrap();

        let all = list(&pool).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[0].mode, CompareMode::Data);
        assert_eq!(all[1].source.table, "invoices");
        assert_eq!(all[1].source.database.as_deref(), Some("shop"));
        assert_eq!(all[1].target.database, None);
        assert_eq!(all[1].options.key_columns, vec!["id"]);
        assert_eq!(all[1].options.row_limit, Some(500));
        assert!(!all[1].updated_at.is_empty());

        delete(&pool, "alpha").await.unwrap();
        assert_eq!(list(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unreadable_options_fall_back_to_defaults() {
        let pool = pool().await;
        ensure_table(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO data_compare_saved (name, source_connection_id, source_table,
             target_connection_id, target_table, options_json) VALUES ('x', 1, 'a', 2, 'b', 'not json')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let all = list(&pool).await.unwrap();
        assert_eq!(all[0].options, CompareOptions::default());
        assert_eq!(all[0].mode, CompareMode::Data);
        assert_eq!(all[0].structure_options, StructureOptions::default());
    }

    #[tokio::test]
    async fn legacy_options_json_reads_as_data_mode() {
        let pool = pool().await;
        ensure_table(&pool).await.unwrap();
        let legacy_json = serde_json::to_string(&CompareOptions {
            key_columns: vec!["uuid".to_string()],
            where_clause: Some("active = 1".to_string()),
            row_limit: Some(100),
            ..Default::default()
        })
        .unwrap();
        sqlx::query(
            "INSERT INTO data_compare_saved (name, source_connection_id, source_table,
             target_connection_id, target_table, options_json) VALUES ('legacy', 1, 'a', 2, 'b', ?)",
        )
        .bind(&legacy_json)
        .execute(&pool)
        .await
        .unwrap();

        let all = list(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "legacy");
        assert_eq!(all[0].mode, CompareMode::Data);
        assert_eq!(all[0].options.key_columns, vec!["uuid"]);
        assert_eq!(all[0].options.where_clause.as_deref(), Some("active = 1"));
        assert_eq!(all[0].options.row_limit, Some(100));
        assert_eq!(all[0].structure_options, StructureOptions::default());
    }

    #[tokio::test]
    async fn structure_mode_saved_roundtrips() {
        let pool = pool().await;
        let mut it = item("schema diff", "users");
        it.mode = CompareMode::Structure;
        it.structure_options = StructureOptions {
            ignore_columns: vec!["temp".to_string()],
            case_insensitive_names: true,
            compare_nullability: false,
        };
        save(&pool, &it).await.unwrap();

        let all = list(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "schema diff");
        assert_eq!(all[0].mode, CompareMode::Structure);
        assert_eq!(all[0].structure_options.ignore_columns, vec!["temp"]);
        assert!(all[0].structure_options.case_insensitive_names);
        assert!(!all[0].structure_options.compare_nullability);
    }
}
