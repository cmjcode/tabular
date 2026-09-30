//! Ref terstruktur untuk label di Git Graph: branch lokal, branch remote, tag,
//! dan HEAD, dikelompokkan per commit.

use std::collections::HashMap;
use std::path::Path;

use super::{GitError, cli};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    /// Branch lokal (`refs/heads/x`).
    Head,
    /// Branch remote (`refs/remotes/origin/x`).
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub kind: RefKind,
    /// Nama pendek: `main`, `origin/main`, `v1.0`.
    pub name: String,
    /// Branch lokal yang sedang di-checkout.
    pub is_current: bool,
    /// Tag annotated (punya pesan sendiri).
    pub annotated: bool,
}

impl RefLabel {
    /// Remote dan nama branch untuk ref remote (`origin/feat/x` → `origin`, `feat/x`).
    pub fn remote_parts(&self) -> Option<(&str, &str)> {
        (self.kind == RefKind::Remote)
            .then(|| self.name.split_once('/'))
            .flatten()
    }
}

/// Semua ref repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefSet {
    /// Hash commit penuh → label.
    pub by_commit: HashMap<String, Vec<RefLabel>>,
    /// Commit HEAD (None bila repository kosong).
    pub head: Option<String>,
    /// Branch lokal aktif (None bila detached).
    pub head_branch: Option<String>,
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub tags: Vec<String>,
    pub remotes: Vec<String>,
}

impl RefSet {
    pub fn labels(&self, hash: &str) -> &[RefLabel] {
        self.by_commit.get(hash).map_or(&[], Vec::as_slice)
    }
}

const FORMAT: &str =
    "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(HEAD)%1f%(objecttype)";

pub fn list(repo: &Path) -> Result<RefSet, GitError> {
    let out = cli::run_text(
        repo,
        &[
            "for-each-ref",
            FORMAT,
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    let mut set = parse(&out);
    set.head = cli::run_text(repo, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    set.remotes = cli::run_text(repo, &["remote"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    Ok(set)
}

pub fn parse(raw: &str) -> RefSet {
    let mut set = RefSet::default();
    for line in raw.lines() {
        let f: Vec<&str> = line.split('\u{1f}').collect();
        if f.len() < 5 {
            continue;
        }
        let full = f[0];
        let (kind, name) = if let Some(n) = full.strip_prefix("refs/heads/") {
            (RefKind::Head, n)
        } else if let Some(n) = full.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, n)
        } else if let Some(n) = full.strip_prefix("refs/tags/") {
            (RefKind::Tag, n)
        } else {
            continue;
        };
        // Tag annotated menunjuk objek tag; commit-nya ada di `*objectname`.
        let annotated = f[4].trim() == "tag";
        let commit = if annotated && !f[2].is_empty() {
            f[2]
        } else {
            f[1]
        };
        let is_current = kind == RefKind::Head && f[3].trim() == "*";
        if is_current {
            set.head_branch = Some(name.to_string());
        }
        match kind {
            RefKind::Head => set.local.push(name.to_string()),
            RefKind::Remote => set.remote.push(name.to_string()),
            RefKind::Tag => set.tags.push(name.to_string()),
        }
        set.by_commit
            .entry(commit.to_string())
            .or_default()
            .push(RefLabel {
                kind,
                name: name.to_string(),
                is_current,
                annotated,
            });
    }
    for labels in set.by_commit.values_mut() {
        labels.sort_by(|a, b| {
            (!a.is_current, a.kind, &a.name).cmp(&(!b.is_current, b.kind, &b.name))
        });
    }
    set
}

/// Detail tag annotated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagDetails {
    pub name: String,
    pub hash: String,
    pub tagger: String,
    pub email: String,
    pub time: i64,
    pub message: String,
}

pub fn tag_details(repo: &Path, name: &str) -> Result<TagDetails, GitError> {
    let r = format!("refs/tags/{name}");
    let out = cli::run_text(
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname)%1f%(taggername)%1f%(taggeremail:trim)%1f%(taggerdate:unix)%1f%(contents)",
            &r,
        ],
    )?;
    let f: Vec<&str> = out.splitn(5, '\u{1f}').collect();
    if f.len() < 5 {
        return Err(GitError::Parse(format!("tag {name} not found")));
    }
    Ok(TagDetails {
        name: name.to_string(),
        hash: f[0].to_string(),
        tagger: f[1].to_string(),
        email: f[2].to_string(),
        time: f[3].trim().parse().unwrap_or(0),
        message: f[4].trim_end().to_string(),
    })
}

/// Satu remote dengan URL fetch/push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

pub fn remotes(repo: &Path) -> Result<Vec<RemoteInfo>, GitError> {
    Ok(parse_remotes(&cli::run_text(repo, &["remote", "-v"])?))
}

pub fn parse_remotes(raw: &str) -> Vec<RemoteInfo> {
    let mut out: Vec<RemoteInfo> = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let idx = match out.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                out.push(RemoteInfo {
                    name: name.to_string(),
                    ..Default::default()
                });
                out.len() - 1
            }
        };
        let url = crate::repo_scan::redact(url);
        if kind == "(push)" {
            out[idx].push_url = url;
        } else {
            out[idx].fetch_url = url;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_refs_by_commit_and_peels_tags() {
        let raw = "refs/heads/main\u{1f}aaa\u{1f}\u{1f}*\u{1f}commit\n\
refs/heads/feat\u{1f}bbb\u{1f}\u{1f} \u{1f}commit\n\
refs/remotes/origin/HEAD\u{1f}aaa\u{1f}\u{1f} \u{1f}commit\n\
refs/remotes/origin/main\u{1f}aaa\u{1f}\u{1f} \u{1f}commit\n\
refs/tags/v1\u{1f}ttt\u{1f}bbb\u{1f} \u{1f}tag\n\
refs/tags/light\u{1f}aaa\u{1f}\u{1f} \u{1f}commit\n";
        let s = parse(raw);
        assert_eq!(s.head_branch.as_deref(), Some("main"));
        let a = s.labels("aaa");
        assert_eq!(a.len(), 3);
        assert!(a[0].is_current && a[0].name == "main");
        assert_eq!(a[1].kind, RefKind::Remote);
        assert_eq!(a[1].remote_parts(), Some(("origin", "main")));
        assert_eq!(a[2].kind, RefKind::Tag);
        let b = s.labels("bbb");
        assert!(b.iter().any(|l| l.name == "v1" && l.annotated));
        assert!(s.labels("ttt").is_empty());
        assert_eq!(s.remote, ["origin/main"]);
    }

    #[test]
    fn parses_remote_urls() {
        let r = parse_remotes(
            "origin\thttps://github.com/a/b.git (fetch)\norigin\tgit@github.com:a/b.git (push)\nup\thttps://x/y (fetch)\nup\thttps://x/y (push)\n",
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].fetch_url, "https://github.com/a/b.git");
        assert_eq!(r[0].push_url, "git@github.com:a/b.git");
        assert_eq!(r[1].name, "up");
    }
}
