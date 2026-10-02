//! Riwayat untuk Git Graph: halaman commit lintas branch dengan urutan dan
//! filter, detail commit lengkap, dan file berubah pada rentang dua revisi
//! (atau terhadap working tree).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::log::{CommitInfo, parse as parse_log, parse_name_status};
use super::status::{ChangeKind, FileChange};
use super::{GitError, cli};

const FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%P%x1f%D%x1f%s%x1e";

/// Urutan commit di graf.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommitOrder {
    #[default]
    Date,
    AuthorDate,
    Topo,
}

impl CommitOrder {
    pub const ALL: [CommitOrder; 3] = [Self::Date, Self::AuthorDate, Self::Topo];

    pub fn flag(self) -> &'static str {
        match self {
            Self::Date => "--date-order",
            Self::AuthorDate => "--author-date-order",
            Self::Topo => "--topo-order",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Date => "Commit date",
            Self::AuthorDate => "Author date",
            Self::Topo => "Topological",
        }
    }
}

/// Pilihan query graf.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphQuery {
    /// Branch/ref yang ditampilkan; kosong = semua.
    pub branches: Vec<String>,
    pub show_remote: bool,
    pub show_tags: bool,
    pub order: CommitOrder,
    pub first_parent: bool,
    /// Ikutkan commit yang hanya ada di reflog.
    pub reflog: bool,
}

impl GraphQuery {
    fn rev_args(&self) -> Vec<String> {
        if !self.branches.is_empty() {
            return self.branches.clone();
        }
        let mut v = vec!["--branches".to_string()];
        if self.show_tags {
            v.push("--tags".to_string());
        }
        if self.show_remote {
            v.push("--remotes".to_string());
        }
        if self.reflog {
            v.push("--reflog".to_string());
        }
        v.push("HEAD".to_string());
        v
    }
}

/// Satu halaman commit untuk graf. Repository kosong menghasilkan daftar kosong.
pub fn graph_page(
    repo: &Path,
    q: &GraphQuery,
    skip: usize,
    limit: usize,
) -> Result<Vec<CommitInfo>, GitError> {
    let n = format!("-n{limit}");
    let skip = format!("--skip={skip}");
    let mut args: Vec<String> = vec!["log".into(), FORMAT.into(), q.order.flag().into(), n, skip];
    if q.first_parent {
        args.push("--first-parent".into());
    }
    args.extend(q.rev_args());
    args.push("--".into());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match cli::run_text(repo, &refs) {
        Ok(out) => Ok(parse_log(&out)),
        Err(GitError::Command { detail, .. })
            if detail.contains("does not have any commits")
                || detail.contains("bad default revision")
                || detail.contains("unknown revision or path not in the working tree")
                    && q.branches.is_empty() =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(e),
    }
}

/// Detail lengkap satu commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitDetails {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    /// Huruf `%G?`: G (valid), B (buruk), U (tak dikenal), N (tanpa tanda tangan), dll.
    pub signature: char,
    pub signer: String,
    pub signing_key: String,
    pub body: String,
}

impl CommitDetails {
    pub fn signature_label(&self) -> Option<&'static str> {
        Some(match self.signature {
            'G' => "Good signature",
            'B' => "Bad signature",
            'U' => "Good signature, unknown validity",
            'X' => "Good signature, expired",
            'Y' => "Good signature, expired key",
            'R' => "Good signature, revoked key",
            'E' => "Signature cannot be checked",
            _ => return None,
        })
    }
}

pub fn details(repo: &Path, hash: &str, check_signature: bool) -> Result<CommitDetails, GitError> {
    let fmt = if check_signature {
        "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%G?%x1f%GS%x1f%GK%x1f%B"
    } else {
        "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1fN%x1f%x1f%x1f%B"
    };
    let out = cli::run_text(repo, &["show", "-s", fmt, hash, "--"])?;
    parse_details(&out).ok_or_else(|| GitError::Parse(format!("commit {hash}")))
}

pub fn parse_details(raw: &str) -> Option<CommitDetails> {
    let f: Vec<&str> = raw.splitn(12, '\u{1f}').collect();
    if f.len() < 12 {
        return None;
    }
    Some(CommitDetails {
        hash: f[0].trim().to_string(),
        parents: f[1].split_whitespace().map(str::to_string).collect(),
        author: f[2].to_string(),
        author_email: f[3].to_string(),
        author_time: f[4].trim().parse().unwrap_or(0),
        committer: f[5].to_string(),
        committer_email: f[6].to_string(),
        commit_time: f[7].trim().parse().unwrap_or(0),
        signature: f[8].trim().chars().next().unwrap_or('N'),
        signer: f[9].to_string(),
        signing_key: f[10].to_string(),
        body: f[11].trim_end().to_string(),
    })
}

/// File berubah beserta jumlah baris.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub change: FileChange,
    /// `None` untuk file biner.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

/// Rentang yang dibandingkan.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffRange {
    /// Satu commit terhadap parent pertama (commit akar terhadap pohon kosong).
    Commit {
        hash: String,
        parent: Option<String>,
    },
    /// `from` → `to`.
    Between { from: String, to: String },
    /// Revisi → working tree (termasuk file untracked bila `from` = HEAD).
    WorkingTree { from: String },
}

impl DiffRange {
    /// Kunci stabil untuk menyimpan status "sudah dilihat" di code review.
    pub fn review_key(&self) -> String {
        match self {
            Self::Commit { hash, .. } => hash.clone(),
            Self::Between { from, to } => format!("{from}..{to}"),
            Self::WorkingTree { from } => format!("{from}..*"),
        }
    }
}

fn diff_args<'a>(range: &'a DiffRange, extra: &[&'a str]) -> Vec<&'a str> {
    match range {
        DiffRange::Commit { hash, parent: None } => {
            let mut v = vec!["diff-tree", "--root", "-r", "--no-commit-id"];
            v.extend_from_slice(extra);
            v.push(hash);
            v
        }
        DiffRange::Commit {
            hash,
            parent: Some(p),
        } => {
            let mut v = vec!["diff"];
            v.extend_from_slice(extra);
            v.push(p);
            v.push(hash);
            v
        }
        DiffRange::Between { from, to } => {
            let mut v = vec!["diff"];
            v.extend_from_slice(extra);
            v.push(from);
            v.push(to);
            v
        }
        DiffRange::WorkingTree { from } => {
            let mut v = vec!["diff"];
            v.extend_from_slice(extra);
            v.push(from);
            v
        }
    }
}

/// File berubah pada `range` dengan statistik +/-.
pub fn range_files(repo: &Path, range: &DiffRange) -> Result<Vec<FileStat>, GitError> {
    let mut a = diff_args(range, &["-M", "--name-status", "-z"]);
    a.push("--");
    let names = parse_name_status(&cli::run_text(repo, &a)?);
    let mut a = diff_args(range, &["-M", "--numstat", "-z"]);
    a.push("--");
    let stats = parse_numstat(&cli::run_text(repo, &a)?);
    let mut out: Vec<FileStat> = names
        .into_iter()
        .map(|change| {
            let s = stats.iter().find(|(p, _, _)| *p == change.path);
            FileStat {
                additions: s.and_then(|s| s.1),
                deletions: s.and_then(|s| s.2),
                change,
            }
        })
        .collect();
    if let DiffRange::WorkingTree { from } = range
        && from == "HEAD"
    {
        let untracked = cli::run_text(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for p in untracked.split('\0').filter(|p| !p.is_empty()) {
            let lines = std::fs::read(repo.join(p))
                .ok()
                .filter(|b| !b.contains(&0))
                .map(|b| String::from_utf8_lossy(&b).lines().count() as u64);
            out.push(FileStat {
                change: FileChange {
                    path: p.to_string(),
                    orig_path: None,
                    kind: ChangeKind::Untracked,
                },
                additions: lines,
                deletions: lines.map(|_| 0),
            });
        }
    }
    Ok(out)
}

/// Parse `--numstat -z`: `add\tdel\tpath\0` atau `add\tdel\t\0old\0new\0`.
pub fn parse_numstat(raw: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
    let mut out = Vec::new();
    let mut it = raw.split('\0');
    while let Some(rec) = it.next() {
        let rec = rec.trim_start_matches('\n');
        if rec.is_empty() {
            continue;
        }
        let mut f = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let path = if p.is_empty() {
            // Rename: path lama lalu path baru.
            let _old = it.next();
            it.next().unwrap_or_default().to_string()
        } else {
            p.to_string()
        };
        out.push((path, a.parse().ok(), d.parse().ok()));
    }
    out
}

/// Patch satu file pada `range`.
pub fn range_file_patch(
    repo: &Path,
    range: &DiffRange,
    change: &FileChange,
) -> Result<String, GitError> {
    if change.kind == ChangeKind::Untracked {
        return super::diff::untracked(repo, &change.path);
    }
    if let DiffRange::Commit { hash, parent } = range {
        return super::diff::commit_file(
            repo,
            hash,
            parent.as_deref(),
            &change.path,
            change.orig_path.as_deref(),
        );
    }
    let mut a = diff_args(range, &["--no-color", "--no-ext-diff", "-M"]);
    a.push("--");
    a.push(&change.path);
    if let Some(o) = &change.orig_path {
        a.push(o);
    }
    cli::run_text(repo, &a)
}

/// Isi file pada revisi `rev` (None = working tree), untuk "View file at this revision".
pub fn file_at(repo: &Path, rev: Option<&str>, path: &str) -> Result<String, GitError> {
    match rev {
        Some(r) => cli::run_text(repo, &["show", &format!("{r}:{path}")]),
        None => Ok(String::from_utf8_lossy(&std::fs::read(repo.join(path))?).into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat_with_renames_and_binary() {
        let s = parse_numstat("3\t1\ta.rs\0-\t-\timg.png\x002\t0\t\0old.rs\0new.rs\0");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], ("a.rs".to_string(), Some(3), Some(1)));
        assert_eq!(s[1].1, None);
        assert_eq!(s[2].0, "new.rs");
    }

    #[test]
    fn parses_commit_details() {
        let d = parse_details(
            "abc\u{1f}p1 p2\u{1f}Ann\u{1f}a@x\u{1f}10\u{1f}Bob\u{1f}b@x\u{1f}20\u{1f}G\u{1f}Ann <a@x>\u{1f}KEY\u{1f}subject\n\nbody\n",
        )
        .expect("details");
        assert_eq!(d.parents.len(), 2);
        assert_eq!(d.committer, "Bob");
        assert_eq!(d.commit_time, 20);
        assert_eq!(d.signature_label(), Some("Good signature"));
        assert_eq!(d.body, "subject\n\nbody");
    }

    #[test]
    fn query_args_follow_options() {
        let mut q = GraphQuery {
            show_remote: true,
            show_tags: true,
            ..Default::default()
        };
        assert_eq!(q.rev_args(), ["--branches", "--tags", "--remotes", "HEAD"]);
        q.branches = vec!["main".into()];
        assert_eq!(q.rev_args(), ["main"]);
    }
}
