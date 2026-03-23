use crate::{driver_mssql, driver_mysql, driver_postgres, driver_redis, driver_sqlite, models, modules};
use futures_util::stream::StreamExt;
use sqlx::{Column, SqlitePool};
use sqlx::Connection as SqlxConnection; // required for MySqlConnection::connect

// Limit concurrent prefetch tasks
pub(super) const PREFETCH_CONCURRENCY: usize = 6;

// Fetch and cache metadata for all databases/tables/columns per connection
#[allow(dead_code)]
pub(crate) async fn fetch_and_cache_all_data(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    pool: &models::enums::DatabasePool,
    cache_pool: &SqlitePool,
) -> bool {
    match &connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            if let models::enums::DatabasePool::MySQL(mysql_pool) = pool {
                driver_mysql::fetch_mysql_data(connection_id, mysql_pool, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::SQLite => {
            if let models::enums::DatabasePool::SQLite(sqlite_pool) = pool {
                driver_sqlite::fetch_data(connection_id, sqlite_pool, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            if let models::enums::DatabasePool::PostgreSQL(postgres_pool) = pool {
                driver_postgres::fetch_postgres_data(connection_id, postgres_pool, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::Redis => {
            if let models::enums::DatabasePool::Redis(redis_manager) = pool {
                driver_redis::fetch_redis_data(connection_id, redis_manager, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::MsSQL => {
            if let models::enums::DatabasePool::MsSQL(mssql_cfg) = pool {
                driver_mssql::fetch_mssql_data(connection_id, mssql_cfg.clone(), cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::MongoDB => {
            if let models::enums::DatabasePool::MongoDB(client) = pool {
                crate::driver_mongodb::fetch_mongodb_data(connection_id, client.clone(), cache_pool)
                    .await
            } else {
                false
            }
        }
        models::enums::DatabaseType::ApiHttp => false,
    }
}

// Helper: upsert row_cache directly using cache pool
pub(super) async fn save_row_cache_direct(
    cache_pool: &SqlitePool,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    headers: &[String],
    rows: &Vec<Vec<String>>,
) {
    let headers_json = serde_json::to_string(headers).unwrap_or_else(|_| "[]".to_string());
    let rows_json = serde_json::to_string(rows).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query(
        r#"INSERT INTO row_cache (connection_id, database_name, table_name, headers_json, rows_json, updated_at)
           VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(connection_id, database_name, table_name)
           DO UPDATE SET headers_json=excluded.headers_json, rows_json=excluded.rows_json, updated_at=CURRENT_TIMESTAMP"#,
    )
    .bind(connection_id)
    .bind(database_name)
    .bind(table_name)
    .bind(headers_json)
    .bind(rows_json)
    .execute(cache_pool)
    .await;
}

// After metadata is cached, fetch first 100 rows for all tables and store in row_cache
#[allow(dead_code)]
async fn prefetch_first_rows_for_all_tables(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    pool: &models::enums::DatabasePool,
    cache_pool: &SqlitePool,
) -> bool {
    use sqlx::Row;
    let tables_res = sqlx::query_as::<_, (String, String)>(
        "SELECT database_name, table_name FROM table_cache WHERE connection_id = ? AND table_type = 'table' ORDER BY database_name, table_name",
    )
    .bind(connection_id)
    .fetch_all(cache_pool)
    .await;

    let rows = match tables_res {
        Ok(v) => v,
        Err(_) => return false,
    };

    match pool {
        models::enums::DatabasePool::MySQL(_mysql_pool) => {
            let enc_user = modules::url_encode(&connection.username);
            let enc_pass = modules::url_encode(&connection.password);
            futures_util::stream::iter(rows)
                .map(|(dbn, tbn)| {
                    let host = connection.host.clone();
                    let port = connection.port.clone();
                    let enc_user = enc_user.clone();
                    let enc_pass = enc_pass.clone();
                    async move {
                        let dsn = format!(
                            "mysql://{}:{}@{}:{}/{}",
                            enc_user, enc_pass, host, port, dbn
                        );
                        if let Ok(mut conn) = sqlx::mysql::MySqlConnection::connect(&dsn).await {
                            let q = format!("SELECT * FROM `{}` LIMIT 100", tbn.replace('`', "``"));
                            if let Ok(mysql_rows) = sqlx::query(&q).fetch_all(&mut conn).await {
                                let headers: Vec<String> = if let Some(r0) = mysql_rows.first() {
                                    r0.columns().iter().map(|c| c.name().to_string()).collect()
                                } else {
                                    let dq = format!("DESCRIBE `{}`", tbn.replace('`', "``"));
                                    match sqlx::query(&dq).fetch_all(&mut conn).await {
                                        Ok(desc_rows) => desc_rows
                                            .iter()
                                            .filter_map(|r| r.try_get::<String, _>(0).ok())
                                            .collect(),
                                        Err(_) => Vec::new(),
                                    }
                                };
                                let data =
                                    crate::driver_mysql::convert_mysql_rows_to_table_data(mysql_rows);
                                save_row_cache_direct(
                                    cache_pool,
                                    connection_id,
                                    &dbn,
                                    &tbn,
                                    &headers,
                                    &data,
                                )
                                .await;
                            }
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        models::enums::DatabasePool::PostgreSQL(pg_pool) => {
            futures_util::stream::iter(rows)
                .map(|(dbn, tbn)| {
                    let pool = pg_pool.clone();
                    async move {
                        let q = format!(
                            "SELECT * FROM \"public\".\"{}\" LIMIT 100",
                            tbn.replace('"', "\\\"")
                        );
                        if let Ok(pg_rows) = sqlx::query(&q).fetch_all(pool.as_ref()).await {
                            let headers: Vec<String> = if let Some(r0) = pg_rows.first() {
                                r0.columns().iter().map(|c| c.name().to_string()).collect()
                            } else {
                                let iq = format!(
                                    "SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name='{}' ORDER BY ordinal_position",
                                    tbn.replace("'", "''")
                                );
                                match sqlx::query(&iq).fetch_all(pool.as_ref()).await {
                                    Ok(infos) => infos
                                        .iter()
                                        .filter_map(|r| r.try_get::<String, _>(0).ok())
                                        .collect(),
                                    Err(_) => Vec::new(),
                                }
                            };
                            let data: Vec<Vec<String>> = pg_rows
                                .iter()
                                .map(|row| {
                                    (0..row.len())
                                        .map(|j| match row.try_get::<Option<String>, _>(j) {
                                            Ok(Some(v)) => v,
                                            Ok(None) => "NULL".to_string(),
                                            Err(_) => {
                                                if let Ok(Some(bytes)) =
                                                    row.try_get::<Option<Vec<u8>>, _>(j)
                                                {
                                                    String::from_utf8_lossy(&bytes).to_string()
                                                } else {
                                                    "".to_string()
                                                }
                                            }
                                        })
                                        .collect()
                                })
                                .collect();
                            save_row_cache_direct(
                                cache_pool, connection_id, &dbn, &tbn, &headers, &data,
                            )
                            .await;
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        models::enums::DatabasePool::SQLite(sqlite_pool) => {
            futures_util::stream::iter(rows)
                .map(|(_dbn, tbn)| {
                    let pool = sqlite_pool.clone();
                    async move {
                        let q =
                            format!("SELECT * FROM `{}` LIMIT 100", tbn.replace('`', "``"));
                        if let Ok(sqlite_rows) = sqlx::query(&q).fetch_all(pool.as_ref()).await {
                            let headers: Vec<String> = if let Some(r0) = sqlite_rows.first() {
                                r0.columns().iter().map(|c| c.name().to_string()).collect()
                            } else {
                                let iq = format!(
                                    "PRAGMA table_info(\"{}\")",
                                    tbn.replace('"', "\\\"")
                                );
                                match sqlx::query(&iq).fetch_all(pool.as_ref()).await {
                                    Ok(infos) => infos
                                        .iter()
                                        .filter_map(|r| r.try_get::<String, _>(1).ok())
                                        .collect(),
                                    Err(_) => Vec::new(),
                                }
                            };
                            let data =
                                crate::driver_sqlite::convert_sqlite_rows_to_table_data(sqlite_rows);
                            save_row_cache_direct(
                                cache_pool,
                                connection_id,
                                "main",
                                &tbn,
                                &headers,
                                &data,
                            )
                            .await;
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        _ => false,
    }
}
