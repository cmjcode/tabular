//! Daftar repository untuk tab Git.
//!
//! Sumbernya digabung dari tiga tempat yang memakai URL git yang sama:
//! - group diagram (`DiagramGroup::repo_url` + folder personal di
//!   `diagram_repo_paths.json`),
//! - folder HTTP API (`HttpFolder::repo_url` + kunci `http:{id}` di file yang sama),
//! - project (`Project::repo_url`),
//! - repository yang ditambahkan langsung di tab Git (`git_repos.json`, personal,
//!   tidak ikut sync).
//!
//! Entri dengan kunci repository sama (lihat [`crate::repo_scan::repo_key`])
//! menjadi satu, sehingga folder yang sudah di-set di Diagram atau HTTP API
//! langsung terbaca di Git, dan clone dari Git mengisi folder mereka.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::repo_scan::{expand_home, repo_key};

/// Nama file di `{data_dir}`.
pub const FILE_NAME: &str = "git_repos.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManualRepo {
    pub path: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileBody {
    #[serde(default)]
    repos: Vec<ManualRepo>,
    /// Kunci repository yang terakhir dipilih.
    #[serde(default)]
    active: Option<String>,
}

/// Repository yang ditambahkan langsung di tab Git.
#[derive(Debug)]
pub struct GitRepoStore {
    file: PathBuf,
    pub repos: Vec<ManualRepo>,
    pub active: Option<String>,
}

impl GitRepoStore {
    /// Muat dari `file`; file yang belum ada atau rusak dianggap kosong.
    pub fn load(file: PathBuf) -> Self {
        let body = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<FileBody>(&bytes).unwrap_or_else(|e| {
                log::warn!("[GIT] ignoring unreadable {}: {e}", file.display());
                FileBody::default()
            }),
            Err(_) => FileBody::default(),
        };
        Self {
            file,
            repos: body.repos,
            active: body.active,
        }
    }

    pub fn default_file() -> PathBuf {
        crate::config::get_data_dir().join(FILE_NAME)
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = FileBody {
            repos: self.repos.clone(),
            active: self.active.clone(),
        };
        let json = serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?;
        std::fs::write(&self.file, json)
    }

    /// Tambah folder; `false` bila sudah ada.
    pub fn add(&mut self, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty() || self.repos.iter().any(|r| same_path(&r.path, path)) {
            return false;
        }
        self.repos.push(ManualRepo {
            path: path.to_string(),
        });
        true
    }

    pub fn remove(&mut self, path: &str) {
        self.repos.retain(|r| !same_path(&r.path, path));
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let p = expand_home(s.trim());
        p.canonicalize().unwrap_or(p)
    };
    norm(a) == norm(b)
}

/// Asal sebuah tautan repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkKind {
    Diagram,
    HttpFolder,
    Project,
    Manual,
}

impl LinkKind {
    pub fn label(self) -> &'static str {
        match self {
            LinkKind::Diagram => "Diagram",
            LinkKind::HttpFolder => "API",
            LinkKind::Project => "Project",
            LinkKind::Manual => "Git",
        }
    }
}

/// Satu pemakai repository (group diagram, folder API, project, atau manual).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSource {
    pub kind: LinkKind,
    /// Id group / folder / project; path untuk manual.
    pub id: String,
    pub label: String,
    pub url: Option<String>,
    /// Folder lokal yang tercatat (belum tentu ada di disk).
    pub path: Option<String>,
}

/// Repository gabungan yang tampil di tab Git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    /// Kunci stabil: URL ternormalisasi, atau `path:{folder}` tanpa remote.
    pub key: String,
    pub name: String,
    /// Working tree yang ada di komputer ini.
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub links: Vec<LinkSource>,
}

impl RepoEntry {
    /// Tautan diagram/API yang belum punya folder lokal; diisi saat clone
    /// atau lewat "Use this folder for linked items".
    pub fn links_without_folder(&self) -> impl Iterator<Item = &LinkSource> {
        self.links.iter().filter(|l| {
            matches!(l.kind, LinkKind::Diagram | LinkKind::HttpFolder)
                && l.path
                    .as_deref()
                    .map(expand_home)
                    .is_none_or(|p| !p.is_dir())
        })
    }
}

/// Gabungkan sumber menjadi daftar repository. `remote_of` membaca URL remote
/// sebuah folder (di produksi [`crate::repo_scan::git_remote_url`]).
pub fn merge(sources: &[LinkSource], remote_of: impl Fn(&Path) -> Option<String>) -> Vec<RepoEntry> {
    let mut by_key: BTreeMap<String, RepoEntry> = BTreeMap::new();
    for src in sources {
        let dir = src
            .path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(expand_home)
            .filter(|p| p.is_dir());
        let url = src
            .url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or_else(|| dir.as_deref().and_then(&remote_of));
        let key = url.as_deref().and_then(repo_key).or_else(|| {
            dir.as_ref().map(|d| {
                let d = d.canonicalize().unwrap_or_else(|_| d.clone());
                format!("path:{}", d.display())
            })
        });
        let Some(key) = key else {
            continue;
        };
        let entry = by_key.entry(key.clone()).or_insert_with(|| RepoEntry {
            key: key.clone(),
            name: String::new(),
            path: None,
            url: None,
            links: Vec::new(),
        });
        if entry.path.is_none() {
            entry.path = dir;
        }
        if entry.url.is_none() {
            entry.url = url;
        }
        entry.links.push(src.clone());
    }
    let mut out: Vec<RepoEntry> = by_key
        .into_values()
        .map(|mut e| {
            e.links.sort_by(|a, b| (a.kind, &a.label).cmp(&(b.kind, &b.label)));
            e.name = display_name(&e);
            e
        })
        .collect();
    out.sort_by(|a, b| {
        (a.path.is_none(), a.name.to_lowercase()).cmp(&(b.path.is_none(), b.name.to_lowercase()))
    });
    out
}

fn display_name(e: &RepoEntry) -> String {
    if let Some(name) = e.path.as_deref().and_then(Path::file_name) {
        return name.to_string_lossy().into_owned();
    }
    if let Some(last) = e.key.rsplit('/').next().filter(|s| !s.is_empty()) {
        return last.to_string();
    }
    e.key.clone()
}

/// Catat `path` sebagai folder project untuk tautan diagram/API yang belum
/// punya folder. Mengembalikan jumlah tautan yang diperbarui.
pub fn assign_folder(entry: &RepoEntry, path: &Path) -> Result<usize, String> {
    let path_s = path.to_string_lossy().to_string();
    let mut n = 0;
    for link in entry.links_without_folder() {
        match link.kind {
            LinkKind::Diagram => crate::diagram_repo_paths::set_local_repo_path(&link.id, Some(&path_s))?,
            LinkKind::HttpFolder => {
                crate::diagram_repo_paths::set_http_folder_repo_path(&link.id, Some(&path_s))?
            }
            _ => continue,
        }
        n += 1;
    }
    Ok(n)
}

/// Folder tujuan clone default: `~/Projects/<nama repo>` (atau home bila tidak ada).
pub fn default_clone_dir(url: &str) -> PathBuf {
    let name = repo_key(url)
        .and_then(|k| k.rsplit('/').next().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "repository".to_string());
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let base = ["Projects", "projects", "Developer", "src"]
        .iter()
        .map(|d| home.join(d))
        .find(|p| p.is_dir())
        .unwrap_or(home);
    base.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tabular-git-repos-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    fn src(kind: LinkKind, id: &str, url: Option<&str>, path: Option<&Path>) -> LinkSource {
        LinkSource {
            kind,
            id: id.to_string(),
            label: id.to_string(),
            url: url.map(str::to_string),
            path: path.map(|p| p.to_string_lossy().to_string()),
        }
    }

    #[test]
    fn merges_by_repo_key_across_sources() {
        let dir = tmp("merge");
        let sources = vec![
            src(LinkKind::Diagram, "g1", Some("git@github.com:Org/App.git"), None),
            src(LinkKind::HttpFolder, "f1", Some("https://github.com/org/app"), Some(&dir)),
            src(LinkKind::Manual, "m", None, Some(&dir)),
            src(LinkKind::Project, "p", Some("https://gitlab.com/x/other"), None),
            src(LinkKind::Diagram, "empty", None, None),
        ];
        // Folder manual tidak punya `.git`; remote dibaca lewat closure.
        let remote = |_: &Path| Some("https://github.com/org/app.git".to_string());
        let repos = merge(&sources, remote);
        assert_eq!(repos.len(), 2, "{repos:?}");
        let app = &repos[0];
        assert_eq!(app.key, "github.com/org/app");
        assert_eq!(app.links.len(), 3);
        assert!(app.path.is_some());
        assert_eq!(app.links_without_folder().count(), 1);
        let other = &repos[1];
        assert!(other.path.is_none());
        assert_eq!(other.name, "other");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_without_remote_uses_path_key() {
        let dir = tmp("local");
        let repos = merge(&[src(LinkKind::Manual, "m", None, Some(&dir))], |_| None);
        assert_eq!(repos.len(), 1);
        assert!(repos[0].key.starts_with("path:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_roundtrip_and_dedup() {
        let dir = tmp("store");
        let file = dir.join(FILE_NAME);
        let mut s = GitRepoStore::load(file.clone());
        assert!(s.add(&dir.to_string_lossy()));
        assert!(!s.add(&dir.to_string_lossy()));
        s.active = Some("k".into());
        s.save().expect("save");
        let s2 = GitRepoStore::load(file.clone());
        assert_eq!(s2.repos.len(), 1);
        assert_eq!(s2.active.as_deref(), Some("k"));
        std::fs::write(&file, "{broken").expect("write");
        assert!(GitRepoStore::load(file).repos.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
