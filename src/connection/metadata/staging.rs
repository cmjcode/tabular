use log::debug;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Default)]
pub(crate) struct ColumnMetaStaging {
    pub column_name: String,
    pub data_type: String,
    pub ordinal_position: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct IndexMetaStaging {
    pub index_name: String,
    pub method: Option<String>,
    pub is_unique: bool,
    pub columns_json: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TableMetaStaging {
    pub table_name: String,
    pub table_type: String, // "table" or "view"
    pub columns: Vec<ColumnMetaStaging>,
    pub indexes: Vec<IndexMetaStaging>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DatabaseMetaStaging {
    pub database_name: String,
    pub tables: Vec<TableMetaStaging>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MetadataStaging {
    pub connection_id: i64,
    pub databases: Vec<DatabaseMetaStaging>,
}

impl MetadataStaging {
    pub(crate) fn new(connection_id: i64) -> Self {
        Self {
            connection_id,
            databases: Vec::new(),
        }
    }

    pub(crate) fn add_database(&mut self, db_name: impl Into<String>) -> &mut DatabaseMetaStaging {
        let name = db_name.into();
        if let Some(idx) = self.databases.iter().position(|d| d.database_name == name) {
            &mut self.databases[idx]
        } else {
            self.databases.push(DatabaseMetaStaging {
                database_name: name,
                tables: Vec::new(),
            });
            self.databases.last_mut().unwrap()
        }
    }

    pub(crate) async fn commit_to_sqlite(&self, cache_pool: &SqlitePool) -> Result<(), sqlx::Error> {
        let total_tables: usize = self.databases.iter().map(|d| d.tables.len()).sum();
        eprintln!(
            "[METADATA-STAGING] conn={} starting atomic SQLite commit: {} databases, {} tables total",
            self.connection_id,
            self.databases.len(),
            total_tables
        );

        let mut tx = match cache_pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                eprintln!(
                    "[METADATA-STAGING] conn={} failed to begin SQLite transaction: {}",
                    self.connection_id, e
                );
                return Err(e);
            }
        };

        // 1. Delete existing cache rows for this connection
        sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
            .bind(self.connection_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
            .bind(self.connection_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
            .bind(self.connection_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM index_cache WHERE connection_id = ?")
            .bind(self.connection_id)
            .execute(&mut *tx)
            .await?;

        // 2. Insert staged data in batch
        for db in &self.databases {
            sqlx::query(
                "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
            )
            .bind(self.connection_id)
            .bind(&db.database_name)
            .execute(&mut *tx)
            .await?;

            for tbl in &db.tables {
                sqlx::query(
                    "INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)",
                )
                .bind(self.connection_id)
                .bind(&db.database_name)
                .bind(&tbl.table_name)
                .bind(&tbl.table_type)
                .execute(&mut *tx)
                .await?;

                for col in &tbl.columns {
                    sqlx::query(
                        "INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(self.connection_id)
                    .bind(&db.database_name)
                    .bind(&tbl.table_name)
                    .bind(&col.column_name)
                    .bind(&col.data_type)
                    .bind(col.ordinal_position)
                    .execute(&mut *tx)
                    .await?;
                }

                for idx in &tbl.indexes {
                    sqlx::query(
                        "INSERT OR REPLACE INTO index_cache (connection_id, database_name, table_name, index_name, method, is_unique, columns_json) VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(self.connection_id)
                    .bind(&db.database_name)
                    .bind(&tbl.table_name)
                    .bind(&idx.index_name)
                    .bind(&idx.method)
                    .bind(if idx.is_unique { 1 } else { 0 })
                    .bind(&idx.columns_json)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        if let Err(e) = tx.commit().await {
            eprintln!(
                "[METADATA-STAGING] conn={} tx.commit() failed: {}",
                self.connection_id, e
            );
            return Err(e);
        }
        eprintln!(
            "[METADATA-STAGING] conn={} SUCCESS committed {} databases and {} tables to SQLite",
            self.connection_id,
            self.databases.len(),
            total_tables
        );
        Ok(())
    }
}
