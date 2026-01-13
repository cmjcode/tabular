use log::{debug, info};

use crate::{
    cache_data, connection, driver_mysql, driver_redis, driver_sqlite, models,
    window_egui::{self, Tabular},
};

pub(crate) fn get_tables_from_cache(
    tabular: &Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let fut = async {
            sqlx::query_as::<_, (String,)>("SELECT table_name FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type = ? ORDER BY table_name")
              .bind(connection_id)
              .bind(database_name)
              .bind(table_type)
              .fetch_all(pool_clone.as_ref())
              .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(rows) => Some(rows.into_iter().map(|(name,)| name).collect()),
            Err(_) => None,
        }
    } else {
        None
    }
}

pub(crate) fn get_databases_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
) -> Option<Vec<String>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let fut = async {
            sqlx::query_as::<_, (String,)>("SELECT database_name FROM database_cache WHERE connection_id = ? ORDER BY database_name")
              .bind(connection_id)
              .fetch_all(pool_clone.as_ref())
              .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(rows) => {
                let databases: Vec<String> = rows.into_iter().map(|(name,)| name).collect();
                Some(databases)
            }
            Err(e) => {
                debug!("Error reading from cache: {}", e);
                None
            }
        }
    } else {
        debug!("No database pool available for cache lookup");
        None
    }
}

pub(crate) fn build_redis_structure_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    node: &mut models::structs::TreeNode,
    databases: &[String],
) {
    let mut main_children = Vec::new();

    // Create databases folder for Redis
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(connection_id);
    databases_folder.is_expanded = false;
    databases_folder.is_loaded = true;

    // Add each Redis database from cache (db0, db1, etc.)
    for db_name in databases {
        if db_name.starts_with("db") {
            let mut db_node =
                models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
            db_node.connection_id = Some(connection_id);
            db_node.database_name = Some(db_name.clone());
            db_node.is_loaded = false; // Keys will be loaded when clicked

            // Check if this database has keys by looking for the marker
            let has_keys =
                driver_redis::check_redis_database_has_keys(tabular, connection_id, db_name);
            if has_keys {
                // Add a placeholder for keys that will be loaded on expansion
                let loading_node = models::structs::TreeNode::new(
                    "Loading keys...".to_string(),
                    models::enums::NodeType::Table,
                );
                db_node.children.push(loading_node);
            }

            databases_folder.children.push(db_node);
        }
    }

    main_children.push(databases_folder);
    node.children = main_children;
}

// Cache functions for database structure
pub(crate) fn save_databases_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    databases: &[String],
) {
    for db_name in databases {
        debug!("  - {}", db_name);
    }
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let databases_clone = databases.to_vec();
        let fut = async {
            // Clear existing cache for this connection
            let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(pool_clone.as_ref())
                .await;

            // Insert new database names
            for db_name in databases_clone {
                let _ = sqlx::query("INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)")
                     .bind(connection_id)
                     .bind(db_name)
                     .execute(pool_clone.as_ref())
                     .await;
            }
        };
        if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
    }
}

pub(crate) fn fetch_and_cache_connection_data(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
) {
    // Clone connection info to avoid borrowing issues
    let connection = if let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
    {
        conn.clone()
    } else {
        debug!("Connection not found for ID: {}", connection_id);
        return;
    };

    // Fetch databases from server
    let databases_result = connection::fetch_databases_from_connection(tabular, connection_id);

    if let Some(databases) = databases_result {
        // Save databases to cache
        save_databases_to_cache(tabular, connection_id, &databases);

        // For each database, fetch tables and columns
        for database_name in &databases {
            // Fetch different types of tables based on database type
            let table_types = match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    vec!["table", "view", "procedure", "function", "trigger", "event"]
                }
                models::enums::DatabaseType::PostgreSQL => vec!["table", "view"],
                models::enums::DatabaseType::SQLite => vec!["table", "view"],
                models::enums::DatabaseType::Redis => vec!["info_section", "redis_keys"],
                models::enums::DatabaseType::MsSQL => {
                    vec!["table", "view", "procedure", "function", "trigger"]
                }
                models::enums::DatabaseType::MongoDB => vec!["collection"],
            };

            let mut all_tables = Vec::new();

            for table_type in table_types {
                let tables_result = match connection.connection_type {
                    models::enums::DatabaseType::MySQL => {
                        driver_mysql::fetch_tables_from_mysql_connection(
                            tabular,
                            connection_id,
                            database_name,
                            table_type,
                        )
                    }
                    models::enums::DatabaseType::SQLite => {
                        driver_sqlite::fetch_tables_from_sqlite_connection(
                            tabular,
                            connection_id,
                            table_type,
                        )
                    }
                    models::enums::DatabaseType::PostgreSQL => {
                        crate::driver_postgres::fetch_tables_from_postgres_connection(
                            tabular,
                            connection_id,
                            database_name,
                            table_type,
                        )
                    }
                    models::enums::DatabaseType::Redis => {
                        driver_redis::fetch_tables_from_redis_connection(
                            tabular,
                            connection_id,
                            database_name,
                            table_type,
                        )
                    }
                    models::enums::DatabaseType::MsSQL => match table_type {
                        "table" | "view" => {
                            crate::driver_mssql::fetch_tables_from_mssql_connection(
                                tabular,
                                connection_id,
                                database_name,
                                table_type,
                            )
                        }
                        "procedure" | "function" | "trigger" => {
                            crate::driver_mssql::fetch_objects_from_mssql_connection(
                                tabular,
                                connection_id,
                                database_name,
                                table_type,
                            )
                        }
                        _ => None,
                    },
                    models::enums::DatabaseType::MongoDB => {
                        if table_type == "collection" {
                            crate::driver_mongodb::fetch_collections_from_mongodb_connection(
                                tabular,
                                connection_id,
                                database_name,
                            )
                        } else {
                            None
                        }
                    }
                };

                if let Some(tables) = tables_result {
                    for table_name in tables {
                        all_tables.push((table_name, table_type.to_string()));
                    }
                }
            }

            if !all_tables.is_empty() {
                // Save tables to cache
                cache_data::save_tables_to_cache(
                    tabular,
                    connection_id,
                    database_name,
                    &all_tables,
                );

                // For each table, fetch columns
                for (table_name, table_type) in &all_tables {
                    if table_type == "table" {
                        // Only fetch columns for actual tables, not views/procedures

                        let columns_result = connection::fetch_columns_from_database(
                            connection_id,
                            database_name,
                            table_name,
                            &connection,
                        );

                        if let Some(columns) = columns_result {
                            // Save columns to cache
                            cache_data::save_columns_to_cache(
                                tabular,
                                connection_id,
                                database_name,
                                table_name,
                                &columns,
                            );
                        }
                    }
                }
            }
        }
    } else {
        debug!(
            "Failed to fetch databases from server for connection_id: {}",
            connection_id
        );
    }
}

pub(crate) fn save_tables_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    tables: &[(String, String)],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let tables_clone = tables.to_vec();
        let database_name = database_name.to_string();
        let fut = async {
            // Clear existing cache for this database
            let _ = sqlx::query(
                "DELETE FROM table_cache WHERE connection_id = ? AND database_name = ?",
            )
            .bind(connection_id)
            .bind(&database_name)
            .execute(pool_clone.as_ref())
            .await;

            // Insert new table names with types
            for (table_name, table_type) in tables_clone {
                let _ = sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)")
                     .bind(connection_id)
                     .bind(&database_name)
                     .bind(table_name)
                     .bind(table_type)
                     .execute(pool_clone.as_ref())
                     .await;
            }
        };
        if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
    }
}

pub(crate) fn save_columns_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    columns: &[(String, String)],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let columns_clone = columns.to_vec();
        let database_name = database_name.to_string();
        let table_name = table_name.to_string();
        let fut = async {
            // Clear existing cache for this table
            let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
              .bind(connection_id)
              .bind(&database_name)
              .bind(&table_name)
              .execute(pool_clone.as_ref())
              .await;

            // Insert new column names with types
            for (i, (column_name, data_type)) in columns_clone.iter().enumerate() {
                let _ = sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)")
                     .bind(connection_id)
                     .bind(&database_name)
                     .bind(&table_name)
                     .bind(column_name)
                     .bind(data_type)
                     .bind(i as i64)
                     .execute(pool_clone.as_ref())
                     .await;
            }
        };
        if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
    }
}

pub(crate) fn get_columns_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<(String, String)>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let query_sql = "SELECT column_name, data_type FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? ORDER BY ordinal_position";
        debug!("📋 Executing cache query for columns: {}", query_sql);
        debug!(
            "   Parameters: connection_id={}, database={}, table={}",
            connection_id, database_name, table_name
        );

        let fut = async {
            sqlx::query_as::<_, (String, String)>(query_sql)
                .bind(connection_id)
                .bind(database_name)
                .bind(table_name)
                .fetch_all(pool_clone.as_ref())
                .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(ref rows) => {
                debug!(
                    "✅ Successfully retrieved {} columns from column_cache",
                    rows.len()
                );
            }
            Err(ref e) => {
                debug!("❌ Error retrieving columns from cache: {}", e);
            }
        }

        result.ok()
    } else {
        None
    }
}

pub(crate) fn get_primary_keys_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<String>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let query_sql = "SELECT columns_json FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? AND index_name = 'PRIMARY' ORDER BY index_name";
        debug!("🔐 Executing cache query for PRIMARY KEY: {}", query_sql);
        debug!(
            "   Parameters: connection_id={}, database={}, table={}",
            connection_id, database_name, table_name
        );

        let fut = async {
            sqlx::query_as::<_, (String,)>(query_sql)
                .bind(connection_id)
                .bind(database_name)
                .bind(table_name)
                .fetch_optional(pool_clone.as_ref())
                .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(Some((columns_json,))) => {
                // Parse JSON array dari columns_json
                let columns: Vec<String> = serde_json::from_str(&columns_json).unwrap_or_default();
                debug!(
                    "✅ Found PRIMARY KEY with {} columns: {:?}",
                    columns.len(),
                    columns
                );
                Some(columns)
            }
            Ok(None) => {
                debug!(
                    "⚠️ No PRIMARY KEY found in index_cache for {}.{}",
                    database_name, table_name
                );
                None
            }
            Err(e) => {
                debug!("❌ Error retrieving PRIMARY KEY from cache: {}", e);
                None
            }
        }
    } else {
        None
    }
}

#[allow(dead_code)]
pub(crate) fn get_indexed_columns_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<String>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let fut = async {
            sqlx::query_as::<_, (String,)>("SELECT DISTINCT column_name FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? AND is_indexed = 1 ORDER BY column_name")
              .bind(connection_id)
              .bind(database_name)
              .bind(table_name)
              .fetch_all(pool_clone.as_ref())
              .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(rows) => Some(rows.into_iter().map(|(name,)| name).collect()),
            Err(_) => None,
        }
    } else {
        None
    }
}

// Row cache: store and retrieve first-page (100 rows) snapshot for a table
pub(crate) fn save_table_rows_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    headers: &[String],
    rows: &[Vec<String>],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let database_name = database_name.to_string();
        let table_name = table_name.to_string();
        let headers_json = serde_json::to_string(headers).unwrap_or("[]".to_string());
        let rows_json = serde_json::to_string(rows).unwrap_or("[]".to_string());
        let fut = async {
            let _ = sqlx::query(
                r#"INSERT INTO row_cache (connection_id, database_name, table_name, headers_json, rows_json, updated_at)
                   VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
                   ON CONFLICT(connection_id, database_name, table_name)
                   DO UPDATE SET headers_json=excluded.headers_json, rows_json=excluded.rows_json, updated_at=CURRENT_TIMESTAMP"#,
            )
            .bind(connection_id)
            .bind(&database_name)
            .bind(&table_name)
            .bind(headers_json)
            .bind(rows_json)
            .execute(pool_clone.as_ref())
            .await;
        };
        if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
        info!(
            "💾 Saved first 100 rows to cache for {}/{}/{}",
            connection_id, database_name, table_name
        );
    }
}

pub(crate) fn get_table_rows_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let fut = async {
            sqlx::query_as::<_, (String, String)>(
                "SELECT headers_json, rows_json FROM row_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?",
            )
            .bind(connection_id)
            .bind(database_name)
            .bind(table_name)
            .fetch_optional(pool_clone.as_ref())
            .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(Some((headers_json, rows_json))) => {
                let headers: Vec<String> = serde_json::from_str(&headers_json).unwrap_or_default();
                let rows: Vec<Vec<String>> = serde_json::from_str(&rows_json).unwrap_or_default();
                info!(
                    "📦 Cache hit for rows {}/{}/{} ({} cols, {} rows)",
                    connection_id,
                    database_name,
                    table_name,
                    headers.len(),
                    rows.len()
                );
                Some((headers, rows))
            }
            Ok(None) => {
                info!(
                    "🕳️ No row cache found for {}/{}/{} — will use live server",
                    connection_id, database_name, table_name
                );
                None
            }
            Err(e) => {
                debug!(
                    "Row cache lookup error for {}/{}/{}: {}",
                    connection_id, database_name, table_name, e
                );
                None
            }
        }
    } else {
        None
    }
}

// Index cache: save full index metadata for a table (names, method, uniqueness, columns)
pub(crate) fn save_indexes_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    indexes: &[models::structs::IndexStructInfo],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let dbn = database_name.to_string();
        let tbn = table_name.to_string();
        let items: Vec<models::structs::IndexStructInfo> = indexes.to_vec();
        let fut = async move {
            // Clear existing index cache for this table
            let _ = sqlx::query(
                "DELETE FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?",
            )
            .bind(connection_id)
            .bind(&dbn)
            .bind(&tbn)
            .execute(pool_clone.as_ref())
            .await;

            // Insert each index row
            for idx in items {
                let cols_json = serde_json::to_string(&idx.columns).unwrap_or("[]".to_string());
                let _ = sqlx::query(
                    r#"INSERT OR REPLACE INTO index_cache
                        (connection_id, database_name, table_name, index_name, method, is_unique, columns_json)
                        VALUES (?, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(connection_id)
                .bind(&dbn)
                .bind(&tbn)
                .bind(idx.name)
                .bind(idx.method)
                .bind(if idx.unique { 1 } else { 0 })
                .bind(cols_json)
                .execute(pool_clone.as_ref())
                .await;
            }
        };
        if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
        info!(
            "💾 Saved {} indexes to cache for {}/{}/{}",
            indexes.len(),
            connection_id,
            database_name,
            table_name
        );
    }
}

// Get full index metadata from cache
pub(crate) fn get_indexes_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<models::structs::IndexStructInfo>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let query_sql = "SELECT index_name, method, is_unique, columns_json FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? ORDER BY index_name";
        debug!("🔑 Executing cache query for indexes: {}", query_sql);
        debug!(
            "   Parameters: connection_id={}, database={}, table={}",
            connection_id, database_name, table_name
        );

        let fut = async move {
            sqlx::query(query_sql)
                .bind(connection_id)
                .bind(database_name)
                .bind(table_name)
                .fetch_all(pool_clone.as_ref())
                .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(rows) => {
                use sqlx::Row;
                let mut list = Vec::new();
                for r in rows {
                    let name: String = r.try_get(0).unwrap_or_default();
                    let method: Option<String> = r.try_get(1).ok();
                    let is_unique_i: i64 = r.try_get(2).unwrap_or(0);
                    let cols_json: String = r.try_get(3).unwrap_or("[]".to_string());
                    let columns: Vec<String> = serde_json::from_str(&cols_json).unwrap_or_default();

                    // Log detailed info untuk PRIMARY KEY
                    if name == "PRIMARY" {
                        debug!(
                            "   🔐 Found PRIMARY KEY: columns={:?}, is_unique={}, method={:?}",
                            columns,
                            is_unique_i != 0,
                            method
                        );
                    }

                    list.push(models::structs::IndexStructInfo {
                        name,
                        method,
                        unique: is_unique_i != 0,
                        columns,
                    });
                }
                debug!(
                    "✅ Successfully retrieved {} indexes from index_cache",
                    list.len()
                );
                Some(list)
            }
            Err(e) => {
                debug!("❌ Error retrieving indexes from cache: {}", e);
                None
            }
        }
    } else {
        None
    }
}

// Get only index NAMES from cache (for quick tree rendering)
pub(crate) fn get_index_names_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<String>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let fut = async move {
            sqlx::query_as::<_, (String,)>(
                "SELECT DISTINCT index_name FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? ORDER BY index_name",
            )
            .bind(connection_id)
            .bind(database_name)
            .bind(table_name)
            .fetch_all(pool_clone.as_ref())
            .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
        match result {
            Ok(rows) => Some(rows.into_iter().map(|(n,)| n).collect()),
            Err(_) => None,
        }
    } else {
        None
    }
}

// Partition cache: save full partition metadata for a table
pub(crate) fn save_partitions_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    partitions: &[models::structs::PartitionStructInfo],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let dbn = database_name.to_string();
        let tbn = table_name.to_string();
        let fut = async move {
            for part in partitions {
                let _ = sqlx::query(
                    r#"INSERT OR REPLACE INTO partition_cache
                        (connection_id, database_name, table_name, partition_name, partition_type, partition_expression, subpartition_type)
                        VALUES (?, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(connection_id)
                .bind(&dbn)
                .bind(&tbn)
                .bind(&part.name)
                .bind(&part.partition_type)
                .bind(&part.partition_expression)
                .bind(&part.subpartition_type)
                .execute(pool_clone.as_ref())
                .await;
            }
        };
        if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };
        info!(
            "✅ Saved {} partitions to cache for {}.{}",
            partitions.len(),
            database_name,
            table_name
        );
    }
}

// Get full partition metadata from cache
pub(crate) fn get_partitions_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<models::structs::PartitionStructInfo>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let query_sql = "SELECT partition_name, partition_type, partition_expression, subpartition_type FROM partition_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? ORDER BY partition_name";

        let fut = async move {
            sqlx::query(query_sql)
                .bind(connection_id)
                .bind(database_name)
                .bind(table_name)
                .fetch_all(pool_clone.as_ref())
                .await
        };
        let result = if let Some(rt) = tabular.runtime.clone() {
            rt.block_on(fut)
        } else {
            tokio::runtime::Runtime::new().unwrap().block_on(fut)
        };

        match result {
            Ok(rows) => {
                use sqlx::Row;
                let mut list = Vec::new();
                for r in rows {
                    let name: String = r.try_get(0).unwrap_or_default();
                    let partition_type: Option<String> = r.try_get(1).ok();
                    let partition_expression: Option<String> = r.try_get(2).ok();
                    let subpartition_type: Option<String> = r.try_get(3).ok();

                    list.push(models::structs::PartitionStructInfo {
                        name,
                        partition_type,
                        partition_expression,
                        subpartition_type,
                    });
                }
                Some(list)
            }
            Err(_) => None,
        }
    } else {
        None
    }
}

