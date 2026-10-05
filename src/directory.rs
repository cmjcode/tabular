use crate::models;

pub(crate) fn get_app_data_dir() -> std::path::PathBuf {
    // Use the configurable data directory from config.rs
    crate::config::get_data_dir()
}

pub(crate) fn get_data_dir() -> std::path::PathBuf {
    get_app_data_dir().join("data")
}

/// Penghitung untuk nama file sementara, supaya dua penulisan bersamaan dalam
/// satu proses tidak memakai nama yang sama.
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Nama file sementara yang unik (pid + nanodetik + penghitung) di folder yang
/// sama dengan `path`, supaya GUI dan `tabular mcp` tidak saling menimpa file
/// sementara masing-masing.
fn unique_tmp_path(path: &std::path::Path) -> std::path::PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let seq = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_file_name(format!(
        ".{}.{}.{}.{}.tmp",
        file_name,
        std::process::id(),
        nanos,
        seq
    ))
}

/// Tulis file secara atomik: tulis ke file sementara di folder yang sama lalu
/// rename. Crash atau disk penuh di tengah penulisan tidak akan meninggalkan
/// file setengah jadi yang membuat data user hilang saat dibaca ulang.
pub(crate) fn write_file_atomically(
    path: &std::path::Path,
    contents: &[u8],
) -> std::io::Result<()> {
    write_file_atomically_with_mode(path, contents, None)
}

/// Seperti [`write_file_atomically`], tetapi di Unix file dibuat langsung
/// dengan `mode` (mis. `0o600` untuk berkas rahasia) sehingga tidak pernah ada
/// jendela waktu di mana isinya terbaca user lain. `mode` diabaikan di
/// platform non-Unix.
pub(crate) fn write_file_atomically_with_mode(
    path: &std::path::Path,
    contents: &[u8],
    mode: Option<u32>,
) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = unique_tmp_path(path);

    let write_tmp = || -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = options.open(&tmp)?;
        file.write_all(contents)?;
        // Pastikan isi sudah di disk sebelum rename; tanpa ini mati listrik
        // bisa meninggalkan file tujuan berukuran nol.
        file.sync_all()?;
        Ok(())
    };

    if let Err(e) = write_tmp().and_then(|_| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Usaha terbaik: fsync folder induk supaya rename-nya sendiri ikut awet.
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let dir = if dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            dir
        };
        if let Ok(handle) = std::fs::File::open(dir) {
            let _ = handle.sync_all();
        }
    }
    Ok(())
}

/// Pindahkan file yang isinya tidak bisa dibaca ke `<nama>.corrupt-<timestamp>`
/// supaya penyimpanan berikutnya tidak menimpa (dan menghilangkan) data user.
/// Mengembalikan lokasi cadangan bila berhasil dipindahkan.
pub(crate) fn quarantine_corrupt_file(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
    let mut backup = path.with_file_name(format!("{}.corrupt-{}", file_name, stamp));
    let mut attempt = 1;
    while backup.exists() {
        backup = path.with_file_name(format!("{}.corrupt-{}-{}", file_name, stamp, attempt));
        attempt += 1;
    }
    match std::fs::rename(path, &backup) {
        Ok(()) => {
            log::error!(
                "[STORAGE] {} is unreadable; preserved as {}",
                path.display(),
                backup.display()
            );
            Some(backup)
        }
        Err(e) => {
            log::error!(
                "[STORAGE] {} is unreadable and could not be preserved: {}",
                path.display(),
                e
            );
            None
        }
    }
}

pub(crate) fn get_query_dir() -> std::path::PathBuf {
    get_app_data_dir().join("query")
}

pub(crate) fn ensure_app_directories() -> Result<(), std::io::Error> {
    let app_dir = get_app_data_dir();
    let data_dir = get_data_dir();
    let query_dir = get_query_dir();

    // Create directories if they don't exist
    let create_result = std::fs::create_dir_all(&app_dir)
        .and_then(|_| std::fs::create_dir_all(&data_dir))
        .and_then(|_| std::fs::create_dir_all(&query_dir));

    if let Err(e) = create_result {
        let default_dir = crate::config::get_local_data_dir();
        if app_dir != default_dir {
            log::warn!(
                "Configured app data directory {:?} is not writable ({}). Falling back to local default {:?}",
                app_dir,
                e,
                default_dir
            );
            // Simpan di static, bukan `set_var`: mengubah environment saat
            // thread lain berjalan adalah UB di Unix.
            crate::config::override_data_dir(default_dir.clone());
            let fallback_data_dir = default_dir.join("data");
            let fallback_query_dir = default_dir.join("query");
            std::fs::create_dir_all(&default_dir)?;
            std::fs::create_dir_all(&fallback_data_dir)?;
            std::fs::create_dir_all(&fallback_query_dir)?;
            return Ok(());
        }
        return Err(e);
    }

    Ok(())
}

pub(crate) fn load_directory_recursive(
    dir_path: &std::path::Path,
) -> Vec<models::structs::TreeNode> {
    let mut items = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_dir() {
                    // This is a folder
                    if let Some(folder_name) = entry.file_name().to_str() {
                        let folder_path = entry.path();

                        // Recursively load the folder contents
                        let folder_contents = load_directory_recursive(&folder_path);

                        let mut folder_node = models::structs::TreeNode::new(
                            folder_name.to_string(),
                            models::enums::NodeType::QueryFolder,
                        );
                        folder_node.children = folder_contents;
                        folder_node.is_expanded = true;
                        folder_node.file_path = Some(folder_path.to_string_lossy().to_string());
                        items.push(folder_node);
                    }
                } else if metadata.is_file() {
                    // This is a file
                    if let Some(file_name) = entry.file_name().to_str()
                        && file_name.ends_with(".sql")
                    {
                        let mut node = models::structs::TreeNode::new(
                            file_name.to_string(),
                            models::enums::NodeType::Query,
                        );
                        node.file_path = Some(entry.path().to_string_lossy().to_string());
                        items.push(node);
                    }
                }
            }
        }
    }

    // Sort the items: folders first, then files, all alphabetically
    items.sort_by(|a, b| {
        match (&a.node_type, &b.node_type) {
            (models::enums::NodeType::QueryFolder, models::enums::NodeType::Query) => {
                std::cmp::Ordering::Less
            } // Folders first
            (models::enums::NodeType::Query, models::enums::NodeType::QueryFolder) => {
                std::cmp::Ordering::Greater
            } // Files after folders
            _ => a.name.cmp(&b.name), // Alphabetical within same type
        }
    });

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tabular-directory-{}-{}-{}",
            tag,
            std::process::id(),
            TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn atomic_write_roundtrip_leaves_no_temp_file() {
        let dir = tmp_dir("atomic");
        let file = dir.join("data.json");
        write_file_atomically(&file, b"one").expect("first write");
        write_file_atomically(&file, b"two").expect("second write");
        assert_eq!(std::fs::read(&file).expect("read"), b"two");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["data.json".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_names_are_unique() {
        let path = std::path::Path::new("/tmp/x.json");
        assert_ne!(unique_tmp_path(path), unique_tmp_path(path));
    }

    #[test]
    fn failed_write_cleans_up_and_keeps_old_content() {
        let dir = tmp_dir("fail");
        // Tujuan berupa folder berisi: rename pasti gagal.
        let target = dir.join("target");
        std::fs::create_dir_all(target.join("child")).expect("mkdir");
        assert!(write_file_atomically(&target, b"x").is_err());
        let leftovers = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn mode_is_applied_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("mode");
        let file = dir.join("secret");
        write_file_atomically_with_mode(&file, b"k", Some(0o600)).expect("write");
        let mode = std::fs::metadata(&file).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_moves_file_aside() {
        let dir = tmp_dir("quarantine");
        let file = dir.join("prefs.json");
        std::fs::write(&file, "{broken").expect("write");
        let backup = quarantine_corrupt_file(&file).expect("backup path");
        assert!(!file.exists());
        assert_eq!(std::fs::read_to_string(&backup).expect("read"), "{broken");
        assert!(
            backup
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("prefs.json.corrupt-"))
                .unwrap_or(false)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
