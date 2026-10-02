//! Daftar branch lokal dan remote dari `git for-each-ref`.

use std::path::Path;

use super::{GitError, cli};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// Nama pendek, mis. `main` atau `origin/main`.
    pub name: String,
    pub is_remote: bool,
    pub is_head: bool,
    pub oid: String,
    pub upstream: Option<String>,
    /// Keterangan tracking, mis. `ahead 1, behind 2` atau `gone`.
    pub track: String,
    pub subject: String,
    /// Unix timestamp commit terakhir.
    pub time: i64,
}

const FORMAT: &str = "%(refname)%1f%(refname:short)%1f%(objectname:short)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(HEAD)%1f%(committerdate:unix)%1f%(contents:subject)";

pub fn list(repo: &Path) -> Result<Vec<BranchInfo>, GitError> {
    let fmt = format!("--format={FORMAT}");
    let out = cli::run_text(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            &fmt,
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse(&out))
}

pub fn parse(raw: &str) -> Vec<BranchInfo> {
    raw.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\u{1f}').collect();
            if f.len() < 8 {
                return None;
            }
            let full = f[0];
            // `refs/remotes/origin/HEAD` hanya alias.
            if full.ends_with("/HEAD") {
                return None;
            }
            Some(BranchInfo {
                name: f[1].to_string(),
                is_remote: full.starts_with("refs/remotes/"),
                is_head: f[5].trim() == "*",
                oid: f[2].to_string(),
                upstream: Some(f[3].to_string()).filter(|s| !s.is_empty()),
                track: f[4].to_string(),
                time: f[6].trim().parse().unwrap_or(0),
                subject: f[7].to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_refs_and_skips_remote_head() {
        let raw = "refs/heads/main\u{1f}main\u{1f}abc1234\u{1f}origin/main\u{1f}ahead 1\u{1f}*\u{1f}1700000000\u{1f}init\n\
refs/heads/feat\u{1f}feat\u{1f}def5678\u{1f}\u{1f}\u{1f} \u{1f}1700000001\u{1f}wip: x\n\
refs/remotes/origin/HEAD\u{1f}origin\u{1f}abc1234\u{1f}\u{1f}\u{1f} \u{1f}0\u{1f}\n\
refs/remotes/origin/main\u{1f}origin/main\u{1f}abc1234\u{1f}\u{1f}\u{1f} \u{1f}1700000000\u{1f}init\n";
        let b = parse(raw);
        assert_eq!(b.len(), 3);
        assert!(b[0].is_head && !b[0].is_remote);
        assert_eq!(b[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(b[0].track, "ahead 1");
        assert!(b[1].upstream.is_none());
        assert!(b[2].is_remote);
        assert_eq!(b[2].name, "origin/main");
    }
}
