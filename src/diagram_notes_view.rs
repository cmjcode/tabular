//! UI sticky note diagram: kartu Markdown di kanvas, garis putus-putus ke
//! tabel yang di-link dengan `[[...]]`, badge jumlah note di header tabel /
//! group, jendela daftar note, dan modal editor. Logika murninya ada di
//! `crate::diagram_notes`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::diagram_lod::{quantize_font, truncate_for_width};
use crate::diagram_notes::{self as notes, LINK_SCHEME, NOTE_COLORS};
use crate::models::structs::{
    DiagramNote, DiagramState, DiagramViewAnimation, NoteAnchor, NoteDraft,
};
use crate::window_egui::style;

/// Warna teks di atas kartu pastel.
const NOTE_TEXT: egui::Color32 = egui::Color32::from_rgb(42, 40, 34);
/// Di bawah zoom ini kartu hanya menampilkan judul.
const COLLAPSE_ZOOM: f32 = 0.45;
const HEADER_H: f32 = 26.0;
/// Zoom minimum saat melompat ke note atau tabel dari link.
const JUMP_MIN_ZOOM: f32 = 0.7;

type SharedCache = Arc<Mutex<CommonMarkCache>>;

/// Cache `egui_commonmark` bersama untuk semua kartu dan pratinjau editor.
fn md_cache(ctx: &egui::Context) -> SharedCache {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<SharedCache>(egui::Id::new("diagram_notes_md_cache"))
            .clone()
    })
}

fn rects_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("diagram_note_card_rects")
}

/// Bingkai layar kartu note pada frame sebelumnya. Dipakai kanvas supaya
/// scroll di atas kartu menggulung isi note, bukan zoom diagram.
pub fn last_card_rects(ui: &egui::Ui) -> Vec<egui::Rect> {
    ui.data(|d| d.get_temp::<Vec<egui::Rect>>(rects_id(ui)))
        .unwrap_or_default()
}

fn darken(c: egui::Color32, f: f32) -> egui::Color32 {
    let k = 1.0 - f;
    egui::Color32::from_rgb(
        (c.r() as f32 * k) as u8,
        (c.g() as f32 * k) as u8,
        (c.b() as f32 * k) as u8,
    )
}

fn format_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn note_meta(note: &DiagramNote) -> String {
    let when = format_time(if note.updated_at.is_empty() {
        &note.created_at
    } else {
        &note.updated_at
    });
    match (&note.author, when.is_empty()) {
        (Some(a), false) => format!("{a} · {when}"),
        (Some(a), true) => a.clone(),
        (None, false) => when,
        (None, true) => String::new(),
    }
}

/// Animasikan viewport supaya titik diagram `center` ada di tengah kanvas.
pub fn animate_to(state: &mut DiagramState, center: egui::Pos2, view_size: egui::Vec2, now: f64) {
    let to_zoom = state
        .zoom
        .max(JUMP_MIN_ZOOM)
        .clamp(crate::diagram_view::MIN_ZOOM, crate::diagram_view::MAX_ZOOM);
    state.view_anim = Some(DiagramViewAnimation {
        from_pan: state.pan,
        to_pan: crate::diagram_view::pan_to_center(center, view_size, to_zoom),
        from_zoom: state.zoom,
        to_zoom,
        start_time: now,
        duration: crate::diagram_view::VIEW_ANIM_SECS,
    });
    state.is_centered = true;
}

/// Note yang tampil frame ini. Id note yang sudah dihapus dibuang dari
/// `open_notes` dan kartu yang belum punya posisi ditata di sekitar anchor.
pub fn prepare_visible(state: &mut DiagramState) -> HashSet<String> {
    if !state.open_notes.is_empty() {
        let ids: HashSet<&str> = state.notes.iter().map(|n| n.id.as_str()).collect();
        state.open_notes.retain(|id| ids.contains(id.as_str()));
    }
    let visible = notes::visible_note_ids(state);
    let mut pending: Vec<NoteAnchor> = Vec::new();
    for n in &state.notes {
        if n.offset.is_none() && visible.contains(&n.id) && !pending.contains(&n.anchor) {
            pending.push(n.anchor.clone());
        }
    }
    for a in pending {
        notes::arrange_anchor_notes(state, &a, &visible, false);
    }
    visible
}

/// Permintaan dari badge / menu konteks, diproses setelah loop gambar.
#[derive(Clone, Debug)]
pub enum NoteRequest {
    /// Buka editor note baru untuk anchor ini.
    New(NoteAnchor),
    /// Buka jendela daftar note milik anchor ini.
    List(NoteAnchor),
    /// Tampilkan / sembunyikan semua note anchor ini di kanvas.
    Toggle(NoteAnchor),
}

pub fn apply_request(state: &mut DiagramState, req: NoteRequest) {
    match req {
        NoteRequest::New(anchor) => open_new_note(state, anchor),
        NoteRequest::List(anchor) => {
            state.notes_panel = Some(Some(anchor));
            state.notes_panel_query.clear();
        }
        NoteRequest::Toggle(anchor) => toggle_anchor_notes(state, &anchor),
    }
}

/// Klik badge: tampilkan semua note anchor ini (ditata rapi). Bila semua
/// sudah tampil, sembunyikan yang dibuka; bila semuanya di-pin, buka daftar.
pub fn toggle_anchor_notes(state: &mut DiagramState, anchor: &NoteAnchor) {
    state.show_notes = true;
    let ids: Vec<(String, bool)> = state
        .notes
        .iter()
        .filter(|n| &n.anchor == anchor)
        .map(|n| (n.id.clone(), n.pinned))
        .collect();
    let any_hidden = ids
        .iter()
        .any(|(id, pinned)| !pinned && !state.open_notes.contains(id));
    if any_hidden {
        for (id, _) in &ids {
            state.open_notes.insert(id.clone());
        }
        let visible = notes::visible_note_ids(state);
        notes::arrange_anchor_notes(state, anchor, &visible, false);
    } else if ids.iter().any(|(id, _)| state.open_notes.contains(id)) {
        for (id, _) in &ids {
            state.open_notes.remove(id);
        }
    } else {
        state.notes_panel = Some(Some(anchor.clone()));
    }
}

/// Badge "[ikon] n" di `pos` (disejajarkan dengan `align`). `None` bila zoom
/// terlalu kecil untuk badge.
pub fn draw_badge(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    align: egui::Align2,
    count: usize,
    scale: f32,
    id: egui::Id,
    active: bool,
) -> Option<egui::Response> {
    if scale < 0.3 {
        return None;
    }
    let s = scale.clamp(0.7, 1.4);
    let galley = ui.painter().layout_no_wrap(
        format!(
            "{} {count}",
            egui_icons::icons::ICON_STICKY_NOTE_2.codepoint
        ),
        egui::FontId::proportional(quantize_font(10.5 * s)),
        NOTE_TEXT,
    );
    let size = galley.size() + egui::vec2(10.0 * s, 4.0 * s);
    let rect = align.anchor_size(pos, size);
    let resp = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if resp.hovered() {
        egui::Color32::from_rgb(255, 245, 190)
    } else {
        NOTE_COLORS[0]
    };
    ui.painter().rect_filled(rect, size.y / 2.0, fill);
    ui.painter().rect_stroke(
        rect,
        size.y / 2.0,
        egui::Stroke::new(
            if active { 1.5 } else { 1.0 },
            darken(NOTE_COLORS[0], if active { 0.55 } else { 0.35 }),
        ),
        egui::StrokeKind::Middle,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, NOTE_TEXT);
    Some(resp)
}

/// Tooltip badge note.
pub fn badge_tooltip(count: usize) -> String {
    let noun = if count == 1 { "note" } else { "notes" };
    format!("{count} {noun}\nClick to show or hide them on the canvas\nRight-click for the list")
}

fn screen_rect(r: egui::Rect, to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max))
}

/// Titik awal/akhir + arah keluar garis dari kartu `a` ke tabel `b`.
/// `target_y` = baris kolom tujuan (layar) bila link menyebut kolom.
fn link_endpoints(
    a: egui::Rect,
    b: egui::Rect,
    target_y: Option<f32>,
) -> (egui::Pos2, egui::Vec2, egui::Pos2, egui::Vec2) {
    let ty = target_y.unwrap_or(b.center().y).clamp(b.top(), b.bottom());
    let (l, r, u, d) = (
        egui::vec2(-1.0, 0.0),
        egui::vec2(1.0, 0.0),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.0, 1.0),
    );
    if b.left() >= a.right() {
        (
            egui::pos2(a.right(), a.center().y),
            r,
            egui::pos2(b.left(), ty),
            l,
        )
    } else if b.right() <= a.left() {
        (
            egui::pos2(a.left(), a.center().y),
            l,
            egui::pos2(b.right(), ty),
            r,
        )
    } else if target_y.is_some() {
        (
            egui::pos2(a.left(), a.center().y),
            l,
            egui::pos2(b.left(), ty),
            l,
        )
    } else if b.top() >= a.bottom() {
        (a.center_bottom(), d, b.center_top(), u)
    } else {
        (a.center_top(), u, b.center_bottom(), d)
    }
}

fn dist_to_polyline(p: egui::Pos2, points: &[egui::Pos2]) -> f32 {
    points
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_sq().max(f32::EPSILON)).clamp(0.0, 1.0);
            (a + ab * t - p).length()
        })
        .fold(f32::INFINITY, f32::min)
}

/// Garis putus-putus dari kartu note yang tampil ke tabel `[[...]]`, plus
/// tali tipis dari kartu ke anchor-nya bila tidak bersentuhan.
pub fn draw_note_links(
    ui: &egui::Ui,
    state: &DiagramState,
    visible: &HashSet<String>,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    clip: egui::Rect,
    hover: Option<egui::Pos2>,
) {
    if visible.is_empty() {
        return;
    }
    let scale = state.zoom;
    let line_scale = scale.max(0.5);
    let painter = ui.painter();

    for note in state.notes.iter().filter(|n| visible.contains(&n.id)) {
        let Some(ar) = notes::anchor_rect(state, &note.anchor) else {
            continue;
        };
        let Some(nr) = notes::note_rect(ar, note) else {
            continue;
        };
        let (a, b) = (screen_rect(nr, to_screen), screen_rect(ar, to_screen));
        if a.expand(2.0).intersects(b) {
            continue;
        }
        let p = a.clamp(b.center());
        let q = b.clamp(p);
        if clip.intersects(egui::Rect::from_two_pos(p, q)) {
            painter.line_segment(
                [p, q],
                egui::Stroke::new(1.0 * line_scale, note.color.gamma_multiply(0.45)),
            );
        }
    }

    let mut label: Option<(egui::Pos2, String, egui::Color32)> = None;
    for link in notes::note_links(state, visible) {
        let Some(note) = state.notes.iter().find(|n| n.id == link.note_id) else {
            continue;
        };
        let Some(node) = state.nodes.iter().find(|n| n.id == link.table_id) else {
            continue;
        };
        let Some(card) = notes::anchor_rect(state, &note.anchor)
            .and_then(|ar| notes::note_rect(ar, note))
            .map(|r| screen_rect(r, to_screen))
        else {
            continue;
        };
        let target = screen_rect(egui::Rect::from_min_size(node.pos, node.size), to_screen);
        let target_y = link.column.as_deref().map(|c| {
            to_screen(egui::pos2(
                node.pos.x,
                crate::diagram_view::column_anchor_y(node, c),
            ))
            .y
        });
        let (p0, d0, p3, d3) = link_endpoints(card, target, target_y);
        let k = ((p3 - p0).length() * 0.4).clamp(20.0 * line_scale, 140.0 * line_scale);
        let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
            [p0, p0 + d0 * k, p3 + d3 * k, p3],
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::NONE,
        );
        let points: Vec<egui::Pos2> = (0..=24).map(|i| bezier.sample(i as f32 / 24.0)).collect();
        if !clip.intersects(egui::Rect::from_points(&points).expand(4.0)) {
            continue;
        }
        let hovered = hover.is_some_and(|h| dist_to_polyline(h, &points) < 8.0);
        let (width, color) = if hovered {
            (2.5, note.color)
        } else {
            (1.5, note.color.gamma_multiply(0.85))
        };
        painter.extend(egui::Shape::dashed_line(
            &points,
            egui::Stroke::new(width * line_scale, color),
            7.0 * line_scale,
            5.0 * line_scale,
        ));
        painter.circle_filled(p3, 3.5 * line_scale, color);
        if hovered {
            let target_label = match &link.column {
                Some(c) => format!("{}.{c}", node.title),
                None => node.title.clone(),
            };
            label = Some((
                points[12],
                format!("{} → {target_label}", notes::display_title(note)),
                color,
            ));
        }
    }
    if let Some((p, text, color)) = label {
        painter.text(
            p - egui::vec2(0.0, 8.0),
            egui::Align2::CENTER_BOTTOM,
            text,
            egui::FontId::proportional(11.0),
            color,
        );
    }
}

/// Gaya isi kartu: teks diskalakan dengan zoom dan warna gelap di atas
/// latar pastel.
fn style_card_body(ui: &mut egui::Ui, s: f32, color: egui::Color32) {
    let st = ui.style_mut();
    for f in st.text_styles.values_mut() {
        f.size = quantize_font(f.size * s).max(1.0);
    }
    st.spacing.item_spacing *= s;
    st.spacing.indent *= s;
    st.spacing.icon_width *= s;
    st.spacing.icon_spacing *= s;
    st.interaction.selectable_labels = false;
    let v = &mut st.visuals;
    let tint = darken(color, 0.10);
    v.override_text_color = Some(NOTE_TEXT);
    v.weak_text_color = Some(NOTE_TEXT.gamma_multiply(0.65));
    v.hyperlink_color = egui::Color32::from_rgb(25, 88, 190);
    v.code_bg_color = tint;
    v.extreme_bg_color = tint;
    v.faint_bg_color = tint;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, NOTE_TEXT.gamma_multiply(0.3));
    v.widgets.noninteractive.fg_stroke.color = NOTE_TEXT;
    // Heading, teks tebal, bullet dan garis blockquote `egui_commonmark`
    // memakai `strong_text_color()` (= warna widget aktif, putih di tema
    // gelap) yang tidak ikut `override_text_color`.
    v.widgets.active.fg_stroke.color = NOTE_TEXT;
    v.widgets.hovered.fg_stroke.color = NOTE_TEXT;
    v.widgets.inactive.fg_stroke.color = NOTE_TEXT;
}

/// Render Markdown note; mengembalikan tabel link yang diklik.
fn show_markdown(
    ui: &mut egui::Ui,
    nodes: &[crate::models::structs::DiagramNode],
    body: &str,
) -> Option<String> {
    let (md, targets) = notes::render_markdown(nodes, body);
    let cache = md_cache(ui.ctx());
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.link_hooks_clear();
    for i in 0..targets.len() {
        cache.add_link_hook(format!("{LINK_SCHEME}{i}"));
    }
    CommonMarkViewer::new().show(ui, &mut cache, &md);
    let clicked = cache
        .link_hooks()
        .iter()
        .filter(|(_, v)| **v)
        .find_map(|(k, _)| notes::link_index(k))
        .and_then(|i| targets.get(i))
        .map(|t| t.table_id.clone());
    cache.link_hooks_clear();
    clicked
}

/// Hasil interaksi kartu pada satu frame.
#[derive(Default)]
pub struct CardsOutcome {
    /// Buka editor note ini.
    pub edit: Option<String>,
    /// Link `[[tabel]]` diklik: lompat ke tabel ini.
    pub jump_table: Option<String>,
}

#[derive(Default)]
struct CardChange {
    drag: Option<(String, egui::Vec2)>,
    resize: Option<(String, egui::Vec2)>,
    released: bool,
    close: Option<String>,
    pin: Option<String>,
    raise: Option<String>,
}

fn header_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    icon: &str,
    tip: &str,
    on: bool,
    s: f32,
) -> bool {
    let resp = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0 * s, NOTE_TEXT.gamma_multiply(0.12));
    }
    let color = if on || resp.hovered() {
        NOTE_TEXT
    } else {
        NOTE_TEXT.gamma_multiply(0.55)
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(quantize_font(13.0 * s)),
        color,
    );
    resp.clicked()
}

/// Gambar kartu note yang tampil. `interactive` = false pada mode Hand Tool.
pub fn render_note_cards(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    visible: &HashSet<String>,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    interactive: bool,
) -> CardsOutcome {
    let mut out = CardsOutcome::default();
    let mut change = CardChange::default();
    let mut rects: Vec<egui::Rect> = Vec::new();
    let clip = ui.clip_rect();
    let s = state.zoom;
    let read_only = state.scoped_to.is_some();

    for note in state.notes.iter().filter(|n| visible.contains(&n.id)) {
        let Some(sr) = notes::anchor_rect(state, &note.anchor)
            .and_then(|ar| notes::note_rect(ar, note))
            .map(|r| screen_rect(r, to_screen))
        else {
            continue;
        };
        if !clip.intersects(sr) {
            continue;
        }
        rects.push(sr);
        let color = note.color;
        let radius = 6.0 * s;
        let base_id = ui.id().with(("diagram_note", &note.id));

        ui.painter().rect_filled(
            sr.translate(egui::vec2(2.0, 3.0) * s),
            radius,
            egui::Color32::from_black_alpha(70),
        );
        ui.painter().rect_filled(sr, radius, color);
        ui.painter().rect_stroke(
            sr,
            radius,
            egui::Stroke::new(1.0 * s.max(0.5), darken(color, 0.35)),
            egui::StrokeKind::Middle,
        );

        let sense = if interactive {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        };
        let body_resp = ui.interact(sr, base_id.with("card"), sense);
        if body_resp.drag_started() {
            change.raise = Some(note.id.clone());
        }
        if body_resp.dragged() {
            change.drag = Some((note.id.clone(), body_resp.drag_delta() / s));
        }
        if body_resp.drag_stopped() {
            change.released = true;
        }
        if body_resp.double_clicked() && !read_only {
            out.edit = Some(note.id.clone());
        }

        let title = notes::display_title(note);
        if s < COLLAPSE_ZOOM {
            let px = 12.0f32.min(sr.height() * 0.5).max(6.0);
            ui.painter().text(
                sr.center(),
                egui::Align2::CENTER_CENTER,
                truncate_for_width(&title, sr.width() - 4.0, px),
                egui::FontId::proportional(quantize_font(px)),
                NOTE_TEXT,
            );
            continue;
        }

        // Header: judul + tombol pin / edit / tutup.
        let header = egui::Rect::from_min_size(sr.min, egui::vec2(sr.width(), HEADER_H * s));
        let r = radius.round().clamp(0.0, 255.0) as u8;
        ui.painter().rect_filled(
            header,
            egui::CornerRadius {
                nw: r,
                ne: r,
                sw: 0,
                se: 0,
            },
            darken(color, 0.08),
        );
        let btn = 20.0 * s;
        let mut bx = header.right() - 4.0 * s - btn;
        let mut next_btn = || {
            let rect = egui::Rect::from_min_size(
                egui::pos2(bx, header.center().y - btn / 2.0),
                egui::vec2(btn, btn),
            );
            bx -= btn + 2.0 * s;
            rect
        };
        let mut title_right = header.right() - 8.0 * s;
        if interactive {
            let close_rect = next_btn();
            title_right = close_rect.left() - 4.0 * s;
            let close_tip = if note.pinned {
                "Unpin and close"
            } else {
                "Close"
            };
            if header_button(
                ui,
                close_rect,
                base_id.with("close"),
                egui_icons::icons::ICON_CLOSE.codepoint,
                close_tip,
                false,
                s,
            ) {
                change.close = Some(note.id.clone());
            }
            if !read_only {
                let edit_rect = next_btn();
                if header_button(
                    ui,
                    edit_rect,
                    base_id.with("edit"),
                    egui_icons::icons::ICON_EDIT.codepoint,
                    "Edit note",
                    false,
                    s,
                ) {
                    out.edit = Some(note.id.clone());
                }
                let pin_rect = next_btn();
                title_right = pin_rect.left() - 4.0 * s;
                let pin_tip = if note.pinned {
                    "Pinned: shown to everyone who opens this diagram. Click to unpin."
                } else {
                    "Pin: always show this note to everyone who opens this diagram"
                };
                if header_button(
                    ui,
                    pin_rect,
                    base_id.with("pin"),
                    egui_icons::icons::ICON_PUSH_PIN.codepoint,
                    pin_tip,
                    note.pinned,
                    s,
                ) {
                    change.pin = Some(note.id.clone());
                }
            }
        }
        let title_px = quantize_font(13.0 * s);
        ui.painter().text(
            egui::pos2(header.left() + 8.0 * s, header.center().y),
            egui::Align2::LEFT_CENTER,
            truncate_for_width(&title, title_right - header.left() - 8.0 * s, title_px),
            egui::FontId::proportional(title_px),
            NOTE_TEXT,
        );

        // Isi Markdown.
        let body_rect = egui::Rect::from_min_max(
            egui::pos2(sr.left() + 8.0 * s, header.bottom() + 4.0 * s),
            egui::pos2(sr.right() - 8.0 * s, sr.bottom() - 6.0 * s),
        );
        if body_rect.height() > 8.0 && body_rect.width() > 8.0 {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .id_salt(("diagram_note_body", &note.id))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            child.set_clip_rect(body_rect.intersect(clip));
            style_card_body(&mut child, s, color);
            let meta = note_meta(note);
            egui::ScrollArea::vertical()
                .id_salt(("diagram_note_scroll", &note.id))
                .auto_shrink([false, false])
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Never,
                    ..egui::scroll_area::ScrollSource::ALL
                })
                .show(&mut child, |ui| {
                    if note.body.trim().is_empty() {
                        ui.label(
                            egui::RichText::new("Empty note. Double-click to edit.")
                                .italics()
                                .weak(),
                        );
                    } else if let Some(t) = show_markdown(ui, &state.nodes, &note.body) {
                        out.jump_table = Some(t);
                    }
                    if !meta.is_empty() {
                        ui.add_space(4.0 * s);
                        ui.label(egui::RichText::new(meta).small().weak());
                    }
                });
        }

        // Pegangan resize di pojok kanan bawah.
        if interactive {
            let h = 14.0 * s;
            let grip = egui::Rect::from_min_size(sr.max - egui::vec2(h, h), egui::vec2(h, h));
            let resp = ui
                .interact(grip, base_id.with("resize"), egui::Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeNwSe);
            if resp.dragged() {
                change.resize = Some((note.id.clone(), resp.drag_delta() / s));
            }
            if resp.drag_stopped() {
                change.released = true;
            }
            let stroke = egui::Stroke::new(1.0, NOTE_TEXT.gamma_multiply(0.4));
            for k in [0.35, 0.7] {
                ui.painter().line_segment(
                    [
                        egui::pos2(grip.right() - 3.0, grip.bottom() - h * k),
                        egui::pos2(grip.right() - h * k, grip.bottom() - 3.0),
                    ],
                    stroke,
                );
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(rects_id(ui), rects));

    if let Some(id) = change.raise
        && let Some(i) = state.notes.iter().position(|n| n.id == id)
    {
        let n = state.notes.remove(i);
        state.notes.push(n);
    }
    if let Some((id, delta)) = change.drag
        && let Some(n) = state.notes.iter_mut().find(|n| n.id == id)
        && let Some(off) = &mut n.offset
    {
        off[0] += delta.x;
        off[1] += delta.y;
    }
    if let Some((id, delta)) = change.resize
        && let Some(n) = state.notes.iter_mut().find(|n| n.id == id)
    {
        let size = (egui::vec2(n.size[0], n.size[1]) + delta).max(notes::MIN_NOTE_SIZE);
        n.size = [size.x, size.y];
    }
    if change.released {
        state.save_requested = true;
    }
    if let Some(id) = change.pin
        && let Some(n) = state.notes.iter_mut().find(|n| n.id == id)
    {
        n.pinned = !n.pinned;
        if !n.pinned {
            state.open_notes.insert(id);
        }
        state.save_requested = true;
    }
    if let Some(id) = change.close {
        state.open_notes.remove(&id);
        if let Some(n) = state.notes.iter_mut().find(|n| n.id == id && n.pinned) {
            n.pinned = false;
            state.save_requested = true;
        }
    }
    out
}

/// Buka editor untuk note baru pada `anchor`.
pub fn open_new_note(state: &mut DiagramState, anchor: NoteAnchor) {
    if state.scoped_to.is_some() {
        return;
    }
    let color = NOTE_COLORS[state.notes.len() % NOTE_COLORS.len()];
    state.note_editor = Some(NoteDraft {
        note_id: None,
        anchor,
        title: String::new(),
        body: String::new(),
        color,
        pinned: false,
        preview: true,
        confirm_delete: false,
    });
}

/// Buka editor untuk note yang sudah ada.
pub fn open_note_editor(state: &mut DiagramState, id: &str) {
    if state.scoped_to.is_some() {
        return;
    }
    if let Some(n) = state.notes.iter().find(|n| n.id == id) {
        state.note_editor = Some(NoteDraft {
            note_id: Some(n.id.clone()),
            anchor: n.anchor.clone(),
            title: n.title.clone(),
            body: n.body.clone(),
            color: n.color,
            pinned: n.pinned,
            preview: true,
            confirm_delete: false,
        });
    }
}

fn remove_note(state: &mut DiagramState, id: &str) {
    state.notes.retain(|n| n.id != id);
    state.open_notes.remove(id);
    state.save_requested = true;
}

/// Tampilkan note `id` di kanvas dan geser viewport ke kartunya.
pub fn reveal_note(state: &mut DiagramState, id: &str, view_size: egui::Vec2, now: f64) {
    let Some(anchor) = state
        .notes
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.anchor.clone())
    else {
        return;
    };
    state.show_notes = true;
    state.open_notes.insert(id.to_string());
    let visible = notes::visible_note_ids(state);
    notes::arrange_anchor_notes(state, &anchor, &visible, false);
    if let Some(r) = notes::anchor_rect(state, &anchor).and_then(|ar| {
        state
            .notes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| notes::note_rect(ar, n))
    }) {
        animate_to(state, r.center(), view_size, now);
    }
}

fn swatch(ui: &mut egui::Ui, color: egui::Color32, size: f32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    ui.painter().rect_filled(rect, 3.0, color);
    let stroke = if selected {
        egui::Stroke::new(2.0, ui.visuals().strong_text_color())
    } else {
        egui::Stroke::new(1.0, darken(color, 0.35))
    };
    ui.painter()
        .rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Middle);
    resp
}

enum PanelAction {
    Filter(Option<NoteAnchor>),
    New(NoteAnchor),
    Edit(String),
    Remove(String),
    Toggle(String),
    Pin(String),
    Reveal(String),
    ShowAll(NoteAnchor),
    HideAll(NoteAnchor),
    Tidy(NoteAnchor),
    JumpAnchor(NoteAnchor),
}

fn note_matches(state: &DiagramState, n: &DiagramNote, q: &str) -> bool {
    q.is_empty()
        || n.title.to_lowercase().contains(q)
        || n.body.to_lowercase().contains(q)
        || notes::anchor_label(state, &n.anchor)
            .to_lowercase()
            .contains(q)
}

fn snippet(n: &DiagramNote) -> String {
    let title = notes::display_title(n);
    n.body
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['#', '>', '-', '*', ' '])
                .trim()
        })
        .filter(|l| !l.is_empty() && *l != title && !l.starts_with("```"))
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(90)
        .collect()
}

/// Jendela "Notes": daftar note per tabel/group, plus note yang anchor-nya
/// sudah tidak ada. Klik judul menampilkan note di kanvas dan menggeser
/// viewport ke sana.
pub fn render_notes_panel(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    canvas: egui::Rect,
    now: f64,
) {
    let Some(filter) = state.notes_panel.clone() else {
        return;
    };
    if let Some(a) = &filter
        && !notes::anchor_exists(state, a)
    {
        state.notes_panel = Some(None);
        return;
    }
    let read_only = state.scoped_to.is_some();
    let visible = notes::visible_note_ids(state);
    let query = state.notes_panel_query.trim().to_lowercase();
    let confirm_id = ui.id().with("diagram_notes_confirm_remove");
    let mut confirm: Option<String> = ui.data(|d| d.get_temp(confirm_id));
    let mut actions: Vec<PanelAction> = Vec::new();
    let title = match &filter {
        Some(a) => format!("Notes · {}", notes::anchor_label(state, a)),
        None => format!("Notes ({})", state.notes.len()),
    };

    // Anchor yang ditampilkan, urut tabel lalu group, per nama.
    let anchors: Vec<NoteAnchor> = match &filter {
        Some(a) => vec![a.clone()],
        None => {
            let mut list: Vec<NoteAnchor> = Vec::new();
            for n in &state.notes {
                if !list.contains(&n.anchor) && notes::anchor_exists(state, &n.anchor) {
                    list.push(n.anchor.clone());
                }
            }
            list.sort_by_key(|a| {
                (
                    matches!(a, NoteAnchor::Group(_)),
                    notes::anchor_label(state, a).to_lowercase(),
                )
            });
            list
        }
    };
    let orphans: Vec<String> = if filter.is_none() {
        notes::orphan_notes(state)
            .into_iter()
            .map(|n| n.id.clone())
            .collect()
    } else {
        Vec::new()
    };

    let mut open = true;
    egui::Window::new(title)
        .id(ui.id().with("diagram_notes_panel"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_pos(canvas.right_top() + egui::vec2(-400.0, 16.0))
        .default_size(egui::vec2(370.0, 460.0))
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.notes_panel_query)
                        .hint_text("Search notes")
                        .desired_width(ui.available_width() - 110.0),
                );
                if ui
                    .checkbox(&mut state.show_notes, "On canvas")
                    .on_hover_text("Show note cards and their dashed links on the diagram")
                    .changed()
                {
                    state.save_requested = true;
                }
            });
            ui.horizontal(|ui| match &filter {
                Some(a) => {
                    if ui
                        .add_enabled(
                            !read_only,
                            egui::Button::new(format!(
                                "{} New note",
                                egui_icons::icons::ICON_NOTE_ADD.codepoint
                            )),
                        )
                        .clicked()
                    {
                        actions.push(PanelAction::New(a.clone()));
                    }
                    if ui.button("All notes").clicked() {
                        actions.push(PanelAction::Filter(None));
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("Right-click a table or group to add a note.")
                            .small()
                            .weak(),
                    );
                }
            });
            if read_only {
                ui.label(
                    egui::RichText::new(
                        "This tab shows a subset and is not saved. Edit notes in the full diagram tab.",
                    )
                    .small()
                    .weak(),
                );
            }
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut shown = 0usize;
                    for anchor in &anchors {
                        let ids: Vec<String> = state
                            .notes
                            .iter()
                            .filter(|n| &n.anchor == anchor && note_matches(state, n, &query))
                            .map(|n| n.id.clone())
                            .collect();
                        if ids.is_empty() && filter.is_none() {
                            continue;
                        }
                        shown += ids.len();
                        anchor_header(ui, state, anchor, &ids, &visible, read_only, &mut actions);
                        for id in &ids {
                            note_row(ui, state, id, &visible, read_only, &mut confirm, &mut actions);
                        }
                        ui.add_space(6.0);
                    }
                    let orphan_ids: Vec<&String> = orphans
                        .iter()
                        .filter(|id| {
                            state
                                .notes
                                .iter()
                                .find(|n| &n.id == *id)
                                .is_some_and(|n| note_matches(state, n, &query))
                        })
                        .collect();
                    if !orphan_ids.is_empty() {
                        shown += orphan_ids.len();
                        ui.separator();
                        ui.label(egui::RichText::new("Unattached notes").strong());
                        ui.label(
                            egui::RichText::new(
                                "Their table or group is no longer in this diagram. They are kept until you remove them.",
                            )
                            .small()
                            .weak(),
                        );
                        for id in orphan_ids {
                            note_row(ui, state, id, &visible, read_only, &mut confirm, &mut actions);
                        }
                    }
                    if shown == 0 {
                        ui.add_space(12.0);
                        ui.vertical_centered(|ui| {
                            let msg = if query.is_empty() {
                                "No notes yet."
                            } else {
                                "No notes match your search."
                            };
                            ui.label(egui::RichText::new(msg).weak());
                        });
                    }
                });
        });
    ui.data_mut(|d| match &confirm {
        Some(c) => {
            d.insert_temp(confirm_id, c.clone());
        }
        None => d.remove::<String>(confirm_id),
    });
    if !open {
        state.notes_panel = None;
    }

    let view_size = canvas.size();
    for action in actions {
        match action {
            PanelAction::Filter(f) => state.notes_panel = Some(f),
            PanelAction::New(a) => open_new_note(state, a),
            PanelAction::Edit(id) => open_note_editor(state, &id),
            PanelAction::Remove(id) => remove_note(state, &id),
            PanelAction::Toggle(id) => {
                if !state.open_notes.remove(&id) {
                    reveal_note(state, &id, view_size, now);
                }
            }
            PanelAction::Pin(id) => {
                if let Some(n) = state.notes.iter_mut().find(|n| n.id == id) {
                    n.pinned = !n.pinned;
                    if !n.pinned {
                        state.open_notes.insert(id);
                    }
                    state.save_requested = true;
                }
            }
            PanelAction::Reveal(id) => reveal_note(state, &id, view_size, now),
            PanelAction::ShowAll(a) => {
                let ids: Vec<String> = state
                    .notes
                    .iter()
                    .filter(|n| n.anchor == a)
                    .map(|n| n.id.clone())
                    .collect();
                state.show_notes = true;
                state.open_notes.extend(ids);
                let visible = notes::visible_note_ids(state);
                notes::arrange_anchor_notes(state, &a, &visible, false);
                if let Some(r) = notes::anchor_rect(state, &a) {
                    animate_to(state, r.center(), view_size, now);
                }
            }
            PanelAction::HideAll(a) => {
                for n in state.notes.iter().filter(|n| n.anchor == a) {
                    state.open_notes.remove(&n.id);
                }
            }
            PanelAction::Tidy(a) => {
                let visible = notes::visible_note_ids(state);
                if notes::arrange_anchor_notes(state, &a, &visible, true) {
                    state.save_requested = true;
                }
            }
            PanelAction::JumpAnchor(a) => {
                if let Some(r) = notes::anchor_rect(state, &a) {
                    animate_to(state, r.center(), view_size, now);
                }
            }
        }
    }
}

fn anchor_header(
    ui: &mut egui::Ui,
    state: &DiagramState,
    anchor: &NoteAnchor,
    ids: &[String],
    visible: &HashSet<String>,
    read_only: bool,
    actions: &mut Vec<PanelAction>,
) {
    let icon = match anchor {
        NoteAnchor::Table(_) => egui_icons::icons::ICON_TABLE_CHART.codepoint,
        NoteAnchor::Group(_) => egui_icons::icons::ICON_GRID_VIEW.codepoint,
    };
    let any_hidden = ids.iter().any(|id| !visible.contains(id));
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !read_only
                && ui
                    .small_button(egui_icons::icons::ICON_ADD.codepoint)
                    .on_hover_text("New note here")
                    .clicked()
            {
                actions.push(PanelAction::New(anchor.clone()));
            }
            if !ids.is_empty() {
                if ui
                    .small_button(egui_icons::icons::ICON_AUTO_FIX_HIGH.codepoint)
                    .on_hover_text("Tidy up: rearrange the shown notes next to this table or group")
                    .clicked()
                {
                    actions.push(PanelAction::Tidy(anchor.clone()));
                }
                let (icon, tip) = if any_hidden {
                    (
                        egui_icons::icons::ICON_VISIBILITY.codepoint,
                        "Show all on canvas",
                    )
                } else {
                    (egui_icons::icons::ICON_VISIBILITY_OFF.codepoint, "Hide all")
                };
                if ui.small_button(icon).on_hover_text(tip).clicked() {
                    actions.push(if any_hidden {
                        PanelAction::ShowAll(anchor.clone())
                    } else {
                        PanelAction::HideAll(anchor.clone())
                    });
                }
            }
            let label = format!("{icon} {}", notes::anchor_label(state, anchor));
            if ui
                .add(
                    egui::Label::new(egui::RichText::new(label).strong())
                        .sense(egui::Sense::click())
                        .truncate(),
                )
                .on_hover_text("Go to this table or group")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                actions.push(PanelAction::JumpAnchor(anchor.clone()));
            }
        });
    });
}

fn note_row(
    ui: &mut egui::Ui,
    state: &DiagramState,
    id: &str,
    visible: &HashSet<String>,
    read_only: bool,
    confirm: &mut Option<String>,
    actions: &mut Vec<PanelAction>,
) {
    let Some(n) = state.notes.iter().find(|n| n.id == id) else {
        return;
    };
    let attached = notes::anchor_exists(state, &n.anchor);
    let shown = visible.contains(id);
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(6, 4))
        .corner_radius(4.0)
        .fill(if shown {
            n.color.gamma_multiply(0.12)
        } else {
            egui::Color32::TRANSPARENT
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                swatch(ui, n.color, 10.0, false);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if confirm.as_deref() == Some(id) {
                        if ui.add(style::btn_danger_ctx(ui.ctx(), "Remove")).clicked() {
                            actions.push(PanelAction::Remove(id.to_string()));
                            *confirm = None;
                        }
                        if ui.small_button("Cancel").clicked() {
                            *confirm = None;
                        }
                        return;
                    }
                    if !read_only {
                        if ui
                            .small_button(egui_icons::icons::ICON_DELETE.codepoint)
                            .on_hover_text("Remove note")
                            .clicked()
                        {
                            *confirm = Some(id.to_string());
                        }
                        if ui
                            .small_button(egui_icons::icons::ICON_EDIT.codepoint)
                            .on_hover_text("Edit note")
                            .clicked()
                        {
                            actions.push(PanelAction::Edit(id.to_string()));
                        }
                    }
                    if attached {
                        if !read_only {
                            let pin =
                                egui::RichText::new(egui_icons::icons::ICON_PUSH_PIN.codepoint)
                                    .color(if n.pinned {
                                        ui.visuals().strong_text_color()
                                    } else {
                                        ui.visuals().weak_text_color()
                                    });
                            let tip = if n.pinned {
                                "Pinned for everyone. Click to unpin."
                            } else {
                                "Pin: always show this note to everyone who opens this diagram"
                            };
                            if ui.small_button(pin).on_hover_text(tip).clicked() {
                                actions.push(PanelAction::Pin(id.to_string()));
                            }
                        }
                        let (icon, tip) = if shown {
                            (
                                egui_icons::icons::ICON_VISIBILITY.codepoint,
                                "Hide from canvas",
                            )
                        } else {
                            (
                                egui_icons::icons::ICON_VISIBILITY_OFF.codepoint,
                                "Show on canvas",
                            )
                        };
                        let eye = ui.add_enabled(!n.pinned, egui::Button::new(icon).small());
                        let eye = if n.pinned {
                            eye.on_disabled_hover_text("Pinned notes are always shown")
                        } else {
                            eye.on_hover_text(tip)
                        };
                        if eye.clicked() {
                            actions.push(PanelAction::Toggle(id.to_string()));
                        }
                    }
                    let title = egui::RichText::new(notes::display_title(n)).strong();
                    let resp = ui.add(
                        egui::Label::new(title)
                            .sense(egui::Sense::click())
                            .truncate(),
                    );
                    if attached
                        && resp
                            .on_hover_text("Show on canvas and go to it")
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                    {
                        actions.push(PanelAction::Reveal(id.to_string()));
                    }
                });
            });
            let snip = snippet(n);
            if !snip.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(snip).small()).truncate());
            }
            let mut meta = note_meta(n);
            if !attached {
                let kind = match n.anchor {
                    NoteAnchor::Table(_) => "table",
                    NoteAnchor::Group(_) => "group",
                };
                meta = format!("was on {kind} {} · {meta}", n.anchor.id());
            }
            if !meta.is_empty() {
                ui.label(egui::RichText::new(meta).small().weak());
            }
        });
}

/// Modal editor note: judul, warna, pin, isi Markdown dengan pratinjau
/// langsung dan saran nama tabel saat mengetik `[[`.
pub fn render_note_editor(ctx: &egui::Context, state: &mut DiagramState) {
    let Some(mut draft) = state.note_editor.take() else {
        return;
    };
    if !notes::anchor_exists(state, &draft.anchor) && draft.note_id.is_none() {
        return;
    }
    let anchor_label = notes::anchor_label(state, &draft.anchor);
    let kind = match draft.anchor {
        NoteAnchor::Table(_) => "table",
        NoteAnchor::Group(_) => "group",
    };
    let heading = if draft.note_id.is_some() {
        format!("Edit note on {kind} {anchor_label}")
    } else {
        format!("New note on {kind} {anchor_label}")
    };
    let mut close = false;
    let mut save = false;
    let mut remove = false;
    let body_id = egui::Id::new("diagram_note_editor_body");

    style::render_modal_backdrop(ctx, "diagram_note_editor_backdrop", true);
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(420.0, 900.0);
    let editor_h = (screen.height() - 330.0).clamp(160.0, 420.0);

    egui::Window::new("Diagram note")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            style::render_modal_header(ui, heading, &mut close);
            ui.label(
                egui::RichText::new(
                    "Markdown. Link other tables with [[table]], [[table#column]] or \
                     [[table|label]]; each link is drawn as a dashed line. Notes are saved \
                     with the diagram, so everyone who opens it can read them.",
                )
                .weak()
                .small(),
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut draft.title).hint_text("Title (optional)"),
                    (ui.available_width() - 330.0).max(160.0),
                    None,
                );
                ui.add_space(8.0);
                for c in NOTE_COLORS {
                    if swatch(ui, c, 18.0, draft.color == c)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        draft.color = c;
                    }
                }
                ui.add_space(8.0);
                ui.checkbox(&mut draft.pinned, "Pin")
                    .on_hover_text("Always show this note to everyone who opens this diagram");
                ui.checkbox(&mut draft.preview, "Preview");
            });
            ui.add_space(6.0);

            let mut edit_output: Option<egui::text_edit::TextEditOutput> = None;
            let mut editor = |ui: &mut egui::Ui, body: &mut String| {
                egui::ScrollArea::vertical()
                    .id_salt("diagram_note_editor_scroll")
                    .max_height(editor_h)
                    .min_scrolled_height(editor_h)
                    .show(ui, |ui| {
                        edit_output = Some(
                            egui::TextEdit::multiline(body)
                                .id(body_id)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("## Purpose\nWhat this table is for…\n\nSee [[orders]].")
                                .desired_rows(14)
                                .desired_width(f32::INFINITY)
                                .lock_focus(true)
                                .show(ui),
                        );
                    });
            };
            if draft.preview {
                let nodes = &state.nodes;
                let color = draft.color;
                ui.columns(2, |cols| {
                    editor(&mut cols[0], &mut draft.body);
                    egui::Frame::NONE
                        .fill(color)
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::same(10))
                        .show(&mut cols[1], |ui| {
                            style_card_body(ui, 1.0, color);
                            egui::ScrollArea::vertical()
                                .id_salt("diagram_note_editor_preview")
                                .max_height(editor_h - 20.0)
                                .min_scrolled_height(editor_h - 20.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if draft.body.trim().is_empty() {
                                        ui.label(egui::RichText::new("Preview").italics().weak());
                                    } else {
                                        // Klik link di pratinjau diabaikan.
                                        let _ = show_markdown(ui, nodes, &draft.body);
                                    }
                                });
                        });
                });
            } else {
                editor(ui, &mut draft.body);
            }

            // Saran tabel saat kursor berada setelah `[[`.
            let cursor = edit_output
                .as_ref()
                .filter(|o| o.response.response.has_focus())
                .and_then(|o| o.cursor_range)
                .map(|r| r.primary.index.0);
            let pending =
                cursor.and_then(|c| notes::pending_wikilink(&draft.body, c).map(|p| (c, p)));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| match &pending {
                Some((cursor, (start, typed))) => {
                    let names = notes::table_suggestions(&state.nodes, typed, 8);
                    if names.is_empty() {
                        ui.label(egui::RichText::new("No matching table").small().weak());
                    } else {
                        ui.label(egui::RichText::new("Link table:").small().weak());
                    }
                    for name in names {
                        if ui.small_button(&name).clicked() {
                            insert_link(ctx, body_id, &mut draft.body, *start, *cursor, &name);
                        }
                    }
                }
                None => {
                    let links = notes::parse_wikilinks(&draft.body);
                    let missing: Vec<String> = links
                        .iter()
                        .filter(|l| notes::resolve_link(&state.nodes, l).is_none())
                        .map(|l| l.target.clone())
                        .collect();
                    let text = if !missing.is_empty() {
                        egui::RichText::new(format!("Not in this diagram: {}", missing.join(", ")))
                            .small()
                            .color(ui.visuals().warn_fg_color)
                    } else if links.is_empty() {
                        egui::RichText::new("Type [[ to link a table.")
                            .small()
                            .weak()
                    } else {
                        egui::RichText::new(format!("{} table link(s)", links.len()))
                            .small()
                            .weak()
                    };
                    ui.label(text);
                }
            });

            ui.add_space(10.0);
            let can_save = !draft.title.trim().is_empty() || !draft.body.trim().is_empty();
            ui.horizontal(|ui| {
                if draft.note_id.is_some() {
                    if draft.confirm_delete {
                        if ui
                            .add(style::btn_danger_ctx(ui.ctx(), "Remove note"))
                            .clicked()
                        {
                            remove = true;
                        }
                        if ui.button("Keep").clicked() {
                            draft.confirm_delete = false;
                        }
                    } else if ui
                        .add(egui::Button::new("Remove").min_size(egui::vec2(0.0, 28.0)))
                        .clicked()
                    {
                        draft.confirm_delete = true;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            can_save,
                            style::btn_primary_ctx(ui.ctx(), "Save")
                                .min_size(egui::vec2(72.0, 28.0)),
                        )
                        .on_hover_text("Save (Cmd/Ctrl+Enter)")
                        .clicked()
                    {
                        save = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 28.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            if can_save
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
            {
                save = true;
            }
        });

    if remove {
        if let Some(id) = &draft.note_id {
            remove_note(state, id);
        }
        return;
    }
    if save {
        save_draft(state, draft);
        return;
    }
    if !close {
        state.note_editor = Some(draft);
    }
}

/// Ganti `[[ketikan` (dari `start` sampai `cursor`, indeks karakter) dengan
/// `[[name]]` dan pindahkan kursor ke belakangnya.
fn insert_link(
    ctx: &egui::Context,
    id: egui::Id,
    body: &mut String,
    start: usize,
    cursor: usize,
    name: &str,
) {
    let byte = |c: usize| body.char_indices().nth(c).map_or(body.len(), |(b, _)| b);
    let (from, to) = (byte(start), byte(cursor));
    // Lewati `]]` yang sudah ada tepat setelah kursor.
    let to = if body[to..].starts_with("]]") {
        to + 2
    } else {
        to
    };
    let inserted = format!("{name}]]");
    body.replace_range(from..to, &inserted);
    let new_cursor = start + inserted.chars().count();
    if let Some(mut st) = egui::TextEdit::load_state(ctx, id) {
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(new_cursor),
        )));
        st.store(ctx, id);
    }
    ctx.memory_mut(|m| m.request_focus(id));
}

fn save_draft(state: &mut DiagramState, draft: NoteDraft) {
    let now = chrono::Utc::now().to_rfc3339();
    let title = draft.title.trim().to_string();
    match &draft.note_id {
        Some(id) => {
            if let Some(n) = state.notes.iter_mut().find(|n| &n.id == id) {
                n.title = title;
                n.body = draft.body;
                n.color = draft.color;
                n.pinned = draft.pinned;
                n.updated_at = now;
            }
        }
        None => {
            let id = notes::new_note_id(&state.notes);
            state.notes.push(DiagramNote {
                id: id.clone(),
                anchor: draft.anchor,
                title,
                body: draft.body,
                color: draft.color,
                offset: None,
                size: notes::DEFAULT_NOTE_SIZE,
                pinned: draft.pinned,
                author: notes::current_author(),
                created_at: now.clone(),
                updated_at: now,
            });
            state.show_notes = true;
            state.open_notes.insert(id);
        }
    }
    state.save_requested = true;
}
