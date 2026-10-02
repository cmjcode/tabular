//! flexurio_import.rs — Import endpoint HTTP API dari konfigurasi Flexurio NoCode API.
//!
//! Format Flexurio NoCode API (https://github.com/flexurio/flx-nocode-api):
//! - `backend/config/routes.json` (atau `config/routes.json`): mendefinisikan array `routes`
//!   dan `route_publics`.
//! - `backend/config/entity/<route>.json`: skema entitas per route (kolom, relasi master-detail,
//!   serta method HTTP `get`, `post`, `put`, `del`, `patch`, `trace` yang diaktifkan).
//!
//! Modul ini membaca konfigurasi tersebut dan membangun `HttpWorkspace` lengkap dengan
//! folder per route, request tersimpan dengan sample body & parameter, tautan ke tabel ERD,
//! serta variabel environment (`base_url`, `id`, `token`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::http_collection::{HttpFolder, HttpWorkspace, SavedRequest, YaakEnvironment};
use crate::models::structs::{HttpAuthType, HttpBodyType, HttpMethod};

// ─── Struct Definisi Skema Flexurio ──────────────────────────────────────────

/// Konfigurasi utama `routes.json`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioRoutes {
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub route_publics: Vec<String>,
    #[serde(default)]
    pub converter_token: Option<serde_json::Value>,
}

/// Skema entitas dari file `entity/<route>.json`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioEntity {
    #[serde(default)]
    pub table: String,
    #[serde(default)]
    pub primary_key: Option<FlexurioPrimaryKey>,
    #[serde(default)]
    pub columns: Vec<FlexurioColumn>,
    #[serde(default)]
    pub foreign_keys: Vec<serde_json::Value>,
    #[serde(default)]
    pub indexes: Vec<serde_json::Value>,
    #[serde(default)]
    pub details: Vec<FlexurioDetail>,
    #[serde(default)]
    pub get: Option<FlexurioGet>,
    #[serde(default)]
    pub post: Option<FlexurioPostPut>,
    #[serde(default)]
    pub put: Option<FlexurioPostPut>,
    #[serde(default)]
    pub del: Option<FlexurioDelete>,
    #[serde(default)]
    pub patch: Option<FlexurioPatch>,
    #[serde(default)]
    pub trace: Option<FlexurioTrace>,
    #[serde(default)]
    pub auto_generate: Option<bool>,
    #[serde(default)]
    pub state_machine: Option<FlexurioStateMachine>,
    #[serde(default)]
    pub locked_when: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioPrimaryKey {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub auto_increment: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioColumn {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub type_data: String,
    #[serde(default)]
    pub auto_increment: Option<bool>,
    #[serde(default)]
    pub nullable: Option<bool>,
    #[serde(default)]
    pub function: Option<String>,
    #[serde(default)]
    pub function_endpoint: Option<String>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub encrypt: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioDetail {
    #[serde(default)]
    pub field: String,
    #[serde(default)]
    pub target_table: String,
    #[serde(default)]
    pub foreign_key_column: String,
    #[serde(default)]
    pub parent_key_column: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub update_strategy: Option<String>,
    #[serde(default)]
    pub cascade_delete: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioGet {
    #[serde(default)]
    pub enable_method: Option<bool>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(default)]
    pub join_tables: Vec<serde_json::Value>,
    #[serde(default)]
    pub order_by: Vec<String>,
    #[serde(default)]
    pub column_groups: Vec<String>,
    #[serde(default)]
    pub having: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioPostPut {
    #[serde(default)]
    pub enable_method: Option<bool>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub validate_data: Option<String>,
    #[serde(default)]
    pub pre_process: Option<String>,
    #[serde(default)]
    pub post_process: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioDelete {
    #[serde(default)]
    pub enable_method: Option<bool>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub type_delete: Option<String>,
    #[serde(default)]
    pub pre_process: Option<String>,
    #[serde(default)]
    pub post_process: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioPatch {
    #[serde(default)]
    pub enable_method: Option<bool>,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(default)]
    pub pre_process_sp: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioTrace {
    #[serde(default)]
    pub enable_method: Option<bool>,
    #[serde(default)]
    pub insert_into: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FlexurioStateMachine {
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub initial: Option<String>,
}

/// Hasil dari import konfigurasi Flexurio.
#[derive(Debug, Clone)]
pub struct FlexurioImportResult {
    pub workspaces: Vec<HttpWorkspace>,
    pub total_requests: usize,
    pub total_routes: usize,
    pub warnings: Vec<String>,
}

// ─── Resolusi Jalur File / Folder ───────────────────────────────────────────

/// Jalur file yang sudah berhasil diselesaikan dari input pengguna.
struct ResolvedConfig {
    routes_file: Option<PathBuf>,
    entity_dir: Option<PathBuf>,
    single_entity_file: Option<PathBuf>,
    config_dir: PathBuf,
}

/// Deteksi struktur direktori Flexurio dari path yang dipilih pengguna.
fn resolve_flexurio_paths(path: &Path) -> Result<ResolvedConfig, String> {
    if path.is_file() {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name == "routes.json" {
            let config_dir = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            let entity_dir = config_dir.join("entity");
            return Ok(ResolvedConfig {
                routes_file: Some(path.to_path_buf()),
                entity_dir: if entity_dir.is_dir() {
                    Some(entity_dir)
                } else {
                    None
                },
                single_entity_file: None,
                config_dir,
            });
        }

        // Jika user memilih file dalam folder entity (contoh: entity/flx_users.json)
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(parent) = path.parent() {
                if parent.file_name().and_then(|n| n.to_str()) == Some("entity") {
                    if let Some(grandparent) = parent.parent() {
                        let potential_routes = grandparent.join("routes.json");
                        if potential_routes.is_file() {
                            return Ok(ResolvedConfig {
                                routes_file: Some(potential_routes),
                                entity_dir: Some(parent.to_path_buf()),
                                single_entity_file: None,
                                config_dir: grandparent.to_path_buf(),
                            });
                        }
                    }
                }
            }

            // Fallback: impor single entity file
            let config_dir = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            return Ok(ResolvedConfig {
                routes_file: None,
                entity_dir: None,
                single_entity_file: Some(path.to_path_buf()),
                config_dir,
            });
        }

        return Err(format!(
            "File '{}' bukan file JSON konfigurasi Flexurio yang valid.",
            path.display()
        ));
    }

    if path.is_dir() {
        // Cek langsung di direktori yang dipilih
        let direct_routes = path.join("routes.json");
        let direct_entity = path.join("entity");
        if direct_routes.is_file() {
            return Ok(ResolvedConfig {
                routes_file: Some(direct_routes),
                entity_dir: if direct_entity.is_dir() {
                    Some(direct_entity)
                } else {
                    None
                },
                single_entity_file: None,
                config_dir: path.to_path_buf(),
            });
        }

        // Cek subfolder umum: backend/config, config, binaries/backend/config
        let candidate_subdirs = [
            "backend/config",
            "config",
            "binaries/backend/config",
            "server/config",
        ];
        for sub in candidate_subdirs {
            let candidate_config = path.join(sub);
            let candidate_routes = candidate_config.join("routes.json");
            let candidate_entity = candidate_config.join("entity");
            if candidate_routes.is_file() {
                return Ok(ResolvedConfig {
                    routes_file: Some(candidate_routes),
                    entity_dir: if candidate_entity.is_dir() {
                        Some(candidate_entity)
                    } else {
                        None
                    },
                    single_entity_file: None,
                    config_dir: candidate_config,
                });
            }
        }

        // Jika hanya ada folder entity di dalam path
        if direct_entity.is_dir() {
            return Ok(ResolvedConfig {
                routes_file: None,
                entity_dir: Some(direct_entity),
                single_entity_file: None,
                config_dir: path.to_path_buf(),
            });
        }

        // Cek dengan deteksi Flexurio yang lebih fleksibel
        if let Some(routes) = detect_flexurio_config(path) {
            let config_dir = routes.parent().unwrap_or(path).to_path_buf();
            let entity_dir = config_dir.join("entity");
            return Ok(ResolvedConfig {
                routes_file: Some(routes),
                entity_dir: if entity_dir.is_dir() {
                    Some(entity_dir)
                } else {
                    None
                },
                single_entity_file: None,
                config_dir,
            });
        }

        return Err(format!(
            "Tidak dapat menemukan 'routes.json' atau folder 'entity' di dalam '{}'.",
            path.display()
        ));
    }

    Err(format!("Jalur '{}' tidak ditemukan.", path.display()))
}

/// Mencoba mencari konfigurasi port dan base URL dari file `.env` di sekitar konfigurasi.
fn detect_env_settings(start_dir: &Path) -> (String, Option<String>) {
    let mut current = Some(start_dir);
    for _ in 0..5 {
        if let Some(dir) = current {
            let env_file = dir.join(".env");
            if env_file.is_file() {
                if let Ok(content) = std::fs::read_to_string(&env_file) {
                    let mut port = None;
                    let mut base_url = None;
                    for line in content.lines() {
                        let line = line.trim();
                        if line.starts_with('#') || !line.contains('=') {
                            continue;
                        }
                        let (k, v) = line.split_once('=').unwrap_or(("", ""));
                        let k = k.trim();
                        let v = v.trim().trim_matches('"').trim_matches('\'');
                        if k.eq_ignore_ascii_case("PORT") && !v.is_empty() {
                            port = Some(v.to_string());
                        } else if k.eq_ignore_ascii_case("BASE_URL") && !v.is_empty() {
                            base_url = Some(v.to_string());
                        }
                    }

                    if let Some(url) = base_url {
                        let p = port.unwrap_or_else(|| "8080".to_string());
                        return (p, Some(url));
                    }
                    if let Some(p) = port {
                        return (p.clone(), Some(format!("http://localhost:{p}")));
                    }
                }
            }
            current = dir.parent();
        } else {
            break;
        }
    }
    (
        "8080".to_string(),
        Some("http://localhost:8080".to_string()),
    )
}

/// Deteksi base URL dari file .env di sekitar konfigurasi Flexurio.
pub fn detect_flexurio_base_url(start_dir: &Path) -> Option<String> {
    let (_, base_url) = detect_env_settings(start_dir);
    base_url
}

// ─── Logika Impor Utama ──────────────────────────────────────────────────────

/// Impor endpoint Flexurio NoCode API dari file `routes.json` atau direktori config.
pub fn import_from_flexurio(path: &Path) -> Result<FlexurioImportResult, String> {
    let resolved = resolve_flexurio_paths(path)?;
    let mut warnings = Vec::new();

    let (port, detected_base_url) = detect_env_settings(&resolved.config_dir);
    let default_base_url = detected_base_url.unwrap_or_else(|| format!("http://localhost:{port}"));

    // Tentukan nama workspace dari folder project
    let project_name = resolved
        .config_dir
        .ancestors()
        .find_map(|p| {
            let name = p.file_name()?.to_str()?;
            if name != "config" && name != "backend" && name != "binaries" && !name.is_empty() {
                Some(name)
            } else {
                None
            }
        })
        .unwrap_or("Flexurio API");

    let ws_id = format!("flx_ws_{}", chrono::Utc::now().timestamp_millis());
    let environments = vec![YaakEnvironment {
        id: format!("flx_env_{}", chrono::Utc::now().timestamp_millis()),
        name: "Default".to_string(),
        variables: vec![
            ("base_url".to_string(), default_base_url),
            ("id".to_string(), "1".to_string()),
            ("token".to_string(), String::new()),
        ],
    }];

    // Kasus 1: Mengimpor melalui `routes.json`
    if let Some(routes_path) = &resolved.routes_file {
        let content = std::fs::read_to_string(routes_path).map_err(|e| {
            format!(
                "Gagal membaca file routes '{}': {}",
                routes_path.display(),
                e
            )
        })?;

        let routes_config: FlexurioRoutes = serde_json::from_str(&content).map_err(|e| {
            format!(
                "Format JSON tidak valid pada '{}': {}",
                routes_path.display(),
                e
            )
        })?;

        let mut folders = Vec::new();
        let mut total_requests = 0usize;
        let mut processed_routes = HashSet::new();

        let entity_dir = resolved.entity_dir.as_deref();

        for route in &routes_config.routes {
            let route_clean = route.trim();
            if route_clean.is_empty() || !processed_routes.insert(route_clean.to_string()) {
                continue;
            }

            let entity_opt = if let Some(edir) = entity_dir {
                let entity_file = edir.join(format!("{route_clean}.json"));
                if entity_file.is_file() {
                    match std::fs::read_to_string(&entity_file) {
                        Ok(data) => match serde_json::from_str::<FlexurioEntity>(&data) {
                            Ok(ent) => Some((ent, entity_file)),
                            Err(e) => {
                                warnings.push(format!(
                                    "Route '{route_clean}': gagal mem-parse {}: {e}",
                                    entity_file.display()
                                ));
                                None
                            }
                        },
                        Err(e) => {
                            warnings.push(format!(
                                "Route '{route_clean}': gagal membaca {}: {e}",
                                entity_file.display()
                            ));
                            None
                        }
                    }
                } else {
                    warnings.push(format!(
                        "Route '{route_clean}': file skema entity tidak ditemukan di {}",
                        entity_file.display()
                    ));
                    None
                }
            } else {
                None
            };

            let folder_id = format!(
                "flx_fld_{route_clean}_{}",
                chrono::Utc::now().timestamp_millis()
            );
            let (folder, req_count) = build_route_folder(
                &ws_id,
                &folder_id,
                route_clean,
                entity_opt.as_ref(),
                &routes_config.route_publics,
            );
            total_requests += req_count;
            folders.push(folder);
        }

        // Tambahkan folder Public / Auth jika ada route_publics seperti login/register
        let public_routes: Vec<_> = routes_config
            .route_publics
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if !public_routes.is_empty() {
            let auth_folder_id = format!("flx_fld_auth_{}", chrono::Utc::now().timestamp_millis());
            let mut auth_requests = Vec::new();
            for pub_route in public_routes {
                if !processed_routes.contains(pub_route) {
                    let req = build_public_route_request(&ws_id, &auth_folder_id, pub_route);
                    auth_requests.push(req);
                    total_requests += 1;
                }
            }
            if !auth_requests.is_empty() {
                folders.insert(
                    0,
                    HttpFolder {
                        id: auth_folder_id,
                        name: "Authentication & Public".to_string(),
                        parent_folder_id: None,
                        requests: auth_requests,
                        children: Vec::new(),
                        repo_url: None,
                    },
                );
            }
        }

        let total_routes = folders.len();
        let workspace = HttpWorkspace {
            id: ws_id,
            name: format!("Flexurio - {project_name}"),
            requests: Vec::new(),
            folders,
            environments,
        };

        return Ok(FlexurioImportResult {
            workspaces: vec![workspace],
            total_requests,
            total_routes,
            warnings,
        });
    }

    // Kasus 2: Mengimpor direktori `entity/` langsung tanpa `routes.json`
    if let Some(entity_dir) = &resolved.entity_dir {
        let Ok(entries) = std::fs::read_dir(entity_dir) else {
            return Err(format!("Gagal membaca folder '{}'", entity_dir.display()));
        };

        let mut folders = Vec::new();
        let mut total_requests = 0usize;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let route_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if route_name.is_empty() {
                continue;
            }

            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(ent) = serde_json::from_str::<FlexurioEntity>(&data) {
                    let folder_id = format!(
                        "flx_fld_{route_name}_{}",
                        chrono::Utc::now().timestamp_millis()
                    );
                    let (folder, req_count) = build_route_folder(
                        &ws_id,
                        &folder_id,
                        &route_name,
                        Some(&(ent, path.clone())),
                        &[],
                    );
                    total_requests += req_count;
                    folders.push(folder);
                }
            }
        }

        folders.sort_by(|a, b| a.name.cmp(&b.name));
        let total_routes = folders.len();
        let workspace = HttpWorkspace {
            id: ws_id,
            name: format!("Flexurio - {project_name}"),
            requests: Vec::new(),
            folders,
            environments,
        };

        return Ok(FlexurioImportResult {
            workspaces: vec![workspace],
            total_requests,
            total_routes,
            warnings,
        });
    }

    // Kasus 3: Mengimpor single entity JSON file
    if let Some(single_file) = &resolved.single_entity_file {
        let route_name = single_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("entity")
            .to_string();

        let data = std::fs::read_to_string(single_file)
            .map_err(|e| format!("Gagal membaca file '{}': {e}", single_file.display()))?;

        let ent: FlexurioEntity = serde_json::from_str(&data).map_err(|e| {
            format!(
                "Gagal mem-parse entity JSON '{}': {e}",
                single_file.display()
            )
        })?;

        let folder_id = format!(
            "flx_fld_{route_name}_{}",
            chrono::Utc::now().timestamp_millis()
        );
        let (folder, req_count) = build_route_folder(
            &ws_id,
            &folder_id,
            &route_name,
            Some(&(ent, single_file.clone())),
            &[],
        );

        let workspace = HttpWorkspace {
            id: ws_id,
            name: format!("Flexurio - {route_name}"),
            requests: Vec::new(),
            folders: vec![folder],
            environments,
        };

        return Ok(FlexurioImportResult {
            workspaces: vec![workspace],
            total_requests: req_count,
            total_routes: 1,
            warnings,
        });
    }

    Err("Tidak ada konfigurasi Flexurio yang dapat diimpor.".to_string())
}

/// Memeriksa apakah suatu file routes.json memiliki format array "routes" yang valid.
pub fn is_flexurio_routes_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            return val.get("routes").and_then(|r| r.as_array()).is_some();
        }
    }
    false
}

/// Menelusuri subfolder hingga kedalaman tertentu untuk mencari file `config/routes.json`.
fn find_config_routes(dir: &Path, depth: usize, max_depth: usize) -> Option<PathBuf> {
    if depth > max_depth {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if ft.is_dir() {
            if name_str.starts_with('.')
                || name_str == "node_modules"
                || name_str == "target"
                || name_str == "vendor"
                || name_str == "dist"
                || name_str == "build"
            {
                continue;
            }
            if name_str.eq_ignore_ascii_case("config") {
                let candidate = entry.path().join("routes.json");
                if is_flexurio_routes_file(&candidate) {
                    return Some(candidate);
                }
            }
            subdirs.push(entry.path());
        }
    }
    for sub in subdirs {
        if let Some(found) = find_config_routes(&sub, depth + 1, max_depth) {
            return Some(found);
        }
    }
    None
}

/// Deteksi apakah suatu folder/path berisi konfigurasi Flexurio NoCode API
/// (misalnya `routes.json` di dalam folder `config` atau `backend/config`).
pub fn detect_flexurio_config(root: &Path) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }
    // 1. Jika pengguna memilih file langsung
    if root.is_file() {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("routes.json") {
            let parent_is_config = root
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("config"))
                .unwrap_or(false);
            if parent_is_config || is_flexurio_routes_file(root) {
                return Some(root.to_path_buf());
            }
        }
        return None;
    }

    // 2. Jika root adalah folder yang bernama "config" atau berisi direct "routes.json"
    let direct_routes = root.join("routes.json");
    if direct_routes.is_file() {
        let is_config = root
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("config"))
            .unwrap_or(false);
        if is_config || root.join("entity").is_dir() || is_flexurio_routes_file(&direct_routes) {
            return Some(direct_routes);
        }
    }

    // 3. Cek kandidat subfolder umum (cepat tanpa rekursif)
    let candidate_subpaths = [
        "config/routes.json",
        "backend/config/routes.json",
        "binaries/backend/config/routes.json",
        "server/config/routes.json",
        "api/config/routes.json",
        "app/config/routes.json",
        "src/config/routes.json",
    ];
    for sub in candidate_subpaths {
        let candidate = root.join(sub);
        if is_flexurio_routes_file(&candidate) {
            return Some(candidate);
        }
    }

    // 4. Telusuri subfolder rekursif hingga kedalaman 4 untuk menemukan */config/routes.json
    find_config_routes(root, 0, 4)
}

/// Impor seluruh route Flexurio NoCode API langsung ke dalam suatu folder spesifik.
pub fn import_flexurio_into_folder(
    workspaces: &mut [HttpWorkspace],
    target_folder_id: &str,
    config_path: &Path,
) -> Result<FlexurioImportResult, String> {
    let result = import_from_flexurio(config_path)?;
    let Some(first_ws) = result.workspaces.first() else {
        return Ok(result);
    };

    let Some((ws, _)) = crate::http_collection::find_workspace_folder(workspaces, target_folder_id)
    else {
        return Err("Target folder tidak ditemukan dalam workspace".to_string());
    };
    let ws_id = ws.id.clone();

    let Some(target_ws) = workspaces.iter_mut().find(|w| w.id == ws_id) else {
        return Err("Workspace tidak ditemukan".to_string());
    };

    // Tambahkan environment variables jika belum ada
    if let Some(imported_env) = first_ws.environments.first() {
        if target_ws.environments.is_empty() {
            target_ws.environments.push(imported_env.clone());
        } else {
            for (k, v) in &imported_env.variables {
                for env in target_ws.environments.iter_mut() {
                    if !env.variables.iter().any(|(ek, _)| ek == k) {
                        env.variables.push((k.clone(), v.clone()));
                    }
                }
            }
        }
    }

    // Set children folders ke target folder
    if let Some(target_folder) =
        crate::http_collection::find_folder_mut(&mut target_ws.folders, target_folder_id)
    {
        let mut new_children = first_ws.folders.clone();
        for child in &mut new_children {
            child.parent_folder_id = Some(target_folder_id.to_string());
            for req in &mut child.requests {
                req.workspace_id = ws_id.clone();
                req.folder_id = Some(child.id.clone());
            }
        }
        for new_child in new_children {
            if let Some(existing) = target_folder
                .children
                .iter_mut()
                .find(|c| c.name == new_child.name)
            {
                *existing = new_child;
            } else {
                target_folder.children.push(new_child);
            }
        }
    }

    Ok(result)
}

/// Membangun contoh JSON body dari definisi FlexurioEntity untuk POST atau PUT.
pub fn build_entity_sample_body(ent: &FlexurioEntity, is_post: bool) -> String {
    let mut columns_map = HashMap::new();
    for c in &ent.columns {
        columns_map.insert(c.name.clone(), c.clone());
    }
    let cols: Vec<String> = if is_post {
        ent.post
            .as_ref()
            .map(|p| p.columns.clone())
            .unwrap_or_else(|| ent.columns.iter().map(|c| c.name.clone()).collect())
    } else {
        ent.put
            .as_ref()
            .map(|p| p.columns.clone())
            .unwrap_or_else(|| ent.columns.iter().map(|c| c.name.clone()).collect())
    };
    let sm_init = ent.state_machine.as_ref().and_then(|sm| sm.initial.clone());
    build_sample_json_body(&cols, &columns_map, &ent.details, sm_init.as_deref())
}

// ─── Builder Folder dan Request ──────────────────────────────────────────────

/// Membangun `HttpFolder` beserta seluruh `SavedRequest` untuk satu route Flexurio.
fn build_route_folder(
    ws_id: &str,
    folder_id: &str,
    route: &str,
    entity_info: Option<&(FlexurioEntity, PathBuf)>,
    public_routes: &[String],
) -> (HttpFolder, usize) {
    let mut requests = Vec::new();
    let is_public = public_routes.iter().any(|r| r.trim() == route);

    let default_headers = vec![("Accept".to_string(), "application/json".to_string(), true)];
    let json_headers = vec![
        (
            "Content-Type".to_string(),
            "application/json".to_string(),
            true,
        ),
        ("Accept".to_string(), "application/json".to_string(), true),
    ];

    let (table_name, tables, source_str, columns_map, pk_field, state_machine_initial) =
        if let Some((ent, path)) = entity_info {
            let tbl = if !ent.table.is_empty() {
                ent.table.clone()
            } else {
                route.to_string()
            };

            let mut all_tables = vec![tbl.clone()];
            for d in &ent.details {
                if !d.target_table.is_empty() && !all_tables.contains(&d.target_table) {
                    all_tables.push(d.target_table.clone());
                }
            }

            let mut cols = HashMap::new();
            for c in &ent.columns {
                cols.insert(c.name.clone(), c.clone());
            }

            let pk = ent
                .primary_key
                .as_ref()
                .and_then(|k| k.columns.first())
                .cloned()
                .unwrap_or_else(|| "id".to_string());

            let sm_init = ent.state_machine.as_ref().and_then(|sm| sm.initial.clone());

            (
                tbl,
                all_tables,
                Some(path.to_string_lossy().to_string()),
                cols,
                pk,
                sm_init,
            )
        } else {
            (
                route.to_string(),
                vec![route.to_string()],
                None,
                HashMap::new(),
                "id".to_string(),
                None,
            )
        };

    let base_auth = if is_public {
        HttpAuthType::NoAuth
    } else {
        HttpAuthType::BearerToken
    };
    let base_token = if is_public {
        String::new()
    } else {
        "{{token}}".to_string()
    };

    // 1. GET /<route> (List / Search)
    let get_enabled = entity_info
        .map(|(e, _)| e.get.as_ref().and_then(|g| g.enable_method).unwrap_or(true))
        .unwrap_or(true);

    if get_enabled {
        let mut params = Vec::new();
        let mut description_lines = Vec::new();
        description_lines.push(format!("### List & Search `{route}`"));
        description_lines.push(format!("Table: `{table_name}`\n"));

        // Parameter pencarian bawaan Flexurio jika dideklarasikan di get.parameters
        if let Some((ent, _)) = entity_info {
            if let Some(get_cfg) = &ent.get {
                for param in &get_cfg.parameters {
                    let p = param.trim();
                    if p.is_empty() {
                        continue;
                    }
                    if p == "limit" {
                        params.push(("limit".to_string(), "20".to_string(), false));
                    } else if p == "page" {
                        params.push(("page".to_string(), "1".to_string(), false));
                    } else if p == "sort" {
                        params.push(("sort".to_string(), pk_field.clone(), false));
                    } else if p == "ascending" {
                        params.push(("ascending".to_string(), "true".to_string(), false));
                    } else if p == "search" {
                        params.push(("search".to_string(), String::new(), false));
                    } else {
                        // Filter operator seperti title.like, status.eq
                        params.push((p.to_string(), String::new(), false));
                    }
                }
            }
        }

        // Jika parameters kosong, sediakan standar Flexurio
        if params.is_empty() {
            params.push(("limit".to_string(), "20".to_string(), false));
            params.push(("page".to_string(), "1".to_string(), false));
        }

        requests.push(SavedRequest {
            id: format!(
                "req_get_list_{route}_{}",
                chrono::Utc::now().timestamp_millis()
            ),
            workspace_id: ws_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            name: format!("List {route}"),
            url: format!("{{{{base_url}}}}/{route}"),
            method: HttpMethod::GET,
            params,
            headers: default_headers.clone(),
            body_type: HttpBodyType::NoBody,
            auth_type: base_auth.clone(),
            bearer_token: base_token.clone(),
            description: description_lines.join("\n"),
            tables: tables.clone(),
            source: source_str.clone(),
            route: Some(format!("/{route}")),
            ..Default::default()
        });

        // 2. GET /<route>/{id} (Single Record)
        requests.push(SavedRequest {
            id: format!(
                "req_get_one_{route}_{}",
                chrono::Utc::now().timestamp_millis()
            ),
            workspace_id: ws_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            name: format!("Get {route} by ID"),
            url: format!("{{{{base_url}}}}/{route}/{{{{id}}}}"),
            method: HttpMethod::GET,
            params: Vec::new(),
            headers: default_headers.clone(),
            body_type: HttpBodyType::NoBody,
            auth_type: base_auth.clone(),
            bearer_token: base_token.clone(),
            description: format!(
                "Ambil record tunggal `{route}` berdasarkan primary key `{pk_field}`."
            ),
            tables: tables.clone(),
            source: source_str.clone(),
            route: Some(format!("/{route}/{{{pk_field}}}")),
            ..Default::default()
        });
    }

    // 3. POST /<route> (Create Record)
    let post_enabled = entity_info
        .map(|(e, _)| {
            e.post
                .as_ref()
                .and_then(|p| p.enable_method)
                .unwrap_or(true)
        })
        .unwrap_or(true);

    if post_enabled {
        let sample_body = if let Some((ent, _)) = entity_info {
            let cols = ent
                .post
                .as_ref()
                .map(|p| p.columns.as_slice())
                .unwrap_or(&[]);
            build_sample_json_body(
                cols,
                &columns_map,
                &ent.details,
                state_machine_initial.as_deref(),
            )
        } else {
            serde_json::json!({ "name": "sample" }).to_string()
        };

        requests.push(SavedRequest {
            id: format!("req_post_{route}_{}", chrono::Utc::now().timestamp_millis()),
            workspace_id: ws_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            name: format!("Create {route}"),
            url: format!("{{{{base_url}}}}/{route}"),
            method: HttpMethod::POST,
            params: Vec::new(),
            headers: json_headers.clone(),
            body_type: HttpBodyType::Json,
            body_text: sample_body,
            auth_type: base_auth.clone(),
            bearer_token: base_token.clone(),
            description: format!(
                "Tambah record baru `{route}`. Mendukung transaksi atomik master-detail."
            ),
            tables: tables.clone(),
            source: source_str.clone(),
            route: Some(format!("/{route}")),
            ..Default::default()
        });
    }

    // 4. PUT /<route>/{id} (Update Record)
    let put_enabled = entity_info
        .map(|(e, _)| e.put.as_ref().and_then(|p| p.enable_method).unwrap_or(true))
        .unwrap_or(true);

    if put_enabled {
        let sample_body = if let Some((ent, _)) = entity_info {
            let cols = ent
                .put
                .as_ref()
                .map(|p| p.columns.as_slice())
                .unwrap_or(&[]);
            build_sample_json_body(cols, &columns_map, &[], None)
        } else {
            serde_json::json!({ "name": "updated_sample" }).to_string()
        };

        requests.push(SavedRequest {
            id: format!("req_put_{route}_{}", chrono::Utc::now().timestamp_millis()),
            workspace_id: ws_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            name: format!("Update {route}"),
            url: format!("{{{{base_url}}}}/{route}/{{{{id}}}}"),
            method: HttpMethod::PUT,
            params: Vec::new(),
            headers: json_headers.clone(),
            body_type: HttpBodyType::Json,
            body_text: sample_body,
            auth_type: base_auth.clone(),
            bearer_token: base_token.clone(),
            description: format!("Perbarui record `{route}` berdasarkan `{pk_field}`."),
            tables: tables.clone(),
            source: source_str.clone(),
            route: Some(format!("/{route}/{{{pk_field}}}")),
            ..Default::default()
        });
    }

    // 5. DELETE /<route>/{id}
    let del_enabled = entity_info
        .map(|(e, _)| e.del.as_ref().and_then(|d| d.enable_method).unwrap_or(true))
        .unwrap_or(true);

    if del_enabled {
        let del_type = entity_info
            .and_then(|(e, _)| e.del.as_ref())
            .and_then(|d| d.type_delete.as_deref())
            .unwrap_or("soft");

        requests.push(SavedRequest {
            id: format!("req_del_{route}_{}", chrono::Utc::now().timestamp_millis()),
            workspace_id: ws_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            name: format!("Delete {route}"),
            url: format!("{{{{base_url}}}}/{route}/{{{{id}}}}"),
            method: HttpMethod::DELETE,
            params: Vec::new(),
            headers: default_headers.clone(),
            body_type: HttpBodyType::NoBody,
            auth_type: base_auth.clone(),
            bearer_token: base_token.clone(),
            description: format!(
                "Hapus record `{route}` berdasarkan `{pk_field}` (Tipe: {del_type} delete)."
            ),
            tables: tables.clone(),
            source: source_str.clone(),
            route: Some(format!("/{route}/{{{pk_field}}}")),
            ..Default::default()
        });
    }

    // 6. PATCH /<route> (jika diaktifkan)
    let patch_enabled = entity_info
        .and_then(|(e, _)| e.patch.as_ref())
        .and_then(|p| p.enable_method)
        .unwrap_or(false);

    if patch_enabled {
        requests.push(SavedRequest {
            id: format!(
                "req_patch_{route}_{}",
                chrono::Utc::now().timestamp_millis()
            ),
            workspace_id: ws_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            name: format!("Patch {route}"),
            url: format!("{{{{base_url}}}}/{route}"),
            method: HttpMethod::PATCH,
            params: Vec::new(),
            headers: json_headers.clone(),
            body_type: HttpBodyType::Json,
            body_text: "{}".to_string(),
            auth_type: base_auth.clone(),
            bearer_token: base_token.clone(),
            description: format!("Prosedur PATCH untuk `{route}`."),
            tables: tables.clone(),
            source: source_str.clone(),
            route: Some(format!("/{route}")),
            ..Default::default()
        });
    }

    // 7. Utility: GET /validate/<route>
    requests.push(SavedRequest {
        id: format!("req_val_{route}_{}", chrono::Utc::now().timestamp_millis()),
        workspace_id: ws_id.to_string(),
        folder_id: Some(folder_id.to_string()),
        name: format!("Validate {route} Schema"),
        url: format!("{{{{base_url}}}}/validate/{route}"),
        method: HttpMethod::GET,
        params: Vec::new(),
        headers: default_headers,
        body_type: HttpBodyType::NoBody,
        auth_type: base_auth.clone(),
        bearer_token: base_token.clone(),
        description: format!(
            "Validasi kesesuaian skema entity JSON `{route}` dengan struktur database."
        ),
        tables: tables.clone(),
        source: source_str.clone(),
        route: Some(format!("/validate/{route}")),
        ..Default::default()
    });

    let count = requests.len();
    (
        HttpFolder {
            id: folder_id.to_string(),
            name: route.to_string(),
            parent_folder_id: None,
            requests,
            children: Vec::new(),
            repo_url: None,
        },
        count,
    )
}

/// Request untuk public endpoint seperti login/register.
fn build_public_route_request(ws_id: &str, folder_id: &str, pub_route: &str) -> SavedRequest {
    let is_login = pub_route.eq_ignore_ascii_case("login");
    let is_register = pub_route.eq_ignore_ascii_case("register");

    let body_text = if is_login {
        serde_json::json!({
            "email": "admin",
            "password": "1234"
        })
        .to_string()
    } else if is_register {
        serde_json::json!({
            "email": "user@example.com",
            "password": "Password123!",
            "name": "New User"
        })
        .to_string()
    } else {
        "{}".to_string()
    };

    SavedRequest {
        id: format!(
            "req_pub_{pub_route}_{}",
            chrono::Utc::now().timestamp_millis()
        ),
        workspace_id: ws_id.to_string(),
        folder_id: Some(folder_id.to_string()),
        name: if is_login {
            "Login (Auth)".to_string()
        } else if is_register {
            "Register (Auth)".to_string()
        } else {
            format!("Public {pub_route}")
        },
        url: format!("{{{{base_url}}}}/{pub_route}"),
        method: HttpMethod::POST,
        params: Vec::new(),
        headers: vec![
            (
                "Content-Type".to_string(),
                "application/json".to_string(),
                true,
            ),
            ("Accept".to_string(), "application/json".to_string(), true),
        ],
        body_type: HttpBodyType::Json,
        body_text,
        auth_type: HttpAuthType::NoAuth,
        bearer_token: String::new(),
        description: format!("Endpoint publik `{pub_route}`."),
        tables: Vec::new(),
        source: None,
        route: Some(format!("/{pub_route}")),
        ..Default::default()
    }
}

// ─── Generator Sample JSON Body ──────────────────────────────────────────────

/// Membangun representasi JSON contoh dari daftar kolom dan relasi details.
pub(crate) fn build_sample_json_body(
    columns: &[String],
    column_defs: &HashMap<String, FlexurioColumn>,
    details: &[FlexurioDetail],
    initial_status: Option<&str>,
) -> String {
    let mut map = serde_json::Map::new();

    for raw_col in columns {
        let col_name = raw_col.trim().trim_end_matches('*');
        if col_name.is_empty() {
            continue;
        }

        // Kolom id jika auto_increment tidak perlu di-generate di body create
        if col_name == "id" {
            if let Some(c) = column_defs.get("id") {
                if c.auto_increment == Some(true) {
                    continue;
                }
            }
        }

        let val = if let Some(c) = column_defs.get(col_name) {
            sample_value_for_column(col_name, c, initial_status)
        } else {
            fallback_sample_value(col_name, initial_status)
        };

        map.insert(col_name.to_string(), val);
    }

    // Tambahkan master-detail sample jika ada
    for detail in details {
        if detail.field.is_empty() {
            continue;
        }
        let mut child_map = serde_json::Map::new();
        for child_col in &detail.columns {
            let col = child_col.trim().trim_end_matches('*');
            if !col.is_empty() {
                child_map.insert(col.to_string(), fallback_sample_value(col, None));
            }
        }
        if child_map.is_empty() {
            child_map.insert("item_name".to_string(), serde_json::json!("Sample Item"));
            child_map.insert("qty".to_string(), serde_json::json!(1));
        }
        map.insert(
            detail.field.clone(),
            serde_json::Value::Array(vec![serde_json::Value::Object(child_map)]),
        );
    }

    serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .unwrap_or_else(|_| "{}".to_string())
}

/// Menghasilkan nilai sample berdasarkan definisi tipe data kolom Flexurio.
fn sample_value_for_column(
    name: &str,
    col: &FlexurioColumn,
    initial_status: Option<&str>,
) -> serde_json::Value {
    let type_data = col.type_data.to_ascii_lowercase();

    // Jika memiliki nilai default di skema
    if let Some(def) = &col.default {
        if !def.is_null() {
            if let Some(s) = def.as_str() {
                if s != "CURRENT_TIMESTAMP" && !s.is_empty() {
                    return serde_json::json!(s);
                }
            } else {
                return def.clone();
            }
        }
    }

    if type_data.contains("bool") {
        return serde_json::json!(true);
    }
    if type_data.contains("tinyint") {
        if name.starts_with("is_") || name.starts_with("has_") {
            return serde_json::json!(1);
        }
        return serde_json::json!(1);
    }
    if type_data.contains("int") || type_data.contains("bigint") || type_data.contains("serial") {
        return serde_json::json!(1);
    }
    if type_data.contains("decimal")
        || type_data.contains("numeric")
        || type_data.contains("float")
        || type_data.contains("double")
    {
        return serde_json::json!(100.00);
    }
    if type_data.contains("datetime") || type_data.contains("timestamp") {
        return serde_json::json!("2026-10-02T10:00:00Z");
    }
    if type_data.contains("date") {
        return serde_json::json!("2026-10-02");
    }
    if type_data.contains("time") {
        return serde_json::json!("10:00:00");
    }
    if type_data.contains("json") {
        return serde_json::json!({});
    }

    fallback_sample_value(name, initial_status)
}

/// Nilai sample fallback berdasarkan nama field umum.
fn fallback_sample_value(name: &str, initial_status: Option<&str>) -> serde_json::Value {
    let lower = name.to_ascii_lowercase();
    if lower == "status" {
        return serde_json::json!(initial_status.unwrap_or("DRAFT"));
    }
    if lower.contains("email") {
        return serde_json::json!("user@example.com");
    }
    if lower.contains("phone") || lower.contains("telp") {
        return serde_json::json!("08123456789");
    }
    if lower.contains("password") {
        return serde_json::json!("secret123");
    }
    if lower.contains("nip") {
        return serde_json::json!("19800101");
    }
    if lower.starts_with("is_") || lower.starts_with("has_") {
        return serde_json::json!(false);
    }
    if lower.ends_with("_id") {
        return serde_json::json!(1);
    }
    if lower.contains("count") || lower.contains("qty") || lower.contains("number") {
        return serde_json::json!(1);
    }
    if lower.contains("price") || lower.contains("amount") || lower.contains("cost") {
        return serde_json::json!(100000);
    }
    if lower.contains("date") {
        return serde_json::json!("2026-10-02");
    }

    serde_json::json!(format!("Sample {name}"))
}

// ─── Unit Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flexurio_routes() {
        let json = r#"{
            "routes": ["users", "products"],
            "route_publics": ["login"]
        }"#;
        let routes: FlexurioRoutes = serde_json::from_str(json).unwrap();
        assert_eq!(routes.routes, vec!["users", "products"]);
        assert_eq!(routes.route_publics, vec!["login"]);
    }

    #[test]
    fn test_parse_flexurio_entity() {
        let json = r#"{
            "table": "users",
            "primary_key": { "columns": ["id"] },
            "columns": [
                { "name": "id", "type_data": "bigint", "auto_increment": true },
                { "name": "email", "type_data": "varchar(100)", "nullable": false }
            ],
            "get": {
                "enable_method": true,
                "columns": ["users.id", "users.email"],
                "parameters": ["email.eq", "limit", "page"]
            },
            "post": {
                "enable_method": true,
                "columns": ["email*", "name"]
            },
            "put": {
                "enable_method": true,
                "columns": ["name"]
            },
            "del": {
                "enable_method": true,
                "type_delete": "soft"
            }
        }"#;
        let entity: FlexurioEntity = serde_json::from_str(json).unwrap();
        assert_eq!(entity.table, "users");
        assert_eq!(entity.columns.len(), 2);
        assert_eq!(entity.get.as_ref().unwrap().parameters.len(), 3);
        assert_eq!(entity.post.as_ref().unwrap().columns.len(), 2);
    }

    #[test]
    fn test_build_sample_json_body() {
        let mut col_defs = HashMap::new();
        col_defs.insert(
            "email".to_string(),
            FlexurioColumn {
                name: "email".to_string(),
                type_data: "varchar(100)".to_string(),
                ..Default::default()
            },
        );
        col_defs.insert(
            "age".to_string(),
            FlexurioColumn {
                name: "age".to_string(),
                type_data: "int".to_string(),
                ..Default::default()
            },
        );

        let cols = vec!["email*".to_string(), "age".to_string()];
        let body_str = build_sample_json_body(&cols, &col_defs, &[], None);
        let val: serde_json::Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(val["email"], "user@example.com");
        assert_eq!(val["age"], 1);
    }

    #[test]
    fn test_build_route_folder() {
        let entity = FlexurioEntity {
            table: "products".to_string(),
            primary_key: Some(FlexurioPrimaryKey {
                columns: vec!["id".to_string()],
                auto_increment: Some(true),
            }),
            columns: vec![FlexurioColumn {
                name: "name".to_string(),
                type_data: "varchar(50)".to_string(),
                ..Default::default()
            }],
            get: Some(FlexurioGet {
                enable_method: Some(true),
                parameters: vec!["name.like".to_string()],
                ..Default::default()
            }),
            post: Some(FlexurioPostPut {
                enable_method: Some(true),
                columns: vec!["name*".to_string()],
                ..Default::default()
            }),
            put: Some(FlexurioPostPut {
                enable_method: Some(true),
                columns: vec!["name".to_string()],
                ..Default::default()
            }),
            del: Some(FlexurioDelete {
                enable_method: Some(true),
                type_delete: Some("soft".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let dummy_path = PathBuf::from("config/entity/products.json");
        let (folder, count) = build_route_folder(
            "ws_1",
            "fld_1",
            "products",
            Some(&(entity, dummy_path)),
            &[],
        );

        assert_eq!(folder.name, "products");
        // GET list, GET one, POST, PUT, DELETE, Validate = 6 requests
        assert_eq!(count, 6);
        assert_eq!(folder.requests.len(), 6);
        assert_eq!(folder.requests[0].method, HttpMethod::GET);
        assert_eq!(folder.requests[0].tables, vec!["products"]);
        assert_eq!(folder.requests[0].url, "{{base_url}}/products");
    }

    #[test]
    fn test_import_from_real_or_temp_dir() {
        let real_path = PathBuf::from(
            "/Users/jayuda/Documents/PROJECT/MF/erp-flexurio-plant-cpob/binaries/backend/config/routes.json",
        );
        if real_path.exists() {
            let res = import_from_flexurio(&real_path)
                .expect("Gagal mengimpor konfigurasi Flexurio riil");
            assert!(!res.workspaces.is_empty());
            assert!(res.total_requests > 0);
            assert!(res.total_routes > 0);
            println!(
                "Sukses mengimpor {} request di {} route dari konfigurasi Flexurio riil!",
                res.total_requests, res.total_routes
            );
        }
    }

    #[test]
    fn test_detect_flexurio_config() {
        // 1. Uji deteksi pada direktori riil jika ada
        let repo_root = PathBuf::from("/Users/jayuda/Documents/PROJECT/MF/erp-flexurio-plant-cpob");
        if repo_root.exists() {
            let detected = detect_flexurio_config(&repo_root);
            assert!(
                detected.is_some(),
                "detect_flexurio_config harus mendeteksi format Flexurio dari root repo"
            );

            let backend_dir = repo_root.join("binaries/backend");
            let detected_backend = detect_flexurio_config(&backend_dir);
            assert!(
                detected_backend.is_some(),
                "detect_flexurio_config harus mendeteksi format Flexurio dari backend dir"
            );

            let config_dir = backend_dir.join("config");
            let detected_config = detect_flexurio_config(&config_dir);
            assert!(
                detected_config.is_some(),
                "detect_flexurio_config harus mendeteksi format Flexurio dari folder config langsung"
            );
        }

        // 2. Uji deteksi sintetis menggunakan direktori sementara
        let temp_dir = std::env::temp_dir().join(format!(
            "flx_test_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let cfg_dir = temp_dir.join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let routes_file = cfg_dir.join("routes.json");
        std::fs::write(
            &routes_file,
            r#"{"routes": ["users", "orders"], "public": ["login"]}"#,
        )
        .unwrap();

        let detected = detect_flexurio_config(&temp_dir);
        assert_eq!(detected, Some(routes_file.clone()));

        let detected_from_config = detect_flexurio_config(&cfg_dir);
        assert_eq!(detected_from_config, Some(routes_file.clone()));

        let detected_from_file = detect_flexurio_config(&routes_file);
        assert_eq!(detected_from_file, Some(routes_file));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
