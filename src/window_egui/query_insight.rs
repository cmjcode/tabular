//! Panel "Query Insight": split view di kanan editor SQL berisi diagram
//! alur query beranimasi dan saran optimasi dari AI.
//!
//! Analisis dan penggambaran ada di `crate::query_diagram` (headless); modul
//! ini hanya menghubungkannya ke state `Tabular`, cache skema, dan backend AI.

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;

use super::Tabular;
use crate::models::enums::DatabaseType;
use crate::query_diagram::layout::{QueryLayout, build_layout};
use crate::query_diagram::render::{
    QueryDiagramView, floating_bar, floating_window_at_bottom, render_query_diagram,
};
use crate::query_diagram::{self, QueryDiagramModel, StatementKind, prompt};

/// Lebar minimum panel kanan.
const MIN_PANEL_W: f32 = 320.0;
/// Lebar awal panel kanan.
pub const DEFAULT_PANEL_W: f32 = 640.0;
const SPLITTER_W: f32 = 6.0;

/// Status permintaan saran AI.
pub enum AiStatus {
    Idle,
    Loading {
        started: std::time::Instant,
    },
    Done(String),
    Failed(String),
    /// Backend AI belum dikonfigurasi; isinya pesan untuk user.
    Unavailable(String),
}

/// State panel per tab (kunci: `QueryTab::id`).
pub struct QueryInsight {
    pub sql: String,
    pub db_label: String,
    pub model: Option<QueryDiagramModel>,
    pub layout: Option<QueryLayout>,
    pub error: Option<String>,
    pub view: QueryDiagramView,
    pub hints: Vec<String>,
    pub ai: AiStatus,
    ai_rx: Option<Receiver<Result<String, String>>>,
    /// Jendela floating "How it works" sedang terbuka.
    pub show_steps: bool,
    /// Jendela floating saran AI sedang terbuka.
    pub show_ai: bool,
    pub notice: Option<String>,
    md_cache: egui_commonmark::CommonMarkCache,
}

/// Aksi yang dijalankan setelah panel selesai digambar (butuh `&mut Tabular`).
enum PanelAction {
    Close,
    AskAi,
    Apply(String),
    OpenInTab,
    ContinueInChat,
    Refresh,
}

fn db_label(db: &DatabaseType) -> &'static str {
    match db {
        DatabaseType::MySQL => "MySQL",
        DatabaseType::PostgreSQL => "PostgreSQL",
        DatabaseType::SQLite => "SQLite",
        DatabaseType::MsSQL => "SQL Server",
        DatabaseType::Redis => "Redis",
        DatabaseType::MongoDB => "MongoDB",
        DatabaseType::ApiHttp => "HTTP",
    }
}

/// Jenis database tab aktif; tanpa koneksi dianggap MySQL (parser tetap
/// mencoba dialek generik bila gagal).
fn active_db_type(tabular: &Tabular) -> DatabaseType {
    tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id)
        .and_then(|cid| tabular.connections.iter().find(|c| c.id == Some(cid)))
        .map(|c| c.connection_type.clone())
        .unwrap_or(DatabaseType::MySQL)
}

fn active_conn_db(tabular: &Tabular) -> Option<(i64, String)> {
    let tab = tabular.query_tabs.get(tabular.active_tab_index)?;
    Some((
        tab.connection_id?,
        tab.database_name.clone().unwrap_or_default(),
    ))
}

/// Kolom tabel dari cache lokal; mencoba nama lengkap lalu tanpa schema.
fn cached_columns(tabular: &Tabular, table: &str) -> Option<Vec<(String, String)>> {
    let (cid, db) = active_conn_db(tabular)?;
    let short = table.rsplit('.').next().unwrap_or(table);
    crate::cache_data::get_columns_from_cache(tabular, cid, &db, table)
        .filter(|c| !c.is_empty())
        .or_else(|| {
            (short != table)
                .then(|| crate::cache_data::get_columns_from_cache(tabular, cid, &db, short))
                .flatten()
        })
        .filter(|c| !c.is_empty())
}

/// Apakah tab aktif sedang menampilkan panel.
pub fn has_panel(tabular: &Tabular) -> bool {
    tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .is_some_and(|t| tabular.query_insights.contains_key(&t.id))
}

/// Statement yang dipilih, atau statement di posisi kursor.
fn current_statement(tabular: &mut Tabular) -> String {
    let selected = tabular.selected_text.trim();
    if !selected.is_empty() {
        return selected.to_string();
    }
    crate::editor::extract_query_from_cursor(tabular)
}

/// Buka (atau segarkan) panel untuk statement di kursor pada tab aktif.
pub fn open_query_insight(tabular: &mut Tabular) {
    let Some(tab_id) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.id)
    else {
        return;
    };
    let sql = current_statement(tabular);
    analyze_into(tabular, tab_id, sql);
}

/// Analisis `sql` dan simpan hasilnya di panel tab `tab_id`.
fn analyze_into(tabular: &mut Tabular, tab_id: usize, sql: String) {
    let db = active_db_type(tabular);
    let result = {
        let lookup = |table: &str| {
            cached_columns(tabular, table).map(|cols| cols.into_iter().map(|(n, _)| n).collect())
        };
        query_diagram::analyze_with_schema(&sql, &db, &lookup)
    };
    let mut insight = QueryInsight {
        sql: sql.trim().to_string(),
        db_label: db_label(&db).to_string(),
        model: None,
        layout: None,
        error: None,
        view: QueryDiagramView::default(),
        hints: Vec::new(),
        ai: AiStatus::Idle,
        ai_rx: None,
        show_steps: false,
        show_ai: false,
        notice: None,
        md_cache: egui_commonmark::CommonMarkCache::default(),
    };
    // Jendela floating yang sedang terbuka tetap terbuka saat dianalisis ulang.
    if let Some(prev) = tabular.query_insights.get(&tab_id) {
        insight.show_steps = prev.show_steps;
        insight.show_ai = prev.show_ai;
    }
    match result {
        Ok(model) => {
            log::info!(
                "[QUERY_DIAGRAM] {} dengan {} sumber, {} join",
                model.kind.label(),
                model.sources.len(),
                model.joins.len()
            );
            insight.hints = prompt::quick_hints(&model);
            insight.layout = Some(build_layout(&model));
            insight.model = Some(model);
        }
        Err(e) => {
            log::debug!("[QUERY_DIAGRAM] analisis gagal: {e}");
            insight.error = Some(e.to_string());
        }
    }
    tabular.query_insights.insert(tab_id, insight);
}

/// Ringkasan skema tabel yang dipakai statement untuk prompt AI.
fn schema_context(tabular: &mut Tabular, model: &QueryDiagramModel) -> String {
    let Some((cid, db)) = active_conn_db(tabular) else {
        return String::new();
    };
    let tables = model.physical_tables();
    let mut out = String::new();
    for table in &tables {
        let Some(cols) = cached_columns(tabular, table) else {
            continue;
        };
        let cols: Vec<String> = cols.iter().map(|(n, t)| format!("{n} {t}")).collect();
        out.push_str(&format!("- {table}({})", cols.join(", ")));
        let short = table.rsplit('.').next().unwrap_or(table).to_string();
        if let Some(pk) = crate::cache_data::get_primary_keys_from_cache(tabular, cid, &db, &short)
            && !pk.is_empty()
        {
            out.push_str(&format!(" PRIMARY KEY({})", pk.join(", ")));
        }
        out.push('\n');
    }
    if let Some(fks) = crate::cache_data::get_foreign_keys_from_cache(tabular, cid, &db) {
        let names: Vec<String> = tables
            .iter()
            .map(|t| t.rsplit('.').next().unwrap_or(t).to_lowercase())
            .collect();
        for fk in fks
            .iter()
            .filter(|fk| names.contains(&fk.table_name.to_lowercase()))
            .take(40)
        {
            out.push_str(&format!(
                "- FOREIGN KEY {}.{} -> {}.{}\n",
                fk.table_name, fk.column_name, fk.referenced_table_name, fk.referenced_column_name
            ));
        }
    }
    out
}

/// Kirim permintaan saran optimasi ke backend AI yang dipilih user.
fn start_ai(tabular: &mut Tabular, tab_id: usize) {
    let Some((model, hints, db)) = tabular
        .query_insights
        .get(&tab_id)
        .and_then(|i| Some((i.model.clone()?, i.hints.clone(), i.db_label.clone())))
    else {
        return;
    };
    let target = tabular.ai_chat_target;
    if let Err(msg) = crate::ai_assistant::backend_ready_for(tabular, target) {
        if let Some(i) = tabular.query_insights.get_mut(&tab_id) {
            i.ai = AiStatus::Unavailable(msg);
            i.ai_rx = None;
        }
        return;
    }
    let schema = schema_context(tabular, &model);
    let cfg = crate::ai_assistant::chat_backend_for(tabular, target);
    let rx = crate::ai_assistant::request_text(
        &cfg,
        prompt::optimize_system_prompt(&db),
        prompt::optimize_user_prompt(&model, &schema, &hints),
    );
    if let Some(i) = tabular.query_insights.get_mut(&tab_id) {
        i.ai = AiStatus::Loading {
            started: std::time::Instant::now(),
        };
        i.ai_rx = Some(rx);
    }
    log::info!("[QUERY_DIAGRAM] meminta saran optimasi AI");
}

/// Ambil jawaban AI yang sudah datang untuk semua panel.
fn poll_ai(tabular: &mut Tabular) {
    let open_ids: Vec<usize> = tabular.query_tabs.iter().map(|t| t.id).collect();
    tabular.query_insights.retain(|id, _| open_ids.contains(id));
    for insight in tabular.query_insights.values_mut() {
        let Some(rx) = &insight.ai_rx else { continue };
        match rx.try_recv() {
            Ok(Ok(text)) => {
                insight.ai = AiStatus::Done(text);
                insight.ai_rx = None;
            }
            Ok(Err(e)) => {
                log::warn!("[QUERY_DIAGRAM] saran AI gagal: {e}");
                insight.ai = AiStatus::Failed(e);
                insight.ai_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                insight.ai =
                    AiStatus::Failed("The AI backend stopped without a reply.".to_string());
                insight.ai_rx = None;
            }
        }
    }
}

/// Bagi area menjadi editor (kiri) dan panel insight (kanan) dengan
/// pembatas yang bisa digeser.
pub fn render_split(
    tabular: &mut Tabular,
    ui: &mut egui::Ui,
    left: impl FnOnce(&mut Tabular, &mut egui::Ui),
) {
    poll_ai(tabular);
    let full = ui.available_rect_before_wrap();
    let max_w = (full.width() * 0.7).max(MIN_PANEL_W);
    let panel_w = if full.width() < MIN_PANEL_W * 2.0 {
        full.width() * 0.5
    } else {
        tabular.query_insight_width.clamp(MIN_PANEL_W, max_w)
    };
    let split_x = full.max.x - panel_w - SPLITTER_W;
    let left_rect = egui::Rect::from_min_max(full.min, egui::pos2(split_x, full.max.y));
    let split_rect = egui::Rect::from_min_max(
        egui::pos2(split_x, full.min.y),
        egui::pos2(split_x + SPLITTER_W, full.max.y),
    );
    let right_rect = egui::Rect::from_min_max(egui::pos2(split_rect.max.x, full.min.y), full.max);

    let split_resp = ui.interact(
        split_rect,
        ui.id().with("query_insight_splitter"),
        egui::Sense::drag(),
    );
    if split_resp.dragged() {
        tabular.query_insight_width =
            (panel_w - split_resp.drag_delta().x).clamp(MIN_PANEL_W, max_w);
    }
    let active = split_resp.hovered() || split_resp.dragged();
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let line = if active {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    ui.painter().line_segment(
        [split_rect.center_top(), split_rect.center_bottom()],
        egui::Stroke::new(if active { 2.0 } else { 1.0 }, line),
    );

    let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect));
    left_ui.set_clip_rect(left_rect.intersect(ui.clip_rect()));
    left(tabular, &mut left_ui);

    let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(right_rect));
    right_ui.set_clip_rect(right_rect.intersect(ui.clip_rect()));
    render_panel(tabular, &mut right_ui);

    ui.allocate_rect(full, egui::Sense::hover());
}

fn kind_color(kind: StatementKind) -> egui::Color32 {
    match kind {
        StatementKind::Select => egui::Color32::from_rgb(170, 110, 255),
        StatementKind::Insert => egui::Color32::from_rgb(60, 200, 120),
        StatementKind::Update => egui::Color32::from_rgb(255, 170, 40),
        StatementKind::Delete => egui::Color32::from_rgb(240, 85, 85),
    }
}

fn icon_button(ui: &mut egui::Ui, icon: egui_icons::MaterialIcon, tip: &str) -> bool {
    ui.add(egui::Button::new(icon.rich_text().size(15.0)).frame(false))
        .on_hover_text(tip)
        .clicked()
}

fn render_panel(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let Some(tab_id) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.id)
    else {
        return;
    };
    let backend = crate::ai_assistant::backend_label_for(tabular, tabular.ai_chat_target);
    let bg = super::style::ai_panel_bg(ui.ctx());
    let Some(ins) = tabular.query_insights.get_mut(&tab_id) else {
        return;
    };
    let mut actions: Vec<PanelAction> = Vec::new();

    egui::Frame::NONE
        .fill(bg)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            // Header.
            ui.horizontal(|ui| {
                if let Some(m) = &ins.model {
                    let c = kind_color(m.kind);
                    egui::Frame::NONE
                        .fill(c.gamma_multiply(0.18))
                        .stroke(egui::Stroke::new(1.0, c))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(m.kind.label())
                                    .color(c)
                                    .strong()
                                    .size(11.0),
                            );
                        });
                }
                ui.label(egui::RichText::new("Query Diagram").strong());
                ui.label(egui::RichText::new(&ins.db_label).weak().size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, egui_icons::icons::ICON_CLOSE, "Close") {
                        actions.push(PanelAction::Close);
                    }
                    if ins.layout.is_some() {
                        if icon_button(
                            ui,
                            egui_icons::icons::ICON_OPEN_IN_NEW,
                            "Open as a diagram tab",
                        ) {
                            actions.push(PanelAction::OpenInTab);
                        }
                        if icon_button(
                            ui,
                            egui_icons::icons::ICON_FIT_SCREEN,
                            "Fit to panel (or double-click the canvas)",
                        ) {
                            ins.view.fit();
                        }
                        if icon_button(
                            ui,
                            egui_icons::icons::ICON_RESTART_ALT,
                            "Move all tables back to their original position",
                        ) {
                            ins.view.reset_positions();
                        }
                        if icon_button(ui, egui_icons::icons::ICON_REPLAY, "Replay animation") {
                            ins.view.replay();
                        }
                    }
                    if icon_button(
                        ui,
                        egui_icons::icons::ICON_REFRESH,
                        "Analyze the statement at the cursor again",
                    ) {
                        actions.push(PanelAction::Refresh);
                    }
                });
            });

            if let Some(err) = &ins.error {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("This statement can't be drawn").strong());
                ui.label(egui::RichText::new(err).weak());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(crate::query_diagram::clip(&ins.sql, 300))
                        .family(egui::FontFamily::Monospace)
                        .size(11.0),
                );
                return;
            }
            let (Some(model), Some(layout)) = (&ins.model, &ins.layout) else {
                return;
            };
            ui.label(egui::RichText::new(model.kind.summary()).weak().size(11.5));
            if let Some(w) = &layout.warning {
                let red = egui::Color32::from_rgb(240, 85, 85);
                egui::Frame::NONE
                    .fill(red.gamma_multiply(0.14))
                    .stroke(egui::Stroke::new(1.0, red))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui_icons::icons::ICON_WARNING.rich_text().color(red));
                            ui.label(egui::RichText::new(w).color(red));
                        });
                    });
            }
            ui.add_space(4.0);

            // Kanvas mengisi seluruh sisa panel; jendela lain melayang di atasnya.
            let canvas = render_query_diagram(ui, layout, &mut ins.view);

            // --- Tombol floating di pojok kanan bawah ---
            let ai_label = format!(
                "{}  Analyze AI",
                String::from(egui_icons::icons::ICON_AUTO_AWESOME)
            );
            let bar_h = 36.0;
            floating_bar(
                ui,
                ui.id().with(("qi_buttons", tab_id)),
                canvas,
                0.92,
                |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let help = ui
                        .add(
                            egui::Button::new(egui_icons::icons::ICON_HELP_OUTLINE.rich_text())
                                .selected(ins.show_steps),
                        )
                        .on_hover_text("How it works");
                    if help.clicked() {
                        ins.show_steps = !ins.show_steps;
                    }
                    let busy = matches!(ins.ai, AiStatus::Loading { .. });
                    let ai = ui
                        .add(egui::Button::new(ai_label).selected(ins.show_ai))
                        .on_hover_text(if busy {
                            "The AI is analyzing this query"
                        } else {
                            "Ask the AI how to make this query faster"
                        });
                    if ai.clicked() {
                        if ins.show_ai {
                            ins.show_ai = false;
                        } else {
                            ins.show_ai = true;
                            if matches!(ins.ai, AiStatus::Idle) {
                                actions.push(PanelAction::AskAi);
                            }
                        }
                    }
                },
            );

            // Kedua jendela floating menempel di kanan bawah, tepat di atas
            // deretan tombol. Bila keduanya terbuka dan muat, "How it works"
            // berada di kiri jendela AI; bila tidak muat, jendela AI di atasnya.
            let bar_top = canvas.max.y - 12.0 - bar_h - 8.0;
            let right = canvas.max.x - 12.0;
            let avail_h = (bar_top - canvas.min.y - 12.0).max(120.0);
            let ai_w = (canvas.width() - 24.0).min(440.0);
            let steps_w = (canvas.width() - 24.0).min(380.0);
            let side_by_side = ins.show_ai && canvas.width() >= ai_w + steps_w + 36.0;
            let steps_right = if side_by_side {
                right - ai_w - 12.0
            } else {
                right
            };

            // --- Jendela floating "How it works" (latar 60%) ---
            if ins.show_steps {
                let steps_max_h = (avail_h * 0.7).max(120.0);
                floating_window_at_bottom(
                    ui,
                    ui.id().with(("qi_steps", tab_id)),
                    egui::pos2(steps_right, bar_top),
                    steps_w,
                    steps_max_h,
                    0.6,
                    |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui_icons::icons::ICON_HELP_OUTLINE.rich_text().size(14.0));
                        ui.label(egui::RichText::new("How it works").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, egui_icons::icons::ICON_CLOSE, "Close") {
                                ins.show_steps = false;
                            }
                        });
                    });
                    egui::ScrollArea::vertical()
                        .id_salt(("query_insight_steps_scroll", tab_id))
                        .max_height(steps_max_h - 56.0)
                        .show(ui, |ui| {
                            for (i, s) in layout.steps.iter().enumerate() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{}.", i + 1))
                                            .weak()
                                            .size(11.5),
                                    );
                                    ui.label(egui::RichText::new(s).size(11.5));
                                });
                            }
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "Drag a table to move it, drag the background to pan, scroll to zoom, hover a column to trace it.",
                                )
                                .weak()
                                .size(10.5),
                            );
                        });
                });
            }

            // --- Jendela floating saran AI ---
            if ins.show_ai {
                floating_window_at_bottom(
                    ui,
                    ui.id().with(("qi_ai", tab_id)),
                    egui::pos2(right, bar_top),
                    ai_w,
                    avail_h,
                    0.97,
                    |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui_icons::icons::ICON_AUTO_AWESOME
                                .rich_text()
                                .color(egui::Color32::from_rgb(170, 110, 255)),
                        );
                        ui.label(egui::RichText::new("Optimization suggestions").strong());
                        ui.label(egui::RichText::new(&backend).weak().size(11.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, egui_icons::icons::ICON_CLOSE, "Close") {
                                ins.show_ai = false;
                            }
                            let busy = matches!(ins.ai, AiStatus::Loading { .. });
                            let asked = !matches!(ins.ai, AiStatus::Idle);
                            if asked
                                && !busy
                                && icon_button(
                                    ui,
                                    egui_icons::icons::ICON_REFRESH,
                                    "Ask the AI again",
                                )
                            {
                                actions.push(PanelAction::AskAi);
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(("query_insight_ai", tab_id))
                        .max_height(avail_h - 64.0)
                        .show(ui, |ui| {
                            render_ai_body(ui, ins, &backend, &mut actions);
                        });
                });
            }
        });

    for action in actions {
        match action {
            PanelAction::Close => {
                tabular.query_insights.remove(&tab_id);
            }
            PanelAction::AskAi => start_ai(tabular, tab_id),
            PanelAction::Refresh => open_query_insight(tabular),
            PanelAction::Apply(sql) => apply_optimized(tabular, tab_id, sql, ui.ctx()),
            PanelAction::OpenInTab => open_in_tab(tabular, tab_id),
            PanelAction::ContinueInChat => {
                if let Some(ins) = tabular.query_insights.get(&tab_id) {
                    tabular.ai_input = format!(
                        "Help me optimize this {} query further:\n```sql\n{}\n```",
                        ins.db_label, ins.sql
                    );
                    tabular.show_ai_panel = true;
                }
            }
        }
    }
}

/// Isi jendela saran AI: petunjuk cepat lalu status/jawaban AI.
fn render_ai_body(
    ui: &mut egui::Ui,
    ins: &mut QueryInsight,
    backend: &str,
    actions: &mut Vec<PanelAction>,
) {
    if !ins.hints.is_empty() {
        ui.label(egui::RichText::new("Quick checks").strong().size(12.0));
        for h in &ins.hints {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui_icons::icons::ICON_LIGHTBULB
                        .rich_text()
                        .size(13.0)
                        .color(egui::Color32::from_rgb(255, 190, 60)),
                );
                ui.label(egui::RichText::new(h).size(12.0));
            });
        }
        ui.add_space(6.0);
    }
    if let Some(n) = &ins.notice {
        ui.label(egui::RichText::new(n).weak().italics());
    }
    match &ins.ai {
        AiStatus::Idle => {
            if ui.button("Analyze with AI").clicked() {
                actions.push(PanelAction::AskAi);
            }
        }
        AiStatus::Loading { started } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "Asking {backend} for suggestions… {}s",
                    started.elapsed().as_secs()
                ));
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        AiStatus::Unavailable(msg) => {
            ui.label(egui::RichText::new(msg).weak());
            if ui.button("Try again").clicked() {
                actions.push(PanelAction::AskAi);
            }
        }
        AiStatus::Failed(e) => {
            ui.label(
                egui::RichText::new(format!("The AI request failed: {e}"))
                    .color(ui.visuals().error_fg_color),
            );
            if ui.button("Retry").clicked() {
                actions.push(PanelAction::AskAi);
            }
        }
        AiStatus::Done(text) => {
            let optimized = prompt::extract_sql_block(text);
            ui.horizontal_wrapped(|ui| {
                if let Some(sql) = &optimized
                    && ui
                        .button("Apply optimized query")
                        .on_hover_text("Replace the statement in the editor (undo with Cmd/Ctrl+Z)")
                        .clicked()
                {
                    actions.push(PanelAction::Apply(sql.clone()));
                }
                if let Some(sql) = &optimized
                    && ui.button("Copy SQL").clicked()
                {
                    ui.ctx().copy_text(sql.clone());
                }
                if ui.button("Continue in chat").clicked() {
                    actions.push(PanelAction::ContinueInChat);
                }
            });
            ui.add_space(4.0);
            egui_commonmark::CommonMarkViewer::new().show(ui, &mut ins.md_cache, text);
        }
    }
}

/// Ganti statement asal di editor dengan versi optimal dari AI.
fn apply_optimized(tabular: &mut Tabular, tab_id: usize, sql: String, ctx: &egui::Context) {
    let Some(original) = tabular.query_insights.get(&tab_id).map(|i| i.sql.clone()) else {
        return;
    };
    let text = tabular.editor.text.clone();
    let Some(pos) = text.find(&original) else {
        ctx.copy_text(sql);
        if let Some(i) = tabular.query_insights.get_mut(&tab_id) {
            i.notice = Some("The statement changed in the editor, so the optimized query was copied to the clipboard instead.".to_string());
        }
        return;
    };
    let new_text = format!("{}{}{}", &text[..pos], sql, &text[pos + original.len()..]);
    let idx = tabular.active_tab_index;
    crate::editor::ai_write_tab_content(tabular, idx, new_text, true);
    log::info!("[QUERY_DIAGRAM] query optimal diterapkan ke editor");
    // Gambar ulang diagram untuk statement baru, saran AI lama disimpan.
    let prev_ai = tabular
        .query_insights
        .get_mut(&tab_id)
        .map(|i| std::mem::replace(&mut i.ai, AiStatus::Idle));
    analyze_into(tabular, tab_id, sql);
    if let (Some(i), Some(ai)) = (tabular.query_insights.get_mut(&tab_id), prev_ai) {
        i.ai = ai;
        i.notice = Some("Applied. The diagram now shows the optimized query.".to_string());
    }
}

/// Buka diagram query sebagai tab diagram biasa (tidak pernah disimpan).
fn open_in_tab(tabular: &mut Tabular, tab_id: usize) {
    let Some((state, kind)) = tabular.query_insights.get(&tab_id).and_then(|i| {
        let l = i.layout.as_ref()?;
        Some((crate::query_diagram::build::build_diagram_state(l), l.kind))
    }) else {
        return;
    };
    let (conn, db) = tabular
        .query_tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| (t.connection_id, t.database_name.clone()))
        .unwrap_or((None, None));
    crate::editor::create_new_tab_with_connection_and_database(
        tabular,
        format!("Diagram: {} query", kind.label()),
        String::new(),
        conn,
        db,
    );
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.diagram_state = Some(state);
    }
    tabular.table_bottom_view = crate::models::structs::TableBottomView::Query;
}
