//! Operasi riwayat untuk menu konteks Git Graph: merge, rebase, cherry-pick,
//! revert, reset, tag, remote, dan kelanjutan operasi yang berhenti karena
//! konflik.
//!
//! Nama branch/tag/remote dari user ditolak bila diawali `-` supaya tidak
//! dibaca sebagai opsi; hash selalu berasal dari git sendiri.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::{GitError, cli};

fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    cli::run_text(repo, args)
}

fn net(repo: &Path, args: &[&str], cancel: &AtomicBool) -> Result<String, GitError> {
    cli::run(Some(repo), args, cancel, cli::DEFAULT_TIMEOUT)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn check_name(what: &str, name: &str) -> Result<(), GitError> {
    if name.trim().is_empty() || name.starts_with('-') || name.contains(char::is_whitespace) {
        return Err(GitError::Parse(format!("invalid {what} name \"{name}\"")));
    }
    Ok(())
}

/// Validasi nama tag memakai aturan git.
pub fn is_valid_tag_name(repo: &Path, name: &str) -> bool {
    check_name("tag", name).is_ok()
        && run(repo, &["check-ref-format", &format!("refs/tags/{name}")]).is_ok()
}

// ─── State repository ───────────────────────────────────────────────────────

/// Operasi multi-langkah yang sedang berhenti (biasanya karena konflik).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RepoState {
    #[default]
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

impl RepoState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Clean => "",
            Self::Merging => "Merge in progress",
            Self::Rebasing => "Rebase in progress",
            Self::CherryPicking => "Cherry-pick in progress",
            Self::Reverting => "Revert in progress",
        }
    }

    fn command(self) -> Option<&'static str> {
        match self {
            Self::Clean => None,
            Self::Merging => Some("merge"),
            Self::Rebasing => Some("rebase"),
            Self::CherryPicking => Some("cherry-pick"),
            Self::Reverting => Some("revert"),
        }
    }
}

pub fn state(repo: &Path) -> Result<RepoState, GitError> {
    let dir = run(repo, &["rev-parse", "--absolute-git-dir"])?;
    let dir = Path::new(dir.trim());
    Ok(
        if dir.join("rebase-merge").is_dir() || dir.join("rebase-apply").is_dir() {
            RepoState::Rebasing
        } else if dir.join("MERGE_HEAD").is_file() {
            RepoState::Merging
        } else if dir.join("CHERRY_PICK_HEAD").is_file() {
            RepoState::CherryPicking
        } else if dir.join("REVERT_HEAD").is_file() {
            RepoState::Reverting
        } else {
            RepoState::Clean
        },
    )
}

/// Lanjutkan operasi setelah konflik diselesaikan dan di-stage.
pub fn continue_op(repo: &Path, st: RepoState) -> Result<(), GitError> {
    match st.command() {
        Some(cmd) => run(repo, &["-c", "core.editor=true", cmd, "--continue"]).map(|_| ()),
        None => Ok(()),
    }
}

pub fn abort_op(repo: &Path, st: RepoState) -> Result<(), GitError> {
    match st.command() {
        Some(cmd) => run(repo, &[cmd, "--abort"]).map(|_| ()),
        None => Ok(()),
    }
}

// ─── Merge / rebase / cherry-pick / revert ──────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeOptions {
    /// Selalu buat merge commit.
    pub no_ff: bool,
    /// Gabungkan jadi satu set perubahan tanpa merge commit.
    pub squash: bool,
    /// Berhenti sebelum commit.
    pub no_commit: bool,
}

/// Merge `rev` ke branch aktif.
pub fn merge(repo: &Path, rev: &str, opt: MergeOptions) -> Result<(), GitError> {
    check_name("revision", rev)?;
    let mut args = vec!["merge", "--no-edit"];
    if opt.squash {
        args.push("--squash");
    } else if opt.no_ff {
        args.push("--no-ff");
    }
    if opt.no_commit && !opt.squash {
        args.push("--no-commit");
    }
    args.push(rev);
    run(repo, &args).map(|_| ())
}

/// Rebase branch aktif ke atas `onto`.
pub fn rebase(repo: &Path, onto: &str, ignore_date: bool) -> Result<(), GitError> {
    check_name("revision", onto)?;
    let mut args = vec!["-c", "core.editor=true", "rebase"];
    if ignore_date {
        args.push("--ignore-date");
    }
    args.push(onto);
    run(repo, &args).map(|_| ())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PickOptions {
    /// Tambahkan "(cherry picked from commit …)" di pesan.
    pub record_origin: bool,
    pub no_commit: bool,
    /// Parent acuan untuk merge commit (1-based).
    pub mainline: Option<u32>,
}

pub fn cherry_pick(repo: &Path, hash: &str, opt: PickOptions) -> Result<(), GitError> {
    let m;
    let mut args = vec!["cherry-pick"];
    if opt.record_origin {
        args.push("-x");
    }
    if opt.no_commit {
        args.push("--no-commit");
    }
    if let Some(n) = opt.mainline {
        m = n.to_string();
        args.push("-m");
        args.push(&m);
    }
    args.push(hash);
    run(repo, &args).map(|_| ())
}

pub fn revert(repo: &Path, hash: &str, mainline: Option<u32>) -> Result<(), GitError> {
    let m;
    let mut args = vec!["revert", "--no-edit"];
    if let Some(n) = mainline {
        m = n.to_string();
        args.push("-m");
        args.push(&m);
    }
    args.push(hash);
    run(repo, &args).map(|_| ())
}

/// Buang satu commit dari branch aktif (rebase `hash^` → atas `hash`).
pub fn drop_commit(repo: &Path, hash: &str) -> Result<(), GitError> {
    let parent = format!("{hash}^");
    run(
        repo,
        &["-c", "core.editor=true", "rebase", "--onto", &parent, hash],
    )
    .map(|_| ())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    #[default]
    Mixed,
    Hard,
}

impl ResetMode {
    pub const ALL: [ResetMode; 3] = [Self::Soft, Self::Mixed, Self::Hard];

    fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft - keep all changes staged",
            Self::Mixed => "Mixed - keep changes in the working tree, unstaged",
            Self::Hard => "Hard - discard all changes",
        }
    }
}

/// Pindahkan branch aktif ke `rev`.
pub fn reset(repo: &Path, rev: &str, mode: ResetMode) -> Result<(), GitError> {
    check_name("revision", rev)?;
    run(repo, &["reset", mode.flag(), rev]).map(|_| ())
}

/// Checkout commit sebagai HEAD detached.
pub fn checkout_detached(repo: &Path, hash: &str) -> Result<(), GitError> {
    run(repo, &["switch", "--detach", hash]).map(|_| ())
}

/// Hapus file untracked; `dirs` juga menghapus folder untracked.
pub fn clean(repo: &Path, dirs: bool) -> Result<(), GitError> {
    let args: &[&str] = if dirs {
        &["clean", "-f", "-d"]
    } else {
        &["clean", "-f"]
    };
    run(repo, args).map(|_| ())
}

// ─── Branch ─────────────────────────────────────────────────────────────────

/// Buat branch di `at` (bukan hanya HEAD).
pub fn create_branch_at(
    repo: &Path,
    name: &str,
    at: &str,
    checkout: bool,
    force: bool,
) -> Result<(), GitError> {
    check_name("branch", name)?;
    if checkout {
        let flag = if force { "-C" } else { "-c" };
        run(repo, &["switch", flag, name, at]).map(|_| ())
    } else {
        let mut args = vec!["branch"];
        if force {
            args.push("-f");
        }
        args.extend([name, at]);
        run(repo, &args).map(|_| ())
    }
}

pub fn rename_branch(repo: &Path, old: &str, new: &str) -> Result<(), GitError> {
    check_name("branch", old)?;
    check_name("branch", new)?;
    run(repo, &["branch", "-m", old, new]).map(|_| ())
}

pub fn delete_remote_branch(
    repo: &Path,
    remote: &str,
    branch: &str,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    check_name("remote", remote)?;
    check_name("branch", branch)?;
    net(repo, &["push", remote, "--delete", branch], cancel).map(|_| ())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ForceMode {
    #[default]
    None,
    /// `--force-with-lease`: gagal bila remote berubah sejak fetch terakhir.
    Lease,
    Force,
}

pub fn push_branch(
    repo: &Path,
    remote: &str,
    branch: &str,
    set_upstream: bool,
    force: ForceMode,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    check_name("remote", remote)?;
    check_name("branch", branch)?;
    let mut args = vec!["push"];
    if set_upstream {
        args.push("-u");
    }
    match force {
        ForceMode::None => {}
        ForceMode::Lease => args.push("--force-with-lease"),
        ForceMode::Force => args.push("--force"),
    }
    args.extend([remote, branch]);
    net(repo, &args, cancel).map(|_| ())
}

/// Pull `remote/branch` ke branch aktif.
pub fn pull_into_current(
    repo: &Path,
    remote: &str,
    branch: &str,
    opt: MergeOptions,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    check_name("remote", remote)?;
    check_name("branch", branch)?;
    let mut args = vec!["pull", "--no-edit"];
    if opt.squash {
        args.push("--squash");
    } else if opt.no_ff {
        args.push("--no-ff");
    }
    args.extend([remote, branch]);
    net(repo, &args, cancel).map(|_| ())
}

/// Perbarui branch lokal `local` dari `remote/branch` tanpa checkout.
pub fn fetch_into_local(
    repo: &Path,
    remote: &str,
    branch: &str,
    local: &str,
    force: bool,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    check_name("remote", remote)?;
    check_name("branch", branch)?;
    check_name("branch", local)?;
    let spec = format!("{}{branch}:{local}", if force { "+" } else { "" });
    net(repo, &["fetch", remote, &spec], cancel).map(|_| ())
}

/// Fetch semua remote (atau satu remote) dengan pilihan prune.
pub fn fetch(
    repo: &Path,
    remote: Option<&str>,
    prune: bool,
    prune_tags: bool,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    let mut args = vec!["fetch"];
    match remote {
        Some(r) => {
            check_name("remote", r)?;
            args.push(r);
        }
        None => args.push("--all"),
    }
    if prune {
        args.push("--prune");
    }
    if prune_tags {
        args.push("--prune-tags");
    }
    net(repo, &args, cancel).map(|_| ())
}

// ─── Tag ────────────────────────────────────────────────────────────────────

/// Buat tag di `at`. `message` terisi = tag annotated.
pub fn add_tag(
    repo: &Path,
    name: &str,
    at: &str,
    message: Option<&str>,
    force: bool,
) -> Result<(), GitError> {
    check_name("tag", name)?;
    let mut args = vec!["tag"];
    if force {
        args.push("-f");
    }
    if let Some(m) = message {
        args.extend(["-a", "-m", m]);
    }
    args.extend([name, at]);
    run(repo, &args).map(|_| ())
}

pub fn delete_tag(repo: &Path, name: &str) -> Result<(), GitError> {
    check_name("tag", name)?;
    run(repo, &["tag", "-d", name]).map(|_| ())
}

pub fn push_tag(
    repo: &Path,
    remote: &str,
    name: &str,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    check_name("remote", remote)?;
    check_name("tag", name)?;
    let spec = format!("refs/tags/{name}");
    net(repo, &["push", remote, &spec], cancel).map(|_| ())
}

pub fn delete_remote_tag(
    repo: &Path,
    remote: &str,
    name: &str,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    check_name("remote", remote)?;
    check_name("tag", name)?;
    let spec = format!("refs/tags/{name}");
    net(repo, &["push", remote, "--delete", &spec], cancel).map(|_| ())
}

// ─── Remote ─────────────────────────────────────────────────────────────────

pub fn remote_add(repo: &Path, name: &str, url: &str) -> Result<(), GitError> {
    check_name("remote", name)?;
    if url.trim().is_empty() || url.starts_with('-') {
        return Err(GitError::Parse("invalid remote URL".into()));
    }
    run(repo, &["remote", "add", name, url.trim()]).map(|_| ())
}

pub fn remote_remove(repo: &Path, name: &str) -> Result<(), GitError> {
    check_name("remote", name)?;
    run(repo, &["remote", "remove", name]).map(|_| ())
}

pub fn remote_rename(repo: &Path, old: &str, new: &str) -> Result<(), GitError> {
    check_name("remote", old)?;
    check_name("remote", new)?;
    run(repo, &["remote", "rename", old, new]).map(|_| ())
}

/// Ubah URL fetch (dan push bila `push_url` diisi).
pub fn remote_set_url(
    repo: &Path,
    name: &str,
    url: &str,
    push_url: Option<&str>,
) -> Result<(), GitError> {
    check_name("remote", name)?;
    if url.trim().is_empty() || url.starts_with('-') {
        return Err(GitError::Parse("invalid remote URL".into()));
    }
    run(repo, &["remote", "set-url", name, url.trim()])?;
    if let Some(p) = push_url.map(str::trim).filter(|p| !p.is_empty()) {
        if p.starts_with('-') {
            return Err(GitError::Parse("invalid push URL".into()));
        }
        run(repo, &["remote", "set-url", "--push", name, p])?;
    }
    Ok(())
}

pub fn remote_prune(repo: &Path, name: &str, cancel: &AtomicBool) -> Result<(), GitError> {
    check_name("remote", name)?;
    net(repo, &["remote", "prune", name], cancel).map(|_| ())
}

// ─── Archive ────────────────────────────────────────────────────────────────

/// Simpan isi `rev` sebagai arsip; format dari ekstensi (`.zip`, `.tar`, `.tar.gz`).
pub fn archive(repo: &Path, rev: &str, dest: &Path) -> Result<(), GitError> {
    let name = dest.to_string_lossy().to_lowercase();
    let format = if name.ends_with(".zip") {
        "zip"
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        "tar.gz"
    } else {
        "tar"
    };
    let fmt = format!("--format={format}");
    let out = format!("--output={}", dest.display());
    run(repo, &["archive", &fmt, &out, rev]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness;
    use crate::git::{ops, refs, stash, status};
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
            "tabular-git-hist-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        ops::init(&dir).expect("init");
        for (k, v) in [
            ("user.email", "t@example.com"),
            ("user.name", "Tester"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
        ] {
            run(&dir, &["config", k, v]).expect("cfg");
        }
        Some(TempRepo(dir))
    }

    fn commit_file(r: &Path, name: &str, body: &str, msg: &str) -> String {
        std::fs::write(r.join(name), body).expect("write");
        ops::stage_all(r).expect("stage");
        ops::commit(r, msg, false).expect("commit");
        run(r, &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string()
    }

    #[test]
    fn history_operations_roundtrip() {
        let Some(repo) = temp_repo("ops") else {
            eprintln!("git not installed; skipping");
            return;
        };
        let r = &repo.0;
        let base = commit_file(r, "a.txt", "1\n", "base");
        let main = status::read(r).expect("st").branch.expect("branch");

        create_branch_at(r, "feat", &base, true, false).expect("branch");
        let f1 = commit_file(r, "f.txt", "f\n", "feat one");
        ops::checkout(r, &main).expect("checkout");
        commit_file(r, "b.txt", "b\n", "main two");

        // Merge dengan merge commit.
        merge(
            r,
            "feat",
            MergeOptions {
                no_ff: true,
                ..Default::default()
            },
        )
        .expect("merge");
        let parents = run(r, &["show", "-s", "--format=%P", "HEAD"]).expect("show");
        assert_eq!(parents.split_whitespace().count(), 2);
        assert_eq!(state(r).expect("state"), RepoState::Clean);

        // Tag annotated & lightweight.
        add_tag(r, "v1", "HEAD", Some("release"), false).expect("tag");
        add_tag(r, "light", &base, None, false).expect("tag");
        let set = refs::list(r).expect("refs");
        assert!(set.labels(&base).iter().any(|l| l.name == "light"));
        let head = set.head.clone().expect("head");
        assert!(
            set.labels(&head)
                .iter()
                .any(|l| l.name == "v1" && l.annotated)
        );
        assert_eq!(refs::tag_details(r, "v1").expect("tag").message, "release");
        delete_tag(r, "light").expect("del tag");
        assert!(is_valid_tag_name(r, "v2.0"));
        assert!(!is_valid_tag_name(r, "-x"));

        // Cherry-pick lalu revert.
        create_branch_at(r, "other", &base, true, false).expect("other");
        cherry_pick(r, &f1, PickOptions::default()).expect("pick");
        assert!(r.join("f.txt").exists());
        let picked = run(r, &["rev-parse", "HEAD"]).expect("head");
        revert(r, picked.trim(), None).expect("revert");
        assert!(!r.join("f.txt").exists());

        // Reset & drop commit.
        let x = commit_file(r, "x.txt", "x\n", "x");
        commit_file(r, "y.txt", "y\n", "y");
        drop_commit(r, &x).expect("drop");
        assert!(!r.join("x.txt").exists());
        assert!(r.join("y.txt").exists());
        reset(r, &base, ResetMode::Hard).expect("reset");
        assert!(!r.join("y.txt").exists());

        rename_branch(r, "other", "renamed").expect("rename");
        assert!(
            refs::list(r)
                .expect("refs")
                .local
                .contains(&"renamed".to_string())
        );

        // Stash.
        std::fs::write(r.join("a.txt"), "changed\n").expect("write");
        stash::push(r, "wip", false).expect("stash");
        let list = stash::list(r).expect("list");
        assert_eq!(list.len(), 1);
        assert!(list[0].message.contains("wip"));
        stash::apply(r, &list[0].selector, false).expect("apply");
        stash::drop(r, &list[0].selector).expect("drop stash");
        assert!(stash::list(r).expect("list").is_empty());

        // Konflik merge → state Merging → abort.
        ops::checkout(r, &main).expect("checkout");
        std::fs::write(r.join("a.txt"), "main\n").expect("write");
        ops::stage_all(r).expect("stage");
        ops::commit(r, "main edit", false).expect("commit");
        ops::checkout(r, "renamed").expect("checkout");
        std::fs::write(r.join("a.txt"), "other\n").expect("write");
        ops::stage_all(r).expect("stage");
        ops::commit(r, "other edit", false).expect("commit");
        assert!(merge(r, &main, MergeOptions::default()).is_err());
        assert_eq!(state(r).expect("state"), RepoState::Merging);
        abort_op(r, RepoState::Merging).expect("abort");
        assert_eq!(state(r).expect("state"), RepoState::Clean);

        // Remote lokal.
        remote_add(r, "up", "https://example.invalid/x.git").expect("remote");
        remote_set_url(r, "up", "https://example.invalid/y.git", None).expect("url");
        let rem = refs::remotes(r).expect("remotes");
        assert_eq!(rem[0].fetch_url, "https://example.invalid/y.git");
        remote_rename(r, "up", "up2").expect("rename remote");
        remote_remove(r, "up2").expect("remove remote");
        assert!(remote_add(r, "-x", "u").is_err());

        let dest = r.join("out.zip");
        archive(r, "HEAD", &dest).expect("archive");
        assert!(dest.is_file());
    }
}
