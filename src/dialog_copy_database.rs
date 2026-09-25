use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::backup_restore::{
    BackupRestoreRunner, BinaryDetector, CopyDatabaseOptions, NativeBinaryInfo, OperationStatus,
    OperationType, ProgressSnapshot, ProgressTracker,
};
use crate::dialog_backup_restore::{render_header_card, render_progress_dashboard};
use crate::models::enums::DatabaseType;
use crate::models::structs::ConnectionConfig;
use crate::rfd;
use crate::window_egui::Tabular;

// ─── Dialog State: Copy Database ────────────────────────────────────────────

#[derive(Clone)]
pub struct CopyDatabaseDialogState {
    pub connection_id: i64,
    pub source_database_name: String,
    pub target_database_name: String,
    pub connection_type: DatabaseType,
    pub target_file: Option<PathBuf>,
    pub binary_info: Option<NativeBinaryInfo>,
    pub custom_binary_path: String,
    pub drop_target_if_exists: bool,
    pub include_routines: bool,
    pub tracker: Option<Arc<Mutex<ProgressTracker>>>,
    pub cancel_token: Option<Arc<AtomicBool>>,
    pub is_running: bool,
    pub last_snapshot: Option<ProgressSnapshot>,
    pub validation_error: Option<String>,
}

impl CopyDatabaseDialogState {
    pub fn new(conn_id: i64, db_name: String, connections: &[ConnectionConfig]) -> Self {
        let conn = connections.iter().find(|c| c.id == Some(conn_id));
        let conn_type = conn.map_or(DatabaseType::MySQL, |c| c.connection_type.clone());

        let binary_info = match conn_type {
            DatabaseType::PostgreSQL => BinaryDetector::find_binary("pg_dump", None)
                .or_else(|| BinaryDetector::find_binary("psql", None)),
            DatabaseType::MySQL => BinaryDetector::find_binary("mysqldump", None)
                .or_else(|| BinaryDetector::find_binary("mysql", None)),
            _ => None,
        };

        let default_target_name = format!("{}_copy", db_name);

        let target_file = if conn_type == DatabaseType::SQLite {
            if let Some(c) = conn {
                let p = PathBuf::from(&c.database);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("database");
                let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("sqlite");
                let parent = p.parent().unwrap_or_else(|| std::path::Path::new(""));
                Some(parent.join(format!("{}_copy.{}", stem, ext)))
            } else {
                dirs::download_dir()
                    .or_else(dirs::home_dir)
                    .map(|p| p.join(format!("{}_copy.sqlite", db_name)))
            }
        } else {
            None
        };

        Self {
            connection_id: conn_id,
            source_database_name: db_name,
            target_database_name: default_target_name,
            connection_type: conn_type,
            target_file,
            binary_info,
            custom_binary_path: String::new(),
            drop_target_if_exists: false,
            include_routines: false,
            tracker: None,
            cancel_token: None,
            is_running: false,
            last_snapshot: None,
            validation_error: None,
        }
    }
}

// ─── Rendering: Copy Database Dialog ────────────────────────────────────────

pub fn render_copy_database_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    let mut open = tabular.show_copy_database_dialog;
    let mut start_copy_requested = false;
    let mut cancel_requested = false;
    let mut close_dialog = false;
    let mut refresh_connection_id: Option<i64> = None;

    if let Some(state) = &mut tabular.copy_database_state {
        if let Some(tracker) = &state.tracker {
            let snap = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .snapshot();
            state.is_running = matches!(snap.status, OperationStatus::Running);
            state.last_snapshot = Some(snap);
            if state.is_running {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
    }

    crate::window_egui::style::render_modal_backdrop(ctx, "modal_backdrop_copy_database", open);

    let screen = ctx.content_rect();
    let dialog_w = (screen.width() - 32.0).min(580.0);
    let dialog_h = (screen.height() - 32.0).min(490.0);

    egui::Window::new("📋 Copy Database")
        .open(&mut open)
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .fixed_size(egui::vec2(dialog_w, dialog_h))
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(ui.available_width());

            crate::window_egui::style::render_modal_header(
                ui,
                "📋 Copy Database",
                &mut close_dialog,
            );

            if let Some(state) = &mut tabular.copy_database_state {
                // Header card
                let display_db = if state.target_database_name.is_empty() {
                    format!("{} → ...", state.source_database_name)
                } else {
                    format!("{} → {}", state.source_database_name, state.target_database_name)
                };
                render_header_card(
                    ui,
                    "Copy Database",
                    &display_db,
                    &state.connection_type,
                    state.binary_info.as_ref(),
                );

                ui.add_space(6.0);

                let scroll_height = (ui.available_height() - 48.0).max(100.0);

                if state.is_running || state.last_snapshot.is_some() {
                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
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
                            ui.set_width(ui.available_width());
                            // Section 1: Source & Target Configuration
                            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(egui::RichText::new("🎯 Copy Configuration").strong().small());
                                ui.add_space(4.0);

                                egui::Grid::new("copy_db_form_grid")
                                    .num_columns(2)
                                    .spacing([12.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new("Source Database:").weak());
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(&state.source_database_name).strong());
                                        });
                                        ui.end_row();

                                        ui.label(egui::RichText::new("New Database Name:").strong());
                                        let field_w = (ui.available_width() - 4.0).max(150.0);
                                        let resp = crate::window_egui::style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut state.target_database_name)
                                                .hint_text("Enter new database name..."),
                                            field_w,
                                            None,
                                        );
                                        if resp.changed() {
                                            state.validation_error = None;
                                        }
                                        ui.end_row();

                                        if state.connection_type == DatabaseType::SQLite {
                                            ui.label(egui::RichText::new("Destination File:").strong());
                                            ui.horizontal(|ui| {
                                                let mut path_str = state
                                                    .target_file
                                                    .as_ref()
                                                    .map_or(String::new(), |p| p.to_string_lossy().to_string());

                                                let browse_w = 80.0;
                                                let spacing = 8.0;
                                                let path_w = (ui.available_width() - browse_w - spacing).max(100.0);
                                                let resp = crate::window_egui::style::render_text_field(
                                                    ui,
                                                    egui::TextEdit::singleline(&mut path_str)
                                                        .hint_text("Select target SQLite file path..."),
                                                    path_w,
                                                    None,
                                                );
                                                if resp.changed() {
                                                    state.target_file = Some(PathBuf::from(path_str));
                                                    state.validation_error = None;
                                                }
                                                ui.add_space(spacing);

                                                if ui
                                                    .add(
                                                        crate::window_egui::style::btn_field_action(ui, "Browse...")
                                                            .min_size(egui::vec2(browse_w, 0.0)),
                                                    )
                                                    .clicked()
                                                {
                                                    let default_name = format!("{}.sqlite", state.target_database_name);
                                                    if let Some(path) = rfd::FileDialog::new()
                                                        .set_file_name(&default_name)
                                                        .add_filter("SQLite Database", &["sqlite", "db", "sqlite3"])
                                                        .save_file()
                                                    {
                                                        state.target_file = Some(path);
                                                        state.validation_error = None;
                                                    }
                                                }
                                            });
                                            ui.end_row();
                                        }
                                    });

                                ui.add_space(8.0);

                                // Checkboxes for advanced copy options
                                let drop_label = if state.connection_type == DatabaseType::SQLite {
                                    "Overwrite destination SQLite file if it already exists"
                                } else {
                                    "Overwrite / Drop target database if it already exists"
                                };
                                let drop_color = if state.drop_target_if_exists {
                                    egui::Color32::from_rgb(235, 140, 30)
                                } else {
                                    ui.visuals().text_color()
                                };
                                ui.checkbox(
                                    &mut state.drop_target_if_exists,
                                    egui::RichText::new(drop_label).color(drop_color).small(),
                                );

                                if state.connection_type == DatabaseType::MySQL {
                                    ui.checkbox(
                                        &mut state.include_routines,
                                        egui::RichText::new("Include Stored Procedures & Functions (Routines)").small(),
                                    );
                                }

                                ui.add_space(6.0);

                                // Information / Guidance Box
                                egui::Frame::new()
                                    .fill(if ui.visuals().dark_mode {
                                        egui::Color32::from_rgba_unmultiplied(30, 80, 160, 30)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(50, 120, 220, 25)
                                    })
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label("ℹ️");
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(
                                                        "A new database will be created on the server. All schema, tables, views, and data will be copied from the source database.",
                                                    )
                                                    .size(11.5),
                                                )
                                                .wrap(),
                                            );
                                        });
                                    });

                                if let Some(err) = &state.validation_error {
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new(format!("⚠️ {}", err))
                                            .color(egui::Color32::from_rgb(220, 50, 50))
                                            .size(12.0)
                                            .strong(),
                                    );
                                }
                            });

                            // Section 2: Advanced / Custom CLI path (if not SQLite)
                            if state.connection_type != DatabaseType::SQLite {
                                ui.add_space(4.0);
                                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(egui::RichText::new("⚙️ Advanced Settings").strong().small());
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        ui.set_width(ui.available_width());
                                        let tool_name = match state.connection_type {
                                            DatabaseType::PostgreSQL => "pg_dump / psql",
                                            DatabaseType::MySQL => "mysqldump / mysql",
                                            _ => "database CLI",
                                        };
                                        ui.label(egui::RichText::new(format!("Custom {} Path:", tool_name)).weak().small());
                                        let w = (ui.available_width() - 4.0).max(100.0);
                                        crate::window_egui::style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut state.custom_binary_path)
                                                .hint_text("Optional: Directory or binary path..."),
                                            w,
                                            None,
                                        );
                                    });
                                });
                            }
                        });
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // ── Action Buttons ─────────────────────────────────────────
                ui.horizontal(|ui| {
                    if state.is_running {
                        let cancel_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("⛔ Cancel Copy")
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(200, 40, 40))
                            .min_size(egui::vec2(140.0, 32.0)),
                        );
                        if cancel_btn.clicked() {
                            cancel_requested = true;
                        }
                    } else if let Some(snap) = &state.last_snapshot {
                        if matches!(snap.status, OperationStatus::Completed) {
                            let done_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("✅ Done")
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                )
                                .fill(egui::Color32::from_rgb(30, 130, 70))
                                .min_size(egui::vec2(120.0, 32.0)),
                            );
                            if done_btn.clicked() {
                                refresh_connection_id = Some(state.connection_id);
                                close_dialog = true;
                            }
                        } else {
                            let retry_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("🔄 Try Again").strong(),
                                )
                                .min_size(egui::vec2(140.0, 32.0)),
                            );
                            if retry_btn.clicked() {
                                state.last_snapshot = None;
                                state.tracker = None;
                                state.cancel_token = None;
                                state.validation_error = None;
                            }

                            let close_btn = ui.add(
                                egui::Button::new("Close")
                                    .min_size(egui::vec2(80.0, 32.0)),
                            );
                            if close_btn.clicked() {
                                close_dialog = true;
                            }
                        }
                    } else {
                        let target_name_clean = state.target_database_name.trim();
                        let can_start = !target_name_clean.is_empty()
                            && target_name_clean != state.source_database_name.trim()
                            && (state.connection_type != DatabaseType::SQLite || state.target_file.is_some());

                        let start_btn = ui.add_enabled(
                            can_start,
                            egui::Button::new(
                                egui::RichText::new("🚀 Start Copy")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(egui::Color32::from_rgb(30, 130, 70))
                            .min_size(egui::vec2(140.0, 32.0)),
                        );
                        if start_btn.clicked() {
                            if target_name_clean.is_empty() {
                                state.validation_error = Some("New database name cannot be empty.".to_string());
                            } else if target_name_clean == state.source_database_name.trim() {
                                state.validation_error = Some("New database name must be different from source database.".to_string());
                            } else if state.connection_type == DatabaseType::SQLite && state.target_file.is_none() {
                                state.validation_error = Some("Please select destination file for SQLite database.".to_string());
                            } else {
                                state.validation_error = None;
                                start_copy_requested = true;
                            }
                        }

                        let cancel_btn = ui.add(
                            egui::Button::new("Cancel")
                                .min_size(egui::vec2(80.0, 32.0)),
                        );
                        if cancel_btn.clicked() {
                            close_dialog = true;
                        }
                    }
                });
            }
        });

    if close_dialog {
        open = false;
    }
    tabular.show_copy_database_dialog = open;

    if let Some(conn_id) = refresh_connection_id {
        tabular.refresh_connection(conn_id);
        crate::sidebar_database::refresh_connections_tree(tabular);
    }

    if cancel_requested {
        if let Some(state) = &mut tabular.copy_database_state {
            if let Some(token) = &state.cancel_token {
                token.store(true, Ordering::Relaxed);
            }
        }
    }

    if start_copy_requested {
        if let Some(state) = &mut tabular.copy_database_state {
            if let Some(conn) = tabular
                .connections
                .iter()
                .find(|c| c.id == Some(state.connection_id))
            {
                let target_file_path = state.target_file.clone().unwrap_or_else(|| {
                    PathBuf::from(&state.target_database_name)
                });

                let options = CopyDatabaseOptions {
                    source_database_name: state.source_database_name.trim().to_string(),
                    target_database_name: state.target_database_name.trim().to_string(),
                    target_file: state.target_file.clone(),
                    custom_binary_path: if state.custom_binary_path.trim().is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(&state.custom_binary_path))
                    },
                    drop_target_if_exists: state.drop_target_if_exists,
                    include_routines: state.include_routines,
                };

                let tracker = Arc::new(Mutex::new(ProgressTracker::new(
                    OperationType::CopyDatabase,
                    format!("{} → {}", state.source_database_name, state.target_database_name),
                    target_file_path,
                )));
                let cancel_token = Arc::new(AtomicBool::new(false));

                state.tracker = Some(tracker.clone());
                state.cancel_token = Some(cancel_token.clone());
                state.is_running = true;

                BackupRestoreRunner::run_copy_database(conn, options, tracker, cancel_token);
            }
        }
    }
}
