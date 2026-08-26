use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::backup_restore::{
    BackupContentScope, BackupFormat, BackupOptions, BackupRestoreRunner, BinaryDetector,
    NativeBinaryInfo, OperationStatus, OperationType, ProgressSnapshot, ProgressTracker,
    RestoreOptions,
};
use crate::models::enums::DatabaseType;
use crate::models::structs::ConnectionConfig;
use crate::rfd;
use crate::window_egui::Tabular;

// ─── Dialog State: Backup ───────────────────────────────────────────────────

#[derive(Clone)]
pub struct BackupDialogState {
    pub connection_id: i64,
    pub database_name: String,
    pub connection_type: DatabaseType,
    pub target_file: Option<PathBuf>,
    pub format: BackupFormat,
    pub scope: BackupContentScope,
    pub include_triggers_routines: bool,
    pub single_transaction: bool,
    pub clean_before_recreate: bool,
    pub no_owner: bool,
    pub no_privileges: bool,
    pub available_tables: Vec<String>,
    pub selected_tables: HashSet<String>,
    pub table_search_query: String,
    pub binary_info: Option<NativeBinaryInfo>,
    pub custom_binary_path: String,
    pub tracker: Option<Arc<Mutex<ProgressTracker>>>,
    pub cancel_token: Option<Arc<AtomicBool>>,
    pub is_running: bool,
    pub last_snapshot: Option<ProgressSnapshot>,
}

impl BackupDialogState {
    pub fn new(conn_id: i64, db_name: String, connections: &[ConnectionConfig]) -> Self {
        let conn = connections.iter().find(|c| c.id == Some(conn_id));
        let conn_type = conn.map_or(DatabaseType::MySQL, |c| c.connection_type.clone());

        let binary_info = match conn_type {
            DatabaseType::PostgreSQL => BinaryDetector::find_binary("pg_dump", None),
            DatabaseType::MySQL => BinaryDetector::find_binary("mysqldump", None),
            _ => None,
        };

        let default_format = match conn_type {
            DatabaseType::SQLite => BackupFormat::SqliteNative,
            _ => BackupFormat::GzipSql,
        };

        let default_file_name = format!(
            "{}_{}.{}",
            db_name.replace(' ', "_"),
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            default_format.extension()
        );

        let default_target = dirs::download_dir()
            .or_else(|| dirs::home_dir())
            .map(|p| p.join(&default_file_name));

        Self {
            connection_id: conn_id,
            database_name: db_name,
            connection_type: conn_type,
            target_file: default_target,
            format: default_format,
            scope: BackupContentScope::Both,
            include_triggers_routines: true,
            single_transaction: true,
            clean_before_recreate: false,
            no_owner: true,
            no_privileges: false,
            available_tables: Vec::new(),
            selected_tables: HashSet::new(),
            table_search_query: String::new(),
            binary_info,
            custom_binary_path: String::new(),
            tracker: None,
            cancel_token: None,
            is_running: false,
            last_snapshot: None,
        }
    }

    pub fn update_target_file_extension(&mut self) {
        if let Some(target) = &self.target_file {
            let stem = target
                .file_stem()
                .map_or("backup", |s| s.to_str().unwrap_or("backup"));
            let parent = target.parent().unwrap_or_else(|| std::path::Path::new(""));
            let new_filename = format!("{}.{}", stem, self.format.extension());
            self.target_file = Some(parent.join(new_filename));
        }
    }
}

// ─── Dialog State: Restore ──────────────────────────────────────────────────

#[derive(Clone)]
pub struct RestoreDialogState {
    pub connection_id: i64,
    pub target_database_name: String,
    pub connection_type: DatabaseType,
    pub source_file: Option<PathBuf>,
    pub clean_before_restore: bool,
    pub single_transaction: bool,
    pub stop_on_error: bool,
    pub data_only: bool,
    pub schema_only: bool,
    pub binary_info: Option<NativeBinaryInfo>,
    pub custom_binary_path: String,
    pub tracker: Option<Arc<Mutex<ProgressTracker>>>,
    pub cancel_token: Option<Arc<AtomicBool>>,
    pub is_running: bool,
    pub last_snapshot: Option<ProgressSnapshot>,
}

impl RestoreDialogState {
    pub fn new(conn_id: i64, db_name: String, connections: &[ConnectionConfig]) -> Self {
        let conn = connections.iter().find(|c| c.id == Some(conn_id));
        let conn_type = conn.map_or(DatabaseType::MySQL, |c| c.connection_type.clone());

        let binary_info = match conn_type {
            DatabaseType::PostgreSQL => BinaryDetector::find_binary("pg_restore", None)
                .or_else(|| BinaryDetector::find_binary("psql", None)),
            DatabaseType::MySQL => BinaryDetector::find_binary("mysql", None),
            _ => None,
        };

        Self {
            connection_id: conn_id,
            target_database_name: db_name,
            connection_type: conn_type,
            source_file: None,
            clean_before_restore: false,
            single_transaction: false,
            stop_on_error: true,
            data_only: false,
            schema_only: false,
            binary_info,
            custom_binary_path: String::new(),
            tracker: None,
            cancel_token: None,
            is_running: false,
            last_snapshot: None,
        }
    }
}

// ─── Rendering: Backup Dialog ───────────────────────────────────────────────

pub fn render_backup_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    let mut open = tabular.show_backup_dialog;
    let mut start_backup_requested = false;
    let mut cancel_requested = false;
    let mut close_dialog = false;

    // Refresh tables from cache if empty
    let (need_tables, conn_id, db_name) = if let Some(state) = &tabular.backup_state {
        (
            state.available_tables.is_empty(),
            state.connection_id,
            state.database_name.clone(),
        )
    } else {
        (false, 0, String::new())
    };

    if need_tables {
        if let Some(tables) =
            crate::cache_data::get_tables_from_cache(tabular, conn_id, &db_name, "table")
        {
            if let Some(state) = &mut tabular.backup_state {
                state.available_tables = tables;
            }
        }
    }

    if let Some(state) = &mut tabular.backup_state {
        // Poll progress if tracker is active
        if let Some(tracker) = &state.tracker {
            let snap = tracker.lock().unwrap().snapshot();
            state.is_running = matches!(snap.status, OperationStatus::Running);
            state.last_snapshot = Some(snap);
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }

    egui::Window::new("💾 Database Backup & Export")
        .open(&mut open)
        .default_size(egui::vec2(580.0, 440.0))
        .max_size(egui::vec2(660.0, (ctx.content_rect().height() * 0.85).max(360.0)))
        .resizable(true)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            if let Some(state) = &mut tabular.backup_state {
                // ── Header Bar ─────────────────────────────────────────────
                render_header_card(
                    ui,
                    "Database Backup",
                    &state.database_name,
                    &state.connection_type,
                    state.binary_info.as_ref(),
                );

                ui.add_space(6.0);

                // Calculate scroll area height so bottom buttons are always visible
                let scroll_height = (ui.available_height() - 44.0).max(100.0);

                // ── If operation has started, show progress dashboard ─────
                if state.is_running || state.last_snapshot.is_some() {
                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            render_progress_dashboard(
                                ui,
                                state.last_snapshot.as_ref(),
                                state.is_running,
                                &mut cancel_requested,
                            );
                        });
                } else {
                    // ── Settings Configuration Tabs / Sections ──────────────
                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // Section 1: Output Destination
                            ui.group(|ui| {
                                ui.label(egui::RichText::new("📁 Destination File").strong().small());
                                ui.horizontal(|ui| {
                                    let mut path_str = state
                                        .target_file
                                        .as_ref()
                                        .map_or(String::new(), |p| p.to_string_lossy().to_string());

                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut path_str)
                                            .desired_width(ui.available_width() - 95.0)
                                            .hint_text("Choose target backup file path..."),
                                    );
                                    if resp.changed() {
                                        state.target_file = Some(PathBuf::from(path_str));
                                    }

                                    if ui.button("Browse...").clicked() {
                                        let default_name = state.target_file.as_ref().map_or_else(
                                            || {
                                                format!(
                                                    "{}.{}",
                                                    state.database_name,
                                                    state.format.extension()
                                                )
                                            },
                                            |p| {
                                                p.file_name()
                                                    .map_or("backup.sql".to_string(), |n| {
                                                        n.to_string_lossy().to_string()
                                                    })
                                            },
                                        );

                                        let ext = state.format.extension();
                                        if let Some(path) = rfd::FileDialog::new()
                                            .set_file_name(default_name)
                                            .add_filter(state.format.display_label(), &[ext])
                                            .save_file()
                                        {
                                            state.target_file = Some(path);
                                        }
                                    }
                                });
                            });

                            ui.add_space(4.0);

                            // Section 2: Format & Scope
                            ui.group(|ui| {
                                ui.label(egui::RichText::new("⚙️ Format & Scope").strong().small());
                                ui.horizontal(|ui| {
                                    ui.label("Format:");
                                    let prev_format = state.format;
                                    egui::ComboBox::from_id_salt("backup_format_combo")
                                        .selected_text(state.format.display_label())
                                        .show_ui(ui, |ui| {
                                            for fmt in [
                                                BackupFormat::GzipSql,
                                                BackupFormat::PlainSql,
                                                BackupFormat::PostgresCustom,
                                                BackupFormat::PostgresTar,
                                                BackupFormat::SqliteNative,
                                            ] {
                                                if fmt.supported_for(&state.connection_type) {
                                                    ui.selectable_value(
                                                        &mut state.format,
                                                        fmt,
                                                        fmt.display_label(),
                                                    );
                                                }
                                            }
                                        });

                                    if state.format != prev_format {
                                        state.update_target_file_extension();
                                    }

                                    ui.add_space(12.0);
                                    ui.label("Content:");
                                    egui::ComboBox::from_id_salt("backup_scope_combo")
                                        .selected_text(match state.scope {
                                            BackupContentScope::Both => "Schema & Data",
                                            BackupContentScope::SchemaOnly => "Schema Only",
                                            BackupContentScope::DataOnly => "Data Only",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut state.scope,
                                                BackupContentScope::Both,
                                                "Schema & Data",
                                            );
                                            ui.selectable_value(
                                                &mut state.scope,
                                                BackupContentScope::SchemaOnly,
                                                "Schema Only",
                                            );
                                            ui.selectable_value(
                                                &mut state.scope,
                                                BackupContentScope::DataOnly,
                                                "Data Only",
                                            );
                                        });
                                });
                            });

                            ui.add_space(4.0);

                            // Section 3: Advanced Options
                            ui.group(|ui| {
                                ui.label(egui::RichText::new("🛠️ Advanced Options").strong().small());
                                ui.horizontal_wrapped(|ui| {
                                    ui.checkbox(
                                        &mut state.single_transaction,
                                        "Single Transaction",
                                    );
                                    ui.checkbox(
                                        &mut state.include_triggers_routines,
                                        "Include Triggers/Routines",
                                    );
                                    ui.checkbox(
                                        &mut state.clean_before_recreate,
                                        "DROP TABLE before CREATE",
                                    );
                                    if state.connection_type == DatabaseType::PostgreSQL {
                                        ui.checkbox(
                                            &mut state.no_owner,
                                            "No Owner (--no-owner)",
                                        );
                                        ui.checkbox(
                                            &mut state.no_privileges,
                                            "No Privileges (--no-privileges)",
                                        );
                                    }
                                });
                            });

                            ui.add_space(4.0);

                            // Section 4: Table Selection Filter (Collapsible to save vertical space)
                            if !state.available_tables.is_empty() {
                                ui.group(|ui| {
                                    let filter_title = if state.selected_tables.is_empty() {
                                        format!("📋 Table Filter (All {} tables included)", state.available_tables.len())
                                    } else {
                                        format!("📋 Table Filter ({} of {} selected)", state.selected_tables.len(), state.available_tables.len())
                                    };

                                    egui::CollapsingHeader::new(egui::RichText::new(filter_title).strong().small())
                                        .default_open(false)
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut state.table_search_query)
                                                        .hint_text("🔍 Filter table list...")
                                                        .desired_width(ui.available_width() - 140.0),
                                                );

                                                if ui.button("Select All").clicked() {
                                                    for tbl in &state.available_tables {
                                                        state.selected_tables.insert(tbl.clone());
                                                    }
                                                }
                                                if ui.button("Clear").clicked() {
                                                    state.selected_tables.clear();
                                                }
                                            });

                                            egui::ScrollArea::vertical()
                                                .max_height(100.0)
                                                .show(ui, |ui| {
                                                    let q = state.table_search_query.to_lowercase();
                                                    for table in &state.available_tables {
                                                        if !q.is_empty()
                                                            && !table.to_lowercase().contains(&q)
                                                        {
                                                            continue;
                                                        }

                                                        let mut is_checked =
                                                            state.selected_tables.contains(table);
                                                        if ui.checkbox(&mut is_checked, table).changed() {
                                                            if is_checked {
                                                                state.selected_tables.insert(table.clone());
                                                            } else {
                                                                state.selected_tables.remove(table);
                                                            }
                                                        }
                                                    }
                                                });
                                        });
                                });
                            }
                        });
                }

                ui.add_space(6.0);

                // ── Action Buttons (ALWAYS pinned at bottom) ───────────────
                ui.horizontal(|ui| {
                    if state.is_running {
                        let cancel_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("⛔ Cancel Backup").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(200, 40, 40)),
                        );
                        if cancel_btn.clicked() {
                            cancel_requested = true;
                        }
                    } else if state.last_snapshot.is_some() {
                        if ui.button("🔄 Start Another Backup").clicked() {
                            state.last_snapshot = None;
                            state.tracker = None;
                            state.cancel_token = None;
                        }
                    } else {
                        let can_start = state.target_file.is_some();
                        let start_btn = ui.add_enabled(
                            can_start,
                            egui::Button::new(
                                egui::RichText::new("🚀 Start Backup")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(egui::Color32::from_rgb(30, 130, 70)),
                        );
                        if start_btn.clicked() {
                            start_backup_requested = true;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close_dialog = true;
                        }
                    });
                });
            }
        });

    if close_dialog {
        open = false;
    }
    tabular.show_backup_dialog = open;

    // Handle cancel request
    if cancel_requested {
        if let Some(state) = &mut tabular.backup_state {
            if let Some(token) = &state.cancel_token {
                token.store(true, Ordering::Relaxed);
            }
        }
    }

    // Handle start backup request
    if start_backup_requested {
        if let Some(state) = &mut tabular.backup_state {
            if let Some(conn) = tabular
                .connections
                .iter()
                .find(|c| c.id == Some(state.connection_id))
            {
                let target_file = state.target_file.clone().unwrap_or_else(|| {
                    dirs::download_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(format!("{}.sql", state.database_name))
                });

                let options = BackupOptions {
                    database_name: state.database_name.clone(),
                    target_file: target_file.clone(),
                    format: state.format,
                    scope: state.scope,
                    selected_tables: state.selected_tables.iter().cloned().collect(),
                    excluded_tables: Vec::new(),
                    include_triggers_routines: state.include_triggers_routines,
                    single_transaction: state.single_transaction,
                    clean_before_recreate: state.clean_before_recreate,
                    no_owner: state.no_owner,
                    no_privileges: state.no_privileges,
                    custom_binary_path: if state.custom_binary_path.trim().is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(&state.custom_binary_path))
                    },
                };

                let tracker = Arc::new(Mutex::new(ProgressTracker::new(
                    OperationType::Backup,
                    state.database_name.clone(),
                    target_file,
                )));
                let cancel_token = Arc::new(AtomicBool::new(false));

                state.tracker = Some(tracker.clone());
                state.cancel_token = Some(cancel_token.clone());
                state.is_running = true;

                BackupRestoreRunner::run_backup(conn, options, tracker, cancel_token);
            }
        }
    }
}

// ─── Rendering: Restore Dialog ──────────────────────────────────────────────

pub fn render_restore_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    let mut open = tabular.show_restore_dialog;
    let mut start_restore_requested = false;
    let mut cancel_requested = false;
    let mut close_dialog = false;

    if let Some(state) = &mut tabular.restore_state {
        if let Some(tracker) = &state.tracker {
            let snap = tracker.lock().unwrap().snapshot();
            state.is_running = matches!(snap.status, OperationStatus::Running);
            state.last_snapshot = Some(snap);
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }

    egui::Window::new("📥 Database Restore & Import")
        .open(&mut open)
        .default_size(egui::vec2(580.0, 420.0))
        .max_size(egui::vec2(660.0, (ctx.content_rect().height() * 0.85).max(340.0)))
        .resizable(true)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            if let Some(state) = &mut tabular.restore_state {
                // Header
                render_header_card(
                    ui,
                    "Database Restore",
                    &state.target_database_name,
                    &state.connection_type,
                    state.binary_info.as_ref(),
                );

                ui.add_space(6.0);

                let scroll_height = (ui.available_height() - 44.0).max(100.0);

                if state.is_running || state.last_snapshot.is_some() {
                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            render_progress_dashboard(
                                ui,
                                state.last_snapshot.as_ref(),
                                state.is_running,
                                &mut cancel_requested,
                            );
                        });
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // Section 1: Source File Picker
                            ui.group(|ui| {
                                ui.label(egui::RichText::new("📂 Backup Source File").strong().small());
                                ui.horizontal(|ui| {
                                    let mut path_str = state
                                        .source_file
                                        .as_ref()
                                        .map_or(String::new(), |p| p.to_string_lossy().to_string());

                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut path_str)
                                            .desired_width(ui.available_width() - 95.0)
                                            .hint_text(
                                                "Select .sql, .sql.gz, .dump, .tar or .sqlite file...",
                                            ),
                                    );
                                    if resp.changed() {
                                        state.source_file = Some(PathBuf::from(path_str));
                                    }

                                    if ui.button("Browse...").clicked() {
                                        if let Some(path) = rfd::FileDialog::new()
                                            .add_filter(
                                                "Database Backup Files",
                                                &["sql", "gz", "dump", "pgdump", "tar", "sqlite", "db"],
                                            )
                                            .pick_file()
                                        {
                                            state.source_file = Some(path);
                                        }
                                    }
                                });
                            });

                            ui.add_space(4.0);

                            // Section 2: Restore Options
                            ui.group(|ui| {
                                ui.label(egui::RichText::new("⚙️ Restore Options").strong().small());
                                ui.horizontal_wrapped(|ui| {
                                    ui.checkbox(
                                        &mut state.clean_before_restore,
                                        "Drop Objects Before Recreating (--clean)",
                                    );
                                    ui.checkbox(
                                        &mut state.single_transaction,
                                        "Single Transaction",
                                    );
                                    ui.checkbox(
                                        &mut state.stop_on_error,
                                        "Stop on Error",
                                    );
                                    ui.checkbox(
                                        &mut state.data_only,
                                        "Data Only",
                                    );
                                    ui.checkbox(
                                        &mut state.schema_only,
                                        "Schema Only",
                                    );
                                });
                            });

                            ui.add_space(4.0);

                            // Warning Banner
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("⚠️ CAUTION:")
                                            .color(egui::Color32::from_rgb(230, 160, 20))
                                            .strong()
                                            .small(),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Queries will be executed into database '{}'. Existing data may be overwritten.",
                                            state.target_database_name
                                        ))
                                        .small(),
                                    );
                                });
                            });
                        });
                }

                ui.add_space(6.0);

                // ── Action Buttons ─────────────────────────────────────────
                ui.horizontal(|ui| {
                    if state.is_running {
                        let cancel_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("⛔ Cancel Restore").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(200, 40, 40)),
                        );
                        if cancel_btn.clicked() {
                            cancel_requested = true;
                        }
                    } else if state.last_snapshot.is_some() {
                        if ui.button("🔄 Start Another Restore").clicked() {
                            state.last_snapshot = None;
                            state.tracker = None;
                            state.cancel_token = None;
                        }
                    } else {
                        let can_start = state.source_file.is_some();
                        let start_btn = ui.add_enabled(
                            can_start,
                            egui::Button::new(
                                egui::RichText::new("🚀 Start Restore")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(egui::Color32::from_rgb(30, 120, 190)),
                        );
                        if start_btn.clicked() {
                            start_restore_requested = true;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close_dialog = true;
                        }
                    });
                });
            }
        });

    if close_dialog {
        open = false;
    }
    tabular.show_restore_dialog = open;

    if cancel_requested {
        if let Some(state) = &mut tabular.restore_state {
            if let Some(token) = &state.cancel_token {
                token.store(true, Ordering::Relaxed);
            }
        }
    }

    if start_restore_requested {
        if let Some(state) = &mut tabular.restore_state {
            if let (Some(conn), Some(src_file)) = (
                tabular
                    .connections
                    .iter()
                    .find(|c| c.id == Some(state.connection_id)),
                state.source_file.clone(),
            ) {
                let options = RestoreOptions {
                    target_database_name: state.target_database_name.clone(),
                    source_file: src_file.clone(),
                    clean_before_restore: state.clean_before_restore,
                    single_transaction: state.single_transaction,
                    stop_on_error: state.stop_on_error,
                    data_only: state.data_only,
                    schema_only: state.schema_only,
                    custom_binary_path: if state.custom_binary_path.trim().is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(&state.custom_binary_path))
                    },
                };

                let tracker = Arc::new(Mutex::new(ProgressTracker::new(
                    OperationType::Restore,
                    state.target_database_name.clone(),
                    src_file,
                )));
                let cancel_token = Arc::new(AtomicBool::new(false));

                state.tracker = Some(tracker.clone());
                state.cancel_token = Some(cancel_token.clone());
                state.is_running = true;

                BackupRestoreRunner::run_restore(conn, options, tracker, cancel_token);
            }
        }
    }
}

// ─── UI Helper Components ───────────────────────────────────────────────────

fn render_header_card(
    ui: &mut egui::Ui,
    title: &str,
    database_name: &str,
    db_type: &DatabaseType,
    binary_info: Option<&NativeBinaryInfo>,
) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(db_type.icon()).size(20.0));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(title).strong());
                    ui.label(
                        egui::RichText::new(format!("[{}]", db_type.badge_label()))
                            .color(egui::Color32::from_rgb(
                                db_type.badge_color().0,
                                db_type.badge_color().1,
                                db_type.badge_color().2,
                            ))
                            .small(),
                    );
                });
                ui.label(egui::RichText::new(format!("Target DB: {}", database_name)).weak().small());
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if *db_type == DatabaseType::SQLite {
                    ui.label(
                        egui::RichText::new("⚡ Pure-Rust Engine")
                            .color(egui::Color32::from_rgb(40, 180, 90))
                            .small(),
                    );
                } else if let Some(info) = binary_info {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(format!("✅ {} Detected", info.name))
                                .color(egui::Color32::from_rgb(40, 180, 90))
                                .small(),
                        );
                        if let Some(ver) = &info.version {
                            let short_ver = ver.lines().next().unwrap_or("");
                            ui.label(egui::RichText::new(short_ver).weak().small());
                        }
                    });
                } else {
                    ui.label(
                        egui::RichText::new("⚠️ Native CLI binary not found in PATH")
                            .color(egui::Color32::from_rgb(220, 130, 20))
                            .small(),
                    );
                }
            });
        });
    });
}

fn render_progress_dashboard(
    ui: &mut egui::Ui,
    snapshot_opt: Option<&ProgressSnapshot>,
    _is_running: bool,
    _cancel_requested: &mut bool,
) {
    if let Some(snap) = snapshot_opt {
        ui.group(|ui| {
            // Status row
            ui.horizontal(|ui| {
                match &snap.status {
                    OperationStatus::Running => {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Operation in progress...")
                                .color(egui::Color32::from_rgb(50, 150, 250))
                                .strong(),
                        );
                    }
                    OperationStatus::Completed => {
                        ui.label(
                            egui::RichText::new("🎉 Completed Successfully!")
                                .color(egui::Color32::from_rgb(40, 180, 90))
                                .strong(),
                        );
                    }
                    OperationStatus::Failed(err) => {
                        ui.label(
                            egui::RichText::new(format!("❌ Failed: {}", err))
                                .color(egui::Color32::from_rgb(220, 50, 50))
                                .strong(),
                        );
                    }
                    OperationStatus::Cancelled => {
                        ui.label(
                            egui::RichText::new("⚠️ Cancelled by User")
                                .color(egui::Color32::from_rgb(230, 160, 20))
                                .strong(),
                        );
                    }
                    OperationStatus::Idle => {
                        ui.label("Ready");
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("Elapsed: {:.1}s", snap.elapsed_secs)).weak(),
                    );
                });
            });

            ui.add_space(6.0);

            // Metrics Cards
            ui.horizontal(|ui| {
                let bytes_str = format_byte_size(snap.bytes_processed);
                let speed_str = format!("{}/s", format_byte_size(snap.bytes_per_sec as u64));

                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Transferred").weak().small());
                    ui.label(egui::RichText::new(bytes_str).strong());
                });

                ui.add_space(24.0);

                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Transfer Speed").weak().small());
                    ui.label(egui::RichText::new(speed_str).strong());
                });

                if snap.total_pages > 0 {
                    ui.add_space(24.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Pages").weak().small());
                        ui.label(
                            egui::RichText::new(format!(
                                "{}/{}",
                                snap.pages_copied, snap.total_pages
                            ))
                            .strong(),
                        );
                    });
                }
            });

            ui.add_space(6.0);

            // Stage Label
            ui.label(
                egui::RichText::new(format!("Stage: {}", snap.current_stage))
                    .small()
                    .weak(),
            );

            ui.add_space(6.0);

            // Live Log Console
            ui.label(egui::RichText::new("Output Stream:").strong().small());
            let console_bg = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(14, 15, 18)
            } else {
                egui::Color32::from_rgb(240, 243, 246)
            };

            egui::Frame::new()
                .fill(console_bg)
                .inner_margin(6.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &snap.log_lines {
                                let text_color = if line.contains("❌") || line.contains("Error") {
                                    egui::Color32::from_rgb(230, 80, 80)
                                } else if line.contains("⚠️") || line.contains("stderr") {
                                    egui::Color32::from_rgb(220, 160, 40)
                                } else if line.contains("finished") || line.contains("Completed") {
                                    egui::Color32::from_rgb(60, 200, 100)
                                } else {
                                    ui.visuals().text_color()
                                };

                                ui.label(
                                    egui::RichText::new(line)
                                        .monospace()
                                        .size(11.0)
                                        .color(text_color),
                                );
                            }
                        });
                });
        });
    }
}

fn format_byte_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}
