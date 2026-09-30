//! Tampilan endpoint HTTP API di diagram database: badge jumlah endpoint di
//! header tabel dan panel daftar endpoint per tabel. Data tautannya
//! (`DiagramState::endpoint_links`) diisi oleh [`crate::repo_links`] saat
//! endpoint di-generate dari repository folder HTTP API yang sama dengan
//! repository group diagram.

use eframe::egui;

use crate::diagram_lod::quantize_font;
use crate::diagram_view::DiagramAction;
use crate::models::structs::DiagramState;

const BADGE_FILL: egui::Color32 = egui::Color32::from_rgb(33, 150, 243);

/// Badge "[ikon API] n" di header tabel. `None` bila zoom terlalu kecil.
pub fn draw_badge(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    count: usize,
    scale: f32,
    id: egui::Id,
    active: bool,
) -> Option<egui::Response> {
    if scale < 0.3 || count == 0 {
        return None;
    }
    let s = scale.clamp(0.7, 1.4);
    let galley = ui.painter().layout_no_wrap(
        format!("{} {count}", egui_icons::icons::MDI_API.codepoint),
        egui::FontId::proportional(quantize_font(10.5 * s)),
        egui::Color32::WHITE,
    );
    let size = galley.size() + egui::vec2(10.0 * s, 4.0 * s);
    let rect = egui::Align2::LEFT_CENTER.anchor_size(pos, size);
    let resp = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if resp.hovered() || active {
        BADGE_FILL.gamma_multiply(1.15)
    } else {
        BADGE_FILL
    };
    ui.painter().rect_filled(rect, size.y / 2.0, fill);
    if active {
        ui.painter().rect_stroke(
            rect,
            size.y / 2.0,
            egui::Stroke::new(1.5, egui::Color32::WHITE),
            egui::StrokeKind::Middle,
        );
    }
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        egui::Color32::WHITE,
    );
    Some(resp)
}

/// Tooltip badge: daftar endpoint tabel (maksimal 12 baris).
pub fn badge_tooltip(all: &[crate::models::structs::EndpointLink], table: &str) -> String {
    let mut links: Vec<&crate::models::structs::EndpointLink> =
        all.iter().filter(|l| l.table == table).collect();
    links.sort_by(|a, b| {
        (a.path.as_str(), crate::repo_links::method_rank(&a.method))
            .cmp(&(b.path.as_str(), crate::repo_links::method_rank(&b.method)))
    });
    let mut lines: Vec<String> = links
        .iter()
        .take(12)
        .map(|l| format!("{} {}", l.method, l.path))
        .collect();
    if links.len() > 12 {
        lines.push(format!("… and {} more", links.len() - 12));
    }
    lines.push("Click for the endpoint list".to_string());
    format!(
        "{} API endpoint(s) use this table\n{}",
        links.len(),
        lines.join("\n")
    )
}

/// Panel daftar endpoint untuk tabel `state.endpoints_panel`.
/// Lebar panel endpoint (layar).
const PANEL_WIDTH: f32 = 380.0;
/// Jarak panel dari tabel.
const PANEL_GAP: f32 = 24.0;

/// Posisi kiri-atas panel di dekat tabel `table` (layar): di kiri tabel
/// sejajar header, atau di kanan bila ruang kiri tidak cukup. Tetap di dalam
/// kanvas `canvas`.
pub fn panel_pos(table: egui::Rect, canvas: egui::Rect) -> egui::Pos2 {
    let left_x = table.left() - PANEL_GAP - PANEL_WIDTH;
    let x = if left_x >= canvas.left() + 8.0 {
        left_x
    } else {
        table.right() + PANEL_GAP
    };
    let max_x = (canvas.right() - PANEL_WIDTH - 8.0).max(canvas.left() + 8.0);
    let max_y = (canvas.bottom() - 160.0).max(canvas.top() + 8.0);
    egui::pos2(
        x.clamp(canvas.left() + 8.0, max_x),
        table.top().clamp(canvas.top() + 8.0, max_y),
    )
}

/// Panel daftar endpoint untuk tabel `state.endpoints_panel`, ditempel di
/// dekat tabelnya (`table_screen`, koordinat layar) seperti kartu note dan
/// ikut bergeser saat kanvas di-pan/zoom.
pub fn render_endpoints_panel(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    rect: egui::Rect,
    table_screen: Option<egui::Rect>,
) -> Option<DiagramAction> {
    let table = state.endpoints_panel.clone()?;
    let Some(title) = state
        .nodes
        .iter()
        .find(|n| n.id == table)
        .map(|n| n.title.clone())
    else {
        state.endpoints_panel = None;
        return None;
    };
    let query = state.endpoints_panel_query.to_lowercase();
    let links: Vec<crate::models::structs::EndpointLink> =
        crate::repo_links::links_for_table(state, &table)
            .into_iter()
            .filter(|l| {
                query.is_empty()
                    || l.path.to_lowercase().contains(&query)
                    || l.method.to_lowercase().contains(&query)
                    || l.summary.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
    let total = state
        .endpoint_links
        .iter()
        .filter(|l| l.table == table)
        .count();

    let mut open = true;
    let mut action = None;
    let mut unlink: Option<crate::models::structs::EndpointLink> = None;
    let pos = table_screen
        .map(|t| panel_pos(t, rect))
        .unwrap_or_else(|| rect.right_top() + egui::vec2(-PANEL_WIDTH - 16.0, 16.0));
    let shown = egui::Window::new(format!("Endpoints · {title}"))
        .id(ui.id().with("diagram_endpoints_panel"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .current_pos(pos)
        .default_width(PANEL_WIDTH)
        .max_width(PANEL_WIDTH)
        .constrain_to(rect)
        .show(ui.ctx(), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{total} HTTP API endpoint(s) read or write this table"
                ))
                .weak(),
            );
            ui.add_space(4.0);
            crate::window_egui::style::render_search_field(
                ui,
                &mut state.endpoints_panel_query,
                "Filter by method or path…",
                ui.available_width(),
            );
            ui.add_space(4.0);
            if links.is_empty() {
                ui.label(egui::RichText::new("No endpoints match.").weak());
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for l in &links {
                        ui.horizontal(|ui| {
                            crate::http_repo::method_chip(ui, &l.method);
                            ui.label(
                                egui::RichText::new(&l.path)
                                    .family(egui::FontFamily::Monospace)
                                    .size(12.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button(egui_icons::icons::ICON_LINK_OFF.codepoint)
                                        .on_hover_text("Unlink this endpoint from the table")
                                        .clicked()
                                    {
                                        unlink = Some(l.clone());
                                    }
                                    if ui
                                        .small_button(egui_icons::icons::ICON_OPEN_IN_NEW.codepoint)
                                        .on_hover_text("Open the request in the HTTP client")
                                        .clicked()
                                    {
                                        action = Some(DiagramAction::OpenEndpointRequest {
                                            request_id: l.request_id.clone(),
                                            label: format!("{} {}", l.method, l.path),
                                        });
                                    }
                                },
                            );
                        });
                        let mut detail = Vec::new();
                        if !l.summary.is_empty() {
                            detail.push(l.summary.clone());
                        }
                        if let Some(src) = &l.source {
                            detail.push(src.clone());
                        }
                        if !detail.is_empty() {
                            ui.label(egui::RichText::new(detail.join(" · ")).small().weak());
                        }
                        ui.add_space(3.0);
                    }
                });
        });
    // Garis putus-putus dari panel ke header tabel, seperti link kartu note.
    if let (Some(win), Some(table)) = (shown, table_screen) {
        let panel = win.response.rect;
        let head_y = table.top() + 12.0;
        let (from, to) = if panel.center().x < table.center().x {
            (
                egui::pos2(panel.right(), head_y.clamp(panel.top(), panel.bottom())),
                egui::pos2(table.left(), head_y),
            )
        } else {
            (
                egui::pos2(panel.left(), head_y.clamp(panel.top(), panel.bottom())),
                egui::pos2(table.right(), head_y),
            )
        };
        ui.painter()
            .with_clip_rect(rect)
            .extend(egui::Shape::dashed_line(
                &[from, to],
                egui::Stroke::new(1.5, BADGE_FILL),
                6.0,
                4.0,
            ));
    }
    if let Some(l) = unlink {
        state.endpoint_links.retain(|x| !x.same_endpoint(&l));
        state.save_requested = true;
    }
    if !open {
        state.endpoints_panel = None;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn panel_sits_left_of_table_when_there_is_room() {
        let canvas = r(0.0, 0.0, 1600.0, 900.0);
        let pos = panel_pos(r(800.0, 300.0, 300.0, 400.0), canvas);
        assert_eq!(pos, egui::pos2(800.0 - PANEL_GAP - PANEL_WIDTH, 300.0));
    }

    #[test]
    fn panel_moves_right_and_stays_inside_canvas() {
        let canvas = r(0.0, 0.0, 1600.0, 900.0);
        let pos = panel_pos(r(100.0, -50.0, 300.0, 400.0), canvas);
        assert_eq!(pos.x, 100.0 + 300.0 + PANEL_GAP);
        assert_eq!(pos.y, 8.0);
        let far = panel_pos(r(1500.0, 2000.0, 300.0, 400.0), r(0.0, 0.0, 600.0, 500.0));
        assert!(far.x <= 600.0 - PANEL_WIDTH - 8.0 + 0.01);
        assert!(far.y <= 500.0 - 160.0 + 0.01);
    }
}
