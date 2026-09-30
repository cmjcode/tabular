//! Project untuk agent: daftar project, konteks satu project (environment,
//! koneksi per environment, folder query, workspace HTTP), dan memory agent
//! per project.
//!
//! Nilai variabel rahasia tidak pernah dikirim ke agent; hanya kuncinya.
//! Koneksi yang diblokir untuk client ini tidak ikut ditampilkan.

use serde::Serialize;

use super::core::{AgentError, HeadlessSession};
use crate::project::{self, Project};
use crate::project_memory::{self, MemoryEntry};

/// Jumlah maksimum nama file query per project di konteks.
const MAX_QUERY_FILES: usize = 200;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub active_environment: Option<String>,
    pub environments: Vec<String>,
    pub memory_entries: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct EnvVarInfo {
    pub key: String,
    /// Kosong untuk variabel rahasia.
    pub value: Option<String>,
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProjectEnvInfo {
    pub name: String,
    /// `production`, `staging`, `development`, `testing`, `local`.
    pub kind: Option<&'static str>,
    pub active: bool,
    pub variables: Vec<EnvVarInfo>,
    /// Id koneksi yang dipakai environment ini (hanya yang boleh diakses).
    pub connection_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProjectConnectionInfo {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProjectContext {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repo_url: Option<String>,
    pub environments: Vec<ProjectEnvInfo>,
    pub connections: Vec<ProjectConnectionInfo>,
    /// Path file query relatif terhadap folder query project.
    pub query_files: Vec<String>,
    pub http_workspace: Option<String>,
    pub http_request_count: usize,
    pub memory: Vec<MemoryEntry>,
    pub how_to_use: &'static str,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SavedMemory {
    pub project: String,
    pub name: String,
    pub path: String,
}

impl HeadlessSession {
    fn find_project(&self, key: &str) -> Result<Project, AgentError> {
        let all = project::load_all(self.app_dir());
        let key = key.trim();
        all.iter()
            .find(|p| p.id == key)
            .or_else(|| all.iter().find(|p| p.name.eq_ignore_ascii_case(key)))
            .cloned()
            .ok_or_else(|| {
                AgentError::Refused(format!(
                    "project '{}' not found; call list_projects for valid names",
                    key
                ))
            })
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectSummary>, AgentError> {
        let app_dir = self.app_dir().to_path_buf();
        Ok(project::load_all(&app_dir)
            .into_iter()
            .map(|p| ProjectSummary {
                memory_entries: project_memory::list(&app_dir, &p.id).len(),
                active_environment: p.active_environment().map(|e| e.name.clone()),
                environments: p.environments.iter().map(|e| e.name.clone()).collect(),
                id: p.id,
                name: p.name,
                description: p.description,
            })
            .collect())
    }

    pub async fn project_context(
        &self,
        client: &str,
        key: &str,
    ) -> Result<ProjectContext, AgentError> {
        let p = self.find_project(key)?;
        let app_dir = self.app_dir().to_path_buf();
        let visible = self.list_connections_for(client).await?;
        let connections: Vec<ProjectConnectionInfo> = visible
            .iter()
            .filter(|c| p.owns_connection_folder(c.summary.folder.as_deref()))
            .map(|c| ProjectConnectionInfo {
                id: c.summary.id,
                name: c.summary.name.clone(),
                kind: c.summary.kind.clone(),
                folder: c.summary.folder.clone(),
            })
            .collect();
        let active = p.active_environment().map(|e| e.name.clone());
        let environments = p
            .environments
            .iter()
            .map(|env| ProjectEnvInfo {
                name: env.name.clone(),
                kind: env.environment().map(|k| k.key()),
                active: active.as_deref() == Some(env.name.as_str()),
                variables: env
                    .variables
                    .iter()
                    .filter(|v| !v.key.trim().is_empty())
                    .map(|v| EnvVarInfo {
                        key: v.key.clone(),
                        value: (!v.secret).then(|| v.value.clone()),
                        secret: v.secret,
                    })
                    .collect(),
                connection_ids: connections
                    .iter()
                    .filter(|c| env.connections.contains(&c.name))
                    .map(|c| c.id)
                    .collect(),
            })
            .collect();

        let query_root = crate::directory::get_query_dir().join(&p.query_folder);
        let mut query_files = Vec::new();
        collect_files(&query_root, &query_root, &mut query_files);
        query_files.sort();
        query_files.truncate(MAX_QUERY_FILES);

        let workspaces = crate::http_collection::load_workspaces();
        let ws = p
            .http_workspace_id
            .as_deref()
            .and_then(|id| workspaces.iter().find(|w| w.id == id));
        let http_request_count = ws
            .map(|w| {
                w.requests.len()
                    + w.folders
                        .iter()
                        .map(|f| f.all_requests().len())
                        .sum::<usize>()
            })
            .unwrap_or(0);

        Ok(ProjectContext {
            memory: project_memory::list(&app_dir, &p.id),
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            repo_url: p.repo_url.clone(),
            environments,
            connections,
            query_files,
            http_workspace: ws.map(|w| w.name.clone()),
            http_request_count,
            how_to_use: "Use connection ids from the active environment by default. SQL and HTTP \
                         in this project may contain {{KEY}} placeholders that Tabular fills from \
                         the active environment. Project memory holds the team's durable facts; \
                         store new ones with save_project_memory, never secrets or query results.",
        })
    }

    pub async fn save_project_memory(
        &self,
        key: &str,
        title: &str,
        description: &str,
        content: &str,
    ) -> Result<SavedMemory, AgentError> {
        let p = self.find_project(key)?;
        if p.is_read_only() {
            return Err(AgentError::Refused(
                "this project is shared with you read-only; its memory cannot be changed".into(),
            ));
        }
        let entry = project_memory::save(
            self.app_dir(),
            &p.id,
            title,
            description,
            content,
            &p.secret_values(),
        )
        .map_err(|e| AgentError::Refused(e.to_string()))?;
        let path = project_memory::memory_dir(self.app_dir(), &p.id)
            .join(format!("{}.md", entry.name))
            .display()
            .to_string();
        log::info!(
            "[AGENT] Saved project memory '{}' in '{}'",
            entry.name,
            p.name
        );
        Ok(SavedMemory {
            project: p.name,
            name: entry.name,
            path,
        })
    }

    pub async fn delete_project_memory(&self, key: &str, name: &str) -> Result<bool, AgentError> {
        let p = self.find_project(key)?;
        if p.is_read_only() {
            return Err(AgentError::Refused(
                "this project is shared with you read-only; its memory cannot be changed".into(),
            ));
        }
        project_memory::delete(self.app_dir(), &p.id, name)
            .map_err(|e| AgentError::Refused(e.to_string()))
    }
}

fn collect_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.extension().is_some_and(|x| x == "sql")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
        if out.len() >= MAX_QUERY_FILES {
            return;
        }
    }
}
