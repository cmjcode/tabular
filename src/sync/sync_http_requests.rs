//! Sync HTTP Requests — sync saved HTTP requests with the server.
//!
//! Offline-first: local collections (`http_collections/*.json`) are the source of truth.
//! Checksum detects conflicts; last-write-wins / local-kept default.

use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::mpsc;

use crate::http_collection::{
    HttpFolder, HttpWorkspace, SavedRequest, load_workspaces, save_workspaces,
};
use crate::models::structs::{HttpAuthType, HttpBodyType, HttpMethod};
use super::api_client::{ApiClient, CreateHttpRequestReq, RemoteHttpRequest, RemoteSharedFolder, UpdateHttpRequestReq};
use super::vault_crypto::{self, SymKey};
use super::vault_sync;

/// Compute MD5 checksum of a SavedRequest (for conflict detection)
pub fn checksum(req: &SavedRequest) -> String {
    let json = serde_json::to_string(req).unwrap_or_default();
    let digest = md5::compute(json.as_bytes());
    format!("{:x}", digest)
}

/// Local request item flattened with workspace and folder path info.
struct FlatLocalRequest {
    workspace_name: String,
    folder_path: String,
    request: SavedRequest,
}

/// Push all local HTTP requests to the server, encrypted with the
/// AccountKey (personal folders) or the owning Team's key (Team-shared
/// folders — resolved per item via `vault_sync::resolve_key_for_folder`).
/// Requests whose folder is Team-shared but whose Team key isn't unlocked
/// yet are skipped (retried on a later sync tick). Already-synced requests
/// (same checksum) are skipped too.
pub fn push_http_requests_to_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let workspaces = load_workspaces();

        let flat_requests = collect_flat_requests(&workspaces);
        if flat_requests.is_empty() {
            let _ = result_tx.send(Ok(0));
            return;
        }

        // Get remote HTTP request list for dedup
        let remote_requests = match client.list_http_requests(&token).await {
            Ok(reqs) => reqs,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let mut pushed = 0usize;
        for flat in flat_requests {
            let req_name = flat.request.display_name();
            let cs = checksum(&flat.request);

            // Skip if server already has same checksum
            let already_synced = remote_requests.iter().any(|r| {
                r.workspace_name == flat.workspace_name
                    && r.folder_path == flat.folder_path
                    && r.name == req_name
                    && r.client_checksum.as_deref() == Some(&cs)
            });
            if already_synced {
                continue;
            }

            let key = match vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "http",
                &flat.folder_path,
            ) {
                Some(k) => k,
                None => continue, // Team key not unlocked yet — retried next tick
            };

            let (headers_json, body_json, auth_json) = match pack_request_details(key, &flat.request) {
                Ok(v) => v,
                Err(e) => {
                    warn!("❌ [sync_http] Failed to encrypt '{}': {}", req_name, e);
                    continue;
                }
            };

            let req = CreateHttpRequestReq {
                workspace_name: flat.workspace_name.clone(),
                folder_path: Some(flat.folder_path.clone()),
                name: req_name.clone(),
                method: flat.request.method.label().to_string(),
                url: flat.request.url.clone(),
                headers_json: Some(headers_json),
                body_json: Some(body_json),
                auth_json: Some(auth_json),
                client_checksum: Some(cs),
                crypto_version: 1,
            };

            match client.create_http_request(&token, &req).await {
                Ok(_) => pushed += 1,
                Err(e) => warn!("❌ [sync_http] Failed to push '{}': {}", req_name, e),
            }
        }

        info!("✅ [sync_http] Pushed {} new/updated HTTP requests to server", pushed);
        let _ = result_tx.send(Ok(pushed));
    });
}

/// Re-encrypt a single legacy (`crypto_version = 0`, never-encrypted) HTTP
/// request under the resolved vault key and persist it as `crypto_version = 1`.
/// Fire-and-forget: on failure the row stays `crypto_version = 0` and
/// migration is retried on the next pull.
fn migrate_legacy_http_request(remote: RemoteHttpRequest, key: SymKey, token: String, server_url: String) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let legacy = unpack_remote_request_legacy(&remote);
        let (headers_json, body_json, auth_json) = match pack_request_details(&key, &legacy) {
            Ok(v) => v,
            Err(e) => {
                warn!("❌ [migrate] Failed to encrypt legacy request '{}': {}", remote.name, e);
                return;
            }
        };
        let update = UpdateHttpRequestReq {
            workspace_name: None,
            folder_path: None,
            name: None,
            method: None,
            url: None,
            headers_json: Some(headers_json),
            body_json: Some(body_json),
            auth_json: Some(auth_json),
            client_checksum: None,
            crypto_version: Some(1),
        };
        match client.update_http_request(&token, &remote.id, &update).await {
            Ok(_) => info!("✅ [migrate] Migrated legacy HTTP request '{}' to end-to-end encryption", remote.name),
            Err(e) => warn!("❌ [migrate] Failed to migrate HTTP request '{}': {}", remote.name, e),
        }
    });
}

/// Re-encrypt every local HTTP request under `folder_path` with `key` and
/// upsert it to the server. Used right after a folder becomes newly
/// Team-shared, so items already synced under the personal AccountKey move
/// onto the Team key instead of staying owner-only-readable. Fire-and-forget;
/// failures are logged, not surfaced to the UI.
pub fn reencrypt_folder_to_server(
    key: SymKey,
    folder_path: String,
    token: String,
    server_url: String,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);
        let workspaces = load_workspaces();
        let flat_requests: Vec<FlatLocalRequest> = collect_flat_requests(&workspaces)
            .into_iter()
            .filter(|f| f.folder_path == folder_path)
            .collect();
        if flat_requests.is_empty() {
            return;
        }

        let remote_requests = match client.list_http_requests(&token).await {
            Ok(r) => r,
            Err(e) => {
                warn!("❌ [sync_http] re-encrypt: failed to list remote requests: {}", e);
                return;
            }
        };

        let mut migrated = 0usize;
        for flat in flat_requests {
            let req_name = flat.request.display_name();
            let (headers_json, body_json, auth_json) = match pack_request_details(&key, &flat.request) {
                Ok(v) => v,
                Err(e) => {
                    warn!("❌ [sync_http] re-encrypt: failed to encrypt '{}': {}", req_name, e);
                    continue;
                }
            };
            let cs = checksum(&flat.request);

            let existing = remote_requests.iter().find(|r| {
                r.workspace_name == flat.workspace_name && r.folder_path == flat.folder_path && r.name == req_name
            });
            let result = match existing {
                Some(r) => {
                    let update = UpdateHttpRequestReq {
                        workspace_name: None,
                        folder_path: None,
                        name: None,
                        method: None,
                        url: None,
                        headers_json: Some(headers_json),
                        body_json: Some(body_json),
                        auth_json: Some(auth_json),
                        client_checksum: Some(cs),
                        crypto_version: Some(1),
                    };
                    client.update_http_request(&token, &r.id, &update).await.map(|_| ())
                }
                None => {
                    let req = CreateHttpRequestReq {
                        workspace_name: flat.workspace_name.clone(),
                        folder_path: Some(flat.folder_path.clone()),
                        name: req_name.clone(),
                        method: flat.request.method.label().to_string(),
                        url: flat.request.url.clone(),
                        headers_json: Some(headers_json),
                        body_json: Some(body_json),
                        auth_json: Some(auth_json),
                        client_checksum: Some(cs),
                        crypto_version: 1,
                    };
                    client.create_http_request(&token, &req).await.map(|_| ())
                }
            };
            match result {
                Ok(()) => migrated += 1,
                Err(e) => warn!("❌ [sync_http] re-encrypt: failed to upsert '{}': {}", req_name, e),
            }
        }
        info!("✅ [sync_http] Re-encrypted {} request(s) in '{}' under the Team key", migrated, folder_path);
    });
}

/// Pull remote HTTP requests and merge into local workspaces. Requests that
/// can't be decrypted yet (legacy plaintext rows pre-dating crypto_version 1,
/// or a Team-shared item whose Team key isn't unlocked) are skipped.
pub fn pull_http_requests_from_server(
    account_key: SymKey,
    team_keys: HashMap<String, SymKey>,
    shared_folders: Vec<RemoteSharedFolder>,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<usize, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);

        let remote_requests = match client.list_http_requests(&token).await {
            Ok(reqs) => reqs,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        if remote_requests.is_empty() {
            let _ = result_tx.send(Ok(0));
            return;
        }

        let mut workspaces = load_workspaces();
        let mut saved = 0usize;

        for remote in &remote_requests {
            let key = match vault_sync::resolve_key_for_folder(
                &account_key,
                &team_keys,
                &shared_folders,
                "http",
                &remote.folder_path,
            ) {
                Some(k) => k,
                None => {
                    info!("[sync_http] Skipping Team-shared '{}': Team key not unlocked yet", remote.name);
                    continue;
                }
            };

            let unpacked = if remote.crypto_version >= 1 {
                match unpack_remote_request(key, remote) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("❌ [sync_http] Failed to decrypt '{}': {}", remote.name, e);
                        continue;
                    }
                }
            } else {
                // Legacy (pre-vault) row — was never encrypted, just plain
                // JSON. Parse it as-is, then queue a re-upload under the real
                // vault key so it migrates for good.
                let legacy = unpack_remote_request_legacy(remote);
                migrate_legacy_http_request(remote.clone(), key.clone(), token.clone(), server_url.clone());
                legacy
            };
            let added = merge_remote_request(&mut workspaces, remote, unpacked);
            if added {
                saved += 1;
            }
        }

        if saved > 0 {
            save_workspaces(&workspaces);
        }

        info!("✅ [sync_http] Downloaded {} HTTP requests from server", saved);
        let _ = result_tx.send(Ok(saved));
    });
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn collect_flat_requests(workspaces: &[HttpWorkspace]) -> Vec<FlatLocalRequest> {
    let mut out = Vec::new();
    for ws in workspaces {
        let root_path = format!("/{}", ws.name);
        for req in &ws.requests {
            out.push(FlatLocalRequest {
                workspace_name: ws.name.clone(),
                folder_path: root_path.clone(),
                request: req.clone(),
            });
        }
        for folder in &ws.folders {
            collect_folder_requests(&ws.name, &root_path, folder, &mut out);
        }
    }
    out
}

fn collect_folder_requests(
    ws_name: &str,
    parent_path: &str,
    folder: &HttpFolder,
    out: &mut Vec<FlatLocalRequest>,
) {
    let current_path = format!("{}/{}", parent_path, folder.name);
    for req in &folder.requests {
        out.push(FlatLocalRequest {
            workspace_name: ws_name.to_string(),
            folder_path: current_path.clone(),
            request: req.clone(),
        });
    }
    for child in &folder.children {
        collect_folder_requests(ws_name, &current_path, child, out);
    }
}

/// Encrypts each of the three JSON blobs independently with `key` (AES-256-GCM).
/// `auth_json` in particular carries bearer tokens / basic-auth passwords /
/// API keys — the whole point of this module's crypto.
fn pack_request_details(key: &SymKey, req: &SavedRequest) -> Result<(String, String, String), String> {
    let headers_data = serde_json::json!({
        "params": req.params,
        "headers": req.headers,
        "description": req.description,
    });
    let body_data = serde_json::json!({
        "body_type": req.body_type,
        "body_text": req.body_text,
        "form_data": req.form_data,
    });
    let auth_data = serde_json::json!({
        "auth_type": req.auth_type,
        "bearer_token": req.bearer_token,
        "basic_user": req.basic_user,
        "basic_pass": req.basic_pass,
        "api_key_name": req.api_key_name,
        "api_key_value": req.api_key_value,
        "api_key_in_header": req.api_key_in_header,
    });

    Ok((
        vault_crypto::encrypt_str(key, &headers_data.to_string())?,
        vault_crypto::encrypt_str(key, &body_data.to_string())?,
        vault_crypto::encrypt_str(key, &auth_data.to_string())?,
    ))
}

/// Decrypts (vault key) each JSON blob before parsing — the `crypto_version >= 1` path.
fn unpack_remote_request(key: &SymKey, remote: &RemoteHttpRequest) -> Result<SavedRequest, String> {
    let decode = |encoded: &str| -> Option<serde_json::Value> {
        let plaintext = vault_crypto::decrypt_str(key, encoded).ok()?;
        serde_json::from_str::<serde_json::Value>(&plaintext).ok()
    };
    Ok(unpack_remote_request_with(remote, decode))
}

/// Parses each JSON blob as plain (never-encrypted) JSON — the pre-vault
/// `crypto_version = 0` scheme, used only for one-time migration.
fn unpack_remote_request_legacy(remote: &RemoteHttpRequest) -> SavedRequest {
    let decode = |raw: &str| -> Option<serde_json::Value> { serde_json::from_str::<serde_json::Value>(raw).ok() };
    unpack_remote_request_with(remote, decode)
}

fn unpack_remote_request_with(
    remote: &RemoteHttpRequest,
    decode: impl Fn(&str) -> Option<serde_json::Value>,
) -> SavedRequest {
    let mut req = SavedRequest {
        id: remote.id.clone(),
        workspace_id: String::new(),
        folder_id: None,
        name: remote.name.clone(),
        url: remote.url.clone(),
        method: parse_http_method(&remote.method),
        params: Vec::new(),
        headers: Vec::new(),
        body_type: HttpBodyType::NoBody,
        body_text: String::new(),
        form_data: Vec::new(),
        auth_type: HttpAuthType::NoAuth,
        bearer_token: String::new(),
        basic_user: String::new(),
        basic_pass: String::new(),
        api_key_name: String::new(),
        api_key_value: String::new(),
        api_key_in_header: true,
        description: String::new(),
    };

    if let Some(h_json) = &remote.headers_json
        && let Some(val) = decode(h_json)
    {
        if let Some(params) = val.get("params") {
            if let Ok(p) = serde_json::from_value(params.clone()) {
                req.params = p;
            }
        }
        if let Some(headers) = val.get("headers") {
            if let Ok(h) = serde_json::from_value(headers.clone()) {
                req.headers = h;
            }
        }
        if let Some(desc) = val.get("description").and_then(|v| v.as_str()) {
            req.description = desc.to_string();
        }
    }

    if let Some(b_json) = &remote.body_json
        && let Some(val) = decode(b_json)
    {
        if let Some(bt) = val.get("body_type") {
            if let Ok(b) = serde_json::from_value(bt.clone()) {
                req.body_type = b;
            }
        }
        if let Some(btxt) = val.get("body_text").and_then(|v| v.as_str()) {
            req.body_text = btxt.to_string();
        }
        if let Some(fd) = val.get("form_data") {
            if let Ok(f) = serde_json::from_value(fd.clone()) {
                req.form_data = f;
            }
        }
    }

    if let Some(a_json) = &remote.auth_json
        && let Some(val) = decode(a_json)
    {
        if let Some(at) = val.get("auth_type") {
            if let Ok(a) = serde_json::from_value(at.clone()) {
                req.auth_type = a;
            }
        }
        if let Some(tok) = val.get("bearer_token").and_then(|v| v.as_str()) {
            req.bearer_token = tok.to_string();
        }
        if let Some(u) = val.get("basic_user").and_then(|v| v.as_str()) {
            req.basic_user = u.to_string();
        }
        if let Some(p) = val.get("basic_pass").and_then(|v| v.as_str()) {
            req.basic_pass = p.to_string();
        }
        if let Some(kn) = val.get("api_key_name").and_then(|v| v.as_str()) {
            req.api_key_name = kn.to_string();
        }
        if let Some(kv) = val.get("api_key_value").and_then(|v| v.as_str()) {
            req.api_key_value = kv.to_string();
        }
        if let Some(kh) = val.get("api_key_in_header").and_then(|v| v.as_bool()) {
            req.api_key_in_header = kh;
        }
    }

    req
}

fn parse_http_method(s: &str) -> HttpMethod {
    match s.to_ascii_uppercase().as_str() {
        "POST" => HttpMethod::POST,
        "PUT" => HttpMethod::PUT,
        "DELETE" => HttpMethod::DELETE,
        "PATCH" => HttpMethod::PATCH,
        "HEAD" => HttpMethod::HEAD,
        "OPTIONS" => HttpMethod::OPTIONS,
        _ => HttpMethod::GET,
    }
}

/// Merges a downloaded remote request into the local `HttpWorkspace` collection.
/// Returns true if a new request was added or updated.
fn merge_remote_request(
    workspaces: &mut Vec<HttpWorkspace>,
    remote: &RemoteHttpRequest,
    mut unpacked_req: SavedRequest,
) -> bool {
    let ws_name = &remote.workspace_name;
    // Find or create workspace
    let ws_idx = if let Some(pos) = workspaces.iter().position(|w| &w.name == ws_name) {
        pos
    } else {
        let ws_id = format!("ws_{}_{}", chrono::Utc::now().timestamp_millis(), rand_suffix());
        workspaces.push(HttpWorkspace {
            id: ws_id,
            name: ws_name.clone(),
            requests: Vec::new(),
            folders: Vec::new(),
            environments: Vec::new(),
        });
        workspaces.len() - 1
    };

    let ws = &mut workspaces[ws_idx];
    unpacked_req.workspace_id = ws.id.clone();

    // Parse folder path: e.g. "/My Workspace/Auth/OAuth" -> subfolders = ["Auth", "OAuth"]
    let root_prefix = format!("/{}", ws_name);
    let sub_path = if remote.folder_path.starts_with(&root_prefix) {
        remote.folder_path[root_prefix.len()..].trim_start_matches('/')
    } else {
        remote.folder_path.trim_start_matches('/')
    };

    let folder_parts: Vec<&str> = if sub_path.is_empty() {
        Vec::new()
    } else {
        sub_path.split('/').filter(|s| !s.is_empty()).collect()
    };

    if folder_parts.is_empty() {
        // Workspace top-level request
        if let Some(existing) = ws.requests.iter_mut().find(|r| r.id == remote.id || r.name == remote.name) {
            let cs = checksum(existing);
            if remote.client_checksum.as_deref() == Some(&cs) {
                return false; // In sync
            }
            debug!("⚠️ [sync_http] Conflict on '{}' — keeping local version", remote.name);
            return false;
        }
        ws.requests.push(unpacked_req);
        true
    } else {
        // Request inside nested folder
        let folder = navigate_or_create_folders(&mut ws.folders, &folder_parts);
        unpacked_req.folder_id = Some(folder.id.clone());

        if let Some(existing) = folder.requests.iter_mut().find(|r| r.id == remote.id || r.name == remote.name) {
            let cs = checksum(existing);
            if remote.client_checksum.as_deref() == Some(&cs) {
                return false; // In sync
            }
            debug!("⚠️ [sync_http] Conflict on '{}' — keeping local version", remote.name);
            return false;
        }
        folder.requests.push(unpacked_req);
        true
    }
}

fn rand_suffix() -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::Instant::now().hash(&mut hasher);
    (hasher.finish() % 100_000) as u32
}

fn navigate_or_create_folders<'a>(
    folders: &'a mut Vec<HttpFolder>,
    parts: &[&str],
) -> &'a mut HttpFolder {
    let current_folders = folders;
    let mut target_idx = 0;

    for (i, part) in parts.iter().enumerate() {
        let idx = if let Some(pos) = current_folders.iter().position(|f| f.name == *part) {
            pos
        } else {
            let new_id = format!("folder_{}_{}", chrono::Utc::now().timestamp_millis(), rand_suffix());
            current_folders.push(HttpFolder {
                id: new_id,
                name: (*part).to_string(),
                parent_folder_id: None,
                requests: Vec::new(),
                children: Vec::new(),
            });
            current_folders.len() - 1
        };

        if i == parts.len() - 1 {
            target_idx = idx;
        } else {
            let child_folders = &mut current_folders[idx].children;
            return navigate_or_create_folders(child_folders, &parts[i + 1..]);
        }
    }

    &mut current_folders[target_idx]
}
