use eframe::egui;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::collections::{BTreeSet, HashMap};
use log::{debug, error};
use crate::models;
use crate::connection;
use crate::driver_redis;
use crate::{sidebar_database, sidebar_query, editor};
use crate::editor_buffer::EditorBuffer;
use super::PrefTab;

impl super::Tabular {
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

    pub fn set_initial_prefs(&mut self, prefs: crate::config::AppPreferences) {
        self.app_theme = prefs.theme;
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
        self.redis_browser_auto_refresh_default_seconds = prefs.redis_browser_auto_refresh_seconds.max(1);
        // Mirror AI settings
        self.ai_api_key = prefs.ai_api_key.clone();
        self.ai_model = prefs.ai_model.clone();
        self.ai_provider = prefs.ai_provider;
        self.ai_base_url = prefs.ai_base_url.clone();
        self.ai_settings_api_key_input = prefs.ai_api_key.clone();
        self.ai_settings_model_input = if prefs.ai_model.is_empty() {
            prefs.ai_provider.default_model().to_string()
        } else {
            prefs.ai_model.clone()
        };
        self.ai_settings_base_url_input = prefs.ai_base_url.clone();
        if let Some(url) = prefs.sync_server_url.clone()
            && !url.trim().is_empty() {
                self.sync_server_url = url;
            }

        // Store as last saved
        self.last_saved_prefs = Some(prefs);
        self.prefs_loaded = true;
    }

    // Duplicate selected row for editing


    // Delete selected row


    // End: Spreadsheet helpers
    pub fn get_runtime(&mut self) -> Arc<tokio::runtime::Runtime> {
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

    /// Load DB-type PNG icons from `assets/db_icons/<key>.png` into GPU textures.
    /// Called once per each missing key; safe to call every frame (skips already loaded keys).
    pub fn load_db_icon_textures(&mut self, ctx: &egui::Context) {
        use models::enums::DatabaseType;
        let types = [
            DatabaseType::MySQL,
            DatabaseType::PostgreSQL,
            DatabaseType::SQLite,
            DatabaseType::Redis,
            DatabaseType::MsSQL,
            DatabaseType::MongoDB,
            DatabaseType::ApiHttp,
        ];
        for db_type in &types {
            let key = db_type.icon_key();
            if self.db_icon_textures.contains_key(key) {
                continue;
            }
            let path = format!("assets/db_icons/{}.png", key);
            if let Ok(bytes) = std::fs::read(&path)
                && let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let size = [img.width() as usize, img.height() as usize];
                    let pixels = rgba.as_flat_samples();
                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
                    let handle = ctx.load_texture(key, color_image, Default::default());
                    self.db_icon_textures.insert(key.to_string(), handle);
                }
        }
    }

    pub fn triangle_toggle(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
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
        crate::log_startup_step("Tabular::new() entered");
        // Create background processing channels
        let (background_sender, background_receiver) =
            mpsc::channel::<models::enums::BackgroundTask>();
        let (result_sender, result_receiver) = mpsc::channel::<models::enums::BackgroundResult>();
        let (query_result_sender, query_result_receiver) =
            mpsc::channel::<connection::QueryResultMessage>();

        // Create shared runtime for all database operations
        crate::log_startup_step("creating shared Tokio runtime");
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => Some(Arc::new(rt)),
            Err(e) => {
                error!("Failed to create runtime: {}", e);
                None
            }
        };

        // Initialize ConfigStore
        crate::log_startup_step("initializing ConfigStore");
        let config_store = if let Some(rt) = &runtime {
            rt.block_on(async {
                crate::config::ConfigStore::new().await.ok()
            })
        } else {
            None
        };

        crate::log_startup_step("loading sync account from secrets");
        let loaded_account = crate::sync::api_client::load_account();
        crate::log_startup_step("sync account loaded -> constructing Tabular struct");
        let profile_username_input = loaded_account
            .as_ref()
            .and_then(|a| a.username.clone())
            .unwrap_or_default();
        let profile_phone_input = loaded_account
            .as_ref()
            .and_then(|a| a.phone.clone())
            .unwrap_or_default();

        let mut app = Self {
            editor: EditorBuffer::new(""),
            multi_selection: crate::editor_selection::MultiSelection::new(),
            selected_menu: "Database".to_string(),
            selected_database_sub_menu: "Connections".to_string(),
            selected_collab_sub_menu: "Teams".to_string(),
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
            pending_started_at: HashMap::new(),
            prefetch_progress: HashMap::new(),
            prefetch_in_progress: std::collections::HashSet::new(),
            show_edit_connection: false,
            edit_connection: models::structs::ConnectionConfig::default(),
            needs_refresh: false,
            current_table_data: Vec::new(),
            current_table_headers: Vec::new(),
            current_table_name: String::new(),
            current_column_metadata: None,
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
            query_job_batches: Vec::new(),
            pending_paginated_jobs: std::collections::HashSet::new(),
            next_query_job_id: 1,
            refreshing_connections: std::collections::HashSet::new(),
            connection_errors: std::collections::HashMap::new(),
            fetching_redis_keys: std::collections::HashSet::new(),
            fetching_redis_browser: std::collections::HashSet::new(),
            fetching_databases: std::collections::HashSet::new(),
            pending_expansion_restore: std::collections::HashMap::new(),
            pending_auto_load: std::collections::HashSet::new(),
            auto_synced_connections: std::collections::HashSet::new(),
            query_tabs: Vec::new(),
            active_tab_index: 0,
            next_tab_id: 1,
            scroll_to_active_tab: true,
            last_active_tab_index: None,
            show_save_dialog: false,
            save_filename: String::new(),
            save_directory: String::new(),
            save_directory_picker_result: None,
            show_connection_selector: false,
            pending_query: String::new(),
            auto_execute_after_connection: false,
            query_execution_in_progress: false,
            query_icon_hold_until: None,
            show_parameter_dialog: false,
            parameter_dialog_query: String::new(),
            parameter_inputs: Vec::new(),
            show_unsafe_dml_dialog: false,
            unsafe_dml_query: String::new(),
            unsafe_dml_type: String::new(),
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
            // App UI theme (default dark)
            app_theme: crate::config::AppTheme::Dark,
            link_editor_theme: true,
            show_settings_window: false,
            // Database search functionality
            database_search_text: String::new(),
            filtered_items_tree: Vec::new(),
            cache_miss_request: None,
            show_search_results: false,
            history_search_text: String::new(),
            filtered_history_tree: Vec::new(),
            show_clear_history_confirm: false,
            filtered_queries_tree: Vec::new(),
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
            db_icon_textures: HashMap::new(),
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
            autocomplete_debounce_ms: 180,
            fk_cache_warmed: std::collections::HashSet::new(),
            autocomplete_cols_warmed: std::collections::HashSet::new(),
            autocomplete_cols_mem: std::collections::HashMap::new(),
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
            pending_delete_connection: None,
            pending_delete_http_request: None,
            pending_rename_http_request: None,
            pending_delete_http_folder: None,
            pending_rename_http_folder: None,
            pending_create_http_folder: None,
            pending_create_http_workspace: None,
            pending_delete_http_workspace: None,
            dragged_http_conn_id: None,

            structure_col_widths: Vec::new(),
            structure_idx_col_widths: Vec::new(),
            structure_sub_view: models::structs::StructureSubView::Columns,
            last_structure_target: None,
            request_structure_refresh: false,
            is_refreshing_structure: false,
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
            update_stage: crate::auto_updater::UpdateStage::Idle,
            update_stage_receiver: None,
            staged_update_script: None,
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
            redis_browser_auto_refresh_default_seconds: 5,
            // Query message panel
            query_message: String::new(),
            query_message_is_error: false,
            show_message_panel: false,
            message_shown_at: None,
            message_panel_height: 100.0,
            query_message_display_buffer: String::new(),
            show_add_view_dialog: false,
            new_view_name: String::new(),
            new_view_query: String::new(),
            new_view_connection_id: None,
            edit_view_original_name: None,
            custom_view_save_receiver: None,
            global_backspace_pressed: false,
            sidebar_visible: true,
            show_add_replication_dialog: false,
            replication_dialog: None,
            replication_setup_receiver: None,
            show_create_subfolder_dialog: false,
            new_subfolder_name: String::new(),
            subfolder_parent_path: String::new(),
            connection_folders: Vec::new(),
            // AI Assistant
            show_ai_panel: false,
            ai_input: String::new(),
            ai_suggestion: String::new(),
            ai_is_loading: false,
            ai_error: None,
            ai_suggestion_receiver: None,
            ai_api_key: String::new(),
            ai_model: String::new(),
            ai_provider: crate::config::AiProvider::OpenAI,
            ai_base_url: String::new(),
            ai_settings_api_key_input: String::new(),
            ai_settings_model_input: String::new(),
            ai_settings_base_url_input: String::new(),
            ai_inline_processed: std::collections::HashSet::new(),
            ai_inline_receiver: None,
            toasts: crate::window_egui::notifications::ToastManager::default(),
            show_csv_import_dialog: false,
            csv_import_state: None,
            rename_symbol_active: false,
            rename_symbol_old: String::new(),
            rename_symbol_new: String::new(),
            data_scroll_x: 0.0,
            data_scroll_y: 0.0,
            cached_connection_types: std::collections::HashMap::new(),
            pending_clipboard_text: None,
            show_schema_diff_dialog: false,
            schema_diff_state: None,
            schema_diff_receiver: None,
            // ── Sync & Collaboration ─────────────────────────────────────────
            sync_account: loaded_account,
            sync_server_url: std::env::var("TABULAR_SERVER_URL")
                .unwrap_or_else(|_| "https://api.tabular.id".to_string()),
            sync_enabled: true,
            sync_status: crate::sync::SyncStatus::Offline,
            crdt_state: None,
            collab_rooms: Vec::new(),
            show_collab_panel: false,
            profile_username_input,
            profile_phone_input,
            profile_update_receiver: None,
            new_collab_room_name: String::new(),
            collab_rooms_receiver: None,
            collab_room_create_receiver: None,
            collab_room_delete_receiver: None,
            teams: Vec::new(),
            new_team_name: String::new(),
            new_team_desc: String::new(),
            expanded_team_ids: std::collections::HashSet::new(),
            show_add_member_team_ids: std::collections::HashSet::new(),
            team_members: std::collections::HashMap::new(),
            team_add_member_inputs: std::collections::HashMap::new(),
            show_add_member_dialog: false,
            add_member_target_team_id: None,
            add_member_identifier: String::new(),
            add_member_role_idx: 0,
            add_member_search_results: Vec::new(),
            add_member_search_in_progress: false,
            add_member_search_query: String::new(),
            add_member_search_receiver: None,
            teams_receiver: None,
            team_create_receiver: None,
            team_delete_receiver: None,
            team_members_receiver: None,
            team_add_member_receiver: None,
            show_share_folder_dialog: false,
            share_folder_target: None,
            share_folder_selected_team_id: None,
            share_folder_type_idx: 0,
            share_folder_path_input: String::new(),
            share_folder_receiver: None,
            shared_folders_cache: Vec::new(),
            shared_folders_receiver: None,
            sync_login_pending: false,
            sync_token_input: String::new(),
            sync_login_error: None,
            sync_auth_receiver: None,
            sync_refresh_receiver: None,
            sync_refresh_attempt_count: 0,
            sync_session_expired_notified: false,
            sync_trigger_connections: false,
            sync_trigger_history: false,
            sync_trigger_queries: false,
            sync_connections_receiver: None,
            sync_history_push_receiver: None,
            sync_history_pull_receiver: None,
            sync_queries_push_receiver: None,
            sync_queries_pull_receiver: None,
            sync_history_last_ts: None,
            db_init_receiver: None,
            yaak_workspaces: Vec::new(),
            workspaces_load_receiver: None,
            collection_search: String::new(),
            collection_expanded_folders: std::collections::HashSet::new(),
            show_yaak_import_dialog: false,
            show_postman_import_dialog: false,
        };

        // Clear any old cached pools
        app.connection_pools.clear();

        // Asynchronously initialize database and load connections in background thread
        crate::log_startup_step("spawning async background thread for initialize_database");
        let (db_tx, db_rx) = std::sync::mpsc::channel();
        app.db_init_receiver = Some(db_rx);
        std::thread::spawn(move || {
            if let Some(res) = sidebar_database::initialize_database_background() {
                let _ = db_tx.send(res);
            }
        });

        // Load saved queries from directory
        crate::log_startup_step("calling sidebar_query::load_queries_from_directory(&mut app)");
        sidebar_query::load_queries_from_directory(&mut app);
        crate::log_startup_step("sidebar_query::load_queries_from_directory finished");

        // Asynchronously load saved HTTP collections (Yaak imports) in background thread
        crate::log_startup_step("spawning async background thread for http_collection::load_workspaces");
        let (ws_tx, ws_rx) = std::sync::mpsc::channel();
        app.workspaces_load_receiver = Some(ws_rx);
        std::thread::spawn(move || {
            let ws = crate::http_collection::load_workspaces();
            let _ = ws_tx.send(ws);
        });

        // Create initial query tab
        crate::log_startup_step("creating initial query tab");
        editor::create_new_tab(&mut app, "Untitled Query".to_string(), String::new());

        // Start background thread AFTER database is initialized
        crate::log_startup_step("starting background worker thread");
        app.start_background_worker(background_receiver, result_sender);
        crate::log_startup_step("background worker thread started");

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
    pub fn start_background_worker(
        &self,
        task_receiver: Receiver<models::enums::BackgroundTask>,
        result_sender: Sender<models::enums::BackgroundResult>,
    ) {
        // Spawn a background thread to process queued tasks
        // Clone cache DB pool for use inside the worker
        let cache_pool = self.db_pool.clone();
        let shared_pools = self.shared_connection_pools.clone();
        
        std::thread::spawn(move || {
            while let Ok(task) = task_receiver.recv() {
                crate::window_egui::connection_mgr::autosync_log(&format!(
                    "[WORKER-LOOP] received background task: {:?}",
                    task
                ));
                match task {
                    models::enums::BackgroundTask::FetchDatabases { connection_id } => {
                        // Spawn a thread so this does NOT block the worker loop.
                        // The old block_on approach caused the worker to stall for the
                        // entire MySQL/PG connection time, preventing RefreshConnection
                        // from being processed concurrently.
                        let cache_pool_thread = cache_pool.clone();
                        let shared_pools_thread = shared_pools.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            eprintln!("[FETCH-DB] FetchDatabases id={} STARTED", connection_id);
                            if let Some(pool) = &cache_pool_thread
                                && let Ok(rt) = tokio::runtime::Runtime::new()
                            {
                                let dbs_opt = rt.block_on(connection::fetch_databases_background_task(
                                    connection_id,
                                    pool,
                                    &shared_pools_thread,
                                ));
                                eprintln!("[FETCH-DB] FetchDatabases id={} result: {:?}", connection_id, dbs_opt);

                                if let Some(dbs) = dbs_opt {
                                    let _ = result_sender_thread.send(
                                        models::enums::BackgroundResult::DatabasesFetched {
                                            connection_id,
                                            databases: dbs,
                                        },
                                    );
                                } else {
                                    let _ = result_sender_thread.send(
                                        models::enums::BackgroundResult::ConnectionFailed {
                                            connection_id,
                                            error_message: "Failed to connect or fetch databases from server".to_string(),
                                        },
                                    );
                                }
                            } else {
                                let _ = result_sender_thread.send(
                                    models::enums::BackgroundResult::ConnectionFailed {
                                        connection_id,
                                        error_message: "Runtime initialization error".to_string(),
                                    },
                                );
                            }
                        });
                    }
                    models::enums::BackgroundTask::TestConnection { connection } => {
                        // Spawn a thread so this does NOT block the worker loop.
                        // Testing a hung host can take the full connect budget,
                        // and running it inline stalled every other queued task
                        // behind it — the app looked frozen even though the UI
                        // thread itself was fine.
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            let (success, message) =
                                crate::connection::test_database_connection(&connection);
                            let _ = result_sender_thread.send(
                                models::enums::BackgroundResult::TestConnectionComplete {
                                    success,
                                    message,
                                },
                            );
                        });
                    }
                    models::enums::BackgroundTask::FetchRedisKeys { connection_id, database_name } => {
                        // Spawn a thread so a slow Redis server does not stall
                        // the worker loop and every task queued behind it.
                        let cache_pool_thread = cache_pool.clone();
                        let shared_pools_thread = shared_pools.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            if let Ok(rt) = tokio::runtime::Runtime::new() {
                                let keys = rt.block_on(async {
                                    if database_name == driver_redis::REDIS_CLUSTER_KEYSPACE {
                                        let redis_manager = {
                                            let pools = shared_pools_thread.lock().ok()?;
                                            if let Some(models::enums::DatabasePool::Redis(mgr)) = pools.get(&connection_id) {
                                                Some(mgr.as_ref().clone())
                                            } else {
                                                None
                                            }
                                        }?;

                                        let cache_pool_ref = cache_pool_thread.as_ref()?;
                                        let connection = driver_redis::load_redis_connection_config(
                                            cache_pool_ref.as_ref(),
                                            connection_id,
                                        )
                                        .await?;

                                        return Some(
                                            driver_redis::fetch_cluster_keys_with_types(
                                                &connection,
                                                &redis_manager,
                                                500,
                                            )
                                            .await,
                                        );
                                    }

                                    log::debug!(
                                        "[redis_keys] background standalone fetch start conn={} keyspace={}",
                                        connection_id,
                                        database_name
                                    );

                                    let cache_pool_ref = cache_pool_thread.as_ref()?;
                                    let connection = driver_redis::load_redis_connection_config(
                                        cache_pool_ref.as_ref(),
                                        connection_id,
                                    )
                                    .await?;

                                    let all_keys = driver_redis::fetch_standalone_keys_with_types(
                                        &connection,
                                        &database_name,
                                        500,
                                    )
                                    .await;

                                    log::debug!(
                                        "[redis_keys] background standalone fetch done conn={} keyspace={} total_keys={}",
                                        connection_id,
                                        database_name,
                                        all_keys.len()
                                    );

                                    Some(all_keys)
                                });

                                let _ = result_sender_thread.send(models::enums::BackgroundResult::RedisKeysFetched {
                                    connection_id,
                                    database_name,
                                    keys: keys.unwrap_or_default(),
                                });
                            }
                        });
                    }
                    models::enums::BackgroundTask::FetchRedisBrowserState {
                        connection_id,
                        database_name,
                    } => {
                        // Off the worker loop: see FetchRedisKeys above.
                        let cache_pool_thread = cache_pool.clone();
                        let shared_pools_thread = shared_pools.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                        if let Ok(rt) = tokio::runtime::Runtime::new() {
                            let state = rt.block_on(async {
                                let redis_manager = {
                                    let pools = shared_pools_thread.lock().ok()?;
                                    if let Some(models::enums::DatabasePool::Redis(mgr)) = pools.get(&connection_id) {
                                        Some(mgr.as_ref().clone())
                                    } else {
                                        None
                                    }
                                }?;

                                let cache_pool_ref = cache_pool_thread.as_ref()?;
                                let connection = driver_redis::load_redis_connection_config(
                                    cache_pool_ref.as_ref(),
                                    connection_id,
                                )
                                .await?;

                                match driver_redis::load_redis_browser_state_for_keyspace(
                                    &connection,
                                    &redis_manager,
                                    database_name.as_deref(),
                                )
                                .await
                                {
                                    Ok((available_keyspaces, keyspace_label, key_pairs, is_cluster)) => {
                                        let key_count = key_pairs.len();
                                        Some(models::structs::RedisBrowserState {
                                            available_keyspaces,
                                            keyspace_label: keyspace_label.clone(),
                                            keys: key_pairs
                                                .into_iter()
                                                .map(|(key_name, key_type)| models::structs::RedisBrowserKeyEntry {
                                                    key_name,
                                                    key_type,
                                                    ttl_label: if is_cluster {
                                                        "Cluster".to_string()
                                                    } else {
                                                        keyspace_label.clone()
                                                    },
                                                    size_label: "-".to_string(),
                                                })
                                                .collect(),
                                            status_text: if is_cluster {
                                                format!("Redis Cluster keyspace · {} keys loaded · metadata loads on selection", key_count)
                                            } else {
                                                format!("{} · {} keys loaded", keyspace_label, key_count)
                                            },
                                            ..Default::default()
                                        })
                                    }
                                    Err(error) => Some(models::structs::RedisBrowserState {
                                        last_error: Some(error),
                                        ..Default::default()
                                    }),
                                }
                            });

                            let _ = result_sender_thread.send(models::enums::BackgroundResult::RedisBrowserStateFetched {
                                connection_id,
                                state: state.unwrap_or_else(|| models::structs::RedisBrowserState {
                                    last_error: Some(format!("Failed to load Redis browser for connection {}", connection_id)),
                                    ..Default::default()
                                }),
                            });
                        }
                        });
                    }
                    models::enums::BackgroundTask::SearchRedisBrowserKeys {
                        connection_id,
                        database_name,
                        search_text,
                    } => {
                        // Off the worker loop: see FetchRedisKeys above.
                        let cache_pool_thread = cache_pool.clone();
                        let shared_pools_thread = shared_pools.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                        if let Ok(rt) = tokio::runtime::Runtime::new() {
                            let keys = rt.block_on(async {
                                let redis_manager = {
                                    let pools = shared_pools_thread.lock().ok()?;
                                    if let Some(models::enums::DatabasePool::Redis(mgr)) = pools.get(&connection_id) {
                                        Some(mgr.as_ref().clone())
                                    } else {
                                        None
                                    }
                                }?;

                                let cache_pool_ref = cache_pool_thread.as_ref()?;
                                let connection = driver_redis::load_redis_connection_config(
                                    cache_pool_ref.as_ref(),
                                    connection_id,
                                )
                                .await?;

                                Some(
                                    driver_redis::search_redis_browser_keys_from_connection(
                                        &connection,
                                        &redis_manager,
                                        &database_name,
                                        &search_text,
                                        200,
                                    )
                                    .await,
                                )
                            });

                            let _ = result_sender_thread.send(models::enums::BackgroundResult::RedisBrowserSearchFetched {
                                connection_id,
                                database_name,
                                search_text,
                                keys: keys.unwrap_or_default(),
                            });
                        }
                        });
                    }
                    models::enums::BackgroundTask::RefreshConnection { connection_id } => {
                        let cache_pool_thread = cache_pool.clone();
                        let shared_pools_thread = shared_pools.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            eprintln!(
                                "[AUTO-SYNC] bg RefreshConnection id={} STARTED cache_pool_present={}",
                                connection_id,
                                cache_pool_thread.is_some()
                            );
                            let (success, databases) = if let Some(cache_pool_arc) = &cache_pool_thread {
                                match tokio::runtime::Runtime::new() {
                                    Ok(rt) => rt.block_on(
                                        crate::connection::refresh_connection_background_async(
                                            connection_id,
                                            &Some(cache_pool_arc.clone()),
                                            &shared_pools_thread,
                                        ),
                                    ),
                                    Err(e) => {
                                        eprintln!(
                                            "[AUTO-SYNC] bg RefreshConnection id={} Runtime::new failed: {}",
                                            connection_id, e
                                        );
                                        (false, vec![])
                                    }
                                }
                            } else {
                                eprintln!(
                                    "[AUTO-SYNC] bg RefreshConnection id={} cache_pool is None!",
                                    connection_id
                                );
                                (false, vec![])
                            };
                            eprintln!(
                                "[AUTO-SYNC] bg RefreshConnection id={} FINISHED success={} databases={:?}",
                                connection_id, success, databases
                            );
                            let _ = result_sender_thread.send(
                                models::enums::BackgroundResult::RefreshComplete {
                                    connection_id,
                                    success,
                                    databases,
                                },
                            );
                        });
                    }
                    models::enums::BackgroundTask::EnsureConnectionPool { connection_id } => {
                        // Spawn a thread so pool creation doesn't block the worker loop.
                        let cache_pool_thread = cache_pool.clone();
                        let shared_pools_thread = shared_pools.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            eprintln!("[POOL] EnsureConnectionPool id={} STARTED", connection_id);
                            if let Some(pool) = &cache_pool_thread
                                && let Ok(rt) = tokio::runtime::Runtime::new()
                            {
                                let res = rt.block_on(async {
                                    crate::connection::create_connection_pool_by_id(connection_id, pool).await
                                });
                                eprintln!("[POOL] EnsureConnectionPool id={} result ok={}", connection_id, res.is_ok());
                                match res {
                                    Ok(new_pool) => {
                                        if let Ok(mut shared) = shared_pools_thread.lock() {
                                            shared.insert(connection_id, new_pool);
                                        }
                                    }
                                    Err(err_msg) => {
                                        let _ = result_sender_thread.send(models::enums::BackgroundResult::ConnectionFailed {
                                            connection_id,
                                            error_message: err_msg,
                                        });
                                    }
                                }
                            }
                        });
                    }
                    models::enums::BackgroundTask::FetchTableStructure {
                        connection_id,
                        database_name,
                        table_name,
                    } => {
                        let cache_pool_thread = cache_pool.clone();
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            if let Some(pool) = &cache_pool_thread
                                && let Ok(rt) = tokio::runtime::Runtime::new()
                            {
                                let conn_opt = rt.block_on(async {
                                    crate::connection::pool::load_connection_by_id(connection_id, pool).await
                                });

                                if let Some(conn) = conn_opt {
                                    if let Err(e) = crate::connection::pool::check_host_reachability(&conn, 2500) {
                                        log::warn!("Background FetchTableStructure reachability failed: {}", e);
                                        let _ = result_sender_thread.send(
                                            models::enums::BackgroundResult::TableStructureFetched {
                                                connection_id,
                                                database_name,
                                                table_name,
                                                columns: None,
                                            },
                                        );
                                        return;
                                    }

                                    let cols = crate::connection::fetch_columns_from_database(
                                        connection_id,
                                        &database_name,
                                        &table_name,
                                        &conn,
                                    );

                                    let _ = result_sender_thread.send(
                                        models::enums::BackgroundResult::TableStructureFetched {
                                            connection_id,
                                            database_name,
                                            table_name,
                                            columns: cols,
                                        },
                                    );
                                }
                            }
                        });
                    }
                    models::enums::BackgroundTask::CheckForUpdates => {
                        // Off the worker loop: this is an HTTP round-trip, so an
                        // unreachable update server would otherwise hold up every
                        // queued connection task behind it.
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            // Perform update check on a lightweight runtime (if required by async API)
                            let result = if let Ok(rt) = tokio::runtime::Runtime::new() {
                                rt.block_on(crate::self_update::check_for_updates())
                                    .map_err(|e| e.to_string())
                            } else {
                                Err("Failed to create runtime for update check".to_string())
                            };
                            let _ = result_sender_thread.send(
                                models::enums::BackgroundResult::UpdateCheckComplete { result },
                            );
                        });
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
                        // Open the picker on its own thread. This blocks until the
                        // user chooses — potentially minutes — and running it on
                        // the worker loop meant no connection could be opened for
                        // as long as the file dialog stayed on screen.
                        let result_sender_thread = result_sender.clone();
                        std::thread::spawn(move || {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_title("Pilih File / Folder SQLite")
                                .pick_folder()
                            {
                                let _ = result_sender_thread.send(
                                    models::enums::BackgroundResult::SqlitePathPicked {
                                        path: path.to_string_lossy().to_string(),
                                    },
                                );
                            }
                        });
                    }
                }
            }
        });
    }
}
