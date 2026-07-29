/// Sync History — push local history to server, pull remote history.
///
/// Offline-first: local SQLite is always the source of truth.
/// Server sync happens in background when online.

use log::{debug, info, warn};
use std::sync::mpsc;

use crate::models::structs::HistoryItem;
use crate::window_egui::Tabular;

use super::api_client::{ApiClient, HistoryPushItem};

/// Push all local history items to the server (incremental by last_sync_ts).
/// Runs in background — spawned via tokio::spawn.
pub fn push_history_to_server(
    items: Vec<HistoryItem>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<u64, String>>,
) {
    tokio::spawn(async move {
        let client = ApiClient::new(&server_url);

        let push_items: Vec<HistoryPushItem> = items
            .into_iter()
            .map(|h| HistoryPushItem {
                connection_name: h.connection_name,
                query_text: h.query,
                executed_at: h.executed_at,
            })
            .collect();

        match client.push_history(&token, push_items).await {
            Ok(inserted) => {
                info!("✅ [sync_history] Pushed {} new history items to server", inserted);
                let _ = result_tx.send(Ok(inserted));
            }
            Err(e) => {
                warn!("❌ [sync_history] Push failed: {}", e);
                let _ = result_tx.send(Err(e.to_string()));
            }
        }
    });
}

/// Pull remote history and merge into local SQLite (deduplication by query_text + executed_at).
/// Returns new items that were inserted locally.
pub fn pull_history_from_server(
    token: String,
    server_url: String,
    db_pool: std::sync::Arc<sqlx::SqlitePool>,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    tokio::spawn(async move {
        let client = ApiClient::new(&server_url);

        let remote_items = match client.fetch_history(&token, None).await {
            Ok(items) => items,
            Err(e) => {
                warn!("❌ [sync_history] Pull failed: {}", e);
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let mut inserted = 0usize;
        for item in &remote_items {
            // Check if already exists locally
            let exists: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM query_history
                 WHERE query_text = ? AND connection_name = ? AND executed_at = ?"
            )
            .bind(&item.query_text)
            .bind(&item.connection_name)
            .bind(&item.executed_at)
            .fetch_one(db_pool.as_ref())
            .await
            .unwrap_or(false);

            if !exists {
                let _ = sqlx::query(
                    "INSERT INTO query_history (query_text, connection_id, connection_name)
                     VALUES (?, 0, ?)"
                )
                .bind(&item.query_text)
                .bind(&item.connection_name)
                .execute(db_pool.as_ref())
                .await;
                inserted += 1;
            }
        }

        info!("✅ [sync_history] Pulled {} new remote history items", inserted);
        let _ = result_tx.send(Ok(inserted));
    });
}
