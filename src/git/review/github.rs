//! GitHub REST API v3 (port `githubService.ts`).

use std::collections::HashSet;

use serde::Deserialize;

use super::http::{client, send_json, send_text};
use super::types::{ChangedFile, FileStatus, MergeMethod, MergeRequest, MrState, Provider};
use crate::git::GitError;

const BASE: &str = "https://api.github.com";
/// Batas halaman file per PR (100 file per halaman).
const MAX_FILE_PAGES: usize = 30;

#[derive(Deserialize)]
struct GhUser {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct GhLabel {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct GhRef {
    #[serde(rename = "ref", default)]
    name: String,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Deserialize)]
struct GhPullRequest {
    id: u64,
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<GhUser>,
    head: GhRef,
    base: GhRef,
    #[serde(default)]
    state: String,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    requested_reviewers: Vec<GhUser>,
    #[serde(default)]
    comments: Option<u64>,
    #[serde(default)]
    additions: Option<u64>,
    #[serde(default)]
    deletions: Option<u64>,
    #[serde(default)]
    changed_files: Option<u64>,
    #[serde(default)]
    draft: Option<bool>,
}

#[derive(Deserialize)]
struct GhIssue {
    id: u64,
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
    #[serde(default)]
    repository_url: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    user: Option<GhUser>,
    #[serde(default)]
    comments: Option<u64>,
}

#[derive(Deserialize)]
struct GhSearch {
    #[serde(default)]
    items: Vec<GhIssue>,
}

#[derive(Deserialize)]
struct GhFile {
    filename: String,
    #[serde(default)]
    previous_filename: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(default)]
    patch: Option<String>,
}

fn get(token: &str, url: &str) -> Result<reqwest::blocking::RequestBuilder, GitError> {
    Ok(with_headers(client()?.get(url), token))
}

fn with_headers(
    req: reqwest::blocking::RequestBuilder,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    req.bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

fn repo_path(full_name: &str) -> String {
    // `owner/name` → aman sebagai path; tiap segmen di-encode.
    full_name
        .split('/')
        .map(super::http::encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// PR terbuka yang relevan untuk user di semua repository: review diminta,
/// di-assign, atau dibuat sendiri. Detail branch diisi [`pr_details`].
pub fn assigned_prs(token: &str) -> Result<Vec<MergeRequest>, GitError> {
    const QUERIES: [&str; 3] = [
        "is:pr is:open review-requested:@me",
        "is:pr is:open assignee:@me",
        "is:pr is:open author:@me",
    ];
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut last_err = None;
    let mut any_ok = false;
    for q in QUERIES {
        let url = format!(
            "{BASE}/search/issues?q={}&per_page=100",
            super::http::encode_segment(q)
        );
        match get(token, &url).and_then(send_json::<GhSearch>) {
            Ok(res) => {
                any_ok = true;
                log::debug!(
                    "[GIT] GitHub search \"{q}\" returned {} item(s)",
                    res.items.len()
                );
                for issue in res.items {
                    if issue.pull_request.is_none() || !seen.insert(issue.id) {
                        continue;
                    }
                    out.push(map_issue(issue));
                }
            }
            Err(e) => {
                log::warn!("[GIT] GitHub search \"{q}\" failed: {e}");
                last_err = Some(e);
            }
        }
    }
    match (any_ok, last_err) {
        (false, Some(e)) => Err(e),
        _ => {
            out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            Ok(out)
        }
    }
}

/// PR milik satu repository.
pub fn repo_prs(
    token: &str,
    full_name: &str,
    include_closed: bool,
) -> Result<Vec<MergeRequest>, GitError> {
    let state = if include_closed { "all" } else { "open" };
    let url = format!(
        "{BASE}/repos/{}/pulls?state={state}&per_page=50&sort=updated&direction=desc",
        repo_path(full_name)
    );
    let raw: Vec<GhPullRequest> = send_json(get(token, &url)?)?;
    Ok(raw.into_iter().map(|pr| map_pr(pr, full_name)).collect())
}

/// Detail lengkap satu PR (branch, statistik, reviewer).
pub fn pr_details(token: &str, mr: &MergeRequest) -> Result<MergeRequest, GitError> {
    let url = format!(
        "{BASE}/repos/{}/pulls/{}",
        repo_path(&mr.repo_full_name),
        mr.number
    );
    let pr: GhPullRequest = send_json(get(token, &url)?)?;
    Ok(map_pr(pr, &mr.repo_full_name))
}

pub fn pr_files(token: &str, full_name: &str, number: u64) -> Result<Vec<ChangedFile>, GitError> {
    let mut out = Vec::new();
    for page in 1..=MAX_FILE_PAGES {
        let url = format!(
            "{BASE}/repos/{}/pulls/{number}/files?per_page=100&page={page}",
            repo_path(full_name)
        );
        let raw: Vec<GhFile> = send_json(get(token, &url)?)?;
        let n = raw.len();
        out.extend(raw.into_iter().map(|f| ChangedFile {
            filename: f.filename,
            old_filename: f.previous_filename,
            status: map_file_status(&f.status),
            additions: f.additions,
            deletions: f.deletions,
            patch: f.patch,
        }));
        if n < 100 {
            break;
        }
    }
    Ok(out)
}

pub fn merge_pr(
    token: &str,
    mr: &MergeRequest,
    method: MergeMethod,
    message: &str,
) -> Result<(), GitError> {
    let url = format!(
        "{BASE}/repos/{}/pulls/{}/merge",
        repo_path(&mr.repo_full_name),
        mr.number
    );
    let mut body = serde_json::json!({
        "merge_method": match method {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        },
    });
    if !message.trim().is_empty() {
        body["commit_message"] = serde_json::Value::String(message.to_string());
    }
    if let Some(sha) = &mr.head_sha {
        // Tolak merge bila branch berubah sejak diff ditinjau.
        body["sha"] = serde_json::Value::String(sha.clone());
    }
    send_text(with_headers(client()?.put(url), token).json(&body)).map(|_| ())
}

pub fn close_pr(token: &str, mr: &MergeRequest) -> Result<(), GitError> {
    let url = format!(
        "{BASE}/repos/{}/pulls/{}",
        repo_path(&mr.repo_full_name),
        mr.number
    );
    let body = serde_json::json!({ "state": "closed" });
    send_text(with_headers(client()?.patch(url), token).json(&body)).map(|_| ())
}

pub fn post_comment(token: &str, mr: &MergeRequest, text: &str) -> Result<(), GitError> {
    let url = format!(
        "{BASE}/repos/{}/issues/{}/comments",
        repo_path(&mr.repo_full_name),
        mr.number
    );
    let body = serde_json::json!({ "body": text });
    send_text(with_headers(client()?.post(url), token).json(&body)).map(|_| ())
}

fn map_issue(issue: GhIssue) -> MergeRequest {
    // repository_url: https://api.github.com/repos/owner/name
    let full = issue
        .repository_url
        .split_once("/repos/")
        .map(|(_, r)| r.to_string())
        .unwrap_or_default();
    MergeRequest {
        id: issue.id,
        number: issue.number,
        title: issue.title,
        description: issue.body.unwrap_or_default(),
        author: issue.user.map(|u| u.login).unwrap_or_default(),
        source_branch: String::new(),
        target_branch: String::new(),
        state: MrState::Open,
        url: issue.html_url,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
        provider: Provider::GitHub,
        repo_full_name: full,
        labels: issue.labels.into_iter().map(|l| l.name).collect(),
        reviewers: Vec::new(),
        comment_count: issue.comments.unwrap_or(0),
        additions: 0,
        deletions: 0,
        changed_files_count: 0,
        is_draft: issue.draft.unwrap_or(false),
        gitlab_project_id: None,
        head_sha: None,
    }
}

fn map_pr(pr: GhPullRequest, full_name: &str) -> MergeRequest {
    let state = if pr.merged_at.is_some() {
        MrState::Merged
    } else if pr.state == "open" {
        MrState::Open
    } else {
        MrState::Closed
    };
    MergeRequest {
        id: pr.id,
        number: pr.number,
        title: pr.title,
        description: pr.body.unwrap_or_default(),
        author: pr.user.map(|u| u.login).unwrap_or_default(),
        source_branch: pr.head.name,
        target_branch: pr.base.name,
        state,
        url: pr.html_url,
        created_at: pr.created_at,
        updated_at: pr.updated_at,
        provider: Provider::GitHub,
        repo_full_name: full_name.to_string(),
        labels: pr.labels.into_iter().map(|l| l.name).collect(),
        reviewers: pr
            .requested_reviewers
            .into_iter()
            .map(|u| u.login)
            .collect(),
        comment_count: pr.comments.unwrap_or(0),
        additions: pr.additions.unwrap_or(0),
        deletions: pr.deletions.unwrap_or(0),
        changed_files_count: pr.changed_files.unwrap_or(0),
        is_draft: pr.draft.unwrap_or(false),
        gitlab_project_id: None,
        head_sha: pr.head.sha,
    }
}

fn map_file_status(s: &str) -> FileStatus {
    match s {
        "added" => FileStatus::Added,
        "removed" => FileStatus::Deleted,
        "renamed" => FileStatus::Renamed,
        "copied" => FileStatus::Copied,
        _ => FileStatus::Modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_search_issue() {
        let json = r#"{"items":[
            {"id":1,"number":7,"title":"Fix","body":null,"draft":true,"pull_request":{"url":"x"},
             "repository_url":"https://api.github.com/repos/org/app","html_url":"https://github.com/org/app/pull/7",
             "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z",
             "labels":[{"name":"bug"}],"user":{"login":"ann"}},
            {"id":2,"number":8,"title":"Issue only","repository_url":"https://api.github.com/repos/org/app"}
        ]}"#;
        let res: GhSearch = serde_json::from_str(json).expect("parse");
        let prs: Vec<MergeRequest> = res
            .items
            .into_iter()
            .filter(|i| i.pull_request.is_some())
            .map(map_issue)
            .collect();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].repo_full_name, "org/app");
        assert!(prs[0].is_draft);
        assert_eq!(prs[0].description, "");
        assert_eq!(prs[0].display_number(), "#7");
    }

    #[test]
    fn maps_pull_and_files() {
        let json = r#"{"id":3,"number":9,"title":"T","body":"b","user":{"login":"bob"},
            "head":{"ref":"feat","sha":"abc"},"base":{"ref":"main"},"state":"closed","merged_at":"2026-01-03T00:00:00Z",
            "html_url":"u","created_at":"c","updated_at":"u","labels":[],"requested_reviewers":[{"login":"cy"}],
            "additions":5,"deletions":2,"changed_files":1,"draft":false}"#;
        let pr: GhPullRequest = serde_json::from_str(json).expect("parse");
        let mr = map_pr(pr, "org/app");
        assert_eq!(mr.state, MrState::Merged);
        assert_eq!(mr.source_branch, "feat");
        assert_eq!(mr.head_sha.as_deref(), Some("abc"));
        assert_eq!(mr.reviewers, ["cy"]);
        assert_eq!(map_file_status("removed"), FileStatus::Deleted);
        assert_eq!(repo_path("org/my app"), "org/my%20app");
    }
}
