//! Penampil diff side-by-side / unified untuk tab Git dan Merge Review.
//!
//! Baris digambar langsung dengan painter di dalam `ScrollArea::show_rows`
//! sehingga hanya baris yang terlihat yang diproses; diff ribuan baris tetap
//! ringan. Teks memakai `FontId::monospace` langsung (painter tidak terkena
//! `override_font_id` tema).

use eframe::egui;

use crate::git::diff::{DiffRow, RowKind};

const FONT_SIZE: f32 = 12.5;
const GUTTER_CHARS: usize = 5;

struct Palette {
    add_bg: egui::Color32,
    del_bg: egui::Color32,
    empty_bg: egui::Color32,
    hunk_bg: egui::Color32,
    hunk_fg: egui::Color32,
    gutter_fg: egui::Color32,
    text: egui::Color32,
    add_mark: egui::Color32,
    del_mark: egui::Color32,
    divider: egui::Color32,
}

fn palette(ctx: &egui::Context) -> Palette {
    let dark = ctx.global_style().visuals.dark_mode;
    if dark {
        Palette {
            add_bg: egui::Color32::from_rgba_unmultiplied(46, 160, 67, 46),
            del_bg: egui::Color32::from_rgba_unmultiplied(248, 81, 73, 46),
            empty_bg: egui::Color32::from_rgba_unmultiplied(128, 128, 128, 14),
            hunk_bg: egui::Color32::from_rgba_unmultiplied(56, 139, 253, 30),
            hunk_fg: egui::Color32::from_rgb(121, 170, 238),
            gutter_fg: egui::Color32::from_gray(110),
            text: egui::Color32::from_gray(215),
            add_mark: egui::Color32::from_rgb(63, 185, 80),
            del_mark: egui::Color32::from_rgb(248, 81, 73),
            divider: egui::Color32::from_gray(55),
        }
    } else {
        Palette {
            add_bg: egui::Color32::from_rgb(218, 251, 225),
            del_bg: egui::Color32::from_rgb(255, 235, 233),
            empty_bg: egui::Color32::from_gray(246),
            hunk_bg: egui::Color32::from_rgb(221, 244, 255),
            hunk_fg: egui::Color32::from_rgb(9, 105, 218),
            gutter_fg: egui::Color32::from_gray(140),
            text: egui::Color32::from_gray(36),
            add_mark: egui::Color32::from_rgb(26, 127, 55),
            del_mark: egui::Color32::from_rgb(207, 34, 46),
            divider: egui::Color32::from_gray(215),
        }
    }
}

fn expand_tabs(s: &str) -> String {
    if s.contains('\t') {
        s.replace('\t', "    ")
    } else {
        s.to_string()
    }
}

/// Satu sisi (kiri/kanan) baris side-by-side, atau baris unified.
struct Cell<'a> {
    num: Option<u32>,
    text: Option<&'a str>,
    mark: &'static str,
    bg: egui::Color32,
    mark_color: egui::Color32,
}

fn paint_cell(
    painter: &egui::Painter,
    rect: egui::Rect,
    cell: &Cell<'_>,
    pal: &Palette,
    char_w: f32,
) {
    painter.rect_filled(rect, 0.0, cell.bg);
    let font = egui::FontId::monospace(FONT_SIZE);
    let gutter_w = char_w * GUTTER_CHARS as f32 + 8.0;
    let y = rect.center().y;
    if let Some(n) = cell.num {
        painter.text(
            egui::pos2(rect.left() + gutter_w - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            font.clone(),
            pal.gutter_fg,
        );
    }
    if let Some(text) = cell.text {
        let x = rect.left() + gutter_w + 4.0;
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            cell.mark,
            font.clone(),
            cell.mark_color,
        );
        painter.with_clip_rect(rect).text(
            egui::pos2(x + char_w * 1.5, y),
            egui::Align2::LEFT_CENTER,
            expand_tabs(text),
            font,
            pal.text,
        );
    }
}

/// Baris tampilan unified.
#[derive(Clone, Copy)]
enum Line<'a> {
    Hunk(&'a str),
    Context(Option<u32>, Option<u32>, &'a str),
    Removed(Option<u32>, &'a str),
    Added(Option<u32>, &'a str),
}

fn unified_lines(rows: &[DiffRow]) -> Vec<Line<'_>> {
    let mut out = Vec::with_capacity(rows.len() + rows.len() / 4);
    let mut i = 0;
    while i < rows.len() {
        let r = &rows[i];
        match r.kind {
            RowKind::Hunk => {
                out.push(Line::Hunk(&r.hunk_header));
                i += 1;
            }
            RowKind::Context => {
                out.push(Line::Context(
                    r.left_num,
                    r.right_num,
                    r.left.as_deref().unwrap_or(""),
                ));
                i += 1;
            }
            RowKind::Change => {
                // Satu blok perubahan: semua baris hapus dulu, lalu semua tambah.
                let start = i;
                while i < rows.len() && rows[i].kind == RowKind::Change {
                    i += 1;
                }
                let block = &rows[start..i];
                out.extend(
                    block
                        .iter()
                        .filter_map(|b| b.left.as_deref().map(|t| Line::Removed(b.left_num, t))),
                );
                out.extend(
                    block
                        .iter()
                        .filter_map(|b| b.right.as_deref().map(|t| Line::Added(b.right_num, t))),
                );
            }
        }
    }
    out
}

/// Render diff. `limit` membatasi jumlah baris yang ditampilkan (sisanya
/// diringkas dengan tombol "Show all"). Mengembalikan `true` bila user
/// meminta semua baris ditampilkan.
pub fn render_diff(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    rows: &[DiffRow],
    side_by_side: bool,
    limit: Option<usize>,
) -> bool {
    let pal = palette(ui.ctx());
    let font = egui::FontId::monospace(FONT_SIZE);
    let (char_w, row_h) = ui.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font) + 4.0));
    let gutter_w = char_w * GUTTER_CHARS as f32 + 8.0;
    let lines = if side_by_side {
        Vec::new()
    } else {
        unified_lines(rows)
    };
    let total = if side_by_side {
        rows.len()
    } else {
        lines.len()
    };
    let shown = limit.map_or(total, |l| l.min(total));
    let hidden = total - shown;
    let mut show_all = false;

    // Lebar konten unified mengikuti baris terpanjang supaya bisa digeser.
    let longest = lines[..shown.min(lines.len())]
        .iter()
        .map(|l| match l {
            Line::Hunk(t) | Line::Context(_, _, t) | Line::Removed(_, t) | Line::Added(_, t) => {
                t.len()
            }
        })
        .max()
        .unwrap_or(0);

    let scroll = if side_by_side {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    scroll
        .id_salt(id_salt)
        .auto_shrink([false, false])
        .show_rows(ui, row_h, shown + usize::from(hidden > 0), |ui, range| {
            let full_w = if side_by_side {
                ui.available_width()
            } else {
                ui.available_width()
                    .max(gutter_w * 2.0 + char_w * (longest as f32 + 6.0))
            };
            for i in range {
                if i >= shown {
                    ui.horizontal(|ui| {
                        ui.label(format!("{hidden} more lines not shown."));
                        if ui.button("Show all").clicked() {
                            show_all = true;
                        }
                    });
                    continue;
                }
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(full_w, row_h), egui::Sense::hover());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter();
                if side_by_side {
                    paint_side_by_side(painter, rect, &rows[i], &pal, char_w);
                } else {
                    paint_unified(painter, rect, lines[i], &pal, char_w);
                }
            }
        });
    show_all
}

fn paint_hunk(painter: &egui::Painter, rect: egui::Rect, header: &str, pal: &Palette) {
    painter.rect_filled(rect, 0.0, pal.hunk_bg);
    painter.with_clip_rect(rect).text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        header,
        egui::FontId::monospace(FONT_SIZE),
        pal.hunk_fg,
    );
}

fn paint_side_by_side(
    painter: &egui::Painter,
    rect: egui::Rect,
    row: &DiffRow,
    pal: &Palette,
    char_w: f32,
) {
    if row.kind == RowKind::Hunk {
        paint_hunk(painter, rect, &row.hunk_header, pal);
        return;
    }
    let change = row.kind == RowKind::Change;
    let left = Cell {
        num: row.left_num,
        text: row.left.as_deref(),
        mark: if change && row.left.is_some() {
            "-"
        } else {
            " "
        },
        bg: match (change, row.left.is_some()) {
            (true, true) => pal.del_bg,
            (true, false) => pal.empty_bg,
            _ => egui::Color32::TRANSPARENT,
        },
        mark_color: pal.del_mark,
    };
    let right = Cell {
        num: row.right_num,
        text: row.right.as_deref(),
        mark: if change && row.right.is_some() {
            "+"
        } else {
            " "
        },
        bg: match (change, row.right.is_some()) {
            (true, true) => pal.add_bg,
            (true, false) => pal.empty_bg,
            _ => egui::Color32::TRANSPARENT,
        },
        mark_color: pal.add_mark,
    };
    let mid = rect.center().x;
    let l = egui::Rect::from_min_max(rect.min, egui::pos2(mid - 0.5, rect.max.y));
    let r = egui::Rect::from_min_max(egui::pos2(mid + 0.5, rect.min.y), rect.max);
    paint_cell(painter, l, &left, pal, char_w);
    paint_cell(painter, r, &right, pal, char_w);
    painter.vline(mid, rect.y_range(), egui::Stroke::new(1.0, pal.divider));
}

fn paint_unified(
    painter: &egui::Painter,
    rect: egui::Rect,
    line: Line<'_>,
    pal: &Palette,
    char_w: f32,
) {
    let (old, new, mark, text, bg, mark_color) = match line {
        Line::Hunk(h) => {
            paint_hunk(painter, rect, h, pal);
            return;
        }
        Line::Context(o, n, t) => (o, n, " ", t, egui::Color32::TRANSPARENT, pal.text),
        Line::Removed(o, t) => (o, None, "-", t, pal.del_bg, pal.del_mark),
        Line::Added(n, t) => (None, n, "+", t, pal.add_bg, pal.add_mark),
    };
    let font = egui::FontId::monospace(FONT_SIZE);
    let gutter_w = char_w * GUTTER_CHARS as f32 + 8.0;
    let y = rect.center().y;
    painter.rect_filled(rect, 0.0, bg);
    for (i, num) in [old, new].into_iter().enumerate() {
        if let Some(n) = num {
            painter.text(
                egui::pos2(rect.left() + gutter_w * (i as f32 + 1.0) - 4.0, y),
                egui::Align2::RIGHT_CENTER,
                n.to_string(),
                font.clone(),
                pal.gutter_fg,
            );
        }
    }
    let x = rect.left() + gutter_w * 2.0 + 4.0;
    painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        mark,
        font.clone(),
        mark_color,
    );
    painter.text(
        egui::pos2(x + char_w * 1.5, y),
        egui::Align2::LEFT_CENTER,
        expand_tabs(text),
        font,
        pal.text,
    );
}

/// Ringkasan "+a −d" berwarna.
pub fn stat_label(ui: &mut egui::Ui, additions: u64, deletions: u64) {
    let pal = palette(ui.ctx());
    ui.label(
        egui::RichText::new(format!("+{additions}"))
            .color(pal.add_mark)
            .size(11.5),
    );
    ui.label(
        egui::RichText::new(format!("−{deletions}"))
            .color(pal.del_mark)
            .size(11.5),
    );
}
