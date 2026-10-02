//! Pemindai repository kode untuk fitur "Suggest tables from repo" pada group
//! diagram.
//!
//! Alurnya headless (tanpa `Tabular`/egui) supaya bisa diuji:
//! 1. [`RepoSource::parse`] + [`resolve_repo`]: URL git di-clone dangkal ke
//!    cache, path lokal dipakai langsung.
//! 2. [`grep_tables`]: pencarian teks deterministik nama tabel dengan bobot
//!    konteks SQL/ORM. Cepat, offline, dan menjadi cadangan bila AI gagal.
//! 3. [`build_scan_prompts`] + [`parse_ai_reply`]: backend AI yang sudah
//!    dikonfigurasi (agy, Claude Code, Gemini CLI, atau API) menyaring hasil.
//! 4. [`spawn_scan`]: menjalankan semuanya di thread terpisah dan melaporkan
//!    kemajuan lewat channel.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::agent::harness::{self, AgentEvent, ProgressStatus, ProgressStep};
use crate::ai_assistant::{ChatBackend, ChatWorkspace};
use crate::config::{AiBackend, CliAgentKind};

/// Batas jumlah file yang dipindai per repository.
pub const MAX_FILES: usize = 20_000;
/// File lebih besar dari ini dilewati (biasanya hasil build atau data).
pub(crate) const MAX_FILE_BYTES: u64 = 1_000_000;
/// Baris lebih panjang dari ini dianggap minified dan dilewati.
pub(crate) const MAX_LINE_BYTES: usize = 2_000;
/// Bukti (path:line) maksimum per tabel.
const MAX_EVIDENCE: usize = 5;
/// Batas waktu operasi git (clone/fetch).
const GIT_TIMEOUT: Duration = Duration::from_secs(300);
/// Batas waktu satu giliran AI.
const AI_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Tool Claude Code yang diizinkan saat membaca repository (semuanya read-only).
const READ_ONLY_TOOLS: [&str; 4] = ["Read", "Grep", "Glob", "LS"];

/// Direktori yang tidak pernah berisi kode aplikasi yang relevan.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "bower_components",
    "vendor",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".gradle",
    "coverage",
    ".cache",
    "Pods",
    "DerivedData",
];

/// Ekstensi biner / aset yang tidak perlu dibaca.
const SKIP_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "tiff", "svg", "pdf", "zip", "gz", "tgz",
    "bz2", "xz", "7z", "rar", "tar", "jar", "war", "class", "so", "dylib", "dll", "exe", "o", "a",
    "lib", "wasm", "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4", "mov", "avi", "wav", "ogg",
    "lock", "map", "db", "sqlite", "sqlite3", "bin", "dat", "pyc",
];

/// Kata sebelum nama tabel yang menandakan konteks SQL/ORM.
const STRONG_PREFIXES: &[&str] = &[
    "from",
    "join",
    "into",
    "update",
    "table",
    "tablename",
    "__tablename__",
    "table_name",
    "target_table",
    "db_table",
    "collection",
];

#[derive(Debug, thiserror::Error)]
pub enum RepoScanError {
    #[error("Repository is empty. Set a git URL or a local folder first.")]
    Empty,
    #[error("Unsupported repository address: {0}")]
    Unsupported(String),
    #[error("Folder not found: {0}")]
    NotFound(String),
    #[error("git is not installed or not found in PATH")]
    GitMissing,
    #[error("git {action} failed: {detail}")]
    Git {
        action: &'static str,
        detail: String,
    },
    #[error("Cancelled")]
    Cancelled,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("AI reply could not be parsed: {0}")]
    Parse(String),
    #[error("{0}")]
    Ai(String),
}

// ─── Sumber repository ───────────────────────────────────────────────────────

/// Alamat repository yang diisi user di group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoSource {
    /// URL git (https, ssh, git, file, atau bentuk scp `user@host:path`).
    Remote(String),
    /// Folder lokal yang sudah ada di disk.
    Local(PathBuf),
}

impl RepoSource {
    pub fn parse(input: &str) -> Result<Self, RepoScanError> {
        let s = input.trim();
        if s.is_empty() {
            return Err(RepoScanError::Empty);
        }
        // Jangan pernah biarkan input terbaca sebagai opsi git.
        if s.starts_with('-') {
            return Err(RepoScanError::Unsupported(s.to_string()));
        }
        let lower = s.to_ascii_lowercase();
        const SCHEMES: [&str; 5] = ["https://", "http://", "ssh://", "git://", "file://"];
        if SCHEMES.iter().any(|p| lower.starts_with(p)) || is_scp_like(s) {
            return Ok(Self::Remote(s.to_string()));
        }
        // Skema lain (mis. `ext::`) bisa menjalankan perintah lewat git.
        if lower.contains("://") || lower.contains("::") {
            return Err(RepoScanError::Unsupported(s.to_string()));
        }
        Ok(Self::Local(expand_home(s)))
    }
}

/// `git@github.com:org/repo.git`: ada `@` sebelum `:` dan tidak ada `/`
/// sebelum `:` (membedakan dari path biasa dan drive Windows `C:\`).
fn is_scp_like(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let head = &s[..colon];
    let Some(at) = head.find('@') else {
        return false;
    };
    at > 0
        && at + 1 < head.len()
        && !head.contains('/')
        && !head.contains('\\')
        && colon + 1 < s.len()
}

/// Ekspansi `~` ke direktori home.
pub fn expand_home(s: &str) -> PathBuf {
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// Kunci normal URL git untuk mencocokkan folder HTTP API dengan group
/// diagram yang menunjuk repository yang sama. `https://github.com/Org/App.git`,
/// `git@github.com:org/app` dan `ssh://git@github.com:22/org/app/` semuanya
/// menjadi `github.com/org/app`. Kredensial, port, `.git` dan huruf besar
/// dibuang. `None` bila bukan URL git.
pub fn repo_key(url: &str) -> Option<String> {
    let s = url.trim();
    if s.is_empty() {
        return None;
    }
    let (host, path) = if let Some((scheme, rest)) = s.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            let path = rest.trim_end_matches('/');
            let path = path.strip_suffix(".git").unwrap_or(path);
            return (!path.is_empty()).then(|| format!("file:{path}"));
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host.split(':').next().unwrap_or(host), path)
    } else if is_scp_like(s) {
        let (head, path) = s.split_once(':')?;
        (head.rsplit_once('@').map_or(head, |(_, h)| h), path)
    } else {
        return None;
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!(
        "{}/{}",
        host.to_ascii_lowercase(),
        path.to_ascii_lowercase()
    ))
}

/// Kunci repository dari URL git, atau bila kosong dari remote `.git/config`
/// folder project lokal.
pub fn repo_key_for(url: Option<&str>, path: Option<&str>) -> Option<String> {
    url.and_then(repo_key).or_else(|| {
        let path = path.map(str::trim).filter(|p| !p.is_empty())?;
        git_remote_url(&expand_home(path)).and_then(|u| repo_key(&u))
    })
}

/// Pilih sumber pemindaian: folder project bila ada di komputer ini, kalau
/// tidak URL git. Folder yang diisi tapi tidak ditemukan hanya jadi error bila
/// tidak ada URL cadangan.
pub fn choose_source(path: Option<&str>, url: Option<&str>) -> Result<RepoSource, RepoScanError> {
    let path = path.map(str::trim).filter(|p| !p.is_empty());
    let url = url.map(str::trim).filter(|u| !u.is_empty());
    if let Some(p) = path {
        let dir = expand_home(p);
        if dir.is_dir() {
            return Ok(RepoSource::Local(dir));
        }
        if url.is_none() {
            return Err(RepoScanError::NotFound(p.to_string()));
        }
        log::info!("[REPO_SCAN] folder {p} not found here, falling back to the git URL");
    }
    match url {
        Some(u) => RepoSource::parse(u),
        None => Err(RepoScanError::Empty),
    }
}

/// Clone penuh `url` ke folder project `dest` milik user (bukan cache), mis.
/// saat folder yang tercatat di group belum ada di komputer ini. `dest` harus
/// belum ada atau masih kosong.
pub fn clone_into(url: &str, dest: &Path, cancel: &AtomicBool) -> Result<(), RepoScanError> {
    let RepoSource::Remote(url) = RepoSource::parse(url)? else {
        return Err(RepoScanError::Unsupported(url.trim().to_string()));
    };
    if dest.exists() {
        let empty = dest.is_dir() && std::fs::read_dir(dest)?.next().is_none();
        if !empty {
            return Err(RepoScanError::Git {
                action: "clone",
                detail: format!("{} already exists and is not empty", dest.display()),
            });
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest_s = dest.to_string_lossy().to_string();
    log::info!("[REPO_SCAN] cloning {} into {dest_s}", redact(&url));
    run_git(&["clone", "--quiet", "--", &url, &dest_s], cancel).map_err(|detail| {
        if cancel.load(Ordering::SeqCst) {
            RepoScanError::Cancelled
        } else {
            git_error("clone", detail)
        }
    })
}

/// Direktori git (`.git`) untuk `dir` atau salah satu induknya. Mendukung
/// file `.git` berisi `gitdir: …` (worktree dan submodule).
fn find_git_dir(dir: &Path) -> Option<PathBuf> {
    for ancestor in dir.ancestors() {
        let dot_git = ancestor.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let body = std::fs::read_to_string(&dot_git).ok()?;
            let target = body
                .lines()
                .find_map(|l| l.trim().strip_prefix("gitdir:"))?;
            let target = PathBuf::from(target.trim());
            let resolved = if target.is_absolute() {
                target
            } else {
                ancestor.join(target)
            };
            // Worktree menyimpan config di direktori git utama (`commondir`).
            if let Ok(common) = std::fs::read_to_string(resolved.join("commondir")) {
                let common = PathBuf::from(common.trim());
                return Some(if common.is_absolute() {
                    common
                } else {
                    resolved.join(common)
                });
            }
            return Some(resolved);
        }
    }
    None
}

/// Ambil URL remote dari `.git/config` folder project: `origin` bila ada,
/// kalau tidak remote pertama yang punya `url`.
pub fn git_remote_url(dir: &Path) -> Option<String> {
    let git_dir = find_git_dir(dir)?;
    let config = std::fs::read_to_string(git_dir.join("config")).ok()?;
    parse_git_config_remote(&config)
}

fn parse_git_config_remote(config: &str) -> Option<String> {
    let mut current: Option<String> = None;
    let mut first: Option<String> = None;
    for raw in config.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // `[remote "origin"]` → Some("origin"); section lain → None.
            current = section
                .trim()
                .strip_prefix("remote")
                .map(|rest| rest.trim().trim_matches('"').to_string())
                .filter(|name| !name.is_empty());
            continue;
        }
        let Some(remote) = &current else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("url") {
            continue;
        }
        let value = value.trim().trim_matches('"').to_string();
        if value.is_empty() {
            continue;
        }
        if remote == "origin" {
            return Some(value);
        }
        first.get_or_insert(value);
    }
    first
}

/// URL membawa kredensial (`user:pass@`, atau token sebagai username seperti
/// `https://ghp_xxx@github.com`). URL group ikut disimpan di database dan
/// disinkron, jadi kredensial tidak boleh ada di dalamnya.
pub fn has_embedded_credentials(url: &str) -> bool {
    let url = url.trim();
    let Some((_, rest)) = url.split_once("://") else {
        return false; // bentuk scp `git@host:` hanya berisi username
    };
    let authority = rest.split('/').next().unwrap_or("");
    let Some((userinfo, _)) = authority.rsplit_once('@') else {
        return false;
    };
    const TOKEN_PREFIXES: [&str; 5] = ["ghp_", "gho_", "github_pat_", "glpat-", "x-token-auth"];
    userinfo.contains(':')
        || TOKEN_PREFIXES.iter().any(|p| userinfo.starts_with(p))
        || userinfo.len() >= 32
}

/// Sembunyikan kredensial `user:token@` di URL sebelum dicatat atau ditampilkan.
pub fn redact(text: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)[^/@\s]+@").expect("valid redact regex")
    });
    re.replace_all(text, "${1}***@").into_owned()
}

/// Repository yang siap dibaca.
#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    pub root: PathBuf,
    /// `true` bila `root` adalah salinan milik Tabular (clone di cache), jadi
    /// agent yang tidak bisa dibatasi read-only boleh bekerja di sana.
    pub isolated: bool,
}

fn cache_dir_for(cache_root: &Path, key: &str) -> PathBuf {
    cache_root.join(format!("{:x}", md5::compute(key.as_bytes())))
}

/// Siapkan repository: clone/perbarui URL ke cache, atau validasi folder lokal.
pub fn resolve_repo(
    source: &RepoSource,
    cache_root: &Path,
    cancel: &AtomicBool,
) -> Result<ResolvedRepo, RepoScanError> {
    match source {
        RepoSource::Local(path) => {
            let root = path
                .canonicalize()
                .map_err(|_| RepoScanError::NotFound(path.display().to_string()))?;
            if !root.is_dir() {
                return Err(RepoScanError::NotFound(root.display().to_string()));
            }
            Ok(ResolvedRepo {
                root,
                isolated: false,
            })
        }
        RepoSource::Remote(url) => {
            let dest = cache_dir_for(cache_root, url);
            sync_clone(url, &dest, true, cancel)?;
            Ok(ResolvedRepo {
                root: dest,
                isolated: true,
            })
        }
    }
}

/// Salinan terisolasi dari repository git lokal (commit terakhir, tanpa
/// perubahan yang belum di-commit). `None` bila folder bukan repository git.
pub fn isolated_copy(
    local_root: &Path,
    cache_root: &Path,
    cancel: &AtomicBool,
) -> Result<Option<PathBuf>, RepoScanError> {
    if !local_root.join(".git").exists() {
        return Ok(None);
    }
    let src = local_root.to_string_lossy().to_string();
    let dest = cache_dir_for(cache_root, &format!("local:{src}"));
    sync_clone(&src, &dest, false, cancel)?;
    Ok(Some(dest))
}

fn sync_clone(
    src: &str,
    dest: &Path,
    shallow: bool,
    cancel: &AtomicBool,
) -> Result<(), RepoScanError> {
    if dest.join(".git").is_dir() {
        let dest_s = dest.to_string_lossy().to_string();
        let mut fetch = vec!["-C", dest_s.as_str(), "fetch", "--quiet"];
        if shallow {
            fetch.extend(["--depth", "1"]);
        }
        fetch.extend(["origin", "HEAD"]);
        match run_git(&fetch, cancel) {
            Ok(()) => {
                run_git(
                    &["-C", &dest_s, "reset", "--quiet", "--hard", "FETCH_HEAD"],
                    cancel,
                )
                .map_err(|detail| git_error("reset", detail))?;
            }
            Err(e) if cancel.load(Ordering::SeqCst) => {
                log::debug!("[REPO_SCAN] fetch cancelled: {e}");
                return Err(RepoScanError::Cancelled);
            }
            // Offline atau remote tidak bisa diakses: pakai salinan terakhir.
            Err(e) => log::warn!(
                "[REPO_SCAN] fetch failed, using cached copy: {}",
                redact(&e)
            ),
        }
        return Ok(());
    }
    // Sisa clone yang gagal: direktori ini milik cache Tabular, aman dihapus.
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest_s = dest.to_string_lossy().to_string();
    let mut args = vec!["clone", "--quiet"];
    if shallow {
        args.extend(["--depth", "1"]);
    }
    args.extend(["--", src, dest_s.as_str()]);
    run_git(&args, cancel).map_err(|detail| {
        if cancel.load(Ordering::SeqCst) {
            RepoScanError::Cancelled
        } else {
            git_error("clone", detail)
        }
    })
}

fn git_error(action: &'static str, detail: String) -> RepoScanError {
    if detail == GIT_MISSING {
        RepoScanError::GitMissing
    } else {
        RepoScanError::Git {
            action,
            detail: redact(&detail),
        }
    }
}

const GIT_MISSING: &str = "git-missing";

/// Jalankan git tanpa prompt interaktif, dengan batas waktu dan pembatalan.
fn run_git(args: &[&str], cancel: &AtomicBool) -> Result<(), String> {
    run_git_output(args, cancel).map(|_| ())
}

/// Seperti [`run_git`], tetapi mengembalikan stdout. Implementasinya ada di
/// [`crate::git::cli`]; pesan error dipetakan ke format lama modul ini.
fn run_git_output(args: &[&str], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    use crate::git::GitError;
    crate::git::cli::run(None, args, cancel, GIT_TIMEOUT).map_err(|e| match e {
        GitError::GitMissing => GIT_MISSING.to_string(),
        GitError::Cancelled => "cancelled".to_string(),
        GitError::Command { detail, .. } | GitError::Auth { detail, .. } => detail,
        other => other.to_string(),
    })
}

// ─── Pencarian teks ──────────────────────────────────────────────────────────

/// Tabel diagram yang boleh disarankan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Id node di diagram.
    pub id: String,
    /// Nama tabel yang tampil (dicari di kode).
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    /// Path relatif terhadap root repository, dengan pemisah `/`.
    pub path: String,
    /// Nomor baris (mulai dari 1).
    pub line: usize,
    pub snippet: String,
    /// Ditemukan dalam konteks SQL/ORM (bukan sekadar teks berkutip).
    pub strong: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableHit {
    pub id: String,
    pub title: String,
    pub score: f32,
    pub refs: usize,
    pub files: usize,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanReport {
    pub files_scanned: usize,
    /// Batas [`MAX_FILES`] tercapai sebelum semua file dibaca.
    pub truncated: bool,
    /// Diurutkan dari skor tertinggi.
    pub hits: Vec<TableHit>,
}

/// Nama pendek tanpa skema/namespace (`public.users` → `users`).
fn short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_quote(b: u8) -> bool {
    matches!(b, b'\'' | b'"' | b'`')
}

/// File kode yang layak dibaca (bukan aset biner / hasil minify).
fn wanted_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    !SKIP_EXTS.contains(&ext) && !lower.ends_with(".min.js")
}

/// Daftar file menurut git (ter-track + baru yang tidak di-ignore), sehingga
/// `.gitignore` dihormati. `None` bila folder bukan repo git atau git tidak ada.
fn git_listed_files(
    root: &Path,
    cancel: &AtomicBool,
) -> Result<Option<(Vec<PathBuf>, bool)>, RepoScanError> {
    if find_git_dir(root).is_none() || harness::resolve_binary("git").is_none() {
        return Ok(None);
    }
    let root_s = root.to_string_lossy().to_string();
    let out = match run_git_output(
        &[
            "-C",
            &root_s,
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        cancel,
    ) {
        Ok(out) => out,
        Err(_) if cancel.load(Ordering::SeqCst) => return Err(RepoScanError::Cancelled),
        Err(e) => {
            log::debug!("[REPO_SCAN] git ls-files failed, walking folders instead: {e}");
            return Ok(None);
        }
    };
    let mut files = Vec::new();
    for rel in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rel = String::from_utf8_lossy(rel);
        let mut parts = rel.split('/');
        let name = parts.next_back().unwrap_or_default();
        if parts.any(|dir| SKIP_DIRS.contains(&dir)) || !wanted_file(name) {
            continue;
        }
        let path = root.join(rel.as_ref());
        // Symlink tidak diikuti (sama dengan walker).
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            continue;
        }
        if files.len() >= MAX_FILES {
            return Ok(Some((files, true)));
        }
        files.push(path);
    }
    files.sort();
    Ok(Some((files, false)))
}

/// Kumpulkan file yang layak dibaca, urut dan deterministik.
fn collect_files(root: &Path, cancel: &AtomicBool) -> Result<(Vec<PathBuf>, bool), RepoScanError> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::SeqCst) {
            return Err(RepoScanError::Cancelled);
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                log::debug!("[REPO_SCAN] skip unreadable dir {}: {e}", dir.display());
                continue;
            }
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for entry in entries {
            // `file_type()` tidak mengikuti symlink: link tidak pernah dipindai.
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    subdirs.push(entry.path());
                }
            } else if ft.is_file() {
                if !wanted_file(&name) {
                    continue;
                }
                if files.len() >= MAX_FILES {
                    return Ok((files, true));
                }
                files.push(entry.path());
            }
        }
        // Pop dari belakang: balik supaya urutan tetap alfabetis.
        stack.extend(subdirs.into_iter().rev());
    }
    Ok((files, false))
}

/// File kode di bawah `root`: daftar git bila tersedia (menghormati
/// `.gitignore`), kalau tidak telusuri folder. `true` = batas [`MAX_FILES`]
/// tercapai.
pub(crate) fn list_repo_files(
    root: &Path,
    cancel: &AtomicBool,
) -> Result<(Vec<PathBuf>, bool), RepoScanError> {
    match git_listed_files(root, cancel)? {
        Some(listed) => Ok(listed),
        None => collect_files(root, cancel),
    }
}

/// Cari nama tabel `candidates` di seluruh file teks di bawah `root`.
pub fn grep_tables(
    root: &Path,
    candidates: &[Candidate],
    cancel: &AtomicBool,
) -> Result<ScanReport, RepoScanError> {
    // nama (lowercase) → indeks kandidat
    let mut lookup: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        let mut names: HashSet<String> = HashSet::new();
        names.insert(c.title.to_ascii_lowercase());
        names.insert(short_name(&c.title).to_ascii_lowercase());
        for n in names {
            if n.len() >= 2 && n.bytes().all(is_ident_byte) {
                lookup.entry(n).or_default().push(i);
            }
        }
    }

    let (files, truncated) = list_repo_files(root, cancel)?;
    let mut score = vec![0.0f32; candidates.len()];
    let mut refs = vec![0usize; candidates.len()];
    let mut file_sets: Vec<HashSet<usize>> = vec![HashSet::new(); candidates.len()];
    let mut evidence: Vec<Vec<Evidence>> = vec![Vec::new(); candidates.len()];
    let mut files_scanned = 0usize;

    for (file_idx, path) in files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(RepoScanError::Cancelled);
        }
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue; // biner
        }
        files_scanned += 1;
        let text = String::from_utf8_lossy(&bytes);
        let is_sql = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("sql"));
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        for (line_no, line) in text.lines().enumerate() {
            if line.len() > MAX_LINE_BYTES {
                continue;
            }
            scan_line(line, is_sql, &lookup, |cand, strong, weight| {
                score[cand] += weight;
                refs[cand] += 1;
                file_sets[cand].insert(file_idx);
                let ev = &mut evidence[cand];
                let item = Evidence {
                    path: rel.clone(),
                    line: line_no + 1,
                    snippet: truncate_chars(line.trim(), 160),
                    strong,
                };
                if ev.len() < MAX_EVIDENCE {
                    ev.push(item);
                } else if strong && let Some(slot) = ev.iter_mut().find(|e| !e.strong) {
                    // Bukti konteks SQL lebih berguna daripada teks berkutip.
                    *slot = item;
                }
            });
        }
    }

    // Deteksi dan periksa konfigurasi Flexurio NoCode API (routes.json & entity/*.json)
    if let Some(routes_file) = crate::flexurio_import::detect_flexurio_config(root) {
        if let Ok(data) = std::fs::read_to_string(&routes_file) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(routes) = val.get("routes").and_then(|r| r.as_array()) {
                    let config_dir = routes_file.parent().unwrap_or(root);
                    let entity_dir = config_dir.join("entity");

                    for r in routes.iter().filter_map(|r| r.as_str()) {
                        let entity_file = entity_dir.join(format!("{r}.json"));
                        if !entity_file.is_file() {
                            continue;
                        }
                        let Ok(ent_str) = std::fs::read_to_string(&entity_file) else {
                            continue;
                        };
                        let Ok(ent) = serde_json::from_str::<crate::flexurio_import::FlexurioEntity>(
                            &ent_str,
                        ) else {
                            continue;
                        };
                        let ent_rel = entity_file
                            .strip_prefix(root)
                            .unwrap_or(&entity_file)
                            .to_string_lossy()
                            .replace('\\', "/");

                        let mut entity_tables = Vec::new();
                        if !ent.table.is_empty() {
                            entity_tables.push(ent.table.clone());
                        }
                        for d in &ent.details {
                            if !d.target_table.is_empty()
                                && !entity_tables.contains(&d.target_table)
                            {
                                entity_tables.push(d.target_table.clone());
                            }
                        }

                        for tbl in entity_tables {
                            let tbl_lower = tbl.to_ascii_lowercase();
                            if let Some(cands) = lookup.get(&tbl_lower) {
                                for &cand in cands {
                                    score[cand] += 10.0;
                                    refs[cand] += 1;
                                    let ev = &mut evidence[cand];
                                    let item = Evidence {
                                        path: ent_rel.clone(),
                                        line: 1,
                                        snippet: format!("Flexurio entity '{r}' -> table '{tbl}'"),
                                        strong: true,
                                    };
                                    if ev.len() < MAX_EVIDENCE {
                                        ev.push(item);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut hits: Vec<TableHit> = candidates
        .iter()
        .enumerate()
        .filter(|(i, _)| refs[*i] > 0)
        .map(|(i, c)| TableHit {
            id: c.id.clone(),
            title: c.title.clone(),
            score: score[i],
            refs: refs[i],
            files: file_sets[i].len(),
            evidence: std::mem::take(&mut evidence[i]),
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(ScanReport {
        files_scanned,
        truncated,
        hits,
    })
}

/// Tokenisasi satu baris dan laporkan setiap kecocokan nama tabel lewat
/// `on_hit(indeks_kandidat, konteks_sql, bobot)`.
fn scan_line(
    line: &str,
    is_sql: bool,
    lookup: &HashMap<String, Vec<usize>>,
    mut on_hit: impl FnMut(usize, bool, f32),
) {
    let bytes = line.as_bytes();
    let mut prev1: &str = "";
    let mut prev2: &str = "";
    let mut token = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_ident_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_ident_byte(bytes[i]) {
            i += 1;
        }
        let end = i;
        let raw = &line[start..end];
        token.clear();
        token.extend(raw.bytes().map(|b| b.to_ascii_lowercase() as char));

        if let Some(cands) = lookup.get(token.as_str()) {
            let strong = STRONG_PREFIXES
                .iter()
                .any(|p| p.eq_ignore_ascii_case(prev1))
                || (prev1.eq_ignore_ascii_case("name") && prev2.eq_ignore_ascii_case("table"));
            let quoted = start > 0
                && end < bytes.len()
                && is_quote(bytes[start - 1])
                && bytes[end] == bytes[start - 1];
            let weight = if strong {
                3.0
            } else if quoted {
                2.0
            } else if is_sql {
                1.0
            } else {
                0.0
            };
            if weight > 0.0 {
                for &c in cands {
                    on_hit(c, strong || is_sql, weight);
                }
            }
        }

        // Kualifier skema (`public.` di `FROM public.users`) tidak menggeser
        // konteks, supaya `users` tetap terbaca setelah `FROM`.
        let is_qualifier = end < bytes.len()
            && bytes[end] == b'.'
            && bytes.get(end + 1).is_some_and(|b| is_ident_byte(*b));
        if !is_qualifier {
            prev2 = prev1;
            prev1 = raw;
        }
    }
}

pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ─── Saran ───────────────────────────────────────────────────────────────────

/// Satu tabel yang disarankan untuk ditambahkan ke group.
#[derive(Debug, Clone, PartialEq)]
pub struct TableSuggestion {
    /// Id node di diagram.
    pub id: String,
    pub title: String,
    /// 0.0..=1.0
    pub confidence: f32,
    pub reason: String,
    /// `path:line` di repository.
    pub evidence: Vec<String>,
    /// Dikonfirmasi AI; `false` berarti hanya dari pencarian teks.
    pub from_ai: bool,
}

/// Konversi hasil pencarian teks menjadi saran.
pub fn suggestions_from_hits(report: &ScanReport) -> Vec<TableSuggestion> {
    report
        .hits
        .iter()
        .map(|h| TableSuggestion {
            id: h.id.clone(),
            title: h.title.clone(),
            confidence: 1.0 - (-h.score / 6.0).exp(),
            reason: format!(
                "Text search: {} reference(s) in {} file(s)",
                h.refs, h.files
            ),
            evidence: h
                .evidence
                .iter()
                .map(|e| format!("{}:{}", e.path, e.line))
                .collect(),
            from_ai: false,
        })
        .collect()
}

/// Gabungkan saran AI dengan hasil pencarian teks. Saran AI didahulukan;
/// tabel yang hanya ditemukan pencarian teks ditambahkan di belakang.
pub fn merge_suggestions(
    ai: Vec<TableSuggestion>,
    text: Vec<TableSuggestion>,
) -> Vec<TableSuggestion> {
    let mut out = ai;
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let seen: HashSet<String> = out.iter().map(|s| s.id.clone()).collect();
    out.extend(text.into_iter().filter(|s| !seen.contains(&s.id)));
    out
}

/// Cara AI mengakses kode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// Agent CLI berjalan di dalam repository dan membaca file sendiri.
    ReadRepo,
    /// AI hanya melihat cuplikan hasil pencarian teks di prompt.
    Snippets,
}

/// Batas byte daftar kandidat dan petunjuk di prompt.
const PROMPT_CANDIDATE_BYTES: usize = 20_000;
const PROMPT_HINT_BYTES_READ: usize = 20_000;
const PROMPT_HINT_BYTES_SNIPPETS: usize = 50_000;

pub fn build_scan_prompts(
    group_title: &str,
    members: &[String],
    candidates: &[Candidate],
    report: &ScanReport,
    mode: PromptMode,
    repo_root: Option<&Path>,
) -> (String, String) {
    let mut system = String::from(
        "You find which database tables a source code repository uses.\n\
         Include a table only when the code really reads or writes it: SQL strings, ORM \
         models/entities, migrations, repositories/DAOs or query builders. Ignore words that \
         only appear in comments, UI text or unrelated variable names.\n\
         ORM models often name no table at all; the framework derives it from the class name. \
         Count such a model as using that table, for example: Laravel Eloquent and Rails \
         ActiveRecord pluralise to snake_case (`class Invoice extends Model` uses `invoices`, \
         `OrderItem` uses `order_items`); Django uses `<app>_<model>`; Hibernate/JPA, EF Core, \
         GORM, Sequelize, TypeORM and Prisma use the entity name or its plural unless an \
         explicit table name is set. Look through model, entity, repository, migration and \
         query folders, not only SQL files.\n\
         Only use table names from the candidate list.\n\
         Never create, modify or delete files, and never run commands that change anything.\n\
         Reply with ONLY one JSON object, no prose and no markdown fence:\n\
         {\"tables\":[{\"name\":\"<table from the candidate list>\",\"confidence\":0.0,\
         \"reason\":\"<one short sentence>\",\"evidence\":[\"path/to/file:line\"]}]}\n",
    );
    match mode {
        PromptMode::ReadRepo => {
            match repo_root {
                Some(root) => system.push_str(&format!(
                    "The repository is at `{}`, which is also your current working \
                     directory. Stay inside it.",
                    root.display()
                )),
                None => system
                    .push_str("The repository is checked out in your current working directory."),
            }
            system.push_str(
                " Inspect it with your file reading and search tools. The text search hints \
                 below are a starting point and may contain false positives or miss tables.\n",
            );
        }
        PromptMode::Snippets => system.push_str(
            "You cannot open files. Judge only from the snippets below, which come from a \
             text search of the repository.\n",
        ),
    }

    let mut user = String::new();
    user.push_str(&format!(
        "Diagram group: {group_title}\nTables already in this group: {}\n\n",
        if members.is_empty() {
            "(none)".to_string()
        } else {
            members.join(", ")
        }
    ));

    // Kandidat: yang ditemukan pencarian teks lebih dulu, lalu sisanya.
    let hit_ids: HashSet<&str> = report.hits.iter().map(|h| h.id.as_str()).collect();
    let ordered = report.hits.iter().map(|h| h.title.as_str()).chain(
        candidates
            .iter()
            .filter(|c| !hit_ids.contains(c.id.as_str()))
            .map(|c| c.title.as_str()),
    );
    let mut list = String::new();
    let mut listed = 0usize;
    for name in ordered {
        if list.len() + name.len() + 2 > PROMPT_CANDIDATE_BYTES {
            break;
        }
        if !list.is_empty() {
            list.push_str(", ");
        }
        list.push_str(name);
        listed += 1;
    }
    user.push_str(&format!("Candidate tables ({}):\n{list}", candidates.len()));
    if listed < candidates.len() {
        user.push_str(&format!(
            "\n(list truncated, {} more not shown)",
            candidates.len() - listed
        ));
    }
    user.push_str("\n\n");

    let (budget, per_table) = match mode {
        PromptMode::ReadRepo => (PROMPT_HINT_BYTES_READ, 2),
        PromptMode::Snippets => (PROMPT_HINT_BYTES_SNIPPETS, MAX_EVIDENCE),
    };
    user.push_str(&format!(
        "Text search hints ({} files scanned{}):\n",
        report.files_scanned,
        if report.truncated {
            ", limit reached"
        } else {
            ""
        }
    ));
    if report.hits.is_empty() {
        user.push_str("(no matches)\n");
    }
    let mut used = 0usize;
    for h in &report.hits {
        let mut block = format!("- {} ({} refs in {} files)\n", h.title, h.refs, h.files);
        for e in h.evidence.iter().take(per_table) {
            block.push_str(&format!("    {}:{}: {}\n", e.path, e.line, e.snippet));
        }
        if used + block.len() > budget {
            user.push_str("(more hints omitted)\n");
            break;
        }
        used += block.len();
        user.push_str(&block);
    }
    user.push_str("\nReturn every candidate table the repository uses, as JSON only.");
    (system, user)
}

#[derive(Deserialize)]
struct Reply {
    tables: Vec<ReplyTable>,
}

#[derive(Deserialize)]
struct ReplyTable {
    name: String,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    evidence: Vec<serde_json::Value>,
}

pub(crate) fn slice_between(text: &str, open: char, close: char) -> Option<&str> {
    let start = text.find(open)?;
    let end = text.rfind(close)?;
    (end > start).then(|| &text[start..=end])
}

/// Urai jawaban AI. Mengembalikan saran untuk tabel yang dikenal dan daftar
/// nama yang disebut AI tetapi tidak ada di diagram.
pub fn parse_ai_reply(
    text: &str,
    candidates: &[Candidate],
) -> Result<(Vec<TableSuggestion>, Vec<String>), RepoScanError> {
    let as_object = || {
        slice_between(text, '{', '}')
            .and_then(|s| serde_json::from_str::<Reply>(s).ok())
            .map(|r| r.tables)
    };
    let as_array = || slice_between(text, '[', ']').and_then(|s| serde_json::from_str(s).ok());
    // Coba bentuk yang kurung pembukanya muncul lebih dulu.
    let array_first = match (text.find('['), text.find('{')) {
        (Some(a), Some(o)) => a < o,
        (Some(_), None) => true,
        _ => false,
    };
    let tables: Vec<ReplyTable> = if array_first {
        as_array().or_else(as_object)
    } else {
        as_object().or_else(as_array)
    }
    .ok_or_else(|| RepoScanError::Parse(truncate_chars(text.trim(), 200)))?;

    let mut lookup: HashMap<String, usize> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        for key in [c.title.as_str(), short_name(&c.title), c.id.as_str()] {
            lookup.entry(key.to_ascii_lowercase()).or_insert(i);
        }
    }

    let mut out: Vec<TableSuggestion> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for t in tables {
        let name = t
            .name
            .trim()
            .trim_matches(|c| c == '`' || c == '"' || c == '\'');
        if name.is_empty() {
            continue;
        }
        let key = name.to_ascii_lowercase();
        let idx = lookup
            .get(&key)
            .or_else(|| lookup.get(short_name(&key)))
            .copied();
        let Some(idx) = idx else {
            if !unknown.iter().any(|u| u.eq_ignore_ascii_case(name)) {
                unknown.push(name.to_string());
            }
            continue;
        };
        let c = &candidates[idx];
        let confidence = t.confidence.unwrap_or(0.7).clamp(0.0, 1.0);
        let evidence: Vec<String> = t
            .evidence
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .take(MAX_EVIDENCE)
            .collect();
        let reason = t
            .reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "Used by the repository".to_string());
        if let Some(existing) = out.iter_mut().find(|s| s.id == c.id) {
            if confidence > existing.confidence {
                existing.confidence = confidence;
                existing.reason = reason;
            }
            for e in evidence {
                if existing.evidence.len() < MAX_EVIDENCE && !existing.evidence.contains(&e) {
                    existing.evidence.push(e);
                }
            }
        } else {
            out.push(TableSuggestion {
                id: c.id.clone(),
                title: c.title.clone(),
                confidence,
                reason,
                evidence,
                from_ai: true,
            });
        }
    }
    Ok((out, unknown))
}

// ─── Runner background ───────────────────────────────────────────────────────

/// Masukan satu pemindaian; semuanya data biasa supaya aman dipindah ke thread.
#[derive(Clone)]
pub struct ScanInput {
    /// Folder project lokal (dipakai lebih dulu bila ada).
    pub repo_path: Option<String>,
    /// URL git (cadangan bila folder tidak ada di komputer ini).
    pub repo_url: Option<String>,
    pub group_title: String,
    pub members: Vec<String>,
    pub candidates: Vec<Candidate>,
    /// Backend AI; `None` = hanya pencarian teks.
    pub backend: Option<ChatBackend>,
    /// Nama backend untuk pesan kemajuan (mis. "agy · gemini-3.8-flash").
    pub backend_label: String,
    /// Direktori cache clone repository.
    pub cache_root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOutcome {
    /// Saran beserta status centang awal.
    pub items: Vec<(TableSuggestion, bool)>,
    pub unknown: Vec<String>,
    pub note: Option<String>,
    pub files_scanned: usize,
}

/// Event job repository di background (pemindaian tabel, pembuatan
/// endpoint, dll.).
#[derive(Debug)]
pub enum RepoJobEvent<T> {
    Progress(ProgressStep),
    /// AI masih mengirim output (teks/tool) tanpa langkah baru; tanda tidak macet.
    Activity,
    Finished(Result<T, String>),
}

pub type ScanEvent = RepoJobEvent<ScanOutcome>;

/// Pegangan job pemindaian yang berjalan.
pub struct ScanHandle {
    pub rx: mpsc::Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
}

impl ScanHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Direktori cache default: `{data_dir}/agent-workspace/repos`.
pub fn default_cache_root() -> PathBuf {
    harness::agent_workspace_dir().join("repos")
}

/// Folder clone `url` di cache Tabular bila sudah pernah di-clone oleh
/// pemindaian sebelumnya. Tidak pernah memicu operasi git.
pub fn cached_clone_dir(url: &str) -> Option<PathBuf> {
    let dir = cache_dir_for(&default_cache_root(), url.trim());
    dir.is_dir().then_some(dir)
}

/// Jalankan pemindaian di thread terpisah.
pub fn spawn_scan(input: ScanInput) -> ScanHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let result = run_scan(&input, &tx, &flag).map_err(|e| match e {
            RepoScanError::Cancelled => "Cancelled".to_string(),
            other => {
                log::warn!("[REPO_SCAN] scan failed: {other}");
                other.to_string()
            }
        });
        let _ = tx.send(ScanEvent::Finished(result));
    });
    ScanHandle { rx, cancel }
}

pub(crate) fn step<T>(
    index: u64,
    description: impl Into<String>,
    detail: Option<String>,
    status: ProgressStatus,
) -> RepoJobEvent<T> {
    RepoJobEvent::Progress(ProgressStep {
        step_index: Some(index),
        description: description.into(),
        detail,
        status,
        tool_name: Some("repo_scan".to_string()),
    })
}

/// Saran dicentang otomatis bila cukup yakin.
fn with_default_checks(items: Vec<TableSuggestion>) -> Vec<(TableSuggestion, bool)> {
    items
        .into_iter()
        .map(|s| {
            let checked = s.confidence >= 0.5;
            (s, checked)
        })
        .collect()
}

fn run_scan(
    input: &ScanInput,
    tx: &mpsc::Sender<ScanEvent>,
    cancel: &AtomicBool,
) -> Result<ScanOutcome, RepoScanError> {
    let source = choose_source(input.repo_path.as_deref(), input.repo_url.as_deref())?;
    let shown = match &source {
        RepoSource::Local(p) => p.display().to_string(),
        RepoSource::Remote(u) => redact(u),
    };
    let prep = match source {
        RepoSource::Remote(_) => "Cloning or updating repository",
        RepoSource::Local(_) => "Opening local folder",
    };
    let _ = tx.send(step(1, prep, Some(shown.clone()), ProgressStatus::Active));
    let resolved = match resolve_repo(&source, &input.cache_root, cancel) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(step(1, prep, Some(e.to_string()), ProgressStatus::Error));
            return Err(e);
        }
    };
    let _ = tx.send(step(1, prep, Some(shown), ProgressStatus::Done));
    let missing_folder = input
        .repo_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty() && matches!(source, RepoSource::Remote(_)))
        .map(|p| {
            format!(
                "Folder {p} is not on this computer, so a fresh clone of the git URL was \
                 scanned. Use Edit Repository… to clone it there."
            )
        });
    log::info!(
        "[REPO_SCAN] scanning {} for {} candidate table(s)",
        resolved.root.display(),
        input.candidates.len()
    );

    let _ = tx.send(step(
        2,
        "Searching code for table names",
        None,
        ProgressStatus::Active,
    ));
    let report = grep_tables(&resolved.root, &input.candidates, cancel)?;
    let _ = tx.send(step(
        2,
        "Searching code for table names",
        Some(format!(
            "{} file(s) scanned{}, {} table(s) matched",
            report.files_scanned,
            if report.truncated {
                " (limit reached)"
            } else {
                ""
            },
            report.hits.len()
        )),
        ProgressStatus::Done,
    ));
    let text_suggestions = suggestions_from_hits(&report);

    let join_notes = |a: Option<String>, b: Option<String>| match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (a, b) => a.or(b),
    };
    let Some(backend) = input.backend.as_ref() else {
        return Ok(ScanOutcome {
            items: with_default_checks(text_suggestions),
            unknown: Vec::new(),
            note: join_notes(
                missing_folder,
                Some("No AI backend is ready, so results come from text search only.".into()),
            ),
            files_scanned: report.files_scanned,
        });
    };

    let (mode, workspace) = ai_workspace(backend, &resolved, &input.cache_root, tx, cancel, 3)?;

    let repo_root = workspace.cwd.clone();
    let (system, user) = build_scan_prompts(
        &input.group_title,
        &input.members,
        &input.candidates,
        &report,
        mode,
        repo_root
            .as_deref()
            .filter(|_| mode == PromptMode::ReadRepo),
    );
    let ask = format!("Asking {} to review the code", input.backend_label);
    let _ = tx.send(step(4, ask.clone(), None, ProgressStatus::Active));

    let ai = ask_ai(backend, system, user, workspace, tx, cancel);
    let (items, unknown, note) = match ai.and_then(|text| parse_ai_reply(&text, &input.candidates))
    {
        Ok((ai_items, unknown)) => {
            let _ = tx.send(step(
                4,
                ask,
                Some(format!("{} table(s) confirmed", ai_items.len())),
                ProgressStatus::Done,
            ));
            let note = (mode == PromptMode::Snippets).then(|| {
                "The AI judged from code snippets only (it could not open the repository)."
                    .to_string()
            });
            (merge_suggestions(ai_items, text_suggestions), unknown, note)
        }
        Err(RepoScanError::Cancelled) => return Err(RepoScanError::Cancelled),
        Err(e) => {
            let msg = e.to_string();
            let _ = tx.send(step(4, ask, Some(msg.clone()), ProgressStatus::Error));
            log::warn!("[REPO_SCAN] AI step failed: {msg}");
            (
                text_suggestions,
                Vec::new(),
                Some(format!(
                    "AI step failed ({msg}). Showing text search results only."
                )),
            )
        }
    };

    Ok(ScanOutcome {
        items: with_default_checks(items),
        unknown,
        note: join_notes(missing_folder, note),
        files_scanned: report.files_scanned,
    })
}

/// Tentukan cara AI membaca kode. Claude Code dan Gemini CLI bisa dibatasi
/// read-only, jadi boleh langsung di folder user. Agent lain hanya bekerja di
/// salinan milik Tabular; bila salinan gagal dibuat, AI hanya melihat cuplikan.
pub(crate) fn ai_workspace<T>(
    backend: &ChatBackend,
    resolved: &ResolvedRepo,
    cache_root: &Path,
    tx: &mpsc::Sender<RepoJobEvent<T>>,
    cancel: &AtomicBool,
    step_index: u64,
) -> Result<(PromptMode, ChatWorkspace), RepoScanError> {
    Ok(match backend.backend {
        AiBackend::Api => (PromptMode::Snippets, ChatWorkspace::default()),
        AiBackend::Cli => match backend.cli.kind {
            CliAgentKind::ClaudeCode => (
                PromptMode::ReadRepo,
                ChatWorkspace {
                    cwd: Some(resolved.root.clone()),
                    allowed_tools: READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect(),
                    without_mcp: true,
                },
            ),
            CliAgentKind::GeminiCli => (
                PromptMode::ReadRepo,
                ChatWorkspace {
                    cwd: Some(resolved.root.clone()),
                    ..Default::default()
                },
            ),
            CliAgentKind::Antigravity | CliAgentKind::Custom => {
                let copy = if resolved.isolated {
                    Some(resolved.root.clone())
                } else {
                    let _ = tx.send(step(
                        step_index,
                        "Making a private copy for the agent",
                        None,
                        ProgressStatus::Active,
                    ));
                    let copy = isolated_copy(&resolved.root, cache_root, cancel);
                    let status = if copy.is_ok() {
                        ProgressStatus::Done
                    } else {
                        ProgressStatus::Error
                    };
                    let _ = tx.send(step(
                        step_index,
                        "Making a private copy for the agent",
                        None,
                        status,
                    ));
                    match copy {
                        Ok(c) => c,
                        Err(RepoScanError::Cancelled) => return Err(RepoScanError::Cancelled),
                        Err(e) => {
                            log::warn!("[REPO_SCAN] private copy failed: {e}");
                            None
                        }
                    }
                };
                match copy {
                    Some(dir) => (
                        PromptMode::ReadRepo,
                        ChatWorkspace {
                            cwd: Some(dir),
                            ..Default::default()
                        },
                    ),
                    None => (PromptMode::Snippets, ChatWorkspace::default()),
                }
            }
        },
    })
}

/// Satu giliran AI; kemajuan agent diteruskan ke `tx`.
pub(crate) fn ask_ai<T>(
    backend: &ChatBackend,
    system: String,
    user: String,
    workspace: ChatWorkspace,
    tx: &mpsc::Sender<RepoJobEvent<T>>,
    cancel: &AtomicBool,
) -> Result<String, RepoScanError> {
    ask_ai_with_offset(backend, system, user, workspace, tx, cancel, 100)
}

/// Seperti [`ask_ai`], dengan nomor langkah agent digeser `step_offset`.
/// Giliran AI yang berjalan paralel memakai offset berbeda supaya langkah
/// agent-nya tidak saling menimpa di daftar kemajuan.
pub(crate) fn ask_ai_with_offset<T>(
    backend: &ChatBackend,
    system: String,
    user: String,
    workspace: ChatWorkspace,
    tx: &mpsc::Sender<RepoJobEvent<T>>,
    cancel: &AtomicBool,
    step_offset: u64,
) -> Result<String, RepoScanError> {
    let (events, handle) =
        crate::ai_assistant::start_chat_in(backend, system, user, None, workspace)
            .map_err(RepoScanError::Ai)?;
    let start = Instant::now();
    let mut text = String::new();
    let mut last_ping = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            if let Some(h) = &handle {
                h.cancel();
            }
            return Err(RepoScanError::Cancelled);
        }
        if start.elapsed() > AI_TIMEOUT {
            if let Some(h) = &handle {
                h.cancel();
            }
            return Err(RepoScanError::Ai(format!(
                "no answer after {} minutes",
                AI_TIMEOUT.as_secs() / 60
            )));
        }
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(AgentEvent::TextDelta(d)) => {
                text.push_str(&d);
                if last_ping.elapsed() >= Duration::from_secs(1) {
                    last_ping = Instant::now();
                    let _ = tx.send(RepoJobEvent::Activity);
                }
            }
            Ok(AgentEvent::Done { text: full, .. }) => {
                return Ok(if full.trim().is_empty() { text } else { full });
            }
            Ok(AgentEvent::Error(e)) => return Err(RepoScanError::Ai(e)),
            Ok(AgentEvent::Progress(mut p)) => {
                // Nomor step agent digeser supaya tidak bentrok dengan step pemindai.
                p.step_index = p.step_index.map(|i| i + step_offset);
                let _ = tx.send(RepoJobEvent::Progress(p));
            }
            Ok(AgentEvent::ToolUse(_) | AgentEvent::Session(_)) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return if text.trim().is_empty() {
                    Err(RepoScanError::Ai(
                        "AI backend stopped without a reply".into(),
                    ))
                } else {
                    Ok(text)
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(names: &[&str]) -> Vec<Candidate> {
        names
            .iter()
            .map(|n| Candidate {
                id: n.to_string(),
                title: n.to_string(),
            })
            .collect()
    }

    /// Direktori sementara unik per test; dihapus saat drop.
    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tabular-repo-scan-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("create temp repo");
            Self(dir)
        }

        fn write(&self, rel: &str, body: &str) {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(p, body).expect("write file");
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parse_repo_source_variants() {
        assert!(matches!(RepoSource::parse("  "), Err(RepoScanError::Empty)));
        assert_eq!(
            RepoSource::parse("https://github.com/org/app.git").ok(),
            Some(RepoSource::Remote("https://github.com/org/app.git".into()))
        );
        assert_eq!(
            RepoSource::parse("git@github.com:org/app.git").ok(),
            Some(RepoSource::Remote("git@github.com:org/app.git".into()))
        );
        assert_eq!(
            RepoSource::parse("/home/me/app").ok(),
            Some(RepoSource::Local(PathBuf::from("/home/me/app")))
        );
        assert!(matches!(
            RepoSource::parse("C:\\work\\app"),
            Ok(RepoSource::Local(_))
        ));
        // Opsi git dan transport berbahaya ditolak.
        assert!(matches!(
            RepoSource::parse("--upload-pack=touch /tmp/x"),
            Err(RepoScanError::Unsupported(_))
        ));
        assert!(matches!(
            RepoSource::parse("ext::sh -c touch% /tmp/pwned"),
            Err(RepoScanError::Unsupported(_))
        ));
        assert!(matches!(
            RepoSource::parse("ftp://host/repo"),
            Err(RepoScanError::Unsupported(_))
        ));
    }

    #[test]
    fn git_config_prefers_origin_then_first_remote() {
        let cfg = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.com/up.git\n\
                   [remote \"origin\"]\n\turl = git@github.com:org/app.git\n\tfetch = +refs/heads/*\n";
        assert_eq!(
            parse_git_config_remote(cfg).as_deref(),
            Some("git@github.com:org/app.git")
        );
        let cfg = "[remote \"fork\"]\n  url = https://example.com/fork.git\n[branch \"main\"]\n  remote = fork\n";
        assert_eq!(
            parse_git_config_remote(cfg).as_deref(),
            Some("https://example.com/fork.git")
        );
        assert_eq!(parse_git_config_remote("[core]\n\turl = nope\n"), None);
    }

    #[test]
    fn git_remote_url_reads_project_folder_and_subfolders() {
        let repo = TempRepo::new("gitcfg");
        repo.write(
            ".git/config",
            "[remote \"origin\"]\n\turl = https://github.com/org/app.git\n",
        );
        repo.write("src/main.rs", "fn main() {}\n");
        assert_eq!(
            git_remote_url(&repo.0).as_deref(),
            Some("https://github.com/org/app.git")
        );
        assert_eq!(
            git_remote_url(&repo.0.join("src")).as_deref(),
            Some("https://github.com/org/app.git")
        );

        // Worktree: `.git` berupa file yang menunjuk gitdir dengan commondir.
        let wt = TempRepo::new("worktree");
        let main_git = repo.0.join(".git");
        std::fs::create_dir_all(main_git.join("worktrees/wt")).expect("mk worktree dir");
        std::fs::write(main_git.join("worktrees/wt/commondir"), "../..\n").expect("commondir");
        wt.write(
            ".git",
            &format!("gitdir: {}\n", main_git.join("worktrees/wt").display()),
        );
        assert_eq!(
            git_remote_url(&wt.0).as_deref(),
            Some("https://github.com/org/app.git")
        );
    }

    #[test]
    fn choose_source_prefers_existing_folder() {
        let repo = TempRepo::new("choose");
        let dir = repo.0.to_string_lossy().to_string();
        let url = "https://github.com/org/app.git";
        assert_eq!(
            choose_source(Some(&dir), Some(url)).ok(),
            Some(RepoSource::Local(repo.0.clone()))
        );
        // Folder tidak ada di komputer ini: jatuh ke URL.
        assert_eq!(
            choose_source(Some("/definitely/not/here"), Some(url)).ok(),
            Some(RepoSource::Remote(url.into()))
        );
        assert!(matches!(
            choose_source(Some("/definitely/not/here"), None),
            Err(RepoScanError::NotFound(_))
        ));
        assert!(matches!(
            choose_source(Some("  "), Some("")),
            Err(RepoScanError::Empty)
        ));
    }

    #[test]
    fn runner_without_ai_scans_this_crate() {
        let absent = ["zz", "absent", "tbl"].join("_");
        let input = ScanInput {
            repo_path: Some(env!("CARGO_MANIFEST_DIR").to_string()),
            repo_url: None,
            group_title: "Local cache".into(),
            members: vec![],
            // Dirakit saat runtime supaya nama ini tidak muncul berkutip di file ini.
            candidates: cand(&["table_cache", "query_history", &absent]),
            backend: None,
            backend_label: String::new(),
            cache_root: std::env::temp_dir(),
        };
        let handle = spawn_scan(input);
        let mut steps = 0;
        let outcome = loop {
            match handle.rx.recv_timeout(Duration::from_secs(120)) {
                Ok(ScanEvent::Progress(_)) => steps += 1,
                Ok(ScanEvent::Activity) => {}
                Ok(ScanEvent::Finished(r)) => break r.expect("scan succeeds"),
                Err(e) => panic!("scan did not finish: {e}"),
            }
        };
        assert!(steps >= 4, "progress should be reported");
        let ids: Vec<&str> = outcome.items.iter().map(|(s, _)| s.id.as_str()).collect();
        assert!(ids.contains(&"table_cache"), "{ids:?}");
        assert!(ids.contains(&"query_history"), "{ids:?}");
        assert!(!ids.contains(&absent.as_str()), "{ids:?}");
        assert!(outcome.note.is_some(), "text-only mode must be explained");
        assert!(outcome.files_scanned > 50);
    }

    #[test]
    fn remote_clone_then_refresh_from_file_url() {
        if harness::resolve_binary("git").is_none() {
            eprintln!("git not installed; skipping");
            return;
        }
        let origin = TempRepo::new("origin");
        let cache = TempRepo::new("cache");
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
                .args(args)
                .current_dir(&origin.0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?} failed");
        };
        git(&["init", "--quiet"]);
        origin.write("q.sql", "SELECT * FROM orders;\n");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "one"]);

        let url = format!("file://{}", origin.0.to_string_lossy().replace('\\', "/"));
        let src = RepoSource::parse(&url).expect("file url");
        let never = AtomicBool::new(false);
        let first = resolve_repo(&src, &cache.0, &never).expect("clone");
        assert!(first.isolated);
        assert!(first.root.join("q.sql").is_file());

        // Clone penuh ke folder project user.
        let project = cache.0.join("projects/app");
        clone_into(&url, &project, &never).expect("clone into folder");
        assert!(project.join("q.sql").is_file());
        assert_eq!(
            git_remote_url(&project).as_deref(),
            Some(url.as_str()),
            "remote detected from the new clone"
        );
        // Folder yang sudah berisi tidak ditimpa.
        assert!(matches!(
            clone_into(&url, &project, &never),
            Err(RepoScanError::Git { .. })
        ));
        assert!(matches!(
            clone_into("/some/local/path", &cache.0.join("x"), &never),
            Err(RepoScanError::Unsupported(_))
        ));

        // Commit baru di origin terlihat setelah resolve berikutnya (fetch + reset).
        origin.write("r.sql", "SELECT * FROM customers;\n");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "two"]);
        let second = resolve_repo(&src, &cache.0, &never).expect("refresh");
        assert_eq!(first.root, second.root);
        assert!(second.root.join("r.sql").is_file());
    }

    /// Pemindaian penuh dengan CLI agent sungguhan. Fixture berisi model
    /// `Invoice` tanpa nama tabel literal: hanya AI yang bisa memetakannya ke
    /// `invoices`. Jalankan manual: `cargo test --lib real_ -- --ignored`.
    fn real_scan(kind: CliAgentKind, model: &str) -> ScanOutcome {
        let repo = TempRepo::new("real");
        repo.write(
            "sql/report.sql",
            "SELECT id, total FROM orders WHERE total > 0;\n",
        );
        repo.write(
            "app/Models/Invoice.php",
            "<?php\nnamespace App\\Models;\nuse Illuminate\\Database\\Eloquent\\Model;\n\
             class Invoice extends Model {\n    protected $fillable = ['number', 'amount'];\n}\n",
        );
        repo.write("README.md", "Customers are managed in another service.\n");
        let git = |args: &[&str]| {
            let _ = Command::new("git")
                .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
                .args(args)
                .current_dir(&repo.0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        };
        git(&["init", "--quiet"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "fixture"]);
        let cache = TempRepo::new("real-cache");

        let backend = ChatBackend {
            target: crate::config::ChatTarget::Cli(kind),
            backend: AiBackend::Cli,
            provider: crate::config::AiProvider::OpenAI,
            api_key: String::new(),
            model: String::new(),
            base_url: String::new(),
            cli: harness::CliAgentConfig {
                kind,
                model: model.into(),
                ..Default::default()
            },
            mcp_available: false,
            notes_enabled: false,
            notes_writable: false,
        };
        let handle = spawn_scan(ScanInput {
            repo_path: Some(repo.0.to_string_lossy().to_string()),
            repo_url: None,
            group_title: "Billing".into(),
            members: vec![],
            candidates: cand(&["orders", "invoices", "customers", "audit_logs"]),
            backend: Some(backend),
            backend_label: format!("{kind:?}"),
            cache_root: cache.0.clone(),
        });
        loop {
            match handle.rx.recv_timeout(Duration::from_secs(600)) {
                Ok(ScanEvent::Progress(p)) => {
                    eprintln!("[progress] {} {:?}", p.description, p.detail)
                }
                Ok(ScanEvent::Activity) => {}
                Ok(ScanEvent::Finished(r)) => break r.expect("scan succeeds"),
                Err(e) => panic!("scan did not finish: {e}"),
            }
        }
    }

    fn assert_real_outcome(outcome: &ScanOutcome) {
        eprintln!("note: {:?}", outcome.note);
        for (s, on) in &outcome.items {
            eprintln!(
                "{} ai={} conf={:.2} on={on} reason={} ev={:?}",
                s.id, s.from_ai, s.confidence, s.reason, s.evidence
            );
        }
        let ai: Vec<&str> = outcome
            .items
            .iter()
            .filter(|(s, _)| s.from_ai)
            .map(|(s, _)| s.id.as_str())
            .collect();
        assert!(
            outcome.note.is_none(),
            "AI step should succeed: {:?}",
            outcome.note
        );
        assert!(ai.contains(&"orders"), "{ai:?}");
        assert!(ai.contains(&"invoices"), "model Invoice → invoices: {ai:?}");
        assert!(!ai.contains(&"audit_logs"), "{ai:?}");
    }

    #[test]
    #[ignore = "memanggil CLI Claude Code sungguhan"]
    fn real_claude_scan_reads_repo() {
        assert_real_outcome(&real_scan(CliAgentKind::ClaudeCode, "haiku"));
    }

    #[test]
    #[ignore = "memanggil CLI agy sungguhan"]
    fn real_agy_scan_reads_private_copy() {
        assert_real_outcome(&real_scan(
            CliAgentKind::Antigravity,
            "gemini-3.8-flash-low",
        ));
    }

    #[test]
    fn detects_credentials_in_shared_urls() {
        assert!(has_embedded_credentials(
            "https://user:secret@github.com/org/app.git"
        ));
        assert!(has_embedded_credentials(
            "https://ghp_abcdef@github.com/org/app.git"
        ));
        assert!(has_embedded_credentials(
            "https://oauth2:glpat-xyz@gitlab.com/org/app.git"
        ));
        assert!(!has_embedded_credentials("https://github.com/org/app.git"));
        assert!(!has_embedded_credentials(
            "https://alice@bitbucket.org/org/app.git"
        ));
        assert!(!has_embedded_credentials("ssh://git@host/org/app.git"));
        assert!(!has_embedded_credentials("git@github.com:org/app.git"));
    }

    #[test]
    fn redact_hides_credentials() {
        assert_eq!(
            redact("fatal: https://user:tok3n@github.com/org/app.git not found"),
            "fatal: https://***@github.com/org/app.git not found"
        );
        assert_eq!(
            redact("git@github.com:org/app.git"),
            "git@github.com:org/app.git"
        );
    }

    #[test]
    fn grep_finds_sql_and_orm_usage_and_skips_noise() {
        let repo = TempRepo::new("grep");
        repo.write(
            "src/repo.rs",
            "let q = \"SELECT * FROM public.orders o JOIN customers c ON c.id = o.customer_id\";\n",
        );
        repo.write(
            "app/models.py",
            "class Invoice(Base):\n    __tablename__ = 'invoices'\n",
        );
        repo.write("app/User.php", "protected $table = 'users';\n");
        repo.write("db/schema.sql", "CREATE INDEX idx ON payments (id);\n");
        // Kata biasa tanpa konteks SQL/kutip tidak dihitung.
        repo.write("README.md", "Our customers love the orders page.\n");
        // Folder dependency dan file biner dilewati.
        repo.write(
            "node_modules/lib/index.js",
            "db.query('SELECT * FROM products')\n",
        );
        repo.write("assets/blob.dat", "FROM products");
        std::fs::write(repo.0.join("data.bin"), b"FROM products\0\0").expect("write bin");

        let cands = cand(&[
            "orders",
            "customers",
            "invoices",
            "users",
            "payments",
            "products",
        ]);
        let report = grep_tables(&repo.0, &cands, &AtomicBool::new(false)).expect("scan works");
        let found: Vec<&str> = report.hits.iter().map(|h| h.title.as_str()).collect();
        for t in ["orders", "customers", "invoices", "users", "payments"] {
            assert!(found.contains(&t), "{t} should be found, got {found:?}");
        }
        assert!(
            !found.contains(&"products"),
            "ignored dirs/binaries leaked: {found:?}"
        );

        let orders = report
            .hits
            .iter()
            .find(|h| h.title == "orders")
            .expect("orders");
        assert_eq!(orders.files, 1, "README mention must not count");
        assert_eq!(orders.evidence[0].path, "src/repo.rs");
        assert_eq!(orders.evidence[0].line, 1);
        assert!(orders.evidence[0].strong);
    }

    #[test]
    fn grep_honours_cancellation() {
        let repo = TempRepo::new("cancel");
        repo.write("a.sql", "SELECT 1 FROM t");
        let res = grep_tables(&repo.0, &cand(&["t1"]), &AtomicBool::new(true));
        assert!(matches!(res, Err(RepoScanError::Cancelled)));
    }

    #[test]
    fn local_source_must_exist() {
        let missing = std::env::temp_dir().join("tabular-repo-scan-definitely-missing-dir");
        let res = resolve_repo(
            &RepoSource::Local(missing),
            &std::env::temp_dir(),
            &AtomicBool::new(false),
        );
        assert!(matches!(res, Err(RepoScanError::NotFound(_))));
    }

    #[test]
    fn parse_ai_reply_accepts_fenced_json_and_filters_unknown() {
        let cands = vec![
            Candidate {
                id: "orders".into(),
                title: "orders".into(),
            },
            Candidate {
                id: "sales.customers".into(),
                title: "sales.customers".into(),
            },
        ];
        let reply = "Here you go:\n```json\n{\"tables\":[\
            {\"name\":\"ORDERS\",\"confidence\":0.9,\"reason\":\"repo query\",\"evidence\":[\"src/a.rs:3\"]},\
            {\"name\":\"customers\",\"confidence\":1.4},\
            {\"name\":\"orders\",\"confidence\":0.2,\"evidence\":[\"src/b.rs:9\", 12]},\
            {\"name\":\"ghost_table\"}]}\n```";
        let (items, unknown) = parse_ai_reply(reply, &cands).expect("parses");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "orders");
        assert!((items[0].confidence - 0.9).abs() < 1e-6);
        assert_eq!(items[0].evidence, vec!["src/a.rs:3", "src/b.rs:9", "12"]);
        assert_eq!(items[1].id, "sales.customers");
        assert_eq!(items[1].confidence, 1.0);
        assert!(items.iter().all(|s| s.from_ai));
        assert_eq!(unknown, vec!["ghost_table"]);

        // Array polos juga diterima.
        let (items, _) = parse_ai_reply("[{\"name\":\"orders\"}]", &cands).expect("array");
        assert_eq!(items.len(), 1);

        assert!(matches!(
            parse_ai_reply("I could not find anything.", &cands),
            Err(RepoScanError::Parse(_))
        ));
    }

    #[test]
    fn merge_puts_ai_first_and_keeps_text_only_hits() {
        let mk = |id: &str, c: f32, ai: bool| TableSuggestion {
            id: id.into(),
            title: id.into(),
            confidence: c,
            reason: String::new(),
            evidence: vec![],
            from_ai: ai,
        };
        let merged = merge_suggestions(
            vec![mk("a", 0.4, true), mk("b", 0.9, true)],
            vec![mk("b", 0.3, false), mk("c", 0.8, false)],
        );
        let ids: Vec<&str> = merged.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert!(merged[0].from_ai);
        assert!(!merged[2].from_ai);
    }

    #[test]
    fn prompts_follow_mode_and_budget() {
        let cands = cand(&["orders", "customers"]);
        let report = ScanReport {
            files_scanned: 3,
            truncated: false,
            hits: vec![TableHit {
                id: "orders".into(),
                title: "orders".into(),
                score: 3.0,
                refs: 1,
                files: 1,
                evidence: vec![Evidence {
                    path: "src/a.rs".into(),
                    line: 7,
                    snippet: "FROM orders".into(),
                    strong: true,
                }],
            }],
        };
        let (sys, user) = build_scan_prompts(
            "Sales",
            &["orders".into()],
            &cands,
            &report,
            PromptMode::ReadRepo,
            Some(Path::new("/work/app")),
        );
        assert!(sys.contains("The repository is at `/work/app`"));
        assert!(sys.contains("current working directory"));
        assert!(sys.contains("Never create, modify or delete files"));
        assert!(user.contains("Diagram group: Sales"));
        assert!(user.contains("Candidate tables (2):\norders, customers"));
        assert!(user.contains("src/a.rs:7: FROM orders"));

        let (sys, _) =
            build_scan_prompts("Sales", &[], &cands, &report, PromptMode::Snippets, None);
        assert!(sys.contains("cannot open files"));
    }

    #[test]
    fn text_suggestions_confidence_grows_with_score() {
        let hit = |score: f32| TableHit {
            id: "t".into(),
            title: "t".into(),
            score,
            refs: 1,
            files: 1,
            evidence: vec![],
        };
        let report = ScanReport {
            files_scanned: 1,
            truncated: false,
            hits: vec![hit(3.0), hit(12.0)],
        };
        let s = suggestions_from_hits(&report);
        assert!(s[0].confidence < 0.5);
        assert!(s[1].confidence > 0.8);
        assert!(s.iter().all(|x| !x.from_ai));
    }

    #[test]
    fn repo_key_matches_equivalent_urls() {
        let expected = Some("github.com/org/app".to_string());
        for url in [
            "https://github.com/Org/App.git",
            "https://github.com/org/app/",
            "http://github.com/org/app",
            "git@github.com:org/app.git",
            "ssh://git@github.com:22/org/app",
            "https://token@github.com/org/app.git",
            "  git@GitHub.com:Org/App  ",
        ] {
            assert_eq!(repo_key(url), expected, "{url}");
        }
        assert_eq!(
            repo_key("https://gitlab.example.com/group/sub/app.git"),
            Some("gitlab.example.com/group/sub/app".to_string())
        );
        assert_eq!(
            repo_key("file:///srv/git/app.git"),
            Some("file:/srv/git/app".to_string())
        );
        assert_eq!(repo_key(""), None);
        assert_eq!(repo_key("/local/folder"), None);
        assert_eq!(repo_key("https://github.com"), None);
        assert_ne!(
            repo_key("git@github.com:org/app"),
            repo_key("git@github.com:org/api")
        );
    }

    #[test]
    fn repo_key_for_prefers_url_then_folder_remote() {
        let repo = TempRepo::new("key");
        repo.write(
            ".git/config",
            "[remote \"origin\"]\n\turl = git@github.com:org/from-folder.git\n",
        );
        let path = repo.0.to_string_lossy().to_string();
        assert_eq!(
            repo_key_for(Some("https://github.com/org/app"), Some(&path)),
            Some("github.com/org/app".to_string())
        );
        assert_eq!(
            repo_key_for(None, Some(&path)),
            Some("github.com/org/from-folder".to_string())
        );
        assert_eq!(repo_key_for(Some(" "), None), None);
    }
}
