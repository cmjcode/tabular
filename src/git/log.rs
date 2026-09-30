//! Riwayat commit dan isi satu commit.

use std::path::Path;

use super::status::{ChangeKind, FileChange};
use super::{GitError, cli};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub email: String,
    /// Unix timestamp author.
    pub time: i64,
    pub parents: Vec<String>,
    /// Dekorasi ref, mis. `HEAD -> main, origin/main`.
    pub refs: String,
    pub subject: String,
}

/// Ukuran satu halaman riwayat.
pub const PAGE_SIZE: usize = 200;

const FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%P%x1f%D%x1f%s%x1e";

/// Satu halaman riwayat `rev` (default HEAD). Repository tanpa commit
/// menghasilkan daftar kosong.
pub fn page(
    repo: &Path,
    rev: Option<&str>,
    skip: usize,
    limit: usize,
) -> Result<Vec<CommitInfo>, GitError> {
    let n = format!("-n{limit}");
    let skip = format!("--skip={skip}");
    let mut args = vec!["log", FORMAT, n.as_str(), skip.as_str()];
    if let Some(r) = rev {
        args.push(r);
    }
    args.push("--");
    match cli::run_text(repo, &args) {
        Ok(out) => Ok(parse(&out)),
        Err(GitError::Command { detail, .. })
            if detail.contains("does not have any commits")
                || detail.contains("bad default revision") =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(e),
    }
}

pub fn parse(raw: &str) -> Vec<CommitInfo> {
    raw.split('\u{1e}')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            if rec.is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.split('\u{1f}').collect();
            if f.len() < 8 {
                return None;
            }
            Some(CommitInfo {
                hash: f[0].to_string(),
                short: f[1].to_string(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                time: f[4].parse().unwrap_or(0),
                parents: f[5].split_whitespace().map(str::to_string).collect(),
                refs: f[6].to_string(),
                subject: f[7].trim_end().to_string(),
            })
        })
        .collect()
}

/// Pesan commit lengkap.
pub fn message(repo: &Path, hash: &str) -> Result<String, GitError> {
    cli::run_text(repo, &["show", "-s", "--format=%B", hash, "--"])
        .map(|s| s.trim_end().to_string())
}

/// File yang berubah di commit `hash` dibanding parent pertama.
pub fn commit_files(
    repo: &Path,
    hash: &str,
    first_parent: Option<&str>,
) -> Result<Vec<FileChange>, GitError> {
    let out = match first_parent {
        Some(p) => cli::run_text(repo, &["diff", "-M", "--name-status", "-z", p, hash, "--"])?,
        None => cli::run_text(
            repo,
            &[
                "diff-tree",
                "--root",
                "-r",
                "-M",
                "--no-commit-id",
                "--name-status",
                "-z",
                hash,
                "--",
            ],
        )?,
    };
    Ok(parse_name_status(&out))
}

/// Parse `--name-status -z`: `M\0path\0`, `R100\0old\0new\0`.
pub fn parse_name_status(raw: &str) -> Vec<FileChange> {
    let mut out = Vec::new();
    let mut it = raw.split('\0').filter(|s| !s.is_empty());
    while let Some(code) = it.next() {
        let letter = code.chars().next().unwrap_or('M');
        let kind = match letter {
            'A' => ChangeKind::Added,
            'D' => ChangeKind::Deleted,
            'R' => ChangeKind::Renamed,
            'C' => ChangeKind::Copied,
            'T' => ChangeKind::TypeChanged,
            'U' => ChangeKind::Conflicted,
            _ => ChangeKind::Modified,
        };
        if matches!(kind, ChangeKind::Renamed | ChangeKind::Copied) {
            let (Some(old), Some(new)) = (it.next(), it.next()) else {
                break;
            };
            out.push(FileChange {
                path: new.to_string(),
                orig_path: Some(old.to_string()),
                kind,
            });
        } else if let Some(path) = it.next() {
            out.push(FileChange {
                path: path.to_string(),
                orig_path: None,
                kind,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_records() {
        let raw = "aaaa\u{1f}aa\u{1f}Ann\u{1f}a@x\u{1f}1700000000\u{1f}p1 p2\u{1f}HEAD -> main\u{1f}Merge x\u{1e}\n\
bbbb\u{1f}bb\u{1f}Bob\u{1f}b@x\u{1f}1690000000\u{1f}\u{1f}\u{1f}first\u{1e}\n";
        let c = parse(raw);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].parents, ["p1", "p2"]);
        assert_eq!(c[0].refs, "HEAD -> main");
        assert!(c[1].parents.is_empty());
        assert_eq!(c[1].subject, "first");
    }

    #[test]
    fn parses_name_status() {
        let f = parse_name_status("M\0a.rs\0R087\0old.rs\0new.rs\0A\0b c.rs\0");
        assert_eq!(f.len(), 3);
        assert_eq!(f[1].kind, ChangeKind::Renamed);
        assert_eq!(f[1].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(f[2].path, "b c.rs");
    }
}
