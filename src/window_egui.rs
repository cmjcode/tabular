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
    driver_sqlite, editor, models, sidebar_database, sidebar_query,
    spreadsheet::SpreadsheetOperations,
};
use crate::{data_table, driver_mssql};

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
    // Context menu and edit connection fields
    pub show_edit_connection: bool,
    pub edit_connection: models::structs::ConnectionConfig,
    // UI refresh flag
    pub needs_refresh: bool,
    // Table data display
    pub current_table_data: Vec<Vec<String>>,
    pub current_table_headers: Vec<String>,
    pub current_table_name: String,
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
    // Background refresh status tracking
    pub refreshing_connections: std::collections::HashSet<i64>,
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
    // Error message display
    pub error_message: String,
    pub show_error_message: bool,
    // Advanced Editor Configuration
    pub advanced_editor: models::structs::AdvancedEditor,
    // Selected text for executing only selected queries
    pub selected_text: String,
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
    // Query folder management
    pub show_create_folder_dialog: bool,
    pub new_folder_name: String,
    pub selected_query_for_move: Option<String>,
    pub show_move_to_folder_dialog: bool,
    pub target_folder_name: String,
    pub parent_folder_for_creation: Option<String>,
    pub selected_folder_for_removal: Option<String>,
    pub folder_removal_map: std::collections::HashMap<i64, String>, // Map hash to folder path
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
    // Scroll to selected cell flag
    pub scroll_to_selected_cell: bool,
    // Column width management for resizable columns
    pub column_widths: Vec<f32>, // Store individual column widths
    pub min_column_width: f32,
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
    // Pending drop index confirmation
    pub pending_drop_index_name: Option<String>,
    pub pending_drop_index_stmt: Option<String>,
    // Pending drop column confirmation
    pub pending_drop_column_name: Option<String>,
    pub pending_drop_column_stmt: Option<String>,
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

impl Tabular {
    // Multi-cursor: add a new cursor position if not existing
    pub fn add_cursor(&mut self, pos: usize) {
        let p = pos.min(self.editor.text.len());
        if !self.extra_cursors.contains(&p) {
            self.extra_cursors.push(p);
            self.extra_cursors.sort_unstable();
        }
        // Transitional mirror to structured selections
    if self.multi_selection.to_lapce_selection().is_empty() {
            self.multi_selection
                .add_collapsed(self.cursor_position.min(self.editor.text.len()));
        }
    self.multi_selection.add_collapsed(p);
    }

    pub fn clear_extra_cursors(&mut self) {
        self.extra_cursors.clear();
    }
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

        // Create shared runtime for all database operations
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => Some(Arc::new(rt)),
            Err(e) => {
                error!("Failed to create runtime: {}", e);
                None
            }
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
            show_edit_connection: false,
            edit_connection: models::structs::ConnectionConfig::default(),
            needs_refresh: false,
            current_table_data: Vec::new(),
            current_table_headers: Vec::new(),
            current_table_name: String::new(),
            current_connection_id: None,
            current_page: 0,
            page_size: 100, // Default 100 rows per page
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
            refreshing_connections: std::collections::HashSet::new(),
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
            error_message: String::new(),
            show_error_message: false,
            advanced_editor: models::structs::AdvancedEditor::default(),
            selected_text: String::new(),
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
            // Query folder management
            show_create_folder_dialog: false,
            new_folder_name: String::new(),
            selected_query_for_move: None,
            show_move_to_folder_dialog: false,
            target_folder_name: String::new(),
            parent_folder_for_creation: None,
            selected_folder_for_removal: None,
            folder_removal_map: std::collections::HashMap::new(),
            last_cleanup_time: std::time::Instant::now(),
            selected_row: None,
            selected_cell: None,
            selected_rows: BTreeSet::new(),
            selected_columns: BTreeSet::new(),
            last_clicked_row: None,
            last_clicked_column: None,
            table_recently_clicked: false,
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
            selected_autocomplete_index: 0,
            autocomplete_prefix: String::new(),
            last_autocomplete_trigger_len: 0,
            pending_cursor_set: None,
            editor_focus_boost_frames: 0,
            autocomplete_expected_cursor: None,
            autocomplete_protection_frames: 0,
            autocomplete_navigated: false,
            // Index dialog defaults
            show_index_dialog: false,
            index_dialog: None,
            table_bottom_view: models::structs::TableBottomView::default(),
            structure_columns: Vec::new(),
            structure_indexes: Vec::new(),
            pending_drop_index_name: None,
            pending_drop_index_stmt: None,
            pending_drop_column_name: None,
            pending_drop_column_stmt: None,
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
            config_store: None,
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
        if app.auto_check_updates
            && let (Some(sender), Some(rt)) = (&app.background_sender, &app.runtime)
            && let Ok(store) = rt.block_on(crate::config::ConfigStore::new())
        {
            println!("Checking last update check timestamp for initial check...");
            let mut should_queue = true;
            if let Some(last_iso) = rt.block_on(store.get_last_update_check())
                && let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&last_iso)
            {
                let last_utc = parsed.with_timezone(&chrono::Utc);
                if chrono::Utc::now().signed_duration_since(last_utc) < chrono::Duration::days(1) {
                    should_queue = false;
                }
            }
            if should_queue {
                // Persist immediately to prevent multiple queues in rapid restarts
                rt.block_on(store.set_last_update_check_now());
                let _ = sender.send(models::enums::BackgroundTask::CheckForUpdates);
            }
        }
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
                                Ok(rt) => rt.block_on(crate::connection::refresh_connection_background_async(
                                    connection_id,
                                    &Some(cache_pool_arc.clone()),
                                )),
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
                }
            }
        });
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

    fn get_connection_name(&self, connection_id: i64) -> Option<String> {
        self.connections
            .iter()
            .find(|conn| conn.id == Some(connection_id))
            .map(|conn| conn.name.clone())
    }

    fn render_tree(
        &mut self,
        ui: &mut egui::Ui,
        nodes: &mut [models::structs::TreeNode],
        is_search_mode: bool,
    ) -> Vec<(String, String, String)> {
        let mut expansion_requests = Vec::new();
        let mut tables_to_expand = Vec::new();
        let mut context_menu_requests = Vec::new();
        let mut table_click_requests = Vec::new();
        let mut connection_click_requests = Vec::new();
        let mut index_click_requests: Vec<(i64, String, Option<String>, Option<String>)> =
            Vec::new();
        let mut create_index_requests: Vec<(i64, Option<String>, Option<String>)> = Vec::new();
        let mut query_files_to_open = Vec::new();

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
                dba_click_request,
                index_click_request,
                create_index_request,
            ) = Self::render_tree_node_with_table_expansion(
                ui,
                node,
                &mut self.editor,
                index,
                &self.refreshing_connections,
                is_search_mode,
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
            if let Some((connection_id, table_name)) = table_click_request {
                table_click_requests.push((connection_id, table_name));
            }
            if let Some(connection_id) = connection_click_request {
                connection_click_requests.push(connection_id);
            }
            if let Some((filename, content, file_path)) = query_file_to_open {
                query_files_to_open.push((filename, content, file_path));
            }
            // Collect DBA quick view requests
            if let Some((conn_id, node_type)) = dba_click_request {
                // Handle immediately here since we have &mut self
                if let Some(conn) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(conn_id))
                    .cloned()
                {
                    if let Some((tab_title, query_content)) =
                        self.build_dba_query(&conn, &node_type)
                    {
                        editor::create_new_tab_with_connection(
                            self,
                            tab_title.clone(),
                            query_content.clone(),
                            Some(conn_id),
                        );
                        self.current_connection_id = Some(conn_id);
                        if let Some((headers, data)) =
                            connection::execute_query_with_connection(self, conn_id, query_content)
                        {
                            self.current_table_headers = headers;
                            self.current_table_data = data.clone();
                            self.all_table_data = data;
                            self.current_table_name = tab_title;
                            self.is_table_browse_mode = false; // Disable filter for manual queries
                            self.total_rows = self.all_table_data.len();
                            self.current_page = 0;
                        }
                    } else {
                        self.error_message =
                            "DBA view not supported for this database type".to_string();
                        self.show_error_message = true;
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
        }

        // Handle connection clicks (create new tab with that connection)
        // We'll collect connection IDs needing eager pool creation to process after loop
        let mut pools_to_create: Vec<i64> = Vec::new();
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
        if !pools_to_create.is_empty() {
            // We'll create a temporary runtime once (not per connection) to run async pool creations.
            if let Ok(temp_rt) = tokio::runtime::Runtime::new() {
                // Collect needed connection configs first to avoid borrowing self mutably inside block_on closure repeatedly.
                let mut configs: Vec<(i64, models::structs::ConnectionConfig)> = Vec::new();
                for cid in &pools_to_create {
                    if let Some(cfg) = self
                        .connections
                        .iter()
                        .find(|c| c.id == Some(*cid))
                        .cloned()
                    {
                        configs.push((*cid, cfg));
                    }
                }
                for (cid, cfg) in configs.into_iter() {
                    if self.connection_pools.contains_key(&cid) {
                        continue;
                    }
                    let result_pool = temp_rt.block_on(async {
                        match cfg.connection_type {
                            models::enums::DatabaseType::MySQL => {
                                let encoded_username = crate::modules::url_encode(&cfg.username);
                                let encoded_password = crate::modules::url_encode(&cfg.password);
                                let conn_str = format!(
                                    "mysql://{}:{}@{}:{}/{}",
                                    encoded_username,
                                    encoded_password,
                                    cfg.host,
                                    cfg.port,
                                    cfg.database
                                );
                                match sqlx::mysql::MySqlPoolOptions::new()
                                    .max_connections(5)
                                    .min_connections(1)
                                    .acquire_timeout(std::time::Duration::from_secs(15))
                                    .test_before_acquire(true)
                                    .connect(&conn_str)
                                    .await
                                {
                                    Ok(pool) => Some(models::enums::DatabasePool::MySQL(
                                        std::sync::Arc::new(pool),
                                    )),
                                    Err(e) => {
                                        debug!("MySQL eager connect failed ({}): {}", cid, e);
                                        None
                                    }
                                }
                            }
                            models::enums::DatabaseType::PostgreSQL => {
                                let conn_str = format!(
                                    "postgresql://{}:{}@{}:{}/{}",
                                    cfg.username, cfg.password, cfg.host, cfg.port, cfg.database
                                );
                                match sqlx::postgres::PgPoolOptions::new()
                                    .max_connections(5)
                                    .min_connections(1)
                                    .acquire_timeout(std::time::Duration::from_secs(15))
                                    .connect(&conn_str)
                                    .await
                                {
                                    Ok(pool) => Some(models::enums::DatabasePool::PostgreSQL(
                                        std::sync::Arc::new(pool),
                                    )),
                                    Err(e) => {
                                        debug!("PostgreSQL eager connect failed ({}): {}", cid, e);
                                        None
                                    }
                                }
                            }
                            models::enums::DatabaseType::SQLite => {
                                let conn_str = format!("sqlite:{}", cfg.host);
                                match sqlx::sqlite::SqlitePoolOptions::new()
                                    .max_connections(3)
                                    .min_connections(1)
                                    .acquire_timeout(std::time::Duration::from_secs(15))
                                    .connect(&conn_str)
                                    .await
                                {
                                    Ok(pool) => Some(models::enums::DatabasePool::SQLite(
                                        std::sync::Arc::new(pool),
                                    )),
                                    Err(e) => {
                                        debug!("SQLite eager connect failed ({}): {}", cid, e);
                                        None
                                    }
                                }
                            }
                            models::enums::DatabaseType::Redis => {
                                let conn_str = if cfg.password.is_empty() {
                                    format!("redis://{}:{}", cfg.host, cfg.port)
                                } else {
                                    format!(
                                        "redis://{}:{}@{}:{}",
                                        cfg.username, cfg.password, cfg.host, cfg.port
                                    )
                                };
                                match redis::Client::open(conn_str) {
                                    Ok(client) => match redis::aio::ConnectionManager::new(client)
                                        .await
                                    {
                                        Ok(m) => Some(models::enums::DatabasePool::Redis(
                                            std::sync::Arc::new(m),
                                        )),
                                        Err(e) => {
                                            debug!("Redis eager connect failed ({}): {}", cid, e);
                                            None
                                        }
                                    },
                                    Err(e) => {
                                        debug!("Redis client build failed ({}): {}", cid, e);
                                        None
                                    }
                                }
                            }
                            models::enums::DatabaseType::MongoDB => {
                                let uri = if cfg.username.is_empty() {
                                    format!("mongodb://{}:{}", cfg.host, cfg.port)
                                } else if cfg.password.is_empty() {
                                    format!("mongodb://{}@{}:{}", cfg.username, cfg.host, cfg.port)
                                } else {
                                    let enc_user = crate::modules::url_encode(&cfg.username);
                                    let enc_pass = crate::modules::url_encode(&cfg.password);
                                    format!(
                                        "mongodb://{}:{}@{}:{}",
                                        enc_user, enc_pass, cfg.host, cfg.port
                                    )
                                };
                                match mongodb::Client::with_uri_str(uri).await {
                                    Ok(client) => Some(models::enums::DatabasePool::MongoDB(
                                        std::sync::Arc::new(client),
                                    )),
                                    Err(e) => {
                                        debug!("MongoDB eager connect failed ({}): {}", cid, e);
                                        None
                                    }
                                }
                            }
                            models::enums::DatabaseType::MsSQL => {
                                // For MsSQL we store config wrapper only (no network call yet) to keep behavior consistent.
                                let cfgw = crate::driver_mssql::MssqlConfigWrapper::new(
                                    cfg.host.clone(),
                                    cfg.port.clone(),
                                    cfg.database.clone(),
                                    cfg.username.clone(),
                                    cfg.password.clone(),
                                );
                                Some(models::enums::DatabasePool::MsSQL(std::sync::Arc::new(
                                    cfgw,
                                )))
                            }
                        }
                    });
                    if let Some(pool) = result_pool {
                        self.connection_pools.insert(cid, pool);
                        debug!("🔌 Eager pool created for connection {}", cid);
                    } else {
                        debug!("⚠️ Eager pool creation failed for connection {}", cid);
                    }
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
        for (connection_id, table_name) in table_click_requests {
            // Find the connection to determine the database type and database name
            let connection = self
                .connections
                .iter()
                .find(|conn| conn.id == Some(connection_id))
                .cloned();

            if let Some(conn) = connection {
                // Find the database name from the tree structure
                let mut database_name: Option<String> = None;
                for node in nodes.iter() {
                    if let Some(db_name) =
                        Tabular::find_database_name_for_table(node, connection_id, &table_name)
                    {
                        database_name = Some(db_name);
                        break;
                    }
                }

                // If no database found in tree, use connection default
                if database_name.is_none() {
                    database_name = Some(conn.database.clone());
                }

                match conn.connection_type {
                    models::enums::DatabaseType::Redis => {
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

                        let tab_title = format!("Table: {}", table_name);
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
                        self.current_table_name = format!(
                            "Table: {} (Database: {})",
                            table_name,
                            database_name.as_deref().unwrap_or("Unknown")
                        );
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
                                println!("================== A ============================ ");
                                debug!("🚀 Taking server-side pagination path");
                                info!(
                                    "🌐 Loading live data from server for table {}/{} (server pagination)",
                                    database_name.clone().unwrap_or_default(),
                                    table_name
                                );
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
            } else if context_id >= 30000 {
                // ID >= 30000 means alter table operation
                let connection_id = context_id - 30000;
                debug!("🔧 Alter table operation for connection: {}", connection_id);
                self.handle_alter_table_request(connection_id);
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

                    // Force immediate tree refresh and UI update
                    self.items_tree.clear();
                    sidebar_database::refresh_connections_tree(self);
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
        node_index: usize,
        refreshing_connections: &std::collections::HashSet<i64>,
        is_search_mode: bool,
    ) -> models::structs::RenderTreeNodeResult {
        let has_children = !node.children.is_empty();
        let mut expansion_request = None;
        let mut table_expansion = None;
        let mut context_menu_request = None;
        let mut table_click_request = None;
        let mut folder_removal_mapping: Option<(i64, String)> = None;
        let mut connection_click_request = None;
        let mut query_file_to_open = None;
        let mut folder_name_for_removal = None;
        let mut parent_folder_for_creation = None;
        let mut dba_click_request: Option<(i64, models::enums::NodeType)> = None;
        let mut index_click_request: Option<(i64, String, Option<String>, Option<String>)> = None;
        let mut create_index_request: Option<(i64, Option<String>, Option<String>)> = None;

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
                    format!("conn_{}_{}", node_index, node.connection_id.unwrap_or(0))
                }
                _ => format!("node_{}_{:?}", node_index, node.node_type),
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
                        && ((!node.is_loaded) || is_search_mode)
                        && let Some(conn_id) = node.connection_id
                    {
                        // Use stored raw table_name if present; otherwise sanitize display name (strip emojis / annotations)
                        let raw_name = node
                            .table_name
                            .clone()
                            .unwrap_or_else(|| Self::sanitize_display_table_name(&node.name));
                        table_expansion = Some((node_index, conn_id, raw_name));
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
                    models::enums::NodeType::Index => "#",
                    models::enums::NodeType::Query => "🔍",
                    models::enums::NodeType::QueryHistItem => "📜",
                    models::enums::NodeType::Connection => "",
                    models::enums::NodeType::DatabasesFolder => "📁",
                    models::enums::NodeType::TablesFolder => "📋",
                    models::enums::NodeType::ViewsFolder => "👁",
                    models::enums::NodeType::StoredProceduresFolder => "⚙️",
                    models::enums::NodeType::UserFunctionsFolder => "🔧",
                    models::enums::NodeType::TriggersFolder => "⚡",
                    models::enums::NodeType::EventsFolder => "📅",
                    models::enums::NodeType::DBAViewsFolder => "⚙️",
                    models::enums::NodeType::UsersFolder => "👥",
                    models::enums::NodeType::PrivilegesFolder => "🔒",
                    models::enums::NodeType::ProcessesFolder => "⚡",
                    models::enums::NodeType::StatusFolder => "📊",
                    models::enums::NodeType::MetricsUserActiveFolder => "👨‍💼",
                    models::enums::NodeType::View => "👁",
                    models::enums::NodeType::StoredProcedure => "⚙️",
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
                    models::enums::NodeType::MsSQLFolder => "🧰",
                };

                let label_text = if icon.is_empty() {
                    // For connection nodes, add loading indicator if refreshing
                    if node.node_type == models::enums::NodeType::Connection {
                        if let Some(conn_id) = node.connection_id {
                            if refreshing_connections.contains(&conn_id) {
                                format!("{} 🔄", node.name) // Add refresh spinner
                            } else {
                                node.name.clone()
                            }
                        } else {
                            node.name.clone()
                        }
                    } else {
                        node.name.clone()
                    }
                } else {
                    format!("{} {}", icon, node.name)
                };
                let response = if node.node_type == models::enums::NodeType::Connection {
                    // Use button for connections to make them more clickable
                    ui.button(&label_text)
                } else {
                    ui.label(label_text)
                };

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
                    && response.clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    // Use table_name field if available (for search results), otherwise use node.name
                    let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name).clone();
                    table_click_request = Some((conn_id, actual_table_name));
                }

                // Index items: no left-click action; use context menu for Alter Index

                // Add context menu for connection nodes
                if node.node_type == models::enums::NodeType::Connection {
                    response.context_menu(|ui| {
                        if ui.button("Copy Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 10000); // Use +10000 to indicate copy
                            }
                            ui.close();
                        }
                        if ui.button("Refresh Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use +1000 range to indicate refresh (handled in render_tree handler)
                                context_menu_request = Some(conn_id + 1000);
                            }
                            ui.close();
                        }
                        if ui.button("Edit Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id);
                            }
                            ui.close();
                        }
                        if ui.button("Remove Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(-conn_id); // Negative ID indicates removal
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

                // Add context menu for table nodes
                if node.node_type == models::enums::NodeType::Table {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                table_click_request = Some((conn_id, actual_table_name));
                            }
                            ui.close();
                        }
                        if ui.button("📋 SELECT Query (New Tab)").clicked() {
                            // We'll create a new tab instead of modifying current editor
                            // Store the request and handle it in render_tree
                            ui.close();
                        }
                        if ui.button("🔍 COUNT Query (Current Tab)").clicked() {
                            let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name);
                            editor.set_text(format!("SELECT COUNT(*) FROM {};", actual_table_name));
                            editor.mark_text_modified();
                            ui.close();
                        }
                        if ui.button("📝 DESCRIBE Query (Current Tab)").clicked() {
                            let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name);
                            // Different DESCRIBE syntax for different database types
                            if node.database_name.is_some() {
                                editor.set_text(format!("DESCRIBE {};", actual_table_name));
                            } else {
                                editor
                                    .set_text(format!("PRAGMA table_info({});", actual_table_name)); // SQLite syntax
                            }
                            editor.mark_text_modified();
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
                        if ui.button("🔧 Alter Table").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use connection_id + 30000 to indicate alter table request
                                context_menu_request = Some(conn_id + 30000);
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
                                table_click_request = Some((conn_id, actual_table_name));
                            }
                            ui.close();
                        }
                        if ui.button("📋 SELECT Query (New Tab)").clicked() {
                            // We'll create a new tab instead of modifying current editor
                            // Store the request and handle it in render_tree
                            ui.close();
                        }
                        if ui.button("🔍 COUNT Query (Current Tab)").clicked() {
                            let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name);
                            editor.set_text(format!("SELECT COUNT(*) FROM {};", actual_table_name));
                            editor.mark_text_modified();
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
                        ) = Self::render_tree_node_with_table_expansion(
                            ui,
                            child,
                            editor,
                            child_index,
                            refreshing_connections,
                            is_search_mode,
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
                        if let Some((conn_id, table_name)) = child_table_click {
                            table_click_request = Some((conn_id, table_name));
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
                        if let Some(child_context_id) = child_context {
                            context_menu_request = Some(child_context_id);
                        }
                        // Propagate child query file open requests (History) to parent
                        if let Some(child_query_file) = _child_query_file {
                            query_file_to_open = Some(child_query_file);
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
                            ) = Self::render_tree_node_with_table_expansion(
                                ui,
                                child,
                                editor,
                                child_index,
                                refreshing_connections,
                                is_search_mode,
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
                            if let Some((conn_id, table_name)) = child_table_click {
                                table_click_request = Some((conn_id, table_name));
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
                // For all other node types, use horizontal layout with icons
                ui.horizontal(|ui| {
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
                        models::enums::NodeType::StoredProceduresFolder => "⚙️",
                        models::enums::NodeType::UserFunctionsFolder => "🔧",
                        models::enums::NodeType::TriggersFolder => "⚡",
                        models::enums::NodeType::EventsFolder => "📅",
                        models::enums::NodeType::DBAViewsFolder => "⚙️",
                        models::enums::NodeType::UsersFolder => "👥",
                        models::enums::NodeType::PrivilegesFolder => "🔒",
                        models::enums::NodeType::ProcessesFolder => "⚡",
                        models::enums::NodeType::StatusFolder => "📊",
                        models::enums::NodeType::View => "👁",
                        models::enums::NodeType::StoredProcedure => "⚙️",
                        models::enums::NodeType::UserFunction => "🔧",
                        models::enums::NodeType::Trigger => "⚡",
                        models::enums::NodeType::Event => "📅",
                        models::enums::NodeType::MySQLFolder => "🐬",
                        models::enums::NodeType::PostgreSQLFolder => "🐘",
                        models::enums::NodeType::SQLiteFolder => "📄",
                        models::enums::NodeType::RedisFolder => "🔴",
                        models::enums::NodeType::CustomFolder => "📁",
                        models::enums::NodeType::QueryFolder => "📂",
                        models::enums::NodeType::HistoryDateFolder => "📅",
                        _ => "❓",
                    };

                    ui.button(format!("{} {}", icon, node.name))
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
                            table_click_request = Some((conn_id, actual_table_name));
                        }
                    }
                    // DBA quick views: emit a click request to be handled by parent (needs self)
                    models::enums::NodeType::UsersFolder
                    | models::enums::NodeType::PrivilegesFolder
                    | models::enums::NodeType::ProcessesFolder
                    | models::enums::NodeType::StatusFolder
                    | models::enums::NodeType::MetricsUserActiveFolder => {
                        if let Some(conn_id) = node.connection_id {
                            dba_click_request = Some((conn_id, node.node_type.clone()));
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

    // Build standard DBA queries for quick views based on db type and node kind
    fn build_dba_query(
        &self,
        connection: &models::structs::ConnectionConfig,
        node_type: &models::enums::NodeType,
    ) -> Option<(String, String)> {
        use models::enums::{DatabaseType, NodeType};
        match connection.connection_type {
            DatabaseType::MySQL => {
                match node_type {
                    NodeType::UsersFolder => Some((
                        format!("DBA: MySQL Users - {}", connection.name),
                        "SELECT Host, User, plugin, account_locked, password_expired, password_last_changed \
FROM mysql.user ORDER BY User, Host;".to_string()
                    )),
                    NodeType::PrivilegesFolder => Some((
                        format!("DBA: MySQL Privileges - {}", connection.name),
                        "SELECT GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USER_PRIVILEGES \
ORDER BY GRANTEE, PRIVILEGE_TYPE;".to_string()
                    )),
                    NodeType::ProcessesFolder => Some((
                        format!("DBA: MySQL Processlist - {}", connection.name),
                        "SHOW FULL PROCESSLIST;".to_string()
                    )),
                    NodeType::StatusFolder => Some((
                        format!("DBA: MySQL Global Status - {}", connection.name),
                        "SHOW GLOBAL STATUS;".to_string()
                    )),
                    NodeType::MetricsUserActiveFolder => Some((
                        format!("DBA: MySQL User Active - {}", connection.name),
                        "SELECT USER, COUNT(*) AS session_count FROM information_schema.PROCESSLIST GROUP BY USER ORDER BY session_count DESC;".to_string()
                    )),
                    _ => None,
                }
            }
            DatabaseType::PostgreSQL => {
                match node_type {
                    NodeType::UsersFolder => Some((
                        format!("DBA: PostgreSQL Users - {}", connection.name),
                        "SELECT usename AS user, usesysid, usecreatedb, usesuper FROM pg_user ORDER BY usename;".to_string()
                    )),
                    NodeType::PrivilegesFolder => Some((
                        format!("DBA: PostgreSQL Privileges - {}", connection.name),
                        "SELECT grantee, table_catalog, table_schema, table_name, privilege_type \
FROM information_schema.table_privileges ORDER BY grantee, table_schema, table_name;".to_string()
                    )),
                    NodeType::ProcessesFolder => Some((
                        format!("DBA: PostgreSQL Activity - {}", connection.name),
                        "SELECT pid, usename, application_name, client_addr, state, query_start, query FROM pg_stat_activity ORDER BY query_start DESC NULLS LAST;".to_string()
                    )),
                    NodeType::StatusFolder => Some((
                        format!("DBA: PostgreSQL Settings - {}", connection.name),
                        "SELECT name, setting FROM pg_settings ORDER BY name;".to_string()
                    )),
                    NodeType::MetricsUserActiveFolder => Some((
                        format!("DBA: PostgreSQL User Active - {}", connection.name),
                        "SELECT usename AS user, COUNT(*) AS session_count FROM pg_stat_activity GROUP BY usename ORDER BY session_count DESC;".to_string()
                    )),
                    _ => None,
                }
            }
            DatabaseType::MsSQL => {
                match node_type {
                    NodeType::UsersFolder => Some((
                        format!("DBA: MsSQL Principals - {}", connection.name),
                        "SELECT name, type_desc, create_date, modify_date FROM sys.server_principals \
WHERE type IN ('S','U','G') AND name NOT LIKE '##MS_%' ORDER BY name;".to_string()
                    )),
                    NodeType::PrivilegesFolder => Some((
                        format!("DBA: MsSQL Server Permissions - {}", connection.name),
                        "SELECT dp.name AS principal_name, sp.permission_name, sp.state_desc \
FROM sys.server_permissions sp \
JOIN sys.server_principals dp ON sp.grantee_principal_id = dp.principal_id \
ORDER BY dp.name, sp.permission_name;".to_string()
                    )),
                    NodeType::ProcessesFolder => Some((
                        format!("DBA: MsSQL Sessions - {}", connection.name),
                        "SELECT session_id, login_name, host_name, status, program_name, cpu_time, memory_usage \
FROM sys.dm_exec_sessions ORDER BY cpu_time DESC;".to_string()
                    )),
                    NodeType::StatusFolder => Some((
                        format!("DBA: MsSQL Performance Counters - {}", connection.name),
                        "SELECT TOP 200 counter_name, instance_name, cntr_value FROM sys.dm_os_performance_counters ORDER BY counter_name;".to_string()
                    )),
                    NodeType::MetricsUserActiveFolder => Some((
                        format!("DBA: MsSQL User Active - {}", connection.name),
                        "SELECT login_name AS [user], COUNT(*) AS session_count FROM sys.dm_exec_sessions GROUP BY login_name ORDER BY session_count DESC;".to_string()
                    )),
                    _ => None,
                }
            }
            DatabaseType::SQLite | DatabaseType::Redis | DatabaseType::MongoDB => None,
        }
    }

    fn handle_alter_table_request(&mut self, connection_id: i64) {
        debug!(
            "🔍 handle_alter_table_request called with connection_id: {}",
            connection_id
        );

        // Find the connection by ID to determine database type
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            // Find the currently selected table in the tree
            if let Some(table_name) = self.find_selected_table_name(connection_id) {
                // Generate ALTER TABLE template based on database type
                let alter_template = match connection.connection_type {
                    models::enums::DatabaseType::MySQL => self.generate_mysql_alter_table_template(&table_name),
                    models::enums::DatabaseType::PostgreSQL => self.generate_postgresql_alter_table_template(&table_name),
                    models::enums::DatabaseType::SQLite => self.generate_sqlite_alter_table_template(&table_name),
                    models::enums::DatabaseType::Redis => "-- Redis does not support ALTER TABLE operations\n-- Redis is a key-value store, not a relational database".to_string(),
                    models::enums::DatabaseType::MsSQL => self.generate_mysql_alter_table_template(&table_name).replace("MySQL", "MsSQL"),
                    models::enums::DatabaseType::MongoDB => "-- MongoDB collections are schemaless; ALTER TABLE not applicable".to_string(),
                };

                // Set the ALTER TABLE template in the editor
                self.editor.text = alter_template;
                self.current_connection_id = Some(connection_id);
            } else {
                // If no specific table is selected, show a generic template
                let alter_template = match connection.connection_type {
                    models::enums::DatabaseType::MySQL => "-- MySQL ALTER TABLE template\nALTER TABLE your_table_name\n  ADD COLUMN new_column VARCHAR(255),\n  MODIFY COLUMN existing_column INT,\n  DROP COLUMN old_column;".to_string(),
                    models::enums::DatabaseType::PostgreSQL => "-- PostgreSQL ALTER TABLE template\nALTER TABLE your_table_name\n  ADD COLUMN new_column VARCHAR(255),\n  ALTER COLUMN existing_column TYPE INTEGER,\n  DROP COLUMN old_column;".to_string(),
                    models::enums::DatabaseType::SQLite => "-- SQLite ALTER TABLE template\n-- Note: SQLite has limited ALTER TABLE support\nALTER TABLE your_table_name\n  ADD COLUMN new_column TEXT;".to_string(),
                    models::enums::DatabaseType::Redis => "-- Redis does not support ALTER TABLE operations\n-- Redis is a key-value store, not a relational database\n-- Use Redis commands like SET, GET, HSET, etc.".to_string(),
                    models::enums::DatabaseType::MsSQL => "-- MsSQL ALTER TABLE template\nALTER TABLE your_table_name\n  ADD new_column VARCHAR(255) NULL,\n  ALTER COLUMN existing_column INT,\n  DROP COLUMN old_column;".to_string(),
                    models::enums::DatabaseType::MongoDB => "-- MongoDB does not support ALTER TABLE; modify documents with update operators".to_string(),
                };

                self.editor.text = alter_template;
                self.current_connection_id = Some(connection_id);
            }
        } else {
            debug!("❌ Connection with ID {} not found", connection_id);
        }
    }

    fn find_selected_table_name(&self, _connection_id: i64) -> Option<String> {
        // This is a simplified approach - in a more sophisticated implementation,
        // you might track which table was right-clicked
        // For now, we'll return None to show the generic template
        None
    }

    fn generate_mysql_alter_table_template(&self, table_name: &str) -> String {
        format!(
            "-- MySQL ALTER TABLE for {}\nALTER TABLE {}\n  ADD COLUMN new_column VARCHAR(255) DEFAULT NULL COMMENT 'New column description',\n  MODIFY COLUMN existing_column INT NOT NULL,\n  DROP COLUMN old_column,\n  ADD INDEX idx_new_column (new_column);",
            table_name, table_name
        )
    }

    fn generate_postgresql_alter_table_template(&self, table_name: &str) -> String {
        format!(
            "-- PostgreSQL ALTER TABLE for {}\nALTER TABLE {}\n  ADD COLUMN new_column VARCHAR(255) DEFAULT NULL,\n  ALTER COLUMN existing_column TYPE INTEGER,\n  DROP COLUMN old_column;\n\n-- Add constraint example\n-- ALTER TABLE {} ADD CONSTRAINT chk_constraint CHECK (new_column IS NOT NULL);",
            table_name, table_name, table_name
        )
    }

    fn generate_sqlite_alter_table_template(&self, table_name: &str) -> String {
        format!(
            "-- SQLite ALTER TABLE for {}\n-- Note: SQLite has limited ALTER TABLE support\n-- Only ADD COLUMN and RENAME operations are supported\n\nALTER TABLE {} ADD COLUMN new_column TEXT DEFAULT NULL;\n\n-- To modify or drop columns, you need to recreate the table:\n-- CREATE TABLE {}_new AS SELECT existing_columns FROM {};\n-- DROP TABLE {};\n-- ALTER TABLE {}_new RENAME TO {};",
            table_name, table_name, table_name, table_name, table_name, table_name, table_name
        )
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
                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    // Users
                    let mut users_folder = models::structs::TreeNode::new(
                        "Users".to_string(),
                        models::enums::NodeType::UsersFolder,
                    );
                    users_folder.connection_id = Some(connection_id);
                    users_folder.is_loaded = false;
                    dba_children.push(users_folder);

                    // Privileges
                    let mut priv_folder = models::structs::TreeNode::new(
                        "Privileges".to_string(),
                        models::enums::NodeType::PrivilegesFolder,
                    );
                    priv_folder.connection_id = Some(connection_id);
                    priv_folder.is_loaded = false;
                    dba_children.push(priv_folder);

                    // Processes
                    let mut proc_folder = models::structs::TreeNode::new(
                        "Processes".to_string(),
                        models::enums::NodeType::ProcessesFolder,
                    );
                    proc_folder.connection_id = Some(connection_id);
                    proc_folder.is_loaded = false;
                    dba_children.push(proc_folder);

                    // Status
                    let mut status_folder = models::structs::TreeNode::new(
                        "Status".to_string(),
                        models::enums::NodeType::StatusFolder,
                    );
                    status_folder.connection_id = Some(connection_id);
                    status_folder.is_loaded = false;
                    dba_children.push(status_folder);

                    // User Active
                    let mut metrics_user_active_folder = models::structs::TreeNode::new(
                        "User Active".to_string(),
                        models::enums::NodeType::MetricsUserActiveFolder,
                    );
                    metrics_user_active_folder.connection_id = Some(connection_id);
                    metrics_user_active_folder.is_loaded = false;
                    dba_children.push(metrics_user_active_folder);

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

                    // DBA Views folder similar to MySQL
                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();
                    let mut users_folder = models::structs::TreeNode::new(
                        "Users".to_string(),
                        models::enums::NodeType::UsersFolder,
                    );
                    users_folder.connection_id = Some(connection_id);
                    users_folder.is_loaded = false;
                    dba_children.push(users_folder);

                    let mut priv_folder = models::structs::TreeNode::new(
                        "Privileges".to_string(),
                        models::enums::NodeType::PrivilegesFolder,
                    );
                    priv_folder.connection_id = Some(connection_id);
                    priv_folder.is_loaded = false;
                    dba_children.push(priv_folder);

                    let mut proc_folder = models::structs::TreeNode::new(
                        "Processes".to_string(),
                        models::enums::NodeType::ProcessesFolder,
                    );
                    proc_folder.connection_id = Some(connection_id);
                    proc_folder.is_loaded = false;
                    dba_children.push(proc_folder);

                    let mut status_folder = models::structs::TreeNode::new(
                        "Status".to_string(),
                        models::enums::NodeType::StatusFolder,
                    );
                    status_folder.connection_id = Some(connection_id);
                    status_folder.is_loaded = false;
                    dba_children.push(status_folder);

                    // User Active
                    let mut metrics_user_active_folder = models::structs::TreeNode::new(
                        "User Active".to_string(),
                        models::enums::NodeType::MetricsUserActiveFolder,
                    );
                    metrics_user_active_folder.connection_id = Some(connection_id);
                    metrics_user_active_folder.is_loaded = false;
                    dba_children.push(metrics_user_active_folder);

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

            let subfolders = vec![columns_folder, indexes_folder, pks_folder];

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
        let indexes_list = if let Some(names) = cache_data::get_index_names_from_cache(
            self,
            connection_id,
            database_name,
            table_name,
        ) {
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

            if let Some((headers, data)) =
                connection::execute_query_with_connection(self, connection_id, paginated_query)
            {
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
                self.current_table_data = data;
                // For server pagination, total_rows represents current page row count only (used for UI row count display)
                self.total_rows = self.current_table_data.len();
                // Sync ke tab aktif agar mode table tab (tanpa editor) bisa menampilkan Data
                if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                    active_tab.result_headers = self.current_table_headers.clone();
                    active_tab.result_rows = self.current_table_data.clone();
                    active_tab.result_all_rows = self.current_table_data.clone(); // single page snapshot
                    active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
                    active_tab.current_page = self.current_page;
                    active_tab.page_size = self.page_size;
                    active_tab.is_table_browse_mode = true;
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

        ui.separator();

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

            let _ = self.render_tree(ui, &mut items_tree, false);

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
        let string_color = egui::Color32::from_rgb(255, 60, 0); // Orange - strings
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
                    ui.colored_label(egui::Color32::RED, format!("Error: {}", error));
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
                    // Execute now
                    let result = crate::connection::execute_query_with_connection(
                        self,
                        conn_id,
                        queued.clone(),
                    );
                    self.apply_query_result(conn_id, queued, result);
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

        // Handle keyboard shortcuts (collect copy intent inside closure, execute after)
        let mut do_copy = false; // whether to perform copy after closure
        let mut copy_mode: u8 = 0; // 1=cell 2=rows 3=cols
        let mut snapshot_cell: Option<(usize, usize)> = None;
        let mut snapshot_value: Option<String> = None; // only for cell
        let mut snapshot_rows_csv: Option<String> = None;
        let mut snapshot_cols_csv: Option<String> = None;

        // Detect Save shortcut using consume_key so it works reliably on macOS/Windows/Linux
        let mut save_shortcut = false;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
            {
                save_shortcut = true;
                println!("🔥 Save shortcut detected!");
            }
        });

        ctx.input(|i| {
            // Detect Copy event (Cmd/Ctrl+C) which on some platforms (macOS) may emit Event::Copy instead of Key::C with modifiers
            let copy_event = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let key_combo =
                (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::C);
            if copy_event || key_combo {
                if let Some((r, c)) = self.selected_cell {
                    if let Some(row_vec) = self.current_table_data.get(r)
                        && c < row_vec.len()
                    {
                        snapshot_cell = Some((r, c));
                        snapshot_value = Some(row_vec[c].clone());
                        copy_mode = 1;
                        do_copy = true;
                    }
                } else if !self.selected_rows.is_empty() {
                    if let Some(csv) = data_table::copy_selected_rows_as_csv(self) {
                        snapshot_rows_csv = Some(csv);
                        copy_mode = 2;
                        do_copy = true;
                    }
                } else if !self.selected_columns.is_empty() {
                    if let Some(csv) = data_table::copy_selected_columns_as_csv(self) {
                        snapshot_cols_csv = Some(csv);
                        copy_mode = 3;
                        do_copy = true;
                    }
                } else {
                    debug!("⚠️ copy intent but no selection");
                }
            }
            // (Save shortcut is handled via consume_key above)

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

            // (No direct copy execution here; performed after closure)

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
                if let Some((row, col)) = self.selected_cell {
                    let max_rows = self.current_table_data.len();

                    if i.key_pressed(egui::Key::ArrowRight) {
                        // Check the current row's column count for bounds
                        if let Some(current_row) = self.current_table_data.get(row)
                            && col + 1 < current_row.len()
                        {
                            self.selected_cell = Some((row, col + 1));
                            cell_changed = true;
                            self.scroll_to_selected_cell = true;
                            log::debug!("➡️ Arrow Right: Moving to ({}, {})", row, col + 1);
                        }
                    } else if i.key_pressed(egui::Key::ArrowLeft) && col > 0 {
                        self.selected_cell = Some((row, col - 1));
                        cell_changed = true;
                        self.scroll_to_selected_cell = true;
                        log::debug!("⬅️ Arrow Left: Moving to ({}, {})", row, col - 1);
                    } else if i.key_pressed(egui::Key::ArrowDown) && row + 1 < max_rows {
                        // Check if the target row has enough columns
                        if let Some(target_row) = self.current_table_data.get(row + 1) {
                            let target_col = col.min(target_row.len().saturating_sub(1));
                            self.selected_cell = Some((row + 1, target_col));
                            cell_changed = true;
                            self.scroll_to_selected_cell = true;
                            log::debug!("⬇️ Arrow Down: Moving to ({}, {})", row + 1, target_col);
                        }
                    } else if i.key_pressed(egui::Key::ArrowUp) && row > 0 {
                        // Check if the target row has enough columns
                        if let Some(target_row) = self.current_table_data.get(row - 1) {
                            let target_col = col.min(target_row.len().saturating_sub(1));
                            self.selected_cell = Some((row - 1, target_col));
                            cell_changed = true;
                            self.scroll_to_selected_cell = true;
                            log::debug!("⬆️ Arrow Up: Moving to ({}, {})", row - 1, target_col);
                        }
                    }

                    // Update selected_row when cell changes
                    if cell_changed && let Some((new_row, _)) = self.selected_cell {
                        self.selected_row = Some(new_row);
                    }
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
                    self.last_clicked_row = None;
                    self.last_clicked_column = None;
                }
            }
        });

        // Execute deferred copy outside of input closure to avoid potential stalls during event processing
        if do_copy {
            match copy_mode {
                1 => {
                    if let (Some((r, c)), Some(val)) = (snapshot_cell, snapshot_value) {
                        ctx.copy_text(val.clone());
                        debug!("📋 Copied cell (r{},c{}) len={} chars", r, c, val.len());
                    }
                }
                2 => {
                    if let Some(csv) = snapshot_rows_csv {
                        ctx.copy_text(csv.clone());
                        debug!(
                            "📋 Copied {} row(s) CSV ({} chars)",
                            self.selected_rows.len(),
                            csv.len()
                        );
                    }
                }
                3 => {
                    if let Some(csv) = snapshot_cols_csv {
                        ctx.copy_text(csv.clone());
                        debug!(
                            "📋 Copied {} col(s) CSV ({} chars)",
                            self.selected_columns.len(),
                            csv.len()
                        );
                    }
                }
                _ => {}
            }
        }

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
                        let accent = if self.is_dark_mode { egui::Color32::from_rgb(255, 60, 0) } else { egui::Color32::from_rgb(180,30,30) };
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
                            debug!(
                                "Background refresh completed successfully for connection {}",
                                connection_id
                            );
                            // Re-expand connection node to show fresh data
                            for node in &mut self.items_tree {
                                if node.node_type == models::enums::NodeType::Connection
                                    && node.connection_id == Some(connection_id)
                                {
                                    node.is_loaded = false; // Force reload from cache
                                    // Don't auto-expand after refresh, let user manually expand
                                    break;
                                }
                            }
                            // Request UI repaint to show updated data
                            ctx.request_repaint();
                        } else {
                            debug!("Background refresh failed for connection {}", connection_id);
                        }
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
            style.visuals.selection.bg_fill = egui::Color32::from_rgb(255, 60, 0);
            style.visuals.selection.stroke.color = egui::Color32::from_rgb(0, 0, 0);

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
        sidebar_query::render_create_folder_dialog(self, ctx);
        sidebar_query::render_move_to_folder_dialog(self, ctx);
        // Update dialog
        self.render_update_dialog(ctx);

        // Persist preferences if dirty and config store ready (outside of window render to avoid borrow issues)
        // Final attempt (in case any change slipped through)
        try_save_prefs(self);

        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(250.0)
            .min_width(150.0)
            .max_width(500.0)
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
                            .fill(egui::Color32::from_rgb(255, 60, 0))
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
                            .fill(egui::Color32::from_rgb(255, 60, 0)) // Orange fill for active
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
                            .fill(egui::Color32::from_rgb(255, 60, 0)) // Orange fill for active
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

                                for (filename, content, file_path) in query_files_to_open {
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
                                // Render history tree and process clicks into new tabs
                                let mut history_tree = std::mem::take(&mut self.history_tree);
                                let query_files_to_open = self.render_tree(ui, &mut history_tree, false);
                                self.history_tree = history_tree;

                                for (filename, content, file_data) in query_files_to_open {
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
                        ui.separator();

                        match self.selected_menu.as_str() {
                            "Database" => {
                                if ui
                                    .add_sized(
                                        [24.0, 24.0], // Small square button
                                        egui::Button::new("➕").fill(egui::Color32::RED),
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
                            // "Queries" => {
                            //     if ui.add_sized(
                            //         [24.0, 24.0], // Small square button
                            //         egui::Button::new("➕")
                            //             .fill(egui::Color32::RED)
                            //     ).on_hover_text("New Query File").clicked() {
                            //         // Create new tab instead of clearing editor
                            //         editor::create_new_tab(self, "Untitled Query".to_string(), String::new());
                            //     }
                            // },
                            _ => {
                                // No button for History tab
                            }
                        }
                    });
                });
            });

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
                        let mut to_close = None;
                        let mut to_switch = None;
                        for (i, tab) in self.query_tabs.iter().enumerate() {
                            let active = i == self.active_tab_index;
                            let color = if active {
                                egui::Color32::from_rgb(255, 60, 0)
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
                    .map(|t| t.title.starts_with("Table:") || t.title.starts_with("Collection:"))
                    .unwrap_or(false);

                if is_table_tab {
                    // Table tabs: Direct Data/Structure view without query editor
                    ui.vertical(|ui| {
                        // Data/Structure toggle at the top
                        ui.horizontal(|ui| {
                            let is_data =
                                self.table_bottom_view == models::structs::TableBottomView::Data;
                            if ui.selectable_label(is_data, "📊 Data").clicked() {
                                self.table_bottom_view = models::structs::TableBottomView::Data;
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
                            if ui.selectable_label(is_struct, "🏗 Structure").clicked() {
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
                        });

                        ui.separator();

                        // Main content area takes remaining space
                        let remaining_height = ui.available_height();
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), remaining_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                // Render Data or Structure based on toggle
                                if self.table_bottom_view
                                    == models::structs::TableBottomView::Structure
                                {
                                    data_table::render_structure_view(self, ui);
                                } else {
                                    data_table::render_table_data(self, ui);
                                }
                            },
                        );
                    });
                } else {
                    // Regular query tabs: editor on top, results below
                    // Query tab logic: show bottom panel if we have any headers/data, a status name/message, or tab executed at least once
                    // Use the exact remaining height; avoid artificial padding that caused visual gaps
                    let avail = ui.available_height();
                    let executed = self
                        .query_tabs
                        .get(self.active_tab_index)
                        .map(|t| t.has_executed_query)
                        .unwrap_or(false);
                    let has_headers = !self.current_table_headers.is_empty();
                    let has_message = !self.current_table_name.is_empty();
                    let show_bottom = has_headers || has_message || executed;
                    // Draggable splitter: when showing bottom, always reserve some min space; allow adjusting ratio
                    if show_bottom {
                        // Enforce bounds (allow editor to occupy almost full height if user drags)
                        // Previously max 0.9 made a visible "batasan bawah" (bottom limit) that felt restrictive.
                        // Increase to 0.995 so user can nearly hide result panel while still keeping a tiny handle.
                        self.table_split_ratio = self.table_split_ratio.clamp(0.05, 0.995);
                    }
                    let editor_h = if show_bottom {
                        let mut h = avail * self.table_split_ratio;
                        if has_headers {
                            // Old max limited editor height harshly (avail - 140). Reduce reserved space for results.
                            h = h.clamp(100.0, (avail - 50.0).max(100.0));
                        } else {
                            // When only status/message (no table), allow almost full height.
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
                            // Fixed-height container with internal scroll so long queries don't push result panel
                            // No run bar currently; let the editor fully occupy the reserved height to avoid a black gap
                            let editor_area_height = editor_h.max(200.0);
                            // Calculate how many rows fit and pass it to the editor to fill the space
                            let mono_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
                            let rows = ((editor_area_height / mono_h).floor() as i32) as usize;
                            self.advanced_editor.desired_rows = rows;
                            // Scrollable editor area
                            let avail_w = ui.available_width() - 4.0;
                            // Allocate a fixed rectangle then paint a vertical ScrollArea inside it so content doesn't expand layout
                            let desired = egui::vec2(avail_w, editor_area_height);
                            let (rect, _resp) =
                                ui.allocate_exact_size(desired, egui::Sense::hover());
                            let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                            egui::ScrollArea::vertical()
                                .id_salt("query_editor_scroll")
                                .auto_shrink([false, false])
                                .show(&mut child_ui, |ui| {
                                    // Constrain width to avoid horizontal grow
                                    ui.set_min_width(avail_w - 4.0);
                                    // Always render legacy editor
                                    editor::render_advanced_editor(self, ui);
                                });
                            
                            // Key shortcut check
                            if ui.input(|i| {
                                (i.modifiers.ctrl || i.modifiers.mac_cmd)
                                    && i.key_pressed(egui::Key::Enter)
                            }) {
                                let has_q = if !self.selected_text.trim().is_empty() {
                                    true
                                } else {
                                    let cq = editor::extract_query_from_cursor(self);
                                    !cq.trim().is_empty() || !self.editor.text.trim().is_empty()
                                };
                                if has_q {
                                    editor::execute_query(self);
                                }
                            }
                        });
                    if show_bottom {
                        // Draw draggable handle
                        let handle_id = ui.make_persistent_id("editor_table_splitter");
                        let desired_h = 6.0;
                        let available_w = ui.available_width();
                        let (rect, resp) = ui.allocate_at_least(
                            egui::vec2(available_w, desired_h),
                            egui::Sense::click_and_drag(),
                        );
                        let stroke = egui::Stroke::new(
                            1.0,
                            ui.visuals().widgets.noninteractive.fg_stroke.color,
                        );
                        ui.painter().hline(rect.x_range(), rect.center().y, stroke);
                        if resp.dragged() {
                            let drag_delta = resp.drag_delta().y;
                            if avail > 0.0 {
                                self.table_split_ratio =
                                    (self.table_split_ratio + (drag_delta / avail)).clamp(0.05, 0.995);
                            }
                            ui.memory_mut(|m| m.request_focus(handle_id));
                        }
                        ui.add_space(2.0);

                        // Regular query result display
                        data_table::render_table_data(self, ui);
                    }
                }

                data_table::render_drop_index_confirmation(self, ui.ctx());
                data_table::render_drop_column_confirmation(self, ui.ctx());
            });
    } // end update
} // end impl App for Tabular

// Helper to finalize query result display from a raw execution result
impl Tabular {
    fn apply_query_result(
        &mut self,
        connection_id: i64,
        query: String,
        result: Option<(Vec<String>, Vec<Vec<String>>)>,
    ) {
        // Mark active tab as executed
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.has_executed_query = true;
        }

        if let Some((headers, data)) = result {
            let is_error_result = headers.first().map(|h| h == "Error").unwrap_or(false);
            self.current_table_headers = headers;
            data_table::update_pagination_data(self, data);
            if self.total_rows == 0 {
                self.current_table_name = "Query executed successfully (no results)".to_string();
            } else {
                self.current_table_name = format!(
                    "Query Results ({} total rows, showing page {} of {})",
                    self.total_rows,
                    self.current_page + 1,
                    data_table::get_total_pages(self)
                );
            }

            // Set base query for pagination (simple LIMIT removal like in editor::execute_query)
            let base_query_for_pagination = if !is_error_result && self.total_rows > 0 {
                let mut clean_query = query.clone();
                if let Some(limit_pos) = clean_query.to_uppercase().rfind("LIMIT") {
                    if let Some(semicolon_pos) = clean_query[limit_pos..].find(';') {
                        clean_query = format!(
                            "{}{}",
                            &clean_query[..limit_pos].trim(),
                            &clean_query[limit_pos + semicolon_pos..]
                        );
                    } else {
                        clean_query = clean_query[..limit_pos].trim().to_string();
                    }
                }
                clean_query
            } else {
                String::new()
            };
            // Preserve pre-set base query for server-side pagination; otherwise use computed base
            if !self.use_server_pagination || self.current_base_query.is_empty() {
                self.current_base_query = base_query_for_pagination.clone();
            }

            // Save to history unless error
            if !is_error_result {
                crate::sidebar_history::save_query_to_history(self, &query, connection_id);
            }
            // Persist to active tab
            if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                tab.result_headers = self.current_table_headers.clone();
                tab.result_rows = self.current_table_data.clone();
                tab.result_all_rows = self.all_table_data.clone();
                tab.result_table_name = self.current_table_name.clone();
                tab.is_table_browse_mode = self.is_table_browse_mode;
                tab.current_page = self.current_page;
                tab.page_size = self.page_size;
                tab.total_rows = self.total_rows;
                // Preserve pre-set base query for server-side pagination; otherwise store computed base
                if !self.use_server_pagination || self.current_base_query.is_empty() {
                    tab.base_query = self.current_base_query.clone();
                }
            }
        } else {
            self.current_table_name = "Query execution failed".to_string();
            self.current_table_headers.clear();
            self.current_table_data.clear();
            self.all_table_data.clear();
            self.total_rows = 0;
            if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                tab.result_headers.clear();
                tab.result_rows.clear();
                tab.result_all_rows.clear();
                tab.result_table_name = self.current_table_name.clone();
                tab.total_rows = 0;
                tab.current_page = 0;
                tab.base_query.clear();
            }
        }
    }
}
