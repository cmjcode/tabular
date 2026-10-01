//! Panel "API rail" di kiri kanvas diagram: indeks endpoint per tabel utama
//! dan matriks CRUD. Panel ini berskala layar (tidak ikut zoom kanvas), jadi
//! path endpoint selalu terbaca. Memilih endpoint menampilkan card prosesnya
//! di samping tabel-tabelnya (lihat `diagram_flow_layout::spotlight_pos`).
//! Modelnya ada di [`crate::diagram_api_rail`].

use eframe::egui;

use crate::diagram_api_rail::{CRUD, CellFilter, RailEntity, RailModel, RailRow, UNKNOWN_OP};
use crate::diagram_flow_view::{DELETE_COLOR, READ_COLOR, UNKNOWN_COLOR, WRITE_COLOR};
use crate::diagram_view::DiagramAction;
use crate::models::structs::DiagramState;

const DEFAULT_WIDTH: f32 = 320.0;
const MIN_WIDTH: f32 = 240.0;
const MAX_WIDTH: f32 = 560.0;
/// Lebar panel saat dilipat.
const COLLAPSED_WIDTH: f32 = 36.0;
const ROW_HEIGHT: f32 = 22.0;
/// Lebar kolom method di baris endpoint.
const METHOD_WIDTH: f32 = 52.0;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum RailTab {
    #[default]
    Endpoints,
    Matrix,
}

/// State tampilan rail; hanya hidup selama aplikasi berjalan.
#[derive(Clone, Default)]
struct RailUi {
    query: String,
    tab: RailTab,
    cell: Option<CellFilter>,
    collapsed: bool,
}

/// Hasil interaksi rail pada satu frame; diterapkan `render_diagram` setelah
/// ukuran kanvas diketahui.
#[derive(Default)]
pub struct RailOutcome {
    /// Endpoint yang dipilih (id `FlowCard`).
    pub select: Option<String>,
    /// Endpoint terpilih diklik lagi: lepas pilihan.
    pub deselect: bool,
    /// Lompat ke tabel ini.
    pub focus_table: Option<String>,
    pub show_gen_progress: bool,
    pub action: Option<DiagramAction>,
}

/// Rail tampil untuk diagram ini?
pub fn visible(state: &DiagramState) -> bool {
    state.show_endpoints && state.endpoint_display.shows_rail() && !state.flow_cards.is_empty()
}

/// Warna satu huruf operasi, sama dengan garis proses di kanvas. Di tema
/// terang (`dark` = false) digelapkan supaya terbaca di atas panel putih.
pub fn op_letter_color(op: char, dark: bool) -> egui::Color32 {
    let base = match op {
        'C' | 'U' => WRITE_COLOR,
        'R' => READ_COLOR,
        'D' => DELETE_COLOR,
        _ => UNKNOWN_COLOR,
    };
    if dark {
        base
    } else {
        base.lerp_to_gamma(egui::Color32::BLACK, 0.35)
    }
}

/// Potong `text` monospace berukuran `px` supaya muat di `width`, diukur
/// dari lebar glyph sebenarnya.
fn fit_monospace(painter: &egui::Painter, text: &str, width: f32, px: f32) -> String {
    let glyph = painter
        .layout_no_wrap(
            "0".to_owned(),
            egui::FontId::monospace(px),
            egui::Color32::WHITE,
        )
        .size()
        .x
        .max(1.0);
    let max = (width / glyph).floor().max(0.0) as usize;
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn op_name(op: char) -> &'static str {
    match op {
        'C' => "create",
        'R' => "read",
        'U' => "update",
        'D' => "delete",
        _ => "linked, no process yet",
    }
}

fn row_tooltip(row: &RailRow) -> String {
    let mut lines = vec![format!("{} {}", row.method, row.path)];
    if !row.summary.trim().is_empty() {
        lines.push(row.summary.clone());
    }
    if !row.tables.is_empty() {
        let tables: Vec<String> = row
            .tables
            .iter()
            .map(|t| {
                if t.crud.is_empty() {
                    t.title.clone()
                } else {
                    format!("{} ({})", t.title, t.crud)
                }
            })
            .collect();
        lines.push(format!("Tables: {}", tables.join(", ")));
    }
    lines.push(if row.steps == 0 {
        "No business process yet".to_string()
    } else {
        format!("{} step(s) · click to show the process", row.steps)
    });
    lines.join("\n")
}

/// Huruf CRUD berwarna, rata kanan di `right`; mengembalikan tepi kirinya.
fn paint_crud(painter: &egui::Painter, right: egui::Pos2, crud: &str, px: f32, dark: bool) -> f32 {
    let mut x = right.x;
    for c in crud.chars().rev() {
        let g = painter.layout_no_wrap(
            c.to_string(),
            egui::FontId::monospace(px),
            op_letter_color(c, dark),
        );
        x -= g.size().x;
        painter.galley(
            egui::pos2(x, right.y - g.size().y / 2.0),
            g,
            egui::Color32::WHITE,
        );
    }
    x
}

/// Satu baris endpoint: method, path, huruf CRUD.
fn endpoint_row(ui: &mut egui::Ui, row: &RailRow, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let visuals = ui.visuals();
    let method_color = crate::http_repo::method_color(&row.method);
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 4.0, visuals.selection.bg_fill.gamma_multiply(0.4));
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            1.5,
            method_color,
        );
    } else if resp.hovered() {
        painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let cy = rect.center().y;
    painter.text(
        egui::pos2(rect.left() + 8.0, cy),
        egui::Align2::LEFT_CENTER,
        fit_monospace(painter, &row.method, METHOD_WIDTH - 4.0, 10.5),
        egui::FontId::monospace(10.5),
        method_color,
    );
    let right = paint_crud(
        painter,
        egui::pos2(rect.right() - 6.0, cy),
        &row.crud,
        11.0,
        visuals.dark_mode,
    );
    let path_x = rect.left() + 8.0 + METHOD_WIDTH;
    // Endpoint tanpa proses ditulis redup.
    let color = if row.steps == 0 && !selected {
        visuals.weak_text_color()
    } else {
        visuals.text_color()
    };
    painter.text(
        egui::pos2(path_x, cy),
        egui::Align2::LEFT_CENTER,
        fit_monospace(painter, &row.path, (right - 8.0 - path_x).max(0.0), 12.0),
        egui::FontId::monospace(12.0),
        color,
    );
    resp.on_hover_text(row_tooltip(row))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Judul entitas: nama tabel, jumlah endpoint, huruf CRUD berwarna.
fn entity_title(ui: &egui::Ui, entity: &RailEntity) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(13.0);
    job.append(
        &entity.title,
        0.0,
        egui::TextFormat::simple(font.clone(), ui.visuals().strong_text_color()),
    );
    job.append(
        &format!("  {}", entity.rows.len()),
        0.0,
        egui::TextFormat::simple(font, ui.visuals().weak_text_color()),
    );
    for (i, c) in entity.crud.chars().enumerate() {
        job.append(
            &c.to_string(),
            if i == 0 { 8.0 } else { 0.0 },
            egui::TextFormat::simple(
                egui::FontId::monospace(11.0),
                op_letter_color(c, ui.visuals().dark_mode),
            ),
        );
    }
    job
}

/// Menu klik kanan baris endpoint.
fn row_menu(ui: &mut egui::Ui, state: &DiagramState, row: &RailRow, out: &mut RailOutcome) {
    let Some(card) = state.flow_cards.iter().find(|c| c.id == row.card_id) else {
        return;
    };
    if ui
        .add_enabled(
            card.request_id.is_some(),
            egui::Button::new(format!(
                "{} Open Request",
                egui_icons::icons::ICON_OPEN_IN_NEW.codepoint
            )),
        )
        .on_disabled_hover_text("No saved request for this endpoint on this computer")
        .clicked()
    {
        ui.close();
        out.action = Some(DiagramAction::OpenEndpointRequest {
            request_id: card.request_id.clone(),
            label: format!("{} {}", row.method, row.path),
        });
    }
    if ui
        .button(format!(
            "{} Copy as Mermaid",
            egui_icons::icons::ICON_CONTENT_COPY.codepoint
        ))
        .on_hover_text("Copy this process as a Mermaid flowchart")
        .clicked()
    {
        ui.close();
        ui.ctx()
            .copy_text(crate::diagram_mermaid::flow_to_mermaid(state, card));
        out.action = Some(DiagramAction::Info(
            "Mermaid flowchart copied to clipboard".to_string(),
        ));
    }
    if state.scoped_to.is_some() {
        return;
    }
    let generated = card.meta.is_some();
    match crate::diagram_flow_gen_view::generate_menu_item(
        ui,
        crate::diagram_flow_gen_view::is_running(state),
        true,
        generated,
        "",
    ) {
        Some(crate::diagram_flow_gen_view::GenMenuPick::Generate) => {
            ui.close();
            out.action = Some(DiagramAction::GenerateFlows {
                group_id: None,
                card_ids: vec![card.id.clone()],
                force: generated,
            });
        }
        Some(crate::diagram_flow_gen_view::GenMenuPick::ShowProgress) => {
            ui.close();
            out.show_gen_progress = true;
        }
        None => {}
    }
}

/// Tab daftar endpoint.
fn endpoints_tab(
    ui: &mut egui::Ui,
    state: &DiagramState,
    model: &RailModel,
    st: &mut RailUi,
    out: &mut RailOutcome,
) {
    let width = ui.available_width();
    crate::window_egui::style::render_search_field(
        ui,
        &mut st.query,
        "Filter by method, path or table…",
        width,
    );
    if let Some(cell) = st.cell.clone() {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} · {}", cell.title, op_name(cell.op)))
                    .small()
                    .color(op_letter_color(cell.op, ui.visuals().dark_mode)),
            );
            if ui
                .small_button(egui_icons::icons::ICON_CLOSE.codepoint)
                .on_hover_text("Clear this filter")
                .clicked()
            {
                st.cell = None;
            }
        });
    }
    ui.add_space(4.0);
    let filtering = !st.query.trim().is_empty() || st.cell.is_some();
    let shown = model.filtered(&st.query, st.cell.as_ref());
    let many_sections = model.sections.len() > 1;
    let selected = state.selected_flow.as_deref();

    egui::ScrollArea::vertical()
        .id_salt("diagram_api_rail_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if shown.sections.is_empty() {
                ui.label(egui::RichText::new("No endpoints match.").weak());
                return;
            }
            ui.spacing_mut().item_spacing.y = 1.0;
            for section in &shown.sections {
                if many_sections {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{} · {}", section.title, section.count()))
                            .small()
                            .strong(),
                    );
                }
                for entity in &section.entities {
                    let mut header = egui::CollapsingHeader::new(entity_title(ui, entity))
                        .id_salt(("diagram_api_rail_entity", &section.repo_key, &entity.table))
                        .default_open(true);
                    // Saat menyaring, semua hasil langsung terlihat.
                    if filtering {
                        header = header.open(Some(true));
                    }
                    let resp = header.show(ui, |ui| {
                        for row in &entity.rows {
                            let is_selected = Some(row.card_id.as_str()) == selected;
                            let r = endpoint_row(ui, row, is_selected);
                            if r.clicked() {
                                if is_selected {
                                    out.deselect = true;
                                } else {
                                    out.select = Some(row.card_id.clone());
                                }
                            }
                            r.context_menu(|ui| row_menu(ui, state, row, out));
                        }
                    });
                    if let Some(table) = &entity.table {
                        let head = resp
                            .header_response
                            .on_hover_text("Double-click to jump to this table");
                        if head.double_clicked() {
                            out.focus_table = Some(table.clone());
                        }
                    }
                }
            }
        });
}

/// Tab matriks CRUD: jumlah endpoint per operasi untuk tiap tabel.
fn matrix_tab(ui: &mut egui::Ui, model: &RailModel, st: &mut RailUi, out: &mut RailOutcome) {
    ui.label(
        egui::RichText::new(
            "Endpoints that create, read, update or delete each table. \
             ? = linked without a process. Click a number to list them.",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);
    let rows = model.matrix();
    if rows.is_empty() {
        ui.label(egui::RichText::new("No endpoint uses a table of this diagram yet.").weak());
        return;
    }
    let ops: Vec<char> = CRUD.iter().copied().chain([UNKNOWN_OP]).collect();
    egui::ScrollArea::both()
        .id_salt("diagram_api_rail_matrix_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("diagram_api_rail_matrix")
                .num_columns(ops.len() + 1)
                .striped(true)
                .spacing(egui::vec2(10.0, 4.0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Table").small().weak());
                    for &op in &ops {
                        ui.label(
                            egui::RichText::new(op.to_string())
                                .family(egui::FontFamily::Monospace)
                                .strong()
                                .color(op_letter_color(op, ui.visuals().dark_mode)),
                        )
                        .on_hover_text(op_name(op));
                    }
                    ui.end_row();
                    for row in &rows {
                        let title = ui
                            .add(
                                egui::Label::new(egui::RichText::new(&row.title).size(12.5))
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Click to jump to this table")
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if title.clicked() {
                            out.focus_table = Some(row.table.clone());
                        }
                        for (i, &op) in ops.iter().enumerate() {
                            let count = row.counts.get(i).copied().unwrap_or(row.unknown);
                            if count == 0 {
                                ui.label("");
                                continue;
                            }
                            let cell = CellFilter {
                                table: row.table.clone(),
                                title: row.title.clone(),
                                op,
                            };
                            let active = st.cell.as_ref() == Some(&cell);
                            let text = egui::RichText::new(count.to_string())
                                .family(egui::FontFamily::Monospace)
                                .color(op_letter_color(op, ui.visuals().dark_mode));
                            if ui
                                .selectable_label(active, text)
                                .on_hover_text(format!(
                                    "{count} endpoint(s) · {} · {}",
                                    op_name(op),
                                    row.title
                                ))
                                .clicked()
                            {
                                st.cell = Some(cell);
                                st.query.clear();
                                st.tab = RailTab::Endpoints;
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Gambar rail di tepi kiri `ui` (memperkecil area kanvas sesudahnya).
/// Dipanggil `render_diagram` sebelum kanvas mengambil sisa areanya.
pub fn render_rail(ui: &mut egui::Ui, state: &DiagramState, model: &RailModel) -> RailOutcome {
    let mut out = RailOutcome::default();
    let ui_id = ui.id().with("diagram_api_rail_ui");
    let mut st: RailUi = ui.data(|d| d.get_temp(ui_id)).unwrap_or_default();
    let fill = ui.visuals().panel_fill;
    let total = model.total();

    if st.collapsed {
        egui::Panel::left("diagram_api_rail_collapsed")
            .resizable(false)
            .exact_size(COLLAPSED_WIDTH)
            .frame(egui::Frame::new().fill(fill).inner_margin(4))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if ui
                        .small_button(egui_icons::icons::MDI_API.codepoint)
                        .on_hover_text(format!("Show the API panel ({total} endpoints)"))
                        .clicked()
                    {
                        st.collapsed = false;
                    }
                    ui.label(egui::RichText::new(total.to_string()).small().weak());
                });
            });
    } else {
        egui::Panel::left("diagram_api_rail")
            .resizable(true)
            .default_size(DEFAULT_WIDTH)
            .size_range(MIN_WIDTH..=MAX_WIDTH)
            .frame(egui::Frame::new().fill(fill).inner_margin(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} API",
                            egui_icons::icons::MDI_API.codepoint
                        ))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{total} endpoints · {} with process",
                            model.with_process()
                        ))
                        .small()
                        .weak(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(egui_icons::icons::ICON_CHEVRON_LEFT.codepoint)
                            .on_hover_text("Hide the API panel")
                            .clicked()
                        {
                            st.collapsed = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.tab, RailTab::Endpoints, "Endpoints")
                        .on_hover_text("Endpoints grouped by their main table");
                    ui.selectable_value(&mut st.tab, RailTab::Matrix, "CRUD matrix")
                        .on_hover_text("How many endpoints touch each table, per operation");
                });
                ui.add_space(4.0);
                match st.tab {
                    RailTab::Endpoints => endpoints_tab(ui, state, model, &mut st, &mut out),
                    RailTab::Matrix => matrix_tab(ui, model, &mut st, &mut out),
                }
            });
    }
    ui.data_mut(|d| d.insert_temp(ui_id, st));
    out
}
