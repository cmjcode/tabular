//! Item context menu sidebar untuk objek skema. Setiap item hanya memasukkan
//! [`SchemaAction`] ke antrean; eksekusinya ada di `schema_actions`.

use super::schema_actions::{SchemaAction, queue_action};
use crate::models::enums::{DatabaseType, NodeType};
use crate::models::structs::TreeNode;
use crate::schema_objects::catalog::RoutineKind;
use crate::schema_objects::sql::{self, RenameTarget};
use eframe::egui;

fn object_name(node: &TreeNode) -> String {
    node.table_name.clone().unwrap_or_else(|| node.name.clone())
}

fn database_of(node: &TreeNode) -> Option<String> {
    node.database_name
        .clone()
        .or_else(|| (node.node_type == NodeType::Database).then(|| node.name.clone()))
}

fn push(ui: &mut egui::Ui, action: SchemaAction) {
    queue_action(ui.ctx(), action);
    ui.close();
}

/// Jenis source untuk node routine/objek di sidebar.
pub(crate) fn routine_kind_for(node_type: &NodeType) -> Option<RoutineKind> {
    match node_type {
        NodeType::StoredProcedure => Some(RoutineKind::Procedure),
        NodeType::UserFunction => Some(RoutineKind::Function),
        NodeType::Trigger => Some(RoutineKind::Trigger),
        NodeType::Event => Some(RoutineKind::Event),
        NodeType::View => Some(RoutineKind::View),
        NodeType::MaterializedView => Some(RoutineKind::MaterializedView),
        NodeType::UserType => Some(RoutineKind::UserType),
        _ => None,
    }
}

/// Buka source objek di tab baru (klik kiri pada node routine).
pub(crate) fn queue_view_source(ctx: &egui::Context, node: &TreeNode) {
    if let (Some(conn_id), Some(kind)) = (node.connection_id, routine_kind_for(&node.node_type)) {
        queue_action(
            ctx,
            SchemaAction::ViewSource {
                conn_id,
                database: node.database_name.clone(),
                kind,
                name: object_name(node),
                export: false,
            },
        );
    }
}

fn source_items(ui: &mut egui::Ui, node: &TreeNode, kind: RoutineKind) {
    let Some(conn_id) = node.connection_id else {
        return;
    };
    if ui
        .button(format!("📜 Show {} Source", kind.label()))
        .clicked()
    {
        push(
            ui,
            SchemaAction::ViewSource {
                conn_id,
                database: node.database_name.clone(),
                kind,
                name: object_name(node),
                export: false,
            },
        );
    }
    #[cfg(not(target_os = "ios"))]
    if ui.button("💾 Export Source to File…").clicked() {
        push(
            ui,
            SchemaAction::ViewSource {
                conn_id,
                database: node.database_name.clone(),
                kind,
                name: object_name(node),
                export: true,
            },
        );
    }
}

/// Tambahan menu node tabel: rename, comment, maintenance.
pub(crate) fn table_menu_items(ui: &mut egui::Ui, node: &TreeNode, db_type: Option<&DatabaseType>) {
    let (Some(conn_id), Some(db_type)) = (node.connection_id, db_type) else {
        return;
    };
    let name = object_name(node);
    let database = node.database_name.clone();
    if sql::supports_rename(db_type, RenameTarget::Table) && ui.button("✏️ Rename Table…").clicked()
    {
        push(
            ui,
            SchemaAction::Rename {
                conn_id,
                database: database.clone(),
                target: RenameTarget::Table,
                name: name.clone(),
            },
        );
    }
    if matches!(
        db_type,
        DatabaseType::MySQL | DatabaseType::PostgreSQL | DatabaseType::MsSQL
    ) && ui.button("📝 Edit Table Comment…").clicked()
    {
        push(
            ui,
            SchemaAction::EditComment {
                conn_id,
                database: database.clone(),
                table: name.clone(),
                is_view: false,
            },
        );
    }
    if !sql::table_maintenance_ops(db_type).is_empty() && ui.button("🔧 Maintenance…").clicked()
    {
        push(
            ui,
            SchemaAction::Maintenance {
                conn_id,
                database,
                table: Some(name),
            },
        );
    }
}

/// Tambahan menu node view: definisi, rename, comment, drop.
pub(crate) fn view_menu_items(ui: &mut egui::Ui, node: &TreeNode, db_type: Option<&DatabaseType>) {
    let (Some(conn_id), Some(db_type)) = (node.connection_id, db_type) else {
        return;
    };
    ui.separator();
    source_items(ui, node, RoutineKind::View);
    let name = object_name(node);
    let database = node.database_name.clone();
    if sql::supports_rename(db_type, RenameTarget::View) && ui.button("✏️ Rename View…").clicked()
    {
        push(
            ui,
            SchemaAction::Rename {
                conn_id,
                database: database.clone(),
                target: RenameTarget::View,
                name: name.clone(),
            },
        );
    }
    if matches!(db_type, DatabaseType::PostgreSQL | DatabaseType::MsSQL)
        && ui.button("📝 Edit View Comment…").clicked()
    {
        push(
            ui,
            SchemaAction::EditComment {
                conn_id,
                database: database.clone(),
                table: name.clone(),
                is_view: true,
            },
        );
    }
    if ui.button("🗑 Drop View…").clicked() {
        push(
            ui,
            SchemaAction::DropView {
                conn_id,
                database,
                name,
                materialized: false,
            },
        );
    }
}

/// Tambahan menu node database: rename, maintenance, schema PostgreSQL.
pub(crate) fn database_menu_items(
    ui: &mut egui::Ui,
    node: &TreeNode,
    db_type: Option<&DatabaseType>,
) {
    let (Some(conn_id), Some(db_type)) = (node.connection_id, db_type) else {
        return;
    };
    let Some(database) = database_of(node) else {
        return;
    };
    if sql::supports_rename(db_type, RenameTarget::Database)
        && ui.button("✏️ Rename Database…").clicked()
    {
        push(
            ui,
            SchemaAction::Rename {
                conn_id,
                database: Some(database.clone()),
                target: RenameTarget::Database,
                name: database.clone(),
            },
        );
    }
    if !sql::database_maintenance_ops(db_type).is_empty()
        && ui.button("🔧 Database Maintenance…").clicked()
    {
        push(
            ui,
            SchemaAction::Maintenance {
                conn_id,
                database: Some(database.clone()),
                table: None,
            },
        );
    }
    if *db_type == DatabaseType::PostgreSQL {
        if ui.button("➕ Create Schema…").clicked() {
            push(
                ui,
                SchemaAction::CreateSchema {
                    conn_id,
                    database: Some(database.clone()),
                },
            );
        }
        if ui.button("👥 Manage Schemas…").clicked() {
            push(
                ui,
                SchemaAction::ManageSchemas {
                    conn_id,
                    database: Some(database),
                },
            );
        }
    }
}

/// Toggle database sistem di menu folder "Databases".
pub(crate) fn databases_folder_menu_items(ui: &mut egui::Ui) {
    let mut show = crate::schema_objects::show_system_objects();
    if ui.checkbox(&mut show, "Show System Databases").clicked() {
        push(ui, SchemaAction::ToggleSystemObjects);
    }
}

/// Menu node routine, trigger, event, materialized view, dan tipe. `true`
/// jika node ini ditangani.
pub(crate) fn object_node_menu_items(
    ui: &mut egui::Ui,
    node: &TreeNode,
    db_type: Option<&DatabaseType>,
) -> bool {
    let Some(kind) = routine_kind_for(&node.node_type) else {
        return false;
    };
    if node.node_type == NodeType::View {
        return false;
    }
    let Some(conn_id) = node.connection_id else {
        return false;
    };
    let supported = db_type
        .map(|d| crate::schema_objects::catalog::routine_source_query(d, kind, "x").is_some())
        .unwrap_or(false);
    if supported {
        source_items(ui, node, kind);
    }
    if ui.button("📋 Copy Name").clicked() {
        ui.ctx().copy_text(object_name(node));
        ui.close();
    }
    let name = object_name(node);
    let database = node.database_name.clone();
    match node.node_type {
        NodeType::MaterializedView => {
            ui.separator();
            if ui.button("🔄 Refresh Materialized View…").clicked() {
                push(
                    ui,
                    SchemaAction::RefreshMatView {
                        conn_id,
                        database: database.clone(),
                        name: name.clone(),
                    },
                );
            }
            if ui.button("✏️ Rename…").clicked() {
                push(
                    ui,
                    SchemaAction::Rename {
                        conn_id,
                        database: database.clone(),
                        target: RenameTarget::MaterializedView,
                        name: name.clone(),
                    },
                );
            }
            if ui.button("🗑 Drop Materialized View…").clicked() {
                push(
                    ui,
                    SchemaAction::DropView {
                        conn_id,
                        database,
                        name,
                        materialized: true,
                    },
                );
            }
        }
        NodeType::UserType => {
            ui.separator();
            if ui.button("✏️ Edit Type…").clicked() {
                push(
                    ui,
                    SchemaAction::EditType {
                        conn_id,
                        database,
                        name,
                    },
                );
            }
        }
        _ => {}
    }
    true
}

/// Susunan folder database PostgreSQL. Beberapa jalur pembuat tree masih
/// memakai susunan generik (Tables/Views/Stored Procedures), jadi folder yang
/// hilang ditambahkan di sini sebelum tree digambar.
const PG_DATABASE_FOLDERS: [(NodeType, &str); 7] = [
    (NodeType::TablesFolder, "Tables"),
    (NodeType::ViewsFolder, "Views"),
    (NodeType::MaterializedViewsFolder, "Materialized Views"),
    (NodeType::UserFunctionsFolder, "Functions"),
    (NodeType::StoredProceduresFolder, "Procedures"),
    (NodeType::TriggersFolder, "Triggers"),
    (NodeType::TypesFolder, "Types"),
];

/// Lengkapi folder objek pada node database PostgreSQL. Murah untuk dipanggil
/// tiap frame: hanya turun sampai level database dan hanya menyusun ulang
/// bila ada folder yang belum ada.
pub(crate) fn ensure_pg_object_folders(
    nodes: &mut [TreeNode],
    connection_types: &std::collections::HashMap<i64, DatabaseType>,
) {
    for node in nodes.iter_mut() {
        match node.node_type {
            NodeType::Database => {
                let is_pg = node.connection_id.and_then(|id| connection_types.get(&id))
                    == Some(&DatabaseType::PostgreSQL);
                if is_pg && node.database_name.is_some() && !node.children.is_empty() {
                    complete_pg_folders(node);
                }
            }
            NodeType::Table | NodeType::View | NodeType::Query | NodeType::QueryHistItem => {}
            _ => ensure_pg_object_folders(&mut node.children, connection_types),
        }
    }
}

fn complete_pg_folders(db_node: &mut TreeNode) {
    let complete = PG_DATABASE_FOLDERS
        .iter()
        .all(|(t, _)| db_node.children.iter().any(|c| &c.node_type == t));
    if complete {
        return;
    }
    let mut existing = std::mem::take(&mut db_node.children);
    let mut ordered = Vec::with_capacity(existing.len() + PG_DATABASE_FOLDERS.len());
    for (folder_type, label) in PG_DATABASE_FOLDERS.iter() {
        if let Some(pos) = existing.iter().position(|c| &c.node_type == folder_type) {
            let mut folder = existing.remove(pos);
            folder.name = label.to_string();
            ordered.push(folder);
        } else {
            let mut folder = TreeNode::new(label.to_string(), folder_type.clone());
            folder.connection_id = db_node.connection_id;
            folder.database_name = db_node.database_name.clone();
            folder.is_loaded = false;
            ordered.push(folder);
        }
    }
    ordered.extend(existing);
    db_node.children = ordered;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_database_gets_all_object_folders_once() {
        let mut db = TreeNode::new("shop".to_string(), NodeType::Database);
        db.connection_id = Some(1);
        db.database_name = Some("shop".to_string());
        let mut tables = TreeNode::new("Tables".to_string(), NodeType::TablesFolder);
        tables.is_loaded = true;
        db.children = vec![
            TreeNode::new(
                "Stored Procedures".to_string(),
                NodeType::StoredProceduresFolder,
            ),
            tables,
        ];
        let mut conn = TreeNode::new("pg".to_string(), NodeType::Connection);
        conn.children = vec![db];
        let mut nodes = vec![conn];
        let types = std::collections::HashMap::from([(1, DatabaseType::PostgreSQL)]);

        ensure_pg_object_folders(&mut nodes, &types);
        let children = &nodes[0].children[0].children;
        let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Tables",
                "Views",
                "Materialized Views",
                "Functions",
                "Procedures",
                "Triggers",
                "Types"
            ]
        );
        // Folder yang sudah dimuat dipertahankan apa adanya.
        assert!(children[0].is_loaded);
        assert_eq!(children[2].database_name.as_deref(), Some("shop"));

        // Tidak berlaku untuk engine lain.
        let types = std::collections::HashMap::from([(1, DatabaseType::MySQL)]);
        let mut db = TreeNode::new("shop".to_string(), NodeType::Database);
        db.connection_id = Some(1);
        db.database_name = Some("shop".to_string());
        db.children = vec![TreeNode::new("Tables".to_string(), NodeType::TablesFolder)];
        let mut nodes = vec![db];
        ensure_pg_object_folders(&mut nodes, &types);
        assert_eq!(nodes[0].children.len(), 1);
    }
}
