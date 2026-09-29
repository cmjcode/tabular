//! Renderer egui untuk diagram query: kartu per tahap, garis alur yang
//! tumbuh lalu dialiri partikel, glow berdenyut di kartu hasil/target, dan
//! animasi khusus per jenis statement (chip nilai lama/baru untuk UPDATE,
//! baris baru untuk INSERT, baris dicoret untuk DELETE).
//!
//! Hanya bergantung pada egui (tanpa `window_egui`).

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2,
    vec2,
};

use super::StatementKind;
use super::clip;
use super::layout::{CARD_PAD, CardRole, FlowKind, HEADER_H, QueryLayout, ROW_H, RowState};

/// Lama animasi berjalan sebelum berhenti sendiri (detik). Setelah itu
/// diagram statis supaya tidak menggambar ulang terus-menerus.
pub const ANIM_SECS: f64 = 30.0;
const MIN_ZOOM: f32 = 0.3;
const MAX_ZOOM: f32 = 2.0;
/// Jeda muncul antar tahap (lane) dan lama fade kartu.
const STAGE_DELAY: f64 = 0.45;
const CARD_FADE: f64 = 0.35;
const FLOW_GROW: f64 = 0.55;
const CURVE_SEGMENTS: usize = 28;
/// Tinggi strip legenda di bawah kanvas.
const LEGEND_H: f32 = 24.0;

/// State tampilan per panel (pan/zoom/waktu mulai animasi).
#[derive(Debug, Clone)]
pub struct QueryDiagramView {
    pub pan: Vec2,
    pub zoom: f32,
    start_time: Option<f64>,
    fitted: bool,
    /// Geseran tiap kartu dari posisi layout (koordinat diagram), hasil drag user.
    offsets: Vec<Vec2>,
    /// Kartu yang sedang di-drag.
    dragging: Option<usize>,
}

impl Default for QueryDiagramView {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            start_time: None,
            fitted: false,
            offsets: Vec::new(),
            dragging: None,
        }
    }
}

impl QueryDiagramView {
    /// Putar ulang animasi dari awal.
    pub fn replay(&mut self) {
        self.start_time = None;
    }

    /// Paskan diagram ke area panel pada frame berikutnya.
    pub fn fit(&mut self) {
        self.fitted = false;
    }

    /// Kembalikan semua kartu ke posisi layout awal.
    pub fn reset_positions(&mut self) {
        self.offsets.clear();
        self.fitted = false;
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.start_time.is_none_or(|s| now - s < ANIM_SECS)
    }
}

struct Palette {
    dark: bool,
    bg: Color32,
    grid: Color32,
    card: Color32,
    border: Color32,
    text: Color32,
    weak: Color32,
    join: Color32,
    data: Color32,
    filter: Color32,
    aggregate: Color32,
    stage: Color32,
    kind: Color32,
}

impl Palette {
    fn new(dark: bool, kind: StatementKind) -> Self {
        let kind_color = match kind {
            StatementKind::Select => Color32::from_rgb(170, 110, 255),
            StatementKind::Insert => Color32::from_rgb(60, 200, 120),
            StatementKind::Update => Color32::from_rgb(255, 170, 40),
            StatementKind::Delete => Color32::from_rgb(240, 85, 85),
        };
        if dark {
            Self {
                dark,
                bg: Color32::from_rgb(22, 24, 30),
                grid: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
                card: Color32::from_rgb(36, 39, 48),
                border: Color32::from_rgb(64, 68, 82),
                text: Color32::from_rgb(230, 232, 238),
                weak: Color32::from_rgb(150, 155, 170),
                join: Color32::from_rgb(80, 200, 255),
                data: Color32::from_rgb(90, 225, 150),
                filter: Color32::from_rgb(255, 160, 60),
                aggregate: Color32::from_rgb(240, 110, 180),
                stage: Color32::from_rgb(175, 165, 235),
                kind: kind_color,
            }
        } else {
            Self {
                dark,
                bg: Color32::from_rgb(246, 247, 250),
                grid: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
                card: Color32::WHITE,
                border: Color32::from_rgb(210, 214, 224),
                text: Color32::from_rgb(30, 33, 40),
                weak: Color32::from_rgb(110, 115, 130),
                join: Color32::from_rgb(0, 140, 210),
                data: Color32::from_rgb(20, 160, 90),
                filter: Color32::from_rgb(220, 120, 20),
                aggregate: Color32::from_rgb(200, 60, 140),
                stage: Color32::from_rgb(110, 95, 190),
                kind: kind_color,
            }
        }
    }

    fn accent(&self, role: CardRole) -> Color32 {
        match role {
            CardRole::Source => Color32::from_rgb(90, 150, 255),
            CardRole::Clauses => self.filter,
            CardRole::Group => self.aggregate,
            CardRole::Having => Color32::from_rgb(255, 120, 90),
            CardRole::Sort => Color32::from_rgb(110, 170, 210),
            CardRole::Window => Color32::from_rgb(60, 200, 200),
            CardRole::Union => Color32::from_rgb(200, 150, 255),
            CardRole::Values => Color32::from_rgb(40, 190, 170),
            CardRole::Result => Color32::from_rgb(170, 110, 255),
            CardRole::Set => Color32::from_rgb(255, 190, 60),
            CardRole::Target => self.kind,
        }
    }

    /// Warna teks di atas latar aksen: di mode terang digelapkan supaya
    /// kuning/hijau muda tetap terbaca.
    fn ink(&self, c: Color32) -> Color32 {
        if self.dark {
            c
        } else {
            lerp_color(c, Color32::BLACK, 0.4)
        }
    }

    fn flow(&self, kind: FlowKind) -> Color32 {
        match kind {
            FlowKind::Join => self.join,
            FlowKind::Data => self.data,
            FlowKind::Filter => self.filter,
            FlowKind::Stage => self.stage,
        }
    }
}

fn smooth(x: f64) -> f32 {
    let x = x.clamp(0.0, 1.0);
    (x * x * (3.0 - 2.0 * x)) as f32
}

fn bezier(p: &[Pos2; 4], t: f32) -> Pos2 {
    let u = 1.0 - t;
    let a = u * u * u;
    let b = 3.0 * u * u * t;
    let c = 3.0 * u * t * t;
    let d = t * t * t;
    pos2(
        a * p[0].x + b * p[1].x + c * p[2].x + d * p[3].x,
        a * p[0].y + b * p[1].y + c * p[2].y + d * p[3].y,
    )
}

fn radius(v: f32) -> CornerRadius {
    CornerRadius::same(v.clamp(0.0, 255.0).round() as u8)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

type Endpoint = (usize, Option<usize>);

/// Kurva alur di koordinat diagram. Arah ditentukan dari posisi kartu, jadi
/// tetap rapi setelah kartu digeser: sisi ke sisi bila berjauhan, lengkung
/// lewat kiri bila bertumpuk atas-bawah.
fn flow_curve(layout: &QueryLayout, from: Endpoint, to: Endpoint) -> [Pos2; 4] {
    let a = &layout.cards[from.0];
    let b = &layout.cards[to.0];
    if super::layout::stacked(a.rect, b.rect) {
        let s = a.anchor(from.1, false);
        let e = b.anchor(to.1, false);
        let bend = ((e.y - s.y).abs() * 0.25).clamp(30.0, super::layout::SAME_LANE_BEND);
        let x = s.x.min(e.x) - bend;
        [s, pos2(x, s.y), pos2(x, e.y), e]
    } else if b.rect.min.x >= a.rect.max.x {
        let s = a.anchor(from.1, true);
        let e = b.anchor(to.1, false);
        let d = ((e.x - s.x).abs() * 0.5).max(40.0);
        [s, s + vec2(d, 0.0), e - vec2(d, 0.0), e]
    } else {
        let s = a.anchor(from.1, false);
        let e = b.anchor(to.1, true);
        let d = ((e.x - s.x).abs() * 0.5).max(40.0);
        [s, s - vec2(d, 0.0), e + vec2(d, 0.0), e]
    }
}

/// Alur yang terhubung ke baris yang sedang di-hover: hulu dan hilir.
fn lineage(layout: &QueryLayout, hovered: Endpoint) -> Vec<bool> {
    let mut marked = vec![false; layout.flows.len()];
    for forward in [true, false] {
        let mut frontier = vec![hovered];
        while let Some(node) = frontier.pop() {
            for (i, f) in layout.flows.iter().enumerate() {
                let (near, far) = if forward {
                    (f.from, f.to)
                } else {
                    (f.to, f.from)
                };
                if near == node && !marked[i] {
                    marked[i] = true;
                    if f.kind == FlowKind::Data {
                        frontier.push(far);
                    }
                }
            }
        }
    }
    marked
}

/// Gambar diagram query mengisi seluruh ruang `ui` yang tersisa.
/// Layout dengan posisi kartu yang sudah digeser user.
fn moved_layout(layout: &QueryLayout, offsets: &[Vec2]) -> QueryLayout {
    let mut moved = layout.clone();
    for (card, off) in moved.cards.iter_mut().zip(offsets) {
        card.rect = card.rect.translate(*off);
    }
    moved.bounds = moved
        .cards
        .iter()
        .fold(layout.bounds, |b, c| b.union(c.rect));
    moved
}

/// Gambar diagram query mengisi seluruh ruang `ui` yang tersisa. Kartu bisa
/// digeser (drag), area kosong menggeser kanvas. Mengembalikan area kanvas.
pub fn render_query_diagram(
    ui: &mut egui::Ui,
    layout: &QueryLayout,
    view: &mut QueryDiagramView,
) -> Rect {
    let size = ui.available_size().max(vec2(120.0, 120.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    view.offsets.resize(layout.cards.len(), Vec2::ZERO);
    let moved = moved_layout(layout, &view.offsets);
    let layout = &moved;
    let now = ui.input(|i| i.time);
    let start = *view.start_time.get_or_insert(now);
    let t = (now - start).max(0.0);
    let animating = t < ANIM_SECS;
    let pal = Palette::new(ui.visuals().dark_mode, layout.kind);

    // --- Interaksi ---
    if !view.fitted || response.double_clicked() {
        let right = if layout.kind == StatementKind::Update {
            170.0
        } else {
            30.0
        };
        fit(view, layout.bounds, rect, right);
        view.fitted = true;
    }
    let card_at = |p: Pos2, view: &QueryDiagramView| {
        let d = Pos2::ZERO + (p - rect.min - view.pan) / view.zoom;
        // Kartu yang digambar terakhir ada di atas.
        layout.cards.iter().rposition(|c| c.rect.contains(d))
    };
    if response.drag_started() {
        view.dragging = response
            .interact_pointer_pos()
            .and_then(|p| card_at(p, view));
    }
    if response.dragged() {
        match view.dragging {
            Some(ci) => view.offsets[ci] += response.drag_delta() / view.zoom,
            None => view.pan += response.drag_delta(),
        }
    }
    if response.drag_stopped() {
        view.dragging = None;
    }
    let pointer = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p) && (response.hovered() || response.dragged()));
    if view.dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if let Some(p) = pointer
        && card_at(p, view).is_some()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if response.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let mut z = view.zoom;
        if scroll != 0.0 {
            z *= (1.0 + scroll * 0.0015).clamp(0.85, 1.15);
        }
        if pinch != 1.0 {
            z *= pinch;
        }
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        if (z - view.zoom).abs() > f32::EPSILON {
            if let Some(p) = pointer {
                let anchor = p - rect.min;
                view.pan = anchor - (anchor - view.pan) * (z / view.zoom);
            }
            view.zoom = z;
        }
    }

    let zoom = view.zoom;
    let origin = rect.min + view.pan;
    let to_screen = |p: Pos2| origin + p.to_vec2() * zoom;
    let from_screen = |p: Pos2| Pos2::ZERO + (p - origin) / zoom;
    let painter = ui.painter_at(rect);

    // --- Latar ---
    painter.rect_filled(rect, radius(6.0), pal.bg);
    let step = 24.0 * zoom;
    if step >= 8.0 {
        let mut x = rect.min.x + view.pan.x.rem_euclid(step);
        while x < rect.max.x {
            let mut y = rect.min.y + view.pan.y.rem_euclid(step);
            while y < rect.max.y {
                painter.circle_filled(pos2(x, y), 0.9, pal.grid);
                y += step;
            }
            x += step;
        }
    }

    // --- Timeline per tahap ---
    let mut lanes: Vec<usize> = layout.cards.iter().map(|c| c.lane).collect();
    lanes.sort_unstable();
    lanes.dedup();
    let lane_delay =
        |lane: usize| lanes.iter().position(|l| *l == lane).unwrap_or(0) as f64 * STAGE_DELAY;

    // --- Hover ---
    let hovered: Option<Endpoint> = pointer.and_then(|p| {
        let d = from_screen(p);
        layout.cards.iter().enumerate().find_map(|(ci, c)| {
            if !c.rect.contains(d) {
                return None;
            }
            let rel = d.y - c.rect.min.y - HEADER_H - CARD_PAD;
            let row = (rel >= 0.0)
                .then(|| (rel / ROW_H) as usize)
                .filter(|r| *r < c.rows.len());
            Some((ci, row))
        })
    });
    let highlighted = hovered
        .filter(|h| h.1.is_some())
        .map(|h| lineage(layout, h));

    // --- Alur ---
    let line_w = (1.8 * zoom).max(1.0);
    // Garis pipeline digambar paling bawah, di belakang garis kolom.
    let mut order: Vec<usize> = (0..layout.flows.len()).collect();
    order.sort_by_key(|i| layout.flows[*i].kind != FlowKind::Stage);
    for fi in order {
        let f = &layout.flows[fi];
        let appear = lane_delay(layout.cards[f.from.0].lane)
            .max(lane_delay(layout.cards[f.to.0].lane))
            + CARD_FADE;
        let progress = smooth((t - appear) / FLOW_GROW);
        if progress <= 0.0 {
            continue;
        }
        let dim = highlighted.as_ref().is_some_and(|m| !m[fi]);
        let strong = highlighted.as_ref().is_some_and(|m| m[fi]);
        let mut color = pal.flow(f.kind);
        if dim {
            color = color.gamma_multiply(0.18);
        }
        let curve = flow_curve(layout, f.from, f.to).map(to_screen);
        let pts: Vec<Pos2> = (0..=CURVE_SEGMENTS)
            .map(|i| bezier(&curve, progress * i as f32 / CURVE_SEGMENTS as f32))
            .collect();
        let base = if f.kind == FlowKind::Stage {
            line_w * 2.2
        } else {
            line_w
        };
        let width = if strong { base * 1.6 } else { base };
        if f.kind == FlowKind::Filter {
            painter.extend(Shape::dashed_line(
                &pts,
                Stroke::new(width, color),
                6.0 * zoom,
                4.0 * zoom,
            ));
        } else {
            painter.add(Shape::line(
                pts.clone(),
                Stroke::new(width * 3.2, color.gamma_multiply(0.14)),
            ));
            painter.add(Shape::line(pts.clone(), Stroke::new(width, color)));
        }
        // Kepala panah di ujung tujuan.
        if progress >= 1.0 && f.kind != FlowKind::Join && pts.len() >= 2 {
            let tip = pts[pts.len() - 1];
            let dir = (tip - pts[pts.len() - 2]).normalized();
            let n = vec2(-dir.y, dir.x);
            let s = 6.0 * zoom.max(0.5);
            painter.add(Shape::convex_polygon(
                vec![
                    tip,
                    tip - dir * s * 1.4 + n * s * 0.7,
                    tip - dir * s * 1.4 - n * s * 0.7,
                ],
                color,
                Stroke::NONE,
            ));
        }
        // Partikel data mengalir.
        if progress >= 1.0 && animating && !dim {
            let count = match f.kind {
                FlowKind::Filter => 1,
                FlowKind::Stage => 2,
                _ => 3,
            };
            for k in 0..count {
                let u = (t * 0.5 + k as f64 / count as f64 + fi as f64 * 0.137).fract() as f32;
                let p = bezier(&curve, u);
                painter.circle_filled(p, 6.0 * zoom, color.gamma_multiply(0.22));
                painter.circle_filled(p, 2.8 * zoom, color);
            }
        }
        // Label jenis join.
        if let Some(label) = &f.label
            && progress >= 1.0
            && zoom >= 0.55
            && !dim
        {
            let mid = bezier(&curve, 0.5);
            let galley = painter.layout_no_wrap(
                label.clone(),
                FontId::proportional(10.0 * zoom),
                pal.ink(color),
            );
            let r = Rect::from_center_size(mid, galley.size() + vec2(10.0, 4.0) * zoom);
            painter.rect(
                r,
                radius(8.0 * zoom),
                pal.bg,
                Stroke::new(1.0, color.gamma_multiply(0.6)),
                StrokeKind::Inside,
            );
            painter.galley(r.center() - galley.size() * 0.5, galley, pal.ink(color));
        }
    }

    // --- Kartu ---
    for (ci, card) in layout.cards.iter().enumerate() {
        let appear = lane_delay(card.lane);
        let alpha = smooth((t - appear) / CARD_FADE);
        if alpha <= 0.0 {
            continue;
        }
        let since = t - appear - CARD_FADE;
        let slide = (1.0 - alpha) * 16.0;
        let r = Rect::from_min_size(
            to_screen(card.rect.min) - vec2(slide * zoom, 0.0),
            card.rect.size() * zoom,
        );
        let accent = pal.accent(card.role);
        let glowing = matches!(card.role, CardRole::Result | CardRole::Target);

        if glowing {
            let pulse = if animating {
                0.5 + 0.5 * ((t * 2.6).sin() as f32)
            } else {
                0.6
            };
            for k in 1..=4 {
                let grow = k as f32 * 3.2 * zoom * (0.6 + 0.4 * pulse);
                let a = (0.30 - k as f32 * 0.06).max(0.02) * (0.5 + pulse * 0.5) * alpha;
                painter.rect_stroke(
                    r.expand(grow),
                    radius((10.0 + k as f32 * 2.0) * zoom),
                    Stroke::new(2.4 * zoom, accent.gamma_multiply(a)),
                    StrokeKind::Outside,
                );
            }
        }

        let fill = if glowing {
            lerp_color(pal.card, accent, 0.08)
        } else {
            pal.card
        };
        let border = if glowing { accent } else { pal.border };
        painter.rect(
            r,
            radius(8.0 * zoom),
            fill.gamma_multiply(alpha),
            Stroke::new(
                if glowing { 1.6 } else { 1.0 },
                border.gamma_multiply(alpha),
            ),
            StrokeKind::Inside,
        );

        // Header.
        let header = Rect::from_min_size(r.min, vec2(r.width(), HEADER_H * zoom));
        let rr = (8.0 * zoom).clamp(0.0, 255.0).round() as u8;
        painter.rect_filled(
            header,
            CornerRadius {
                nw: rr,
                ne: rr,
                sw: 0,
                se: 0,
            },
            accent.gamma_multiply(0.22 * alpha),
        );
        painter.line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, accent.gamma_multiply(0.5 * alpha)),
        );
        let fs = 13.0 * zoom;
        if fs >= 4.0 {
            let badge = painter.layout_no_wrap(
                card.badge.clone(),
                FontId::proportional(9.5 * zoom),
                pal.ink(accent).gamma_multiply(alpha),
            );
            let badge_rect = Rect::from_min_size(
                pos2(
                    header.max.x - badge.size().x - 18.0 * zoom,
                    header.center().y - badge.size().y * 0.5 - 2.0 * zoom,
                ),
                badge.size() + vec2(10.0, 4.0) * zoom,
            );
            painter.rect_stroke(
                badge_rect,
                radius(6.0 * zoom),
                Stroke::new(1.0, accent.gamma_multiply(0.7 * alpha)),
                StrokeKind::Inside,
            );
            painter.galley(
                badge_rect.min + vec2(5.0, 2.0) * zoom,
                badge,
                pal.ink(accent).gamma_multiply(alpha),
            );

            let title_room = (badge_rect.min.x - header.min.x - 16.0 * zoom).max(20.0);
            let max_chars = (title_room / (fs * 0.56)).max(3.0) as usize;
            painter.text(
                pos2(header.min.x + 10.0 * zoom, header.center().y),
                egui::Align2::LEFT_CENTER,
                clip(&card.title, max_chars),
                FontId::proportional(fs),
                pal.text.gamma_multiply(alpha),
            );
        }

        // Baris.
        let row_font = 12.0 * zoom;
        for (ri, row) in card.rows.iter().enumerate() {
            let cy = to_screen(pos2(card.rect.min.x, card.row_center_y(ri))).y;
            let row_rect = Rect::from_min_max(
                pos2(r.min.x + 4.0 * zoom, cy - ROW_H * 0.5 * zoom + 1.0),
                pos2(r.max.x - 4.0 * zoom, cy + ROW_H * 0.5 * zoom - 1.0),
            );
            let set_color = pal.accent(CardRole::Set);
            let (bg, dot) = match row.state {
                RowState::Join => (pal.join.gamma_multiply(0.10), pal.join),
                RowState::Filter => (pal.filter.gamma_multiply(0.12), pal.filter),
                RowState::Changed => {
                    let flash = if animating {
                        0.16 + 0.12 * ((t * 3.0 + ri as f64).sin() as f32).abs()
                    } else {
                        0.2
                    };
                    (set_color.gamma_multiply(flash), set_color)
                }
                RowState::Added => (accent.gamma_multiply(0.14), accent),
                RowState::Aggregate => (pal.aggregate.gamma_multiply(0.14), pal.aggregate),
                RowState::Used => (Color32::TRANSPARENT, pal.accent(CardRole::Source)),
                RowState::Normal => (Color32::TRANSPARENT, pal.weak.gamma_multiply(0.6)),
            };
            if bg != Color32::TRANSPARENT {
                painter.rect_filled(row_rect, radius(4.0 * zoom), bg.gamma_multiply(alpha));
            }
            if hovered == Some((ci, Some(ri))) {
                painter.rect_stroke(
                    row_rect,
                    radius(4.0 * zoom),
                    Stroke::new(1.2, accent),
                    StrokeKind::Inside,
                );
            }
            painter.circle_filled(
                pos2(r.min.x + 12.0 * zoom, cy),
                3.0 * zoom,
                dot.gamma_multiply(alpha),
            );
            if row_font >= 4.0 {
                let room = r.width() - 30.0 * zoom;
                let max_chars = (room / (row_font * 0.55)).max(3.0) as usize;
                let color = match row.state {
                    RowState::Normal => pal.weak,
                    _ => pal.text,
                };
                painter.text(
                    pos2(r.min.x + 22.0 * zoom, cy),
                    egui::Align2::LEFT_CENTER,
                    clip(&row.label, max_chars),
                    FontId::proportional(row_font),
                    color.gamma_multiply(alpha),
                );
            }

            let is_target = card.role == CardRole::Target;
            // DELETE: tiap baris dicoret berulang (baris dihapus utuh).
            if is_target && layout.kind == StatementKind::Delete && since > 0.0 {
                let cycle = if animating {
                    (since / 2.8).fract()
                } else {
                    0.6
                };
                let sweep = smooth(cycle * 1.7);
                let fade = if cycle > 0.75 {
                    1.0 - smooth((cycle - 0.75) / 0.25)
                } else {
                    1.0
                };
                let x1 = row_rect.min.x + row_rect.width() * sweep;
                painter.line_segment(
                    [pos2(row_rect.min.x, cy), pos2(x1, cy)],
                    Stroke::new(1.6 * zoom, pal.kind.gamma_multiply(0.85 * fade * alpha)),
                );
            }

            // UPDATE: chip nilai lama/baru di kanan kolom yang berubah.
            if is_target
                && layout.kind == StatementKind::Update
                && row.state == RowState::Changed
                && since > 0.0
                && zoom >= 0.45
            {
                let new_value = set_value(layout, &row.key).unwrap_or_default();
                let cycle = if animating {
                    (since / 4.0).fract()
                } else {
                    1.0
                };
                let show_new = smooth((cycle - 0.35) / 0.2);
                let (text, color) = if show_new >= 0.5 {
                    (format!("now: {}", clip(&new_value, 28)), pal.ink(set_color))
                } else {
                    (format!("was: current {}", clip(&row.key, 18)), pal.weak)
                };
                let fade = ((show_new - 0.5).abs() * 2.0).max(0.45);
                let galley = painter.layout_no_wrap(
                    text,
                    FontId::proportional(10.5 * zoom),
                    color.gamma_multiply(fade * alpha),
                );
                let chip = Rect::from_min_size(
                    pos2(
                        r.max.x + 12.0 * zoom,
                        cy - galley.size().y * 0.5 - 3.0 * zoom,
                    ),
                    galley.size() + vec2(12.0, 6.0) * zoom,
                );
                painter.line_segment(
                    [pos2(r.max.x, cy), pos2(chip.min.x, cy)],
                    Stroke::new(1.0, color.gamma_multiply(0.6 * alpha)),
                );
                painter.rect(
                    chip,
                    radius(8.0 * zoom),
                    pal.card.gamma_multiply(alpha),
                    Stroke::new(1.0, color.gamma_multiply(0.8 * alpha)),
                    StrokeKind::Inside,
                );
                painter.galley(chip.min + vec2(6.0, 3.0) * zoom, galley, color);
            }
        }

        if card.role != CardRole::Target || since <= 0.0 || zoom < 0.45 {
            continue;
        }
        // INSERT: pil baris baru meluncur masuk di bawah kartu target.
        if layout.kind == StatementKind::Insert {
            let cycle = if animating {
                (since / 3.0).fract()
            } else {
                1.0
            };
            let enter = smooth(cycle / 0.4);
            let galley = painter.layout_no_wrap(
                inserted_label(layout),
                FontId::proportional(11.0 * zoom),
                pal.ink(pal.kind).gamma_multiply(alpha),
            );
            let pill = Rect::from_min_size(
                pos2(r.min.x - (1.0 - enter) * 30.0 * zoom, r.max.y + 8.0 * zoom),
                vec2(r.width(), galley.size().y + 8.0 * zoom),
            );
            painter.rect(
                pill,
                radius(8.0 * zoom),
                pal.kind.gamma_multiply(0.15 * enter * alpha),
                Stroke::new(1.0, pal.kind.gamma_multiply(enter * alpha)),
                StrokeKind::Inside,
            );
            painter.galley(
                pos2(
                    pill.min.x + 10.0 * zoom,
                    pill.center().y - galley.size().y * 0.5,
                ),
                galley,
                pal.kind,
            );
        }
        // DELETE: label di bawah kartu target.
        if layout.kind == StatementKind::Delete {
            painter.text(
                pos2(r.min.x + 4.0 * zoom, r.max.y + 10.0 * zoom),
                egui::Align2::LEFT_TOP,
                "- matching rows are removed",
                FontId::proportional(11.0 * zoom),
                pal.kind.gamma_multiply(alpha),
            );
        }
    }

    draw_legend(&painter, rect, &pal);

    // --- Tooltip baris ---
    if let (Some((ci, Some(ri))), Some(p)) = (hovered, pointer)
        && let Some(row) = layout.cards[ci].rows.get(ri)
        && !row.detail.is_empty()
    {
        let galley = painter.layout(
            row.detail.clone(),
            FontId::proportional(12.0),
            pal.text,
            320.0,
        );
        let mut tip = Rect::from_min_size(p + vec2(14.0, 14.0), galley.size() + vec2(14.0, 10.0));
        if tip.max.x > rect.max.x {
            tip = tip.translate(vec2(rect.max.x - tip.max.x - 4.0, 0.0));
        }
        if tip.max.y > rect.max.y {
            tip = tip.translate(vec2(0.0, -(tip.height() + 24.0)));
        }
        painter.rect(
            tip,
            radius(6.0),
            pal.card,
            Stroke::new(1.0, pal.border),
            StrokeKind::Inside,
        );
        painter.galley(tip.min + vec2(7.0, 5.0), galley, pal.text);
    }

    if animating {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    rect
}

/// Tampilkan isi `add` sebagai jendela floating di atas kanvas, sebesar
/// kontennya dan dibatasi `max_rect`. Area jendela frame sebelumnya didaftarkan
/// lebih dulu sebagai penghalang supaya klik/scroll tidak tembus ke kanvas.
pub fn floating_window(
    ui: &mut egui::Ui,
    id: egui::Id,
    max_rect: egui::Rect,
    fill_opacity: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let prev: Option<egui::Rect> = ui.data(|d| d.get_temp(id));
    if let Some(r) = prev {
        ui.interact(r, id.with("block"), egui::Sense::click_and_drag());
    }
    let visuals = ui.visuals().clone();
    let stroke = visuals.widgets.noninteractive.bg_stroke;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(max_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(max_rect.intersect(ui.clip_rect()));
    let inner_w = (max_rect.width() - 20.0).max(80.0);
    let shown = egui::Frame::NONE
        .fill(visuals.window_fill.gamma_multiply(fill_opacity))
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(&mut child, |ui| {
            ui.set_width(inner_w);
            add(ui);
        });
    ui.data_mut(|d| d.insert_temp(id, shown.response.rect));
}

/// Jendela floating yang sisi kanan-bawahnya menempel di `right_bottom`.
/// Tingginya mengikuti isi (diukur dari frame sebelumnya), maksimal `max_height`.
pub fn floating_window_at_bottom(
    ui: &mut egui::Ui,
    id: egui::Id,
    right_bottom: Pos2,
    width: f32,
    max_height: f32,
    fill_opacity: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let prev: Option<Rect> = ui.data(|d| d.get_temp(id));
    let h = prev.map_or(120.0, |r| r.height()).min(max_height);
    let max_rect = Rect::from_min_size(
        pos2(right_bottom.x - width, right_bottom.y - h),
        vec2(width, max_height),
    );
    floating_window(ui, id, max_rect, fill_opacity, add);
    let now: Option<Rect> = ui.data(|d| d.get_temp(id));
    if now.map(|r| r.height()) != prev.map(|r| r.height()) {
        ui.ctx().request_repaint();
    }
}

/// Deretan tombol floating yang menempel ke pojok kanan bawah `canvas`
/// (atau kanan atas bila `top`). Lebarnya mengikuti isi (diukur dari frame
/// sebelumnya).
pub fn floating_bar(
    ui: &mut egui::Ui,
    id: egui::Id,
    canvas: egui::Rect,
    fill_opacity: f32,
    top: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    let prev: Option<egui::Rect> = ui.data(|d| d.get_temp(id));
    let size = prev.map_or(vec2(150.0, 36.0), |r| r.size());
    let min = if top {
        pos2(canvas.max.x - size.x - 12.0, canvas.min.y + 12.0)
    } else {
        canvas.right_bottom() - size - vec2(12.0, 12.0)
    };
    if prev.is_some() {
        ui.interact(
            Rect::from_min_size(min, size),
            id.with("block"),
            egui::Sense::click_and_drag(),
        );
    }
    let visuals = ui.visuals().clone();
    let area = Rect::from_min_max(min, canvas.max);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(Rect::from_min_size(min, vec2(400.0, 60.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_clip_rect(
        area.union(Rect::from_min_size(min, size))
            .intersect(ui.clip_rect()),
    );
    let shown = egui::Frame::NONE
        .fill(visuals.window_fill.gamma_multiply(fill_opacity))
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(5))
        .show(&mut child, |ui| {
            ui.horizontal(add);
        });
    let rect = shown.response.rect;
    if prev.map(|p| p.size()) != Some(rect.size()) {
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(id, rect));
}

/// Nilai baru kolom dari kartu SET (untuk chip UPDATE).
fn set_value(layout: &QueryLayout, column: &str) -> Option<String> {
    let set = layout.cards.iter().find(|c| c.role == CardRole::Set)?;
    let row = set
        .rows
        .iter()
        .find(|r| r.key.eq_ignore_ascii_case(column))?;
    row.detail.split_once(" = ").map(|(_, v)| v.to_string())
}

fn inserted_label(layout: &QueryLayout) -> String {
    let transform = layout
        .cards
        .iter()
        .find(|c| matches!(c.role, CardRole::Values | CardRole::Result));
    match transform {
        Some(c) if c.role == CardRole::Values => match c.badge.as_str() {
            "1 ROW" | "SET" => "+ 1 new row".to_string(),
            other => format!("+ {} new rows", other.trim_end_matches(" ROWS")),
        },
        _ => "+ new rows from the SELECT".to_string(),
    }
}

fn draw_legend(painter: &egui::Painter, rect: Rect, pal: &Palette) {
    let items = [
        ("join", pal.join, false),
        ("data flow", pal.data, false),
        ("filter", pal.filter, true),
        ("pipeline", pal.stage, false),
    ];
    let font = FontId::proportional(10.5);
    let mut x = rect.min.x + 10.0;
    let y = rect.max.y - LEGEND_H * 0.5;
    for (label, color, dashed) in items {
        let a = pos2(x, y);
        let b = pos2(x + 18.0, y);
        if dashed {
            painter.extend(Shape::dashed_line(
                &[a, b],
                Stroke::new(1.6, color),
                4.0,
                3.0,
            ));
        } else {
            painter.line_segment([a, b], Stroke::new(1.8, color));
        }
        let g = painter.layout_no_wrap(label.to_string(), font.clone(), pal.weak);
        let w = g.size().x;
        painter.galley(pos2(b.x + 5.0, y - g.size().y * 0.5), g, pal.weak);
        x = b.x + 5.0 + w + 14.0;
    }
}

/// Atur zoom/pan supaya seluruh diagram muat di `rect` dengan margin.
fn fit(view: &mut QueryDiagramView, bounds: Rect, rect: Rect, right_room: f32) {
    // Ruang ekstra kanan untuk chip UPDATE dan bawah untuk label target.
    let content = Rect::from_min_max(
        bounds.min - vec2(40.0, 30.0),
        bounds.max + vec2(right_room, 50.0),
    );
    let avail = rect.size() - vec2(0.0, LEGEND_H);
    let zx = avail.x / content.width().max(1.0);
    let zy = avail.y / content.height().max(1.0);
    view.zoom = zx.min(zy).clamp(MIN_ZOOM, 1.2);
    let scaled = content.size() * view.zoom;
    let offset = (avail - scaled) * 0.5;
    view.pan = offset - content.min.to_vec2() * view.zoom;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bezier_endpoints() {
        let p = [
            pos2(0.0, 0.0),
            pos2(10.0, 0.0),
            pos2(20.0, 10.0),
            pos2(30.0, 10.0),
        ];
        assert_eq!(bezier(&p, 0.0), p[0]);
        assert_eq!(bezier(&p, 1.0), p[3]);
    }

    #[test]
    fn test_fit_keeps_content_inside() {
        let mut v = QueryDiagramView::default();
        let bounds = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 300.0));
        let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 400.0));
        fit(&mut v, bounds, rect, 170.0);
        let tl = rect.min + v.pan + bounds.min.to_vec2() * v.zoom;
        let br = rect.min + v.pan + bounds.max.to_vec2() * v.zoom;
        assert!(rect.contains(tl) && rect.contains(br));
    }

    #[test]
    fn test_moved_layout_shifts_card_and_bounds() {
        use crate::query_diagram::StatementKind;
        use crate::query_diagram::layout::{Card, CardRole};
        let card = Card {
            id: "t".into(),
            title: "t".into(),
            badge: "FROM".into(),
            role: CardRole::Source,
            lane: 0,
            rect: Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0)),
            rows: Vec::new(),
        };
        let layout = QueryLayout {
            kind: StatementKind::Select,
            cards: vec![card],
            flows: Vec::new(),
            bounds: Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0)),
            steps: Vec::new(),
            warning: None,
        };
        let moved = moved_layout(&layout, &[vec2(300.0, 20.0)]);
        assert_eq!(moved.cards[0].rect.min, pos2(300.0, 20.0));
        assert!(moved.bounds.contains_rect(moved.cards[0].rect));
    }

    #[test]
    fn test_animation_stops_by_itself() {
        let mut v = QueryDiagramView::default();
        assert!(v.is_animating(0.0));
        v.start_time = Some(0.0);
        assert!(!v.is_animating(ANIM_SECS + 1.0));
        v.replay();
        assert!(v.is_animating(1000.0));
    }
}
