//! Project: satu root folder yang menyatukan Connections, Queries, dan HTTP.
//!
//! Keanggotaan ditentukan oleh path, bukan tabel relasi: koneksi milik project
//! bila `folder`-nya sama dengan `connection_folder` atau subfoldernya, file
//! query milik project bila ada di bawah `query/{query_folder}`, dan request
//! HTTP milik project bila ada di workspace `http_workspace_id`. Karena itu
//! folder lama bisa diangkat menjadi project tanpa migrasi data.
//!
//! Disimpan di `{app_data}/projects/{id}/project.json`. Nilai variabel rahasia
//! tidak pernah ditulis ke JSON; nilainya ada di keychain (`crate::secrets`)
//! dengan nama dari [`secret_name`]. Memory agent per project ada di
//! `{app_data}/projects/{id}/memory/` (lihat [`crate::project_memory`]).
//!
//! Modul ini headless: tidak bergantung pada `window_egui`, sehingga bisa
//! dipakai GUI, agent MCP, dan test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::connection_env::{self, Environment};
use crate::http_collection::HttpWorkspace;
use crate::models::structs::ConnectionConfig;

/// Nama file manifest di dalam folder project.
const MANIFEST_FILE: &str = "project.json";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("Project name cannot be empty")]
    EmptyName,
    #[error("Project name cannot contain '/', '\\' or '..', and cannot be 'Default'")]
    InvalidName,
    #[error("A project named '{0}' already exists")]
    Duplicate(String),
    #[error("Project not found: {0}")]
    NotFound(String),
    #[error("Invalid value: {0}")]
    Invalid(String),
    #[error("File error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid project file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Satu variabel environment (`KEY=value`).
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    /// Nilai variabel biasa. Selalu kosong untuk variabel rahasia; nilainya
    /// dibaca dari keychain.
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub secret: bool,
    /// Hanya di memori: nilai secret yang sedang diedit di dialog. Tidak
    /// pernah diserialisasi; ditulis ke keychain saat project disimpan.
    #[serde(skip)]
    pub pending_secret: Option<String>,
}

/// Satu environment project, mis. Development atau Production.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ProjectEnv {
    pub name: String,
    /// Jenis environment (`production`, `staging`, ...) untuk warna badge.
    /// Kosong berarti ditebak dari nama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub variables: Vec<EnvVar>,
    /// Nama koneksi (di folder project) yang dipakai environment ini. Nama,
    /// bukan id, karena id koneksi lokal per mesin sedangkan project bisa
    /// dibagikan ke tim.
    #[serde(default)]
    pub connections: Vec<String>,
}

impl ProjectEnv {
    pub fn new(name: &str, kind: Option<Environment>) -> Self {
        Self {
            name: name.to_string(),
            kind: kind.map(|k| k.key().to_string()),
            variables: Vec::new(),
            connections: Vec::new(),
        }
    }

    /// Jenis environment efektif: eksplisit, atau tebakan dari nama.
    pub fn environment(&self) -> Option<Environment> {
        self.kind
            .as_deref()
            .and_then(Environment::parse)
            .or_else(|| connection_env::detect_from_name(&self.name))
    }
}

/// Environment bawaan untuk project baru.
pub fn default_environments() -> Vec<ProjectEnv> {
    vec![
        ProjectEnv::new("Development", Some(Environment::Development)),
        ProjectEnv::new("Staging", Some(Environment::Staging)),
        ProjectEnv::new("Production", Some(Environment::Production)),
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Path folder koneksi (root, tanpa `/` di depan).
    pub connection_folder: String,
    /// Nama folder di bawah direktori query.
    pub query_folder: String,
    /// Workspace HTTP milik project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_workspace_id: Option<String>,
    #[serde(default)]
    pub environments: Vec<ProjectEnv>,
    /// Nama environment aktif.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_env: Option<String>,
    /// URL repository git kode project (tautan ke group diagram).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    /// Team tempat project dibagikan. Lokal: tidak ikut manifest bersama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_team_id: Option<String>,
    /// Unix timestamp (detik) perubahan terakhir.
    #[serde(default)]
    pub updated_at: i64,
    /// Id user pemilik bila project ini ditarik dari team milik orang lain.
    /// Lokal: tidak ikut manifest bersama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Id baris project di server (untuk update project milik orang lain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    /// Akses ke project milik orang lain: `editor` atau `viewer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
}

impl Project {
    /// Project baru dengan tiga folder bernama sama dan environment bawaan.
    pub fn new(name: &str) -> Self {
        let name = name.trim().to_string();
        let environments = default_environments();
        Self {
            id: crate::http_collection::unique_id("prj"),
            connection_folder: name.clone(),
            query_folder: name.clone(),
            name,
            description: String::new(),
            http_workspace_id: None,
            active_env: environments.first().map(|e| e.name.clone()),
            environments,
            repo_url: None,
            shared_team_id: None,
            updated_at: chrono::Utc::now().timestamp(),
            owner_id: None,
            remote_id: None,
            access: None,
        }
    }

    /// Project ini milik orang lain dan hanya boleh dibaca.
    pub fn is_read_only(&self) -> bool {
        self.owner_id.is_some() && self.access.as_deref() != Some("editor")
    }

    /// Salinan untuk dibagikan: tanpa nilai secret dan tanpa field lokal.
    pub fn shared_copy(&self) -> Project {
        let mut p = self.sanitized();
        p.shared_team_id = None;
        p.owner_id = None;
        p.remote_id = None;
        p.access = None;
        p
    }

    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp();
    }

    pub fn env(&self, name: &str) -> Option<&ProjectEnv> {
        self.environments.iter().find(|e| e.name == name)
    }

    /// Environment aktif; bila belum dipilih, environment pertama.
    pub fn active_environment(&self) -> Option<&ProjectEnv> {
        self.active_env
            .as_deref()
            .and_then(|n| self.env(n))
            .or_else(|| self.environments.first())
    }

    /// Koneksi dengan folder `folder` termasuk project ini.
    pub fn owns_connection_folder(&self, folder: Option<&str>) -> bool {
        folder.is_some_and(|f| path_in(f, &self.connection_folder))
    }

    pub fn owns_connection(&self, conn: &ConnectionConfig) -> bool {
        self.owns_connection_folder(conn.folder.as_deref())
    }

    /// Path relatif terhadap direktori query (`Proj/sub/a.sql`) termasuk
    /// project ini.
    pub fn owns_query_path(&self, relative: &str) -> bool {
        let rel = relative.replace('\\', "/");
        path_in(rel.trim_start_matches('/'), &self.query_folder)
    }

    /// Koneksi milik project, urut nama.
    pub fn member_connections<'a>(
        &self,
        connections: &'a [ConnectionConfig],
    ) -> Vec<&'a ConnectionConfig> {
        let mut out: Vec<_> = connections
            .iter()
            .filter(|c| self.owns_connection(c))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Variabel environment `env_name`. Nilai rahasia diambil lewat `lookup`
    /// (nama secret → nilai) sehingga fungsi ini bisa diuji tanpa keychain.
    pub fn env_vars_with(
        &self,
        env_name: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> HashMap<String, String> {
        let Some(env) = self.env(env_name) else {
            return HashMap::new();
        };
        env.variables
            .iter()
            .filter(|v| !v.key.trim().is_empty())
            .filter_map(|v| {
                let value = if v.secret {
                    lookup(&secret_name(&self.id, &env.name, &v.key))?
                } else {
                    v.value.clone()
                };
                Some((v.key.trim().to_string(), value))
            })
            .collect()
    }

    /// Variabel environment aktif, dengan secret dari keychain.
    pub fn active_vars(&self) -> HashMap<String, String> {
        match self.active_environment() {
            Some(env) => self.env_vars_with(&env.name, crate::secrets::get_secret),
            None => HashMap::new(),
        }
    }

    /// Nilai semua secret project (untuk disensor dari memory agent).
    pub fn secret_values(&self) -> Vec<String> {
        self.environments
            .iter()
            .flat_map(|env| {
                env.variables.iter().filter(|v| v.secret).filter_map(|v| {
                    crate::secrets::get_secret(&secret_name(&self.id, &env.name, &v.key))
                })
            })
            .filter(|v| v.len() >= 4)
            .collect()
    }

    /// Salinan yang aman ditulis ke disk atau dibagikan: nilai variabel
    /// rahasia dikosongkan.
    pub fn sanitized(&self) -> Project {
        let mut p = self.clone();
        for env in &mut p.environments {
            for v in &mut env.variables {
                v.pending_secret = None;
                if v.secret {
                    v.value.clear();
                }
            }
        }
        p
    }

    /// Nama semua secret keychain yang dipakai project ini.
    pub fn secret_names(&self) -> Vec<String> {
        self.environments
            .iter()
            .flat_map(|env| {
                env.variables
                    .iter()
                    .filter(|v| v.secret && !v.key.trim().is_empty())
                    .map(|v| secret_name(&self.id, &env.name, &v.key))
            })
            .collect()
    }

    /// Isi `pending_secret` semua variabel rahasia dari `lookup` (dipakai
    /// saat dialog edit dibuka).
    pub fn load_pending_secrets(&mut self, lookup: impl Fn(&str) -> Option<String>) {
        let id = self.id.clone();
        for env in &mut self.environments {
            for v in &mut env.variables {
                if v.secret {
                    v.pending_secret =
                        Some(lookup(&secret_name(&id, &env.name, &v.key)).unwrap_or_default());
                }
            }
        }
    }

    /// Pasangan `(nama secret, nilai)` yang harus ditulis ke keychain dari
    /// `pending_secret`.
    pub fn pending_secret_writes(&self) -> Vec<(String, String)> {
        self.environments
            .iter()
            .flat_map(|env| {
                env.variables.iter().filter_map(move |v| {
                    (v.secret && !v.key.trim().is_empty())
                        .then(|| v.pending_secret.clone())
                        .flatten()
                        .map(|val| (secret_name(&self.id, &env.name, &v.key), val))
                })
            })
            .collect()
    }

    /// Pasangan `(resource_type, folder_path)` yang dibagikan ke team saat
    /// project di-share. Format path mengikuti modul sync masing-masing.
    pub fn share_targets(&self, http_workspace_name: Option<&str>) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("connection", self.connection_folder.clone()),
            ("query", format!("/{}", self.query_folder)),
        ];
        if let Some(ws) = http_workspace_name {
            out.push(("http", format!("/{}", ws)));
        }
        out.push(("project", self.name.clone()));
        out
    }

    /// Koneksi yang dipakai environment `env_name`. Bila ada beberapa,
    /// utamakan yang jenis database-nya sama dengan `current`.
    pub fn connection_for_env(
        &self,
        env_name: &str,
        connections: &[ConnectionConfig],
        current: Option<&ConnectionConfig>,
    ) -> Option<i64> {
        let env = self.env(env_name)?;
        let candidates: Vec<&ConnectionConfig> = self
            .member_connections(connections)
            .into_iter()
            .filter(|c| env.connections.iter().any(|n| n == &c.name))
            .collect();
        let preferred = current.and_then(|cur| {
            candidates
                .iter()
                .find(|c| c.connection_type == cur.connection_type)
        });
        preferred.or(candidates.first()).and_then(|c| c.id)
    }

    /// Tanda environment koneksi `(connection_id, env)` sesuai pemetaan
    /// koneksi per environment; dipakai untuk strip warna tab.
    pub fn environment_marks(&self, connections: &[ConnectionConfig]) -> Vec<(i64, Environment)> {
        let members = self.member_connections(connections);
        let mut out = Vec::new();
        for env in &self.environments {
            let Some(kind) = env.environment() else {
                continue;
            };
            for name in &env.connections {
                if let Some(id) = members.iter().find(|c| &c.name == name).and_then(|c| c.id) {
                    out.push((id, kind));
                }
            }
        }
        out
    }
}

/// `path` sama dengan `root` atau berada di bawahnya.
pub fn path_in(path: &str, root: &str) -> bool {
    let path = path.trim().trim_end_matches('/');
    let root = root.trim().trim_end_matches('/');
    !root.is_empty()
        && (path == root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// Nama secret keychain untuk satu variabel rahasia.
pub fn secret_name(project_id: &str, env: &str, key: &str) -> String {
    format!("project:{}:{}:{}", project_id, env, key.trim())
}

/// Ganti `{{KEY}}` yang dikenal dengan nilainya; placeholder lain dibiarkan.
pub fn substitute(text: &str, vars: &HashMap<String, String>) -> String {
    if vars.is_empty() {
        return text.to_string();
    }
    crate::http_tests::substitute(text, vars)
}

/// Validasi dan normalisasi nama project. `exclude_id` untuk mode edit.
pub fn validate_name(
    name: &str,
    existing: &[Project],
    exclude_id: Option<&str>,
) -> Result<String, ProjectError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProjectError::EmptyName);
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.eq_ignore_ascii_case("default")
    {
        return Err(ProjectError::InvalidName);
    }
    if existing
        .iter()
        .any(|p| Some(p.id.as_str()) != exclude_id && p.name.eq_ignore_ascii_case(trimmed))
    {
        return Err(ProjectError::Duplicate(trimmed.to_string()));
    }
    Ok(trimmed.to_string())
}

// ─── Penyimpanan ──────────────────────────────────────────────────────────────

/// State UI project yang diingat antar sesi.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UiState {
    /// Project yang dipilih di switcher header.
    #[serde(default)]
    pub active_id: Option<String>,
    /// Sidebar hanya menampilkan folder project aktif.
    #[serde(default)]
    pub filter_only: bool,
    /// Nama project milik sendiri yang harus dihapus dari server (setelah
    /// project dihapus atau di-rename lokal).
    #[serde(default)]
    pub pending_remote_deletes: Vec<String>,
}

pub fn load_ui_state(app_dir: &Path) -> UiState {
    std::fs::read_to_string(projects_dir(app_dir).join("ui_state.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_ui_state(app_dir: &Path, state: &UiState) -> Result<(), ProjectError> {
    let json = serde_json::to_string_pretty(state)?;
    crate::directory::write_file_atomically(
        &projects_dir(app_dir).join("ui_state.json"),
        json.as_bytes(),
    )?;
    Ok(())
}

pub fn projects_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("projects")
}

pub fn project_dir(app_dir: &Path, project_id: &str) -> PathBuf {
    projects_dir(app_dir).join(sanitize_id(project_id))
}

/// Id dipakai sebagai nama folder; buang karakter yang bisa keluar folder.
fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// Semua project, urut nama. File rusak dilewati (dicatat di log).
pub fn load_all(app_dir: &Path) -> Vec<Project> {
    let Ok(entries) = std::fs::read_dir(projects_dir(app_dir)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(MANIFEST_FILE);
        if !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(&path)
            .map_err(ProjectError::from)
            .and_then(|s| serde_json::from_str::<Project>(&s).map_err(ProjectError::from))
        {
            Ok(p) => out.push(p),
            Err(e) => log::warn!("[PROJECT] Skipping {}: {}", path.display(), e),
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// Simpan project (atomik). Nilai rahasia tidak pernah ditulis.
pub fn save(app_dir: &Path, project: &Project) -> Result<(), ProjectError> {
    if sanitize_id(&project.id).is_empty() {
        return Err(ProjectError::Invalid("empty project id".into()));
    }
    let json = serde_json::to_string_pretty(&project.sanitized())?;
    let path = project_dir(app_dir, &project.id).join(MANIFEST_FILE);
    crate::directory::write_file_atomically(&path, json.as_bytes())?;
    Ok(())
}

/// Hapus manifest, memory, dan secret project. Folder koneksi, query, dan
/// workspace HTTP tidak disentuh.
pub fn delete(app_dir: &Path, project: &Project) -> Result<(), ProjectError> {
    for env in &project.environments {
        for v in env.variables.iter().filter(|v| v.secret) {
            crate::secrets::delete_secret(&secret_name(&project.id, &env.name, &v.key));
        }
    }
    let dir = project_dir(app_dir, &project.id);
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

// ─── Scaffold folder ─────────────────────────────────────────────────────────

/// Buat folder query project bila belum ada.
pub fn ensure_query_folder(query_root: &Path, folder: &str) -> Result<PathBuf, ProjectError> {
    let path = query_root.join(folder);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// Pastikan project punya workspace HTTP: pakai yang tercatat, adopsi
/// workspace dengan nama sama, atau buat baru. Mengembalikan `true` bila
/// `workspaces` berubah dan perlu disimpan.
pub fn ensure_http_workspace(workspaces: &mut Vec<HttpWorkspace>, project: &mut Project) -> bool {
    if let Some(id) = &project.http_workspace_id
        && workspaces.iter().any(|w| &w.id == id)
    {
        return false;
    }
    if let Some(ws) = workspaces
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(&project.name))
    {
        project.http_workspace_id = Some(ws.id.clone());
        return false;
    }
    let ws = HttpWorkspace {
        id: crate::http_collection::unique_id("ws"),
        name: project.name.clone(),
        requests: Vec::new(),
        folders: Vec::new(),
        environments: Vec::new(),
    };
    project.http_workspace_id = Some(ws.id.clone());
    workspaces.push(ws);
    true
}

/// Catat folder koneksi project di `connection_folders`.
pub async fn ensure_connection_folder(pool: &SqlitePool, path: &str) -> Result<(), ProjectError> {
    sqlx::query("INSERT OR IGNORE INTO connection_folders (path) VALUES (?)")
        .bind(path)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::DatabaseType;

    fn conn(id: i64, name: &str, folder: &str, ty: DatabaseType) -> ConnectionConfig {
        ConnectionConfig {
            id: Some(id),
            name: name.to_string(),
            folder: Some(folder.to_string()),
            connection_type: ty,
            ..Default::default()
        }
    }

    fn temp_app_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tabular-project-test-{}-{}-{}",
            tag,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn path_membership_respects_segment_boundaries() {
        assert!(path_in("Shop", "Shop"));
        assert!(path_in("Shop/api", "Shop"));
        assert!(!path_in("Shopping", "Shop"));
        assert!(!path_in("Shop", ""));
        let p = Project::new("Shop");
        assert!(p.owns_query_path("Shop/reports/a.sql"));
        assert!(p.owns_query_path("/Shop"));
        assert!(!p.owns_query_path("Shopping/a.sql"));
        assert!(p.owns_connection_folder(Some("Shop/replicas")));
        assert!(!p.owns_connection_folder(None));
    }

    #[test]
    fn validates_names() {
        let existing = vec![Project::new("Shop")];
        assert!(matches!(
            validate_name("  ", &existing, None),
            Err(ProjectError::EmptyName)
        ));
        assert!(matches!(
            validate_name("a/b", &existing, None),
            Err(ProjectError::InvalidName)
        ));
        assert!(matches!(
            validate_name("Default", &existing, None),
            Err(ProjectError::InvalidName)
        ));
        assert!(matches!(
            validate_name("shop", &existing, None),
            Err(ProjectError::Duplicate(_))
        ));
        assert_eq!(
            validate_name("shop", &existing, Some(&existing[0].id)).unwrap(),
            "shop"
        );
        assert_eq!(
            validate_name(" Billing ", &existing, None).unwrap(),
            "Billing"
        );
    }

    #[test]
    fn env_vars_resolve_secrets_through_lookup() {
        let mut p = Project::new("Shop");
        p.environments[0].variables = vec![
            EnvVar {
                key: "BASE_URL".into(),
                value: "http://localhost".into(),
                ..Default::default()
            },
            EnvVar {
                key: "TOKEN".into(),
                secret: true,
                ..Default::default()
            },
            EnvVar {
                key: "MISSING".into(),
                secret: true,
                ..Default::default()
            },
        ];
        let token_name = secret_name(&p.id, "Development", "TOKEN");
        let vars = p.env_vars_with("Development", |n| {
            (n == token_name).then(|| "s3cret".to_string())
        });
        assert_eq!(
            vars.get("BASE_URL").map(String::as_str),
            Some("http://localhost")
        );
        assert_eq!(vars.get("TOKEN").map(String::as_str), Some("s3cret"));
        assert!(!vars.contains_key("MISSING"));
        assert!(p.env_vars_with("Nope", |_| None).is_empty());
    }

    #[test]
    fn substitutes_only_known_variables() {
        let vars: HashMap<String, String> = [("SCHEMA".to_string(), "sales".to_string())].into();
        assert_eq!(
            substitute("SELECT '{{x}}' FROM {{SCHEMA}}.orders", &vars),
            "SELECT '{{x}}' FROM sales.orders"
        );
        assert_eq!(substitute("{{SCHEMA}}", &HashMap::new()), "{{SCHEMA}}");
    }

    #[test]
    fn sanitized_blanks_secret_values_and_roundtrips() {
        let dir = temp_app_dir("roundtrip");
        let mut p = Project::new("Shop");
        p.environments[2].variables.push(EnvVar {
            key: "DB_PASS".into(),
            value: "leak".into(),
            secret: true,
            pending_secret: Some("leak2".into()),
        });
        assert_eq!(
            p.pending_secret_writes(),
            vec![(
                secret_name(&p.id, "Production", "DB_PASS"),
                "leak2".to_string()
            )]
        );
        save(&dir, &p).unwrap();
        let raw = std::fs::read_to_string(project_dir(&dir, &p.id).join(MANIFEST_FILE)).unwrap();
        assert!(!raw.contains("leak"));
        assert_eq!(p.secret_names().len(), 1);
        let loaded = load_all(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], p.sanitized());
        delete(&dir, &p).unwrap();
        assert!(load_all(&dir).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn picks_connection_for_environment() {
        let mut p = Project::new("Shop");
        p.environments[0].connections = vec!["shop-dev-pg".into(), "shop-dev-redis".into()];
        p.environments[2].connections = vec!["shop-prod-pg".into()];
        let conns = vec![
            conn(1, "shop-dev-pg", "Shop", DatabaseType::PostgreSQL),
            conn(2, "shop-dev-redis", "Shop", DatabaseType::Redis),
            conn(3, "shop-prod-pg", "Shop/prod", DatabaseType::PostgreSQL),
            conn(4, "shop-prod-pg", "Other", DatabaseType::PostgreSQL),
        ];
        assert_eq!(
            p.connection_for_env("Production", &conns, Some(&conns[0])),
            Some(3)
        );
        assert_eq!(
            p.connection_for_env("Development", &conns, Some(&conns[1])),
            Some(2)
        );
        assert_eq!(p.connection_for_env("Staging", &conns, None), None);
        let marks = p.environment_marks(&conns);
        assert!(marks.contains(&(1, Environment::Development)));
        assert!(marks.contains(&(3, Environment::Production)));
        assert!(!marks.iter().any(|(id, _)| *id == 4));
    }

    #[test]
    fn adopts_or_creates_http_workspace() {
        let mut p = Project::new("Shop");
        let mut ws = vec![HttpWorkspace {
            id: "ws_1".into(),
            name: "shop".into(),
            ..Default::default()
        }];
        assert!(!ensure_http_workspace(&mut ws, &mut p));
        assert_eq!(p.http_workspace_id.as_deref(), Some("ws_1"));
        let mut q = Project::new("Billing");
        assert!(ensure_http_workspace(&mut ws, &mut q));
        assert_eq!(ws.len(), 2);
        assert!(!ensure_http_workspace(&mut ws, &mut q));
    }

    #[test]
    fn share_targets_cover_all_tools() {
        let p = Project::new("Shop");
        let t = p.share_targets(Some("Shop"));
        assert_eq!(
            t,
            vec![
                ("connection", "Shop".to_string()),
                ("query", "/Shop".to_string()),
                ("http", "/Shop".to_string()),
                ("project", "Shop".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn records_connection_folder() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE connection_folders (path TEXT NOT NULL UNIQUE)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_connection_folder(&pool, "Shop").await.unwrap();
        ensure_connection_folder(&pool, "Shop").await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM connection_folders")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
}
