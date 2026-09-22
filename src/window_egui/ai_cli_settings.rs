//! Bagian "CLI Agents" di Settings → AI Assistant: mengonfigurasi beberapa CLI
//! agent sekaligus (`agy` / `claude` / `gemini` / custom), plus pekerjaan latar yang juga
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
use crate::config::{ChatTarget, CliAgentKind, CliAgentProfile};

/// Backend CLI tidak ada sama sekali di mobile: tidak ada binary agent yang
/// bisa dijalankan.
const IS_MOBILE: bool = cfg!(any(target_os = "ios", target_os = "android"));

/// Backend CLI bisa dipakai: bukan mobile dan bukan build Mac App Store
/// (App Sandbox, lihat [`harness::is_app_sandboxed`]).
fn cli_backend_available() -> bool {
    !IS_MOBILE && !harness::is_app_sandboxed()
}

impl Tabular {
    pub(crate) fn cli_profile(&self, kind: CliAgentKind) -> &CliAgentProfile {
        self.ai_cli_profiles
            .get(&kind)
            .expect("cli profile must exist after normalization")
    }

    pub(crate) fn cli_profile_mut(&mut self, kind: CliAgentKind) -> &mut CliAgentProfile {
        self.ai_cli_profiles
            .entry(kind)
            .or_insert_with(|| CliAgentProfile::new(kind))
    }

    pub(crate) fn ai_cli_config_for(&self, kind: CliAgentKind) -> CliAgentConfig {
        let p = self.cli_profile(kind);
        CliAgentConfig::from(p)
    }

    pub(crate) fn target_enabled(&self, target: ChatTarget) -> bool {
        match target {
            ChatTarget::Api => true,
            ChatTarget::Cli(kind) => cli_backend_available() && self.cli_profile(kind).enabled,
        }
    }

    pub(crate) fn enabled_chat_targets(&self) -> Vec<ChatTarget> {
        let mut list = vec![ChatTarget::Api];
        if cli_backend_available() {
            for kind in CliAgentKind::ALL {
                if self.cli_profile(kind).enabled {
                    list.push(ChatTarget::Cli(kind));
                }
            }
        }
        list
    }

    pub(crate) fn effective_chat_target(&self) -> ChatTarget {
        let target = self.ai_chat_target;
        if self.target_enabled(target) {
            target
        } else {
            self.enabled_chat_targets()
                .into_iter()
                .next()
                .unwrap_or(ChatTarget::Api)
        }
    }

    pub(crate) fn effective_default_target(&self) -> ChatTarget {
        let target = self.ai_default_target;
        if self.target_enabled(target) {
            target
        } else {
            self.enabled_chat_targets()
                .into_iter()
                .next()
                .unwrap_or(ChatTarget::Api)
        }
    }

    pub(crate) fn mcp_registered(&self, kind: CliAgentKind) -> Option<bool> {
        self.ai_cli_mcp.get(&kind).and_then(|s| s.registered)
    }

    /// Mulai pemeriksaan "apakah MCP Tabular terdaftar di CLI" untuk agen tertentu bila belum
    /// diketahui. Idempoten; hasilnya diambil oleh [`Self::poll_ai_cli_background`].
    pub(crate) fn ensure_ai_mcp_check(&mut self, kind: CliAgentKind) {
        if !cli_backend_available() || !kind.needs_global_mcp_registration() {
            return;
        }
        if let Some(status) = self.ai_cli_mcp.get(&kind) {
            if status.registered.is_some() || status.receiver.is_some() {
                return;
            }
        }
        let cfg = self.ai_cli_config_for(kind);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(harness::check_mcp_registered(&cfg));
        });
        let status = self.ai_cli_mcp.entry(kind).or_default();
        status.receiver = Some(rx);
    }

    /// Daftarkan MCP Tabular lewat `<cli> mcp add …`, lalu periksa ulang.
    pub(crate) fn start_ai_mcp_register(&mut self, kind: CliAgentKind) {
        let cfg = self.ai_cli_config_for(kind);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result =
                harness::register_mcp(&cfg).and_then(|_| harness::check_mcp_registered(&cfg));
            let _ = tx.send(result);
        });
        let status = self.ai_cli_mcp.entry(kind).or_default();
        status.registered = None;
        status.message = None;
        status.receiver = Some(rx);
    }

    fn start_ai_cli_test(&mut self, kind: CliAgentKind) {
        let cfg = self.ai_cli_config_for(kind);
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
        for (kind, mcp_status) in &mut self.ai_cli_mcp {
            if let Some(rx) = &mcp_status.receiver {
                match rx.try_recv() {
                    Ok(Ok(registered)) => {
                        mcp_status.registered = Some(registered);
                        mcp_status.message = None;
                        mcp_status.receiver = None;
                    }
                    Ok(Err(e)) => {
                        log::warn!("[AGENT] MCP registration check failed for {kind:?}: {e}");
                        mcp_status.registered = Some(false);
                        mcp_status.message = Some(e);
                        mcp_status.receiver = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        mcp_status.registered = Some(false);
                        mcp_status.receiver = None;
                    }
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

    pub(crate) fn save_ai_prefs(&mut self) {
        self.prefs_dirty = true;
        self.try_save_prefs();
    }

    /// Bagian "CLI Agents" di Settings → AI Assistant.
    pub(crate) fn render_ai_cli_section(&mut self, ui: &mut egui::Ui) {
        self.poll_ai_cli_background(ui.ctx());

        section(ui, "CLI Agents", |ui| {
            hint(
                ui,
                "Configure local CLI coding agents. Enabled agents can be selected in the AI Assistant chat panel.",
            );
            if !IS_MOBILE && harness::is_app_sandboxed() {
                ui.add_space(4.0);
                callout(ui, Tone::Warning, |ui| {
                    status(ui, Tone::Warning, harness::SANDBOX_UNAVAILABLE_MESSAGE);
                });
                return;
            }
            if IS_MOBILE {
                return;
            }

            ui.add_space(6.0);
            // Tab bar pemilih agen CLI
            ui.horizontal(|ui| {
                for kind in CliAgentKind::ALL {
                    let is_sel = self.ai_settings_cli_tab == kind;
                    let profile_enabled = self.cli_profile(kind).enabled;
                    let label = if profile_enabled {
                        format!("{} ✓", kind.display_name())
                    } else {
                        kind.display_name().to_string()
                    };
                    if ui.selectable_label(is_sel, label).clicked() {
                        self.ai_settings_cli_tab = kind;
                        self.ai_cli_test_result = None;
                    }
                }
            });
            divider(ui);

            self.render_ai_cli_agent_rows(ui);
        });

        if !IS_MOBILE && !harness::is_app_sandboxed() {
            let tab = self.ai_settings_cli_tab;
            section(ui, "Database Access", |ui| {
                self.render_ai_cli_mcp_status(ui, tab)
            });
            section(ui, "Connection Test", |ui| {
                self.render_ai_cli_test(ui, tab)
            });
        }
    }

    fn render_ai_cli_agent_rows(&mut self, ui: &mut egui::Ui) {
        let tab = self.ai_settings_cli_tab;

        // ── Enable checkbox ─────────────────────────────────────────────
        let mut enabled = self.cli_profile(tab).enabled;
        if ui
            .checkbox(&mut enabled, "Enable this agent for chat")
            .changed()
        {
            self.cli_profile_mut(tab).enabled = enabled;
            self.save_ai_prefs();
        }
        divider(ui);

        // ── Binary / Command ────────────────────────────────────────────
        let is_custom = tab == CliAgentKind::Custom;
        let default_bin = tab.default_binary();
        let bin_hint = is_custom.then_some(
            "The command is run with the arguments below; use {prompt}, {system}, {model} and \
             {session} as placeholders. Output is read as plain text.",
        );
        let mut bin_changed = false;
        let mut detected_path = None;
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
                let has_detect = !default_bin.is_empty();
                let buttons_w = if has_detect { 140.0 } else { 70.0 };
                let spacing = 6.0;
                let field_w = (ui.available_width() - buttons_w - spacing).clamp(160.0, 320.0);
                let profile = self.cli_profile_mut(tab);
                let resp = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut profile.bin).hint_text(hint_text),
                    field_w,
                    None,
                );
                ui.add_space(spacing);
                if resp.lost_focus() || ui.add(style::btn_field_action(ui, "Apply")).clicked() {
                    bin_changed = true;
                }
                if has_detect {
                    ui.add_space(spacing);
                    if ui
                        .add(style::btn_field_action(ui, "Detect"))
                        .on_hover_text("Search PATH and common install locations (~/.local/bin, Homebrew, npm, …)")
                        .clicked()
                    {
                        match harness::resolve_binary(default_bin) {
                            Some(path) => {
                                detected_path = Some(path.to_string_lossy().to_string());
                            }
                            None => {
                                self.toasts.error(format!(
                                    "`{default_bin}` not found. Install it or enter its full path."
                                ));
                            }
                        }
                    }
                }
            },
        );
        if let Some(p) = detected_path {
            self.cli_profile_mut(tab).bin = p.clone();
            if let Some(status) = self.ai_cli_mcp.get_mut(&tab) {
                status.registered = None;
            }
            self.save_ai_prefs();
            self.toasts.success(format!("Found {p}"));
        } else if bin_changed {
            if let Some(status) = self.ai_cli_mcp.get_mut(&tab) {
                status.registered = None;
            }
            self.save_ai_prefs();
        }
        divider(ui);

        // ── Model ───────────────────────────────────────────────────────
        let model_hint = (tab == CliAgentKind::Antigravity).then_some(
            "Run `agy models` for the full list. Gemini models carry their effort level in the name \
             (…-low/-medium/-high); the effort setting is then ignored.",
        );
        let mut model_changed = false;
        let mut clear_model = false;
        row(ui, "Model", model_hint, |ui| {
            let buttons_w = 140.0;
            let spacing = 6.0;
            let field_w = (ui.available_width() - buttons_w - spacing).clamp(160.0, 320.0);
            let profile = self.cli_profile_mut(tab);
            let resp = style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut profile.model).hint_text("(CLI default)"),
                field_w,
                None,
            );
            ui.add_space(spacing);
            if resp.lost_focus() || ui.add(style::btn_field_action(ui, "Apply")).clicked() {
                model_changed = true;
            }
            ui.add_space(spacing);
            if ui
                .add(style::btn_field_action(ui, "Default"))
                .on_hover_text("Let the CLI pick its own default model")
                .clicked()
            {
                clear_model = true;
            }
        });
        if clear_model {
            self.cli_profile_mut(tab).model.clear();
            self.save_ai_prefs();
        } else if model_changed {
            self.save_ai_prefs();
        }
        if let Some(m) = quick_pick(ui, tab.preset_models(), &self.cli_profile(tab).model) {
            self.cli_profile_mut(tab).model = m.to_string();
            self.save_ai_prefs();
        }

        // ── Effort ──────────────────────────────────────────────────────
        if tab.supports_effort() {
            divider(ui);
            row(ui, "Reasoning effort", None, |ui| {
                let before = self.cli_profile(tab).effort.clone();
                let mut current = before.clone();
                egui::ComboBox::from_id_salt(format!("ai_cli_effort_{tab:?}"))
                    .selected_text(if current.is_empty() {
                        "(default)"
                    } else {
                        current.as_str()
                    })
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut current, String::new(), "(default)");
                        for lvl in ["low", "medium", "high"] {
                            ui.selectable_value(&mut current, lvl.to_string(), lvl);
                        }
                    });
                if current != before {
                    self.cli_profile_mut(tab).effort = current;
                    self.save_ai_prefs();
                }
            });
        }
        divider(ui);

        // ── Extra args ──────────────────────────────────────────────────
        let mut args_changed = false;
        row(
            ui,
            if is_custom {
                "Arguments"
            } else {
                "Extra arguments"
            },
            None,
            |ui| {
                let hint_text = match tab {
                    CliAgentKind::Antigravity => "e.g. --sandbox",
                    CliAgentKind::ClaudeCode => "e.g. --max-turns 8",
                    CliAgentKind::GeminiCli => "e.g. --approval-mode yolo",
                    CliAgentKind::Custom => "e.g. chat --model {model} {prompt}",
                };
                let buttons_w = 70.0;
                let spacing = 6.0;
                let field_w = (ui.available_width() - buttons_w - spacing).clamp(160.0, 320.0);
                let profile = self.cli_profile_mut(tab);
                let resp = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut profile.extra_args).hint_text(hint_text),
                    field_w,
                    None,
                );
                ui.add_space(spacing);
                if resp.lost_focus() || ui.add(style::btn_field_action(ui, "Apply")).clicked() {
                    args_changed = true;
                }
            },
        );
        if args_changed {
            self.save_ai_prefs();
        }
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

    fn render_ai_cli_mcp_status(&mut self, ui: &mut egui::Ui, kind: CliAgentKind) {
        hint(
            ui,
            "Tabular's MCP server gives the agent read-only access to your databases.",
        );
        ui.add_space(2.0);
        match kind {
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
                self.ensure_ai_mcp_check(kind);
                let (reg, msg, is_busy) = {
                    let s = self.ai_cli_mcp.get(&kind);
                    (
                        s.and_then(|x| x.registered),
                        s.and_then(|x| x.message.clone()),
                        s.and_then(|x| x.receiver.as_ref()).is_some(),
                    )
                };
                ui.horizontal_wrapped(|ui| {
                    match reg {
                        None => {
                            ui.spinner();
                            hint(ui, "Checking…");
                        }
                        Some(true) => {
                            status(ui, Tone::Success, "✓ Registered in the CLI's global MCP config");
                        }
                        Some(false) => {
                            status(
                                ui,
                                Tone::Warning,
                                "⚠ Not registered: the agent cannot query your databases",
                            );
                            if ui
                                .add(style::btn_primary_ctx(ui.ctx(), "Register"))
                                .on_hover_text(format!(
                                    "Runs `{} mcp add tabular -- {} mcp` (modifies the CLI's global config)",
                                    kind.default_binary(),
                                    harness::tabular_exe()
                                ))
                                .clicked()
                            {
                                self.start_ai_mcp_register(kind);
                            }
                        }
                    }
                    if !is_busy && ui.add(style::btn_secondary("Re-check")).clicked() {
                        if let Some(s) = self.ai_cli_mcp.get_mut(&kind) {
                            s.registered = None;
                            s.message = None;
                        }
                        self.ensure_ai_mcp_check(kind);
                    }
                });
                if let Some(m) = msg {
                    status(ui, Tone::Danger, m);
                }
            }
        }
    }

    fn render_ai_cli_test(&mut self, ui: &mut egui::Ui, kind: CliAgentKind) {
        ui.horizontal(|ui| {
            let testing = self.ai_cli_test_receiver.is_some();
            if ui
                .add_enabled(!testing, style::btn_secondary("Test connection"))
                .on_hover_text("Checks the binary, its version and sends a one-word prompt")
                .clicked()
            {
                self.start_ai_cli_test(kind);
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

    /// Bagian "Default Target" di Settings → AI Assistant.
    pub(crate) fn render_ai_default_target(&mut self, ui: &mut egui::Ui) {
        section(ui, "Default Target", |ui| {
            row(
                ui,
                "Default agent",
                Some(
                    "Used by non-chat AI features: the inline `--AI` block in the SQL editor \
                     and AI generation in the HTTP Client.",
                ),
                |ui| {
                    let targets = self.enabled_chat_targets();
                    let current = self.effective_default_target();
                    let mut selected = current;
                    egui::ComboBox::from_id_salt("ai_default_target_select")
                        .selected_text(crate::ai_assistant::backend_label_for(self, current))
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for t in targets {
                                let label = crate::ai_assistant::backend_label_for(self, t);
                                ui.selectable_value(&mut selected, t, label);
                            }
                        });
                    if selected != self.ai_default_target {
                        self.ai_default_target = selected;
                        self.save_ai_prefs();
                    }
                },
            );
        });
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
                            format!(
                                "✓ {} notes indexed ({} excerpts)",
                                stats.notes, stats.chunks
                            ),
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
