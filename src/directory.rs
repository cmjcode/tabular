use crate::models;

pub(crate) fn get_app_data_dir() -> std::path::PathBuf {
    let home_dir = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home_dir.join(".tabular")
}

pub(crate) fn get_data_dir() -> std::path::PathBuf {
    get_app_data_dir().join("data")
}

pub(crate) fn get_query_dir() -> std::path::PathBuf {
    get_app_data_dir().join("query")
}

pub(crate) fn ensure_app_directories() -> Result<(), std::io::Error> {
    let app_dir = get_app_data_dir();
    let data_dir = get_data_dir();
    let query_dir = get_query_dir();

    // Create directories if they don't exist
    std::fs::create_dir_all(&app_dir)?;
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&query_dir)?;

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
