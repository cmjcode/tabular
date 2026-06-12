use eframe::{App, Frame, egui};
use log::{debug};
use chrono::{DateTime, Duration, Utc};
use super::{Tabular, PrefTab};
use crate::{models, connection, editor, data_table, sidebar_database, sidebar_history,
            sidebar_query, spreadsheet::SpreadsheetOperations, dialog,
            cache_data};

fn light_soft_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let bg = egui::Color32::from_rgb(245, 242, 238);       // warm off-white
    let panel = egui::Color32::from_rgb(237, 233, 227);    // slightly warmer panel
    let text = egui::Color32::from_rgb(55, 50, 45);        // soft dark brown (not pure black)
    let widget_bg = egui::Color32::from_rgb(230, 226, 219);
    let widget_bg_hovered = egui::Color32::from_rgb(218, 213, 205);
    let widget_bg_open = egui::Color32::from_rgb(210, 205, 197);

    v.override_text_color = Some(text);
    v.window_fill = bg;
    v.panel_fill = panel;
    v.faint_bg_color = egui::Color32::from_rgb(240, 237, 232);
    v.extreme_bg_color = egui::Color32::from_rgb(255, 252, 248);

    v.widgets.noninteractive.bg_fill      = panel;
    v.widgets.noninteractive.weak_bg_fill = panel;
    v.widgets.noninteractive.fg_stroke    = egui::Stroke::new(1.0, text);

    v.widgets.inactive.bg_fill            = widget_bg;
    v.widgets.inactive.weak_bg_fill       = widget_bg;

    v.widgets.hovered.bg_fill             = widget_bg_hovered;
    v.widgets.hovered.weak_bg_fill        = widget_bg_hovered;

    v.widgets.active.bg_fill              = widget_bg_open;
    v.widgets.active.weak_bg_fill         = widget_bg_open;

    v.widgets.open.bg_fill                = widget_bg_open;
    v.widgets.open.weak_bg_fill           = widget_bg_open;

    v.selection.bg_fill = egui::Color32::from_rgba_premultiplied(180, 160, 140, 100);
    // suppress the window/panel border that comes with light() defaults
    v.window_stroke = egui::Stroke::NONE;
    v
}


impl Tabular {
    /// Render the "Auto Refresh Interval" modal dialog when requested.
    /// Extracted verbatim from `update()` (behavior-preserving).
    fn render_auto_refresh_dialog(&mut self, ctx: &egui::Context) {
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
    }

    /// Render the centered "Connecting…" overlay while waiting for a
    /// connection pool. Extracted verbatim from `update()`.
    fn render_connecting_overlay(&mut self, ctx: &egui::Context) {
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
    }

    /// Render the Preferences/Settings modal window.
    /// Extracted verbatim from `update()` (behavior-preserving).
    fn render_settings_dialog(&mut self, ctx: &egui::Context) {
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
                            // Accent color (red)
                            let accent = egui::Color32::from_rgb(255, 0, 0);
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
                            draw_tab(ui, &mut self.settings_active_pref_tab, PrefTab::AiAssistant, "✨ AI Assistant");
                        });
                        ui.separator();
                        ui.add_space(4.0);

                        match self.settings_active_pref_tab {
                            PrefTab::ApplicationTheme => {
                                ui.heading("Application Theme");
                                ui.add_space(4.0);
                                let prev = self.app_theme;
                                ui.horizontal(|ui| {
                                    ui.label("Choose theme:");
                                    if ui.radio_value(&mut self.app_theme, crate::config::AppTheme::Dark, "🌙 Dark").clicked() {
                                        ctx.set_visuals(egui::Visuals::dark());
                                        if self.link_editor_theme { self.advanced_editor.theme = crate::models::structs::EditorColorTheme::GithubDark; }
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                    if ui.radio_value(&mut self.app_theme, crate::config::AppTheme::Light, "🔆 Light").clicked() {
                                        ctx.set_visuals(egui::Visuals::light());
                                        if self.link_editor_theme { self.advanced_editor.theme = crate::models::structs::EditorColorTheme::GithubLight; }
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                    if ui.radio_value(&mut self.app_theme, crate::config::AppTheme::LightSoft, "⛅ Light Soft").clicked() {
                                        ctx.set_visuals(light_soft_visuals());
                                        if self.link_editor_theme { self.advanced_editor.theme = crate::models::structs::EditorColorTheme::GithubLight; }
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                });
                                ui.add_space(2.0);
                                ui.label(egui::RichText::new(match self.app_theme {
                                    crate::config::AppTheme::Dark => "Classic dark theme.",
                                    crate::config::AppTheme::Light => "High-contrast white theme.",
                                    crate::config::AppTheme::LightSoft => "Warm off-white with lower contrast — easier on the eyes.",
                                }).size(11.0).color(egui::Color32::from_gray(120)));
                                if self.app_theme != prev { ctx.request_repaint(); }
                            }
                            PrefTab::EditorTheme => {
                                ui.heading("Editor Theme");
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut self.link_editor_theme, "Link with application theme").changed() {
                                        if self.link_editor_theme { self.advanced_editor.theme = if self.app_theme.is_dark() { crate::models::structs::EditorColorTheme::GithubDark } else { crate::models::structs::EditorColorTheme::GithubLight }; }
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                    if ui.button("Reset").on_hover_text("Reset to default & relink").clicked() {
                                        self.link_editor_theme = true;
                                        self.advanced_editor.theme = if self.app_theme.is_dark() { crate::models::structs::EditorColorTheme::GithubDark } else { crate::models::structs::EditorColorTheme::GithubLight };
                                        self.prefs_dirty = true; self.try_save_prefs();
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
                                            self.prefs_dirty = true; self.try_save_prefs();
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
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                    ui.separator();
                                    ui.checkbox(&mut self.advanced_editor.show_line_numbers, "Line numbers").changed();
                                    if ui.checkbox(&mut self.advanced_editor.word_wrap, "Word wrap").changed() { self.prefs_dirty = true; self.try_save_prefs(); }
                                });
                            }
                            PrefTab::Performance => {
                                ui.heading("Performance Settings");
                                ui.horizontal(|ui| {
                                    let prev_pagination = self.use_server_pagination;
                                    if ui.checkbox(&mut self.use_server_pagination, "Server-side pagination")
                                        .on_hover_text("When enabled, queries large tables in pages from the server instead of loading all data at once. Much faster for large datasets.")
                                        .changed() {
                                        self.prefs_dirty = true; self.try_save_prefs();
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
                                        self.prefs_dirty = true; self.try_save_prefs();
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
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.label("Redis browser auto-refresh default (seconds):");
                                    let mut seconds = self.redis_browser_auto_refresh_default_seconds.max(1) as i32;
                                    if ui.add(egui::DragValue::new(&mut seconds).range(1..=3600)).changed() {
                                        self.redis_browser_auto_refresh_default_seconds = seconds.max(1) as u32;
                                        self.prefs_dirty = true;
                                        self.try_save_prefs();
                                    }
                                });
                                ui.label(egui::RichText::new("Default interval used when Redis browser auto-refresh is enabled.").size(11.0).color(egui::Color32::from_gray(120)));
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
                                                self.prefs_dirty = true; self.try_save_prefs();
                                                if let Some(rt) = &self.runtime && let Ok(new_store) = rt.block_on(crate::config::ConfigStore::new()) { self.config_store = Some(new_store); log::debug!("Config store reinitialized for new data directory"); }
                                                self.prefs_save_feedback = Some("Data directory updated successfully!".to_string()); self.prefs_last_saved_at = Some(std::time::Instant::now());
                                                log::debug!("Data directory changed to: {}", self.data_directory);
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
                                ui.horizontal(|ui| { if ui.checkbox(&mut self.auto_check_updates, "Automatically check for updates on startup").changed() { self.prefs_dirty = true; self.try_save_prefs(); } });
                                ui.label(egui::RichText::new("When enabled, Tabular will check for new versions from GitHub releases").size(11.0).color(egui::Color32::from_gray(120)));
                            }
                            PrefTab::AiAssistant => {
                                ui.heading("✨ AI Assistant");
                                ui.label(egui::RichText::new("Press Cmd+Shift+A in the editor to toggle the AI panel.").size(11.0).color(egui::Color32::from_gray(130)));
                                ui.add_space(8.0);

                                // Provider selection
                                ui.label("AI Provider:");
                                ui.horizontal_wrapped(|ui| {
                                    let providers = [
                                        crate::config::AiProvider::OpenAI,
                                        crate::config::AiProvider::Anthropic,
                                        crate::config::AiProvider::Groq,
                                        crate::config::AiProvider::GitHub,
                                        crate::config::AiProvider::Custom,
                                    ];
                                    for p in providers {
                                        if ui.radio_value(&mut self.ai_provider, p, p.display_name()).clicked() {
                                            // Reset model + base_url to defaults for new provider
                                            self.ai_settings_model_input = p.default_model().to_string();
                                            self.ai_settings_base_url_input = p.default_base_url().to_string();
                                            self.ai_model = self.ai_settings_model_input.clone();
                                            self.ai_base_url = self.ai_settings_base_url_input.clone();
                                            self.prefs_dirty = true; self.try_save_prefs();
                                        }
                                    }
                                });
                                // GitHub-specific instructions
                                if self.ai_provider == crate::config::AiProvider::GitHub {
                                    egui::Frame::new()
                                        .fill(egui::Color32::from_rgb(20, 40, 70))
                                        .inner_margin(egui::Margin::symmetric(8, 6))
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new("ℹ GitHub Copilot / Models").strong().color(egui::Color32::from_rgb(100, 180, 255)).size(12.0));
                                            ui.label(egui::RichText::new("Requires a GitHub Personal Access Token (PAT) with 'models:read' scope (or 'copilot' scope for Copilot subscribers).").size(11.0).color(egui::Color32::from_gray(200)));
                                            ui.hyperlink_to(
                                                egui::RichText::new("→ Create token at github.com/settings/tokens").size(11.0).color(egui::Color32::from_rgb(100, 180, 255)),
                                                "https://github.com/settings/tokens"
                                            );
                                        });
                                }
                                ui.add_space(6.0);

                                // API Key
                                ui.label("API Key:");
                                ui.horizontal(|ui| {
                                    let hint = self.ai_provider.api_key_hint();
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut self.ai_settings_api_key_input)
                                            .password(true)
                                            .desired_width(280.0)
                                            .hint_text(hint),
                                    );
                                    if resp.lost_focus() || ui.button("Apply").clicked() {
                                        self.ai_api_key = self.ai_settings_api_key_input.clone();
                                        self.prefs_dirty = true; self.try_save_prefs();
                                        self.prefs_save_feedback = Some("API key saved.".to_string());
                                        self.prefs_last_saved_at = Some(std::time::Instant::now());
                                    }
                                });
                                ui.label(egui::RichText::new(format!("Hint: {}", self.ai_provider.api_key_hint())).size(11.0).color(egui::Color32::from_gray(120)));
                                ui.label(egui::RichText::new("Key stored locally and only sent to the chosen provider.").size(11.0).color(egui::Color32::from_gray(120)));
                                ui.add_space(6.0);

                                // Model
                                ui.label("Model:");
                                ui.horizontal(|ui| {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut self.ai_settings_model_input)
                                            .desired_width(220.0)
                                            .hint_text(self.ai_provider.default_model()),
                                    );
                                    if resp.lost_focus() || ui.button("Apply").clicked() {
                                        self.ai_model = self.ai_settings_model_input.clone();
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                    if ui.small_button("Default").clicked() {
                                        self.ai_settings_model_input = self.ai_provider.default_model().to_string();
                                        self.ai_model = self.ai_settings_model_input.clone();
                                        self.prefs_dirty = true; self.try_save_prefs();
                                    }
                                });
                                // Preset model picker
                                ui.label(egui::RichText::new("Quick pick:").size(11.0).color(egui::Color32::from_gray(140)));
                                ui.horizontal_wrapped(|ui| {
                                    let presets = self.ai_provider.preset_models();
                                    for &m in presets {
                                        let selected = self.ai_settings_model_input == m;
                                        if ui.selectable_label(selected, egui::RichText::new(m).size(11.0).monospace()).clicked() {
                                            self.ai_settings_model_input = m.to_string();
                                            self.ai_model = m.to_string();
                                            self.prefs_dirty = true; self.try_save_prefs();
                                        }
                                    }
                                });

                                // Base URL — always shown, prominently highlighted for Custom provider
                                ui.add_space(6.0);
                                let is_custom = self.ai_provider == crate::config::AiProvider::Custom;
                                if is_custom {
                                    let accent = egui::Color32::from_rgb(120, 80, 220);
                                    egui::Frame::new()
                                        .fill(egui::Color32::from_rgba_unmultiplied(120, 80, 220, 20))
                                        .stroke(egui::Stroke::new(1.5, accent))
                                        .inner_margin(egui::Margin::same(8))
                                        .outer_margin(egui::Margin { left: 0, right: 0, top: 2, bottom: 4 })
                                        .corner_radius(egui::CornerRadius::same(6))
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new("🔗 Server URL (required)").size(12.0).color(accent).strong());
                                            ui.add_space(4.0);
                                            let resp = ui.add(
                                                egui::TextEdit::singleline(&mut self.ai_settings_base_url_input)
                                                    .desired_width(f32::INFINITY)
                                                    .hint_text("https://localhost:11434/v1"),
                                            );
                                            if resp.lost_focus() || { let _ = resp; false } {
                                                self.ai_base_url = self.ai_settings_base_url_input.clone();
                                                self.prefs_dirty = true; self.try_save_prefs();
                                            }
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                if ui.button("Apply").clicked() {
                                                    self.ai_base_url = self.ai_settings_base_url_input.clone();
                                                    self.prefs_dirty = true; self.try_save_prefs();
                                                }
                                                if ui.small_button("Reset to default").clicked() {
                                                    self.ai_settings_base_url_input = self.ai_provider.default_base_url().to_string();
                                                    self.ai_base_url = self.ai_settings_base_url_input.clone();
                                                    self.prefs_dirty = true; self.try_save_prefs();
                                                }
                                            });
                                            ui.add_space(2.0);
                                            ui.label(egui::RichText::new("Enter the base URL of your OpenAI-compatible server (e.g., Ollama, LM Studio).").size(11.0).color(egui::Color32::from_gray(150)));
                                        });
                                } else {
                                    ui.label(egui::RichText::new(format!("Base URL (default: {})", self.ai_provider.default_base_url())).size(12.0));
                                    ui.horizontal(|ui| {
                                        let resp = ui.add(
                                            egui::TextEdit::singleline(&mut self.ai_settings_base_url_input)
                                                .desired_width(280.0)
                                                .hint_text(self.ai_provider.default_base_url()),
                                        );
                                        if resp.lost_focus() || ui.button("Apply").clicked() {
                                            self.ai_base_url = self.ai_settings_base_url_input.clone();
                                            self.prefs_dirty = true; self.try_save_prefs();
                                        }
                                        if ui.small_button("Default").clicked() {
                                            self.ai_settings_base_url_input = self.ai_provider.default_base_url().to_string();
                                            self.ai_base_url = self.ai_settings_base_url_input.clone();
                                            self.prefs_dirty = true; self.try_save_prefs();
                                        }
                                    });
                                    ui.label(egui::RichText::new("For OpenAI-compatible local servers (e.g., Ollama, LM Studio), change the base URL.").size(11.0).color(egui::Color32::from_gray(120)));
                                }

                                // Status indicator
                                ui.add_space(6.0);
                                if self.ai_api_key.is_empty() {
                                    ui.label(egui::RichText::new("⚠ No API key set — AI panel will show a warning.").color(egui::Color32::from_rgb(220, 160, 30)).size(12.0));
                                } else {
                                    let masked = format!("{}…{}", &self.ai_api_key[..self.ai_api_key.len().min(6)], &self.ai_api_key[self.ai_api_key.len().saturating_sub(4)..]);
                                    ui.label(egui::RichText::new(format!("✓ Key configured: {masked}")).color(egui::Color32::from_rgb(0, 180, 80)).size(12.0));
                                }
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("💾 Save Preferences").clicked() {
                                self.prefs_dirty = true; self.try_save_prefs(); self.prefs_save_feedback = Some("Saved".to_string()); self.prefs_last_saved_at = Some(std::time::Instant::now());
                            }
                            if let Some(msg) = &self.prefs_save_feedback { ui.label(egui::RichText::new(msg).color(egui::Color32::from_rgb(0,150,0))); }
                        });
                    });
                if !open_flag {
                    self.show_settings_window = false;
                }
            }
    }

    /// Drain native file/directory picker result channels into state.
    /// Extracted verbatim from `update()`.
    fn process_file_picker_results(&mut self) {
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
    }

    /// Drain and process all pending `BackgroundResult` messages.
    /// Extracted verbatim from `update()`.
    fn process_background_results(&mut self, ctx: &egui::Context) {
            // Check for background task results
            let mut results = Vec::new();
            if let Some(receiver) = &self.background_receiver {
                while let Ok(result) = receiver.try_recv() {
                    results.push(result);
                }
            }
        
            for result in results {
                    match result {
                        models::enums::BackgroundResult::RefreshComplete {
                            connection_id,
                            success,
                        } => {
                            // Remove from refreshing set
                            self.refreshing_connections.remove(&connection_id);

                            if success {
                                debug!(
                                    "✅ Background refresh completed successfully for connection {}",
                                    connection_id
                                );

                                // Extract expansion state before borrowing items_tree mutably
                                let expansion_state =
                                    self.pending_expansion_restore.remove(&connection_id);

                                // Re-expand connection node to show fresh data (search recursively
                                // through folder nodes since connections are nested inside folders)
                                let node_found = if let Some(conn_node) =
                                    Self::find_connection_node_recursive(
                                        &mut self.items_tree,
                                        connection_id,
                                    )
                                {
                                    debug!("   ✅ Found connection node: {}", conn_node.name);
                                    if let Some(state) = expansion_state {
                                        debug!(
                                            "🔄 Restoring {} expansion states for connection {}",
                                            state.len(),
                                            connection_id
                                        );
                                        conn_node.is_loaded = false;
                                        Self::restore_expansion_state(conn_node, &state);
                                        debug!("   ✅ Expansion state restored");
                                        Self::mark_expanded_nodes_loaded(conn_node);
                                        debug!("   ✅ Expanded nodes marked for loading");
                                    } else {
                                        debug!("   ⚠️  No expansion state to restore");
                                        conn_node.is_loaded = false;
                                    }
                                    true
                                } else {
                                    false
                                };

                                if !node_found {
                                    debug!("   ❌ Connection node {} not found in tree!", connection_id);
                                }

                                // Mark this connection as needing auto-load
                                // Will be processed in the sidebar render where we have proper borrow access
                                self.pending_auto_load.insert(connection_id);
                                debug!(
                                    "📂 Marked connection {} for auto-load after restore",
                                    connection_id
                                );
                                debug!(
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
                            // Reload any already-expanded table/view folders so newly-cached
                            // tables become visible without the user having to re-click.
                            self.refresh_all_table_folders(connection_id);
                            ctx.request_repaint();
                        }
                        models::enums::BackgroundResult::SqlitePathPicked { path } => {
                            self.temp_sqlite_path = Some(path);
                            ctx.request_repaint();
                        }
                        models::enums::BackgroundResult::DatabasesFetched {
                            connection_id,
                            databases,
                        } => {
                            debug!("✅ Received background databases fetch result: {} databases", databases.len());
                            // Update cache
                            self.database_cache.insert(connection_id, databases.clone());
                            self.database_cache_time
                                .insert(connection_id, std::time::Instant::now());
                        
                            // Also save to SQLite cache
                            cache_data::save_databases_to_cache(self, connection_id, &databases);
                        
                            // Update UI tree if connection node exists
                            for node in &mut self.items_tree {
                                if node.node_type == models::enums::NodeType::Connection
                                    && node.connection_id == Some(connection_id)
                                {
                                    // Force reload of children
                                    node.is_loaded = false; 
                                    break;
                                }
                            }
                        
                             // Also remove from fetching set
                            self.fetching_databases.remove(&connection_id);

                            // Refresh UI
                            ctx.request_repaint();
                        }
                        models::enums::BackgroundResult::RedisKeysFetched {
                            connection_id,
                            database_name,
                            keys,
                        } => {
                            log::debug!(
                                "[redis_keys] UI received fetch result conn={} keyspace={} keys={}",
                                connection_id,
                                database_name,
                                keys.len()
                            );
                            debug!(
                                "✅ Redis keys fetched for db '{}': {} keys",
                                database_name,
                                keys.len()
                            );

                            // Remove from in-progress set
                            self.fetching_redis_keys.remove(&(connection_id, database_name.clone()));

                            // Group keys by type
                            let mut keys_by_type: std::collections::HashMap<String, Vec<String>> =
                                std::collections::HashMap::new();
                            for (key, key_type) in keys {
                                keys_by_type.entry(key_type).or_default().push(key);
                            }
                            let no_keys_found = keys_by_type.is_empty();

                            // Locate the database node in the tree and populate it
                            for root in &mut self.items_tree {
                                if let Some(db_node) = crate::window_egui::Tabular::find_redis_database_node(
                                    root,
                                    connection_id,
                                    &Some(database_name.clone()),
                                ) {
                                    log::debug!(
                                        "[redis_keys] found UI node '{}' for conn={} keyspace={}",
                                        db_node.name,
                                        connection_id,
                                        database_name
                                    );
                                    db_node.children.clear();

                                    let mut sorted_types: Vec<_> = keys_by_type.into_iter().collect();
                                    sorted_types.sort_by(|a, b| a.0.cmp(&b.0));

                                    for (data_type, type_keys) in sorted_types {
                                        let folder_name = match data_type.as_str() {
                                            "string" => "Strings",
                                            "hash" => "Hashes",
                                            "list" => "Lists",
                                            "set" => "Sets",
                                            "zset" => "Sorted Sets",
                                            "stream" => "Streams",
                                            other => other,
                                        };
                                        let mut type_folder = models::structs::TreeNode::new(
                                            format!("{} ({})", folder_name, type_keys.len()),
                                            models::enums::NodeType::TablesFolder,
                                        );
                                        type_folder.connection_id = Some(connection_id);
                                        type_folder.database_name = Some(database_name.clone());
                                        type_folder.is_expanded = false;
                                        type_folder.is_loaded = true;

                                        for key in type_keys {
                                            let mut key_node = models::structs::TreeNode::new(
                                                key,
                                                models::enums::NodeType::Table,
                                            );
                                            key_node.connection_id = Some(connection_id);
                                            key_node.database_name = Some(database_name.clone());
                                            type_folder.children.push(key_node);
                                        }
                                        db_node.children.push(type_folder);
                                    }

                                    db_node.is_loaded = true;
                                    break;
                                }
                            }

                            if no_keys_found {
                                log::warn!(
                                    "[redis_keys] no keys or no types available for conn={} keyspace={}",
                                    connection_id,
                                    database_name
                                );
                            }

                            ctx.request_repaint();
                        }
                        models::enums::BackgroundResult::RedisBrowserStateFetched {
                            connection_id,
                            state,
                        } => {
                            self.fetching_redis_browser.remove(&connection_id);

                            let keys_to_cache: Vec<(String, String)> = state
                                .keys
                                .iter()
                                .map(|entry| (entry.key_name.clone(), entry.key_type.clone()))
                                .collect();
                            if !state.keyspace_label.is_empty() && !keys_to_cache.is_empty() {
                                cache_data::save_redis_browser_keys_to_cache(
                                    self,
                                    connection_id,
                                    &state.keyspace_label,
                                    &keys_to_cache,
                                );
                            }

                            for tab in &mut self.query_tabs {
                                if tab.connection_id == Some(connection_id)
                                    && tab.redis_browser_state.is_some()
                                {
                                    let mut merged_state = state.clone();
                                    if let Some(previous_state) = tab.redis_browser_state.as_ref() {
                                        merged_state.filter_text = previous_state.filter_text.clone();
                                        merged_state.type_filter = previous_state.type_filter.clone();
                                        merged_state.remote_search_in_progress = false;
                                        merged_state.last_remote_search = previous_state.last_remote_search.clone();
                                        merged_state.auto_refresh_enabled = previous_state.auto_refresh_enabled;
                                        merged_state.auto_refresh_interval_seconds = previous_state.auto_refresh_interval_seconds.max(1);
                                        merged_state.auto_refresh_last_run = previous_state.auto_refresh_last_run;
                                        merged_state.selected_key = previous_state.selected_key.clone();
                                        merged_state.selected_key_type = previous_state.selected_key_type.clone();
                                        merged_state.preview = previous_state.preview.clone();
                                        if !previous_state.last_error.as_deref().unwrap_or_default().is_empty() {
                                            merged_state.last_error = previous_state.last_error.clone();
                                        }
                                    } else {
                                        merged_state.auto_refresh_enabled = true;
                                        merged_state.auto_refresh_interval_seconds =
                                            self.redis_browser_auto_refresh_default_seconds.max(1);
                                    }
                                    tab.redis_browser_state = Some(merged_state);
                                }
                            }

                            ctx.request_repaint();
                        }
                        models::enums::BackgroundResult::RedisBrowserSearchFetched {
                            connection_id,
                            database_name,
                            search_text,
                            keys,
                        } => {
                            let mut merged_keys_for_cache: Option<Vec<(String, String)>> = None;

                            for tab in &mut self.query_tabs {
                                if tab.connection_id == Some(connection_id)
                                    && let Some(state) = &mut tab.redis_browser_state
                                {
                                    state.remote_search_in_progress = false;
                                    state.last_remote_search = Some(search_text.clone());

                                    for (key_name, key_type) in &keys {
                                        if !state.keys.iter().any(|entry| entry.key_name == *key_name) {
                                            state.keys.push(models::structs::RedisBrowserKeyEntry {
                                                key_name: key_name.clone(),
                                                key_type: key_type.clone(),
                                                ttl_label: if database_name == crate::driver_redis::REDIS_CLUSTER_KEYSPACE {
                                                    "Cluster".to_string()
                                                } else {
                                                    database_name.clone()
                                                },
                                                size_label: "-".to_string(),
                                            });
                                        }
                                    }

                                    state.keys.sort_by(|left, right| left.key_name.cmp(&right.key_name));
                                    state.status_text = if keys.is_empty() {
                                        format!("No Redis server matches for '{}'", search_text)
                                    } else {
                                        format!("Loaded {} Redis server matches for '{}'", keys.len(), search_text)
                                    };
                                    state.last_error = None;

                                    merged_keys_for_cache = Some(
                                        state
                                            .keys
                                            .iter()
                                            .map(|entry| (entry.key_name.clone(), entry.key_type.clone()))
                                            .collect(),
                                    );
                                }
                            }

                            if let Some(keys_to_cache) = merged_keys_for_cache
                                && !database_name.is_empty()
                            {
                                cache_data::save_redis_browser_keys_to_cache(
                                    self,
                                    connection_id,
                                    &database_name,
                                    &keys_to_cache,
                                );
                            }

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


            while let Ok(message) = self.query_result_receiver.try_recv() {
                self.handle_query_result_message(message);
                ctx.request_repaint();
            }
    }

    /// Render the resizable left sidebar (connections/queries/history tree).
    /// Extracted verbatim from `update()`.
    fn render_left_sidebar(&mut self, ctx: &egui::Context) {
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
                        // .inner_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 6 }),
                )
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.add_space(-2.0);
                        // Top section with tabs
                        ui.horizontal(|ui| {
                            let available_width = ui.available_width();
                            let button_spacing = ui.spacing().item_spacing.x;
                            let button_width = (available_width - (button_spacing * 2.0)) / 3.0; // Add extra width to account for padding and make buttons more clickable
                            let button_height = 28.0;

                            // Database tab
                            let database_button = if self.selected_menu == "Database" {
                                egui::Button::new(
                                    egui::RichText::new("Database")
                                        .color(egui::Color32::WHITE)
                                        .text_style(egui::TextStyle::Body),
                                )
                                .fill(egui::Color32::from_rgb(255, 0, 0))
                                .corner_radius(0.0)
                            } else {
                                egui::Button::new("Database").fill(egui::Color32::TRANSPARENT).corner_radius(0.0)
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
                                .fill(egui::Color32::from_rgb(255, 0, 0))
                                .corner_radius(0.0)
                            } else {
                                egui::Button::new("Queries").fill(egui::Color32::TRANSPARENT).corner_radius(0.0)
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
                                .fill(egui::Color32::from_rgb(255, 0, 0)) // Orange fill for active
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

                        // Paint a 1px border line flush under the buttons — zero height allocation,
                        // matching the editor tab bar's bottom border so they align as one continuous line.
                        let line_width = ui.available_width();
                        let (line_rect, _) = ui.allocate_exact_size(
                            egui::vec2(line_width, 1.0),
                            egui::Sense::hover(),
                        );

                        // ===============================================================
                        // Draw line across entire width of sidebar (ignoring padding)
                        let full_x = ui.clip_rect().x_range();
                        ui.painter().hline(
                            full_x, // Align with button padding
                            line_rect.top() - 3.5, // Position line flush under buttons
                            egui::Stroke::new(
                                1.0,
                                if ui.visuals().dark_mode {
                                    egui::Color32::from_rgb(55, 55, 55)
                                } else {
                                    egui::Color32::from_rgb(200, 200, 200)
                                },
                            ),
                        );

                        // Middle section with scrollable content
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            match self.selected_menu.as_str() {
                                "Database" => {
                                    // Right-click context menu on empty space in the database sidebar
                                    let db_area_response = ui.interact(
                                        ui.available_rect_before_wrap(),
                                        egui::Id::new("database_area"),
                                        egui::Sense::click(),
                                    );
                                    db_area_response.context_menu(|ui| {
                                        if ui.button("📁 Create Folder").clicked() {
                                            // Empty parent = create a top-level folder
                                            self.subfolder_parent_path = String::new();
                                            self.new_subfolder_name.clear();
                                            self.show_create_subfolder_dialog = true;
                                            ui.close();
                                        }
                                        if ui.button("➕ Add Connection").clicked() {
                                            self.new_connection.folder = Some("Default".to_string());
                                            self.show_add_connection = true;
                                            ui.close();
                                        }
                                    });
                                    // Always render the tree so standalone folders without
                                    // connections are also visible.
                                    if self.connections.is_empty() && self.items_tree.is_empty() {
                                        ui.label("No connections configured");
                                        ui.label("Click ➕ to add a new connection");
                                    } else {
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
                                                egui::Color32::from_rgb(255, 0, 0),
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
                                                        .fill(egui::Color32::from_rgb(255, 0, 0));
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

                                    ui.add_space(-2.0);

                                    // Search box for history
                                    ui.horizontal(|ui| {
                                        ui.add_space(4.0);
                                        let search_bg = if ui.visuals().dark_mode {
                                            egui::Color32::from_rgb(40, 40, 40)
                                        } else {
                                            egui::Color32::from_rgb(210, 210, 210)
                                        };
                                        // Make search box responsive to sidebar width
                                        let available_width = ui.available_width() - 5.0; // Leave space for clear button and padding
                                        let search_response = ui.add_sized(
                                            [available_width, 20.0],
                                            egui::TextEdit::singleline(&mut self.history_search_text)
                                                .desired_width(f32::INFINITY)
                                                .hint_text("Search history...")
                                                .background_color(search_bg)
                                        );

                                        if search_response.has_focus() {
                                            ui.painter().rect_stroke(
                                                search_response.rect,
                                                0.0,
                                                egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 0, 0)),
                                                egui::StrokeKind::Outside,
                                            );
                                        }

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
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                            ui.add_space(10.0); // Bottom spacing

                            ui.horizontal(|ui| {
                                ui.add_space(5.0); // Right spacing (goes before button since layout is right-aligned)
                                match self.selected_menu.as_str() {
                                    "Database" => {
                                        let plus_text_color = if ui.visuals().dark_mode {
                                            egui::Color32::from_rgb(30, 30, 30)
                                        } else {
                                            egui::Color32::WHITE
                                        };
                                        if ui
                                            .add_sized(
                                                [24.0, 24.0], // Small square button
                                                egui::Button::new(egui::RichText::new("➕").color(plus_text_color)).fill(egui::Color32::from_rgb(255, 0, 0)),
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
                });
            }
    }

    /// Render the AI Assistant right side panel.
    /// Extracted verbatim from `update()`.
    fn render_ai_right_panel(&mut self, ctx: &egui::Context) {
            if self.show_ai_panel {
                egui::SidePanel::right("ai_right_panel")
                    .resizable(true)
                    .default_width(350.0)
                    .min_width(280.0)
                    .max_width(600.0)
                    .frame(
                        egui::Frame::default()
                            .fill(if ctx.style().visuals.dark_mode {
                                egui::Color32::from_rgb(22, 24, 34)
                            } else {
                                egui::Color32::from_rgb(240, 242, 252)
                            })
                            .inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| {
                        editor::render_ai_panel(self, ui);
                    });
            }
    }

    /// Render the central panel (editor / data grid / structure).
    /// Extracted verbatim from `update()`.
    fn render_central_panel(&mut self, ctx: &egui::Context) {
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
                                let inactive_bg = if ui.visuals().dark_mode {
                                    egui::Color32::from_rgb(28, 28, 28)
                                } else {
                                    egui::Color32::from_rgb(230, 230, 230)
                                };
                                let tab_bg = if active {
                                    egui::Color32::from_rgb(255, 0, 0)
                                } else {
                                    inactive_bg
                                };
                                let text_color = if active {
                                    egui::Color32::WHITE
                                } else {
                                    ui.visuals().text_color()
                                };
                                let mut title = tab.title.clone();
                                if let Some(cid) = tab.connection_id
                                    && let Some(n) = self.get_connection_name(cid)
                                {
                                    title = format!("{} [{}]", title, n);
                                }
                                // Render tab as one unified rect: [  label  ×  ]
                                let close_size = 16.0;
                                let tab_width = 150.0; // total width including close button
                                let tab_height = 26.0;
                                let (tab_rect, tab_resp) = ui.allocate_exact_size(
                                    egui::vec2(tab_width, tab_height),
                                    egui::Sense::click(),
                                );
                                // Background for whole tab
                                ui.painter().rect_filled(tab_rect, 0.0, tab_bg);
                                // Bottom indicator line for inactive tabs
                                if !active {
                                    ui.painter().hline(
                                        tab_rect.x_range(),
                                        tab_rect.bottom() - 1.0,
                                        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 0, 0)),
                                    );
                                }
                                // Close button rect on the right side of the tab
                                let close_rect = egui::Rect::from_min_size(
                                    egui::pos2(tab_rect.right() - close_size - 2.0, tab_rect.center().y - close_size / 2.0),
                                    egui::vec2(close_size, close_size),
                                );
                                // Label centered in the area left of the close button
                                let label_max_width = tab_rect.width() - close_size - 8.0;
                                let label_area = egui::Rect::from_min_size(
                                    egui::pos2(tab_rect.left() + 2.0, tab_rect.top()),
                                    egui::vec2(label_max_width, tab_rect.height()),
                                );
                                let label_area_center = label_area.left_center();
                                ui.painter().with_clip_rect(label_area).text(
                                    label_area_center,
                                    egui::Align2::LEFT_CENTER,
                                    &title,
                                    egui::FontId::proportional(12.0),
                                    text_color,
                                );
                                // Paint close "×" inside the tab
                                let show_close = self.query_tabs.len() > 1 || !active;
                                if show_close {
                                    // Hover effect on close button
                                    let close_resp = ui.interact(close_rect, ui.id().with(("tab_close", i)), egui::Sense::click());
                                    if close_resp.hovered() {
                                        let hover_color = if active {
                                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 60)
                                        } else {
                                            egui::Color32::from_rgba_unmultiplied(128, 128, 128, 60)
                                        };
                                        ui.painter().rect_filled(close_rect, 4.0, hover_color);
                                    }
                                    let x_color = if active {
                                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(180, 180, 180, 220)
                                    };
                                    ui.painter().text(
                                        close_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "×",
                                        egui::FontId::proportional(14.0),
                                        x_color,
                                    );
                                    if close_resp.clicked() {
                                        to_close = Some(i);
                                    }
                                }
                                // Switch tab on click (but not on the close button)
                                if tab_resp.clicked() && !active && !close_rect.contains(tab_resp.interact_pointer_pos().unwrap_or(egui::Pos2::ZERO)) {
                                    to_switch = Some(i);
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

                            // AI Assistant toggle button (✨)
                            let ai_btn_bg = if self.show_ai_panel {
                                egui::Color32::from_rgb(99, 135, 255)
                            } else if ui.visuals().dark_mode {
                                egui::Color32::from_rgb(40, 40, 40)
                            } else {
                                egui::Color32::from_rgb(220, 220, 220)
                            };
                            let ai_btn_label = egui::RichText::new("✨")
                                .color(if self.show_ai_panel {
                                    egui::Color32::WHITE
                                } else {
                                    ui.visuals().text_color()
                                });
                            if ui
                                .add_sized([24.0, 20.0], egui::Button::new(ai_btn_label).fill(ai_btn_bg))
                                .on_hover_text(if self.show_ai_panel { "Close AI Assistant (Cmd+Shift+A)" } else { "Open AI Assistant (Cmd+Shift+A)" })
                                .clicked()
                            {
                                self.show_ai_panel = !self.show_ai_panel;
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
                                style.visuals.selection.bg_fill = egui::Color32::from_rgb(255, 0, 0);
                                style.visuals.selection.stroke.color = egui::Color32::from_rgb(255, 0, 0);
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
                                                                debug!("📦 Showing cached data (toggle) for {}/{} ({} cols, {} rows)", db_name, table, hdrs.len(), rows.len());
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
                        let mut rendered_http = false;
                        let mut rendered_redis_browser = false;
                        let mut diagram_to_save = None;
                        let mut redis_action = None;
                        let mut redis_connection_id = None;

                        // Check for HTTP client tab
                        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                            && tab.http_client_state.is_some()
                        {
                            if let Some(state) = &mut tab.http_client_state {
                                crate::http_client::render_http_client(ui, state);
                            }
                            rendered_http = true;
                        }

                        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                            && tab.redis_browser_state.is_some()
                        {
                            redis_connection_id = tab.connection_id;
                            if let Some(state) = &mut tab.redis_browser_state {
                                redis_action = crate::redis_browser::render_redis_browser(ui, state);
                            }
                            rendered_redis_browser = true;
                        }
                    
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

                        if let Some(conn_id) = redis_connection_id
                            && let Some(action) = redis_action
                        {
                            match action {
                                crate::redis_browser::RedisBrowserAction::Refresh => {
                                    let selected_keyspace = self
                                        .query_tabs
                                        .get(self.active_tab_index)
                                        .and_then(|tab| tab.redis_browser_state.as_ref())
                                        .map(|state| state.keyspace_label.clone());
                                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                                        if let Some(state) = &mut tab.redis_browser_state {
                                            state.status_text = "Refreshing Redis browser in background...".to_string();
                                            state.last_error = None;
                                        } else {
                                            let mut state = crate::driver_redis::redis_browser_loading_state(
                                                "Refreshing Redis browser in background...",
                                            );
                                            state.auto_refresh_interval_seconds =
                                                self.redis_browser_auto_refresh_default_seconds.max(1);
                                            tab.redis_browser_state = Some(state);
                                        }
                                    }
                                    if self.fetching_redis_browser.insert(conn_id)
                                        && let Some(sender) = &self.background_sender
                                    {
                                        let _ = sender.send(models::enums::BackgroundTask::FetchRedisBrowserState {
                                            connection_id: conn_id,
                                            database_name: selected_keyspace,
                                        });
                                    }
                                }
                                crate::redis_browser::RedisBrowserAction::SelectKeyspace { database_name } => {
                                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                                        && let Some(state) = &mut tab.redis_browser_state {
                                        state.keyspace_label = database_name.clone();
                                        state.keys.clear();
                                        state.selected_key = None;
                                        state.selected_key_type = None;
                                        state.preview = None;
                                        state.filter_text.clear();
                                        state.last_remote_search = None;
                                        state.remote_search_in_progress = false;
                                        state.status_text = format!("Loading {} in background...", database_name);
                                        state.last_error = None;
                                    }
                                    if self.fetching_redis_browser.insert(conn_id)
                                        && let Some(sender) = &self.background_sender
                                    {
                                        let _ = sender.send(models::enums::BackgroundTask::FetchRedisBrowserState {
                                            connection_id: conn_id,
                                            database_name: Some(database_name),
                                        });
                                    }
                                }
                                crate::redis_browser::RedisBrowserAction::SearchServer { search_text } => {
                                    let database_name = self
                                        .query_tabs
                                        .get(self.active_tab_index)
                                        .and_then(|tab| tab.redis_browser_state.as_ref())
                                        .map(|state| state.keyspace_label.clone())
                                        .unwrap_or_else(|| crate::driver_redis::REDIS_CLUSTER_KEYSPACE.to_string());
                                    if let Some(sender) = &self.background_sender {
                                        let _ = sender.send(models::enums::BackgroundTask::SearchRedisBrowserKeys {
                                            connection_id: conn_id,
                                            database_name,
                                            search_text,
                                        });
                                    }
                                }
                                crate::redis_browser::RedisBrowserAction::SelectKey { key_name, key_type } => {
                                    let database_name = self
                                        .query_tabs
                                        .get(self.active_tab_index)
                                        .and_then(|tab| tab.redis_browser_state.as_ref())
                                        .map(|state| state.keyspace_label.clone())
                                        .unwrap_or_else(|| crate::driver_redis::REDIS_CLUSTER_KEYSPACE.to_string());
                                    let preview = crate::driver_redis::fetch_redis_browser_preview(
                                        self,
                                        conn_id,
                                        &database_name,
                                        &key_name,
                                        &key_type,
                                    );
                                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                                        && let Some(state) = &mut tab.redis_browser_state {
                                        state.selected_key = Some(key_name);
                                        match preview {
                                            Ok(preview) => {
                                                state.selected_key_type = Some(preview.key_type.clone());
                                                state.preview = Some(preview);
                                                state.last_error = None;
                                            }
                                            Err(error) => {
                                                state.selected_key_type = Some(key_type);
                                                state.last_error = Some(error);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    
                        if !rendered_diagram && !rendered_http && !rendered_redis_browser {
                            self.render_query_editor_with_split(ui, "regular_query");
                        }
                    
                        // Floating tab buttons at bottom-right corner (only show if executed or has message, and not HTTP tab)
                        let executed = self.query_tabs.get(self.active_tab_index).map(|t| t.has_executed_query).unwrap_or(false);
                        let has_headers = !self.current_table_headers.is_empty();
                        if !rendered_http && !rendered_redis_browser && (executed || has_headers || !self.query_message.is_empty()) {
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
                                            egui::Color32::from_rgb(255, 0, 0)
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
                                                egui::Color32::from_rgb(255, 0, 0)
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
                                        .button(egui::RichText::new("Confirm").color(egui::Color32::from_rgb(255, 0, 0)))
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
                                            self.toasts.success(format!("Collection '{}.{}' berhasil di-drop", db, coll));
                                        } else {
                                            self.toasts.error(format!("Gagal drop collection '{}.{}'", db, coll));
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
                                        .button(egui::RichText::new("Confirm").color(egui::Color32::from_rgb(255, 0, 0)))
                                        .clicked()
                                    {
                                        use log::{error};
                                        debug!("🗑️ Executing DROP TABLE:");
                                        debug!("   Connection ID: {}", conn_id);
                                        debug!("   Database: {}", db);
                                        debug!("   Table: {}", table);
                                        debug!("   Statement: {}", stmt_str);
                                        // Execute DROP TABLE statement
                                        let result = crate::connection::execute_query_with_connection(
                                            self,
                                            conn_id,
                                            stmt_str.clone(),
                                        );
                                        // Log detailed result
                                        match &result {
                                            Some((headers, rows)) => {
                                                debug!("   Result: Success");
                                                debug!("   Headers: {:?}", headers);
                                                debug!("   Rows count: {}", rows.len());
                                                if !rows.is_empty() {
                                                    debug!("   First row: {:?}", rows.first());
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
                                            debug!("✅ DROP TABLE succeeded for {}.{}", db, table);
                                            debug!("   Connection ID: {}", conn_id);
                                            debug!("   Database: '{}'", db);
                                            debug!("   Table: '{}'", table);
                                            // Use incremental update: just remove the table from tree
                                            debug!("🌲 Removing table from sidebar tree (incremental)...");
                                            self.remove_table_from_tree(conn_id, db, table);
                                            // Clear cache for this table (but don't refresh entire connection)
                                            debug!("🧹 Clearing cache for table {}.{}", db, table);
                                            self.clear_table_cache(conn_id, db, table);
                                            // Force UI repaint to reflect changes immediately
                                            ui.ctx().request_repaint();
                                            self.toasts.success(format!("Table '{}.{}' berhasil di-drop", db, table));
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
                                            self.toasts.error(error_msg);
                                        }
                                        self.pending_drop_table = None;
                                    }
                                });
                            });
                    }

                    self.render_active_query_jobs_overlay(ctx);
                });
    }

    /// Handle Cmd/Ctrl+C copy for the data table / structure views.
    /// Extracted verbatim from `update()`; `copy_shortcut_detected` is the
    /// per-frame flag computed during keyboard handling.
    fn handle_table_copy_shortcut(&mut self, ctx: &egui::Context, copy_shortcut_detected: bool) {
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
    }

    /// Render the feature-gated "Query AST Debug" floating window (Phase F).
    /// Extracted verbatim from `update()`.
    #[cfg(feature = "query_ast")]
    fn render_query_ast_debug_window(&mut self, ctx: &egui::Context) {
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
    }

    /// Persist preferences immediately when `prefs_dirty` is set.
    /// Extracted from the former `try_save_prefs` closure in `update()`.
    fn try_save_prefs(&mut self) {
        if self.prefs_dirty {
            if let (Some(store), Some(rt)) = (self.config_store.as_ref(), self.runtime.as_ref()) {
                let prefs = crate::config::AppPreferences {
                    theme: self.app_theme,
                    link_editor_theme: self.link_editor_theme,
                    editor_theme: match self.advanced_editor.theme {
                        crate::models::structs::EditorColorTheme::GithubLight => {
                            "GITHUB_LIGHT".into()
                        }
                        crate::models::structs::EditorColorTheme::Gruvbox => "GRUVBOX".into(),
                        _ => "GITHUB_DARK".into(),
                    },
                    font_size: self.advanced_editor.font_size,
                    word_wrap: self.advanced_editor.word_wrap,
                    data_directory: if self.data_directory
                        != crate::config::get_data_dir().to_string_lossy()
                    {
                        Some(self.data_directory.clone())
                    } else {
                        None
                    },
                    auto_check_updates: self.auto_check_updates,
                    use_server_pagination: self.use_server_pagination,
                    last_update_check_iso: self
                        .last_saved_prefs
                        .as_ref()
                        .and_then(|p| p.last_update_check_iso.clone()),
                    enable_debug_logging: self.enable_debug_logging,
                    ai_api_key: self.ai_api_key.clone(),
                    ai_model: self.ai_model.clone(),
                    ai_provider: self.ai_provider,
                    ai_base_url: self.ai_base_url.clone(),
                    redis_browser_auto_refresh_seconds: self.redis_browser_auto_refresh_default_seconds.max(1),
                };
                rt.block_on(store.save(&prefs));
                log::debug!(
                    "Preferences saved successfully to: {}",
                    crate::config::get_data_dir().display()
                );
                self.last_saved_prefs = Some(prefs);
                self.prefs_dirty = false;
            } else {
                log::error!("Cannot save preferences: config store or runtime not initialized");
            }
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
        // Load DB-type PNG icons once from assets/db_icons/ if files are present
        self.load_db_icon_textures(ctx);
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
        self.render_auto_refresh_dialog(ctx);

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

        if let Some(active_tab) = self.query_tabs.get(self.active_tab_index)
            && let Some(redis_state) = active_tab.redis_browser_state.as_ref()
            && redis_state.auto_refresh_enabled
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
            if let Some(conn_id) = active_tab.connection_id
                && !self.fetching_redis_browser.contains(&conn_id)
            {
                let now = std::time::Instant::now();
                let should_run = match redis_state.auto_refresh_last_run {
                    None => true,
                    Some(last) => {
                        let interval = std::time::Duration::from_secs(
                            redis_state.auto_refresh_interval_seconds.max(1) as u64,
                        );
                        now.duration_since(last) >= interval
                    }
                };

                if should_run {
                    let selected_keyspace = self
                        .query_tabs
                        .get(self.active_tab_index)
                        .and_then(|tab| tab.redis_browser_state.as_ref())
                        .map(|state| state.keyspace_label.clone());
                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                        && let Some(state) = &mut tab.redis_browser_state
                    {
                        state.auto_refresh_last_run = Some(now);
                        state.status_text = format!(
                            "Auto-refreshing Redis browser every {}s...",
                            state.auto_refresh_interval_seconds.max(1)
                        );
                    }
                    if self.fetching_redis_browser.insert(conn_id)
                        && let Some(sender) = &self.background_sender
                    {
                        let _ = sender.send(models::enums::BackgroundTask::FetchRedisBrowserState {
                            connection_id: conn_id,
                            database_name: selected_keyspace,
                        });
                    }
                }
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
                    self.app_theme = prefs.theme;
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
                    log::debug!("Preferences loaded successfully on startup");

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
        match self.app_theme {
            crate::config::AppTheme::Dark => ctx.set_visuals(egui::Visuals::dark()),
            crate::config::AppTheme::Light => ctx.set_visuals(egui::Visuals::light()),
            crate::config::AppTheme::LightSoft => ctx.set_visuals(light_soft_visuals()),
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
            let desired_editor_theme = if self.app_theme.is_dark() {
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
        self.render_query_ast_debug_window(ctx);

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

        // Show cache miss dialog (topmost)
        self.render_cache_miss_dialog(ctx);
        
        // Settings window with higher z-order
        self.render_settings_dialog(ctx);

        // Centered loading overlay when waiting for connection pool
        self.render_connecting_overlay(ctx);

        // Drain any native file/directory picker channels into state.
        self.process_file_picker_results();

        // Check for background task results
        self.process_background_results(ctx);

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
        sidebar_database::render_create_subfolder_dialog(self, ctx);
        if let Some(rx) = &self.replication_setup_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(msg) => {
                         // Extract IDs before mutable borrows
                         let (target_id_opt, source_id_opt) = if let Some(dialog_state) = &self.replication_dialog {
                             (Some(dialog_state.target_connection_id), dialog_state.source_connection_id)
                         } else {
                             (None, None)
                         };
                         
                         // Save replication_master_id to the target connection
                         if let (Some(target_id), Some(source_id)) = (target_id_opt, source_id_opt) {
                             // Update the connection in memory
                             if let Some(conn) = self.connections.iter_mut().find(|c| c.id == Some(target_id)) {
                                 conn.replication_master_id = Some(source_id);
                             }
                             // Save to database (clone to avoid borrow issues)
                             if let Some(conn) = self.connections.iter().find(|c| c.id == Some(target_id)).cloned() {
                                 sidebar_database::update_connection_in_database(self, &conn);
                             }
                         }
                         
                         self.show_add_replication_dialog = false;
                         self.replication_dialog = None;
                         self.replication_setup_receiver = None;
                         self.query_message = msg;
                         self.show_message_panel = true;
                         self.query_message_is_error = false;
                         self.request_structure_refresh = true;
                }
                Err(err_msg) => {
                    if let Some(state) = &mut self.replication_dialog {
                        state.is_executing = false;
                        state.error = Some(err_msg);
                    }
                }
            }
        }
        
        self.render_replication_dialog(ctx);
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
        self.try_save_prefs();

        self.render_left_sidebar(ctx);

        // ─── AI Assistant Right Panel ───────────────────────────────────────────────
        self.render_ai_right_panel(ctx);

        // Central panel (main editor / data / structure)
        self.render_central_panel(ctx);

        // Handle copy operations AFTER UI render (state already updated)
        // Note: We only reach here if table/structure has potential focus (not editor/message)
        self.handle_table_copy_shortcut(ctx, copy_shortcut_detected);

        // Centralized, non-blocking toast notifications. Rendered last so they
        // stack above all panels and dialogs.
        self.toasts.show(ctx);
    } // end update
} // end impl App for Tabular


