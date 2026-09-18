//! Bagian "Backend" di Settings → AI Assistant: memilih HTTP API atau CLI
//! agent (`agy` / `claude` / `gemini` / custom), plus pekerjaan latar yang juga
//! dipakai panel chat: tes koneksi CLI dan pemeriksaan/registrasi MCP server
//! Tabular di konfigurasi global CLI. Juga bagian "Memory": vault Obsidian yang
//! dipakai sebagai memory AI (pemilihan folder, indeks latar, simpan catatan).

use std::sync::mpsc;

use eframe::egui;

use super::Tabular;
use super::preferences::{
    Tone, callout, divider, hint, quick_pick, row, section, status, toggle_row,
};
use super::style;
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
            let result =
                harness::register_mcp(&cfg).and_then(|_| harness::check_mcp_registered(&cfg));
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

    /// Ambil hasil thread latar (tes koneksi, cek MCP, indeks vault).
    /// Dipanggil tiap frame oleh panel chat dan tab settings.
    pub(crate) fn poll_ai_cli_background(&mut self, ctx: &egui::Context) {
        self.ensure_obsidian_index();
        if let Some(rx) = &self.ai_obsidian_index_receiver {
            match rx.try_recv() {
                Ok(result) => {
                    if let Err(e) = &result {
                        log::warn!("[OBSIDIAN] indexing failed: {e}");
                    }
                    self.ai_obsidian_index = Some(result);
                    self.ai_obsidian_index_receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.ai_obsidian_index =
                        Some(Err("Indexing thread stopped unexpectedly.".to_string()));
                    self.ai_obsidian_index_receiver = None;
                }
            }
        }
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
                    self.ai_cli_test_result =
                        Some(Err("Test thread stopped unexpectedly.".to_string()));
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
    /// pengaturannya. Pengaturan API digambar oleh pemanggil bila backend = API.
    pub(crate) fn render_ai_backend_settings(&mut self, ui: &mut egui::Ui) {
        self.poll_ai_cli_background(ui.ctx());

        section(ui, "Backend", |ui| {
            row(
                ui,
                "Backend",
                Some(
                    "CLI agents use the login of a tool already installed on this machine (no API key) \
                     and can inspect your databases through Tabular's built-in MCP server.",
                ),
                |ui| {
                    let mut backend = self.ai_backend;
                    ui.radio_value(&mut backend, AiBackend::Api, AiBackend::Api.display_name());
                    if !IS_MOBILE {
                        // Di build App Store tetap ditampilkan (nonaktif) supaya user tahu
                        // fitur ini ada di versi download langsung.
                        let resp = ui
                            .add_enabled(
                                cli_backend_available(),
                                egui::RadioButton::new(
                                    backend == AiBackend::Cli,
                                    AiBackend::Cli.display_name(),
                                ),
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
                },
            );
            if !IS_MOBILE && harness::is_app_sandboxed() {
                ui.add_space(4.0);
                callout(ui, Tone::Warning, |ui| {
                    status(ui, Tone::Warning, harness::SANDBOX_UNAVAILABLE_MESSAGE);
                });
                // Preferensi CLI yang terbawa dari build lain: kembalikan ke API
                // supaya pengaturan provider di bawah langsung tampil.
                if self.ai_backend == AiBackend::Cli {
                    self.ai_backend = AiBackend::Api;
                    self.save_ai_prefs();
                }
            }
        });

        if self.ai_backend != AiBackend::Cli {
            return;
        }

        section(ui, "CLI Agent", |ui| self.render_ai_cli_agent_rows(ui));
        section(ui, "Database Access", |ui| {
            self.render_ai_cli_mcp_status(ui)
        });
        section(ui, "Connection Test", |ui| self.render_ai_cli_test(ui));
    }

    fn render_ai_cli_agent_rows(&mut self, ui: &mut egui::Ui) {
        // ── Jenis CLI ───────────────────────────────────────────────────
        row(ui, "Agent", None, |ui| {
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
        });
        divider(ui);

        // ── Binary ──────────────────────────────────────────────────────
        let is_custom = self.ai_cli_kind == CliAgentKind::Custom;
        let default_bin = self.ai_cli_kind.default_binary();
        let bin_hint = is_custom.then_some(
            "The command is run with the arguments below; use {prompt}, {system}, {model} and \
             {session} as placeholders. Output is read as plain text.",
        );
        row(
            ui,
            if is_custom {
                "Command"
            } else {
                "Command / path"
            },
            bin_hint,
            |ui| {
                let hint_text = if default_bin.is_empty() {
                    "path to your CLI".to_string()
                } else {
                    format!("{default_bin} (found in PATH)")
                };
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.ai_settings_cli_bin_input)
                        .desired_width(220.0)
                        .hint_text(hint_text),
                );
                if resp.lost_focus() || ui.add(style::btn_secondary("Apply")).clicked() {
                    self.ai_cli_bin = self.ai_settings_cli_bin_input.trim().to_string();
                    self.ai_cli_mcp_registered = None;
                    self.save_ai_prefs();
                }
                if !default_bin.is_empty()
                && ui
                    .add(style::btn_secondary("Detect"))
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
                        self.toasts
                            .error(format!("`{default_bin}` not found. Install it or enter its full path."));
                    }
                }
            }
            },
        );
        divider(ui);

        // ── Model ───────────────────────────────────────────────────────
        let model_hint = (self.ai_cli_kind == CliAgentKind::Antigravity).then_some(
            "Run `agy models` for the full list. Gemini models carry their effort level in the name \
             (…-low/-medium/-high); the effort setting is then ignored.",
        );
        row(ui, "Model", model_hint, |ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.ai_settings_cli_model_input)
                    .desired_width(220.0)
                    .hint_text("(CLI default)"),
            );
            if resp.lost_focus() || ui.add(style::btn_secondary("Apply")).clicked() {
                self.ai_cli_model = self.ai_settings_cli_model_input.trim().to_string();
                self.save_ai_prefs();
            }
            if ui
                .add(style::btn_secondary("Default"))
                .on_hover_text("Let the CLI pick its own default model")
                .clicked()
            {
                self.ai_settings_cli_model_input.clear();
                self.ai_cli_model.clear();
                self.save_ai_prefs();
            }
        });
        if let Some(m) = quick_pick(
            ui,
            self.ai_cli_kind.preset_models(),
            &self.ai_settings_cli_model_input,
        ) {
            self.ai_settings_cli_model_input = m.to_string();
            self.ai_cli_model = m.to_string();
            self.save_ai_prefs();
        }

        // ── Effort ──────────────────────────────────────────────────────
        if self.ai_cli_kind.supports_effort() {
            divider(ui);
            row(ui, "Reasoning effort", None, |ui| {
                let before = self.ai_cli_effort.clone();
                egui::ComboBox::from_id_salt("ai_cli_effort")
                    .selected_text(if before.is_empty() {
                        "(default)"
                    } else {
                        before.as_str()
                    })
                    .width(140.0)
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
        }
        divider(ui);

        // ── Extra args ──────────────────────────────────────────────────
        row(
            ui,
            if is_custom {
                "Arguments"
            } else {
                "Extra arguments"
            },
            None,
            |ui| {
                let hint_text = match self.ai_cli_kind {
                    CliAgentKind::Antigravity => "e.g. --sandbox",
                    CliAgentKind::ClaudeCode => "e.g. --max-turns 8",
                    CliAgentKind::GeminiCli => "e.g. --approval-mode yolo",
                    CliAgentKind::Custom => "e.g. chat --model {model} {prompt}",
                };
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.ai_settings_cli_extra_args_input)
                        .desired_width(220.0)
                        .hint_text(hint_text),
                );
                if resp.lost_focus() || ui.add(style::btn_secondary("Apply")).clicked() {
                    self.ai_cli_extra_args =
                        self.ai_settings_cli_extra_args_input.trim().to_string();
                    self.save_ai_prefs();
                }
            },
        );
        divider(ui);

        // ── Live edit ───────────────────────────────────────────────────
        if toggle_row(
            ui,
            &mut self.ai_cli_auto_apply_edits,
            "Live edit",
            Some(
                "Write agent output marked for a tab straight into the SQL editor. When off, each edit \
                 shows an Apply button in the chat instead. Every edit can be reverted.",
            ),
        ) {
            self.save_ai_prefs();
        }
    }

    fn render_ai_cli_mcp_status(&mut self, ui: &mut egui::Ui) {
        hint(
            ui,
            "Tabular's MCP server gives the agent read-only access to your databases.",
        );
        ui.add_space(2.0);
        match self.ai_cli_kind {
            CliAgentKind::ClaudeCode => {
                status(
                    ui,
                    Tone::Success,
                    "✓ Passed to Claude Code on every request (--mcp-config); only Tabular's own tools are allowed and database access is read-only.",
                );
            }
            CliAgentKind::Custom => {
                status(
                    ui,
                    Tone::Muted,
                    "Register it yourself with the snippet from `tabular mcp --print-config`.",
                );
            }
            _ => {
                self.ensure_ai_mcp_check();
                ui.horizontal_wrapped(|ui| {
                    match self.ai_cli_mcp_registered {
                        None => {
                            ui.spinner();
                            hint(ui, "Checking…");
                        }
                        Some(true) => {
                            status(ui, Tone::Success, "✓ Registered in the CLI's global MCP config");
                        }
                        Some(false) => {
                            status(ui, Tone::Warning, "⚠ Not registered: the agent cannot query your databases");
                            if ui
                                .add(style::btn_primary_ctx(ui.ctx(), "Register"))
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
                    if self.ai_cli_mcp_receiver.is_none() && ui.add(style::btn_secondary("Re-check")).clicked() {
                        self.ai_cli_mcp_registered = None;
                        self.ai_cli_mcp_message = None;
                        self.ensure_ai_mcp_check();
                    }
                });
                if let Some(msg) = &self.ai_cli_mcp_message {
                    status(ui, Tone::Danger, msg.clone());
                }
            }
        }
    }

    fn render_ai_cli_test(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let testing = self.ai_cli_test_receiver.is_some();
            if ui
                .add_enabled(!testing, style::btn_secondary("Test connection"))
                .on_hover_text("Checks the binary, its version and sends a one-word prompt")
                .clicked()
            {
                self.start_ai_cli_test();
            }
            if testing {
                ui.spinner();
                hint(ui, "Running…");
            }
        });
        match &self.ai_cli_test_result {
            Some(Ok(msg)) => status(ui, Tone::Success, format!("✓ {msg}")),
            Some(Err(msg)) => status(ui, Tone::Danger, format!("✗ {msg}")),
            None => {}
        }
        ui.add_space(2.0);
        hint(
            ui,
            "The agent runs headless with permission prompts disabled, inside an empty working directory \
             under Tabular's data folder. Database access goes through Tabular's read-only MCP tools; \
             write statements must still be run by you.",
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Memory: vault Obsidian
// ─────────────────────────────────────────────────────────────────────────

impl Tabular {
    /// Root vault bila memory aktif dan folder sudah dipilih.
    pub(crate) fn obsidian_root(&self) -> Option<std::path::PathBuf> {
        let path = self.ai_obsidian_vault_path.trim();
        (self.ai_obsidian_enabled && !path.is_empty()).then(|| std::path::PathBuf::from(path))
    }

    /// Sinkronkan indeks vault di thread latar. Hasilnya diambil oleh
    /// [`Self::poll_ai_cli_background`].
    pub(crate) fn start_obsidian_index(&mut self) {
        let (Some(root), Some(pool), Some(rt)) = (
            self.obsidian_root(),
            self.db_pool.clone(),
            self.runtime.clone(),
        ) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = rt.block_on(crate::vector_index::sync_note_embeddings(&pool, &root));
            let _ = tx.send(result);
        });
        self.ai_obsidian_index_receiver = Some(rx);
    }

    /// Indeks sekali per sesi begitu panel AI / settings pertama kali dibuka.
    /// Sinkronisasi inkremental, jadi murah bila vault tidak berubah.
    fn ensure_obsidian_index(&mut self) {
        if self.ai_obsidian_index.is_none() && self.ai_obsidian_index_receiver.is_none() {
            self.start_obsidian_index();
        }
    }

    /// Simpan satu jawaban chat sebagai catatan memory di vault, lalu indeks
    /// ulang supaya langsung bisa di-recall.
    pub(crate) fn save_chat_to_vault(&mut self, title: &str, content: &str) {
        let Some(root) = self.obsidian_root() else {
            return;
        };
        let result = crate::obsidian::save_memory_note(&root, title, content, &[]);
        match &result {
            Ok(path) => log::info!("[OBSIDIAN] saved memory note: {path}"),
            Err(e) => log::warn!("[OBSIDIAN] save failed: {e}"),
        }
        if result.is_ok() && self.ai_obsidian_index_receiver.is_none() {
            self.start_obsidian_index();
        }
        self.ai_obsidian_save_message = Some(result);
    }

    /// Bagian "Memory" di Settings → AI Assistant; berlaku untuk semua backend.
    pub(crate) fn render_ai_memory_settings(&mut self, ui: &mut egui::Ui) {
        if IS_MOBILE {
            return;
        }
        self.poll_ai_cli_background(ui.ctx());

        section(ui, "Memory (Obsidian vault)", |ui| {
            hint(
                ui,
                "Point Tabular at an Obsidian vault (or any folder of Markdown notes) with your schema notes, \
                 business rules and query conventions. The most relevant note excerpts are added to each AI \
                 request, and CLI agents can search and read the notes themselves.",
            );
            ui.add_space(4.0);

            row(ui, "Vault folder", None, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.ai_obsidian_vault_path.is_empty() {
                        hint(ui, "No folder selected");
                    } else {
                        ui.label(egui::RichText::new(&self.ai_obsidian_vault_path).monospace());
                    }
                    let label = if self.ai_obsidian_vault_path.is_empty() {
                        "Add Obsidian folder…"
                    } else {
                        "Change…"
                    };
                    if ui.add(style::btn_secondary(label)).clicked()
                        && let Some(path) = crate::rfd::FileDialog::new()
                            .set_title("Select Obsidian vault folder")
                            .pick_folder()
                    {
                        self.ai_obsidian_vault_path = path.to_string_lossy().to_string();
                        self.ai_obsidian_enabled = true;
                        self.ai_obsidian_index = None;
                        self.save_ai_prefs();
                        self.start_obsidian_index();
                    }
                    if !self.ai_obsidian_vault_path.is_empty()
                        && ui.add(style::btn_secondary("Remove")).clicked()
                    {
                        self.ai_obsidian_vault_path.clear();
                        self.ai_obsidian_enabled = false;
                        self.ai_obsidian_allow_write = false;
                        self.ai_obsidian_index = None;
                        self.save_ai_prefs();
                    }
                });
            });

            if self.ai_obsidian_vault_path.is_empty() {
                return;
            }
            let root = std::path::PathBuf::from(self.ai_obsidian_vault_path.trim());
            if !root.is_dir() {
                status(
                    ui,
                    Tone::Danger,
                    "✗ Vault folder not found or not accessible. Choose it again.",
                );
                return;
            }
            if !root.join(".obsidian").is_dir() {
                status(
                    ui,
                    Tone::Muted,
                    "This folder has no .obsidian settings; it is used as a plain Markdown folder.",
                );
            }
            divider(ui);

            if toggle_row(
                ui,
                &mut self.ai_obsidian_enabled,
                "Use notes as AI memory",
                Some(
                    "Relevant excerpts of your notes are sent to the AI provider together with your request. \
                     Turn this off to keep the vault private.",
                ),
            ) {
                self.ai_obsidian_index = None;
                self.save_ai_prefs();
            }
            if !self.ai_obsidian_enabled {
                return;
            }

            if toggle_row(
                ui,
                &mut self.ai_obsidian_allow_write,
                "Allow AI to save notes",
                Some(
                    "Lets the assistant store things worth remembering as new notes in the \
                     \"Tabular Memory\" folder of the vault. Existing notes are never modified.",
                ),
            ) {
                self.save_ai_prefs();
            }
            divider(ui);

            ui.horizontal_wrapped(|ui| {
                let indexing = self.ai_obsidian_index_receiver.is_some();
                if indexing {
                    ui.spinner();
                    hint(ui, "Indexing…");
                } else {
                    match &self.ai_obsidian_index {
                        Some(Ok(stats)) => status(
                            ui,
                            Tone::Success,
                            format!("✓ {} notes indexed ({} excerpts)", stats.notes, stats.chunks),
                        ),
                        Some(Err(e)) => status(ui, Tone::Danger, format!("✗ {e}")),
                        None => hint(ui, "Not indexed yet"),
                    }
                }
                if ui
                    .add_enabled(!indexing, style::btn_secondary("Re-index"))
                    .on_hover_text("Only new and changed notes are read again")
                    .clicked()
                {
                    self.start_obsidian_index();
                }
            });
            if harness::is_app_sandboxed() {
                hint(
                    ui,
                    "App Store build: macOS may revoke access to the folder after a restart; choose it again if indexing fails.",
                );
            }
        });
    }
}
