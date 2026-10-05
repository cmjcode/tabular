use log::debug;

use crate::{
    cache_data, connection, driver_mysql, driver_redis, driver_sqlite, models,
    window_egui::{self, Tabular},
};

fn spawn_cache_write<F>(tabular: &Tabular, fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Some(rt) = &tabular.runtime {
        rt.spawn(fut);
    }
}

/// Jalankan future pembacaan cache sampai selesai di runtime bersama milik
/// aplikasi. Bila runtime belum ada dan runtime sementara gagal dibuat (mis.
/// kehabisan thread/file descriptor), kembalikan error alih-alih panic.
fn block_on_cache<T, F>(tabular: &Tabular, fut: F) -> Result<T, sqlx::Error>
where
    F: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    if let Some(rt) = tabular.runtime.as_ref() {
        return rt.block_on(fut);
    }
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt.block_on(fut),
        Err(e) => {
            log::warn!("[CACHE] cannot create a runtime for cache lookup: {}", e);
            Err(sqlx::Error::Io(e))
        }
    }
}

/// Catat kegagalan penulisan cache; cache boleh gagal, tetapi tidak diam-diam.
fn log_cache_write(what: &str, connection_id: i64, result: Result<(), sqlx::Error>) {
    if let Err(e) = result {
        log::warn!(
            "[CACHE] failed to save {} for connection {}: {}",
            what,
            connection_id,
            e
        );
    }
}

/// Ganti daftar database sebuah koneksi dalam SATU transaksi: DELETE dan semua
/// INSERT berhasil bersama atau tidak sama sekali (tx yang di-drop = rollback).
pub(crate) async fn write_databases_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    databases: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;
    for db_name in databases {
        sqlx::query(
            "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
        )
        .bind(connection_id)
        .bind(db_name)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Ganti isi `table_cache` untuk tiap `table_type` yang ada di `tables`
/// (nama, tipe, komentar) dalam satu transaksi.
pub(crate) async fn write_tables_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    database_name: &str,
    tables: &[(String, String, Option<String>)],
) -> Result<(), sqlx::Error> {
    let types_to_replace: std::collections::BTreeSet<&str> =
        tables.iter().map(|(_, t, _)| t.as_str()).collect();
    let mut tx = pool.begin().await?;
    for table_type in types_to_replace {
        sqlx::query(
            "DELETE FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type = ?",
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(table_type)
        .execute(&mut *tx)
        .await?;
    }
    for (table_name, table_type, comment) in tables {
        sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type, comment) VALUES (?, ?, ?, ?, ?)")
            .bind(connection_id)
            .bind(database_name)
            .bind(table_name)
            .bind(table_type)
            .bind(comment)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// Ganti kolom (nama, tipe) sebuah tabel dalam satu transaksi.
pub(crate) async fn write_columns_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    columns: &[(String, String)],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?",
    )
    .bind(connection_id)
    .bind(database_name)
    .bind(table_name)
    .execute(&mut *tx)
    .await?;
    for (i, (column_name, data_type)) in columns.iter().enumerate() {
        sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(connection_id)
            .bind(database_name)
            .bind(table_name)
            .bind(column_name)
            .bind(data_type)
            .bind(i as i64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// Ganti daftar key Redis browser (nama, tipe) sebuah database dalam satu
/// transaksi.
pub(crate) async fn write_redis_browser_keys_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    database_name: &str,
    keys: &[(String, String)],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type LIKE 'redis_browser_key::%'",
    )
    .bind(connection_id)
    .bind(database_name)
    .execute(&mut *tx)
    .await?;
    for (key_name, key_type) in keys {
        sqlx::query(
            "INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)",
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(key_name)
        .bind(redis_browser_cache_type(key_type))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Ganti metadata index sebuah tabel dalam satu transaksi.
pub(crate) async fn write_indexes_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    indexes: &[models::structs::IndexStructInfo],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?",
    )
    .bind(connection_id)
    .bind(database_name)
    .bind(table_name)
    .execute(&mut *tx)
    .await?;
    for idx in indexes {
        let cols_json = serde_json::to_string(&idx.columns).unwrap_or("[]".to_string());
        sqlx::query(
            r#"INSERT OR REPLACE INTO index_cache
                (connection_id, database_name, table_name, index_name, method, is_unique, columns_json)
                VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(table_name)
        .bind(&idx.name)
        .bind(&idx.method)
        .bind(if idx.unique { 1 } else { 0 })
        .bind(cols_json)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Ganti metadata partisi sebuah tabel dalam satu transaksi. Partisi lama yang
/// sudah tidak ada ikut terhapus.
pub(crate) async fn write_partitions_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    partitions: &[models::structs::PartitionStructInfo],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM partition_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?",
    )
    .bind(connection_id)
    .bind(database_name)
    .bind(table_name)
    .execute(&mut *tx)
    .await?;
    for part in partitions {
        sqlx::query(
            r#"INSERT OR REPLACE INTO partition_cache
                (connection_id, database_name, table_name, partition_name, partition_type, partition_expression, subpartition_type)
                VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(table_name)
        .bind(&part.name)
        .bind(&part.partition_type)
        .bind(&part.partition_expression)
        .bind(&part.subpartition_type)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Umur maksimum entri memo foreign key. Jaring pengaman untuk penulis
/// `foreign_key_cache` di modul lain yang belum memanggil
/// [`invalidate_foreign_key_memo`].
const FK_MEMO_TTL: std::time::Duration = std::time::Duration::from_secs(2);

type FkMemoKey = (i64, String);
type FkMemoValue = (
    std::time::Instant,
    std::sync::Arc<Vec<models::structs::ForeignKey>>,
);

/// Memo proses untuk [`get_foreign_keys_from_cache_shared`]: grid memanggilnya
/// tiap frame, dan tanpa memo itu berarti satu query SQLite per frame.
static FK_MEMO: std::sync::Mutex<Option<std::collections::HashMap<FkMemoKey, FkMemoValue>>> =
    std::sync::Mutex::new(None);

fn lock_fk_memo()
-> std::sync::MutexGuard<'static, Option<std::collections::HashMap<FkMemoKey, FkMemoValue>>> {
    FK_MEMO.lock().unwrap_or_else(|e| e.into_inner())
}

fn fk_memo_get(
    connection_id: i64,
    database_name: &str,
    now: std::time::Instant,
) -> Option<std::sync::Arc<Vec<models::structs::ForeignKey>>> {
    let guard = lock_fk_memo();
    let (stored_at, fks) = guard
        .as_ref()?
        .get(&(connection_id, database_name.to_string()))?;
    (now.saturating_duration_since(*stored_at) < FK_MEMO_TTL).then(|| fks.clone())
}

fn fk_memo_put(
    connection_id: i64,
    database_name: &str,
    now: std::time::Instant,
    fks: std::sync::Arc<Vec<models::structs::ForeignKey>>,
) {
    lock_fk_memo()
        .get_or_insert_with(std::collections::HashMap::new)
        .insert((connection_id, database_name.to_string()), (now, fks));
}

/// Buang memo foreign key milik satu koneksi (semua database-nya). Panggil
/// setiap kali tabel `foreign_key_cache` ditulis atau dihapus untuk koneksi itu.
pub fn invalidate_foreign_key_memo(connection_id: i64) {
    if let Some(memo) = lock_fk_memo().as_mut() {
        memo.retain(|(cid, _), _| *cid != connection_id);
    }
}

/// Buang seluruh memo foreign key (mis. setelah `connections.db` diganti atau
/// di-import ulang, ketika id koneksi bisa terpakai lagi).
pub fn invalidate_foreign_key_memo_all() {
    *lock_fk_memo() = None;
}

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
        let result = block_on_cache(tabular, fut);

        match result {
            Ok(rows) => {
                // Deduplicate — same table_name can appear multiple times if caching
                // paths ran concurrently or on reconnect.
                let mut seen = std::collections::HashSet::new();
                let deduped: Vec<String> = rows
                    .into_iter()
                    .map(|(name,)| name)
                    .filter(|n| seen.insert(n.clone()))
                    .collect();
                Some(deduped)
            }
            Err(e) => {
                debug!(
                    "get_tables_from_cache error: conn={} db={:?} type={:?} err={}",
                    connection_id, database_name, table_type, e
                );
                None
            }
        }
    } else {
        None
    }
}

pub(crate) fn get_tables_with_comments_from_cache(
    tabular: &Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<(String, Option<String>)>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let fut = async {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT table_name, comment FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type = ? ORDER BY table_name",
            )
            .bind(connection_id)
            .bind(database_name)
            .bind(table_type)
            .fetch_all(pool_clone.as_ref())
            .await
        };
        let result = block_on_cache(tabular, fut);

        match result {
            Ok(rows) => {
                let mut seen = std::collections::HashSet::new();
                let deduped: Vec<(String, Option<String>)> = rows
                    .into_iter()
                    .filter(|(name, _)| seen.insert(name.clone()))
                    .collect();
                Some(deduped)
            }
            Err(e) => {
                debug!(
                    "get_tables_with_comments_from_cache error: conn={} db={:?} type={:?} err={}",
                    connection_id, database_name, table_type, e
                );
                None
            }
        }
    } else {
        None
    }
}

/// Like `get_tables_from_cache` but NOT scoped to a database — returns every
/// cached table/view of `table_type` for the connection across all databases.
/// Used as an autocomplete fallback when the active editor tab isn't pinned to a
/// specific database (so `database_name` is empty or doesn't match the cache).
#[allow(dead_code)]
pub(crate) fn get_tables_for_connection_any_db(
    tabular: &Tabular,
    connection_id: i64,
    table_type: &str,
) -> Option<Vec<String>> {
    let pool = tabular.db_pool.as_ref()?.clone();
    let fut = async {
        sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT table_name FROM table_cache WHERE connection_id = ? AND table_type = ? ORDER BY table_name",
        )
        .bind(connection_id)
        .bind(table_type)
        .fetch_all(pool.as_ref())
        .await
    };
    let result = block_on_cache(tabular, fut);
    result
        .ok()
        .map(|rows| rows.into_iter().map(|(n,)| n).collect())
}

/// Resolve which database a cached table belongs to (first match). Used so the
/// autocomplete can lazily fetch a table's columns with the right database when
/// the editor tab isn't pinned to one.
#[allow(dead_code)]
pub(crate) fn get_table_database_from_cache(
    tabular: &Tabular,
    connection_id: i64,
    table_name: &str,
) -> Option<String> {
    let pool = tabular.db_pool.as_ref()?.clone();
    let fut = async {
        sqlx::query_as::<_, (String,)>(
            "SELECT database_name FROM table_cache WHERE connection_id = ? AND table_name = ? COLLATE NOCASE LIMIT 1",
        )
        .bind(connection_id)
        .bind(table_name)
        .fetch_optional(pool.as_ref())
        .await
    };
    let result = block_on_cache(tabular, fut);
    result.ok().flatten().map(|(d,)| d)
}

/// Every cached table/view name across ALL connections and databases. Last-ditch
/// autocomplete fallback when neither the tab's connection nor the database can
/// be resolved but `table_cache` does hold data.
#[allow(dead_code)]
pub(crate) fn get_all_cached_tables_global(tabular: &Tabular) -> Option<Vec<String>> {
    let pool = tabular.db_pool.as_ref()?.clone();
    let fut = async {
        sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT table_name FROM table_cache WHERE table_type IN ('table','view') ORDER BY table_name",
        )
        .fetch_all(pool.as_ref())
        .await
    };
    let result = block_on_cache(tabular, fut);
    result
        .ok()
        .map(|rows| rows.into_iter().map(|(n,)| n).collect())
}

/// Like `get_columns_from_cache` but NOT scoped to a database. Returns the first
/// cached column set found for `table_name` under the connection (any database).
#[allow(dead_code)]
pub(crate) fn get_columns_for_connection_any_db(
    tabular: &Tabular,
    connection_id: i64,
    table_name: &str,
) -> Option<Vec<(String, String)>> {
    let pool = tabular.db_pool.as_ref()?.clone();
    let fut = async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT column_name, data_type FROM column_cache WHERE connection_id = ? AND table_name = ? COLLATE NOCASE ORDER BY ordinal_position",
        )
        .bind(connection_id)
        .bind(table_name)
        .fetch_all(pool.as_ref())
        .await
    };
    let result = block_on_cache(tabular, fut);
    match result {
        Ok(rows) if !rows.is_empty() => Some(rows),
        _ => None,
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
        let result = block_on_cache(tabular, fut);

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
    _tabular: &mut window_egui::Tabular,
    connection_id: i64,
    node: &mut models::structs::TreeNode,
    databases: &[String],
) {
    if databases.len() == 1 && databases[0] == crate::driver_redis::REDIS_CLUSTER_KEYSPACE {
        let mut cluster_node =
            models::structs::TreeNode::new("Keys".to_string(), models::enums::NodeType::Database);
        cluster_node.connection_id = Some(connection_id);
        cluster_node.database_name = Some(crate::driver_redis::REDIS_CLUSTER_KEYSPACE.to_string());
        cluster_node.is_loaded = false;
        cluster_node.children.push(models::structs::TreeNode::new(
            "Loading keys...".to_string(),
            models::enums::NodeType::Table,
        ));
        node.children = vec![cluster_node];
        return;
    }

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

            // Always add a placeholder so the node is expandable and triggers a
            // background key-fetch on click. This also handles Redis Cluster, where
            // the _has_keys marker is never written by this path.
            let loading_node = models::structs::TreeNode::new(
                "Loading keys...".to_string(),
                models::enums::NodeType::Table,
            );
            db_node.children.push(loading_node);

            databases_folder.children.push(db_node);
        }
    }

    main_children.push(databases_folder);
    node.children = main_children;
}

// Cache functions for database structure

/// Delete all table_cache rows for a specific connection + database (all table_types).
/// Used before a forced refresh so the live fetch always runs instead of returning stale cache.
pub(crate) fn clear_tables_from_cache_for_db(
    tabular: &window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let db = database_name.to_string();
        let fut = async move {
            match sqlx::query(
                "DELETE FROM table_cache WHERE connection_id = ? AND database_name = ?",
            )
            .bind(connection_id)
            .bind(&db)
            .execute(pool_clone.as_ref())
            .await
            {
                Ok(_) => {}
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("code: 11")
                        || err_str.contains("malformed")
                        || err_str.contains("corrupt")
                    {
                        let vacuum_result =
                            sqlx::query("VACUUM").execute(pool_clone.as_ref()).await;
                        match vacuum_result {
                            Ok(_) => {
                                let _ = sqlx::query(
                                    "DELETE FROM table_cache WHERE connection_id = ? AND database_name = ?",
                                )
                                .bind(connection_id)
                                .bind(&db)
                                .execute(pool_clone.as_ref())
                                .await;
                            }
                            Err(_) => {
                                let _ = sqlx::query("DELETE FROM table_cache")
                                    .execute(pool_clone.as_ref())
                                    .await;
                            }
                        }
                    }
                }
            }
        };
        spawn_cache_write(tabular, fut);
    }
}

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
        let fut = async move {
            let result =
                write_databases_cache(pool_clone.as_ref(), connection_id, &databases_clone).await;
            log_cache_write("databases", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
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

    // Engine plugin: metadata diambil lewat driver API dalam satu langkah.
    if connection.connection_type.plugin_id().is_some() {
        let pool = tabular.connection_pools.get(&connection_id).cloned();
        if let (Some(models::enums::DatabasePool::Plugin(pool)), Some(cache_pool)) =
            (pool, tabular.db_pool.clone())
        {
            let rt = tabular.get_runtime();
            rt.block_on(crate::driver_api::cache::fetch_plugin_data(
                connection_id,
                &pool,
                &connection.database,
                cache_pool.as_ref(),
            ));
        }
        return;
    }

    // Fetch databases from server
    #[allow(deprecated)]
    #[allow(deprecated)]
    let databases_result =
        connection::fetch_databases_from_connection_blocking(tabular, connection_id);

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
                models::enums::DatabaseType::ApiHttp | models::enums::DatabaseType::Plugin(_) => {
                    vec![]
                }
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
                    models::enums::DatabaseType::ApiHttp
                    | models::enums::DatabaseType::Plugin(_) => None,
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

pub(crate) fn save_tables_with_comments_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    tables: &[(String, String, Option<String>)],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let tables_clone = tables.to_vec();
        let database_name = database_name.to_string();
        let fut = async move {
            let result = write_tables_cache(
                pool_clone.as_ref(),
                connection_id,
                &database_name,
                &tables_clone,
            )
            .await;
            log_cache_write("tables", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
    }
}

pub(crate) fn save_tables_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    tables: &[(String, String)],
) {
    let with_comments: Vec<(String, String, Option<String>)> = tables
        .iter()
        .map(|(name, t_type)| (name.clone(), t_type.clone(), None))
        .collect();
    save_tables_with_comments_to_cache(tabular, connection_id, database_name, &with_comments);
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
        let fut = async move {
            let result = write_columns_cache(
                pool_clone.as_ref(),
                connection_id,
                &database_name,
                &table_name,
                &columns_clone,
            )
            .await;
            log_cache_write("columns", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
    }
}

/// Read cached foreign keys for a connection. When `database_name` is empty,
/// returns FKs across all cached databases for that connection (autocomplete
/// often doesn't have an explicit active database).
///
/// Pembungkus tipis di atas [`get_foreign_keys_from_cache_shared`]; pemanggil
/// per-frame sebaiknya memakai versi `_shared` supaya `Vec`-nya tidak disalin.
pub(crate) fn get_foreign_keys_from_cache(
    tabular: &window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) -> Option<Vec<models::structs::ForeignKey>> {
    get_foreign_keys_from_cache_shared(tabular, connection_id, database_name)
        .map(|fks| fks.as_ref().clone())
}

/// Seperti [`get_foreign_keys_from_cache`] tetapi hasilnya dibagi lewat `Arc`
/// dan dimemo per `(connection_id, database_name)`, sehingga pemanggilan tiap
/// frame tidak menyentuh SQLite. Memo dibuang lewat
/// [`invalidate_foreign_key_memo`] dan kedaluwarsa sendiri setelah
/// [`FK_MEMO_TTL`].
pub(crate) fn get_foreign_keys_from_cache_shared(
    tabular: &window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) -> Option<std::sync::Arc<Vec<models::structs::ForeignKey>>> {
    let now = std::time::Instant::now();
    if let Some(hit) = fk_memo_get(connection_id, database_name, now) {
        return Some(hit);
    }
    let pool = tabular.db_pool.as_ref()?.clone();
    let result = block_on_cache(
        tabular,
        read_foreign_keys_cache(pool.as_ref(), connection_id, database_name),
    );
    match result {
        Ok(fks) => {
            let fks = std::sync::Arc::new(fks);
            fk_memo_put(connection_id, database_name, now, fks.clone());
            Some(fks)
        }
        Err(e) => {
            debug!("❌ Error retrieving foreign keys from cache: {}", e);
            None
        }
    }
}

/// Baca foreign key dari `foreign_key_cache` (headless).
pub(crate) async fn read_foreign_keys_cache(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
    database_name: &str,
) -> Result<Vec<models::structs::ForeignKey>, sqlx::Error> {
    let rows = if database_name.is_empty() {
        sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT table_name, column_name, referenced_table_name, referenced_column_name, constraint_name FROM foreign_key_cache WHERE connection_id = ?",
        )
        .bind(connection_id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT table_name, column_name, referenced_table_name, referenced_column_name, constraint_name FROM foreign_key_cache WHERE connection_id = ? AND database_name = ?",
        )
        .bind(connection_id)
        .bind(database_name)
        .fetch_all(pool)
        .await?
    };
    Ok(rows
        .into_iter()
        .map(
            |(
                table_name,
                column_name,
                referenced_table_name,
                referenced_column_name,
                constraint_name,
            )| {
                models::structs::ForeignKey {
                    constraint_name,
                    table_name,
                    column_name,
                    referenced_table_name,
                    referenced_column_name,
                }
            },
        )
        .collect())
}

pub(crate) fn get_columns_from_cache(
    tabular: &window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
) -> Option<Vec<(String, String)>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let query_sql = "SELECT column_name, data_type FROM column_cache WHERE connection_id = ? AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE ORDER BY ordinal_position";
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
        let result = block_on_cache(tabular, fut);

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
        let result = block_on_cache(tabular, fut);

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
        let result = block_on_cache(tabular, fut);

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
        let db_name_str = database_name.to_string();
        let tbl_name_str = table_name.to_string();
        let headers_json = serde_json::to_string(headers).unwrap_or("[]".to_string());
        let rows_json = serde_json::to_string(rows).unwrap_or("[]".to_string());
        let fut = async move {
            let result = sqlx::query(
                r#"INSERT INTO row_cache (connection_id, database_name, table_name, headers_json, rows_json, updated_at)
                   VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
                   ON CONFLICT(connection_id, database_name, table_name)
                   DO UPDATE SET headers_json=excluded.headers_json, rows_json=excluded.rows_json, updated_at=CURRENT_TIMESTAMP"#,
            )
            .bind(connection_id)
            .bind(&db_name_str)
            .bind(&tbl_name_str)
            .bind(headers_json)
            .bind(rows_json)
            .execute(pool_clone.as_ref())
            .await
            .map(|_| ());
            log_cache_write("row preview", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
        debug!(
            "💾 Queued saving first 100 rows to cache for {}/{}/{}",
            connection_id, database_name, table_name
        );
    }
}

fn redis_browser_cache_type(key_type: &str) -> String {
    format!("redis_browser_key::{}", key_type)
}

fn redis_browser_preview_cache_name(key_name: &str) -> String {
    format!("__redis_preview__::{}", key_name)
}

pub(crate) fn save_redis_browser_keys_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    keys: &[(String, String)],
) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let database_name = database_name.to_string();
        let keys = keys.to_vec();
        let fut = async move {
            let result = write_redis_browser_keys_cache(
                pool_clone.as_ref(),
                connection_id,
                &database_name,
                &keys,
            )
            .await;
            log_cache_write("redis keys", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
    }
}

pub(crate) fn get_redis_browser_keys_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) -> Option<Vec<(String, String)>> {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let database_name = database_name.to_string();
        let fut = async move {
            sqlx::query_as::<_, (String, String)>(
                "SELECT table_name, table_type FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type LIKE 'redis_browser_key::%' ORDER BY table_name",
            )
            .bind(connection_id)
            .bind(&database_name)
            .fetch_all(pool_clone.as_ref())
            .await
        };
        let result = block_on_cache(tabular, fut);

        match result {
            Ok(rows) if !rows.is_empty() => Some(
                rows.into_iter()
                    .map(|(key_name, table_type)| {
                        let key_type = table_type
                            .strip_prefix("redis_browser_key::")
                            .unwrap_or("unknown")
                            .to_string();
                        (key_name, key_type)
                    })
                    .collect(),
            ),
            _ => None,
        }
    } else {
        None
    }
}

pub(crate) fn save_redis_browser_preview_to_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    preview: &models::structs::RedisBrowserPreview,
) {
    let headers = vec![
        "key_type".to_string(),
        "ttl_label".to_string(),
        "size_label".to_string(),
        "length_label".to_string(),
        "json_text".to_string(),
    ];
    let rows = vec![vec![
        preview.key_type.clone(),
        preview.ttl_label.clone(),
        preview.size_label.clone(),
        preview.length_label.clone(),
        preview.json_text.clone(),
    ]];
    save_table_rows_to_cache(
        tabular,
        connection_id,
        &preview.database_name,
        &redis_browser_preview_cache_name(&preview.key_name),
        &headers,
        &rows,
    );
}

pub(crate) fn get_redis_browser_preview_from_cache(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    key_name: &str,
) -> Option<models::structs::RedisBrowserPreview> {
    let cache_name = redis_browser_preview_cache_name(key_name);
    let (headers, rows) =
        get_table_rows_from_cache(tabular, connection_id, database_name, &cache_name)?;
    let first_row = rows.first()?;
    if first_row.len() != headers.len() {
        return None;
    }

    let mut values = std::collections::HashMap::new();
    for (header, value) in headers.into_iter().zip(first_row.iter().cloned()) {
        values.insert(header, value);
    }

    Some(models::structs::RedisBrowserPreview {
        key_name: key_name.to_string(),
        key_type: values
            .remove("key_type")
            .unwrap_or_else(|| "unknown".to_string()),
        database_name: database_name.to_string(),
        ttl_label: values
            .remove("ttl_label")
            .unwrap_or_else(|| "-".to_string()),
        size_label: values
            .remove("size_label")
            .unwrap_or_else(|| "-".to_string()),
        length_label: values
            .remove("length_label")
            .unwrap_or_else(|| "-".to_string()),
        json_text: values.remove("json_text").unwrap_or_default(),
    })
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
        let result = block_on_cache(tabular, fut);

        match result {
            Ok(Some((headers_json, rows_json))) => {
                let headers: Vec<String> = serde_json::from_str(&headers_json).unwrap_or_default();
                let rows: Vec<Vec<String>> = serde_json::from_str(&rows_json).unwrap_or_default();
                debug!(
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
                debug!(
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
            let result =
                write_indexes_cache(pool_clone.as_ref(), connection_id, &dbn, &tbn, &items).await;
            log_cache_write("indexes", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
        debug!(
            "💾 Queued saving {} indexes to cache for {}/{}/{}",
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
        let result = block_on_cache(tabular, fut);

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
        let result = block_on_cache(tabular, fut);
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
        let partitions_clone = partitions.to_vec();
        let fut = async move {
            let result = write_partitions_cache(
                pool_clone.as_ref(),
                connection_id,
                &dbn,
                &tbn,
                &partitions_clone,
            )
            .await;
            log_cache_write("partitions", connection_id, result);
        };
        spawn_cache_write(tabular, fut);
        debug!(
            "✅ Queued saving {} partitions to cache for {}.{}",
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
        let result = block_on_cache(tabular, fut);

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

#[cfg(test)]
mod tests {
    use super::*;

    async fn cache_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        // Skema sama dengan `sidebar_database::initialize_database_background`.
        sqlx::query(
            r#"
            CREATE TABLE database_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(connection_id, database_name));
            CREATE TABLE table_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, table_type TEXT NOT NULL, comment TEXT, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(connection_id, database_name, table_name, table_type));
            CREATE TABLE column_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, column_name TEXT NOT NULL, data_type TEXT NOT NULL, ordinal_position INTEGER NOT NULL, is_primary_key INTEGER NOT NULL DEFAULT 0, is_indexed INTEGER NOT NULL DEFAULT 0, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(connection_id, database_name, table_name, column_name));
            CREATE TABLE index_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, index_name TEXT NOT NULL, method TEXT NULL, is_unique INTEGER NOT NULL DEFAULT 0, columns_json TEXT NOT NULL DEFAULT '[]', created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(connection_id, database_name, table_name, index_name));
            CREATE TABLE partition_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, partition_name TEXT NOT NULL, partition_type TEXT NULL, partition_expression TEXT NULL, subpartition_type TEXT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(connection_id, database_name, table_name, partition_name));
            CREATE TABLE foreign_key_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, column_name TEXT NOT NULL, referenced_table_name TEXT NOT NULL, referenced_column_name TEXT NOT NULL, constraint_name TEXT NOT NULL DEFAULT '', created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(connection_id, database_name, table_name, column_name, referenced_table_name, referenced_column_name));
            "#,
        )
        .execute(&pool)
        .await
        .expect("cache schema");
        pool
    }

    async fn names(pool: &sqlx::SqlitePool, sql: &'static str) -> Vec<String> {
        sqlx::query_as::<_, (String,)>(sql)
            .fetch_all(pool)
            .await
            .expect("select")
            .into_iter()
            .map(|(n,)| n)
            .collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn databases_save_replaces_prior_rows_without_duplicates() {
        let pool = cache_pool().await;
        write_databases_cache(&pool, 1, &strings(&["a", "b", "c"]))
            .await
            .expect("first save");
        write_databases_cache(&pool, 2, &strings(&["other"]))
            .await
            .expect("other connection");
        // Daftar baru (dengan duplikat di input) menggantikan yang lama.
        write_databases_cache(&pool, 1, &strings(&["b", "d", "d"]))
            .await
            .expect("second save");

        let sql = "SELECT database_name FROM database_cache WHERE connection_id = 1 ORDER BY database_name";
        assert_eq!(names(&pool, sql).await, strings(&["b", "d"]));
        let other = "SELECT database_name FROM database_cache WHERE connection_id = 2";
        assert_eq!(names(&pool, other).await, strings(&["other"]));
    }

    /// INSERT yang gagal di tengah harus membatalkan DELETE-nya juga: cache
    /// lama tetap utuh, bukan kosong atau setengah terisi.
    #[tokio::test]
    async fn failed_save_rolls_back_and_keeps_previous_rows() {
        let pool = cache_pool().await;
        write_databases_cache(&pool, 1, &strings(&["a", "b"]))
            .await
            .expect("seed");
        sqlx::query(
            "CREATE TRIGGER reject_boom BEFORE INSERT ON database_cache WHEN NEW.database_name = 'boom' BEGIN SELECT RAISE(ABORT, 'boom'); END",
        )
        .execute(&pool)
        .await
        .expect("trigger");

        let result = write_databases_cache(&pool, 1, &strings(&["x", "boom", "y"])).await;
        assert!(result.is_err());

        let sql = "SELECT database_name FROM database_cache WHERE connection_id = 1 ORDER BY database_name";
        assert_eq!(names(&pool, sql).await, strings(&["a", "b"]));
    }

    #[tokio::test]
    async fn tables_save_replaces_only_the_given_types() {
        let pool = cache_pool().await;
        let t = |name: &str, kind: &str| (name.to_string(), kind.to_string(), None::<String>);
        write_tables_cache(
            &pool,
            1,
            "db",
            &[t("users", "table"), t("orders", "table"), t("v1", "view")],
        )
        .await
        .expect("first save");
        write_tables_cache(
            &pool,
            1,
            "db",
            &[
                (
                    "users".to_string(),
                    "table".to_string(),
                    Some("[A]".to_string()),
                ),
                t("items", "table"),
            ],
        )
        .await
        .expect("second save");

        let tables = "SELECT table_name FROM table_cache WHERE connection_id = 1 AND table_type = 'table' ORDER BY table_name";
        assert_eq!(names(&pool, tables).await, strings(&["items", "users"]));
        // Tipe yang tidak ikut disimpan tidak tersentuh.
        let views =
            "SELECT table_name FROM table_cache WHERE connection_id = 1 AND table_type = 'view'";
        assert_eq!(names(&pool, views).await, strings(&["v1"]));
        let comment = "SELECT comment FROM table_cache WHERE table_name = 'users'";
        assert_eq!(names(&pool, comment).await, strings(&["[A]"]));
    }

    #[tokio::test]
    async fn columns_indexes_and_partitions_are_replaced_per_table() {
        let pool = cache_pool().await;
        let col = |n: &str| (n.to_string(), "int".to_string());
        write_columns_cache(&pool, 1, "db", "t", &[col("a"), col("b"), col("c")])
            .await
            .expect("columns 1");
        write_columns_cache(&pool, 1, "db", "other", &[col("z")])
            .await
            .expect("columns other");
        write_columns_cache(&pool, 1, "db", "t", &[col("c"), col("a")])
            .await
            .expect("columns 2");
        let sql =
            "SELECT column_name FROM column_cache WHERE table_name = 't' ORDER BY ordinal_position";
        assert_eq!(names(&pool, sql).await, strings(&["c", "a"]));
        let other = "SELECT column_name FROM column_cache WHERE table_name = 'other'";
        assert_eq!(names(&pool, other).await, strings(&["z"]));

        let idx = |n: &str| models::structs::IndexStructInfo {
            name: n.to_string(),
            method: Some("btree".to_string()),
            unique: false,
            columns: vec!["a".to_string()],
        };
        write_indexes_cache(&pool, 1, "db", "t", &[idx("i1"), idx("i2")])
            .await
            .expect("indexes 1");
        write_indexes_cache(&pool, 1, "db", "t", &[idx("i2")])
            .await
            .expect("indexes 2");
        let sql = "SELECT index_name FROM index_cache WHERE table_name = 't'";
        assert_eq!(names(&pool, sql).await, strings(&["i2"]));

        let part = |n: &str| models::structs::PartitionStructInfo {
            name: n.to_string(),
            ..Default::default()
        };
        write_partitions_cache(&pool, 1, "db", "t", &[part("p1"), part("p2")])
            .await
            .expect("partitions 1");
        write_partitions_cache(&pool, 1, "db", "t", &[part("p2"), part("p3")])
            .await
            .expect("partitions 2");
        let sql = "SELECT partition_name FROM partition_cache WHERE table_name = 't' ORDER BY partition_name";
        assert_eq!(names(&pool, sql).await, strings(&["p2", "p3"]));
    }

    #[tokio::test]
    async fn redis_keys_save_keeps_regular_tables() {
        let pool = cache_pool().await;
        write_tables_cache(
            &pool,
            1,
            "0",
            &[("t".to_string(), "table".to_string(), None)],
        )
        .await
        .expect("table");
        let key = |n: &str| (n.to_string(), "string".to_string());
        write_redis_browser_keys_cache(&pool, 1, "0", &[key("k1"), key("k2")])
            .await
            .expect("keys 1");
        write_redis_browser_keys_cache(&pool, 1, "0", &[key("k3")])
            .await
            .expect("keys 2");
        let sql = "SELECT table_name FROM table_cache WHERE connection_id = 1 ORDER BY table_name";
        assert_eq!(names(&pool, sql).await, strings(&["k3", "t"]));
    }

    #[tokio::test]
    async fn foreign_keys_are_read_per_database_or_for_all() {
        let pool = cache_pool().await;
        for (db, table) in [("a", "orders"), ("b", "items")] {
            sqlx::query("INSERT INTO foreign_key_cache (connection_id, database_name, table_name, column_name, referenced_table_name, referenced_column_name, constraint_name) VALUES (1, ?, ?, 'user_id', 'users', 'id', 'fk')")
                .bind(db)
                .bind(table)
                .execute(&pool)
                .await
                .expect("insert fk");
        }
        assert_eq!(
            read_foreign_keys_cache(&pool, 1, "")
                .await
                .expect("all")
                .len(),
            2
        );
        let only_a = read_foreign_keys_cache(&pool, 1, "a").await.expect("a");
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].table_name, "orders");
        assert!(
            read_foreign_keys_cache(&pool, 2, "")
                .await
                .expect("none")
                .is_empty()
        );
    }

    #[test]
    fn fk_memo_hits_expires_and_invalidates_per_connection() {
        // Id unik supaya tidak bertabrakan dengan tes lain (memo bersifat global).
        let (conn_a, conn_b) = (-9_000_001, -9_000_002);
        let now = std::time::Instant::now();
        let fks = std::sync::Arc::new(vec![models::structs::ForeignKey {
            constraint_name: "fk".to_string(),
            table_name: "orders".to_string(),
            column_name: "user_id".to_string(),
            referenced_table_name: "users".to_string(),
            referenced_column_name: "id".to_string(),
        }]);
        fk_memo_put(conn_a, "db", now, fks.clone());
        fk_memo_put(conn_a, "", now, fks.clone());
        fk_memo_put(conn_b, "db", now, fks.clone());

        let hit = fk_memo_get(conn_a, "db", now).expect("memo hit");
        assert!(std::sync::Arc::ptr_eq(&hit, &fks));
        assert!(fk_memo_get(conn_a, "other", now).is_none());
        // Kedaluwarsa setelah TTL.
        assert!(fk_memo_get(conn_a, "db", now + FK_MEMO_TTL).is_none());

        invalidate_foreign_key_memo(conn_a);
        assert!(fk_memo_get(conn_a, "db", now).is_none());
        assert!(fk_memo_get(conn_a, "", now).is_none());
        assert!(fk_memo_get(conn_b, "db", now).is_some());
        invalidate_foreign_key_memo(conn_b);
        assert!(fk_memo_get(conn_b, "db", now).is_none());
    }
}
