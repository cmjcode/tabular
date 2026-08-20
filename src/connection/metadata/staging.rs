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
        match self.commit_to_sqlite_inner(cache_pool).await {
            Ok(()) => Ok(()),
            Err(e) if is_corrupt_error(&e) => {
                eprintln!(
                    "[METADATA-STAGING] conn={} detected SQLite malformed/corruption error: {}. Attempting self-healing checkpoint & reindex...",
                    self.connection_id, e
                );
                let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(cache_pool).await;
                let _ = sqlx::query("REINDEX").execute(cache_pool).await;
                // Retry once after healing
                self.commit_to_sqlite_inner(cache_pool).await
            }
            Err(e) => Err(e),
        }
    }

    async fn commit_to_sqlite_inner(&self, cache_pool: &SqlitePool) -> Result<(), sqlx::Error> {
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
                "INSERT INTO database_cache (connection_id, database_name) VALUES (?, ?) \
                 ON CONFLICT(connection_id, database_name) DO NOTHING",
            )
            .bind(self.connection_id)
            .bind(&db.database_name)
            .execute(&mut *tx)
            .await?;

            let mut seen_tables = std::collections::HashSet::new();
            for tbl in &db.tables {
                let table_key = (tbl.table_name.clone(), tbl.table_type.clone());
                if !seen_tables.insert(table_key) {
                    continue;
                }

                sqlx::query(
                    "INSERT INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?) \
                     ON CONFLICT(connection_id, database_name, table_name, table_type) DO NOTHING",
                )
                .bind(self.connection_id)
                .bind(&db.database_name)
                .bind(&tbl.table_name)
                .bind(&tbl.table_type)
                .execute(&mut *tx)
                .await?;

                let mut seen_cols = std::collections::HashSet::new();
                for col in &tbl.columns {
                    if !seen_cols.insert(col.column_name.clone()) {
                        continue;
                    }

                    sqlx::query(
                        "INSERT INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?) \
                         ON CONFLICT(connection_id, database_name, table_name, column_name) DO UPDATE SET data_type = excluded.data_type, ordinal_position = excluded.ordinal_position",
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

                let mut seen_indexes = std::collections::HashSet::new();
                for idx in &tbl.indexes {
                    if !seen_indexes.insert(idx.index_name.clone()) {
                        continue;
                    }

                    sqlx::query(
                        "INSERT INTO index_cache (connection_id, database_name, table_name, index_name, method, is_unique, columns_json) VALUES (?, ?, ?, ?, ?, ?, ?) \
                         ON CONFLICT(connection_id, database_name, table_name, index_name) DO UPDATE SET method = excluded.method, is_unique = excluded.is_unique, columns_json = excluded.columns_json",
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

fn is_corrupt_error(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        if db_err.code().is_some_and(|c| c.as_ref() == "11") {
            return true;
        }
        let msg = db_err.message().to_lowercase();
        return msg.contains("malformed")
            || msg.contains("disk image is malformed")
            || msg.contains("corrupt");
    }
    false
}
