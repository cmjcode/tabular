use chrono::{DateTime, Duration, Utc};
use eframe::{App, Frame, egui};
// Removed egui_code_editor; using simple TextEdit + lapce-core buffer backend
use crate::editor_buffer::EditorBuffer;
use log::{debug, error, info};
use sqlx::SqlitePool;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use crate::{
    cache_data, connection, dialog, directory, driver_mysql, driver_postgres, driver_redis,
    driver_sqlite, editor, models, query_tools, sidebar_database, sidebar_history, sidebar_query,
    spreadsheet::SpreadsheetOperations,
};
use crate::{data_table, driver_mssql};


pub mod app_impl;
pub mod connection_mgr;
pub mod diagram;
pub mod init;
pub mod pagination;
pub mod query_jobs;
pub mod render_dialogs;
pub mod search;
pub mod settings;
pub mod sidebar_tree;
pub mod table_wizard;
pub mod tree_loader;
pub mod update;

pub struct Tabular {
    pub editor: EditorBuffer,
    // Transitional multi-selection model (will move to lapce-core selection)
    pub multi_selection: crate::editor_selection::MultiSelection,
    pub selected_menu: String,
    pub items_tree: Vec<models::structs::TreeNode>,
    pub queries_tree: Vec<models::structs::TreeNode>,
    pub history_tree: Vec<models::structs::TreeNode>,
    pub history_items: Vec<models::structs::HistoryItem>, // Actual history data
    pub connections: Vec<models::structs::ConnectionConfig>,
    pub show_add_connection: bool,
    pub new_connection: models::structs::ConnectionConfig,
    pub db_pool: Option<Arc<SqlitePool>>,
    // Global async runtime for all database operations
    pub runtime: Option<Arc<tokio::runtime::Runtime>>,
    // Connection cache untuk menghindari membuat koneksi berulang
    pub connection_pools: HashMap<i64, models::enums::DatabasePool>,
    // Track connection pools currently being created to avoid duplicate work
    pub pending_connection_pools: std::collections::HashSet<i64>,
    // Shared connection pools for background tasks
    pub shared_connection_pools: Arc<std::sync::Mutex<HashMap<i64, models::enums::DatabasePool>>>,
    // Rate-limit log spam for pending pool creation messages
    pub pending_pool_log_last: HashMap<i64, std::time::Instant>,
    // Prefetch progress tracking
    pub prefetch_progress: HashMap<i64, (usize, usize)>, // connection_id -> (completed, total)
    pub prefetch_in_progress: std::collections::HashSet<i64>, // connections currently prefetching
    // Context menu and edit connection fields
    pub show_edit_connection: bool,
    pub edit_connection: models::structs::ConnectionConfig,
    // UI refresh flag
    pub needs_refresh: bool,
    // Table data display
    pub current_table_data: Vec<Vec<String>>,
    pub current_table_headers: Vec<String>,
    pub current_table_name: String,
    pub current_column_metadata: Option<Vec<models::structs::ColumnMetadata>>, // Metadata for current table columns
    pub current_object_ddl: Option<String>,
    pub current_connection_id: Option<i64>,
    // Pagination
    pub current_page: usize,
    pub page_size: usize,
    pub total_rows: usize,
    pub all_table_data: Vec<Vec<String>>, // Store all data for pagination
    // Server-side pagination
    pub use_server_pagination: bool,
    pub actual_total_rows: Option<usize>, // Real total from COUNT query
    pub current_base_query: String,       // Original query without LIMIT/OFFSET
    // Splitter position for resizable table view (0.0 to 1.0)
    pub table_split_ratio: f32,
    // Table sorting state
    pub sort_column: Option<usize>,
    pub sort_ascending: bool,
    // Test connection status
    pub test_connection_status: Option<(bool, String)>, // (success, message)
    pub test_connection_in_progress: bool,
    // Background processing channels
    pub background_sender: Option<Sender<models::enums::BackgroundTask>>,
    pub background_receiver: Option<Receiver<models::enums::BackgroundResult>>,
    pub query_result_sender: Sender<connection::QueryResultMessage>,
    pub query_result_receiver: Receiver<connection::QueryResultMessage>,
    pub active_query_jobs: std::collections::HashMap<u64, connection::QueryJobStatus>,
    pub active_query_handles: std::collections::HashMap<u64, tokio::task::JoinHandle<()>>,
    pub cancelled_query_jobs: std::collections::HashMap<u64, std::time::Instant>,
    pub pending_paginated_jobs: std::collections::HashSet<u64>,
    pub next_query_job_id: u64,
    // Background refresh status tracking
    pub refreshing_connections: std::collections::HashSet<i64>,
    // Pending expansion state restore after refresh
    pub pending_expansion_restore:
        std::collections::HashMap<i64, std::collections::HashMap<String, bool>>,
    // Connections that need their expanded nodes loaded after state restore
    pub pending_auto_load: std::collections::HashSet<i64>,
    // Query tab system
    pub query_tabs: Vec<models::structs::QueryTab>,
    pub active_tab_index: usize,
    pub next_tab_id: usize,
    // Save dialog
    pub show_save_dialog: bool,
    pub save_filename: String,
    pub save_directory: String,
    pub save_directory_picker_result: Option<std::sync::mpsc::Receiver<String>>,
    // Connection selection dialog
    pub show_connection_selector: bool,
    pub pending_query: String, // Store query to execute after connection is selected
    pub auto_execute_after_connection: bool, // Flag to auto-execute after connection selected
    pub query_execution_in_progress: bool,
    pub query_icon_hold_until: Option<std::time::Instant>,
    // Error message display
    pub error_message: String,
    pub show_error_message: bool,
    // Advanced Editor Configuration
    pub advanced_editor: models::structs::AdvancedEditor,
    // Selected text for executing only selected queries
    pub selected_text: String,
    pub clipboard_multi_segments: Option<Vec<String>>,
    pub clipboard_multi_regions: Option<Vec<crate::editor_selection::SelRegion>>,
    pub clipboard_multi_version: Option<u64>,
    // Cursor position for query extraction
    pub cursor_position: usize,
    // Selection range indices (start inclusive, end exclusive) for advanced editing (indent/outdent)
    pub selection_start: usize,
    pub selection_end: usize,
    // Command Palette
    pub show_command_palette: bool,
    pub command_palette_input: String,
    pub show_theme_selector: bool,
    pub command_palette_items: Vec<String>,
    pub command_palette_selected_index: usize,
    pub theme_selector_selected_index: usize,
    // Flag to request theme selector on next frame
    pub request_theme_selector: bool,
    pub app_theme: crate::config::AppTheme,
    pub link_editor_theme: bool, // when true editor theme follows app theme
    // Settings window visibility
    pub show_settings_window: bool,
    // Database search functionality
    pub database_search_text: String,
    pub filtered_items_tree: Vec<models::structs::TreeNode>,
    // Cache miss confirmation state
    pub cache_miss_request: Option<(i64, String, String)>, // (connection_id, database_name, table_name)
    pub show_search_results: bool,
    // History search functionality
    pub history_search_text: String,
    pub filtered_history_tree: Vec<models::structs::TreeNode>,
    // Query folder management
    pub show_create_folder_dialog: bool,
    pub new_folder_name: String,
    pub selected_query_for_move: Option<String>,
    pub show_move_to_folder_dialog: bool,
    pub target_folder_name: String,
    pub parent_folder_for_creation: Option<String>,
    pub selected_folder_for_removal: Option<String>,
    pub folder_removal_map: std::collections::HashMap<i64, String>, // Map hash to folder path
    // Create Table wizard state
    pub show_create_table_dialog: bool,
    pub create_table_wizard: Option<models::structs::CreateTableWizardState>,
    pub create_table_error: Option<String>,
    // Connection pool cleanup tracking
    pub last_cleanup_time: std::time::Instant,
    // Table selection tracking
    pub selected_row: Option<usize>,
    pub selected_cell: Option<(usize, usize)>, // (row_index, column_index)
    // Multi-selection (per page)
    pub selected_rows: BTreeSet<usize>,
    pub selected_columns: BTreeSet<usize>,
    pub last_clicked_row: Option<usize>,
    pub last_clicked_column: Option<usize>,
    // Track if table was recently clicked for focus management
    pub table_recently_clicked: bool,
    // Anchor cell for multi-cell selection in Data table (with shift-click/arrow or drag)
    pub table_sel_anchor: Option<(usize, usize)>,
    // True while user is dragging to select a multi-cell rectangle in Data table
    pub table_dragging: bool,
    // Scroll to selected cell flag
    pub scroll_to_selected_cell: bool,
    // Column width management for resizable columns
    pub column_widths: Vec<f32>, // Store individual column widths
    pub min_column_width: f32,
    // One-frame suppression flag to prevent editor autocomplete reacting to arrow keys consumed by table navigation
    /// One-frame flag set by table arrow navigation to suppress editor autocomplete
    /// reacting to the same left/right arrow key event.
    pub suppress_editor_arrow_once: bool,
    // Gear menu and about dialog
    pub show_about_dialog: bool,
    // Preferences persistence
    pub config_store: Option<crate::config::ConfigStore>,
    pub last_saved_prefs: Option<crate::config::AppPreferences>,
    pub prefs_dirty: bool,
    pub prefs_save_feedback: Option<String>,
    pub prefs_last_saved_at: Option<std::time::Instant>,
    pub prefs_loaded: bool,
    // Data directory setting
    pub data_directory: String,
    pub temp_data_directory: String,
    pub show_directory_picker: bool,
    pub directory_picker_result: Option<std::sync::mpsc::Receiver<String>>,
    pub sqlite_path_picker_result: Option<std::sync::mpsc::Receiver<String>>,
    pub temp_sqlite_path: Option<String>,
    // Logo texture
    pub logo_texture: Option<egui::TextureHandle>,
    // Pre-loaded PNG icons for each DB type (key = DatabaseType::icon_key())
    pub db_icon_textures: HashMap<String, egui::TextureHandle>,
    // Database cache for performance
    pub database_cache: std::collections::HashMap<i64, Vec<String>>, // connection_id -> databases
    pub database_cache_time: std::collections::HashMap<i64, std::time::Instant>, // connection_id -> cache time
    // Autocomplete state
    pub show_autocomplete: bool,
    pub autocomplete_suggestions: Vec<String>,
    pub autocomplete_kinds: Vec<models::enums::AutocompleteKind>,
    pub autocomplete_notes: Vec<Option<String>>, // optional description per suggestion
    pub autocomplete_payloads: Vec<Option<String>>, // optional payload such as snippet expansion text
    pub selected_autocomplete_index: usize,
    pub autocomplete_prefix: String,
    pub last_autocomplete_trigger_len: usize,
    pub pending_cursor_set: Option<usize>,
    // Keep editor focused for a few frames after actions like autocomplete accept
    pub editor_focus_boost_frames: u8,
    // Enforce caret after autocomplete for a few frames
    pub autocomplete_expected_cursor: Option<usize>,
    pub autocomplete_protection_frames: u8,
    // Tracks whether user has navigated autocomplete popup (ArrowUp/Down or similar)
    pub autocomplete_navigated: bool,
    // Autocomplete throttle
    pub autocomplete_last_update: Option<std::time::Instant>,
    pub autocomplete_debounce_ms: u64,
    // Ensure selection is cleared on the next frame after a destructive action (e.g., Delete)
    pub selection_force_clear: bool,
    // Multi-cursor support: additional caret positions (primary caret tracked separately)
    pub extra_cursors: Vec<usize>,
    pub last_editor_text: String, // For detecting text changes in multi-cursor mode (deprecated; will derive from editor.text)
    // Syntax highlighting cache (text_hash -> LayoutJob)
    pub highlight_cache: std::collections::HashMap<u64, eframe::egui::text::LayoutJob>,
    pub last_highlight_hash: Option<u64>,
    // New per-line highlight cache was used by the removed custom editor; no longer needed
    // Index dialog
    pub show_index_dialog: bool,
    pub index_dialog: Option<models::structs::IndexDialogState>,
    // Bottom panel view mode (Data / Structure)
    pub table_bottom_view: models::structs::TableBottomView,
    // Cached structure info for current table
    pub structure_columns: Vec<models::structs::ColumnStructInfo>,
    pub structure_indexes: Vec<models::structs::IndexStructInfo>,
    // Selection for Structure views (independent from Data grid selection)
    pub structure_selected_row: Option<usize>,
    pub structure_selected_cell: Option<(usize, usize)>,
    // Anchor cell for multi-cell selection in Structure views (with shift-click or drag)
    pub structure_sel_anchor: Option<(usize, usize)>,
    // True while user is dragging mouse to select a multi-cell rectangle in Structure
    pub structure_dragging: bool,
    // Pending drop index confirmation
    pub pending_drop_index_name: Option<String>,
    pub pending_drop_index_stmt: Option<String>,
    // Pending drop column confirmation
    pub pending_drop_column_name: Option<String>,
    pub pending_drop_column_stmt: Option<String>,
    // Pending drop Mongo collection confirmation
    pub pending_drop_collection: Option<(i64, String, String)>, // (connection_id, db, collection)
    // Pending drop table confirmation
    pub pending_drop_table: Option<(i64, String, String, String)>, // (connection_id, database, table, stmt)
    // Structure view column widths (separate from data grid)
    pub structure_col_widths: Vec<f32>,     // for columns table
    pub structure_idx_col_widths: Vec<f32>, // for indexes table
    pub structure_sub_view: models::structs::StructureSubView,
    // Track last loaded structure target to avoid redundant reloads on tab toggles
    pub last_structure_target: Option<(i64, String, String)>, // (connection_id, database, table)
    // Flag to force next structure load even if target unchanged (used by manual refresh)
    pub request_structure_refresh: bool,
    // Inline add/edit column state for Structure -> Columns
    pub adding_column: bool,
    pub new_column_name: String,
    pub new_column_type: String,
    pub new_column_nullable: bool,
    pub new_column_default: String,
    pub editing_column: bool,
    pub edit_column_original_name: String,
    pub edit_column_name: String,
    pub edit_column_type: String,
    pub edit_column_nullable: bool,
    pub edit_column_default: String,
    // Inline add-index state for Structure -> Indexes
    pub adding_index: bool,
    pub new_index_name: String,
    pub new_index_method: String,
    pub new_index_unique: bool,
    pub new_index_columns: String,
    // SQL filter/where clause for data table
    pub sql_filter_text: String,
    // Flag to indicate if current data is from table browse (true) or manual query (false)
    pub is_table_browse_mode: bool,
    // Store original query for manual queries (to apply filters)
    // Self-update functionality
    pub update_info: Option<crate::self_update::UpdateInfo>,
    pub show_update_dialog: bool,
    pub update_check_in_progress: bool,
    pub update_check_error: Option<String>,
    pub last_update_check: Option<std::time::Instant>,
    pub update_download_in_progress: bool,
    pub auto_check_updates: bool,
    pub manual_update_check: bool, // Track if update check was manually triggered
    // Lightweight notification (toast) instead of full dialog for auto updates
    pub show_update_notification: bool,
    pub update_download_started: bool,
    pub update_installed: bool,
    pub update_install_receiver: Option<std::sync::mpsc::Receiver<bool>>, // receive success flag
    pub enable_debug_logging: bool, // New field for debug logging
    // Auto updater instance
    pub auto_updater: Option<crate::auto_updater::AutoUpdater>,
    // Preferences window active tab
    pub settings_active_pref_tab: PrefTab,
    // Lightweight settings context menu (gear popup)
    pub show_settings_menu: bool,
    // Query execution wait when pool is being created
    pub pool_wait_in_progress: bool,
    pub pool_wait_connection_id: Option<i64>,
    pub pool_wait_query: String,
    pub pool_wait_started_at: Option<std::time::Instant>,
    // Spreadsheet editing state
    pub spreadsheet_state: crate::models::structs::SpreadsheetState,
    // Lapce buffer integration for editor (replaces egui_code_editor)
    // Deprecated standalone lapce buffer (now integrated in EditorBuffer)
    // pub lapce_buffer: Option<Buffer>,
    // Context menu for row operations
    pub show_row_context_menu: bool,
    pub context_menu_row: Option<usize>,
    pub context_menu_just_opened: bool,
    pub context_menu_pos: egui::Pos2,
    // Track newly created/duplicated rows for highlighting
    pub newly_created_rows: std::collections::HashSet<usize>,
    // --- Query AST Debug Panel (feature gated at runtime; safe if feature off) ---
    pub show_query_ast_debug: bool,
    pub last_compiled_sql: Option<String>,
    pub last_compiled_headers: Vec<String>,
    pub last_debug_plan: Option<String>,
    pub last_cache_hits: u64,
    pub last_cache_misses: u64,
    pub last_plan_hash: Option<u64>,
    pub last_plan_cache_key: Option<String>,
    pub last_ctes: Option<Vec<String>>, // names of remaining CTEs after rewrites
    pub sql_semantic_snapshot: Option<Arc<crate::syntax_ts::SqlSemanticSnapshot>>,
    pub lint_messages: Vec<query_tools::LintMessage>,
    pub show_lint_panel: bool,
    // Lint panel auto-hide timer
    pub lint_panel_shown_at: Option<std::time::Instant>,
    pub lint_panel_auto_hide_ms: u64,
    pub lint_panel_pinned: bool,
    pub auto_format_on_execute: bool,
    // Auto-refresh execute from history
    pub auto_refresh_active: bool,
    pub auto_refresh_interval_seconds: u32,
    pub auto_refresh_last_run: Option<std::time::Instant>,
    pub auto_refresh_query: Option<String>,
    pub auto_refresh_connection_id: Option<i64>,
    pub show_auto_refresh_dialog: bool,
    pub auto_refresh_interval_input: String,
    // Query execution message panel (similar to TablePlus message tab)
    pub query_message: String,
    pub query_message_is_error: bool,
    pub show_message_panel: bool,
    pub message_panel_height: f32, // Height of message panel in pixels
    pub query_message_display_buffer: String, // Buffer for TextEdit to maintain selection state
    // Custom Views state
    pub show_add_view_dialog: bool,
    pub new_view_name: String,
    pub new_view_query: String,
    pub new_view_connection_id: Option<i64>,
    pub edit_view_original_name: Option<String>,
    
    pub global_backspace_pressed: bool,
    pub sidebar_visible: bool,
    
    // Replication dialog state
    pub show_add_replication_dialog: bool,
    pub replication_dialog: Option<crate::models::structs::ReplicationDialogState>,
    pub replication_setup_receiver: Option<std::sync::mpsc::Receiver<Result<String, String>>>,

    // Background fetch tracking
    pub fetching_databases: std::collections::HashSet<i64>,
}

// Preference tabs enumeration
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PrefTab {
    ApplicationTheme,
    EditorTheme,
    Performance,
    DataDirectory,
    Update,
}

impl Default for Tabular {
    fn default() -> Self {
        Self::new()
    }
}

