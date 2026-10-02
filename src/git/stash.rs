//! Daftar dan operasi stash.

use std::path::Path;

use super::{GitError, cli};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashInfo {
    pub hash: String,
    /// Commit dasar (parent pertama stash).
    pub base: String,
    /// Selector, mis. `stash@{0}`.
    pub selector: String,
    pub time: i64,
    pub author: String,
    pub email: String,
    pub message: String,
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%gd%x1f%at%x1f%an%x1f%ae%x1f%s%x1e";

pub fn list(repo: &Path) -> Result<Vec<StashInfo>, GitError> {
    Ok(parse(&cli::run_text(repo, &["stash", "list", FORMAT])?))
}

pub fn parse(raw: &str) -> Vec<StashInfo> {
    raw.split('\u{1e}')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.split('\u{1f}').collect();
            if f.len() < 7 {
                return None;
            }
            Some(StashInfo {
                hash: f[0].to_string(),
                base: f[1]
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                selector: f[2].to_string(),
                time: f[3].trim().parse().unwrap_or(0),
                author: f[4].to_string(),
                email: f[5].to_string(),
                message: f[6].trim_end().to_string(),
            })
        })
        .collect()
}

fn check_selector(selector: &str) -> Result<(), GitError> {
    if selector.starts_with("stash@{") && selector.ends_with('}') {
        Ok(())
    } else {
        Err(GitError::Parse(format!(
            "invalid stash selector {selector}"
        )))
    }
}

/// Simpan perubahan ke stash baru.
pub fn push(repo: &Path, message: &str, include_untracked: bool) -> Result<(), GitError> {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("--include-untracked");
    }
    if !message.trim().is_empty() {
        args.push("-m");
        args.push(message);
    }
    cli::run_text(repo, &args).map(|_| ())
}

pub fn apply(repo: &Path, selector: &str, reinstate_index: bool) -> Result<(), GitError> {
    check_selector(selector)?;
    let mut args = vec!["stash", "apply"];
    if reinstate_index {
        args.push("--index");
    }
    args.push(selector);
    cli::run_text(repo, &args).map(|_| ())
}

pub fn pop(repo: &Path, selector: &str, reinstate_index: bool) -> Result<(), GitError> {
    check_selector(selector)?;
    let mut args = vec!["stash", "pop"];
    if reinstate_index {
        args.push("--index");
    }
    args.push(selector);
    cli::run_text(repo, &args).map(|_| ())
}

/// Buang stash (tidak bisa dibatalkan dari UI).
pub fn drop(repo: &Path, selector: &str) -> Result<(), GitError> {
    check_selector(selector)?;
    cli::run_text(repo, &["stash", "drop", selector]).map(|_| ())
}

/// Buat branch baru dari stash lalu hapus stash-nya.
pub fn branch(repo: &Path, name: &str, selector: &str) -> Result<(), GitError> {
    check_selector(selector)?;
    cli::run_text(repo, &["stash", "branch", name, selector]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stash_records() {
        let s = parse(
            "s1\u{1f}base1 idx1\u{1f}stash@{0}\u{1f}100\u{1f}Ann\u{1f}a@x\u{1f}WIP on main: x\u{1e}\n",
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].base, "base1");
        assert_eq!(s[0].selector, "stash@{0}");
        assert!(check_selector("stash@{3}").is_ok());
        assert!(check_selector("--all").is_err());
    }
}
