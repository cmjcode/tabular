//! Folder project lokal per group diagram. Bersifat **personal**: disimpan di
//! `{data_dir}/diagram_repo_paths.json` milik user OS ini, tidak pernah ikut
//! ke file diagram, tabel `diagram_by_tabular`, vault, atau sync. Path lokal
//! user lain hampir pasti tidak ada di komputer ini, sedangkan URL git
//! (`DiagramGroup::repo_url`) berlaku untuk semua user dan tetap ikut diagram.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Nama file di `{data_dir}`; dibaca juga oleh lapisan agent (proses MCP).
pub const FILE_NAME: &str = "diagram_repo_paths.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileBody {
    #[serde(default)]
    paths: BTreeMap<String, String>,
}

/// Peta id group → folder project, dibaca sekali dan ditulis ulang tiap ubah.
#[derive(Debug)]
pub struct RepoPathStore {
    file: PathBuf,
    paths: BTreeMap<String, String>,
}

impl RepoPathStore {
    /// Muat dari `file`; file yang belum ada atau rusak dianggap kosong.
    pub fn load(file: PathBuf) -> Self {
        let paths = match std::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice::<FileBody>(&bytes) {
                Ok(body) => body.paths,
                Err(e) => {
                    log::warn!("[DIAGRAM] ignoring unreadable {}: {e}", file.display());
                    BTreeMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                log::warn!("[DIAGRAM] cannot read {}: {e}", file.display());
                BTreeMap::new()
            }
        };
        Self { file, paths }
    }

    pub fn get(&self, group_id: &str) -> Option<&str> {
        self.paths.get(group_id).map(String::as_str)
    }

    /// Set (`Some`) atau hapus (`None`/kosong) folder group, lalu simpan.
    pub fn set(&mut self, group_id: &str, path: Option<&str>) -> std::io::Result<()> {
        let path = path.map(str::trim).filter(|p| !p.is_empty());
        let changed = match path {
            Some(p) => {
                self.paths
                    .insert(group_id.to_string(), p.to_string())
                    .as_deref()
                    != Some(p)
            }
            None => self.paths.remove(group_id).is_some(),
        };
        if !changed {
            return Ok(());
        }
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = FileBody {
            paths: self.paths.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?;
        crate::diagram_view::write_atomic(&self.file, &bytes)
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

fn store() -> &'static Mutex<RepoPathStore> {
    static STORE: OnceLock<Mutex<RepoPathStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(RepoPathStore::load(
            crate::config::get_data_dir().join(FILE_NAME),
        ))
    })
}

/// Folder project personal untuk group `group_id` di komputer ini.
pub fn local_repo_path(group_id: &str) -> Option<String> {
    store()
        .lock()
        .ok()
        .and_then(|s| s.get(group_id).map(str::to_string))
}

/// Simpan (atau hapus bila `None`) folder project personal group `group_id`.
pub fn set_local_repo_path(group_id: &str, path: Option<&str>) -> Result<(), String> {
    let mut guard = store()
        .lock()
        .map_err(|_| "repository path store is unavailable".to_string())?;
    guard.set(group_id, path).map_err(|e| {
        log::warn!("[DIAGRAM] cannot save {}: {e}", guard.file().display());
        format!("Cannot save the project folder: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "tabular-repo-paths-{tag}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn set_get_remove_persist_across_loads() {
        let file = temp_file("persist");
        let mut s = RepoPathStore::load(file.clone());
        assert_eq!(s.get("g1"), None);
        s.set("g1", Some("  /work/app ")).expect("save");
        s.set("g2", Some("/work/other")).expect("save");

        let mut again = RepoPathStore::load(file.clone());
        assert_eq!(again.get("g1"), Some("/work/app"));
        assert_eq!(again.get("g2"), Some("/work/other"));

        again.set("g1", None).expect("remove");
        again.set("g2", Some("")).expect("empty removes");
        let last = RepoPathStore::load(file.clone());
        assert_eq!(last.get("g1"), None);
        assert_eq!(last.get("g2"), None);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn corrupt_or_missing_file_loads_empty() {
        let file = temp_file("corrupt");
        assert_eq!(RepoPathStore::load(file.clone()).get("g"), None);
        std::fs::write(&file, b"{not json").expect("write");
        let mut s = RepoPathStore::load(file.clone());
        assert_eq!(s.get("g"), None);
        // Tulis ulang menghasilkan file yang valid.
        s.set("g", Some("/x")).expect("save");
        assert_eq!(RepoPathStore::load(file.clone()).get("g"), Some("/x"));
        let _ = std::fs::remove_file(file);
    }
}
