//! Menu bar aplikasi: File, Edit, View, Database, Project, Tools, Help.
//!
//! Sebelumnya fitur hanya bisa dicapai lewat popup gear, menu konteks sidebar,
//! atau command palette. Menu bar ini memakai eksekutor yang sama dengan
//! palette (`editor::execute_command`) sehingga satu perintah selalu berperilaku
//! sama dari mana pun dipanggil. Perintah yang sebelumnya hanya ada di menu
//! konteks (Backup, Restore, Schema Diff, ...) ditambahkan di [`run_extra_command`]
//! dan juga muncul di palette lewat [`PALETTE_COMMANDS`].

use eframe::egui;

use super::Tabular;
use crate::keymap::Action;

/// Satu baris menu.
enum Entry {
    /// `(label, kunci perintah, shortcut keymap)`.
    Item(&'static str, &'static str, Option<Action>),
    Separator,
}

use Entry::{Item, Separator};

/// Struktur menu. Label diakhiri "…" bila membuka dialog.
#[rustfmt::skip]
const MENUS: &[(&str, &[Entry])] = &[
    ("File", &[
        Item("New Query Tab", "Query: New Tab", Some(Action::NewTab)),
        Item("Open SQL File…", "Query: Open SQL File...", Some(Action::OpenFile)),
        Item("Open File…", "File: Open File...", None),
        Item("Open Data File…", "Data: Open Data File", None),
        Separator,
        Item("Save", "Query: Save Tab", Some(Action::SaveTab)),
        Item("Close Tab", "Query: Close Tab", Some(Action::CloseTab)),
        Separator,
        Item("Import Data into Table…", "Data: Import CSV", None),
        Item("Export Results…", "Data: Export with Options", None),
        Separator,
        Item("Export App Data (ZIP)…", "File: Export All Data (ZIP)", None),
        Item("Import App Data (ZIP)…", "File: Import All Data (ZIP)", None),
    ]),
    ("Edit", &[
        Item("Find and Replace", "Editor: Toggle Find & Replace", Some(Action::FindReplace)),
        Item("Go to Definition", "Editor: Go to Definition", Some(Action::GoToDefinition)),
        Item("Rename Symbol", "Editor: Rename Symbol", Some(Action::RenameSymbol)),
        Separator,
        Item("Next Problem", "Editor: Next Problem", Some(Action::NextProblem)),
        Item("Previous Problem", "Editor: Previous Problem", Some(Action::PrevProblem)),
        Item("Quick Fix", "Editor: Quick Fix", Some(Action::QuickFix)),
        Item("Show Problems", "Editor: Show Problems", None),
        Item("Toggle Error Checking While Typing", "Editor: Toggle Live Error Checking", None),
        Separator,
        Item("Toggle Word Wrap", "Editor: Toggle Word Wrap", None),
        Item("Toggle Line Numbers", "Editor: Toggle Line Numbers", None),
        Separator,
        Item("Preferences…", "Preferences: Settings", Some(Action::OpenSettings)),
    ]),
    ("View", &[
        Item("Command Palette…", "View: Command Palette", Some(Action::QuickOpen)),
        Item("Toggle Sidebar", "View: Toggle Sidebar", None),
        Item("Toggle AI Assistant", "View: Toggle AI Assistant", Some(Action::ToggleAiPanel)),
        Item("Toggle Auto-Hide Menu Bar", "View: Toggle Auto-Hide Menu Bar", None),
        Item("Query Insights", "Query: Insights (Most Run / Slowest)", None),
        Separator,
        Item("Refresh", "View: Refresh", Some(Action::Refresh)),
        Item("Color Theme…", "Preferences: Color Theme", None),
    ]),
    ("Database", &[
        Item("New Connection…", "Database: New Connection", None),
        Item("Edit Connection…", "Database: Edit Connection", None),
        Item("Refresh Connection", "Database: Refresh Connection", None),
        Separator,
        Item("Run Query", "Query: Run", Some(Action::RunQuery)),
        Item("Explain Query", "Query: Explain", Some(Action::ExplainQuery)),
        Item("Toggle Manual Commit", "Transaction: Begin / Toggle", Some(Action::ToggleTransactionMode)),
        Item("Commit", "Transaction: Commit", None),
        Item("Roll Back", "Transaction: Rollback", None),
        Separator,
        Item("Backup Database…", "Database: Backup", None),
        Item("Restore Database…", "Database: Restore", None),
        Item("Copy Database…", "Database: Copy Database", None),
        Item("Schema Diff…", "Database: Schema Diff", None),
        Item("Transfer Tables…", "Data: Transfer Tables", None),
        Item("Compare Two Tables…", "Data: Compare Data", None),
        Separator,
        Item("Users and Privileges", "DBA: Manage Users & Privileges", None),
        Item("Process Monitor", "DBA: Live Process Monitor", None),
        Item("Server Dashboard", "DBA: Server Dashboard", None),
        Item("Lock Tree", "DBA: Deadlock & Lock Tree", None),
    ]),
    ("Project", &[
        Item("Database Diagram", "Project: Database Diagram", None),
        Item("Integration Tests", "Project: Integration Tests", None),
        Item("Data Health", "Project: Data Health", None),
        Item("Set Up from Git Folder…", "Project: Set Up from Git Folder", None),
    ]),
    ("Tools", &[
        Item("Plugins…", "Plugins: Open Plugins Manager", None),
        Item("Agent Access (MCP Server)…", "Tools: Agent Access", None),
        Item("Decrypt Exported File…", "Data: Decrypt Exported File", None),
        Separator,
        Item("Open Logs Folder", "Tools: Open Logs Folder", None),
        Item("Copy Diagnostics", "Tools: Copy Diagnostics", None),
    ]),
    ("Help", &[
        Item("Keyboard Shortcuts", "Help: Keyboard Shortcuts", Some(Action::ShowShortcuts)),
        Item("Documentation", "Help: Documentation", None),
        Item("Check for Updates", "Help: Check for Updates", None),
        Separator,
        Item("About Tabular", "Help: About Tabular", None),
    ]),
];

/// Perintah baru yang juga ditampilkan di command palette: `(judul, deskripsi)`.
pub(crate) const PALETTE_COMMANDS: &[(&str, &str)] = &[
    (
        "Database: New Connection",
        "Add a database or API connection",
    ),
    (
        "Database: Edit Connection",
        "Edit the active tab's connection",
    ),
    (
        "Database: Refresh Connection",
        "Reload databases and tables of the active connection",
    ),
    ("Database: Backup", "Back up the active database"),
    (
        "Database: Restore",
        "Restore a backup into the active database",
    ),
    (
        "Database: Copy Database",
        "Copy the active database to another connection",
    ),
    (
        "Database: Schema Diff",
        "Compare the schema of the active database with another",
    ),
    (
        "Project: Database Diagram",
        "Open the diagram of the active database",
    ),
    (
        "Project: Integration Tests",
        "Open integration tests of the active project",
    ),
    (
        "Project: Data Health",
        "Run read-only data checks that find broken data in the active project",
    ),
    (
        "Project: Set Up from Git Folder",
        "Create connections and API folders from a repository",
    ),
    (
        "Tools: Agent Access",
        "Control what AI agents may do through the MCP server",
    ),
    ("Tools: Open Logs Folder", "Show Tabular's log files"),
    (
        "Tools: Copy Diagnostics",
        "Copy an environment report for bug reports",
    ),
    ("View: Toggle Sidebar", "Show or hide the sidebar"),
    (
        "View: Toggle AI Assistant",
        "Show or hide the AI Assistant panel",
    ),
    (
        "View: Toggle Auto-Hide Menu Bar",
        "Automatically hide the top menu bar until hovered",
    ),
    ("Help: Documentation", "Open the Tabular documentation"),
    (
        "Help: Check for Updates",
        "Look for a newer version of Tabular",
    ),
    ("Help: About Tabular", "Version and license information"),
];

const DOCS_URL: &str = "https://github.com/tabular-id/tabular#readme";

impl Tabular {
    /// Daftarkan database contoh (SQLite lokal) lalu muat ulang sidebar.
    pub(crate) fn add_sample_database(&mut self) {
        let Some(pool) = self.db_pool.clone() else {
            self.toasts
                .error("The local database is not available yet; try again in a moment.");
            return;
        };
        match crate::runtime::block_on(crate::sample_data::add_sample_connection(&pool)) {
            Some(Ok(id)) => {
                crate::sidebar_database::load_connections(self);
                self.maybe_auto_sync_connection(id);
                self.toasts
                    .success("Added \"Sample Database\" to your connections.");
            }
            Some(Err(e)) => self
                .toasts
                .error(format!("Could not create the sample database: {e}")),
            None => self.toasts.error("Could not create the sample database."),
        }
    }

    /// Koneksi dan database tujuan perintah menu Database: tab aktif dulu,
    /// lalu hasil yang sedang tampil.
    fn menu_target(&self) -> Option<(i64, String)> {
        let tab = self.query_tabs.get(self.active_tab_index);
        let conn_id = tab
            .and_then(|t| t.connection_id)
            .or(self.current_connection_id)?;
        let database = tab
            .and_then(|t| t.database_name.clone())
            .filter(|d| !d.is_empty())
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|c| c.id == Some(conn_id))
                    .map(|c| c.database.clone())
            })
            .unwrap_or_default();
        Some((conn_id, database))
    }

    fn menu_target_or_hint(&mut self) -> Option<(i64, String)> {
        let target = self.menu_target();
        if target.is_none() {
            self.toasts.info(
                "Open a query tab on a connection first (or double-click a connection in the sidebar).",
            );
        }
        target
    }

    /// Gambar menu bar. Dipanggil sekali per frame dari `ui()`.
    pub(crate) fn render_app_menu_bar(&mut self, root_ui: &mut egui::Ui) {
        let mut chosen: Option<&'static str> = None;
        let mut toggle_auto_hide = false;

        let ctx = root_ui.ctx().clone();

        if !self.auto_hide_menu_bar {
            egui::Panel::top("app_menu_bar")
                .resizable(false)
                .show(root_ui, |ui| {
                    self.render_menu_bar_content(ui, &mut chosen, &mut toggle_auto_hide);
                });
        } else {
            let hover_pos = ctx.input(|i| i.pointer.hover_pos());
            let alt_pressed = ctx.input(|i| {
                i.key_pressed(egui::Key::F10)
                    || (i.modifiers.alt && !i.modifiers.command && !i.modifiers.shift)
            });
            if alt_pressed {
                self.menu_bar_revealed = !self.menu_bar_revealed;
            }

            let any_popup_open = egui::Popup::is_any_open(&ctx);
            let in_trigger_zone = hover_pos.map_or(false, |p| p.y <= 6.0);
            let in_menu_bar_zone = hover_pos.map_or(false, |p| p.y <= 36.0);

            if in_trigger_zone {
                self.menu_bar_revealed = true;
            } else if self.menu_bar_revealed && !in_menu_bar_zone && !any_popup_open {
                self.menu_bar_revealed = false;
            }

            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !any_popup_open {
                self.menu_bar_revealed = false;
            }

            if self.menu_bar_revealed {
                egui::Area::new(egui::Id::new("app_menu_bar_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .show(&ctx, |ui| {
                        let screen_width = ctx.content_rect().width();
                        let visuals = ui.visuals().clone();
                        egui::Frame::new()
                            .fill(visuals.panel_fill)
                            .stroke(egui::Stroke::new(
                                1.0,
                                visuals.widgets.noninteractive.bg_stroke.color,
                            ))
                            .shadow(visuals.window_shadow)
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.set_width(screen_width);
                                self.render_menu_bar_content(
                                    ui,
                                    &mut chosen,
                                    &mut toggle_auto_hide,
                                );
                            });
                    });
            }
        }

        if toggle_auto_hide {
            self.auto_hide_menu_bar = !self.auto_hide_menu_bar;
            self.prefs_dirty = true;
            if self.auto_hide_menu_bar {
                self.toasts.info(
                    "Menu bar is now set to auto-hide. Move cursor to the top edge to reveal it.",
                );
            } else {
                self.menu_bar_revealed = false;
                self.toasts.info("Menu bar is now pinned.");
            }
        }

        if let Some(command) = chosen {
            self.menu_bar_revealed = false;
            crate::editor::execute_command(self, command);
        }
    }

    /// Render isi baris menu beserta tombol pin/auto-hide di sebelah kanan.
    fn render_menu_bar_content(
        &self,
        ui: &mut egui::Ui,
        chosen: &mut Option<&'static str>,
        toggle_auto_hide: &mut bool,
    ) {
        egui::MenuBar::new().ui(ui, |ui| {
            for (title, entries) in MENUS {
                ui.menu_button(*title, |ui| {
                    ui.set_min_width(220.0);
                    for entry in entries.iter() {
                        match entry {
                            Separator => {
                                ui.separator();
                            }
                            Item(label, command, shortcut) => {
                                let mut button = egui::Button::new(*label);
                                if let Some(action) = shortcut {
                                    button = button.shortcut_text(self.keymap.label(*action));
                                }
                                if ui.add(button).clicked() {
                                    *chosen = Some(command);
                                    ui.close();
                                }
                            }
                        }
                    }
                });
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, label, tooltip) = if self.auto_hide_menu_bar {
                    (
                        egui_icons::icons::ICON_PUSH_PIN.codepoint,
                        "Pin Menu",
                        "Auto-hide is enabled: click to keep menu bar pinned",
                    )
                } else {
                    (
                        egui_icons::icons::ICON_VISIBILITY_OFF.codepoint,
                        "Auto-hide",
                        "Menu bar is pinned: click to automatically hide it",
                    )
                };
                let btn = egui::Button::new(
                    egui::RichText::new(format!("{} {}", icon, label))
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                )
                .frame(false);
                if ui.add(btn).on_hover_text(tooltip).clicked() {
                    *toggle_auto_hide = true;
                }
            });
        });
    }
}

/// Jalankan perintah yang tidak dikenal eksekutor palette lama. `true` bila
/// perintah dikenali.
pub(crate) fn run_extra_command(t: &mut Tabular, key: &str) -> bool {
    match key {
        "Database: New Connection" => {
            t.new_connection.folder = Some("Default".to_string());
            t.show_add_connection = true;
        }
        "Database: Edit Connection" => {
            if let Some((conn_id, _)) = t.menu_target_or_hint() {
                crate::sidebar_database::start_edit_connection(t, conn_id);
            }
        }
        "Database: Refresh Connection" => {
            if let Some((conn_id, _)) = t.menu_target_or_hint() {
                t.refresh_connection(conn_id);
            }
        }
        "Database: Backup" => {
            if let Some((conn_id, db)) = t.menu_target_or_hint() {
                t.show_backup_dialog = true;
                t.backup_state = Some(crate::dialog_backup_restore::BackupDialogState::new(
                    conn_id,
                    db,
                    &t.connections,
                ));
            }
        }
        "Database: Restore" => {
            if let Some((conn_id, db)) = t.menu_target_or_hint() {
                t.show_restore_dialog = true;
                t.restore_state = Some(crate::dialog_backup_restore::RestoreDialogState::new(
                    conn_id,
                    db,
                    &t.connections,
                ));
            }
        }
        "Database: Copy Database" => {
            if let Some((conn_id, db)) = t.menu_target_or_hint() {
                t.show_copy_database_dialog = true;
                t.copy_database_state =
                    Some(crate::dialog_copy_database::CopyDatabaseDialogState::new(
                        conn_id,
                        db,
                        &t.connections,
                    ));
            }
        }
        "Database: Schema Diff" => {
            if let Some((conn_id, db)) = t.menu_target_or_hint() {
                t.show_schema_diff_dialog = true;
                t.schema_diff_state = Some(crate::models::structs::SchemaDiffState::new(
                    conn_id,
                    db,
                    &t.connections,
                ));
            }
        }
        "Project: Database Diagram" => {
            if let Some((conn_id, db)) = t.menu_target_or_hint() {
                t.open_database_diagram(conn_id, db);
            }
        }
        "Project: Integration Tests" => {
            super::integration_test_ui::open_for_active_project(t);
        }
        "Project: Data Health" => {
            if let Some(ctx) = t.egui_ctx.clone() {
                super::data_health_ui::open_for_active_project(t, &ctx);
            }
        }
        "Project: Set Up from Git Folder" => {
            super::repo_setup_jobs::pick_and_start(t);
        }
        "Tools: Agent Access" => {
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            t.open_agent_access();
        }
        "Tools: Open Logs Folder" => {
            let dir = crate::app_logging::logs_dir();
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = crate::url_opener::open_url(&dir.to_string_lossy()) {
                t.toasts
                    .error(format!("Cannot open {}: {}", dir.display(), e));
            }
        }
        "Tools: Copy Diagnostics" => {
            t.pending_clipboard_text = Some(crate::app_logging::diagnostics_report());
            t.toasts.success(
                "Diagnostics copied. Review it before sharing — recent log lines are included.",
            );
        }
        "View: Toggle Sidebar" => {
            t.sidebar_visible = !t.sidebar_visible;
        }
        "View: Toggle Auto-Hide Menu Bar" => {
            t.auto_hide_menu_bar = !t.auto_hide_menu_bar;
            t.prefs_dirty = true;
            if t.auto_hide_menu_bar {
                t.toasts.info(
                    "Menu bar is now set to auto-hide. Move cursor to the top edge to reveal it.",
                );
            } else {
                t.menu_bar_revealed = false;
                t.toasts.info("Menu bar is now pinned.");
            }
        }
        "View: Toggle AI Assistant" => {
            t.show_ai_panel = !t.show_ai_panel;
        }
        "View: Command Palette" => {
            t.show_command_palette = true;
            t.command_palette_input.clear();
            t.command_palette_selected_index = 0;
        }
        "Help: Documentation" => {
            if let Err(e) = crate::url_opener::open_url(DOCS_URL) {
                t.toasts
                    .error(format!("Cannot open the documentation: {e}"));
            }
        }
        "Help: Check for Updates" => {
            if crate::self_update::SELF_UPDATE_SUPPORTED {
                t.check_for_updates(true);
            } else {
                t.toasts
                    .info("Updates for this version are delivered by the App Store.");
            }
        }
        "Help: About Tabular" => {
            t.show_about_dialog = true;
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_label_is_unique_within_its_menu_and_dialogs_use_ellipsis() {
        for (title, entries) in MENUS {
            let mut seen = std::collections::HashSet::new();
            for entry in entries.iter() {
                if let Item(label, _, _) = entry {
                    assert!(seen.insert(*label), "duplicate \"{label}\" in {title}");
                    // Satu bentuk elipsis di menu.
                    assert!(!label.contains("..."), "use … in \"{label}\"");
                }
            }
        }
    }

    #[test]
    fn new_palette_commands_are_handled_by_the_extra_executor() {
        // Kunci yang hanya ada di menu/palette baru harus dikenali; perintah
        // lama ditangani `editor::execute_command`.
        let known_old = [
            "Query: ",
            "File: ",
            "Data: ",
            "Editor: ",
            "Transaction: ",
            "DBA: ",
            "Plugins: ",
            "Preferences: ",
            "Help: Keyboard Shortcuts",
            "View: Refresh",
        ];
        for (title, _) in PALETTE_COMMANDS {
            assert!(
                !known_old.iter().any(|p| title.starts_with(p)),
                "{title} collides with an existing palette command"
            );
        }
        for (_, entries) in MENUS {
            for entry in entries.iter() {
                if let Item(_, command, _) = entry
                    && !known_old.iter().any(|p| command.starts_with(p))
                {
                    assert!(
                        PALETTE_COMMANDS.iter().any(|(t, _)| t == command)
                            || *command == "View: Command Palette",
                        "menu command {command} is not listed for the palette"
                    );
                }
            }
        }
    }
}
