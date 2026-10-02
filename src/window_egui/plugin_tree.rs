//! Pohon sidebar generik untuk koneksi engine plugin (ADR 0002).
//!
//! Engine dengan konsep database: `Databases` -> database -> Tables/Views.
//! Engine tanpa database: langsung Tables/Views memakai satu nama database
//! yang sama dengan yang ditulis `driver_api::cache` ke cache metadata.

use crate::driver_api::{self, TableKind, run_blocking};
use crate::models::enums::{DatabasePool, NodeType};
use crate::models::structs::{ConnectionConfig, TreeNode};
use crate::window_egui::Tabular;
use crate::{cache_data, connection};

fn node(name: &str, kind: NodeType, connection_id: i64, database: Option<&str>) -> TreeNode {
    let mut n = TreeNode::new(name.to_string(), kind);
    n.connection_id = Some(connection_id);
    n.database_name = database.map(str::to_string);
    n.is_loaded = false;
    n
}

/// Nama database tunggal untuk engine tanpa daftar database.
pub(crate) fn single_database(connection: &ConnectionConfig) -> String {
    if connection.database.is_empty() {
        "default".to_string()
    } else {
        connection.database.clone()
    }
}

fn object_folders(connection_id: i64, database: &str) -> Vec<TreeNode> {
    vec![
        node(
            "Tables",
            NodeType::TablesFolder,
            connection_id,
            Some(database),
        ),
        node(
            "Views",
            NodeType::ViewsFolder,
            connection_id,
            Some(database),
        ),
    ]
}

/// Kerangka awal selagi metadata dimuat di latar belakang.
pub(crate) fn load_structure(
    connection_id: i64,
    connection: &ConnectionConfig,
    root: &mut TreeNode,
) {
    let engine_id = connection.connection_type.plugin_id().unwrap_or_default();
    let caps = driver_api::query::capabilities(engine_id);
    root.children = if caps.databases {
        vec![node(
            "Databases",
            NodeType::DatabasesFolder,
            connection_id,
            None,
        )]
    } else {
        object_folders(connection_id, &single_database(connection))
    };
}

/// Anak node koneksi dari daftar database yang sudah ada di cache.
pub(crate) fn structure_from_databases(
    connection_id: i64,
    connection: &ConnectionConfig,
    databases: &[String],
) -> Vec<TreeNode> {
    let engine_id = connection.connection_type.plugin_id().unwrap_or_default();
    if !driver_api::query::capabilities(engine_id).databases {
        return object_folders(connection_id, &single_database(connection));
    }
    let mut folder = node("Databases", NodeType::DatabasesFolder, connection_id, None);
    folder.is_loaded = true;
    folder.children = databases
        .iter()
        .map(|db| {
            let mut db_node = node(db, NodeType::Database, connection_id, Some(db));
            db_node.children = object_folders(connection_id, db);
            db_node
        })
        .collect();
    vec![folder]
}

fn table_nodes(
    connection_id: i64,
    database: &str,
    names: Vec<String>,
    kind: NodeType,
) -> Vec<TreeNode> {
    let mut nodes: Vec<TreeNode> = names
        .into_iter()
        .map(|name| node(&name, kind.clone(), connection_id, Some(database)))
        .collect();
    nodes.sort_by_key(|n| n.name.to_lowercase());
    nodes
}

/// Isi folder Tables/Views: dari cache dulu, lalu live lewat sesi plugin.
pub(crate) fn load_folder_content(
    tabular: &mut Tabular,
    connection_id: i64,
    connection: &ConnectionConfig,
    folder: &mut TreeNode,
    folder_type: &NodeType,
    force_live_fetch: bool,
) {
    let (table_type, child_kind) = match folder_type {
        NodeType::TablesFolder => ("table", NodeType::Table),
        NodeType::ViewsFolder => ("view", NodeType::View),
        _ => {
            folder.children.clear();
            return;
        }
    };
    let database = folder
        .database_name
        .clone()
        .unwrap_or_else(|| single_database(connection));

    if !force_live_fetch
        && let Some(cached) =
            cache_data::get_tables_from_cache(tabular, connection_id, &database, table_type)
        && !cached.is_empty()
    {
        folder.children = table_nodes(connection_id, &database, cached, child_kind);
        return;
    }

    let rt = tabular.get_runtime();
    let pool = rt.block_on(connection::pool_if_connected_or_start(
        tabular,
        connection_id,
    ));
    let Some(DatabasePool::Plugin(pool)) = pool else {
        folder.children = vec![TreeNode::new("Connecting…".to_string(), NodeType::Column)];
        return;
    };
    let caps = pool.capabilities.clone();
    let db_arg = caps.databases.then(|| database.clone());
    let listed = rt.block_on(run_blocking(move || {
        let schemas: Vec<Option<String>> = if caps.schemas {
            pool.session
                .list_schemas(db_arg.as_deref())?
                .into_iter()
                .map(Some)
                .collect()
        } else {
            vec![None]
        };
        let mut out = Vec::new();
        for schema in schemas {
            for t in pool
                .session
                .list_tables(db_arg.as_deref(), schema.as_deref())?
            {
                out.push((
                    driver_api::cache::cache_table_name(schema.as_deref(), &t.name),
                    t.kind,
                ));
            }
        }
        Ok(out)
    }));
    match listed {
        Ok(all) => {
            let staged: Vec<(String, String)> = all
                .iter()
                .map(|(name, kind)| {
                    let label = if *kind == TableKind::View {
                        "view"
                    } else {
                        "table"
                    };
                    (name.clone(), label.to_string())
                })
                .collect();
            cache_data::save_tables_to_cache(tabular, connection_id, &database, &staged);
            let names = staged
                .into_iter()
                .filter(|(_, t)| t == table_type)
                .map(|(n, _)| n)
                .collect();
            folder.children = table_nodes(connection_id, &database, names, child_kind);
        }
        Err(e) => {
            log::warn!("[DRIVER-PLUGIN] Failed to list tables: {e}");
            folder.children = vec![TreeNode::new(
                format!("Failed to load: {e}"),
                NodeType::Column,
            )];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::DatabaseType;

    #[test]
    fn engine_without_databases_gets_object_folders() {
        let conn = ConnectionConfig {
            connection_type: DatabaseType::Plugin("not-installed".into()),
            ..Default::default()
        };
        // Driver tidak terpasang: capability default (databases = true).
        let children = structure_from_databases(1, &conn, &["a".into(), "b".into()]);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].children.len(), 2);
        assert_eq!(children[0].children[0].children.len(), 2);
        assert_eq!(single_database(&conn), "default");
    }
}
