//! State dan job tab Git Graph: halaman commit lintas branch, ref, stash,
//! detail/perbandingan commit, dan eksekusi aksi dari menu konteks.
//!
//! Data graf disimpan di [`GitUiState::graphs`](super::git_jobs::GitUiState)
//! per id tab (bukan di `GitTabState`) karena besar dan tidak perlu ikut
//! `Clone` tab. Setiap muat ulang menaikkan `seq`; hasil job dengan `seq` lama
//! dibuang supaya halaman dari query sebelumnya tidak tercampur.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;

use super::git_jobs::{self, GitTabState, GitView, JobResult, run_op};
use crate::git::diff::{self, DiffRow};
use crate::git::graph::{self, GraphRow};
use crate::git::history::{self, CommitDetails, DiffRange, FileStat, GraphQuery};
use crate::git::history_ops::{self as hops, ForceMode, MergeOptions, PickOptions, ResetMode};
use crate::git::log::CommitInfo;
use crate::git::refs::{self, RefSet, RemoteInfo, TagDetails};
use crate::git::stash::{self, StashInfo};
use crate::git::status::{self, RepoStatus};
use crate::git::{GitError, ops};
use crate::window_egui::Tabular;

/// Hash semu untuk baris "Uncommitted Changes".
pub const UNCOMMITTED: &str = "*";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Commit,
    Uncommitted,
    Stash,
}

/// Satu baris tabel graf.
#[derive(Debug, Clone)]
pub struct GraphItem {
    pub kind: RowKind,
    pub info: CommitInfo,
    pub stash: Option<StashInfo>,
}

/// Panel detail commit (atau perbandingan dua commit).
pub struct DetailState {
    pub range: DiffRange,
    /// Hash baris yang dipilih (atau [`UNCOMMITTED`]).
    pub primary: String,
    /// Hash pembanding (Ctrl/Cmd+klik).
    pub compare: Option<String>,
    pub details: Option<CommitDetails>,
    pub files: Vec<FileStat>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected_file: Option<usize>,
    pub rows: Vec<DiffRow>,
    pub binary: bool,
    pub diff_loading: bool,
    pub diff_error: Option<String>,
    pub side_by_side: bool,
    pub show_all_rows: bool,
    /// Tampilan file sebagai pohon folder (bukan daftar).
    pub tree: bool,
    pub collapsed: HashSet<String>,
    /// Mode code review: file yang dibuka ditandai sudah dilihat.
    pub reviewing: bool,
    seq: u64,
}

#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub open: bool,
    pub text: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub matches: Vec<usize>,
    pub current: usize,
    pub focus: bool,
    pub error: Option<String>,
}

pub struct GraphState {
    pub key: String,
    pub repo: PathBuf,
    pub commits: Vec<CommitInfo>,
    pub items: Vec<GraphItem>,
    pub rows: Vec<GraphRow>,
    pub max_lanes: usize,
    pub refs: RefSet,
    pub stashes: Vec<StashInfo>,
    pub status: Option<RepoStatus>,
    pub loading: bool,
    pub done: bool,
    pub error: Option<String>,
    seq: u64,
    /// Filter branch (kosong = semua).
    pub branches: Vec<String>,
    pub branch_query: String,
    pub find: FindState,
    pub selected: Option<String>,
    pub detail: Option<DetailState>,
    pub scroll_to: Option<usize>,
    pub dialog: Option<GraphDialog>,
    pub remotes: Vec<RemoteInfo>,
    pub remotes_open: bool,
    pub remote_form: RemoteForm,
    pub tag_view: Option<TagDetails>,
    pub settings_open: bool,
}

/// Form tambah/ubah remote di dialog Remotes.
#[derive(Debug, Clone, Default)]
pub struct RemoteForm {
    /// Nama remote yang sedang diubah; `None` = tambah baru.
    pub editing: Option<String>,
    pub name: String,
    pub url: String,
    pub push_url: String,
}

impl GraphState {
    fn new(key: String, repo: PathBuf, branches: Vec<String>) -> Self {
        Self {
            key,
            repo,
            commits: Vec::new(),
            items: Vec::new(),
            rows: Vec::new(),
            max_lanes: 1,
            refs: RefSet::default(),
            stashes: Vec::new(),
            status: None,
            loading: false,
            done: false,
            error: None,
            seq: 0,
            branches,
            branch_query: String::new(),
            find: FindState::default(),
            selected: None,
            detail: None,
            scroll_to: None,
            dialog: None,
            remotes: Vec::new(),
            remotes_open: false,
            remote_form: RemoteForm::default(),
            tag_view: None,
            settings_open: false,
        }
    }

    pub fn index_of(&self, hash: &str) -> Option<usize> {
        self.items.iter().position(|i| i.info.hash == hash)
    }

    pub fn head_index(&self) -> Option<usize> {
        self.refs.head.as_deref().and_then(|h| self.index_of(h))
    }

    pub fn default_remote(&self) -> String {
        self.refs
            .remotes
            .iter()
            .find(|r| *r == "origin")
            .or(self.refs.remotes.first())
            .cloned()
            .unwrap_or_else(|| "origin".to_string())
    }

    /// Susun ulang baris dari commit, stash, dan status working tree.
    fn rebuild(&mut self, settings: &crate::git::repos::GitSettings) {
        let mut items = Vec::with_capacity(self.commits.len() + 1);
        let changes = self.status.as_ref().map_or(0, RepoStatus::change_count);
        if settings.show_uncommitted
            && changes > 0
            && self.branches.is_empty()
            && let Some(head) = self.refs.head.clone()
        {
            items.push(GraphItem {
                kind: RowKind::Uncommitted,
                info: CommitInfo {
                    hash: UNCOMMITTED.to_string(),
                    short: UNCOMMITTED.to_string(),
                    author: String::new(),
                    email: String::new(),
                    time: chrono::Utc::now().timestamp(),
                    parents: vec![head],
                    refs: String::new(),
                    subject: format!("Uncommitted Changes ({changes})"),
                },
                stash: None,
            });
        }
        let mut placed: HashSet<&str> = HashSet::new();
        for c in &self.commits {
            if settings.show_stashes {
                for s in self.stashes.iter().filter(|s| s.base == c.hash) {
                    if placed.insert(s.hash.as_str()) {
                        items.push(GraphItem {
                            kind: RowKind::Stash,
                            info: CommitInfo {
                                hash: s.hash.clone(),
                                short: s.hash.chars().take(8).collect(),
                                author: s.author.clone(),
                                email: s.email.clone(),
                                time: s.time,
                                parents: vec![s.base.clone()],
                                refs: String::new(),
                                subject: s.message.clone(),
                            },
                            stash: Some(s.clone()),
                        });
                    }
                }
            }
            items.push(GraphItem {
                kind: RowKind::Commit,
                info: c.clone(),
                stash: None,
            });
        }
        self.rows = graph::layout(
            items
                .iter()
                .map(|i| (i.info.hash.as_str(), i.info.parents.as_slice())),
        );
        self.max_lanes = self.rows.iter().map(|r| r.width).max().unwrap_or(1);
        self.items = items;
        self.update_find();
    }

    /// Hitung ulang baris yang cocok dengan teks Find.
    pub fn update_find(&mut self) {
        let f = &mut self.find;
        f.matches.clear();
        f.error = None;
        let q = f.text.trim();
        if q.is_empty() {
            return;
        }
        let re = if f.regex {
            match regex::RegexBuilder::new(q)
                .case_insensitive(!f.case_sensitive)
                .build()
            {
                Ok(r) => Some(r),
                Err(e) => {
                    f.error = Some(e.to_string());
                    return;
                }
            }
        } else {
            None
        };
        let needle = if f.case_sensitive {
            q.to_string()
        } else {
            q.to_lowercase()
        };
        let hit = |s: &str| match &re {
            Some(r) => r.is_match(s),
            None if f.case_sensitive => s.contains(&needle),
            None => s.to_lowercase().contains(&needle),
        };
        for (i, it) in self.items.iter().enumerate() {
            let c = &it.info;
            let refs = self.refs.labels(&c.hash);
            if hit(&c.subject)
                || hit(&c.author)
                || c.hash.starts_with(q)
                || refs.iter().any(|r| hit(&r.name))
            {
                f.matches.push(i);
            }
        }
        if f.current >= f.matches.len() {
            f.current = 0;
        }
    }
}

/// Hasil job Git Graph.
pub enum GraphJob {
    Page {
        tab_id: usize,
        seq: u64,
        skip: usize,
        res: Result<Vec<CommitInfo>, GitError>,
    },
    Meta {
        tab_id: usize,
        seq: u64,
        res: Result<(RefSet, Vec<StashInfo>, RepoStatus), GitError>,
    },
    Details {
        tab_id: usize,
        seq: u64,
        res: Result<(Option<CommitDetails>, Vec<FileStat>), GitError>,
    },
    FilePatch {
        tab_id: usize,
        seq: u64,
        res: Result<String, GitError>,
    },
    Remotes {
        tab_id: usize,
        res: Result<Vec<RemoteInfo>, GitError>,
    },
    Tag {
        tab_id: usize,
        res: Result<TagDetails, GitError>,
    },
}

fn send(t: &mut Tabular, job: impl FnOnce() -> GraphJob + Send + 'static) {
    t.git.spawn(move || JobResult::Graph(job()));
}

fn query(t: &Tabular, st: &GraphState) -> GraphQuery {
    let s = &t.git.store.settings;
    GraphQuery {
        branches: st.branches.clone(),
        show_remote: s.show_remote_branches,
        show_tags: s.show_tags,
        order: s.graph_order,
        first_parent: s.first_parent,
        reflog: s.show_reflog,
    }
}

// ─── Membuka & memuat ───────────────────────────────────────────────────────

/// Buka Git Graph repository `key` (satu tab per repository).
pub fn open_graph(t: &mut Tabular, key: &str) {
    let Some(entry) = t.git.entry(key).cloned() else {
        return;
    };
    let Some(repo) = entry.path.clone() else {
        t.toasts
            .error("This repository has no local folder on this computer");
        return;
    };
    if let Some(idx) = git_jobs::find_tab(
        t,
        |s| matches!(&s.view, GitView::Graph { key: k, .. } if k == key),
    ) {
        crate::editor::switch_to_tab(t, idx);
        return;
    }
    let title = format!(
        "{} Git Graph · {}",
        egui_icons::icons::ICON_ACCOUNT_TREE.codepoint,
        entry.name
    );
    let view = GitView::Graph {
        key: key.to_string(),
        repo: repo.clone(),
    };
    let mut state = GitTabState::new(view);
    state.loading = false;
    let tab_id = git_jobs::open_tab(t, None, title, state);
    let branches = t
        .git
        .store
        .prefs
        .get(key)
        .map(|p| p.graph_branches.clone())
        .unwrap_or_default();
    t.git
        .graphs
        .insert(tab_id, GraphState::new(key.to_string(), repo, branches));
    reload(t, tab_id);
}

/// Pasang state graf untuk tab yang sudah ada (mis. dipulihkan dari sesi).
pub fn attach(t: &mut Tabular, tab_id: usize, key: &str, repo: &std::path::Path) {
    if !t.git.repos_loaded {
        git_jobs::reload_repos(t);
    }
    let branches = t
        .git
        .store
        .prefs
        .get(key)
        .map(|p| p.graph_branches.clone())
        .unwrap_or_default();
    t.git.graphs.insert(
        tab_id,
        GraphState::new(key.to_string(), repo.to_path_buf(), branches),
    );
    reload(t, tab_id);
}

/// Muat ulang graf dari awal (ref, stash, status, halaman pertama).
pub fn reload(t: &mut Tabular, tab_id: usize) {
    let (q, key) = match t.git.graphs.get(&tab_id) {
        Some(st) => (query(t, st), st.key.clone()),
        None => return,
    };
    // Status sidebar juga memuat state merge/rebase untuk banner graf.
    git_jobs::refresh_status_of(t, &key);
    let limit = t.git.store.settings.graph_page.max(50);
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    st.seq += 1;
    st.loading = true;
    st.error = None;
    let (seq, repo) = (st.seq, st.repo.clone());
    let repo2 = repo.clone();
    send(t, move || GraphJob::Meta {
        tab_id,
        seq,
        res: refs::list(&repo2).and_then(|r| {
            let stashes = stash::list(&repo2).unwrap_or_default();
            status::read(&repo2).map(|s| (r, stashes, s))
        }),
    });
    send(t, move || GraphJob::Page {
        tab_id,
        seq,
        skip: 0,
        res: history::graph_page(&repo, &q, 0, limit),
    });
}

/// Muat halaman commit berikutnya (dipanggil saat scroll mendekati bawah).
pub fn load_more(t: &mut Tabular, tab_id: usize) {
    let q = match t.git.graphs.get(&tab_id) {
        Some(st) if !st.loading && !st.done => query(t, st),
        _ => return,
    };
    let limit = t.git.store.settings.graph_page.max(50);
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    st.loading = true;
    let (seq, repo, skip) = (st.seq, st.repo.clone(), st.commits.len());
    send(t, move || GraphJob::Page {
        tab_id,
        seq,
        skip,
        res: history::graph_page(&repo, &q, skip, limit),
    });
}

/// Muat ulang semua graf yang terbuka (saat jendela kembali fokus).
pub fn refresh_open_graphs(t: &mut Tabular) {
    let ids: Vec<usize> = t.git.graphs.keys().copied().collect();
    for id in ids {
        reload(t, id);
    }
}

/// Repository `key` berubah (operasi selesai): muat ulang grafnya.
pub fn on_repo_changed(t: &mut Tabular, key: &str) {
    let ids: Vec<usize> = t
        .git
        .graphs
        .iter()
        .filter(|(_, g)| g.key == key)
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        reload(t, id);
        if t.git.graphs.get(&id).is_some_and(|g| g.remotes_open) {
            load_remotes(t, id);
        }
    }
}

/// Ganti filter branch lalu muat ulang.
pub fn set_branches(t: &mut Tabular, tab_id: usize, branches: Vec<String>) {
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    st.branches = branches.clone();
    let key = st.key.clone();
    t.git.store.prefs.entry(key).or_default().graph_branches = branches;
    let _ = t.git.save_store();
    reload(t, tab_id);
}

// ─── Detail & perbandingan ──────────────────────────────────────────────────

fn range_for(item: &GraphItem) -> DiffRange {
    match item.kind {
        RowKind::Uncommitted => DiffRange::WorkingTree {
            from: "HEAD".to_string(),
        },
        RowKind::Commit | RowKind::Stash => DiffRange::Commit {
            hash: item.info.hash.clone(),
            parent: item.info.parents.first().cloned(),
        },
    }
}

fn start_detail(
    t: &mut Tabular,
    tab_id: usize,
    range: DiffRange,
    primary: String,
    compare: Option<String>,
) {
    let check_sig = t.git.store.settings.check_signatures;
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    let seq = st.detail.as_ref().map_or(1, |d| d.seq + 1);
    let (tree, reviewing) = st.detail.as_ref().map_or((false, false), |d| {
        (d.tree, d.reviewing && d.range == range)
    });
    st.selected = Some(primary.clone());
    st.detail = Some(DetailState {
        range: range.clone(),
        primary: primary.clone(),
        compare: compare.clone(),
        details: None,
        files: Vec::new(),
        loading: true,
        error: None,
        selected_file: None,
        rows: Vec::new(),
        binary: false,
        diff_loading: false,
        diff_error: None,
        side_by_side: true,
        show_all_rows: false,
        tree,
        collapsed: HashSet::new(),
        reviewing,
        seq,
    });
    let repo = st.repo.clone();
    let want_details = primary != UNCOMMITTED && compare.is_none();
    send(t, move || GraphJob::Details {
        tab_id,
        seq,
        res: (|| {
            let d = if want_details {
                Some(history::details(&repo, &primary, check_sig)?)
            } else {
                None
            };
            Ok((d, history::range_files(&repo, &range)?))
        })(),
    });
}

/// Klik baris: buka detail (klik lagi menutup).
pub fn select(t: &mut Tabular, tab_id: usize, idx: usize) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let Some(item) = st.items.get(idx).cloned() else {
        return;
    };
    if st.selected.as_deref() == Some(item.info.hash.as_str())
        && st.detail.as_ref().is_some_and(|d| d.compare.is_none())
    {
        close_detail(t, tab_id);
        return;
    }
    start_detail(t, tab_id, range_for(&item), item.info.hash.clone(), None);
}

/// Ctrl/Cmd+klik: bandingkan baris terpilih dengan baris `idx`.
pub fn compare(t: &mut Tabular, tab_id: usize, idx: usize) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let (Some(sel), Some(other)) = (
        st.selected.clone().and_then(|h| st.index_of(&h)),
        st.items.get(idx).cloned(),
    ) else {
        select(t, tab_id, idx);
        return;
    };
    if sel == idx {
        return;
    }
    let first = st.items[sel].clone();
    // Baris lebih bawah = lebih lama = sisi "from".
    let (older, newer) = if sel > idx {
        (&first, &other)
    } else {
        (&other, &first)
    };
    let range = if newer.kind == RowKind::Uncommitted {
        DiffRange::WorkingTree {
            from: older.info.hash.clone(),
        }
    } else if older.kind == RowKind::Uncommitted {
        DiffRange::WorkingTree {
            from: newer.info.hash.clone(),
        }
    } else {
        DiffRange::Between {
            from: older.info.hash.clone(),
            to: newer.info.hash.clone(),
        }
    };
    start_detail(
        t,
        tab_id,
        range,
        first.info.hash.clone(),
        Some(other.info.hash.clone()),
    );
}

/// Bandingkan commit `idx` dengan working tree.
pub fn compare_with_working_tree(t: &mut Tabular, tab_id: usize, idx: usize) {
    let Some(item) = t
        .git
        .graphs
        .get(&tab_id)
        .and_then(|s| s.items.get(idx).cloned())
    else {
        return;
    };
    start_detail(
        t,
        tab_id,
        DiffRange::WorkingTree {
            from: item.info.hash.clone(),
        },
        item.info.hash.clone(),
        Some(UNCOMMITTED.to_string()),
    );
}

pub fn close_detail(t: &mut Tabular, tab_id: usize) {
    if let Some(st) = t.git.graphs.get_mut(&tab_id) {
        st.detail = None;
        st.selected = None;
    }
}

/// Pilih file di panel detail dan muat diff-nya.
pub fn select_file(t: &mut Tabular, tab_id: usize, idx: usize) {
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    let repo = st.repo.clone();
    let key = st.key.clone();
    let Some(d) = st.detail.as_mut() else {
        return;
    };
    let Some(file) = d.files.get(idx).map(|f| f.change.clone()) else {
        return;
    };
    d.seq += 1;
    d.selected_file = Some(idx);
    d.diff_loading = true;
    d.diff_error = None;
    d.rows.clear();
    let (seq, range, reviewing) = (d.seq, d.range.clone(), d.reviewing);
    if reviewing {
        let rk = range.review_key();
        t.git
            .store
            .prefs
            .entry(key)
            .or_default()
            .mark_reviewed(&rk, &file.path);
        let _ = t.git.save_store();
    }
    send(t, move || GraphJob::FilePatch {
        tab_id,
        seq,
        res: history::range_file_patch(&repo, &range, &file),
    });
}

/// Buka diff file terpilih di tab tersendiri.
pub fn open_file_tab(t: &mut Tabular, tab_id: usize, idx: usize) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let Some(d) = &st.detail else {
        return;
    };
    let Some(file) = d.files.get(idx).map(|f| f.change.clone()) else {
        return;
    };
    let (repo, range) = (st.repo.clone(), d.range.clone());
    git_jobs::open_range_diff(t, repo, range, file);
}

/// Mulai/akhiri code review untuk rentang yang terbuka.
pub fn toggle_review(t: &mut Tabular, tab_id: usize) {
    if let Some(d) = t
        .git
        .graphs
        .get_mut(&tab_id)
        .and_then(|s| s.detail.as_mut())
    {
        d.reviewing = !d.reviewing;
    }
}

pub fn is_reviewed(t: &Tabular, key: &str, range: &DiffRange, path: &str) -> bool {
    t.git
        .store
        .prefs
        .get(key)
        .is_some_and(|p| p.is_reviewed(&range.review_key(), path))
}

pub fn load_remotes(t: &mut Tabular, tab_id: usize) {
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    st.remotes_open = true;
    let repo = st.repo.clone();
    send(t, move || GraphJob::Remotes {
        tab_id,
        res: refs::remotes(&repo),
    });
}

pub fn show_tag(t: &mut Tabular, tab_id: usize, name: String) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let repo = st.repo.clone();
    send(t, move || GraphJob::Tag {
        tab_id,
        res: refs::tag_details(&repo, &name),
    });
}

/// Simpan isi revisi `rev` sebagai arsip (zip/tar) lewat dialog simpan.
pub fn archive(t: &mut Tabular, tab_id: usize, rev: String) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let key = st.key.clone();
    let name = st
        .repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    let short: String = rev.chars().take(12).collect::<String>().replace('/', "-");
    let Some(dest) = crate::rfd::FileDialog::new()
        .set_title("Create archive")
        .set_file_name(format!("{name}-{short}.zip"))
        .add_filter("Zip", &["zip"])
        .add_filter("Tar", &["tar", "tar.gz", "tgz"])
        .save_file()
    else {
        return;
    };
    run_op(t, &key, "Create archive", move |p, _| {
        hops::archive(p, &rev, &dest).map(|_| dest.display().to_string())
    });
}

/// URL "buat pull/merge request" untuk branch `branch` di repository `key`.
pub fn pull_request_url(t: &mut Tabular, key: &str, branch: &str) -> Option<String> {
    let host = t.git.provider_access().gitlab_host();
    let remote = crate::git::review::remote_from_key(key, &host)?;
    let (h, _) = key.split_once('/')?;
    Some(match remote.provider {
        crate::git::review::Provider::GitHub => format!(
            "https://github.com/{}/compare/{}?expand=1",
            remote.full_name, branch
        ),
        crate::git::review::Provider::GitLab => format!(
            "https://{h}/{}/-/merge_requests/new?merge_request%5Bsource_branch%5D={}",
            remote.full_name, branch
        ),
    })
}

// ─── Dialog aksi ────────────────────────────────────────────────────────────

/// Dialog untuk aksi yang butuh input atau konfirmasi.
#[derive(Debug, Clone)]
pub enum GraphDialog {
    CreateBranch {
        at: String,
        name: String,
        checkout: bool,
        force: bool,
    },
    AddTag {
        at: String,
        name: String,
        annotated: bool,
        message: String,
        push: bool,
        remote: String,
        force: bool,
    },
    CheckoutCommit {
        hash: String,
    },
    CheckoutBranch {
        name: String,
        remote: bool,
        local_name: String,
    },
    Merge {
        rev: String,
        what: String,
        opt: MergeOptions,
    },
    Rebase {
        onto: String,
        what: String,
        ignore_date: bool,
    },
    CherryPick {
        hash: String,
        parents: usize,
        opt: PickOptions,
    },
    Revert {
        hash: String,
        parents: usize,
        mainline: u32,
    },
    DropCommit {
        hash: String,
    },
    Reset {
        hash: String,
        mode: ResetMode,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    DeleteBranch {
        name: String,
        force: bool,
        remote: Option<String>,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    PushBranch {
        branch: String,
        remote: String,
        set_upstream: bool,
        force: ForceMode,
    },
    PullInto {
        remote: String,
        branch: String,
        opt: MergeOptions,
    },
    FetchInto {
        remote: String,
        branch: String,
        local: String,
        force: bool,
    },
    DeleteTag {
        name: String,
        remote: Option<String>,
    },
    PushTag {
        name: String,
        remote: String,
    },
    StashApply {
        selector: String,
        pop: bool,
        index: bool,
    },
    StashDrop {
        selector: String,
    },
    StashBranch {
        selector: String,
        name: String,
    },
    StashPush {
        message: String,
        untracked: bool,
    },
    ResetUncommitted {
        mode: ResetMode,
    },
    CleanUntracked {
        dirs: bool,
    },
    RemoveRemote {
        name: String,
    },
}

impl GraphDialog {
    /// Dialog ini bisa menghilangkan pekerjaan (tombol merah).
    pub fn is_dangerous(&self) -> bool {
        match self {
            Self::DropCommit { .. }
            | Self::DeleteBranch { .. }
            | Self::DeleteRemoteBranch { .. }
            | Self::DeleteTag { .. }
            | Self::StashDrop { .. }
            | Self::CleanUntracked { .. }
            | Self::RemoveRemote { .. } => true,
            Self::Reset { mode, .. } | Self::ResetUncommitted { mode } => *mode == ResetMode::Hard,
            Self::PushBranch { force, .. } => *force != ForceMode::None,
            _ => false,
        }
    }
}

/// Validasi input dialog sebelum dijalankan; `Err` berisi pesan untuk user.
fn validate(repo: &std::path::Path, d: &GraphDialog) -> Result<(), String> {
    let branch_ok = |n: &str| {
        if ops::is_valid_branch_name(repo, n.trim()) {
            Ok(())
        } else {
            Err(format!("\"{}\" is not a valid branch name", n.trim()))
        }
    };
    match d {
        GraphDialog::CreateBranch { name, .. } => branch_ok(name),
        GraphDialog::RenameBranch { new, .. } => branch_ok(new),
        GraphDialog::StashBranch { name, .. } => branch_ok(name),
        GraphDialog::CheckoutBranch {
            remote: true,
            local_name,
            ..
        } => branch_ok(local_name),
        GraphDialog::FetchInto { local, .. } => branch_ok(local),
        GraphDialog::AddTag { name, .. } => {
            if hops::is_valid_tag_name(repo, name.trim()) {
                Ok(())
            } else {
                Err(format!("\"{}\" is not a valid tag name", name.trim()))
            }
        }
        _ => Ok(()),
    }
}

/// Jalankan dialog yang dikonfirmasi. Mengembalikan `false` bila input tidak
/// valid (dialog tetap terbuka).
pub fn execute(t: &mut Tabular, tab_id: usize, d: GraphDialog) -> bool {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return true;
    };
    let (key, repo) = (st.key.clone(), st.repo.clone());
    if let Err(e) = validate(&repo, &d) {
        t.toasts.error(e);
        return false;
    }
    let done = |_: ()| String::new();
    match d {
        GraphDialog::CreateBranch {
            at,
            name,
            checkout,
            force,
        } => run_op(t, &key, "Create branch", move |p, _| {
            hops::create_branch_at(p, name.trim(), &at, checkout, force).map(done)
        }),
        GraphDialog::AddTag {
            at,
            name,
            annotated,
            message,
            push,
            remote,
            force,
        } => run_op(t, &key, "Add tag", move |p, c| {
            let name = name.trim();
            let msg = annotated.then_some(message.as_str());
            hops::add_tag(p, name, &at, msg, force)?;
            if push {
                hops::push_tag(p, &remote, name, c)?;
            }
            Ok(String::new())
        }),
        GraphDialog::CheckoutCommit { hash } => run_op(t, &key, "Checkout", move |p, _| {
            hops::checkout_detached(p, &hash).map(done)
        }),
        GraphDialog::CheckoutBranch {
            name,
            remote,
            local_name,
        } => run_op(t, &key, "Checkout", move |p, _| {
            if remote {
                hops::create_branch_at(p, local_name.trim(), &name, true, false)?;
                crate::git::cli::run_text(
                    p,
                    &["branch", "--set-upstream-to", &name, local_name.trim()],
                )?;
                Ok(String::new())
            } else {
                ops::checkout(p, &name).map(done)
            }
        }),
        GraphDialog::Merge { rev, opt, .. } => run_op(t, &key, "Merge", move |p, _| {
            hops::merge(p, &rev, opt).map(done)
        }),
        GraphDialog::Rebase {
            onto, ignore_date, ..
        } => run_op(t, &key, "Rebase", move |p, _| {
            hops::rebase(p, &onto, ignore_date).map(done)
        }),
        GraphDialog::CherryPick { hash, opt, .. } => run_op(t, &key, "Cherry pick", move |p, _| {
            hops::cherry_pick(p, &hash, opt).map(done)
        }),
        GraphDialog::Revert {
            hash,
            parents,
            mainline,
        } => run_op(t, &key, "Revert", move |p, _| {
            hops::revert(p, &hash, (parents > 1).then_some(mainline)).map(done)
        }),
        GraphDialog::DropCommit { hash } => run_op(t, &key, "Drop commit", move |p, _| {
            hops::drop_commit(p, &hash).map(done)
        }),
        GraphDialog::Reset { hash, mode } => run_op(t, &key, "Reset", move |p, _| {
            hops::reset(p, &hash, mode).map(done)
        }),
        GraphDialog::RenameBranch { old, new } => run_op(t, &key, "Rename branch", move |p, _| {
            hops::rename_branch(p, &old, new.trim()).map(done)
        }),
        GraphDialog::DeleteBranch {
            name,
            force,
            remote,
        } => run_op(t, &key, "Delete branch", move |p, c| {
            ops::remove_branch(p, &name, force)?;
            if let Some(r) = remote {
                hops::delete_remote_branch(p, &r, &name, c)?;
            }
            Ok(String::new())
        }),
        GraphDialog::DeleteRemoteBranch { remote, branch } => {
            run_op(t, &key, "Delete remote branch", move |p, c| {
                hops::delete_remote_branch(p, &remote, &branch, c).map(done)
            })
        }
        GraphDialog::PushBranch {
            branch,
            remote,
            set_upstream,
            force,
        } => run_op(t, &key, "Push", move |p, c| {
            hops::push_branch(p, &remote, &branch, set_upstream, force, c).map(done)
        }),
        GraphDialog::PullInto {
            remote,
            branch,
            opt,
        } => run_op(t, &key, "Pull", move |p, c| {
            hops::pull_into_current(p, &remote, &branch, opt, c).map(done)
        }),
        GraphDialog::FetchInto {
            remote,
            branch,
            local,
            force,
        } => run_op(t, &key, "Fetch into local branch", move |p, c| {
            hops::fetch_into_local(p, &remote, &branch, local.trim(), force, c).map(done)
        }),
        GraphDialog::DeleteTag { name, remote } => run_op(t, &key, "Delete tag", move |p, c| {
            hops::delete_tag(p, &name)?;
            if let Some(r) = remote {
                hops::delete_remote_tag(p, &r, &name, c)?;
            }
            Ok(String::new())
        }),
        GraphDialog::PushTag { name, remote } => run_op(t, &key, "Push tag", move |p, c| {
            hops::push_tag(p, &remote, &name, c).map(done)
        }),
        GraphDialog::StashApply {
            selector,
            pop,
            index,
        } => run_op(
            t,
            &key,
            if pop { "Pop stash" } else { "Apply stash" },
            move |p, _| {
                if pop {
                    stash::pop(p, &selector, index)
                } else {
                    stash::apply(p, &selector, index)
                }
                .map(done)
            },
        ),
        GraphDialog::StashDrop { selector } => run_op(t, &key, "Drop stash", move |p, _| {
            stash::drop(p, &selector).map(done)
        }),
        GraphDialog::StashBranch { selector, name } => {
            run_op(t, &key, "Create branch from stash", move |p, _| {
                stash::branch(p, name.trim(), &selector).map(done)
            })
        }
        GraphDialog::StashPush { message, untracked } => run_op(t, &key, "Stash", move |p, _| {
            stash::push(p, &message, untracked).map(done)
        }),
        GraphDialog::ResetUncommitted { mode } => {
            run_op(t, &key, "Reset uncommitted changes", move |p, _| {
                hops::reset(p, "HEAD", mode).map(done)
            })
        }
        GraphDialog::CleanUntracked { dirs } => {
            run_op(t, &key, "Clean untracked files", move |p, _| {
                hops::clean(p, dirs).map(done)
            })
        }
        GraphDialog::RemoveRemote { name } => {
            run_op(t, &key, "Remove remote", move |p, _| {
                hops::remote_remove(p, &name).map(done)
            });
            if let Some(st) = t.git.graphs.get_mut(&tab_id) {
                // Daftar dimuat ulang oleh `on_repo_changed` setelah operasi selesai.
                st.remotes.clear();
            }
        }
    }
    true
}

/// Simpan form remote (tambah atau ubah).
pub fn save_remote(t: &mut Tabular, tab_id: usize) {
    let Some(st) = t.git.graphs.get_mut(&tab_id) else {
        return;
    };
    let form = std::mem::take(&mut st.remote_form);
    let key = st.key.clone();
    run_op(t, &key, "Save remote", move |p, _| {
        let name = form.name.trim();
        match &form.editing {
            Some(old) => {
                if old != name {
                    hops::remote_rename(p, old, name)?;
                }
                hops::remote_set_url(p, name, &form.url, Some(&form.push_url))?;
            }
            None => {
                hops::remote_add(p, name, &form.url)?;
                if !form.push_url.trim().is_empty() {
                    hops::remote_set_url(p, name, &form.url, Some(&form.push_url))?;
                }
            }
        }
        Ok(String::new())
    });
}

pub fn fetch_remote(t: &mut Tabular, tab_id: usize, remote: Option<String>) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let key = st.key.clone();
    let s = &t.git.store.settings;
    let (prune, prune_tags) = (s.fetch_prune, s.fetch_prune_tags);
    run_op(t, &key, "Fetch", move |p, c| {
        hops::fetch(p, remote.as_deref(), prune, prune_tags, c).map(|_| String::new())
    });
}

pub fn prune_remote(t: &mut Tabular, tab_id: usize, remote: String) {
    let Some(st) = t.git.graphs.get(&tab_id) else {
        return;
    };
    let key = st.key.clone();
    run_op(t, &key, "Prune remote", move |p, c| {
        hops::remote_prune(p, &remote, c).map(|_| String::new())
    });
}

/// Label pendek untuk dialog (hash pendek bila revisi berupa hash).
pub fn rev_label(rev: &str) -> String {
    if rev.len() >= 20 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        rev.chars().take(8).collect()
    } else {
        rev.to_string()
    }
}

// ─── Hasil job ──────────────────────────────────────────────────────────────

pub fn apply(t: &mut Tabular, job: GraphJob, _ctx: &egui::Context) {
    let settings = t.git.store.settings.clone();
    match job {
        GraphJob::Page {
            tab_id,
            seq,
            skip,
            res,
        } => {
            let page_size = settings.graph_page.max(50);
            let Some(st) = t.git.graphs.get_mut(&tab_id) else {
                return;
            };
            if st.seq != seq {
                return;
            }
            st.loading = false;
            match res {
                Ok(page) => {
                    st.done = page.len() < page_size;
                    if skip == 0 {
                        st.commits = page;
                    } else {
                        st.commits.extend(page);
                    }
                    st.error = None;
                    st.rebuild(&settings);
                }
                Err(e) => st.error = Some(git_jobs::describe(&e)),
            }
        }
        GraphJob::Meta { tab_id, seq, res } => {
            let Some(st) = t.git.graphs.get_mut(&tab_id) else {
                return;
            };
            if st.seq != seq {
                return;
            }
            match res {
                Ok((r, s, status)) => {
                    st.refs = r;
                    st.stashes = s;
                    st.status = Some(status);
                    st.rebuild(&settings);
                }
                Err(e) => st.error = Some(git_jobs::describe(&e)),
            }
        }
        GraphJob::Details { tab_id, seq, res } => {
            let Some(d) = t
                .git
                .graphs
                .get_mut(&tab_id)
                .and_then(|s| s.detail.as_mut())
            else {
                return;
            };
            if d.seq != seq {
                return;
            }
            d.loading = false;
            match res {
                Ok((details, files)) => {
                    d.details = details;
                    d.files = files;
                }
                Err(e) => d.error = Some(git_jobs::describe(&e)),
            }
        }
        GraphJob::FilePatch { tab_id, seq, res } => {
            let Some(d) = t
                .git
                .graphs
                .get_mut(&tab_id)
                .and_then(|s| s.detail.as_mut())
            else {
                return;
            };
            if d.seq != seq {
                return;
            }
            d.diff_loading = false;
            match res {
                Ok(p) => {
                    d.binary = diff::is_binary_patch(&p);
                    d.rows = if d.binary {
                        Vec::new()
                    } else {
                        diff::parse_patch(&p)
                    };
                    d.show_all_rows = false;
                }
                Err(e) => d.diff_error = Some(git_jobs::describe(&e)),
            }
        }
        GraphJob::Remotes { tab_id, res } => match res {
            Ok(r) => {
                if let Some(st) = t.git.graphs.get_mut(&tab_id) {
                    st.remotes = r;
                }
            }
            Err(e) => t.toasts.error(format!("Remotes: {e}")),
        },
        GraphJob::Tag { tab_id, res } => match res {
            Ok(tag) => {
                if let Some(st) = t.git.graphs.get_mut(&tab_id) {
                    st.tag_view = Some(tag);
                }
            }
            Err(e) => t.toasts.error(format!("Tag: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(h: &str, parents: &[&str]) -> CommitInfo {
        CommitInfo {
            hash: h.into(),
            short: h.into(),
            author: "Ann".into(),
            email: "a@x".into(),
            time: 0,
            parents: parents.iter().map(|s| s.to_string()).collect(),
            refs: String::new(),
            subject: format!("subject {h}"),
        }
    }

    #[test]
    fn rebuild_inserts_uncommitted_and_stash_rows() {
        let mut st = GraphState::new("k".into(), PathBuf::from("/tmp/x"), Vec::new());
        st.commits = vec![commit("b", &["a"]), commit("a", &[])];
        st.refs.head = Some("b".into());
        st.status = Some(RepoStatus {
            untracked: vec![crate::git::status::FileChange {
                path: "n".into(),
                orig_path: None,
                kind: crate::git::status::ChangeKind::Untracked,
            }],
            ..Default::default()
        });
        st.stashes = vec![StashInfo {
            hash: "s".into(),
            base: "a".into(),
            selector: "stash@{0}".into(),
            time: 0,
            author: "Ann".into(),
            email: "a@x".into(),
            message: "WIP".into(),
        }];
        st.rebuild(&crate::git::repos::GitSettings::default());
        let kinds: Vec<RowKind> = st.items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Uncommitted,
                RowKind::Commit,
                RowKind::Stash,
                RowKind::Commit
            ]
        );
        assert_eq!(st.rows.len(), 4);
        assert_eq!(st.head_index(), Some(1));

        st.find.text = "SUBJECT A".into();
        st.update_find();
        assert_eq!(st.find.matches, [3]);
        st.find.case_sensitive = true;
        st.update_find();
        assert!(st.find.matches.is_empty());
        st.find.regex = true;
        st.find.case_sensitive = false;
        st.find.text = "subject (a|b)".into();
        st.update_find();
        assert_eq!(st.find.matches, [1, 3]);
    }
}
