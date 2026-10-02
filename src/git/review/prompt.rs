//! Prompt review AI (port `buildPrompts`/`buildDiffContent` dari
//! `aiService.ts`) dan prompt pesan commit. Diff disaring dengan
//! [`crate::repo_endpoints::redact_code_secrets`] sebelum masuk prompt.

use super::types::{ChangedFile, MergeRequest, Recommendation};

/// Batas total isi diff di prompt, supaya muat di context window.
pub const MAX_DIFF_CHARS: usize = 40_000;
/// Batas staged diff untuk pesan commit.
pub const MAX_COMMIT_DIFF_CHARS: usize = 24_000;

/// Bahasa jawaban review yang bisa dipilih di Preferences.
pub const LANGUAGES: [&str; 10] = [
    "English",
    "Indonesian",
    "Spanish",
    "French",
    "German",
    "Japanese",
    "Korean",
    "Chinese (Simplified)",
    "Portuguese",
    "Arabic",
];

pub fn review_system_prompt(language: &str) -> String {
    format!(
        "You are a senior software engineer performing a code review.
You MUST write your entire response in {language}. All section headings, descriptions, and feedback must be in {language}.
Analyze the provided code diff and give a structured review with these sections:

## Summary
A brief description of what this merge request does.

## Issues Found
List any bugs, logic errors, or problems. Use bullet points. Say \"None found\" if clean.

## Code Quality
Comment on readability, naming, complexity, and adherence to best practices.

## Security Concerns
Identify any security vulnerabilities (injection, auth issues, data exposure, etc.). Say \"None found\" if clean.

## Recommendation
End with exactly one of these markers (keep the marker text in English):
✅ **APPROVE** — changes look good
⚠️ **APPROVE WITH SUGGESTIONS** — minor issues but can be merged
❌ **REQUEST CHANGES** — significant issues that should be fixed first

Be concise, specific, and constructive. Do not use tools; the full diff is in the message."
    )
}

pub fn review_user_prompt(mr: &MergeRequest, files: &[ChangedFile]) -> String {
    let description = if mr.description.trim().is_empty() {
        "No description provided".to_string()
    } else {
        crate::repo_endpoints::redact_code_secrets(mr.description.trim())
    };
    format!(
        "Please review the following merge request:

**Title:** {title}
**Repository:** {repo}
**Branch:** `{src}` → `{dst}`
**Author:** {author}
**Description:** {description}
**Stats:** +{add} additions, -{del} deletions across {n} files

---

{diff}",
        title = mr.title,
        repo = mr.repo_full_name,
        src = mr.source_branch,
        dst = mr.target_branch,
        author = mr.author,
        add = mr.additions,
        del = mr.deletions,
        n = files.len(),
        diff = diff_content(files, MAX_DIFF_CHARS),
    )
}

/// Gabungan diff per file, berhenti saat melewati `max_chars`.
pub fn diff_content(files: &[ChangedFile], max_chars: usize) -> String {
    let mut total = 0;
    let mut out = String::new();
    for (i, file) in files.iter().enumerate() {
        let header = format!(
            "### {} ({}, +{} -{})",
            file.filename,
            file.status.label(),
            file.additions,
            file.deletions
        );
        let body = match &file.patch {
            Some(p) => format!(
                "```diff\n{}\n```",
                crate::repo_endpoints::redact_code_secrets(p)
            ),
            None => "_Binary file or no diff available_".to_string(),
        };
        let piece = format!("{header}\n{body}\n\n");
        if total + piece.len() > max_chars {
            let rest = files.len() - i;
            out.push_str(&format!(
                "### {} _(diff truncated — {rest} more file(s) not shown)_\n",
                file.filename
            ));
            break;
        }
        total += piece.len();
        out.push_str(&piece);
    }
    out
}

/// Ambil rekomendasi dari jawaban review. Penanda terakhir yang muncul
/// menang, karena bagian Recommendation ada di akhir.
pub fn parse_recommendation(reply: &str) -> Option<Recommendation> {
    let upper = reply.to_uppercase();
    let candidates = [
        ("REQUEST CHANGES", Recommendation::RequestChanges),
        (
            "APPROVE WITH SUGGESTIONS",
            Recommendation::ApproveWithSuggestions,
        ),
        ("APPROVE", Recommendation::Approve),
    ];
    let mut best: Option<(usize, Recommendation)> = None;
    for (marker, rec) in candidates {
        if let Some(pos) = upper.rfind(marker) {
            // "APPROVE" juga cocok di dalam "APPROVE WITH SUGGESTIONS"; posisi
            // sama → penanda yang lebih panjang (dicek lebih dulu) tetap menang.
            if best.is_none_or(|(p, _)| pos > p) {
                best = Some((pos, rec));
            }
        }
    }
    best.map(|(_, r)| r)
}

pub fn commit_message_system_prompt() -> String {
    "You write git commit messages. Reply with the commit message only: a subject line in \
     imperative mood of at most 72 characters, optionally followed by a blank line and a short \
     body with bullet points. Follow the Conventional Commits style (feat:, fix:, refactor:, …) \
     when it fits. No code fences, no preamble, no tools."
        .to_string()
}

pub fn commit_message_user_prompt(staged_diff: &str, branch: Option<&str>) -> String {
    let mut diff = crate::repo_endpoints::redact_code_secrets(staged_diff);
    if diff.len() > MAX_COMMIT_DIFF_CHARS {
        let mut end = MAX_COMMIT_DIFF_CHARS;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
        diff.push_str("\n… (diff truncated)");
    }
    format!(
        "Branch: {}\n\nWrite a commit message for this staged diff:\n\n```diff\n{diff}\n```",
        branch.unwrap_or("(detached)")
    )
}

/// Bersihkan jawaban model: buang pagar kode dan baris kosong di tepi.
pub fn clean_commit_message(reply: &str) -> String {
    let t = reply.trim();
    let t = t
        .strip_prefix("```")
        .map(|r| r.split_once('\n').map_or("", |(_, rest)| rest))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::review::types::{FileStatus, MrState, Provider};

    fn mr() -> MergeRequest {
        MergeRequest {
            id: 1,
            number: 2,
            title: "Add x".into(),
            description: String::new(),
            author: "ann".into(),
            source_branch: "feat".into(),
            target_branch: "main".into(),
            state: MrState::Open,
            url: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            provider: Provider::GitHub,
            repo_full_name: "org/app".into(),
            labels: vec![],
            reviewers: vec![],
            comment_count: 0,
            additions: 1,
            deletions: 0,
            changed_files_count: 1,
            is_draft: false,
            gitlab_project_id: None,
            head_sha: None,
        }
    }

    fn file(name: &str, patch_len: usize) -> ChangedFile {
        ChangedFile {
            filename: name.into(),
            old_filename: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            patch: Some(format!("+{}", "x".repeat(patch_len))),
        }
    }

    #[test]
    fn diff_is_truncated_at_budget() {
        let files = vec![file("a.rs", 100), file("b.rs", 100), file("c.rs", 100)];
        let full = diff_content(&files, MAX_DIFF_CHARS);
        assert!(full.contains("### c.rs (modified"));
        let cut = diff_content(&files, 200);
        assert!(cut.contains("### a.rs (modified"));
        assert!(cut.contains("b.rs _(diff truncated — 2 more"));
        assert!(!cut.contains("c.rs"));
    }

    #[test]
    fn prompts_contain_metadata_and_language() {
        let p = review_user_prompt(&mr(), &[file("a.rs", 5)]);
        assert!(p.contains("`feat` → `main`"));
        assert!(p.contains("No description provided"));
        assert!(review_system_prompt("Indonesian").contains("in Indonesian"));
    }

    #[test]
    fn recommendation_markers() {
        assert_eq!(
            parse_recommendation("## Recommendation\n❌ **REQUEST CHANGES** — fix"),
            Some(Recommendation::RequestChanges)
        );
        assert_eq!(
            parse_recommendation("I would approve.\n⚠️ **APPROVE WITH SUGGESTIONS**"),
            Some(Recommendation::ApproveWithSuggestions)
        );
        assert_eq!(
            parse_recommendation("✅ **APPROVE**"),
            Some(Recommendation::Approve)
        );
        assert_eq!(parse_recommendation("no verdict"), None);
    }

    #[test]
    fn commit_message_cleanup() {
        assert_eq!(clean_commit_message("```\nfeat: x\n```"), "feat: x");
        assert_eq!(clean_commit_message("  fix: y \n"), "fix: y");
        let long = "a".repeat(MAX_COMMIT_DIFF_CHARS + 10);
        assert!(commit_message_user_prompt(&long, Some("main")).contains("diff truncated"));
    }
}
