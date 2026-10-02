//! Sisi GUI integrasi platform (M1, M5, M6, M8, M10): kotak masuk deep link,
//! dialog konfirmasi query dari link, Handoff, font bahasa, cache environment
//! koneksi, dan install update saat keluar.
//!
//! Semua state baru dikumpulkan di [`PlatformUiState`] supaya struct `Tabular`
//! cukup bertambah satu field.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::connection_env::Environment;
use crate::deeplink::{self, DeepLink};
use crate::editor;
use crate::i18n::{tr, trf};
use crate::quick_open::{QuickOpenItem, QuickOpenKind};

use super::Tabular;

/// Link yang datang sebelum daftar koneksi termuat ditunda paling lama ini.
const DEFER_WINDOW: Duration = Duration::from_secs(10);
/// Batas teks query yang ikut Handoff; lebih panjang cukup koneksinya saja.
#[cfg(target_os = "macos")]
const HANDOFF_MAX_SQL: usize = 2000;

#[derive(Debug, Clone)]
pub struct PendingLinkRun {
    pub tab_id: usize,
    pub connection_name: String,
    pub environment: Option<Environment>,
    pub sql: String,
}

#[derive(Default)]
pub struct PlatformUiState {
    started: bool,
    started_at: Option<Instant>,
    deferred: Vec<String>,
    pub pending_run: Option<PendingLinkRun>,
    /// Tanda environment eksplisit per id koneksi (tabel `connection_environment`).
    pub connection_envs: HashMap<i64, Environment>,
    last_slow_tick: Option<Instant>,
    known_signature: Vec<(i64, String)>,
    #[cfg(target_os = "macos")]
    handoff_receiver_installed: bool,
    /// Hasil Touch ID yang sedang ditunggu (vault unlock).
    pub touch_id_rx: Option<mpsc::Receiver<Result<(), String>>>,
    pub touch_id_available: Option<bool>,
    /// Cache "passphrase Touch ID tersimpan di keychain" agar keychain tidak
    /// dibaca tiap frame; `None` = belum dicek.
    pub touch_id_secret_saved: Option<bool>,
    /// Layout sempit aktif (M11) dan status panel sebelum disembunyikan.
    compact_layout: bool,
    panels_before_compact: Option<(bool, bool)>,
}

/// Di bawah lebar ini (pt) panel samping disembunyikan otomatis: iPad Slide
/// Over (~320), Split View 1/3 (~375–507) dan 1/2 (~678 di iPad 11").
pub const COMPACT_WIDTH: f32 = 700.0;

/// Transisi layout sempit: `Some(true)` = masuk, `Some(false)` = keluar.
pub fn compact_transition(was_compact: bool, width: f32) -> Option<bool> {
    let compact = width < COMPACT_WIDTH;
    (compact != was_compact).then_some(compact)
}

impl Tabular {
    /// Dipanggil sekali per frame dari `App::ui`.
    pub(crate) fn platform_tick(&mut self, ctx: &egui::Context) {
        if !self.platform_ui.started {
            self.platform_ui.started = true;
            self.platform_ui.started_at = Some(Instant::now());
            let waker_ctx = ctx.clone();
            deeplink::set_waker(move || waker_ctx.request_repaint());
            self.reload_connection_envs();
        }
        crate::i18n::install_fonts(ctx);

        let mut incoming = std::mem::take(&mut self.platform_ui.deferred);
        incoming.extend(deeplink::drain_incoming());
        if !incoming.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            for url in incoming {
                self.handle_deep_link(&url);
            }
            // Link yang ditunda perlu dicoba lagi walau tidak ada input baru.
            if !self.platform_ui.deferred.is_empty() {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        let slow_due = self
            .platform_ui
            .last_slow_tick
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
        if slow_due {
            self.platform_ui.last_slow_tick = Some(Instant::now());
            self.publish_known_connections();
            #[cfg(target_os = "macos")]
            self.update_handoff();
        }
    }

    fn within_defer_window(&self) -> bool {
        self.platform_ui
            .started_at
            .is_some_and(|t| t.elapsed() < DEFER_WINDOW)
    }

    fn handle_deep_link(&mut self, raw: &str) {
        let link = match deeplink::parse(raw) {
            Ok(l) => l,
            Err(e) => {
                self.toasts
                    .error(trf("Invalid link: {}", &[&e.to_string()]));
                return;
            }
        };
        match link {
            DeepLink::Open {
                connection,
                database,
                table,
            } => {
                let Some(conn) = self.resolve_deeplink_connection(raw, &connection) else {
                    return;
                };
                self.open_connection_from_link(conn, database, table);
            }
            DeepLink::Query {
                connection,
                sql,
                database,
                run,
            } => {
                let Some(conn) = self.resolve_deeplink_connection(raw, &connection) else {
                    return;
                };
                let Some(conn_id) = conn.id else {
                    return;
                };
                let database = database
                    .or_else(|| (!conn.database.trim().is_empty()).then(|| conn.database.clone()));
                let title = format!("{} — {}", tr("Opened from link"), conn.name);
                let tab_id = editor::create_new_tab_with_connection_and_database(
                    self,
                    title,
                    sql.clone(),
                    Some(conn_id),
                    database,
                );
                if run {
                    self.platform_ui.pending_run = Some(PendingLinkRun {
                        tab_id,
                        environment: self.connection_environment(conn_id, &conn.name),
                        connection_name: conn.name.clone(),
                        sql,
                    });
                }
            }
            DeepLink::Import { dsn, name } => {
                if self.connections.is_empty() && self.within_defer_window() {
                    self.platform_ui.deferred.push(raw.to_string());
                    return;
                }
                if let Some(id) = deeplink::find_matching_connection(&self.connections, &dsn)
                    && let Some(conn) = self.connections.iter().find(|c| c.id == Some(id)).cloned()
                {
                    self.open_connection_from_link(conn, None, None);
                    return;
                }
                self.new_connection = dsn.to_connection_config(name.as_deref());
                self.show_add_connection = true;
            }
        }
    }

    /// Cari koneksi; saat startup, tunda link bila daftar koneksi belum termuat.
    fn resolve_deeplink_connection(
        &mut self,
        raw: &str,
        key: &str,
    ) -> Option<crate::models::structs::ConnectionConfig> {
        if let Some(c) = deeplink::resolve_connection(&self.connections, key) {
            return Some(c.clone());
        }
        if self.within_defer_window() {
            self.platform_ui.deferred.push(raw.to_string());
        } else {
            self.toasts.error(trf("Connection not found: {}", &[key]));
        }
        None
    }

    fn open_connection_from_link(
        &mut self,
        conn: crate::models::structs::ConnectionConfig,
        database: Option<String>,
        table: Option<String>,
    ) {
        let Some(conn_id) = conn.id else {
            return;
        };
        let database =
            database.or_else(|| (!conn.database.trim().is_empty()).then(|| conn.database.clone()));
        let kind = if table.is_some() {
            QuickOpenKind::Table
        } else {
            QuickOpenKind::Connection
        };
        let item = QuickOpenItem::new(
            format!("deeplink:{conn_id}"),
            table.clone().unwrap_or_else(|| conn.name.clone()),
            String::new(),
            kind,
            Some(conn_id),
            Some(conn.name.clone()),
            database.clone(),
            table.clone(),
            None,
            None,
            None,
        );
        crate::quick_open::execute_quick_open_item(self, &item);
        if table.is_none() {
            editor::create_new_tab_with_connection_and_database(
                self,
                conn.name.clone(),
                String::new(),
                Some(conn_id),
                database,
            );
        }
    }

    /// Dialog konfirmasi untuk `tabular://query?...&run=1`.
    pub(crate) fn render_platform_dialogs(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.platform_ui.pending_run.clone() else {
            return;
        };
        let mut decision: Option<bool> = None;
        super::style::render_modal_backdrop(ctx, "deeplink_run_backdrop", true);
        egui::Window::new("deeplink_run_confirm")
            .title_bar(false)
            .frame(super::style::modal_window_frame(ctx))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(520.0)
            .show(ctx, |ui| {
                let mut close = false;
                super::style::render_modal_header(ui, tr("Run query from link?"), &mut close);
                if close {
                    decision = Some(false);
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(trf(
                        "A link asked Tabular to run this query on {}:",
                        &[&pending.connection_name],
                    ));
                    if let Some(env) = pending.environment {
                        environment_badge(ui, env);
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        let mut sql = pending.sql.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut sql)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(super::style::btn_primary_ctx(ui.ctx(), tr("Run")))
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    if ui.button(tr("Open in Editor")).clicked() {
                        decision = Some(false);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                self.platform_ui.pending_run = None;
                if let Some(idx) = self.query_tabs.iter().position(|t| t.id == pending.tab_id) {
                    editor::switch_to_tab(self, idx);
                    self.pending_query = pending.sql;
                    editor::execute_query(self);
                }
            }
            Some(false) => self.platform_ui.pending_run = None,
            None => {}
        }
    }

    /// M11: saat jendela menyempit (Split View / Slide Over), sembunyikan
    /// sidebar dan panel AI; kembalikan keadaan semula saat melebar lagi.
    /// Pengguna tetap bisa membuka sidebar manual dalam mode sempit.
    pub(crate) fn apply_adaptive_layout(&mut self, ctx: &egui::Context) {
        let width = ctx.content_rect().width();
        match compact_transition(self.platform_ui.compact_layout, width) {
            Some(true) => {
                self.platform_ui.panels_before_compact =
                    Some((self.sidebar_visible, self.show_ai_panel));
                self.sidebar_visible = false;
                self.show_ai_panel = false;
                self.platform_ui.compact_layout = true;
            }
            Some(false) => {
                if let Some((sidebar, ai)) = self.platform_ui.panels_before_compact.take() {
                    self.sidebar_visible = sidebar;
                    self.show_ai_panel = ai;
                }
                self.platform_ui.compact_layout = false;
            }
            None => {}
        }
    }

    // ── Environment koneksi (M10) ───────────────────────────────────────────

    pub(crate) fn reload_connection_envs(&mut self) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        match rt.block_on(crate::connection_env::load_all(&pool)) {
            Ok(map) => self.platform_ui.connection_envs = map,
            Err(e) => log::warn!("[ENV] cannot load connection environments: {e}"),
        }
    }

    /// Environment efektif (tanda eksplisit, atau tebakan dari nama).
    pub(crate) fn connection_environment(&self, conn_id: i64, name: &str) -> Option<Environment> {
        crate::connection_env::effective(
            self.platform_ui.connection_envs.get(&conn_id).copied(),
            name,
        )
    }

    pub(crate) fn connection_environment_by_id(&self, conn_id: i64) -> Option<Environment> {
        let name = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.name.as_str())?;
        self.connection_environment(conn_id, name)
    }

    pub(crate) fn set_connection_environment(&mut self, conn_id: i64, env: Option<Environment>) {
        match env {
            Some(e) => {
                self.platform_ui.connection_envs.insert(conn_id, e);
            }
            None => {
                self.platform_ui.connection_envs.remove(&conn_id);
            }
        }
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        if let Err(e) = rt.block_on(crate::connection_env::set(&pool, conn_id, env)) {
            log::warn!("[ENV] cannot save environment for connection {conn_id}: {e}");
        }
    }

    // ── AppleScript / Handoff ───────────────────────────────────────────────

    fn publish_known_connections(&mut self) {
        let list: Vec<(i64, String)> = self
            .connections
            .iter()
            .filter_map(|c| c.id.map(|id| (id, c.name.clone())))
            .collect();
        if list != self.platform_ui.known_signature {
            deeplink::set_known_connections(list.clone());
            self.platform_ui.known_signature = list;
        }
    }

    #[cfg(target_os = "macos")]
    fn update_handoff(&mut self) {
        use crate::privacy::{NetCategory, allowed, record};
        if !self.platform_ui.handoff_receiver_installed {
            self.platform_ui.handoff_receiver_installed = true;
            crate::platform_macos::install_handoff_receiver();
        }
        let enabled =
            crate::platform_prefs::current().handoff_enabled && allowed(NetCategory::Handoff);
        match enabled.then(|| self.handoff_payload()).flatten() {
            Some((title, url)) => {
                crate::platform_macos::set_handoff_activity(Some((&title, &url)));
                record(NetCategory::Handoff, "handoff://activity", true);
            }
            None => crate::platform_macos::set_handoff_activity(None),
        }
    }

    #[cfg(target_os = "macos")]
    fn handoff_payload(&self) -> Option<(String, String)> {
        let tab = self.query_tabs.get(self.active_tab_index)?;
        let conn_id = tab.connection_id?;
        let conn = self.connections.iter().find(|c| c.id == Some(conn_id))?;
        let link = if !tab.content.trim().is_empty() && tab.content.len() <= HANDOFF_MAX_SQL {
            DeepLink::Query {
                connection: conn.name.clone(),
                sql: tab.content.clone(),
                database: tab.database_name.clone(),
                run: false,
            }
        } else {
            DeepLink::Open {
                connection: conn.name.clone(),
                database: tab.database_name.clone(),
                table: None,
            }
        };
        Some((format!("Tabular — {}", tab.title), link.to_url()))
    }

    // ── Keluar aplikasi ─────────────────────────────────────────────────────

    pub(crate) fn platform_on_exit(&mut self) {
        #[cfg(not(target_os = "ios"))]
        crate::single_instance::shutdown_global();
        #[cfg(target_os = "macos")]
        crate::platform_macos::set_handoff_activity(None);
        if self.update_installed && crate::platform_prefs::effective_install_on_quit() {
            crate::auto_updater::AutoUpdater::install_on_quit(self.staged_update_script.as_ref());
        }
    }
}

/// Badge kecil berwarna untuk environment.
pub(crate) fn environment_badge(ui: &mut egui::Ui, env: Environment) -> egui::Response {
    let text = egui::RichText::new(env.short())
        .size(10.0)
        .strong()
        .color(egui::Color32::WHITE);
    egui::Frame::NONE
        .fill(env.color())
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| ui.label(text))
        .response
        .on_hover_text(env.label())
}

/// Garis warna environment di tepi atas `rect` (tab/toolbar).
pub(crate) fn paint_environment_strip(ui: &egui::Ui, rect: egui::Rect, env: Environment) {
    let strip = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + 2.0));
    ui.painter().rect_filled(strip, 0.0, env.color());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_transition_only_fires_on_threshold_crossing() {
        assert_eq!(compact_transition(false, 1024.0), None);
        assert_eq!(compact_transition(false, 507.0), Some(true));
        assert_eq!(compact_transition(true, 320.0), None);
        assert_eq!(compact_transition(true, 1180.0), Some(false));
        assert_eq!(compact_transition(false, COMPACT_WIDTH), None);
    }
}
