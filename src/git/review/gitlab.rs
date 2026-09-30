//! GitLab REST API v4 (port `gitlabService.ts`), termasuk GitLab self-hosted.

use std::collections::HashSet;

use serde::Deserialize;

use super::http::{client, encode_segment, send_json, send_text};
use super::types::{ChangedFile, FileStatus, MergeMethod, MergeRequest, MrState, Provider};
use crate::git::GitError;

/// Batas halaman diff per MR (100 file per halaman).
const MAX_DIFF_PAGES: usize = 30;

#[derive(Deserialize)]
struct GlUser {
    #[serde(default)]
    username: String,
}

#[derive(Deserialize)]
struct GlMergeRequest {
    id: u64,
    iid: u64,
    project_id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<GlUser>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    web_url: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    reviewers: Vec<GlUser>,
    #[serde(default)]
    user_notes_count: u64,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Deserialize)]
struct GlDiff {
    #[serde(default)]
    old_path: String,
    #[serde(default)]
    new_path: String,
    #[serde(default)]
    new_file: bool,
    #[serde(default)]
    deleted_file: bool,
    #[serde(default)]
    renamed_file: bool,
    #[serde(default)]
    diff: String,
}

#[derive(Deserialize)]
struct GlChanges {
    #[serde(default)]
    changes: Vec<GlDiff>,
}

fn api(base: &str, path: &str) -> String {
    format!("{}/api/v4{path}", base.trim_end_matches('/'))
}

fn with_token(req: reqwest::blocking::RequestBuilder, token: &str) -> reqwest::blocking::RequestBuilder {
    req.header("PRIVATE-TOKEN", token)
}

fn get<T: serde::de::DeserializeOwned>(token: &str, url: &str) -> Result<T, GitError> {
    send_json(with_token(client()?.get(url), token))
}

fn project_ref(mr: &MergeRequest) -> String {
    mr.gitlab_project_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| encode_segment(&mr.repo_full_name))
}

/// MR terbuka yang di-assign ke user atau meminta review user.
pub fn assigned_mrs(token: &str, base: &str) -> Result<Vec<MergeRequest>, GitError> {
    let assigned: Vec<GlMergeRequest> = get(
        token,
        &api(base, "/merge_requests?scope=assigned_to_me&state=opened&per_page=100"),
    )?;
    let mut all = assigned;
    match get::<GlUser>(token, &api(base, "/user")) {
        Ok(me) if !me.username.is_empty() => {
            let url = api(
                base,
                &format!(
                    "/merge_requests?scope=all&reviewer_username={}&state=opened&per_page=100",
                    encode_segment(&me.username)
                ),
            );
            match get::<Vec<GlMergeRequest>>(token, &url) {
                Ok(r) => all.extend(r),
                Err(e) => log::warn!("[GIT] GitLab reviewer query failed: {e}"),
            }
            let url = api(base, "/merge_requests?scope=created_by_me&state=opened&per_page=100");
            match get::<Vec<GlMergeRequest>>(token, &url) {
                Ok(r) => all.extend(r),
                Err(e) => log::warn!("[GIT] GitLab created_by_me query failed: {e}"),
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("[GIT] GitLab /user failed: {e}"),
    }
    let mut seen = HashSet::new();
    let mut out: Vec<MergeRequest> = all
        .into_iter()
        .filter(|m| seen.insert(m.id))
        .map(map_mr)
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

/// MR milik satu project (`group/sub/name`).
pub fn project_mrs(token: &str, base: &str, full_name: &str, include_closed: bool) -> Result<Vec<MergeRequest>, GitError> {
    let state = if include_closed { "" } else { "&state=opened" };
    let url = api(
        base,
        &format!(
            "/projects/{}/merge_requests?per_page=50&order_by=updated_at{state}",
            encode_segment(full_name)
        ),
    );
    let raw: Vec<GlMergeRequest> = get(token, &url)?;
    Ok(raw.into_iter().map(map_mr).collect())
}

/// File berubah di MR. Memakai endpoint `/diffs` (GitLab 15.7+) dengan
/// cadangan `/changes` untuk server lama.
pub fn mr_changes(token: &str, base: &str, mr: &MergeRequest) -> Result<Vec<ChangedFile>, GitError> {
    let project = project_ref(mr);
    let mut diffs: Vec<GlDiff> = Vec::new();
    for page in 1..=MAX_DIFF_PAGES {
        let url = api(
            base,
            &format!("/projects/{project}/merge_requests/{}/diffs?per_page=100&page={page}", mr.number),
        );
        match get::<Vec<GlDiff>>(token, &url) {
            Ok(batch) => {
                let n = batch.len();
                diffs.extend(batch);
                if n < 100 {
                    break;
                }
            }
            Err(GitError::Http { status: 404, .. }) if page == 1 => {
                let url = api(base, &format!("/projects/{project}/merge_requests/{}/changes", mr.number));
                diffs = get::<GlChanges>(token, &url)?.changes;
                break;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(diffs.into_iter().map(map_diff).collect())
}

pub fn merge_mr(token: &str, base: &str, mr: &MergeRequest, method: MergeMethod, message: &str) -> Result<(), GitError> {
    let url = api(base, &format!("/projects/{}/merge_requests/{}/merge", project_ref(mr), mr.number));
    let mut body = serde_json::json!({
        "should_remove_source_branch": false,
        "squash": method == MergeMethod::Squash,
    });
    if !message.trim().is_empty() {
        let key = if method == MergeMethod::Squash { "squash_commit_message" } else { "merge_commit_message" };
        body[key] = serde_json::Value::String(message.to_string());
    }
    if let Some(sha) = &mr.head_sha {
        body["sha"] = serde_json::Value::String(sha.clone());
    }
    send_text(with_token(client()?.put(url), token).json(&body)).map(|_| ())
}

pub fn close_mr(token: &str, base: &str, mr: &MergeRequest) -> Result<(), GitError> {
    let url = api(base, &format!("/projects/{}/merge_requests/{}", project_ref(mr), mr.number));
    let body = serde_json::json!({ "state_event": "close" });
    send_text(with_token(client()?.put(url), token).json(&body)).map(|_| ())
}

pub fn post_note(token: &str, base: &str, mr: &MergeRequest, text: &str) -> Result<(), GitError> {
    let url = api(base, &format!("/projects/{}/merge_requests/{}/notes", project_ref(mr), mr.number));
    let body = serde_json::json!({ "body": text });
    send_text(with_token(client()?.post(url), token).json(&body)).map(|_| ())
}

/// Path project dari `web_url` (`https://host/grp/sub/app/-/merge_requests/1`).
fn full_name_from_web_url(web_url: &str) -> String {
    let rest = web_url.split_once("://").map_or(web_url, |(_, r)| r);
    let path = rest.split_once('/').map_or("", |(_, p)| p);
    path.split("/-/").next().unwrap_or(path).trim_matches('/').to_string()
}

fn map_mr(mr: GlMergeRequest) -> MergeRequest {
    let state = match mr.state.as_str() {
        "opened" => MrState::Open,
        "merged" => MrState::Merged,
        _ => MrState::Closed,
    };
    MergeRequest {
        id: mr.id,
        number: mr.iid,
        title: mr.title,
        description: mr.description.unwrap_or_default(),
        author: mr.author.map(|a| a.username).unwrap_or_default(),
        source_branch: mr.source_branch,
        target_branch: mr.target_branch,
        state,
        repo_full_name: full_name_from_web_url(&mr.web_url),
        url: mr.web_url,
        created_at: mr.created_at,
        updated_at: mr.updated_at,
        provider: Provider::GitLab,
        labels: mr.labels,
        reviewers: mr.reviewers.into_iter().map(|r| r.username).collect(),
        comment_count: mr.user_notes_count,
        additions: 0,
        deletions: 0,
        changed_files_count: 0,
        is_draft: mr.draft,
        gitlab_project_id: Some(mr.project_id),
        head_sha: mr.sha,
    }
}

fn map_diff(d: GlDiff) -> ChangedFile {
    let (additions, deletions) = crate::git::diff::count_lines(&d.diff);
    let status = if d.new_file {
        FileStatus::Added
    } else if d.deleted_file {
        FileStatus::Deleted
    } else if d.renamed_file {
        FileStatus::Renamed
    } else {
        FileStatus::Modified
    };
    ChangedFile {
        old_filename: d.renamed_file.then(|| d.old_path.clone()),
        filename: d.new_path,
        status,
        additions: additions as u64,
        deletions: deletions as u64,
        patch: (!d.diff.is_empty()).then_some(d.diff),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_mr_and_nested_path() {
        let json = r#"{"id":10,"iid":3,"project_id":42,"title":"T","description":null,
            "author":{"username":"ann"},"source_branch":"f","target_branch":"main","state":"opened",
            "web_url":"https://git.acme.io/grp/sub/app/-/merge_requests/3","created_at":"c","updated_at":"u",
            "labels":["x"],"reviewers":[{"username":"bob"}],"user_notes_count":2,"draft":true,"sha":"abc"}"#;
        let mr = map_mr(serde_json::from_str(json).expect("parse"));
        assert_eq!(mr.repo_full_name, "grp/sub/app");
        assert_eq!(mr.display_number(), "!3");
        assert_eq!(mr.gitlab_project_id, Some(42));
        assert_eq!(project_ref(&mr), "42");
        assert!(mr.is_draft);
        assert_eq!(api("https://git.acme.io/", "/user"), "https://git.acme.io/api/v4/user");
    }

    #[test]
    fn maps_diff_entries() {
        let d = GlDiff {
            old_path: "a.rs".into(),
            new_path: "b.rs".into(),
            new_file: false,
            deleted_file: false,
            renamed_file: true,
            diff: "@@ -1 +1 @@\n-x\n+y\n".into(),
        };
        let f = map_diff(d);
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_filename.as_deref(), Some("a.rs"));
        assert_eq!((f.additions, f.deletions), (1, 1));
    }
}
