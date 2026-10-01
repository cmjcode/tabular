//! Menjalankan `git` sebagai proses anak: tanpa prompt interaktif, dengan
//! batas waktu dan pembatalan. Dipakai git client dan `repo_scan`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::GitError;
use crate::agent::harness;

/// Batas waktu default satu perintah git (clone/fetch/push bisa lama).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Flag pembatalan yang tidak pernah diset, untuk perintah singkat.
pub static NEVER: AtomicBool = AtomicBool::new(false);

/// Jalankan git di `cwd` (atau direktori proses bila `None`) dan kembalikan
/// stdout. Exit code bukan nol menjadi [`GitError::Command`] /
/// [`GitError::Auth`].
pub fn run(
    cwd: Option<&Path>,
    args: &[&str],
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<Vec<u8>, GitError> {
    run_with_codes(cwd, args, cancel, timeout, &[0]).map(|(out, _)| out)
}

/// Perintah singkat tanpa pembatalan; stdout sebagai teks UTF-8 (lossy).
pub fn run_text(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = run(Some(cwd), args, &NEVER, DEFAULT_TIMEOUT)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Seperti [`run`], tetapi exit code di `ok_codes` dianggap sukses (mis.
/// `git diff --exit-code` memakai 1 untuk "ada perbedaan").
pub fn run_with_codes(
    cwd: Option<&Path>,
    args: &[&str],
    cancel: &AtomicBool,
    timeout: Duration,
    ok_codes: &[i32],
) -> Result<(Vec<u8>, i32), GitError> {
    let action = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .copied()
        .unwrap_or("command")
        .to_string();
    let git = harness::resolve_binary("git").ok_or(GitError::GitMissing)?;
    let mut cmd = Command::new(git);
    cmd.args(args)
        .env("PATH", harness::augmented_path())
        // Repo privat tanpa kredensial tersimpan harus gagal, bukan menunggu input.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        // `git status` tidak perlu mengunci index; aman saat editor lain aktif.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::GitMissing
        } else {
            GitError::Command {
                action: action.clone(),
                detail: format!("cannot start git: {e}"),
            }
        }
    })?;
    // Baca stdout/stderr di thread lain supaya pipe penuh tidak membuat git macet.
    let stdout_reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut out, &mut buf);
            buf
        })
    });
    let stderr_reader = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut err, &mut buf);
            buf
        })
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if cancel.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GitError::Cancelled);
                }
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GitError::Command {
                        action,
                        detail: format!("timed out after {}s", timeout.as_secs()),
                    });
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return Err(GitError::Command {
                    action,
                    detail: format!("wait failed: {e}"),
                });
            }
        }
    };
    let stderr = stderr_reader
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stdout = stdout_reader
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    if ok_codes.contains(&code) {
        return Ok((stdout, code));
    }
    let detail = crate::repo_scan::redact(&clean_stderr(&stderr));
    let detail = if detail.is_empty() {
        format!("{status}")
    } else {
        detail
    };
    if is_auth_failure(&detail) {
        return Err(GitError::Auth { action, detail });
    }
    if detail.contains("not a git repository") {
        return Err(GitError::NotARepo(
            cwd.map(|p| p.display().to_string()).unwrap_or_default(),
        ));
    }
    Err(GitError::Command { action, detail })
}

/// Ringkas stderr git menjadi penyebab gagalnya saja: peringatan ssh dan
/// progres fetch (`From …`, daftar ref) dibuang bila ada baris `error:` /
/// `fatal:`, dan baris yang berulang hanya ditampilkan sekali.
pub fn clean_stderr(stderr: &str) -> String {
    const NOISE: &[&str] = &["Pseudo-terminal will not be allocated"];
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim_end)
        .filter(|l| !NOISE.iter().any(|n| l.starts_with(n)))
        .collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with("error:") || l.starts_with("fatal:"))
        .unwrap_or(0);
    let mut out: Vec<&str> = Vec::new();
    for line in &lines[start..] {
        if !out.contains(line) {
            out.push(line);
        }
    }
    out.join("\n").trim().to_string()
}

/// stderr git yang menandakan kredensial tidak tersedia.
pub fn is_auth_failure(stderr: &str) -> bool {
    const MARKERS: &[&str] = &[
        "Authentication failed",
        "could not read Username",
        "could not read Password",
        "terminal prompts disabled",
        "Permission denied (publickey",
        "Host key verification failed",
        "invalid credentials",
    ];
    MARKERS.iter().any(|m| stderr.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_markers_detected() {
        assert!(is_auth_failure(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ));
        assert!(is_auth_failure(
            "git@github.com: Permission denied (publickey)."
        ));
        assert!(!is_auth_failure("fatal: not a git repository"));
    }

    #[test]
    fn stderr_keeps_only_the_cause() {
        let raw = "Pseudo-terminal will not be allocated because stdin is not a terminal.\n\
From gitlab.com:group/repo\n   8ff4d05..2728dac  a -> origin/a\n * [new tag]         v1 -> v1\n\
error: Your local changes to the following files would be overwritten by merge:\n\tDockerfile\n\
Please commit your changes or stash them before you merge.\n\
error: Your local changes to the following files would be overwritten by merge:\n\t.DS_Store\n\
Please commit your changes or stash them before you merge.\nAborting\n";
        assert_eq!(
            clean_stderr(raw),
            "error: Your local changes to the following files would be overwritten by merge:\n\
\tDockerfile\nPlease commit your changes or stash them before you merge.\n\t.DS_Store\nAborting"
        );
        // Tanpa baris error, isi dipertahankan kecuali peringatan ssh.
        assert_eq!(
            clean_stderr("Pseudo-terminal will not be allocated\nsomething odd\n"),
            "something odd"
        );
    }

    #[test]
    fn missing_repo_is_reported() {
        if harness::resolve_binary("git").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("tabular-git-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        // GIT_CEILING_DIRECTORIES tidak bisa diset lewat helper; cukup pastikan error bertipe.
        let res = run(
            Some(&dir),
            &["rev-parse", "--show-toplevel"],
            &NEVER,
            DEFAULT_TIMEOUT,
        );
        if let Err(e) = res {
            assert!(
                matches!(e, GitError::NotARepo(_) | GitError::Command { .. }),
                "{e}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
