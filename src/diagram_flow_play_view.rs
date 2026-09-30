//! Gambar dan kontrol pemutaran flow card, plus detail proses yang menempel
//! di bawah card terpilih.
//!
//! Pemutaran dimulai dari double-click card: partikel request masuk ke
//! card, lalu per langkah partikel berjalan di garis card → tabel sesuai
//! arah operasinya sementara tabel dan kolomnya berdenyut, lalu partikel
//! respons keluar. Timeline murninya ada di `crate::diagram_flow_play`;
//! geometri card dan garis diberikan pemanggil lewat closure supaya modul
//! ini tidak bergantung pada cara card ditata.

use std::collections::HashSet;

use eframe::egui;

use crate::diagram_flow::{self, FlowDirection, op_direction};
use crate::diagram_flow_play::{self as play, PlayItem, PlayPhase};
use crate::diagram_lod::{Lod, curve_visible, lod_for_zoom};
use crate::models::structs::{DiagramNode, DiagramState, FlowCard, FlowOp, FlowStepKind};

/// Operasi baca.
pub const READ_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 200, 255);
/// Operasi tulis (insert, update, upsert, call, publish).
pub const WRITE_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 220, 140);
pub const DELETE_COLOR: egui::Color32 = egui::Color32::from_rgb(239, 83, 80);
pub const UNKNOWN_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 160);
/// Warna request / respons di luar card.
const REQUEST_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 196, 60);
/// Jarak (layar) asal partikel request dan tujuan partikel respons.
const OUTSIDE_PX: f32 = 90.0;
/// Tinggi header card (koordinat diagram), sama dengan
/// `diagram_flow_layout::CARD_HEADER_H`.
const CARD_HEADER_H: f32 = 34.0;
/// Interval repaint selama pemutaran terlihat (≈30 fps).
const FRAME_MS: u64 = 33;
/// Interval repaint selama pemutaran berjalan tetapi card di luar layar.
const OFFSCREEN_MS: u64 = 250;

/// Rect layar card (sudah mengikuti LOD) menurut id card.
pub type CardRectFn<'a> = &'a dyn Fn(&str) -> Option<egui::Rect>;
/// Titik kontrol kurva layar dari card ke tabel (card id, id tabel),
/// sama dengan garis proses yang digambar kanvas.
pub type CurveFn<'a> = &'a dyn Fn(&str, &str) -> Option<[egui::Pos2; 4]>;

/// Warna garis / partikel untuk operasi `op`.
pub fn op_color(op: Option<FlowOp>) -> egui::Color32 {
    match op {
        Some(FlowOp::Read | FlowOp::Consume) => READ_COLOR,
        Some(FlowOp::Delete) => DELETE_COLOR,
        Some(FlowOp::Insert | FlowOp::Update | FlowOp::Upsert | FlowOp::Call | FlowOp::Publish) => {
            WRITE_COLOR
        }
        Some(FlowOp::Unknown) | None => UNKNOWN_COLOR,
    }
}

/// Ikon jenis langkah (`egui_icons`, bukan glyph Unicode mentah).
pub fn step_icon(kind: FlowStepKind) -> &'static str {
    use egui_icons::icons::*;
    match kind {
        FlowStepKind::Auth => ICON_LOCK.codepoint,
        FlowStepKind::Validate => ICON_RULE.codepoint,
        FlowStepKind::Db => ICON_STORAGE.codepoint,
        FlowStepKind::External => ICON_CLOUD.codepoint,
        FlowStepKind::Queue => ICON_QUEUE.codepoint,
        FlowStepKind::Cache => ICON_MEMORY.codepoint,
        FlowStepKind::Logic => ICON_FUNCTIONS.codepoint,
        FlowStepKind::Branch => ICON_CALL_SPLIT.codepoint,
        FlowStepKind::Respond => ICON_REPLY.codepoint,
        FlowStepKind::Unknown => ICON_CODE.codepoint,
    }
}

fn card_of<'a>(state: &'a DiagramState, id: &str) -> Option<&'a FlowCard> {
    state.flow_cards.iter().find(|c| c.id == id)
}

/// Item timeline card (langkah, atau langkah semu per tabel).
pub fn items_of(state: &DiagramState, card: &FlowCard) -> Vec<PlayItem> {
    play::play_items(card, &diagram_flow::tables_of(state, card))
}

/// Mulai (atau ulang) pemutaran card dari awal. Animasi aliran relasi
/// tabel dihentikan supaya partikelnya tidak bercampur.
pub fn start_playback(state: &mut DiagramState, card_id: &str) {
    if card_of(state, card_id).is_none() {
        return;
    }
    state.flow_anim = None;
    state.flow_play = Some(play::new_playback(card_id));
}

/// Lepas pilihan, fokus, dan pemutaran flow card (Esc / klik kanvas kosong).
pub fn clear(state: &mut DiagramState) {
    state.selected_flow = None;
    state.focus_flow = None;
    state.flow_play = None;
    state.flow_open_step = None;
}

/// Majukan pemutaran ke waktu `now`. Pemutaran untuk card yang sudah hilang
/// dibuang. Mengembalikan `true` bila masih berjalan.
pub fn tick(state: &mut DiagramState, now: f64) -> bool {
    let Some(p) = &state.flow_play else {
        return false;
    };
    let Some(card) = card_of(state, &p.card_id) else {
        state.flow_play = None;
        return false;
    };
    let duration = play::play_duration(&items_of(state, card));
    match &mut state.flow_play {
        Some(p) => play::advance(p, now, duration),
        None => false,
    }
}

/// Fase pemutaran card `card` saat ini, bila card itu yang diputar.
pub fn phase_of(state: &DiagramState, card: &FlowCard) -> Option<PlayPhase> {
    let p = state.flow_play.as_ref().filter(|p| p.card_id == card.id)?;
    Some(play::play_phase(&items_of(state, card), p.position))
}

/// Indeks langkah (`FlowCard::steps`) yang sedang diputar; dipakai card
/// untuk menyalakan baris langkahnya.
pub fn active_step(state: &DiagramState, card: &FlowCard) -> Option<usize> {
    let p = state.flow_play.as_ref().filter(|p| p.card_id == card.id)?;
    let items = items_of(state, card);
    match play::play_phase(&items, p.position) {
        PlayPhase::Step { index, .. } => items.get(index)?.step,
        _ => None,
    }
}

/// Spasi = Play/Pause selama ada pemutaran dan tidak sedang mengetik.
/// Mengembalikan `true` bila Spasi milik pemutaran (hand tool tidak boleh
/// ikut aktif).
pub fn handle_space(ui: &egui::Ui, state: &mut DiagramState, typing: bool) -> bool {
    if typing {
        return false;
    }
    let Some(card) = state
        .flow_play
        .as_ref()
        .and_then(|p| card_of(state, &p.card_id))
    else {
        return false;
    };
    let duration = play::play_duration(&items_of(state, card));
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
        && let Some(p) = &mut state.flow_play
    {
        play::toggle_play(p, duration);
    }
    true
}

/// Denyut 0..1 untuk sorotan.
fn pulse(now: f64) -> f32 {
    0.55 + 0.45 * ((now * 5.0).sin() as f32 * 0.5 + 0.5)
}

/// Partikel berekor di posisi `t` (0..1) lintasan `at`, bergerak ke arah t naik.
fn particle(
    painter: &egui::Painter,
    at: &dyn Fn(f32) -> egui::Pos2,
    t: f32,
    color: egui::Color32,
    s: f32,
) {
    for tail in 0..4 {
        let tt = t - tail as f32 * 0.03;
        if tt < 0.0 {
            break;
        }
        let p = at(tt.min(1.0));
        let fade = 1.0 - tail as f32 * 0.25;
        let r = (3.4 - tail as f32 * 0.6) * s;
        painter.circle_filled(p, r, egui::Color32::WHITE.linear_multiply(fade));
        if tail == 0 {
            painter.circle_filled(p, r * 2.4, color.linear_multiply(0.3));
        }
    }
}

/// Label kecil berlatar gelap di layar.
fn chip(painter: &egui::Painter, at: egui::Pos2, text: String, color: egui::Color32) {
    let galley =
        painter.layout_no_wrap(text, egui::FontId::proportional(11.0), egui::Color32::WHITE);
    let r = egui::Rect::from_center_size(at, galley.size() + egui::vec2(12.0, 6.0));
    painter.rect_filled(r, 4.0, egui::Color32::from_black_alpha(215));
    painter.rect_stroke(
        r,
        4.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    painter.galley(r.min + egui::vec2(6.0, 3.0), galley, egui::Color32::WHITE);
}

/// Tinggi header node (koordinat diagram), sama dengan
/// `diagram_view::node_header_height`.
fn node_header_height(node: &DiagramNode) -> f32 {
    if node.database_name.is_some() {
        30.0
    } else {
        24.0
    }
}

/// Sorotan tabel target: bingkai + header berdenyut, dan baris `columns`
/// (hanya LOD Detail).
fn pulse_table(
    painter: &egui::Painter,
    node: &DiagramNode,
    columns: &[String],
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    scale: f32,
    color: egui::Color32,
    k: f32,
    detail: bool,
) {
    let r = egui::Rect::from_min_size(to_screen(node.pos), node.size * scale);
    painter.rect_stroke(
        r.expand(3.0 * scale.max(0.5)),
        6.0 * scale,
        egui::Stroke::new(2.5 * scale.max(0.6), color.linear_multiply(k)),
        egui::StrokeKind::Outside,
    );
    let header = egui::Rect::from_min_size(
        r.min,
        egui::vec2(r.width(), node_header_height(node) * scale),
    );
    painter.rect_filled(header, 4.0 * scale, color.linear_multiply(0.30 * k));
    if !detail {
        return;
    }
    for col in columns {
        if !node.columns.iter().any(|c| c == col) {
            continue;
        }
        let y = crate::diagram_view::column_anchor_y(node, col);
        let row = egui::Rect::from_min_max(
            to_screen(egui::pos2(node.pos.x, y - 8.0)),
            to_screen(egui::pos2(node.pos.x + node.size.x, y + 8.0)),
        );
        painter.rect_filled(row, 0.0, color.linear_multiply(0.22 * k));
        painter.rect_stroke(
            row,
            0.0,
            egui::Stroke::new(1.0 * scale.max(0.6), color.linear_multiply(k)),
            egui::StrokeKind::Inside,
        );
    }
}

/// Gambar pemutaran yang sedang aktif: partikel request/respons, garis dan
/// partikel langkah aktif, denyut tabel dan kolom target, caption langkah.
/// Dipanggil setelah tabel digambar. Meminta repaint hanya selama
/// pemutaran berjalan; saat jeda atau `Done` tidak ada repaint.
pub fn draw_playback(
    ui: &egui::Ui,
    state: &DiagramState,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    card_rect: CardRectFn,
    curve: CurveFn,
    now: f64,
) {
    let Some(p) = &state.flow_play else {
        return;
    };
    let Some(card) = card_of(state, &p.card_id) else {
        return;
    };
    let clip = ui.clip_rect();
    let visible = card_rect(&card.id).is_some_and(|r| clip.intersects(r.expand(OUTSIDE_PX)));
    if p.playing {
        let ms = if visible { FRAME_MS } else { OFFSCREEN_MS };
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(ms));
    }
    let Some(cr) = card_rect(&card.id) else {
        return;
    };
    let items = items_of(state, card);
    let phase = play::play_phase(&items, p.position);
    let painter = ui.painter();
    let scale = state.zoom;
    let s = scale.max(0.6);
    let detail = lod_for_zoom(scale) == Lod::Detail;
    let k = if p.playing { pulse(now) } else { 1.0 };
    let header_y = cr.top() + (CARD_HEADER_H * scale * 0.5).min(cr.height() * 0.5);
    let entry = egui::pos2(cr.left(), header_y);
    let outside = entry - egui::vec2(OUTSIDE_PX, 0.0);

    match phase {
        PlayPhase::Response(t) => {
            painter.line_segment(
                [entry, outside],
                egui::Stroke::new(1.5 * s, REQUEST_COLOR.linear_multiply(0.5)),
            );
            particle(painter, &|t| entry.lerp(outside, t), t, REQUEST_COLOR, s);
        }
        PlayPhase::Step { index, t } => {
            let Some(item) = items.get(index) else {
                return;
            };
            let step = item.step.and_then(|i| card.steps.get(i));
            let op = step.and_then(|s| s.op);
            let color = op_color(op);
            let number = index + 1;
            let title = match (step, &item.table) {
                (Some(st), _) if !st.title.trim().is_empty() => st.title.clone(),
                (_, Some(tb)) => format!("Uses {}", table_title(state, tb)),
                _ => "Step".to_string(),
            };
            let target = item
                .table
                .as_deref()
                .and_then(|tb| Some((tb, state.nodes.iter().find(|n| n.id == tb)?)));
            let Some((table_id, node)) = target else {
                // Langkah tanpa tabel: card saja yang berdenyut.
                painter.rect_stroke(
                    cr.expand(2.0 * s),
                    6.0 * scale,
                    egui::Stroke::new(2.0 * s, color.linear_multiply(k)),
                    egui::StrokeKind::Outside,
                );
                if detail {
                    chip(
                        painter,
                        cr.center_bottom() + egui::vec2(0.0, 14.0),
                        format!("{number}. {title}"),
                        color,
                    );
                }
                return;
            };
            let columns = step.map(|s| s.columns.as_slice()).unwrap_or(&[]);
            pulse_table(painter, node, columns, to_screen, scale, color, k, detail);
            let Some(ctrl) = curve(&card.id, table_id) else {
                return;
            };
            if !curve_visible(&ctrl, clip, 24.0) {
                return;
            }
            let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
                ctrl,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::NONE,
            );
            let points: Vec<egui::Pos2> =
                (0..=32).map(|i| bezier.sample(i as f32 / 32.0)).collect();
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(2.5 * s, color.linear_multiply(0.8)),
            ));
            let dir = if item.step.is_none() {
                FlowDirection::ToTarget
            } else {
                op_direction(op)
            };
            let along: &dyn Fn(f32) -> egui::Pos2 = &|t| bezier.sample(t);
            let back: &dyn Fn(f32) -> egui::Pos2 = &|t| bezier.sample(1.0 - t);
            match dir {
                FlowDirection::FromTarget => particle(painter, back, t, color, s),
                FlowDirection::Both => {
                    if t < 0.5 {
                        particle(painter, along, t * 2.0, color, s);
                    } else {
                        particle(painter, back, (t - 0.5) * 2.0, color, s);
                    }
                }
                FlowDirection::ToTarget | FlowDirection::Unknown => {
                    particle(painter, along, t, color, s)
                }
            }
            // Caption di dekat ujung tabel, supaya tidak menutupi chip nomor
            // langkah yang digambar garis proses di tengah kurva.
            chip(
                painter,
                bezier.sample(0.8) - egui::vec2(0.0, 16.0),
                format!("{number}. {title}"),
                color,
            );
        }
        PlayPhase::Done => {}
    }
}

fn table_title(state: &DiagramState, id: &str) -> String {
    state
        .nodes
        .iter()
        .find(|n| n.id == id)
        .map_or_else(|| id.to_string(), |n| n.title.clone())
}

/// Tombol ikon kecil di bilah kontrol.
fn bar_button(ui: &mut egui::Ui, icon: &str, tip: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(16.0))
            .frame(false)
            .min_size(egui::vec2(26.0, 26.0)),
    )
    .on_hover_text(tip)
    .clicked()
}

/// Bilah kontrol melayang di bawah card yang sedang diputar: Previous,
/// Play/Pause, Next, kecepatan, Replay, posisi langkah, Close. `card` =
/// rect layar card. Ukurannya tetap (tidak ikut zoom) supaya selalu bisa
/// diklik.
pub fn render_play_controls(ui: &mut egui::Ui, state: &mut DiagramState, card: Option<egui::Rect>) {
    let (Some(cr), Some(p)) = (card, state.flow_play.clone()) else {
        return;
    };
    let Some(c) = card_of(state, &p.card_id) else {
        return;
    };
    let items = items_of(state, c);
    let duration = play::play_duration(&items);
    let clip = ui.clip_rect();
    let size = egui::vec2(262.0, 34.0);
    let mut min = egui::pos2(cr.center().x - size.x / 2.0, cr.bottom() + 8.0);
    min.x = min.x.clamp(
        clip.left() + 8.0,
        (clip.right() - size.x - 8.0).max(clip.left() + 8.0),
    );
    min.y = min.y.clamp(
        clip.top() + 8.0,
        (clip.bottom() - size.y - 8.0).max(clip.top() + 8.0),
    );
    let bar = egui::Rect::from_min_size(min, size);
    if !clip.intersects(bar) {
        return;
    }
    let visuals = ui.visuals().clone();
    ui.painter().rect_filled(
        bar.translate(egui::vec2(1.0, 3.0)),
        8.0,
        egui::Color32::from_black_alpha(80),
    );
    ui.painter().rect_filled(bar, 8.0, visuals.window_fill);
    ui.painter().rect_stroke(
        bar,
        8.0,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Middle,
    );
    // Klik di sela tombol tidak diteruskan ke kanvas.
    let _ = ui.interact(
        bar,
        ui.id().with("flow_play_bar"),
        egui::Sense::click_and_drag(),
    );

    let mut new = p.clone();
    let mut close = false;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar.shrink2(egui::vec2(6.0, 4.0)))
            .id_salt("flow_play_bar_ui")
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
    use egui_icons::icons::*;
    if bar_button(&mut child, ICON_SKIP_PREVIOUS.codepoint, "Previous step") {
        play::seek_item(&mut new, &items, play::previous_item(&items, p.position));
    }
    let (icon, tip) = if p.playing {
        (ICON_PAUSE.codepoint, "Pause (Space)")
    } else {
        (ICON_PLAY_ARROW.codepoint, "Play (Space)")
    };
    if bar_button(&mut child, icon, tip) {
        play::toggle_play(&mut new, duration);
    }
    if bar_button(&mut child, ICON_SKIP_NEXT.codepoint, "Next step") {
        play::seek_item(&mut new, &items, play::next_item(&items, p.position));
    }
    let speed = if p.speed.fract() == 0.0 {
        format!("{:.0}×", p.speed)
    } else {
        format!("{}×", p.speed)
    };
    if child
        .add(
            egui::Button::new(egui::RichText::new(speed).monospace())
                .min_size(egui::vec2(34.0, 22.0)),
        )
        .on_hover_text("Playback speed")
        .clicked()
    {
        new.speed = play::next_speed(p.speed);
        new.last_tick = None;
    }
    if bar_button(&mut child, ICON_REPLAY.codepoint, "Replay from the start") {
        new = play::new_playback(&p.card_id);
        new.speed = p.speed;
    }
    let pos_label = match play::play_phase(&items, p.position) {
        PlayPhase::Step { index, .. } => format!("Step {}/{}", index + 1, items.len()),
        PlayPhase::Response(_) => "Response".to_string(),
        PlayPhase::Done => "Done".to_string(),
    };
    child.add_space(4.0);
    child.label(egui::RichText::new(pos_label).small());
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if bar_button(ui, ICON_CLOSE.codepoint, "Stop the animation") {
            close = true;
        }
    });

    if close {
        state.flow_play = None;
    } else if new != p {
        state.flow_play = Some(new);
    }
}

/// Lencana CRUD kecil berwarna; `s` = skala teks footer card.
fn crud_badge(ui: &mut egui::Ui, ch: char, s: f32) {
    let color = match ch {
        'R' => READ_COLOR,
        'D' => DELETE_COLOR,
        _ => WRITE_COLOR,
    };
    egui::Frame::NONE
        .fill(color.linear_multiply(0.2))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(ch.to_string())
                    .family(egui::FontFamily::Monospace)
                    .size(10.5 * s)
                    .strong()
                    .color(color),
            );
        });
}

/// Bagian bawah card terpilih, di dalam bingkai card yang sama (`card_rect`,
/// layar, sudah termasuk `state.flow_footer_h`) dan ikut pan/zoom: tabel yang
/// disentuh (lencana CRUD + nomor langkah, klik = lompat ke tabel). Play,
/// Open Request dan Generate tidak ada di sini: klik card langsung memutar
/// prosesnya, sisanya ada di menu klik kanan card. Tinggi yang terukur disimpan ke
/// `state.flow_footer_h` supaya card memanjang setinggi itu di frame
/// berikutnya. `card_rect` = `None` saat card tidak tampil penuh (LOD kecil,
/// di luar layar). `canvas` = rect kanvas diagram.
pub fn render_process_panel(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    card_rect: Option<egui::Rect>,
    canvas: egui::Rect,
    now: f64,
) {
    let Some(id) = state.selected_flow.clone() else {
        return;
    };
    let Some(card) = card_of(state, &id).cloned() else {
        state.selected_flow = None;
        return;
    };
    let Some(card_rect) = card_rect.filter(|r| canvas.intersects(*r)) else {
        return;
    };
    let tables = diagram_flow::tables_of(state, &card);
    let table_refs: Vec<&str> = tables.iter().map(String::as_str).collect();
    let uses = diagram_flow::table_uses(&card, &table_refs);
    let known: HashSet<&str> = state.nodes.iter().map(|n| n.id.as_str()).collect();

    // Teks dan jarak diskalakan seperti card, dibatasi supaya tetap terbaca.
    let s = state.zoom.clamp(0.5, 1.4);
    let margin = (10.0 * s).round();
    let top = card_rect.bottom() - state.flow_footer_h * state.zoom;

    let mut jump: Option<String> = None;
    let shown = egui::Area::new(ui.id().with(("diagram_flow_process", &card.id)))
        .fixed_pos(egui::pos2(card_rect.left(), top))
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            ui.set_clip_rect(canvas);
            let style = ui.style_mut();
            for font in style.text_styles.values_mut() {
                font.size *= s;
            }
            style.spacing.item_spacing *= s;
            style.spacing.button_padding *= s;
            style.spacing.interact_size *= s;
            // Tanpa isi dan bingkai: latar dan tepinya digambar card.
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: margin as i8,
                    right: margin as i8,
                    top: 0,
                    bottom: margin as i8,
                })
                .show(ui, |ui| {
                    ui.set_width((card_rect.width() - 2.0 * margin).max(80.0));
                    ui.separator();
                    ui.label(egui::RichText::new("Tables involved").strong());
                    if uses.is_empty() {
                        ui.label(egui::RichText::new("No tables linked.").weak());
                    }
                    for u in &uses {
                        ui.horizontal_wrapped(|ui| {
                            let badges = u.crud_badges();
                            if badges.is_empty() {
                                ui.label(egui::RichText::new("?").small().weak())
                                    .on_hover_text("Operation unknown");
                            }
                            for ch in badges {
                                crud_badge(ui, ch, s);
                            }
                            let title = table_title(state, &u.table);
                            let exists = known.contains(u.table.as_str());
                            let resp = ui.add_enabled(
                                exists,
                                egui::Button::new(egui::RichText::new(title).monospace())
                                    .frame(false),
                            );
                            if resp
                                .on_hover_text("Jump to this table")
                                .on_disabled_hover_text("This table is not in the diagram")
                                .clicked()
                            {
                                jump = Some(u.table.clone());
                            }
                            if !u.steps.is_empty() {
                                let nums: Vec<String> =
                                    u.steps.iter().map(|i| (i + 1).to_string()).collect();
                                let noun = if nums.len() == 1 { "step" } else { "steps" };
                                ui.label(
                                    egui::RichText::new(format!("{noun} {}", nums.join(", ")))
                                        .small()
                                        .weak(),
                                );
                            }
                        });
                    }
                });
        });

    // Card memanjang setinggi bagian bawah ini mulai frame berikutnya.
    let height = shown.response.rect.height() / state.zoom.max(0.01);
    if (height - state.flow_footer_h).abs() > 0.5 {
        state.flow_footer_h = height;
        ui.ctx().request_repaint();
    }
    if let Some(table) = jump
        && let Some(n) = state.nodes.iter().find(|n| n.id == table)
    {
        let center = n.pos + n.size / 2.0;
        let zoom = state.zoom.max(crate::diagram_lod::DETAIL_MIN_ZOOM);
        crate::diagram_view::animate_view_to(state, center, zoom, canvas.size(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{EndpointLink, FlowStep, FlowTarget, FlowTrigger};

    fn fixture() -> DiagramState {
        let node = |id: &str, x: f32| DiagramNode {
            id: id.into(),
            title: id.into(),
            pos: egui::pos2(x, 0.0),
            size: egui::vec2(200.0, 120.0),
            columns: vec!["id".into(), "email".into()],
            ..Default::default()
        };
        DiagramState {
            nodes: vec![node("users", 600.0), node("orders", 900.0)],
            endpoint_links: vec![EndpointLink {
                table: "orders".into(),
                method: "POST".into(),
                path: "/orders".into(),
                summary: String::new(),
                request_id: None,
                repo_key: None,
                source: None,
            }],
            flow_cards: vec![FlowCard {
                id: "flw_1".into(),
                trigger: FlowTrigger {
                    method: "POST".into(),
                    target: "/orders".into(),
                    ..Default::default()
                },
                steps: vec![
                    FlowStep {
                        kind: FlowStepKind::Db,
                        title: "Load user".into(),
                        target: Some(FlowTarget::Table("users".into())),
                        op: Some(FlowOp::Read),
                        columns: vec!["email".into()],
                        ..Default::default()
                    },
                    FlowStep {
                        kind: FlowStepKind::Db,
                        title: "Insert order".into(),
                        target: Some(FlowTarget::Table("orders".into())),
                        op: Some(FlowOp::Insert),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            is_centered: true,
            ..Default::default()
        }
    }

    /// Satu frame tanpa jendela: tick + gambar pemutaran. Mengembalikan
    /// jeda repaint yang diminta (`Duration::MAX` = tidak ada).
    fn frame(ctx: &egui::Context, state: &mut DiagramState, time: f64) -> std::time::Duration {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1000.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| {
            let now = ui.input(|i| i.time);
            tick(state, now);
            let card = |_: &str| {
                Some(egui::Rect::from_min_size(
                    egui::pos2(100.0, 100.0),
                    egui::vec2(300.0, 120.0),
                ))
            };
            let curve = |_: &str, _: &str| {
                Some([
                    egui::pos2(400.0, 120.0),
                    egui::pos2(500.0, 120.0),
                    egui::pos2(550.0, 20.0),
                    egui::pos2(600.0, 20.0),
                ])
            };
            draw_playback(ui, state, &|p| p, &card, &curve, now);
        });
        // Atlas font berubah di frame awal; delta tekstur harus dibuang eksplisit.
        let mut out = out;
        out.textures_delta.clear();
        out.viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(std::time::Duration::MAX, |v| v.repaint_delay)
    }

    #[test]
    fn playback_stops_by_itself_and_paused_frames_do_not_repaint() {
        let ctx = egui::Context::default();
        let mut state = fixture();
        // Beberapa frame awal untuk layout font egui.
        for t in 0..3 {
            frame(&ctx, &mut state, t as f64 * 0.01);
        }
        start_playback(&mut state, "flw_1");
        let d = frame(&ctx, &mut state, 1.0);
        assert!(d <= std::time::Duration::from_millis(FRAME_MS));
        let d = frame(&ctx, &mut state, 1.7);
        assert!(d <= std::time::Duration::from_millis(FRAME_MS));
        assert_eq!(active_step(&state, &state.flow_cards[0].clone()), Some(0));

        // Jeda: tidak ada repaint, posisi tidak maju.
        let pos = state.flow_play.as_ref().unwrap().position;
        let dur = play::play_duration(&items_of(&state, &state.flow_cards[0]));
        play::toggle_play(state.flow_play.as_mut().unwrap(), dur);
        let d = frame(&ctx, &mut state, 5.0);
        assert_eq!(d, std::time::Duration::MAX);
        assert_eq!(state.flow_play.as_ref().unwrap().position, pos);

        // Lanjut sampai habis: berhenti sendiri di Done tanpa repaint.
        play::toggle_play(state.flow_play.as_mut().unwrap(), dur);
        frame(&ctx, &mut state, 6.0);
        let d = frame(&ctx, &mut state, 60.0);
        let p = state.flow_play.as_ref().unwrap();
        assert!(!p.playing);
        assert_eq!(p.position, dur);
        assert_eq!(d, std::time::Duration::MAX);
        assert_eq!(active_step(&state, &state.flow_cards[0].clone()), None);
    }

    #[test]
    fn tick_drops_playback_of_removed_card() {
        let mut state = fixture();
        start_playback(&mut state, "flw_1");
        state.flow_cards.clear();
        assert!(!tick(&mut state, 1.0));
        assert!(state.flow_play.is_none());
        start_playback(&mut state, "missing");
        assert!(state.flow_play.is_none());
    }

    #[test]
    fn card_without_steps_plays_linked_tables() {
        let mut state = fixture();
        state.flow_cards[0].steps.clear();
        let items = items_of(&state, &state.flow_cards[0]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].table.as_deref(), Some("orders"));
        assert_eq!(active_step(&state, &state.flow_cards[0].clone()), None);
    }

    #[test]
    fn process_panel_renders_and_closing_clears_selection() {
        let ctx = egui::Context::default();
        let mut state = fixture();
        state.selected_flow = Some("flw_1".into());
        start_playback(&mut state, "flw_1");
        for t in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(t as f64),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let rect = ui.max_rect();
                let card =
                    egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(300.0, 80.0));
                render_process_panel(ui, &mut state, Some(card), rect, 0.0);
                // Card tidak tampil penuh: footer tidak digambar, pilihan tetap.
                render_process_panel(ui, &mut state, None, rect, 0.0);
            });
            out.textures_delta.clear();
        }
        assert_eq!(state.selected_flow.as_deref(), Some("flw_1"));
        clear(&mut state);
        assert!(state.selected_flow.is_none() && state.flow_play.is_none());
    }

    #[test]
    fn op_colors_follow_plan() {
        assert_eq!(op_color(Some(FlowOp::Read)), READ_COLOR);
        assert_eq!(op_color(Some(FlowOp::Insert)), WRITE_COLOR);
        assert_eq!(op_color(Some(FlowOp::Upsert)), WRITE_COLOR);
        assert_eq!(op_color(Some(FlowOp::Delete)), DELETE_COLOR);
        assert_eq!(op_color(None), UNKNOWN_COLOR);
    }
}
