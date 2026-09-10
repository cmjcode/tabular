use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::export_import_all::{
    import_all_data, inspect_archive, ConflictStrategy, ExportAllManifest,
    ExportAllOptions, ExportSummary, ImportAllOptions, ImportSummary,
};
use crate::rfd;
use crate::window_egui::Tabular;

// ─── Export Dialog State ────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ExportAllDialogState {
    pub target_file: Option<PathBuf>,
    pub options: ExportAllOptions,
    pub is_running: bool,
    pub status_message: Option<String>,
    pub error_message: Option<String>,
    pub summary: Option<ExportSummary>,
    pub result_receiver: Option<Arc<Mutex<Option<Result<ExportSummary, String>>>>>,
}

impl std::fmt::Debug for ExportAllDialogState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportAllDialogState")
            .field("target_file", &self.target_file)
            .field("options", &self.options)
            .field("is_running", &self.is_running)
            .field("status_message", &self.status_message)
            .field("error_message", &self.error_message)
            .field("summary", &self.summary)
            .finish()
    }
}

impl Default for ExportAllDialogState {
    fn default() -> Self {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let default_filename = format!("tabular_backup_{}.zip", timestamp);
        let default_path = dirs::download_dir()
            .or_else(|| dirs::home_dir())
            .map(|p| p.join(default_filename));

        Self {
            target_file: default_path,
            options: ExportAllOptions::default(),
            is_running: false,
            status_message: None,
            error_message: None,
            summary: None,
            result_receiver: None,
        }
    }
}

// ─── Import Dialog State ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ImportAllDialogState {
    pub archive_path: Option<PathBuf>,
    pub manifest_preview: Option<ExportAllManifest>,
    pub options: ImportAllOptions,
    pub is_running: bool,
    pub status_message: Option<String>,
    pub error_message: Option<String>,
    pub summary: Option<ImportSummary>,
}

impl Default for ImportAllDialogState {
    fn default() -> Self {
        Self {
            archive_path: None,
            manifest_preview: None,
            options: ImportAllOptions::default(),
            is_running: false,
            status_message: None,
            error_message: None,
            summary: None,
        }
    }
}

// ─── Render Export Dialog ───────────────────────────────────────────────────

pub fn render_export_all_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    let mut is_open = tabular.show_export_all_dialog;
    let mut state = tabular
        .export_all_state
        .take()
        .unwrap_or_default();
    let mut close_requested = false;

    // Check background export thread if running
    if state.is_running {
        if let Some(receiver) = state.result_receiver.clone() {
            if let Ok(mut lock) = receiver.try_lock() {
                if let Some(result) = lock.take() {
                    state.is_running = false;
                    state.result_receiver = None;
                    match result {
                        Ok(summary) => {
                            state.summary = Some(summary);
                            state.status_message = Some("Export completed successfully!".to_string());
                            state.error_message = None;
                        }
                        Err(err) => {
                            state.error_message = Some(format!("Export failed: {}", err));
                        }
                    }
                }
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    crate::window_egui::style::render_modal_backdrop(ctx, "export_all_dialog", is_open);

    let screen_rect = ctx.content_rect();
    let dialog_w = (screen_rect.width() - 32.0).min(560.0).max(420.0);

    egui::Window::new("📦 Export All Application Data (ZIP)")
        .open(&mut is_open)
        .default_width(dialog_w)
        .max_width(dialog_w)
        .min_width(dialog_w)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.add_space(4.0);

            // ── Top Header Banner ──
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("📦").size(24.0));
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Full Workspace Backup")
                                .strong()
                                .size(14.0),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Package your Database Connections, Queries, HTTP API Collections, and Query History into a portable ZIP file.",
                            )
                            .small()
                            .color(egui::Color32::from_gray(170)),
                        );
                    });
                });
            });

            ui.add_space(8.0);

            // ── In-Progress Progress Indicator ──
            if state.is_running {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Exporting workspace data to ZIP archive, please wait...")
                                .color(egui::Color32::from_rgb(100, 180, 255))
                                .strong(),
                        );
                    });
                });
                ui.add_space(8.0);
            }

            // ── Summary Card (if already exported) ──
            if let Some(ref summary) = state.summary {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("✅")
                                .size(18.0)
                                .color(egui::Color32::from_rgb(80, 200, 120)),
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Archive successfully created!")
                                    .strong()
                                    .color(egui::Color32::from_rgb(80, 200, 120)),
                            );
                            ui.label(format!(
                                "Path: {}",
                                summary.archive_path.display()
                            ));
                            ui.label(format!(
                                "Size: {:.2} KB",
                                summary.zip_file_size as f64 / 1024.0
                            ));
                            ui.label(format!(
                                "Included: {} connections, {} folders, {} queries, {} HTTP workspaces, {} history items",
                                summary.connections_count,
                                summary.folders_count,
                                summary.queries_count,
                                summary.http_workspaces_count,
                                summary.history_count
                            ));
                        });
                    });
                });
                ui.add_space(8.0);
            }

            // ── Error Banner (if any) ──
            if let Some(ref err) = state.error_message {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("❌").size(16.0));
                        ui.label(
                            egui::RichText::new(err)
                                .color(egui::Color32::from_rgb(240, 80, 80))
                                .small(),
                        );
                    });
                });
                ui.add_space(8.0);
            }

            // ── Destination Path Selection ──
            ui.add_enabled_ui(!state.is_running, |ui| {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new("📁 Backup Destination Path").strong());
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        let mut path_str = state
                            .target_file
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();

                        let button_width = 75.0;
                        let spacing = ui.spacing().item_spacing.x;
                        let text_edit_width = (ui.available_width() - button_width - spacing).max(100.0);

                        let text_resp = ui.add_sized(
                            [text_edit_width, 22.0],
                            egui::TextEdit::singleline(&mut path_str)
                                .hint_text("Choose target zip file path..."),
                        );
                        if text_resp.changed() {
                            state.target_file = Some(PathBuf::from(path_str));
                        }

                        if ui.add_sized([button_width, 22.0], egui::Button::new("Browse...")).clicked() {
                            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
                            let default_name = format!("tabular_backup_{}.zip", timestamp);

                            let mut dialog = rfd::FileDialog::new()
                                .set_file_name(&default_name)
                                .add_filter("ZIP Archive (*.zip)", &["zip"]);

                            if let Some(ref current) = state.target_file {
                                if let Some(dir) = current.parent() {
                                    dialog = dialog.set_directory(dir);
                                }
                            }

                            if let Some(path) = dialog.save_file() {
                                state.target_file = Some(path);
                            }
                        }
                    });
                });
            });

            ui.add_space(8.0);

            // ── Data Inclusions Checkboxes ──
            ui.add_enabled_ui(!state.is_running, |ui| {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new("📋 Data to Include in Backup").strong());
                    ui.add_space(4.0);

                    ui.checkbox(
                        &mut state.options.include_connections,
                        format!(
                            "🔌 Database Connections & Folders ({} connections)",
                            tabular.connections.len()
                        ),
                    );
                    ui.checkbox(
                        &mut state.options.include_queries,
                        "📝 Saved Queries (SQL scripts & directory hierarchy)",
                    );
                    ui.checkbox(
                        &mut state.options.include_http_api,
                        format!(
                            "🌐 HTTP API Collections ({} workspaces)",
                            tabular.yaak_workspaces.len()
                        ),
                    );
                    ui.checkbox(
                        &mut state.options.include_history,
                        format!(
                            "📜 Query Execution History ({} items)",
                            tabular.history_items.len()
                        ),
                    );
                });
            });

            ui.add_space(12.0);

            // ── Bottom Action Buttons ──
            ui.horizontal(|ui| {
                if ui.add_enabled(!state.is_running, egui::Button::new("Cancel")).clicked() {
                    close_requested = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let has_selected = state.options.include_connections
                        || state.options.include_queries
                        || state.options.include_http_api
                        || state.options.include_history;

                    let can_export = has_selected
                        && state.target_file.is_some()
                        && !state.is_running;

                    let btn = egui::Button::new(
                        egui::RichText::new(if state.is_running {
                            "⏳ Exporting..."
                        } else {
                            "📦 Export Now"
                        })
                        .strong()
                        .color(egui::Color32::WHITE),
                    )
                    .fill(if state.is_running {
                        egui::Color32::from_gray(80)
                    } else {
                        egui::Color32::from_rgb(45, 120, 220)
                    });

                    if ui.add_enabled(can_export, btn).clicked() {
                        if let Some(target_path) = state.target_file.clone() {
                            state.error_message = None;
                            state.summary = None;
                            state.is_running = true;

                            let receiver = Arc::new(Mutex::new(None));
                            state.result_receiver = Some(Arc::clone(&receiver));

                            let options = state.options.clone();
                            let connections = tabular.connections.clone();
                            let connection_folders = tabular.connection_folders.clone();
                            let yaak_workspaces = tabular.yaak_workspaces.clone();
                            let history_items = tabular.history_items.clone();

                            std::thread::spawn(move || {
                                let res = crate::export_import_all::export_all_data_payload(
                                    &target_path,
                                    &options,
                                    &connections,
                                    &connection_folders,
                                    &yaak_workspaces,
                                    &history_items,
                                )
                                .map_err(|e| e.to_string());

                                if let Ok(mut lock) = receiver.lock() {
                                    *lock = Some(res);
                                }
                            });

                            ctx.request_repaint();
                        }
                    }
                });
            });
        });

    if close_requested {
        is_open = false;
    }
    tabular.show_export_all_dialog = is_open;
    tabular.export_all_state = Some(state);
}

// ─── Render Import Dialog ───────────────────────────────────────────────────

pub fn render_import_all_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    let mut is_open = tabular.show_import_all_dialog;
    let mut state = tabular
        .import_all_state
        .take()
        .unwrap_or_default();
    let mut close_requested = false;

    crate::window_egui::style::render_modal_backdrop(ctx, "import_all_dialog", is_open);

    let screen_rect = ctx.content_rect();
    let dialog_w = (screen_rect.width() - 32.0).min(580.0).max(440.0);

    egui::Window::new("📥 Import & Restore All Data (ZIP)")
        .open(&mut is_open)
        .default_width(dialog_w)
        .max_width(dialog_w)
        .min_width(dialog_w)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.add_space(4.0);

            // ── Top Header Banner ──
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("📥").size(24.0));
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Workspace Restore")
                                .strong()
                                .size(14.0),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Restore Database Connections, Queries, HTTP API Collections, and History from a backup ZIP archive.",
                            )
                            .small()
                            .color(egui::Color32::from_gray(170)),
                        );
                    });
                });
            });

            ui.add_space(8.0);

            // ── Summary Card (if already restored) ──
            if let Some(ref summary) = state.summary {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("✅")
                                .size(18.0)
                                .color(egui::Color32::from_rgb(80, 200, 120)),
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Data restored successfully!")
                                    .strong()
                                    .color(egui::Color32::from_rgb(80, 200, 120)),
                            );
                            ui.label(format!(
                                "Restored: {} connections, {} folders, {} queries, {} HTTP workspaces, {} history items",
                                summary.connections_restored,
                                summary.folders_restored,
                                summary.queries_restored,
                                summary.http_workspaces_restored,
                                summary.history_restored
                            ));
                        });
                    });
                });
                ui.add_space(8.0);
            }

            // ── Error Banner (if any) ──
            if let Some(ref err) = state.error_message {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("❌").size(16.0));
                        ui.label(
                            egui::RichText::new(err)
                                .color(egui::Color32::from_rgb(240, 80, 80))
                                .small(),
                        );
                    });
                });
                ui.add_space(8.0);
            }

            // ── Archive File Selection ──
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("📁 Select Backup Archive (.zip)").strong());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let mut path_str = state
                        .archive_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let button_width = 75.0;
                    let spacing = ui.spacing().item_spacing.x;
                    let text_edit_width = (ui.available_width() - button_width - spacing).max(100.0);

                    let text_resp = ui.add_sized(
                        [text_edit_width, 22.0],
                        egui::TextEdit::singleline(&mut path_str)
                            .hint_text("Choose tabular backup .zip file..."),
                    );
                    if text_resp.changed() {
                        let path = PathBuf::from(path_str);
                        if path.is_file() {
                            eprintln!("[RESTORE-UI] Path entered: '{}'. Inspecting archive...", path.display());
                            match inspect_archive(&path) {
                                Ok(manifest) => {
                                    eprintln!(
                                        "[RESTORE-UI] ✅ Archive inspected: {} conns, {} folders, {} queries, {} http, {} history",
                                        manifest.counts.connections,
                                        manifest.counts.connection_folders,
                                        manifest.counts.queries,
                                        manifest.counts.http_workspaces,
                                        manifest.counts.history_items
                                    );
                                    state.manifest_preview = Some(manifest);
                                    state.error_message = None;
                                }
                                Err(e) => {
                                    eprintln!("[RESTORE-UI] ❌ Archive inspect failed: {}", e);
                                    state.error_message = Some(format!("Invalid archive: {}", e));
                                    state.manifest_preview = None;
                                }
                            }
                            state.archive_path = Some(path);
                        } else {
                            state.archive_path = Some(path);
                            state.manifest_preview = None;
                        }
                    }

                    if ui.add_sized([button_width, 22.0], egui::Button::new("Browse...")).clicked() {
                        let dialog = rfd::FileDialog::new()
                            .add_filter("ZIP Archive (*.zip)", &["zip"]);

                        if let Some(path) = dialog.pick_file() {
                            eprintln!("[RESTORE-UI] User selected file from picker: '{}'. Inspecting archive...", path.display());
                            match inspect_archive(&path) {
                                Ok(manifest) => {
                                    eprintln!(
                                        "[RESTORE-UI] ✅ Archive inspected: {} conns, {} folders, {} queries, {} http, {} history",
                                        manifest.counts.connections,
                                        manifest.counts.connection_folders,
                                        manifest.counts.queries,
                                        manifest.counts.http_workspaces,
                                        manifest.counts.history_items
                                    );
                                    state.manifest_preview = Some(manifest);
                                    state.error_message = None;
                                }
                                Err(e) => {
                                    eprintln!("[RESTORE-UI] ❌ Archive inspect failed: {}", e);
                                    state.error_message = Some(format!("Invalid archive: {}", e));
                                    state.manifest_preview = None;
                                }
                            }
                            state.archive_path = Some(path);
                        }
                    }
                });
            });

            ui.add_space(8.0);

            // ── Archive Manifest Preview (if archive selected) ──
            if let Some(ref manifest) = state.manifest_preview {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new("🔍 Archive Contents Preview").strong());
                    ui.add_space(2.0);
                    if !manifest.exported_at.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("Exported At: {}", manifest.exported_at))
                                .small()
                                .color(egui::Color32::from_gray(160)),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!("• Connections: {}", manifest.counts.connections));
                        ui.label(format!("• Folders: {}", manifest.counts.connection_folders));
                        ui.label(format!("• Queries: {}", manifest.counts.queries));
                    });
                    ui.horizontal(|ui| {
                        ui.label(format!("• HTTP Workspaces: {}", manifest.counts.http_workspaces));
                        ui.label(format!("• History Items: {}", manifest.counts.history_items));
                    });
                });
                ui.add_space(8.0);
            }

            // ── Data Restoration Selection Checkboxes ──
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("📋 Data to Include in Restore").strong());
                ui.add_space(4.0);

                ui.checkbox(
                    &mut state.options.include_connections,
                    "🔌 Database Connections & Folders",
                );
                ui.checkbox(
                    &mut state.options.include_queries,
                    "📝 Saved Queries (SQL scripts & directory hierarchy)",
                );
                ui.checkbox(
                    &mut state.options.include_http_api,
                    "🌐 HTTP API Collections (workspaces & requests)",
                );
                ui.checkbox(
                    &mut state.options.include_history,
                    "📜 Query Execution History",
                );
            });

            ui.add_space(8.0);

            // ── Conflict Resolution Strategy ──
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("⚙️ Conflict Handling Strategy").strong());
                ui.add_space(4.0);

                ui.radio_value(
                    &mut state.options.conflict_strategy,
                    ConflictStrategy::MergeKeepExisting,
                    ConflictStrategy::MergeKeepExisting.display_name(),
                );
                ui.radio_value(
                    &mut state.options.conflict_strategy,
                    ConflictStrategy::MergeOverwrite,
                    ConflictStrategy::MergeOverwrite.display_name(),
                );
                ui.radio_value(
                    &mut state.options.conflict_strategy,
                    ConflictStrategy::CleanRestore,
                    ConflictStrategy::CleanRestore.display_name(),
                );

                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(state.options.conflict_strategy.description())
                        .small()
                        .color(egui::Color32::from_gray(160)),
                );
            });

            ui.add_space(12.0);

            // ── Bottom Action Buttons ──
            ui.horizontal(|ui| {
                if ui.add_enabled(!state.is_running, egui::Button::new("Cancel")).clicked() {
                    close_requested = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let has_selected = state.options.include_connections
                        || state.options.include_queries
                        || state.options.include_http_api
                        || state.options.include_history;

                    let file_exists = state
                        .archive_path
                        .as_ref()
                        .map(|p| p.is_file())
                        .unwrap_or(false);

                    let can_restore = has_selected && file_exists && !state.is_running;

                    let btn = egui::Button::new(
                        egui::RichText::new(if state.is_running {
                            "⏳ Restoring..."
                        } else {
                            "📥 Restore Now"
                        })
                        .strong()
                        .color(egui::Color32::WHITE),
                    )
                    .fill(if state.is_running {
                        egui::Color32::from_gray(80)
                    } else {
                        egui::Color32::from_rgb(40, 160, 90)
                    });

                    // If restore was triggered on previous frame, perform it now so egui rendered "Restoring..."
                    if state.is_running {
                        if let Some(archive_path) = state.archive_path.clone() {
                            eprintln!("[RESTORE-UI] Starting restore execution for: {}", archive_path.display());
                            match import_all_data(tabular, &archive_path, &state.options) {
                                Ok(summary) => {
                                    eprintln!("[RESTORE-UI] ✅ Restore succeeded!");
                                    state.summary = Some(summary);
                                    state.status_message = Some("Restore completed successfully!".to_string());
                                }
                                Err(e) => {
                                    eprintln!("[RESTORE-UI] ❌ Restore failed with error: {}", e);
                                    log::error!("[RESTORE-UI] Restore failed with error: {}", e);
                                    state.error_message = Some(format!("Restore failed: {}", e));
                                }
                            }
                        } else {
                            eprintln!("[RESTORE-UI] ❌ No archive path available in state!");
                            state.error_message = Some("Restore failed: No backup archive selected".to_string());
                        }
                        state.is_running = false;
                        ctx.request_repaint();
                    } else if ui.add_enabled(can_restore, btn).clicked() {
                        eprintln!("[RESTORE-UI] 'Restore Now' button clicked. Setting is_running=true.");
                        state.error_message = None;
                        state.summary = None;
                        state.status_message = None;
                        state.is_running = true;
                        ctx.request_repaint();
                    }
                });
            });
        });

    if close_requested {
        is_open = false;
    }
    tabular.show_import_all_dialog = is_open;
    tabular.import_all_state = Some(state);
}
