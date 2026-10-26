use crate::{driver_mssql, driver_mysql, driver_postgres, driver_redis, driver_sqlite, models};
use sqlx::SqlitePool;

// Fetch and cache metadata for all databases/tables/columns per connection
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
        models::enums::DatabaseType::Plugin(_) => {
            if let models::enums::DatabasePool::Plugin(plugin_pool) = pool {
                crate::driver_api::cache::fetch_plugin_data(
                    connection_id,
                    plugin_pool,
                    &connection.database,
                    cache_pool,
                )
                .await
            } else {
                false
            }
        }
    }
}
