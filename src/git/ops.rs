//! Operasi yang mengubah repository: stage, commit, branch, sinkron remote.
//! Semua path dikirim setelah `--` supaya nama file tidak dibaca sebagai opsi.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::{GitError, cli};

fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    cli::run_text(repo, args)
}

fn with_paths<'a>(mut base: Vec<&'a str>, paths: &'a [String]) -> Vec<&'a str> {
    base.push("--");
    base.extend(paths.iter().map(String::as_str));
    base
}

/// Root working tree dari `dir` (bisa subfolder repository).
pub fn discover_root(dir: &Path) -> Result<std::path::PathBuf, GitError> {
    let out = run(dir, &["rev-parse", "--show-toplevel"])?;
    let root = out.trim();
    if root.is_empty() {
        return Err(GitError::NotARepo(dir.display().to_string()));
    }
    Ok(std::path::PathBuf::from(root))
}

pub fn init(dir: &Path) -> Result<(), GitError> {
    std::fs::create_dir_all(dir)?;
    run(dir, &["init"]).map(|_| ())
}

pub fn stage(repo: &Path, paths: &[String]) -> Result<(), GitError> {
    run(repo, &with_paths(vec!["add", "-A"], paths)).map(|_| ())
}

pub fn stage_all(repo: &Path) -> Result<(), GitError> {
    run(repo, &["add", "-A"]).map(|_| ())
}

/// Keluarkan dari index. Repository tanpa commit tidak punya HEAD, jadi
/// memakai `rm --cached`.
pub fn unstage(repo: &Path, paths: &[String], has_head: bool) -> Result<(), GitError> {
    if has_head {
        run(repo, &with_paths(vec!["restore", "--staged"], paths)).map(|_| ())
    } else {
        run(repo, &with_paths(vec!["rm", "--cached", "-r", "-q"], paths)).map(|_| ())
    }
}

pub fn unstage_all(repo: &Path, has_head: bool) -> Result<(), GitError> {
    if has_head {
        run(repo, &["reset", "-q"]).map(|_| ())
    } else {
        run(repo, &["rm", "--cached", "-r", "-q", "."]).map(|_| ())
    }
}

/// Buang perubahan working tree file terlacak (kembali ke isi index).
pub fn discard_tracked(repo: &Path, paths: &[String]) -> Result<(), GitError> {
    run(repo, &with_paths(vec!["restore", "--worktree"], paths)).map(|_| ())
}

/// Hapus file untracked dari disk.
pub fn discard_untracked(repo: &Path, paths: &[String]) -> Result<(), GitError> {
    run(repo, &with_paths(vec!["clean", "-f", "-q"], paths)).map(|_| ())
}

pub fn commit(repo: &Path, message: &str, amend: bool) -> Result<String, GitError> {
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }
    run(repo, &args)
}

pub fn checkout(repo: &Path, branch: &str) -> Result<(), GitError> {
    run(repo, &["switch", branch]).map(|_| ())
}

/// Checkout branch remote `origin/x` sebagai branch lokal `x` yang tracking.
pub fn checkout_remote(repo: &Path, remote_branch: &str) -> Result<(), GitError> {
    let local = remote_branch.split_once('/').map_or(remote_branch, |(_, b)| b);
    run(repo, &["switch", "-c", local, "--track", remote_branch]).map(|_| ())
}

pub fn create_branch(repo: &Path, name: &str, checkout: bool) -> Result<(), GitError> {
    if checkout {
        run(repo, &["switch", "-c", name]).map(|_| ())
    } else {
        run(repo, &["branch", "--", name]).map(|_| ())
    }
}

/// Hapus branch lokal; `force` mengizinkan branch yang belum di-merge.
pub fn remove_branch(repo: &Path, name: &str, force: bool) -> Result<(), GitError> {
    let flag = if force { "-D" } else { "-d" };
    run(repo, &["branch", flag, "--", name]).map(|_| ())
}

pub fn fetch(repo: &Path, cancel: &AtomicBool) -> Result<(), GitError> {
    cli::run(Some(repo), &["fetch", "--all", "--prune"], cancel, cli::DEFAULT_TIMEOUT).map(|_| ())
}

/// Pull tanpa membuat merge commit diam-diam; `rebase` memakai `--rebase`.
pub fn pull(repo: &Path, rebase: bool, cancel: &AtomicBool) -> Result<(), GitError> {
    let mode = if rebase { "--rebase" } else { "--ff-only" };
    cli::run(Some(repo), &["pull", mode], cancel, cli::DEFAULT_TIMEOUT).map(|_| ())
}

/// Push branch aktif. Branch tanpa upstream di-push ke `origin` dengan `-u`.
pub fn push(
    repo: &Path,
    branch: &str,
    has_upstream: bool,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    if has_upstream {
        cli::run(Some(repo), &["push"], cancel, cli::DEFAULT_TIMEOUT).map(|_| ())
    } else {
        cli::run(
            Some(repo),
            &["push", "-u", "origin", branch],
            cancel,
            cli::DEFAULT_TIMEOUT,
        )
        .map(|_| ())
    }
}

/// Clone `url` ke `dest` (folder belum ada atau kosong).
pub fn clone(url: &str, dest: &Path, cancel: &AtomicBool) -> Result<(), GitError> {
    crate::repo_scan::clone_into(url, dest, cancel).map_err(|e| match e {
        crate::repo_scan::RepoScanError::GitMissing => GitError::GitMissing,
        crate::repo_scan::RepoScanError::Cancelled => GitError::Cancelled,
        other => {
            let detail = other.to_string();
            if cli::is_auth_failure(&detail) {
                GitError::Auth {
                    action: "clone".to_string(),
                    detail,
                }
            } else {
                GitError::Command {
                    action: "clone".to_string(),
                    detail,
                }
            }
        }
    })
}

/// Validasi nama branch memakai aturan git sendiri.
pub fn is_valid_branch_name(repo: &Path, name: &str) -> bool {
    !name.trim().is_empty()
        && !name.starts_with('-')
        && run(repo, &["check-ref-format", "--branch", name]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness;
    use crate::git::{branch, log, status};
    use std::path::PathBuf;

    struct TempRepo(PathBuf);
    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_repo(tag: &str) -> Option<TempRepo> {
        harness::resolve_binary("git")?;
        let dir = std::env::temp_dir().join(format!(
            "tabular-git-ops-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        init(&dir).expect("init");
        run(&dir, &["config", "user.email", "t@example.com"]).expect("cfg");
        run(&dir, &["config", "user.name", "Tester"]).expect("cfg");
        run(&dir, &["config", "commit.gpgsign", "false"]).expect("cfg");
        Some(TempRepo(dir))
    }

    #[test]
    fn stage_commit_branch_roundtrip() {
        let Some(repo) = temp_repo("flow") else {
            eprintln!("git not installed; skipping");
            return;
        };
        let r = &repo.0;
        std::fs::write(r.join("a.txt"), "one\n").expect("write");
        std::fs::write(r.join("b c.txt"), "x\n").expect("write");

        let st = status::read(r).expect("status");
        assert_eq!(st.untracked.len(), 2);
        assert!(st.head_oid.is_none());
        assert!(log::page(r, None, 0, 10).expect("log").is_empty());

        stage(r, &["a.txt".to_string()]).expect("stage");
        unstage(r, &["a.txt".to_string()], false).expect("unstage no head");
        stage_all(r).expect("stage all");
        assert_eq!(status::read(r).expect("status").staged.len(), 2);
        commit(r, "first", false).expect("commit");

        let st = status::read(r).expect("status");
        assert!(st.is_clean());
        let main = st.branch.clone().expect("branch");

        std::fs::write(r.join("a.txt"), "two\n").expect("write");
        let st = status::read(r).expect("status");
        assert_eq!(st.unstaged[0].path, "a.txt");
        let patch = crate::git::diff::worktree(r, "a.txt").expect("diff");
        assert_eq!(crate::git::diff::count_lines(&patch), (1, 1));
        discard_tracked(r, &["a.txt".to_string()]).expect("discard");
        assert!(status::read(r).expect("status").is_clean());

        std::fs::write(r.join("junk.txt"), "j\n").expect("write");
        discard_untracked(r, &["junk.txt".to_string()]).expect("clean");
        assert!(!r.join("junk.txt").exists());

        assert!(is_valid_branch_name(r, "feat/x"));
        assert!(!is_valid_branch_name(r, "bad..name"));
        create_branch(r, "feat/x", true).expect("branch");
        assert_eq!(status::read(r).expect("status").branch.as_deref(), Some("feat/x"));
        checkout(r, &main).expect("checkout");
        let names: Vec<String> = branch::list(r).expect("branches").into_iter().map(|b| b.name).collect();
        assert!(names.contains(&"feat/x".to_string()), "{names:?}");
        remove_branch(r, "feat/x", false).expect("remove");

        std::fs::write(r.join("a.txt"), "three\n").expect("write");
        stage_all(r).expect("stage");
        commit(r, "second", false).expect("commit");
        let commits = log::page(r, None, 0, 10).expect("log");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "second");
        let files = log::commit_files(r, &commits[0].hash, commits[0].parents.first().map(String::as_str))
            .expect("files");
        assert_eq!(files.len(), 1);
        let root_files = log::commit_files(r, &commits[1].hash, None).expect("root files");
        assert_eq!(root_files.len(), 2);
        let p = crate::git::diff::commit_file(r, &commits[0].hash, Some(&commits[1].hash), "a.txt", None)
            .expect("commit diff");
        assert!(p.contains("+three"));
        assert_eq!(discover_root(&r.join(".")).expect("root").file_name(), r.file_name());
    }
}
