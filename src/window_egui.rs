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

// Grouped parameters for render_tree_node_with_table_expansion to keep call sites tidy
struct RenderTreeNodeParams<'a> {
    node_index: usize,
    refreshing_connections: &'a std::collections::HashSet<i64>,
    connection_pools: &'a std::collections::HashMap<i64, models::enums::DatabasePool>,
    pending_connection_pools: &'a std::collections::HashSet<i64>,
    shared_connection_pools:
        &'a Arc<std::sync::Mutex<std::collections::HashMap<i64, models::enums::DatabasePool>>>,
    is_search_mode: bool,
    // New: fallback map of connection_id -> DatabaseType for DB type detection when pool not ready
    connection_types: &'a std::collections::HashMap<i64, models::enums::DatabaseType>,
    // Prefetch progress tracking
    prefetch_progress: &'a HashMap<i64, (usize, usize)>,
}

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
    pub is_dark_mode: bool,
    pub link_editor_theme: bool, // when true editor theme follows app theme
    // Settings window visibility
    pub show_settings_window: bool,
    // Database search functionality
    pub database_search_text: String,
    pub filtered_items_tree: Vec<models::structs::TreeNode>,
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
    
    // DEBUGGING INPUT
    pub global_backspace_pressed: bool,
    pub sidebar_visible: bool,
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

impl Tabular {
    // Multi-cursor: add a new cursor position if not existing
    pub fn add_cursor(&mut self, pos: usize) {
        let p = pos.min(self.editor.text.len());
        if !self.extra_cursors.contains(&p) {
            self.extra_cursors.push(p);
            self.extra_cursors.sort_unstable();
        }
        // Transitional mirror to structured selections
        if self.multi_selection.is_empty() {
            self.multi_selection
                .add_collapsed(self.cursor_position.min(self.editor.text.len()));
        }
        self.multi_selection.add_collapsed(p);
        log::debug!(
            "[multi] add_cursor pos={} extra_cursors={:?} regions={:?}",
            p,
            self.extra_cursors,
            self.multi_selection.ranges()
        );
    }

    pub fn clear_extra_cursors(&mut self) {
        self.extra_cursors.clear();
    }

    pub fn stop_auto_refresh(&mut self) {
        self.auto_refresh_active = false;
        self.auto_refresh_query = None; // Reset the query to None
        self.auto_refresh_connection_id = None;
        self.auto_refresh_last_run = None;
    }

    /// Set initial preferences loaded from startup
    pub fn set_initial_prefs(&mut self, prefs: crate::config::AppPreferences) {
        self.is_dark_mode = prefs.is_dark_mode;
        self.link_editor_theme = prefs.link_editor_theme;
        self.advanced_editor.theme = match prefs.editor_theme.as_str() {
            "GITHUB_LIGHT" => crate::models::structs::EditorColorTheme::GithubLight,
            "GRUVBOX" => crate::models::structs::EditorColorTheme::Gruvbox,
            _ => crate::models::structs::EditorColorTheme::GithubDark,
        };
        self.advanced_editor.font_size = prefs.font_size;
        self.advanced_editor.word_wrap = prefs.word_wrap;
        if let Some(dir) = prefs.data_directory.clone() {
            self.data_directory = dir;
        }
        self.auto_check_updates = prefs.auto_check_updates;
        self.use_server_pagination = prefs.use_server_pagination;
        self.enable_debug_logging = prefs.enable_debug_logging;

        // Store as last saved
        self.last_saved_prefs = Some(prefs);
        self.prefs_loaded = true;
    }

    // Duplicate selected row for editing


    // Delete selected row


    // End: Spreadsheet helpers
    // Ensure a shared Tokio runtime exists (lazy init) to avoid spawning many runtimes
    fn get_runtime(&mut self) -> Arc<tokio::runtime::Runtime> {
        if self.runtime.is_none() {
            // Multi-threaded runtime so we can run blocking DB IO without freezing UI thread completely
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(4)
                .thread_name("tabular-rt")
                .build()
                .expect("Failed to build global runtime");
            self.runtime = Some(Arc::new(rt));
            debug!("🌐 Global runtime initialized");
        }
        self.runtime.as_ref().unwrap().clone()
    }

    // Small painter-drawn triangle toggle to avoid font glyph issues
    fn triangle_toggle(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
        let size = egui::vec2(16.0, 16.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            let color = ui.visuals().text_color();
            let stroke = egui::Stroke { width: 1.0, color };
            if expanded {
                // Down triangle
                let p1 = egui::pos2(rect.center().x - 6.0, rect.top() + 5.0);
                let p2 = egui::pos2(rect.center().x + 6.0, rect.top() + 5.0);
                let p3 = egui::pos2(rect.center().x, rect.top() + 11.0);
                painter.add(egui::Shape::convex_polygon(vec![p1, p2, p3], color, stroke));
            } else {
                // Right triangle
                let p1 = egui::pos2(rect.left() + 5.0, rect.center().y - 6.0);
                let p2 = egui::pos2(rect.left() + 5.0, rect.center().y + 6.0);
                let p3 = egui::pos2(rect.left() + 11.0, rect.center().y);
                painter.add(egui::Shape::convex_polygon(vec![p1, p2, p3], color, stroke));
            }
        }

        response
    }

    pub fn new() -> Self {
        // Create background processing channels
        let (background_sender, background_receiver) =
            mpsc::channel::<models::enums::BackgroundTask>();
        let (result_sender, result_receiver) = mpsc::channel::<models::enums::BackgroundResult>();
        let (query_result_sender, query_result_receiver) =
            mpsc::channel::<connection::QueryResultMessage>();

        // Create shared runtime for all database operations
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => Some(Arc::new(rt)),
            Err(e) => {
                error!("Failed to create runtime: {}", e);
                None
            }
        };

        // Initialize ConfigStore
        let config_store = if let Some(rt) = &runtime {
            rt.block_on(async {
                crate::config::ConfigStore::new().await.ok()
            })
        } else {
            None
        };

        let mut app = Self {
            editor: EditorBuffer::new(""),
            multi_selection: crate::editor_selection::MultiSelection::new(),
            selected_menu: "Database".to_string(),
            items_tree: Vec::new(),
            queries_tree: Vec::new(),
            history_tree: Vec::new(),
            history_items: Vec::new(),
            connections: Vec::new(),
            show_add_connection: false,
            new_connection: models::structs::ConnectionConfig::default(),
            db_pool: None,
            runtime,
            connection_pools: HashMap::new(), // Start with empty cache
            pending_connection_pools: std::collections::HashSet::new(), // Track pools being created
            shared_connection_pools: Arc::new(std::sync::Mutex::new(HashMap::new())), // Shared pools for background tasks
            pending_pool_log_last: HashMap::new(),
            prefetch_progress: HashMap::new(),
            prefetch_in_progress: std::collections::HashSet::new(),
            show_edit_connection: false,
            edit_connection: models::structs::ConnectionConfig::default(),
            needs_refresh: false,
            current_table_data: Vec::new(),
            current_table_headers: Vec::new(),
            current_table_name: String::new(),
            current_object_ddl: None,
            current_connection_id: None,
            current_page: 0,
            page_size: 500, // Default 500 rows per page
            total_rows: 0,
            all_table_data: Vec::new(),
            // Server-side pagination
            use_server_pagination: true, // Enable by default for better performance
            actual_total_rows: None,
            current_base_query: String::new(),
            table_split_ratio: 0.6, // Default 60% for editor, 40% for table
            sort_column: None,
            sort_ascending: true,
            test_connection_status: None,
            test_connection_in_progress: false,
            background_sender: Some(background_sender),
            background_receiver: Some(result_receiver),
            query_result_sender,
            query_result_receiver,
            active_query_jobs: std::collections::HashMap::new(),
            active_query_handles: std::collections::HashMap::new(),
            cancelled_query_jobs: std::collections::HashMap::new(),
            pending_paginated_jobs: std::collections::HashSet::new(),
            next_query_job_id: 1,
            refreshing_connections: std::collections::HashSet::new(),
            pending_expansion_restore: std::collections::HashMap::new(),
            pending_auto_load: std::collections::HashSet::new(),
            query_tabs: Vec::new(),
            active_tab_index: 0,
            next_tab_id: 1,
            show_save_dialog: false,
            save_filename: String::new(),
            save_directory: String::new(),
            save_directory_picker_result: None,
            show_connection_selector: false,
            pending_query: String::new(),
            auto_execute_after_connection: false,
            query_execution_in_progress: false,
            query_icon_hold_until: None,
            error_message: String::new(),
            show_error_message: false,
            advanced_editor: models::structs::AdvancedEditor::default(),
            selected_text: String::new(),
            clipboard_multi_segments: None,
            clipboard_multi_regions: None,
            clipboard_multi_version: None,
            cursor_position: 0,
            selection_start: 0,
            selection_end: 0,
            show_command_palette: false,
            command_palette_input: String::new(),
            show_theme_selector: false,
            command_palette_items: Vec::new(),
            command_palette_selected_index: 0,
            theme_selector_selected_index: 0,
            request_theme_selector: false,
            // Dark / Light UI theme setting (default dark)
            is_dark_mode: true,
            link_editor_theme: true,
            show_settings_window: false,
            // Database search functionality
            database_search_text: String::new(),
            filtered_items_tree: Vec::new(),
            show_search_results: false,
            history_search_text: String::new(),
            filtered_history_tree: Vec::new(),
            // Query folder management
            show_create_folder_dialog: false,
            new_folder_name: String::new(),
            selected_query_for_move: None,
            show_move_to_folder_dialog: false,
            target_folder_name: String::new(),
            parent_folder_for_creation: None,
            selected_folder_for_removal: None,
            folder_removal_map: std::collections::HashMap::new(),
            show_create_table_dialog: false,
            create_table_wizard: None,
            create_table_error: None,
            last_cleanup_time: std::time::Instant::now(),
            selected_row: None,
            selected_cell: None,
            selected_rows: BTreeSet::new(),
            selected_columns: BTreeSet::new(),
            last_clicked_row: None,
            last_clicked_column: None,
            table_recently_clicked: false,
            table_sel_anchor: None,
            table_dragging: false,
            scroll_to_selected_cell: false,
            // Column width management
            column_widths: Vec::new(),
            min_column_width: 50.0,
            // Gear menu and about dialog
            show_about_dialog: false,
            // Logo texture
            logo_texture: None,
            // Database cache for performance
            database_cache: std::collections::HashMap::new(),
            database_cache_time: std::collections::HashMap::new(),
            // Autocomplete
            show_autocomplete: false,
            autocomplete_suggestions: Vec::new(),
            autocomplete_kinds: Vec::new(),
            autocomplete_notes: Vec::new(),
            autocomplete_payloads: Vec::new(),
            selected_autocomplete_index: 0,
            autocomplete_prefix: String::new(),
            last_autocomplete_trigger_len: 0,
            pending_cursor_set: None,
            editor_focus_boost_frames: 0,
            autocomplete_expected_cursor: None,
            autocomplete_protection_frames: 0,
            autocomplete_navigated: false,
            autocomplete_last_update: None,
            autocomplete_debounce_ms: 120,
            selection_force_clear: false,
            // Index dialog defaults
            show_index_dialog: false,
            index_dialog: None,
            table_bottom_view: models::structs::TableBottomView::default(),
            structure_columns: Vec::new(),
            structure_indexes: Vec::new(),
            structure_selected_row: None,
            structure_selected_cell: None,
            structure_sel_anchor: None,
            structure_dragging: false,
            pending_drop_index_name: None,
            pending_drop_index_stmt: None,
            pending_drop_column_name: None,
            pending_drop_column_stmt: None,
            pending_drop_collection: None,
            pending_drop_table: None,
            structure_col_widths: Vec::new(),
            structure_idx_col_widths: Vec::new(),
            structure_sub_view: models::structs::StructureSubView::Columns,
            last_structure_target: None,
            request_structure_refresh: false,
            adding_column: false,
            new_column_name: String::new(),
            new_column_type: String::new(),
            new_column_nullable: true,
            new_column_default: String::new(),
            editing_column: false,
            edit_column_original_name: String::new(),
            edit_column_name: String::new(),
            edit_column_type: String::new(),
            edit_column_nullable: true,
            edit_column_default: String::new(),
            adding_index: false,
            new_index_name: String::new(),
            new_index_method: String::new(),
            new_index_unique: false,
            new_index_columns: String::new(),
            sql_filter_text: String::new(),
            is_table_browse_mode: false,
            config_store,
            last_saved_prefs: None,
            prefs_dirty: false,
            prefs_save_feedback: None,
            prefs_last_saved_at: None,
            prefs_loaded: false,
            // Data directory settings
            data_directory: crate::config::get_data_dir().to_string_lossy().to_string(),
            temp_data_directory: String::new(),
            show_directory_picker: false,
            directory_picker_result: None,
            sqlite_path_picker_result: None,
            temp_sqlite_path: None,
            // Self-update settings
            update_info: None,
            show_update_dialog: false,
            update_check_in_progress: false,
            update_check_error: None,
            last_update_check: None,
            update_download_in_progress: false,
            auto_check_updates: true,
            manual_update_check: false,
            show_update_notification: false,
            update_download_started: false,
            update_installed: false,
            update_install_receiver: None,
            enable_debug_logging: false, // Default to false
            auto_updater: crate::auto_updater::AutoUpdater::new().ok(),
            settings_active_pref_tab: PrefTab::ApplicationTheme,
            show_settings_menu: false,
            // Pool-wait defaults
            pool_wait_in_progress: false,
            pool_wait_connection_id: None,
            pool_wait_query: String::new(),
            pool_wait_started_at: None,
            // Spreadsheet editing state
            spreadsheet_state: crate::models::structs::SpreadsheetState::default(),
            extra_cursors: Vec::new(),
            last_editor_text: String::new(),
            highlight_cache: std::collections::HashMap::new(),
            last_highlight_hash: None,
            suppress_editor_arrow_once: false,
            sql_semantic_snapshot: None,
            // Context menu for row operations
            show_row_context_menu: false,
            context_menu_row: None,
            context_menu_just_opened: false,
            context_menu_pos: egui::Pos2::ZERO,
            newly_created_rows: std::collections::HashSet::new(),
            // Query AST debug defaults
            show_query_ast_debug: false,
            last_compiled_sql: None,
            last_compiled_headers: Vec::new(),
            last_debug_plan: None,
            last_cache_hits: 0,
            last_cache_misses: 0,
            last_plan_hash: None,
            last_plan_cache_key: None,
            last_ctes: None,
            lint_messages: Vec::new(),
            show_lint_panel: false,
            lint_panel_shown_at: None,
            lint_panel_auto_hide_ms: 2_000,
            lint_panel_pinned: false,
            auto_format_on_execute: false,
            auto_refresh_active: false,
            auto_refresh_interval_seconds: 1,
            auto_refresh_last_run: None,
            auto_refresh_query: None,
            auto_refresh_connection_id: None,
            show_auto_refresh_dialog: false,
            auto_refresh_interval_input: String::new(),
            // Query message panel
            query_message: String::new(),
            query_message_is_error: false,
            show_message_panel: false,
            message_panel_height: 100.0,
            query_message_display_buffer: String::new(),
            show_add_view_dialog: false,
            new_view_name: String::new(),
            new_view_query: String::new(),
            new_view_connection_id: None,
            edit_view_original_name: None,
            global_backspace_pressed: false,
            sidebar_visible: true,
        };

        // Clear any old cached pools
        app.connection_pools.clear();

        // Initialize database and sample data FIRST
        sidebar_database::initialize_database(&mut app);
        sidebar_database::initialize_sample_data(&mut app);

        // Load saved queries from directory
        sidebar_query::load_queries_from_directory(&mut app);

        // Create initial query tab
        editor::create_new_tab(&mut app, "Untitled Query".to_string(), String::new());

        // Start background thread AFTER database is initialized
        app.start_background_worker(background_receiver, result_sender);

        // Do NOT force an immediate update check here; let the preference load path enforce 24h throttling.
        // If preferences haven't been loaded yet (first run path) we can perform a very conservative check:
        // Only queue if default auto_check_updates is true AND no persisted timestamp available or older than 24h.
        // if app.auto_check_updates
        //     && let (Some(sender), Some(rt)) = (&app.background_sender, &app.runtime)
        //     && let Ok(store) = rt.block_on(crate::config::ConfigStore::new())
        // {
        //     println!("Checking last update check timestamp for initial check...");
        //     let mut should_queue = true;
        //     if let Some(last_iso) = rt.block_on(store.get_last_update_check())
        //         && let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&last_iso)
        //     {
        //         let last_utc = parsed.with_timezone(&chrono::Utc);
        //         if chrono::Utc::now().signed_duration_since(last_utc) < chrono::Duration::days(1) {
        //             should_queue = false;
        //         }
        //     }
        //     if should_queue {
        //         // Persist immediately to prevent multiple queues in rapid restarts
        //         rt.block_on(store.set_last_update_check_now());
        //         let _ = sender.send(models::enums::BackgroundTask::CheckForUpdates);
        //     }
        // }
        app
    }

    fn start_background_worker(
        &self,
        task_receiver: Receiver<models::enums::BackgroundTask>,
        result_sender: Sender<models::enums::BackgroundResult>,
    ) {
        // Spawn a background thread to process queued tasks
        // Clone cache DB pool for use inside the worker
        let cache_pool = self.db_pool.clone();
        std::thread::spawn(move || {
            while let Ok(task) = task_receiver.recv() {
                match task {
                    models::enums::BackgroundTask::RefreshConnection { connection_id } => {
                        // Perform actual refresh and cache preload on a lightweight runtime
                        let success = if let Some(cache_pool_arc) = &cache_pool {
                            match tokio::runtime::Runtime::new() {
                                Ok(rt) => rt.block_on(
                                    crate::connection::refresh_connection_background_async(
                                        connection_id,
                                        &Some(cache_pool_arc.clone()),
                                    ),
                                ),
                                Err(_) => false,
                            }
                        } else {
                            false
                        };
                        let _ =
                            result_sender.send(models::enums::BackgroundResult::RefreshComplete {
                                connection_id,
                                success,
                            });
                    }
                    models::enums::BackgroundTask::CheckForUpdates => {
                        // Perform update check on a lightweight runtime (if required by async API)
                        let result = if let Ok(rt) = tokio::runtime::Runtime::new() {
                            rt.block_on(crate::self_update::check_for_updates())
                                .map_err(|e| e.to_string())
                        } else {
                            Err("Failed to create runtime for update check".to_string())
                        };
                        let _ = result_sender
                            .send(models::enums::BackgroundResult::UpdateCheckComplete { result });
                    }
                    models::enums::BackgroundTask::StartPrefetch {
                        connection_id,
                        show_progress: _,
                    } => {
                        // Start optional background prefetch with progress tracking
                        if let Some(_cache_pool_arc) = &cache_pool {
                            // Need to get connection config and pool
                            // This is a bit tricky since we're in background thread
                            // We'll need to pass the necessary data or fetch from cache
                            let _ = result_sender.send(
                                models::enums::BackgroundResult::PrefetchComplete { connection_id },
                            );
                        }
                    }
                    models::enums::BackgroundTask::PickSqlitePath => {
                        // Open picker on this background thread and send result back
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Pilih File / Folder SQLite")
                            .pick_folder()
                        {
                            let _ = result_sender.send(
                                models::enums::BackgroundResult::SqlitePathPicked {
                                    path: path.to_string_lossy().to_string(),
                                },
                            );
                        }
                    }
                }
            }
        });
    }

    fn handle_query_result_message(&mut self, message: connection::QueryResultMessage) {
        self.prune_cancelled_jobs();
        self.active_query_handles.remove(&message.job_id);

        if self.cancelled_query_jobs.remove(&message.job_id).is_some() {
            self.pending_paginated_jobs.remove(&message.job_id);
            if self.active_query_jobs.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }
            return;
        }

        if let Some(status) = self.active_query_jobs.get_mut(&message.job_id) {
            status.completed = true;
        }
        self.active_query_jobs.remove(&message.job_id);

        let was_paginated = self.pending_paginated_jobs.remove(&message.job_id);

        if let Some(ast_sql) = message.ast_debug_sql.clone() {
            self.last_compiled_sql = Some(ast_sql);
        }
        if let Some(ast_headers) = message.ast_headers.clone() {
            self.last_compiled_headers = ast_headers;
        }

        // Update query message panel
        if message.success {
            let duration_ms = message.duration.as_millis();
            let row_count = message.affected_rows.unwrap_or(message.rows.len());
            self.query_message = format!(
                "Query executed successfully in {}.{:03}s • {} row(s) affected",
                duration_ms / 1000,
                duration_ms % 1000,
                row_count
            );
            self.query_message_is_error = false;
            // Auto-switch to Data tab to show results
            self.table_bottom_view = models::structs::TableBottomView::Data;
        } else {
            let error_msg = message.error.clone().unwrap_or_else(|| "Unknown error".to_string());
            self.query_message = format!("Error: {}", error_msg);
            self.query_message_is_error = true;
            // Auto-switch to Messages tab to show error
            self.table_bottom_view = models::structs::TableBottomView::Messages;
        }
        self.show_message_panel = true;

        // Update active tab message
        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.query_message = self.query_message.clone();
            active_tab.query_message_is_error = self.query_message_is_error;
        }

        if was_paginated && message.success {
            self.apply_paginated_query_result(&message);
            return;
        }
        // For errors, fall through to regular handler to reuse error display logic.

        let result_tuple = Some((message.headers.clone(), message.rows.clone()));
        editor::process_query_result(self, &message.query, message.connection_id, result_tuple);
    }

    fn apply_paginated_query_result(&mut self, message: &connection::QueryResultMessage) {
        self.current_table_headers = message.headers.clone();
        self.current_table_data = message.rows.clone();
        self.all_table_data = self.current_table_data.clone();
        self.total_rows = self.current_table_data.len();

        if self.total_rows == 0 {
            self.current_table_name = format!(
                "Query Results (page {} empty)",
                self.current_page.saturating_add(1)
            );
        } else {
            self.current_table_name = format!(
                "Query Results (page {} showing {} rows)",
                self.current_page.saturating_add(1),
                self.current_table_data.len()
            );
        }

        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.result_headers = self.current_table_headers.clone();
            active_tab.result_rows = self.current_table_data.clone();
            active_tab.result_all_rows = self.current_table_data.clone();
            active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
            active_tab.current_page = self.current_page;
            active_tab.page_size = self.page_size;
            active_tab.is_table_browse_mode = self.is_table_browse_mode;
            active_tab.base_query = self.current_base_query.clone();
            active_tab.result_table_name = self.current_table_name.clone();
        }

        self.query_execution_in_progress = false;
        self.extend_query_icon_hold();
    }

    pub fn set_active_tab_connection_with_database(
        &mut self,
        connection_id: Option<i64>,
        database_name: Option<String>,
    ) {
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.connection_id = connection_id;
            tab.database_name = database_name;
        }

        // Eagerly open the connection pool when a connection is assigned to the active tab.
        // This restores previous behavior where opening a query file (with embedded connection_id)
        // would ensure the connection is ready before the user executes a query.
        if let Some(cid) = connection_id {
            // Update global current_connection_id so other components (e.g. spreadsheet) pick it up
            self.current_connection_id = Some(cid);

            // Skip if we already have a pool or it's being created
            let already_has_pool = self.connection_pools.contains_key(&cid);
            let already_pending = self.pending_connection_pools.contains(&cid);
            if !already_has_pool && !already_pending {
                // Use (or create) the shared runtime to synchronously kick off pool creation.
                // We block only for the quick-attempt path inside get_or_create_connection_pool;
                // if it becomes a background creation it will return fast.
                let rt = self.get_runtime();
                rt.block_on(async {
                    let _ = crate::connection::get_or_create_connection_pool(self, cid).await;
                });
            }
        }
    }

    fn render_lint_panel(&mut self, ui: &mut egui::Ui) {
        if self.lint_messages.is_empty() {
            return;
        }

        ui.add_space(6.0);
        let count = self.lint_messages.len();
        let plural = if count == 1 { "" } else { "s" };

        if !self.show_lint_panel {
            ui.horizontal(|ui| {
                let warning_text =
                    egui::RichText::new(format!("⚠ {} lint issue{} detected", count, plural))
                        .color(egui::Color32::from_rgb(255, 183, 0));
                ui.label(warning_text);
                if ui.button("Show details").clicked() {
                    self.show_lint_panel = true;
                    self.lint_panel_shown_at = Some(std::time::Instant::now());
                }
            });
            return;
        }

        // Panel is shown: start timer if needed (hover/pin logic handled after rendering)
        if self.lint_panel_shown_at.is_none() {
            self.lint_panel_shown_at = Some(std::time::Instant::now());
        }

        let panel_fill = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(40, 40, 40)
        } else {
            egui::Color32::from_rgb(255, 244, 234)
        };

        let inner = egui::Frame::group(ui.style())
            .fill(panel_fill)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 30, 0)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Lint ({})", count)).strong());
                    if ui.button("Hide").clicked() {
                        self.show_lint_panel = false;
                        self.lint_panel_shown_at = None;
                    }
                    ui.add_space(12.0);
                    ui.checkbox(&mut self.lint_panel_pinned, "Pin (keep open)");
                    ui.checkbox(
                        &mut self.auto_format_on_execute,
                        "Auto-format before execute",
                    );
                    if ui.button("Format now").clicked()
                        && let Some(formatted) = query_tools::format_sql(&self.editor.text)
                        && formatted != self.editor.text
                    {
                        self.editor.set_text(formatted.clone());
                        let new_len = self.editor.text.len();
                        self.cursor_position = new_len;
                        self.multi_selection.clear();
                        self.multi_selection.add_collapsed(self.cursor_position);
                        self.last_editor_text = self.editor.text.clone();
                        self.lint_messages = query_tools::lint_sql(&self.editor.text);
                        self.show_lint_panel = !self.lint_messages.is_empty();
                        if self.show_lint_panel {
                            self.lint_panel_shown_at = Some(std::time::Instant::now());
                        } else {
                            self.lint_panel_shown_at = None;
                        }
                        self.editor_focus_boost_frames = self.editor_focus_boost_frames.max(4);
                        self.pending_cursor_set = Some(self.cursor_position);
                    }
                });

                ui.separator();

                for msg in &self.lint_messages {
                    let (icon, color) = match msg.severity {
                        query_tools::LintSeverity::Info => {
                            ("ℹ", egui::Color32::from_rgb(120, 170, 255))
                        }
                        query_tools::LintSeverity::Warning => {
                            ("⚠", egui::Color32::from_rgb(255, 183, 0))
                        }
                        query_tools::LintSeverity::Error => {
                            ("⛔", egui::Color32::from_rgb(255, 80, 80))
                        }
                    };

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icon).color(color).strong());
                        ui.label(egui::RichText::new(&msg.message));
                    });

                    if let Some(hint) = &msg.hint {
                        ui.label(egui::RichText::new(hint).small().italics().weak());
                    }

                    if let Some(span) = &msg.span {
                        ui.label(
                            egui::RichText::new(format!("range {}..{}", span.start, span.end))
                                .small()
                                .weak(),
                        );
                    }

                    ui.add_space(4.0);
                }
            });

        // After rendering, handle auto-hide with hover/pin behavior
        if self.show_lint_panel {
            // If pinned, do not auto-hide
            if self.lint_panel_pinned {
                self.lint_panel_shown_at = Some(std::time::Instant::now());
            } else {
                // If hovered, refresh timer to prevent hiding while interacting
                if inner.response.hovered() {
                    self.lint_panel_shown_at = Some(std::time::Instant::now());
                }
                // Check elapsed when not hovered
                if let Some(shown_at) = self.lint_panel_shown_at {
                    let elapsed_ms = shown_at.elapsed().as_millis() as u64;
                    if elapsed_ms >= self.lint_panel_auto_hide_ms {
                        self.show_lint_panel = false;
                        self.lint_panel_shown_at = None;
                    }
                }
            }
        }
    }

    fn render_messages_content(&mut self, ui: &mut egui::Ui) {
        if self.query_message.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    egui::RichText::new("No messages")
                        .size(16.0)
                        .weak()
                );
            });
            return;
        }

        // Full-height messages view
        egui::ScrollArea::vertical()
            .id_salt("messages_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);

                let (text_color, icon) = if self.query_message_is_error {
                    // Error styling
                    if ui.visuals().dark_mode {
                        (egui::Color32::from_rgb(255, 120, 120), "❌")
                    } else {
                        (egui::Color32::from_rgb(180, 40, 40), "❌")
                    }
                } else {
                    // Success styling
                    if ui.visuals().dark_mode {
                        (egui::Color32::from_rgb(120, 220, 120), "👍")
                    } else {
                        (egui::Color32::from_rgb(40, 140, 40), "👍")
                    }
                };

                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(icon)
                            .size(20.0)
                            .color(text_color)
                    );
                    
                    ui.spacing_mut().item_spacing.x = 8.0;
                    
                    // Sync display buffer with actual message if they differ
                    if self.query_message_display_buffer != self.query_message {
                        self.query_message_display_buffer = self.query_message.clone();
                    }
                    
                    // Use TextEdit with persistent buffer for selection state
                    // Use absolute ID so we can check focus in copy handler
                    let message_text_id = egui::Id::new("tabular_message_text_edit_widget");
                    let output = egui::TextEdit::multiline(&mut self.query_message_display_buffer)
                        .id(message_text_id)
                        .desired_width(f32::INFINITY)
                        .text_color(text_color)
                        .font(egui::TextStyle::Body)
                        .frame(false)
                        .interactive(true)
                        .show(ui);
                    
                    // Request focus when clicked to ensure CMD+C works
                    if output.response.clicked() {
                        output.response.request_focus();
                    }
                    
                    // Manual copy handling for CMD+C in message TextEdit
                    if output.response.has_focus() {
                        ui.input(|i| {
                            let copy_event = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                            let key_combo = (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::C);
                            
                            if copy_event || key_combo {
                                // Get cursor range to find selected text
                                if let Some(state) = egui::TextEdit::load_state(ui.ctx(), message_text_id)
                                    && let Some(cursor_range) = state.cursor.char_range() {
                                    let start = cursor_range.primary.index;
                                    let end = cursor_range.secondary.index;
                                    let (min, max) = if start < end { (start, end) } else { (end, start) };
                                    
                                    if min < max && max <= self.query_message_display_buffer.len() {
                                        let selected_text = &self.query_message_display_buffer[min..max];
                                        ui.ctx().copy_text(selected_text.to_string());
                                        debug!("📋 Copied selected text from message: {} chars", selected_text.len());
                                    }
                                }
                            }
                        });
                    }
                    
                    // Don't sync changes back - keep it read-only
                    // But preserve selection state by keeping the buffer
                    
                    // Context menu on right-click
                    output.response.context_menu(|ui| {
                        if ui.button("📋 Copy Text").clicked() {
                            ui.ctx().copy_text(self.query_message.clone());
                            ui.close();
                        }
                    });
                });

                ui.add_space(8.0);
            });
    }

    // Custom View Dialog
    fn render_add_view_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let show_dialog = self.show_add_view_dialog;
        
        if show_dialog {
            // Log raw input events
            ctx.input(|i| {
                if i.key_pressed(egui::Key::Backspace) {
                    println!("🔍 [Dialog] Backspace key detected in raw input");
                }
                if !i.events.is_empty() {
                    println!("🔍 [Dialog] Input events count: {}", i.events.len());
                }
            });
                        


            let title = if self.edit_view_original_name.is_some() { "Edit Custom View" } else { "Add Custom View" };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(true)
                .default_size([600.0, 400.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Name:");
                    let before_len = self.new_view_name.len();
                    let name_edit = egui::TextEdit::singleline(&mut self.new_view_name)
                        .desired_width(f32::INFINITY);
                    
                    let name_response = ui.add(name_edit);
                    let after_len = self.new_view_name.len();
                    
                    println!("🔍 [Name Field] Before len: {}, After len: {}", before_len, after_len);
                    println!("🔍 [Name Field] Has focus: {}, changed: {}, lost_focus: {}", 
                        name_response.has_focus(), name_response.changed(), name_response.lost_focus());
                    
                    // Request focus on the name field when dialog first opens
                    if ui.memory(|mem| mem.focused().is_none()) {
                        println!("🔍 [Name Field] Requesting focus (first open)");
                        name_response.request_focus();
                    }

                    ui.add_space(8.0);
                    ui.label("SQL Query:");
                    
                    let before_query_len = self.new_view_query.len();
                    let query_edit = egui::TextEdit::multiline(&mut self.new_view_query)
                        .desired_width(f32::INFINITY)
                        .desired_rows(10);
                    
                    let query_response = ui.add(query_edit);
                    let after_query_len = self.new_view_query.len();
                    
                    println!("🔍 [Query Field] Before len: {}, After len: {}", before_query_len, after_query_len);
                    println!("🔍 [Query Field] Has focus: {}, changed: {}", 
                        query_response.has_focus(), query_response.changed());

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked()
                             && !self.new_view_name.is_empty() && !self.new_view_query.is_empty()
                                 && let Some(conn_id) = self.new_view_connection_id {
                                     // Save logic
                                     if let Some(conn_idx) = self.connections.iter().position(|c| c.id == Some(conn_id)) {
                                         let mut conn = self.connections[conn_idx].clone();
                                         let new_view = models::structs::CustomView {
                                             name: self.new_view_name.clone(),
                                             query: self.new_view_query.clone(),
                                         };
                                         
                                         if let Some(original_name) = &self.edit_view_original_name {
                                             // Edit mode: find and update
                                             if let Some(view_idx) = conn.custom_views.iter().position(|v| v.name == *original_name) {
                                                 conn.custom_views[view_idx] = new_view;
                                             } else {
                                                 // Should not happen normally, but treat as new if not found
                                                 conn.custom_views.push(new_view);
                                             }
                                         } else {
                                             // Add mode: append
                                             conn.custom_views.push(new_view);
                                         }
                                         
                                         // Update database
                                         if crate::sidebar_database::update_connection_in_database(self, &conn) {
                                             // Update in-memory
                                              self.connections[conn_idx] = conn;
                                              // Trigger refresh
                                              crate::sidebar_database::refresh_connections_tree(self);
                                              self.show_add_view_dialog = false;
                                         } else {
                                             // Handle error (maybe show toast/log)
                                             log::error!("Failed to save custom view to database");
                                         }
                                     }
                                 }
                        if ui.button("Cancel").clicked() {
                            self.show_add_view_dialog = false;
                        }
                    });
                });
        }

        if !open {
            self.show_add_view_dialog = false;
        }
    }

    /// Consolidated rendering of query editor with split results panel
    /// Used by both View Query mode and regular query tabs to avoid duplication
    fn render_query_editor_with_split(
        &mut self,
        ui: &mut egui::Ui,
        context_id: &str, // "view_query" or "regular_query"
    ) {
        let avail = ui.available_height();
        let executed = self
            .query_tabs
            .get(self.active_tab_index)
            .map(|t| t.has_executed_query)
            .unwrap_or(false);
        let has_headers = !self.current_table_headers.is_empty();
        let has_message = !self.current_table_name.is_empty();
        let show_bottom = has_headers || has_message || executed;

        if show_bottom {
            self.table_split_ratio = self.table_split_ratio.clamp(0.05, 0.995);
        }

        let editor_h = if show_bottom {
            let mut h = avail * self.table_split_ratio;
            if has_headers {
                h = h.clamp(100.0, (avail - 50.0).max(100.0));
            } else {
                h = h.clamp(140.0, (avail - 30.0).max(140.0));
            }
            h
        } else {
            avail
        };

        egui::Frame::NONE
            .fill(if ui.visuals().dark_mode {
                egui::Color32::from_rgb(30, 30, 30)
            } else {
                egui::Color32::WHITE
            })
            .show(ui, |ui| {
                let editor_area_height = editor_h.max(200.0);
                let mono_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
                let rows = ((editor_area_height / mono_h).floor() as i32) as usize;
                self.advanced_editor.desired_rows = rows;

                let avail_w = ui.available_width() - 4.0;
                let desired = egui::vec2(avail_w, editor_area_height);
                let (rect, _resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
                let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));

                egui::ScrollArea::vertical()
                    .id_salt(format!("query_editor_scroll_{}", context_id))
                    .auto_shrink([false, false])
                    .show(&mut child_ui, |ui| {
                        ui.set_min_width(avail_w - 4.0);
                        editor::render_advanced_editor(self, ui);
                    });

                // Floating execute button
                let button_margin = 4.0;
                let button_size = egui::vec2(32.0, 32.0);
                let button_pos = egui::pos2(
                    rect.max.x - button_size.x - button_margin,
                    rect.min.y + button_margin,
                );
                let play_fill = egui::Color32::TRANSPARENT;
                let is_loading = self.query_execution_in_progress || self.pool_wait_in_progress;
                let (play_icon, play_color, play_border, tooltip_text) = if is_loading {
                    ("⏳", egui::Color32::WHITE, egui::Color32::TRANSPARENT, "Executing query…")
                } else {
                    ("▶", egui::Color32::GREEN, egui::Color32::TRANSPARENT, "CMD+Enter to execute")
                };
                let play_text = egui::RichText::new(play_icon).color(play_color).size(18.0);
                let button_corner = (button_size.y / 2.0).round().clamp(2.0, u8::MAX as f32) as u8;

                let mut execute_clicked = false;
                let mut captured_selection_text = String::new();

                // Auto-execute if requested by the tab (e.g. Custom View opened)
                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                    && tab.should_run_on_open {
                        execute_clicked = true;
                        tab.should_run_on_open = false;
                        self.query_execution_in_progress = true;
                    }

                egui::Area::new(egui::Id::new((format!("floating_execute_button_{}", context_id), self.active_tab_index)))
                    .order(egui::Order::Foreground)
                    .fixed_pos(button_pos)
                    .show(ui.ctx(), |area_ui| {
                        let mut button = egui::Button::new(play_text.clone())
                            .fill(play_fill)
                            .stroke(egui::Stroke::new(1.5, play_border))
                            .corner_radius(egui::CornerRadius::same(button_corner));
                        if is_loading {
                            button = button.sense(egui::Sense::hover());
                        }
                        let response = area_ui.add_sized(button_size, button).on_hover_text(tooltip_text);
                        if !is_loading && response.clicked() {
                            let id = egui::Id::new("sql_editor");
                            let mut direct_selected = String::new();
                            if let Some(range) = crate::editor_state_adapter::EditorStateAdapter::get_range(area_ui.ctx(), id) {
                                let to_byte_index = |s: &str, char_idx: usize| -> usize {
                                    s.char_indices().map(|(b, _)| b).chain(std::iter::once(s.len())).nth(char_idx).unwrap_or(s.len())
                                };
                                let start_b = to_byte_index(&self.editor.text, range.start);
                                let end_b = to_byte_index(&self.editor.text, range.end);
                                if start_b < end_b && end_b <= self.editor.text.len() {
                                    direct_selected = self.editor.text[start_b..end_b].to_string();
                                }
                            }
                            self.query_execution_in_progress = true;
                            execute_clicked = true;
                            captured_selection_text = if !direct_selected.is_empty() {
                                direct_selected
                            } else {
                                self.selected_text.clone()
                            };
                        }
                    });

                // Floating format button
                let format_spacing = 6.0;
                let format_button_pos = egui::pos2(
                    button_pos.x - button_size.x - format_spacing,
                    button_pos.y,
                );
                let format_clicked = draw_format_sql_button(
                    ui.ctx(),
                    egui::Id::new((format!("floating_format_button_{}", context_id), self.active_tab_index)),
                    format_button_pos,
                    button_size,
                    button_corner,
                );

                if execute_clicked {
                    self.is_table_browse_mode = false;
                    self.extend_query_icon_hold();
                    editor::execute_query_with_text(self, captured_selection_text);
                    ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                    ui.ctx().request_repaint();
                }

                if format_clicked {
                    editor::reformat_current_sql(self, ui);
                    ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                    ui.ctx().request_repaint();
                }

                // Keyboard shortcut
                if ui.input(|i| (i.modifiers.ctrl || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::Enter)) {
                    let has_q = if !self.selected_text.trim().is_empty() {
                        true
                    } else {
                        let cq = editor::extract_query_from_cursor(self);
                        !cq.trim().is_empty() || !self.editor.text.trim().is_empty()
                    };
                    if has_q {
                        let id = egui::Id::new("sql_editor");
                        let mut direct_selected = String::new();
                        if let Some(range) = crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id) {
                            let to_byte_index = |s: &str, char_idx: usize| -> usize {
                                s.char_indices().map(|(b, _)| b).chain(std::iter::once(s.len())).nth(char_idx).unwrap_or(s.len())
                            };
                            let start_b = to_byte_index(&self.editor.text, range.start);
                            let end_b = to_byte_index(&self.editor.text, range.end);
                            if start_b < end_b && end_b <= self.editor.text.len() {
                                direct_selected = self.editor.text[start_b..end_b].to_string();
                            }
                        }
                        self.extend_query_icon_hold();
                        let captured_selection = if !direct_selected.is_empty() {
                            direct_selected
                        } else {
                            self.selected_text.clone()
                        };
                        editor::execute_query_with_text(self, captured_selection);
                    }
                }
            });

        self.render_lint_panel(ui);

        if show_bottom {
            let handle_id = ui.make_persistent_id(format!("editor_table_splitter_{}", context_id));
            let desired_h = 6.0;
            let available_w = ui.available_width();
            let (rect, resp) = ui.allocate_at_least(egui::vec2(available_w, desired_h), egui::Sense::click_and_drag());
            let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color);
            ui.painter().hline(rect.x_range(), rect.center().y, stroke);
            if resp.dragged() {
                let drag_delta = resp.drag_delta().y;
                if avail > 0.0 {
                    self.table_split_ratio = (self.table_split_ratio + (drag_delta / avail)).clamp(0.05, 0.995);
                }
                ui.memory_mut(|m| m.request_focus(handle_id));
            }
            ui.add_space(2.0);

            // Render bottom panel based on view mode
            match self.table_bottom_view {
                models::structs::TableBottomView::Messages => {
                    self.render_messages_content(ui);
                }
                _ => {
                    data_table::render_table_data(self, ui);
                }
            }
        }
    }

    fn open_create_table_wizard(&mut self, connection_id: i64, database_name: Option<String>) {
        let connection = match self
            .connections
            .iter()
            .find(|conn| conn.id == Some(connection_id))
            .cloned()
        {
            Some(conn) => conn,
            None => {
                self.error_message = format!(
                    "Connection {} tidak ditemukan untuk Create Table.",
                    connection_id
                );
                self.show_error_message = true;
                return;
            }
        };

        match connection.connection_type {
            models::enums::DatabaseType::Redis | models::enums::DatabaseType::MongoDB => {
                self.error_message =
                    "Create Table tidak tersedia untuk jenis database ini.".to_string();
                self.show_error_message = true;
                return;
            }
            _ => {}
        }

        let mut target_db = database_name.filter(|s| !s.trim().is_empty());
        if target_db.is_none() {
            let trimmed = connection.database.trim();
            if !trimmed.is_empty() {
                target_db = Some(trimmed.to_string());
            }
        }

        let mut state = models::structs::CreateTableWizardState::new(
            connection_id,
            connection.connection_type.clone(),
            target_db,
        );

        if let Some(first_column) = state.columns.first_mut()
            && first_column.data_type.is_empty()
        {
            first_column.data_type = match connection.connection_type {
                models::enums::DatabaseType::PostgreSQL => "SERIAL".to_string(),
                models::enums::DatabaseType::SQLite => "INTEGER".to_string(),
                models::enums::DatabaseType::MySQL => "INT".to_string(),
                models::enums::DatabaseType::MsSQL => "INT".to_string(),
                _ => String::new(),
            };
        }

        self.current_connection_id = Some(connection_id);
        self.create_table_wizard = Some(state);
        self.create_table_error = None;
        self.show_create_table_dialog = true;
    }

    fn quote_identifier(&self, ident: &str, db_type: &models::enums::DatabaseType) -> String {
        let mut parts: Vec<String> = Vec::new();
        for part in ident.split('.') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            let quoted = match db_type {
                models::enums::DatabaseType::MySQL => {
                    if trimmed.starts_with('`') && trimmed.ends_with('`') {
                        trimmed.to_string()
                    } else {
                        format!("`{}`", trimmed.replace('`', "``"))
                    }
                }
                models::enums::DatabaseType::PostgreSQL | models::enums::DatabaseType::SQLite => {
                    if trimmed.starts_with('"') && trimmed.ends_with('"') {
                        trimmed.to_string()
                    } else {
                        format!("\"{}\"", trimmed.replace('"', "\"\""))
                    }
                }
                models::enums::DatabaseType::MsSQL => {
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        trimmed.to_string()
                    } else {
                        format!("[{}]", trimmed.replace(']', "]]"))
                    }
                }
                _ => trimmed.to_string(),
            };
            parts.push(quoted);
        }

        if parts.is_empty() {
            ident.trim().to_string()
        } else {
            parts.join(".")
        }
    }

    pub fn generate_create_table_sql(
        &self,
        state: &models::structs::CreateTableWizardState,
    ) -> Result<String, String> {
        use models::enums::DatabaseType;

        if state.table_name.trim().is_empty() {
            return Err("Please describe the table name.".to_string());
        }

        if matches!(state.db_type, DatabaseType::Redis | DatabaseType::MongoDB) {
            return Err("Create table is not available for this database type.".to_string());
        }

        if state.columns.is_empty() {
            return Err("Please add at least one column.".to_string());
        }

        let mut column_defs: Vec<String> = Vec::new();
        let mut pk_columns: Vec<String> = Vec::new();

        for column in &state.columns {
            let name_trim = column.name.trim();
            if name_trim.is_empty() {
                return Err("Each column must have a name.".to_string());
            }
            if column.data_type.trim().is_empty() {
                return Err(format!("Column '{}' does not have a data type.", name_trim));
            }

            let mut pieces = vec![
                self.quote_identifier(name_trim, &state.db_type),
                column.data_type.trim().to_string(),
            ];

            if !column.allow_null {
                pieces.push("NOT NULL".to_string());
            }
            if !column.default_value.trim().is_empty() {
                pieces.push(format!("DEFAULT {}", column.default_value.trim()));
            }

            column_defs.push(pieces.join(" "));

            if column.is_primary_key {
                pk_columns.push(self.quote_identifier(name_trim, &state.db_type));
            }
        }

        if !pk_columns.is_empty() {
            column_defs.push(format!("PRIMARY KEY ({})", pk_columns.join(", ")));
        }

        let mut statements: Vec<String> = Vec::new();
        let table_identifier = match state.db_type {
            DatabaseType::PostgreSQL => {
                let schema = state
                    .database_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("public");
                format!(
                    "{}.{}",
                    self.quote_identifier(schema, &state.db_type),
                    self.quote_identifier(state.table_name.trim(), &state.db_type)
                )
            }
            DatabaseType::SQLite => self.quote_identifier(state.table_name.trim(), &state.db_type),
            DatabaseType::MySQL => {
                if let Some(db) = state
                    .database_name
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    statements.push(format!("USE `{}`;", db));
                }
                self.quote_identifier(state.table_name.trim(), &state.db_type)
            }
            DatabaseType::MsSQL => {
                if let Some(db) = state
                    .database_name
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    statements.push(format!("USE [{}];", db));
                }
                self.quote_identifier(state.table_name.trim(), &state.db_type)
            }
            DatabaseType::Redis | DatabaseType::MongoDB => {
                return Err("Create table is not available for this database type.".to_string());
            }
        };

        let create_stmt = format!(
            "CREATE TABLE {} (\n    {}\n);",
            table_identifier,
            column_defs.join(",\n    ")
        );
        statements.push(create_stmt);

        for index in &state.indexes {
            let name_trim = index.name.trim();
            if name_trim.is_empty() {
                continue;
            }
            let columns: Vec<&str> = index
                .columns
                .split(',')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();
            if columns.is_empty() {
                continue;
            }
            if matches!(state.db_type, DatabaseType::Redis | DatabaseType::MongoDB) {
                continue;
            }

            let quoted_cols = columns
                .iter()
                .map(|c| self.quote_identifier(c, &state.db_type))
                .collect::<Vec<_>>()
                .join(", ");
            let quoted_index_name = self.quote_identifier(name_trim, &state.db_type);
            let prefix = if index.unique { "UNIQUE " } else { "" };
            statements.push(format!(
                "CREATE {}INDEX {} ON {} ({});",
                prefix, quoted_index_name, table_identifier, quoted_cols
            ));
        }

        Ok(statements.join("\n"))
    }

    pub fn validate_create_table_step(
        &self,
        state: &mut models::structs::CreateTableWizardState,
        step: models::structs::CreateTableWizardStep,
    ) -> Option<String> {
        use models::structs::CreateTableWizardStep as Step;
        match step {
            Step::Basics => {
                if state.table_name.trim().is_empty() {
                    return Some("Table name must be provided.".to_string());
                }
                if matches!(
                    state.db_type,
                    models::enums::DatabaseType::Redis | models::enums::DatabaseType::MongoDB
                ) {
                    return Some(
                        "Create table is not available for this database type.".to_string(),
                    );
                }
                None
            }
            Step::Columns => {
                if state.columns.is_empty() {
                    return Some("Please add at least one column.".to_string());
                }
                let mut seen = std::collections::HashSet::new();
                for (idx, column) in state.columns.iter_mut().enumerate() {
                    let name_trim = column.name.trim();
                    if name_trim.is_empty() {
                        return Some(format!("Column {} does not have a name.", idx + 1));
                    }
                    let key = name_trim.to_lowercase();
                    if !seen.insert(key) {
                        return Some(format!("Column name '{}' is duplicated.", name_trim));
                    }
                    if column.data_type.trim().is_empty() {
                        return Some(format!("Column '{}' does not have a data type.", name_trim));
                    }
                    if column.is_primary_key {
                        column.allow_null = false;
                    }
                }
                None
            }
            Step::Indexes => {
                for (idx, index) in state.indexes.iter().enumerate() {
                    let name_trim = index.name.trim();
                    let has_columns = index.columns.split(',').any(|c| !c.trim().is_empty());
                    if name_trim.is_empty() && has_columns {
                        return Some(format!("Index {} requires a name.", idx + 1));
                    }
                    if !name_trim.is_empty() && !has_columns {
                        return Some(format!("Index '{}' requires columns.", name_trim));
                    }
                }
                None
            }
            Step::Review => self.generate_create_table_sql(state).err(),
        }
    }

    pub fn submit_create_table_wizard(&mut self, state: models::structs::CreateTableWizardState) {
        match self.generate_create_table_sql(&state) {
            Ok(sql) => {
                let execution = crate::connection::execute_query_with_connection(
                    self,
                    state.connection_id,
                    sql,
                );
                let (success, message) = match execution {
                    Some((headers, rows)) => {
                        let is_error = headers.first().map(|h| h == "Error").unwrap_or(false);
                        if is_error {
                            let msg = rows
                                .first()
                                .and_then(|row| row.first())
                                .cloned()
                                .unwrap_or_else(|| "Failed to create table.".to_string());
                            (false, Some(msg))
                        } else {
                            (true, None)
                        }
                    }
                    None => (
                        false,
                        Some("Failed to execute CREATE TABLE command.".to_string()),
                    ),
                };

                if success {
                    self.create_table_error = None;
                    self.create_table_wizard = None;
                    self.show_create_table_dialog = false;
                    self.error_message = format!(
                        "Table '{}' has been created successfully.",
                        state.table_name.trim()
                    );
                    self.show_error_message = true;
                    self.refresh_connection(state.connection_id);
                } else {
                    let msg = message.unwrap_or_else(|| "Failed to create table.".to_string());
                    self.create_table_error = Some(msg.clone());
                    self.error_message = msg;
                    self.show_error_message = true;
                    self.create_table_wizard = Some(state);
                    self.show_create_table_dialog = true;
                }
            }
            Err(err) => {
                self.create_table_error = Some(err.clone());
                self.create_table_wizard = Some(state);
                self.show_create_table_dialog = true;
            }
        }
    }

    fn get_connection_name(&self, connection_id: i64) -> Option<String> {
        self.connections
            .iter()
            .find(|conn| conn.id == Some(connection_id))
            .map(|conn| conn.name.clone())
    }

    fn render_active_query_jobs_overlay(&mut self, ctx: &egui::Context) {
        self.prune_cancelled_jobs();
        if self.active_query_jobs.is_empty() {
            return;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(200));

        let mut jobs: Vec<connection::QueryJobStatus> =
            self.active_query_jobs.values().cloned().collect();
        jobs.sort_by_key(|status| status.started_at);

        let count = jobs.len();
        let title = if count == 1 {
            "1 running query".to_string()
        } else {
            format!("{} running queries", count)
        };

        let visuals = ctx.style().visuals.clone();
        let frame_fill = if visuals.dark_mode {
            egui::Color32::from_rgb(40, 40, 40)
        } else {
            egui::Color32::from_rgb(255, 245, 235)
        };
        let frame_stroke = if visuals.dark_mode {
            egui::Color32::from_rgb(70, 70, 70)
        } else {
            egui::Color32::from_rgb(225, 190, 170)
        };
        egui::Area::new(egui::Id::new("active_query_jobs_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .show(ctx, |area_ui| {
                egui::Frame::default()
                    .fill(frame_fill)
                    .stroke(egui::Stroke::new(1.0, frame_stroke))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(area_ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("⏳").strong().size(14.0));
                            ui.label(
                                egui::RichText::new(title.clone())
                                    .strong()
                                    .color(egui::Color32::from_rgb(255, 60, 0)), // rgba(255, 60, 0, 1)
                            );
                        });

                        ui.add_space(4.0);

                        ui.vertical(|ui| {
                            ui.set_max_width(420.0);
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 6.0);
                            for status in jobs.iter() {
                                let connection_label = self
                                    .get_connection_name(status.connection_id)
                                    .unwrap_or_else(|| {
                                        format!("Connection {}", status.connection_id)
                                    });
                                let elapsed = status.started_at.elapsed();
                                let elapsed_label = if elapsed.as_secs() >= 60 {
                                    let minutes = elapsed.as_secs() / 60;
                                    let seconds = elapsed.as_secs() % 60;
                                    format!("{}m {:02}s", minutes, seconds)
                                } else {
                                    format!("{:.1}s", elapsed.as_secs_f32())
                                };

                                let sanitised = status.query_preview.replace('\n', " ");
                                let mut preview = sanitised.chars().take(60).collect::<String>();
                                if sanitised.chars().count() > 60 {
                                    preview.push('…');
                                }

                                let chip_text = format!(
                                    "{} • {} • {}",
                                    connection_label,
                                    elapsed_label,
                                    preview.trim()
                                );

                                let job_id = status.job_id;
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);

                                    let response = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(chip_text.clone()).size(11.0),
                                        )
                                        .wrap(),
                                    );
                                    response.on_hover_text(status.query_preview.clone());

                                    let cancel_button = ui.add(
                                        egui::Button::new(
                                            egui::RichText::new("Cancel")
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(230, 80, 60)),
                                        )
                                        .min_size(egui::vec2(64.0, 22.0)),
                                    );

                                    if cancel_button.clicked()
                                        && self.cancel_active_query_job(job_id)
                                    {
                                        ctx.request_repaint();
                                    }
                                });
                            }
                        });
                    });
            });
    }

    fn cancel_active_query_job(&mut self, job_id: u64) -> bool {
        self.prune_cancelled_jobs();

        let preview_text = self
            .active_query_jobs
            .get(&job_id)
            .map(|status| status.query_preview.replace('\n', " "));

        let mut cancelled = false;
        if let Some(handle) = self.active_query_handles.remove(&job_id) {
            handle.abort();
            cancelled = true;
        }

        let had_status = self.active_query_jobs.remove(&job_id).is_some();
        let was_paginated = self.pending_paginated_jobs.remove(&job_id);

        if had_status || was_paginated || cancelled {
            self.cancelled_query_jobs
                .insert(job_id, std::time::Instant::now());

            if self.active_query_jobs.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }

            if !was_paginated {
                if let Some(preview) = preview_text.filter(|p| !p.is_empty()) {
                    let truncated: String = if preview.chars().count() > 80 {
                        preview.chars().take(80).collect::<String>() + "…"
                    } else {
                        preview
                    };
                    self.error_message = format!("Query cancelled: {}", truncated.trim());
                } else {
                    self.error_message = "Query cancelled.".to_string();
                }
                self.show_error_message = true;
                self.current_table_name = "Query cancelled".to_string();
            }

            true
        } else {
            false
        }
    }

    fn prune_cancelled_jobs(&mut self) {
        let now = std::time::Instant::now();
        let ttl = std::time::Duration::from_secs(30);
        self.cancelled_query_jobs
            .retain(|_, timestamp| now.duration_since(*timestamp) < ttl);
    }

    fn render_tree(
        &mut self,
        ui: &mut egui::Ui,
        nodes: &mut [models::structs::TreeNode],
        is_search_mode: bool,
    ) -> Vec<(String, String, String, Option<i64>)> {
        // Process pending auto-load requests FIRST, before rendering
        // This ensures expanded nodes are loaded from cache before first render
        let pending_loads: Vec<i64> = self.pending_auto_load.drain().collect();
        if !pending_loads.is_empty() {
            info!(
                "📂 Processing {} pending auto-loads BEFORE render",
                pending_loads.len()
            );
        }
        for connection_id in pending_loads {
            info!("📂 Processing auto-load for connection {}", connection_id);
            // Find the connection node
            let mut found = false;
            for node in nodes.iter_mut() {
                if node.node_type == models::enums::NodeType::Connection
                    && node.connection_id == Some(connection_id)
                {
                    info!("   ✅ Found connection node: {}", node.name);
                    info!("   🔄 Loading expanded nodes recursively from cache...");
                    self.load_expanded_nodes_recursive(connection_id, node);
                    found = true;
                    break;
                }
            }
            if !found {
                info!("   ❌ Connection node {} not found in tree!", connection_id);
            }
        }

        // Build quick lookup: connection_id -> DatabaseType
        let mut connection_types: std::collections::HashMap<i64, models::enums::DatabaseType> =
            std::collections::HashMap::new();
        for c in &self.connections {
            if let Some(id) = c.id {
                connection_types.insert(id, c.connection_type.clone());
            }
        }
        let mut expansion_requests = Vec::new();
        let mut tables_to_expand = Vec::new();
        let mut context_menu_requests = Vec::new();
        let mut table_click_requests: Vec<(i64, String, models::enums::NodeType, Option<String>)> = Vec::new();
        let mut connection_click_requests = Vec::new();
        let mut index_click_requests: Vec<(i64, String, Option<String>, Option<String>)> =
            Vec::new();
        let mut create_index_requests: Vec<(i64, Option<String>, Option<String>)> = Vec::new();
        let mut alter_table_requests: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut query_files_to_open: Vec<(String, String, String, Option<i64>)> = Vec::new();
        let mut create_table_requests: Vec<(i64, Option<String>)> = Vec::new();
        let mut stored_procedure_click_requests: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut generate_ddl_requests: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut open_diagram_requests: Vec<(i64, String)> = Vec::new();
        let mut add_view_requests: Vec<i64> = Vec::new();
        let mut custom_view_click_requests: Vec<(i64, String, String)> = Vec::new();
        let mut delete_custom_view_requests: Vec<(i64, String)> = Vec::new();
        let mut edit_custom_view_requests: Vec<(i64, String, String)> = Vec::new();

        for (index, node) in nodes.iter_mut().enumerate() {
            let (
                expansion_request,
                table_expansion,
                context_menu_request,
                table_click_request,
                connection_click_request,
                query_file_to_open,
                folder_for_removal,
                parent_for_creation,
                folder_removal_mapping,
                _dba_click_request,
                index_click_request,
                create_index_request,
                alter_table_request,
                drop_collection_request,
                drop_table_request,
                create_table_request,
                stored_procedure_click_request,
                generate_ddl_request,
                open_diagram_request,
                request_add_view_dialog,
                custom_view_click_request,
                delete_custom_view_request,
                edit_custom_view_request,
            ) = Self::render_tree_node_with_table_expansion(
                ui,
                node,
                &mut self.editor,
                RenderTreeNodeParams {
                    node_index: index,
                    refreshing_connections: &self.refreshing_connections,
                    connection_pools: &self.connection_pools,
                    pending_connection_pools: &self.pending_connection_pools,
                    shared_connection_pools: &self.shared_connection_pools,
                    is_search_mode,
                    connection_types: &connection_types,
                    prefetch_progress: &self.prefetch_progress,
                },
            );
            if let Some(expansion_req) = expansion_request {
                expansion_requests.push(expansion_req);
            }
            if let Some((table_index, connection_id, table_name)) = table_expansion {
                tables_to_expand.push((table_index, connection_id, table_name));
            }
            if let Some(folder_name) = folder_for_removal {
                self.selected_folder_for_removal = Some(folder_name.clone());
            }
            if let Some((hash, folder_path)) = folder_removal_mapping {
                self.folder_removal_map.insert(hash, folder_path);
            }
            if let Some(parent_folder) = parent_for_creation {
                self.parent_folder_for_creation = Some(parent_folder);
            }
            if let Some(context_id) = context_menu_request {
                context_menu_requests.push(context_id);
            }
            if let Some((connection_id, table_name, node_type, db_name)) = table_click_request {
                table_click_requests.push((connection_id, table_name, node_type, db_name));
            }
            if let Some(connection_id) = connection_click_request {
                connection_click_requests.push(connection_id);
            }
            if let Some((filename, content, file_path)) = query_file_to_open {
                query_files_to_open.push((filename, content, file_path, node.connection_id));
            }
            if let Some((conn_id, db_name, table_name)) = alter_table_request {
                alter_table_requests.push((conn_id, db_name, table_name));
            }
            // Collect DBA quick view requests
            // Collect Custom View click requests (Run immediately like DBA Views)
            if let Some((conn_id, view_name, query)) = custom_view_click_requests.last().cloned() {
                // Handle immediately here
                 editor::create_new_tab_with_connection(
                    self,
                    view_name.clone(),
                    query.clone(),
                    Some(conn_id),
                );

                // Detect special mode from query (Preserve DBA special modes)
                let trimmed_query = query.trim();
                let special_mode = if trimmed_query.eq_ignore_ascii_case("SHOW REPLICA STATUS;") {
                    Some(models::enums::DBASpecialMode::ReplicationStatus)
                } else if trimmed_query.eq_ignore_ascii_case("SHOW MASTER STATUS;") {
                    Some(models::enums::DBASpecialMode::MasterStatus)
                } else {
                    None
                };

                if let Some(mode) = special_mode
                    && let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                        tab.dba_special_mode = Some(mode);
                    }
                // Handle immediately here
                 editor::create_new_tab_with_connection(
                    self,
                    view_name.clone(),
                    query.clone(),
                    Some(conn_id),
                );
                
                self.current_connection_id = Some(conn_id);
                // Ensure (or kick off) connection pool before executing
                if let Some(rt) = self.runtime.clone() {
                    rt.block_on(async {
                        let _ =
                            crate::connection::get_or_create_connection_pool(self, conn_id)
                                .await;
                    });
                }
                
                if let Some((headers, data)) =
                    connection::execute_query_with_connection(self, conn_id, query.clone())
                {
                    self.current_table_headers = headers;
                    self.current_table_data = data.clone();
                    self.all_table_data = data;
                    self.current_table_name = view_name;
                    self.is_table_browse_mode = false;
                    self.total_rows = self.all_table_data.len();
                    self.current_page = 0;
                    
                    // Mark as executed
                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                         tab.has_executed_query = true;
                    }
                }
            }
            // Collect index click requests
            if let Some((conn_id, index_name, db_name, table_name)) = index_click_request {
                index_click_requests.push((conn_id, index_name, db_name, table_name));
            }
            // Collect create index requests
            if let Some((conn_id, db_name, table_name)) = create_index_request {
                create_index_requests.push((conn_id, db_name, table_name));
            }
            // Collect Mongo drop collection requests
            if let Some((conn_id, db, coll)) = drop_collection_request {
                // Store pending state for confirmation window outside the loop
                self.pending_drop_collection = Some((conn_id, db, coll));
            }
            // Collect DROP TABLE requests
            if let Some((conn_id, db, table, stmt)) = drop_table_request {
                // Store pending state for confirmation window outside the loop
                self.pending_drop_table = Some((conn_id, db, table, stmt));
            }
            if let Some((conn_id, db_name)) = create_table_request {
                create_table_requests.push((conn_id, db_name));
            }
            if let Some((conn_id, db_name, proc_name)) = stored_procedure_click_request {
                stored_procedure_click_requests.push((conn_id, db_name, proc_name));
            }
            if let Some((conn_id, db_name, table_name)) = generate_ddl_request {
                generate_ddl_requests.push((conn_id, db_name, table_name));
            }
            if let Some((conn_id, db_name)) = open_diagram_request {
                open_diagram_requests.push((conn_id, db_name));
            }
            if let Some((conn_id, name, query)) = custom_view_click_request {
                custom_view_click_requests.push((conn_id, name, query));
            }
            if let Some(conn_id) = request_add_view_dialog {
                log::warn!("!!! REQUEST ADD VIEW DIALOG for conn_id: {}", conn_id);
                add_view_requests.push(conn_id);
            }
            if let Some(req) = delete_custom_view_request {
                delete_custom_view_requests.push(req);
            }
            if let Some(req) = edit_custom_view_request {
                edit_custom_view_requests.push(req);
            }
        }

        // Process add view requests
        for conn_id in add_view_requests {
             self.show_add_view_dialog = true;
             self.new_view_connection_id = Some(conn_id);
             self.new_view_name = String::new();
             self.new_view_query = "SELECT * FROM ...".to_string();
        }

        if let Some((conn_id, view_name)) = delete_custom_view_requests.pop() {
            let mut conn_to_save = None;
            // Find connection and remove view
            if let Some(conn) = self.connections.iter_mut().find(|c| c.id == Some(conn_id)) {
                 conn.custom_views.retain(|v| v.name != view_name);
                 conn_to_save = Some(conn.clone());
            }

            // Save connection (outside of mutable borrow of connections)
            if let Some(conn) = conn_to_save
                 && crate::sidebar_database::save_connection_to_database(self, &conn) {
                     crate::sidebar_database::refresh_connections_tree(self);
                 }
        }

        if let Some((conn_id, view_name, query)) = edit_custom_view_requests.pop() {
            self.show_add_view_dialog = true;
            self.new_view_connection_id = Some(conn_id);
            self.new_view_name = view_name.clone();
            self.new_view_query = query;
            self.edit_view_original_name = Some(view_name);
        }


        for (conn_id, db_name) in create_table_requests {
            self.open_create_table_wizard(conn_id, db_name);
        }

        for (conn_id, db_name) in open_diagram_requests {
            // 1. Fetch Foreign Keys (blocking for now, MVP)
            let mut fks = Vec::new();
            let mut columns_map = std::collections::HashMap::new();
            if let Some(rt) = self.runtime.clone() {
                // Ensure pool exists
                rt.block_on(async {
                    let _ = crate::connection::get_or_create_connection_pool(self, conn_id).await;
                    fks = crate::connection::get_foreign_keys(self, conn_id, &db_name).await;
                    
                    // Fetch all columns for diagram (MySQL optimization)
                    if let Some(pool_enum) = self.connection_pools.get(&conn_id)
                         && let models::enums::DatabasePool::MySQL(p) = pool_enum
                             && let Ok(cols) = crate::driver_mysql::fetch_mysql_columns(p, &db_name).await {
                                 columns_map = cols;
                             }
                });
            }

            // 1b. Fetch All Tables (to ensure isolated tables are shown)
            let mut all_tables = Vec::new();
            let db_type = self.connections.iter().find(|c| c.id == Some(conn_id)).map(|c| c.connection_type.clone());
            match db_type {
                Some(models::enums::DatabaseType::MySQL) => {
                     if let Some(t) = crate::driver_mysql::fetch_tables_from_mysql_connection(self, conn_id, &db_name, "table") {
                         all_tables = t;
                     }
                },
                Some(models::enums::DatabaseType::PostgreSQL) => {
                      if let Some(t) = crate::driver_postgres::fetch_tables_from_postgres_connection(self, conn_id, &db_name, "BASE TABLE") {
                          all_tables = t;
                      }
                },
                Some(models::enums::DatabaseType::SQLite) => {
                      if let Some(t) = crate::driver_sqlite::fetch_tables_from_sqlite_connection(self, conn_id, "table") {
                          all_tables = t;
                      }
                },
                Some(models::enums::DatabaseType::MsSQL) => {
                      if let Some(t) = crate::driver_mssql::fetch_tables_from_mssql_connection(self, conn_id, &db_name, "table") {
                          all_tables = t;
                      }
                },
                _ => {}
            }

            // 2. Initialize Diagram State
            let mut state = self.load_diagram(conn_id, &db_name).unwrap_or_default();
            
            // Populate nodes (tables)
            let mut table_names = std::collections::HashSet::new();
            for fk in &fks {
                table_names.insert(fk.table_name.clone());
                table_names.insert(fk.referenced_table_name.clone());
            }
            log::info!("Diagram Init: Found {} FKs and {} tables", fks.len(), all_tables.len());
            for t in all_tables {
                table_names.insert(t);
            }
            
            for (t_name, cols) in &columns_map {
                log::debug!("Table {} has {} columns", t_name, cols.len());
            }

            // Sync FKs (edges) - Always refresh edges based on current Schema
            let edges: Vec<models::structs::DiagramEdge> = fks.iter().map(|fk| models::structs::DiagramEdge {
                source: fk.table_name.clone(),
                target: fk.referenced_table_name.clone(),
                label: "".to_string(),
            }).collect();
            state.edges = edges;

            // Grouping Logic (Refresh groups if empty or for new nodes?)
            // For MVP, we regenerate groups map for new nodes usage, 
            // but we should probably keep existing groups if possible?
            // Let's re-calculate groups for ALL tables.
            let mut groups_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
            
            // Helper to get prefix
            let get_prefix = |name: &str| -> String {
                name.split('_').next().unwrap_or(name).to_string()
            };

            for table in &table_names {
                let prefix = get_prefix(table);
                groups_map.entry(prefix).or_default().push(table.to_string());
            }

            // Update/Create DiagramGroups 
            let mut existing_group_ids: std::collections::HashSet<String> = state.groups.iter().map(|g| g.id.clone()).collect();
            
            // Simple color palette generator
            let colors = [
                eframe::egui::Color32::from_rgb(100, 149, 237), // Cornflower Blue
                eframe::egui::Color32::from_rgb(60, 179, 113),  // Medium Sea Green
                eframe::egui::Color32::from_rgb(205, 92, 92),   // Indian Red
                eframe::egui::Color32::from_rgb(218, 165, 32),  // Goldenrod
                eframe::egui::Color32::from_rgb(147, 112, 219), // Medium Purple
                eframe::egui::Color32::from_rgb(70, 130, 180),  // Steel Blue
                eframe::egui::Color32::from_rgb(255, 127, 80),  // Coral
            ];
            let mut color_idx = 0;

            for (prefix, tables) in groups_map {
                if tables.len() > 1 {
                    let group_id = format!("group_{}", prefix);
                    
                    if !existing_group_ids.contains(&group_id) {
                        let title = prefix[0..1].to_uppercase() + &prefix[1..]; // Capitalize
                        let color = colors[color_idx % colors.len()];
                        color_idx += 1;

                        state.groups.push(models::structs::DiagramGroup {
                            id: group_id.clone(),
                            title,
                            color,
                            manual_pos: None,
                        });
                        existing_group_ids.insert(group_id.clone());
                    }
                }
            }

            // Sync Nodes
            // 1. Remove nodes that no longer exist
            state.nodes.retain(|n| table_names.contains(&n.id));
            
            // 2. Identify new nodes
            let existing_node_ids: std::collections::HashSet<String> = state.nodes.iter().map(|n| n.id.clone()).collect();
            let new_tables: Vec<String> = table_names.iter().filter(|t| !existing_node_ids.contains(*t)).cloned().collect();
            let is_init = state.nodes.is_empty();

            // apply to state (this block replaces the old logic)
            // We need to call layout ONLY if it was empty, or only for new nodes?
            // If we have saved state, we DON'T run full auto layout that resets everything.
            
            // Add new nodes
            for table in new_tables {
                 let hash: u64 = table.bytes().fold(5381, |acc, c| acc.wrapping_shl(5).wrapping_add(acc).wrapping_add(c as u64));
                 let x = (hash % 800) as f32 + 100.0;
                 let y = ((hash / 800) % 600) as f32 + 100.0;
                 
                  let mut node = models::structs::DiagramNode {
                    id: table.clone(),
                    title: table.clone(),
                     pos: eframe::egui::pos2(x, y),
                     size: eframe::egui::vec2(150.0, 100.0), // Default, will be auto-sized
                     columns: columns_map.get(&table).cloned().unwrap_or_default(),
                     foreign_keys: fks.iter().filter(|fk| fk.table_name == table).cloned().collect(),
                     group_id: None,
                 };
                // Assign group
                let prefix = get_prefix(&table);
                if existing_group_ids.contains(&format!("group_{}", prefix)) {
                     node.group_id = Some(format!("group_{}", prefix));
                }
                state.nodes.push(node);
            }
            
             // Refresh columns for existing nodes too (in case of schema change)
             for node in &mut state.nodes {
                  if let Some(cols) = columns_map.get(&node.id) {
                      node.columns = cols.clone();
                  }
             }

            // Apply Layout ONLY if it was fresh init (no saved state used)
            if is_init {
                 crate::diagram_view::perform_auto_layout(&mut state);
            }
            
            // 3. Create Tab
            // fks consumed? No, we used iter().
            // Original code used into_iter() for edges. I replaced it with iter above.


            // 3. Create Tab
            let title = format!("Diagram: {}", db_name);
            editor::create_new_tab_with_connection_and_database(
                self,
                title,
                String::new(), // No query content
                Some(conn_id),
                Some(db_name.clone()),
            );
            
            // 4. Attach Diagram State to the new active tab
            if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                tab.diagram_state = Some(state);
            }
            self.table_bottom_view = models::structs::TableBottomView::Query;
        }

        for (conn_id, db_name, table_name) in generate_ddl_requests {
            if let Some(conn) = self.connections.iter().find(|c| c.id == Some(conn_id)).cloned() {
                let definition = crate::connection::fetch_table_definition(&conn, db_name.as_deref(), &table_name);
                if let Some(sql) = definition {
                    let title = format!("DDL: {}", table_name);
                    crate::editor::create_new_tab_with_connection_and_database(
                        self,
                        title,
                        sql,
                        Some(conn_id),
                        db_name.clone(),
                    );
                    self.table_bottom_view = models::structs::TableBottomView::Query;
                } else {
                    self.error_message = format!("Could not generate DDL for table '{}'. It might not be supported for this database type.", table_name);
                    self.show_error_message = true;
                }
            }
        }

        for (connection_id, database_name, table_name) in alter_table_requests {
            self.handle_alter_table_request(connection_id, database_name, table_name);
        }

        // Handle stored procedure clicks - open the actual definition in a new tab (no templates)
        for (conn_id, db_name, proc_name) in stored_procedure_click_requests {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.id == Some(conn_id))
                .cloned()
            {
                let script =
                    connection::fetch_procedure_definition(&conn, db_name.as_deref(), &proc_name)
                        // If we can't fetch, just show the procedure name (no template as requested)
                        .unwrap_or_else(|| proc_name.clone());

                let title = format!("Procedure: {}", proc_name);
                editor::create_new_tab_with_connection_and_database(
                    self,
                    title,
                    script,
                    Some(conn_id),
                    db_name.clone(),
                );
                // Ensure the active tab stores selected database context for later executions
                if let (Some(dbn), Some(active_tab)) =
                    (db_name, self.query_tabs.get_mut(self.active_tab_index))
                {
                    active_tab.database_name = Some(dbn);
                }
                // Focus Query view
                self.table_bottom_view = models::structs::TableBottomView::Query;
            }
        }

        // Handle connection clicks (create new tab with that connection)
        // We'll collect connection IDs needing eager pool creation to process after loop
        let mut pools_to_create: Vec<i64> = Vec::new();

        // Check table clicks for missing pools too
        for (connection_id, _, _, _) in &table_click_requests {
             if !self.connection_pools.contains_key(connection_id) && !pools_to_create.contains(connection_id) {
                 pools_to_create.push(*connection_id);
             }
        }

        for connection_id in connection_click_requests {
            // Find connection name for tab title
            let connection_name = self
                .connections
                .iter()
                .find(|conn| conn.id == Some(connection_id))
                .map(|conn| conn.name.clone())
                .unwrap_or_else(|| format!("Connection {}", connection_id));

            // Create new tab with this connection pre-selected
            let tab_title = format!("Query - {}", connection_name);
            editor::create_new_tab_with_connection(
                self,
                tab_title,
                String::new(),
                Some(connection_id),
            );

            debug!("Created new tab with connection ID: {}", connection_id);

            // NEW: Immediately (lazily-once) create the underlying connection pool so that
            // first table/data click feels faster. Previously pool was only created
            // when executing a query or expanding tables.
            if !self.connection_pools.contains_key(&connection_id) {
                pools_to_create.push(connection_id);
            } else {
                debug!(
                    "✅ Connection pool already exists for {} (click)",
                    connection_id
                );
            }
        }

        // Now create pools (after mutable/immutable borrows ended)
        // Now create pools (after mutable/immutable borrows ended)
        if !pools_to_create.is_empty() {
             for cid in pools_to_create {
                 if !self.connection_pools.contains_key(&cid) {
                    crate::connection::start_background_pool_creation(self, cid);
                 }
             }
        }

        // Handle expansions after rendering
        for expansion_req in expansion_requests {
            match expansion_req.node_type {
                models::enums::NodeType::Connection => {
                    // Find Connection node recursively and load if not already loaded
                    if let Some(connection_node) =
                        Self::find_connection_node_recursive(nodes, expansion_req.connection_id)
                    {
                        if !connection_node.is_loaded {
                            self.load_connection_tables(
                                expansion_req.connection_id,
                                connection_node,
                            );
                        }
                    } else {
                        debug!(
                            "Connection node not found for ID: {}",
                            expansion_req.connection_id
                        );
                    }
                }
                models::enums::NodeType::DatabasesFolder => {
                    // Handle DatabasesFolder expansion - load actual databases from server
                    for node in nodes.iter_mut() {
                        if node.node_type == models::enums::NodeType::Connection
                            && node.connection_id == Some(expansion_req.connection_id)
                        {
                            // Find the DatabasesFolder within this connection
                            for child in &mut node.children {
                                if child.node_type == models::enums::NodeType::DatabasesFolder
                                    && !child.is_loaded
                                {
                                    self.load_databases_for_folder(
                                        expansion_req.connection_id,
                                        child,
                                    );
                                    break;
                                }
                            }
                            break;
                        }
                    }
                }
                models::enums::NodeType::Database => {
                    debug!(
                        "🔍 Database expansion request received for connection_id: {}, database_name: {:?}",
                        expansion_req.connection_id, expansion_req.database_name
                    );

                    // Handle Database expansion for Redis - load keys for the database
                    if let Some(connection) = self
                        .connections
                        .iter()
                        .find(|c| c.id == Some(expansion_req.connection_id))
                    {
                        debug!(
                            "✅ Found connection: {} (type: {:?})",
                            connection.name, connection.connection_type
                        );

                        if connection.connection_type == models::enums::DatabaseType::Redis {
                            debug!("🔑 Processing Redis database expansion");

                            // Find the database node and load its keys
                            let mut node_found = false;
                            for (node_idx, node) in nodes.iter_mut().enumerate() {
                                debug!(
                                    "🌳 Checking tree node [{}]: '{}' (type: {:?}, connection_id: {:?})",
                                    node_idx, node.name, node.node_type, node.connection_id
                                );

                                if let Some(db_node) = Self::find_redis_database_node(
                                    node,
                                    expansion_req.connection_id,
                                    &expansion_req.database_name,
                                ) {
                                    debug!(
                                        "📁 Found database node: {}, is_loaded: {}",
                                        db_node.name, db_node.is_loaded
                                    );
                                    node_found = true;

                                    if !db_node.is_loaded {
                                        debug!(
                                            "⏳ Loading keys for database: {}",
                                            expansion_req.database_name.clone().unwrap_or_default()
                                        );
                                        self.load_redis_keys_for_database(
                                            expansion_req.connection_id,
                                            &expansion_req
                                                .database_name
                                                .clone()
                                                .unwrap_or_default(),
                                            db_node,
                                        );
                                    } else {
                                        debug!(
                                            "✅ Database already loaded with {} children",
                                            db_node.children.len()
                                        );
                                    }
                                    break;
                                }
                            }

                            if !node_found {
                                debug!(
                                    "❌ Database node not found in any tree branch for database: {:?}",
                                    expansion_req.database_name
                                );
                            }
                        } else {
                            debug!(
                                "❌ Connection is not Redis type: {:?}",
                                connection.connection_type
                            );
                        }
                    } else {
                        debug!(
                            "❌ Connection not found for ID: {}",
                            expansion_req.connection_id
                        );
                    }
                }
                models::enums::NodeType::TablesFolder
                | models::enums::NodeType::ViewsFolder
                | models::enums::NodeType::StoredProceduresFolder
                | models::enums::NodeType::UserFunctionsFolder
                | models::enums::NodeType::TriggersFolder
                | models::enums::NodeType::EventsFolder => {
                    // Find the specific folder node and load if not already loaded

                    // We need to find the exact folder node in the tree
                    let connection_id = expansion_req.connection_id;
                    let folder_type = expansion_req.node_type.clone();
                    let database_name = expansion_req.database_name.clone();

                    // Search for folder node by traversing the tree recursively
                    let mut found = false;
                    for node in nodes.iter_mut() {
                        // Search recursively through all nodes, not just top level
                        if let Some(folder_node) = Self::find_specific_folder_node(
                            node,
                            connection_id,
                            &folder_type,
                            &database_name,
                        ) {
                            if !folder_node.is_loaded {
                                self.load_folder_content(
                                    connection_id,
                                    folder_node,
                                    folder_type.clone(),
                                );
                                found = true;
                            }
                            break;
                        }
                    }
                    if !found {
                        debug!(
                            "Could not find folder node with type {:?} and database {:?} in any of the nodes",
                            folder_type, database_name
                        );
                    }
                }
                _ => {
                    debug!("Unhandled node type: {:?}", expansion_req.node_type);
                }
            }
        }

        // Handle table column expansions
        // Handle table expansions
        for (table_index, connection_id, table_name) in tables_to_expand {
            self.load_table_columns_for_node(connection_id, &table_name, nodes, table_index);
        }

        // Handle table click requests - create new tab for each table
        for (connection_id, table_name, node_type, predefined_db_name) in table_click_requests {
            // Find the connection to determine the database type and database name
            let connection = self
                .connections
                .iter()
                .find(|conn| conn.id == Some(connection_id))
                .cloned();

            if let Some(conn) = connection {
                let is_view = node_type == models::enums::NodeType::View;
                // Find the database name from the tree structure
                let mut database_name: Option<String> = predefined_db_name;

                // Optimization: Only search if not provided (should be provided for most table clicks)
                if database_name.is_none() {
                    for node in nodes.iter() {
                        if let Some(db_name) =
                            Tabular::find_database_name_for_table(node, connection_id, &table_name)
                        {
                            database_name = Some(db_name);
                            break;
                        }
                    }
                }

                // If no database found in tree, use connection default
                if database_name.is_none() {
                    database_name = Some(conn.database.clone());
                }

                match conn.connection_type {
                    models::enums::DatabaseType::Redis => {
                        // Redis objects never carry ALTER view DDL
                        self.current_object_ddl = None;
                        // Check if this is a Redis key (has specific Redis data types in the tree structure)
                        // For Redis keys, we need to find which database they belong to
                        let mut is_redis_key = false;
                        let mut key_type: Option<String> = None;

                        for node in nodes.iter() {
                            if let Some((_, k_type)) =
                                Tabular::find_redis_key_info(node, &table_name)
                            {
                                key_type = Some(k_type.clone());
                                is_redis_key = true;
                                break;
                            }
                        }

                        if is_redis_key {
                            if let Some(k_type) = key_type {
                                // This is a Redis key - create a query tab with appropriate Redis command
                                let redis_command = match k_type.to_lowercase().as_str() {
                                    "string" => format!("GET {}", table_name),
                                    "hash" => format!("HGETALL {}", table_name),
                                    "list" => format!("LRANGE {} 0 -1", table_name),
                                    "set" => format!("SMEMBERS {}", table_name),
                                    "zset" | "sorted_set" => {
                                        format!("ZRANGE {} 0 -1 WITHSCORES", table_name)
                                    }
                                    "stream" => format!("XRANGE {} - +", table_name),
                                    _ => format!("TYPE {}", table_name), // Fallback to show type
                                };

                                let tab_title = format!("Redis Key: {} ({})", table_name, k_type);
                                editor::create_new_tab_with_connection_and_database(
                                    self,
                                    tab_title,
                                    redis_command.clone(),
                                    Some(connection_id),
                                    database_name.clone(),
                                );

                                // Set current connection ID and database for Redis query execution
                                self.current_connection_id = Some(connection_id);

                                // Auto-execute the Redis query and display results in bottom
                                if let Some((headers, data)) =
                                    connection::execute_query_with_connection(
                                        self,
                                        connection_id,
                                        redis_command,
                                    )
                                {
                                    self.current_table_headers = headers;
                                    self.current_table_data = data.clone();
                                    self.all_table_data = data;
                                    self.current_table_name = format!("Redis Key: {}", table_name);
                                    self.total_rows = self.all_table_data.len();
                                    self.current_page = 0;
                                }
                            }
                        } else {
                            // This is a Redis folder/type - create a query tab for scanning keys
                            let redis_command = match table_name.as_str() {
                                "hashes" => "SCAN 0 MATCH *:* TYPE hash COUNT 100".to_string(),
                                "strings" => "SCAN 0 MATCH *:* TYPE string COUNT 100".to_string(),
                                "lists" => "SCAN 0 MATCH *:* TYPE list COUNT 100".to_string(),
                                "sets" => "SCAN 0 MATCH *:* TYPE set COUNT 100".to_string(),
                                "sorted_sets" => "SCAN 0 MATCH *:* TYPE zset COUNT 100".to_string(),
                                "streams" => "SCAN 0 MATCH *:* TYPE stream COUNT 100".to_string(),
                                _ => {
                                    // Extract folder name from display format like "Strings (5)"
                                    let clean_name =
                                        table_name.split('(').next().unwrap_or(&table_name).trim();
                                    format!("SCAN 0 MATCH *:* COUNT 100 # Browse {}", clean_name)
                                }
                            };
                            let tab_title = format!("Redis {}", table_name);
                            editor::create_new_tab_with_connection_and_database(
                                self,
                                tab_title,
                                redis_command.clone(),
                                Some(connection_id),
                                database_name.clone(),
                            );

                            // Set database and auto-execute
                            self.current_connection_id = Some(connection_id);
                            // Reset spreadsheet editing state when opening a key browse
                            self.reset_spreadsheet_state();
                            if let Some((headers, data)) = connection::execute_query_with_connection(
                                self,
                                connection_id,
                                redis_command,
                            ) {
                                self.current_table_headers = headers;
                                self.current_table_data = data.clone();
                                self.all_table_data = data;
                                self.current_table_name = format!("Redis {}", table_name);
                                self.total_rows = self.all_table_data.len();
                                self.current_page = 0;
                                if let Some(active_tab) =
                                    self.query_tabs.get_mut(self.active_tab_index)
                                {
                                    active_tab.result_headers = self.current_table_headers.clone();
                                    active_tab.result_rows = self.current_table_data.clone();
                                    active_tab.result_all_rows = self.all_table_data.clone();
                                    active_tab.result_table_name = self.current_table_name.clone();
                                    active_tab.is_table_browse_mode = self.is_table_browse_mode;
                                    active_tab.current_page = self.current_page;
                                    active_tab.page_size = self.page_size;
                                    active_tab.total_rows = self.total_rows;
                                }
                            }
                        }
                    }
                    models::enums::DatabaseType::MongoDB => {
                        // For MongoDB, treat table_name as a collection; database_name must be present
                        if let Some(db_name) = &database_name {
                            let tab_title = format!("Collection: {}.{}", db_name, table_name);
                            editor::create_new_tab_with_connection_and_database(
                                self,
                                tab_title.clone(),
                                String::new(),
                                Some(connection_id),
                                database_name.clone(),
                            );
                            self.current_connection_id = Some(connection_id);
                            // Reset spreadsheet editing state when opening a collection
                            self.reset_spreadsheet_state();
                            if let Some((headers, data)) =
                                crate::driver_mongodb::sample_collection_documents(
                                    self,
                                    connection_id,
                                    db_name,
                                    &table_name,
                                    100,
                                )
                            {
                                self.current_table_headers = headers;
                                self.current_table_data = data.clone();
                                self.all_table_data = data;
                                self.current_table_name = tab_title;
                                self.total_rows = self.all_table_data.len();
                                self.current_page = 0;
                                if let Some(active_tab) =
                                    self.query_tabs.get_mut(self.active_tab_index)
                                {
                                    active_tab.result_headers = self.current_table_headers.clone();
                                    active_tab.result_rows = self.current_table_data.clone();
                                    active_tab.result_all_rows = self.all_table_data.clone();
                                    active_tab.result_table_name = self.current_table_name.clone();
                                    active_tab.is_table_browse_mode = self.is_table_browse_mode;
                                    active_tab.current_page = self.current_page;
                                    active_tab.page_size = self.page_size;
                                    active_tab.total_rows = self.total_rows;
                                }
                            }
                        } else {
                            self.error_message =
                                "MongoDB requires a database; please select a database."
                                    .to_string();
                            self.show_error_message = true;
                        }
                    }
                    _ => {
                        if !is_view
                            && self.table_bottom_view == models::structs::TableBottomView::Query
                        {
                            self.table_bottom_view = models::structs::TableBottomView::Data;
                        }
                        self.current_object_ddl = None;
                        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            active_tab.object_ddl = None;
                        }
                        // SQL databases - use regular SELECT query with proper database context
                        let query_content = if let Some(db_name) = &database_name {
                            match conn.connection_type {
                                models::enums::DatabaseType::MySQL => {
                                    format!(
                                        "USE `{}`;\nSELECT * FROM `{}` LIMIT 100;",
                                        db_name, table_name
                                    )
                                }
                                models::enums::DatabaseType::PostgreSQL => {
                                    format!(
                                        "SELECT * FROM \"{}\".\"{}\" LIMIT 100;",
                                        db_name, table_name
                                    )
                                }
                                models::enums::DatabaseType::MsSQL => {
                                    // Build robust MsSQL SELECT with explicit database context
                                    driver_mssql::build_mssql_select_query(
                                        db_name.clone(),
                                        table_name.clone(),
                                    )
                                }
                                models::enums::DatabaseType::SQLite
                                | models::enums::DatabaseType::Redis => {
                                    format!("SELECT * FROM `{}` LIMIT 100;", table_name)
                                }
                                models::enums::DatabaseType::MongoDB => {
                                    // Unreachable here; MongoDB handled above with sampling
                                    String::new()
                                }
                            }
                        } else {
                            match conn.connection_type {
                                models::enums::DatabaseType::MsSQL => {
                                    driver_mssql::build_mssql_select_query(
                                        "".to_string(),
                                        table_name.clone(),
                                    )
                                }
                                _ => format!("SELECT * FROM `{}` LIMIT 100;", table_name),
                            }
                        };
                        let tab_title = if is_view {
                            format!("View: {}", table_name)
                        } else {
                            format!("Table: {}", table_name)
                        };
                        editor::create_new_tab_with_connection_and_database(
                            self,
                            tab_title.clone(),
                            query_content.clone(),
                            Some(connection_id),
                            database_name.clone(),
                        );

                        // Reset spreadsheet editing state when opening a table
                        self.reset_spreadsheet_state();

                        // Set database context for current tab and auto-execute the query and display results in bottom
                        self.current_connection_id = Some(connection_id);
                        // Ensure the newly created tab stores selected database (important for MsSQL)
                        if let Some(dbn) = &database_name
                            && let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index)
                        {
                            active_tab.database_name = Some(dbn.clone());
                        }

                        // Set early so infer_current_table_name() bekerja saat Structure view aktif
                        let label_prefix = if is_view { "View" } else { "Table" };
                        self.current_table_name = format!(
                            "{}: {} (Database: {})",
                            label_prefix,
                            table_name,
                            database_name.as_deref().unwrap_or("Unknown")
                        );

                        // Clear newly created rows highlight when switching tables
                        self.newly_created_rows.clear();

                        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            active_tab.result_table_name = self.current_table_name.clone();
                        }

                        // Try show cached 100 rows immediately (cache-first UX)
                        let mut had_cache = false;
                        if let Some(dbn) = &database_name
                            && let Some((cached_headers, cached_rows)) =
                                crate::cache_data::get_table_rows_from_cache(
                                    self,
                                    connection_id,
                                    dbn,
                                    &table_name,
                                )
                            && !cached_headers.is_empty()
                        {
                            info!(
                                "📦 Showing cached data for table {}/{} ({} cols, {} rows)",
                                dbn,
                                table_name,
                                cached_headers.len(),
                                cached_rows.len()
                            );
                            self.current_table_headers = cached_headers.clone();
                            self.current_table_data = cached_rows.clone();
                            self.all_table_data = cached_rows;
                            self.total_rows = self.all_table_data.len();
                            self.current_page = 0;
                            had_cache = true;
                            // Table context changed; ensure future Structure load is for this table
                            self.last_structure_target = None;
                            if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index)
                            {
                                active_tab.result_headers = self.current_table_headers.clone();
                                active_tab.result_rows = self.current_table_data.clone();
                                active_tab.result_all_rows = self.all_table_data.clone();
                                active_tab.result_table_name = self.current_table_name.clone();
                                active_tab.is_table_browse_mode = true;
                                active_tab.current_page = self.current_page;
                                active_tab.page_size = self.page_size;
                                active_tab.total_rows = self.total_rows;
                            }
                        }

                        // Use server-side pagination only when refreshing or when no cache available.
                        if self.use_server_pagination {
                            // Build base query without LIMIT for potential server pagination (store for future refresh),
                            // but don't execute it if we already have cache.
                            let base_query = if let Some(db_name) = &database_name {
                                match conn.connection_type {
                                    models::enums::DatabaseType::MySQL => {
                                        format!(
                                            "USE `{}`;\nSELECT * FROM `{}`",
                                            db_name, table_name
                                        )
                                    }
                                    models::enums::DatabaseType::PostgreSQL => {
                                        format!("SELECT * FROM \"{}\".\"{}\"", db_name, table_name)
                                    }
                                    models::enums::DatabaseType::MsSQL => {
                                        // Build robust MsSQL SELECT with explicit database context but without LIMIT
                                        let mssql_query = driver_mssql::build_mssql_select_query(
                                            db_name.clone(),
                                            table_name.clone(),
                                        );
                                        // Remove the LIMIT part from MsSQL query
                                        mssql_query.replace("SELECT TOP 100", "SELECT")
                                    }
                                    models::enums::DatabaseType::SQLite
                                    | models::enums::DatabaseType::Redis => {
                                        format!("SELECT * FROM `{}`", table_name)
                                    }
                                    models::enums::DatabaseType::MongoDB => {
                                        // MongoDB handled separately above
                                        String::new()
                                    }
                                }
                            } else {
                                match conn.connection_type {
                                    models::enums::DatabaseType::MsSQL => {
                                        let mssql_query = driver_mssql::build_mssql_select_query(
                                            "".to_string(),
                                            table_name.clone(),
                                        );
                                        mssql_query.replace("SELECT TOP 100", "SELECT")
                                    }
                                    _ => format!("SELECT * FROM `{}`", table_name),
                                }
                            };
                            // Always store base_query for potential manual refresh
                            if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index)
                            {
                                active_tab.base_query = base_query.clone();
                            }
                            self.current_base_query = base_query;

                            // If we already showed cache, do NOT auto-fetch from server now.
                            if had_cache {
                                debug!(
                                    "🛑 Skipping live server load on table click because cache exists"
                                );
                                // Keep browse mode enabled for filters to apply on cached data
                                self.is_table_browse_mode = true;
                                self.sql_filter_text.clear();
                                // New table opened; structure target should refresh on demand
                                self.last_structure_target = None;
                            } else {
                                // Set browse mode when opening table via sidebar click
                                self.is_table_browse_mode = true;
                                // If the pool is not ready, queue the first-page query; otherwise execute.
                                let mut pool_ready = true;
                                if self.pending_connection_pools.contains(&connection_id) {
                                    pool_ready = false;
                                } else if !self.connection_pools.contains_key(&connection_id) {
                                    let created_now = if let Some(rt) = self.runtime.clone() {
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    } else {
                                        let rt = self.get_runtime();
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    };
                                    if !created_now {
                                        pool_ready = false;
                                    }
                                }

                                if !pool_ready {
                                    // Prepare server pagination state but defer execution
                                    self.current_page = 0;
                                    if let Some(total) = self.execute_count_query() {
                                        self.actual_total_rows = Some(total);
                                    }
                                    let first_query = self.build_paginated_query(0, self.page_size);
                                    self.pool_wait_in_progress = true;
                                    self.pool_wait_connection_id = Some(connection_id);
                                    self.pool_wait_query = first_query;
                                    self.pool_wait_started_at = Some(std::time::Instant::now());
                                    self.current_table_name =
                                        "Connecting… waiting for pool".to_string();
                                } else {
                                    self.initialize_server_pagination(
                                        self.current_base_query.clone(),
                                    );
                                }
                            }
                        } else {
                            // Client-side path (rare). Only run live query if no cache.
                            if !had_cache {
                                // Set browse mode when opening table via sidebar click
                                self.is_table_browse_mode = true;
                                println!("================== 1 ============================ ");
                                debug!("🔄 Taking client-side pagination fallback path");
                                info!(
                                    "🌐 Loading live data from server for table {}/{} (client pagination)",
                                    database_name.clone().unwrap_or_default(),
                                    table_name
                                );
                                // New table; force structure reload on next toggle
                                self.last_structure_target = None;
                                // Fallback to client-side pagination (original behavior)
                                // For MsSQL, we need to strip TOP from query_content to avoid conflicts
                                let safe_query =
                                    if conn.connection_type == models::enums::DatabaseType::MsSQL {
                                        driver_mssql::sanitize_mssql_select_for_pagination(
                                            &query_content,
                                        )
                                    } else {
                                        query_content.clone()
                                    };
                                debug!("🔄 Client-side query after sanitization: {}", safe_query);

                                // If pool not ready, queue and show loading; otherwise execute now
                                let mut pool_ready = true;
                                if self.pending_connection_pools.contains(&connection_id) {
                                    pool_ready = false;
                                } else if !self.connection_pools.contains_key(&connection_id) {
                                    let created_now = if let Some(rt) = self.runtime.clone() {
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    } else {
                                        let rt = self.get_runtime();
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    };
                                    if !created_now {
                                        pool_ready = false;
                                    }
                                }

                                if !pool_ready {
                                    self.pool_wait_in_progress = true;
                                    self.pool_wait_connection_id = Some(connection_id);
                                    self.pool_wait_query = safe_query;
                                    self.pool_wait_started_at = Some(std::time::Instant::now());
                                    self.current_table_name =
                                        "Connecting… waiting for pool".to_string();
                                } else if let Some((headers, data)) =
                                    connection::execute_query_with_connection(
                                        self,
                                        connection_id,
                                        safe_query,
                                    )
                                {
                                    self.current_table_headers = headers;
                                    self.current_table_data = data.clone();
                                    self.all_table_data = data;
                                    // current_table_name sudah diset lebih awal
                                    self.is_table_browse_mode = true; // Enable filter for table browse
                                    self.sql_filter_text.clear(); // Clear any previous filter
                                    self.total_rows = self.all_table_data.len();
                                    self.current_page = 0;
                                    if let Some(active_tab) =
                                        self.query_tabs.get_mut(self.active_tab_index)
                                    {
                                        active_tab.result_headers =
                                            self.current_table_headers.clone();
                                        active_tab.result_rows = self.current_table_data.clone();
                                        active_tab.result_all_rows = self.all_table_data.clone();
                                        active_tab.result_table_name =
                                            self.current_table_name.clone();
                                        active_tab.is_table_browse_mode = self.is_table_browse_mode;
                                        active_tab.current_page = self.current_page;
                                        active_tab.page_size = self.page_size;
                                        active_tab.total_rows = self.total_rows;
                                    }
                                    // Save latest first page into row cache (best-effort)
                                    if let Some(dbn) = &database_name {
                                        let snapshot: Vec<Vec<String>> =
                                            self.all_table_data.iter().take(100).cloned().collect();
                                        let headers_clone = self.current_table_headers.clone();
                                        crate::cache_data::save_table_rows_to_cache(
                                            self,
                                            connection_id,
                                            dbn,
                                            &table_name,
                                            &headers_clone,
                                            &snapshot,
                                        );
                                        info!(
                                            "💾 Cached first 100 rows after live fetch for {}/{}",
                                            dbn, table_name
                                        );
                                    }
                                }
                            } else {
                                debug!(
                                    "🛑 Skipping client-side live load on table click because cache exists"
                                );
                                self.last_structure_target = None;
                            }
                        }
                    }
                };

                if is_view {
                    let ddl = connection::fetch_view_definition(
                        &conn,
                        database_name.as_deref(),
                        &table_name,
                    )
                    .unwrap_or_else(|| {
                        format!("-- Unable to fetch view definition for {}", table_name)
                    });
                    self.current_object_ddl = Some(ddl.clone());
                    if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                        active_tab.object_ddl = Some(ddl);
                    }
                    self.table_bottom_view = models::structs::TableBottomView::Query;
                }
            }

            // FIX: Jika user sedang berada pada view Structure dan berpindah klik ke table lain,
            // sebelumnya struktur tidak di-refresh sehingga masih menampilkan struktur table lama.
            // Di sini kita paksa reload struktur untuk table baru.
            if self.table_bottom_view == models::structs::TableBottomView::Structure {
                // Load only if target changed
                if let Some(conn_id) = self.current_connection_id {
                    let db = self
                        .query_tabs
                        .get(self.active_tab_index)
                        .and_then(|t| t.database_name.clone())
                        .unwrap_or_default();
                    let table = data_table::infer_current_table_name(self);
                    let current_target = (conn_id, db.clone(), table.clone());
                    if self
                        .last_structure_target
                        .as_ref()
                        .map(|t| t != &current_target)
                        .unwrap_or(true)
                    {
                        data_table::load_structure_info_for_current_table(self);
                    }
                } else {
                    data_table::load_structure_info_for_current_table(self);
                }
            } else {
                // Pastikan struktur lama dibersihkan agar ketika user pindah ke Structure langsung memicu load.
                self.structure_columns.clear();
                self.structure_indexes.clear();
            }
        }

        // Handle index click requests - open Edit Index dialog
        for (connection_id, index_name, database_name, table_name) in index_click_requests {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .cloned()
            {
                // Prefill dialog state for Edit
                if let Some(tn) = table_name.clone() {
                    self.index_dialog = Some(models::structs::IndexDialogState {
                        mode: models::structs::IndexDialogMode::Edit,
                        connection_id,
                        database_name: database_name.clone(),
                        table_name: tn,
                        existing_index_name: Some(index_name.clone()),
                        index_name: index_name.clone(),
                        columns: String::new(),
                        unique: false,
                        method: None,
                        db_type: conn.connection_type.clone(),
                    });
                    self.show_index_dialog = true;
                }
            }
        }

        // Handle create index requests - open Create Index dialog
        for (connection_id, database_name, table_name) in create_index_requests {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .cloned()
                && let Some(tn) = table_name.clone()
            {
                self.index_dialog = Some(models::structs::IndexDialogState {
                    mode: models::structs::IndexDialogMode::Create,
                    connection_id,
                    database_name: database_name.clone(),
                    table_name: tn.clone(),
                    existing_index_name: None,
                    index_name: format!("idx_{}_col", tn),
                    columns: "columns comma-separated".to_string(),
                    unique: false,
                    method: None,
                    db_type: conn.connection_type.clone(),
                });
                self.show_index_dialog = true;
            }
        }

        let results = query_files_to_open.clone();

        // Handle context menu requests (deduplicate to avoid multiple calls)
        let mut processed_removals = std::collections::HashSet::new();
        let mut processed_refreshes = std::collections::HashSet::new();
        let mut needs_full_refresh = false;

        for context_id in context_menu_requests {
            debug!("🔍 Processing context_id: {}", context_id);

            if context_id >= 50000 {
                // ID >= 50000 means create folder in folder operation
                let hash = context_id - 50000;
                debug!("📁 Create folder operation with hash: {}", hash);
                self.handle_create_folder_in_folder_request(hash);
                // Force immediate UI repaint after create folder request
                ui.ctx().request_repaint();
            } else if context_id >= 40000 {
                // ID >= 40000 means move query to folder operation
                let hash = context_id - 40000;
                debug!("📦 Move query operation with hash: {}", hash);
                sidebar_query::handle_query_move_request(self, hash);
            } else if context_id >= 20000 {
                // ID >= 20000 means query edit operation
                let hash = context_id - 20000;
                debug!("✏️ Query edit operation with hash: {}", hash);
                sidebar_query::handle_query_edit_request(self, hash);
            } else if context_id <= -50000 {
                // ID <= -50000 means remove folder operation
                let hash = (-context_id) - 50000;
                debug!("🗑️ Remove folder operation with hash: {}", hash);
                self.handle_remove_folder_request(hash);
                // Force immediate UI repaint after folder removal
                ui.ctx().request_repaint();
            } else if context_id <= -20000 {
                // ID <= -20000 means query removal operation
                let hash = (-context_id) - 20000;
                debug!("🗑️ Remove query operation with hash: {}", hash);
                if sidebar_query::handle_query_remove_request_by_hash(self, hash) {
                    // Force refresh of queries tree if removal was successful
                    sidebar_query::load_queries_from_directory(self);

                    // Force immediate UI repaint - this is crucial!
                    ui.ctx().request_repaint();

                    // Set needs_refresh flag to ensure UI updates
                    self.needs_refresh = true;
                }
            } else if context_id > 10000 {
                // ID > 10000 means copy connection (connection_id = context_id - 10000)
                let connection_id = context_id - 10000;
                debug!(
                    "📋 Copy connection operation for connection: {}",
                    connection_id
                );
                sidebar_database::copy_connection(self, connection_id);

                // Force immediate tree refresh and UI update
                self.items_tree.clear();
                sidebar_database::refresh_connections_tree(self);
                needs_full_refresh = true;
                ui.ctx().request_repaint();

                // Break early to prevent further processing
                break;
            } else if (3000..4000).contains(&context_id) {
                // ID 3000-3999 means disconnect (connection_id = context_id - 3000)
                let connection_id = context_id - 3000;
                debug!("🔌 Disconnect operation for connection: {}", connection_id);
                self.disconnect_connection(connection_id);
                // Mark for repaint so status updates immediately
                ui.ctx().request_repaint();
            } else if (1000..10000).contains(&context_id) {
                // ID 1000-9999 means refresh connection (connection_id = context_id - 1000)
                let connection_id = context_id - 1000;
                debug!(
                    "🔄 Refresh connection operation for connection: {}",
                    connection_id
                );
                if !processed_refreshes.contains(&connection_id) {
                    processed_refreshes.insert(connection_id);
                    // Only refresh that single connection node without rebuilding the whole tree
                    self.refresh_connection(connection_id);
                    // Mark for repaint so spinner state shows immediately
                    ui.ctx().request_repaint();
                    // Do NOT trigger full tree rebuild here; preserving folder expansion avoids the
                    // perception that the connection disappeared after refresh.
                }
            } else if context_id > 0 {
                // Positive ID means edit connection
                sidebar_database::start_edit_connection(self, context_id);
            } else {
                // Negative ID means remove connection
                let connection_id = -context_id;
                if !processed_removals.contains(&connection_id) {
                    processed_removals.insert(connection_id);
                    connection::remove_connection(self, connection_id);

                    // No need for full tree refresh - remove_connection already does incremental update
                    needs_full_refresh = true;
                    ui.ctx().request_repaint();

                    // Break early to prevent further processing
                    break;
                }
            }
        }

        // Force complete UI refresh after any removal
        if needs_full_refresh {
            // Completely clear and rebuild the tree
            self.items_tree.clear();
            sidebar_database::refresh_connections_tree(self);
            self.needs_refresh = true; // Set flag for next update cycle
            ui.ctx().request_repaint();

            // Return early to prevent any further processing of the old tree
            return Vec::new();
        }

        // Clean up processed folder removal mappings (optional - only if we want to prevent memory buildup)
        // We could also keep them for potential retry scenarios

        // Return query files that were clicked
        results
    }

    fn render_tree_node_with_table_expansion(
        ui: &mut egui::Ui,
        node: &mut models::structs::TreeNode,
        editor: &mut crate::editor_buffer::EditorBuffer,
        params: RenderTreeNodeParams,
    ) -> models::structs::RenderTreeNodeResult {
        let has_children = !node.children.is_empty();
        let mut expansion_request = None;
        let mut table_expansion = None;
        let mut context_menu_request = None;
        let mut table_click_request: Option<(i64, String, models::enums::NodeType, Option<String>)> = None;
        let mut folder_removal_mapping: Option<(i64, String)> = None;
        let mut connection_click_request = None;
        let mut query_file_to_open = None;
        let mut folder_name_for_removal = None;
        let mut parent_folder_for_creation = None;
        let mut dba_click_request: Option<(i64, models::enums::NodeType)> = None;
        let mut index_click_request: Option<(i64, String, Option<String>, Option<String>)> = None;
        let mut create_index_request: Option<(i64, Option<String>, Option<String>)> = None;
        let mut alter_table_request: Option<(i64, Option<String>, String)> = None;
        let mut drop_collection_request: Option<(i64, String, String)> = None;
        let mut drop_table_request: Option<(i64, String, String, String)> = None;
        let mut create_table_request: Option<(i64, Option<String>)> = None;
        let mut request_add_view_dialog: Option<i64> = None;
        let mut stored_procedure_click_request: Option<(i64, Option<String>, String)> = None;
        let mut generate_ddl_request: Option<(i64, Option<String>, String)> = None;
        let mut open_diagram_request: Option<(i64, String)> = None;
        let mut custom_view_click_request: Option<(i64, String, String)> = None;
        let mut delete_custom_view_request: Option<(i64, String)> = None;
        let mut edit_custom_view_request: Option<(i64, String, String)> = None;

        if has_children || node.node_type == models::enums::NodeType::Connection || node.node_type == models::enums::NodeType::Table ||
       node.node_type == models::enums::NodeType::View ||
        // Show expand toggles for container folders and schema folders only
       node.node_type == models::enums::NodeType::DatabasesFolder || node.node_type == models::enums::NodeType::TablesFolder ||
       node.node_type == models::enums::NodeType::ViewsFolder || node.node_type == models::enums::NodeType::StoredProceduresFolder ||
       node.node_type == models::enums::NodeType::UserFunctionsFolder || node.node_type == models::enums::NodeType::TriggersFolder ||
    node.node_type == models::enums::NodeType::EventsFolder || node.node_type == models::enums::NodeType::DBAViewsFolder ||
       // Do NOT show expand toggles for DBA leaf items; they act as actions when clicked
    node.node_type == models::enums::NodeType::Database || node.node_type == models::enums::NodeType::QueryFolder
        {
            // Use more unique ID including connection_id for connections
            let unique_id = match node.node_type {
                models::enums::NodeType::Connection => {
                    format!(
                        "conn_{}_{}",
                        params.node_index,
                        node.connection_id.unwrap_or(0)
                    )
                }
                _ => format!("node_{}_{:?}", params.node_index, node.node_type),
            };
            let id = egui::Id::new(&unique_id);
            ui.horizontal(|ui| {
                // Painter-drawn triangle toggle (no font dependency)
                if Self::triangle_toggle(ui, node.is_expanded).clicked() {
                    node.is_expanded = !node.is_expanded;

                    // If this is a connection node and not loaded, request expansion
                    if node.node_type == models::enums::NodeType::Connection
                        && !node.is_loaded
                        && node.is_expanded
                        && let Some(conn_id) = node.connection_id
                    {
                        expansion_request = Some(models::structs::ExpansionRequest {
                            node_type: models::enums::NodeType::Connection,
                            connection_id: conn_id,
                            database_name: None,
                        });
                        // Also set as active connection when expanding
                        connection_click_request = Some(conn_id);
                    }

                    // If this is a table or view node and not loaded, request column expansion
                    // In search mode, always allow expansion even if already loaded
                    if (node.node_type == models::enums::NodeType::Table
                        || node.node_type == models::enums::NodeType::View)
                        && node.is_expanded
                        && ((!node.is_loaded) || params.is_search_mode)
                        && let Some(conn_id) = node.connection_id
                    {
                        // Use stored raw table_name if present; otherwise sanitize display name (strip emojis / annotations)
                        let raw_name = node
                            .table_name
                            .clone()
                            .unwrap_or_else(|| Self::sanitize_display_table_name(&node.name));
                        table_expansion = Some((params.node_index, conn_id, raw_name));
                    }

                    // If this is a folder node and not loaded, request folder content expansion
                    if (node.node_type == models::enums::NodeType::DatabasesFolder
                        || node.node_type == models::enums::NodeType::TablesFolder
                        || node.node_type == models::enums::NodeType::ViewsFolder
                        || node.node_type == models::enums::NodeType::StoredProceduresFolder
                        || node.node_type == models::enums::NodeType::UserFunctionsFolder
                        || node.node_type == models::enums::NodeType::TriggersFolder
                        || node.node_type == models::enums::NodeType::EventsFolder
                        || node.node_type == models::enums::NodeType::ColumnsFolder
                        || node.node_type == models::enums::NodeType::IndexesFolder
                        || node.node_type == models::enums::NodeType::PrimaryKeysFolder
                        || node.node_type == models::enums::NodeType::PartitionsFolder)
                        && !node.is_loaded
                        && node.is_expanded
                        && let Some(conn_id) = node.connection_id
                    {
                        expansion_request = Some(models::structs::ExpansionRequest {
                            node_type: node.node_type.clone(),
                            connection_id: conn_id,
                            database_name: node.database_name.clone(),
                        });
                    }

                    // If this is a Database node and not loaded, request database expansion (for Redis keys)
                    if node.node_type == models::enums::NodeType::Database
                        && !node.is_loaded
                        && node.is_expanded
                        && let Some(conn_id) = node.connection_id
                    {
                        expansion_request = Some(models::structs::ExpansionRequest {
                            node_type: models::enums::NodeType::Database,
                            connection_id: conn_id,
                            database_name: node.database_name.clone(),
                        });
                    }
                }

                let icon = match node.node_type {
                    models::enums::NodeType::Database => "🗄",
                    models::enums::NodeType::Table => "",
                    // Use a plain bullet to avoid emoji font issues for column icons
                    models::enums::NodeType::Column => "•",
                    models::enums::NodeType::ColumnsFolder => "📑",
                    models::enums::NodeType::IndexesFolder => "🧭",
                    models::enums::NodeType::PrimaryKeysFolder => "🔑",
                    models::enums::NodeType::PartitionsFolder => "📊",
                    models::enums::NodeType::Index => "#",
                    models::enums::NodeType::Query => "🔍",
                    models::enums::NodeType::QueryHistItem => "📜",
                    models::enums::NodeType::Connection => "",
                    models::enums::NodeType::DatabasesFolder => "📁",
                    models::enums::NodeType::TablesFolder => "📋",
                    models::enums::NodeType::ViewsFolder => "👁",
                    models::enums::NodeType::StoredProceduresFolder => "📦",
                    models::enums::NodeType::CustomView => "👁️",
                    models::enums::NodeType::UserFunctionsFolder => "🔧",
                    models::enums::NodeType::TriggersFolder => "⚡",
                    models::enums::NodeType::EventsFolder => "📅",
                    models::enums::NodeType::DBAViewsFolder => "☢",
                    models::enums::NodeType::UsersFolder => "👥",
                    models::enums::NodeType::PrivilegesFolder => "🔒",
                    models::enums::NodeType::ProcessesFolder => "⚡",
                    models::enums::NodeType::StatusFolder => "📊",
                    models::enums::NodeType::BlockedQueriesFolder => "🚫",
                    models::enums::NodeType::ReplicationStatusFolder => "🔁",
                    models::enums::NodeType::MasterStatusFolder => "⭐",
                    models::enums::NodeType::MetricsUserActiveFolder => "👨‍💼",
                    models::enums::NodeType::View => "👁",
                    models::enums::NodeType::StoredProcedure => "⚛",
                    models::enums::NodeType::UserFunction => "🔧",
                    models::enums::NodeType::Trigger => "⚡",
                    models::enums::NodeType::Event => "📅",
                    models::enums::NodeType::MySQLFolder => "🐬",
                    models::enums::NodeType::PostgreSQLFolder => "🐘",
                    models::enums::NodeType::SQLiteFolder => "📄",
                    models::enums::NodeType::RedisFolder => "🔴",
                    models::enums::NodeType::MongoDBFolder => "🍃",
                    models::enums::NodeType::CustomFolder => "📁",
                    models::enums::NodeType::QueryFolder => "📂",
                    models::enums::NodeType::HistoryDateFolder => "📅",
                    models::enums::NodeType::MsSQLFolder => "🗳️",
                    models::enums::NodeType::DiagramsFolder => "📂",
                    models::enums::NodeType::Diagram => "🗺",
                };

                // Build status info for Connection nodes (used below)
                let (status_color, status_text) = if node.node_type == models::enums::NodeType::Connection {
                    if let Some(conn_id) = node.connection_id {
                        // Determine connected/connecting/disconnected
                        let mut has_shared = false;
                        if let Ok(shared) = params.shared_connection_pools.lock() {
                            has_shared = shared.contains_key(&conn_id);
                        }
                        if params.connection_pools.contains_key(&conn_id) || has_shared {
                            (egui::Color32::from_rgb(46, 204, 113), "Connected") // green
                        } else if params.pending_connection_pools.contains(&conn_id) {
                            (egui::Color32::from_rgb(241, 196, 15), "Connecting") // yellow
                        } else {
                            (egui::Color32::from_rgb(255, 30, 0), "Disconnected") // red
                        }
                    } else {
                        (egui::Color32::from_rgb(255, 30, 0), "Disconnected")
                    }
                } else { (ui.visuals().text_color(), "") };

                let mut response = if node.node_type == models::enums::NodeType::Connection {
                    // Draw colored status dot then a clickable, truncated label occupying full row width
                    ui.colored_label(status_color, egui::RichText::new("●").strong());
                    let mut name_text = node.name.clone();
                    if let Some(conn_id) = node.connection_id {
                        // Show refreshing spinner
                        if params.refreshing_connections.contains(&conn_id) {
                            name_text.push_str(" 🔄");
                        }
                        // Show prefetch progress
                        if let Some((completed, total)) = params.prefetch_progress.get(&conn_id) {
                            name_text.push_str(&format!(" 📦 {}/{}", completed, total));
                        }
                    }
                    // Use a regular left-aligned label (no explicit width) so text is not centered.
                    // truncate() will respect the remaining available width in this row.
                    ui.add(
                        egui::Label::new(name_text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                } else {
                    // Non-connection nodes: icon + name, truncated to available width and clickable
                    let label_text = if icon.is_empty() {
                        node.name.clone()
                    } else {
                        format!("{} {}", icon, node.name)
                    };
                    // Left-align non-connection labels as well; rely on parent row width for truncation.
                    ui.add(
                        egui::Label::new(label_text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                };

                // Tooltip for connection status
                if node.node_type == models::enums::NodeType::Connection && !status_text.is_empty() {
                    response = response.on_hover_text(format!("Status: {}", status_text));
                }

                // New: Allow clicking the label to also expand/collapse for expandable nodes
                if response.clicked() {
                    // We toggle on label click for expandable/container nodes, but not for Table/View (they open data)
                    let allow_label_toggle = has_children
                        || matches!(
                            node.node_type,
                            models::enums::NodeType::Connection
                                | models::enums::NodeType::Database
                                | models::enums::NodeType::DatabasesFolder
                                | models::enums::NodeType::TablesFolder
                                | models::enums::NodeType::ViewsFolder
                                | models::enums::NodeType::StoredProceduresFolder
                                | models::enums::NodeType::UserFunctionsFolder
                                | models::enums::NodeType::TriggersFolder
                                | models::enums::NodeType::EventsFolder
                                | models::enums::NodeType::DBAViewsFolder
                                | models::enums::NodeType::UsersFolder
                                | models::enums::NodeType::PrivilegesFolder
                                | models::enums::NodeType::ProcessesFolder
                                | models::enums::NodeType::StatusFolder
                                | models::enums::NodeType::BlockedQueriesFolder
                                | models::enums::NodeType::ReplicationStatusFolder
                                | models::enums::NodeType::MasterStatusFolder
                                | models::enums::NodeType::MetricsUserActiveFolder
                                | models::enums::NodeType::ColumnsFolder
                                | models::enums::NodeType::IndexesFolder
                                | models::enums::NodeType::PrimaryKeysFolder
                        ) && node.node_type != models::enums::NodeType::Table
                            && node.node_type != models::enums::NodeType::View;

                    if allow_label_toggle {
                        node.is_expanded = !node.is_expanded;

                        // Mirror triangle click behaviors for lazy-loading
                        if node.node_type == models::enums::NodeType::Connection
                            && !node.is_loaded
                            && node.is_expanded
                            && let Some(conn_id) = node.connection_id
                        {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: models::enums::NodeType::Connection,
                                connection_id: conn_id,
                                database_name: None,
                            });
                            // Also set as active connection when expanding
                            connection_click_request = Some(conn_id);
                        }

                        if (node.node_type == models::enums::NodeType::DatabasesFolder
                            || node.node_type == models::enums::NodeType::TablesFolder
                            || node.node_type == models::enums::NodeType::ViewsFolder
                            || node.node_type == models::enums::NodeType::StoredProceduresFolder
                            || node.node_type == models::enums::NodeType::UserFunctionsFolder
                            || node.node_type == models::enums::NodeType::TriggersFolder
                            || node.node_type == models::enums::NodeType::EventsFolder
                            || node.node_type == models::enums::NodeType::ColumnsFolder
                            || node.node_type == models::enums::NodeType::IndexesFolder
                            || node.node_type == models::enums::NodeType::PrimaryKeysFolder)
                            && !node.is_loaded
                            && node.is_expanded
                            && let Some(conn_id) = node.connection_id
                        {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: node.node_type.clone(),
                                connection_id: conn_id,
                                database_name: node.database_name.clone(),
                            });
                        }

                        // Database node expansion (e.g., Redis keys)
                        if node.node_type == models::enums::NodeType::Database
                            && !node.is_loaded
                            && node.is_expanded
                            && let Some(conn_id) = node.connection_id
                        {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: models::enums::NodeType::Database,
                                connection_id: conn_id,
                                database_name: node.database_name.clone(),
                            });
                        }
                    }
                }

                // Handle clicks on connection labels to set active connection
                if node.node_type == models::enums::NodeType::Connection
                    && response.clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    connection_click_request = Some(conn_id);
                }

                // Handle clicks on table/view labels to load data - open in new tab
                if (node.node_type == models::enums::NodeType::Table
                    || node.node_type == models::enums::NodeType::View)
                    && response.double_clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    // Use table_name field if available (for search results), otherwise use node.name
                    let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name).clone();
                    table_click_request = Some((conn_id, actual_table_name, node.node_type.clone(), node.database_name.clone()));
                }

                // Handle clicks on Diagram nodes
                if node.node_type == models::enums::NodeType::Diagram
                    && response.clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    open_diagram_request = Some((conn_id, node.database_name.clone().unwrap_or_default()));
                }

                // Index items: no left-click action; use context menu for Alter Index

                // Add context menu for connection nodes
                if node.node_type == models::enums::NodeType::Connection {
                    response.context_menu(|ui| {
                        if ui.button("📋 Copy Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 10000); // Use +10000 to indicate copy
                            }
                            ui.close();
                        }
                        if ui.button("🔄 Refresh Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use +1000 range to indicate refresh (handled in render_tree handler)
                                context_menu_request = Some(conn_id + 1000);
                            }
                            ui.close();
                        }
                        // NEW: Disconnect option
                        if ui.button("🔌 Disconnect").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use +3000 range to indicate disconnect (handled in render_tree handler)
                                context_menu_request = Some(conn_id + 3000);
                            }
                            ui.close();
                        }
                        if ui.button("🔧 Edit Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id);
                            }
                            ui.close();
                        }
                        if ui.button("🗑 Remove Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(-conn_id); // Negative ID indicates removal
                            }
                            ui.close();
                        }
                    });
                }

                // Add context menu for DBA Views folder
                if node.node_type == models::enums::NodeType::DBAViewsFolder {
                    response.context_menu(|ui| {
                        if ui.button("➕ Add New View").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Set state to show dialog
                                request_add_view_dialog = Some(conn_id);
                            }
                            ui.close();
                        }
                    });
                }

                // Add context menu for folder nodes
                if node.node_type == models::enums::NodeType::QueryFolder {
                    response.context_menu(|ui| {
                        if ui.button("📁 Create New Folder").clicked() {
                            // Store the parent folder name for creation
                            parent_folder_for_creation = Some(node.name.clone());
                            // Use ID range 50000+ for create folder in folder operations
                            let create_in_folder_id = 50000 + (node.name.len() as i64 % 1000);
                            context_menu_request = Some(create_in_folder_id);
                            ui.close();
                        }

                        if ui.button("🗑️ Remove Folder").clicked() {
                            // Store the full folder path for removal (relative to query dir)
                            if let Some(full_path) = &node.file_path {
                                let query_dir = directory::get_query_dir();
                                // Get relative path from query directory
                                let relative_path = std::path::Path::new(full_path)
                                    .strip_prefix(&query_dir)
                                    .unwrap_or(std::path::Path::new(&node.name))
                                    .to_string_lossy()
                                    .to_string();
                                folder_name_for_removal = Some(relative_path.clone());

                                // Use ID range -50000 for remove folder operations
                                let remove_folder_id = -50000 - (node.name.len() as i64 % 1000);
                                let hash = (-remove_folder_id) - 50000;
                                folder_removal_mapping = Some((hash, relative_path));
                                context_menu_request = Some(remove_folder_id);
                            } else {
                                // Fallback to just folder name for root folders
                                let folder_name = node.name.clone();
                                folder_name_for_removal = Some(folder_name.clone());

                                // Use ID range -50000 for remove folder operations
                                let remove_folder_id = -50000 - (node.name.len() as i64 % 1000);
                                let hash = (-remove_folder_id) - 50000;
                                folder_removal_mapping = Some((hash, folder_name));
                                context_menu_request = Some(remove_folder_id);
                            }
                            ui.close();
                        }
                    });
                }

                if node.node_type == models::enums::NodeType::Database {
                    response.context_menu(|ui| {
                        if let Some(conn_id) = node.connection_id {
                            let db_type = params.connection_types.get(&conn_id);
                            let supported = matches!(
                                db_type,
                                Some(models::enums::DatabaseType::MySQL)
                                    | Some(models::enums::DatabaseType::PostgreSQL)
                                    | Some(models::enums::DatabaseType::SQLite)
                                    | Some(models::enums::DatabaseType::MsSQL)
                            );
                            if supported {
                                if ui.button("➕ Create New Table").clicked() {
                                    let database_name = node
                                        .database_name
                                        .clone()
                                        .or_else(|| Some(node.name.clone()));
                                    create_table_request = Some((conn_id, database_name));
                                    ui.close();
                                }
                                if ui.button("📊 Diagrams").clicked() {
                                    let database_name = node
                                        .database_name
                                        .clone()
                                        .or_else(|| Some(node.name.clone()))
                                        .unwrap_or_default();
                                    open_diagram_request = Some((conn_id, database_name));
                                    ui.close();
                                }
                            } else {
                                ui.label("Create table not supported for this database");
                            }
                        }
                    });
                }

                if node.node_type == models::enums::NodeType::TablesFolder {
                    response.context_menu(|ui| {
                        if let Some(conn_id) = node.connection_id {
                            let db_type = params.connection_types.get(&conn_id);
                            let supported = matches!(
                                db_type,
                                Some(models::enums::DatabaseType::MySQL)
                                    | Some(models::enums::DatabaseType::PostgreSQL)
                                    | Some(models::enums::DatabaseType::SQLite)
                                    | Some(models::enums::DatabaseType::MsSQL)
                            );
                            if supported {
                                if ui.button("➕ Create New Table").clicked() {
                                    create_table_request = Some((
                                        conn_id,
                                        node.database_name.clone(),
                                    ));
                                    ui.close();
                                }
                            } else {
                                ui.label("Create table not supported for this database");
                            }
                        }
                    });
                }

                // Add context menu for table nodes
                if node.node_type == models::enums::NodeType::Table {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                table_click_request = Some((conn_id, actual_table_name, models::enums::NodeType::Table, node.database_name.clone()));
                            }
                            ui.close();
                        }
                        // Detect DB type for MongoDB-specific options using available pools; fallback to connection_types
                        let mut is_mongodb = false;
                        if let Some(conn_id) = node.connection_id {
                            if let Some(pool) = params.connection_pools.get(&conn_id) {
                                if let models::enums::DatabasePool::MongoDB(_) = pool {
                                    is_mongodb = true;
                                }
                            } else if let Some(t) = params.connection_types.get(&conn_id)
                                && *t == models::enums::DatabaseType::MongoDB {
                                    is_mongodb = true;
                                }
                        }

                        if !is_mongodb {
                            if ui.button("📜 Generate Query Create Table").clicked() {
                                if let Some(conn_id) = node.connection_id {
                                    let actual_table_name =
                                        node.table_name.as_ref().unwrap_or(&node.name).clone();
                                    generate_ddl_request = Some((
                                        conn_id,
                                        node.database_name.clone(),
                                        actual_table_name,
                                    ));
                                }
                                ui.close();
                            }
                        } else {
                            // MongoDB specific quick actions
                            if ui.button("🔍 Count Documents (Current Tab)").clicked() {
                                if let Some(db) = node.database_name.as_ref() {
                                    let coll = node.table_name.as_ref().unwrap_or(&node.name);
                                    editor.set_text(format!("// MongoDB mongo shell snippet\ndb.{}.{}.countDocuments({{}});", db, coll));
                                } else {
                                    editor.set_text("// Select a database first for MongoDB operations".to_string());
                                }
                                editor.mark_text_modified();
                                ui.close();
                            }
                            if ui.button("📝 Show Collection Stats (Current Tab)").clicked() {
                                if let Some(db) = node.database_name.as_ref() {
                                    let coll = node.table_name.as_ref().unwrap_or(&node.name);
                                    editor.set_text(format!("db.{}.runCommand({{ collStats: \"{}\" }});", db, coll));
                                } else {
                                    editor.set_text("// Select a database first for MongoDB operations".to_string());
                                }
                                editor.mark_text_modified();
                                ui.close();
                            }
                        }
                        ui.separator();
                        if !is_mongodb {
                            if ui.button("🗑 Drop Table").clicked() {
                                if let (Some(conn_id), Some(db)) = (node.connection_id, node.database_name.as_ref()) {
                                    let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name).clone();
                                    // Generate the DROP TABLE statement with USE database
                                    let stmt = format!("USE [{}];\nDROP TABLE IF EXISTS {};", db, actual_table_name);
                                    drop_table_request = Some((conn_id, db.clone(), actual_table_name, stmt));
                                }
                                ui.close();
                            }
                        } else if ui.button("🗑️ Drop Collection").clicked() {
                            if let (Some(conn_id), Some(db)) = (node.connection_id, node.database_name.as_ref()) {
                                let coll = node.table_name.as_ref().unwrap_or(&node.name).clone();
                                drop_collection_request = Some((conn_id, db.clone(), coll));
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("➕ Add Index (New Tab)").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                create_index_request = Some((
                                    conn_id,
                                    node.database_name.clone(),
                                    Some(actual_table_name),
                                ));
                            }
                            ui.close();
                        }
                        ui.separator();
                        if !is_mongodb && ui.button("🔧 Alter Table").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                alter_table_request = Some((
                                    conn_id,
                                    node.database_name.clone(),
                                    actual_table_name,
                                ));
                            }
                            ui.close();
                        }
                    });
                }

                // Add context menu for view nodes
                if node.node_type == models::enums::NodeType::View {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                table_click_request = Some((conn_id, actual_table_name, models::enums::NodeType::View, node.database_name.clone()));
                            }
                            ui.close();
                        }
                        if ui.button("📝 DESCRIBE View (Current Tab)").clicked() {
                            // Different DESCRIBE syntax for different database types
                            if node.database_name.is_some() {
                                editor.set_text(format!("DESCRIBE {};", node.name));
                            } else {
                                editor.set_text(format!("PRAGMA table_info({});", node.name)); // SQLite syntax
                            }
                            editor.mark_text_modified();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("🗂️ Show Columns").clicked() {
                            // Trigger table expansion to show columns
                            if let Some(conn_id) = node.connection_id {
                                table_expansion = Some((0, conn_id, node.name.clone()));
                            }
                            ui.close();
                        }
                    });
                }

                // Context menu for Indexes folder: create index
                if node.node_type == models::enums::NodeType::IndexesFolder {
                    response.context_menu(|ui| {
                        if ui.button("➕ New Index").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                create_index_request = Some((
                                    conn_id,
                                    node.database_name.clone(),
                                    node.table_name.clone(),
                                ));
                            }
                            ui.close();
                        }
                    });
                }

                // Context menu for Index node: edit index
                if node.node_type == models::enums::NodeType::Index {
                    response.context_menu(|ui| {
                        if ui.button("✏️ Edit Index").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                index_click_request = Some((
                                    conn_id,
                                    node.name.clone(),
                                    node.database_name.clone(),
                                    node.table_name.clone(),
                                ));
                            }
                            ui.close();
                        }
                    });
                }
            });

            // (central panel logic handled inside update previously)

            if node.is_expanded {
                // Khusus HistoryDateFolder: render children tanpa indent tambahan (full width)
                let is_history_date_folder =
                    node.node_type == models::enums::NodeType::HistoryDateFolder;
                if is_history_date_folder {
                    for (child_index, child) in node.children.iter_mut().enumerate() {
                        let (
                            child_expansion_request,
                            child_table_expansion,
                            child_context,
                            child_table_click,
                            _child_connection_click,
                            _child_query_file,
                            _child_folder_removal,
                            _child_parent_creation,
                            _child_folder_removal_mapping,
                            child_dba_click,
                            child_index_click,
                            child_create_index_request,
                            child_alter_table_request,
                            _child_drop_collection_request,
                            _child_drop_table_request,
                            _child_create_table_request,
                            _child_sp_click,
                            child_generate_ddl_request,
                            child_open_diagram_request,
                            child_request_add_view_dialog,
                            child_custom_view_click_request,
                            child_delete_custom_view,
                            child_edit_custom_view,
                        ) = Self::render_tree_node_with_table_expansion(
                            ui,
                            child,
                            editor,
                            RenderTreeNodeParams {
                                node_index: child_index,
                                refreshing_connections: params.refreshing_connections,
                                connection_pools: params.connection_pools,
                                pending_connection_pools: params.pending_connection_pools,
                                shared_connection_pools: params.shared_connection_pools,
                                is_search_mode: params.is_search_mode,
                                connection_types: params.connection_types,
                                prefetch_progress: params.prefetch_progress,
                            },
                        );
                        if let Some(child_expansion) = child_expansion_request {
                            expansion_request = Some(child_expansion);
                        }
                        if table_expansion.is_none()
                            && let Some((child_index, child_conn_id, table_name)) =
                                child_table_expansion
                        {
                            if let Some(conn_id) = node.connection_id {
                                table_expansion = Some((child_index, conn_id, table_name));
                            } else {
                                table_expansion = Some((child_index, child_conn_id, table_name));
                            }
                        }
                        if let Some((conn_id, table_name, node_type, db_name)) = child_table_click {
                            table_click_request = Some((conn_id, table_name, node_type, db_name));
                        }
                        if let Some(v) = _child_drop_collection_request {
                            drop_collection_request = Some(v);
                        }
                        if let Some(v) = _child_drop_table_request {
                            drop_table_request = Some(v);
                        }
                        if let Some(v) = child_dba_click {
                            dba_click_request = Some(v);
                        }
                        if let Some(v) = child_index_click {
                            index_click_request = Some(v);
                        }
                        if let Some(v) = child_create_index_request {
                            create_index_request = Some(v);
                        }
                        if let Some(v) = child_alter_table_request {
                            alter_table_request = Some(v);
                        }
                        if let Some(v) = _child_create_table_request {
                            create_table_request = Some(v);
                        }
                        if let Some(v) = _child_sp_click {
                            stored_procedure_click_request = Some(v);
                        }
                        if let Some(v) = child_generate_ddl_request {
                            generate_ddl_request = Some(v);
                        }
                        if let Some(v) = child_open_diagram_request {
                            open_diagram_request = Some(v);
                        }
                        if let Some(child_context_id) = child_context {
                            context_menu_request = Some(child_context_id);
                        }
                        // Propagate child query file open requests (History) to parent
                        if let Some(child_query_file) = _child_query_file {
                            query_file_to_open = Some(child_query_file);
                        }
                        if let Some(child_req) = child_request_add_view_dialog {
                            request_add_view_dialog = Some(child_req);
                        }
                        if let Some(child_req) = child_custom_view_click_request {
                            custom_view_click_request = Some(child_req);
                        }
                        if let Some(child_req) = child_delete_custom_view {
                             delete_custom_view_request = Some(child_req);
                        }
                        if let Some(child_req) = child_edit_custom_view {
                             edit_custom_view_request = Some(child_req);
                        }
                    }
                } else {
                    ui.indent(id, |ui| {
                        for (child_index, child) in node.children.iter_mut().enumerate() {
                            let (
                                child_expansion_request,
                                child_table_expansion,
                                child_context,
                                child_table_click,
                                _child_connection_click,
                                _child_query_file,
                                _child_folder_removal,
                                _child_parent_creation,
                                _child_folder_removal_mapping,
                                child_dba_click,
                                child_index_click,
                                child_create_index_request,
                                child_alter_table_request,
                                _child_drop_collection_request,
                                child_drop_table_request,
                                child_create_table_request,
                                child_stored_procedure_click_request,
                                child_generate_ddl_request,
                                child_open_diagram_request,
                                child_request_add_view_dialog,
                                child_custom_view_click_request,
                                child_delete_custom_view_request,
                                child_edit_custom_view_request,
                            ) = Self::render_tree_node_with_table_expansion(
                                ui,
                                child,
                                editor,
                                RenderTreeNodeParams {
                                    node_index: child_index,
                                    refreshing_connections: params.refreshing_connections,
                                    connection_pools: params.connection_pools,
                                    pending_connection_pools: params.pending_connection_pools,
                                    shared_connection_pools: params.shared_connection_pools,
                                    is_search_mode: params.is_search_mode,
                                    connection_types: params.connection_types,
                                    prefetch_progress: params.prefetch_progress,
                                },
                            );

                            // Handle child expansion requests - propagate to parent
                            if let Some(child_expansion) = child_expansion_request {
                                expansion_request = Some(child_expansion);
                            }

                            // Handle child table expansions with the parent connection ID
                            // Only set if we don't already have a table expansion from this node
                            if table_expansion.is_none()
                                && let Some((child_index, child_conn_id, table_name)) =
                                    child_table_expansion
                            {
                                if let Some(conn_id) = node.connection_id {
                                    table_expansion = Some((child_index, conn_id, table_name));
                                } else {
                                    table_expansion =
                                        Some((child_index, child_conn_id, table_name));
                                }
                            }

                            // Handle child table clicks - propagate to parent
                            if let Some((conn_id, table_name, node_type, db_name)) = child_table_click {
                                table_click_request = Some((conn_id, table_name, node_type, db_name));
                            }
                            // Propagate drop collection request to parent
                            if let Some(v) = _child_drop_collection_request {
                                drop_collection_request = Some(v);
                            }
                            // Propagate drop table request to parent
                            if let Some(v) = child_drop_table_request {
                                drop_table_request = Some(v);
                            }
                            // Propagate DBA click to parent
                            if let Some(v) = child_dba_click {
                                dba_click_request = Some(v);
                            }
                            if let Some(v) = child_index_click {
                                index_click_request = Some(v);
                            }
                            if let Some(v) = child_create_index_request {
                                create_index_request = Some(v);
                            }
                            if let Some(v) = child_alter_table_request {
                                alter_table_request = Some(v);
                            }
                            if let Some(v) = child_create_table_request {
                                create_table_request = Some(v);
                            }
                            if let Some(v) = child_stored_procedure_click_request {
                                stored_procedure_click_request = Some(v);
                            }
                            if let Some(v) = child_generate_ddl_request {
                                generate_ddl_request = Some(v);
                            }
                            if let Some(v) = child_open_diagram_request {
                                open_diagram_request = Some(v);
                            }
                            if let Some(child_req) = child_request_add_view_dialog {
                                request_add_view_dialog = Some(child_req);
                            }
                            if let Some(child_req) = child_custom_view_click_request {
                                custom_view_click_request = Some(child_req);
                            }
                            if let Some(child_req) = child_delete_custom_view_request {
                                delete_custom_view_request = Some(child_req);
                            }
                            if let Some(child_req) = child_edit_custom_view_request {
                                edit_custom_view_request = Some(child_req);
                            }

                            // Handle child folder removal - propagate to parent
                            if let Some(child_folder_name) = _child_folder_removal {
                                folder_name_for_removal = Some(child_folder_name);
                            }

                            // Handle child parent folder creation - propagate to parent
                            if let Some(child_parent) = _child_parent_creation {
                                parent_folder_for_creation = Some(child_parent);
                            }

                            // Handle child folder removal mapping - propagate to parent
                            if let Some(child_mapping) = _child_folder_removal_mapping {
                                folder_removal_mapping = Some(child_mapping);
                            }

                            // Handle child query file open requests - propagate to parent
                            if let Some(child_query_file) = _child_query_file {
                                query_file_to_open = Some(child_query_file);
                            }

                            // Handle child context menu requests - propagate to parent
                            if let Some(child_context_id) = child_context {
                                context_menu_request = Some(child_context_id);
                            }
                        }
                    });
                }
            }
        } else {
            let response = if node.node_type == models::enums::NodeType::QueryHistItem {
                // Special handling for history items - make the entire area clickable
                let available_width = ui.available_width();
                let button_response = ui.add_sized(
                    [
                        available_width,
                        ui.text_style_height(&egui::TextStyle::Body),
                    ],
                    egui::Button::new(format!("📜  {}", node.name))
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                );

                // Add tooltip with the full query if available
                if let Some(data) = &node.file_path {
                    if let Some((connection_name, original_query)) = data.split_once("||") {
                        button_response.on_hover_text_at_pointer(format!(
                            "Connection: {}\nFull query:\n{}",
                            connection_name, original_query
                        ))
                    } else {
                        button_response.on_hover_text_at_pointer(format!("Full query:\n{}", data))
                    }
                } else {
                    button_response
                }
            } else {
                // For all other node types, use horizontal layout with icons.
                // Add a spacer equal to the triangle width so leaf rows align with expandable rows (left-aligned look).
                ui.horizontal(|ui| {
                    // Reserve space equal to triangle toggle width (16px) for alignment
                    let _sp = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());

                    let icon = match node.node_type {
                        models::enums::NodeType::Database => "🗄",
                        models::enums::NodeType::Table => "",
                        // Use a plain bullet again for columns in fallback rendering
                        models::enums::NodeType::Column => "•",
                        models::enums::NodeType::Query => "🔍",
                        models::enums::NodeType::Connection => "🔗",
                        models::enums::NodeType::DatabasesFolder => "📁",
                        models::enums::NodeType::TablesFolder => "📋",
                        models::enums::NodeType::ViewsFolder => "👁",
                        models::enums::NodeType::StoredProceduresFolder => "📦",
                        models::enums::NodeType::UserFunctionsFolder => "🔧",
                        models::enums::NodeType::TriggersFolder => "⚡",
                        models::enums::NodeType::EventsFolder => "📅",
                        models::enums::NodeType::DBAViewsFolder => "☢",
                        models::enums::NodeType::UsersFolder => "👥",
                        models::enums::NodeType::PrivilegesFolder => "🔒",
                        models::enums::NodeType::ProcessesFolder => "⚡",
                        models::enums::NodeType::StatusFolder => "📊",
                        models::enums::NodeType::BlockedQueriesFolder => "🚫",
                        models::enums::NodeType::ReplicationStatusFolder => "🔁",
                        models::enums::NodeType::MasterStatusFolder => "⭐",
                        models::enums::NodeType::View => "👁",
                        models::enums::NodeType::StoredProcedure => "⚛",
                        models::enums::NodeType::UserFunction => "🔧",
                        models::enums::NodeType::Trigger => "⚡",
                        models::enums::NodeType::Event => "📅",
                        models::enums::NodeType::MySQLFolder => "🐬",
                        models::enums::NodeType::PostgreSQLFolder => "🐘",
                        models::enums::NodeType::SQLiteFolder => "📄",
                        models::enums::NodeType::RedisFolder => "🔴",
                        models::enums::NodeType::MongoDBFolder => "🍃",
                        models::enums::NodeType::MsSQLFolder => "⛁",
                        models::enums::NodeType::CustomFolder => "📁",
                        models::enums::NodeType::QueryFolder => "📂",
                        models::enums::NodeType::HistoryDateFolder => "📅",
                        models::enums::NodeType::ColumnsFolder => "📑",
                        models::enums::NodeType::IndexesFolder => "🧭",
                        models::enums::NodeType::PrimaryKeysFolder => "🔑",
                        models::enums::NodeType::PartitionsFolder => "📊",
                        models::enums::NodeType::Index => "#",
                        _ => "🧾",
                    };

                    let label_text = format!("{} {}", icon, node.name);
                    // Use left-aligned label without forcing a full-row size to avoid centered look.
                    ui.add(
                        egui::Label::new(label_text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                })
                .inner
            };

            if response.clicked() {
                debug!(
                    "🎯 CLICK DETECTED! Node type: {:?}, Name: {}",
                    node.node_type, node.name
                );
                // Handle node selection
                match node.node_type {
                    models::enums::NodeType::Table | models::enums::NodeType::View => {
                        // Don't modify current editor_text, we'll create a new tab
                        // Just trigger table data loading
                        if let Some(conn_id) = node.connection_id {
                            let actual_table_name =
                                node.table_name.as_ref().unwrap_or(&node.name).clone();
                            table_click_request =
                                Some((conn_id, actual_table_name, node.node_type.clone(), node.database_name.clone()));
                        }
                    }
                    // DBA quick views: emit a click request to be handled by parent (needs self)
                    // Unified View Processing (DBA Views + Custom Views)
                    models::enums::NodeType::UsersFolder
                    | models::enums::NodeType::PrivilegesFolder
                    | models::enums::NodeType::ProcessesFolder
                    | models::enums::NodeType::StatusFolder
                    | models::enums::NodeType::BlockedQueriesFolder
                    | models::enums::NodeType::ReplicationStatusFolder
                    | models::enums::NodeType::MasterStatusFolder
                    | models::enums::NodeType::MetricsUserActiveFolder
                    | models::enums::NodeType::CustomView => {
                        debug!("👁️ View clicked: {}", node.name);
                        if let Some(query) = &node.query {
                           // Use the robust execution path
                           if let Some(conn_id) = node.connection_id {
                                custom_view_click_request = Some((conn_id, node.name.clone(), query.clone()));
                           }
                        }
                    }
                    models::enums::NodeType::Query => {
                        // Load query file content
                        debug!("🔍 Query node clicked: {}", node.name);
                        if let Some(file_path) = &node.file_path {
                            debug!("📁 File path: {}", file_path);
                            if let Ok(content) = std::fs::read_to_string(file_path) {
                                debug!(
                                    "✅ File read successfully, content length: {}",
                                    content.len()
                                );
                                // Don't modify editor_text directly, let open_query_file handle it
                                query_file_to_open =
                                    Some((node.name.clone(), content, file_path.clone()));
                            } else {
                                debug!("❌ Failed to read file: {}", file_path);
                                // Handle read error case
                                query_file_to_open = Some((
                                    node.name.clone(),
                                    format!("-- Failed to load query file: {}", node.name),
                                    file_path.clone(),
                                ));
                            }
                        } else {
                            debug!("❌ No file path for query node: {}", node.name);
                            // Handle missing file path case - create a placeholder query
                            let placeholder_content =
                                format!("-- {}\nSELECT * FROM table_name;", node.name);
                            // For files without path, we'll create a new unsaved tab
                            query_file_to_open =
                                Some((node.name.clone(), placeholder_content, String::new()));
                        }
                    }
                    models::enums::NodeType::QueryHistItem => {
                        debug!("🖱️ QueryHistItem clicked: {}", node.name);
                        // For history items, create a new tab with the original query
                        if let Some(data) = &node.file_path {
                            // Parse connection name and query from the stored data
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                // Create a descriptive tab title based on the query type
                                let tab_title = if original_query.len() > 50 {
                                    format!(
                                        "History: {}...",
                                        &original_query[0..50].replace("\n", " ").trim()
                                    )
                                } else {
                                    format!("History: {}", original_query.replace("\n", " ").trim())
                                };
                                // Collect to be handled by parent (render_tree) -> will create a NEW TAB
                                debug!(
                                    "📝 Setting query_file_to_open (history): title='{}', query_len={}",
                                    tab_title,
                                    original_query.len()
                                );
                                // Pass the original data (connection_name||query) in the 3rd field so caller can bind connection
                                query_file_to_open =
                                    Some((tab_title, original_query.to_string(), data.clone()));
                            } else {
                                debug!("📝 Using fallback format for old history item");
                                // Fallback for old format without connection name
                                query_file_to_open = Some((
                                    "History Query".to_string(),
                                    data.clone(),
                                    String::new(),
                                ));
                            }
                        } else {
                            debug!("❌ No file_path data for history item");
                            // Fallback to display name if no original query stored
                        }
                    }
                    models::enums::NodeType::StoredProcedure => {
                        if let Some(conn_id) = node.connection_id {
                            stored_procedure_click_request =
                                Some((conn_id, node.database_name.clone(), node.name.clone()));
                        }
                    }
                    _ => {}
                }
            }


            // Add context menu for query nodes
            if node.node_type == models::enums::NodeType::Query {
                response.context_menu(|ui| {
                    if ui.button("Edit Query").clicked() {
                        if let Some(file_path) = &node.file_path {
                            // Use the file path directly as context identifier
                            // Format: 20000 + simple index to differentiate from connections
                            let edit_id = 20000 + (file_path.len() as i64 % 1000); // Simple deterministic ID
                            context_menu_request = Some(edit_id);
                        }
                        ui.close();
                    }

                    if ui.button("Move to Folder").clicked() {
                        if let Some(file_path) = &node.file_path {
                            // Use a different ID range for move operations
                            let move_id = 40000 + (file_path.len() as i64 % 1000);
                            context_menu_request = Some(move_id);
                        }
                        ui.close();
                    }

                    if ui.button("Remove Query").clicked() {
                        if let Some(file_path) = &node.file_path {
                            // Use the file path directly as context identifier
                            // Format: -20000 - simple index to differentiate from connections
                            let remove_id = -20000 - (file_path.len() as i64 % 1000); // Simple deterministic ID
                            context_menu_request = Some(remove_id);
                        }
                        ui.close();
                    }
                });
            }

            // Add context menu for history items
            if node.node_type == models::enums::NodeType::QueryHistItem {
                response.context_menu(|ui| {
                    if ui.button("📋 Copy Query").clicked() {
                        if let Some(data) = &node.file_path {
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                ui.ctx().copy_text(original_query.to_string());
                            } else {
                                ui.ctx().copy_text(data.clone());
                            }
                        }
                        ui.close();
                    }

                    if ui.button("▶️ Execute Query").clicked() {
                        if let Some(data) = &node.file_path {
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                editor.set_text(original_query.to_string());
                            } else {
                                editor.set_text(data.clone());
                            }
                            editor.mark_text_modified();
                            // This will trigger the execution flow when the context menu closes
                        }
                        ui.close();
                    }

                    if ui.button("🔁 Auto Refresh Execute").clicked() {
                        if let Some(data) = &node.file_path {
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                editor.set_text(original_query.to_string());
                            } else {
                                editor.set_text(data.clone());
                            }
                            editor.mark_text_modified();
                            // Store query + connection for auto refresh; central UI will read these fields
                            if let Some(conn_id) = node.connection_id {
                                let query_text = editor.text_snapshot();
                                ui.ctx().data_mut(|data| {
                                    data.insert_persisted(
                                        egui::Id::new("auto_refresh_request_conn_id"),
                                        conn_id,
                                    );
                                    data.insert_persisted(
                                        egui::Id::new("auto_refresh_request_query"),
                                        query_text,
                                    );
                                });
                            }
                        }
                        ui.close();
                    }
                });
            }


            // Add context menu for Custom View items
            if node.node_type == models::enums::NodeType::CustomView {
                response.context_menu(|ui| {
                     if ui.button("✏️ Edit this view").clicked() {
                         if let Some(conn_id) = node.connection_id
                             && let Some(query) = &node.query {
                                 edit_custom_view_request = Some((conn_id, node.name.clone(), query.clone()));
                             }
                         ui.close();
                     }
                     if ui.button("🗑️ Delete this dba view").clicked() {
                         if let Some(conn_id) = node.connection_id {
                             delete_custom_view_request = Some((conn_id, node.name.clone()));
                         }
                         ui.close();
                     }
                });
            }

            // Add context menu for Index nodes (non-expandable branch)
            if node.node_type == models::enums::NodeType::Index {
                response.context_menu(|ui| {
                    if ui.button("✏️ Edit Index").clicked() {
                        if let Some(conn_id) = node.connection_id {
                            index_click_request = Some((
                                conn_id,
                                node.name.clone(),
                                node.database_name.clone(),
                                node.table_name.clone(),
                            ));
                        }
                        ui.close();
                    }
                });
            }
        }

        (
            expansion_request,
            table_expansion,
            context_menu_request,
            table_click_request,
            connection_click_request,
            query_file_to_open,
            folder_name_for_removal,
            parent_folder_for_creation,
            folder_removal_mapping,
            dba_click_request,
            index_click_request,
            create_index_request,
            alter_table_request,
            drop_collection_request,
            drop_table_request,
            create_table_request,
            stored_procedure_click_request,
            generate_ddl_request,
            open_diagram_request,
            request_add_view_dialog,
            custom_view_click_request,
            delete_custom_view_request,
            edit_custom_view_request,
        )
    }


    // Sanitize a display table name (with icons / annotations) back to the raw table name suitable for SQL queries
    fn sanitize_display_table_name(display: &str) -> String {
        // Remove leading known emoji + whitespace
        let mut s = display.trim_start();
        for prefix in ["📋", "📁", "🔧", "🗄", "•", "#", "📑"] {
            // extend as needed
            if s.starts_with(prefix) {
                s = s[prefix.len()..].trim_start();
            }
        }
        // Truncate at first " (" which denotes annotations like "(table name match)" or column counts
        if let Some(pos) = s.find(" (") {
            s[..pos].trim().to_string()
        } else {
            s.to_string()
        }
    }



    
    fn handle_alter_table_request(
        &mut self,
        connection_id: i64,
        database_name: Option<String>,
        table_name: String,
    ) {
        debug!(
            "🔍 handle_alter_table_request called with connection_id: {}, table: {}",
            connection_id, table_name
        );

        let connection = match self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .cloned()
        {
            Some(conn) => conn,
            None => {
                debug!("❌ Connection with ID {} not found", connection_id);
                return;
            }
        };

        let resolved_db = database_name
            .filter(|db| !db.trim().is_empty())
            .or_else(|| {
                let default_db = connection.database.trim();
                if default_db.is_empty() {
                    None
                } else {
                    Some(connection.database.clone())
                }
            });

        let table_title = format!("Table: {}", table_name);
        let matches_target = |tab: &models::structs::QueryTab| {
            tab.title == table_title
                && tab.connection_id == Some(connection_id)
                && match (&resolved_db, &tab.database_name) {
                    (Some(expected), Some(existing)) => expected == existing,
                    (Some(_), None) => false,
                    _ => true,
                }
        };

        if let Some((existing_index, _)) = self
            .query_tabs
            .iter()
            .enumerate()
            .find(|(_, tab)| matches_target(tab))
        {
            if existing_index != self.active_tab_index {
                editor::switch_to_tab(self, existing_index);
            }
        } else {
            editor::create_new_tab_with_connection_and_database(
                self,
                table_title.clone(),
                String::new(),
                Some(connection_id),
                resolved_db.clone(),
            );
        }

        self.current_connection_id = Some(connection_id);
        self.is_table_browse_mode = true;

        let caption = resolved_db
            .as_ref()
            .map(|db| format!("Table: {} (Database: {})", table_name, db))
            .unwrap_or_else(|| format!("Table: {}", table_name));

        self.current_table_name = caption.clone();
        self.table_bottom_view = models::structs::TableBottomView::Structure;
        self.structure_sub_view = models::structs::StructureSubView::Columns;
        self.last_structure_target = None;
        self.request_structure_refresh = false;

        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.title = table_title;
            active_tab.connection_id = Some(connection_id);
            if resolved_db.is_some() {
                active_tab.database_name = resolved_db.clone();
            }
            active_tab.is_table_browse_mode = true;
            active_tab.result_table_name = caption.clone();
        }

        data_table::load_structure_info_for_current_table(self);
    }

    fn handle_create_folder_in_folder_request(&mut self, _hash: i64) {
        debug!(
            "🔍 handle_create_folder_in_folder_request called with hash: {}",
            _hash
        );
        // Parent folder should already be set when context menu was clicked
        if self.parent_folder_for_creation.is_some() {
            // Show the create folder dialog
            self.show_create_folder_dialog = true;
        } else {
            debug!("❌ No parent folder set for creation! This should not happen.");
            self.error_message = "No parent folder selected for creation".to_string();
            self.show_error_message = true;
        }
    }

    fn handle_remove_folder_request(&mut self, hash: i64) {
        // Look up the folder path using the hash
        if let Some(folder_relative_path) = self.folder_removal_map.get(&hash).cloned() {
            let query_dir = directory::get_query_dir();
            let folder_path = query_dir.join(&folder_relative_path);

            if folder_path.exists() && folder_path.is_dir() {
                // Check if folder is empty (recursively)
                let is_empty = Self::is_directory_empty(&folder_path);

                if is_empty {
                    // Remove empty folder
                    match std::fs::remove_dir(&folder_path) {
                        Ok(()) => {
                            // Refresh the queries tree
                            sidebar_query::load_queries_from_directory(self);
                            // Force UI refresh
                            self.needs_refresh = true;
                        }
                        Err(e) => {
                            debug!("❌ Failed to remove folder: {}", e);
                            self.error_message = format!(
                                "Failed to remove folder '{}': {}",
                                folder_relative_path, e
                            );
                            self.show_error_message = true;
                        }
                    }
                } else {
                    // Offer option to remove folder and all contents
                    self.error_message = format!(
                        "Folder '{}' is not empty.\n\nWould you like to remove it and all its contents?",
                        folder_relative_path
                    );
                    self.show_error_message = true;
                    debug!(
                        "❌ Cannot remove non-empty folder: {}",
                        folder_relative_path
                    );
                }
            } else {
                self.error_message = format!("Folder '{}' does not exist", folder_relative_path);
                self.show_error_message = true;
                debug!("❌ Folder does not exist: {}", folder_relative_path);
            }

            // Remove the mapping after processing
            self.folder_removal_map.remove(&hash);
        } else {
            debug!("❌ No folder path found for hash: {}", hash);
            debug!("❌ Available mappings: {:?}", self.folder_removal_map);
            // Fallback to the old method
            if let Some(folder_relative_path) = &self.selected_folder_for_removal {
                let query_dir = directory::get_query_dir();
                let folder_path = query_dir.join(folder_relative_path);

                if folder_path.exists() && folder_path.is_dir() {
                    let is_empty = Self::is_directory_empty(&folder_path);

                    if is_empty {
                        match std::fs::remove_dir(&folder_path) {
                            Ok(()) => {
                                sidebar_query::load_queries_from_directory(self);
                                self.needs_refresh = true;
                            }
                            Err(e) => {
                                debug!("❌ Failed to remove folder: {}", e);
                                self.error_message = format!(
                                    "Failed to remove folder '{}': {}",
                                    folder_relative_path, e
                                );
                                self.show_error_message = true;
                            }
                        }
                    } else {
                        self.error_message = format!(
                            "Folder '{}' is not empty.\n\nWould you like to remove it and all its contents?",
                            folder_relative_path
                        );
                        self.show_error_message = true;
                        debug!(
                            "❌ Cannot remove non-empty folder: {}",
                            folder_relative_path
                        );
                    }
                } else {
                    self.error_message =
                        format!("Folder '{}' does not exist", folder_relative_path);
                    self.show_error_message = true;
                    debug!("❌ Folder does not exist: {}", folder_relative_path);
                }

                self.selected_folder_for_removal = None;
            } else {
                debug!("❌ No folder selected for removal in fallback either");
            }
        }
    }

    fn is_directory_empty(dir_path: &std::path::Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            entries.count() == 0
        } else {
            false
        }
    }

    fn find_connection_node_recursive(
        nodes: &mut [models::structs::TreeNode],
        connection_id: i64,
    ) -> Option<&mut models::structs::TreeNode> {
        for node in nodes.iter_mut() {
            // Check if this is the connection node we're looking for
            if node.node_type == models::enums::NodeType::Connection
                && node.connection_id == Some(connection_id)
            {
                return Some(node);
            }

            // Recursively search in children
            if !node.children.is_empty()
                && let Some(found) =
                    Self::find_connection_node_recursive(&mut node.children, connection_id)
            {
                return Some(found);
            }
        }
        None
    }

    fn refresh_connection(&mut self, connection_id: i64) {
        // Clear all cached data for this connection
        self.clear_connection_cache(connection_id);

        // Remove from connection pool cache to force reconnection
        self.connection_pools.remove(&connection_id);

        // Mark as refreshing
        self.refreshing_connections.insert(connection_id);

        // Find the connection node in the tree (recursively) and reset its loaded state
        if let Some(conn_node) =
            Self::find_connection_node_recursive(&mut self.items_tree, connection_id)
        {
            conn_node.is_loaded = false;
            // Keep current expansion state so it doesn't visually disappear; we'll repopulate on next expand
            let was_expanded = conn_node.is_expanded;
            conn_node.children.clear();
            conn_node.is_expanded = was_expanded; // preserve state
            debug!(
                "🔄 Reset (cached cleared) connection node: {} (expanded: {})",
                conn_node.name, was_expanded
            );
        } else {
            debug!(
                "⚠️ Could not locate connection node {} in primary tree; trying filtered tree / rebuild",
                connection_id
            );
            // Try filtered tree (search results)
            if let Some(conn_node) =
                Self::find_connection_node_recursive(&mut self.filtered_items_tree, connection_id)
            {
                let was_expanded = conn_node.is_expanded;
                conn_node.children.clear();
                conn_node.is_loaded = false;
                conn_node.is_expanded = was_expanded;
                debug!(
                    "🔄 Reset connection node in filtered tree: {} (expanded: {})",
                    conn_node.name, was_expanded
                );
            } else {
                // As a last resort rebuild the whole tree then search again
                crate::sidebar_database::refresh_connections_tree(self);
                if let Some(conn_node2) =
                    Self::find_connection_node_recursive(&mut self.items_tree, connection_id)
                {
                    let was_expanded = conn_node2.is_expanded;
                    conn_node2.children.clear();
                    conn_node2.is_loaded = false;
                    conn_node2.is_expanded = was_expanded;
                    debug!(
                        "🔄 Reset connection node after rebuild: {} (expanded: {})",
                        conn_node2.name, was_expanded
                    );
                } else {
                    debug!(
                        "❌ Still could not locate connection node {} after rebuild. Existing connection IDs: {:?}",
                        connection_id,
                        self.connections
                            .iter()
                            .filter_map(|c| c.id)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }

        // Send background task instead of blocking refresh
        if let Some(sender) = &self.background_sender {
            if let Err(e) =
                sender.send(models::enums::BackgroundTask::RefreshConnection { connection_id })
            {
                debug!("Failed to send background refresh task: {}", e);
                // Fallback to synchronous refresh if background thread is not available
                self.refreshing_connections.remove(&connection_id);
                cache_data::fetch_and_cache_connection_data(self, connection_id);
            } else {
                debug!(
                    "Background refresh task sent for connection {}",
                    connection_id
                );
            }
        } else {
            // Fallback to synchronous refresh if background system is not initialized
            self.refreshing_connections.remove(&connection_id);
            cache_data::fetch_and_cache_connection_data(self, connection_id);
        }
    }

    // Helper to restore expansion state of a tree recursively
    fn restore_expansion_state(
        node: &mut models::structs::TreeNode,
        state_map: &std::collections::HashMap<String, bool>,
    ) {
        use log::info;

        // Create unique key for this node
        let node_type_str = format!("{:?}", node.node_type);
        let key = format!(
            "{}:{}:{}:{}",
            node.connection_id.unwrap_or(0),
            node.database_name.as_ref().unwrap_or(&String::new()),
            node_type_str,
            node.name
        );

        // Restore expansion state from saved map
        if let Some(&expanded) = state_map.get(&key) {
            node.is_expanded = expanded;
            if expanded {
                info!(
                    "   📂 Restoring expanded: {:?} - {}",
                    node.node_type, node.name
                );
            }
        }

        // Force expand important container folders if they were expanded before
        // This ensures Database and TablesFolder are visible after refresh
        match node.node_type {
            models::enums::NodeType::Connection => {
                // If connection was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            models::enums::NodeType::DatabasesFolder => {
                // If DatabasesFolder was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            models::enums::NodeType::Database => {
                // If Database was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            models::enums::NodeType::TablesFolder => {
                // If TablesFolder was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            _ => {}
        }

        // Recursively restore children
        for child in &mut node.children {
            Self::restore_expansion_state(child, state_map);
        }
    }

    // Helper to mark expanded nodes as loaded (they'll auto-load from cache on render)
    fn mark_expanded_nodes_loaded(node: &mut models::structs::TreeNode) {
        use log::debug;

        // If this node is expanded, mark it as not loaded so it will reload from cache
        if node.is_expanded {
            match node.node_type {
                models::enums::NodeType::Database
                | models::enums::NodeType::TablesFolder
                | models::enums::NodeType::ViewsFolder
                | models::enums::NodeType::StoredProceduresFolder
                | models::enums::NodeType::UserFunctionsFolder
                | models::enums::NodeType::TriggersFolder
                | models::enums::NodeType::EventsFolder => {
                    // Mark as not loaded so it will trigger loading from cache on next render
                    node.is_loaded = false;
                    debug!(
                        "   📂 Marked expanded {:?} as needing reload: {}",
                        node.node_type, node.name
                    );
                }
                _ => {}
            }
        }

        // Recursively process children
        for child in &mut node.children {
            Self::mark_expanded_nodes_loaded(child);
        }
    }

    // Helper to recursively load all expanded nodes from cache
    fn load_expanded_nodes_recursive(
        &mut self,
        connection_id: i64,
        node: &mut models::structs::TreeNode,
    ) {
        use log::info;

        info!(
            "🔍 Checking node: {:?} '{}' - expanded={}, loaded={}",
            node.node_type, node.name, node.is_expanded, node.is_loaded
        );

        // If this node is expanded and not loaded, load it
        if node.is_expanded && !node.is_loaded {
            match node.node_type {
                models::enums::NodeType::Connection => {
                    info!("   📂 Loading Connection node from cache");
                    self.load_connection_tables(connection_id, node);
                }
                models::enums::NodeType::DatabasesFolder => {
                    info!("   📂 Loading DatabasesFolder from cache");
                    self.load_databases_for_folder(connection_id, node);
                }
                models::enums::NodeType::Database => {
                    info!("   📂 Loading Database node from cache: {}", node.name);
                    // Database node contains folders (Tables, Views, etc), they'll be loaded by their children
                    node.is_loaded = true;
                }
                models::enums::NodeType::TablesFolder => {
                    info!("   📂 Loading TablesFolder from cache");
                    self.load_folder_content(
                        connection_id,
                        node,
                        models::enums::NodeType::TablesFolder,
                    );
                }
                models::enums::NodeType::ViewsFolder => {
                    info!("   📂 Loading ViewsFolder from cache");
                    self.load_folder_content(
                        connection_id,
                        node,
                        models::enums::NodeType::ViewsFolder,
                    );
                }
                models::enums::NodeType::StoredProceduresFolder => {
                    info!("   📂 Loading StoredProceduresFolder from cache");
                    self.load_folder_content(
                        connection_id,
                        node,
                        models::enums::NodeType::StoredProceduresFolder,
                    );
                }
                _ => {
                    info!("   ⏭️  Skipping {:?} node (no loader)", node.node_type);
                }
            }
        } else if node.is_expanded {
            info!("   ⏭️  Node already loaded, skipping");
        }

        // Recursively process children (depth-first)
        // Clone children vec to avoid borrow issues
        let children_count = node.children.len();
        info!("   👶 Processing {} children...", children_count);
        for i in 0..children_count {
            // Process each child
            if let Some(child) = node.children.get_mut(i) {
                Self::load_expanded_nodes_recursive(self, connection_id, child);
            }
        }
    }

    // NEW: Disconnect connection - close pool and clear cache
    fn disconnect_connection(&mut self, connection_id: i64) {
        debug!("🔌 Disconnecting connection: {}", connection_id);

        // 1. Remove from local connection pool cache
        if self.connection_pools.remove(&connection_id).is_some() {
            debug!("✅ Removed connection pool from local cache");
        }

        // 2. Remove from shared connection pools
        if let Ok(mut shared_pools) = self.shared_connection_pools.lock()
            && shared_pools.remove(&connection_id).is_some()
        {
            debug!("✅ Removed connection pool from shared cache");
        }

        // 3. Remove from pending pools (if connection was being created)
        if self.pending_connection_pools.remove(&connection_id) {
            debug!("✅ Removed from pending connection pools");
        }

        // 4. Stop any prefetch in progress
        if self.prefetch_in_progress.remove(&connection_id) {
            debug!("✅ Stopped prefetch for connection");
        }
        self.prefetch_progress.remove(&connection_id);

        // 5. Remove from refreshing set
        if self.refreshing_connections.remove(&connection_id) {
            debug!("✅ Removed from refreshing connections");
        }

        // 6. Clear database cache for this connection
        self.database_cache.remove(&connection_id);
        self.database_cache_time.remove(&connection_id);

        // 7. Clear connection cache (database/table/column metadata)
        self.clear_connection_cache(connection_id);

        // 8. Reset connection node state in tree
        if let Some(conn_node) =
            Self::find_connection_node_recursive(&mut self.items_tree, connection_id)
        {
            conn_node.is_loaded = false;
            conn_node.is_expanded = false; // Collapse node
            conn_node.children.clear();
            debug!(
                "✅ Reset connection node: {} (collapsed and cleared)",
                conn_node.name
            );
        }

        // Also check filtered tree
        if let Some(conn_node) =
            Self::find_connection_node_recursive(&mut self.filtered_items_tree, connection_id)
        {
            conn_node.is_loaded = false;
            conn_node.is_expanded = false;
            conn_node.children.clear();
        }

        debug!("✅ Connection {} disconnected successfully", connection_id);
    }

    // Function to clear cache for a connection (useful for refresh)
    fn clear_connection_cache(&self, connection_id: i64) {
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();

            rt.block_on(async {
                // Clear all cache tables for this connection
                let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;

                let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;

                let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;

                // Also clear row and index caches to avoid stale data after refresh
                let _ = sqlx::query("DELETE FROM row_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;

                let _ = sqlx::query("DELETE FROM index_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;
            });
        }
    }

    // Clear cache for a specific table only
    pub(crate) fn clear_table_cache(
        &self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) {
        use log::info;

        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let db = database_name.to_string();
            let tbl = table_name.to_string();
            let rt = tokio::runtime::Runtime::new().unwrap();

            rt.block_on(async {
                info!("🧹 Clearing cache for table {}.{}", db, tbl);

                // Clear table cache entry
                let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                // Clear column cache for this table
                let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                // Clear row cache for this table
                let _ = sqlx::query("DELETE FROM row_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                // Clear index cache for this table
                let _ = sqlx::query("DELETE FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                info!("✅ Cache cleared for table {}.{}", db, tbl);
            });
        }
    }

    // Remove a specific table from the sidebar tree without reloading entire connection
    pub(crate) fn remove_table_from_tree(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) {
        use log::{debug, info};

        info!(
            "🌲 Removing table {}.{} from sidebar tree",
            database_name, table_name
        );
        info!("   Connection ID: {}", connection_id);
        info!("   Database name: '{}'", database_name);
        info!("   Table name: '{}'", table_name);

        // Debug: print tree structure
        debug!("   Current tree structure:");
        for (i, conn_node) in self.items_tree.iter().enumerate() {
            debug!(
                "     [{}] Connection: {} (id={:?}, type={:?})",
                i, conn_node.name, conn_node.connection_id, conn_node.node_type
            );
            for (j, child) in conn_node.children.iter().enumerate() {
                debug!(
                    "       [{}] Child: {} (type={:?}, db={:?})",
                    j, child.name, child.node_type, child.database_name
                );
            }
        }

        // Helper to match table names - handles [schema].[table], schema.table, or just table
        let matches_table = |node_name: &str, search_name: &str| -> bool {
            // Direct match
            if node_name == search_name {
                return true;
            }

            // Remove brackets and compare
            let clean_node = node_name.replace("[", "").replace("]", "");
            let clean_search = search_name.replace("[", "").replace("]", "");

            if clean_node == clean_search {
                return true;
            }

            // Compare just the table part (after last dot)
            let node_table = clean_node.split('.').next_back().unwrap_or(&clean_node);
            let search_table = clean_search.split('.').next_back().unwrap_or(&clean_search);

            node_table == search_table
        };

        // Find the connection node (may be inside a CustomFolder)
        for folder_or_conn in &mut self.items_tree {
            // First check if this is a CustomFolder, if so search its children for the connection
            if folder_or_conn.node_type == models::enums::NodeType::CustomFolder {
                info!("   Searching in folder: {}", folder_or_conn.name);
                for conn_node in &mut folder_or_conn.children {
                    if conn_node.connection_id == Some(connection_id) {
                        info!(
                            "   ✓ Found connection node: {} (ID: {})",
                            conn_node.name, connection_id
                        );

                        // Navigate through the tree structure to find the table
                        // Structure: Connection -> Databases Folder -> Database -> Tables Folder -> Table
                        if Self::remove_table_from_connection_node(
                            conn_node,
                            database_name,
                            table_name,
                            &matches_table,
                        ) {
                            return;
                        }
                    }
                }
            }
            // Also check if this node itself is a connection (for backward compatibility with non-folder structure)
            else if folder_or_conn.connection_id == Some(connection_id) {
                info!(
                    "   ✓ Found connection node (direct): {}",
                    folder_or_conn.name
                );

                // Navigate through the tree structure to find the table
                if Self::remove_table_from_connection_node(
                    folder_or_conn,
                    database_name,
                    table_name,
                    &matches_table,
                ) {
                    return;
                }
            }
        }

        info!("   ⚠️ Connection {} not found in tree", connection_id);
        info!(
            "   ⚠️ Table '{}' not found in tree (may have been already removed)",
            table_name
        );
    }

    // Helper function to remove table from a connection node (static to avoid borrow checker issues)
    fn remove_table_from_connection_node(
        conn_node: &mut models::structs::TreeNode,
        database_name: &str,
        table_name: &str,
        matches_table: &dyn Fn(&str, &str) -> bool,
    ) -> bool {
        use log::info;

        // Navigate through the tree structure to find the table
        // Structure: Connection -> Databases Folder -> Database -> Tables Folder -> Table
        for child in &mut conn_node.children {
            // Look for Databases folder
            if child.node_type == models::enums::NodeType::DatabasesFolder {
                info!("   Found DatabasesFolder");
                for db_node in &mut child.children {
                    // Find matching database
                    if let Some(ref db_name) = db_node.database_name {
                        info!("   Checking database: {}", db_name);
                        if db_name == database_name {
                            info!("   ✓ Database matches!");
                            // Find Tables folder in this database
                            for folder in &mut db_node.children {
                                if folder.node_type == models::enums::NodeType::TablesFolder {
                                    info!(
                                        "   Found TablesFolder with {} tables",
                                        folder.children.len()
                                    );

                                    // Log all tables before removal
                                    for table_node in &folder.children {
                                        let tbl_name = table_node
                                            .table_name
                                            .as_ref()
                                            .unwrap_or(&table_node.name);
                                        info!(
                                            "      - Table in tree: '{}' (node.name='{}', node.table_name={:?})",
                                            tbl_name, table_node.name, table_node.table_name
                                        );
                                    }

                                    // Remove the table from Tables folder
                                    let before_count = folder.children.len();
                                    folder.children.retain(|table_node| {
                                        let node_name = table_node.table_name.as_ref().unwrap_or(&table_node.name);
                                        let keep = !matches_table(node_name, table_name);
                                        if !keep {
                                            info!("   ✅ Removed table '{}' from tree (matched with '{}')", node_name, table_name);
                                        }
                                        keep
                                    });
                                    let after_count = folder.children.len();
                                    info!("   Tables count: {} -> {}", before_count, after_count);
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
            // Also check direct children for databases (some DB types don't use DatabasesFolder)
            else if child.node_type == models::enums::NodeType::Database
                && let Some(ref db_name) = child.database_name
            {
                info!("   Checking direct database node: {}", db_name);
                if db_name == database_name {
                    info!("   ✓ Database matches!");
                    // Find Tables folder in this database
                    for folder in &mut child.children {
                        if folder.node_type == models::enums::NodeType::TablesFolder {
                            info!(
                                "   Found TablesFolder with {} tables",
                                folder.children.len()
                            );

                            // Log all tables before removal
                            for table_node in &folder.children {
                                let tbl_name =
                                    table_node.table_name.as_ref().unwrap_or(&table_node.name);
                                info!(
                                    "      - Table in tree: '{}' (node.name='{}', node.table_name={:?})",
                                    tbl_name, table_node.name, table_node.table_name
                                );
                            }

                            // Remove the table from Tables folder
                            let before_count = folder.children.len();
                            folder.children.retain(|table_node| {
                                let node_name =
                                    table_node.table_name.as_ref().unwrap_or(&table_node.name);
                                let keep = !matches_table(node_name, table_name);
                                if !keep {
                                    info!(
                                        "   ✅ Removed table '{}' from tree (matched with '{}')",
                                        node_name, table_name
                                    );
                                }
                                keep
                            });
                            let after_count = folder.children.len();
                            info!("   Tables count: {} -> {}", before_count, after_count);
                            return true;
                        }
                    }
                }
            }
        }

        false // Table not found
    }

    fn load_connection_tables(&mut self, connection_id: i64, node: &mut models::structs::TreeNode) {
        debug!("Loading connection tables for ID: {}", connection_id);

        // Ensure a connection pool is opened/initialized before proceeding.
        if !self.connection_pools.contains_key(&connection_id) {
            let rt = self.get_runtime();
            let start_time = std::time::Instant::now();
            let pool_res = rt.block_on(async {
                crate::connection::get_or_create_connection_pool(self, connection_id).await
            });
            match pool_res {
                Some(_) => debug!(
                    "✅ Connection pool ready for {} (took {:?})",
                    connection_id,
                    start_time.elapsed()
                ),
                None => debug!(
                    "❌ Failed to initialize connection pool for {}",
                    connection_id
                ),
            }
        } else {
            debug!("🔁 Reusing existing connection pool for {}", connection_id);
        }

        // First check if we have cached data
        if let Some(databases) = cache_data::get_databases_from_cache(self, connection_id) {
            debug!(
                "Found cached databases for connection {}: {:?}",
                connection_id, databases
            );
            if !databases.is_empty() {
                self.build_connection_structure_from_cache(connection_id, node, &databases);
                node.is_loaded = true;
                return;
            }
        }

        debug!(
            "🔄 Cache empty or not found, fetching databases from server for connection {}",
            connection_id
        );

        // Try to fetch from actual database server
        // Use async variant with shared runtime to avoid creating a new runtime per call
        let fresh_databases_opt = {
            let rt = self.get_runtime();
            rt.block_on(async {
                crate::connection::fetch_databases_from_connection_async(self, connection_id).await
            })
        };
        if let Some(fresh_databases) = fresh_databases_opt {
            debug!(
                "✅ Successfully fetched {} databases from server",
                fresh_databases.len()
            );
            // Save to cache for future use
            cache_data::save_databases_to_cache(self, connection_id, &fresh_databases);
            // Build structure from fresh data
            self.build_connection_structure_from_cache(connection_id, node, &fresh_databases);
            node.is_loaded = true;
            return;
        } else {
            debug!("❌ Failed to fetch databases from server, creating default structure");
        }

        // Find the connection by ID
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();

            // Create the main structure based on database type
            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    driver_mysql::load_mysql_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::PostgreSQL => {
                    driver_postgres::load_postgresql_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::SQLite => {
                    driver_sqlite::load_sqlite_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::Redis => {
                    driver_redis::load_redis_structure(self, connection_id, &connection, node);
                }
                models::enums::DatabaseType::MsSQL => {
                    crate::driver_mssql::load_mssql_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::MongoDB => {
                    crate::driver_mongodb::load_mongodb_structure(connection_id, &connection, node);
                }
            }
            node.is_loaded = true;
        }
    }

    fn build_connection_structure_from_cache(
        &mut self,
        connection_id: i64,
        node: &mut models::structs::TreeNode,
        databases: &[String],
    ) {
        // Find the connection to get its type
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let mut main_children = Vec::new();

            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    // 1. Databases folder
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);
                    databases_folder.is_loaded = false;

                    // Add each database from cache
                    for db_name in databases {
                        // Skip system databases for cleaner view
                        if !["information_schema", "performance_schema", "mysql", "sys"]
                            .contains(&db_name.as_str())
                        {
                            let mut db_node = models::structs::TreeNode::new(
                                db_name.clone(),
                                models::enums::NodeType::Database,
                            );
                            db_node.connection_id = Some(connection_id);
                            db_node.database_name = Some(db_name.clone());
                            db_node.is_loaded = false; // Will be loaded when expanded

                            // Create folder structure but don't load content yet
                            let mut tables_folder = models::structs::TreeNode::new(
                                "Tables".to_string(),
                                models::enums::NodeType::TablesFolder,
                            );
                            tables_folder.connection_id = Some(connection_id);
                            tables_folder.database_name = Some(db_name.clone());
                            tables_folder.is_loaded = false;

                            let mut views_folder = models::structs::TreeNode::new(
                                "Views".to_string(),
                                models::enums::NodeType::ViewsFolder,
                            );
                            views_folder.connection_id = Some(connection_id);
                            views_folder.database_name = Some(db_name.clone());
                            views_folder.is_loaded = false;

                            let mut procedures_folder = models::structs::TreeNode::new(
                                "Stored Procedures".to_string(),
                                models::enums::NodeType::StoredProceduresFolder,
                            );
                            procedures_folder.connection_id = Some(connection_id);
                            procedures_folder.database_name = Some(db_name.clone());
                            procedures_folder.is_loaded = false;

                            let mut functions_folder = models::structs::TreeNode::new(
                                "Functions".to_string(),
                                models::enums::NodeType::UserFunctionsFolder,
                            );
                            functions_folder.connection_id = Some(connection_id);
                            functions_folder.database_name = Some(db_name.clone());
                            functions_folder.is_loaded = false;

                            let mut triggers_folder = models::structs::TreeNode::new(
                                "Triggers".to_string(),
                                models::enums::NodeType::TriggersFolder,
                            );
                            triggers_folder.connection_id = Some(connection_id);
                            triggers_folder.database_name = Some(db_name.clone());
                            triggers_folder.is_loaded = false;

                            let mut events_folder = models::structs::TreeNode::new(
                                "Events".to_string(),
                                models::enums::NodeType::EventsFolder,
                            );
                            events_folder.connection_id = Some(connection_id);
                            events_folder.database_name = Some(db_name.clone());
                            events_folder.is_loaded = false;

                            db_node.children = vec![
                                tables_folder,
                                views_folder,
                                procedures_folder,
                                functions_folder,
                                triggers_folder,
                                events_folder,
                            ];

                            databases_folder.children.push(db_node);
                        }
                    }

                    // 2. DBA Views folder
                    // 2. DBA Views folder
                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::MySQL) {
                        let mut node = models::structs::TreeNode::new(name.to_string(), node_type);
                        node.connection_id = Some(connection_id);
                        node.is_loaded = false;
                        node.query = Some(query.to_string());
                        dba_children.push(node);
                    }

                    // Render Custom Views
                    log::info!("Cache Builder: Rendering custom views for connection {}: found {}", connection_id, connection.custom_views.len());
                    for view in connection.custom_views.iter() {
                        let mut view_node = models::structs::TreeNode::new(
                            view.name.clone(),
                            models::enums::NodeType::CustomView,
                        );
                        view_node.connection_id = Some(connection_id);
                        view_node.query = Some(view.query.clone()); 
                        view_node.is_loaded = true;
                        dba_children.push(view_node);
                    }

                    dba_folder.children = dba_children;

                    main_children.push(databases_folder);
                    main_children.push(dba_folder);
                }
                models::enums::DatabaseType::PostgreSQL => {
                    // Similar structure for PostgreSQL
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);

                    for db_name in databases {
                        if !["template0", "template1", "postgres"].contains(&db_name.as_str()) {
                            let mut db_node = models::structs::TreeNode::new(
                                db_name.clone(),
                                models::enums::NodeType::Database,
                            );
                            db_node.connection_id = Some(connection_id);
                            db_node.database_name = Some(db_name.clone());
                            db_node.is_loaded = false;

                            let mut tables_folder = models::structs::TreeNode::new(
                                "Tables".to_string(),
                                models::enums::NodeType::TablesFolder,
                            );
                            tables_folder.connection_id = Some(connection_id);
                            tables_folder.database_name = Some(db_name.clone());
                            tables_folder.is_loaded = false;

                            let mut views_folder = models::structs::TreeNode::new(
                                "Views".to_string(),
                                models::enums::NodeType::ViewsFolder,
                            );
                            views_folder.connection_id = Some(connection_id);
                            views_folder.database_name = Some(db_name.clone());
                            views_folder.is_loaded = false;

                            db_node.children = vec![tables_folder, views_folder];
                            databases_folder.children.push(db_node);
                        }
                    }

                    main_children.push(databases_folder);

                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::PostgreSQL) {
                         let mut node = models::structs::TreeNode::new(name.to_string(), node_type);
                         node.connection_id = Some(connection_id);
                         node.is_loaded = false;
                         node.query = Some(query.to_string());
                         dba_children.push(node);
                    }

                    // Render Custom Views
                    log::info!("Cache Builder: Rendering custom views for connection {}: found {}", connection_id, connection.custom_views.len());
                    for view in connection.custom_views.iter() {
                        let mut view_node = models::structs::TreeNode::new(
                            view.name.clone(),
                            models::enums::NodeType::CustomView,
                        );
                        view_node.connection_id = Some(connection_id);
                        view_node.query = Some(view.query.clone()); 
                        view_node.is_loaded = true;
                        dba_children.push(view_node);
                    }

                    dba_folder.children = dba_children;
                    main_children.push(dba_folder);
                }
                models::enums::DatabaseType::MongoDB => {
                    // MongoDB: Databases -> Collections
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);

                    for db_name in databases {
                        let mut db_node = models::structs::TreeNode::new(
                            db_name.clone(),
                            models::enums::NodeType::Database,
                        );
                        db_node.connection_id = Some(connection_id);
                        db_node.database_name = Some(db_name.clone());
                        db_node.is_loaded = false;

                        // Collections folder (reuse TablesFolder type for UI rendering)
                        let mut collections_folder = models::structs::TreeNode::new(
                            "Collections".to_string(),
                            models::enums::NodeType::TablesFolder,
                        );
                        collections_folder.connection_id = Some(connection_id);
                        collections_folder.database_name = Some(db_name.clone());
                        collections_folder.is_loaded = false;
                        db_node.children = vec![collections_folder];
                        databases_folder.children.push(db_node);
                    }

                    main_children.push(databases_folder);
                }
                models::enums::DatabaseType::SQLite => {
                    // SQLite structure - single database
                    let mut tables_folder = models::structs::TreeNode::new(
                        "Tables".to_string(),
                        models::enums::NodeType::TablesFolder,
                    );
                    tables_folder.connection_id = Some(connection_id);
                    tables_folder.database_name = Some("main".to_string());
                    tables_folder.is_loaded = false;

                    let mut views_folder = models::structs::TreeNode::new(
                        "Views".to_string(),
                        models::enums::NodeType::ViewsFolder,
                    );
                    views_folder.connection_id = Some(connection_id);
                    views_folder.database_name = Some("main".to_string());
                    views_folder.is_loaded = false;

                    main_children = vec![tables_folder, views_folder];
                }
                models::enums::DatabaseType::Redis => {
                    // Redis structure with databases
                    cache_data::build_redis_structure_from_cache(
                        self,
                        connection_id,
                        node,
                        databases,
                    );
                    return;
                }
                models::enums::DatabaseType::MsSQL => {
                    // Databases folder
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);
                    for db_name in databases {
                        let mut db_node = models::structs::TreeNode::new(
                            db_name.clone(),
                            models::enums::NodeType::Database,
                        );
                        db_node.connection_id = Some(connection_id);
                        db_node.database_name = Some(db_name.clone());
                        db_node.is_loaded = false;
                        let mut tables_folder = models::structs::TreeNode::new(
                            "Tables".to_string(),
                            models::enums::NodeType::TablesFolder,
                        );
                        tables_folder.connection_id = Some(connection_id);
                        tables_folder.database_name = Some(db_name.clone());
                        tables_folder.is_loaded = false;
                        let mut views_folder = models::structs::TreeNode::new(
                            "Views".to_string(),
                            models::enums::NodeType::ViewsFolder,
                        );
                        views_folder.connection_id = Some(connection_id);
                        views_folder.database_name = Some(db_name.clone());
                        views_folder.is_loaded = false;
                        // Stored Procedures folder
                        let mut sp_folder = models::structs::TreeNode::new(
                            "Stored Procedures".to_string(),
                            models::enums::NodeType::StoredProceduresFolder,
                        );
                        sp_folder.connection_id = Some(connection_id);
                        sp_folder.database_name = Some(db_name.clone());
                        sp_folder.is_loaded = false;
                        // Functions folder
                        let mut fn_folder = models::structs::TreeNode::new(
                            "Functions".to_string(),
                            models::enums::NodeType::UserFunctionsFolder,
                        );
                        fn_folder.connection_id = Some(connection_id);
                        fn_folder.database_name = Some(db_name.clone());
                        fn_folder.is_loaded = false;
                        // Triggers folder (events not supported in MsSQL)
                        let mut trg_folder = models::structs::TreeNode::new(
                            "Triggers".to_string(),
                            models::enums::NodeType::TriggersFolder,
                        );
                        trg_folder.connection_id = Some(connection_id);
                        trg_folder.database_name = Some(db_name.clone());
                        trg_folder.is_loaded = false;

                        db_node.children = vec![
                            tables_folder,
                            views_folder,
                            sp_folder,
                            fn_folder,
                            trg_folder,
                        ];
                        databases_folder.children.push(db_node);
                    }

                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::MsSQL) {
                        let mut node = models::structs::TreeNode::new(name.to_string(), node_type);
                        node.connection_id = Some(connection_id);
                        node.is_loaded = false;
                        node.query = Some(query.to_string());
                        dba_children.push(node);
                   }

                    // Render Custom Views
                    log::info!("Cache Builder: Rendering custom views for connection {}: found {}", connection_id, connection.custom_views.len());
                    for view in connection.custom_views.iter() {
                        let mut view_node = models::structs::TreeNode::new(
                            view.name.clone(),
                            models::enums::NodeType::CustomView,
                        );
                        view_node.connection_id = Some(connection_id);
                        view_node.query = Some(view.query.clone()); 
                        view_node.is_loaded = true;
                        dba_children.push(view_node);
                    }

                    dba_folder.children = dba_children;

                    main_children.push(databases_folder);
                    main_children.push(dba_folder);
                }
            }

            node.children = main_children;
        }
    }

    // More specific function to find folder node with exact type and database name
    fn find_specific_folder_node<'a>(
        node: &'a mut models::structs::TreeNode,
        connection_id: i64,
        folder_type: &models::enums::NodeType,
        database_name: &Option<String>,
    ) -> Option<&'a mut models::structs::TreeNode> {
        // Check if this node is the folder we're looking for
        if node.node_type == *folder_type
            && node.connection_id == Some(connection_id)
            && node.database_name == *database_name
            && node.is_expanded
            && !node.is_loaded
        {
            return Some(node);
        }

        // Recursively search in children
        for child in &mut node.children {
            if let Some(result) =
                Self::find_specific_folder_node(child, connection_id, folder_type, database_name)
            {
                return Some(result);
            }
        }

        None
    }

    fn load_databases_for_folder(
        &mut self,
        connection_id: i64,
        databases_folder: &mut models::structs::TreeNode,
    ) {
        // Check connection type to handle Redis differently
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            && connection.connection_type == models::enums::DatabaseType::Redis
        {
            self.load_redis_databases_for_folder(connection_id, databases_folder);
            return;
        }

        // Clear any loading placeholders
        databases_folder.children.clear();

        // First check cache
        if let Some(cached_databases) = cache_data::get_databases_from_cache(self, connection_id)
            && !cached_databases.is_empty()
        {
            for db_name in cached_databases {
                let mut db_node = models::structs::TreeNode::new(
                    db_name.clone(),
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Add subfolders for each database
                let mut db_children = Vec::new();



                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new(
                    "Tables".to_string(),
                    models::enums::NodeType::TablesFolder,
                );
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);

                // Views folder
                let mut views_folder = models::structs::TreeNode::new(
                    "Views".to_string(),
                    models::enums::NodeType::ViewsFolder,
                );
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);

                // Stored Procedures folder
                let mut sp_folder = models::structs::TreeNode::new(
                    "Stored Procedures".to_string(),
                    models::enums::NodeType::StoredProceduresFolder,
                );
                sp_folder.connection_id = Some(connection_id);
                sp_folder.database_name = Some(db_name.clone());
                sp_folder.is_loaded = false;
                db_children.push(sp_folder);

                db_node.children = db_children;
                databases_folder.children.push(db_node);
            }

            databases_folder.is_loaded = true;
            return;
        }

        // Try to fetch real databases from the connection
        if let Some(real_databases) =
            connection::fetch_databases_from_connection(self, connection_id)
        {
            // Save to cache for future use
            cache_data::save_databases_to_cache(self, connection_id, &real_databases);

            // Create tree nodes from fetched data
            for db_name in real_databases {
                let mut db_node = models::structs::TreeNode::new(
                    db_name.clone(),
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Add subfolders for each database
                let mut db_children = Vec::new();



                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new(
                    "Tables".to_string(),
                    models::enums::NodeType::TablesFolder,
                );
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);

                // Views folder
                let mut views_folder = models::structs::TreeNode::new(
                    "Views".to_string(),
                    models::enums::NodeType::ViewsFolder,
                );
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);

                // Stored Procedures / Functions / Triggers depending on DB type
                if let Some(conn) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                {
                    match conn.connection_type {
                        models::enums::DatabaseType::MySQL => {
                            let mut sp_folder = models::structs::TreeNode::new(
                                "Stored Procedures".to_string(),
                                models::enums::NodeType::StoredProceduresFolder,
                            );
                            sp_folder.connection_id = Some(connection_id);
                            sp_folder.database_name = Some(db_name.clone());
                            sp_folder.is_loaded = false;
                            db_children.push(sp_folder);
                        }
                        models::enums::DatabaseType::MsSQL => {
                            let mut sp_folder = models::structs::TreeNode::new(
                                "Stored Procedures".to_string(),
                                models::enums::NodeType::StoredProceduresFolder,
                            );
                            sp_folder.connection_id = Some(connection_id);
                            sp_folder.database_name = Some(db_name.clone());
                            sp_folder.is_loaded = false;
                            db_children.push(sp_folder);
                            let mut fn_folder = models::structs::TreeNode::new(
                                "Functions".to_string(),
                                models::enums::NodeType::UserFunctionsFolder,
                            );
                            fn_folder.connection_id = Some(connection_id);
                            fn_folder.database_name = Some(db_name.clone());
                            fn_folder.is_loaded = false;
                            db_children.push(fn_folder);
                            let mut trg_folder = models::structs::TreeNode::new(
                                "Triggers".to_string(),
                                models::enums::NodeType::TriggersFolder,
                            );
                            trg_folder.connection_id = Some(connection_id);
                            trg_folder.database_name = Some(db_name.clone());
                            trg_folder.is_loaded = false;
                            db_children.push(trg_folder);
                        }
                        _ => {}
                    }
                }

                db_node.children = db_children;
                databases_folder.children.push(db_node);
            }

            databases_folder.is_loaded = true;
        } else {
            self.populate_sample_databases_for_folder(connection_id, databases_folder);
        }
    }

    fn populate_sample_databases_for_folder(
        &mut self,
        connection_id: i64,
        databases_folder: &mut models::structs::TreeNode,
    ) {
        // Find the connection to determine type
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let sample_databases = match connection.connection_type {
                models::enums::DatabaseType::MySQL => vec![
                    "information_schema".to_string(),
                    "sakila".to_string(),
                    "world".to_string(),
                    "test".to_string(),
                ],
                models::enums::DatabaseType::PostgreSQL => vec![
                    "postgres".to_string(),
                    "template1".to_string(),
                    "dvdrental".to_string(),
                ],
                models::enums::DatabaseType::SQLite => vec!["main".to_string()],
                models::enums::DatabaseType::Redis => vec!["redis".to_string(), "info".to_string()],
                models::enums::DatabaseType::MsSQL => vec![
                    "master".to_string(),
                    "tempdb".to_string(),
                    "model".to_string(),
                    "msdb".to_string(),
                ],
                models::enums::DatabaseType::MongoDB => {
                    vec!["admin".to_string(), "local".to_string()]
                }
            };

            // Clear loading message
            databases_folder.children.clear();

            // Add sample databases
            for db_name in sample_databases {
                // Skip system databases for display
                if matches!(
                    connection.connection_type,
                    models::enums::DatabaseType::MySQL
                ) && ["information_schema", "performance_schema", "mysql", "sys"]
                    .contains(&db_name.as_str())
                {
                    continue;
                }

                let mut db_node = models::structs::TreeNode::new(
                    db_name.clone(),
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Add subfolders for each database
                let mut db_children = Vec::new();

                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new(
                    "Tables".to_string(),
                    models::enums::NodeType::TablesFolder,
                );
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);

                // Views folder
                let mut views_folder = models::structs::TreeNode::new(
                    "Views".to_string(),
                    models::enums::NodeType::ViewsFolder,
                );
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);

                if matches!(
                    connection.connection_type,
                    models::enums::DatabaseType::MySQL
                ) {
                    // Stored Procedures folder
                    let mut sp_folder = models::structs::TreeNode::new(
                        "Stored Procedures".to_string(),
                        models::enums::NodeType::StoredProceduresFolder,
                    );
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);

                    // User Functions folder
                    let mut uf_folder = models::structs::TreeNode::new(
                        "User Functions".to_string(),
                        models::enums::NodeType::UserFunctionsFolder,
                    );
                    uf_folder.connection_id = Some(connection_id);
                    uf_folder.database_name = Some(db_name.clone());
                    uf_folder.is_loaded = false;
                    db_children.push(uf_folder);

                    // Triggers folder
                    let mut triggers_folder = models::structs::TreeNode::new(
                        "Triggers".to_string(),
                        models::enums::NodeType::TriggersFolder,
                    );
                    triggers_folder.connection_id = Some(connection_id);
                    triggers_folder.database_name = Some(db_name.clone());
                    triggers_folder.is_loaded = false;
                    db_children.push(triggers_folder);

                    // Events folder
                    let mut events_folder = models::structs::TreeNode::new(
                        "Events".to_string(),
                        models::enums::NodeType::EventsFolder,
                    );
                    events_folder.connection_id = Some(connection_id);
                    events_folder.database_name = Some(db_name.clone());
                    events_folder.is_loaded = false;
                    db_children.push(events_folder);
                } else if matches!(
                    connection.connection_type,
                    models::enums::DatabaseType::MsSQL
                ) {
                    // For MsSQL, add Procedures, Functions, and Triggers (no Events)
                    let mut sp_folder = models::structs::TreeNode::new(
                        "Stored Procedures".to_string(),
                        models::enums::NodeType::StoredProceduresFolder,
                    );
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);

                    let mut fn_folder = models::structs::TreeNode::new(
                        "Functions".to_string(),
                        models::enums::NodeType::UserFunctionsFolder,
                    );
                    fn_folder.connection_id = Some(connection_id);
                    fn_folder.database_name = Some(db_name.clone());
                    fn_folder.is_loaded = false;
                    db_children.push(fn_folder);

                    let mut trg_folder = models::structs::TreeNode::new(
                        "Triggers".to_string(),
                        models::enums::NodeType::TriggersFolder,
                    );
                    trg_folder.connection_id = Some(connection_id);
                    trg_folder.database_name = Some(db_name.clone());
                    trg_folder.is_loaded = false;
                    db_children.push(trg_folder);
                }

                db_node.children = db_children;
                databases_folder.children.push(db_node);
            }
        }
    }

    fn load_redis_databases_for_folder(
        &mut self,
        connection_id: i64,
        databases_folder: &mut models::structs::TreeNode,
    ) {
        // Clear loading placeholders
        databases_folder.children.clear();

        // Ambil daftar database Redis dari cache
        if let Some(cached_databases) = cache_data::get_databases_from_cache(self, connection_id) {
            for db_name in cached_databases {
                if db_name.starts_with("db") {
                    let mut db_node = models::structs::TreeNode::new(
                        db_name.clone(),
                        models::enums::NodeType::Database,
                    );
                    db_node.connection_id = Some(connection_id);
                    db_node.database_name = Some(db_name.clone());
                    db_node.is_loaded = false;

                    // Tambahkan node child untuk key, akan di-load saat node db di-expand
                    let loading_keys_node = models::structs::TreeNode::new(
                        "Loading keys...".to_string(),
                        models::enums::NodeType::Table,
                    );
                    db_node.children.push(loading_keys_node);

                    databases_folder.children.push(db_node);
                }
            }
            databases_folder.is_loaded = true;
        }
    }

    fn find_redis_database_node<'a>(
        node: &'a mut models::structs::TreeNode,
        connection_id: i64,
        database_name: &Option<String>,
    ) -> Option<&'a mut models::structs::TreeNode> {
        // Check if this is the database node we're looking for
        if node.connection_id == Some(connection_id)
            && node.node_type == models::enums::NodeType::Database
            && node.database_name == *database_name
        {
            return Some(node);
        }

        // Recursively search in children
        for child in &mut node.children {
            if let Some(found) = Self::find_redis_database_node(child, connection_id, database_name)
            {
                return Some(found);
            }
        }

        None
    }

    fn load_redis_keys_for_database(
        &mut self,
        connection_id: i64,
        database_name: &str,
        db_node: &mut models::structs::TreeNode,
    ) {
        // Clear existing children and mark as loading
        db_node.children.clear();

        // Extract database number from database_name (e.g., "db0" -> 0)
        let db_number = if let Some(suffix) = database_name.strip_prefix("db") {
            suffix.parse::<u8>().unwrap_or(0)
        } else {
            0
        };

        // Get connection pool and fetch keys
        let rt = tokio::runtime::Runtime::new().unwrap();
        let keys_result = rt.block_on(async {
            if let Some(pool) = connection::get_or_create_connection_pool(self, connection_id).await
            {
                if let models::enums::DatabasePool::Redis(redis_manager) = pool {
                    let mut conn = redis_manager.as_ref().clone();

                    // Select the specific database
                    if let Err(e) = redis::cmd("SELECT")
                        .arg(db_number)
                        .query_async::<()>(&mut conn)
                        .await
                    {
                        debug!("❌ Failed to select database {}: {}", db_number, e);
                        return Vec::new();
                    }

                    // Use SCAN for safe key enumeration (better than KEYS * in production)
                    let mut cursor = 0u64;
                    let mut all_keys = Vec::new();
                    let max_keys = 100; // Limit to first 100 keys to avoid overwhelming UI

                    loop {
                        match redis::cmd("SCAN")
                            .arg(cursor)
                            .arg("COUNT")
                            .arg(10)
                            .query_async::<(u64, Vec<String>)>(&mut conn)
                            .await
                        {
                            Ok((next_cursor, keys)) => {
                                for key in keys {
                                    if all_keys.len() >= max_keys {
                                        break;
                                    }

                                    // Get the type of each key
                                    if let Ok(key_type) = redis::cmd("TYPE")
                                        .arg(&key)
                                        .query_async::<String>(&mut conn)
                                        .await
                                    {
                                        all_keys.push((key, key_type));
                                    }
                                }

                                cursor = next_cursor;
                                if cursor == 0 || all_keys.len() >= max_keys {
                                    break;
                                }
                            }
                            Err(e) => {
                                debug!("❌ SCAN command failed: {}", e);
                                break;
                            }
                        }
                    }

                    debug!(
                        "✅ Found {} keys in database {}",
                        all_keys.len(),
                        database_name
                    );
                    all_keys
                } else {
                    debug!("❌ Connection pool is not Redis type");
                    Vec::new()
                }
            } else {
                debug!("❌ Failed to get Redis connection pool");
                Vec::new()
            }
        });

        // Group keys by type
        let mut keys_by_type: std::collections::HashMap<String, Vec<(String, String)>> =
            std::collections::HashMap::new();
        for (key, key_type) in keys_result {
            keys_by_type
                .entry(key_type.clone())
                .or_default()
                .push((key, key_type));
        }

        // Create folder structure for each Redis data type
        for (data_type, keys) in keys_by_type {
            let folder_name = match data_type.as_str() {
                "string" => "Strings",
                "hash" => "Hashes",
                "list" => "Lists",
                "set" => "Sets",
                "zset" => "Sorted Sets",
                "stream" => "Streams",
                _ => &data_type,
            };

            let mut type_folder = models::structs::TreeNode::new(
                format!("{} ({})", folder_name, keys.len()),
                models::enums::NodeType::TablesFolder,
            );
            type_folder.connection_id = Some(connection_id);
            type_folder.database_name = Some(database_name.to_string());
            type_folder.is_expanded = false;
            type_folder.is_loaded = true;

            // Add keys of this type to the folder
            for (key, _key_type) in keys {
                let mut key_node =
                    models::structs::TreeNode::new(key.clone(), models::enums::NodeType::Table);
                key_node.connection_id = Some(connection_id);
                key_node.database_name = Some(database_name.to_string());
                type_folder.children.push(key_node);
            }

            db_node.children.push(type_folder);
        }

        db_node.is_loaded = true;
        debug!(
            "✅ Database node loaded with {} type folders",
            db_node.children.len()
        );
    }

    // Cached database fetcher for better performance
    fn get_databases_cached(&mut self, connection_id: i64) -> Vec<String> {
        const CACHE_DURATION: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes cache

        // Check if we have cached data and it's still valid
        if let Some(cache_time) = self.database_cache_time.get(&connection_id)
            && cache_time.elapsed() < CACHE_DURATION
            && let Some(cached_databases) = self.database_cache.get(&connection_id)
        {
            return cached_databases.clone();
        }

        // Cache is invalid or doesn't exist, fetch fresh data
        // But do this in background to avoid blocking UI
        if let Some(databases) = connection::fetch_databases_from_connection(self, connection_id) {
            // Update cache
            self.database_cache.insert(connection_id, databases.clone());
            self.database_cache_time
                .insert(connection_id, std::time::Instant::now());
            databases
        } else {
            // Return empty list if fetch failed, but don't cache the failure
            Vec::new()
        }
    }

    fn load_folder_content(
        &mut self,
        connection_id: i64,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
        // Find the connection by ID
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();

            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    self.load_mysql_folder_content(connection_id, &connection, node, folder_type);
                }
                models::enums::DatabaseType::PostgreSQL => {
                    self.load_postgresql_folder_content(
                        connection_id,
                        &connection,
                        node,
                        folder_type,
                    );
                }
                models::enums::DatabaseType::SQLite => {
                    self.load_sqlite_folder_content(connection_id, &connection, node, folder_type);
                }
                models::enums::DatabaseType::Redis => {
                    self.load_redis_folder_content(connection_id, &connection, node, folder_type);
                }
                models::enums::DatabaseType::MsSQL => {
                    self.load_mssql_folder_content(connection_id, &connection, node, folder_type);
                }
                models::enums::DatabaseType::MongoDB => {
                    // For MongoDB, TablesFolder represents collections
                    let database_name = node
                        .database_name
                        .clone()
                        .unwrap_or_else(|| connection.database.clone());
                    let table_type = "collection";

                    // Try cache first
                    if let Some(cached) = cache_data::get_tables_from_cache(
                        self,
                        connection_id,
                        &database_name,
                        table_type,
                    ) && !cached.is_empty()
                    {
                        node.children = cached
                            .into_iter()
                            .map(|name| {
                                let mut child = models::structs::TreeNode::new(
                                    name,
                                    models::enums::NodeType::Table,
                                );
                                child.connection_id = Some(connection_id);
                                child.database_name = Some(database_name.clone());
                                child.is_loaded = false;
                                child
                            })
                            .collect();
                        return;
                    }

                    // Fallback to live fetch
                    if let Some(cols) =
                        crate::driver_mongodb::fetch_collections_from_mongodb_connection(
                            self,
                            connection_id,
                            &database_name,
                        )
                    {
                        let table_data: Vec<(String, String)> = cols
                            .iter()
                            .map(|n| (n.clone(), table_type.to_string()))
                            .collect();
                        cache_data::save_tables_to_cache(
                            self,
                            connection_id,
                            &database_name,
                            &table_data,
                        );
                        node.children = cols
                            .into_iter()
                            .map(|name| {
                                let mut child = models::structs::TreeNode::new(
                                    name,
                                    models::enums::NodeType::Table,
                                );
                                child.connection_id = Some(connection_id);
                                child.database_name = Some(database_name.clone());
                                child.is_loaded = false;
                                child
                            })
                            .collect();
                    } else {
                        node.children = vec![models::structs::TreeNode::new(
                            "Failed to load collections".to_string(),
                            models::enums::NodeType::Column,
                        )];
                    }
                }
            }

            node.is_loaded = true;
        } else {
            debug!("ERROR: Connection with ID {} not found!", connection_id);
        }
    }

    fn load_mysql_folder_content(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
        // Get database name from node or connection default
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        // Map folder type to cache table type
        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            models::enums::NodeType::StoredProceduresFolder => "procedure",
            models::enums::NodeType::UserFunctionsFolder => "function",
            models::enums::NodeType::TriggersFolder => "trigger",
            models::enums::NodeType::EventsFolder => "event",
            _ => {
                debug!("Unsupported folder type: {:?}", folder_type);
                return;
            }
        };

        // First try to get from cache
        if let Some(cached_items) =
            cache_data::get_tables_from_cache(self, connection_id, database_name, table_type)
            && !cached_items.is_empty()
        {
            // Create tree nodes from cached data
            let child_nodes: Vec<models::structs::TreeNode> = cached_items
                .into_iter()
                .map(|item_name| {
                    let mut child_node = models::structs::TreeNode::new(
                        item_name.clone(),
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                            models::enums::NodeType::StoredProceduresFolder => {
                                models::enums::NodeType::StoredProcedure
                            }
                            models::enums::NodeType::UserFunctionsFolder => {
                                models::enums::NodeType::UserFunction
                            }
                            models::enums::NodeType::TriggersFolder => {
                                models::enums::NodeType::Trigger
                            }
                            models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                            _ => models::enums::NodeType::Table,
                        },
                    );
                    child_node.connection_id = Some(connection_id);
                    child_node.database_name = Some(database_name.clone());
                    child_node.is_loaded = false; // Will load columns on expansion if it's a table
                    child_node
                })
                .collect();

            node.children = child_nodes;
            return;
        }

        // If cache is empty, fetch from actual database
        if let Some(real_items) = driver_mysql::fetch_tables_from_mysql_connection(
            self,
            connection_id,
            database_name,
            table_type,
        ) {
            debug!(
                "Successfully fetched {} {} from MySQL database",
                real_items.len(),
                table_type
            );

            // Save to cache for future use
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|name| (name.clone(), table_type.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);

            // Create tree nodes from fetched data
            let child_nodes: Vec<models::structs::TreeNode> = real_items
                .into_iter()
                .map(|item_name| {
                    let mut child_node = models::structs::TreeNode::new(
                        item_name.clone(),
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                            models::enums::NodeType::StoredProceduresFolder => {
                                models::enums::NodeType::StoredProcedure
                            }
                            models::enums::NodeType::UserFunctionsFolder => {
                                models::enums::NodeType::UserFunction
                            }
                            models::enums::NodeType::TriggersFolder => {
                                models::enums::NodeType::Trigger
                            }
                            models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                            _ => models::enums::NodeType::Table,
                        },
                    );
                    child_node.connection_id = Some(connection_id);
                    child_node.database_name = Some(database_name.clone());
                    child_node.is_loaded = false; // Will load columns on expansion if it's a table
                    child_node
                })
                .collect();

            node.children = child_nodes;
        } else {
            // If database fetch fails, show an informative placeholder instead of confusing sample data
            debug!(
                "Failed to fetch from MySQL, showing placeholder for {}",
                table_type
            );
            let placeholder = match folder_type {
                models::enums::NodeType::TablesFolder => "Failed to load tables",
                models::enums::NodeType::ViewsFolder => "Failed to load views",
                models::enums::NodeType::StoredProceduresFolder => "Failed to load procedures",
                models::enums::NodeType::UserFunctionsFolder => "Failed to load functions",
                models::enums::NodeType::TriggersFolder => "Failed to load triggers",
                models::enums::NodeType::EventsFolder => "Failed to load events",
                _ => "Failed to load items",
            };
            node.children = vec![models::structs::TreeNode::new(
                placeholder.to_string(),
                models::enums::NodeType::Column,
            )];
        }

        debug!(
            "Loaded {} {} items for MySQL",
            node.children.len(),
            table_type
        );
    }

    fn load_postgresql_folder_content(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            _ => {
                node.children = vec![models::structs::TreeNode::new(
                    "Not supported for PostgreSQL".to_string(),
                    models::enums::NodeType::Column,
                )];
                return;
            }
        };

        // Try cache first
        if let Some(cached) =
            cache_data::get_tables_from_cache(self, connection_id, database_name, table_type)
            && !cached.is_empty()
        {
            node.children = cached
                .into_iter()
                .map(|name| {
                    let mut child = models::structs::TreeNode::new(
                        name,
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            _ => models::enums::NodeType::View,
                        },
                    );
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child.is_loaded = false;
                    child
                })
                .collect();
            return;
        }

        // Fallback to live fetch
        if let Some(real_items) = crate::driver_postgres::fetch_tables_from_postgres_connection(
            self,
            connection_id,
            database_name,
            table_type,
        ) {
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|n| (n.clone(), table_type.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            node.children = real_items
                .into_iter()
                .map(|name| {
                    let mut child = models::structs::TreeNode::new(
                        name,
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            _ => models::enums::NodeType::View,
                        },
                    );
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child.is_loaded = false;
                    child
                })
                .collect();
        } else {
            node.children = vec![models::structs::TreeNode::new(
                "Failed to load items".to_string(),
                models::enums::NodeType::Column,
            )];
        }
    }

    fn load_sqlite_folder_content(
        &mut self,
        connection_id: i64,
        _connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
        debug!("Loading {:?} content for SQLite", folder_type);

        // Try to get from cache first
        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            _ => {
                // For other folder types, return empty for now
                node.children = vec![models::structs::TreeNode::new(
                    "Not supported for SQLite".to_string(),
                    models::enums::NodeType::Column,
                )];
                return;
            }
        };

        if let Some(cached_items) =
            cache_data::get_tables_from_cache(self, connection_id, "main", table_type)
            && !cached_items.is_empty()
        {
            debug!(
                "Loading {} {} from cache for SQLite",
                cached_items.len(),
                table_type
            );

            node.children = cached_items
                .into_iter()
                .map(|item_name| {
                    let node_type = match folder_type {
                        models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                        models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                        _ => models::enums::NodeType::Table,
                    };

                    let mut item_node = models::structs::TreeNode::new(item_name, node_type);
                    item_node.connection_id = Some(connection_id);
                    item_node.database_name = Some("main".to_string());
                    item_node.is_loaded = false; // Will load columns on expansion if it's a table
                    item_node
                })
                .collect();

            return;
        }

        // If cache is empty, fetch from actual SQLite database
        debug!(
            "Cache miss, fetching {} from actual SQLite database",
            table_type
        );

        if let Some(real_items) =
            driver_sqlite::fetch_tables_from_sqlite_connection(self, connection_id, table_type)
        {
            debug!(
                "Successfully fetched {} {} from SQLite database",
                real_items.len(),
                table_type
            );

            // Save to cache for future use
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|name| (name.clone(), table_type.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, "main", &table_data);

            // Create tree nodes from fetched data
            let child_nodes: Vec<models::structs::TreeNode> = real_items
                .into_iter()
                .map(|item_name| {
                    let node_type = match folder_type {
                        models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                        models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                        _ => models::enums::NodeType::Table,
                    };

                    let mut item_node = models::structs::TreeNode::new(item_name, node_type);
                    item_node.connection_id = Some(connection_id);
                    item_node.database_name = Some("main".to_string());
                    item_node.is_loaded = false; // Will load columns on expansion if it's a table
                    item_node
                })
                .collect();

            node.children = child_nodes;
        } else {
            // If database fetch fails, add sample data as fallback
            debug!(
                "Failed to fetch from SQLite, using sample {} data",
                table_type
            );

            let sample_items = match folder_type {
                models::enums::NodeType::TablesFolder => vec![
                    "users".to_string(),
                    "products".to_string(),
                    "orders".to_string(),
                    "categories".to_string(),
                ],
                models::enums::NodeType::ViewsFolder => {
                    vec!["user_summary".to_string(), "order_details".to_string()]
                }
                _ => vec![],
            };

            let item_type = match folder_type {
                models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                _ => models::enums::NodeType::Column, // fallback
            };

            node.children = sample_items
                .into_iter()
                .map(|item_name| {
                    let mut item_node =
                        models::structs::TreeNode::new(item_name.clone(), item_type.clone());
                    item_node.connection_id = Some(connection_id);
                    item_node.database_name = Some("main".to_string());
                    item_node.is_loaded = false;
                    item_node
                })
                .collect();
        }

        debug!(
            "Loaded {} items into {:?} folder for SQLite",
            node.children.len(),
            folder_type
        );
    }

    fn load_mssql_folder_content(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
        debug!("Loading {:?} content for MsSQL", folder_type);
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        let (kind, node_mapper): (&str, fn(String) -> models::structs::TreeNode) = match folder_type
        {
            models::enums::NodeType::TablesFolder => ("table", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::Table);
                child.is_loaded = false;
                child
            }),
            models::enums::NodeType::ViewsFolder => ("view", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::View);
                child.is_loaded = false;
                child
            }),
            models::enums::NodeType::StoredProceduresFolder => ("procedure", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::StoredProcedure);
                child.is_loaded = true;
                child
            }),
            models::enums::NodeType::UserFunctionsFolder => ("function", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::UserFunction);
                child.is_loaded = true;
                child
            }),
            models::enums::NodeType::TriggersFolder => ("trigger", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::Trigger);
                child.is_loaded = true;
                child
            }),
            _ => {
                node.children = vec![models::structs::TreeNode::new(
                    "Unsupported folder for MsSQL".to_string(),
                    models::enums::NodeType::Column,
                )];
                return;
            }
        };

        // Try cache first
        if let Some(cached) =
            cache_data::get_tables_from_cache(self, connection_id, database_name, kind)
            && !cached.is_empty()
        {
            node.children = cached
                .into_iter()
                .map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                })
                .collect();
            return;
        }

        let fetched = match kind {
            "table" | "view" => crate::driver_mssql::fetch_tables_from_mssql_connection(
                self,
                connection_id,
                database_name,
                kind,
            ),
            "procedure" | "function" | "trigger" => {
                crate::driver_mssql::fetch_objects_from_mssql_connection(
                    self,
                    connection_id,
                    database_name,
                    kind,
                )
            }
            _ => None,
        };

        if let Some(real_items) = fetched {
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|n| (n.clone(), kind.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            node.children = real_items
                .into_iter()
                .map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                })
                .collect();
        } else {
            // fallback sample
            let sample = match kind {
                "table" => vec!["users".to_string(), "orders".to_string()],
                "view" => vec!["user_summary".to_string()],
                "procedure" => vec!["[dbo].[sp_sample]".to_string()],
                "function" => vec!["[dbo].[fn_sample]".to_string()],
                "trigger" => vec!["[dbo].[Table].[trg_sample]".to_string()],
                _ => Vec::new(),
            };
            node.children = sample
                .into_iter()
                .map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                })
                .collect();
        }
    }

    fn load_redis_folder_content(
        &mut self,
        connection_id: i64,
        _connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
        debug!("Loading {:?} content for Redis", folder_type);

        // Redis doesn't have traditional folder structures like SQL databases
        // We'll create a simplified structure based on Redis concepts
        match folder_type {
            models::enums::NodeType::TablesFolder => {
                // For Redis, "tables" could be key patterns or data structures
                let redis_structures = vec![
                    "strings".to_string(),
                    "hashes".to_string(),
                    "lists".to_string(),
                    "sets".to_string(),
                    "sorted_sets".to_string(),
                    "streams".to_string(),
                ];

                node.children = redis_structures
                    .into_iter()
                    .map(|structure_name| {
                        let mut structure_node = models::structs::TreeNode::new(
                            structure_name,
                            models::enums::NodeType::Table,
                        );
                        structure_node.connection_id = Some(connection_id);
                        structure_node.database_name = Some("redis".to_string());
                        structure_node.is_loaded = false;
                        structure_node
                    })
                    .collect();
            }
            models::enums::NodeType::ViewsFolder => {
                // For Redis, "views" could be info sections
                let info_sections = vec![
                    "server".to_string(),
                    "clients".to_string(),
                    "memory".to_string(),
                    "persistence".to_string(),
                    "stats".to_string(),
                    "replication".to_string(),
                    "cpu".to_string(),
                    "keyspace".to_string(),
                ];

                node.children = info_sections
                    .into_iter()
                    .map(|section_name| {
                        let mut section_node = models::structs::TreeNode::new(
                            section_name,
                            models::enums::NodeType::View,
                        );
                        section_node.connection_id = Some(connection_id);
                        section_node.database_name = Some("info".to_string());
                        section_node.is_loaded = false;
                        section_node
                    })
                    .collect();
            }
            _ => {
                // Other folder types not supported for Redis
                node.children = vec![models::structs::TreeNode::new(
                    "Not supported for Redis".to_string(),
                    models::enums::NodeType::Column,
                )];
            }
        }

        debug!(
            "Loaded {} items into {:?} folder for Redis",
            node.children.len(),
            folder_type
        );
    }

    fn load_table_columns_sync(
        &mut self,
        connection_id: i64,
        table_name: &str,
        connection: &models::structs::ConnectionConfig,
        database_name: &str,
    ) -> Vec<models::structs::TreeNode> {
        // First try to get from cache
        if let Some(cached_columns) =
            cache_data::get_columns_from_cache(self, connection_id, database_name, table_name)
            && !cached_columns.is_empty()
        {
            return cached_columns
                .into_iter()
                .map(|(column_name, data_type)| {
                    models::structs::TreeNode::new(
                        format!("{} ({})", column_name, data_type),
                        models::enums::NodeType::Column,
                    )
                })
                .collect();
        }

        // If cache is empty, fetch from actual database
        if let Some(real_columns) = connection::fetch_columns_from_database(
            connection_id,
            database_name,
            table_name,
            connection,
        ) {
            // Save to cache for future use
            cache_data::save_columns_to_cache(
                self,
                connection_id,
                database_name,
                table_name,
                &real_columns,
            );

            // Convert to models::structs::TreeNode
            real_columns
                .into_iter()
                .map(|(column_name, data_type)| {
                    models::structs::TreeNode::new(
                        format!("{} ({})", column_name, data_type),
                        models::enums::NodeType::Column,
                    )
                })
                .collect()
        } else {
            // If database fetch fails, return sample columns
            vec![
                models::structs::TreeNode::new(
                    "id (INTEGER)".to_string(),
                    models::enums::NodeType::Column,
                ),
                models::structs::TreeNode::new(
                    "name (VARCHAR)".to_string(),
                    models::enums::NodeType::Column,
                ),
                models::structs::TreeNode::new(
                    "created_at (TIMESTAMP)".to_string(),
                    models::enums::NodeType::Column,
                ),
            ]
        }
    }

    fn load_table_columns_for_node(
        &mut self,
        connection_id: i64,
        table_name: &str,
        nodes: &mut [models::structs::TreeNode],
        _table_index: usize,
    ) {
        // Find the connection by ID
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();

            // Find the table node to get the correct database_name
            let database_name = Tabular::find_table_database_name(nodes, table_name, connection_id)
                .unwrap_or_else(|| connection.database.clone());

            // Load columns, indexes, and primary keys from cache instead of querying server
            let columns_from_cache =
                self.load_table_columns_from_cache(connection_id, table_name, &database_name);
            let (indexes_list, pk_columns) =
                self.extract_indexes_and_pks_from_cache(connection_id, &database_name, table_name);
            let partitions_list = self.extract_partitions_from_cache(connection_id, &database_name, table_name);

            let mut columns_folder = models::structs::TreeNode::new(
                "Columns".to_string(),
                models::enums::NodeType::ColumnsFolder,
            );
            columns_folder.connection_id = Some(connection_id);
            columns_folder.database_name = Some(database_name.clone());
            columns_folder.table_name = Some(table_name.to_string());
            columns_folder.is_loaded = true;
            columns_folder.children = columns_from_cache;

            let mut indexes_folder = models::structs::TreeNode::new(
                "Indexes".to_string(),
                models::enums::NodeType::IndexesFolder,
            );
            indexes_folder.connection_id = Some(connection_id);
            indexes_folder.database_name = Some(database_name.clone());
            indexes_folder.table_name = Some(table_name.to_string());
            indexes_folder.is_loaded = true;
            indexes_folder.children = indexes_list
                .into_iter()
                .map(|idx| {
                    let mut n = models::structs::TreeNode::new(idx, models::enums::NodeType::Index);
                    n.connection_id = Some(connection_id);
                    n.database_name = Some(database_name.clone());
                    n.table_name = Some(table_name.to_string());
                    n
                })
                .collect();

            let mut pks_folder = models::structs::TreeNode::new(
                "Primary Keys".to_string(),
                models::enums::NodeType::PrimaryKeysFolder,
            );
            pks_folder.connection_id = Some(connection_id);
            pks_folder.database_name = Some(database_name.clone());
            pks_folder.table_name = Some(table_name.to_string());
            pks_folder.is_loaded = true;
            pks_folder.children = pk_columns
                .into_iter()
                .map(|col| models::structs::TreeNode::new(col, models::enums::NodeType::Column))
                .collect();

            let mut partitions_folder = models::structs::TreeNode::new(
                "Partitions".to_string(),
                models::enums::NodeType::PartitionsFolder,
            );
            partitions_folder.connection_id = Some(connection_id);
            partitions_folder.database_name = Some(database_name.clone());
            partitions_folder.table_name = Some(table_name.to_string());
            partitions_folder.is_loaded = true;
            partitions_folder.children = partitions_list
                .into_iter()
                .map(|part| {
                    // Format partition display: "name (TYPE)" if type is available
                    let display_name = if let Some(ref ptype) = part.partition_type {
                        format!("{} ({})", part.name, ptype)
                    } else {
                        part.name.clone()
                    };
                    let mut n = models::structs::TreeNode::new(display_name, models::enums::NodeType::Index);
                    n.connection_id = Some(connection_id);
                    n.database_name = Some(database_name.clone());
                    n.table_name = Some(table_name.to_string());
                    // Store the full partition info in file_path for later use
                    if let Some(ref ptype) = part.partition_type {
                        n.file_path = Some(format!("{}|{}", part.name, ptype));
                    }
                    n
                })
                .collect();

            let subfolders = vec![columns_folder, indexes_folder, pks_folder, partitions_folder];

            // Find the table node recursively and update it with subfolders
            let updated = Self::update_table_node_with_columns_recursive(
                nodes,
                table_name,
                subfolders,
                connection_id,
            );

            if !updated {
                // Log only if update failed
            }
        }
    }

    fn find_table_database_name(
        nodes: &[models::structs::TreeNode],
        table_name: &str,
        connection_id: i64,
    ) -> Option<String> {
        for node in nodes {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table
                || node.node_type == models::enums::NodeType::View)
                && node.connection_id == Some(connection_id)
            {
                let matches = if let Some(raw) = &node.table_name {
                    raw == table_name
                } else {
                    node.name == table_name
                        || Self::sanitize_display_table_name(&node.name) == table_name
                };
                if matches {
                    return node.database_name.clone();
                }
            }

            // Recursively search in children
            if let Some(found_db) =
                Self::find_table_database_name(&node.children, table_name, connection_id)
            {
                return Some(found_db);
            }
        }
        None
    }

    fn update_table_node_with_columns_recursive(
        nodes: &mut [models::structs::TreeNode],
        table_name: &str,
        columns: Vec<models::structs::TreeNode>,
        connection_id: i64,
    ) -> bool {
        for node in nodes.iter_mut() {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table
                || node.node_type == models::enums::NodeType::View)
                && node.connection_id == Some(connection_id)
            {
                let matches = if let Some(raw) = &node.table_name {
                    raw == table_name
                } else {
                    node.name == table_name
                        || Self::sanitize_display_table_name(&node.name) == table_name
                };
                if matches {
                    node.children = columns;
                    node.is_loaded = true;
                    return true;
                }
            }

            // Recursively search in children
            if Self::update_table_node_with_columns_recursive(
                &mut node.children,
                table_name,
                columns.clone(),
                connection_id,
            ) {
                return true;
            }
        }
        false
    }

    fn find_table_node_in_main_tree(
        &self,
        table_name: &str,
        connection_id: i64,
    ) -> Option<models::structs::TreeNode> {
        Self::find_table_node_recursive(&self.items_tree, table_name, connection_id)
    }

    fn find_table_node_recursive(
        nodes: &[models::structs::TreeNode],
        table_name: &str,
        connection_id: i64,
    ) -> Option<models::structs::TreeNode> {
        for node in nodes {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table
                || node.node_type == models::enums::NodeType::View)
                && node.connection_id == Some(connection_id)
            {
                let matches = if let Some(raw) = &node.table_name {
                    raw == table_name
                } else {
                    node.name == table_name
                        || Self::sanitize_display_table_name(&node.name) == table_name
                };
                if matches {
                    return Some(node.clone());
                }
            }

            // Recursively search in children
            if let Some(found_node) =
                Self::find_table_node_recursive(&node.children, table_name, connection_id)
            {
                return Some(found_node);
            }
        }
        None
    }

    fn load_table_columns_from_cache(
        &mut self,
        connection_id: i64,
        table_name: &str,
        database_name: &str,
    ) -> Vec<models::structs::TreeNode> {
        // First try to get columns from cache
        let columns_from_cache = crate::cache_data::get_columns_from_cache(
            self,
            connection_id,
            database_name,
            table_name,
        );

        if let Some(columns_data) = columns_from_cache
            && !columns_data.is_empty()
        {
            // If cache has data, use it
            return columns_data
                .into_iter()
                .map(|(column_name, data_type)| {
                    let mut column_node = models::structs::TreeNode::new(
                        format!("{} ({})", column_name, data_type),
                        models::enums::NodeType::Column,
                    );
                    column_node.connection_id = Some(connection_id);
                    column_node.database_name = Some(database_name.to_string());
                    column_node.table_name = Some(table_name.to_string());
                    column_node
                })
                .collect();
        }

        // If cache doesn't have data or is empty, fallback to server query
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();
            self.load_table_columns_sync(connection_id, table_name, &connection, database_name)
        } else {
            Vec::new()
        }
    }

    fn extract_indexes_and_pks_from_cache(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) -> (Vec<String>, Vec<String>) {
        // Try to get primary keys from cache first
        let pk_columns = if let Some(pks) =
            cache_data::get_primary_keys_from_cache(self, connection_id, database_name, table_name)
        {
            if !pks.is_empty() {
                pks
            } else {
                // Cache is empty, fallback to server query
                if let Some(connection) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                {
                    let connection = connection.clone();
                    self.fetch_primary_key_columns_for_table(
                        connection_id,
                        &connection,
                        database_name,
                        table_name,
                    )
                } else {
                    Vec::new()
                }
            }
        } else {
            // Cache doesn't have data, fallback to server query
            if let Some(connection) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
            {
                let connection = connection.clone();
                self.fetch_primary_key_columns_for_table(
                    connection_id,
                    &connection,
                    database_name,
                    table_name,
                )
            } else {
                Vec::new()
            }
        };

        // Try to get index names from cache first (fast tree render)
        let indexes_list = if let Some(names) =
            cache_data::get_index_names_from_cache(self, connection_id, database_name, table_name)
        {
            if !names.is_empty() {
                names
            } else {
                // Cache empty: fallback to live fetch and seed cache with names
                if let Some(connection) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                {
                    let connection = connection.clone();
                    let names = self.fetch_index_names_for_table(
                        connection_id,
                        &connection,
                        database_name,
                        table_name,
                    );
                    if !names.is_empty() {
                        let stubs: Vec<models::structs::IndexStructInfo> = names
                            .iter()
                            .map(|n| models::structs::IndexStructInfo {
                                name: n.clone(),
                                method: None,
                                unique: false,
                                columns: Vec::new(),
                            })
                            .collect();
                        cache_data::save_indexes_to_cache(
                            self,
                            connection_id,
                            database_name,
                            table_name,
                            &stubs,
                        );
                    }
                    names
                } else {
                    Vec::new()
                }
            }
        } else {
            // No cache table or error: fallback and seed cache
            if let Some(connection) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
            {
                let connection = connection.clone();
                let names = self.fetch_index_names_for_table(
                    connection_id,
                    &connection,
                    database_name,
                    table_name,
                );
                if !names.is_empty() {
                    let stubs: Vec<models::structs::IndexStructInfo> = names
                        .iter()
                        .map(|n| models::structs::IndexStructInfo {
                            name: n.clone(),
                            method: None,
                            unique: false,
                            columns: Vec::new(),
                        })
                        .collect();
                    cache_data::save_indexes_to_cache(
                        self,
                        connection_id,
                        database_name,
                        table_name,
                        &stubs,
                    );
                }
                names
            } else {
                Vec::new()
            }
        };

        (indexes_list, pk_columns)
    }

    fn extract_partitions_from_cache(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) -> Vec<models::structs::PartitionStructInfo> {
        // Try cache first
        if let Some(cached_partitions) = cache_data::get_partitions_from_cache(self, connection_id, database_name, table_name) {
            debug!("📚 Using cached partitions for {}/{} ({} partitions)", database_name, table_name, cached_partitions.len());
            return cached_partitions;
        }
        
        // Cache miss - fetch from database
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();
            debug!("🔍 Fetching partition details from DB for {}/{}", database_name, table_name);
            // Use the fetch function from data_table module
            let partitions = crate::data_table::fetch_partition_details_for_table(
                self,
                connection_id,
                &connection,
                database_name,
                table_name,
            );
            debug!("📊 Fetched {} partitions from database", partitions.len());
            if !partitions.is_empty() {
                debug!("💾 Saving {} partitions to cache", partitions.len());
                cache_data::save_partitions_to_cache(
                    self,
                    connection_id,
                    database_name,
                    table_name,
                    &partitions,
                );
            }
            partitions
        } else {
            debug!("⚠️  Connection not found: {}", connection_id);
            Vec::new()
        }
    }

    // Fetch index names per database type
    fn fetch_index_names_for_table(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        database_name: &str,
        table_name: &str,
    ) -> Vec<String> {
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT DISTINCT INDEX_NAME FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(database_name)
                            .bind(table_name)
                            .fetch_all(mysql_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::PostgreSQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT indexname FROM pg_indexes WHERE schemaname = 'public' AND tablename = $1 ORDER BY indexname";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(table_name)
                            .fetch_all(pg_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::SQLite => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::SQLite(sqlite_pool)) =
                        connection::get_or_create_connection_pool(self, connection_id).await
                    {
                        let escaped = table_name.replace("'", "''");
                        let q = format!("PRAGMA index_list('{}')", escaped);
                        match sqlx::query(&q).fetch_all(sqlite_pool.as_ref()).await {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut names = Vec::new();
                                for r in rows {
                                    if let Ok(Some(n)) = r.try_get::<Option<String>, _>("name") {
                                        names.push(n);
                                    }
                                }
                                names
                            }
                            Err(_) => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                })
            }
            models::enums::DatabaseType::MsSQL => {
                // Use tiberius
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                let host = connection.host.clone();
                let port: u16 = connection.port.parse().unwrap_or(1433);
                let user = connection.username.clone();
                let pass = connection.password.clone();
                let db = database_name.to_string();
                let tbl = table_name.to_string();
                let rt_res = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    // Parse schema-qualified name
                    let parse = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 { return (Some(parts[0].to_string()), parts[1].to_string()); }
                        }
                        if let Some((s, t)) = name.split_once('.') { return (Some(s.trim_matches(|c| c=='['||c==']').to_string()), t.trim_matches(|c| c=='['||c==']').to_string()); }
                        (None, name.trim_matches(|c| c=='['||c==']').to_string())
                    };
                    let (schema_opt, table_only) = parse(&tbl);
                    let mut q = format!("SELECT i.name FROM sys.indexes i INNER JOIN sys.objects o ON i.object_id = o.object_id WHERE o.name = '{}' AND i.name IS NOT NULL", table_only.replace("'", "''"));
                    if let Some(s) = schema_opt { q.push_str(&format!(" AND SCHEMA_NAME(o.schema_id) = '{}'", s.replace("'", "''"))); }
                    q.push_str(" ORDER BY i.name");
                    let mut stream = client.simple_query(q).await.map_err(|e| e.to_string())?;
                    let mut list = Vec::new();
                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let tiberius::QueryItem::Row(r) = item { let n: Option<&str> = r.get(0); if let Some(nm) = n { list.push(nm.to_string()); } } }
                    Ok::<_, String>(list)
                });
                rt_res.unwrap_or_default()
            }
            models::enums::DatabaseType::Redis => Vec::new(),
            models::enums::DatabaseType::MongoDB => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MongoDB(client)) =
                        connection::get_or_create_connection_pool(self, connection_id).await
                    {
                        let coll = client
                            .database(database_name)
                            .collection::<mongodb::bson::Document>(table_name);
                        (coll.list_index_names().await).unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                })
            }
        }
    }

    // Fetch primary key column names per database type
    fn fetch_primary_key_columns_for_table(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        database_name: &str,
        table_name: &str,
    ) -> Vec<String> {
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(database_name)
                            .bind(table_name)
                            .fetch_all(mysql_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::PostgreSQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT a.attname FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = ANY(i.indkey) JOIN pg_namespace n ON n.oid = c.relnamespace WHERE i.indisprimary AND c.relname = $1 AND n.nspname = 'public' ORDER BY a.attnum";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(table_name)
                            .fetch_all(pg_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::SQLite => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::SQLite(sqlite_pool)) =
                        connection::get_or_create_connection_pool(self, connection_id).await
                    {
                        let escaped = table_name.replace("'", "''");
                        let q = format!("PRAGMA table_info('{}')", escaped);
                        match sqlx::query(&q).fetch_all(sqlite_pool.as_ref()).await {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut names = Vec::new();
                                for r in rows {
                                    let pk: i64 = r.try_get::<i64, _>("pk").unwrap_or(0);
                                    if pk > 0
                                        && let Ok(Some(n)) = r.try_get::<Option<String>, _>("name")
                                    {
                                        names.push(n);
                                    }
                                }
                                names
                            }
                            Err(_) => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                })
            }
            models::enums::DatabaseType::MsSQL => {
                // Use tiberius
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                let host = connection.host.clone();
                let port: u16 = connection.port.parse().unwrap_or(1433);
                let user = connection.username.clone();
                let pass = connection.password.clone();
                let db = database_name.to_string();
                let tbl = table_name.to_string();
                let rt_res = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    // Parse schema-qualified name
                    let parse = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 { return (Some(parts[0].to_string()), parts[1].to_string()); }
                        }
                        if let Some((s, t)) = name.split_once('.') { return (Some(s.trim_matches(|c| c=='['||c==']').to_string()), t.trim_matches(|c| c=='['||c==']').to_string()); }
                        (None, name.trim_matches(|c| c=='['||c==']').to_string())
                    };
                    let (schema_opt, table_only) = parse(&tbl);
                    let mut q = String::from("SELECT c.name FROM sys.indexes i JOIN sys.index_columns ic ON i.object_id = ic.object_id AND i.index_id = ic.index_id JOIN sys.columns c ON ic.object_id = c.object_id AND ic.column_id = c.column_id JOIN sys.objects o ON i.object_id = o.object_id WHERE i.is_primary_key = 1");
                    q.push_str(&format!(" AND o.name = '{}'", table_only.replace("'", "''")));
                    if let Some(s) = schema_opt { q.push_str(&format!(" AND SCHEMA_NAME(o.schema_id) = '{}'", s.replace("'", "''"))); }
                    q.push_str(" ORDER BY ic.key_ordinal");
                    let mut stream = client.simple_query(q).await.map_err(|e| e.to_string())?;
                    let mut list = Vec::new();
                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let tiberius::QueryItem::Row(r) = item { let n: Option<&str> = r.get(0); if let Some(nm) = n { list.push(nm.to_string()); } } }
                    Ok::<_, String>(list)
                });
                rt_res.unwrap_or_default()
            }
            models::enums::DatabaseType::Redis => Vec::new(),
            models::enums::DatabaseType::MongoDB => vec!["_id".to_string()],
        }
    }



    pub fn execute_paginated_query(&mut self) {
        debug!("🔥 Starting execute_paginated_query()");
        self.query_execution_in_progress = true;
        self.extend_query_icon_hold();
        // Note: is_table_browse_mode is NOT set here - it should only be true when browsing tables via sidebar
        // Use connection from active tab, not global current_connection_id
        let connection_id = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|tab| tab.connection_id);

        debug!(
            "🔥 execute_paginated_query: active_tab_index={}, connection_id={:?}",
            self.active_tab_index, connection_id
        );

        if let Some(connection_id) = connection_id {
            // Check if connection pool is being created to avoid infinite retry loops
            if self.pending_connection_pools.contains(&connection_id) {
                debug!(
                    "⏳ Connection pool creation in progress for connection {}, skipping pagination for now",
                    connection_id
                );
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
                return;
            }

            let offset = self.current_page * self.page_size;
            debug!(
                "🔥 About to build paginated query with offset={}, page_size={}, connection_id={}",
                offset, self.page_size, connection_id
            );
            let paginated_query = self.build_paginated_query(offset, self.page_size);
            debug!("🔥 Built paginated query: {}", paginated_query);
            let prev_headers = self.current_table_headers.clone();
            let requested_page = self.current_page;

            let job_id = self.next_query_job_id;
            self.next_query_job_id = self.next_query_job_id.wrapping_add(1);

            match connection::prepare_query_job(
                self,
                connection_id,
                paginated_query.clone(),
                job_id,
            ) {
                Ok(mut job) => {
                    job.options.save_to_history = false;
                    let status = connection::QueryJobStatus {
                        job_id,
                        connection_id,
                        query_preview: paginated_query.chars().take(80).collect(),
                        started_at: std::time::Instant::now(),
                        completed: false,
                    };
                    self.active_query_jobs.insert(job_id, status);
                    self.pending_paginated_jobs.insert(job_id);

                    match connection::spawn_query_job(self, job, self.query_result_sender.clone()) {
                        Ok(handle) => {
                            self.active_query_handles.insert(job_id, handle);
                            self.current_table_name =
                                format!("Loading page {}…", self.current_page.saturating_add(1));
                            return;
                        }
                        Err(err) => {
                            debug!(
                                "⚠️ Failed to spawn paginated query job {:?}. Falling back to sync execution.",
                                err
                            );
                            self.active_query_jobs.remove(&job_id);
                            self.pending_paginated_jobs.remove(&job_id);
                        }
                    }
                }
                Err(err) => {
                    debug!(
                        "⚠️ Failed to prepare paginated query job: {:?}. Falling back to sync execution.",
                        err
                    );
                }
            }

            if let Some((headers, data)) =
                connection::execute_query_with_connection(self, connection_id, paginated_query)
            {
                debug!(
                    "[execute_paginated_query] got result: rows={}, cols={}",
                    data.len(),
                    headers.len()
                );
                // If we navigated past the last page (offset beyond available rows), keep previous headers and revert page
                if data.is_empty() && offset > 0 {
                    // Heuristic: previous page had < page_size rows or actual_total_rows known and offset >= actual_total_rows
                    let past_end = if let Some(total) = self.actual_total_rows {
                        offset >= total
                    } else {
                        self.current_page > 0 && self.total_rows < self.page_size
                    };
                    if past_end {
                        debug!(
                            "🔙 Requested page {} out of range (offset {}), reverting to previous page",
                            requested_page + 1,
                            offset
                        );
                        // Revert page index
                        if requested_page > 0 {
                            self.current_page = requested_page - 1;
                        }
                        // Keep previous headers and data (do not overwrite)
                        self.current_table_headers = prev_headers;
                        // No further sync needed
                        self.query_execution_in_progress = false;
                        self.extend_query_icon_hold();
                        return;
                    }
                }

                // Normal assignment (including empty last page that is valid)
                self.current_table_headers = if headers.is_empty() {
                    if !prev_headers.is_empty() {
                        prev_headers
                    } else {
                        headers
                    }
                } else {
                    headers
                };
                debug!(
                    "[execute_paginated_query] assigning to current_table: rows={}, cols={}",
                    self.current_table_data.len(),
                    self.current_table_headers.len()
                );
                self.current_table_data = data;
                // For server pagination, total_rows represents current page row count only (used for UI row count display)
                self.total_rows = self.current_table_data.len();
                // Sync ke tab aktif agar mode table tab (tanpa editor) bisa menampilkan Data
                if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                    debug!(
                        "[execute_paginated_query] sync to tab {}: rows={} cols={}",
                        self.active_tab_index,
                        self.current_table_data.len(),
                        self.current_table_headers.len()
                    );
                    active_tab.result_headers = self.current_table_headers.clone();
                    active_tab.result_rows = self.current_table_data.clone();
                    active_tab.result_all_rows = self.current_table_data.clone(); // single page snapshot
                    active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
                    active_tab.current_page = self.current_page;
                    active_tab.page_size = self.page_size;
                    // Note: is_table_browse_mode is not forced here - it inherits from self
                    active_tab.is_table_browse_mode = self.is_table_browse_mode;
                }

                // Save this first page into row cache (only when on first page)
                if self.current_page == 0 {
                    // Determine database and table names for cache key
                    let db_name = self
                        .query_tabs
                        .get(self.active_tab_index)
                        .and_then(|t| t.database_name.clone())
                        .unwrap_or_default();
                    let table = data_table::infer_current_table_name(self);
                    if !db_name.is_empty() && !table.is_empty() {
                        let snapshot: Vec<Vec<String>> =
                            self.current_table_data.iter().take(100).cloned().collect();
                        let headers_clone = self.current_table_headers.clone();
                        crate::cache_data::save_table_rows_to_cache(
                            self,
                            connection_id,
                            &db_name,
                            &table,
                            &headers_clone,
                            &snapshot,
                        );
                        info!(
                            "💾 Cached first 100 rows (server pagination) for {}/{}",
                            db_name, table
                        );
                    }
                }
            }
        } else {
            debug!("🔥 No connection_id available in active tab for paginated query");
        }

        self.query_execution_in_progress = false;
        self.extend_query_icon_hold();
    }

    fn build_paginated_query(&self, offset: usize, limit: usize) -> String {
        // Get the base query from the active tab - NO fallback to global state
        let base_query = if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
            if tab.base_query.is_empty() {
                None
            } else {
                Some(&tab.base_query)
            }
        } else {
            None
        };

        debug!(
            "🔍 build_paginated_query: active_tab_index={}, base_query='{}'",
            self.active_tab_index,
            base_query.unwrap_or(&"<empty>".to_string())
        );

        let Some(base_query) = base_query else {
            debug!("❌ build_paginated_query: base_query is empty, returning empty string");
            return String::new();
        };

        // Get the database type from active tab's connection
        let connection_id = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|tab| tab.connection_id);

        let db_type = if let Some(connection_id) = connection_id {
            self.connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .map(|c| &c.connection_type)
                .unwrap_or(&models::enums::DatabaseType::MySQL)
        } else {
            &models::enums::DatabaseType::MySQL
        };

        // If base_query already contains a LIMIT clause, avoid appending another LIMIT/OFFSET
        let has_limit = {
            let upper = base_query.to_uppercase();
            upper.contains(" LIMIT ")
                || upper.ends_with(" LIMIT")
                || upper.contains("\nLIMIT ")
        };

        if has_limit {
            debug!(
                "🔍 build_paginated_query: base_query already has LIMIT, returning without pagination"
            );
            return base_query.clone();
        }

        match db_type {
            models::enums::DatabaseType::MySQL | models::enums::DatabaseType::SQLite => {
                format!("{} LIMIT {} OFFSET {}", base_query, limit, offset)
            }
            models::enums::DatabaseType::PostgreSQL => {
                format!("{} LIMIT {} OFFSET {}", base_query, limit, offset)
            }
            models::enums::DatabaseType::MsSQL => {
                // MsSQL requires ORDER BY for OFFSET/FETCH. Inject ORDER BY 1 if missing.
                // Handle optional leading USE statement separated by semicolon.
                let mut base = base_query.clone();
                debug!("🔍 MsSQL base query before processing: {}", base);

                let mut prefix = String::new();
                // Separate USE ...; prefix if present so pagination applies only to SELECT part
                if let Some(use_end) = base.find(";\nSELECT") {
                    // include the semicolon in prefix
                    prefix = base[..=use_end].to_string();
                    base = base[use_end + 2..].to_string(); // skip "\n" keeping SELECT...
                }

                // Trim and remove trailing semicolons/spaces
                let mut select_part = base.trim().trim_end_matches(';').to_string();
                debug!("🔍 MsSQL select part before TOP removal: {}", select_part);

                // Enhanced TOP removal using case-insensitive regex-like approach
                select_part = driver_mssql::sanitize_mssql_select_for_pagination(&select_part);
                debug!("🔍 MsSQL select part after TOP removal: {}", select_part);

                // Detect ORDER BY (case-insensitive)
                let has_order = select_part.to_lowercase().contains("order by");
                if !has_order {
                    select_part.push_str(" ORDER BY 1");
                }
                let effective_limit = if limit == 0 { 100 } else { limit }; // safety
                let mut final_query = format!(
                    "{}{} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                    prefix, select_part, offset, effective_limit
                );
                // check if contain TOP 1000 than replace it
                final_query = final_query.replace("TOP 10000", "");
                debug!(" *** final_query *** : {}", final_query);

                debug!("🧪 MsSQL final paginated query: {}", final_query);
                final_query
            }
            _ => {
                // For Redis/MongoDB, return original query (these don't use SQL pagination)
                base_query.clone()
            }
        }
    }

    pub fn set_page_size(&mut self, new_size: usize) {
        if new_size > 0 {
            // Check if we have a base query in the active tab for server-side pagination
            let has_base_query = self
                .query_tabs
                .get(self.active_tab_index)
                .map(|tab| !tab.base_query.is_empty())
                .unwrap_or(false);

            self.page_size = new_size;
            if self.use_server_pagination && has_base_query {
                // Reset to first page and re-execute query
                self.current_page = 0;
                self.execute_paginated_query();
            } else {
                // Client-side pagination
                self.current_page = 0;
                self.update_current_page_data();
            }
            data_table::clear_table_selection(self);
        }
    }

    fn execute_count_query(&mut self) -> Option<usize> {
        // For large tables, we don't want to run actual count queries as they can be very slow
        // or cause timeouts. Instead, we assume a reasonable default size for pagination.
        // This prevents the server from being overwhelmed by expensive COUNT(*) operations.

        debug!("📊 Using default row count assumption for large table pagination");
        debug!("✅ Assuming table has data with default pagination size of 10,000 rows");

        // Return a reasonable default that enables pagination
        // This allows users to navigate through pages without expensive count operations
        Some(10000)
    }

    fn initialize_server_pagination(&mut self, base_query: String) {
        debug!(
            "🚀 Initializing server pagination with base query: {}",
            base_query
        );
        self.current_base_query = base_query.clone();
        self.current_page = 0;

        // Also save the base query to the active tab
        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.base_query = base_query;
        }

        // Execute count query to get total rows (now using default assumption)
        if let Some(total) = self.execute_count_query() {
            debug!("✅ Count query successful, total rows: {}", total);
            self.actual_total_rows = Some(total);
        } else {
            debug!("❌ Count query failed, no total available");
            self.actual_total_rows = None;
        }

        // Execute first page
        debug!("📄 Executing first page query...");
        self.execute_paginated_query();
        debug!(
            "🏁 Server pagination initialization complete. actual_total_rows: {:?}",
            self.actual_total_rows
        );
        debug!(
            "🎯 Ready for pagination with {} total pages",
            data_table::get_total_pages(self)
        );
    }

    fn render_tree_for_database_section(&mut self, ui: &mut egui::Ui) {
        // Add responsive search box
        ui.horizontal(|ui| {
            // Make search box responsive to sidebar width
            let available_width = ui.available_width() - 5.0; // Leave space for clear button and padding
            let search_response = ui.add_sized(
                [available_width, 20.0],
                egui::TextEdit::singleline(&mut self.database_search_text)
                    .hint_text("Search databases, tables, keys..."),
            );

            if search_response.changed() {
                self.update_search_results();
            }
        });

        // Use search results if search is active, otherwise use normal tree
        if self.show_search_results && !self.database_search_text.trim().is_empty() {
            // Show search results
            let mut filtered_tree = std::mem::take(&mut self.filtered_items_tree);
            let _ = self.render_tree(ui, &mut filtered_tree, true);
            self.filtered_items_tree = filtered_tree;
        } else {
            // Show normal tree
            // Use slice to avoid borrowing issues
            let mut items_tree = std::mem::take(&mut self.items_tree);

            let query_files_to_open = self.render_tree(ui, &mut items_tree, false);
            
            for (filename, content, file_path, context_connection_id) in query_files_to_open {
                 if file_path.is_empty() {
                     // Custom View or similar: Use the context connection ID if available
                     let _ = crate::editor::create_new_tab_with_connection_and_database(
                        self,
                        filename,
                        content,
                        context_connection_id,
                        None // Database name is usually baked into the query or will be selected
                     );
                     
                     // Auto-execute if it's a Custom View (implied by having a connection ID context)
                     if context_connection_id.is_some()
                        && let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            tab.should_run_on_open = true;
                        }
                 } else if let Err(err) = crate::sidebar_query::open_query_file(self, &file_path) {
                     log::error!("Failed to open query file: {}", err);
                 }
            }


            // Check if tree was refreshed inside render_tree
            if self.items_tree.is_empty() {
                // Tree was not refreshed, restore the modified tree
                self.items_tree = items_tree;
            } else {
                // Tree was refreshed inside render_tree, keep the new tree
                debug!("Tree was refreshed inside render_tree, keeping the new tree");
            }
        }
    }

    fn update_search_results(&mut self) {
        // Clone search text to avoid borrowing issues
        let search_text = self.database_search_text.trim().to_string();

        if search_text.is_empty() {
            self.show_search_results = false;
            self.filtered_items_tree.clear();
            return;
        }

        self.show_search_results = true;
        self.filtered_items_tree.clear();

        // Search through the main items_tree with LIKE functionality
        for node in &self.items_tree {
            if let Some(filtered_node) = self.filter_node_with_like_search(node, &search_text) {
                self.filtered_items_tree.push(filtered_node);
            }
        }

        // Search in all connections' cached data
        let connection_ids: Vec<i64> = self.connections.iter().filter_map(|c| c.id).collect();

        for connection_id in connection_ids {
            self.search_in_connection_data(connection_id, &search_text);
        }
    }

    fn filter_node_with_like_search(
        &self,
        node: &models::structs::TreeNode,
        search_text: &str,
    ) -> Option<models::structs::TreeNode> {
        let mut matches = false;
        let mut filtered_children = Vec::new();

        // Check if current node matches using case-sensitive LIKE search
        // LIKE search: if search text is contained anywhere in the node name
        if node.name.contains(search_text) {
            matches = true;
        }

        // Check children recursively
        for child in &node.children {
            if let Some(filtered_child) = self.filter_node_with_like_search(child, search_text) {
                filtered_children.push(filtered_child);
                matches = true;
            }
        }

        if matches {
            let mut filtered_node = node.clone();

            // For table nodes, preserve loaded state and children from main tree
            if (filtered_node.node_type == models::enums::NodeType::Table
                || filtered_node.node_type == models::enums::NodeType::View)
                && filtered_node.connection_id.is_some()
            {
                if let Some(main_tree_node) = self.find_table_node_in_main_tree(
                    &filtered_node.name,
                    filtered_node.connection_id.unwrap(),
                ) {
                    filtered_node.is_loaded = main_tree_node.is_loaded;
                    filtered_node.is_expanded = main_tree_node.is_expanded;
                    if main_tree_node.is_loaded {
                        filtered_node.children = main_tree_node.children.clone();
                    } else {
                        filtered_node.children = filtered_children;
                    }
                } else {
                    filtered_node.children = filtered_children;
                    filtered_node.is_expanded = true; // Auto-expand search results
                }
            } else {
                filtered_node.children = filtered_children;
                filtered_node.is_expanded = true; // Auto-expand search results
            }

            Some(filtered_node)
        } else {
            None
        }
    }

    fn search_in_connection_data(&mut self, connection_id: i64, search_text: &str) {
        // Find the connection to determine its type
        let connection_type = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .map(|c| c.connection_type.clone());

        if let Some(conn_type) = connection_type {
            match conn_type {
                models::enums::DatabaseType::Redis => {
                    self.search_redis_keys(connection_id, search_text);
                }
                models::enums::DatabaseType::MySQL
                | models::enums::DatabaseType::PostgreSQL
                | models::enums::DatabaseType::SQLite => {
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
                models::enums::DatabaseType::MsSQL => {
                    // Basic table search (reuse SQL logic)
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
                models::enums::DatabaseType::MongoDB => {
                    // Reuse SQL table cache search; collections are stored in table_cache with table_type='collection'
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
            }
        }
    }

    fn search_redis_keys(&mut self, connection_id: i64, search_text: &str) {
        // Search through Redis keys using SCAN with flexible pattern
        let rt = tokio::runtime::Runtime::new().unwrap();

        let search_results = rt.block_on(async {
            if let Some(models::enums::DatabasePool::Redis(redis_manager)) =
                connection::get_or_create_connection_pool(self, connection_id).await
            {
                let mut conn = redis_manager.as_ref().clone();

                // Use flexible pattern for LIKE search - search text can appear anywhere
                let pattern = format!("*{}*", search_text);
                let mut cursor = 0u64;
                let mut found_keys = Vec::new();

                // First try exact pattern match
                for _iteration in 0..20 {
                    // Increase iterations for more comprehensive search
                    let scan_result: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(&pattern)
                        .arg("COUNT")
                        .arg(100) // Increase count for better performance
                        .query_async(&mut conn)
                        .await;

                    if let Ok((new_cursor, keys)) = scan_result {
                        // Additional filtering for case-sensitive LIKE search
                        for key in keys {
                            if key.contains(search_text) {
                                found_keys.push(key);
                            }
                        }
                        cursor = new_cursor;
                        if cursor == 0 {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                found_keys
            } else {
                Vec::new()
            }
        });

        // Add search results to filtered tree
        if !search_results.is_empty() {
            // Find or create the connection node in filtered results
            let connection_name = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "Unknown Connection".to_string());

            let mut search_result_node = models::structs::TreeNode::new(
                format!(
                    "🔍 Search Results in {} ({} keys)",
                    connection_name,
                    search_results.len()
                ),
                models::enums::NodeType::CustomFolder,
            );
            search_result_node.connection_id = Some(connection_id);
            search_result_node.is_expanded = true;

            // Add found keys as children
            for key in search_results {
                let mut key_node =
                    models::structs::TreeNode::new(key.clone(), models::enums::NodeType::Table);
                key_node.connection_id = Some(connection_id);
                search_result_node.children.push(key_node);
            }

            self.filtered_items_tree.push(search_result_node);
        }
    }

    fn search_sql_tables(
        &mut self,
        connection_id: i64,
        search_text: &str,
        db_type: &models::enums::DatabaseType,
    ) {
        // Search through cached table data and column data
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let search_pattern = format!("*{}*", search_text); // Using GLOB pattern for case-sensitive search
            let rt = tokio::runtime::Runtime::new().unwrap();

            // Search tables
            let table_search_results = rt.block_on(async {
                let query = match db_type {
                    models::enums::DatabaseType::SQLite => {
                        "SELECT table_name, database_name, table_type FROM table_cache WHERE connection_id = ? AND table_name GLOB ? ORDER BY table_name"
                    }
                    _ => {
                        "SELECT table_name, database_name, table_type FROM table_cache WHERE connection_id = ? AND table_name LIKE ? COLLATE BINARY ORDER BY database_name, table_name"
                    }
                };

                let search_param = match db_type {
                    models::enums::DatabaseType::SQLite => &search_pattern,
                    _ => &format!("%{}%", search_text), // For non-SQLite, use LIKE with COLLATE BINARY for case sensitivity
                };

                sqlx::query_as::<_, (String, String, String)>(query)
                    .bind(connection_id)
                    .bind(search_param)
                    .fetch_all(pool_clone.as_ref())
                    .await
                    .unwrap_or_default()
            });

            // Search columns
            let column_search_results = rt.block_on(async {
                let query = match db_type {
                    models::enums::DatabaseType::SQLite => {
                        "SELECT DISTINCT table_name, database_name, column_name, data_type FROM column_cache WHERE connection_id = ? AND column_name GLOB ? ORDER BY table_name"
                    }
                    _ => {
                        "SELECT DISTINCT table_name, database_name, column_name, data_type FROM column_cache WHERE connection_id = ? AND column_name LIKE ? COLLATE BINARY ORDER BY database_name, table_name"
                    }
                };

                let search_param = match db_type {
                    models::enums::DatabaseType::SQLite => &search_pattern,
                    _ => &format!("%{}%", search_text), // For non-SQLite, use LIKE with COLLATE BINARY for case sensitivity
                };

                sqlx::query_as::<_, (String, String, String, String)>(query)
                    .bind(connection_id)
                    .bind(search_param)
                    .fetch_all(pool_clone.as_ref())
                    .await
                    .unwrap_or_default()
            });

            // Group table results by database
            let mut table_results_by_db: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for (table_name, database_name, _table_type) in table_search_results {
                table_results_by_db
                    .entry(database_name)
                    .or_default()
                    .push(table_name);
            }

            // Group column results by database and table
            let mut column_results_by_db: std::collections::HashMap<
                String,
                std::collections::HashMap<String, Vec<(String, String)>>,
            > = std::collections::HashMap::new();
            for (table_name, database_name, column_name, data_type) in column_search_results {
                column_results_by_db
                    .entry(database_name)
                    .or_default()
                    .entry(table_name)
                    .or_default()
                    .push((column_name, data_type));
            }

            // Add search results to filtered tree
            if !table_results_by_db.is_empty() || !column_results_by_db.is_empty() {
                let connection_name = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "Unknown Connection".to_string());

                let total_tables: usize = table_results_by_db.values().map(|v| v.len()).sum();
                let total_columns: usize = column_results_by_db
                    .values()
                    .flat_map(|db| db.values())
                    .map(|cols| cols.len())
                    .sum();

                let mut search_result_node = models::structs::TreeNode::new(
                    format!(
                        "🔍 Search Results in {} ({} tables, {} columns)",
                        connection_name, total_tables, total_columns
                    ),
                    models::enums::NodeType::CustomFolder,
                );
                search_result_node.connection_id = Some(connection_id);
                search_result_node.is_expanded = true;

                // Combine all databases from both searches
                let mut all_databases: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                all_databases.extend(table_results_by_db.keys().cloned());
                all_databases.extend(column_results_by_db.keys().cloned());

                // Add databases and their tables/columns
                for database_name in all_databases {
                    let tables = table_results_by_db
                        .get(&database_name)
                        .cloned()
                        .unwrap_or_default();
                    let column_tables = column_results_by_db
                        .get(&database_name)
                        .cloned()
                        .unwrap_or_default();

                    let mut db_node = models::structs::TreeNode::new(
                        format!(
                            "📁 {} ({} tables, {} column matches)",
                            database_name,
                            tables.len(),
                            column_tables.values().map(|cols| cols.len()).sum::<usize>()
                        ),
                        models::enums::NodeType::Database,
                    );
                    db_node.connection_id = Some(connection_id);
                    db_node.database_name = Some(database_name.clone());
                    db_node.is_expanded = true;

                    // Add tables found by table name search
                    for table_name in tables {
                        let mut table_node = models::structs::TreeNode::new(
                            format!("📋 {} (table name match)", table_name),
                            models::enums::NodeType::Table,
                        );
                        table_node.connection_id = Some(connection_id);
                        table_node.database_name = Some(database_name.clone());
                        // Store the actual table name without icon for query generation
                        table_node.table_name = Some(table_name);
                        db_node.children.push(table_node);
                    }

                    // Add tables found by column name search
                    for (table_name, columns) in column_tables {
                        let mut table_node = models::structs::TreeNode::new(
                            format!("📋 {} ({} column matches)", table_name, columns.len()),
                            models::enums::NodeType::Table,
                        );
                        table_node.connection_id = Some(connection_id);
                        table_node.database_name = Some(database_name.clone());
                        // Store the actual table name without icon for query generation
                        table_node.table_name = Some(table_name.clone());

                        // Add matching columns as children
                        for (column_name, data_type) in columns {
                            let mut column_node = models::structs::TreeNode::new(
                                format!("🔧 {} ({})", column_name, data_type),
                                models::enums::NodeType::Column,
                            );
                            column_node.connection_id = Some(connection_id);
                            column_node.database_name = Some(database_name.clone());
                            // For columns, we can store the table name in table_name field
                            // The actual column name is already in the display name without icon
                            column_node.table_name = Some(table_name.clone());
                            table_node.children.push(column_node);
                        }

                        db_node.children.push(table_node);
                    }

                    search_result_node.children.push(db_node);
                }

                self.filtered_items_tree.push(search_result_node);
            }
        }
    }

    fn find_redis_key_info(
        node: &models::structs::TreeNode,
        key_name: &str,
    ) -> Option<(String, String)> {
        // Check if this node is a type folder (like "Strings (5)")
        if node.node_type == models::enums::NodeType::TablesFolder {
            // Extract the type from folder name
            let folder_type = if node.name.starts_with("Strings") {
                "string"
            } else if node.name.starts_with("Hashes") {
                "hash"
            } else if node.name.starts_with("Lists") {
                "list"
            } else if node.name.starts_with("Sets") {
                "set"
            } else if node.name.starts_with("Sorted Sets") {
                "zset"
            } else if node.name.starts_with("Streams") {
                "stream"
            } else {
                // Continue searching instead of returning None
                "unknown"
            };

            // Search for the key in this folder's children
            for child in &node.children {
                debug!(
                    "🔍 Checking child: '{}' (type: {:?})",
                    child.name, child.node_type
                );
                if child.node_type == models::enums::NodeType::Table
                    && child.name == key_name
                    && let Some(db_name) = &child.database_name
                {
                    return Some((db_name.clone(), folder_type.to_string()));
                }
            }
        }

        // Recursively search in children
        for child in &node.children {
            if let Some((db_name, key_type)) = Self::find_redis_key_info(child, key_name) {
                return Some((db_name, key_type));
            }
        }

        None
    }

    fn find_database_name_for_table(
        node: &models::structs::TreeNode,
        connection_id: i64,
        table_name: &str,
    ) -> Option<String> {
        // Look for the table in the tree structure to find its database context

        // Check if this node is a table with the matching name and connection
        // Use table_name field if available (for search results), otherwise use node.name
        let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name);
        if (node.node_type == models::enums::NodeType::Table
            || node.node_type == models::enums::NodeType::View)
            && actual_table_name == table_name
            && node.connection_id == Some(connection_id)
        {
            return node.database_name.clone();
        }

        // Recursively search in children
        for child in &node.children {
            if let Some(db_name) =
                Self::find_database_name_for_table(child, connection_id, table_name)
            {
                return Some(db_name);
            }
        }

        None
    }

    #[allow(dead_code)]
    fn highlight_sql_syntax(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
        let mut job = egui::text::LayoutJob {
            text: text.to_owned(),
            ..Default::default()
        };

        // If text is empty, return empty job
        if text.is_empty() {
            return job;
        }

        // SQL keywords for highlighting
        let keywords = [
            "SELECT",
            "FROM",
            "WHERE",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "TABLE",
            "INDEX",
            "VIEW",
            "TRIGGER",
            "PROCEDURE",
            "FUNCTION",
            "JOIN",
            "INNER",
            "LEFT",
            "RIGHT",
            "OUTER",
            "ON",
            "AS",
            "AND",
            "OR",
            "NOT",
            "NULL",
            "TRUE",
            "FALSE",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "IF",
            "EXISTS",
            "IN",
            "LIKE",
            "BETWEEN",
            "GROUP BY",
            "ORDER BY",
            "HAVING",
            "LIMIT",
            "OFFSET",
            "UNION",
            "ALL",
            "DISTINCT",
            "COUNT",
            "SUM",
            "AVG",
            "MIN",
            "MAX",
            "ASC",
            "DESC",
            "PRIMARY",
            "KEY",
            "FOREIGN",
            "REFERENCES",
            "CONSTRAINT",
            "UNIQUE",
            "DEFAULT",
            "AUTO_INCREMENT",
            "SERIAL",
            "INT",
            "INTEGER",
            "VARCHAR",
            "TEXT",
            "CHAR",
            "DECIMAL",
            "FLOAT",
            "DOUBLE",
            "DATE",
            "TIME",
            "DATETIME",
            "TIMESTAMP",
            "BOOLEAN",
            "BOOL",
            "USE",
        ];

        // Define colors for different themes
        let text_color = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(220, 220, 220) // Light text for dark mode
        } else {
            egui::Color32::from_rgb(40, 40, 40) // Dark text for light mode
        };

        let keyword_color = egui::Color32::from_rgb(86, 156, 214); // Blue - SQL keywords
        let string_color = egui::Color32::from_rgb(255, 30, 0); // Orange - strings
        let comment_color = egui::Color32::from_rgb(106, 153, 85); // Green - comments
        let number_color = egui::Color32::from_rgb(181, 206, 168); // Light green - numbers
        let function_color = egui::Color32::from_rgb(255, 206, 84); // Yellow - functions
        let operator_color = egui::Color32::from_rgb(212, 212, 212); // Light gray - operators

        // Process line by line to handle comments properly
        let lines: Vec<&str> = text.lines().collect();
        let mut byte_offset = 0;

        for (line_idx, line) in lines.iter().enumerate() {
            let line_start_offset = byte_offset;

            // Check if this line is a comment
            if line.trim_start().starts_with("--") {
                // Entire line is a comment
                job.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: line_start_offset..line_start_offset + line.len(),
                    format: egui::TextFormat {
                        color: comment_color,
                        font_id: egui::FontId::monospace(14.0),
                        ..Default::default()
                    },
                });
            } else {
                // Process words in the line
                let words: Vec<&str> = line.split_whitespace().collect();
                let mut line_pos = line_start_offset;
                let mut word_search_start = 0;

                for word in words {
                    // Find the word position in the line
                    if let Some(word_start_in_line) = line[word_search_start..].find(word) {
                        let absolute_word_start =
                            line_start_offset + word_search_start + word_start_in_line;
                        let absolute_word_end = absolute_word_start + word.len();

                        // Add whitespace before word if any
                        if absolute_word_start > line_pos {
                            job.sections.push(egui::text::LayoutSection {
                                leading_space: 0.0,
                                byte_range: line_pos..absolute_word_start,
                                format: egui::TextFormat {
                                    color: text_color,
                                    font_id: egui::FontId::monospace(14.0),
                                    ..Default::default()
                                },
                            });
                        }

                        // Determine word color
                        let word_color = if word.starts_with('\'') || word.starts_with('"') {
                            // String
                            string_color
                        } else if word.chars().all(|c| c.is_ascii_digit() || c == '.')
                            && !word.is_empty()
                        {
                            // Number
                            number_color
                        } else if keywords.contains(&word.to_uppercase().as_str()) {
                            // SQL keyword
                            keyword_color
                        } else if word.contains('(') {
                            // Function call
                            function_color
                        } else if "(){}[]<>=!+-*/%,;".contains(word.chars().next().unwrap_or(' ')) {
                            // Operator
                            operator_color
                        } else {
                            // Default text
                            text_color
                        };

                        // Add the word with appropriate color
                        job.sections.push(egui::text::LayoutSection {
                            leading_space: 0.0,
                            byte_range: absolute_word_start..absolute_word_end,
                            format: egui::TextFormat {
                                color: word_color,
                                font_id: egui::FontId::monospace(14.0),
                                ..Default::default()
                            },
                        });

                        // Update positions
                        word_search_start = word_search_start + word_start_in_line + word.len();
                        line_pos = absolute_word_end;
                    }
                }

                // Add any remaining text in the line
                if line_pos < line_start_offset + line.len() {
                    job.sections.push(egui::text::LayoutSection {
                        leading_space: 0.0,
                        byte_range: line_pos..line_start_offset + line.len(),
                        format: egui::TextFormat {
                            color: text_color,
                            font_id: egui::FontId::monospace(14.0),
                            ..Default::default()
                        },
                    });
                }
            }

            // Add newline character if not the last line
            byte_offset += line.len();
            if line_idx < lines.len() - 1 {
                // Add the newline character
                job.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: byte_offset..byte_offset + 1,
                    format: egui::TextFormat {
                        color: text_color,
                        font_id: egui::FontId::monospace(14.0),
                        ..Default::default()
                    },
                });
                byte_offset += 1; // for the \n character
            }
        }

        job
    }

    fn handle_directory_picker(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.directory_picker_result = Some(receiver);

        // Spawn directory picker in a separate thread to avoid blocking UI
        let current_dir = self.data_directory.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Pilih Lokasi Data Directory")
                .set_directory(&current_dir)
                .pick_folder()
            {
                let _ = sender.send(path.to_string_lossy().to_string());
            }
        });
        self.show_directory_picker = false;
    }

    // Handle SQLite file/folder picker for new connection dialog
    #[allow(dead_code)]
    pub(crate) fn handle_sqlite_path_picker(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.sqlite_path_picker_result = Some(receiver);

        let default_dir = if !self.data_directory.is_empty() {
            self.data_directory.clone()
        } else {
            crate::config::get_data_dir().to_string_lossy().to_string()
        };

        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Pilih File / Folder SQLite")
                .set_directory(&default_dir)
                .pick_folder()
            {
                let _ = sender.send(path.to_string_lossy().to_string());
            }
        });
    }

    // Handle save directory picker dialog
    pub(crate) fn handle_save_directory_picker(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.save_directory_picker_result = Some(receiver);

        // Spawn directory picker in a separate thread to avoid blocking UI
        let default_dir = if !self.save_directory.is_empty() {
            self.save_directory.clone()
        } else {
            crate::directory::get_query_dir()
                .to_string_lossy()
                .to_string()
        };

        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Pilih Lokasi Penyimpanan Query")
                .set_directory(&default_dir)
                .pick_folder()
            {
                let _ = sender.send(path.to_string_lossy().to_string());
            }
        });
    }

    // Refresh data directory from current environment/config
    fn refresh_data_directory(&mut self) {
        self.data_directory = crate::config::get_data_dir().to_string_lossy().to_string();
    }

    // Self-update functionality
    pub fn check_for_updates(&mut self, manual: bool) {
        if self.update_check_in_progress {
            return; // Already checking
        }

        self.update_check_in_progress = true;
        self.update_check_error = None;
        self.last_update_check = Some(std::time::Instant::now());
        self.manual_update_check = manual;

        // Persist last check time to avoid multiple checks within 24 hours
        if let (Some(store), Some(rt)) = (self.config_store.as_ref(), self.runtime.as_ref()) {
            rt.block_on(store.set_last_update_check_now());
        }

        // Send background task to check for updates
        if let Some(sender) = &self.background_sender {
            let _ = sender.send(models::enums::BackgroundTask::CheckForUpdates);
        }
    }

    fn render_update_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_update_dialog {
            return;
        }

        egui::Window::new("Software Update")
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(400.0);

                if self.update_check_in_progress {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking for updates...");
                    });
                } else if let Some(error) = &self.update_check_error {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 30, 0),
                        format!("Error: {}", error),
                    );
                    ui.separator();
                    if ui.button("Close").clicked() {
                        self.show_update_dialog = false;
                    }
                } else if let Some(update_info) = &self.update_info.clone() {
                    if update_info.update_available {
                        ui.heading("Update Available!");
                        ui.separator();

                        ui.horizontal(|ui| {
                            ui.label("Current version:");
                            ui.strong(&update_info.current_version);
                        });

                        ui.horizontal(|ui| {
                            ui.label("Latest version:");
                            ui.strong(&update_info.latest_version);
                        });

                        if let Some(published_at) = &update_info.published_at {
                            ui.label(format!("Released: {}", published_at));
                        }

                        ui.separator();

                        ui.label("Release Notes:");
                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                ui.text_edit_multiline(&mut update_info.release_notes.clone());
                            });

                        ui.separator();

                        ui.horizontal(|ui| {
                            if update_info.download_url.is_some() {
                                if self.update_download_in_progress {
                                    ui.add_enabled(false, egui::Button::new("Downloading..."));
                                    ui.spinner();
                                } else if ui.button("Update Now").clicked() {
                                    self.start_update_download();
                                }
                            } else {
                                // No download URL available - show manual download option
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    "Auto-update not available for this platform",
                                );
                            }

                            if ui.button("View Release").clicked() {
                                crate::self_update::open_release_page(update_info);
                            }

                            if ui.button("Later").clicked() {
                                self.show_update_dialog = false;
                            }
                        });
                    } else {
                        ui.heading("You're up to date!");
                        ui.separator();
                        ui.label(format!(
                            "Tabular {} is the latest version.",
                            update_info.current_version
                        ));
                        ui.separator();
                        if ui.button("Close").clicked() {
                            self.show_update_dialog = false;
                        }
                    }
                } else {
                    ui.label("No update information available.");
                    if ui.button("Close").clicked() {
                        self.show_update_dialog = false;
                    }
                }
            });
    }

    fn start_update_download(&mut self) {
        log::info!("🚀 Starting auto update process...");

        // Prevent multiple simultaneous downloads
        if self.update_download_in_progress {
            log::warn!("⚠️ Download already in progress, ignoring request");
            return;
        }

        // Prevent re-downloading if already completed
        if self.update_installed {
            log::warn!("⚠️ Update already downloaded, ignoring request");
            return;
        }

        if let Some(update_info) = &self.update_info {
            if let Some(auto_updater) = &self.auto_updater {
                log::info!(
                    "📦 Update info available: {} -> {}",
                    update_info.current_version,
                    update_info.latest_version
                );
                log::info!("📥 Download URL: {:?}", update_info.download_url);
                log::info!("📄 Asset name: {:?}", update_info.asset_name);

                self.update_download_in_progress = true;
                // Prepare channel to receive completion signal
                let (tx, rx) = std::sync::mpsc::channel();
                self.update_install_receiver = Some(rx);

                let update_info_clone = update_info.clone();
                let auto_updater_clone = auto_updater.clone();

                std::thread::spawn(move || {
                    log::info!("🔄 Background update thread started (auto updater)");

                    // Create a completely new, independent Tokio runtime for the update process
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            log::error!("❌ Failed to create update runtime: {}", e);
                            let _ = tx.send(false);
                            return;
                        }
                    };

                    match rt
                        .block_on(auto_updater_clone.download_and_stage_update(&update_info_clone))
                    {
                        Ok(()) => {
                            log::info!("✅ Update staged successfully");
                            let _ = tx.send(true);
                        }
                        Err(e) => {
                            log::error!("❌ Update failed: {}", e);
                            let _ = tx.send(false);
                        }
                    }
                });
            } else {
                log::error!("❌ Auto updater not available");
                self.update_download_in_progress = false;
            }
        } else {
            log::error!("❌ No update info available");
        }
    }
}


impl App for Tabular {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        
        // If Cmd+A was pressed, set a short-lived flag or state?
        // Actually, we need to know if "Select All" happened recently.
        // Let's store a timestamp or frame counter? 
        // Simpler: Just store the bool for this frame.
        // But the user sequence is Cmd+A (frame X), Release keys, Backspace (frame Y).
        // So checking "is Cmd+A pressed NOW" won't work for backspace.
        
        // Wait, if the user holds Cmd+A and presses Backspace, that's one thing.
        // But usually they press Cmd+A, release, then Backspace.
        // The TextEdit "selection" state persists.
        // So we really need to know "Is the whole text selected?".
        
        // Since we can't easily query that from outside without `TextEdit::load_state`,
        // let's try to load state in the dialog render function instead.
        // So here we just track backspace.
        
        // Simple state machine: if Cmd+A pressed, remember it for a short time?
        // Actually, TextEdit handles selection internally.
        // If we want to support "Select All -> Delete", we need to know if everything is selected.
        // But we can't easily.
        
        // Alternative Heuristic:
        // If Backspace is pressed, checking if modifiers.command is also held? No, that deletes word usually.
        // The user sequence is: Press Cmd+A (release). Press Backspace.
        
        // Let's rely on `TextEditState`.
        // We can get `TextEditState` from memory using the ID.
        // `if let Some(state) = egui::TextEdit::load_state(ctx, query_id)`
        // `state.cursor.range()` tells us the selection!
        // Keyboard shortcut to toggle Query AST debug panel (Phase F)
        #[cfg(feature = "query_ast")]
        if ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            self.show_query_ast_debug = !self.show_query_ast_debug;
        }
        // Periodic cleanup of stuck connection pools to prevent infinite loops
        if self.pending_connection_pools.len() > 10 {
            // If we have too many pending connections, force cleanup
            log::debug!(
                "🧹 Force cleaning up {} pending connections",
                self.pending_connection_pools.len()
            );
            self.pending_connection_pools.clear();
        }

        // helper closure to save immediately when prefs_dirty flagged
        let try_save_prefs = |app: &mut Tabular| {
            if app.prefs_dirty {
                if let (Some(store), Some(rt)) = (app.config_store.as_ref(), app.runtime.as_ref()) {
                    let prefs = crate::config::AppPreferences {
                        is_dark_mode: app.is_dark_mode,
                        link_editor_theme: app.link_editor_theme,
                        editor_theme: match app.advanced_editor.theme {
                            crate::models::structs::EditorColorTheme::GithubLight => {
                                "GITHUB_LIGHT".into()
                            }
                            crate::models::structs::EditorColorTheme::Gruvbox => "GRUVBOX".into(),
                            _ => "GITHUB_DARK".into(),
                        },
                        font_size: app.advanced_editor.font_size,
                        word_wrap: app.advanced_editor.word_wrap,
                        data_directory: if app.data_directory
                            != crate::config::get_data_dir().to_string_lossy()
                        {
                            Some(app.data_directory.clone())
                        } else {
                            None
                        },
                        auto_check_updates: app.auto_check_updates,
                        use_server_pagination: app.use_server_pagination,
                        last_update_check_iso: app
                            .last_saved_prefs
                            .as_ref()
                            .and_then(|p| p.last_update_check_iso.clone()),
                        enable_debug_logging: app.enable_debug_logging,
                    };
                    rt.block_on(store.save(&prefs));
                    log::info!(
                        "Preferences saved successfully to: {}",
                        crate::config::get_data_dir().display()
                    );
                    app.last_saved_prefs = Some(prefs);
                    app.prefs_dirty = false;
                } else {
                    log::error!("Cannot save preferences: config store or runtime not initialized");
                }
            }
        };
        // Handle forced refresh flag
        if self.needs_refresh {
            self.needs_refresh = false;

            // Force refresh of query tree
            sidebar_query::load_queries_from_directory(self);

            // Request UI repaint
            ctx.request_repaint();
        }

        // Handle pending Auto Refresh request coming from History context menu
        ctx.data_mut(|data| {
            if let Some(conn_id) = data.get_persisted::<i64>(egui::Id::new("auto_refresh_request_conn_id"))
                && let Some(query) = data.get_persisted::<String>(egui::Id::new("auto_refresh_request_query"))
            {
                // Initialize auto-refresh parameters but wait for user to confirm interval
                self.auto_refresh_connection_id = Some(conn_id);
                self.auto_refresh_query = Some(query);
                // Show global auto-refresh dialog for interval input
                self.auto_refresh_active = false;
                self.auto_refresh_last_run = None;
                self.show_auto_refresh_dialog = true;
                self.auto_refresh_interval_input = self.auto_refresh_interval_seconds.to_string();
                // Clear request markers to avoid repeated dialogs
                data.remove::<i64>(egui::Id::new("auto_refresh_request_conn_id"));
                data.remove::<String>(egui::Id::new("auto_refresh_request_query"));
            }
        });

        // Render Auto Refresh interval popup dialog if requested
        if self.show_auto_refresh_dialog {
            egui::Window::new("Auto Refresh Interval")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Set auto refresh interval (seconds):");
                    ui.text_edit_singleline(&mut self.auto_refresh_interval_input);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            if let Ok(v) = self.auto_refresh_interval_input.trim().parse::<u32>() {
                                let v = std::cmp::max(1, v); // minimum 1 second
                                self.auto_refresh_interval_seconds = v;
                                self.auto_refresh_active = true;
                                self.auto_refresh_last_run = None;
                                self.show_auto_refresh_dialog = false;
                            } else {
                                // Invalid input keeps dialog open; user can correct it
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_auto_refresh_dialog = false;
                            self.stop_auto_refresh();
                        }
                    });
                });
        }

        // Auto Refresh execution loop: run query when interval elapsed
        if self.auto_refresh_active {
            // Ensure UI updates regularly so countdown label stays smooth
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
            if let (Some(query), Some(conn_id)) = (
                self.auto_refresh_query.clone(),
                self.auto_refresh_connection_id,
            ) {
                // Do not start new run while previous execution still in progress
                if !self.query_execution_in_progress {
                    let now = std::time::Instant::now();
                    let should_run = match self.auto_refresh_last_run {
                        None => true,
                        Some(last) => {
                            let interval = std::time::Duration::from_secs(
                                self.auto_refresh_interval_seconds.max(1) as u64,
                            );
                            now.duration_since(last) >= interval
                        }
                    };

                    if should_run {
                        debug!(
                            "[auto-refresh] firing run: conn_id={:?}, len(query)={}, active_tab_index={}",
                            conn_id,
                            query.len(),
                            self.active_tab_index
                        );
                        // Ensure active tab has the right connection
                        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            active_tab.connection_id = Some(conn_id);
                            active_tab.has_executed_query = true;
                            active_tab.base_query = query.clone();
                        }
                        self.current_connection_id = Some(conn_id);
                        self.is_table_browse_mode = false;
                        // Put query into editor
                        self.editor.set_text(query.clone());
                        self.editor.mark_text_modified();
                        // Execute using existing flow (button Execute behavior)
                        self.execute_paginated_query();
                        self.auto_refresh_last_run = Some(now);
                    }
                }
            } else {
                // Missing data: stop auto refresh to avoid looping
                self.stop_auto_refresh();
            }
        }

        // Lazy load preferences once (before applying visuals)
        if self.config_store.is_none()
            && !self.prefs_loaded
            && let Some(rt) = &self.runtime
        {
            match rt.block_on(crate::config::ConfigStore::new()) {
                Ok(store) => {
                    let prefs = rt.block_on(store.load());
                    self.is_dark_mode = prefs.is_dark_mode;
                    self.link_editor_theme = prefs.link_editor_theme;
                    self.advanced_editor.theme = match prefs.editor_theme.as_str() {
                        "GITHUB_LIGHT" => crate::models::structs::EditorColorTheme::GithubLight,
                        "GRUVBOX" => crate::models::structs::EditorColorTheme::Gruvbox,
                        _ => crate::models::structs::EditorColorTheme::GithubDark,
                    };
                    self.advanced_editor.font_size = prefs.font_size;
                    self.advanced_editor.word_wrap = prefs.word_wrap;
                    // Load custom data directory if set
                    if let Some(custom_dir) = &prefs.data_directory {
                        self.data_directory = custom_dir.clone();
                        // Apply the custom directory
                        if let Err(e) = crate::config::set_data_dir(custom_dir) {
                            log::error!(
                                "Failed to set custom data directory '{}': {}",
                                custom_dir,
                                e
                            );
                            // Fallback to default
                            self.data_directory =
                                crate::config::get_data_dir().to_string_lossy().to_string();
                        }
                    }

                    // Load auto-update preference
                    self.auto_check_updates = prefs.auto_check_updates;

                    // Load server pagination preference
                    self.use_server_pagination = prefs.use_server_pagination;

                    self.config_store = Some(store);
                    self.last_saved_prefs = Some(prefs.clone());
                    self.prefs_loaded = true;
                    log::info!("Preferences loaded successfully on startup");

                    // Check for updates on startup if enabled, but only once per day
                    if prefs.auto_check_updates {
                        let mut should_check = true;
                        if let Some(store_ref) = self.config_store.as_ref()
                            && let Some(last_iso) = rt.block_on(store_ref.get_last_update_check())
                            && let Ok(parsed) = DateTime::parse_from_rfc3339(&last_iso)
                        {
                            let last_utc = parsed.with_timezone(&Utc);
                            let now = Utc::now();
                            if now.signed_duration_since(last_utc) < Duration::days(1) {
                                should_check = false;
                                debug!(
                                    "⏱️ Skipping auto update check; last check at {} (< 24h)",
                                    last_iso
                                );
                            }
                        }
                        if should_check
                            && let (Some(sender), Some(store_ref)) =
                                (&self.background_sender, self.config_store.as_ref())
                        {
                            // Persist timestamp immediately to prevent repeated checks this session
                            rt.block_on(store_ref.set_last_update_check_now());
                            let _ = sender.send(models::enums::BackgroundTask::CheckForUpdates);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to initialize config store: {}", e);
                    self.prefs_loaded = true; // Don't retry every frame
                }
            }
        }

        // Apply global UI visuals based on (possibly loaded) theme
        if self.is_dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        // If waiting for pool, check readiness and auto-run queued query
        if self.pool_wait_in_progress {
            let mut ready = false;
            if let Some(conn_id) = self.pool_wait_connection_id {
                if self.connection_pools.contains_key(&conn_id) {
                    ready = true;
                } else if let Ok(shared) = self.shared_connection_pools.lock()
                    && shared.contains_key(&conn_id)
                {
                    // Move to local cache for speed
                    if let Some(pool) = shared.get(&conn_id).cloned() {
                        self.connection_pools.insert(conn_id, pool);
                    }
                    ready = true;
                }
            }

            if ready {
                if let Some(conn_id) = self.pool_wait_connection_id {
                    let queued = self.pool_wait_query.clone();
                    
                    // Execute asynchronously to avoid freezing if connection is still slow
                    let job_id = self.next_query_job_id;
                    self.next_query_job_id += 1;
                    
                    match crate::connection::prepare_query_job(self, conn_id, queued.clone(), job_id) {
                        Ok(job) => {
                            match crate::connection::spawn_query_job(self, job.clone(), self.query_result_sender.clone()) {
                                Ok(handle) => {
                                    self.active_query_jobs.insert(job_id, crate::connection::QueryJobStatus {
                                        job_id,
                                        connection_id: conn_id,
                                        query_preview: queued.chars().take(50).collect(),
                                        started_at: std::time::Instant::now(),
                                        completed: false,
                                    });
                                    self.active_query_handles.insert(job_id, handle);
                                    log::debug!("🚀 Asynchronously queued pool-wait query (Job {})", job_id);
                                }
                                Err(e) => {
                                    log::error!("Failed to spawn queued query: {:?}", e);
                                    self.error_message = format!("Failed to spawn queued query: {:?}", e);
                                    self.show_error_message = true;
                                }
                            }
                        }
                        Err(e) => {
                             log::error!("Failed to prepare queued query: {:?}", e);
                             self.error_message = format!("Failed to prepare queued query: {:?}", e);
                             self.show_error_message = true;
                        }
                    }

                }
                // Clear wait state
                self.pool_wait_in_progress = false;
                self.pool_wait_connection_id = None;
                self.pool_wait_query.clear();
                self.pool_wait_started_at = None;
            } else {
                // Keep UI updated while waiting
                ctx.request_repaint();
            }
        }
        // Sync editor theme only if linking enabled
        if self.link_editor_theme {
            let desired_editor_theme = if self.is_dark_mode {
                crate::models::structs::EditorColorTheme::GithubDark
            } else {
                crate::models::structs::EditorColorTheme::GithubLight
            };
            if self.advanced_editor.theme != desired_editor_theme {
                self.advanced_editor.theme = desired_editor_theme;
            }
        }

        // Periodic cleanup of stale connection pools (every 10 minutes to reduce overhead)
        if self.last_cleanup_time.elapsed().as_secs() > 600 {
            // 10 minutes instead of 5
            debug!("🧹 Performing periodic connection pool cleanup");

            // Clean up connections that might be stale
            let mut connections_to_refresh: Vec<i64> =
                self.connection_pools.keys().copied().collect();

            // Limit cleanup to avoid blocking UI
            if connections_to_refresh.len() > 5 {
                connections_to_refresh.truncate(5);
            }

            for connection_id in connections_to_refresh {
                connection::cleanup_connection_pool(self, connection_id);
            }

            self.last_cleanup_time = std::time::Instant::now();
        }

        // Handle deferred theme selector request
        if self.request_theme_selector {
            self.request_theme_selector = false;
            self.show_theme_selector = true;
        }

        // --- Query AST Debug floating window (Phase F) ---
        #[cfg(feature = "query_ast")]
        if self.show_query_ast_debug {
            egui::Window::new("Query AST Debug")
                .open(&mut self.show_query_ast_debug)
                .resizable(true)
                .default_size(egui::vec2(520.0, 320.0))
                .show(ctx, |ui| {
                    // Attempt to capture latest plan hash/cache key from thread-local store (pop once per frame)
                    if let Some((h, key, ctes)) = crate::query_ast::take_last_debug() {
                        self.last_plan_hash = Some(h);
                        self.last_plan_cache_key = Some(key);
                        self.last_ctes = ctes;
                    }
                    ui.label("Press F9 to toggle this panel.");
                    if ui.button("Refresh Stats").clicked() {
                        let (h, m) = crate::query_ast::cache_stats();
                        self.last_cache_hits = h;
                        self.last_cache_misses = m;
                        if let Some(sql) = &self.last_compiled_sql
                            && let Some(active_tab) = self.query_tabs.get(self.active_tab_index)
                            && let Some(conn_id) = active_tab.connection_id
                            && let Some(conn) =
                                self.connections.iter().find(|c| c.id == Some(conn_id))
                        {
                            if let Ok(plan_txt) =
                                crate::query_ast::debug_plan(sql, &conn.connection_type)
                            {
                                self.last_debug_plan = Some(plan_txt);
                            }
                            if let Ok((nodes, depth, subs_total, subs_corr, wins)) =
                                crate::query_ast::plan_metrics(sql)
                            {
                                ui.label(format!(
                                    "Plan: nodes={} depth={} subqueries={} (corr={}) windows={}",
                                    nodes, depth, subs_total, subs_corr, wins
                                ));
                            }
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "Cache: hits={} misses={} hit_rate={:.1}%",
                            self.last_cache_hits,
                            self.last_cache_misses,
                            if self.last_cache_hits + self.last_cache_misses > 0 {
                                (self.last_cache_hits as f64 * 100.0)
                                    / (self.last_cache_hits + self.last_cache_misses) as f64
                            } else {
                                0.0
                            }
                        ));
                    });
                    let rules = crate::query_ast::last_rewrite_rules();
                    if !rules.is_empty() {
                        ui.collapsing("Rewrite Rules Applied", |ui| {
                            ui.label(rules.join(", "));
                        });
                    }
                    if let Some(h) = self.last_plan_hash {
                        ui.label(format!("Plan Hash: {:x}", h));
                    }
                    if let Some(k) = &self.last_plan_cache_key {
                        ui.collapsing("Cache Key", |ui| {
                            ui.code(k);
                        });
                    }
                    if let Some(ctes) = &self.last_ctes
                        && !ctes.is_empty()
                    {
                        ui.collapsing("Remaining CTEs", |ui| {
                            ui.label(ctes.join(", "));
                        });
                    }
                    if let Some(sql) = &self.last_compiled_sql {
                        ui.collapsing("Last Emitted SQL", |ui| {
                            ui.code(sql);
                        });
                    }
                    if !self.last_compiled_headers.is_empty() {
                        ui.collapsing("Last Inferred Headers", |ui| {
                            ui.label(self.last_compiled_headers.join(", "));
                        });
                    }
                    if let Some(plan) = &self.last_debug_plan {
                        ui.collapsing("Logical Plan", |ui| {
                            ui.code(plan);
                        });
                    }
                    if self.last_compiled_sql.is_none() {
                        ui.label("(Run a SELECT query to populate data)");
                    }
                });
        }

        // Detect Copy shortcut ONLY for table/structure - rely on table_recently_clicked flag
        // which is set when user clicks table cell and reset when clicking editor.
        // This avoids timing issues with egui focus state which updates AFTER render.
        let mut copy_shortcut_detected = false;
        
        ctx.input(|i| {
            // Check for Copy event OR CMD+C key combo
            let copy_event = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let key_c_pressed = i.key_pressed(egui::Key::C);
            let cmd_held = i.modifiers.mac_cmd || i.modifiers.ctrl;
            
            if copy_event || (cmd_held && key_c_pressed) {
                // Only handle copy for table/structure based on recent click flag
                // If table_recently_clicked=false, user is in editor, so let editor handle copy
                if self.table_recently_clicked {
                    copy_shortcut_detected = true;
                    debug!("📋 Copy shortcut detected for table! copy_event={}, cmd_held={}, key_c={}", 
                        copy_event, cmd_held, key_c_pressed);
                } else {
                    debug!("📋 Copy event but not handling - table_recently_clicked=false (user in editor/elsewhere)");
                }
            }
        });

        // Detect Save shortcut using consume_key so it works reliably on macOS/Windows/Linux
        let mut save_shortcut = false;
        
        // Check if current tab is a diagram tab. If so, let diagram handle save.
        let is_diagram_active = if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
            tab.diagram_state.is_some()
        } else {
            false
        };

        if !is_diagram_active {
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
                {
                    save_shortcut = true;
                    println!("🔥 Save shortcut detected!");
                }
            });
        }

        // Handle keyboard shortcuts
        ctx.input(|i| {
            // CMD+W or CTRL+W to close current tab
            if (i.modifiers.mac_cmd || i.modifiers.ctrl)
                && i.key_pressed(egui::Key::W)
                && !self.query_tabs.is_empty()
            {
                editor::close_tab(self, self.active_tab_index);
            }

            // CMD+Q or CTRL+Q to quit application
            if (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::Q) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }

            // CMD+SHIFT+P to open command palette (on macOS)
            if i.modifiers.mac_cmd && i.modifiers.shift && i.key_pressed(egui::Key::P) {
                editor::open_command_palette(self);
            }

            // CMD/CTRL+R to refresh current view
            if (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::R) {
                match self.table_bottom_view {
                    models::structs::TableBottomView::Structure => {
                        self.request_structure_refresh = true;
                        data_table::load_structure_info_for_current_table(self);
                    }
                    _ => {
                        data_table::refresh_current_table_data(self);
                    }
                }
            }

            // Handle table cell navigation with arrow keys
            // Only allow table navigation when table was recently clicked
            if !self.show_command_palette
                && !self.show_theme_selector
                && self.selected_cell.is_some()
                && self.table_recently_clicked
            {
                let mut cell_changed = false;
                let mut consumed_arrow = false; // track if we handled an arrow key so we can suppress editor reaction
                if let Some((row, col)) = self.selected_cell {
                    let max_rows = self.current_table_data.len();
                    let shift = i.modifiers.shift;
                    if shift && self.table_sel_anchor.is_none() {
                        self.table_sel_anchor = Some((row, col));
                    }

                    if i.key_pressed(egui::Key::ArrowRight) {
                        // Check the current row's column count for bounds
                        if let Some(current_row) = self.current_table_data.get(row)
                            && col + 1 < current_row.len()
                        {
                            self.selected_cell = Some((row, col + 1));
                            cell_changed = true;
                            self.scroll_to_selected_cell = true;
                            log::debug!("➡️ Arrow Right: Moving to ({}, {})", row, col + 1);
                            consumed_arrow = true;
                        }
                    } else if i.key_pressed(egui::Key::ArrowLeft) && col > 0 {
                        self.selected_cell = Some((row, col - 1));
                        cell_changed = true;
                        self.scroll_to_selected_cell = true;
                        log::debug!("⬅️ Arrow Left: Moving to ({}, {})", row, col - 1);
                        consumed_arrow = true;
                    } else if i.key_pressed(egui::Key::ArrowDown) && row + 1 < max_rows {
                        // Check if the target row has enough columns
                        if let Some(target_row) = self.current_table_data.get(row + 1) {
                            let target_col = col.min(target_row.len().saturating_sub(1));
                            self.selected_cell = Some((row + 1, target_col));
                            cell_changed = true;
                            self.scroll_to_selected_cell = true;
                            log::debug!("⬇️ Arrow Down: Moving to ({}, {})", row + 1, target_col);
                            consumed_arrow = true;
                        }
                    } else if i.key_pressed(egui::Key::ArrowUp) && row > 0 {
                        // Check if the target row has enough columns
                        if let Some(target_row) = self.current_table_data.get(row - 1) {
                            let target_col = col.min(target_row.len().saturating_sub(1));
                            self.selected_cell = Some((row - 1, target_col));
                            cell_changed = true;
                            self.scroll_to_selected_cell = true;
                            log::debug!("⬆️ Arrow Up: Moving to ({}, {})", row - 1, target_col);
                            consumed_arrow = true;
                        }
                    }

                    // Update selected_row when cell changes
                    if cell_changed && let Some((new_row, _)) = self.selected_cell {
                        self.selected_row = Some(new_row);
                        if !shift {
                            self.table_sel_anchor = None;
                        }
                    }
                }
                if consumed_arrow {
                    self.suppress_editor_arrow_once = true;
                }
            }

            // Handle Structure (Columns/Indexes) cell navigation with arrow keys
            if !self.show_command_palette
                && !self.show_theme_selector
                && self.table_bottom_view == models::structs::TableBottomView::Structure
                && self.structure_selected_cell.is_some()
            {
                let mut cell_changed = false;
                let mut consumed_arrow = false;
                if let Some((row, col)) = self.structure_selected_cell {
                    let shift = i.modifiers.shift;
                    // Determine grid dimensions for current Structure subview
                    let (max_rows, max_cols) = match self.structure_sub_view {
                        models::structs::StructureSubView::Columns => {
                            let cols = if self.structure_col_widths.is_empty() {
                                6
                            } else {
                                self.structure_col_widths.len()
                            };
                            (self.structure_columns.len(), cols)
                        }
                        models::structs::StructureSubView::Indexes => {
                            let cols = if self.structure_idx_col_widths.is_empty() {
                                6
                            } else {
                                self.structure_idx_col_widths.len()
                            };
                            (self.structure_indexes.len(), cols)
                        }
                    };
                    // If extending selection with Shift, latch anchor at the starting cell
                    if shift && self.structure_sel_anchor.is_none() {
                        self.structure_sel_anchor = Some((row, col));
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        if col + 1 < max_cols {
                            self.structure_selected_cell = Some((row, col + 1));
                            cell_changed = true;
                            consumed_arrow = true;
                            log::debug!(
                                "➡️ Arrow Right (Structure): Moving to ({}, {})",
                                row,
                                col + 1
                            );
                        }
                    } else if i.key_pressed(egui::Key::ArrowLeft) {
                        if col > 0 {
                            self.structure_selected_cell = Some((row, col - 1));
                            cell_changed = true;
                            consumed_arrow = true;
                            log::debug!(
                                "⬅️ Arrow Left (Structure): Moving to ({}, {})",
                                row,
                                col - 1
                            );
                        }
                    } else if i.key_pressed(egui::Key::ArrowDown) {
                        if row + 1 < max_rows {
                            let target_col = col.min(max_cols.saturating_sub(1));
                            self.structure_selected_cell = Some((row + 1, target_col));
                            cell_changed = true;
                            consumed_arrow = true;
                            log::debug!(
                                "⬇️ Arrow Down (Structure): Moving to ({}, {})",
                                row + 1,
                                target_col
                            );
                        }
                    } else if i.key_pressed(egui::Key::ArrowUp) && row > 0 {
                        let target_col = col.min(max_cols.saturating_sub(1));
                        self.structure_selected_cell = Some((row - 1, target_col));
                        cell_changed = true;
                        consumed_arrow = true;
                        log::debug!(
                            "⬆️ Arrow Up (Structure): Moving to ({}, {})",
                            row - 1,
                            target_col
                        );
                    }

                    if cell_changed {
                        // On non-Shift navigation, collapse selection (clear anchor)
                        if !shift {
                            self.structure_sel_anchor = None;
                        }
                        if let Some((r, _)) = self.structure_selected_cell {
                            self.structure_selected_row = Some(r);
                        }
                    }
                }
                if consumed_arrow {
                    self.suppress_editor_arrow_once = true;
                }
            }

            // Handle command palette navigation
            if self.show_command_palette {
                // Arrow key navigation
                if i.key_pressed(egui::Key::ArrowDown) {
                    editor::navigate_command_palette(self, 1);
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    editor::navigate_command_palette(self, -1);
                }
                // Enter to execute selected command (only when command palette is visible)
                else if i.key_pressed(egui::Key::Enter) && self.show_command_palette {
                    log::debug!("🔥 GLOBAL DEBUG: Command palette Enter consumed");
                    editor::execute_selected_command(self);
                }
            }

            // Handle theme selector navigation
            if self.show_theme_selector {
                // Arrow key navigation
                if i.key_pressed(egui::Key::ArrowDown) {
                    editor::navigate_theme_selector(self, 1);
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    editor::navigate_theme_selector(self, -1);
                }
                // Enter to select theme (only when theme selector is visible)
                else if i.key_pressed(egui::Key::Enter) && self.show_theme_selector {
                    editor::select_current_theme(self);
                }
            }

            // Escape to close overlays, cancel edits, or discard unsaved spreadsheet changes
            if i.key_pressed(egui::Key::Escape) {
                if self.show_settings_window {
                    self.show_settings_window = false;
                } else if self.show_theme_selector {
                    self.show_theme_selector = false;
                } else if self.show_command_palette {
                    self.show_command_palette = false;
                    self.command_palette_input.clear();
                    self.command_palette_selected_index = 0;
                } else if self.spreadsheet_state.editing_cell.is_some() {
                    // If currently editing a cell, cancel the in-progress edit only
                    self.spreadsheet_finish_cell_edit(false);
                } else if !self.spreadsheet_state.pending_operations.is_empty()
                    || self.spreadsheet_state.is_dirty
                {
                    // Discard all pending spreadsheet changes and refresh data
                    debug!(
                        "⎋ ESC: Discarding {} pending ops (is_dirty={})",
                        self.spreadsheet_state.pending_operations.len(),
                        self.spreadsheet_state.is_dirty
                    );
                    self.reset_spreadsheet_state();

                    // Reload table view to revert any in-memory edits
                    if self.is_table_browse_mode {
                        // Ensure we stay in table browse mode so double-click editing works
                        self.is_table_browse_mode = true;
                        if self.use_server_pagination && !self.current_base_query.is_empty() {
                            self.execute_paginated_query();
                        } else {
                            data_table::refresh_current_table_data(self);
                        }
                    }
                } else {
                    // Clear selections in table
                    self.selected_rows.clear();
                    self.selected_columns.clear();
                    self.selected_row = None;
                    self.selected_cell = None;
                    self.table_sel_anchor = None;
                    self.table_dragging = false;
                    self.last_clicked_row = None;
                    self.last_clicked_column = None;
                }
            }
        });

        // Execute Save action if shortcut was pressed
        if save_shortcut {
            println!(
                "🔥 Save shortcut execution block reached! pending_operations: {}, is_dirty: {}",
                self.spreadsheet_state.pending_operations.len(),
                self.spreadsheet_state.is_dirty
            );
            debug!(
                "🔥 Save shortcut pressed! pending_operations: {}, is_dirty: {}",
                self.spreadsheet_state.pending_operations.len(),
                self.spreadsheet_state.is_dirty
            );

            // If a cell is being edited, commit it first so its change is included in save
            if self.spreadsheet_state.editing_cell.is_some() {
                println!("🔥 Committing active cell edit");
                debug!("🔥 Committing active cell edit");
                self.spreadsheet_finish_cell_edit(true);
            }
            // Prefer saving pending spreadsheet changes if any are queued
            if !self.spreadsheet_state.pending_operations.is_empty() {
                println!(
                    "🔥 Calling spreadsheet_save_changes with {} operations",
                    self.spreadsheet_state.pending_operations.len()
                );
                debug!(
                    "🔥 Calling spreadsheet_save_changes with {} operations",
                    self.spreadsheet_state.pending_operations.len()
                );
                self.spreadsheet_save_changes();
            } else if !self.query_tabs.is_empty() {
                println!("🔥 No spreadsheet operations, saving query tab instead");
                debug!("🔥 No spreadsheet operations, saving query tab instead");
                if let Err(error) = editor::save_current_tab(self) {
                    self.error_message = format!("Save failed: {}", error);
                    self.show_error_message = true;
                }
            } else {
                println!("🔥 Nothing to save - no operations and no query tabs");
            }
        }

        // Render command palette if open
        if self.show_command_palette {
            editor::render_command_palette(self, ctx);
        }

        // Render theme selector if open
        if self.show_theme_selector {
            editor::render_theme_selector(self, ctx);
        }

        // Render settings window if open
        if self.show_settings_window {
            let mut open_flag = true; // local to satisfy borrow rules
            egui::Window::new("Preferences")
                .open(&mut open_flag)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_width(420.0)
                .show(ctx, |ui| {
                    // Tab bar
                    ui.horizontal(|ui| {
                        // Accent color (red) can adapt for light/dark if needed
                        let accent = if self.is_dark_mode { egui::Color32::from_rgb(255, 30, 0) } else { egui::Color32::from_rgb(180,30,30) };
                        let inactive_fg = ui.visuals().text_color();
                        let draw_tab = |ui: &mut egui::Ui, current: &mut PrefTab, me: PrefTab, label: &str| {
                            let selected = *current == me;
                            let (bg, fg) = if selected { (accent, egui::Color32::WHITE) } else { (egui::Color32::TRANSPARENT, inactive_fg) };
                            let button = egui::Button::new(egui::RichText::new(label).color(fg).size(13.0))
                                .fill(bg)
                                .stroke(if selected { egui::Stroke { width: 1.0, color: accent } } else { egui::Stroke { width: 1.0, color: ui.visuals().widgets.inactive.bg_stroke.color } })
                                .min_size(egui::vec2(0.0, 24.0));
                            // Attempt to use new corner radius API if available (ignore if not)
                            // Rounding disabled for compatibility with current egui version
                            let resp = ui.add(button);
                            if resp.clicked() { *current = me; }
                        };
                        draw_tab(ui, &mut self.settings_active_pref_tab, PrefTab::ApplicationTheme, "Application Theme");
                        draw_tab(ui, &mut self.settings_active_pref_tab, PrefTab::EditorTheme, "Editor Theme");
                        draw_tab(ui, &mut self.settings_active_pref_tab, PrefTab::Performance, "Performance Settings");
                        draw_tab(ui, &mut self.settings_active_pref_tab, PrefTab::DataDirectory, "Data Directory");
                        draw_tab(ui, &mut self.settings_active_pref_tab, PrefTab::Update, "Update");
                    });
                    ui.separator();
                    ui.add_space(4.0);

                    match self.settings_active_pref_tab {
                        PrefTab::ApplicationTheme => {
                            ui.heading("Application Theme");
                            ui.horizontal(|ui| {
                                ui.label("Choose theme:");
                                let prev = self.is_dark_mode;
                                if ui.radio_value(&mut self.is_dark_mode, true, "🌙 Dark").clicked() {
                                    ctx.set_visuals(egui::Visuals::dark());
                                    if self.link_editor_theme { self.advanced_editor.theme = crate::models::structs::EditorColorTheme::GithubDark; }
                                    self.prefs_dirty = true; try_save_prefs(self);
                                }
                                if ui.radio_value(&mut self.is_dark_mode, false, "☀️ Light").clicked() {
                                    ctx.set_visuals(egui::Visuals::light());
                                    if self.link_editor_theme { self.advanced_editor.theme = crate::models::structs::EditorColorTheme::GithubLight; }
                                    self.prefs_dirty = true; try_save_prefs(self);
                                }
                                if self.is_dark_mode != prev { ctx.request_repaint(); }
                            });
                        }
                        PrefTab::EditorTheme => {
                            ui.heading("Editor Theme");
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut self.link_editor_theme, "Link with application theme").changed() {
                                    if self.link_editor_theme { self.advanced_editor.theme = if self.is_dark_mode { crate::models::structs::EditorColorTheme::GithubDark } else { crate::models::structs::EditorColorTheme::GithubLight }; }
                                    self.prefs_dirty = true; try_save_prefs(self);
                                }
                                if ui.button("Reset").on_hover_text("Reset to default & relink").clicked() {
                                    self.link_editor_theme = true;
                                    self.advanced_editor.theme = if self.is_dark_mode { crate::models::structs::EditorColorTheme::GithubDark } else { crate::models::structs::EditorColorTheme::GithubLight };
                                    self.prefs_dirty = true; try_save_prefs(self);
                                }
                            });
                            if self.link_editor_theme { ui.label(egui::RichText::new("(Editor theme follows application theme; uncheck to customize)").size(11.0).color(egui::Color32::from_gray(120))); }
                            ui.label("Choose syntax highlighting theme for SQL editor");
                            ui.add_space(4.0);
                            let themes: &[(crate::models::structs::EditorColorTheme, &str, &str)] = &[
                                (crate::models::structs::EditorColorTheme::GithubDark, "GitHub Dark", "Dark theme with blue accents"),
                                (crate::models::structs::EditorColorTheme::GithubLight, "GitHub Light", "Clean light theme"),
                                (crate::models::structs::EditorColorTheme::Gruvbox, "Gruvbox", "Warm earthy retro palette"),
                            ];
                            for (theme, name, desc) in themes {
                                ui.horizontal(|ui| {
                                    let selected = self.advanced_editor.theme == *theme;
                                    if ui.selectable_label(selected, *name).clicked() {
                                        self.advanced_editor.theme = *theme;
                                        if self.link_editor_theme { self.link_editor_theme = false; }
                                        self.prefs_dirty = true; try_save_prefs(self);
                                    }
                                    if selected { ui.label(egui::RichText::new("✓").color(egui::Color32::from_rgb(0,150,255))); }
                                });
                                ui.label(egui::RichText::new(*desc).size(11.0).color(egui::Color32::from_gray(120)));
                                ui.add_space(4.0);
                            }
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Font size:");
                                let mut fs = self.advanced_editor.font_size as i32;
                                if ui.add(egui::DragValue::new(&mut fs).range(8..=32)).changed() {
                                    self.advanced_editor.font_size = fs as f32;
                                    self.prefs_dirty = true; try_save_prefs(self);
                                }
                                ui.separator();
                                ui.checkbox(&mut self.advanced_editor.show_line_numbers, "Line numbers").changed();
                                if ui.checkbox(&mut self.advanced_editor.word_wrap, "Word wrap").changed() { self.prefs_dirty = true; try_save_prefs(self); }
                            });
                        }
                        PrefTab::Performance => {
                            ui.heading("Performance Settings");
                            ui.horizontal(|ui| {
                                let prev_pagination = self.use_server_pagination;
                                if ui.checkbox(&mut self.use_server_pagination, "Server-side pagination")
                                    .on_hover_text("When enabled, queries large tables in pages from the server instead of loading all data at once. Much faster for large datasets.")
                                    .changed() {
                                    self.prefs_dirty = true; try_save_prefs(self);
                                    if prev_pagination != self.use_server_pagination && !self.current_table_headers.is_empty() {
                                        if self.use_server_pagination { self.prefs_save_feedback = Some("Server pagination enabled. Browse a table to see the difference!".to_string()); }
                                        else { self.prefs_save_feedback = Some("Client pagination enabled. Data will be loaded all at once.".to_string()); }
                                        self.prefs_last_saved_at = Some(std::time::Instant::now());
                                    }
                                }
                            });
                            ui.label(egui::RichText::new("Server pagination queries data in smaller chunks (e.g., 100 rows at a time) from the database.\nThis is much faster for large tables but may not work with all custom queries.").size(11.0).color(egui::Color32::from_gray(120)));
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut self.enable_debug_logging, "Enable Debug Logging").changed() {
                                    self.prefs_dirty = true; try_save_prefs(self);
                                    if self.enable_debug_logging {
                                        self.prefs_save_feedback = Some("Debug logging enabled. Please restart the application for this to take effect.".to_string());
                                    } else {
                                        self.prefs_save_feedback = Some("Debug logging disabled. Restart the application to improve performance.".to_string());
                                    }
                                    self.prefs_last_saved_at = Some(std::time::Instant::now());
                                }
                                ui.label(egui::RichText::new("(Requires Restart)").size(11.0).color(egui::Color32::from_gray(120)));
                            });
                            ui.label(egui::RichText::new("Turns on verbose logs. Disable this to improve application performance and reduce disk I/O.").size(11.0).color(egui::Color32::from_gray(120)));
                        }
                        PrefTab::DataDirectory => {
                            ui.heading("Data Directory");
                            ui.label("Choose where Tabular stores its data (connections, queries, history):");
                            ui.add_space(4.0);
                            if self.temp_data_directory.is_empty() { self.temp_data_directory = self.data_directory.clone(); }
                            ui.horizontal(|ui| { ui.label("Current location:"); ui.monospace(&self.data_directory); });
                            ui.horizontal(|ui| { ui.label("New location:"); ui.text_edit_singleline(&mut self.temp_data_directory); if ui.button("📁 Browse").clicked() { self.handle_directory_picker(); } });
                            ui.horizontal(|ui| {
                                let changed = self.temp_data_directory != self.data_directory;
                                let valid_path = !self.temp_data_directory.trim().is_empty() && std::path::Path::new(&self.temp_data_directory).is_absolute();
                                if ui.add_enabled(changed && valid_path, egui::Button::new("Apply Changes")).clicked() {
                                    match crate::config::set_data_dir(&self.temp_data_directory) {
                                        Ok(()) => {
                                            self.refresh_data_directory();
                                            self.prefs_dirty = true; try_save_prefs(self);
                                            if let Some(rt) = &self.runtime && let Ok(new_store) = rt.block_on(crate::config::ConfigStore::new()) { self.config_store = Some(new_store); log::info!("Config store reinitialized for new data directory"); }
                                            self.prefs_save_feedback = Some("Data directory updated successfully!".to_string()); self.prefs_last_saved_at = Some(std::time::Instant::now());
                                            log::info!("Data directory changed to: {}", self.data_directory);
                                        }
                                        Err(e) => { self.error_message = format!("Failed to change data directory: {}", e); self.show_error_message = true; }
                                    }
                                }
                                if ui.button("Reset to Default").clicked() { self.temp_data_directory = dirs::home_dir().map(|mut p| { p.push(".tabular"); p.to_string_lossy().to_string() }).unwrap_or_else(|| ".".to_string()); }
                            });
                            ui.label(egui::RichText::new("⚠️ Changing data directory will require restarting the application").size(11.0).color(egui::Color32::from_rgb(200, 150, 0)));
                        }
                        PrefTab::Update => {
                            ui.heading("Updates");
                            ui.horizontal(|ui| { if ui.checkbox(&mut self.auto_check_updates, "Automatically check for updates on startup").changed() { self.prefs_dirty = true; try_save_prefs(self); } });
                            ui.label(egui::RichText::new("When enabled, Tabular will check for new versions from GitHub releases").size(11.0).color(egui::Color32::from_gray(120)));
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("💾 Save Preferences").clicked() {
                            self.prefs_dirty = true; try_save_prefs(self); self.prefs_save_feedback = Some("Saved".to_string()); self.prefs_last_saved_at = Some(std::time::Instant::now());
                        }
                        if let Some(msg) = &self.prefs_save_feedback { ui.label(egui::RichText::new(msg).color(egui::Color32::from_rgb(0,150,0))); }
                    });
                });
            if !open_flag {
                self.show_settings_window = false;
            }
        }

        // Centered loading overlay when waiting for connection pool
        if self.pool_wait_in_progress {
            let elapsed = self
                .pool_wait_started_at
                .map(|t| t.elapsed())
                .unwrap_or_default();
            let mut keep_open = true; // local to control overlay
            egui::Window::new("Connecting…")
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .collapsible(false)
                .resizable(false)
                .title_bar(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let conn_name = self
                            .pool_wait_connection_id
                            .and_then(|id| self.get_connection_name(id))
                            .unwrap_or_else(|| "(connection)".to_string());
                        ui.label(format!("Establishing connection pool for '{}'…", conn_name));
                    });
                    if elapsed.as_secs() >= 10 {
                        ui.label(
                            egui::RichText::new("This can take a while for slow networks.")
                                .size(11.0)
                                .weak(),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep_open = false;
                        }
                        ui.label(
                            egui::RichText::new(format!("Waiting {}s", elapsed.as_secs()))
                                .size(11.0)
                                .weak(),
                        );
                    });
                });
            if !keep_open {
                // Cancel waiting but keep background creation going
                self.pool_wait_in_progress = false;
                self.pool_wait_connection_id = None;
                self.pool_wait_query.clear();
                self.pool_wait_started_at = None;
            }
        }

        // Check for directory picker results
        if let Some(receiver) = &self.directory_picker_result
            && let Ok(selected_path) = receiver.try_recv()
        {
            self.temp_data_directory = selected_path;
            self.directory_picker_result = None; // Clean up the receiver
        }

        // Check for save directory picker results
        if let Some(receiver) = &self.save_directory_picker_result
            && let Ok(selected_path) = receiver.try_recv()
        {
            self.save_directory = selected_path;
            self.save_directory_picker_result = None; // Clean up the receiver
        }

        // Check for SQLite path picker results (for new SQLite connection)
        if let Some(receiver) = &self.sqlite_path_picker_result
            && let Ok(selected_path) = receiver.try_recv()
        {
            self.temp_sqlite_path = Some(selected_path);
            self.sqlite_path_picker_result = None;
        }

        // Check for background task results
        if let Some(receiver) = &self.background_receiver {
            while let Ok(result) = receiver.try_recv() {
                match result {
                    models::enums::BackgroundResult::RefreshComplete {
                        connection_id,
                        success,
                    } => {
                        // Remove from refreshing set
                        self.refreshing_connections.remove(&connection_id);

                        if success {
                            info!(
                                "✅ Background refresh completed successfully for connection {}",
                                connection_id
                            );

                            // Debug: log all connection nodes in tree
                            info!(
                                "   🔍 Searching in items_tree with {} nodes",
                                self.items_tree.len()
                            );
                            for (i, n) in self.items_tree.iter().enumerate() {
                                info!(
                                    "      Node {}: type={:?}, conn_id={:?}, name={}",
                                    i, n.node_type, n.connection_id, n.name
                                );
                            }

                            // Re-expand connection node to show fresh data
                            let mut node_found = false;
                            for node in &mut self.items_tree {
                                if node.node_type == models::enums::NodeType::Connection
                                    && node.connection_id == Some(connection_id)
                                {
                                    node_found = true;
                                    info!("   ✅ Found connection node: {}", node.name);

                                    // Restore expansion state if we have one pending
                                    if let Some(expansion_state) =
                                        self.pending_expansion_restore.remove(&connection_id)
                                    {
                                        info!(
                                            "🔄 Restoring {} expansion states for connection {}",
                                            expansion_state.len(),
                                            connection_id
                                        );

                                        // Force reload from cache
                                        node.is_loaded = false;

                                        // Restore the expansion state
                                        Self::restore_expansion_state(node, &expansion_state);
                                        info!("   ✅ Expansion state restored");

                                        // Mark expanded child nodes to reload from cache on next render
                                        Self::mark_expanded_nodes_loaded(node);
                                        info!("   ✅ Expanded nodes marked for loading");
                                    } else {
                                        info!("   ⚠️  No expansion state to restore");
                                        // No expansion state to restore, just mark as not loaded
                                        node.is_loaded = false;
                                    }

                                    break;
                                }
                            }

                            if !node_found {
                                info!("   ❌ Connection node {} not found in tree!", connection_id);
                            }

                            // Mark this connection as needing auto-load
                            // Will be processed in the sidebar render where we have proper borrow access
                            self.pending_auto_load.insert(connection_id);
                            info!(
                                "📂 Marked connection {} for auto-load after restore",
                                connection_id
                            );
                            info!(
                                "   pending_auto_load size: {}",
                                self.pending_auto_load.len()
                            );

                            // Request UI repaint to show updated data
                            ctx.request_repaint();
                        } else {
                            debug!("Background refresh failed for connection {}", connection_id);
                            // Clean up pending restore state on failure
                            self.pending_expansion_restore.remove(&connection_id);
                        }
                    }
                    models::enums::BackgroundResult::PrefetchProgress {
                        connection_id,
                        completed,
                        total,
                    } => {
                        // Update prefetch progress
                        self.prefetch_progress
                            .insert(connection_id, (completed, total));
                        ctx.request_repaint();
                    }
                    models::enums::BackgroundResult::PrefetchComplete { connection_id } => {
                        // Prefetch completed
                        self.prefetch_in_progress.remove(&connection_id);
                        self.prefetch_progress.remove(&connection_id);
                        debug!("Prefetch completed for connection {}", connection_id);
                        ctx.request_repaint();
                    }
                    models::enums::BackgroundResult::SqlitePathPicked { path } => {
                        self.temp_sqlite_path = Some(path);
                        ctx.request_repaint();
                    }
                    models::enums::BackgroundResult::UpdateCheckComplete { result } => {
                        // Finish check state first
                        self.update_check_in_progress = false;
                        let was_manual = self.manual_update_check;
                        self.manual_update_check = false;

                        // Defer actions requiring mutable self in separate block to avoid borrow overlap
                        match result {
                            Ok(info) => {
                                let update_available = info.update_available;
                                self.update_info = Some(info.clone());
                                self.update_check_error = None;
                                if was_manual {
                                    self.show_update_dialog = true;
                                } else if update_available {
                                    self.show_update_notification = true;
                                    if !self.update_download_started
                                        && !self.update_download_in_progress
                                    {
                                        self.update_download_started = true;
                                        // Start download after loop ends via flag (can't call method that mutably borrows self again inside borrow scope)
                                    }
                                }
                            }
                            Err(err) => {
                                self.update_check_error = Some(err);
                                self.show_update_dialog = true;
                            }
                        }
                        ctx.request_repaint();
                    }
                }
            }
        }

        while let Ok(message) = self.query_result_receiver.try_recv() {
            self.handle_query_result_message(message);
            ctx.request_repaint();
        }

        // Kick off deferred auto download if flagged (done outside borrow loops)
        if self.update_download_started && !self.update_download_in_progress {
            self.start_update_download();
        }

        // Poll for update install completion (async thread sends on channel)
        if let Some(rx) = &self.update_install_receiver
            && let Ok(success) = rx.try_recv()
        {
            self.update_download_in_progress = false;
            self.update_download_started = false; // Reset this flag to prevent loop
            self.update_installed = success;
            self.show_update_notification = true; // show completion toast
            self.update_install_receiver = None; // cleanup
            ctx.request_repaint();
        }

        // Render mini notification (toast) for update events
        if self.show_update_notification {
            // Clone minimal info to avoid borrow issues in closure
            let info_clone = self.update_info.clone();
            let downloading = self.update_download_in_progress;
            let installed = self.update_installed;
            let download_started = self.update_download_started;
            let mut keep_open = true;
            egui::Window::new("Update")
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .frame(egui::Frame::window(&ctx.style()))
                .show(ctx, |ui| {
                    if let Some(info) = &info_clone {
                        if downloading {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("Downloading update {}...", info.latest_version));
                            });
                        } else if installed {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("✅ Update downloaded successfully!")
                                        .strong(),
                                );

                                #[cfg(target_os = "macos")]
                                ui.label(
                                    egui::RichText::new("DMG file opened for installation")
                                        .size(12.0),
                                );

                                #[cfg(target_os = "linux")]
                                ui.label(
                                    egui::RichText::new("Update downloaded to Downloads folder")
                                        .size(12.0),
                                );

                                #[cfg(target_os = "windows")]
                                ui.label(
                                    egui::RichText::new("Installer opened for installation")
                                        .size(12.0),
                                );

                                ui.horizontal(|ui| {
                                    if ui.button("Open Downloads Folder").clicked() {
                                        #[cfg(target_os = "macos")]
                                        {
                                            let _ = std::process::Command::new("open")
                                                .arg(dirs::download_dir().unwrap_or_else(|| {
                                                    std::path::PathBuf::from("/")
                                                }))
                                                .spawn();
                                        }
                                        #[cfg(target_os = "linux")]
                                        {
                                            let _ = std::process::Command::new("xdg-open")
                                                .arg(dirs::download_dir().unwrap_or_else(|| {
                                                    std::path::PathBuf::from("/")
                                                }))
                                                .spawn();
                                        }
                                        #[cfg(target_os = "windows")]
                                        {
                                            let _ = std::process::Command::new("explorer")
                                                .arg(dirs::download_dir().unwrap_or_else(|| {
                                                    std::path::PathBuf::from("C:\\")
                                                }))
                                                .spawn();
                                        }
                                    }
                                    if ui.button("Dismiss").clicked() {
                                        self.show_update_notification = false;
                                    }
                                });
                            });
                        } else if info.update_available {
                            ui.horizontal(|ui| {
                                ui.label(format!("Update {} available", info.latest_version));
                                if ui.button("Details").clicked() {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new("trigger_update_details"),
                                            true,
                                        );
                                    });
                                }
                                if !download_started && ui.button("Download").clicked() {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new("trigger_manual_download"),
                                            true,
                                        );
                                    });
                                }
                            });
                        } else {
                            keep_open = false;
                        }
                    } else {
                        keep_open = false;
                    }
                });
            if !keep_open {
                self.show_update_notification = false;
            }
            // Check for manual download trigger flag set inside closure
            if ctx.data(|d| {
                d.get_temp::<bool>(egui::Id::new("trigger_manual_download"))
                    .unwrap_or(false)
            }) {
                ctx.data_mut(|d| {
                    d.remove::<bool>(egui::Id::new("trigger_manual_download"));
                });
                if !self.update_download_started && !self.update_download_in_progress {
                    self.update_download_started = true; // Start next frame (handled by deferred block above)
                }
            }
            if ctx.data(|d| {
                d.get_temp::<bool>(egui::Id::new("trigger_update_details"))
                    .unwrap_or(false)
            }) {
                ctx.data_mut(|d| {
                    d.remove::<bool>(egui::Id::new("trigger_update_details"));
                });
                self.show_update_dialog = true;
            }
        }

        // Disable visual indicators for active/focused elements (but keep text selection visible)
        ctx.style_mut(|style| {
            // Keep text selection visible with a subtle highlight
            style.visuals.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(255, 30, 0, 60);
            style.visuals.selection.stroke.color = egui::Color32::BLACK;

            // Only disable other widget visual indicators
            style.visuals.widgets.active.bg_fill = egui::Color32::TRANSPARENT;
            style.visuals.widgets.active.bg_stroke.color = egui::Color32::TRANSPARENT;
            style.visuals.widgets.hovered.bg_stroke.color = egui::Color32::TRANSPARENT;
        });

        // Check if we need to refresh the UI after a connection removal
        if self.needs_refresh {
            self.needs_refresh = false;
            ctx.request_repaint();
        }

        sidebar_database::render_add_connection_dialog(self, ctx);
        sidebar_database::render_edit_connection_dialog(self, ctx);
        dialog::render_save_dialog(self, ctx);
        connection::render_connection_selector(self, ctx);
        dialog::render_error_dialog(self, ctx);
        dialog::render_about_dialog(self, ctx);
        // Index create/edit dialog
        dialog::render_index_dialog(self, ctx);
        dialog::render_create_table_dialog(self, ctx);
        sidebar_query::render_create_folder_dialog(self, ctx);
        sidebar_query::render_move_to_folder_dialog(self, ctx);
        // Update dialog
        self.render_update_dialog(ctx);

        // Persist preferences if dirty and config store ready (outside of window render to avoid borrow issues)
        // Final attempt (in case any change slipped through)
        try_save_prefs(self);

        if self.sidebar_visible {
            egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(250.0)
            .min_width(150.0)
            .max_width(500.0)
            // Reduce default inner padding so tree rows (connection/database/table) start closer to the left edge
            .frame(
                egui::Frame::default()
                    .fill(if ctx.style().visuals.dark_mode {
                        egui::Color32::from_rgb(20, 20, 20)
                    } else {
                        egui::Color32::from_rgb(245, 245, 245)
                    })
                    .inner_margin(egui::Margin::symmetric(4, 6)),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // Top section with tabs
                    ui.horizontal(|ui| {
                        // Calculate equal width for three buttons with responsive design
                        let available_width = ui.available_width();
                        let button_spacing = ui.spacing().item_spacing.x;
                        let button_width = (available_width - (button_spacing * 2.0)) / 3.0;
                        let button_height = 24.0;

                        // Database tab
                        let database_button = if self.selected_menu == "Database" {
                            egui::Button::new(
                                egui::RichText::new("Database")
                                    .color(egui::Color32::WHITE)
                                    .text_style(egui::TextStyle::Body),
                            )
                            .fill(egui::Color32::from_rgb(255, 30, 0))
                        } else {
                            egui::Button::new("Database").fill(egui::Color32::TRANSPARENT)
                        };
                        if ui
                            .add_sized([button_width, button_height], database_button)
                            .clicked()
                        {
                            self.selected_menu = "Database".to_string();
                        }

                        // Queries tab
                        let queries_button = if self.selected_menu == "Queries" {
                            egui::Button::new(
                                egui::RichText::new("Queries")
                                    .color(egui::Color32::WHITE)
                                    .text_style(egui::TextStyle::Body),
                            )
                            .fill(egui::Color32::from_rgb(255, 30, 0)) // Orange fill for active
                        } else {
                            egui::Button::new("Queries").fill(egui::Color32::TRANSPARENT)
                        };
                        if ui
                            .add_sized([button_width, button_height], queries_button)
                            .clicked()
                        {
                            self.selected_menu = "Queries".to_string();
                        }

                        // History tab
                        let history_button = if self.selected_menu == "History" {
                            egui::Button::new(
                                egui::RichText::new("History")
                                    .color(egui::Color32::WHITE)
                                    .text_style(egui::TextStyle::Body),
                            )
                            .fill(egui::Color32::from_rgb(255, 30, 0)) // Orange fill for active
                        } else {
                            egui::Button::new("History").fill(egui::Color32::TRANSPARENT)
                        };
                        if ui
                            .add_sized([button_width, button_height], history_button)
                            .clicked()
                        {
                            self.selected_menu = "History".to_string();
                        }
                    });

                    ui.separator();

                    // Middle section with scrollable content
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        match self.selected_menu.as_str() {
                            "Database" => {
                                if self.connections.is_empty() {
                                    ui.label("No connections configured");
                                    ui.label("Click ➕ to add a new connection");
                                } else {
                                    // Render tree directly without mem::take to avoid race conditions
                                    self.render_tree_for_database_section(ui);
                                }
                            }
                            "Queries" => {
                                // Add right-click context menu support to the UI area itself
                                let queries_response = ui.interact(
                                    ui.available_rect_before_wrap(),
                                    egui::Id::new("queries_area"),
                                    egui::Sense::click(),
                                );
                                queries_response.context_menu(|ui| {
                                    if ui.button("📂 Create Folder").clicked() {
                                        self.show_create_folder_dialog = true;
                                        ui.close();
                                    }
                                });

                                // Render the queries tree and process any clicked items into new tabs
                                let mut queries_tree = std::mem::take(&mut self.queries_tree);
                                let query_files_to_open = self.render_tree(ui, &mut queries_tree, false);
                                self.queries_tree = queries_tree;

                                for (filename, content, file_path, _) in query_files_to_open {
                                    if file_path.is_empty() {
                                        // Placeholder or unsaved query; open as new tab
                                        log::debug!("✅ Processing query click: New unsaved tab '{}'", filename);
                                        crate::editor::create_new_tab(self, filename, content);
                                    } else {
                                        // Open actual file via centralized logic (handles de-dup and metadata)
                                        log::debug!("✅ Processing query click: Opening file '{}'", file_path);
                                        if let Err(err) = sidebar_query::open_query_file(self, &file_path) {
                                            log::debug!("❌ Failed to open query file '{}': {}", file_path, err);
                                        }
                                    }
                                }
                            }
                            "History" => {
                                // Auto Refresh status bar + STOP button
                                if self.auto_refresh_active {
                                    egui::Frame::new()
                                        .stroke(egui::Stroke::new(
                                            1.0,
                                            egui::Color32::from_rgb(255, 30, 0),
                                        ))
                                        .corner_radius(3.0)
                                        .inner_margin(egui::Margin::symmetric(4, 4))
                                        .show(ui, |ui| {
                                            ui.vertical(|ui| {
                                                ui.horizontal(|ui| {
                                                    // Show countdown until next auto-refresh
                                                    let remaining = if let Some(last) = self.auto_refresh_last_run {
                                                        let elapsed = last.elapsed().as_secs();
                                                        let interval = self.auto_refresh_interval_seconds.max(1) as u64;
                                                        if elapsed >= interval {
                                                            0
                                                        } else {
                                                            (interval - elapsed) as u32
                                                        }
                                                    } else {
                                                        self.auto_refresh_interval_seconds
                                                    };
                                                    ui.label(format!(
                                                        "Auto Query {} second(s)",
                                                        remaining
                                                    ));
                                                    ui.add_space(ui.available_width() - 60.0);
                                                    let stop_button = egui::Button::new(
                                                        egui::RichText::new("⏹ STOP")
                                                            .color(egui::Color32::WHITE),
                                                    )
                                                    .fill(egui::Color32::from_rgb(255, 30, 0));
                                                    if ui.add(stop_button).clicked() {
                                                        self.stop_auto_refresh();
                                                    }
                                                });
                                                // Show the query currently being auto-refreshed
                                                if let Some(q) = &self.auto_refresh_query {
                                                    ui.add(
                                                        egui::TextEdit::multiline(&mut q.clone())
                                                            .desired_rows(3)
                                                            .desired_width(f32::INFINITY)
                                                            .interactive(false),
                                                    );
                                                }
                                            });
                                        });
                                }

                                // Search box for history
                                ui.horizontal(|ui| {
                                    ui.label("🔍");
                                    let search_response = ui.text_edit_singleline(&mut self.history_search_text);
                                    if search_response.changed() {
                                        // Refilter history when search text changes
                                        sidebar_history::filter_history_tree(self);
                                    }
                                });

                                // Render history tree and process clicks into new tabs
                                let is_searching = !self.history_search_text.is_empty();
                                
                                let mut history_tree = if is_searching {
                                    std::mem::take(&mut self.filtered_history_tree)
                                } else {
                                    std::mem::take(&mut self.history_tree)
                                };
                                
                                let query_files_to_open = self.render_tree(ui, &mut history_tree, false);
                                
                                if is_searching {
                                    self.filtered_history_tree = history_tree;
                                } else {
                                    self.history_tree = history_tree;
                                }

                                for (filename, content, file_data, _) in query_files_to_open {
                                    // file_data for history contains "connection_name||query"
                                    if let Some((connection_name, _query)) = file_data.split_once("||") {
                                        // Try to find matching connection by name to preselect in the new tab
                                        let conn_id = self
                                            .connections
                                            .iter()
                                            .find(|c| c.name == connection_name)
                                            .and_then(|c| c.id);
                                        if let Some(cid) = conn_id {
                                            log::debug!(
                                                "✅ Processing history click: New tab '{}' with connection '{}' (id={})",
                                                filename, connection_name, cid
                                            );
                                            crate::editor::create_new_tab_with_connection(
                                                self,
                                                filename,
                                                content,
                                                Some(cid),
                                            );
                                            continue;
                                        } else if !connection_name.is_empty() {
                                            log::debug!(
                                                "⚠️ Connection '{}' from history not found. Opening tab without binding.",
                                                connection_name
                                            );
                                        }
                                    }
                                    log::debug!(
                                        "✅ Processing history click: Creating new tab for '{}' (no connection binding)",
                                        filename
                                    );
                                    crate::editor::create_new_tab(self, filename, content);
                                }
                            }
                            _ => {}
                        }
                    });

                    // Bottom section with add button - conditional based on active tab
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {


                        match self.selected_menu.as_str() {
                            "Database" => {
                                if ui
                                    .add_sized(
                                        [24.0, 24.0], // Small square button
                                        egui::Button::new("➕").fill(egui::Color32::from_rgb(255, 30, 0)),
                                    )
                                    .on_hover_text("Add New Database Connection")
                                    .clicked()
                                {
                                    // Reset test connection status saat buka add dialog
                                    self.test_connection_status = None;
                                    self.test_connection_in_progress = false;
                                    self.show_add_connection = true;
                                }
                            }
                            _ => {
                                // No button for History tab
                            }
                        }
                    });
                });
            });
        }

        // Central panel (main editor / data / structure)
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(if ctx.style().visuals.dark_mode {
                        egui::Color32::from_rgb(20, 20, 20)
                    } else {
                        egui::Color32::from_rgb(250, 250, 250)
                    })
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                // Remove the full_table_tab logic - all tabs will now show query editor + results
                // Table tabs will just have additional Data/Structure toggle in the bottom panel

                // Normal query tab: tab bar, editor, toggle, content
                // Compact top bar: tabs on left, selectors on right, single row
                let top_bar_height = 26.0;
                let available_width = ui.available_width();
                let (bar_rect, _resp) = ui.allocate_exact_size(
                    egui::vec2(available_width, top_bar_height),
                    egui::Sense::hover(),
                );
                // Paint background untuk top bar agar mengikuti tema.
                // Sebelumnya area ini tidak di-fill sehingga pada mode light tetap terlihat gelap.
                let bar_bg = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(25, 25, 25)
                } else {
                    egui::Color32::from_rgb(245, 245, 245)
                };
                ui.painter().rect_filled(bar_rect, 0.0, bar_bg);
                // Garis bawah tipis sebagai pemisah dari area editor.
                let bottom_y = bar_rect.bottom();
                // Single subtle bottom border (avoid double-thick dark line in light mode)
                ui.painter().hline(
                    bar_rect.x_range(),
                    bottom_y - 0.5,
                    egui::Stroke::new(
                        1.0,
                        if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(55, 55, 55)
                        } else {
                            egui::Color32::from_rgb(200, 200, 200)
                        },
                    ),
                );
                let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar_rect));
                left_ui.allocate_ui_with_layout(
                    bar_rect.size(),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        
                        // Sidebar Toggle
                        let toggle_icon = if self.sidebar_visible { "◀" } else { "▶" };
                        if ui
                            .add_sized(
                                [20.0, 20.0],
                                egui::Button::new(toggle_icon)
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::NONE),
                            )
                            .on_hover_text(if self.sidebar_visible {
                                "Hide Sidebar"
                            } else {
                                "Show Sidebar"
                            })
                            .clicked()
                        {
                            self.sidebar_visible = !self.sidebar_visible;
                        }
                        
                        let mut to_close = None;
                        let mut to_switch = None;
                        for (i, tab) in self.query_tabs.iter().enumerate() {
                            let active = i == self.active_tab_index;
                            let color = if active {
                                egui::Color32::from_rgb(255, 30, 0)
                            } else {
                                ui.visuals().text_color()
                            };
                            let bg = if ui.visuals().dark_mode {
                                egui::Color32::from_rgb(40, 40, 40)
                            } else {
                                egui::Color32::from_rgb(240, 240, 240)
                            };
                            let mut title = tab.title.clone();
                            if let Some(cid) = tab.connection_id
                                && let Some(n) = self.get_connection_name(cid)
                            {
                                title = format!("{} [{}]", title, n);
                            }
                            let resp = ui.add_sized(
                                [120.0, 20.0],
                                egui::Button::new(
                                    egui::RichText::new(title).color(color).size(12.0),
                                )
                                .fill(bg)
                                .stroke(egui::Stroke::NONE),
                            );
                            if resp.clicked() && !active {
                                to_switch = Some(i);
                            }
                            if (self.query_tabs.len() > 1 || !active)
                                && ui
                                    .add_sized(
                                        [16.0, 16.0],
                                        egui::Button::new("×")
                                            .fill(egui::Color32::TRANSPARENT)
                                            .stroke(egui::Stroke::NONE),
                                    )
                                    .clicked()
                            {
                                to_close = Some(i);
                            }
                        }
                        let plus_bg = if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(50, 50, 50)
                        } else {
                            egui::Color32::from_rgb(220, 220, 220)
                        };
                        if ui
                            .add_sized([20.0, 20.0], egui::Button::new("+").fill(plus_bg))
                            .clicked()
                        {
                            editor::create_new_tab(
                                self,
                                "Untitled Query".to_string(),
                                String::new(),
                            );
                        }
                        if let Some(i) = to_close {
                            editor::close_tab(self, i);
                        }
                        if let Some(i) = to_switch {
                            editor::switch_to_tab(self, i);
                        }
                    },
                );
                // Right side overlay for selectors
                let selectors_width = 400.0; // widened to fit gear + combos
                let selectors_rect = egui::Rect::from_min_size(
                    egui::pos2(bar_rect.right() - selectors_width, bar_rect.top()),
                    egui::vec2(selectors_width, top_bar_height),
                );
                let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(selectors_rect));
                right_ui.allocate_ui_with_layout(
                    selectors_rect.size(),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;

                        // Settings (gear) button on far right with left-click context menu
                        let gear_bg = if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(40, 40, 40)
                        } else {
                            egui::Color32::from_rgb(220, 220, 220)
                        };
                        let gear_btn = egui::Button::new("⚙").fill(gear_bg);
                        let gear_response = ui
                            .add_sized([24.0, 20.0], gear_btn)
                            .on_hover_text("Settings");
                        if gear_response.clicked() {
                            gear_response.request_focus();
                            self.show_settings_menu = !self.show_settings_menu;
                        }
                        if self.show_settings_menu {
                            let pos = gear_response.rect.left_bottom();
                            let mut menu_rect: Option<egui::Rect> = None;
                            egui::Area::new(egui::Id::new("settings_menu"))
                                .order(egui::Order::Foreground)
                                .fixed_pos(pos + egui::vec2(0.0, 4.0))
                                .show(ui.ctx(), |ui| {
                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                        ui.set_min_width(180.0);
                                        if ui
                                            .add(
                                                egui::Button::new("Preferences")
                                                    .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .clicked()
                                        {
                                            self.show_settings_window = true;
                                            self.show_settings_menu = false;
                                        }
                                        ui.separator();
                                        if ui
                                            .add(
                                                egui::Button::new("Check for Updates")
                                                    .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .clicked()
                                        {
                                            self.check_for_updates(true);
                                            self.show_settings_menu = false;
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new("About")
                                                    .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .clicked()
                                        {
                                            self.show_about_dialog = true;
                                            self.show_settings_menu = false;
                                        }
                                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            self.show_settings_menu = false;
                                        }
                                        menu_rect = Some(ui.min_rect());
                                    });
                                });
                            // Close when clicking outside (after drawing)
                            if self.show_settings_menu {
                                let clicked_outside = ui.ctx().input(|i| i.pointer.any_click())
                                    && menu_rect
                                        .map(|r| {
                                            !r.contains(
                                                ui.ctx().pointer_latest_pos().unwrap_or(r.center()),
                                            )
                                        })
                                        .unwrap_or(false)
                                    && !gear_response.clicked();
                                if clicked_outside {
                                    self.show_settings_menu = false;
                                }
                            }
                        }

                        // Small gap between gear and selectors
                        ui.add_space(4.0);

                        let conn_list: Vec<(i64, String)> = self
                            .connections
                            .iter()
                            .filter_map(|c| c.id.map(|id| (id, c.name.clone())))
                            .collect();
                        // Use per-tab connection
                        let (tab_conn_id, tab_db_name) = self
                            .query_tabs
                            .get(self.active_tab_index)
                            .map(|t| (t.connection_id, t.database_name.clone()))
                            .unwrap_or((None, None));
                        let current_conn_name = if let Some(cid) = tab_conn_id {
                            self.get_connection_name(cid)
                                .unwrap_or_else(|| "(conn)".to_string())
                        } else {
                            "Select Connection".to_string()
                        };

                        // Database selector (placed right of connection due to right_to_left order)
                        if let Some(cid) = tab_conn_id {
                            let mut dbs = self.get_databases_cached(cid);
                            if dbs.is_empty() {
                                dbs.push("(default)".to_string());
                            }
                            let active_db = tab_db_name
                                .clone()
                                .unwrap_or_else(|| "(default)".to_string());
                            egui::ComboBox::from_id_salt("query_db_select")
                                .width(140.0)
                                .selected_text(active_db.clone())
                                .show_ui(ui, |ui| {
                                    for db in &dbs {
                                        if ui.selectable_label(active_db == *db, db).clicked() {
                                            if let Some(tab) =
                                                self.query_tabs.get_mut(self.active_tab_index)
                                            {
                                                tab.database_name = if db == "(default)" {
                                                    None
                                                } else {
                                                    Some(db.clone())
                                                };
                                            }
                                            self.current_table_headers.clear();
                                            self.current_table_data.clear();
                                        }
                                    }
                                });
                            ui.add_space(6.0);
                        }

                        // Connection selector
                        egui::ComboBox::from_id_salt("query_conn_select")
                            .width(150.0)
                            .selected_text(current_conn_name)
                            .show_ui(ui, |ui| {
                                for (cid, name) in &conn_list {
                                    let selected = tab_conn_id == Some(*cid);
                                    if ui.selectable_label(selected, name).clicked() {
                                        if let Some(tab) =
                                            self.query_tabs.get_mut(self.active_tab_index)
                                        {
                                            tab.connection_id = Some(*cid);
                                            tab.database_name = None; // reset db for new connection
                                        }
                                        self.current_table_headers.clear();
                                        self.current_table_data.clear();
                                    }
                                }
                            });
                    },
                );

                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                    && tab.content != self.editor.text
                {
                    tab.content = self.editor.text.clone();
                    tab.is_modified = true;
                }

                // Check if this is a table/collection tab for different layout
                let is_table_tab = self
                    .query_tabs
                    .get(self.active_tab_index)
                    .map(|t| {
                        t.title.starts_with("Table:")
                            || t.title.starts_with("View:")
                            || t.title.starts_with("Collection:")
                    })
                    .unwrap_or(false);

                if is_table_tab {
                    // Table tabs: Direct Data/Structure view without query editor
                    ui.vertical(|ui| {
                        // Data/Structure toggle at the top
                        ui.scope(|ui| {
                            // Provide consistent active styling for the toggle buttons.
                            let mut style = ui.style().as_ref().clone();
                            style.visuals.selection.bg_fill = egui::Color32::from_rgb(255, 30, 0);
                            style.visuals.selection.stroke.color = egui::Color32::from_rgb(255, 30, 0);
                            ui.set_style(style);

                            ui.horizontal(|ui| {
                                let default_text = ui.visuals().widgets.inactive.fg_stroke.color;

                                let is_data = self.table_bottom_view
                                    == models::structs::TableBottomView::Data;
                                let data_text = egui::RichText::new("📊 Data").color(if is_data {
                                    egui::Color32::WHITE
                                } else {
                                    default_text
                                });
                                if ui.selectable_label(is_data, data_text).clicked() {
                                    self.table_bottom_view =
                                        models::structs::TableBottomView::Data;
                                    // Ensure DATA view uses persisted cache when available.
                                    if self.current_table_headers.is_empty() {
                                        if let Some(tab) = self.query_tabs.get(self.active_tab_index)
                                            && let Some(conn_id) = tab.connection_id {
                                                let db_name = tab.database_name.clone().unwrap_or_default();
                                                let table = data_table::infer_current_table_name(self);
                                                if !db_name.is_empty() && !table.is_empty()
                                                    && let Some((hdrs, rows)) = crate::cache_data::get_table_rows_from_cache(self, conn_id, &db_name, &table)
                                                        && !hdrs.is_empty() {
                                                            info!("📦 Showing cached data (toggle) for {}/{} ({} cols, {} rows)", db_name, table, hdrs.len(), rows.len());
                                                            self.current_table_headers = hdrs.clone();
                                                            self.current_table_data = rows.clone();
                                                            self.all_table_data = rows;
                                                            self.total_rows = self.all_table_data.len();
                                                            self.current_page = 0;
                                                            if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                                                                active_tab.result_headers = self.current_table_headers.clone();
                                                                active_tab.result_rows = self.current_table_data.clone();
                                                                active_tab.result_all_rows = self.all_table_data.clone();
                                                                active_tab.result_table_name = self.current_table_name.clone();
                                                                active_tab.is_table_browse_mode = true;
                                                                active_tab.current_page = self.current_page;
                                                                active_tab.page_size = self.page_size;
                                                                active_tab.total_rows = self.total_rows;
                                                            }
                                                        }
                                            }
                                    } else {
                                        // Data already present in memory; no need to hit persistent cache
                                        debug!("✅ Using in-memory data for Data tab (no cached reload)");
                                    }
                                }
                                let is_struct = self.table_bottom_view
                                    == models::structs::TableBottomView::Structure;
                                let struct_text = egui::RichText::new("⊞ Structure").color(if is_struct {
                                    egui::Color32::WHITE
                                } else {
                                    default_text
                                });
                                if ui.selectable_label(is_struct, struct_text).clicked() {
                                    self.table_bottom_view =
                                        models::structs::TableBottomView::Structure;
                                    // Load structure only if target changed; otherwise keep in-memory (avoid repeated cache hits)
                                    if let Some(conn_id) = self.current_connection_id {
                                        let db = self
                                            .query_tabs
                                            .get(self.active_tab_index)
                                            .and_then(|t| t.database_name.clone())
                                            .unwrap_or_default();
                                        let table = data_table::infer_current_table_name(self);
                                        let current_target = (conn_id, db.clone(), table.clone());
                                        if self
                                            .last_structure_target
                                            .as_ref()
                                            .map(|t| t != &current_target)
                                            .unwrap_or(true)
                                        {
                                            data_table::load_structure_info_for_current_table(self);
                                        } else {
                                            debug!("✅ Using in-memory structure for {}/{} (no reload)", db, table);
                                        }
                                    } else {
                                        // No active connection, try load to ensure state sane
                                        data_table::load_structure_info_for_current_table(self);
                                    }
                                }

                                // Show Query toggle only for View tabs and when we have DDL
                                let is_view_tab = self
                                    .query_tabs
                                    .get(self.active_tab_index)
                                    .map(|t| t.title.starts_with("View:"))
                                    .unwrap_or(false);
                                let has_ddl = self.current_object_ddl.is_some()
                                    || self
                                        .query_tabs
                                        .get(self.active_tab_index)
                                        .and_then(|t| t.object_ddl.clone())
                                        .is_some();
                                if is_view_tab && has_ddl {
                                    let is_query = self.table_bottom_view
                                        == models::structs::TableBottomView::Query;
                                    let query_text = egui::RichText::new("📝 Query").color(if is_query {
                                        egui::Color32::WHITE
                                    } else {
                                        default_text
                                    });
                                    if ui.selectable_label(is_query, query_text).clicked() {
                                        self.table_bottom_view = models::structs::TableBottomView::Query;
                                    }
                                }

                                // Messages tab - show when there's a query message
                                if !self.query_message.is_empty() {
                                    let is_messages = self.table_bottom_view
                                        == models::structs::TableBottomView::Messages;
                                    let messages_text = egui::RichText::new("💬 Messages").color(if is_messages {
                                        egui::Color32::WHITE
                                    } else {
                                        default_text
                                    });
                                    if ui.selectable_label(is_messages, messages_text).clicked() {
                                        self.table_bottom_view = models::structs::TableBottomView::Messages;
                                    }
                                }
                            });
                        });

                        ui.separator();

                        // Main content area takes remaining space
                        let remaining_height = ui.available_height();
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), remaining_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                // Render Data / Structure / Query (DDL) based on toggle
                                match self.table_bottom_view {
                                    models::structs::TableBottomView::Structure => {
                                        data_table::render_structure_view(self, ui);
                                    }
                                    models::structs::TableBottomView::Query => {
                                        // Ensure editor text = DDL for this view
                                        let ddl_text = self
                                            .query_tabs
                                            .get(self.active_tab_index)
                                            .and_then(|tab| tab.object_ddl.clone())
                                            .or_else(|| self.current_object_ddl.clone())
                                            .unwrap_or_default();
                                        if self.editor.text != ddl_text {
                                            self.editor.set_text(ddl_text.clone());
                                        }

                                        // Use consolidated query editor rendering
                                        self.render_query_editor_with_split(ui, "view_query");

                                        // Keep object_ddl in sync with the active editor content
                                        let current_text = self.editor.text.clone();
                                        self.current_object_ddl = Some(current_text.clone());
                                        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                                            tab.object_ddl = Some(current_text);
                                        }
                                    }
                                    models::structs::TableBottomView::Messages => {
                                        // Render messages panel content
                                        self.render_messages_content(ui);
                                    }
                                    _ => {
                                        data_table::render_table_data(self, ui);
                                    }
                                }
                            },
                        );
                    });
                } else {
                    // Regular query tabs: Use consolidated rendering
                    let mut rendered_diagram = false;
                    let mut diagram_to_save = None;
                    
                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                        && let Some(diagram_state) = &mut tab.diagram_state {
                           crate::diagram_view::render_diagram(ui, diagram_state);
                           rendered_diagram = true;
                           
                           if diagram_state.save_requested {
                               diagram_state.save_requested = false;
                               diagram_to_save = Some((tab.connection_id, tab.database_name.clone(), diagram_state.clone()));
                           }
                        }
                    
                    if let Some((conn_id_opt, db_name_opt, state)) = diagram_to_save
                         && let Some(cid) = conn_id_opt {
                             let db = db_name_opt.unwrap_or_else(|| "default".to_string());
                             self.save_diagram(cid, &db, &state);
                         }
                    
                    if !rendered_diagram {
                        self.render_query_editor_with_split(ui, "regular_query");
                    }
                    
                    // Floating tab buttons at bottom-right corner (only show if executed or has message)
                    let executed = self.query_tabs.get(self.active_tab_index).map(|t| t.has_executed_query).unwrap_or(false);
                    let has_headers = !self.current_table_headers.is_empty();
                    if executed || has_headers || !self.query_message.is_empty() {
                        let margin = 6.0;
                        let button_height = 18.0; // Match Clear selection button height
                        let button_spacing = 4.0;
                        
                        // Calculate total width needed for buttons
                        let data_button_width = 80.0;
                        let messages_button_width = if !self.query_message.is_empty() { 110.0 } else { 0.0 };
                        let total_width = data_button_width + if !self.query_message.is_empty() { button_spacing + messages_button_width } else { 0.0 };
                        
                        // Position at bottom-right
                        let screen_rect = ui.ctx().screen_rect();
                        let button_pos = egui::pos2(
                            screen_rect.max.x - total_width - margin,
                            screen_rect.max.y - button_height - margin
                        );

                        egui::Area::new(egui::Id::new("bottom_tab_buttons"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(button_pos)
                            .show(ui.ctx(), |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = button_spacing;
                                    
                                    let is_data = self.table_bottom_view == models::structs::TableBottomView::Data;
                                    let data_bg = if is_data {
                                        egui::Color32::from_rgb(255, 30, 0)
                                    } else if ui.visuals().dark_mode {
                                        egui::Color32::from_rgb(50, 50, 50)
                                    } else {
                                        egui::Color32::from_rgb(230, 230, 230)
                                    };
                                    let data_text_color = if is_data {
                                        egui::Color32::WHITE
                                    } else {
                                        ui.visuals().text_color()
                                    };
                                    
                                    if ui.add_sized(
                                        [data_button_width, button_height],
                                        egui::Button::new(egui::RichText::new("📊 Data").color(data_text_color))
                                            .fill(data_bg)
                                    ).clicked() {
                                        self.table_bottom_view = models::structs::TableBottomView::Data;
                                    }

                                    // Messages button - only show when there's a query message
                                    if !self.query_message.is_empty() {
                                        let is_messages = self.table_bottom_view == models::structs::TableBottomView::Messages;
                                        let messages_bg = if is_messages {
                                            egui::Color32::from_rgb(255, 30, 0)
                                        } else if ui.visuals().dark_mode {
                                            egui::Color32::from_rgb(50, 50, 50)
                                        } else {
                                            egui::Color32::from_rgb(230, 230, 230)
                                        };
                                        let messages_text_color = if is_messages {
                                            egui::Color32::WHITE
                                        } else {
                                            ui.visuals().text_color()
                                        };
                                        
                                        if ui.add_sized(
                                            [messages_button_width, button_height],
                                            egui::Button::new(egui::RichText::new("💬 Messages").color(messages_text_color))
                                                .fill(messages_bg)
                                        ).clicked() {
                                            self.table_bottom_view = models::structs::TableBottomView::Messages;
                                        }
                                    }
                                });
                            });
                    }
                }

                data_table::render_drop_index_confirmation(self, ui.ctx());
                data_table::render_drop_column_confirmation(self, ui.ctx());

                // Custom View Dialog
                self.render_add_view_dialog(ui.ctx());

                // Render context menu for row operations
                if self.show_row_context_menu {
                    let mut close_menu = false;

                    let area_response = egui::Area::new(egui::Id::new("row_context_menu"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(self.context_menu_pos)
                        .show(ui.ctx(), |ui| {
                            let frame_response = egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_min_width(150.0);
                                if ui.button("📋 Duplicate Row").clicked() {
                                    self.spreadsheet_duplicate_selected_row();
                                    close_menu = true;
                                }
                                ui.separator();
                                if ui.button("🗑️ Delete Row").clicked() {
                                    self.spreadsheet_delete_selected_row();
                                    close_menu = true;
                                }
                            });
                            frame_response.response.hovered()
                        });
                    let hovered_menu = area_response.inner;
                    // Close context menu when clicking elsewhere or pressing Escape
                    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.show_row_context_menu = false;
                        self.context_menu_row = None;
                        self.context_menu_just_opened = false;
                        self.context_menu_pos = egui::Pos2::ZERO;
                    }
                    if close_menu {
                        self.show_row_context_menu = false;
                        self.context_menu_row = None;
                        self.context_menu_just_opened = false;
                        self.context_menu_pos = egui::Pos2::ZERO;
                    }
                    // Close context menu when clicking anywhere outside the menu
                    // Skip the first frame after opening to avoid immediate closure from the right-click event
                    if !self.context_menu_just_opened {
                        if ui.ctx().input(|i| i.pointer.any_click()) && !hovered_menu {
                            self.show_row_context_menu = false;
                            self.context_menu_row = None;
                            self.context_menu_pos = egui::Pos2::ZERO;
                        }
                    } else {
                        // Clear the flag after first frame
                        self.context_menu_just_opened = false;
                    }
                }

                // Render MongoDB drop collection confirmation dialog if pending
                if let Some((conn_id, ref db, ref coll)) = self.pending_drop_collection.clone() {
                    let title = format!("Konfirmasi Drop Collection: {}.{}", db, coll);
                    egui::Window::new(title)
                        .collapsible(false)
                        .resizable(false)
                        .pivot(egui::Align2::CENTER_CENTER)
                        .fixed_size(egui::vec2(480.0, 160.0))
                        .show(ui.ctx(), |ui| {
                            ui.label("Tindakan ini tidak dapat dibatalkan.");
                            ui.add_space(8.0);
                            ui.code(format!("db.{}.{}.drop()", db, coll));
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    self.pending_drop_collection = None;
                                }
                                if ui
                                    .button(egui::RichText::new("Confirm").color(egui::Color32::from_rgb(255, 30, 0)))
                                    .clicked()
                                {
                                    // Execute drop via Mongo driver
                                    let (cid, dbn, colln) = (conn_id, db.clone(), coll.clone());
                                    let mut ok = false;
                                    if let Some(rt) = self.runtime.clone() {
                                        ok = rt.block_on(async {
                                            crate::driver_mongodb::drop_collection(self, cid, &dbn, &colln).await
                                        });
                                    } else if let Ok(rt) = tokio::runtime::Runtime::new() {
                                        ok = rt.block_on(async {
                                            crate::driver_mongodb::drop_collection(self, cid, &dbn, &colln).await
                                        });
                                    }
                                    if ok {
                                        // Clear caches and refresh connection tree
                                        self.clear_connection_cache(conn_id);
                                        self.refresh_connection(conn_id);
                                        self.error_message = format!("Collection '{}.{}' berhasil di-drop", db, coll);
                                        self.show_error_message = true; // Show as toast/dialog
                                    } else {
                                        self.error_message = format!("Gagal drop collection '{}.{}'", db, coll);
                                        self.show_error_message = true;
                                    }
                                    self.pending_drop_collection = None;
                                }
                            });
                        });
                }

                // Render DROP TABLE confirmation dialog if pending
                if let Some((conn_id, ref db, ref table, ref stmt)) = self.pending_drop_table.clone() {
                    let title = format!("Konfirmasi Drop Table: {}.{}", db, table);
                    let stmt_str = stmt.clone();
                    egui::Window::new(title)
                        .collapsible(false)
                        .resizable(false)
                        .pivot(egui::Align2::CENTER_CENTER)
                        .fixed_size(egui::vec2(480.0, 180.0))
                        .show(ui.ctx(), |ui| {
                            ui.label("Tindakan ini tidak dapat dibatalkan.");
                            ui.add_space(8.0);
                            ui.code(&stmt_str);
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    self.pending_drop_table = None;
                                }
                                if ui
                                    .button(egui::RichText::new("Confirm").color(egui::Color32::from_rgb(255, 30, 0)))
                                    .clicked()
                                {
                                    use log::{info, error};
                                    info!("🗑️ Executing DROP TABLE:");
                                    info!("   Connection ID: {}", conn_id);
                                    info!("   Database: {}", db);
                                    info!("   Table: {}", table);
                                    info!("   Statement: {}", stmt_str);
                                    // Execute DROP TABLE statement
                                    let result = crate::connection::execute_query_with_connection(
                                        self,
                                        conn_id,
                                        stmt_str.clone(),
                                    );
                                    // Log detailed result
                                    match &result {
                                        Some((headers, rows)) => {
                                            info!("   Result: Success");
                                            info!("   Headers: {:?}", headers);
                                            info!("   Rows count: {}", rows.len());
                                            if !rows.is_empty() {
                                                info!("   First row: {:?}", rows.first());
                                            }
                                            // Check if it's an error result
                                            if headers.first().map(|h| h == "Error").unwrap_or(false) {
                                                error!("   ⚠️ Query returned Error header!");
                                                if let Some(err_row) = rows.first() {
                                                    error!("   Error message: {:?}", err_row);
                                                }
                                            }
                                        }
                                        None => {
                                            error!("   Result: None (Failed)");
                                        }
                                    }
                                    // Check if result is successful (not None and not Error)
                                    let is_success = match &result {
                                        Some((headers, _)) => {
                                            !headers.first().map(|h| h == "Error").unwrap_or(false)
                                        }
                                        None => false,
                                    };
                                    if is_success {
                                        info!("✅ DROP TABLE succeeded for {}.{}", db, table);
                                        info!("   Connection ID: {}", conn_id);
                                        info!("   Database: '{}'", db);
                                        info!("   Table: '{}'", table);
                                        // Use incremental update: just remove the table from tree
                                        info!("🌲 Removing table from sidebar tree (incremental)...");
                                        self.remove_table_from_tree(conn_id, db, table);
                                        // Clear cache for this table (but don't refresh entire connection)
                                        info!("🧹 Clearing cache for table {}.{}", db, table);
                                        self.clear_table_cache(conn_id, db, table);
                                        // Force UI repaint to reflect changes immediately
                                        ui.ctx().request_repaint();
                                        self.error_message = format!("Table '{}.{}' berhasil di-drop", db, table);
                                        self.show_error_message = true;
                                    } else {
                                        error!("❌ DROP TABLE failed for {}.{}", db, table);                                        
                                        // Show error message from result if available
                                        let error_msg = if let Some((headers, rows)) = result {
                                            if headers.first().map(|h| h == "Error").unwrap_or(false) {
                                                rows.first()
                                                    .and_then(|row| row.first())
                                                    .cloned()
                                                    .unwrap_or_else(|| format!("Gagal drop table '{}.{}'", db, table))
                                            } else {
                                                format!("Gagal drop table '{}.{}'", db, table)
                                            }
                                        } else {
                                            format!("Gagal drop table '{}.{}'", db, table)
                                        };
                                        self.error_message = error_msg;
                                        self.show_error_message = true;
                                    }
                                    self.pending_drop_table = None;
                                }
                            });
                        });
                }

                self.render_active_query_jobs_overlay(ctx);
            });

        // Handle copy operations AFTER UI render (state already updated)
        // Note: We only reach here if table/structure has potential focus (not editor/message)
        if copy_shortcut_detected {
            debug!("📋 CMD+C for table/structure - executing copy...");
            
            let has_structure_selection = self.structure_selected_cell.is_some() 
                || self.structure_sel_anchor.is_some();
            let has_data_selection = self.selected_cell.is_some() 
                || self.table_sel_anchor.is_some();
                
            let structure_focus = self.table_bottom_view
                == models::structs::TableBottomView::Structure
                && (self.table_recently_clicked || has_structure_selection);
            let data_focus = self.table_recently_clicked || has_data_selection;
            
            debug!("📋 Table copy: table_flag={}, data_sel={:?}, struct_focus={}, data_focus={}", 
                self.table_recently_clicked,
                self.selected_cell,
                structure_focus,
                data_focus
            );

            // Handle structure/data copy
            if structure_focus {
                    // Structure multi-cell block
                    if let (Some((ar, ac)), Some((br, bc))) =
                        (self.structure_sel_anchor, self.structure_selected_cell)
                    {
                        let rmin = ar.min(br);
                        let rmax = ar.max(br);
                        let cmin = ac.min(bc);
                        let cmax = ac.max(bc);
                        let mut csv_out = String::new();
                        
                        match self.structure_sub_view {
                            models::structs::StructureSubView::Columns => {
                                for r in rmin..=rmax {
                                    if let Some(row) = self.structure_columns.get(r) {
                                        let rowvals = [
                                            (r + 1).to_string(),
                                            row.name.clone(),
                                            row.data_type.clone(),
                                            row.nullable.map(|b| if b { "YES" } else { "NO" }).unwrap_or("?").to_string(),
                                            row.default_value.clone().unwrap_or_default(),
                                            row.extra.clone().unwrap_or_default(),
                                        ];
                                        let mut fields: Vec<String> = Vec::new();
                                        for c in cmin..=cmax {
                                            let v = rowvals.get(c).cloned().unwrap_or_default();
                                            fields.push(if v.contains(',') || v.contains('"') { format!("\"{}\"", v.replace('"', "\"\"")) } else { v });
                                        }
                                        csv_out.push_str(&fields.join(","));
                                        csv_out.push('\n');
                                    }
                                }
                            }
                            models::structs::StructureSubView::Indexes => {
                                for r in rmin..=rmax {
                                    if let Some(row) = self.structure_indexes.get(r) {
                                        let rowvals = [
                                            (r + 1).to_string(),
                                            row.name.clone(),
                                            row.method.clone().unwrap_or_default(),
                                            if row.unique { "YES".to_string() } else { "NO".to_string() },
                                            if row.columns.is_empty() { String::new() } else { row.columns.join(",") },
                                        ];
                                        let mut fields: Vec<String> = Vec::new();
                                        for c in cmin..=cmax {
                                            let v = rowvals.get(c).cloned().unwrap_or_default();
                                            fields.push(if v.contains(',') || v.contains('"') { format!("\"{}\"", v.replace('"', "\"\"")) } else { v });
                                        }
                                        csv_out.push_str(&fields.join(","));
                                        csv_out.push('\n');
                                    }
                                }
                            }
                        }
                        
                        if !csv_out.is_empty() {
                            ctx.copy_text(csv_out.clone());
                            debug!("📋 Copied Structure block {}x{} ({} chars)", rmax-rmin+1, cmax-cmin+1, csv_out.len());
                        }
                    }
                    // Structure single cell
                    else if let Some((r, c)) = self.structure_selected_cell {
                        let val = match self.structure_sub_view {
                            models::structs::StructureSubView::Columns => {
                                if let Some(row) = self.structure_columns.get(r) {
                                    let rowvals = [(r + 1).to_string(), row.name.clone(), row.data_type.clone(), 
                                                   row.nullable.map(|b| if b { "YES" } else { "NO" }).unwrap_or("?").to_string(),
                                                   row.default_value.clone().unwrap_or_default(), row.extra.clone().unwrap_or_default()];
                                    rowvals.get(c).cloned().unwrap_or_default()
                                } else { String::new() }
                            }
                            models::structs::StructureSubView::Indexes => {
                                if let Some(row) = self.structure_indexes.get(r) {
                                    let rowvals = [(r + 1).to_string(), row.name.clone(), row.method.clone().unwrap_or_default(),
                                                   if row.unique { "YES".to_string() } else { "NO".to_string() },
                                                   if row.columns.is_empty() { String::new() } else { row.columns.join(",") }];
                                    rowvals.get(c).cloned().unwrap_or_default()
                                } else { String::new() }
                            }
                        };
                        ctx.copy_text(val.clone());
                        debug!("📋 Copied Structure cell ({},{}) len={} chars", r, c, val.len());
                    }
                }
                // Data table copy
                else if data_focus {
                    // Multi-cell block
                    if let (Some(a), Some(b)) = (self.table_sel_anchor, self.selected_cell) {
                        if let Some(csv) = crate::data_table::copy_selected_block_as_csv(self, a, b) {
                            ctx.copy_text(csv.clone());
                            debug!("📋 Copied Data block ({} chars)", csv.len());
                        }
                    }
                    // Single cell
                    else if let Some((r, c)) = self.selected_cell {
                        if let Some(row) = self.current_table_data.get(r)
                            && let Some(val) = row.get(c)
                        {
                            ctx.copy_text(val.clone());
                            debug!("📋 Copied cell ({},{}) len={} chars", r, c, val.len());
                        }
                    }
                    // Selected rows
                    else if !self.selected_rows.is_empty() {
                        if let Some(csv) = data_table::copy_selected_rows_as_csv(self) {
                            ctx.copy_text(csv.clone());
                            debug!("📋 Copied {} row(s) ({} chars)", self.selected_rows.len(), csv.len());
                        }
                    }
                    // Selected columns
                    else if !self.selected_columns.is_empty()
                        && let Some(csv) = data_table::copy_selected_columns_as_csv(self)
                    {
                        ctx.copy_text(csv.clone());
                        debug!(
                            "📋 Copied {} col(s) ({} chars)",
                            self.selected_columns.len(),
                            csv.len()
                        );
                    }
                } else {
                    debug!("⚠️ CMD+C but no focus target (table_flag={}, data_sel={:?})", 
                        self.table_recently_clicked, self.selected_cell);
                }
        }
    } // end update
} // end impl App for Tabular

// Small reusable helper to render the floating "Format SQL" button.
// Returns true if the button was clicked in this frame.
fn draw_format_sql_button(
    ctx: &egui::Context,
    area_id: egui::Id,
    pos: egui::Pos2,
    size: egui::Vec2,
    corner: u8,
) -> bool {
    let mut clicked = false;
    let format_text = egui::RichText::new("</>").size(16.0);
    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |area_ui| {
            let button = egui::Button::new(format_text.clone())
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.5, egui::Color32::TRANSPARENT))
                .corner_radius(egui::CornerRadius::same(corner));
            let response = area_ui
                .add_sized(size, button)
                .on_hover_text("Format SQL (Cmd+Shift+F)");
            if response.clicked() {
                clicked = true;
            }
        });
    clicked
}

// Helper to finalize query result display from a raw execution result
impl Tabular {


    pub(crate) fn extend_query_icon_hold(&mut self) {
        self.query_icon_hold_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(900));
    }
    fn get_diagram_path(&self, conn_id: i64, db_name: &str) -> Option<std::path::PathBuf> {
        let mut path = if !self.data_directory.is_empty() {
             std::path::PathBuf::from(&self.data_directory).join("diagrams")
        } else if let Some(config_dir) = dirs::data_local_dir() {
             config_dir.join("tabular").join("diagrams")
        } else {
             return None;
        };

        let _ = std::fs::create_dir_all(&path);
        // Sanitize filename
        let safe_db_name: String = db_name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        path.push(format!("conn_{}_{}.json", conn_id, safe_db_name));
        log::debug!("get_diagram_path: inputs=({}, '{}') -> path={:?}", conn_id, db_name, path);
        Some(path)
    }

    fn save_diagram(&self, conn_id: i64, db_name: &str, state: &models::structs::DiagramState) {
        if let Some(path) = self.get_diagram_path(conn_id, db_name) {
            match std::fs::File::create(&path) {
                Ok(file) => {
                    let writer = std::io::BufWriter::new(file);
                    if let Err(e) = serde_json::to_writer_pretty(writer, state) {
                        log::error!("Failed to serialize diagram state: {}", e);
                    } else {
                        log::info!("Diagram layout saved to {:?}", path);
                    }
                },
                Err(e) => log::error!("Failed to create diagram file {:?}: {}", path, e),
            }
        }
    }

    fn load_diagram(&self, conn_id: i64, db_name: &str) -> Option<models::structs::DiagramState> {
        if let Some(path) = self.get_diagram_path(conn_id, db_name)
            && path.exists() {
                 match std::fs::File::open(&path) {
                    Ok(file) => {
                        let reader = std::io::BufReader::new(file);
                        match serde_json::from_reader(reader) {
                            Ok(state) => {
                                log::info!("Diagram layout loaded from {:?}", path);
                                return Some(state);
                            },
                            Err(e) => log::error!("Failed to deserialize diagram state: {}", e),
                        }
                    },
                    Err(e) => log::error!("Failed to open diagram file {:?}: {}", path, e),
                 }
                 None
            } else {
                None
            }
    }
}
