//! Bagian "Backend" di Settings → AI Assistant: memilih HTTP API atau CLI
//! agent (`agy` / `claude` / `gemini` / custom), plus pekerjaan latar yang juga
//! dipakai panel chat: tes koneksi CLI dan pemeriksaan/registrasi MCP server
//! Tabular di konfigurasi global CLI.

use std::sync::mpsc;

use eframe::egui;

use super::Tabular;
use crate::agent::harness::{self, CliAgentConfig};
use crate::config::{AiBackend, CliAgentKind};

/// Backend CLI tidak ada sama sekali di mobile: tidak ada binary agent yang
/// bisa dijalankan.
const IS_MOBILE: bool = cfg!(any(target_os = "ios", target_os = "android"));

/// Backend CLI bisa dipakai: bukan mobile dan bukan build Mac App Store
/// (App Sandbox, lihat [`harness::is_app_sandboxed`]).
fn cli_backend_available() -> bool {
    !IS_MOBILE && !harness::is_app_sandboxed()
}

impl Tabular {
    pub(crate) fn ai_cli_config(&self) -> CliAgentConfig {
        CliAgentConfig {
            kind: self.ai_cli_kind,
            bin: self.ai_cli_bin.clone(),
            model: self.ai_cli_model.clone(),
            effort: self.ai_cli_effort.clone(),
            extra_args: self.ai_cli_extra_args.clone(),
        }
    }

    /// Mulai pemeriksaan "apakah MCP Tabular terdaftar di CLI" bila belum
    /// diketahui. Idempoten; hasilnya diambil oleh [`Self::poll_ai_cli_background`].
    pub(crate) fn ensure_ai_mcp_check(&mut self) {
        if !cli_backend_available()
            || self.ai_backend != AiBackend::Cli
            || !self.ai_cli_kind.needs_global_mcp_registration()
            || self.ai_cli_mcp_registered.is_some()
            || self.ai_cli_mcp_receiver.is_some()
        {
            return;
        }
        let cfg = self.ai_cli_config();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(harness::check_mcp_registered(&cfg));
        });
        self.ai_cli_mcp_receiver = Some(rx);
    }

    /// Daftarkan MCP Tabular lewat `<cli> mcp add …`, lalu periksa ulang.
    pub(crate) fn start_ai_mcp_register(&mut self) {
        let cfg = self.ai_cli_config();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = harness::register_mcp(&cfg).and_then(|_| harness::check_mcp_registered(&cfg));
            let _ = tx.send(result);
        });
        self.ai_cli_mcp_registered = None;
        self.ai_cli_mcp_message = None;
        self.ai_cli_mcp_receiver = Some(rx);
    }

    fn start_ai_cli_test(&mut self) {
        let cfg = self.ai_cli_config();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(harness::test_connection(&cfg));
        });
        self.ai_cli_test_result = None;
        self.ai_cli_test_receiver = Some(rx);
    }

    /// Ambil hasil thread latar (tes koneksi, cek MCP). Dipanggil tiap frame
    /// oleh panel chat dan tab settings.
    pub(crate) fn poll_ai_cli_background(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.ai_cli_mcp_receiver {
            match rx.try_recv() {
                Ok(Ok(registered)) => {
                    self.ai_cli_mcp_registered = Some(registered);
                    self.ai_cli_mcp_message = None;
                    self.ai_cli_mcp_receiver = None;
                }
                Ok(Err(e)) => {
                    log::warn!("[AGENT] MCP registration check failed: {e}");
                    self.ai_cli_mcp_registered = Some(false);
                    self.ai_cli_mcp_message = Some(e);
                    self.ai_cli_mcp_receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.ai_cli_mcp_registered = Some(false);
                    self.ai_cli_mcp_receiver = None;
                }
            }
        }
        if let Some(rx) = &self.ai_cli_test_receiver {
            match rx.try_recv() {
                Ok(result) => {
                    self.ai_cli_test_result = Some(result);
                    self.ai_cli_test_receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.ai_cli_test_result = Some(Err("Test thread stopped unexpectedly.".to_string()));
                    self.ai_cli_test_receiver = None;
                }
            }
        }
    }

    fn save_ai_prefs(&mut self) {
        self.prefs_dirty = true;
        self.try_save_prefs();
    }

    /// Bagian atas tab AI Assistant: pemilihan backend dan, untuk CLI, semua
    /// pengaturannya. Pengaturan API yang lama digambar oleh pemanggil bila
    /// backend = API.
    pub(crate) fn render_ai_backend_settings(&mut self, ui: &mut egui::Ui) {
        self.poll_ai_cli_background(ui.ctx());
        let muted = egui::Color32::from_gray(130);

        ui.label("Backend:");
        ui.horizontal_wrapped(|ui| {
            let mut backend = self.ai_backend;
            ui.radio_value(&mut backend, AiBackend::Api, AiBackend::Api.display_name());
            if !IS_MOBILE {
                // Di build App Store tetap ditampilkan (nonaktif) supaya user tahu
                // fitur ini ada di versi download langsung.
                let resp = ui
                    .add_enabled(
                        cli_backend_available(),
                        egui::RadioButton::new(backend == AiBackend::Cli, AiBackend::Cli.display_name()),
                    )
                    .on_disabled_hover_text(harness::SANDBOX_UNAVAILABLE_MESSAGE);
                if resp.clicked() {
                    backend = AiBackend::Cli;
                }
            }
            if backend != self.ai_backend {
                self.ai_backend = backend;
                self.ai_cli_mcp_registered = None;
                self.save_ai_prefs();
            }
        });
        ui.label(
            egui::RichText::new(
                "CLI agents use the login of a tool already installed on this machine (no API key) \
                 and can inspect your databases through Tabular's built-in MCP server.",
            )
            .size(11.0)
            .color(muted),
        );
        if !IS_MOBILE && harness::is_app_sandboxed() {
            ui.label(
                egui::RichText::new(format!("ℹ {}", harness::SANDBOX_UNAVAILABLE_MESSAGE))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 160, 30)),
            );
            // Preferensi CLI yang terbawa dari build lain: kembalikan ke API
            // supaya pengaturan provider di bawah langsung tampil.
            if self.ai_backend == AiBackend::Cli {
                self.ai_backend = AiBackend::Api;
                self.save_ai_prefs();
            }
        }
        ui.add_space(6.0);

        if self.ai_backend != AiBackend::Cli {
            return;
        }

        // ── Jenis CLI ───────────────────────────────────────────────────
        ui.label("CLI agent:");
        ui.horizontal_wrapped(|ui| {
            let mut kind = self.ai_cli_kind;
            for k in [
                CliAgentKind::Antigravity,
                CliAgentKind::ClaudeCode,
                CliAgentKind::GeminiCli,
                CliAgentKind::Custom,
            ] {
                ui.radio_value(&mut kind, k, k.display_name());
            }
            if kind != self.ai_cli_kind {
                self.ai_cli_kind = kind;
                self.ai_cli_bin.clear();
                self.ai_settings_cli_bin_input.clear();
                self.ai_cli_model.clear();
                self.ai_settings_cli_model_input.clear();
                self.ai_cli_effort.clear();
                self.ai_cli_extra_args.clear();
                self.ai_settings_cli_extra_args_input.clear();
                self.ai_cli_mcp_registered = None;
                self.ai_cli_mcp_message = None;
                self.ai_cli_test_result = None;
                self.ai_session_id = None;
                self.save_ai_prefs();
            }
        });
        ui.add_space(6.0);

        // ── Binary ──────────────────────────────────────────────────────
        let default_bin = self.ai_cli_kind.default_binary();
        ui.label(if self.ai_cli_kind == CliAgentKind::Custom { "Command:" } else { "Command / path:" });
        ui.horizontal(|ui| {
            let hint = if default_bin.is_empty() {
                "path to your CLI".to_string()
            } else {
                format!("{default_bin} (found in PATH)")
            };
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.ai_settings_cli_bin_input)
                    .desired_width(300.0)
                    .hint_text(hint),
            );
            if resp.lost_focus() || ui.button("Apply").clicked() {
                self.ai_cli_bin = self.ai_settings_cli_bin_input.trim().to_string();
                self.ai_cli_mcp_registered = None;
                self.save_ai_prefs();
            }
            if !default_bin.is_empty()
                && ui
                    .small_button("Detect")
                    .on_hover_text("Search PATH and common install locations (~/.local/bin, Homebrew, npm, …)")
                    .clicked()
            {
                match harness::resolve_binary(default_bin) {
                    Some(path) => {
                        self.ai_settings_cli_bin_input = path.to_string_lossy().to_string();
                        self.ai_cli_bin = self.ai_settings_cli_bin_input.clone();
                        self.ai_cli_mcp_registered = None;
                        self.save_ai_prefs();
                        self.toasts.success(format!("Found {}", path.display()));
                    }
                    None => {
                        self.toasts.error(format!("`{default_bin}` not found. Install it or enter its full path."));
                    }
                }
            }
        });
        if self.ai_cli_kind == CliAgentKind::Custom {
            ui.label(
                egui::RichText::new(
                    "Custom: the command is run with the extra arguments below; use {prompt}, {system}, \
                     {model} and {session} as placeholders. Output is read as plain text.",
                )
                .size(11.0)
                .color(muted),
            );
        }
        ui.add_space(6.0);

        // ── Model ───────────────────────────────────────────────────────
        ui.label("Model:");
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.ai_settings_cli_model_input)
                    .desired_width(220.0)
                    .hint_text("(CLI default)"),
            );
            if resp.lost_focus() || ui.button("Apply").clicked() {
                self.ai_cli_model = self.ai_settings_cli_model_input.trim().to_string();
                self.save_ai_prefs();
            }
            if ui.small_button("Default").on_hover_text("Let the CLI pick its own default model").clicked() {
                self.ai_settings_cli_model_input.clear();
                self.ai_cli_model.clear();
                self.save_ai_prefs();
            }
        });
        let presets = self.ai_cli_kind.preset_models();
        if !presets.is_empty() {
            ui.label(egui::RichText::new("Quick pick:").size(11.0).color(muted));
            ui.horizontal_wrapped(|ui| {
                for &m in presets {
                    let selected = self.ai_settings_cli_model_input == m;
                    if ui.selectable_label(selected, egui::RichText::new(m).size(11.0).monospace()).clicked() {
                        self.ai_settings_cli_model_input = m.to_string();
                        self.ai_cli_model = m.to_string();
                        self.save_ai_prefs();
                    }
                }
            });
            if self.ai_cli_kind == CliAgentKind::Antigravity {
                ui.label(
                    egui::RichText::new(
                        "Run `agy models` in a terminal for the full list. Gemini models carry their effort level in the name (…-low/-medium/-high); the effort setting below is then ignored.",
                    )
                    .size(11.0)
                    .color(muted),
                );
            }
        }
        ui.add_space(6.0);

        // ── Effort ──────────────────────────────────────────────────────
        if self.ai_cli_kind.supports_effort() {
            ui.horizontal(|ui| {
                ui.label("Reasoning effort:");
                let before = self.ai_cli_effort.clone();
                egui::ComboBox::from_id_salt("ai_cli_effort")
                    .selected_text(if before.is_empty() { "(default)" } else { before.as_str() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.ai_cli_effort, String::new(), "(default)");
                        for lvl in ["low", "medium", "high"] {
                            ui.selectable_value(&mut self.ai_cli_effort, lvl.to_string(), lvl);
                        }
                    });
                if self.ai_cli_effort != before {
                    self.save_ai_prefs();
                }
            });
            ui.add_space(6.0);
        }

        // ── Extra args ──────────────────────────────────────────────────
        ui.label(if self.ai_cli_kind == CliAgentKind::Custom { "Arguments:" } else { "Extra arguments:" });
        ui.horizontal(|ui| {
            let hint = match self.ai_cli_kind {
                CliAgentKind::Antigravity => "e.g. --sandbox",
                CliAgentKind::ClaudeCode => "e.g. --max-turns 8",
                CliAgentKind::GeminiCli => "e.g. --approval-mode yolo",
                CliAgentKind::Custom => "e.g. chat --model {model} {prompt}",
            };
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.ai_settings_cli_extra_args_input)
                    .desired_width(300.0)
                    .hint_text(hint),
            );
            if resp.lost_focus() || ui.button("Apply").clicked() {
                self.ai_cli_extra_args = self.ai_settings_cli_extra_args_input.trim().to_string();
                self.save_ai_prefs();
            }
        });
        ui.add_space(6.0);

        // ── Live edit ───────────────────────────────────────────────────
        if ui
            .checkbox(
                &mut self.ai_cli_auto_apply_edits,
                "Live edit: write agent output marked for a tab straight into the SQL editor",
            )
            .on_hover_text("When off, each edit shows an Apply button in the chat instead. Every edit can be reverted.")
            .changed()
        {
            self.save_ai_prefs();
        }
        ui.add_space(6.0);

        // ── MCP status ──────────────────────────────────────────────────
        ui.label("Database access (Tabular MCP server):");
        match self.ai_cli_kind {
            CliAgentKind::ClaudeCode => {
                ui.label(
                    egui::RichText::new("✓ Passed to Claude Code on every request (--mcp-config); only Tabular's read-only tools are allowed.")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(0, 180, 80)),
                );
            }
            CliAgentKind::Custom => {
                ui.label(
                    egui::RichText::new("Register it yourself with the snippet from `tabular mcp --print-config`.")
                        .size(11.0)
                        .color(muted),
                );
            }
            _ => {
                self.ensure_ai_mcp_check();
                ui.horizontal_wrapped(|ui| {
                    match self.ai_cli_mcp_registered {
                        None => {
                            ui.spinner();
                            ui.label(egui::RichText::new("Checking…").size(11.0).color(muted));
                        }
                        Some(true) => {
                            ui.label(
                                egui::RichText::new("✓ Registered in the CLI's global MCP config")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(0, 180, 80)),
                            );
                        }
                        Some(false) => {
                            ui.label(
                                egui::RichText::new("⚠ Not registered — the agent cannot query your databases")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(220, 160, 30)),
                            );
                            if ui
                                .button("Register")
                                .on_hover_text(format!(
                                    "Runs `{} mcp add tabular -- {} mcp` (modifies the CLI's global config)",
                                    self.ai_cli_kind.default_binary(),
                                    harness::tabular_exe()
                                ))
                                .clicked()
                            {
                                self.start_ai_mcp_register();
                            }
                        }
                    }
                    if self.ai_cli_mcp_receiver.is_none() && ui.small_button("Re-check").clicked() {
                        self.ai_cli_mcp_registered = None;
                        self.ai_cli_mcp_message = None;
                        self.ensure_ai_mcp_check();
                    }
                });
                if let Some(msg) = &self.ai_cli_mcp_message {
                    ui.label(egui::RichText::new(msg).size(11.0).color(egui::Color32::from_rgb(255, 90, 90)));
                }
            }
        }
        ui.add_space(6.0);

        // ── Test ────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            let testing = self.ai_cli_test_receiver.is_some();
            if ui
                .add_enabled(!testing, egui::Button::new("Test connection"))
                .on_hover_text("Checks the binary, its version and sends a one-word prompt")
                .clicked()
            {
                self.start_ai_cli_test();
            }
            if testing {
                ui.spinner();
                ui.label(egui::RichText::new("Running…").size(11.0).color(muted));
            }
        });
        match &self.ai_cli_test_result {
            Some(Ok(msg)) => {
                ui.label(egui::RichText::new(format!("✓ {msg}")).size(11.0).color(egui::Color32::from_rgb(0, 180, 80)));
            }
            Some(Err(msg)) => {
                ui.label(egui::RichText::new(format!("✗ {msg}")).size(11.0).color(egui::Color32::from_rgb(255, 90, 90)));
            }
            None => {}
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Note: the agent runs in headless mode with permission prompts disabled, inside an empty \
                 working directory under Tabular's data folder. Database access goes through Tabular's \
                 read-only MCP tools; write statements must still be run by you.",
            )
            .size(11.0)
            .color(muted),
        );
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
    }
}
