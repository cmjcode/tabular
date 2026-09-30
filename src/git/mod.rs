//! Git client headless: operasi repository lokal lewat `git` CLI dan Merge
//! Review (pull request GitHub / merge request GitLab).
//!
//! Modul ini tidak bergantung pada `window_egui`; semua fungsi menerima data
//! polos (path repository, token, URL) sehingga bisa diuji dan dipanggil dari
//! thread latar. UI ada di `window_egui::git_*`.

pub mod branch;
pub mod cli;
pub mod diff;
pub mod log;
pub mod ops;
pub mod repos;
pub mod review;
pub mod status;

/// Error domain git client.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git is not installed or not found in PATH")]
    GitMissing,
    #[error("Not a git repository: {0}")]
    NotARepo(String),
    #[error("git {action} failed: {detail}")]
    Command { action: String, detail: String },
    #[error("git {action} needs credentials: {detail}")]
    Auth { action: String, detail: String },
    #[error("Cancelled")]
    Cancelled,
    #[error("{0} token is not configured. Set it in Preferences > Git.")]
    TokenMissing(&'static str),
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("Network error: {0}")]
    Network(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unexpected response: {0}")]
    Parse(String),
}
