//! Sync Diagrams — sync Multi-Database / ERD diagram state with Tabular Server.
//!
//! Offline-first: local filesystem (~/.tabular/diagrams/*.json) is the source of truth.
//! Checksum (SHA-256) detects conflicts; last-write-wins by default.
//!
//! Security: `DiagramState` JSON is encrypted with AES-256-GCM by `sync::vault_crypto`
//! BEFORE being sent to the server, using either the user's own AccountKey (personal)
//! or the owning Team's key (Team-shared folders). The server only stores ciphertext.

use log::{info, warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::api_client::{
    ApiClient, CreateDiagramReq, RemoteSharedFolder, UpdateDiagramReq,
};
use super::vault_crypto::{self, SymKey};
use super::vault_sync;
use crate::models::structs::DiagramState;

/// Hitung checksum MD5 dari string konten JSON (untuk deteksi konflik & dedup).
pub fn checksum(content: &str) -> String {
    let digest = md5::compute(content.as_bytes());
    format!("{:x}", digest)
}

/// Dapatkan direktori penyimpanan diagram lokal (~/.tabular/diagrams/).
pub fn get_diagrams_dir() -> PathBuf {
    if let Some(config_dir) = dirs::data_local_dir() {
        let p = config_dir.join("tabular").join("diagrams");
        let _ = std::fs::create_dir_all(&p);
        p
    } else {
        PathBuf::from("diagrams")
    }
}

/// Kumpulkan semua file diagram JSON (.json) dari folder diagram lokal.
pub fn collect_diagram_files(dir: &Path) -> Vec<(PathBuf, String, String)> {
    let mut results = Vec::new();
    if !dir.exists() {
        return results;
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
                let file_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("diagram")
                    .to_string();
                // Folder path default root "/"
                results.push((path, "/".to_string(), file_name));
            }
        }
    }
    results
}

/// Push semua diagram lokal ke server dengan enkripsi AES-256-GCM.
pub fn push_diagrams_to_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let diagrams_dir = get_diagrams_dir();

        let files = collect_diagram_files(&diagrams_dir);
        if files.is_empty() {
            let _ = result_tx.send(Ok(0));
            return;
        }

        let remote_diagrams = match client.list_diagrams(&token).await {
            Ok(d) => d,
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

            // Lewati jika server sudah memiliki checksum yang sama persis
            let already_synced = remote_diagrams.iter().any(|d| {
                d.name == name
                    && d.folder_path == folder_path
                    && d.client_checksum.as_deref() == Some(&cs)
            });
            if already_synced {
                continue;
            }

            let key = match vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "diagram",
                &folder_path,
            ) {
                Some(k) => k,
                None => continue,
            };

            let encrypted = match vault_crypto::encrypt_str(key, &content) {
                Ok(e) => e,
                Err(e) => {
                    warn!("❌ [sync_diagrams] Gagal mengenkripsi diagram '{}': {}", name, e);
                    continue;
                }
            };

            let existing_remote = remote_diagrams
                .iter()
                .find(|d| d.name == name && d.folder_path == folder_path);

            if let Some(existing) = existing_remote {
                let req = UpdateDiagramReq {
                    name: Some(name.clone()),
                    folder_path: Some(folder_path),
                    encrypted_data: Some(encrypted),
                    client_checksum: Some(cs),
                    crypto_version: Some(1),
                };
                if let Err(e) = client.update_diagram(&token, &existing.id, &req).await {
                    warn!("❌ [sync_diagrams] Gagal update diagram '{}': {}", name, e);
                    continue;
                }
            } else {
                let req = CreateDiagramReq {
                    name: name.clone(),
                    folder_path: Some(folder_path),
                    encrypted_data: encrypted,
                    client_checksum: Some(cs),
                    crypto_version: 1,
                };
                if let Err(e) = client.create_diagram(&token, &req).await {
                    warn!("❌ [sync_diagrams] Gagal membuat diagram '{}': {}", name, e);
                    continue;
                }
            }

            pushed += 1;
        }

        info!("✅ [sync_diagrams] Berhasil push {} diagram ke server", pushed);
        let _ = result_tx.send(Ok(pushed));
    });
}

/// Push satu diagram aktif ke server secara instan.
pub fn push_single_diagram(
    diagram_name: String,
    state: DiagramState,
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<String, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let content = match serde_json::to_string(&state) {
            Ok(c) => c,
            Err(e) => {
                let _ = result_tx.send(Err(format!("Serialisasi gagal: {e}")));
                return;
            }
        };

        let cs = checksum(&content);
        let folder_path = "/".to_string();

        let key = match vault_sync::resolve_key_for_folder(
            &account_key,
            &team_keys,
            &shared_folders,
            "diagram",
            &folder_path,
        ) {
            Some(k) => k,
            None => {
                let _ = result_tx.send(Err("Vault key tidak ditemukan".to_string()));
                return;
            }
        };

        let encrypted = match vault_crypto::encrypt_str(key, &content) {
            Ok(e) => e,
            Err(e) => {
                let _ = result_tx.send(Err(format!("Enkripsi gagal: {e}")));
                return;
            }
        };

        let remote_diagrams = match client.list_diagrams(&token).await {
            Ok(d) => d,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let existing = remote_diagrams
            .into_iter()
            .find(|d| d.name == diagram_name && d.folder_path == folder_path);

        if let Some(d) = existing {
            let req = UpdateDiagramReq {
                name: Some(diagram_name.clone()),
                folder_path: Some(folder_path),
                encrypted_data: Some(encrypted),
                client_checksum: Some(cs),
                crypto_version: Some(1),
            };
            match client.update_diagram(&token, &d.id, &req).await {
                Ok(_) => {
                    info!("✅ [sync_diagrams] Diagram '{}' diperbarui di server", diagram_name);
                    let _ = result_tx.send(Ok(d.id));
                }
                Err(e) => {
                    let _ = result_tx.send(Err(e.to_string()));
                }
            }
        } else {
            let req = CreateDiagramReq {
                name: diagram_name.clone(),
                folder_path: Some(folder_path),
                encrypted_data: encrypted,
                client_checksum: Some(cs),
                crypto_version: 1,
            };
            match client.create_diagram(&token, &req).await {
                Ok(res) => {
                    info!("✅ [sync_diagrams] Diagram '{}' dibuat di server (id: {})", diagram_name, res.id);
                    let _ = result_tx.send(Ok(res.id));
                }
                Err(e) => {
                    let _ = result_tx.send(Err(e.to_string()));
                }
            }
        }
    });
}

/// Pull diagram dari server dan dekripsi ke lokal bila ada yang baru/berubah.
pub fn pull_diagrams_from_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let diagrams_dir = get_diagrams_dir();

        let remote_diagrams = match client.list_diagrams(&token).await {
            Ok(d) => d,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let mut pulled = 0usize;
        for remote in remote_diagrams {
            let file_name = format!("{}.json", remote.name.replace('/', "_"));
            let local_path = diagrams_dir.join(&file_name);

            if local_path.exists() {
                if let Ok(local_content) = std::fs::read_to_string(&local_path) {
                    let local_cs = checksum(&local_content);
                    if remote.client_checksum.as_deref() == Some(&local_cs) {
                        continue; // Checksum sama, tidak perlu download ulang
                    }
                }
            }

            let key = match vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "diagram",
                &remote.folder_path,
            ) {
                Some(k) => k,
                None => continue,
            };

            let decrypted = match vault_crypto::decrypt_str(key, &remote.encrypted_data) {
                Ok(d) => d,
                Err(e) => {
                    warn!("❌ [sync_diagrams] Gagal mendekripsi diagram '{}': {}", remote.name, e);
                    continue;
                }
            };

            if let Err(e) = crate::diagram_view::write_atomic(&local_path, decrypted.as_bytes()) {
                warn!("❌ [sync_diagrams] Gagal menulis diagram lokal '{}': {}", local_path.display(), e);
                continue;
            }

            pulled += 1;
        }

        info!("✅ [sync_diagrams] Berhasil pull {} diagram dari server", pulled);
        let _ = result_tx.send(Ok(pulled));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramNode, DiagramState};

    #[test]
    fn test_checksum_computation() {
        let content_a = "{\"nodes\": []}";
        let content_b = "{\"nodes\": [{\"id\": \"t1\"}]}";
        let cs_a1 = checksum(content_a);
        let cs_a2 = checksum(content_a);
        let cs_b = checksum(content_b);

        assert_eq!(cs_a1, cs_a2);
        assert_ne!(cs_a1, cs_b);
        assert_eq!(cs_a1.len(), 32); // MD5 hex length
    }

    #[test]
    fn test_multi_database_diagram_encryption_roundtrip() {
        let key = SymKey::generate();

        let mut state = DiagramState {
            diagram_title: Some("multi_tenant_erd".to_string()),
            ..Default::default()
        };

        let node1 = DiagramNode {
            id: "users".to_string(),
            title: "users".to_string(),
            pos: eframe::egui::pos2(100.0, 100.0),
            size: eframe::egui::vec2(220.0, 160.0),
            columns: vec!["id".to_string(), "email".to_string()],
            foreign_keys: vec![],
            group_ids: vec!["group_db1".to_string()],
            group_id: Some("group_db1".to_string()),
            column_meta: vec![],
            detached: false,
            database_name: Some("auth_db".to_string()),
            connection_id: Some(1),
            connection_name: Some("Auth Service".to_string()),
        };

        let node2 = DiagramNode {
            id: "orders_db::users".to_string(),
            title: "users".to_string(),
            pos: eframe::egui::pos2(400.0, 100.0),
            size: eframe::egui::vec2(220.0, 160.0),
            columns: vec!["id".to_string(), "user_id".to_string()],
            foreign_keys: vec![],
            group_ids: vec!["group_db2".to_string()],
            group_id: Some("group_db2".to_string()),
            column_meta: vec![],
            detached: false,
            database_name: Some("orders_db".to_string()),
            connection_id: Some(2),
            connection_name: Some("Orders Service".to_string()),
        };

        state.nodes.push(node1);
        state.nodes.push(node2);

        let serialized = serde_json::to_string(&state).expect("Serialization failed");
        let encrypted = vault_crypto::encrypt_str(&key, &serialized).expect("Encryption failed");

        // Verifikasi bahwa ciphertext tidak membocorkan teks plaintext
        assert!(!encrypted.contains("auth_db"));
        assert!(!encrypted.contains("orders_db"));

        // Dekripsi
        let decrypted = vault_crypto::decrypt_str(&key, &encrypted).expect("Decryption failed");
        let restored: DiagramState = serde_json::from_str(&decrypted).expect("Deserialization failed");

        assert_eq!(restored.diagram_title.as_deref(), Some("multi_tenant_erd"));
        assert_eq!(restored.nodes.len(), 2);

        let restored_node1 = &restored.nodes[0];
        assert_eq!(restored_node1.database_name.as_deref(), Some("auth_db"));
        assert_eq!(restored_node1.connection_id, Some(1));
        assert_eq!(restored_node1.connection_name.as_deref(), Some("Auth Service"));

        let restored_node2 = &restored.nodes[1];
        assert_eq!(restored_node2.id, "orders_db::users");
        assert_eq!(restored_node2.database_name.as_deref(), Some("orders_db"));
        assert_eq!(restored_node2.connection_id, Some(2));
    }
}

