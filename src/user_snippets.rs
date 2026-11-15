//! Snippet SQL buatan pengguna untuk autocomplete.
//!
//! Disimpan di `<data dir>/snippets.json` sebagai array
//! `[{"prefix": "...", "body": "...", "description": "..."}]`. `body` boleh
//! memuat `$0` untuk posisi kursor setelah snippet disisipkan.
//!
//! File dibaca dari cache dan hanya di-`stat` paling sering sekali per
//! [`RECHECK`], jadi autocomplete per ketukan tidak membaca disk setiap kali.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

const RECHECK: Duration = Duration::from_secs(2);
const FILE_NAME: &str = "snippets.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSnippet {
    pub prefix: String,
    pub body: String,
    #[serde(default)]
    pub description: String,
}

struct Cache {
    path: PathBuf,
    checked_at: Option<Instant>,
    modified: Option<SystemTime>,
    snippets: Vec<UserSnippet>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

pub fn file_path() -> PathBuf {
    crate::config::get_data_dir().join(FILE_NAME)
}

/// Parse isi file. Entri tanpa prefix atau body ditolak supaya kesalahan
/// ketik tidak diam-diam hilang.
pub fn parse(text: &str) -> Result<Vec<UserSnippet>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let list: Vec<UserSnippet> =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    for (i, s) in list.iter().enumerate() {
        if s.prefix.trim().is_empty() || s.body.is_empty() {
            return Err(format!("entry {}: prefix and body are required", i + 1));
        }
    }
    Ok(list)
}

fn load(path: &Path) -> Vec<UserSnippet> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            crate::persist_notice::report(
                "user_snippets_read",
                crate::persist_notice::Level::Warning,
                format!("Could not read {}: {e}", path.display()),
            );
            return Vec::new();
        }
    };
    match parse(&text) {
        Ok(list) => list,
        Err(e) => {
            crate::persist_notice::report(
                &format!("user_snippets_parse:{e}"),
                crate::persist_notice::Level::Warning,
                format!("SQL snippets in {} were not loaded: {e}", path.display()),
            );
            Vec::new()
        }
    }
}

/// Snippet pengguna saat ini (dari cache, dimuat ulang bila file berubah).
pub fn current() -> Vec<UserSnippet> {
    current_at(&file_path())
}

fn current_at(path: &Path) -> Vec<UserSnippet> {
    let mut guard = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let cache = guard.get_or_insert_with(|| Cache {
        path: path.to_path_buf(),
        checked_at: None,
        modified: None,
        snippets: Vec::new(),
    });
    if cache.path != path {
        cache.path = path.to_path_buf();
        cache.checked_at = None;
        cache.modified = None;
    }
    let due = cache.checked_at.is_none_or(|t| t.elapsed() >= RECHECK);
    if due {
        cache.checked_at = Some(Instant::now());
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if modified != cache.modified {
            cache.modified = modified;
            cache.snippets = load(path);
        }
    }
    cache.snippets.clone()
}

/// Snippet yang prefix-nya diawali `typed` (tidak peka huruf besar).
pub fn matching(snippets: &[UserSnippet], typed: &str) -> Vec<UserSnippet> {
    let typed = typed.trim().to_lowercase();
    if typed.is_empty() {
        return Vec::new();
    }
    snippets
        .iter()
        .filter(|s| s.prefix.to_lowercase().starts_with(&typed))
        .cloned()
        .collect()
}

/// Teks sisipan: `$0` menjadi penanda kursor autocomplete.
pub fn insert_text(body: &str) -> String {
    body.replacen("$0", &crate::autocomplete::CURSOR_MARK.to_string(), 1)
}

const EXAMPLE: &str = r#"[
  {
    "prefix": "cnt",
    "body": "SELECT COUNT(*) FROM $0;",
    "description": "Count rows"
  },
  {
    "prefix": "dup",
    "body": "SELECT col, COUNT(*)\nFROM $0\nGROUP BY col\nHAVING COUNT(*) > 1;",
    "description": "Find duplicate values"
  }
]
"#;

/// Pastikan file ada (berisi contoh) lalu kembalikan path-nya.
pub fn ensure_file() -> Result<PathBuf, String> {
    let path = file_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, EXAMPLE).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_file_parses() {
        let list = parse(EXAMPLE).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].prefix, "cnt");
    }

    #[test]
    fn rejects_entries_without_prefix_or_body() {
        assert!(parse(r#"[{"prefix":"","body":"x"}]"#).is_err());
        assert!(parse(r#"[{"prefix":"a","body":""}]"#).is_err());
        assert!(parse("not json").is_err());
        assert!(parse("  ").unwrap().is_empty());
    }

    #[test]
    fn matching_is_prefix_and_case_insensitive() {
        let list = parse(EXAMPLE).unwrap();
        assert_eq!(matching(&list, "CN").len(), 1);
        assert!(matching(&list, "x").is_empty());
        assert!(matching(&list, "").is_empty());
    }

    #[test]
    fn cursor_marker_replaces_first_dollar_zero() {
        let s = insert_text("SELECT $0 FROM t -- $0");
        assert_eq!(
            s,
            format!("SELECT {} FROM t -- $0", crate::autocomplete::CURSOR_MARK)
        );
    }

    #[test]
    fn cache_reloads_after_file_change() {
        let dir = std::env::temp_dir().join(format!("tabular_snip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, EXAMPLE).unwrap();
        assert_eq!(current_at(&path).len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
