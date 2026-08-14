//! Sync HTTP Requests — sync saved HTTP requests with the server.
//!
//! Offline-first: local collections (`http_collections/*.json`) are the source of truth.
//! Checksum detects conflicts; last-write-wins / local-kept default.

use log::{debug, info, warn};
use std::sync::mpsc;

use crate::http_collection::{
    HttpFolder, HttpWorkspace, SavedRequest, load_workspaces, save_workspaces,
};
use crate::models::structs::{HttpAuthType, HttpBodyType, HttpMethod};
use super::api_client::{ApiClient, CreateHttpRequestReq, RemoteHttpRequest};

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

/// Push all local HTTP requests to the server.
/// Requests that already exist on server (same checksum) are skipped.
pub fn push_http_requests_to_server(
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

            let (headers_json, body_json, auth_json) = pack_request_details(&flat.request);

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

/// Pull remote HTTP requests and merge into local workspaces.
pub fn pull_http_requests_from_server(
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
            let unpacked = unpack_remote_request(remote);
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

fn pack_request_details(req: &SavedRequest) -> (String, String, String) {
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

    (
        headers_data.to_string(),
        body_data.to_string(),
        auth_data.to_string(),
    )
}

fn unpack_remote_request(remote: &RemoteHttpRequest) -> SavedRequest {
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
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(h_json)
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
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(b_json)
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
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(a_json)
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
