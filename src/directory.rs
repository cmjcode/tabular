use crate::models;

pub(crate) fn get_app_data_dir() -> std::path::PathBuf {
    // Use the configurable data directory from config.rs
    crate::config::get_data_dir()
}

pub(crate) fn get_data_dir() -> std::path::PathBuf {
    get_app_data_dir().join("data")
}

/// Tulis file secara atomik: tulis ke file sementara di folder yang sama lalu
/// rename. Crash atau disk penuh di tengah penulisan tidak akan meninggalkan
/// file setengah jadi yang membuat data user hilang saat dibaca ulang.
pub(crate) fn write_file_atomically(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp = path.with_file_name(format!(".{}.tmp", file_name));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
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
            unsafe {
                std::env::set_var("TABULAR_DATA_DIR", &default_dir);
            }
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
