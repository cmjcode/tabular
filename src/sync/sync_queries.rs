//! Sync Saved Queries — sync .sql files with the server.
//!
//! Offline-first: local filesystem is the source of truth.
//! Checksum (SHA-256) detects conflicts; last-write-wins by default.

use log::{debug, info, warn};
use std::sync::mpsc;
use std::path::Path;

use crate::directory;
use super::api_client::{ApiClient, CreateQueryReq};

/// Compute SHA-256 checksum of a string (for conflict detection)
pub fn checksum(content: &str) -> String {
    
    let digest = md5::compute(content.as_bytes());
    format!("{:x}", digest)
}

/// Push all local .sql files to the server.
/// Files that already exist on server (same checksum) are skipped.
pub fn push_queries_to_server(
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

            let req = CreateQueryReq {
                name: name.clone(),
                folder_path: Some(folder_path),
                query_text: content,
                connection_name: None,
                client_checksum: Some(cs),
            };

            match client.create_saved_query(&token, &req).await {
                Ok(_) => pushed += 1,
                Err(e) => warn!("❌ [sync_queries] Failed to push '{}': {}", name, e),
            }
        }

        info!("✅ [sync_queries] Pushed {} new/updated queries to server", pushed);
        let _ = result_tx.send(Ok(pushed));
    });
}

/// Pull remote queries and save missing ones locally.
pub fn pull_queries_from_server(
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
                    debug!("⚠️ [sync_queries] Conflict on '{}' — local version kept", rq.name);
                    continue;
                }
            }

            // File doesn't exist locally — download it
            if let Err(e) = std::fs::create_dir_all(&folder) {
                warn!("❌ [sync_queries] Cannot create folder {:?}: {}", folder, e);
                continue;
            }

            if let Err(e) = std::fs::write(&file_path, &rq.query_text) {
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

/// Recursively collect all .sql files.
/// Returns (absolute_path, folder_path, name_without_ext)
fn collect_sql_files(dir: &Path) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let subdir_results = collect_sql_files(&path);
                results.extend(subdir_results);
            } else if path.extension().map(|e| e == "sql").unwrap_or(false) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let folder = path
                    .parent()
                    .and_then(|p| p.strip_prefix(dir).ok())
                    .and_then(|p| p.to_str())
                    .map(|s| format!("/{}", s))
                    .unwrap_or_else(|| "/".to_string());
                results.push((path.to_string_lossy().to_string(), folder, name));
            }
        }
    }
    results
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect()
}
