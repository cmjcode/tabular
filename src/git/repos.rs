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
    /// Project Tabular pemilik folder ini (diisi saat ditambahkan ketika
    /// sebuah project sedang dipilih).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// Gaya garis graf.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphStyle {
    #[default]
    Rounded,
    Angular,
}

/// Format tanggal kolom Date di Git Graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateFormat {
    #[default]
    DateTime,
    Date,
    Relative,
}

/// Penyedia avatar author.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AvatarSource {
    /// Tanpa avatar jaringan: inisial berwarna.
    #[default]
    Initials,
    /// Gravatar (hash MD5 email dikirim ke gravatar.com).
    Gravatar,
}

/// State UI per repository yang diingat antar sesi.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoPrefs {
    /// Sub-tab terakhir: Changes / Branches / History / Review.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub: String,
    #[serde(default)]
    pub expanded: bool,
    /// Code review Git Graph: rentang → file yang sudah dilihat.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reviewed: BTreeMap<String, Vec<String>>,
    /// Branch yang dipilih di filter graf (kosong = semua).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graph_branches: Vec<String>,
}

/// Batas jumlah rentang code review yang disimpan per repository.
pub const MAX_REVIEWS: usize = 30;

impl RepoPrefs {
    pub fn mark_reviewed(&mut self, range: &str, path: &str) {
        let files = self.reviewed.entry(range.to_string()).or_default();
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
        while self.reviewed.len() > MAX_REVIEWS {
            let Some(k) = self.reviewed.keys().next().cloned() else {
                break;
            };
            self.reviewed.remove(&k);
        }
    }

    pub fn is_reviewed(&self, range: &str, path: &str) -> bool {
        self.reviewed
            .get(range)
            .is_some_and(|f| f.iter().any(|x| x == path))
    }
}

/// Pengaturan tab Git. Personal (tidak ikut sync); token ada di keychain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitSettings {
    /// URL GitLab (self-hosted atau gitlab.com).
    #[serde(default = "default_gitlab_url")]
    pub gitlab_url: String,
    /// Bahasa jawaban review AI.
    #[serde(default = "default_language")]
    pub review_language: String,
    /// Interval refresh daftar merge request (menit); 0 = mati.
    #[serde(default = "default_refresh_min")]
    pub auto_refresh_min: u32,
    /// Tampilkan juga MR yang sudah merged/closed (mode repository aktif).
    #[serde(default)]
    pub show_closed: bool,
    /// Pull memakai `--rebase` alih-alih `--ff-only`.
    #[serde(default)]
    pub pull_rebase: bool,
    // ── Git Graph ──
    #[serde(default)]
    pub graph_order: crate::git::history::CommitOrder,
    #[serde(default)]
    pub graph_style: GraphStyle,
    #[serde(default)]
    pub date_format: DateFormat,
    #[serde(default)]
    pub avatars: AvatarSource,
    #[serde(default = "yes")]
    pub show_date: bool,
    #[serde(default = "yes")]
    pub show_author: bool,
    #[serde(default = "yes")]
    pub show_commit: bool,
    #[serde(default = "yes")]
    pub show_remote_branches: bool,
    #[serde(default = "yes")]
    pub show_tags: bool,
    #[serde(default = "yes")]
    pub show_stashes: bool,
    #[serde(default = "yes")]
    pub show_uncommitted: bool,
    #[serde(default)]
    pub first_parent: bool,
    #[serde(default)]
    pub show_reflog: bool,
    /// Redupkan merge commit seperti Git Graph.
    #[serde(default = "yes")]
    pub mute_merges: bool,
    #[serde(default = "yes")]
    pub check_signatures: bool,
    #[serde(default = "default_page")]
    pub graph_page: usize,
    #[serde(default = "yes")]
    pub fetch_prune: bool,
    #[serde(default)]
    pub fetch_prune_tags: bool,
    /// Pola URL issue, mis. `https://jira.example.com/browse/$1`; kosong =
    /// tautan otomatis GitHub/GitLab dari remote.
    #[serde(default)]
    pub issue_url: String,
    /// Regex nomor issue; kosong = `#(\d+)`.
    #[serde(default)]
    pub issue_regex: String,
    /// Lebar kolom Date, Author, Commit (px).
    #[serde(default = "default_widths")]
    pub column_widths: [f32; 3],
}

fn yes() -> bool {
    true
}

fn default_page() -> usize {
    300
}

fn default_widths() -> [f32; 3] {
    [130.0, 140.0, 80.0]
}

fn default_gitlab_url() -> String {
    crate::git::review::DEFAULT_GITLAB_URL.to_string()
}

fn default_language() -> String {
    "English".to_string()
}

fn default_refresh_min() -> u32 {
    5
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            gitlab_url: default_gitlab_url(),
            review_language: default_language(),
            auto_refresh_min: default_refresh_min(),
            show_closed: false,
            pull_rebase: false,
            graph_order: Default::default(),
            graph_style: GraphStyle::default(),
            date_format: DateFormat::default(),
            avatars: AvatarSource::default(),
            show_date: true,
            show_author: true,
            show_commit: true,
            show_remote_branches: true,
            show_tags: true,
            show_stashes: true,
            show_uncommitted: true,
            first_parent: false,
            show_reflog: false,
            mute_merges: true,
            check_signatures: true,
            graph_page: default_page(),
            fetch_prune: true,
            fetch_prune_tags: false,
            issue_url: String::new(),
            issue_regex: String::new(),
            column_widths: default_widths(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileBody {
    #[serde(default)]
    repos: Vec<ManualRepo>,
    /// Kunci repository yang terakhir dipilih.
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    settings: GitSettings,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    repo_prefs: BTreeMap<String, RepoPrefs>,
}

/// Repository yang ditambahkan langsung di tab Git.
#[derive(Debug)]
pub struct GitRepoStore {
    file: PathBuf,
    pub repos: Vec<ManualRepo>,
    pub active: Option<String>,
    pub settings: GitSettings,
    /// State UI per kunci repository.
    pub prefs: BTreeMap<String, RepoPrefs>,
}

impl GitRepoStore {
    /// Muat dari `file`; file yang belum ada dianggap kosong. File yang rusak
    /// dipindahkan ke `<nama>.corrupt-<timestamp>` dulu supaya `save()`
    /// berikutnya tidak menimpa daftar repository milik user.
    pub fn load(file: PathBuf) -> Self {
        let body = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<FileBody>(&bytes).unwrap_or_else(|e| {
                log::warn!("[GIT] ignoring unreadable {}: {e}", file.display());
                let _ = crate::directory::quarantine_corrupt_file(&file);
                FileBody::default()
            }),
            Err(_) => FileBody::default(),
        };
        Self {
            file,
            repos: body.repos,
            active: body.active,
            settings: body.settings,
            prefs: body.repo_prefs,
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
            settings: self.settings.clone(),
            repo_prefs: self.prefs.clone(),
        };
        let json = serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?;
        crate::directory::write_file_atomically(&self.file, &json)
    }

    /// Tambah folder; `false` bila sudah ada.
    pub fn add(&mut self, path: &str) -> bool {
        self.add_to_project(path, None)
    }

    /// Tambah folder milik `project_id`. Folder yang sudah ada tetapi belum
    /// punya project ikut ditandai.
    pub fn add_to_project(&mut self, path: &str, project_id: Option<&str>) -> bool {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        if let Some(r) = self.repos.iter_mut().find(|r| same_path(&r.path, path)) {
            if r.project_id.is_none() && project_id.is_some() {
                r.project_id = project_id.map(str::to_string);
            }
            return false;
        }
        self.repos.push(ManualRepo {
            path: path.to_string(),
            project_id: project_id.map(str::to_string),
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
    /// Project Tabular pemilik tautan ini.
    pub project: Option<String>,
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
    /// Repository dipakai oleh project `id` (lewat tautan mana pun).
    pub fn in_project(&self, id: &str) -> bool {
        self.links.iter().any(|l| l.project.as_deref() == Some(id))
    }

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
pub fn merge(
    sources: &[LinkSource],
    remote_of: impl Fn(&Path) -> Option<String>,
) -> Vec<RepoEntry> {
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
            e.links
                .sort_by(|a, b| (a.kind, &a.label).cmp(&(b.kind, &b.label)));
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
            LinkKind::Diagram => {
                crate::diagram_repo_paths::set_local_repo_path(&link.id, Some(&path_s))?
            }
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
        let d =
            std::env::temp_dir().join(format!("tabular-git-repos-{tag}-{}", std::process::id()));
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
            project: None,
        }
    }

    #[test]
    fn merges_by_repo_key_across_sources() {
        let dir = tmp("merge");
        let sources = vec![
            src(
                LinkKind::Diagram,
                "g1",
                Some("git@github.com:Org/App.git"),
                None,
            ),
            src(
                LinkKind::HttpFolder,
                "f1",
                Some("https://github.com/org/app"),
                Some(&dir),
            ),
            src(LinkKind::Manual, "m", None, Some(&dir)),
            src(
                LinkKind::Project,
                "p",
                Some("https://gitlab.com/x/other"),
                None,
            ),
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
    fn project_scope_and_prefs() {
        let dir = tmp("scope");
        let mut a = src(
            LinkKind::Project,
            "p1",
            Some("https://github.com/o/a"),
            None,
        );
        a.project = Some("p1".into());
        let b = src(LinkKind::Manual, "m", Some("https://github.com/o/b"), None);
        let repos = merge(&[a, b], |_| None);
        assert!(repos[0].in_project("p1"));
        assert!(!repos[1].in_project("p1"));

        let file = dir.join(FILE_NAME);
        let mut s = GitRepoStore::load(file.clone());
        assert!(s.add_to_project(&dir.to_string_lossy(), None));
        assert!(!s.add_to_project(&dir.to_string_lossy(), Some("p1")));
        assert_eq!(s.repos[0].project_id.as_deref(), Some("p1"));
        let p = s.prefs.entry("k".into()).or_default();
        p.expanded = true;
        p.mark_reviewed("abc", "x.rs");
        p.mark_reviewed("abc", "x.rs");
        s.save().expect("save");
        let s2 = GitRepoStore::load(file);
        assert!(s2.prefs["k"].expanded);
        assert!(s2.prefs["k"].is_reviewed("abc", "x.rs"));
        assert_eq!(s2.prefs["k"].reviewed["abc"].len(), 1);
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
        assert_eq!(s2.settings, GitSettings::default());
        assert_eq!(s2.repos.len(), 1);
        assert_eq!(s2.active.as_deref(), Some("k"));
        std::fs::write(&file, "{broken").expect("write");
        assert!(GitRepoStore::load(file).repos.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_store_is_preserved_before_next_save() {
        let dir = tmp("corrupt");
        let file = dir.join(FILE_NAME);
        std::fs::write(&file, "{broken").expect("write");
        let mut s = GitRepoStore::load(file.clone());
        assert!(s.repos.is_empty());
        // File rusak sudah disisihkan, bukan dibiarkan untuk ditimpa.
        assert!(!file.exists());
        assert!(s.add(&dir.to_string_lossy()));
        s.save().expect("save");
        let backups: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains(".corrupt-"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&backups[0]).expect("read backup"),
            "{broken"
        );
        assert_eq!(GitRepoStore::load(file).repos.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
