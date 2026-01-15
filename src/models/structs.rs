use serde::{Deserialize, Serialize};

use crate::models::{self, enums::NodeType};

#[derive(Clone)]
pub struct TreeNode {
    pub name: String,
    pub children: Vec<TreeNode>,
    pub is_expanded: bool,
    pub(crate) node_type: NodeType,
    pub connection_id: Option<i64>,    // For connection nodes
    pub is_loaded: bool,               // For tracking if tables/columns are loaded
    pub database_name: Option<String>, // For storing database context
    pub file_path: Option<String>,     // For query files
    pub table_name: Option<String>,    // For storing table context for subfolders/items
}

impl TreeNode {
    pub fn new(name: String, node_type: NodeType) -> Self {
        Self {
            name,
            children: Vec::new(),
            is_expanded: false,
            node_type,
            connection_id: None,
            is_loaded: true, // Regular nodes are always loaded
            database_name: None,
            file_path: None,
            table_name: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_children(name: String, node_type: NodeType, children: Vec<TreeNode>) -> Self {
        Self {
            name,
            children,
            is_expanded: false,
            node_type,
            connection_id: None,
            is_loaded: true,
            database_name: None,
            file_path: None,
            table_name: None,
        }
    }

    pub fn new_connection(name: String, connection_id: i64) -> Self {
        Self {
            name,
            children: Vec::new(),
            is_expanded: false,
            node_type: NodeType::Connection,
            connection_id: Some(connection_id),
            is_loaded: false, // Connection nodes need to load tables
            database_name: None,
            file_path: None,
            table_name: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForeignKey {
    pub constraint_name: String,
    pub table_name: String,
    pub column_name: String,
    pub referenced_table_name: String,
    pub referenced_column_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramGroup {
    pub id: String,
    pub title: String,
    #[serde(with = "serde_color")]
    pub color: eframe::egui::Color32,
    // nodes are linked by group_id in DiagramNode
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramNode {
    pub id: String, // usually table name
    pub title: String,
    #[serde(with = "serde_pos2")]
    pub pos: eframe::egui::Pos2,
    #[serde(with = "serde_vec2")]
    pub size: eframe::egui::Vec2,
    pub columns: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>, // FKs originating from this table
    pub group_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramEdge {
    pub source: String,
    pub target: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramState {
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    pub groups: Vec<DiagramGroup>,
    #[serde(with = "serde_vec2")]
    pub pan: eframe::egui::Vec2,
    pub zoom: f32,
    #[serde(skip)]
    pub dragging_node: Option<String>,
    #[serde(skip)]
    pub dragging_offset: eframe::egui::Vec2,
    #[serde(skip)]
    pub last_mouse_pos: Option<eframe::egui::Pos2>,
    pub is_centered: bool,
    #[serde(skip)]
    pub save_requested: bool,
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            pan: eframe::egui::Vec2::ZERO,
            zoom: 1.0,
            dragging_node: None,
            dragging_offset: eframe::egui::Vec2::ZERO,
            last_mouse_pos: None,
            is_centered: false,
            save_requested: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueryTab {
    pub title: String,
    pub content: String,
    pub file_path: Option<String>,
    pub is_saved: bool,
    pub is_modified: bool,
    pub connection_id: Option<i64>, // Each tab can have its own database connection
    pub database_name: Option<String>, // Each tab can have its own database selection
    pub has_executed_query: bool,   // Track if this tab has ever executed a query
    // NEW: per-tab result state so switching tabs restores its own data
    pub result_headers: Vec<String>,
    pub result_rows: Vec<Vec<String>>, // current page (or all rows if client side)
    pub result_all_rows: Vec<Vec<String>>, // full dataset for client pagination
    pub result_table_name: String,     // caption/status e.g. Table: ... or Query Results
    pub is_table_browse_mode: bool,    // was this produced by table browse
    pub current_page: usize,
    pub page_size: usize,
    pub total_rows: usize,
    pub base_query: String, // Store the base query (without LIMIT/OFFSET) for pagination
    // DBA quick view special post-processing mode (Replication Status, Master Status, etc.)
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub object_ddl: Option<String>, // Optional DDL (e.g., ALTER VIEW) for browsed objects
    // Query execution message (similar to TablePlus message tab)
    pub query_message: String,      // Message text (success/error)
    pub query_message_is_error: bool, // Whether the message is an error or success

    // Diagram state for "Diagrams" tab
    pub diagram_state: Option<DiagramState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorColorTheme {
    GithubDark,
    GithubLight,
    Gruvbox,
}

#[derive(Clone)]
pub struct AdvancedEditor {
    pub show_line_numbers: bool,
    pub theme: EditorColorTheme,
    pub font_size: f32,
    #[allow(dead_code)]
    pub tab_size: usize,
    #[allow(dead_code)]
    pub auto_indent: bool,
    #[allow(dead_code)]
    pub show_whitespace: bool,
    pub word_wrap: bool,
    // Number of visible rows the editor should aim to display; set dynamically to fill height
    pub desired_rows: usize,
    pub find_text: String,
    pub replace_text: String,
    pub show_find_replace: bool,
    pub case_sensitive: bool,
    pub use_regex: bool,
}

impl Default for AdvancedEditor {
    fn default() -> Self {
        Self {
            show_line_numbers: true,
            theme: EditorColorTheme::GithubDark,
            font_size: 14.0,
            tab_size: 4,
            auto_indent: true,
            show_whitespace: false,
            word_wrap: false,
            desired_rows: 25,
            find_text: String::new(),
            replace_text: String::new(),
            show_find_replace: false,
            case_sensitive: false,
            use_regex: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: Option<i64>,
    pub query: String,
    pub connection_id: i64,
    pub connection_name: String,
    pub executed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: Option<i64>,
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub connection_type: models::enums::DatabaseType,
    pub folder: Option<String>, // Custom folder name
    pub ssh_enabled: bool,
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_username: String,
    pub ssh_auth_method: models::enums::SshAuthMethod,
    pub ssh_private_key: String,
    pub ssh_password: String,
    pub ssh_accept_unknown_host_keys: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            host: "localhost".to_string(),
            port: "3306".to_string(),
            username: String::new(),
            password: String::new(),
            database: String::new(),
            connection_type: models::enums::DatabaseType::MySQL,
            folder: None, // No custom folder by default
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: "22".to_string(),
            ssh_username: String::new(),
            ssh_auth_method: models::enums::SshAuthMethod::Key,
            ssh_private_key: String::new(),
            ssh_password: String::new(),
            ssh_accept_unknown_host_keys: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExpansionRequest {
    pub node_type: models::enums::NodeType,
    pub connection_id: i64,
    pub database_name: Option<String>,
}

// UI state for Create/Edit Index modal
#[derive(Clone, Debug, PartialEq)]
pub enum IndexDialogMode {
    Create,
    Edit,
}

#[derive(Clone, Debug)]
pub struct IndexDialogState {
    pub mode: IndexDialogMode,
    pub connection_id: i64,
    pub database_name: Option<String>, // For PG schema or MsSQL db context
    pub table_name: String,
    pub existing_index_name: Option<String>,
    pub index_name: String,
    pub columns: String, // comma-separated list the user can edit
    pub unique: bool,
    pub method: Option<String>, // e.g., btree/hash for PG, BTREE/HASH for MySQL
    pub db_type: crate::models::enums::DatabaseType,
}

// Bottom panel view mode for a selected table
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TableBottomView {
    #[default]
    Data,
    Structure,
    Query,
    Messages,
}

// Simplified column info for Structure tab (can be extended later per RDBMS)
#[derive(Clone, Debug, Default)]
pub struct ColumnStructInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: Option<bool>,
    pub default_value: Option<String>,
    pub extra: Option<String>,
}

// Simplified index info shown in Structure -> Indexes
#[derive(Clone, Debug, Default)]
pub struct IndexStructInfo {
    pub name: String,
    pub method: Option<String>, // algorithm / type (btree, hash, etc.)
    pub unique: bool,
    pub columns: Vec<String>,
}

// Simplified partition info shown in Structure -> Partitions
#[derive(Clone, Debug, Default)]
pub struct PartitionStructInfo {
    pub name: String,
    pub partition_type: Option<String>, // RANGE, LIST, HASH, etc.
    pub partition_expression: Option<String>, // PARTITION BY expression
    pub subpartition_type: Option<String>, // For composite partitioning
}

// Sub view inside Structure (so kita tidak render dua tabel sekaligus)
#[derive(Clone, Debug, PartialEq, Default)]
pub enum StructureSubView {
    #[default]
    Columns,
    Indexes,
}

// Spreadsheet editing structures
#[derive(Clone, Debug, PartialEq)]
pub enum SpreadsheetOperationType {
    Update,
    Insert,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellEditOperation {
    Update {
        row_index: usize,
        col_index: usize,
        old_value: String,
        new_value: String,
    },
    InsertRow {
        row_index: usize,
        values: Vec<String>,
    },
    DeleteRow {
        row_index: usize,
        values: Vec<String>, // Store original values for undo
    },
}

#[derive(Clone, Debug)]
pub struct SpreadsheetOperation {
    pub operation_type: SpreadsheetOperationType,
    pub table_name: String,
    pub row_index: usize,
    pub column_name: String,
    pub old_value: String,
    pub new_value: String,
    pub primary_key_values: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct SpreadsheetState {
    pub editing_cell: Option<(usize, usize)>, // (row, col) being edited
    pub cell_edit_text: String,               // Text being edited in the cell
    pub pending_operations: Vec<CellEditOperation>, // Unsaved changes
    pub is_dirty: bool,                       // Whether there are unsaved changes
    pub primary_key_columns: Vec<String>,     // Primary key column names for generating SQL
}

#[derive(Clone, Debug)]
pub struct TableColumnDefinition {
    pub name: String,
    pub data_type: String,
    pub allow_null: bool,
    pub default_value: String,
    pub is_primary_key: bool,
}

impl TableColumnDefinition {
    pub fn blank(index: usize) -> Self {
        let base_name = if index == 0 {
            "id".to_string()
        } else {
            format!("column_{}", index + 1)
        };
        Self {
            name: base_name,
            data_type: String::new(),
            allow_null: true,
            default_value: String::new(),
            is_primary_key: index == 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TableIndexDefinition {
    pub name: String,
    pub columns: String,
    pub unique: bool,
}

impl TableIndexDefinition {
    pub fn blank(index: usize) -> Self {
        Self {
            name: format!("idx_{}", index + 1),
            columns: String::new(),
            unique: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateTableWizardStep {
    Basics,
    Columns,
    Indexes,
    Review,
}

impl CreateTableWizardStep {
    pub fn all_steps() -> [Self; 4] {
        [Self::Basics, Self::Columns, Self::Indexes, Self::Review]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Basics => "Basics",
            Self::Columns => "Columns",
            Self::Indexes => "Indexes",
            Self::Review => "Review",
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Basics => Some(Self::Columns),
            Self::Columns => Some(Self::Indexes),
            Self::Indexes => Some(Self::Review),
            Self::Review => None,
        }
    }

    pub fn previous(self) -> Option<Self> {
        match self {
            Self::Basics => None,
            Self::Columns => Some(Self::Basics),
            Self::Indexes => Some(Self::Columns),
            Self::Review => Some(Self::Indexes),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateTableWizardState {
    pub connection_id: i64,
    pub db_type: models::enums::DatabaseType,
    pub database_name: Option<String>,
    pub table_name: String,
    pub columns: Vec<TableColumnDefinition>,
    pub indexes: Vec<TableIndexDefinition>,
    pub current_step: CreateTableWizardStep,
}

impl CreateTableWizardState {
    pub fn new(
        connection_id: i64,
        db_type: models::enums::DatabaseType,
        database_name: Option<String>,
    ) -> Self {
        let mut first_column = TableColumnDefinition::blank(0);
        first_column.data_type = match db_type {
            models::enums::DatabaseType::PostgreSQL => "SERIAL".to_string(),
            models::enums::DatabaseType::SQLite => "INTEGER".to_string(),
            models::enums::DatabaseType::MySQL => "INT".to_string(),
            models::enums::DatabaseType::MsSQL => "INT".to_string(),
            _ => String::new(),
        };
        first_column.allow_null = false;
        first_column.is_primary_key = true;

        Self {
            connection_id,
            db_type,
            database_name,
            table_name: String::new(),
            columns: vec![first_column],
            indexes: Vec::new(),
            current_step: CreateTableWizardStep::Basics,
        }
    }
}

/// Type alias for the complex tuple returned by render_tree_node_with_table_expansion
pub type RenderTreeNodeResult = (
    Option<models::structs::ExpansionRequest>,
    Option<(usize, i64, String)>,
    Option<i64>,
    Option<(i64, String, models::enums::NodeType)>,
    Option<i64>,
    Option<(String, String, String)>,
    Option<String>,
    Option<String>,
    Option<(i64, String)>,
    Option<(i64, models::enums::NodeType)>,
    Option<(i64, String, Option<String>, Option<String>)>,
    Option<(i64, Option<String>, Option<String>)>,
    // New: request to open Structure view for Alter Table (connection_id, database, table_name)
    Option<(i64, Option<String>, String)>,
    // New: request to drop a MongoDB collection (connection_id, database_name, collection_name)
    Option<(i64, String, String)>,
    // New: request to drop a table (connection_id, database_name, table_name, stmt)
    Option<(i64, String, String, String)>,
    // New: request to open Create Table wizard (connection_id, optional database/schema)
    Option<(i64, Option<String>)>,
    // New: request to open ALTER script for stored procedure (connection_id, database, procedure_name)
    Option<(i64, Option<String>, String)>,
    // New: request to generate CREATE TABLE script (connection_id, database, table_name)
    Option<(i64, Option<String>, String)>,
    Option<(i64, String)>,
);

mod serde_color {
    use serde::{Deserialize, Deserializer, Serializer};
    use eframe::egui::Color32;

    pub fn serialize<S>(color: &Color32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let array = [color.r(), color.g(), color.b(), color.a()];
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(4)?;
        tup.serialize_element(&array[0])?;
        tup.serialize_element(&array[1])?;
        tup.serialize_element(&array[2])?;
        tup.serialize_element(&array[3])?;
        tup.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Color32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: [u8; 4] = Deserialize::deserialize(deserializer)?;
        Ok(Color32::from_rgba_premultiplied(opt[0], opt[1], opt[2], opt[3]))
    }
}

mod serde_pos2 {
    use serde::{Deserialize, Deserializer, Serializer};
    use eframe::egui::Pos2;

    pub fn serialize<S>(pos: &Pos2, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&pos.x)?;
        tup.serialize_element(&pos.y)?;
        tup.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Pos2, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: [f32; 2] = Deserialize::deserialize(deserializer)?;
        Ok(Pos2::new(opt[0], opt[1]))
    }
}

mod serde_vec2 {
    use serde::{Deserialize, Deserializer, Serializer};
    use eframe::egui::Vec2;

    pub fn serialize<S>(vec: &Vec2, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&vec.x)?;
        tup.serialize_element(&vec.y)?;
        tup.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec2, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: [f32; 2] = Deserialize::deserialize(deserializer)?;
        Ok(Vec2::new(opt[0], opt[1]))
    }
}
