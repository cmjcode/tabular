//! Sync Saved Queries — sync .sql files with the server.
//!
//! Offline-first: local filesystem is the source of truth.
//! Checksum (SHA-256) detects conflicts; last-write-wins by default.
//!
//! Security: `query_text` is encrypted with AES-256-GCM by `sync::vault_crypto`
//! BEFORE being sent to the server, using either the user's own AccountKey
//! (personal folders) or the owning Team's key (Team-shared folders) — resolved
//! by `sync::vault_sync::resolve_key_for_folder`. The server only ever stores
//! ciphertext it cannot decrypt. `name` / `folder_path` / `connection_name`
//! stay plaintext server-side (needed for listing and Team-share folder joins).
//! `client_checksum` is always the SHA/MD5 of the *plaintext* content — it has
//! to be, since AES-GCM's random nonce means the same plaintext encrypted
//! twice never produces the same ciphertext, which would break dedup.

use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;

use super::api_client::{
    ApiClient, CreateQueryReq, RemoteSavedQuery, RemoteSharedFolder, UpdateQueryReq,
};
use super::vault_crypto::{self, SymKey};
use super::vault_sync;
use crate::directory;

/// Compute SHA-256 checksum of a string (for conflict detection)
pub fn checksum(content: &str) -> String {
    let digest = md5::compute(content.as_bytes());
    format!("{:x}", digest)
}

/// Push all local .sql files to the server, encrypted with the AccountKey
/// (personal folders) or the owning Team's key (Team-shared folders — resolved
/// per file via `vault_sync::resolve_key_for_folder`). Files whose folder is
/// Team-shared but whose Team key isn't unlocked yet are skipped (retried on a
/// later sync tick). Files that already exist on server (same checksum) are
/// skipped too.
pub fn push_queries_to_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let query_dir = directory::get_query_dir();

        // Collect all .sql files recursively
        let files = collect_sql_files(&query_dir);
        if files.is_empty() {
            let _ = result_tx.send(Ok(0));
            return;
        }

        // Get remote query list for dedup
        let remote_queries = match client.list_queries(&token).await {
            Ok(q) => q,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let mut pushed = 0usize;
        for (file_path, folder_path, name) in files {
            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let cs = checksum(&content);

            // Skip if server already has same checksum
            let already_synced = remote_queries.iter().any(|q| {
                q.name == name
                    && q.folder_path == folder_path
                    && q.client_checksum.as_deref() == Some(&cs)
            });
            if already_synced {
                continue;
            }

            // Versi lama menyinkronkan semua file sebagai folder `/`. Baris
            // lama dengan isi sama dipindah ke folder yang benar, bukan
            // diduplikasi.
            if folder_path != "/"
                && let Some(legacy) = remote_queries.iter().find(|q| {
                    q.name == name
                        && q.folder_path == "/"
                        && q.client_checksum.as_deref() == Some(&cs)
                        && q.access.as_deref().is_none_or(|a| a == "owner")
                })
            {
                let update = UpdateQueryReq {
                    folder_path: Some(folder_path.clone()),
                    ..Default::default()
                };
                match client.update_saved_query(&token, &legacy.id, &update).await {
                    Ok(_) => pushed += 1,
                    Err(e) => warn!("❌ [sync_queries] Failed to move '{}': {}", name, e),
                }
                continue;
            }

            let key = match vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "query",
                &folder_path,
            ) {
                Some(k) => k,
                None => continue, // Team key not unlocked yet — retried next tick
            };

            let encrypted = match vault_crypto::encrypt_str(key, &content) {
                Ok(e) => e,
                Err(e) => {
                    warn!("❌ [sync_queries] Failed to encrypt '{}': {}", name, e);
                    continue;
                }
            };

            let req = CreateQueryReq {
                name: name.clone(),
                folder_path: Some(folder_path),
                query_text: encrypted,
                connection_name: None,
                client_checksum: Some(cs),
                crypto_version: 1,
            };

            match client.create_saved_query(&token, &req).await {
                Ok(_) => pushed += 1,
                Err(e) => warn!("❌ [sync_queries] Failed to push '{}': {}", name, e),
            }
        }

        info!(
            "✅ [sync_queries] Pushed {} new/updated queries to server",
            pushed
        );
        let _ = result_tx.send(Ok(pushed));
    });
}

/// Re-encrypt a single legacy (`crypto_version = 0`) query — already plaintext
/// as pulled from the server — under the resolved vault key, and persist it
/// server-side as `crypto_version = 1` (in place, via PUT — not a new row).
/// Fire-and-forget: on failure the row simply stays `crypto_version = 0` and
/// migration is retried on the next pull.
fn migrate_legacy_query(remote: RemoteSavedQuery, key: SymKey, token: String, server_url: String) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let encrypted = match vault_crypto::encrypt_str(&key, &remote.query_text) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    "❌ [migrate] Failed to encrypt legacy query '{}': {}",
                    remote.name, e
                );
                return;
            }
        };
        let update = UpdateQueryReq {
            query_text: Some(encrypted),
            crypto_version: Some(1),
            ..Default::default()
        };
        match client.update_saved_query(&token, &remote.id, &update).await {
            Ok(_) => info!(
                "✅ [migrate] Migrated legacy query '{}' to end-to-end encryption",
                remote.name
            ),
            Err(e) => warn!(
                "❌ [migrate] Failed to migrate query '{}': {}",
                remote.name, e
            ),
        }
    });
}

/// Re-encrypt every local .sql file under `folder_path` with `key` and upsert
/// it to the server. Used right after a folder becomes newly Team-shared, so
/// items already synced under the personal AccountKey move onto the Team key
/// instead of staying owner-only-readable. Fire-and-forget; failures are
/// logged, not surfaced to the UI (the regular sync cadence will retry).
pub fn reencrypt_folder_to_server(
    key: SymKey,
    folder_path: String,
    token: String,
    server_url: String,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let query_dir = directory::get_query_dir();
        let files: Vec<(String, String, String)> = collect_sql_files(&query_dir)
            .into_iter()
            .filter(|(_, folder, _)| vault_sync::folder_covers(&folder_path, folder))
            .collect();
        if files.is_empty() {
            return;
        }

        let remote_queries = match client.list_queries(&token).await {
            Ok(q) => q,
            Err(e) => {
                warn!(
                    "❌ [sync_queries] re-encrypt: failed to list remote queries: {}",
                    e
                );
                return;
            }
        };

        let mut migrated = 0usize;
        for (file_path, folder, name) in files {
            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let cs = checksum(&content);
            let encrypted = match vault_crypto::encrypt_str(&key, &content) {
                Ok(e) => e,
                Err(e) => {
                    warn!(
                        "❌ [sync_queries] re-encrypt: failed to encrypt '{}': {}",
                        name, e
                    );
                    continue;
                }
            };

            let existing = remote_queries
                .iter()
                .find(|q| q.name == name && q.folder_path == folder);
            let result = match existing {
                Some(r) => {
                    let update = UpdateQueryReq {
                        query_text: Some(encrypted),
                        client_checksum: Some(cs),
                        crypto_version: Some(1),
                        ..Default::default()
                    };
                    client
                        .update_saved_query(&token, &r.id, &update)
                        .await
                        .map(|_| ())
                }
                None => {
                    let req = CreateQueryReq {
                        name: name.clone(),
                        folder_path: Some(folder.clone()),
                        query_text: encrypted,
                        connection_name: None,
                        client_checksum: Some(cs),
                        crypto_version: 1,
                    };
                    client.create_saved_query(&token, &req).await.map(|_| ())
                }
            };
            match result {
                Ok(()) => migrated += 1,
                Err(e) => warn!(
                    "❌ [sync_queries] re-encrypt: failed to upsert '{}': {}",
                    name, e
                ),
            }
        }
        info!(
            "✅ [sync_queries] Re-encrypted {} quer(y/ies) in '{}' under the Team key",
            migrated, folder_path
        );
    });
}

/// Pull remote queries and save missing ones locally. Queries that can't be
/// decrypted yet (a Team-shared item whose Team key isn't unlocked) are
/// skipped; legacy plaintext rows (`crypto_version = 0`) are used as-is and
/// queued for migration to E2E encryption.
pub fn pull_queries_from_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let query_dir = directory::get_query_dir();

        let remote_queries = match client.list_queries(&token).await {
            Ok(q) => q,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let mut saved = 0usize;
        for rq in &remote_queries {
            let key = match vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "query",
                &rq.folder_path,
            ) {
                Some(k) => k,
                None => {
                    info!(
                        "[sync_queries] Skipping Team-shared '{}': Team key not unlocked yet",
                        rq.name
                    );
                    continue;
                }
            };

            let plaintext = if rq.crypto_version >= 1 {
                match vault_crypto::decrypt_str(key, &rq.query_text) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("❌ [sync_queries] Failed to decrypt '{}': {}", rq.name, e);
                        continue;
                    }
                }
            } else {
                // Legacy (pre-vault) row — was never encrypted, just plain SQL
                // text. Use it as-is, then queue a re-upload under the real
                // vault key so it migrates for good.
                migrate_legacy_query(rq.clone(), key.clone(), token.clone(), server_url.clone());
                rq.query_text.clone()
            };

            // Determine local file path
            let folder = if rq.folder_path == "/" {
                query_dir.clone()
            } else {
                query_dir.join(rq.folder_path.trim_start_matches('/'))
            };

            let file_path = folder.join(format!("{}.sql", sanitize_filename(&rq.name)));

            if file_path.exists() {
                // Check if local is same as remote
                if let Ok(local) = std::fs::read_to_string(&file_path) {
                    if checksum(&local) == rq.client_checksum.as_deref().unwrap_or("") {
                        continue; // In sync
                    }
                    // Conflict: local differs — skip (local wins)
                    debug!(
                        "⚠️ [sync_queries] Conflict on '{}' — local version kept",
                        rq.name
                    );
                    continue;
                }
            }

            // File doesn't exist locally — download it
            if let Err(e) = std::fs::create_dir_all(&folder) {
                warn!("❌ [sync_queries] Cannot create folder {:?}: {}", folder, e);
                continue;
            }

            if let Err(e) = std::fs::write(&file_path, &plaintext) {
                warn!("❌ [sync_queries] Cannot write {:?}: {}", file_path, e);
                continue;
            }

            info!("✅ [sync_queries] Downloaded '{}'", rq.name);
            saved += 1;
        }

        let _ = result_tx.send(Ok(saved));
    });
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Recursively collect all .sql files as `(file_path, folder_path, name)`.
/// `folder_path` selalu relatif terhadap root direktori query (`/`, `/A`,
/// `/A/B`), sama seperti cara pull menaruh file.
fn collect_sql_files(dir: &Path) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    collect_sql_files_under(dir, dir, &mut results);
    results
}

fn collect_sql_files_under(root: &Path, dir: &Path, results: &mut Vec<(String, String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sql_files_under(root, &path, results);
        } else if path.extension().map(|e| e == "sql").unwrap_or(false) {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let folder = path
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .map(|s| format!("/{}", s.trim_start_matches('/')))
                .unwrap_or_else(|| "/".to_string());
            results.push((path.to_string_lossy().to_string(), folder, name));
        }
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_folder_paths_relative_to_root() {
        let root = std::env::temp_dir().join(format!(
            "tabular-sync-queries-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(root.join("Shop/reports")).unwrap();
        std::fs::write(root.join("top.sql"), "select 1").unwrap();
        std::fs::write(root.join("Shop/a.sql"), "select 2").unwrap();
        std::fs::write(root.join("Shop/reports/b.sql"), "select 3").unwrap();
        let mut got: Vec<(String, String)> = collect_sql_files(&root)
            .into_iter()
            .map(|(_, folder, name)| (folder, name))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("/".to_string(), "top".to_string()),
                ("/Shop".to_string(), "a".to_string()),
                ("/Shop/reports".to_string(), "b".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
