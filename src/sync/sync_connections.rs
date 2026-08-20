//! Sync Connections — upload/download encrypted connection configs.
//!
//! Security: connection credentials are encrypted with AES-256-GCM by
//! `sync::vault_crypto` BEFORE being sent to the server, using either the
//! user's own AccountKey (personal folders) or the owning Team's key
//! (Team-shared folders) — resolved by `sync::vault_sync::resolve_key_for_folder`.
//! The server only ever stores ciphertext it cannot decrypt.
//!
//! Decryption of pulled rows happens in `window_egui::sync_tick::merge_remote_connections`
//! (it needs `Tabular`'s local connection list to merge into), not here.

use log::{info, warn};
use std::sync::mpsc;

use crate::models::structs::ConnectionConfig;

use super::api_client::{ApiClient, CreateConnectionReq, RemoteConnection};
use super::vault_crypto::{self, SymKey};

/// Push a single connection to the server (called after local save).
/// `key` must already be resolved for this connection's folder via
/// `vault_sync::resolve_key_for_folder` — callers with no key yet (Team not
/// unlocked) should not call this until one is available.
pub fn push_connection_to_server(
    conn: ConnectionConfig,
    key: SymKey,
    folder_path: String,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<String, String>>, // returns server connection ID
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);

        let encrypted = match vault_crypto::encrypt_json(&key, &conn) {
            Ok(e) => e,
            Err(e) => {
                let _ = result_tx.send(Err(format!("Encryption error: {}", e)));
                return;
            }
        };

        let req = CreateConnectionReq {
            name: conn.name.clone(),
            db_type: format!("{:?}", conn.connection_type),
            encrypted_config: encrypted,
            color_tag: None,
            folder_path: Some(folder_path),
            crypto_version: 1,
        };

        match client.create_connection(&token, &req).await {
            Ok(remote) => {
                info!("✅ [sync_connections] Pushed connection '{}' → server id {}", conn.name, remote.id);
                let _ = result_tx.send(Ok(remote.id));
            }
            Err(e) => {
                warn!("❌ [sync_connections] Push failed for '{}': {}", conn.name, e);
                let _ = result_tx.send(Err(e.to_string()));
            }
        }
    });
}

/// Re-encrypt every given local connection with `key` and upsert it to the
/// server (update if a same-named row already exists in `folder_path`,
/// otherwise create it). Used right after a folder becomes newly Team-shared,
/// to move items that were already synced under the personal AccountKey onto
/// the Team key — without this, they'd stay owner-only-readable forever even
/// though the folder now says "shared". Fire-and-forget; failures are logged,
/// not surfaced to the UI (the regular sync cadence will retry on its own).
pub fn reencrypt_folder_to_server(
    connections: Vec<ConnectionConfig>,
    key: SymKey,
    folder_path: String,
    token: String,
    server_url: String,
) {
    if connections.is_empty() {
        return;
    }
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let remote = match client.list_connections(&token).await {
            Ok(r) => r,
            Err(e) => {
                warn!("❌ [sync_connections] re-encrypt: failed to list remote connections: {}", e);
                return;
            }
        };

        let mut migrated = 0usize;
        for conn in connections {
            let encrypted = match vault_crypto::encrypt_json(&key, &conn) {
                Ok(e) => e,
                Err(e) => {
                    warn!("❌ [sync_connections] re-encrypt: failed to encrypt '{}': {}", conn.name, e);
                    continue;
                }
            };

            let existing = remote.iter().find(|r| r.name == conn.name && r.folder_path == folder_path);
            let result = match existing {
                Some(r) => {
                    let body = serde_json::json!({ "encrypted_config": encrypted, "crypto_version": 1 });
                    client.update_connection(&token, &r.id, &body).await.map(|_| ())
                }
                None => {
                    let req = CreateConnectionReq {
                        name: conn.name.clone(),
                        db_type: format!("{:?}", conn.connection_type),
                        encrypted_config: encrypted,
                        color_tag: None,
                        folder_path: Some(folder_path.clone()),
                        crypto_version: 1,
                    };
                    client.create_connection(&token, &req).await.map(|_| ())
                }
            };
            match result {
                Ok(()) => migrated += 1,
                Err(e) => warn!("❌ [sync_connections] re-encrypt: failed to upsert '{}': {}", conn.name, e),
            }
        }
        info!("✅ [sync_connections] Re-encrypted {} connection(s) in '{}' under the Team key", migrated, folder_path);
    });
}

/// Re-encrypt a single legacy (`crypto_version = 0`) connection — already
/// decrypted by the caller via `legacy_crypto::legacy_decrypt_best_effort` —
/// under the resolved vault key, and persist it server-side as
/// `crypto_version = 1`. Fire-and-forget: on failure the row simply stays
/// `crypto_version = 0` and migration is retried on the next pull.
pub fn migrate_legacy_connection(
    remote_id: String,
    conn: ConnectionConfig,
    key: SymKey,
    token: String,
    server_url: String,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let encrypted = match vault_crypto::encrypt_json(&key, &conn) {
            Ok(e) => e,
            Err(e) => {
                warn!("❌ [migrate] Failed to encrypt legacy connection '{}': {}", conn.name, e);
                return;
            }
        };
        let body = serde_json::json!({ "encrypted_config": encrypted, "crypto_version": 1 });
        match client.update_connection(&token, &remote_id, &body).await {
            Ok(_) => info!("✅ [migrate] Migrated legacy connection '{}' to end-to-end encryption", conn.name),
            Err(e) => warn!("❌ [migrate] Failed to migrate connection '{}': {}", conn.name, e),
        }
    });
}

/// Pull all connections from server (still encrypted — decryption + merge
/// into the local list happens in `window_egui::sync_tick`, which has
/// access to the unlocked vault and Team keys).
pub fn pull_connections_from_server(
    _user_id: String,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<Vec<RemoteConnection>, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);

        match client.list_connections(&token).await {
            Ok(remote_conns) => {
                info!("✅ [sync_connections] Pulled {} connections from server", remote_conns.len());
                let _ = result_tx.send(Ok(remote_conns));
            }
            Err(e) => {
                warn!("❌ [sync_connections] Pull failed: {}", e);
                let _ = result_tx.send(Err(e.to_string()));
            }
        }
    });
}
