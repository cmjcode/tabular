//! Status working tree dari `git status --porcelain=v2 --branch -z`.

use std::path::Path;

use super::{GitError, cli};

/// Jenis perubahan satu file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// Huruf pendek ala VS Code untuk badge di sidebar.
    pub fn letter(self) -> &'static str {
        match self {
            ChangeKind::Added => "A",
            ChangeKind::Modified => "M",
            ChangeKind::Deleted => "D",
            ChangeKind::Renamed => "R",
            ChangeKind::Copied => "C",
            ChangeKind::TypeChanged => "T",
            ChangeKind::Untracked => "U",
            ChangeKind::Conflicted => "!",
        }
    }

    fn from_code(c: char) -> Option<Self> {
        Some(match c {
            'A' => ChangeKind::Added,
            'M' => ChangeKind::Modified,
            'D' => ChangeKind::Deleted,
            'R' => ChangeKind::Renamed,
            'C' => ChangeKind::Copied,
            'T' => ChangeKind::TypeChanged,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// Path lama untuk rename/copy.
    pub orig_path: Option<String>,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Nama branch aktif; `None` bila HEAD detached.
    pub branch: Option<String>,
    /// Commit HEAD; `None` bila repository belum punya commit.
    pub head_oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<FileChange>,
    pub conflicted: Vec<FileChange>,
}

impl RepoStatus {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty()
            && self.unstaged.is_empty()
            && self.untracked.is_empty()
            && self.conflicted.is_empty()
    }

    pub fn change_count(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.untracked.len() + self.conflicted.len()
    }

    /// Label branch untuk header: nama branch, atau hash pendek saat detached.
    pub fn head_label(&self) -> String {
        match (&self.branch, &self.head_oid) {
            (Some(b), _) => b.clone(),
            (None, Some(oid)) => format!("detached @ {}", &oid[..oid.len().min(7)]),
            (None, None) => "(no commits)".to_string(),
        }
    }
}

/// Baca status repository `repo`.
pub fn read(repo: &Path) -> Result<RepoStatus, GitError> {
    let out = cli::run(
        Some(repo),
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
        &cli::NEVER,
        cli::DEFAULT_TIMEOUT,
    )?;
    Ok(parse(&String::from_utf8_lossy(&out)))
}

/// Parse keluaran porcelain v2 berpemisah NUL.
pub fn parse(raw: &str) -> RepoStatus {
    let mut st = RepoStatus::default();
    let mut fields = raw.split('\0').peekable();
    while let Some(entry) = fields.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(header) = entry.strip_prefix("# ") {
            parse_header(&mut st, header);
            continue;
        }
        let mut chars = entry.chars();
        let tag = chars.next().unwrap_or(' ');
        match tag {
            '1' => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = entry.splitn(9, ' ').collect();
                if parts.len() == 9 {
                    push_xy(&mut st, parts[1], parts[8], None);
                }
            }
            '2' => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let parts: Vec<&str> = entry.splitn(10, ' ').collect();
                let orig = fields.next().map(str::to_string);
                if parts.len() == 10 {
                    push_xy(&mut st, parts[1], parts[9], orig);
                }
            }
            'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let parts: Vec<&str> = entry.splitn(11, ' ').collect();
                if parts.len() == 11 {
                    st.conflicted.push(FileChange {
                        path: parts[10].to_string(),
                        orig_path: None,
                        kind: ChangeKind::Conflicted,
                    });
                }
            }
            '?' => {
                if let Some(path) = entry.get(2..) {
                    st.untracked.push(FileChange {
                        path: path.to_string(),
                        orig_path: None,
                        kind: ChangeKind::Untracked,
                    });
                }
            }
            _ => {}
        }
    }
    st
}

fn parse_header(st: &mut RepoStatus, header: &str) {
    if let Some(oid) = header.strip_prefix("branch.oid ") {
        st.head_oid = (oid != "(initial)").then(|| oid.to_string());
    } else if let Some(head) = header.strip_prefix("branch.head ") {
        st.branch = (head != "(detached)").then(|| head.to_string());
    } else if let Some(up) = header.strip_prefix("branch.upstream ") {
        st.upstream = Some(up.to_string());
    } else if let Some(ab) = header.strip_prefix("branch.ab ") {
        for part in ab.split_whitespace() {
            if let Some(n) = part.strip_prefix('+') {
                st.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix('-') {
                st.behind = n.parse().unwrap_or(0);
            }
        }
    }
}

fn push_xy(st: &mut RepoStatus, xy: &str, path: &str, orig: Option<String>) {
    let mut c = xy.chars();
    let x = c.next().unwrap_or('.');
    let y = c.next().unwrap_or('.');
    if let Some(kind) = ChangeKind::from_code(x) {
        st.staged.push(FileChange {
            path: path.to_string(),
            orig_path: orig.clone().filter(|_| matches!(kind, ChangeKind::Renamed | ChangeKind::Copied)),
            kind,
        });
    }
    if let Some(kind) = ChangeKind::from_code(y) {
        st.unstaged.push(FileChange {
            path: path.to_string(),
            orig_path: orig.filter(|_| matches!(kind, ChangeKind::Renamed | ChangeKind::Copied)),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_entries() {
        let raw = "# branch.oid 1234567890abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 M. N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 .M N... 100644 100644 100644 aaa bbb dir with space/b.rs\0\
1 MM N... 100644 100644 100644 aaa bbb both.rs\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.rs\0\
? new file.txt\0";
        let st = parse(raw);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        let staged: Vec<&str> = st.staged.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(staged, ["src/a.rs", "both.rs", "new.rs"]);
        assert_eq!(st.staged[2].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(st.staged[2].kind, ChangeKind::Renamed);
        let unstaged: Vec<&str> = st.unstaged.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(unstaged, ["dir with space/b.rs", "both.rs"]);
        assert_eq!(st.conflicted[0].path, "conflict.rs");
        assert_eq!(st.untracked[0].path, "new file.txt");
        assert_eq!(st.change_count(), 7);
    }

    #[test]
    fn detached_and_initial() {
        let st = parse("# branch.oid (initial)\0# branch.head (detached)\0");
        assert!(st.branch.is_none());
        assert!(st.head_oid.is_none());
        assert!(st.is_clean());
        assert_eq!(st.head_label(), "(no commits)");
        let st = parse("# branch.oid abcdef1234\0# branch.head (detached)\0");
        assert_eq!(st.head_label(), "detached @ abcdef1");
    }
}
