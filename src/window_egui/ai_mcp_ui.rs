//! UI MCP server luar untuk AI Assistant (K5): bagian "MCP Servers" di
//! Settings → AI Assistant, kartu pemanggilan tool (Approve / Deny) di
//! transkrip chat, dan penghubung giliran chat HTTP API ke loop tool calling
//! (`crate::ai_tool_chat`).
//!
//! Logika murni ada di `ai_tool_calling` dan `outside_mcp`; client MCP di
//! `outside_mcp_client`. Modul ini hanya glue antara state `Tabular` dan
//! modul-modul headless tersebut.

use std::sync::mpsc;

use eframe::egui;

use super::Tabular;
use super::preferences::{Tone, callout, divider, hint, section, status, toggle};
use crate::ai_tool_calling::{ToolCallRecord, ToolCallStatus};
use crate::outside_mcp::{self, EnvVar, OutsideMcpServer};

/// Hasil "Test / List tools": (id server, daftar tool atau error).
type ListResult = Result<Vec<(String, String, serde_json::Value)>, String>;

/// Form tambah / edit server.
#[derive(Default, Clone)]
pub struct ServerDraft {
    pub id: i64,
    pub name: String,
    pub command: String,
    pub args_text: String,
    pub env: Vec<EnvVar>,
    pub enabled: bool,
    pub error: Option<String>,
}

/// State UI MCP server luar (tidak dipersist; konfigurasi ada di DB).
#[derive(Default)]
pub struct AiMcpUiState {
    loaded: bool,
    /// Server dengan env yang sudah di-unseal (nilai asli).
    pub servers: Vec<OutsideMcpServer>,
    draft: Option<ServerDraft>,
    list_rx: Option<(i64, mpsc::Receiver<ListResult>)>,
    /// Pesan hasil terakhir per server (sukses?, teks).
    messages: std::collections::HashMap<i64, (bool, String)>,
    confirm_delete: Option<i64>,
    /// Giliran chat bertool yang sedang berjalan.
    #[cfg(not(target_os = "ios"))]
    turn: Option<crate::ai_tool_chat::ToolChatHandle>,
}

impl Tabular {
    /// Muat server dari `connections.db` sekali (lazy).
    fn ai_mcp_ensure_loaded(&mut self) {
        if self.ai_mcp.loaded {
            return;
        }
        self.ai_mcp.loaded = true;
        self.ai_mcp_reload();
    }

    fn ai_mcp_reload(&mut self) {
        let (Some(pool), Some(rt)) = (self.db_pool.clone(), self.runtime.clone()) else {
            return;
        };
        match rt.block_on(outside_mcp::load_all(&pool)) {
            Ok(list) => {
                self.ai_mcp.servers = list
                    .into_iter()
                    .map(|mut s| {
                        s.env = outside_mcp::unseal_env(&s.secret_ns, &s.env);
                        s
                    })
                    .collect();
            }
            Err(e) => log::warn!("[AI] failed to load MCP servers: {e}"),
        }
    }

    /// Simpan satu server (env di-seal ke secret store). Mengembalikan id.
    fn ai_mcp_persist(&mut self, server: &OutsideMcpServer) -> Result<i64, String> {
        let (Some(pool), Some(rt)) = (self.db_pool.clone(), self.runtime.clone()) else {
            return Err("Local database is not available.".to_string());
        };
        let mut sealed = server.clone();
        sealed.env = outside_mcp::seal_env(&server.secret_ns, &server.env);
        rt.block_on(outside_mcp::save(&pool, &sealed))
            .map_err(|e| format!("Could not save the MCP server: {e}"))
    }

    fn ai_mcp_save_draft(&mut self) {
        let Some(draft) = self.ai_mcp.draft.clone() else {
            return;
        };
        let name = draft.name.trim().to_string();
        let err = if let Err(e) = crate::ai_tool_calling::validate_server_name(&name) {
            Some(e)
        } else if draft.command.trim().is_empty() {
            Some("Command is required.".to_string())
        } else if self
            .ai_mcp
            .servers
            .iter()
            .any(|s| s.id != draft.id && s.name.eq_ignore_ascii_case(&name))
        {
            Some(format!("A server named '{name}' already exists."))
        } else {
            None
        };
        if let Some(e) = err {
            if let Some(d) = self.ai_mcp.draft.as_mut() {
                d.error = Some(e);
            }
            return;
        }
        let previous = self
            .ai_mcp
            .servers
            .iter()
            .find(|s| s.id == draft.id)
            .cloned();
        let env: Vec<EnvVar> = draft
            .env
            .iter()
            .filter(|e| !e.key.trim().is_empty())
            .map(|e| EnvVar {
                key: e.key.trim().to_string(),
                value: e.value.clone(),
            })
            .collect();
        let server = OutsideMcpServer {
            id: draft.id,
            name,
            command: draft.command.trim().to_string(),
            args: crate::agent::harness::split_args(&draft.args_text),
            env,
            enabled: draft.enabled,
            tools: previous
                .as_ref()
                .map(|p| p.tools.clone())
                .unwrap_or_default(),
            secret_ns: previous
                .as_ref()
                .map(|p| p.secret_ns.clone())
                .filter(|ns| !ns.is_empty())
                .unwrap_or_else(outside_mcp::new_secret_ns),
        };
        // Secret env yang dibuang atau tidak lagi rahasia dihapus dari store.
        if let Some(prev) = &previous {
            let removed: Vec<String> = prev
                .env
                .iter()
                .filter(|e| !server.env.iter().any(|n| n.key == e.key))
                .map(|e| e.key.clone())
                .collect();
            outside_mcp::delete_env_secrets(&prev.secret_ns, &removed);
        }
        match self.ai_mcp_persist(&server) {
            Ok(id) => {
                log::info!("[AI] MCP server '{}' saved", server.name);
                self.ai_mcp.draft = None;
                self.ai_mcp_reload();
                self.ai_mcp.messages.insert(
                    id,
                    (
                        true,
                        "Saved. Press \"Test / List tools\" to choose which tools the AI may use."
                            .to_string(),
                    ),
                );
            }
            Err(e) => {
                if let Some(d) = self.ai_mcp.draft.as_mut() {
                    d.error = Some(e);
                }
            }
        }
    }

    fn ai_mcp_delete(&mut self, id: i64) {
        let (Some(pool), Some(rt)) = (self.db_pool.clone(), self.runtime.clone()) else {
            return;
        };
        if let Some(s) = self.ai_mcp.servers.iter().find(|s| s.id == id) {
            let keys: Vec<String> = s.env.iter().map(|e| e.key.clone()).collect();
            outside_mcp::delete_env_secrets(&s.secret_ns, &keys);
        }
        if let Err(e) = rt.block_on(outside_mcp::delete(&pool, id)) {
            log::warn!("[AI] failed to delete MCP server: {e}");
        }
        self.ai_mcp.messages.remove(&id);
        self.ai_mcp_reload();
    }

    /// Simpan perubahan allowlist / enabled tanpa lewat form.
    fn ai_mcp_update(&mut self, server: OutsideMcpServer) {
        if let Err(e) = self.ai_mcp_persist(&server) {
            self.ai_mcp.messages.insert(server.id, (false, e));
        }
        if let Some(s) = self.ai_mcp.servers.iter_mut().find(|s| s.id == server.id) {
            *s = server;
        }
    }

    #[cfg(not(target_os = "ios"))]
    fn ai_mcp_start_list(&mut self, id: i64) {
        let Some(server) = self.ai_mcp.servers.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        let Some(rt) = self.runtime.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let handle = rt.handle().clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::outside_mcp_client::list_tools_blocking(
                &handle, &server,
            ));
        });
        self.ai_mcp.messages.remove(&id);
        self.ai_mcp.list_rx = Some((id, rx));
    }

    fn ai_mcp_poll_list(&mut self, ctx: &egui::Context) {
        let Some((id, rx)) = self.ai_mcp.list_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(tools)) => {
                let count = tools.len();
                if let Some(mut s) = self.ai_mcp.servers.iter().find(|s| s.id == id).cloned() {
                    s.tools = outside_mcp::merge_discovered(&s.tools, tools);
                    self.ai_mcp_update(s);
                }
                self.ai_mcp.messages.insert(
                    id,
                    (
                        true,
                        format!("Connected. {count} tool(s) found. New tools start disallowed."),
                    ),
                );
            }
            Ok(Err(e)) => {
                log::warn!("[AI] MCP list tools failed: {e}");
                self.ai_mcp.messages.insert(id, (false, e));
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.ai_mcp.list_rx = Some((id, rx));
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.ai_mcp
                    .messages
                    .insert(id, (false, "The test stopped unexpectedly.".to_string()));
            }
        }
    }

    /// Bagian "MCP Servers" di Settings → AI Assistant.
    pub(crate) fn render_ai_mcp_servers_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "MCP Servers", |ui| {
            hint(
                ui,
                "Let the AI Assistant call tools from other MCP servers when it uses an HTTP API provider. \
                 Servers run locally as child processes (stdio). Only tools you allow are offered to the model.",
            );
            if cfg!(target_os = "ios") {
                status(ui, Tone::Muted, "Not available on iOS.");
                return;
            }
            self.ai_mcp_ensure_loaded();
            self.ai_mcp_poll_list(ui.ctx());

            let servers = self.ai_mcp.servers.clone();
            let mut to_update: Option<OutsideMcpServer> = None;
            let mut to_list: Option<i64> = None;
            let mut to_edit: Option<ServerDraft> = None;
            let mut to_delete: Option<i64> = None;

            for s in &servers {
                divider(ui);
                let listing = self
                    .ai_mcp
                    .list_rx
                    .as_ref()
                    .is_some_and(|(id, _)| *id == s.id);
                ui.horizontal(|ui| {
                    let mut enabled = s.enabled;
                    if toggle(ui, &mut enabled)
                        .on_hover_text("Offer this server's allowed tools to the AI")
                        .changed()
                    {
                        let mut n = s.clone();
                        n.enabled = enabled;
                        to_update = Some(n);
                    }
                    ui.label(egui::RichText::new(&s.name).strong().size(13.0));
                    let cmdline = std::iter::once(s.command.as_str())
                        .chain(s.args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" ");
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(cmdline)
                                .monospace()
                                .size(11.0)
                                .color(super::style::theme_muted_text(ui.ctx())),
                        )
                        .truncate(),
                    );
                });
                ui.horizontal(|ui| {
                    if listing {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(egui::RichText::new("Connecting…").size(11.5));
                    } else if ui
                        .button("Test / List tools")
                        .on_hover_text("Start the server, read its tool list, then stop it")
                        .clicked()
                    {
                        to_list = Some(s.id);
                    }
                    if ui.button("Edit").clicked() {
                        to_edit = Some(ServerDraft {
                            id: s.id,
                            name: s.name.clone(),
                            command: s.command.clone(),
                            args_text: s
                                .args
                                .iter()
                                .map(|a| {
                                    if a.contains(char::is_whitespace) || a.is_empty() {
                                        format!("\"{}\"", a.replace('"', "\\\""))
                                    } else {
                                        a.clone()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(" "),
                            env: s.env.clone(),
                            enabled: s.enabled,
                            error: None,
                        });
                    }
                    if self.ai_mcp.confirm_delete == Some(s.id) {
                        if ui
                            .button(
                                egui::RichText::new("Confirm delete")
                                    .color(super::style::theme_danger(ui.ctx())),
                            )
                            .clicked()
                        {
                            to_delete = Some(s.id);
                        }
                        if ui.button("Cancel").clicked() {
                            self.ai_mcp.confirm_delete = None;
                        }
                    } else if ui.button("Delete").clicked() {
                        self.ai_mcp.confirm_delete = Some(s.id);
                    }
                });
                if let Some((ok, msg)) = self.ai_mcp.messages.get(&s.id) {
                    status(
                        ui,
                        if *ok { Tone::Success } else { Tone::Danger },
                        msg.clone(),
                    );
                }
                if s.tools.is_empty() {
                    hint(ui, "No tools listed yet.");
                } else {
                    let allowed = s.tools.iter().filter(|t| t.allowed).count();
                    egui::CollapsingHeader::new(format!(
                        "Tools ({allowed} of {} allowed)",
                        s.tools.len()
                    ))
                    .id_salt(("ai_mcp_tools", s.id))
                    .default_open(allowed == 0)
                    .show(ui, |ui| {
                        egui::Grid::new(("ai_mcp_tool_grid", s.id))
                            .num_columns(3)
                            .spacing([10.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("Allow").size(11.0).strong());
                                ui.label(egui::RichText::new("Ask first").size(11.0).strong());
                                ui.label(egui::RichText::new("Tool").size(11.0).strong());
                                ui.end_row();
                                for (ti, t) in s.tools.iter().enumerate() {
                                    let mut allow = t.allowed;
                                    let mut confirm = t.confirm;
                                    let a = ui.checkbox(&mut allow, "");
                                    let c = ui
                                        .add_enabled(allow, egui::Checkbox::without_text(&mut confirm))
                                        .on_hover_text(
                                            "Show an Approve / Deny card in the chat before running",
                                        );
                                    let label = ui.label(
                                        egui::RichText::new(&t.name).monospace().size(11.5),
                                    );
                                    if !t.description.is_empty() {
                                        label.on_hover_text(&t.description);
                                    }
                                    ui.end_row();
                                    if a.changed() || c.changed() {
                                        let mut n = to_update.take().unwrap_or_else(|| s.clone());
                                        if let Some(tp) = n.tools.get_mut(ti) {
                                            tp.allowed = allow;
                                            tp.confirm = confirm;
                                        }
                                        to_update = Some(n);
                                    }
                                }
                            });
                    });
                }
            }

            if let Some(n) = to_update {
                self.ai_mcp_update(n);
            }
            #[cfg(not(target_os = "ios"))]
            if let Some(id) = to_list {
                self.ai_mcp_start_list(id);
            }
            #[cfg(target_os = "ios")]
            let _ = to_list;
            if let Some(id) = to_delete {
                self.ai_mcp.confirm_delete = None;
                self.ai_mcp_delete(id);
            }
            if let Some(d) = to_edit {
                self.ai_mcp.draft = Some(d);
            }

            divider(ui);
            if self.ai_mcp.draft.is_some() {
                self.render_ai_mcp_draft(ui);
            } else if ui.button("+ Add MCP server").clicked() {
                self.ai_mcp.draft = Some(ServerDraft {
                    enabled: true,
                    ..Default::default()
                });
            }
            ui.add_space(4.0);
            hint(
                ui,
                "Example: name \"github\", command \"npx\", arguments \"-y @modelcontextprotocol/server-github\", \
                 env GITHUB_PERSONAL_ACCESS_TOKEN. Tabular's own server can be added too (command: path to \
                 tabular, arguments: mcp). Env values whose name looks secret (TOKEN, KEY, SECRET, PASSWORD) \
                 are kept in Tabular's encrypted secret store. Only stdio servers are supported.",
            );
        });
    }

    fn render_ai_mcp_draft(&mut self, ui: &mut egui::Ui) {
        let mut save = false;
        let mut cancel = false;
        let Some(d) = self.ai_mcp.draft.as_mut() else {
            return;
        };
        callout(ui, Tone::Info, |ui| {
            ui.label(
                egui::RichText::new(if d.id == 0 {
                    "Add MCP server"
                } else {
                    "Edit MCP server"
                })
                .strong(),
            );
            egui::Grid::new("ai_mcp_draft_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut d.name)
                            .hint_text("github")
                            .desired_width(220.0),
                    );
                    ui.end_row();
                    ui.label("Command");
                    ui.add(
                        egui::TextEdit::singleline(&mut d.command)
                            .hint_text("npx")
                            .desired_width(320.0),
                    );
                    ui.end_row();
                    ui.label("Arguments");
                    ui.add(
                        egui::TextEdit::singleline(&mut d.args_text)
                            .hint_text("-y @modelcontextprotocol/server-github")
                            .desired_width(320.0),
                    );
                    ui.end_row();
                    ui.label("Enabled");
                    toggle(ui, &mut d.enabled);
                    ui.end_row();
                });
            ui.label(egui::RichText::new("Environment variables").size(12.0));
            let mut remove: Option<usize> = None;
            for (i, e) in d.env.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut e.key)
                            .hint_text("NAME")
                            .desired_width(200.0),
                    );
                    let secret = outside_mcp::env_key_looks_secret(&e.key);
                    ui.add(
                        egui::TextEdit::singleline(&mut e.value)
                            .hint_text("value")
                            .password(secret)
                            .desired_width(220.0),
                    );
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                d.env.remove(i);
            }
            if ui.small_button("+ Add variable").clicked() {
                d.env.push(EnvVar::default());
            }
            if let Some(err) = &d.error {
                status(ui, Tone::Danger, err.clone());
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.ai_mcp.draft = None;
        } else if save {
            self.ai_mcp_save_draft();
        }
    }
}

// ─── Giliran chat ───────────────────────────────────────────────────────────

/// Mulai satu giliran chat. Untuk backend HTTP API dengan minimal satu tool
/// MCP luar yang diizinkan, pakai loop tool calling; selain itu jalur biasa
/// `ai_assistant::start_chat` (tidak berubah).
pub(crate) fn start_chat_turn(
    tabular: &mut Tabular,
    cfg: &crate::ai_assistant::ChatBackend,
    system: String,
    user: String,
    session_id: Option<String>,
) -> Result<
    (
        mpsc::Receiver<crate::agent::harness::AgentEvent>,
        Option<crate::agent::harness::CancelHandle>,
    ),
    String,
> {
    #[cfg(not(target_os = "ios"))]
    {
        tabular.ai_mcp.turn = None;
        if cfg.backend == crate::config::AiBackend::Api {
            tabular.ai_mcp_ensure_loaded();
            let bindings = outside_mcp::build_bindings(&tabular.ai_mcp.servers);
            if !bindings.is_empty()
                && let Some(rt) = tabular.runtime.clone()
            {
                let servers: Vec<OutsideMcpServer> = tabular
                    .ai_mcp
                    .servers
                    .iter()
                    .filter(|s| s.enabled)
                    .cloned()
                    .collect();
                log::info!("[AI] chat turn with {} MCP tool(s)", bindings.len());
                let (rx, handle) = crate::ai_tool_chat::start_tool_chat(
                    cfg,
                    system,
                    user,
                    servers,
                    bindings,
                    rt.handle().clone(),
                );
                tabular.ai_mcp.turn = Some(handle);
                return Ok((rx, None));
            }
        }
    }
    crate::ai_assistant::start_chat(cfg, system, user, session_id)
}

/// Ambil pembaruan kartu tool dari giliran yang berjalan.
pub(crate) fn poll_tool_updates(tabular: &mut Tabular) {
    #[cfg(not(target_os = "ios"))]
    {
        let Some(turn) = tabular.ai_mcp.turn.as_ref() else {
            return;
        };
        let updates: Vec<ToolCallRecord> = turn.updates.try_iter().collect();
        if updates.is_empty() {
            return;
        }
        if let Some(msg) = tabular.ai_chat.last_mut() {
            for rec in updates {
                match msg.tool_calls.iter_mut().find(|r| r.call_id == rec.call_id) {
                    Some(existing) => *existing = rec,
                    None => msg.tool_calls.push(rec),
                }
            }
        }
    }
    #[cfg(target_os = "ios")]
    let _ = tabular;
}

/// Akhiri giliran bertool (selesai atau dihentikan user).
pub(crate) fn end_tool_turn(tabular: &mut Tabular) {
    #[cfg(not(target_os = "ios"))]
    if let Some(turn) = tabular.ai_mcp.turn.take() {
        turn.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // Kartu yang masih menunggu keputusan tidak bisa dijawab lagi.
        if let Some(msg) = tabular.ai_chat.last_mut() {
            for r in &mut msg.tool_calls {
                if matches!(
                    r.status,
                    ToolCallStatus::AwaitingApproval | ToolCallStatus::Running
                ) {
                    r.status = ToolCallStatus::Failed;
                    r.result_summary = "Stopped.".to_string();
                }
            }
        }
    }
    #[cfg(target_os = "ios")]
    let _ = tabular;
}

/// Kirim keputusan Approve / Deny untuk satu pemanggilan tool.
pub(crate) fn send_tool_decision(tabular: &mut Tabular, call_id: &str, approve: bool) {
    #[cfg(not(target_os = "ios"))]
    {
        let Some(turn) = tabular.ai_mcp.turn.as_ref() else {
            return;
        };
        if turn.approvals.send((call_id.to_string(), approve)).is_err() {
            return;
        }
        if let Some(r) = tabular
            .ai_chat
            .last_mut()
            .and_then(|m| m.tool_calls.iter_mut().find(|r| r.call_id == call_id))
        {
            r.status = if approve {
                ToolCallStatus::Running
            } else {
                ToolCallStatus::Denied
            };
        }
    }
    #[cfg(target_os = "ios")]
    let _ = (tabular, call_id, approve);
}

/// Kartu pemanggilan tool di bawah header jawaban assistant. Keputusan user
/// dikembalikan lewat `decisions` sebagai `(call_id, approve)`.
pub(crate) fn render_tool_calls(
    ui: &mut egui::Ui,
    mi: usize,
    calls: &[ToolCallRecord],
    decisions: &mut Vec<(String, bool)>,
) {
    use super::style;
    use egui_icons::icons;

    for (ci, rec) in calls.iter().enumerate() {
        let ctx = ui.ctx().clone();
        let (icon, color, label) = match rec.status {
            ToolCallStatus::AwaitingApproval => (
                icons::ICON_HELP_OUTLINE.codepoint,
                style::theme_warning(&ctx),
                "Needs approval",
            ),
            ToolCallStatus::Running => (
                icons::ICON_BUILD.codepoint,
                style::theme_info(&ctx),
                "Running",
            ),
            ToolCallStatus::Done => (
                icons::ICON_CHECK.codepoint,
                style::theme_success(&ctx),
                "Done",
            ),
            ToolCallStatus::Failed => (
                icons::ICON_ERROR_OUTLINE.codepoint,
                style::theme_danger(&ctx),
                "Failed",
            ),
            ToolCallStatus::Denied => (
                icons::ICON_BLOCK.codepoint,
                style::theme_muted_text(&ctx),
                "Declined",
            ),
        };
        style::ai_notice_frame(color).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(egui::RichText::new(icon).size(12.0).color(color));
                ui.label(
                    egui::RichText::new(&rec.display_name)
                        .monospace()
                        .size(11.5)
                        .strong(),
                );
                if rec.status == ToolCallStatus::Running {
                    ui.add(egui::Spinner::new().size(10.0));
                }
                ui.label(egui::RichText::new(label).size(10.5).color(color));
            });
            egui::CollapsingHeader::new(egui::RichText::new("Arguments").size(10.5))
                .id_salt(("ai_tool_args", mi, ci))
                .default_open(rec.status == ToolCallStatus::AwaitingApproval)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&rec.arguments).monospace().size(10.5),
                        )
                        .wrap(),
                    );
                });
            if !rec.result_summary.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&rec.result_summary)
                            .size(10.5)
                            .color(style::theme_muted_text(&ctx)),
                    )
                    .wrap(),
                );
            }
            if rec.status == ToolCallStatus::AwaitingApproval {
                ui.horizontal(|ui| {
                    if ui
                        .button(egui::RichText::new("Approve").size(11.0))
                        .on_hover_text("Run this tool once")
                        .clicked()
                    {
                        decisions.push((rec.call_id.clone(), true));
                    }
                    if ui
                        .button(egui::RichText::new("Deny").size(11.0))
                        .on_hover_text("Tell the model you declined")
                        .clicked()
                    {
                        decisions.push((rec.call_id.clone(), false));
                    }
                });
            }
        });
        ui.add_space(3.0);
    }
}
