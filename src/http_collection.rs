//! http_collection.rs — Saved Request collections for Tabular HTTP client.
//!
//! Provides:
//!   - Data model: `HttpWorkspace`, `HttpFolder`, `SavedRequest`, `YaakEnvironment`
//!   - Persistence: save/load collections to/from `{app_data}/http_collections/`
//!   - Yaak importer: read Yaak's SQLite database and populate the model

use serde::{Deserialize, Serialize};

use crate::models::structs::{
    HttpAuthType, HttpBodyType, HttpClientState, HttpMethod,
};

// ─── Core Data Model ─────────────────────────────────────────────────────────

/// A single saved HTTP request (mirrors HttpClientState, minus runtime fields).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SavedRequest {
    pub id: String,
    pub workspace_id: String,
    pub folder_id: Option<String>,
    pub name: String,
    pub url: String,
    pub method: HttpMethod,
    pub params: Vec<(String, String, bool)>,
    pub headers: Vec<(String, String, bool)>,
    pub body_type: HttpBodyType,
    pub body_text: String,
    pub form_data: Vec<(String, String, bool)>,
    pub auth_type: HttpAuthType,
    pub bearer_token: String,
    pub basic_user: String,
    pub basic_pass: String,
    pub api_key_name: String,
    pub api_key_value: String,
    pub api_key_in_header: bool,
    pub description: String,
}

/// Extracts the endpoint path from a URL (e.g. "https://api.example.com/v1/users?a=1" -> "/v1/users").
/// Strips protocol, domain/port, query string, and hash fragment.
pub fn extract_endpoint_url(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return "Untitled Request".to_string();
    }

    // 1. Strip query parameters and fragment
    let url_no_query = trimmed.split('?').next().unwrap_or(trimmed);
    let url_clean = url_no_query.split('#').next().unwrap_or(url_no_query);

    // 2. Check and strip scheme (http://, https://, ws://, wss://, etc.)
    let has_scheme = url_clean.contains("://");
    let after_scheme = if let Some(idx) = url_clean.find("://") {
        &url_clean[idx + 3..]
    } else {
        url_clean
    };

    // 3. Strip leading template variables if present (e.g. {{base_url}}/v1/users)
    let (after_var, stripped_var) = if after_scheme.starts_with("{{") {
        if let Some(end_var) = after_scheme.find("}}") {
            (&after_scheme[end_var + 2..], true)
        } else {
            (after_scheme, false)
        }
    } else {
        (after_scheme, false)
    };

    let target = after_var.trim();

    if target.is_empty() {
        return "/".to_string();
    }

    // If target already starts with '/', return it
    if target.starts_with('/') {
        return target.to_string();
    }

    // If scheme was present or template variable stripped, find first '/'
    if has_scheme || stripped_var {
        if let Some(slash_idx) = target.find('/') {
            return target[slash_idx..].to_string();
        } else {
            return "/".to_string();
        }
    }

    // If no scheme and no template variable:
    if let Some(slash_idx) = target.find('/') {
        let first_seg = &target[..slash_idx];
        if first_seg.contains('.') || first_seg.contains(':') || first_seg == "localhost" {
            return target[slash_idx..].to_string();
        } else {
            return format!("/{}", target);
        }
    }

    // No slash at all
    if target.contains('.') || target.contains(':') || target == "localhost" {
        "/".to_string()
    } else {
        format!("/{}", target)
    }
}

impl SavedRequest {
    /// Returns the display name for the request.
    /// If `name` is non-empty, returns `name`.
    /// Otherwise, extracts the endpoint URL path from `url`.
    pub fn display_name(&self) -> String {
        let trimmed_name = self.name.trim();
        if !trimmed_name.is_empty() {
            trimmed_name.to_string()
        } else {
            extract_endpoint_url(&self.url)
        }
    }
}

/// A sub-folder that groups requests inside a workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpFolder {
    pub id: String,
    pub name: String,
    pub parent_folder_id: Option<String>,
    /// Requests directly inside this folder (not in sub-folders).
    pub requests: Vec<SavedRequest>,
    /// Child sub-folders (populated after full tree resolution).
    pub children: Vec<HttpFolder>,
}

/// A workspace (project) containing folders and top-level requests.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpWorkspace {
    pub id: String,
    pub name: String,
    /// Requests not inside any folder.
    pub requests: Vec<SavedRequest>,
    /// Top-level folders (may be nested).
    pub folders: Vec<HttpFolder>,
    /// Environment variables for this workspace.
    pub environments: Vec<YaakEnvironment>,
}

/// An environment (set of key-value variables) from Yaak.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YaakEnvironment {
    pub id: String,
    pub name: String,
    pub variables: Vec<(String, String)>, // (name, value)
}

// ─── UI State ────────────────────────────────────────────────────────────────

/// UI state for the collection panel (sidebar in the HTTP client).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CollectionPanelState {
    /// Whether the sidebar is visible.
    pub visible: bool,
    /// ID of the currently expanded workspace.
    pub active_workspace_id: Option<String>,
    /// Set of folder IDs that are expanded in the tree.
    pub expanded_folders: std::collections::HashSet<String>,
    /// Filter text for request search.
    pub search_text: String,
    /// Active environment per workspace: workspace_id → environment_id.
    pub active_env: std::collections::HashMap<String, String>,
}

// ─── Persistence ─────────────────────────────────────────────────────────────

fn collections_dir() -> std::path::PathBuf {
    crate::directory::get_app_data_dir().join("http_collections")
}

/// Persist a list of workspaces to disk.
/// Each workspace is stored as `{app_data}/http_collections/{workspace_id}.json`.
pub fn save_workspaces(workspaces: &[HttpWorkspace]) {
    let dir = collections_dir();
    let _ = std::fs::create_dir_all(&dir);
    for ws in workspaces {
        let path = dir.join(format!("{}.json", ws.id));
        if let Ok(json) = serde_json::to_string_pretty(ws) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Load all persisted workspaces from disk.
pub fn load_workspaces() -> Vec<HttpWorkspace> {
    let dir = collections_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(ws) = serde_json::from_str::<HttpWorkspace>(&contents) else {
            continue;
        };
        result.push(ws);
    }
    // Sort alphabetically by name for stable ordering.
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Delete a workspace JSON file from disk.
pub fn delete_workspace(workspace_id: &str) {
    let path = collections_dir().join(format!("{}.json", workspace_id));
    let _ = std::fs::remove_file(path);
}

static ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn unique_id(prefix: &str) -> String {
    let count = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}_{}_{}", prefix, chrono::Utc::now().timestamp_millis(), count)
}

/// Create a new workspace/collection, persist it, and return the new workspace.
pub fn create_workspace(workspaces: &mut Vec<HttpWorkspace>, ws_name: &str) -> HttpWorkspace {
    let ws_id = unique_id("ws");
    let new_ws = HttpWorkspace {
        id: ws_id,
        name: ws_name.trim().to_string(),
        requests: Vec::new(),
        folders: Vec::new(),
        environments: Vec::new(),
    };
    workspaces.push(new_ws.clone());
    save_workspaces(workspaces);
    new_ws
}

/// Add a new folder or subfolder inside a workspace.
pub fn create_folder_in_workspace(
    workspaces: &mut [HttpWorkspace],
    ws_id: &str,
    parent_folder_id: Option<&str>,
    folder_name: &str,
) -> Option<HttpFolder> {
    let new_id = unique_id("fld");
    let new_folder = HttpFolder {
        id: new_id,
        name: folder_name.trim().to_string(),
        parent_folder_id: parent_folder_id.map(|s| s.to_string()),
        requests: Vec::new(),
        children: Vec::new(),
    };

    let ws = workspaces.iter_mut().find(|w| w.id == ws_id)?;

    if let Some(p_id) = parent_folder_id {
        fn insert_into_folder(
            folders: &mut [HttpFolder],
            target_id: &str,
            folder_to_insert: HttpFolder,
        ) -> bool {
            for f in folders.iter_mut() {
                if f.id == target_id {
                    f.children.push(folder_to_insert);
                    return true;
                }
                if insert_into_folder(&mut f.children, target_id, folder_to_insert.clone()) {
                    return true;
                }
            }
            false
        }
        if !insert_into_folder(&mut ws.folders, p_id, new_folder.clone()) {
            return None;
        }
    } else {
        ws.folders.push(new_folder.clone());
    }

    save_workspaces(workspaces);
    Some(new_folder)
}

/// Rename an existing folder inside a workspace.
pub fn rename_folder_in_workspaces(
    workspaces: &mut [HttpWorkspace],
    ws_id: &str,
    folder_id: &str,
    new_name: &str,
) -> bool {
    let Some(ws) = workspaces.iter_mut().find(|w| w.id == ws_id) else {
        return false;
    };

    fn rename_in_tree(folders: &mut [HttpFolder], folder_id: &str, new_name: &str) -> bool {
        for f in folders.iter_mut() {
            if f.id == folder_id {
                f.name = new_name.trim().to_string();
                return true;
            }
            if rename_in_tree(&mut f.children, folder_id, new_name) {
                return true;
            }
        }
        false
    }

    if rename_in_tree(&mut ws.folders, folder_id, new_name) {
        save_workspaces(workspaces);
        true
    } else {
        false
    }
}

/// Move a saved request to a new parent folder or workspace root.
pub fn move_request(
    workspaces: &mut [HttpWorkspace],
    req_id: &str,
    target_ws_id: &str,
    target_folder_id: Option<&str>,
) -> bool {
    let mut found_req = None;

    fn extract_from_folders(folders: &mut [HttpFolder], req_id: &str) -> Option<SavedRequest> {
        for f in folders.iter_mut() {
            if let Some(pos) = f.requests.iter().position(|r| r.id == req_id) {
                return Some(f.requests.remove(pos));
            }
            if let Some(r) = extract_from_folders(&mut f.children, req_id) {
                return Some(r);
            }
        }
        None
    }

    for ws in workspaces.iter_mut() {
        if let Some(pos) = ws.requests.iter().position(|r| r.id == req_id) {
            found_req = Some(ws.requests.remove(pos));
            break;
        }
        if let Some(r) = extract_from_folders(&mut ws.folders, req_id) {
            found_req = Some(r);
            break;
        }
    }

    let Some(mut req) = found_req else {
        return false;
    };

    req.workspace_id = target_ws_id.to_string();
    req.folder_id = target_folder_id.map(|s| s.to_string());

    let Some(target_ws) = workspaces.iter_mut().find(|w| w.id == target_ws_id) else {
        return false;
    };

    if let Some(tf_id) = target_folder_id {
        fn insert_into_folder(folders: &mut [HttpFolder], target_id: &str, req: SavedRequest) -> bool {
            for f in folders.iter_mut() {
                if f.id == target_id {
                    f.requests.push(req);
                    return true;
                }
                if insert_into_folder(&mut f.children, target_id, req.clone()) {
                    return true;
                }
            }
            false
        }
        if !insert_into_folder(&mut target_ws.folders, tf_id, req) {
            return false;
        }
    } else {
        target_ws.requests.push(req);
    }

    save_workspaces(workspaces);
    true
}

/// Move an HTTP folder to another folder (as a subfolder) or to the workspace root.
/// Returns false if target parent folder is invalid (e.g. self or a descendant of folder_id).
pub fn move_folder(
    workspaces: &mut [HttpWorkspace],
    folder_id: &str,
    target_ws_id: &str,
    target_parent_folder_id: Option<&str>,
) -> bool {
    // Prevent moving into itself
    if target_parent_folder_id == Some(folder_id) {
        return false;
    }

    // Check if target_parent_folder_id is a descendant of folder_id
    fn is_descendant(folders: &[HttpFolder], root_id: &str, target_id: &str) -> bool {
        for f in folders {
            if f.id == root_id {
                return contains_folder(&f.children, target_id);
            }
            if is_descendant(&f.children, root_id, target_id) {
                return true;
            }
        }
        false
    }

    fn contains_folder(folders: &[HttpFolder], target_id: &str) -> bool {
        for f in folders {
            if f.id == target_id || contains_folder(&f.children, target_id) {
                return true;
            }
        }
        false
    }

    if let Some(t_parent_id) = target_parent_folder_id {
        for ws in workspaces.iter() {
            if is_descendant(&ws.folders, folder_id, t_parent_id) {
                return false;
            }
        }
    }

    // Extract folder from its current location
    let mut found_folder = None;

    fn extract_folder(folders: &mut Vec<HttpFolder>, folder_id: &str) -> Option<HttpFolder> {
        if let Some(pos) = folders.iter().position(|f| f.id == folder_id) {
            return Some(folders.remove(pos));
        }
        for f in folders.iter_mut() {
            if let Some(extracted) = extract_folder(&mut f.children, folder_id) {
                return Some(extracted);
            }
        }
        None
    }

    for ws in workspaces.iter_mut() {
        if let Some(extracted) = extract_folder(&mut ws.folders, folder_id) {
            found_folder = Some(extracted);
            break;
        }
    }

    let Some(mut folder) = found_folder else {
        return false;
    };

    folder.parent_folder_id = target_parent_folder_id.map(|s| s.to_string());

    fn update_ws_id_recursive(folder: &mut HttpFolder, new_ws_id: &str) {
        for req in &mut folder.requests {
            req.workspace_id = new_ws_id.to_string();
        }
        for child in &mut folder.children {
            update_ws_id_recursive(child, new_ws_id);
        }
    }
    update_ws_id_recursive(&mut folder, target_ws_id);

    let Some(target_ws) = workspaces.iter_mut().find(|w| w.id == target_ws_id) else {
        return false;
    };

    if let Some(tf_id) = target_parent_folder_id {
        fn insert_folder_into_parent(folders: &mut [HttpFolder], target_id: &str, folder: HttpFolder) -> bool {
            for f in folders.iter_mut() {
                if f.id == target_id {
                    f.children.push(folder);
                    return true;
                }
                if insert_folder_into_parent(&mut f.children, target_id, folder.clone()) {
                    return true;
                }
            }
            false
        }
        if !insert_folder_into_parent(&mut target_ws.folders, tf_id, folder) {
            return false;
        }
    } else {
        target_ws.folders.push(folder);
    }

    save_workspaces(workspaces);
    true
}



// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Load a `SavedRequest` into an `HttpClientState`, preserving runtime fields.
pub fn apply_saved_request(saved: &SavedRequest, state: &mut HttpClientState) {
    state.url = saved.url.clone();
    state.method = saved.method.clone();
    state.params = saved.params.clone();
    state.headers = saved.headers.clone();
    state.body_type = saved.body_type.clone();
    state.body_text = saved.body_text.clone();
    state.form_data = saved.form_data.clone();
    state.auth_type = saved.auth_type.clone();
    state.bearer_token = saved.bearer_token.clone();
    state.basic_user = saved.basic_user.clone();
    state.basic_pass = saved.basic_pass.clone();
    state.api_key_name = saved.api_key_name.clone();
    state.api_key_value = saved.api_key_value.clone();
    state.api_key_in_header = saved.api_key_in_header;

    // Track original saved request metadata for direct updates
    state.saved_request_id = Some(saved.id.clone());
    state.saved_workspace_id = Some(saved.workspace_id.clone());
    state.saved_folder_id = saved.folder_id.clone();
    state.save_dialog_name = saved.name.clone();

    // Clear previous response
    state.response_status = None;
    state.response_status_text.clear();
    state.response_body.clear();
    state.response_headers.clear();
    state.response_time_ms = None;
    state.response_size_bytes = None;
    state.response_error = None;
    state.is_loading = false;
    state.response_receiver = None;
}

/// Snapshot the current `HttpClientState` into a new `SavedRequest`.
pub fn snapshot_from_state(state: &HttpClientState, name: &str) -> SavedRequest {
    SavedRequest {
        id: format!("sr_{}", chrono::Utc::now().timestamp_millis()),
        workspace_id: "local".to_string(),
        folder_id: None,
        name: name.to_string(),
        url: state.url.clone(),
        method: state.method.clone(),
        params: state.params.clone(),
        headers: state.headers.clone(),
        body_type: state.body_type.clone(),
        body_text: state.body_text.clone(),
        form_data: state.form_data.clone(),
        auth_type: state.auth_type.clone(),
        bearer_token: state.bearer_token.clone(),
        basic_user: state.basic_user.clone(),
        basic_pass: state.basic_pass.clone(),
        api_key_name: state.api_key_name.clone(),
        api_key_value: state.api_key_value.clone(),
        api_key_in_header: state.api_key_in_header,
        description: String::new(),
    }
}

// ─── Yaak Import ─────────────────────────────────────────────────────────────

/// Result returned from a Yaak import attempt.
pub struct YaakImportResult {
    pub workspaces: Vec<HttpWorkspace>,
    pub total_requests: usize,
    pub warnings: Vec<String>,
}

/// Import all workspaces and requests from a Yaak SQLite database file.
///
/// Uses a blocking SQLite connection (via `rusqlite`-style raw reads through
/// `libsqlite3-sys`). Wraps `sqlite_import_inner` which does the actual work.
pub fn import_from_yaak(db_path: &std::path::Path) -> Result<YaakImportResult, String> {
    // We use raw sqlite3 via the bundled libsqlite3-sys crate.
    // Opening read-only to avoid locking conflicts with a running Yaak instance.
    import_yaak_sqlite(db_path)
}

// ─── SQLite helpers (no async needed; runs on calling thread) ─────────────────

/// Raw type-safe wrapper around a sqlite3 connection using libsqlite3-sys.
/// This avoids pulling in rusqlite as a new dep — we can drive the C API
/// directly because libsqlite3-sys is already in the tree (bundled).
mod sqlite_raw {
    use libsqlite3_sys as ffi;
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int};
    use std::path::Path;

    pub struct Conn(*mut ffi::sqlite3);
    pub struct Stmt(*mut ffi::sqlite3_stmt);

    unsafe impl Send for Conn {}
    unsafe impl Send for Stmt {}

    impl Conn {
        pub fn open_readonly(path: &Path) -> Result<Self, String> {
            let path_str = path
                .to_str()
                .ok_or("Path is not valid UTF-8")?;
            let c_path = CString::new(path_str).map_err(|e| e.to_string())?;
            let mut db: *mut ffi::sqlite3 = std::ptr::null_mut();
            let rc = unsafe {
                ffi::sqlite3_open_v2(
                    c_path.as_ptr(),
                    &mut db,
                    ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_NOMUTEX,
                    std::ptr::null(),
                )
            };
            if rc != ffi::SQLITE_OK as c_int {
                return Err(format!("Cannot open Yaak DB ({}): {}", rc, sqlite_errmsg(db)));
            }
            Ok(Conn(db))
        }

        pub fn prepare(&self, sql: &str) -> Result<Stmt, String> {
            let c_sql = CString::new(sql).map_err(|e| e.to_string())?;
            let mut stmt: *mut ffi::sqlite3_stmt = std::ptr::null_mut();
            let rc = unsafe {
                ffi::sqlite3_prepare_v2(
                    self.0,
                    c_sql.as_ptr(),
                    -1,
                    &mut stmt,
                    std::ptr::null_mut(),
                )
            };
            if rc != ffi::SQLITE_OK as c_int {
                return Err(format!("prepare failed ({})", rc));
            }
            Ok(Stmt(stmt))
        }

        /// Execute a query and call `row_fn` for each row.
        /// `row_fn` receives the Stmt and the column count.
        pub fn query<F>(&self, sql: &str, mut row_fn: F) -> Result<(), String>
        where
            F: FnMut(&Stmt, c_int),
        {
            let stmt = self.prepare(sql)?;
            loop {
                let rc = unsafe { ffi::sqlite3_step(stmt.0) };
                match rc {
                    r if r == ffi::SQLITE_ROW as c_int => {
                        let ncols = unsafe { ffi::sqlite3_column_count(stmt.0) };
                        row_fn(&stmt, ncols);
                    }
                    r if r == ffi::SQLITE_DONE as c_int => break,
                    r => return Err(format!("step failed ({})", r)),
                }
            }
            Ok(())
        }
    }

    impl Drop for Conn {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ffi::sqlite3_close(self.0) };
            }
        }
    }

    impl Drop for Stmt {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ffi::sqlite3_finalize(self.0) };
            }
        }
    }

    impl Stmt {
        pub fn col_text(&self, idx: c_int) -> String {
            let ptr = unsafe { ffi::sqlite3_column_text(self.0, idx) };
            if ptr.is_null() {
                return String::new();
            }
            unsafe { CStr::from_ptr(ptr as *const c_char) }
                .to_string_lossy()
                .into_owned()
        }

        pub fn col_text_opt(&self, idx: c_int) -> Option<String> {
            let t = self.col_text(idx);
            if t.is_empty() { None } else { Some(t) }
        }

        #[allow(dead_code)]
        pub fn col_type(&self, idx: c_int) -> c_int {
            unsafe { ffi::sqlite3_column_type(self.0, idx) }
        }
    }

    fn sqlite_errmsg(db: *mut ffi::sqlite3) -> String {
        if db.is_null() {
            return "null db".to_string();
        }
        let ptr = unsafe { ffi::sqlite3_errmsg(db) };
        if ptr.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

// ─── Yaak-specific parsing ───────────────────────────────────────────────────

fn import_yaak_sqlite(db_path: &std::path::Path) -> Result<YaakImportResult, String> {
    use sqlite_raw::Conn;
    use std::collections::HashMap;

    let conn = Conn::open_readonly(db_path)?;
    let mut warnings: Vec<String> = Vec::new();

    // ── 1. Workspaces ───────────────────────────────────────────────────────
    let mut workspaces: Vec<HttpWorkspace> = Vec::new();
    conn.query(
        "SELECT id, name FROM workspaces WHERE deleted_at IS NULL ORDER BY name",
        |row, _| {
            workspaces.push(HttpWorkspace {
                id: row.col_text(0),
                name: row.col_text(1),
                requests: Vec::new(),
                folders: Vec::new(),
                environments: Vec::new(),
            });
        },
    )?;

    // ── 2. Environments ──────────────────────────────────────────────────────
    let mut env_map: HashMap<String, Vec<YaakEnvironment>> = HashMap::new();
    conn.query(
        "SELECT id, workspace_id, name, variables FROM environments WHERE deleted_at IS NULL",
        |row, _| {
            let ws_id = row.col_text(1);
            let vars_json = row.col_text(3);
            let variables = parse_yaak_kv_json(&vars_json)
                .into_iter()
                .map(|(k, v, _)| (k, v))
                .collect();
            env_map.entry(ws_id).or_default().push(YaakEnvironment {
                id: row.col_text(0),
                name: row.col_text(2),
                variables,
            });
        },
    )?;

    // ── 3. Folders ───────────────────────────────────────────────────────────
    // folder_id = parent folder id (nullable), workspace_id = workspace
    let mut flat_folders: HashMap<String, HttpFolder> = HashMap::new();
    // (folder_id → workspace_id) for tree building
    let mut folder_ws: HashMap<String, String> = HashMap::new();
    // (folder_id → parent_folder_id)
    let mut folder_parent: HashMap<String, Option<String>> = HashMap::new();

    conn.query(
        "SELECT id, workspace_id, folder_id, name FROM folders \
         WHERE deleted_at IS NULL ORDER BY sort_priority ASC",
        |row, _| {
            let id = row.col_text(0);
            let ws_id = row.col_text(1);
            let parent_folder_id = row.col_text_opt(2);
            let name = row.col_text(3);
            folder_ws.insert(id.clone(), ws_id);
            folder_parent.insert(id.clone(), parent_folder_id.clone());
            flat_folders.insert(
                id.clone(),
                HttpFolder {
                    id,
                    name,
                    parent_folder_id,
                    requests: Vec::new(),
                    children: Vec::new(),
                },
            );
        },
    )?;

    // ── 4. HTTP Requests ─────────────────────────────────────────────────────
    let mut total_requests = 0usize;
    // (workspace_id → (folder_id_opt → Vec<SavedRequest>))
    let mut req_map: HashMap<String, Vec<SavedRequest>> = HashMap::new(); // workspace_id → all requests

    conn.query(
        "SELECT id, workspace_id, name, url, method, body_type, body, \
                headers, url_parameters, authentication_type, authentication, \
                folder_id, description \
         FROM http_requests \
         WHERE deleted_at IS NULL \
         ORDER BY sort_priority ASC",
        |row, _| {
            let req = parse_yaak_request(row, &mut warnings);
            req_map
                .entry(req.workspace_id.clone())
                .or_default()
                .push(req);
            total_requests += 1;
        },
    )?;

    // ── 5. Assemble tree ─────────────────────────────────────────────────────
    for ws in &mut workspaces {
        // Attach environments
        if let Some(envs) = env_map.remove(&ws.id) {
            ws.environments = envs;
        }

        // Split requests: folder vs top-level
        let ws_requests = req_map.remove(&ws.id).unwrap_or_default();
        for req in ws_requests {
            if let Some(ref fid) = req.folder_id {
                if let Some(folder) = flat_folders.get_mut(fid) {
                    folder.requests.push(req);
                } else {
                    // Folder not found — promote to top-level
                    warnings.push(format!(
                        "Request '{}' references unknown folder '{}'; promoted to top-level",
                        req.name, fid
                    ));
                    ws.requests.push(req);
                }
            } else {
                ws.requests.push(req);
            }
        }

        // Build folder tree: collect root folders for this workspace
        let root_folder_ids: Vec<String> = folder_parent
            .iter()
            .filter(|(fid, parent)| {
                folder_ws.get(*fid).map(|w| w == &ws.id).unwrap_or(false)
                    && parent.is_none()
            })
            .map(|(fid, _)| fid.clone())
            .collect();

        for root_id in &root_folder_ids {
            if let Some(folder) = flat_folders.remove(root_id) {
                let built = build_folder_tree(folder, &mut flat_folders, &folder_parent, &ws.id);
                ws.folders.push(built);
            }
        }
    }

    Ok(YaakImportResult {
        workspaces,
        total_requests,
        warnings,
    })
}

/// Recursively build a folder tree by draining `flat_folders`.
fn build_folder_tree(
    mut folder: HttpFolder,
    flat_folders: &mut std::collections::HashMap<String, HttpFolder>,
    folder_parent: &std::collections::HashMap<String, Option<String>>,
    _workspace_id: &str,
) -> HttpFolder {
    // Find child folders
    let child_ids: Vec<String> = folder_parent
        .iter()
        .filter(|(_, p)| p.as_deref() == Some(&folder.id))
        .map(|(id, _)| id.clone())
        .filter(|id| flat_folders.contains_key(id.as_str()))
        .collect();

    for child_id in child_ids {
        if let Some(child) = flat_folders.remove(&child_id) {
            let built = build_folder_tree(child, flat_folders, folder_parent, _workspace_id);
            folder.children.push(built);
        }
    }
    folder
}

/// Parse one row from `http_requests` into a `SavedRequest`.
fn parse_yaak_request(
    row: &sqlite_raw::Stmt,
    _warnings: &mut Vec<String>,
) -> SavedRequest {
    let id = row.col_text(0);
    let workspace_id = row.col_text(1);
    let name = row.col_text(2);
    let url = row.col_text(3);
    let method_str = row.col_text(4);
    let body_type_str = row.col_text(5);
    let body_json = row.col_text(6);
    let headers_json = row.col_text(7);
    let params_json = row.col_text(8);
    let auth_type_str = row.col_text(9);
    let auth_json = row.col_text(10);
    let folder_id = row.col_text_opt(11);
    let description = row.col_text(12);

    let method = parse_method(&method_str);
    let (body_type, body_text, form_data) = parse_body(&body_type_str, &body_json);
    let params = parse_yaak_kv_json(&params_json);
    let headers = parse_yaak_kv_json(&headers_json);
    let (auth_type, bearer_token, basic_user, basic_pass, api_key_name, api_key_value, api_key_in_header) =
        parse_auth(&auth_type_str, &auth_json);

    SavedRequest {
        id,
        workspace_id,
        folder_id,
        name,
        url,
        method,
        params,
        headers,
        body_type,
        body_text,
        form_data,
        auth_type,
        bearer_token,
        basic_user,
        basic_pass,
        api_key_name,
        api_key_value,
        api_key_in_header,
        description,
    }
}

// ─── Field parsers ───────────────────────────────────────────────────────────

fn parse_method(s: &str) -> HttpMethod {
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

/// Parse Yaak `body_type` + `body` JSON into (HttpBodyType, body_text, form_data).
fn parse_body(
    body_type_str: &str,
    body_json: &str,
) -> (HttpBodyType, String, Vec<(String, String, bool)>) {
    let body_type = match body_type_str {
        "application/json" => HttpBodyType::Json,
        "application/xml" | "text/xml" => HttpBodyType::Xml,
        "application/graphql" | "graphql" => HttpBodyType::GraphQL,
        "text/plain" => HttpBodyType::OtherText,
        "application/x-www-form-urlencoded" | "url_encoded" => HttpBodyType::UrlEncoded,
        "multipart/form-data" => HttpBodyType::MultiPart,
        "binary" => HttpBodyType::BinaryFile,
        "" => HttpBodyType::NoBody,
        _ => HttpBodyType::NoBody,
    };

    match body_type {
        HttpBodyType::Json | HttpBodyType::Xml | HttpBodyType::GraphQL | HttpBodyType::OtherText => {
            // Yaak stores text body as: {"text": "..."} or raw string
            let text = extract_json_text_field(body_json);
            (body_type, text, default_form_data())
        }
        HttpBodyType::UrlEncoded | HttpBodyType::MultiPart => {
            // Yaak stores form as: {"form": [{name, value, enabled, id}]}
            let form = parse_yaak_form_json(body_json);
            (body_type, String::new(), form)
        }
        _ => (body_type, String::new(), default_form_data()),
    }
}

fn default_form_data() -> Vec<(String, String, bool)> {
    vec![("".to_string(), "".to_string(), true)]
}

/// Extract `text` field from Yaak body JSON: `{"text": "..."}` or raw string.
fn extract_json_text_field(json: &str) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(s) = val.get("text").and_then(|v| v.as_str()) {
            return s.to_string();
        }
        // If it's not an object with "text", try to return the raw string
        if let Some(s) = val.as_str() {
            return s.to_string();
        }
    }
    String::new()
}

/// Parse Yaak form body: `{"form": [{name, value, enabled, id}]}`.
fn parse_yaak_form_json(json: &str) -> Vec<(String, String, bool)> {
    let val: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let arr = val
        .get("form")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut result: Vec<(String, String, bool)> = arr
        .iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = item
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let enabled = item
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            (name, value, enabled)
        })
        .collect();

    // Always keep one trailing empty row
    result.push(("".to_string(), "".to_string(), true));
    result
}

/// Parse Yaak KV JSON arrays: `[{name, value, enabled, id}]`.
/// Used for both headers and url_parameters.
fn parse_yaak_kv_json(json: &str) -> Vec<(String, String, bool)> {
    let arr: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    let mut result: Vec<(String, String, bool)> = arr
        .iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = item
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let enabled = item
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            (name, value, enabled)
        })
        .collect();

    // Always keep at least one blank row
    result.push(("".to_string(), "".to_string(), true));
    result
}

/// Parse Yaak authentication fields.
fn parse_auth(
    auth_type_str: &str,
    auth_json: &str,
) -> (HttpAuthType, String, String, String, String, String, bool) {
    let val: serde_json::Value = serde_json::from_str(auth_json).unwrap_or_default();

    let auth_type = match auth_type_str {
        "bearer" => HttpAuthType::BearerToken,
        "basic" => HttpAuthType::BasicAuth,
        "apiKey" | "api_key" => HttpAuthType::ApiKey,
        _ => HttpAuthType::NoAuth,
    };

    let bearer_token = val
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let basic_user = val
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let basic_pass = val
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let api_key_name = val
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let api_key_value = val
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let api_key_in_header = val
        .get("addTo")
        .and_then(|v| v.as_str())
        .map(|s| s != "query_params")
        .unwrap_or(true);

    (
        auth_type,
        bearer_token,
        basic_user,
        basic_pass,
        api_key_name,
        api_key_value,
        api_key_in_header,
    )
}

// ─── Postman Import ──────────────────────────────────────────────────────────

/// Result returned from a Postman import attempt.
#[derive(Debug, Clone)]
pub struct PostmanImportResult {
    pub workspaces: Vec<HttpWorkspace>,
    pub total_requests: usize,
    pub warnings: Vec<String>,
}

/// Import workspaces/collections and requests from a Postman JSON file (Collection or Environment).
pub fn import_from_postman(file_path: &std::path::Path) -> Result<PostmanImportResult, String> {
    let json_str = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read Postman file: {}", e))?;
    import_postman_json(&json_str)
}

/// Parse Postman Collection v2.0/v2.1 or Postman Environment JSON string.
pub fn import_postman_json(json_str: &str) -> Result<PostmanImportResult, String> {
    let val: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("Invalid JSON format: {}", e))?;

    let mut warnings = Vec::new();

    // Check if it's a Postman Collection (has `info` or `item` or `_postman_id`)
    if val.get("info").is_some() || val.get("item").is_some() {
        return parse_postman_collection(&val, &mut warnings);
    }

    // Check if it's a Postman Environment (has `name` and `values` array)
    if val.get("values").and_then(|v| v.as_array()).is_some() {
        return parse_postman_environment(&val, &mut warnings);
    }

    Err("JSON structure is not recognized as a Postman Collection or Environment".to_string())
}

fn parse_postman_collection(
    val: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> Result<PostmanImportResult, String> {
    let info = val.get("info");
    let name = info
        .and_then(|i| i.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("Imported Postman Collection")
        .to_string();

    let ws_id = format!("pm_ws_{}", chrono::Utc::now().timestamp_millis());

    // Parse collection variables if present
    let mut environments = Vec::new();
    if let Some(vars) = val.get("variable").and_then(|v| v.as_array()) {
        let mut key_values = Vec::new();
        for v in vars {
            let k = v.get("key").and_then(|x| x.as_str()).unwrap_or("");
            let val_str = v.get("value").and_then(|x| x.as_str()).unwrap_or("");
            if !k.is_empty() {
                key_values.push((k.to_string(), val_str.to_string()));
            }
        }
        if !key_values.is_empty() {
            environments.push(YaakEnvironment {
                id: format!("pm_env_{}", chrono::Utc::now().timestamp_millis()),
                name: format!("{} Variables", name),
                variables: key_values,
            });
        }
    }

    let items = val
        .get("item")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();

    let mut requests = Vec::new();
    let mut folders = Vec::new();
    let mut total_requests = 0usize;

    for item in items {
        parse_postman_item(
            &item,
            &ws_id,
            None,
            &mut requests,
            &mut folders,
            &mut total_requests,
            warnings,
        );
    }

    let workspace = HttpWorkspace {
        id: ws_id,
        name,
        requests,
        folders,
        environments,
    };

    Ok(PostmanImportResult {
        workspaces: vec![workspace],
        total_requests,
        warnings: warnings.clone(),
    })
}

fn parse_postman_item(
    item: &serde_json::Value,
    ws_id: &str,
    parent_folder_id: Option<String>,
    parent_requests: &mut Vec<SavedRequest>,
    parent_folders: &mut Vec<HttpFolder>,
    total_requests: &mut usize,
    warnings: &mut Vec<String>,
) {
    let name = item
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Untitled")
        .to_string();

    if let Some(sub_items) = item.get("item").and_then(|i| i.as_array()) {
        // It's a folder
        let folder_id = format!("pm_fld_{}_{}", total_requests, rand_id());
        let mut child_requests = Vec::new();
        let mut child_folders = Vec::new();

        for child in sub_items {
            parse_postman_item(
                child,
                ws_id,
                Some(folder_id.clone()),
                &mut child_requests,
                &mut child_folders,
                total_requests,
                warnings,
            );
        }

        parent_folders.push(HttpFolder {
            id: folder_id,
            name,
            parent_folder_id,
            requests: child_requests,
            children: child_folders,
        });
    } else if let Some(req_val) = item.get("request") {
        // It's a request
        let req_id = format!("pm_req_{}_{}", total_requests, rand_id());
        let saved_req = parse_postman_request(req_id, ws_id, parent_folder_id, name, req_val, warnings);
        parent_requests.push(saved_req);
        *total_requests += 1;
    }
}

fn rand_id() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn parse_postman_request(
    id: String,
    workspace_id: &str,
    folder_id: Option<String>,
    name: String,
    req_val: &serde_json::Value,
    _warnings: &mut Vec<String>,
) -> SavedRequest {
    if let Some(url_str) = req_val.as_str() {
        return SavedRequest {
            id,
            workspace_id: workspace_id.to_string(),
            folder_id,
            name,
            url: url_str.to_string(),
            method: HttpMethod::GET,
            params: vec![("".to_string(), "".to_string(), true)],
            headers: vec![("".to_string(), "".to_string(), true)],
            body_type: HttpBodyType::NoBody,
            body_text: String::new(),
            form_data: default_form_data(),
            auth_type: HttpAuthType::NoAuth,
            bearer_token: String::new(),
            basic_user: String::new(),
            basic_pass: String::new(),
            api_key_name: String::new(),
            api_key_value: String::new(),
            api_key_in_header: true,
            description: String::new(),
        };
    }

    let method_str = req_val
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("GET");
    let method = parse_method(method_str);

    let (url, params) = parse_postman_url(req_val.get("url"));
    let headers = parse_postman_headers(req_val.get("header"));
    let (body_type, body_text, form_data) = parse_postman_body(req_val.get("body"));
    let (auth_type, bearer_token, basic_user, basic_pass, api_key_name, api_key_value, api_key_in_header) =
        parse_postman_auth(req_val.get("auth"));

    let description = req_val
        .get("description")
        .and_then(|d| {
            if let Some(s) = d.as_str() {
                Some(s.to_string())
            } else {
                d.get("content").and_then(|c| c.as_str()).map(|s| s.to_string())
            }
        })
        .unwrap_or_default();

    SavedRequest {
        id,
        workspace_id: workspace_id.to_string(),
        folder_id,
        name,
        url,
        method,
        params,
        headers,
        body_type,
        body_text,
        form_data,
        auth_type,
        bearer_token,
        basic_user,
        basic_pass,
        api_key_name,
        api_key_value,
        api_key_in_header,
        description,
    }
}

fn parse_postman_url(url_val: Option<&serde_json::Value>) -> (String, Vec<(String, String, bool)>) {
    let Some(val) = url_val else {
        return (String::new(), vec![("".to_string(), "".to_string(), true)]);
    };

    let mut params = Vec::new();

    if let Some(s) = val.as_str() {
        return (s.to_string(), vec![("".to_string(), "".to_string(), true)]);
    }

    let url_raw = val
        .get("raw")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(query_arr) = val.get("query").and_then(|q| q.as_array()) {
        for q in query_arr {
            let key = q.get("key").and_then(|k| k.as_str()).unwrap_or("").to_string();
            let value = q.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let enabled = q
                .get("disabled")
                .and_then(|d| d.as_bool())
                .map(|disabled| !disabled)
                .unwrap_or(true);
            if !key.is_empty() || !value.is_empty() {
                params.push((key, value, enabled));
            }
        }
    }

    params.push(("".to_string(), "".to_string(), true));

    (url_raw, params)
}

fn parse_postman_headers(header_val: Option<&serde_json::Value>) -> Vec<(String, String, bool)> {
    let mut headers = Vec::new();
    if let Some(arr) = header_val.and_then(|h| h.as_array()) {
        for item in arr {
            let key = item.get("key").and_then(|k| k.as_str()).unwrap_or("").to_string();
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let enabled = item
                .get("disabled")
                .and_then(|d| d.as_bool())
                .map(|disabled| !disabled)
                .unwrap_or(true);
            if !key.is_empty() || !value.is_empty() {
                headers.push((key, value, enabled));
            }
        }
    }
    headers.push(("".to_string(), "".to_string(), true));
    headers
}

fn parse_postman_body(
    body_val: Option<&serde_json::Value>,
) -> (HttpBodyType, String, Vec<(String, String, bool)>) {
    let Some(val) = body_val else {
        return (HttpBodyType::NoBody, String::new(), default_form_data());
    };

    let mode = val.get("mode").and_then(|m| m.as_str()).unwrap_or("");
    match mode {
        "raw" => {
            let raw_text = val.get("raw").and_then(|r| r.as_str()).unwrap_or("").to_string();
            let lang = val
                .get("options")
                .and_then(|o| o.get("raw"))
                .and_then(|r| r.get("language"))
                .and_then(|l| l.as_str())
                .unwrap_or("");
            let body_type = match lang {
                "json" => HttpBodyType::Json,
                "xml" => HttpBodyType::Xml,
                "html" | "text" => HttpBodyType::OtherText,
                _ => {
                    let trimmed = raw_text.trim();
                    if trimmed.starts_with('{') || trimmed.starts_with('[') {
                        HttpBodyType::Json
                    } else if trimmed.starts_with('<') {
                        HttpBodyType::Xml
                    } else {
                        HttpBodyType::OtherText
                    }
                }
            };
            (body_type, raw_text, default_form_data())
        }
        "urlencoded" => {
            let mut form = Vec::new();
            if let Some(arr) = val.get("urlencoded").and_then(|u| u.as_array()) {
                for item in arr {
                    let k = item.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let v = item.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let enabled = item
                        .get("disabled")
                        .and_then(|d| d.as_bool())
                        .map(|disabled| !disabled)
                        .unwrap_or(true);
                    form.push((k, v, enabled));
                }
            }
            form.push(("".to_string(), "".to_string(), true));
            (HttpBodyType::UrlEncoded, String::new(), form)
        }
        "formdata" => {
            let mut form = Vec::new();
            if let Some(arr) = val.get("formdata").and_then(|f| f.as_array()) {
                for item in arr {
                    let k = item.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let v = item.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let enabled = item
                        .get("disabled")
                        .and_then(|d| d.as_bool())
                        .map(|disabled| !disabled)
                        .unwrap_or(true);
                    form.push((k, v, enabled));
                }
            }
            form.push(("".to_string(), "".to_string(), true));
            (HttpBodyType::MultiPart, String::new(), form)
        }
        "graphql" => {
            let query = val
                .get("graphql")
                .and_then(|g| g.get("query"))
                .and_then(|q| q.as_str())
                .unwrap_or("")
                .to_string();
            (HttpBodyType::GraphQL, query, default_form_data())
        }
        _ => (HttpBodyType::NoBody, String::new(), default_form_data()),
    }
}

fn parse_postman_auth(
    auth_val: Option<&serde_json::Value>,
) -> (HttpAuthType, String, String, String, String, String, bool) {
    let Some(val) = auth_val else {
        return (
            HttpAuthType::NoAuth,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            true,
        );
    };

    let auth_type_str = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match auth_type_str {
        "bearer" => {
            let mut token = String::new();
            if let Some(arr) = val.get("bearer").and_then(|b| b.as_array()) {
                for item in arr {
                    if item.get("key").and_then(|k| k.as_str()) == Some("token") {
                        token = item.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    }
                }
            }
            (
                HttpAuthType::BearerToken,
                token,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                true,
            )
        }
        "basic" => {
            let mut user = String::new();
            let mut pass = String::new();
            if let Some(arr) = val.get("basic").and_then(|b| b.as_array()) {
                for item in arr {
                    let k = item.get("key").and_then(|x| x.as_str());
                    let v = item.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if k == Some("username") {
                        user = v;
                    } else if k == Some("password") {
                        pass = v;
                    }
                }
            }
            (
                HttpAuthType::BasicAuth,
                String::new(),
                user,
                pass,
                String::new(),
                String::new(),
                true,
            )
        }
        "apikey" => {
            let mut key_name = String::new();
            let mut key_val = String::new();
            let mut in_header = true;
            if let Some(arr) = val.get("apikey").and_then(|a| a.as_array()) {
                for item in arr {
                    let k = item.get("key").and_then(|x| x.as_str());
                    let v = item.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if k == Some("key") {
                        key_name = v;
                    } else if k == Some("value") {
                        key_val = v;
                    } else if k == Some("in") {
                        in_header = v != "query";
                    }
                }
            }
            (
                HttpAuthType::ApiKey,
                String::new(),
                String::new(),
                String::new(),
                key_name,
                key_val,
                in_header,
            )
        }
        _ => (
            HttpAuthType::NoAuth,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            true,
        ),
    }
}

#[allow(clippy::ptr_arg)]
fn parse_postman_environment(
    val: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> Result<PostmanImportResult, String> {
    let name = val
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Imported Postman Environment")
        .to_string();

    let mut variables = Vec::new();
    if let Some(arr) = val.get("values").and_then(|v| v.as_array()) {
        for item in arr {
            let key = item.get("key").and_then(|k| k.as_str()).unwrap_or("");
            let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let enabled = item.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true);
            if enabled && !key.is_empty() {
                variables.push((key.to_string(), value.to_string()));
            }
        }
    }

    let ws_id = format!("pm_env_ws_{}", chrono::Utc::now().timestamp_millis());
    let env = YaakEnvironment {
        id: format!("pm_env_{}", chrono::Utc::now().timestamp_millis()),
        name: name.clone(),
        variables,
    };

    let workspace = HttpWorkspace {
        id: ws_id,
        name: format!("{} (Environment)", name),
        requests: Vec::new(),
        folders: Vec::new(),
        environments: vec![env],
    };

    Ok(PostmanImportResult {
        workspaces: vec![workspace],
        total_requests: 0,
        warnings: warnings.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_postman_collection_v21() {
        let json = r#"{
            "info": {
                "_postman_id": "12345",
                "name": "Test Postman Collection",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [
                {
                    "name": "User Folder",
                    "item": [
                        {
                            "name": "Get Users",
                            "request": {
                                "method": "GET",
                                "header": [
                                    { "key": "Accept", "value": "application/json" }
                                ],
                                "url": {
                                    "raw": "https://api.example.com/users?page=1",
                                    "query": [
                                        { "key": "page", "value": "1" }
                                    ]
                                }
                            }
                        }
                    ]
                },
                {
                    "name": "Create User",
                    "request": {
                        "method": "POST",
                        "header": [
                            { "key": "Content-Type", "value": "application/json" }
                        ],
                        "body": {
                            "mode": "raw",
                            "raw": "{\"name\": \"John\"}",
                            "options": { "raw": { "language": "json" } }
                        },
                        "url": "https://api.example.com/users",
                        "auth": {
                            "type": "bearer",
                            "bearer": [
                                { "key": "token", "value": "secret-jwt-token" }
                            ]
                        }
                    }
                }
            ]
        }"#;

        let res = import_postman_json(json).expect("Failed to parse Postman collection");
        assert_eq!(res.workspaces.len(), 1);
        let ws = &res.workspaces[0];
        assert_eq!(ws.name, "Test Postman Collection");
        assert_eq!(res.total_requests, 2);
        assert_eq!(ws.requests.len(), 1);
        assert_eq!(ws.folders.len(), 1);

        let create_req = &ws.requests[0];
        assert_eq!(create_req.name, "Create User");
        assert_eq!(create_req.method, HttpMethod::POST);
        assert_eq!(create_req.body_type, HttpBodyType::Json);
        assert_eq!(create_req.body_text, "{\"name\": \"John\"}");
        assert_eq!(create_req.auth_type, HttpAuthType::BearerToken);
        assert_eq!(create_req.bearer_token, "secret-jwt-token");

        let folder = &ws.folders[0];
        assert_eq!(folder.name, "User Folder");
        assert_eq!(folder.requests.len(), 1);
        let get_req = &folder.requests[0];
        assert_eq!(get_req.name, "Get Users");
        assert_eq!(get_req.method, HttpMethod::GET);
        assert_eq!(get_req.url, "https://api.example.com/users?page=1");
    }

    #[test]
    fn test_import_postman_environment() {
        let json = r#"{
            "id": "env-123",
            "name": "Staging Environment",
            "values": [
                { "key": "baseUrl", "value": "https://staging.example.com", "enabled": true },
                { "key": "apiKey", "value": "12345", "enabled": true }
            ]
        }"#;

        let res = import_postman_json(json).expect("Failed to parse Postman environment");
        assert_eq!(res.workspaces.len(), 1);
        let ws = &res.workspaces[0];
        assert_eq!(ws.environments.len(), 1);
        let env = &ws.environments[0];
        assert_eq!(env.name, "Staging Environment");
        assert_eq!(env.variables.len(), 2);
        assert_eq!(env.variables[0], ("baseUrl".to_string(), "https://staging.example.com".to_string()));
    }

    #[test]
    fn test_extract_endpoint_url() {
        assert_eq!(extract_endpoint_url("https://api.example.com/v1/users/profile?query=1#ref"), "/v1/users/profile");
        assert_eq!(extract_endpoint_url("http://localhost:8080/api/v1/orders"), "/api/v1/orders");
        assert_eq!(extract_endpoint_url("https://api.example.com"), "/");
        assert_eq!(extract_endpoint_url("https://api.example.com/"), "/");
        assert_eq!(extract_endpoint_url("/v1/auth/login"), "/v1/auth/login");
        assert_eq!(extract_endpoint_url("{{base_url}}/v1/users"), "/v1/users");
        assert_eq!(extract_endpoint_url("{{base_url}}"), "/");
        assert_eq!(extract_endpoint_url(""), "Untitled Request");
        assert_eq!(extract_endpoint_url("   "), "Untitled Request");
    }

    #[test]
    fn test_saved_request_display_name() {
        let req_with_name = SavedRequest {
            name: "GetUserProfile".to_string(),
            url: "https://api.example.com/v1/users/profile".to_string(),
            ..Default::default()
        };
        assert_eq!(req_with_name.display_name(), "GetUserProfile");

        let req_without_name = SavedRequest {
            name: "".to_string(),
            url: "https://api.example.com/v1/users/profile?a=1".to_string(),
            ..Default::default()
        };
        assert_eq!(req_without_name.display_name(), "/v1/users/profile");
    }

    #[test]
    fn test_create_and_rename_folder_and_subfolder() {
        let mut workspaces = vec![HttpWorkspace {
            id: "ws-test".to_string(),
            name: "Test Workspace".to_string(),
            requests: vec![],
            folders: vec![],
            environments: vec![],
        }];

        // 1. Create root folder
        let root_folder = create_folder_in_workspace(
            &mut workspaces,
            "ws-test",
            None,
            "Auth",
        ).expect("Failed to create root folder");

        assert_eq!(root_folder.name, "Auth");
        assert_eq!(workspaces[0].folders.len(), 1);
        assert_eq!(workspaces[0].folders[0].name, "Auth");
        let root_folder_id = root_folder.id.clone();

        // 2. Create subfolder inside root folder
        let subfolder = create_folder_in_workspace(
            &mut workspaces,
            "ws-test",
            Some(&root_folder_id),
            "OAuth2",
        ).expect("Failed to create subfolder");

        assert_eq!(subfolder.name, "OAuth2");
        assert_eq!(workspaces[0].folders[0].children.len(), 1);
        assert_eq!(workspaces[0].folders[0].children[0].name, "OAuth2");
        let subfolder_id = subfolder.id.clone();

        // 3. Rename subfolder
        let renamed = rename_folder_in_workspaces(
            &mut workspaces,
            "ws-test",
            &subfolder_id,
            "OAuth2 Providers",
        );
        assert!(renamed);
        assert_eq!(workspaces[0].folders[0].children[0].name, "OAuth2 Providers");

        // 4. Rename root folder
        let renamed_root = rename_folder_in_workspaces(
            &mut workspaces,
            "ws-test",
            &root_folder_id,
            "Authentication & Security",
        );
        assert!(renamed_root);
        assert_eq!(workspaces[0].folders[0].name, "Authentication & Security");
    }

    #[test]
    fn test_create_workspace() {
        let mut workspaces = Vec::new();
        let ws = create_workspace(&mut workspaces, "New API Collection");
        assert_eq!(ws.name, "New API Collection");
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "New API Collection");
    }

    #[test]
    fn test_move_request() {
        let mut workspaces = vec![HttpWorkspace {
            id: "ws-test".to_string(),
            name: "Test Workspace".to_string(),
            requests: vec![SavedRequest {
                id: "req-1".to_string(),
                workspace_id: "ws-test".to_string(),
                name: "Get Profile".to_string(),
                ..Default::default()
            }],
            folders: vec![HttpFolder {
                id: "fld-1".to_string(),
                name: "Users".to_string(),
                parent_folder_id: None,
                requests: vec![],
                children: vec![],
            }],
            environments: vec![],
        }];

        // 1. Move request from workspace root to folder
        let moved = move_request(&mut workspaces, "req-1", "ws-test", Some("fld-1"));
        assert!(moved);
        assert_eq!(workspaces[0].requests.len(), 0);
        assert_eq!(workspaces[0].folders[0].requests.len(), 1);
        assert_eq!(workspaces[0].folders[0].requests[0].id, "req-1");
        assert_eq!(workspaces[0].folders[0].requests[0].folder_id.as_deref(), Some("fld-1"));

        // 2. Move request back from folder to workspace root
        let moved_back = move_request(&mut workspaces, "req-1", "ws-test", None);
        assert!(moved_back);
        assert_eq!(workspaces[0].requests.len(), 1);
        assert_eq!(workspaces[0].folders[0].requests.len(), 0);
        assert_eq!(workspaces[0].requests[0].id, "req-1");
        assert_eq!(workspaces[0].requests[0].folder_id, None);
    }

    #[test]
    fn test_move_folder_and_prevent_cycles() {
        let mut workspaces = vec![HttpWorkspace {
            id: "ws-test".to_string(),
            name: "Test Workspace".to_string(),
            requests: vec![],
            folders: vec![
                HttpFolder {
                    id: "fld-parent".to_string(),
                    name: "Parent Folder".to_string(),
                    parent_folder_id: None,
                    requests: vec![],
                    children: vec![HttpFolder {
                        id: "fld-child".to_string(),
                        name: "Child Folder".to_string(),
                        parent_folder_id: Some("fld-parent".to_string()),
                        requests: vec![],
                        children: vec![],
                    }],
                },
                HttpFolder {
                    id: "fld-sibling".to_string(),
                    name: "Sibling Folder".to_string(),
                    parent_folder_id: None,
                    requests: vec![],
                    children: vec![],
                },
            ],
            environments: vec![],
        }];

        // 1. Moving folder into itself must fail
        assert!(!move_folder(&mut workspaces, "fld-parent", "ws-test", Some("fld-parent")));

        // 2. Moving parent folder into its descendant must fail
        assert!(!move_folder(&mut workspaces, "fld-parent", "ws-test", Some("fld-child")));

        // 3. Moving child folder to sibling folder must succeed
        let moved = move_folder(&mut workspaces, "fld-child", "ws-test", Some("fld-sibling"));
        assert!(moved);
        assert_eq!(workspaces[0].folders[0].children.len(), 0);
        assert_eq!(workspaces[0].folders[1].children.len(), 1);
        assert_eq!(workspaces[0].folders[1].children[0].id, "fld-child");

        // 4. Moving child folder to workspace root must succeed
        let moved_root = move_folder(&mut workspaces, "fld-child", "ws-test", None);
        assert!(moved_root);
        assert_eq!(workspaces[0].folders.len(), 3);
        assert_eq!(workspaces[0].folders[2].id, "fld-child");
        assert_eq!(workspaces[0].folders[2].parent_folder_id, None);
    }
}



