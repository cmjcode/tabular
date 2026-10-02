//! Sync Project — manifest project (environment tanpa nilai secret, peta
//! koneksi per environment, dan memory agent) ke server.
//!
//! Payload dienkripsi AES-256-GCM oleh `sync::vault_crypto` sebelum dikirim,
//! memakai AccountKey (project pribadi) atau Team key bila project dibagikan
//! (`resource_type = "project"`, `folder_path` = nama project). Server hanya
//! menyimpan ciphertext. Nilai variabel rahasia tidak pernah ikut: anggota
//! tim mengisinya sendiri di mesin masing-masing.
//!
//! Aturan merge sederhana: id sama → yang `updated_at`-nya lebih baru menang;
//! id baru → ditambahkan; nama bentrok dengan project lokal lain → dilewati.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use log::{info, warn};
use serde::{Deserialize, Serialize};

use super::api_client::{ApiClient, RemoteSharedFolder, UpdateProjectReq, UpsertProjectReq};
use super::vault_crypto::{self, SymKey};
use super::vault_sync;
use crate::project::{self, Project};
use crate::project_memory::{self, MemoryEntry};

/// Isi payload terenkripsi.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedProject {
    pub project: Project,
    #[serde(default)]
    pub memory: Vec<MemoryEntry>,
}

/// Project hasil pull yang sudah didekripsi, siap di-merge di thread UI.
#[derive(Clone, Debug)]
pub struct PulledProject {
    pub shared: SharedProject,
    pub remote_id: String,
    pub owner_id: String,
    pub access: String,
}

/// Payload bersama untuk satu project lokal.
pub fn build_shared(app_dir: &std::path::Path, project: &Project) -> SharedProject {
    SharedProject {
        project: project.shared_copy(),
        memory: project_memory::list(app_dir, &project.id),
    }
}

/// Checksum isi plaintext (tanpa timestamp memory yang berubah tiap tulis).
pub fn checksum(shared: &SharedProject) -> String {
    let mut s = shared.clone();
    s.project.updated_at = 0;
    for m in &mut s.memory {
        m.updated.clear();
    }
    let json = serde_json::to_string(&s).unwrap_or_default();
    format!("{:x}", md5::compute(json.as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    Insert,
    Replace,
    Skip,
}

/// Tentukan apa yang dilakukan pada project remote terhadap daftar lokal.
pub fn merge_action(local: &[Project], remote: &Project) -> MergeAction {
    if let Some(l) = local.iter().find(|p| p.id == remote.id) {
        return if remote.updated_at > l.updated_at {
            MergeAction::Replace
        } else {
            MergeAction::Skip
        };
    }
    if local
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(&remote.name))
    {
        return MergeAction::Skip;
    }
    MergeAction::Insert
}

/// Push semua project lokal yang bisa ditulis. Project milik sendiri di-upsert
/// berdasarkan nama; project milik orang lain dengan akses `editor` di-update
/// lewat id remote.
pub fn push_projects_to_server(
    app_dir: PathBuf,
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let locals = project::load_all(&app_dir);
        if locals.is_empty() {
            let _ = result_tx.send(Ok(0));
            return;
        }
        let remote = match client.list_projects(&token).await {
            Ok(r) => r,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let mut pushed = 0usize;
        for p in locals {
            if p.is_read_only() {
                continue;
            }
            let shared = build_shared(&app_dir, &p);
            let cs = checksum(&shared);
            let existing = match &p.remote_id {
                Some(id) => remote.iter().find(|r| &r.id == id),
                None => remote
                    .iter()
                    .find(|r| r.name == p.name && r.access.as_deref().is_none_or(|a| a == "owner")),
            };
            if existing.is_some_and(|r| r.client_checksum.as_deref() == Some(&cs)) {
                continue;
            }
            let Some(key) = vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "project",
                &p.name,
            ) else {
                continue; // Team key belum terbuka — dicoba lagi tick berikutnya
            };
            let json = match serde_json::to_string(&shared) {
                Ok(j) => j,
                Err(e) => {
                    warn!("[sync_projects] Cannot serialize '{}': {}", p.name, e);
                    continue;
                }
            };
            let payload = match vault_crypto::encrypt_str(key, &json) {
                Ok(c) => c,
                Err(e) => {
                    warn!("[sync_projects] Failed to encrypt '{}': {}", p.name, e);
                    continue;
                }
            };
            let result = if p.owner_id.is_some() {
                let Some(id) = &p.remote_id else { continue };
                let req = UpdateProjectReq {
                    payload: Some(payload),
                    client_checksum: Some(cs),
                    crypto_version: Some(1),
                };
                client.update_project(&token, id, &req).await.map(|_| ())
            } else {
                let req = UpsertProjectReq {
                    name: p.name.clone(),
                    payload,
                    client_checksum: Some(cs),
                    crypto_version: 1,
                };
                client.upsert_project(&token, &req).await.map(|_| ())
            };
            match result {
                Ok(()) => pushed += 1,
                Err(e) => warn!("[sync_projects] Failed to push '{}': {}", p.name, e),
            }
        }
        info!("[sync_projects] Pushed {} project(s)", pushed);
        let _ = result_tx.send(Ok(pushed));
    });
}

/// Tarik dan dekripsi semua project yang terlihat (milik sendiri dan yang
/// dibagikan). Merge dilakukan di thread UI karena perlu membuat folder.
pub fn pull_projects_from_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<Vec<PulledProject>, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let remote = match client.list_projects(&token).await {
            Ok(r) => r,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };
        let mut out = Vec::new();
        for r in remote {
            let Some(key) = vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "project",
                &r.name,
            ) else {
                info!(
                    "[sync_projects] Skipping '{}': Team key not unlocked yet",
                    r.name
                );
                continue;
            };
            let json = match vault_crypto::decrypt_str(key, &r.payload) {
                Ok(j) => j,
                Err(e) => {
                    warn!("[sync_projects] Failed to decrypt '{}': {}", r.name, e);
                    continue;
                }
            };
            match serde_json::from_str::<SharedProject>(&json) {
                Ok(shared) => out.push(PulledProject {
                    shared,
                    remote_id: r.id,
                    owner_id: r.user_id,
                    access: r.access.unwrap_or_else(|| "owner".to_string()),
                }),
                Err(e) => warn!("[sync_projects] Invalid payload for '{}': {}", r.name, e),
            }
        }
        let _ = result_tx.send(Ok(out));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_rules() {
        let mut local = Project::new("Shop");
        local.updated_at = 100;
        let locals = vec![local.clone()];

        let mut newer = local.clone();
        newer.updated_at = 200;
        assert_eq!(merge_action(&locals, &newer), MergeAction::Replace);

        let mut older = local.clone();
        older.updated_at = 50;
        assert_eq!(merge_action(&locals, &older), MergeAction::Skip);

        let other_same_name = Project::new("shop");
        assert_eq!(merge_action(&locals, &other_same_name), MergeAction::Skip);

        assert_eq!(
            merge_action(&locals, &Project::new("Billing")),
            MergeAction::Insert
        );
    }

    #[test]
    fn checksum_ignores_timestamps_and_local_fields() {
        let mut p = Project::new("Shop");
        p.environments[0].variables.push(crate::project::EnvVar {
            key: "TOKEN".into(),
            value: "should-not-leak".into(),
            secret: true,
            pending_secret: Some("also-not".into()),
        });
        p.shared_team_id = Some("team".into());
        let a = SharedProject {
            project: p.shared_copy(),
            memory: vec![],
        };
        let mut b = a.clone();
        b.project.updated_at += 99;
        assert_eq!(checksum(&a), checksum(&b));
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("should-not-leak") && !json.contains("also-not"));
        assert!(!json.contains("shared_team_id"));
    }
}
