use std::sync::Arc;

use futures_util::TryStreamExt;
use log::debug;
use mongodb::{
    Client,
    bson::{Bson, doc},
};

use crate::{connection, models, window_egui::Tabular};

// fn bson_type_name(v: &Bson) -> String {
//     match v {
//         Bson::Double(_) => "double",
//         Bson::String(_) => "string",
//         Bson::Array(_) => "array",
//         Bson::Document(_) => "document",
//         Bson::Boolean(_) => "bool",
//         Bson::Int32(_) => "int32",
//         Bson::Int64(_) => "int64",
//         Bson::Decimal128(_) => "decimal128",
//         Bson::ObjectId(_) => "objectId",
//         Bson::DateTime(_) => "date",
//         Bson::Null => "null",
//         _ => "any",
//     }
//     .to_string()
// }

// Cache full structure for a MongoDB connection: databases and their collections
pub async fn fetch_mongodb_data(
    connection_id: i64,
    client: Arc<Client>,
    cache_pool: &sqlx::SqlitePool,
) -> bool {
    // List databases
    let dbs = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.list_database_names(),
    )
    .await
    .map_err(|_| mongodb::error::Error::from(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")))
    .and_then(|r| r)
    {
        Ok(v) => v,
        Err(e) => {
            debug!("Failed to list MongoDB databases: {}", e);
            return false;
        }
    };

    // Save databases directly into cache table
    let mut ok = true;
    if let Err(e) = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
        .bind(connection_id)
        .execute(cache_pool)
        .await
    {
        debug!("Failed clearing database_cache: {}", e);
    }
    for db in &dbs {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
        )
        .bind(connection_id)
        .bind(db)
        .execute(cache_pool)
        .await;
    }

    // For each database, list collections and cache as table_cache with type 'collection'
    for db_name in &dbs {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.database(db_name).list_collection_names(),
        )
        .await
        .map_err(|_| mongodb::error::Error::from(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")))
        .and_then(|r| r)
        {
            Ok(cols) => {
                // Save collections as table_cache entries
                let _ = sqlx::query(
                    "DELETE FROM table_cache WHERE connection_id = ? AND database_name = ?",
                )
                .bind(connection_id)
                .bind(db_name)
                .execute(cache_pool)
                .await;
                for c in cols {
                    let _ = sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, 'collection')")
                        .bind(connection_id)
                        .bind(db_name)
                        .bind(c)
                        .execute(cache_pool)
                        .await;
                }
            }
            Err(e) => {
                debug!("Failed to list collections for '{}': {}", db_name, e);
                ok = false;
            }
        }
    }

    ok
}

// Build initial tree structure for MongoDB connection
pub fn load_mongodb_structure(
    _connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Simple placeholder; actual databases will be loaded lazily from cache/server
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(_connection_id);
    databases_folder.is_loaded = false; // Will be loaded when expanded
    node.children = vec![databases_folder];
}

pub fn fetch_collections_from_mongodb_connection(
    tabular: &mut Tabular,
    connection_id: i64,
    database_name: &str,
) -> Option<Vec<String>> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        if let Some(models::enums::DatabasePool::MongoDB(client)) =
            connection::get_or_create_connection_pool(tabular, connection_id).await
        {
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.database(database_name).list_collection_names(),
            )
            .await
            .map_err(|_| mongodb::error::Error::from(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")))
            .and_then(|r| r)
            {
                Ok(cols) => Some(cols),
                Err(e) => {
                    debug!("MongoDB list_collection_names error: {}", e);
                    None
                }
            }
        } else {
            None
        }
    })
}

// Sample documents from a collection and present as headers + rows (flatten to JSON column for simplicity)
pub fn sample_collection_documents(
    tabular: &mut Tabular,
    connection_id: i64,
    database_name: &str,
    collection_name: &str,
    limit: i64,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        if let Some(models::enums::DatabasePool::MongoDB(client)) =
            connection::get_or_create_connection_pool(tabular, connection_id).await
        {
            let coll = client
                .database(database_name)
                .collection::<mongodb::bson::Document>(collection_name);
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                coll.find(doc! {}).limit(limit),
            )
            .await
            .map_err(|_| mongodb::error::Error::from(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")))
            .and_then(|r| r)
            {
                Ok(mut cursor) => {
                    let mut rows = Vec::new();
                    while let Some(item) = cursor.try_next().await.unwrap_or(None) {
                        let json = match mongodb::bson::to_bson(&item) {
                            Ok(Bson::Document(d)) => {
                                serde_json::to_string(&d).unwrap_or_else(|_| "{}".to_string())
                            }
                            Ok(other) => other.to_string(),
                            Err(_) => "{}".to_string(),
                        };
                        rows.push(vec![json]);
                    }
                    Some((vec!["_json".to_string()], rows))
                }
                Err(e) => Some((
                    vec!["Error".to_string()],
                    vec![vec![format!("MongoDB find error: {}", e)]],
                )),
            }
        } else {
            None
        }
    })
}
