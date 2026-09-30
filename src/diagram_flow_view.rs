//! Flow card di kanvas diagram: card proses bisnis per endpoint, garis ke
//! tabel yang disentuh langkahnya, dan menu card. Geometrinya ada di
//! [`crate::diagram_flow_layout`], logika datanya di [`crate::diagram_flow`].

use std::collections::HashSet;

use eframe::egui;

use crate::diagram_flow::{FlowDirection, TableUse};
use crate::diagram_flow_layout::{self as layout, BandOwner, FlowFrame};
use crate::diagram_lod::{Lod, lod_for_zoom, quantize_font, truncate_for_width};
use crate::diagram_view::DiagramAction;
use crate::models::structs::{
    DiagramState, FlowCard, FlowLineMode, FlowOp, FlowStep, FlowStepKind, FlowTarget,
};

/// Warna garis operasi baca.
pub const READ_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 200, 255);
/// Warna garis operasi tulis (insert, update, upsert).
pub const WRITE_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 220, 140);
/// Warna garis operasi hapus.
pub const DELETE_COLOR: egui::Color32 = egui::Color32::from_rgb(239, 83, 80);
/// Warna garis operasi yang tidak diketahui (hanya dari `endpoint_links`).
pub const UNKNOWN_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);
/// Lama sorotan tabel setelah baris langkah diklik.
const STEP_HIGHLIGHT_SECS: f64 = 1.6;
/// Opacity card yang tidak termasuk fokus.
const DIM_CARD: f32 = 0.25;

fn highlight_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("diagram_flow_step_highlight")
}

/// Warna sebuah operasi; sama dengan partikel pemutaran.
pub fn op_color(op: Option<FlowOp>) -> egui::Color32 {
    crate::diagram_flow_play_view::op_color(op)
}

/// Warna garis card ke satu tabel: operasi tulis menang atas baca.
pub fn use_color(u: &TableUse) -> egui::Color32 {
    let has = |ops: &[FlowOp]| u.ops.iter().any(|o| ops.contains(o));
    if has(&[FlowOp::Insert, FlowOp::Update, FlowOp::Upsert]) {
        WRITE_COLOR
    } else if has(&[FlowOp::Delete]) {
        DELETE_COLOR
    } else if has(&[FlowOp::Read]) {
        READ_COLOR
    } else {
        UNKNOWN_COLOR
    }
}

/// Ikon dan warna jenis langkah.
pub fn step_kind_style(kind: FlowStepKind) -> (&'static str, egui::Color32) {
    use egui::Color32 as C;
    use egui_icons::icons as i;
    match kind {
        FlowStepKind::Auth => (i::ICON_LOCK.codepoint, C::from_rgb(255, 179, 0)),
        FlowStepKind::Validate => (i::ICON_RULE.codepoint, C::from_rgb(171, 71, 188)),
        FlowStepKind::Db => (i::MDI_DATABASE.codepoint, C::from_rgb(66, 165, 245)),
        FlowStepKind::External => (i::ICON_PUBLIC.codepoint, C::from_rgb(38, 198, 218)),
        FlowStepKind::Queue => (i::ICON_QUEUE.codepoint, C::from_rgb(255, 138, 101)),
        FlowStepKind::Cache => (i::ICON_MEMORY.codepoint, C::from_rgb(240, 98, 146)),
        FlowStepKind::Branch => (i::ICON_CALL_SPLIT.codepoint, C::from_rgb(212, 225, 87)),
        FlowStepKind::Respond => (i::ICON_REPLY.codepoint, C::from_rgb(102, 187, 106)),
        FlowStepKind::Logic | FlowStepKind::Unknown => {
            (i::ICON_CODE.codepoint, C::from_rgb(144, 164, 174))
        }
    }
}

fn screen_rect(r: egui::Rect, to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max))
}

/// Label target sebuah langkah (judul tabel atau nama resource).
fn target_label(state: &DiagramState, step: &FlowStep) -> Option<String> {
    Some(match step.target.as_ref()? {
        FlowTarget::Table(t) => state
            .nodes
            .iter()
            .find(|n| &n.id == t)
            .map_or_else(|| t.clone(), |n| n.title.clone()),
        FlowTarget::External(s) | FlowTarget::Queue(s) | FlowTarget::Cache(s) => s.clone(),
        FlowTarget::Flow(id) => state.flow_cards.iter().find(|c| &c.id == id).map_or_else(
            || id.clone(),
            |c| format!("{} {}", c.trigger.method, c.trigger.target),
        ),
        FlowTarget::Unknown => return None,
    })
}

/// Teks tooltip baris langkah.
fn step_tooltip(state: &DiagramState, index: usize, step: &FlowStep) -> String {
    let mut lines = vec![format!("{}. {}", index + 1, step.title)];
    if let Some(t) = target_label(state, step) {
        let op = step
            .op
            .map(|o| format!("{o:?} ").to_lowercase())
            .unwrap_or_default();
        lines.push(format!("{op}{t}"));
    }
    if !step.columns.is_empty() {
        lines.push(format!("Columns: {}", step.columns.join(", ")));
    }
    if !step.detail.is_empty() {
        lines.push(step.detail.clone());
    }
    if let Some(c) = &step.condition {
        lines.push(format!("When: {c}"));
    }
    if let Some(s) = &step.source {
        lines.push(format!("Source: {s}"));
    }
    lines.join("\n")
}

/// Card yang garisnya selalu digambar: terpilih, difokuskan, atau diputar.
fn is_active(state: &DiagramState, card: &FlowCard) -> bool {
    let id = Some(card.id.as_str());
    state.selected_flow.as_deref() == id
        || state.focus_flow.as_deref() == id
        || state.flow_play.as_ref().map(|p| p.card_id.as_str()) == id
}

/// Urutan gambar card: card aktif paling akhir (paling atas).
fn draw_order(state: &DiagramState) -> Vec<usize> {
    let mut order: Vec<usize> = (0..state.flow_cards.len()).collect();
    order.sort_by_key(|&i| is_active(state, &state.flow_cards[i]));
    order
}

/// Rect layar card `i` sesuai LOD saat ini.
fn card_screen_rect(
    state: &DiagramState,
    frame: &FlowFrame,
    i: usize,
    lod: Lod,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
) -> egui::Rect {
    screen_rect(layout::lod_rect(frame.drawn_rect(state, i), lod), to_screen)
}

/// Card paling atas di bawah pointer.
pub fn card_at(
    state: &DiagramState,
    frame: &FlowFrame,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    pos: egui::Pos2,
) -> Option<usize> {
    let lod = lod_for_zoom(state.zoom);
    draw_order(state)
        .into_iter()
        .rev()
        .find(|&i| card_screen_rect(state, frame, i, lod, to_screen).contains(pos))
}

/// Titik kontrol kurva (layar) dari card `card_index` ke tabel `table_id`.
/// Dipakai juga oleh animasi pemutaran supaya partikel jalan di garis yang
/// sama. `None` bila tabelnya tidak ada di diagram.
pub fn card_curve(
    state: &DiagramState,
    frame: &FlowFrame,
    card_index: usize,
    table_id: &str,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
) -> Option<[egui::Pos2; 4]> {
    let card = state.flow_cards.get(card_index)?;
    let node = state.nodes.iter().find(|n| n.id == table_id)?;
    let lod = lod_for_zoom(state.zoom);
    // Ujung garis di baris kolom pertama yang disebut langkah (LOD Detail).
    let column = (lod == Lod::Detail)
        .then(|| {
            card.steps
                .iter()
                .filter(|s| s.target.as_ref().and_then(|t| t.table()) == Some(table_id))
                .find_map(|s| s.columns.first())
        })
        .flatten()
        .filter(|c| node.columns.iter().any(|x| x == *c));
    let cr = card_screen_rect(state, frame, card_index, lod, to_screen);
    let tr = screen_rect(egui::Rect::from_min_size(node.pos, node.size), to_screen);
    let s = state.zoom;
    let line_scale = s.max(0.5);
    let column_y = column.map(|c| {
        to_screen(egui::pos2(
            node.pos.x,
            crate::diagram_view::column_anchor_y(node, c),
        ))
        .y
    });
    // Card di pita API (di atas tabel): garis turun dari tepi bawah card ke
    // tepi atas tabel, atau masuk dari samping di baris kolomnya.
    if cr.bottom() <= tr.top() {
        let p0 = egui::pos2(cr.center().x, cr.bottom());
        let (p3, d3) = match column_y {
            Some(y) if cr.center().x <= tr.center().x => {
                (egui::pos2(tr.left(), y), egui::vec2(-1.0, 0.0))
            }
            Some(y) => (egui::pos2(tr.right(), y), egui::vec2(1.0, 0.0)),
            None => {
                let inset = (12.0 * s).min(tr.width() / 2.0);
                let x = cr.center().x.clamp(tr.left() + inset, tr.right() - inset);
                (egui::pos2(x, tr.top()), egui::vec2(0.0, -1.0))
            }
        };
        let k0 = ((p3.y - p0.y).abs() * 0.5).clamp(30.0 * line_scale, 240.0 * line_scale);
        let k3 = if d3.y == 0.0 {
            ((p3.x - p0.x).abs() * 0.5).clamp(30.0 * line_scale, 200.0 * line_scale)
        } else {
            k0
        };
        return Some([p0, p0 + egui::vec2(0.0, k0), p3 + d3 * k3, p3]);
    }
    let target_y = match column_y {
        Some(y) => y,
        None => tr.top() + (16.0 * s).min(tr.height() / 2.0),
    };
    let start_y = cr.top() + (layout::CARD_HEADER_H * s / 2.0).min(cr.height() / 2.0);
    let (p0, d0, p3, d3) = if tr.center().x >= cr.center().x {
        (
            egui::pos2(cr.right(), start_y),
            1.0,
            egui::pos2(tr.left(), target_y),
            -1.0,
        )
    } else {
        (
            egui::pos2(cr.left(), start_y),
            -1.0,
            egui::pos2(tr.right(), target_y),
            1.0,
        )
    };
    let k = ((p3.x - p0.x).abs() * 0.45).clamp(30.0 * line_scale, 200.0 * line_scale);
    Some([
        p0,
        p0 + egui::vec2(d0 * k, 0.0),
        p3 + egui::vec2(d3 * k, 0.0),
        p3,
    ])
}

fn sample(ctrl: &[egui::Pos2; 4], n: usize) -> Vec<egui::Pos2> {
    let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
        *ctrl,
        false,
        egui::Color32::TRANSPARENT,
        egui::Stroke::NONE,
    );
    (0..=n)
        .map(|i| bezier.sample(i as f32 / n as f32))
        .collect()
}

/// Panah kecil di `tip` searah `dir` (layar).
fn arrow(
    painter: &egui::Painter,
    tip: egui::Pos2,
    dir: egui::Vec2,
    size: f32,
    color: egui::Color32,
) {
    let d = dir.normalized();
    let n = egui::vec2(-d.y, d.x);
    let base = tip - d * size;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, base + n * size * 0.55, base - n * size * 0.55],
        color,
        egui::Stroke::NONE,
    ));
}

/// Nomor langkah ("1, 3, 5") untuk chip di tengah garis.
fn step_numbers(u: &TableUse) -> String {
    let mut parts: Vec<String> = u
        .steps
        .iter()
        .take(4)
        .map(|i| (i + 1).to_string())
        .collect();
    if u.steps.len() > 4 {
        parts.push("…".into());
    }
    parts.join(", ")
}

/// Huruf CRUD operasi ke satu tabel ("CR", "U", ...); urutan C, R, U, D.
pub fn crud_letters(ops: &[FlowOp]) -> String {
    let has = |xs: &[FlowOp]| ops.iter().any(|o| xs.contains(o));
    [
        (has(&[FlowOp::Insert, FlowOp::Upsert]), 'C'),
        (has(&[FlowOp::Read]), 'R'),
        (has(&[FlowOp::Update, FlowOp::Upsert]), 'U'),
        (has(&[FlowOp::Delete]), 'D'),
    ]
    .into_iter()
    .filter_map(|(on, c)| on.then_some(c))
    .collect()
}

/// Teks chip garis: huruf CRUD lalu nomor langkah, mis. "CU · 3, 5".
fn chip_text(u: &TableUse) -> String {
    let crud = crud_letters(&u.ops);
    match (crud.is_empty(), u.steps.is_empty()) {
        (true, _) => step_numbers(u),
        (false, true) => crud,
        (false, false) => format!("{crud} · {}", step_numbers(u)),
    }
}

/// Card yang garisnya boleh digambar, beserta apakah card itu aktif.
fn line_cards(state: &DiagramState, hovered: Option<usize>) -> Vec<(usize, bool)> {
    state
        .flow_cards
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let active = is_active(state, c) || hovered == Some(i);
            (active || state.flow_lines == FlowLineMode::All).then_some((i, active))
        })
        .collect()
}

/// Garis proses dari card ke tabel yang disentuhnya. Mode `Selected` hanya
/// menggambar card terpilih, di-hover, atau yang sedang diputar; mode `All`
/// menggambar semuanya dengan opacity rendah kecuali yang aktif. Di atas
/// `DASH_BUDGET` garis, garis tidak aktif dilewati. Mengembalikan jumlah
/// garis yang digambar.
pub fn draw_flow_lines(
    ui: &egui::Ui,
    state: &DiagramState,
    frame: &FlowFrame,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    clip: egui::Rect,
    hover: Option<egui::Pos2>,
) -> usize {
    if state.flow_cards.is_empty() {
        return 0;
    }
    let hovered = hover.and_then(|p| card_at(state, frame, to_screen, p));
    let cards = line_cards(state, hovered);
    let total: usize = cards.iter().map(|&(i, _)| frame.tables[i].len()).sum();
    let skip_inactive = total > crate::diagram_lod::DASH_BUDGET;
    let lod = lod_for_zoom(state.zoom);
    let line_scale = state.zoom.max(0.5);
    let painter = ui.painter();
    let mut chips: Vec<(egui::Pos2, String, egui::Color32)> = Vec::new();
    let mut drawn = 0;

    for (i, active) in cards {
        if !active && skip_inactive {
            continue;
        }
        let card = &state.flow_cards[i];
        let tables: Vec<&str> = frame.tables[i].iter().map(String::as_str).collect();
        for u in crate::diagram_flow::table_uses(card, &tables) {
            let Some(ctrl) = card_curve(state, frame, i, &u.table, to_screen) else {
                continue;
            };
            if !crate::diagram_lod::curve_visible(&ctrl, clip, 16.0) {
                continue;
            }
            drawn += 1;
            let base = use_color(&u);
            let color = if active {
                base
            } else {
                base.gamma_multiply(crate::diagram_lod::DENSE_OPACITY)
            };
            let width = if active { 2.2 } else { 1.4 } * line_scale;
            let points = sample(&ctrl, crate::diagram_lod::curve_samples(ctrl[0], ctrl[3]));
            // Chip dekat ujung tabel: di pita, tengah kurva tertutup card lain.
            let mid = points[points.len() * 4 / 5];
            painter.add(egui::Shape::line(points, egui::Stroke::new(width, color)));
            let size = 8.0 * line_scale;
            let end_dir = ctrl[3] - ctrl[2];
            let start_dir = ctrl[0] - ctrl[1];
            match u.direction() {
                FlowDirection::ToTarget => arrow(painter, ctrl[3], end_dir, size, color),
                FlowDirection::FromTarget => arrow(painter, ctrl[0], start_dir, size, color),
                FlowDirection::Both => {
                    arrow(painter, ctrl[3], end_dir, size, color);
                    arrow(painter, ctrl[0], start_dir, size, color);
                }
                FlowDirection::Unknown => {
                    painter.circle_filled(ctrl[3], 3.0 * line_scale, color);
                }
            }
            if active && lod == Lod::Detail {
                let text = chip_text(&u);
                if !text.is_empty() {
                    chips.push((mid, text, base));
                }
            }
        }
    }
    for (p, text, color) in chips {
        crate::diagram_view::draw_chip_label(ui, p, text, color);
    }
    drawn
}

/// Judul pita API: label lapisan "API", garis pemisah API/DATA di atas
/// tabel, dan judul resource tiap tumpukan. Pita di luar group mendapat
/// bingkai sendiri; pita group memakai bingkai group-nya.
pub fn draw_bands(
    ui: &egui::Ui,
    state: &DiagramState,
    frame: &FlowFrame,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    clip: egui::Rect,
) {
    let s = state.zoom;
    let lod = lod_for_zoom(s);
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().strong_text_color();
    let painter = ui.painter();
    // Label lapisan tetap terbaca saat zoom kecil (seperti judul group).
    let label_px = quantize_font((12.0 * s).max(10.0));
    let stack_px = quantize_font(11.0 * s);
    let show_stacks = lod != Lod::Overview && stack_px >= 7.0;

    for band in &frame.bands {
        let area = band.tables.map_or(band.rect, |t| band.rect.union(t));
        if !clip.intersects(screen_rect(area, to_screen)) {
            continue;
        }
        let br = screen_rect(band.rect, to_screen);
        let title = match &band.owner {
            BandOwner::Group(_) | BandOwner::Repo(None) => "API".to_string(),
            BandOwner::Repo(Some(repo)) => format!("API · {repo}"),
            BandOwner::Unmapped => "API · endpoints without tables in this diagram".to_string(),
        };
        if !matches!(band.owner, BandOwner::Group(_)) {
            let r = br.expand(12.0 * s);
            painter.rect_filled(r, 8.0 * s, weak.gamma_multiply(0.06));
            painter.rect_stroke(
                r,
                8.0 * s,
                egui::Stroke::new(1.0, weak.gamma_multiply(0.5)),
                egui::StrokeKind::Middle,
            );
        }

        // Pemisah lapisan API dan DATA, setengah `BAND_GAP` di atas tabel.
        if let Some(t) = band.tables {
            let y = t.top() - layout::BAND_GAP / 2.0;
            let a = to_screen(egui::pos2(band.rect.left().min(t.left()), y));
            let b = to_screen(egui::pos2(band.rect.right().max(t.right()), y));
            painter.extend(egui::Shape::dashed_line(
                &[a, b],
                egui::Stroke::new(1.0, weak.gamma_multiply(0.6)),
                6.0 * s.max(0.5),
                4.0 * s.max(0.5),
            ));
            // Label "DATA" hanya bila muat di celah antara garis dan tabel.
            if layout::BAND_GAP / 2.0 * s >= label_px + 4.0 {
                painter.text(
                    a + egui::vec2(0.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    "DATA",
                    egui::FontId::proportional(label_px),
                    weak,
                );
            }
        }
        painter.text(
            egui::pos2(br.left(), br.top() + layout::BAND_LABEL_H * s / 2.0),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(label_px),
            strong.gamma_multiply(0.85),
        );
        if !show_stacks {
            continue;
        }
        for st in &band.stacks {
            let hr = screen_rect(st.header, to_screen);
            if !clip.intersects(hr) {
                continue;
            }
            let noun = if st.count == 1 {
                "endpoint"
            } else {
                "endpoints"
            };
            painter.text(
                hr.left_center(),
                egui::Align2::LEFT_CENTER,
                format!("{} · {} {noun}", st.resource, st.count),
                egui::FontId::monospace(stack_px),
                weak,
            );
            painter.line_segment(
                [
                    egui::pos2(hr.left(), hr.bottom() - 3.0 * s),
                    egui::pos2(hr.right(), hr.bottom() - 3.0 * s),
                ],
                egui::Stroke::new(1.0, weak.gamma_multiply(0.3)),
            );
        }
    }
}

/// Ringkasan blueprint: jumlah endpoint, yang sudah punya proses, tabel yang
/// disentuh, serta tanggal dan commit generate terakhir.
pub fn blueprint_summary(state: &DiagramState, frame: &FlowFrame) -> String {
    let total = state.flow_cards.len();
    let with_steps = state
        .flow_cards
        .iter()
        .filter(|c| !c.steps.is_empty())
        .count();
    let tables: HashSet<&str> = frame.tables.iter().flatten().map(String::as_str).collect();
    let mut text = format!(
        "{total} endpoint(s) · {with_steps} with process · {} table(s) touched",
        tables.len()
    );
    let latest = state
        .flow_cards
        .iter()
        .filter_map(|c| c.meta.as_ref())
        .filter(|m| !m.generated_at.is_empty())
        .max_by(|a, b| a.generated_at.cmp(&b.generated_at));
    if let Some(m) = latest {
        let date: String = m.generated_at.chars().take(10).collect();
        text.push_str(&format!(" · generated {date}"));
        if let Some(c) = m.commit.as_deref().filter(|c| !c.is_empty()) {
            let short: String = c.chars().take(7).collect();
            text.push_str(&format!(" @ {short}"));
        }
    }
    text
}

/// Legenda notasi flow (warna garis dan chip CRUD) plus ringkasan
/// blueprint, di pojok kiri bawah kanvas dengan tepi bawah di `bottom`.
pub fn draw_flow_legend(
    ui: &egui::Ui,
    state: &DiagramState,
    frame: &FlowFrame,
    left: f32,
    bottom: f32,
) {
    let items: [(egui::Color32, &str); 4] = [
        (READ_COLOR, "read (table to API)"),
        (WRITE_COLOR, "create / update"),
        (DELETE_COLOR, "delete"),
        (UNKNOWN_COLOR, "linked, no process yet"),
    ];
    let font = egui::FontId::proportional(11.0);
    let text_color = egui::Color32::from_gray(200);
    let hint_color = egui::Color32::from_gray(150);
    let painter = ui.painter();
    let galleys: Vec<_> = items
        .iter()
        .map(|(_, t)| painter.layout_no_wrap((*t).to_owned(), font.clone(), text_color))
        .collect();
    let hint = painter.layout_no_wrap(
        "Line chip: C create · R read · U update · D delete · numbers = steps".to_owned(),
        font.clone(),
        hint_color,
    );
    let summary = painter.layout_no_wrap(blueprint_summary(state, frame), font, text_color);
    let swatch = 16.0;
    let row_w: f32 = galleys
        .iter()
        .map(|g| swatch + 4.0 + g.size().x + 10.0)
        .sum();
    let w = row_w.max(hint.size().x).max(summary.size().x) + 12.0;
    let h = 16.0 * 3.0 + 10.0;
    let r = egui::Rect::from_min_size(egui::pos2(left + 12.0, bottom - h), egui::vec2(w, h));
    painter.rect_filled(r, 4.0, egui::Color32::from_black_alpha(170));
    let mut x = r.left() + 6.0;
    let y = r.top() + 5.0;
    for ((color, _), g) in items.iter().zip(galleys) {
        let cy = y + g.size().y / 2.0;
        painter.line_segment(
            [egui::pos2(x, cy), egui::pos2(x + swatch, cy)],
            egui::Stroke::new(2.2, *color),
        );
        x += swatch + 4.0;
        let gw = g.size().x;
        painter.galley(egui::pos2(x, y), g, text_color);
        x += gw + 10.0;
    }
    painter.galley(egui::pos2(r.left() + 6.0, y + 16.0), hint, hint_color);
    painter.galley(egui::pos2(r.left() + 6.0, y + 32.0), summary, text_color);
}

/// Mulai fokus card `i`: pilih dan fokuskan card, lalu tween viewport ke
/// card beserta semua tabelnya (margin 60 px).
pub fn focus_card(
    state: &mut DiagramState,
    frame: &FlowFrame,
    i: usize,
    view_size: egui::Vec2,
    now: f64,
) {
    let Some(card) = state.flow_cards.get(i) else {
        return;
    };
    let id = card.id.clone();
    state.selected_flow = Some(id.clone());
    state.focus_flow = Some(id);
    state.focus_table = None;
    state.focus_group = None;
    let mut bounds = frame.drawn_rect(state, i);
    for t in &frame.tables[i] {
        if let Some(n) = state.nodes.iter().find(|n| &n.id == t) {
            bounds = bounds.union(egui::Rect::from_min_size(n.pos, n.size));
        }
    }
    let zoom = crate::diagram_lod::fit_zoom(
        bounds.size(),
        (view_size - egui::vec2(120.0, 120.0)).max(egui::vec2(1.0, 1.0)),
        crate::diagram_lod::DETAIL_MIN_ZOOM,
        crate::diagram_view::FOCUS_ZOOM,
    );
    crate::diagram_view::animate_view_to(state, bounds.center(), zoom, view_size, now);
}

/// Pilih card `card_id` dan geser viewport ke card itu (dari panel endpoint
/// atau pencarian). `false` bila card tidak ada.
pub fn reveal_card(
    state: &mut DiagramState,
    card_id: &str,
    view_size: egui::Vec2,
    now: f64,
) -> bool {
    let Some(rect) = layout::card_world_rect(state, card_id) else {
        return false;
    };
    state.selected_flow = Some(card_id.to_string());
    let zoom = state.zoom.max(crate::diagram_view::FOCUS_ZOOM);
    crate::diagram_view::animate_view_to(state, rect.center(), zoom, view_size, now);
    true
}

/// Hasil interaksi flow card pada satu frame.
#[derive(Default)]
pub struct FlowCardsOutcome {
    pub action: Option<DiagramAction>,
}

#[derive(Default)]
struct CardChange {
    select: Option<String>,
    /// Buka/tutup detail langkah ke-n di card terpilih.
    toggle_step: Option<usize>,
    /// Klik badan card: putar prosesnya seperti tombol Play.
    autoplay: bool,
    drag: Option<(usize, egui::Vec2)>,
    released: bool,
    focus: Option<usize>,
    /// Menu "Focus on this endpoint": fokus tanpa memutar prosesnya.
    focus_only: Option<usize>,
    toggle_collapse: Option<usize>,
    reset_pos: Option<usize>,
    remove: Option<String>,
    highlight: Option<String>,
    show_gen_progress: bool,
}

/// Chip method di header card; mengembalikan lebarnya.
fn method_chip(painter: &egui::Painter, at: egui::Pos2, method: &str, s: f32, alpha: f32) -> f32 {
    let color = crate::http_repo::method_color(method).gamma_multiply(alpha);
    let px = quantize_font(10.5 * s);
    let galley = painter.layout_no_wrap(method.to_string(), egui::FontId::monospace(px), color);
    let size = egui::vec2(
        (galley.size().x + 10.0 * s).max(46.0 * s),
        galley.size().y + 4.0 * s,
    );
    let r = egui::Rect::from_min_size(egui::pos2(at.x, at.y - size.y / 2.0), size);
    painter.rect_filled(r, 3.0 * s, color.linear_multiply(0.18));
    painter.galley(r.center() - galley.size() / 2.0, galley, color);
    size.x
}

/// Satu baris langkah. `active` = langkah yang sedang diputar (Fase 4).
/// `chevron` = `Some(terbuka)` bila langkah punya detail yang bisa dibuka.
#[allow(clippy::too_many_arguments)]
fn draw_step_row(
    painter: &egui::Painter,
    row: egui::Rect,
    index: usize,
    step: &FlowStep,
    target: Option<String>,
    s: f32,
    text: egui::Color32,
    active: bool,
    hovered: bool,
    chevron: Option<bool>,
) {
    let (icon, kind_color) = step_kind_style(step.kind);
    if active {
        painter.rect_filled(row, 0.0, kind_color.linear_multiply(0.22));
    } else if hovered {
        painter.rect_filled(row, 0.0, text.linear_multiply(0.06));
    }
    let cy = row.center().y;
    let mut x = row.left() + 10.0 * s;
    // Nomor langkah di lingkaran berwarna jenis langkah.
    let radius = 8.0 * s;
    painter.circle_filled(
        egui::pos2(x + radius, cy),
        radius,
        kind_color.linear_multiply(0.85),
    );
    painter.text(
        egui::pos2(x + radius, cy),
        egui::Align2::CENTER_CENTER,
        (index + 1).to_string(),
        egui::FontId::proportional(quantize_font(9.5 * s)),
        egui::Color32::from_rgb(20, 20, 20),
    );
    x += radius * 2.0 + 6.0 * s;
    painter.text(
        egui::pos2(x, cy),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(quantize_font(12.0 * s)),
        kind_color,
    );
    x += 18.0 * s;
    let px = quantize_font(11.5 * s);
    let mut right = row.right() - 8.0 * s;
    if let Some(open) = chevron {
        let icon = if open {
            egui_icons::icons::ICON_EXPAND_LESS.codepoint
        } else {
            egui_icons::icons::ICON_EXPAND_MORE.codepoint
        };
        painter.text(
            egui::pos2(right, cy),
            egui::Align2::RIGHT_CENTER,
            icon,
            egui::FontId::proportional(quantize_font(13.0 * s)),
            text.gamma_multiply(0.6),
        );
        right -= 16.0 * s;
    }
    if let Some(t) = target {
        let color = op_color(step.op);
        let mono = quantize_font(10.5 * s);
        let label = truncate_for_width(&t, 100.0 * s, mono);
        let g = painter.layout_no_wrap(label, egui::FontId::monospace(mono), color);
        right -= g.size().x;
        painter.galley(egui::pos2(right, cy - g.size().y / 2.0), g, color);
        right -= 8.0 * s;
    }
    let title = if step.condition.is_some() {
        format!("{} (conditional)", step.title)
    } else {
        step.title.clone()
    };
    painter.text(
        egui::pos2(x, cy),
        egui::Align2::LEFT_CENTER,
        truncate_for_width(&title, (right - x).max(0.0), px),
        egui::FontId::proportional(px),
        text,
    );
}

/// Blok detail langkah yang dibuka, tepat di bawah barisnya (`row`, layar).
fn draw_step_detail(
    painter: &egui::Painter,
    row: egui::Rect,
    step: &FlowStep,
    s: f32,
    text: egui::Color32,
    accent: egui::Color32,
) {
    let lines = layout::step_detail_lines(step);
    let h = layout::step_detail_height(step) * s;
    let block = egui::Rect::from_min_size(row.left_bottom(), egui::vec2(row.width(), h));
    painter.rect_filled(block, 0.0, accent.linear_multiply(0.08));
    // Garis tepi kiri sejajar lingkaran nomor langkah.
    let x = row.left() + 18.0 * s;
    painter.line_segment(
        [
            egui::pos2(x, block.top() + 2.0 * s),
            egui::pos2(x, block.bottom() - 2.0 * s),
        ],
        egui::Stroke::new(1.5 * s.max(0.5), accent.linear_multiply(0.6)),
    );
    let px = quantize_font(10.5 * s);
    let left = row.left() + 30.0 * s;
    let width = row.right() - 8.0 * s - left;
    let mut y = block.top() + layout::STEP_DETAIL_PAD * s / 2.0;
    for line in lines {
        let mono = line.starts_with("Source: ") || line.starts_with("Columns: ");
        let font = if mono {
            egui::FontId::monospace(px)
        } else {
            egui::FontId::proportional(px)
        };
        painter.text(
            egui::pos2(left, y + layout::STEP_DETAIL_H * s / 2.0),
            egui::Align2::LEFT_CENTER,
            truncate_for_width(&line, width.max(0.0), px),
            font,
            text,
        );
        y += layout::STEP_DETAIL_H * s;
    }
}

/// Gambar flow card yang terlihat dan tangani interaksinya. `interactive`
/// = false pada mode Hand Tool. `focus_tables` = tabel fokus tabel/group
/// yang sedang aktif; card yang tidak menyentuhnya diredupkan.
#[allow(clippy::too_many_arguments)]
pub fn render_flow_cards(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    frame: &FlowFrame,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    canvas: egui::Rect,
    interactive: bool,
    focus_tables: Option<&HashSet<String>>,
    now: f64,
) -> FlowCardsOutcome {
    let mut out = FlowCardsOutcome::default();
    if state.flow_cards.is_empty() {
        return out;
    }
    let mut change = CardChange::default();
    let clip = ui.clip_rect();
    let s = state.zoom;
    let lod = lod_for_zoom(s);
    let read_only = state.scoped_to.is_some();
    let visuals = ui.visuals().clone();
    let accent = crate::window_egui::style::theme_accent(ui.ctx());
    let strong = visuals.strong_text_color();
    let weak = visuals.weak_text_color();
    let separator = visuals.widgets.noninteractive.bg_stroke.color;
    let mut hover_table: Option<(String, egui::Color32)> = None;
    let gen_running = crate::diagram_flow_gen_view::is_running(state);

    for i in draw_order(state) {
        let card = &state.flow_cards[i];
        let world = frame.drawn_rect(state, i);
        let sr = screen_rect(layout::lod_rect(world, lod), to_screen);
        if !clip.intersects(sr) {
            continue;
        }
        let selected = state.selected_flow.as_deref() == Some(card.id.as_str());
        let dimmed = match (&state.focus_flow, focus_tables) {
            (Some(f), _) => f != &card.id,
            (None, Some(set)) => !frame.tables[i].iter().any(|t| set.contains(t)),
            (None, None) => false,
        };
        let alpha = if dimmed { DIM_CARD } else { 1.0 };
        let mcolor = crate::http_repo::method_color(&card.trigger.method);
        let painter = ui.painter().clone();
        let base_id = ui.id().with(("diagram_flow_card", &card.id));
        let label = format!("{} {}", card.trigger.method, card.trigger.target);

        // Interaksi badan card didaftarkan dulu supaya baris langkah di atasnya.
        let sense = match (interactive, read_only) {
            (false, _) => egui::Sense::hover(),
            (true, true) => egui::Sense::click(),
            (true, false) => egui::Sense::click_and_drag(),
        };
        let resp = ui.interact(sr, base_id.with("body"), sense);
        if resp.clicked() || resp.drag_started() {
            change.select = Some(card.id.clone());
        }
        if resp.clicked() {
            change.autoplay = true;
        }
        if resp.dragged() {
            change.drag = Some((i, resp.drag_delta() / s));
        }
        if resp.drag_stopped() {
            change.released = true;
        }
        if resp.double_clicked() {
            change.focus = Some(i);
        }
        if interactive {
            resp.context_menu(|ui| {
                if ui
                    .button(format!(
                        "{} Focus on this endpoint",
                        egui_icons::icons::ICON_FILTER_CENTER_FOCUS.codepoint
                    ))
                    .on_hover_text("Dim everything except this endpoint and its linked tables")
                    .clicked()
                {
                    ui.close();
                    change.focus_only = Some(i);
                }
                ui.separator();
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
                        label: label.clone(),
                    });
                }
                // Ciutkan per card hanya berarti saat langkah semua card tampil.
                if state.flow_show_steps {
                    let (icon, text) = if card.collapsed {
                        (egui_icons::icons::ICON_UNFOLD_MORE.codepoint, "Expand")
                    } else {
                        (egui_icons::icons::ICON_UNFOLD_LESS.codepoint, "Collapse")
                    };
                    if ui.button(format!("{icon} {text}")).clicked() {
                        ui.close();
                        change.toggle_collapse = Some(i);
                    }
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
                if !read_only {
                    let generated = card.meta.is_some();
                    match crate::diagram_flow_gen_view::generate_menu_item(
                        ui,
                        gen_running,
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
                            change.show_gen_progress = true;
                        }
                        None => {}
                    }
                }
                if !read_only {
                    if ui
                        .add_enabled(
                            card.pos.is_some(),
                            egui::Button::new(format!(
                                "{} Reset Position",
                                egui_icons::icons::ICON_RESTART_ALT.codepoint
                            )),
                        )
                        .on_hover_text("Put the card back in its API band above its tables")
                        .clicked()
                    {
                        ui.close();
                        change.reset_pos = Some(i);
                    }
                    ui.separator();
                    if ui
                        .button(format!(
                            "{} Remove Card",
                            egui_icons::icons::ICON_DELETE.codepoint
                        ))
                        .on_hover_text("Remove this card and unlink the endpoint from its tables")
                        .clicked()
                    {
                        ui.close();
                        change.remove = Some(card.id.clone());
                    }
                }
            });
        }

        match lod {
            Lod::Overview => {
                let r = (sr.width() / 2.0).max(3.0);
                painter.circle_filled(sr.center(), r, mcolor.gamma_multiply(alpha));
                if selected {
                    painter.circle_stroke(sr.center(), r + 2.0, egui::Stroke::new(1.5, strong));
                }
                resp.on_hover_text(&label);
                continue;
            }
            Lod::Compact => {
                let radius = sr.height() / 2.0;
                painter.rect_filled(sr, radius, visuals.window_fill.gamma_multiply(alpha));
                let stroke = if selected {
                    egui::Stroke::new(2.5, mcolor)
                } else {
                    egui::Stroke::new(1.0, mcolor.gamma_multiply(0.8 * alpha))
                };
                painter.rect_stroke(sr, radius, stroke, egui::StrokeKind::Middle);
                let px = quantize_font((13.0 * s).clamp(6.0, 13.0));
                let text = format!("{label} · {}", card.steps.len());
                painter.text(
                    sr.left_center() + egui::vec2(radius, 0.0),
                    egui::Align2::LEFT_CENTER,
                    truncate_for_width(&text, sr.width() - radius * 2.0, px),
                    egui::FontId::monospace(px),
                    mcolor.gamma_multiply(alpha),
                );
                resp.on_hover_text(&label);
                continue;
            }
            Lod::Detail => {}
        }

        // Card penuh.
        let radius = 8.0 * s;
        painter.rect_filled(
            sr.translate(egui::vec2(2.0, 4.0) * s),
            radius,
            egui::Color32::from_black_alpha((80.0 * alpha) as u8),
        );
        painter.rect_filled(sr, radius, visuals.window_fill.gamma_multiply(alpha));
        // Card terpilih: bingkai warna method yang lebih tebal plus glow
        // tipis. Warna aksen tema tidak dipakai karena bisa merah (= DELETE).
        if selected {
            painter.rect_stroke(
                sr.expand(3.0 * s.max(0.6)),
                radius + 3.0 * s,
                egui::Stroke::new(4.0 * s.max(0.6), mcolor.gamma_multiply(0.25)),
                egui::StrokeKind::Middle,
            );
        }
        let stroke = if selected {
            egui::Stroke::new(2.5 * s.max(0.6), mcolor)
        } else if resp.hovered() {
            egui::Stroke::new(1.5 * s.max(0.6), mcolor)
        } else {
            egui::Stroke::new(1.0 * s.max(0.5), mcolor.gamma_multiply(0.6 * alpha))
        };
        painter.rect_stroke(sr, radius, stroke, egui::StrokeKind::Middle);
        let text = strong.gamma_multiply(alpha);
        let weak_text = weak.gamma_multiply(alpha);

        // Header: chip method, path, jumlah langkah, label "partial".
        let header =
            egui::Rect::from_min_size(sr.min, egui::vec2(sr.width(), layout::CARD_HEADER_H * s));
        let hy = header.center().y;
        let chip_w = method_chip(
            &painter,
            egui::pos2(header.left() + 10.0 * s, hy),
            &card.trigger.method,
            s,
            alpha,
        );
        let mut right = header.right() - 10.0 * s;
        let small = quantize_font(10.0 * s);
        let mut tags: Vec<(String, egui::Color32)> = Vec::new();
        if card.meta.as_ref().is_some_and(|m| m.partial) {
            tags.push(("partial".into(), egui::Color32::from_rgb(255, 167, 38)));
        }
        let body = layout::shows_body(state, card);
        if !card.steps.is_empty() {
            tags.push((format!("{} steps", card.steps.len()), weak_text));
        } else if !body {
            tags.push(("no steps yet".into(), weak_text));
        }
        for (t, c) in tags.into_iter().rev() {
            let g = painter.layout_no_wrap(t, egui::FontId::proportional(small), c);
            right -= g.size().x;
            painter.galley(egui::pos2(right, hy - g.size().y / 2.0), g, c);
            right -= 8.0 * s;
        }
        let path_x = header.left() + 10.0 * s + chip_w + 8.0 * s;
        let path_px = quantize_font(12.0 * s);
        painter.text(
            egui::pos2(path_x, hy),
            egui::Align2::LEFT_CENTER,
            truncate_for_width(&card.trigger.target, (right - path_x).max(0.0), path_px),
            egui::FontId::monospace(path_px),
            text,
        );
        if !body {
            continue;
        }
        painter.line_segment(
            [
                egui::pos2(sr.left() + 1.0, header.bottom()),
                egui::pos2(sr.right() - 1.0, header.bottom()),
            ],
            egui::Stroke::new(1.0, separator.gamma_multiply(alpha)),
        );

        let body_px = quantize_font(11.5 * s);
        let mut y = header.bottom();
        let summary_h = layout::summary_height(card) * s;
        if summary_h > 0.0 {
            painter.text(
                egui::pos2(sr.left() + 10.0 * s, y + summary_h / 2.0),
                egui::Align2::LEFT_CENTER,
                truncate_for_width(&card.summary, sr.width() - 20.0 * s, body_px),
                egui::FontId::proportional(body_px),
                weak_text,
            );
            y += summary_h;
        }

        if card.steps.is_empty() {
            let n = frame.tables[i].len();
            let t = if n == 0 {
                "No business process yet".to_string()
            } else {
                format!("No business process yet · {n} table(s)")
            };
            painter.text(
                egui::pos2(sr.left() + 10.0 * s, y + layout::STEP_ROW_H * s / 2.0),
                egui::Align2::LEFT_CENTER,
                truncate_for_width(&t, sr.width() - 20.0 * s, body_px),
                egui::FontId::proportional(body_px),
                weak_text,
            );
            continue;
        }

        let (shown, hidden) = layout::step_rows(card, selected);
        let playing = crate::diagram_flow_play_view::active_step(state, card);
        let open = layout::open_step(state, card);
        for (si, step) in card.steps.iter().enumerate().take(shown) {
            let Some(row_world) = layout::step_row_rect(world, card, si, selected, open) else {
                continue;
            };
            let row = screen_rect(row_world, to_screen);
            let is_open = open.is_some_and(|(o, _)| o == si);
            if is_open {
                draw_step_detail(&painter, row, step, s, weak_text, op_color(step.op));
            }
            if !clip.intersects(row) {
                continue;
            }
            // Langkah dengan detail dibuka/ditutup di tempat (hanya card terpilih).
            let expandable = selected && layout::step_detail_height(step) > 0.0;
            let row_resp = interactive.then(|| {
                let r = ui.interact(row, base_id.with(("step", si)), egui::Sense::click());
                if expandable {
                    r.on_hover_text(if is_open {
                        "Click to hide the details"
                    } else {
                        "Click to show the details"
                    })
                } else {
                    r.on_hover_text(step_tooltip(state, si, step))
                }
            });
            let hovered = row_resp.as_ref().is_some_and(|r| r.hovered());
            let table = step.target.as_ref().and_then(|t| t.table());
            if let Some(r) = &row_resp {
                if hovered && let Some(t) = table {
                    hover_table = Some((t.to_string(), op_color(step.op)));
                }
                if r.clicked() {
                    change.select = Some(card.id.clone());
                    change.highlight = table.map(str::to_string);
                    if expandable {
                        change.toggle_step = Some(si);
                    }
                }
                if r.double_clicked() {
                    change.focus = Some(i);
                }
            }
            draw_step_row(
                &painter,
                row,
                si,
                step,
                target_label(state, step),
                s,
                text,
                playing == Some(si),
                hovered,
                expandable.then_some(is_open),
            );
        }
        if hidden > 0 {
            let top = y + shown as f32 * layout::STEP_ROW_H * s;
            painter.text(
                egui::pos2(sr.left() + 10.0 * s, top + layout::STEP_ROW_H * s / 2.0),
                egui::Align2::LEFT_CENTER,
                format!("+{hidden} more · select the card to show all"),
                egui::FontId::proportional(body_px),
                weak_text,
            );
        }
    }

    // Sorotan tabel: target baris langkah yang di-hover, atau yang baru diklik.
    if let Some(t) = change.highlight.take() {
        ui.data_mut(|d| d.insert_temp(highlight_id(ui), (t, now)));
    }
    let clicked = ui
        .data(|d| d.get_temp::<(String, f64)>(highlight_id(ui)))
        .filter(|(_, t0)| now - t0 < STEP_HIGHLIGHT_SECS);
    if clicked.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    let targets = hover_table
        .into_iter()
        .chain(clicked.map(|(t, _)| (t, accent)));
    for (table, color) in targets {
        if let Some(n) = state.nodes.iter().find(|n| n.id == table) {
            let r = screen_rect(egui::Rect::from_min_size(n.pos, n.size), to_screen).expand(3.0);
            ui.painter().rect_stroke(
                r,
                6.0 * s,
                egui::Stroke::new(2.5, color),
                egui::StrokeKind::Outside,
            );
        }
    }

    apply_change(state, frame, change, canvas.size(), now);
    out
}

fn apply_change(
    state: &mut DiagramState,
    frame: &FlowFrame,
    change: CardChange,
    view_size: egui::Vec2,
    now: f64,
) {
    if let Some(id) = change.select {
        if state.focus_flow.as_ref().is_some_and(|f| f != &id) {
            state.focus_flow = None;
        }
        // Detail langkah yang dibuka milik card terpilih sebelumnya.
        if state.selected_flow.as_ref() != Some(&id) {
            state.flow_open_step = None;
        }
        if let Some(si) = change.toggle_step {
            state.flow_open_step = (state.flow_open_step != Some(si)).then_some(si);
        }
        // Klik card langsung memutar prosesnya (sama dengan tombol Play),
        // kecuali card itu sedang diputar.
        let playing = state
            .flow_play
            .as_ref()
            .is_some_and(|p| p.card_id == id && p.playing);
        if change.autoplay && !playing {
            crate::diagram_flow_play_view::start_playback(state, &id);
            state.focus_flow = Some(id.clone());
            state.focus_table = None;
            state.focus_group = None;
        }
        state.selected_flow = Some(id);
    }
    if let Some((i, delta)) = change.drag
        && let Some(card) = state.flow_cards.get_mut(i)
    {
        let min = frame.rects[i].min;
        let [x, y] = card.pos.unwrap_or([min.x, min.y]);
        card.pos = Some([x + delta.x, y + delta.y]);
    }
    if change.released {
        state.save_requested = true;
    }
    if let Some(i) = change.toggle_collapse
        && let Some(card) = state.flow_cards.get_mut(i)
    {
        card.collapsed = !card.collapsed;
        state.save_requested = true;
    }
    if let Some(i) = change.reset_pos
        && let Some(card) = state.flow_cards.get_mut(i)
    {
        card.pos = None;
        state.save_requested = true;
    }
    if let Some(i) = change.focus {
        focus_card(state, frame, i, view_size, now);
        if let Some(id) = state.flow_cards.get(i).map(|c| c.id.clone()) {
            crate::diagram_flow_play_view::start_playback(state, &id);
        }
    }
    if let Some(i) = change.focus_only {
        state.flow_play = None;
        focus_card(state, frame, i, view_size, now);
    }
    if let Some(id) = change.remove {
        crate::diagram_flow::remove_card(state, &id);
        state.save_requested = true;
    }
    if change.show_gen_progress {
        crate::diagram_flow_gen_view::show_progress(state);
    }
}

/// Kembalikan semua card ke pita API otomatis (menu "Arrange API cards" dan
/// "Auto Arrange").
pub fn arrange_all(state: &mut DiagramState) {
    for c in &mut state.flow_cards {
        c.pos = None;
    }
    state.save_requested = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::FlowMeta;

    #[test]
    fn crud_letters_follow_c_r_u_d_order() {
        assert_eq!(crud_letters(&[FlowOp::Delete, FlowOp::Read]), "RD");
        assert_eq!(crud_letters(&[FlowOp::Upsert]), "CU");
        assert_eq!(crud_letters(&[FlowOp::Call]), "");
        let u = TableUse {
            table: "t".into(),
            ops: vec![FlowOp::Insert],
            steps: vec![0, 2],
            column: None,
        };
        assert_eq!(chip_text(&u), "C · 1, 3");
    }

    #[test]
    fn blueprint_summary_counts_cards_tables_and_latest_generate() {
        let mut st = DiagramState::default();
        let mut a = FlowCard {
            id: "a".into(),
            steps: vec![FlowStep::default()],
            ..Default::default()
        };
        a.meta = Some(FlowMeta {
            commit: Some("0123456789abcdef".into()),
            generated_at: "2026-09-30T10:00:00Z".into(),
            ..Default::default()
        });
        let mut b = a.clone();
        b.id = "b".into();
        b.steps.clear();
        b.meta.as_mut().unwrap().generated_at = "2026-09-01T10:00:00Z".into();
        st.flow_cards = vec![a, b];
        let frame = FlowFrame {
            tables: vec![vec!["t1".into(), "t2".into()], vec!["t2".into()]],
            ..Default::default()
        };
        assert_eq!(
            blueprint_summary(&st, &frame),
            "2 endpoint(s) · 1 with process · 2 table(s) touched · generated 2026-09-30 @ 0123456"
        );
    }
}
