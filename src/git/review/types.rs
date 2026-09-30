//! Model data Merge Review (port `models/types.ts`).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    GitHub,
    GitLab,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::GitLab => "GitLab",
        }
    }

    /// Prefix nomor: `#12` (GitHub) atau `!12` (GitLab).
    pub fn number_prefix(self) -> &'static str {
        match self {
            Provider::GitHub => "#",
            Provider::GitLab => "!",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    pub id: u64,
    /// Nomor PR (GitHub) atau iid MR (GitLab) yang dipakai di URL API.
    pub number: u64,
    pub title: String,
    pub description: String,
    pub author: String,
    pub source_branch: String,
    pub target_branch: String,
    pub state: MrState,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    pub provider: Provider,
    /// `owner/name`, GitLab bisa bersarang.
    pub repo_full_name: String,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    pub comment_count: u64,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files_count: u64,
    pub is_draft: bool,
    pub gitlab_project_id: Option<u64>,
    /// SHA HEAD sumber saat daftar diambil (dipakai GitHub merge).
    pub head_sha: Option<String>,
}

impl MergeRequest {
    /// Kunci unik lintas provider.
    pub fn key(&self) -> String {
        format!("{:?}:{}#{}", self.provider, self.repo_full_name, self.number)
    }

    pub fn display_number(&self) -> String {
        format!("{}{}", self.provider.number_prefix(), self.number)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

impl FileStatus {
    pub fn letter(self) -> &'static str {
        match self {
            FileStatus::Added => "A",
            FileStatus::Modified => "M",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Copied => "C",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FileStatus::Added => "added",
            FileStatus::Modified => "modified",
            FileStatus::Deleted => "deleted",
            FileStatus::Renamed => "renamed",
            FileStatus::Copied => "copied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub filename: String,
    pub old_filename: Option<String>,
    pub status: FileStatus,
    pub additions: u64,
    pub deletions: u64,
    /// Patch unified; `None` untuk file biner atau diff terlalu besar.
    pub patch: Option<String>,
}

/// Rekomendasi akhir review AI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recommendation {
    Approve,
    ApproveWithSuggestions,
    RequestChanges,
}

impl Recommendation {
    pub fn label(self) -> &'static str {
        match self {
            Recommendation::Approve => "APPROVE",
            Recommendation::ApproveWithSuggestions => "APPROVE WITH SUGGESTIONS",
            Recommendation::RequestChanges => "REQUEST CHANGES",
        }
    }
}

/// Cara merge di server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub const ALL: [MergeMethod; 3] = [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase];

    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Merge => "Merge commit",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
        }
    }
}
