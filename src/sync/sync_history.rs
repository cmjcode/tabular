//! Sync History — push local history to server, pull remote history.
//!
//! Offline-first: local SQLite is always the source of truth.
//! Server sync happens in background when online.
//!
//! Security: `query_text` is encrypted with AES-256-GCM by `sync::vault_crypto`
//! using the user's AccountKey BEFORE being sent to the server — history has no
//! folder concept (unlike connections/queries/HTTP requests), so there's no
//! Team-key case to resolve here, only the personal AccountKey. The server
//! only ever stores ciphertext it cannot decrypt. `client_checksum` (SHA/MD5 of
//! the *plaintext*) is what dedup runs on, since AES-GCM's random nonce means
//! the same plaintext encrypted twice never produces the same ciphertext.

use log::{info, warn};
use std::sync::mpsc;

use crate::models::structs::HistoryItem;

use super::api_client::{ApiClient, HistoryPushItem};
use super::vault_crypto::{self, SymKey};

/// Push all local history items to the server (incremental by last_sync_ts),
/// encrypted with the AccountKey. Runs in background — spawned via tokio::spawn.
pub fn push_history_to_server(
    items: Vec<HistoryItem>,
    account_key: SymKey,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<u64, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);

        let mut push_items = Vec::with_capacity(items.len());
        for h in items {
            let cs = super::sync_queries::checksum(&h.query);
            let encrypted = match vault_crypto::encrypt_str(&account_key, &h.query) {
                Ok(e) => e,
                Err(e) => {
                    warn!("❌ [sync_history] Failed to encrypt history item: {}", e);
                    continue;
                }
            };
            push_items.push(HistoryPushItem {
                connection_name: h.connection_name,
                query_text: encrypted,
                executed_at: h.executed_at,
                client_checksum: Some(cs),
                crypto_version: 1,
            });
        }

        if push_items.is_empty() {
            let _ = result_tx.send(Ok(0));
            return;
        }

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

/// Pull remote history and merge into local SQLite (deduplication by
/// decrypted query text + connection_name + executed_at). Items that can't be
/// decrypted (wrong/rotated key) are skipped and logged, never inserted as
/// ciphertext.
pub fn pull_history_from_server(
    account_key: SymKey,
    token: String,
    server_url: String,
    db_pool: std::sync::Arc<sqlx::SqlitePool>,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
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
            let plaintext = if item.crypto_version >= 1 {
                match vault_crypto::decrypt_str(&account_key, &item.query_text) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("❌ [sync_history] Failed to decrypt history item: {}", e);
                        continue;
                    }
                }
            } else {
                // Legacy (pre-vault) row — was never encrypted, just plain text.
                // There's no update endpoint for history items, so unlike
                // connections/queries/HTTP requests these aren't migrated in
                // place; they're simply readable as-is until they age out.
                item.query_text.clone()
            };

            // Check if already exists locally (compare against decrypted text —
            // the server-side value is ciphertext and never matches directly).
            let exists: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM query_history
                 WHERE query_text = ? AND connection_name = ? AND executed_at = ?"
            )
            .bind(&plaintext)
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
                .bind(&plaintext)
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
