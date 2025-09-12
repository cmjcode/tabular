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

// Sub view inside Structure (so kita tidak render dua tabel sekaligus)
#[derive(Clone, Debug, PartialEq, Default)]
pub enum StructureSubView {
    #[default]
    Columns,
    Indexes,
}

// Spreadsheet editing structures
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
#[derive(Default)]
pub struct SpreadsheetState {
    pub editing_cell: Option<(usize, usize)>, // (row, col) being edited
    pub cell_edit_text: String,               // Text being edited in the cell
    pub pending_operations: Vec<CellEditOperation>, // Unsaved changes
    pub is_dirty: bool,                       // Whether there are unsaved changes
    pub primary_key_columns: Vec<String>,     // Primary key column names for generating SQL
}


/// Type alias for the complex tuple returned by render_tree_node_with_table_expansion
pub type RenderTreeNodeResult = (
    Option<models::structs::ExpansionRequest>,
    Option<(usize, i64, String)>,
    Option<i64>,
    Option<(i64, String)>,
    Option<i64>,
    Option<(String, String, String)>,
    Option<String>,
    Option<String>,
    Option<(i64, String)>,
    Option<(i64, models::enums::NodeType)>,
    Option<(i64, String, Option<String>, Option<String>)>,
    Option<(i64, Option<String>, Option<String>)>,
);
