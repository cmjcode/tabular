//! Tabel dan view favorit (bintang) per koneksi untuk sidebar.
//!
//! Disimpan di `<data dir>/favorites.json`. State di memori hanya berubah
//! setelah file berhasil ditulis; kegagalan dilaporkan lewat
//! [`crate::persist_notice`] sehingga UI tidak pernah menampilkan favorit yang
//! sebenarnya tidak tersimpan.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

const FILE_NAME: &str = "favorites.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FavoriteKind {
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorite {
    pub connection_id: i64,
    #[serde(default)]
    pub database: Option<String>,
    pub name: String,
    pub kind: FavoriteKind,
}

impl Favorite {
    fn same_object(&self, other: &Favorite) -> bool {
        self.connection_id == other.connection_id
            && self.database == other.database
            && self.name == other.name
    }
}

static FAVORITES: RwLock<Option<Vec<Favorite>>> = RwLock::new(None);

fn file_path() -> PathBuf {
    crate::config::get_data_dir().join(FILE_NAME)
}

fn read_file(path: &Path) -> Vec<Favorite> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            crate::persist_notice::report(
                "favorites_read",
                crate::persist_notice::Level::Warning,
                format!("Could not read favorites ({}): {e}", path.display()),
            );
            return Vec::new();
        }
    };
    match serde_json::from_str(&text) {
        Ok(list) => list,
        Err(e) => {
            let moved = crate::directory::quarantine_corrupt_file(path);
            crate::persist_notice::report(
                "favorites_parse",
                crate::persist_notice::Level::Warning,
                format!(
                    "Favorites file was unreadable ({e}) and was moved aside{}",
                    moved
                        .map(|p| format!(" to {}", p.display()))
                        .unwrap_or_default()
                ),
            );
            Vec::new()
        }
    }
}

/// Daftar favorit (dimuat dari disk saat pertama dipanggil).
pub fn all() -> Vec<Favorite> {
    if let Some(list) = FAVORITES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        return list.clone();
    }
    let loaded = read_file(&file_path());
    let mut guard = FAVORITES.write().unwrap_or_else(PoisonError::into_inner);
    guard.get_or_insert(loaded).clone()
}

pub fn is_favorite(connection_id: i64, database: Option<&str>, name: &str) -> bool {
    all().iter().any(|f| {
        f.connection_id == connection_id && f.database.as_deref() == database && f.name == name
    })
}

/// Tambah atau hapus favorit. `Ok(true)` bila kini menjadi favorit.
pub fn toggle(fav: Favorite) -> Result<bool, String> {
    toggle_at(&file_path(), fav)
}

fn toggle_at(path: &Path, fav: Favorite) -> Result<bool, String> {
    let mut list = all();
    let added = if let Some(pos) = list.iter().position(|f| f.same_object(&fav)) {
        list.remove(pos);
        false
    } else {
        list.push(fav);
        true
    };
    save_at(path, &list)?;
    Ok(added)
}

/// Hapus favorit milik koneksi yang sudah dihapus.
pub fn forget_connection(connection_id: i64) -> Result<(), String> {
    let mut list = all();
    let before = list.len();
    list.retain(|f| f.connection_id != connection_id);
    if list.len() == before {
        return Ok(());
    }
    save_at(&file_path(), &list)
}

fn save_at(path: &Path, list: &[Favorite]) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    crate::directory::write_file_atomically(path, &json)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    *FAVORITES.write().unwrap_or_else(PoisonError::into_inner) = Some(list.to_vec());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(name: &str) -> Favorite {
        Favorite {
            connection_id: 1,
            database: Some("shop".into()),
            name: name.into(),
            kind: FavoriteKind::Table,
        }
    }

    // Satu tes saja: state global `FAVORITES` dipakai bersama, tes paralel
    // akan saling menimpa.
    #[test]
    fn toggle_persists_and_failed_write_keeps_memory_unchanged() {
        let dir = std::env::temp_dir().join(format!("tabular_fav_{}", std::process::id()));
        let path = dir.join(FILE_NAME);
        *FAVORITES.write().unwrap() = Some(Vec::new());

        assert!(toggle_at(&path, fav("orders")).unwrap());
        assert!(is_favorite(1, Some("shop"), "orders"));
        assert_eq!(read_file(&path), vec![fav("orders")]);
        assert!(!toggle_at(&path, fav("orders")).unwrap());
        assert!(!is_favorite(1, Some("shop"), "orders"));
        assert!(read_file(&path).is_empty());
        std::fs::remove_dir_all(&dir).ok();

        // Induk berupa file biasa: create_dir_all gagal, memori tidak berubah.
        let blocker =
            std::env::temp_dir().join(format!("tabular_fav_block_{}", std::process::id()));
        std::fs::write(&blocker, b"x").unwrap();
        assert!(toggle_at(&blocker.join(FILE_NAME), fav("orders")).is_err());
        assert!(!is_favorite(1, Some("shop"), "orders"));
        std::fs::remove_file(&blocker).ok();
    }
}
