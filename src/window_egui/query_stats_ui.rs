//! Jendela "Query Insights" (checklist I1), riwayat load tabel 7 hari dan
//! rincian waktu eksekusi (I2), serta notifikasi OS query panjang (I4).
//!
//! Penyimpanan dan agregasi ada di `crate::query_stats` (headless); modul ini
//! hanya menghubungkannya ke state `Tabular` dan menggambar UI.

use eframe::egui;

use super::Tabular;
use crate::connection::QueryResultMessage;
use crate::connection::timing::QueryTiming;
use crate::query_stats::{
    self as stats, DayBucket, ExecutionRecord, QueryAggregate, RunKind, Sample, TableAggregate,
    format_ms,
};

/// Jumlah baris per tampilan.
const LIST_LIMIT: usize = 100;
/// Rentang riwayat load tabel.
const TABLE_HISTORY_DAYS: i64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InsightsView {
    #[default]
    MostRun,
    Slowest,
    GettingSlower,
    TableLoads,
}

impl InsightsView {
    fn label(self) -> &'static str {
        match self {
            InsightsView::MostRun => "Most Run",
            InsightsView::Slowest => "Slowest",
            InsightsView::GettingSlower => "Getting Slower",
            InsightsView::TableLoads => "Table Loads",
        }
    }
}

/// Data hasil agregasi yang sedang ditampilkan.
#[derive(Default)]
pub struct InsightsData {
    pub queries: Vec<QueryAggregate>,
    pub tables: Vec<TableAggregate>,
    pub sample_count: usize,
}

/// Riwayat load satu tabel.
pub struct TableHistory {
    pub connection_id: i64,
    pub table_name: String,
    pub samples: Vec<Sample>,
    pub buckets: Vec<DayBucket>,
    pub error: Option<String>,
}

pub struct InsightsState {
    pub open: bool,
    pub view: InsightsView,
    /// `None` = semua koneksi.
    pub connection_filter: Option<i64>,
    pub days: i64,
    pub search: String,
    pub data: Option<InsightsData>,
    pub error: Option<String>,
    pub confirm_clear: bool,
    pub table_history: Option<TableHistory>,
}

impl Default for InsightsState {
    fn default() -> Self {
        Self {
            open: false,
            view: InsightsView::default(),
            connection_filter: None,
            days: 7,
            search: String::new(),
            data: None,
            error: None,
            confirm_clear: false,
            table_history: None,
        }
    }
}

/// Aksi dari UI yang butuh `&mut Tabular` setelah window selesai digambar.
enum Action {
    Reload,
    OpenQuery { connection_id: i64, query: String },
    TableHistory { connection_id: i64, table: String },
    Clear,
}

/// Waktu lokal singkat dari `executed_at` UTC.
fn local_time(executed_at: &str) -> String {
    stats::parse_utc(executed_at)
        .map(|t| t.format("%d %b %H:%M").to_string())
        .unwrap_or_else(|| executed_at.to_string())
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Warna segmen breakdown: tunggu, server, transfer, klien.
fn segment_colors() -> [egui::Color32; 4] {
    [
        egui::Color32::from_rgb(150, 150, 160),
        egui::Color32::from_rgb(90, 140, 230),
        egui::Color32::from_rgb(80, 190, 150),
        egui::Color32::from_rgb(230, 170, 70),
    ]
}

/// Bar bertumpuk + legenda untuk satu breakdown waktu.
pub fn timing_breakdown_ui(ui: &mut egui::Ui, t: &QueryTiming) {
    let parts = [
        ("Waiting for connection", t.wait_ms),
        ("Server (until first row)", t.server_ms),
        ("Transfer (rows streamed)", t.transfer_ms),
        ("Client processing", t.client_ms),
    ];
    let total = t.total_ms().max(0.001);
    let colors = segment_colors();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 10.0), egui::Sense::hover());
    let mut x = rect.left();
    for (i, (_, ms)) in parts.iter().enumerate() {
        let w = (rect.width() * (*ms / total) as f32).max(0.0);
        if w > 0.0 {
            let seg =
                egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, rect.height()));
            ui.painter().rect_filled(seg, 2.0, colors[i]);
            x += w;
        }
    }
    ui.add_space(4.0);
    egui::Grid::new("timing_breakdown_grid")
        .num_columns(3)
        .spacing([8.0, 2.0])
        .show(ui, |ui| {
            for (i, (label, ms)) in parts.iter().enumerate() {
                ui.label(egui::RichText::new("■").color(colors[i]));
                ui.label(*label);
                ui.label(format!("{} ({:.0}%)", format_ms(*ms), ms / total * 100.0));
                ui.end_row();
            }
            ui.label("");
            ui.label(egui::RichText::new("Total").strong());
            ui.label(egui::RichText::new(format_ms(t.total_ms())).strong());
            ui.end_row();
        });
}

impl Tabular {
    /// Catat satu eksekusi ke `query_stats` di latar belakang. `paginated`
    /// menandai halaman hasil browse; bila tab sedang browse tabel dan query
    /// hanya membaca satu tabel, run dicatat sebagai load tabel.
    pub(crate) fn record_query_stats(&mut self, message: &QueryResultMessage, paginated: bool) {
        if message.query.trim().is_empty() {
            return;
        }
        if let Some(tab_id) = message.tab_id {
            match message.timing {
                Some(t) => {
                    self.query_timings_by_tab.insert(tab_id, t);
                }
                None => {
                    self.query_timings_by_tab.remove(&tab_id);
                }
            }
        }
        let (Some(pool), Some(rt)) = (self.db_pool.clone(), self.runtime.clone()) else {
            return;
        };
        let active_id = self.query_tabs.get(self.active_tab_index).map(|t| t.id);
        let tab = message
            .tab_id
            .and_then(|id| self.query_tabs.iter().find(|t| t.id == id));
        let browsing = tab.is_some_and(|t| {
            t.is_table_browse_mode || (Some(t.id) == active_id && self.is_table_browse_mode)
        });
        let table_name = if paginated && browsing {
            stats::single_table_of_select(&message.query)
        } else {
            None
        };
        let record = ExecutionRecord {
            connection_id: message.connection_id,
            database_name: tab
                .and_then(|t| t.database_name.clone())
                .filter(|d| !d.is_empty()),
            kind: if table_name.is_some() {
                RunKind::TableLoad
            } else {
                RunKind::Query
            },
            query_text: message.query.clone(),
            table_name,
            success: message.success,
            duration_ms: message.duration.as_secs_f64() * 1000.0,
            row_count: message.success.then(|| {
                message
                    .affected_rows
                    .map(|n| n as i64)
                    .unwrap_or(message.rows.len() as i64)
            }),
            timing: message.timing,
        };
        rt.spawn(async move {
            if let Err(e) = stats::record(&pool, &record).await {
                log::debug!("[INSIGHTS] Failed to record query stats: {e}");
            }
        });
    }

    /// Kirim notifikasi OS bila query berjalan lebih lama dari ambang dan
    /// jendela sedang tidak fokus.
    pub(crate) fn notify_long_query(&mut self, message: &QueryResultMessage) {
        if !crate::os_notify::should_notify(
            self.notify_long_queries,
            self.window_focused,
            message.duration,
            self.notify_threshold_secs,
        ) {
            return;
        }
        let conn = self.connection_name(message.connection_id);
        let elapsed = format_ms(message.duration.as_secs_f64() * 1000.0);
        let (title, body) = if message.success {
            let rows = message
                .affected_rows
                .map(|n| format!("{n} row(s) affected"))
                .unwrap_or_else(|| format!("{} row(s)", message.rows.len()));
            (
                format!("Query finished in {elapsed}"),
                format!("{conn} · {rows} · {}", one_line(&message.query, 120)),
            )
        } else {
            (
                format!("Query failed after {elapsed}"),
                format!(
                    "{conn} · {}",
                    one_line(message.error.as_deref().unwrap_or("Unknown error"), 160)
                ),
            )
        };
        crate::os_notify::send(&title, &body);
    }

    pub(crate) fn open_query_insights(&mut self) {
        self.query_stats_view.open = true;
        if self.query_stats_view.data.is_none() {
            self.query_stats_view.connection_filter = self.current_connection_id;
        }
        self.reload_query_insights();
    }

    fn reload_query_insights(&mut self) {
        let Some(pool) = self.db_pool.clone() else {
            self.query_stats_view.error = Some("Local database is not available".into());
            return;
        };
        let conn = self.query_stats_view.connection_filter;
        let days = self.query_stats_view.days.max(1);
        let rt = self.get_runtime();
        match rt.block_on(stats::load_samples(&pool, conn, None, days)) {
            Ok(samples) => {
                let queries: Vec<Sample> = samples
                    .iter()
                    .filter(|s| s.kind == RunKind::Query)
                    .cloned()
                    .collect();
                self.query_stats_view.data = Some(InsightsData {
                    queries: stats::aggregate(&queries),
                    tables: stats::aggregate_tables(&samples),
                    sample_count: samples.len(),
                });
                self.query_stats_view.error = None;
            }
            Err(e) => {
                log::warn!("[INSIGHTS] Failed to load query stats: {e}");
                self.query_stats_view.error = Some(e.to_string());
            }
        }
    }

    /// Buka jendela riwayat load tabel `TABLE_HISTORY_DAYS` hari terakhir.
    pub(crate) fn open_table_load_history(&mut self, connection_id: i64, table_name: String) {
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        let rt = self.get_runtime();
        let result = rt.block_on(stats::table_load_history(
            &pool,
            connection_id,
            &table_name,
            TABLE_HISTORY_DAYS,
        ));
        let today = chrono::Local::now().date_naive();
        self.query_stats_view.table_history = Some(match result {
            Ok(samples) => TableHistory {
                connection_id,
                buckets: stats::daily_buckets(&samples, TABLE_HISTORY_DAYS, today),
                table_name,
                samples,
                error: None,
            },
            Err(e) => TableHistory {
                connection_id,
                table_name,
                samples: Vec::new(),
                buckets: Vec::new(),
                error: Some(e.to_string()),
            },
        });
    }

    fn connection_name(&self, id: i64) -> String {
        self.connections
            .iter()
            .find(|c| c.id == Some(id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("Connection {id}"))
    }

    /// Gambar jendela Insights dan riwayat load tabel bila terbuka.
    pub(crate) fn render_query_insights(&mut self, ctx: &egui::Context) {
        let mut actions: Vec<Action> = Vec::new();
        if self.query_stats_view.open {
            let mut open = true;
            let connections: Vec<(i64, String)> = self
                .connections
                .iter()
                .filter_map(|c| c.id.map(|id| (id, c.name.clone())))
                .collect();
            egui::Window::new("Query Insights")
                .open(&mut open)
                .default_size([860.0, 520.0])
                .min_width(560.0)
                .show(ctx, |ui| {
                    self.insights_toolbar(ui, &connections, &mut actions);
                    ui.separator();
                    self.insights_body(ui, &mut actions);
                });
            if !open {
                self.query_stats_view.open = false;
                self.query_stats_view.confirm_clear = false;
            }
        }
        self.render_table_history_window(ctx);

        for action in actions {
            match action {
                Action::Reload => self.reload_query_insights(),
                Action::OpenQuery {
                    connection_id,
                    query,
                } => {
                    let title = format!("Insight: {}", one_line(&query, 24));
                    crate::editor::create_new_tab_with_connection_and_database(
                        self,
                        title,
                        query,
                        Some(connection_id),
                        None,
                    );
                }
                Action::TableHistory {
                    connection_id,
                    table,
                } => self.open_table_load_history(connection_id, table),
                Action::Clear => {
                    if let Some(pool) = self.db_pool.clone() {
                        let conn = self.query_stats_view.connection_filter;
                        let rt = self.get_runtime();
                        match rt.block_on(stats::clear(&pool, conn)) {
                            Ok(n) => self.toasts.success(format!("Cleared {n} recorded run(s)")),
                            Err(e) => self.toasts.error(format!("Cannot clear insights: {e}")),
                        }
                    }
                    self.query_stats_view.confirm_clear = false;
                    self.reload_query_insights();
                }
            }
        }
    }

    fn insights_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        connections: &[(i64, String)],
        actions: &mut Vec<Action>,
    ) {
        let state = &mut self.query_stats_view;
        ui.horizontal_wrapped(|ui| {
            for view in [
                InsightsView::MostRun,
                InsightsView::Slowest,
                InsightsView::GettingSlower,
                InsightsView::TableLoads,
            ] {
                if ui
                    .selectable_label(state.view == view, view.label())
                    .clicked()
                {
                    state.view = view;
                }
            }
            ui.separator();

            let selected = state
                .connection_filter
                .and_then(|id| connections.iter().find(|(cid, _)| *cid == id))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "All connections".to_string());
            let before = state.connection_filter;
            egui::ComboBox::from_id_salt("insights_connection")
                .selected_text(selected)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.connection_filter, None, "All connections");
                    for (id, name) in connections {
                        ui.selectable_value(&mut state.connection_filter, Some(*id), name);
                    }
                });
            if before != state.connection_filter {
                actions.push(Action::Reload);
            }

            let before_days = state.days;
            egui::ComboBox::from_id_salt("insights_days")
                .selected_text(format!("Last {} days", state.days))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for d in [1, 7, 14, 30] {
                        ui.selectable_value(&mut state.days, d, format!("Last {d} days"));
                    }
                });
            if before_days != state.days {
                actions.push(Action::Reload);
            }

            ui.add(
                egui::TextEdit::singleline(&mut state.search)
                    .hint_text("Filter…")
                    .desired_width(140.0),
            );
            if ui.button("Refresh").clicked() {
                actions.push(Action::Reload);
            }
            if state.confirm_clear {
                ui.label("Delete recorded runs for this scope?");
                if ui.button("Delete").clicked() {
                    actions.push(Action::Clear);
                }
                if ui.button("Cancel").clicked() {
                    state.confirm_clear = false;
                }
            } else if ui.button("Clear…").clicked() {
                state.confirm_clear = true;
            }
        });
    }

    fn insights_body(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if let Some(err) = &self.query_stats_view.error {
            ui.colored_label(super::style::theme_danger(ui.ctx()), err);
            return;
        }
        let Some(data) = &self.query_stats_view.data else {
            ui.label("Loading…");
            return;
        };
        let show_conn = self.query_stats_view.connection_filter.is_none();
        let needle = self.query_stats_view.search.to_lowercase();
        let view = self.query_stats_view.view;
        let days = self.query_stats_view.days;
        ui.label(
            egui::RichText::new(format!(
                "{} recorded run(s) in the last {days} days. Runs are recorded locally when a query or table page finishes.",
                data.sample_count
            ))
            .weak()
            .size(11.5),
        );
        ui.add_space(4.0);

        if view == InsightsView::TableLoads {
            let rows: Vec<&TableAggregate> = data
                .tables
                .iter()
                .filter(|t| needle.is_empty() || t.table_name.to_lowercase().contains(&needle))
                .take(LIST_LIMIT)
                .collect();
            if rows.is_empty() {
                ui.label(
                    "No table loads recorded yet. Open a table from the sidebar to start collecting.",
                );
                return;
            }
            let names: Vec<String> = rows
                .iter()
                .map(|t| self.connection_name(t.connection_id))
                .collect();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("insights_tables")
                        .striped(true)
                        .spacing([14.0, 4.0])
                        .show(ui, |ui| {
                            for h in ["Table", "Database"] {
                                ui.label(egui::RichText::new(h).strong());
                            }
                            if show_conn {
                                ui.label(egui::RichText::new("Connection").strong());
                            }
                            for h in ["Loads", "Avg", "Max", "Last load", ""] {
                                ui.label(egui::RichText::new(h).strong());
                            }
                            ui.end_row();
                            for (t, conn_name) in rows.iter().zip(&names) {
                                ui.label(&t.table_name);
                                ui.label(t.database_name.as_deref().unwrap_or("—"));
                                if show_conn {
                                    ui.label(conn_name);
                                }
                                ui.label(t.loads.to_string());
                                ui.label(format_ms(t.avg_ms));
                                ui.label(format_ms(t.max_ms));
                                ui.label(format!(
                                    "{} ({})",
                                    local_time(&t.last_run),
                                    format_ms(t.last_ms)
                                ));
                                if ui.small_button("7-day history").clicked() {
                                    actions.push(Action::TableHistory {
                                        connection_id: t.connection_id,
                                        table: t.table_name.clone(),
                                    });
                                }
                                ui.end_row();
                            }
                        });
                });
            return;
        }

        let list = match view {
            InsightsView::MostRun => stats::most_run(&data.queries, usize::MAX),
            InsightsView::Slowest => stats::slowest(&data.queries, usize::MAX),
            _ => stats::increasingly_slow(&data.queries, usize::MAX),
        };
        let rows: Vec<&QueryAggregate> = list
            .iter()
            .filter(|a| needle.is_empty() || a.query_text.to_lowercase().contains(&needle))
            .take(LIST_LIMIT)
            .collect();
        if rows.is_empty() {
            ui.label(match view {
                InsightsView::GettingSlower => format!(
                    "No regressions found. A query is flagged when the median of its recent runs is at least 1.5× (and 20 ms) slower than its earlier runs, with at least {} runs.",
                    stats::MIN_RUNS_FOR_TREND
                ),
                _ => "No queries recorded yet. Run a query to start collecting.".to_string(),
            });
            return;
        }
        let names: Vec<String> = rows
            .iter()
            .map(|a| self.connection_name(a.connection_id))
            .collect();
        let trend_col = view == InsightsView::GettingSlower;
        let warn = super::style::theme_warning(ui.ctx());
        let danger = super::style::theme_danger(ui.ctx());
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("insights_queries")
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Query").strong());
                        if show_conn {
                            ui.label(egui::RichText::new("Connection").strong());
                        }
                        for h in ["Runs", "Avg", "p95", "Max"] {
                            ui.label(egui::RichText::new(h).strong());
                        }
                        if trend_col {
                            ui.label(egui::RichText::new("Before → Recent").strong());
                        }
                        ui.label(egui::RichText::new("Last run").strong());
                        ui.label("");
                        ui.end_row();

                        for (a, conn_name) in rows.iter().zip(&names) {
                            ui.label(
                                egui::RichText::new(one_line(&a.query_text, 80))
                                    .family(egui::FontFamily::Monospace)
                                    .size(12.0),
                            )
                            .on_hover_text(&a.query_text);
                            if show_conn {
                                ui.label(conn_name);
                            }
                            let runs = if a.failures > 0 {
                                egui::RichText::new(format!("{} ({} failed)", a.runs, a.failures))
                                    .color(danger)
                            } else {
                                egui::RichText::new(a.runs.to_string())
                            };
                            ui.label(runs);
                            ui.label(format_ms(a.avg_ms));
                            ui.label(format_ms(a.p95_ms));
                            ui.label(format_ms(a.max_ms));
                            if trend_col {
                                let text = a
                                    .trend
                                    .map(|t| {
                                        format!(
                                            "{} → {} (×{:.1})",
                                            format_ms(t.earlier_ms),
                                            format_ms(t.recent_ms),
                                            t.ratio()
                                        )
                                    })
                                    .unwrap_or_default();
                                ui.label(egui::RichText::new(text).color(warn));
                            }
                            ui.label(local_time(&a.last_run));
                            ui.horizontal(|ui| {
                                if ui
                                    .small_button("Open")
                                    .on_hover_text("Open in a new query tab")
                                    .clicked()
                                {
                                    actions.push(Action::OpenQuery {
                                        connection_id: a.connection_id,
                                        query: a.query_text.clone(),
                                    });
                                }
                                if ui.small_button("Copy").clicked() {
                                    ui.ctx().copy_text(a.query_text.clone());
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
    }

    fn render_table_history_window(&mut self, ctx: &egui::Context) {
        let Some(history) = &self.query_stats_view.table_history else {
            return;
        };
        let conn_name = self.connection_name(history.connection_id);
        let mut open = true;
        egui::Window::new(format!("Load history: {}", history.table_name))
            .id(egui::Id::new("table_load_history_window"))
            .open(&mut open)
            .default_size([620.0, 460.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{conn_name} · last {TABLE_HISTORY_DAYS} days · {} load(s)",
                        history.samples.iter().filter(|s| s.success).count()
                    ))
                    .weak(),
                );
                if let Some(err) = &history.error {
                    ui.colored_label(super::style::theme_danger(ui.ctx()), err);
                    return;
                }
                if history.samples.is_empty() {
                    ui.label("No loads recorded for this table in the last 7 days.");
                    return;
                }
                table_history_chart(ui, &history.buckets);
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Recent loads").strong());
                let danger = super::style::theme_danger(ui.ctx());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("table_history_rows")
                            .striped(true)
                            .spacing([12.0, 3.0])
                            .show(ui, |ui| {
                                for h in [
                                    "When", "Total", "Rows", "Wait", "Server", "Transfer", "Client",
                                ] {
                                    ui.label(egui::RichText::new(h).strong());
                                }
                                ui.end_row();
                                for s in history.samples.iter().take(50) {
                                    ui.label(local_time(&s.executed_at));
                                    let total = egui::RichText::new(format_ms(s.duration_ms));
                                    ui.label(if s.success {
                                        total
                                    } else {
                                        total.color(danger)
                                    });
                                    ui.label(
                                        s.row_count.map(|n| n.to_string()).unwrap_or_default(),
                                    );
                                    match s.timing {
                                        Some(t) => {
                                            ui.label(format_ms(t.wait_ms));
                                            ui.label(format_ms(t.server_ms));
                                            ui.label(format_ms(t.transfer_ms));
                                            ui.label(format_ms(t.client_ms));
                                        }
                                        None => {
                                            for _ in 0..4 {
                                                ui.label("—");
                                            }
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
        if !open {
            self.query_stats_view.table_history = None;
        }
    }
}

/// Bar chart rata-rata durasi load per hari; tooltip bar memuat jumlah load.
fn table_history_chart(ui: &mut egui::Ui, buckets: &[DayBucket]) {
    use egui_plot::{Bar, BarChart, Plot};
    let labels: Vec<String> = buckets
        .iter()
        .map(|b| b.date.format("%a %d").to_string())
        .collect();
    let bars: Vec<Bar> = buckets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            Bar::new(i as f64, b.avg_ms).width(0.7).name(format!(
                "{}: {} load(s), avg {}, max {}",
                labels[i],
                b.loads,
                format_ms(b.avg_ms),
                format_ms(b.max_ms)
            ))
        })
        .collect();
    let axis_labels = labels.clone();
    Plot::new("table_load_history_plot")
        .height(170.0)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .include_y(0.0)
        .y_axis_label("avg ms")
        .x_axis_formatter(move |mark, _| {
            let v = mark.value;
            if (v - v.round()).abs() > 1e-6 || v < 0.0 {
                return String::new();
            }
            axis_labels.get(v as usize).cloned().unwrap_or_default()
        })
        .show(ui, |plot_ui| {
            plot_ui.add(
                BarChart::new("Average load time", bars)
                    .color(egui::Color32::from_rgb(90, 140, 230)),
            );
        });
}

/// Badge "⏱" di bar hasil: hover menampilkan rincian waktu eksekusi terakhir
/// tab aktif; klik membuka riwayat load 7 hari bila tab sedang browse tabel.
pub(crate) fn render_timing_badge(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index) else {
        return;
    };
    let timing = tabular.query_timings_by_tab.get(&tab.id).copied();
    let browse_table = if tabular.is_table_browse_mode || tab.is_table_browse_mode {
        let sql = if tab.base_query.trim().is_empty() {
            tabular.current_base_query.clone()
        } else {
            tab.base_query.clone()
        };
        stats::single_table_of_select(&sql)
    } else {
        None
    };
    let conn_id = tab.connection_id;
    if timing.is_none() && browse_table.is_none() {
        return;
    }
    let resp = ui
        .add(
            egui::Button::new(egui::RichText::new("⏱").size(12.0))
                .small()
                .frame(false),
        )
        .on_hover_ui(|ui| {
            match &timing {
                Some(t) => {
                    ui.label(egui::RichText::new("Time breakdown").strong());
                    timing_breakdown_ui(ui, t);
                }
                None => {
                    ui.label(
                        "No time breakdown for this result (measured for row-returning statements on PostgreSQL, MySQL and SQLite).",
                    );
                }
            }
            if let Some(table) = &browse_table {
                ui.separator();
                ui.label(format!(
                    "Click to see load history of “{table}” (last 7 days)."
                ));
            }
        });
    if resp.clicked()
        && let (Some(table), Some(conn_id)) = (browse_table, conn_id)
    {
        tabular.open_table_load_history(conn_id, table);
    }
}
