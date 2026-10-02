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

/// Teks badge: ikon API, jumlah endpoint, lalu huruf CRUD bila sudah ada
/// proses yang menyebut operasinya.
fn badge_text(count: usize, crud: &str) -> String {
    let icon = egui_icons::icons::MDI_API.codepoint;
    if crud.is_empty() {
        format!("{icon} {count}")
    } else {
        format!("{icon} {count} · {crud}")
    }
}

/// Badge "[ikon API] n · CRUD" di header tabel. `crud` = huruf operasi
/// gabungan semua endpoint ke tabel ini (boleh kosong). `None` bila zoom
/// terlalu kecil.
pub fn draw_badge(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    count: usize,
    crud: &str,
    scale: f32,
    id: egui::Id,
    active: bool,
) -> Option<egui::Response> {
    if scale < 0.3 || count == 0 {
        return None;
    }
    let s = scale.clamp(0.7, 1.4);
    let galley = ui.painter().layout_no_wrap(
        badge_text(count, crud),
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
    lines.push("C create · R read · U update · D delete".to_string());
    lines.push("Click for the endpoint list".to_string());
    format!(
        "{} API endpoint(s) use this table\n{}",
        links.len(),
        lines.join("\n")
    )
}

/// Panel daftar endpoint untuk tabel `state.endpoints_panel`.
/// Lebar kartu endpoint (layar).
const PANEL_WIDTH: f32 = 380.0;
/// Jarak kartu dari tabel.
const PANEL_GAP: f32 = 24.0;
/// Perkiraan tinggi satu baris endpoint (chip + path, lalu ringkasan),
/// dipakai sebelum tinggi sebenarnya terukur.
const ROW_HEIGHT: f32 = 52.0;
/// Perkiraan tinggi header + ringkasan + kotak filter (termasuk padding atas).
const HEAD_HEIGHT: f32 = 90.0;
/// Padding bawah kartu.
const PAD_BOTTOM: f32 = 8.0;

fn measure_id(ui: &egui::Ui, table: &str) -> egui::Id {
    ui.id().with(("diagram_endpoints_measure", table))
}

fn scroll_rect_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("diagram_endpoints_scroll_rect")
}

/// Bingkai layar kartu endpoint pada frame sebelumnya, hanya bila daftarnya
/// perlu di-scroll. Dipakai kanvas supaya scroll di atasnya menggulung
/// daftar, bukan zoom diagram.
pub fn last_scroll_rect(ui: &egui::Ui) -> Option<egui::Rect> {
    ui.data(|d| d.get_temp::<egui::Rect>(scroll_rect_id(ui)))
}

/// Posisi kiri-atas kartu setinggi `height` di dekat tabel `table` (layar):
/// di kiri tabel sejajar header, atau di kanan bila ruang kiri tidak cukup.
/// Tetap di dalam kanvas `canvas`.
pub fn panel_pos(table: egui::Rect, canvas: egui::Rect, height: f32) -> egui::Pos2 {
    let left_x = table.left() - PANEL_GAP - PANEL_WIDTH;
    let x = if left_x >= canvas.left() + 8.0 {
        left_x
    } else {
        table.right() + PANEL_GAP
    };
    let max_x = (canvas.right() - PANEL_WIDTH - 8.0).max(canvas.left() + 8.0);
    let max_y = (canvas.bottom() - height - 8.0).max(canvas.top() + 8.0);
    egui::pos2(
        x.clamp(canvas.left() + 8.0, max_x),
        table.top().clamp(canvas.top() + 8.0, max_y),
    )
}

/// Kartu daftar endpoint untuk tabel `state.endpoints_panel`. Digambar
/// langsung di kanvas (bukan `egui::Window`) seperti kartu note, jadi selalu
/// menempel di samping tabelnya (`table_screen`, koordinat layar) dan ikut
/// bergeser saat kanvas di-pan/zoom.
pub fn render_endpoints_panel(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    rect: egui::Rect,
    table_screen: Option<egui::Rect>,
) -> Option<DiagramAction> {
    ui.data_mut(|d| d.remove::<egui::Rect>(scroll_rect_id(ui)));
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

    // Kartu setinggi isinya (hasil ukur frame sebelumnya); daftar baru
    // di-scroll bila kartu sudah setinggi kanvas.
    let mid = measure_id(ui, &table);
    let (head_h, content_h) = ui
        .data(|d| d.get_temp::<(f32, f32)>(mid))
        .unwrap_or((HEAD_HEIGHT, links.len().max(1) as f32 * ROW_HEIGHT));
    let max_list = (rect.height() - 16.0 - head_h - PAD_BOTTOM).max(ROW_HEIGHT);
    let list_h = content_h.min(max_list);
    let scrollable = content_h > max_list + 0.5;
    let height = head_h + list_h + PAD_BOTTOM;
    let pos = match table_screen {
        Some(t) => panel_pos(t, rect, height),
        None => rect.right_top() + egui::vec2(-PANEL_WIDTH - 16.0, 16.0),
    };
    let card = egui::Rect::from_min_size(pos, egui::vec2(PANEL_WIDTH, height));
    let visuals = ui.visuals().clone();
    let base_id = ui.id().with("diagram_endpoints_card");

    // Garis putus-putus dari kartu ke header tabel, seperti link kartu note.
    let painter = ui.painter().with_clip_rect(rect);
    if let Some(t) = table_screen {
        let head_y = t.top() + 12.0;
        let (from, to) = if card.center().x < t.center().x {
            (
                egui::pos2(card.right(), card.top() + 16.0),
                egui::pos2(t.left(), head_y),
            )
        } else {
            (
                egui::pos2(card.left(), card.top() + 16.0),
                egui::pos2(t.right(), head_y),
            )
        };
        painter.extend(egui::Shape::dashed_line(
            &[from, to],
            egui::Stroke::new(1.5, BADGE_FILL),
            6.0,
            4.0,
        ));
    }
    painter.rect_filled(
        card.translate(egui::vec2(2.0, 4.0)),
        8.0,
        egui::Color32::from_black_alpha(90),
    );
    painter.rect_filled(card, 8.0, visuals.window_fill);
    painter.rect_stroke(
        card,
        8.0,
        egui::Stroke::new(1.0, BADGE_FILL.linear_multiply(0.8)),
        egui::StrokeKind::Middle,
    );
    // Klik/drag di atas kartu tidak diteruskan ke kanvas di bawahnya.
    let _ = ui.interact(card, base_id.with("body"), egui::Sense::click_and_drag());

    let mut close = false;
    let mut action = None;
    let mut unlink: Option<crate::models::structs::EndpointLink> = None;
    let mut show_card: Option<crate::models::structs::EndpointLink> = None;
    let cards_shown = state.endpoint_display.shows_rail();
    if scrollable {
        ui.data_mut(|d| d.insert_temp(scroll_rect_id(ui), card));
    }
    let inner = card.shrink2(egui::vec2(10.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .id_salt(("diagram_endpoints_card", &table))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(inner.intersect(rect));
    child.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} Endpoints · {title}",
                egui_icons::icons::MDI_API.codepoint
            ))
            .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button(egui_icons::icons::ICON_CLOSE.codepoint)
                .on_hover_text("Close")
                .clicked()
            {
                close = true;
            }
        });
    });
    child.label(
        egui::RichText::new(format!(
            "{total} HTTP API endpoint(s) read or write this table"
        ))
        .small()
        .weak(),
    );
    child.add_space(4.0);
    crate::window_egui::style::render_search_field(
        &mut child,
        &mut state.endpoints_panel_query,
        "Filter by method or path…",
        inner.width(),
    );
    child.add_space(4.0);
    let measured_head = child.cursor().top() - card.top();
    let measured_content = if links.is_empty() {
        child
            .label(egui::RichText::new("No endpoints match.").weak())
            .rect
            .height()
    } else {
        egui::ScrollArea::vertical()
            .id_salt(("diagram_endpoints_scroll", &table))
            .max_height(list_h)
            .auto_shrink([false, true])
            .show(&mut child, |ui| {
                for l in &links {
                    ui.horizontal(|ui| {
                        crate::http_repo::method_chip(ui, &l.method);
                        ui.label(
                            egui::RichText::new(&l.path)
                                .family(egui::FontFamily::Monospace)
                                .size(12.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(egui_icons::icons::ICON_LINK_OFF.codepoint)
                                .on_hover_text("Unlink this endpoint from the table")
                                .clicked()
                            {
                                unlink = Some(l.clone());
                            }
                            if cards_shown
                                && ui
                                    .small_button(egui_icons::icons::ICON_VISIBILITY.codepoint)
                                    .on_hover_text("Show the process card of this endpoint")
                                    .clicked()
                            {
                                show_card = Some(l.clone());
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
                        });
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
            })
            .content_size
            .y
    };
    if (measured_head - head_h).abs() > 0.5 || (measured_content - content_h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(mid, (measured_head, measured_content)));
        ui.ctx().request_repaint();
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && ui.rect_contains_pointer(card) {
        close = true;
    }

    if let Some(l) = show_card {
        let id = crate::diagram_flow::card_for_endpoint(
            state,
            l.repo_key.as_deref(),
            &l.method,
            &l.path,
        )
        .map(|c| c.id.clone());
        let now = ui.input(|i| i.time);
        if let Some(id) = id
            && crate::diagram_flow_view::spotlight_card(state, &id, rect.size(), now)
        {
            state.endpoints_panel = None;
        }
    }
    if let Some(l) = unlink {
        crate::diagram_flow::unlink_endpoint(state, &l);
        state.save_requested = true;
    }
    if close {
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
        let pos = panel_pos(r(800.0, 300.0, 300.0, 400.0), canvas, 200.0);
        assert_eq!(pos, egui::pos2(800.0 - PANEL_GAP - PANEL_WIDTH, 300.0));
    }

    #[test]
    fn panel_moves_right_and_stays_inside_canvas() {
        let canvas = r(0.0, 0.0, 1600.0, 900.0);
        let pos = panel_pos(r(100.0, -50.0, 300.0, 400.0), canvas, 200.0);
        assert_eq!(pos.x, 100.0 + 300.0 + PANEL_GAP);
        assert_eq!(pos.y, 8.0);
        let far = panel_pos(
            r(1500.0, 2000.0, 300.0, 400.0),
            r(0.0, 0.0, 600.0, 500.0),
            200.0,
        );
        assert!(far.x <= 600.0 - PANEL_WIDTH - 8.0 + 0.01);
        assert!(far.y <= 500.0 - 200.0 - 8.0 + 0.01);
    }
}
