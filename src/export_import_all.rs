use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use log::info;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::models::structs::{ConnectionConfig, HistoryItem};
use crate::window_egui::Tabular;

// ─── Error Types ─────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum ExportImportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Invalid archive: {0}")]
    InvalidArchive(String),
    #[error("Zip slip path traversal detected: {0}")]
    ZipSlip(String),
    #[error("No active database pool available")]
    NoDatabasePool,
}

// ─── Manifest & Metadata Models ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportAllManifest {
    pub version: String,
    pub app: String,
    pub exported_at: String,
    pub counts: ExportCounts,
    pub includes: ExportIncludes,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportCounts {
    pub connections: usize,
    pub connection_folders: usize,
    pub queries: usize,
    pub http_workspaces: usize,
    pub history_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportIncludes {
    pub connections: bool,
    pub queries: bool,
    pub http_api: bool,
    pub history: bool,
}

// ─── Options & Strategies ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConflictStrategy {
    #[default]
    MergeKeepExisting,
    MergeOverwrite,
    CleanRestore,
}

impl ConflictStrategy {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::MergeKeepExisting => "Merge (Keep Existing)",
            Self::MergeOverwrite => "Merge (Overwrite Existing)",
            Self::CleanRestore => "Clean Restore (Replace All)",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::MergeKeepExisting => "Adds new data from archive without modifying items that already exist.",
            Self::MergeOverwrite => "Updates existing data with archive versions and adds new data.",
            Self::CleanRestore => "Wipes existing connections, queries, HTTP APIs, and history before restoring.",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportAllOptions {
    pub include_connections: bool,
    pub include_queries: bool,
    pub include_http_api: bool,
    pub include_history: bool,
}

impl Default for ExportAllOptions {
    fn default() -> Self {
        Self {
            include_connections: true,
            include_queries: true,
            include_http_api: true,
            include_history: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportAllOptions {
    pub include_connections: bool,
    pub include_queries: bool,
    pub include_http_api: bool,
    pub include_history: bool,
    pub conflict_strategy: ConflictStrategy,
}

impl Default for ImportAllOptions {
    fn default() -> Self {
        Self {
            include_connections: true,
            include_queries: true,
            include_http_api: true,
            include_history: true,
            conflict_strategy: ConflictStrategy::default(),
        }
    }
}

// ─── Summary Reports ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ExportSummary {
    pub connections_count: usize,
    pub folders_count: usize,
    pub queries_count: usize,
    pub http_workspaces_count: usize,
    pub history_count: usize,
    pub zip_file_size: u64,
    pub archive_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    pub connections_restored: usize,
    pub folders_restored: usize,
    pub queries_restored: usize,
    pub http_workspaces_restored: usize,
    pub history_restored: usize,
}

// ─── File Helper Functions ──────────────────────────────────────────────────

fn add_directory_recursive<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    base_dir: &Path,
    current_dir: &Path,
    prefix_in_zip: &str,
    options: SimpleFileOptions,
) -> Result<usize, ExportImportError> {
    let mut count = 0;
    if !current_dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(current_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Skip symlinks to prevent infinite recursion
        if let Ok(ft) = entry.file_type() {
            if ft.is_symlink() {
                continue;
            }
        }

        // Skip macOS metadata and temporary hidden files
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == ".DS_Store" || name.starts_with("._") {
                continue;
            }
        }

        let rel_path = path
            .strip_prefix(base_dir)
            .map_err(|e| ExportImportError::InvalidArchive(e.to_string()))?;
        let zip_rel = rel_path.to_string_lossy().replace('\\', "/");
        let zip_path = format!("{}/{}", prefix_in_zip, zip_rel);

        if path.is_dir() {
            let zip_dir = if zip_path.ends_with('/') {
                zip_path
            } else {
                format!("{}/", zip_path)
            };
            let _ = zip.add_directory(&zip_dir, options);
            count += add_directory_recursive(zip, base_dir, &path, prefix_in_zip, options)?;
        } else if path.is_file() {
            zip.start_file(&zip_path, options)?;
            let content = std::fs::read(&path)?;
            zip.write_all(&content)?;
            count += 1;
        }
    }
    Ok(count)
}

fn clear_directory_contents(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(())
}

// ─── Core Export / Import Implementation ─────────────────────────────────────

/// Core headless export function that does not block on tokio runtime or mutate Tabular.
/// Fully thread-safe and safe to execute in a background thread.
pub fn export_all_data_payload(
    target_path: &Path,
    options: &ExportAllOptions,
    connections: &[ConnectionConfig],
    connection_folders: &[String],
    yaak_workspaces: &[crate::http_collection::HttpWorkspace],
    history_items: &[HistoryItem],
) -> Result<ExportSummary, ExportImportError> {
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = File::create(target_path)?;
    let mut zip = ZipWriter::new(file);
    let file_opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut counts = ExportCounts::default();

    // 1. Export Connections and Connection Folders
    if options.include_connections {
        let conns_json = serde_json::to_string_pretty(connections)?;
        zip.start_file("connections/connections.json", file_opts)?;
        zip.write_all(conns_json.as_bytes())?;
        counts.connections = connections.len();

        let folders_json = serde_json::to_string_pretty(connection_folders)?;
        zip.start_file("connections/folders.json", file_opts)?;
        zip.write_all(folders_json.as_bytes())?;
        counts.connection_folders = connection_folders.len();
    }

    // 2. Export Saved Queries
    if options.include_queries {
        let query_dir = crate::directory::get_query_dir();
        if query_dir.exists() {
            let count = add_directory_recursive(&mut zip, &query_dir, &query_dir, "queries", file_opts)?;
            counts.queries = count;
        }
    }

    // 3. Export HTTP API Collections & Workspaces
    if options.include_http_api {
        let http_dir = crate::directory::get_app_data_dir().join("http_collections");
        let mut exported_ids = std::collections::HashSet::new();

        if http_dir.exists() {
            let count = add_directory_recursive(&mut zip, &http_dir, &http_dir, "http_collections", file_opts)?;
            counts.http_workspaces = count;
            if let Ok(entries) = std::fs::read_dir(&http_dir) {
                for e in entries.flatten() {
                    if let Some(stem) = e.path().file_stem().and_then(|s| s.to_str()) {
                        exported_ids.insert(stem.to_string());
                    }
                }
            }
        }

        // Also ensure in-memory workspaces that haven't hit disk are written
        for ws in yaak_workspaces {
            if !exported_ids.contains(&ws.id) {
                let zip_path = format!("http_collections/{}.json", ws.id);
                zip.start_file(&zip_path, file_opts)?;
                let json = serde_json::to_string_pretty(ws)?;
                zip.write_all(json.as_bytes())?;
                counts.http_workspaces += 1;
            }
        }
    }

    // 4. Export Query History (directly from in-memory history, zero deadlock risk)
    if options.include_history {
        let hist_json = serde_json::to_string_pretty(history_items)?;
        zip.start_file("history/history.json", file_opts)?;
        zip.write_all(hist_json.as_bytes())?;
        counts.history_items = history_items.len();
    }

    // 5. Write manifest.json
    let manifest = ExportAllManifest {
        version: "1.0".to_string(),
        app: "Tabular".to_string(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        counts: counts.clone(),
        includes: ExportIncludes {
            connections: options.include_connections,
            queries: options.include_queries,
            http_api: options.include_http_api,
            history: options.include_history,
        },
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    zip.start_file("manifest.json", file_opts)?;
    zip.write_all(manifest_json.as_bytes())?;

    zip.finish()?;

    let file_size = std::fs::metadata(target_path).map(|m| m.len()).unwrap_or(0);

    info!(
        "📦 Export All Data completed: {} connections, {} folders, {} queries, {} HTTP workspaces, {} history items ({:.2} KB)",
        counts.connections,
        counts.connection_folders,
        counts.queries,
        counts.http_workspaces,
        counts.history_items,
        file_size as f64 / 1024.0
    );

    Ok(ExportSummary {
        connections_count: counts.connections,
        folders_count: counts.connection_folders,
        queries_count: counts.queries,
        http_workspaces_count: counts.http_workspaces,
        history_count: counts.history_items,
        zip_file_size: file_size,
        archive_path: target_path.to_path_buf(),
    })
}

/// Export all specified application data into a ZIP archive file at `target_path`.
pub fn export_all_data(
    tabular: &mut Tabular,
    target_path: &Path,
    options: &ExportAllOptions,
) -> Result<ExportSummary, ExportImportError> {
    export_all_data_payload(
        target_path,
        options,
        &tabular.connections,
        &tabular.connection_folders,
        &tabular.yaak_workspaces,
        &tabular.history_items,
    )
}

/// Inspect an archive without restoring it. Returns manifest metadata and item counts.
pub fn inspect_archive(archive_path: &Path) -> Result<ExportAllManifest, ExportImportError> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;

    // Zip slip validation on inspection
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if entry.enclosed_name().is_none() {
            return Err(ExportImportError::ZipSlip(entry.name().to_string()));
        }
    }

    // Check if manifest.json is present
    if let Ok(mut manifest_file) = archive.by_name("manifest.json") {
        let mut content = String::new();
        manifest_file.read_to_string(&mut content)?;
        if let Ok(manifest) = serde_json::from_str::<ExportAllManifest>(&content) {
            return Ok(manifest);
        }
    }

    // Synthesize manifest if manifest.json was missing or unparseable
    let mut counts = ExportCounts::default();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if name.starts_with("connections/connections.json") {
            // Count connections from JSON content
            let mut c = Vec::new();
            let mut reader = entry;
            let _ = reader.read_to_end(&mut c);
            if let Ok(conns) = serde_json::from_slice::<Vec<ConnectionConfig>>(&c) {
                counts.connections = conns.len();
            }
        } else if name.starts_with("connections/folders.json") {
            let mut c = Vec::new();
            let mut reader = entry;
            let _ = reader.read_to_end(&mut c);
            if let Ok(folders) = serde_json::from_slice::<Vec<String>>(&c) {
                counts.connection_folders = folders.len();
            }
        } else if name.starts_with("queries/") && name.ends_with(".sql") {
            counts.queries += 1;
        } else if name.starts_with("http_collections/") && name.ends_with(".json") {
            counts.http_workspaces += 1;
        } else if name.starts_with("history/history.json") {
            let mut c = Vec::new();
            let mut reader = entry;
            let _ = reader.read_to_end(&mut c);
            if let Ok(items) = serde_json::from_slice::<Vec<HistoryItem>>(&c) {
                counts.history_items = items.len();
            }
        }
    }

    Ok(ExportAllManifest {
        version: "1.0".to_string(),
        app: "Tabular".to_string(),
        exported_at: String::new(),
        counts: counts.clone(),
        includes: ExportIncludes {
            connections: counts.connections > 0 || counts.connection_folders > 0,
            queries: counts.queries > 0,
            http_api: counts.http_workspaces > 0,
            history: counts.history_items > 0,
        },
    })
}

/// Import and restore application data from a ZIP archive file at `archive_path`.
pub fn import_all_data(
    tabular: &mut Tabular,
    archive_path: &Path,
    options: &ImportAllOptions,
) -> Result<ImportSummary, ExportImportError> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;

    // Zip slip verification
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if entry.enclosed_name().is_none() {
            return Err(ExportImportError::ZipSlip(entry.name().to_string()));
        }
    }

    let mut summary = ImportSummary::default();
    let mut old_id_to_new_id: HashMap<i64, i64> = HashMap::new();
    let mut name_to_new_id: HashMap<String, i64> = HashMap::new();

    let rt = tabular.get_runtime();
    let pool = tabular
        .db_pool
        .clone()
        .ok_or(ExportImportError::NoDatabasePool)?;

    // ── 1. Restore Connections & Folders ──
    if options.include_connections {
        if options.conflict_strategy == ConflictStrategy::CleanRestore {
            rt.block_on(async {
                let _ = sqlx::query("DELETE FROM connection_folders").execute(pool.as_ref()).await;
                let _ = sqlx::query("DELETE FROM connections").execute(pool.as_ref()).await;
            });
            tabular.connection_folders.clear();
            tabular.connections.clear();
        }

        // Restore Folders
        if let Ok(mut entry) = archive.by_name("connections/folders.json") {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            if let Ok(folders) = serde_json::from_slice::<Vec<String>>(&content) {
                for folder in folders {
                    let folder_clone = folder.clone();
                    let pool_clone = pool.clone();
                    let ok = rt.block_on(async {
                        sqlx::query("INSERT OR IGNORE INTO connection_folders (path) VALUES (?)")
                            .bind(&folder_clone)
                            .execute(pool_clone.as_ref())
                            .await
                    })
                    .is_ok();

                    if ok && !tabular.connection_folders.contains(&folder) {
                        tabular.connection_folders.push(folder);
                    }
                    summary.folders_restored += 1;
                }
            }
        }

        // Restore Connections
        if let Ok(mut entry) = archive.by_name("connections/connections.json") {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            if let Ok(conns) = serde_json::from_slice::<Vec<ConnectionConfig>>(&content) {
                for conn in conns {
                    let old_id = conn.id;
                    let conn_name = conn.name.clone();

                    // Check if connection with this name already exists in database
                    let pool_clone = pool.clone();
                    let check_name = conn_name.clone();
                    let existing_id: Option<i64> = rt.block_on(async {
                        sqlx::query_scalar::<_, i64>("SELECT id FROM connections WHERE name = ?")
                            .bind(&check_name)
                            .fetch_optional(pool_clone.as_ref())
                            .await
                            .unwrap_or(None)
                    });

                    match (options.conflict_strategy, existing_id) {
                        (ConflictStrategy::MergeKeepExisting, Some(eid)) => {
                            // Keep existing row, register id mappings
                            if let Some(oid) = old_id {
                                old_id_to_new_id.insert(oid, eid);
                            }
                            name_to_new_id.insert(conn_name, eid);
                        }
                        (ConflictStrategy::MergeOverwrite, Some(eid)) => {
                            // Update existing row and externalize secrets
                            let pool_clone = pool.clone();
                            let conn_clone = conn.clone();
                            let _ = rt.block_on(async {
                                sqlx::query(
                                    "UPDATE connections SET host = ?, port = ?, username = ?, password = ?, database_name = ?, connection_type = ?, folder = ?, ssh_enabled = ?, ssh_host = ?, ssh_port = ?, ssh_username = ?, ssh_auth_method = ?, ssh_private_key = ?, ssh_password = ?, ssh_accept_unknown_host_keys = ?, custom_views = ?, replication_master_id = ?, ssh_jump_host = ?, ssl_enabled = ?, ssl_ca_cert = ?, ssl_client_cert = ?, ssl_client_key = ?, ssl_key_passphrase = ?, ssl_verify_server = ? WHERE id = ?"
                                )
                                .bind(conn_clone.host)
                                .bind(conn_clone.port)
                                .bind(conn_clone.username)
                                .bind(&conn_clone.password)
                                .bind(conn_clone.database)
                                .bind(format!("{:?}", conn_clone.connection_type))
                                .bind(conn_clone.folder)
                                .bind(if conn_clone.ssh_enabled { 1 } else { 0 })
                                .bind(conn_clone.ssh_host)
                                .bind(conn_clone.ssh_port)
                                .bind(conn_clone.ssh_username)
                                .bind(conn_clone.ssh_auth_method.as_db_value())
                                .bind(&conn_clone.ssh_private_key)
                                .bind(&conn_clone.ssh_password)
                                .bind(if conn_clone.ssh_accept_unknown_host_keys { 1 } else { 0 })
                                .bind(serde_json::to_string(&conn_clone.custom_views).unwrap_or_else(|_| "[]".to_string()))
                                .bind(conn_clone.replication_master_id)
                                .bind(conn_clone.ssh_jump_host)
                                .bind(if conn_clone.ssl_enabled { 1 } else { 0 })
                                .bind(conn_clone.ssl_ca_cert)
                                .bind(conn_clone.ssl_client_cert)
                                .bind(conn_clone.ssl_client_key)
                                .bind(conn_clone.ssl_key_passphrase)
                                .bind(if conn_clone.ssl_verify_server { 1 } else { 0 })
                                .bind(eid)
                                .execute(pool_clone.as_ref())
                                .await
                            });

                            crate::sidebar_database::externalize_connection_secrets(
                                &rt,
                                &pool,
                                eid,
                                &conn.password,
                                &conn.ssh_private_key,
                                &conn.ssh_password,
                            );

                            if let Some(oid) = old_id {
                                old_id_to_new_id.insert(oid, eid);
                            }
                            name_to_new_id.insert(conn_name, eid);
                            summary.connections_restored += 1;
                        }
                        _ => {
                            // Insert as a new connection
                            let pool_clone = pool.clone();
                            let conn_clone = conn.clone();
                            let insert_result = rt.block_on(async {
                                sqlx::query(
                                    "INSERT INTO connections (name, host, port, username, password, database_name, connection_type, folder, ssh_enabled, ssh_host, ssh_port, ssh_username, ssh_auth_method, ssh_private_key, ssh_password, ssh_accept_unknown_host_keys, custom_views, replication_master_id, ssh_jump_host, ssl_enabled, ssl_ca_cert, ssl_client_cert, ssl_client_key, ssl_key_passphrase, ssl_verify_server) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                                )
                                .bind(&conn_clone.name)
                                .bind(conn_clone.host)
                                .bind(conn_clone.port)
                                .bind(conn_clone.username)
                                .bind(&conn_clone.password)
                                .bind(conn_clone.database)
                                .bind(format!("{:?}", conn_clone.connection_type))
                                .bind(conn_clone.folder)
                                .bind(if conn_clone.ssh_enabled { 1 } else { 0 })
                                .bind(conn_clone.ssh_host)
                                .bind(conn_clone.ssh_port)
                                .bind(conn_clone.ssh_username)
                                .bind(conn_clone.ssh_auth_method.as_db_value())
                                .bind(&conn_clone.ssh_private_key)
                                .bind(&conn_clone.ssh_password)
                                .bind(if conn_clone.ssh_accept_unknown_host_keys { 1 } else { 0 })
                                .bind(serde_json::to_string(&conn_clone.custom_views).unwrap_or_else(|_| "[]".to_string()))
                                .bind(conn_clone.replication_master_id)
                                .bind(conn_clone.ssh_jump_host)
                                .bind(if conn_clone.ssl_enabled { 1 } else { 0 })
                                .bind(conn_clone.ssl_ca_cert)
                                .bind(conn_clone.ssl_client_cert)
                                .bind(conn_clone.ssl_client_key)
                                .bind(conn_clone.ssl_key_passphrase)
                                .bind(if conn_clone.ssl_verify_server { 1 } else { 0 })
                                .execute(pool_clone.as_ref())
                                .await
                            });

                            if let Ok(res) = insert_result {
                                let new_id = res.last_insert_rowid();
                                crate::sidebar_database::externalize_connection_secrets(
                                    &rt,
                                    &pool,
                                    new_id,
                                    &conn.password,
                                    &conn.ssh_private_key,
                                    &conn.ssh_password,
                                );

                                if let Some(oid) = old_id {
                                    old_id_to_new_id.insert(oid, new_id);
                                }
                                name_to_new_id.insert(conn_name, new_id);
                                summary.connections_restored += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // ── 2. Restore Saved Queries ──
    if options.include_queries {
        let query_dir = crate::directory::get_query_dir();
        std::fs::create_dir_all(&query_dir)?;

        if options.conflict_strategy == ConflictStrategy::CleanRestore {
            let _ = clear_directory_contents(&query_dir);
        }

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();

            if let Some(rel) = name.strip_prefix("queries/") {
                if rel.is_empty() {
                    continue;
                }
                let target = query_dir.join(rel);

                // Zip slip defense
                if !target.starts_with(&query_dir) {
                    return Err(ExportImportError::ZipSlip(name));
                }

                if entry.is_dir() {
                    std::fs::create_dir_all(&target)?;
                } else {
                    if options.conflict_strategy == ConflictStrategy::MergeKeepExisting && target.exists() {
                        continue;
                    }
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut out = File::create(&target)?;
                    std::io::copy(&mut entry, &mut out)?;
                    summary.queries_restored += 1;
                }
            }
        }
    }

    // ── 3. Restore HTTP API Collections ──
    if options.include_http_api {
        let http_dir = crate::directory::get_app_data_dir().join("http_collections");
        std::fs::create_dir_all(&http_dir)?;

        if options.conflict_strategy == ConflictStrategy::CleanRestore {
            let _ = clear_directory_contents(&http_dir);
        }

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();

            if let Some(rel) = name.strip_prefix("http_collections/") {
                if rel.is_empty() {
                    continue;
                }
                let target = http_dir.join(rel);

                // Zip slip defense
                if !target.starts_with(&http_dir) {
                    return Err(ExportImportError::ZipSlip(name));
                }

                if entry.is_dir() {
                    std::fs::create_dir_all(&target)?;
                } else {
                    if options.conflict_strategy == ConflictStrategy::MergeKeepExisting && target.exists() {
                        continue;
                    }
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut out = File::create(&target)?;
                    std::io::copy(&mut entry, &mut out)?;
                    summary.http_workspaces_restored += 1;
                }
            }
        }
    }

    // ── 4. Restore History ──
    if options.include_history {
        if options.conflict_strategy == ConflictStrategy::CleanRestore {
            rt.block_on(async {
                let _ = sqlx::query("DELETE FROM query_history").execute(pool.as_ref()).await;
            });
            tabular.history_items.clear();
        }

        if let Ok(mut entry) = archive.by_name("history/history.json") {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            if let Ok(items) = serde_json::from_slice::<Vec<HistoryItem>>(&content) {
                // Fallback connection if needed to satisfy foreign keys
                let default_conn_id: Option<i64> = rt.block_on(async {
                    sqlx::query_scalar::<_, i64>("SELECT id FROM connections LIMIT 1")
                        .fetch_optional(pool.as_ref())
                        .await
                        .unwrap_or(None)
                });

                for item in items {
                    // Try resolving connection_id in priority order:
                    // 1. Mapped from old id
                    // 2. Mapped from connection name
                    // 3. Current connection id in DB by name
                    // 4. Current connection id in DB by ID
                    // 5. Default existing connection id in DB
                    let target_conn_id = old_id_to_new_id
                        .get(&item.connection_id)
                        .copied()
                        .or_else(|| name_to_new_id.get(&item.connection_name).copied())
                        .or_else(|| {
                            let pool_clone = pool.clone();
                            let cname = item.connection_name.clone();
                            rt.block_on(async {
                                sqlx::query_scalar::<_, i64>(
                                    "SELECT id FROM connections WHERE name = ?",
                                )
                                .bind(&cname)
                                .fetch_optional(pool_clone.as_ref())
                                .await
                                .unwrap_or(None)
                            })
                        })
                        .or_else(|| {
                            let pool_clone = pool.clone();
                            let cid = item.connection_id;
                            rt.block_on(async {
                                sqlx::query_scalar::<_, i64>(
                                    "SELECT id FROM connections WHERE id = ?",
                                )
                                .bind(cid)
                                .fetch_optional(pool_clone.as_ref())
                                .await
                                .unwrap_or(None)
                            })
                        })
                        .or(default_conn_id);

                    if let Some(resolved_conn_id) = target_conn_id {
                        let pool_clone = pool.clone();
                        let qtext = item.query.clone();
                        let cname = item.connection_name.clone();
                        let exec_at = item.executed_at.clone();

                        let res = rt.block_on(async {
                            sqlx::query(
                                "INSERT INTO query_history (query_text, connection_id, connection_name, executed_at) VALUES (?, ?, ?, ?)"
                            )
                            .bind(&qtext)
                            .bind(resolved_conn_id)
                            .bind(&cname)
                            .bind(&exec_at)
                            .execute(pool_clone.as_ref())
                            .await
                        });

                        if res.is_ok() {
                            summary.history_restored += 1;
                        }
                    }
                }
            }
        }
    }

    // ── 5. Reload Application State ──
    crate::sidebar_database::load_connections(tabular);
    crate::sidebar_database::load_connection_folders(tabular);
    crate::sidebar_query::load_queries_from_directory(tabular);
    tabular.yaak_workspaces = crate::http_collection::load_workspaces();
    crate::sidebar_history::load_query_history(tabular);
    tabular.needs_refresh = true;

    info!(
        "📥 Import All Data restored: {} connections, {} folders, {} queries, {} HTTP workspaces, {} history items",
        summary.connections_restored,
        summary.folders_restored,
        summary.queries_restored,
        summary.http_workspaces_restored,
        summary.history_restored
    );

    Ok(summary)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::ZipArchive;

    #[test]
    fn test_zip_options() {
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut buf);
        writer.start_file("test.txt", options).unwrap();
        writer.write_all(b"hello world").unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn test_zip_slip_detection_in_archive() {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();

        // Write normal file
        writer.start_file("manifest.json", opts).unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap();

        // Check inspection of normal file
        let mut archive = ZipArchive::new(buf).unwrap();
        assert!(archive.by_name("manifest.json").is_ok());
        assert!(archive.by_name("manifest.json").unwrap().enclosed_name().is_some());
    }

    #[test]
    fn test_conflict_strategy_labels() {
        let keep = ConflictStrategy::MergeKeepExisting;
        let overwrite = ConflictStrategy::MergeOverwrite;
        let clean = ConflictStrategy::CleanRestore;

        assert_eq!(keep.display_name(), "Merge (Keep Existing)");
        assert_eq!(overwrite.display_name(), "Merge (Overwrite Existing)");
        assert_eq!(clean.display_name(), "Clean Restore (Replace All)");

        assert!(!keep.description().is_empty());
        assert!(!overwrite.description().is_empty());
        assert!(!clean.description().is_empty());
    }

    #[test]
    fn test_manifest_serialization() {
        let manifest = ExportAllManifest {
            version: "1.0".to_string(),
            app: "Tabular".to_string(),
            exported_at: "2026-09-09T14:30:00Z".to_string(),
            counts: ExportCounts {
                connections: 3,
                connection_folders: 1,
                queries: 5,
                http_workspaces: 2,
                history_items: 20,
            },
            includes: ExportIncludes {
                connections: true,
                queries: true,
                http_api: true,
                history: true,
            },
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let deserialized: ExportAllManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, "1.0");
        assert_eq!(deserialized.app, "Tabular");
        assert_eq!(deserialized.counts.connections, 3);
        assert_eq!(deserialized.counts.queries, 5);
        assert!(deserialized.includes.connections);
        assert!(deserialized.includes.queries);
    }

    #[test]
    fn test_archive_creation_and_inspection() {
        let temp_dir = std::env::temp_dir().join(format!("tabular_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let zip_path = temp_dir.join("test_export.zip");

        // Create a test ZIP archive
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            let manifest = ExportAllManifest {
                version: "1.0".to_string(),
                app: "Tabular".to_string(),
                exported_at: "2026-09-09T14:30:00Z".to_string(),
                counts: ExportCounts {
                    connections: 2,
                    connection_folders: 1,
                    queries: 1,
                    http_workspaces: 1,
                    history_items: 3,
                },
                includes: ExportIncludes {
                    connections: true,
                    queries: true,
                    http_api: true,
                    history: true,
                },
            };
            zip.start_file("manifest.json", opts).unwrap();
            zip.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes()).unwrap();

            // Connections
            let conns = vec![ConnectionConfig {
                id: Some(1),
                name: "Test MySQL".to_string(),
                host: "localhost".to_string(),
                port: "3306".to_string(),
                username: "root".to_string(),
                password: "secretpassword".to_string(),
                database: "app_db".to_string(),
                connection_type: crate::models::enums::DatabaseType::MySQL,
                folder: Some("Production".to_string()),
                ssh_enabled: false,
                ssh_host: String::new(),
                ssh_port: String::new(),
                ssh_username: String::new(),
                ssh_auth_method: crate::models::enums::SshAuthMethod::Password,
                ssh_private_key: String::new(),
                ssh_password: String::new(),
                ssh_accept_unknown_host_keys: false,
                ssh_jump_host: String::new(),
                ssl_enabled: false,
                ssl_ca_cert: String::new(),
                ssl_client_cert: String::new(),
                ssl_client_key: String::new(),
                ssl_key_passphrase: String::new(),
                ssl_verify_server: true,
                custom_views: Vec::new(),
                replication_master_id: None,
            }];
            zip.start_file("connections/connections.json", opts).unwrap();
            zip.write_all(serde_json::to_string_pretty(&conns).unwrap().as_bytes()).unwrap();

            // Folders
            let folders = vec!["Production".to_string()];
            zip.start_file("connections/folders.json", opts).unwrap();
            zip.write_all(serde_json::to_string_pretty(&folders).unwrap().as_bytes()).unwrap();

            // Queries
            zip.start_file("queries/analytics/summary.sql", opts).unwrap();
            zip.write_all(b"SELECT COUNT(*) FROM users;").unwrap();

            // HTTP Collections
            zip.start_file("http_collections/ws_1.json", opts).unwrap();
            zip.write_all(b"{\"id\":\"ws_1\",\"name\":\"Test API\",\"requests\":[],\"folders\":[],\"environments\":[]}").unwrap();

            // History
            let history = vec![HistoryItem {
                id: Some(10),
                query: "SELECT 1;".to_string(),
                connection_id: 1,
                connection_name: "Test MySQL".to_string(),
                executed_at: "2026-09-09 12:00:00".to_string(),
            }];
            zip.start_file("history/history.json", opts).unwrap();
            zip.write_all(serde_json::to_string_pretty(&history).unwrap().as_bytes()).unwrap();

            zip.finish().unwrap();
        }

        // Now inspect the archive
        let manifest = inspect_archive(&zip_path).expect("Failed to inspect archive");
        assert_eq!(manifest.version, "1.0");
        assert_eq!(manifest.app, "Tabular");
        assert_eq!(manifest.counts.connections, 2);
        assert_eq!(manifest.counts.queries, 1);
        assert_eq!(manifest.counts.http_workspaces, 1);
        assert_eq!(manifest.counts.history_items, 3);
        assert!(manifest.includes.connections);
        assert!(manifest.includes.queries);
        assert!(manifest.includes.http_api);
        assert!(manifest.includes.history);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_zip_slip_rejection() {
        let temp_dir = std::env::temp_dir().join(format!("tabular_slip_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let evil_zip_path = temp_dir.join("evil.zip");

        {
            let file = File::create(&evil_zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let opts = SimpleFileOptions::default();

            // Write an entry with directory traversal
            zip.start_file("../../etc/malicious.txt", opts).unwrap();
            zip.write_all(b"malicious content").unwrap();
            zip.finish().unwrap();
        }

        let result = inspect_archive(&evil_zip_path);
        assert!(result.is_err(), "Expected zip slip detection to fail inspection");
        match result {
            Err(ExportImportError::ZipSlip(path)) => {
                assert!(path.contains("../../etc/malicious.txt"));
            }
            other => panic!("Unexpected result: {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_roundtrip_export_import() {
        let temp_dir = std::env::temp_dir().join(format!("tabular_roundtrip_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("test_tabular.db");
        let zip_path = temp_dir.join("full_backup.zip");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let pool = rt.block_on(async {
            use sqlx::sqlite::SqliteConnectOptions;
            use std::str::FromStr;
            let options = SqliteConnectOptions::from_str(&format!("sqlite://{}?mode=rwc", db_path.display()))
                .unwrap();
            let p = sqlx::SqlitePool::connect_with(options).await.unwrap();

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS connections (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    host TEXT NOT NULL,
                    port TEXT NOT NULL,
                    username TEXT NOT NULL,
                    password TEXT NOT NULL,
                    database_name TEXT NOT NULL,
                    connection_type TEXT NOT NULL,
                    folder TEXT,
                    ssh_enabled INTEGER DEFAULT 0,
                    ssh_host TEXT DEFAULT '',
                    ssh_port TEXT DEFAULT '',
                    ssh_username TEXT DEFAULT '',
                    ssh_auth_method TEXT DEFAULT 'password',
                    ssh_private_key TEXT DEFAULT '',
                    ssh_password TEXT DEFAULT '',
                    ssh_accept_unknown_host_keys INTEGER DEFAULT 0,
                    custom_views TEXT DEFAULT '[]',
                    replication_master_id INTEGER,
                    ssh_jump_host TEXT DEFAULT '',
                    ssl_enabled INTEGER DEFAULT 0,
                    ssl_ca_cert TEXT DEFAULT '',
                    ssl_client_cert TEXT DEFAULT '',
                    ssl_client_key TEXT DEFAULT '',
                    ssl_key_passphrase TEXT DEFAULT '',
                    ssl_verify_server INTEGER DEFAULT 1
                );"
            )
            .execute(&p)
            .await
            .unwrap();

            sqlx::query("CREATE TABLE IF NOT EXISTS connection_folders (path TEXT PRIMARY KEY);")
                .execute(&p)
                .await
                .unwrap();

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS query_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    query_text TEXT NOT NULL,
                    connection_id INTEGER NOT NULL,
                    connection_name TEXT NOT NULL,
                    executed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE
                );"
            )
            .execute(&p)
            .await
            .unwrap();

            p
        });

        let mut tabular = Tabular::new();
        tabular.db_pool = Some(std::sync::Arc::new(pool));

        // Add dummy connection
        let conn = ConnectionConfig {
            id: Some(1),
            name: "Demo Database".to_string(),
            host: "127.0.0.1".to_string(),
            port: "5432".to_string(),
            username: "postgres".to_string(),
            password: "mypassword".to_string(),
            database: "demo_db".to_string(),
            connection_type: crate::models::enums::DatabaseType::PostgreSQL,
            folder: Some("Development".to_string()),
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: String::new(),
            ssh_username: String::new(),
            ssh_auth_method: crate::models::enums::SshAuthMethod::Password,
            ssh_private_key: String::new(),
            ssh_password: String::new(),
            ssh_accept_unknown_host_keys: false,
            ssh_jump_host: String::new(),
            ssl_enabled: false,
            ssl_ca_cert: String::new(),
            ssl_client_cert: String::new(),
            ssl_client_key: String::new(),
            ssl_key_passphrase: String::new(),
            ssl_verify_server: true,
            custom_views: Vec::new(),
            replication_master_id: None,
        };
        tabular.connections.push(conn.clone());
        tabular.connection_folders.push("Development".to_string());

        // Add dummy HTTP workspace
        tabular.yaak_workspaces.push(crate::http_collection::HttpWorkspace {
            id: "ws_test_123".to_string(),
            name: "Internal APIs".to_string(),
            requests: Vec::new(),
            folders: Vec::new(),
            environments: Vec::new(),
        });

        // Add dummy history item
        tabular.history_items.push(HistoryItem {
            id: Some(1),
            query: "SELECT * FROM products;".to_string(),
            connection_id: 1,
            connection_name: "Demo Database".to_string(),
            executed_at: "2026-09-09 14:00:00".to_string(),
        });

        // Populate SQLite tables
        let p = tabular.db_pool.as_ref().unwrap().clone();
        rt.block_on(async {
            sqlx::query("INSERT INTO connections (id, name, host, port, username, password, database_name, connection_type, folder) VALUES (1, 'Demo Database', '127.0.0.1', '5432', 'postgres', 'mypassword', 'demo_db', 'PostgreSQL', 'Development')")
                .execute(p.as_ref())
                .await
                .unwrap();
            sqlx::query("INSERT INTO connection_folders (path) VALUES ('Development')")
                .execute(p.as_ref())
                .await
                .unwrap();
            sqlx::query("INSERT INTO query_history (id, query_text, connection_id, connection_name, executed_at) VALUES (1, 'SELECT * FROM products;', 1, 'Demo Database', '2026-09-09 14:00:00')")
                .execute(p.as_ref())
                .await
                .unwrap();
        });

        // 1. Export data
        let export_res = export_all_data(&mut tabular, &zip_path, &ExportAllOptions::default());
        assert!(export_res.is_ok(), "Export failed: {:?}", export_res.err());
        let summary = export_res.unwrap();
        assert!(summary.connections_count >= 1);
        assert!(summary.folders_count >= 1);
        assert!(summary.http_workspaces_count >= 1);
        assert!(summary.history_count >= 1);
        assert!(summary.zip_file_size > 0);

        // 2. Inspect archive
        let inspect_res = inspect_archive(&zip_path);
        assert!(inspect_res.is_ok(), "Inspect failed: {:?}", inspect_res.err());
        let manifest = inspect_res.unwrap();
        assert_eq!(manifest.counts.connections, summary.connections_count);
        assert_eq!(manifest.counts.connection_folders, summary.folders_count);
        assert_eq!(manifest.counts.http_workspaces, summary.http_workspaces_count);
        assert_eq!(manifest.counts.history_items, summary.history_count);
        assert_eq!(manifest.counts.queries, summary.queries_count);

        // 3. Clear memory and test restore
        tabular.connections.clear();
        tabular.connection_folders.clear();
        tabular.yaak_workspaces.clear();
        tabular.history_items.clear();

        let import_opts = ImportAllOptions {
            conflict_strategy: ConflictStrategy::CleanRestore,
            ..Default::default()
        };

        let import_res = import_all_data(&mut tabular, &zip_path, &import_opts);
        assert!(import_res.is_ok(), "Import failed: {:?}", import_res.err());
        let imp_summary = import_res.unwrap();
        assert!(imp_summary.connections_restored >= 1);
        assert!(imp_summary.folders_restored >= 1);
        assert!(imp_summary.http_workspaces_restored >= 1);
        assert!(imp_summary.history_restored >= 1);

        // Verify in-memory state was reloaded
        assert!(!tabular.connections.is_empty());
        assert!(tabular.connections.iter().any(|c| c.name == "Demo Database"));
        assert!(!tabular.connection_folders.is_empty());
        assert!(tabular.connection_folders.iter().any(|f| f == "Development"));

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
