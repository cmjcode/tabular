//! Merge Review: daftar pull request GitHub / merge request GitLab, diff per
//! file, aksi merge/tutup/komentar, dan prompt review AI. Diadaptasi dari
//! ekstensi VS Code "GitMerge Review"; AI memakai backend Tabular
//! (`ai_assistant::start_chat_in`), bukan provider sendiri.
//!
//! Semua fungsi jaringan blocking dan dipanggil dari thread latar. Token tidak
//! pernah masuk log, pesan error, atau prompt.

pub mod github;
pub mod gitlab;
pub mod http;
pub mod prompt;
pub mod types;

pub use types::*;

use super::GitError;

/// Nama secret di keychain.
pub const GITHUB_TOKEN_SECRET: &str = "git.github_token";
pub const GITLAB_TOKEN_SECRET: &str = "git.gitlab_token";
/// URL default GitLab.com.
pub const DEFAULT_GITLAB_URL: &str = "https://gitlab.com";

/// Konfigurasi akses provider; dibaca UI dari keychain & preferences lalu
/// dipindahkan ke thread.
#[derive(Clone, Default)]
pub struct ProviderAccess {
    pub github_token: Option<String>,
    pub gitlab_token: Option<String>,
    pub gitlab_url: String,
}

impl std::fmt::Debug for ProviderAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderAccess")
            .field("github_token", &self.github_token.as_ref().map(|_| "***"))
            .field("gitlab_token", &self.gitlab_token.as_ref().map(|_| "***"))
            .field("gitlab_url", &self.gitlab_url)
            .finish()
    }
}

impl ProviderAccess {
    /// Baca token dari keychain.
    pub fn load(gitlab_url: &str) -> Self {
        let clean = |s: Option<String>| s.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        Self {
            github_token: clean(crate::secrets::get_secret(GITHUB_TOKEN_SECRET)),
            gitlab_token: clean(crate::secrets::get_secret(GITLAB_TOKEN_SECRET)),
            gitlab_url: normalized_gitlab_url(gitlab_url),
        }
    }

    pub fn token(&self, provider: Provider) -> Result<&str, GitError> {
        match provider {
            Provider::GitHub => self
                .github_token
                .as_deref()
                .ok_or(GitError::TokenMissing("GitHub")),
            Provider::GitLab => self
                .gitlab_token
                .as_deref()
                .ok_or(GitError::TokenMissing("GitLab")),
        }
    }

    /// Host GitLab (tanpa skema) untuk mencocokkan kunci repository.
    pub fn gitlab_host(&self) -> String {
        host_of(&self.gitlab_url)
    }
}

pub fn normalized_gitlab_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    if u.is_empty() {
        DEFAULT_GITLAB_URL.to_string()
    } else if u.contains("://") {
        u.to_string()
    } else {
        format!("https://{u}")
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_ascii_lowercase()
}

/// Repository tujuan dari kunci repository (`host/owner/name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    pub provider: Provider,
    /// `owner/name` (GitLab boleh bersarang: `group/sub/name`).
    pub full_name: String,
}

/// Tentukan provider untuk kunci repository. GitLab self-hosted dikenali dari
/// host `gitlab_url` di preferences.
pub fn remote_from_key(key: &str, gitlab_host: &str) -> Option<RemoteRepo> {
    let (host, path) = key.split_once('/')?;
    if path.is_empty() {
        return None;
    }
    let provider = if host == "github.com" {
        Provider::GitHub
    } else if host == "gitlab.com" || host == gitlab_host {
        Provider::GitLab
    } else {
        return None;
    };
    Some(RemoteRepo {
        provider,
        full_name: path.to_string(),
    })
}

/// Daftar MR/PR yang relevan untuk user (review-requested, assigned, author)
/// dari semua provider yang punya token. Error per provider dikembalikan
/// terpisah supaya satu provider yang gagal tidak menyembunyikan yang lain.
pub fn list_mine(access: &ProviderAccess) -> Vec<(Provider, Result<Vec<MergeRequest>, GitError>)> {
    let mut out = Vec::new();
    if let Some(t) = &access.github_token {
        out.push((Provider::GitHub, github::assigned_prs(t)));
    }
    if let Some(t) = &access.gitlab_token {
        out.push((
            Provider::GitLab,
            gitlab::assigned_mrs(t, &access.gitlab_url),
        ));
    }
    out
}

/// Semua MR/PR terbuka milik satu repository.
pub fn list_repo(
    access: &ProviderAccess,
    repo: &RemoteRepo,
    include_closed: bool,
) -> Result<Vec<MergeRequest>, GitError> {
    let token = access.token(repo.provider)?;
    match repo.provider {
        Provider::GitHub => github::repo_prs(token, &repo.full_name, include_closed),
        Provider::GitLab => {
            gitlab::project_mrs(token, &access.gitlab_url, &repo.full_name, include_closed)
        }
    }
}

/// Lengkapi detail (branch, statistik) dan ambil daftar file berubah.
pub fn load_details(
    access: &ProviderAccess,
    mr: &MergeRequest,
) -> Result<(MergeRequest, Vec<ChangedFile>), GitError> {
    let token = access.token(mr.provider)?;
    match mr.provider {
        Provider::GitHub => {
            let detailed = github::pr_details(token, mr)?;
            let files = github::pr_files(token, &mr.repo_full_name, mr.number)?;
            Ok((detailed, files))
        }
        Provider::GitLab => {
            let files = gitlab::mr_changes(token, &access.gitlab_url, mr)?;
            let mut detailed = mr.clone();
            detailed.additions = files.iter().map(|f| f.additions).sum();
            detailed.deletions = files.iter().map(|f| f.deletions).sum();
            detailed.changed_files_count = files.len() as u64;
            Ok((detailed, files))
        }
    }
}

pub fn merge(
    access: &ProviderAccess,
    mr: &MergeRequest,
    method: MergeMethod,
    message: &str,
) -> Result<(), GitError> {
    let token = access.token(mr.provider)?;
    match mr.provider {
        Provider::GitHub => github::merge_pr(token, mr, method, message),
        Provider::GitLab => gitlab::merge_mr(token, &access.gitlab_url, mr, method, message),
    }
}

pub fn close(access: &ProviderAccess, mr: &MergeRequest) -> Result<(), GitError> {
    let token = access.token(mr.provider)?;
    match mr.provider {
        Provider::GitHub => github::close_pr(token, mr),
        Provider::GitLab => gitlab::close_mr(token, &access.gitlab_url, mr),
    }
}

pub fn comment(access: &ProviderAccess, mr: &MergeRequest, body: &str) -> Result<(), GitError> {
    let token = access.token(mr.provider)?;
    match mr.provider {
        Provider::GitHub => github::post_comment(token, mr, body),
        Provider::GitLab => gitlab::post_note(token, &access.gitlab_url, mr, body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_from_repo_key() {
        let gh = remote_from_key("github.com/org/app", "gitlab.example.com").expect("gh");
        assert_eq!(gh.provider, Provider::GitHub);
        assert_eq!(gh.full_name, "org/app");
        let gl =
            remote_from_key("gitlab.example.com/grp/sub/app", "gitlab.example.com").expect("gl");
        assert_eq!(gl.provider, Provider::GitLab);
        assert_eq!(gl.full_name, "grp/sub/app");
        assert!(remote_from_key("bitbucket.org/a/b", "gitlab.com").is_none());
        assert!(remote_from_key("path:/tmp/x", "gitlab.com").is_none());
    }

    #[test]
    fn gitlab_url_normalized_and_token_hidden() {
        assert_eq!(normalized_gitlab_url(""), DEFAULT_GITLAB_URL);
        assert_eq!(normalized_gitlab_url("git.acme.io/"), "https://git.acme.io");
        let a = ProviderAccess {
            github_token: Some("ghp_secret".into()),
            gitlab_token: None,
            gitlab_url: "https://git.acme.io".into(),
        };
        assert_eq!(a.gitlab_host(), "git.acme.io");
        assert!(!format!("{a:?}").contains("ghp_secret"));
        assert!(matches!(
            a.token(Provider::GitLab),
            Err(GitError::TokenMissing("GitLab"))
        ));
    }
}
