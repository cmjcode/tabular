//! Diff unified dari git dan konversinya ke baris side-by-side (port
//! `parsePatch` dari ekstensi GitMerge Review).

use std::path::Path;

use super::{GitError, cli};

/// File untracked lebih besar dari ini tidak ditampilkan isinya.
pub const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Header hunk `@@ -a,b +c,d @@`.
    Hunk,
    /// Pasangan baris dihapus/ditambah (salah satu sisi boleh kosong).
    Change,
    Context,
}

/// Satu baris tampilan side-by-side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    /// Untuk [`RowKind::Hunk`]: teks header.
    pub hunk_header: String,
    pub left_num: Option<u32>,
    pub left: Option<String>,
    pub right_num: Option<u32>,
    pub right: Option<String>,
}

/// Ringkasan jumlah baris di patch.
pub fn count_lines(patch: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in patch.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    (added, removed)
}

/// Patch berisi penanda file biner, bukan baris teks.
pub fn is_binary_patch(patch: &str) -> bool {
    patch
        .lines()
        .take(8)
        .any(|l| l.starts_with("Binary files ") || l == "GIT binary patch")
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    // @@ -12,5 +12,7 @@ fn x
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let new = rest.strip_prefix('+')?.split(' ').next()?;
    let start = |s: &str| s.split(',').next().and_then(|n| n.parse::<u32>().ok());
    Some((start(old)?, start(new)?))
}

/// Ubah patch unified menjadi baris side-by-side. Header file (`diff --git`,
/// `index`, `---`, `+++`) dilewati; blok hapus/tambah dipasangkan berurutan.
pub fn parse_patch(patch: &str) -> Vec<DiffRow> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut out = Vec::new();
    let (mut old_num, mut new_num) = (0u32, 0u32);
    let mut in_hunk = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@") {
            if let Some((o, n)) = parse_hunk_header(line) {
                old_num = o.saturating_sub(1);
                new_num = n.saturating_sub(1);
            }
            in_hunk = true;
            out.push(DiffRow {
                kind: RowKind::Hunk,
                hunk_header: line.to_string(),
                left_num: None,
                left: None,
                right_num: None,
                right: None,
            });
            i += 1;
            continue;
        }
        if !in_hunk {
            i += 1;
            continue;
        }
        if line.starts_with("diff --git") {
            in_hunk = false;
            i += 1;
            continue;
        }
        let mut removes: Vec<(u32, &str)> = Vec::new();
        let mut adds: Vec<(u32, &str)> = Vec::new();
        while i < lines.len() && (lines[i].starts_with('-') || lines[i].starts_with('+')) {
            let l = lines[i];
            if let Some(t) = l.strip_prefix('-') {
                old_num += 1;
                removes.push((old_num, t));
            } else if let Some(t) = l.strip_prefix('+') {
                new_num += 1;
                adds.push((new_num, t));
            }
            i += 1;
        }
        if !removes.is_empty() || !adds.is_empty() {
            for j in 0..removes.len().max(adds.len()) {
                let rem = removes.get(j);
                let add = adds.get(j);
                out.push(DiffRow {
                    kind: RowKind::Change,
                    hunk_header: String::new(),
                    left_num: rem.map(|r| r.0),
                    left: rem.map(|r| r.1.to_string()),
                    right_num: add.map(|a| a.0),
                    right: add.map(|a| a.1.to_string()),
                });
            }
            continue;
        }
        if let Some(t) = line.strip_prefix(' ').or((line.is_empty()).then_some("")) {
            old_num += 1;
            new_num += 1;
            out.push(DiffRow {
                kind: RowKind::Context,
                hunk_header: String::new(),
                left_num: Some(old_num),
                left: Some(t.to_string()),
                right_num: Some(new_num),
                right: Some(t.to_string()),
            });
        }
        i += 1;
    }
    out
}

/// Diff working tree terhadap index untuk `path`.
pub fn worktree(repo: &Path, path: &str) -> Result<String, GitError> {
    cli::run_text(repo, &["diff", "--no-color", "--no-ext-diff", "-M", "--", path])
}

/// Diff index terhadap HEAD untuk `path` (termasuk path lama bila rename).
pub fn staged(repo: &Path, path: &str, orig: Option<&str>) -> Result<String, GitError> {
    let mut args = vec!["diff", "--cached", "--no-color", "--no-ext-diff", "-M", "--", path];
    if let Some(o) = orig {
        args.push(o);
    }
    cli::run_text(repo, &args)
}

/// Seluruh staged diff (untuk pesan commit buatan AI).
pub fn staged_all(repo: &Path) -> Result<String, GitError> {
    cli::run_text(repo, &["diff", "--cached", "--no-color", "--no-ext-diff", "-M"])
}

/// Diff satu file di commit `hash` terhadap parent pertama (atau commit akar).
pub fn commit_file(
    repo: &Path,
    hash: &str,
    parent: Option<&str>,
    path: &str,
    orig: Option<&str>,
) -> Result<String, GitError> {
    let mut args: Vec<&str> = match parent {
        Some(p) => vec!["diff", "--no-color", "--no-ext-diff", "-M", p, hash, "--", path],
        None => vec!["show", "--no-color", "--no-ext-diff", "--format=", hash, "--", path],
    };
    if let Some(o) = orig {
        args.push(o);
    }
    cli::run_text(repo, &args)
}

/// Patch "semua baris baru" untuk file untracked, dibuat tanpa git supaya
/// portabel (tidak butuh `/dev/null`).
pub fn untracked(repo: &Path, path: &str) -> Result<String, GitError> {
    let full = repo.join(path);
    let meta = std::fs::metadata(&full)?;
    if meta.is_dir() {
        return Ok(String::new());
    }
    if meta.len() > MAX_UNTRACKED_BYTES {
        return Ok(format!(
            "Binary files /dev/null and b/{path} differ (file larger than {} MB)\n",
            MAX_UNTRACKED_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(&full)?;
    if bytes.contains(&0) {
        return Ok(format!("Binary files /dev/null and b/{path} differ\n"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n", lines.len());
    for l in lines {
        out.push('+');
        out.push_str(l);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1,4 +1,5 @@\n a\n-b\n-c\n+B\n+C\n+D\n d\n@@ -10,2 +11,1 @@ fn z\n x\n-y\n";

    #[test]
    fn pairs_removed_and_added() {
        let rows = parse_patch(PATCH);
        assert_eq!(rows[0].kind, RowKind::Hunk);
        assert_eq!(rows[1].left_num, Some(1));
        assert_eq!(rows[1].right_num, Some(1));
        assert_eq!(rows[2].left.as_deref(), Some("b"));
        assert_eq!(rows[2].right.as_deref(), Some("B"));
        assert_eq!(rows[4].left, None);
        assert_eq!(rows[4].right_num, Some(4));
        assert_eq!(rows[5].left_num, Some(4));
        assert_eq!(rows[5].right_num, Some(5));
        assert_eq!(rows[6].kind, RowKind::Hunk);
        assert_eq!(rows[7].left_num, Some(10));
        assert_eq!(rows[7].right_num, Some(11));
        assert_eq!(rows[8].left.as_deref(), Some("y"));
        assert_eq!(rows[8].right, None);
        // Header `---`/`+++` di luar hunk tidak dihitung sebagai baris.
        assert_eq!(rows.len(), 9);
    }

    #[test]
    fn counts_and_binary() {
        assert_eq!(count_lines(PATCH), (3, 3));
        assert!(is_binary_patch("diff --git a/i.png b/i.png\nBinary files a/i.png and b/i.png differ\n"));
        assert!(!is_binary_patch(PATCH));
    }

    #[test]
    fn untracked_file_becomes_all_added() {
        let dir = std::env::temp_dir().join(format!("tabular-git-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("n.txt"), "one\ntwo\n").expect("write");
        let p = untracked(&dir, "n.txt").expect("patch");
        let rows = parse_patch(&p);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].right.as_deref(), Some("two"));
        assert_eq!(rows[2].right_num, Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
