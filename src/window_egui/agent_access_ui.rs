//! UI akses agent MCP (checklist K1, K2 log aktivitas, K4):
//!
//! - Jendela "Agent Access": level akses per koneksi, allowlist klien MCP,
//!   daftar klien yang pernah terhubung (Forget), dan log aktivitas.
//! - Dialog persetujuan: permintaan `execute_statement` dari proses
//!   `tabular mcp` yang butuh izin user muncul di sini (antrean di
//!   `connections.db`, lihat `crate::agent::access`).
//!
//! Penyimpanan dan aturan keputusan ada di `crate::agent::access` (headless);
//! modul ini hanya state UI dan gambar.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

use super::Tabular;
use crate::agent::access::{
    self, AccessLevel, ActivityRow, ApprovalRequest, ApprovalStatus, ConnectionAccess, KnownClient,
};

/// Interval pemantau latar memeriksa antrean persetujuan.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// Jumlah baris log aktivitas yang dimuat.
const ACTIVITY_LIMIT: i64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentAccessTab {
    #[default]
    Connections,
    Clients,
    Activity,
}

#[derive(Default)]
pub struct AgentAccessState {
    pub open: bool,
    pub tab: AgentAccessTab,
    pub access: HashMap<i64, ConnectionAccess>,
    pub clients: Vec<KnownClient>,
    pub activity: Vec<ActivityRow>,
    pub activity_filter: String,
    pub error: Option<String>,
    /// Permintaan persetujuan yang sedang ditampilkan.
    pub pending: Vec<ApprovalRequest>,
    /// Diset oleh pemantau latar bila ada permintaan menunggu.
    wake: Arc<AtomicBool>,
    watcher_started: bool,
    last_check: Option<Instant>,
    /// Id permintaan yang sudah memicu notifikasi.
    notified: Vec<i64>,
    confirm_forget: Option<String>,
    confirm_clear: bool,
}

impl Tabular {
    pub(crate) fn open_agent_access(&mut self) {
        self.agent_access.open = true;
        self.reload_agent_access();
    }

    fn reload_agent_access(&mut self) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        let loaded = rt.block_on(async {
            let access = access::load_all(&pool).await?;
            let clients = access::list_clients(&pool).await?;
            let activity = access::recent_activity(&pool, ACTIVITY_LIMIT).await?;
            Ok::<_, sqlx::Error>((access, clients, activity))
        });
        match loaded {
            Ok((a, c, act)) => {
                self.agent_access.access = a;
                self.agent_access.clients = c;
                self.agent_access.activity = act;
                self.agent_access.error = None;
            }
            Err(e) => {
                log::warn!("[AGENT] cannot load agent access settings: {e}");
                self.agent_access.error = Some(format!("Cannot load agent settings: {e}"));
            }
        }
    }

    fn save_connection_access(&mut self, conn_id: i64, acc: ConnectionAccess) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        match rt.block_on(access::save(&pool, conn_id, &acc)) {
            Ok(()) => {
                if acc == ConnectionAccess::default() {
                    self.agent_access.access.remove(&conn_id);
                } else {
                    self.agent_access.access.insert(conn_id, acc);
                }
            }
            Err(e) => {
                log::warn!("[AGENT] cannot save agent access for connection {conn_id}: {e}");
                self.toasts.error(format!("Cannot save agent access: {e}"));
            }
        }
    }

    /// Pemantau latar: cek antrean tiap detik tanpa membuat UI repaint terus;
    /// UI dibangunkan hanya bila ada permintaan.
    fn start_approval_watcher(&mut self, ctx: &egui::Context) {
        if self.agent_access.watcher_started {
            return;
        }
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        self.agent_access.watcher_started = true;
        let wake = self.agent_access.wake.clone();
        let ctx = ctx.clone();
        let rt = self.get_runtime();
        rt.spawn(async move {
            loop {
                match access::pending_count(&pool).await {
                    Ok(n) if n > 0 => {
                        wake.store(true, Ordering::SeqCst);
                        ctx.request_repaint();
                    }
                    Ok(_) => {}
                    Err(e) => log::debug!("[AGENT] approval watcher: {e}"),
                }
                tokio::time::sleep(WATCH_EVERY).await;
            }
        });
    }

    /// Dipanggil tiap frame: ambil permintaan baru dan gambar jendela.
    pub(crate) fn render_agent_access(&mut self, ctx: &egui::Context) {
        self.start_approval_watcher(ctx);
        let woke = self.agent_access.wake.swap(false, Ordering::SeqCst);
        let due = self
            .agent_access
            .last_check
            .is_none_or(|t| t.elapsed() >= WATCH_EVERY);
        if (woke || !self.agent_access.pending.is_empty()) && due {
            self.refresh_pending_approvals(ctx);
        }
        if !self.agent_access.pending.is_empty() {
            // Permintaan bisa kedaluwarsa di sisi MCP; tetap diperiksa.
            ctx.request_repaint_after(WATCH_EVERY);
            self.render_approval_dialog(ctx);
        }
        if self.agent_access.open {
            self.render_agent_access_window(ctx);
        }
    }

    fn refresh_pending_approvals(&mut self, ctx: &egui::Context) {
        self.agent_access.last_check = Some(Instant::now());
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        match rt.block_on(access::pending_approvals(&pool)) {
            Ok(list) => {
                let focused = ctx.input(|i| i.focused);
                for req in &list {
                    if !self.agent_access.notified.contains(&req.id) {
                        self.agent_access.notified.push(req.id);
                        log::info!(
                            "[AGENT] approval #{} requested by {} on {}",
                            req.id,
                            req.client,
                            req.connection_name
                        );
                        if !focused {
                            crate::os_notify::send(
                                "Agent approval needed",
                                &format!(
                                    "{} wants to run a {} statement on {}",
                                    req.client, req.kind, req.connection_name
                                ),
                            );
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Informational,
                        ));
                    }
                }
                self.agent_access
                    .notified
                    .retain(|id| list.iter().any(|r| r.id == *id));
                self.agent_access.pending = list;
            }
            Err(e) => log::warn!("[AGENT] cannot read approval queue: {e}"),
        }
    }

    fn decide_pending(&mut self, id: i64, status: ApprovalStatus) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        match rt.block_on(access::decide_approval(&pool, id, status)) {
            Ok(true) => {}
            Ok(false) => self
                .toasts
                .info("This request already expired; the agent was told it was not approved."),
            Err(e) => self.toasts.error(format!("Cannot record decision: {e}")),
        }
        self.agent_access.pending.retain(|r| r.id != id);
    }

    fn render_approval_dialog(&mut self, ctx: &egui::Context) {
        let Some(req) = self.agent_access.pending.first().cloned() else {
            return;
        };
        let more = self.agent_access.pending.len() - 1;
        let production = self.connection_environment_by_id(req.connection_id)
            == Some(crate::connection_env::Environment::Production);
        let danger = super::style::theme_danger(ctx);
        let warning = super::style::theme_warning(ctx);
        let mut decision: Option<ApprovalStatus> = None;

        egui::Window::new("Agent wants to run SQL")
            .id(egui::Id::new("agent_approval_dialog"))
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "\"{}\" asks to run a {} statement.",
                        req.client, req.kind
                    ))
                    .strong(),
                );
                ui.add_space(4.0);
                egui::Grid::new("agent_approval_meta")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Connection").weak());
                        ui.horizontal(|ui| {
                            ui.label(&req.connection_name);
                            if production {
                                ui.label(egui::RichText::new("PRODUCTION").color(danger).strong());
                            }
                        });
                        ui.end_row();
                        if !req.database.is_empty() {
                            ui.label(egui::RichText::new("Database").weak());
                            ui.label(&req.database);
                            ui.end_row();
                        }
                        ui.label(egui::RichText::new("Why approval").weak());
                        ui.label(egui::RichText::new(&req.reason).color(warning));
                        ui.end_row();
                        ui.label(egui::RichText::new("Requested").weak());
                        ui.label(format!("{} UTC", req.created_at));
                        ui.end_row();
                    });
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("agent_approval_sql")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        let mut text = req.statement.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "The statement runs on a separate connection opened by the agent process, \
                         not in your editor's transaction. It is recorded in Query History as (agent).",
                    )
                    .weak()
                    .size(11.0),
                );
                if more > 0 {
                    ui.label(
                        egui::RichText::new(format!("{more} more request(s) waiting"))
                            .weak()
                            .size(11.0),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add(super::style::btn_danger_ctx(ui.ctx(), "Approve & Run"))
                        .clicked()
                    {
                        decision = Some(ApprovalStatus::Approved);
                    }
                    if ui.add(super::style::btn_secondary("Deny")).clicked() {
                        decision = Some(ApprovalStatus::Denied);
                    }
                    if ui.add(super::style::btn_secondary("Copy SQL")).clicked() {
                        ui.ctx().copy_text(req.statement.clone());
                    }
                });
            });

        if let Some(status) = decision {
            log::info!("[AGENT] approval #{} -> {}", req.id, status.key());
            self.decide_pending(req.id, status);
        }
    }

    fn render_agent_access_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut reload = false;
        let mut save: Option<(i64, ConnectionAccess)> = None;
        let mut forget: Option<String> = None;
        let mut clear_log = false;

        let conns: Vec<(i64, String, String)> = self
            .connections
            .iter()
            .filter_map(|c| {
                c.id.map(|id| {
                    (
                        id,
                        c.name.clone(),
                        crate::ai_query_fix::engine_label(&c.connection_type).to_string(),
                    )
                })
            })
            .collect();
        let conn_names: HashMap<i64, String> =
            conns.iter().map(|(id, n, _)| (*id, n.clone())).collect();
        let client_names: Vec<String> = self
            .agent_access
            .clients
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let state = &mut self.agent_access;

        egui::Window::new("Agent Access")
            .id(egui::Id::new("agent_access_window"))
            .open(&mut open)
            .default_width(760.0)
            .default_height(480.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Controls what AI agents connected through `tabular mcp` (Claude Code, Cursor, \
                         Codex, ...) may do. Settings are stored on this machine only.",
                    )
                    .weak()
                    .size(11.5),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for tab in [
                        AgentAccessTab::Connections,
                        AgentAccessTab::Clients,
                        AgentAccessTab::Activity,
                    ] {
                        let label = match tab {
                            AgentAccessTab::Connections => "Connections",
                            AgentAccessTab::Clients => "Clients",
                            AgentAccessTab::Activity => "Activity Log",
                        };
                        ui.selectable_value(&mut state.tab, tab, label);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Refresh").clicked() {
                            reload = true;
                        }
                    });
                });
                if let Some(err) = &state.error {
                    ui.colored_label(super::style::theme_danger(ui.ctx()), err);
                }
                ui.separator();

                match state.tab {
                    AgentAccessTab::Connections => {
                        render_connections_tab(ui, &conns, &state.access, &client_names, &mut save)
                    }
                    AgentAccessTab::Clients => render_clients_tab(
                        ui,
                        &state.clients,
                        &mut state.confirm_forget,
                        &mut forget,
                    ),
                    AgentAccessTab::Activity => render_activity_tab(
                        ui,
                        &state.activity,
                        &conn_names,
                        &mut state.activity_filter,
                        &mut state.confirm_clear,
                        &mut clear_log,
                    ),
                }
            });

        if !open {
            self.agent_access.open = false;
        }
        if let Some((id, acc)) = save {
            self.save_connection_access(id, acc);
        }
        if let Some(name) = forget {
            if let Some(pool) = self.db_pool.clone() {
                let rt = self.get_runtime();
                match rt.block_on(access::forget_client(&pool, &name)) {
                    Ok(()) => self
                        .toasts
                        .success(format!("Forgot MCP client \"{name}\".")),
                    Err(e) => self.toasts.error(format!("Cannot forget client: {e}")),
                }
            }
            reload = true;
        }
        if clear_log {
            if let Some(pool) = self.db_pool.clone() {
                let rt = self.get_runtime();
                if let Err(e) = rt.block_on(access::clear_activity(&pool)) {
                    self.toasts
                        .error(format!("Cannot clear the activity log: {e}"));
                }
            }
            reload = true;
        }
        if reload {
            self.reload_agent_access();
        }
    }
}

fn level_combo(ui: &mut egui::Ui, id: i64, level: &mut AccessLevel) -> bool {
    let before = *level;
    egui::ComboBox::from_id_salt(("agent_access_level", id))
        .selected_text(level.label())
        .width(110.0)
        .show_ui(ui, |ui| {
            for l in AccessLevel::ALL {
                ui.selectable_value(level, l, l.label())
                    .on_hover_text(l.description());
            }
        });
    *level != before
}

fn render_connections_tab(
    ui: &mut egui::Ui,
    conns: &[(i64, String, String)],
    access_map: &HashMap<i64, ConnectionAccess>,
    known_clients: &[String],
    save: &mut Option<(i64, ConnectionAccess)>,
) {
    ui.label(
        egui::RichText::new(
            "Read only is the default. Ask, Edit and Agent let the agent use execute_statement; \
             UPDATE/DELETE without WHERE, DROP/TRUNCATE, admin commands and any write on a \
             Production connection always wait for your approval here.",
        )
        .weak()
        .size(11.0),
    );
    ui.add_space(4.0);
    if conns.is_empty() {
        ui.label("No saved connections.");
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("agent_access_conns")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("agent_access_grid")
                .num_columns(4)
                .striped(true)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Connection").strong());
                    ui.label(egui::RichText::new("Engine").strong());
                    ui.label(egui::RichText::new("Access").strong());
                    ui.label(egui::RichText::new("MCP clients").strong());
                    ui.end_row();
                    for (id, name, engine) in conns {
                        let mut acc = access_map.get(id).cloned().unwrap_or_default();
                        let mut changed = false;
                        ui.label(name);
                        ui.label(egui::RichText::new(engine).weak());
                        changed |= level_combo(ui, *id, &mut acc.level);
                        ui.horizontal_wrapped(|ui| {
                            let mut all = acc.allowed_clients.is_none();
                            if ui
                                .checkbox(&mut all, "All")
                                .on_hover_text(
                                    "Any MCP client may use this connection. Untick to pick clients.",
                                )
                                .changed()
                            {
                                acc.allowed_clients = if all { None } else { Some(Vec::new()) };
                                changed = true;
                            }
                            if let Some(list) = acc.allowed_clients.as_mut() {
                                if known_clients.is_empty() {
                                    ui.label(
                                        egui::RichText::new("no clients seen yet — none allowed")
                                            .weak()
                                            .size(11.0),
                                    );
                                }
                                for client in known_clients {
                                    let mut on =
                                        list.iter().any(|c| c.eq_ignore_ascii_case(client));
                                    if ui.checkbox(&mut on, client).changed() {
                                        list.retain(|c| !c.eq_ignore_ascii_case(client));
                                        if on {
                                            list.push(client.clone());
                                        }
                                        changed = true;
                                    }
                                }
                            }
                        });
                        ui.end_row();
                        if changed {
                            *save = Some((*id, acc));
                        }
                    }
                });
        });
}

fn render_clients_tab(
    ui: &mut egui::Ui,
    clients: &[KnownClient],
    confirm_forget: &mut Option<String>,
    forget: &mut Option<String>,
) {
    ui.label(
        egui::RichText::new(
            "Clients are recorded by the name they report when they connect. The name is not \
             verified, so treat client allowlists as a convenience, not a security boundary. \
             Forget removes a client from this list and from every connection's allowlist.",
        )
        .weak()
        .size(11.0),
    );
    ui.add_space(4.0);
    if clients.is_empty() {
        ui.label(
            "No MCP client has connected yet. Register Tabular with `tabular mcp --print-config`.",
        );
        return;
    }
    egui::Grid::new("agent_clients_grid")
        .num_columns(6)
        .striped(true)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for h in [
                "Client",
                "Version",
                "First seen (UTC)",
                "Last seen (UTC)",
                "Sessions",
            ] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.label("");
            ui.end_row();
            for c in clients {
                ui.label(&c.name);
                ui.label(egui::RichText::new(&c.version).weak());
                ui.label(&c.first_seen);
                ui.label(&c.last_seen);
                ui.label(c.sessions.to_string());
                if confirm_forget.as_deref() == Some(c.name.as_str()) {
                    ui.horizontal(|ui| {
                        if ui
                            .add(super::style::btn_danger_ctx(ui.ctx(), "Forget"))
                            .clicked()
                        {
                            *forget = Some(c.name.clone());
                            *confirm_forget = None;
                        }
                        if ui.button("Cancel").clicked() {
                            *confirm_forget = None;
                        }
                    });
                } else if ui.button("Forget…").clicked() {
                    *confirm_forget = Some(c.name.clone());
                }
                ui.end_row();
            }
        });
}

fn render_activity_tab(
    ui: &mut egui::Ui,
    rows: &[ActivityRow],
    conn_names: &HashMap<i64, String>,
    filter: &mut String,
    confirm_clear: &mut bool,
    clear_log: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(
            egui::TextEdit::singleline(filter)
                .desired_width(220.0)
                .hint_text("client, tool, outcome…"),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if *confirm_clear {
                if ui
                    .add(super::style::btn_danger_ctx(ui.ctx(), "Clear log"))
                    .clicked()
                {
                    *clear_log = true;
                    *confirm_clear = false;
                }
                if ui.button("Cancel").clicked() {
                    *confirm_clear = false;
                }
            } else if ui.button("Clear…").clicked() {
                *confirm_clear = true;
            }
        });
    });
    ui.label(
        egui::RichText::new(
            "Last 500 calls, kept 90 days. Statements are stored as a SHA-256 digest; the full text \
             of queries that ran is in Query History as (agent).",
        )
        .weak()
        .size(11.0),
    );
    ui.add_space(4.0);
    let needle = filter.trim().to_ascii_lowercase();
    let ok = super::style::theme_success(ui.ctx());
    let bad = super::style::theme_danger(ui.ctx());
    let warn = super::style::theme_warning(ui.ctx());
    egui::ScrollArea::both()
        .id_salt("agent_activity_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("agent_activity_grid")
                .num_columns(7)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Time (UTC)",
                        "Client",
                        "Call",
                        "Connection",
                        "Kind",
                        "Outcome",
                        "ms",
                    ] {
                        ui.label(egui::RichText::new(h).strong());
                    }
                    ui.end_row();
                    for r in rows {
                        let conn = r
                            .connection_id
                            .map(|id| {
                                conn_names
                                    .get(&id)
                                    .cloned()
                                    .unwrap_or_else(|| format!("#{id}"))
                            })
                            .unwrap_or_default();
                        if !needle.is_empty() {
                            let hay = format!(
                                "{} {} {} {} {} {}",
                                r.client, r.category, r.name, conn, r.outcome, r.detail
                            )
                            .to_ascii_lowercase();
                            if !hay.contains(&needle) {
                                continue;
                            }
                        }
                        ui.label(egui::RichText::new(&r.at).size(11.5));
                        ui.label(&r.client);
                        let call = ui.label(format!("{}: {}", r.category, r.name));
                        if let Some(d) = &r.statement_digest {
                            call.on_hover_text(format!("SHA-256 {d}"));
                        }
                        ui.label(conn);
                        ui.label(r.statement_kind.clone().unwrap_or_default());
                        let color = match r.outcome.as_str() {
                            "ok" | "approved" => ok,
                            "refused" | "denied" => warn,
                            _ => bad,
                        };
                        let outcome = ui.label(egui::RichText::new(&r.outcome).color(color));
                        if !r.detail.is_empty() {
                            outcome.on_hover_text(&r.detail);
                        }
                        ui.label(r.duration_ms.to_string());
                        ui.end_row();
                    }
                });
        });
}
