//! Komponen UI kecil untuk REST client (`http_client.rs`): tab strip bergaya
//! underline dengan badge, pill status, editor kode dengan nomor baris dan
//! highlight pencarian, tabel key-value, dan empty state.
//!
//! Semua warna diambil dari `window_egui::style` supaya konsisten dengan tema.
//! Fungsi murni (format, parser bulk-edit, deteksi header sensitif) diletakkan
//! di sini juga supaya bisa dites tanpa egui.

use crate::window_egui::style;
use eframe::egui;
use std::ops::Range;

// ─── Tab strip ───────────────────────────────────────────────────────────────

/// Satu tab pada `render_tab_strip`.
pub struct TabItem<'a> {
    pub label: &'a str,
    /// Jumlah item (mis. header aktif). `None`/`Some(0)` = badge disembunyikan.
    pub badge: Option<usize>,
    /// Titik kecil penanda tab ini "terisi" (mis. body/auth aktif).
    pub dot: bool,
}

/// Tab level-2 bergaya underline: teks + badge jumlah + titik status, garis
/// aksen 2px pada tab aktif, dan garis pemisah tipis sepanjang baris.
/// `right` digambar rata kanan di baris yang sama (toolbar kecil).
/// Mengembalikan index tab yang diklik pada frame ini.
pub fn render_tab_strip(
    ui: &mut egui::Ui,
    id_salt: &str,
    items: &[TabItem<'_>],
    active: usize,
    right: impl FnOnce(&mut egui::Ui),
) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let height = 34.0;
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter().clone();
    painter.hline(
        row_rect.x_range(),
        row_rect.bottom() - 0.5,
        egui::Stroke::new(1.0, style::nav_border(&ctx)),
    );

    let label_font = egui::FontId::proportional(13.0);
    let badge_font = egui::FontId::proportional(10.5);
    let pad = 10.0;
    let mut clicked = None;
    let mut x = row_rect.left();

    for (i, item) in items.iter().enumerate() {
        let is_active = i == active;
        let label_galley = painter.layout_no_wrap(
            item.label.to_string(),
            label_font.clone(),
            egui::Color32::WHITE,
        );
        let badge_galley = item.badge.filter(|n| *n > 0).map(|n| {
            painter.layout_no_wrap(n.to_string(), badge_font.clone(), egui::Color32::WHITE)
        });
        let badge_w = badge_galley
            .as_ref()
            .map(|g| 6.0 + (g.size().x + 10.0).max(16.0))
            .unwrap_or(0.0);
        let dot_w = if item.dot { 10.0 } else { 0.0 };
        let tab_w = pad * 2.0 + label_galley.size().x + badge_w + dot_w;

        let tab_rect =
            egui::Rect::from_min_size(egui::pos2(x, row_rect.top()), egui::vec2(tab_w, height));
        x += tab_w;

        let resp = ui
            .interact(
                tab_rect,
                egui::Id::new((id_salt, "tab", i)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            clicked = Some(i);
        }

        let color = if is_active || resp.hovered() {
            style::nav_text_strong(&ctx)
        } else {
            style::nav_text_muted(&ctx)
        };
        let cy = tab_rect.center().y - 1.0;
        let mut cx = tab_rect.left() + pad;
        let label_w = label_galley.size().x;
        painter.galley(
            egui::pos2(cx, cy - label_galley.size().y / 2.0),
            label_galley,
            color,
        );
        cx += label_w;

        if let Some(g) = badge_galley {
            cx += 6.0;
            let pill_w = (g.size().x + 10.0).max(16.0);
            let pill = egui::Rect::from_center_size(
                egui::pos2(cx + pill_w / 2.0, cy),
                egui::vec2(pill_w, 16.0),
            );
            let (bg, fg) = if is_active {
                (
                    style::theme_accent(&ctx).gamma_multiply(0.22),
                    style::nav_text_strong(&ctx),
                )
            } else {
                (style::nav_track(&ctx), style::nav_text_muted(&ctx))
            };
            painter.rect_filled(pill, 8.0, bg);
            painter.galley(pill.center() - g.size() / 2.0, g, fg);
            cx += pill_w;
        }

        if item.dot {
            painter.circle_filled(egui::pos2(cx + 6.0, cy), 2.5, style::theme_success(&ctx));
        }

        if is_active {
            let underline = egui::Rect::from_min_max(
                egui::pos2(tab_rect.left() + 6.0, row_rect.bottom() - 2.0),
                egui::pos2(tab_rect.right() - 6.0, row_rect.bottom()),
            );
            painter.rect_filled(underline, 1.0, style::theme_accent(&ctx));
        }
    }

    let right_rect = egui::Rect::from_min_max(
        egui::pos2((x + 8.0).min(row_rect.right()), row_rect.top()),
        egui::pos2(row_rect.right(), row_rect.bottom() - 1.0),
    );
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right(&mut right_ui);

    clicked
}

// ─── Pill, tombol ikon, empty state ─────────────────────────────────────────

/// Pill informasi berwarna tipis (status HTTP, waktu, ukuran).
pub fn pill(
    ui: &mut egui::Ui,
    icon: Option<&str>,
    text: &str,
    color: egui::Color32,
) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
                if let Some(icon) = icon {
                    ui.label(egui::RichText::new(icon).size(12.5).color(color));
                }
                ui.label(egui::RichText::new(text).size(11.5).strong().color(color));
            });
        })
        .response
}

/// Tombol ikon kecil dengan status aktif (toggle), ukuran sama dengan
/// `style::ai_icon_button` supaya bisa dicampur dalam satu toolbar.
pub fn icon_toggle(ui: &mut egui::Ui, icon: &str, tooltip: &str, active: bool) -> egui::Response {
    let ctx = ui.ctx().clone();
    let color = if active {
        style::nav_text_strong(&ctx)
    } else {
        style::theme_muted_text(&ctx)
    };
    let mut btn = egui::Button::new(egui::RichText::new(icon).size(15.0).color(color))
        .frame_when_inactive(active)
        .corner_radius(5.0)
        .min_size(egui::vec2(26.0, 24.0));
    if active {
        btn = btn
            .fill(style::nav_raised(&ctx))
            .stroke(egui::Stroke::new(1.0, style::nav_border(&ctx)));
    }
    ui.add(btn).on_hover_text(tooltip)
}

/// Empty state terpusat: ikon besar redup, judul, dan keterangan.
pub fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, subtitle: &str) {
    let ctx = ui.ctx().clone();
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(icon)
                .size(36.0)
                .color(style::nav_text_muted(&ctx).gamma_multiply(0.6)),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .strong()
                .color(style::nav_text_strong(&ctx)),
        );
        if !subtitle.is_empty() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(subtitle)
                    .size(12.0)
                    .color(style::nav_text_muted(&ctx)),
            );
        }
    });
}

// ─── Editor kode (nomor baris + highlight + pencarian) ──────────────────────

/// Bahasa untuk syntax highlighting editor body/response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Syntax {
    Json,
    Xml,
    GraphQl,
    Plain,
}

/// Highlighter yang di-cache per frame: teks yang sama tidak di-highlight
/// ulang setiap frame (penting untuk response besar).
#[derive(Default)]
struct Highlighter;

type HighlightKey<'a> = (&'a str, Syntax, bool, u32, &'a str);

impl<'a> egui::cache::ComputerMut<HighlightKey<'a>, egui::text::LayoutJob> for Highlighter {
    fn compute(
        &mut self,
        (text, syntax, dark, size_bits, query): HighlightKey<'a>,
    ) -> egui::text::LayoutJob {
        let font_id = egui::FontId::monospace(f32::from_bits(size_bits));
        let job = match syntax {
            Syntax::Json => crate::http_client::highlight_body_json(text, dark, font_id),
            Syntax::Xml => crate::http_client::highlight_body_xml(text, dark, font_id),
            Syntax::GraphQl => crate::http_client::highlight_body_graphql(text, dark, font_id),
            Syntax::Plain => {
                let color = if dark {
                    egui::Color32::from_rgb(220, 220, 220)
                } else {
                    egui::Color32::from_rgb(30, 30, 30)
                };
                let mut j = egui::text::LayoutJob::default();
                j.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        font_id,
                        color,
                        ..Default::default()
                    },
                );
                j
            }
        };
        let bg = if dark {
            egui::Color32::from_rgba_unmultiplied(234, 179, 8, 90)
        } else {
            egui::Color32::from_rgba_unmultiplied(250, 204, 21, 150)
        };
        highlight_matches(job, &match_ranges(text, query), bg)
    }
}

type HighlightCache = egui::cache::FrameCache<egui::text::LayoutJob, Highlighter>;

/// Opsi `render_code_view`.
pub struct CodeView<'a> {
    pub id_salt: &'a str,
    pub syntax: Syntax,
    pub wrap: bool,
    pub hint: &'a str,
    /// Teks yang di-highlight (case-insensitive). Kosong = tanpa highlight.
    pub search: &'a str,
}

/// Editor kode dengan gutter nomor baris, syntax highlighting, dan highlight
/// hasil pencarian. `text` boleh `&mut String` (editable) atau `&mut &str`
/// (read-only tapi tetap bisa diseleksi/di-copy). Mengisi seluruh ruang sisa.
pub fn render_code_view(
    ui: &mut egui::Ui,
    text: &mut dyn egui::TextBuffer,
    opts: CodeView<'_>,
) -> egui::Response {
    let ctx = ui.ctx().clone();
    let dark = ui.visuals().dark_mode;
    let font_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Monospace)
        .map(|f| f.size)
        .unwrap_or(12.5);
    let mono = egui::FontId::monospace(font_size);
    let num_font = egui::FontId::monospace((font_size - 1.0).max(9.0));
    let muted = style::nav_text_muted(&ctx);

    let line_count = count_byte(text.as_str(), b'\n') + 1;
    let digits = line_count.to_string().len().max(2);
    let digits_w = ui
        .painter()
        .layout_no_wrap("0".repeat(digits), num_font.clone(), muted)
        .size()
        .x;
    let gutter_w = (digits_w + 18.0).min(110.0);

    let CodeView {
        id_salt,
        syntax,
        wrap,
        hint,
        search,
    } = opts;

    egui::Frame::new()
        .fill(style::ai_code_bg(&ctx))
        .stroke(egui::Stroke::new(1.0, style::nav_border(&ctx)))
        .corner_radius(6.0)
        .show(ui, |ui| {
            let viewport_h = ui.available_height().max(80.0);
            let scroll = if wrap {
                egui::ScrollArea::vertical()
            } else {
                egui::ScrollArea::both()
            };
            scroll
                .id_salt(id_salt)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let mut layouter =
                        |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                            let key = (buf.as_str(), syntax, dark, font_size.to_bits(), search);
                            let mut job = ui.ctx().memory_mut(|m| {
                                m.caches.cache::<HighlightCache>().get(key).clone()
                            });
                            job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
                            ui.fonts_mut(|f| f.layout_job(job))
                        };

                    let out = egui::TextEdit::multiline(text)
                        .id_salt(id_salt)
                        .font(mono.clone())
                        // Di egui 0.36 `.margin()` diabaikan bila `.frame()` diisi,
                        // jadi ruang gutter dipasang sebagai inner margin frame.
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                            left: (gutter_w + 8.0) as i8,
                            right: 8,
                            top: 6,
                            bottom: 6,
                        }))
                        .hint_text(egui::RichText::new(hint).color(muted))
                        .desired_width(f32::INFINITY)
                        .min_size(egui::vec2(0.0, viewport_h))
                        .lock_focus(true)
                        .layouter(&mut layouter)
                        .show(ui);

                    paint_gutter(ui, &out, gutter_w, &num_font, muted);
                    out.response.response
                })
                .inner
        })
        .inner
}

/// Menggambar gutter nomor baris di margin kiri TextEdit. Hanya baris logis
/// (setelah `\n`) yang diberi nomor, baris hasil wrap dibiarkan kosong.
fn paint_gutter(
    ui: &egui::Ui,
    out: &egui::text_edit::TextEditOutput,
    gutter_w: f32,
    font: &egui::FontId,
    color: egui::Color32,
) {
    let ctx = ui.ctx();
    let rect = out.response.response.rect;
    let gutter =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + gutter_w, rect.bottom()));
    let painter = ui.painter();
    painter.rect_filled(
        gutter,
        0.0,
        style::ai_code_header_bg(ctx).gamma_multiply(0.6),
    );
    painter.vline(
        gutter.right(),
        gutter.y_range(),
        egui::Stroke::new(1.0, style::nav_border(ctx)),
    );

    let clip = ui.clip_rect();
    let mut line = 1usize;
    let mut at_line_start = true;
    for row in &out.galley.rows {
        if at_line_start {
            let y = out.galley_pos.y + row.pos.y;
            let h = row.row.size.y;
            if y + h >= clip.top() && y <= clip.bottom() {
                painter.text(
                    egui::pos2(gutter.right() - 8.0, y + h / 2.0),
                    egui::Align2::RIGHT_CENTER,
                    line.to_string(),
                    font.clone(),
                    color,
                );
            }
            line += 1;
        }
        at_line_start = row.ends_with_newline;
    }
}

fn count_byte(s: &str, needle: u8) -> usize {
    s.as_bytes().iter().filter(|b| **b == needle).count()
}

/// Posisi byte semua kemunculan `query` di `text` (case-insensitive ASCII,
/// tidak tumpang tindih). Dibatasi 10.000 hasil supaya teks besar tetap ringan.
pub fn match_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    // to_ascii_lowercase tidak mengubah panjang byte, jadi offset tetap valid.
    let hay = text.to_ascii_lowercase();
    let needle = query.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(pos) = hay[start..].find(&needle) {
        let begin = start + pos;
        let end = begin + needle.len();
        out.push(begin..end);
        if out.len() >= 10_000 {
            break;
        }
        start = end;
    }
    out
}

/// Memecah section `job` supaya rentang `matches` (terurut, tidak tumpang
/// tindih) mendapat latar `bg`.
pub fn highlight_matches(
    mut job: egui::text::LayoutJob,
    matches: &[Range<usize>],
    bg: egui::Color32,
) -> egui::text::LayoutJob {
    if matches.is_empty() {
        return job;
    }
    let sections = std::mem::take(&mut job.sections);
    let mut out = Vec::with_capacity(sections.len() + matches.len() * 2);
    let mut m = 0;
    for section in sections {
        let range: Range<usize> = section.byte_range.start.into()..section.byte_range.end.into();
        let mut cursor = range.start;
        let mut leading = section.leading_space;
        while m < matches.len() && matches[m].end <= cursor {
            m += 1;
        }
        let mut k = m;
        while cursor < range.end {
            let (piece_end, is_match) = match matches.get(k) {
                Some(mr) if mr.start <= cursor => (mr.end.min(range.end), true),
                Some(mr) if mr.start < range.end => (mr.start, false),
                _ => (range.end, false),
            };
            let mut format = section.format.clone();
            if is_match {
                format.background = bg;
            }
            out.push(egui::text::LayoutSection {
                leading_space: leading,
                byte_range: egui::text::ByteIndex(cursor)..egui::text::ByteIndex(piece_end),
                format,
            });
            leading = 0.0;
            if is_match && piece_end >= matches[k].end {
                k += 1;
            }
            cursor = piece_end;
        }
    }
    job.sections = out;
    job
}

// ─── Tabel key-value (params / headers / form) ──────────────────────────────

/// Header request yang umum dipakai, untuk autocomplete kolom key.
pub const COMMON_HEADERS: &[&str] = &[
    "Accept",
    "Accept-Encoding",
    "Accept-Language",
    "Authorization",
    "Cache-Control",
    "Connection",
    "Content-Type",
    "Content-Length",
    "Cookie",
    "If-Match",
    "If-None-Match",
    "If-Modified-Since",
    "Origin",
    "Referer",
    "User-Agent",
    "X-API-Key",
    "X-Request-ID",
    "X-Requested-With",
    "X-Forwarded-For",
];

/// Opsi `render_kv_table`.
pub struct KvOptions<'a> {
    /// Daftar saran autocomplete untuk kolom key (kosong = tanpa autocomplete).
    pub suggestions: &'a [&'a str],
    /// Samarkan value yang key-nya terlihat seperti secret.
    pub mask_sensitive: bool,
    pub key_hint: &'a str,
    pub value_hint: &'a str,
}

/// Tabel key-value bergaya spreadsheet: checkbox enable, kolom key monospace,
/// value (tersamar bila sensitif), tombol hapus saat hover, baris kosong di
/// bawah yang otomatis menjadi baris baru, dan mode bulk edit.
pub fn render_kv_table(
    ui: &mut egui::Ui,
    rows: &mut Vec<(String, String, bool)>,
    id_salt: &str,
    opts: &KvOptions<'_>,
) {
    let ctx = ui.ctx().clone();
    let is_touch = ui.spacing().interact_size.y >= 30.0;
    let row_h = if is_touch { 38.0 } else { 30.0 };
    let font_size = if is_touch { 14.0 } else { 12.5 };
    let mono = egui::FontId::monospace(font_size);
    let muted = style::nav_text_muted(&ctx);
    let strong = style::nav_text_strong(&ctx);
    let border = style::nav_border(&ctx);

    ensure_trailing_empty_row(rows);

    // ── Toolbar: jumlah aktif + toggle bulk edit ──
    let bulk_id = egui::Id::new((id_salt, "bulk"));
    let bulk_text_id = egui::Id::new((id_salt, "bulk_text"));
    let mut bulk = ctx.data(|d| d.get_temp::<bool>(bulk_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        let n = active_count(rows);
        ui.label(
            egui::RichText::new(match n {
                0 => "No active entries".to_string(),
                1 => "1 active".to_string(),
                n => format!("{n} active"),
            })
            .size(11.5)
            .color(muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_toggle(
                ui,
                egui_icons::icons::ICON_EDIT_NOTE.codepoint,
                "Bulk edit: one `key: value` per line, prefix `//` to disable",
                bulk,
            )
            .clicked()
            {
                bulk = !bulk;
                if bulk {
                    ctx.data_mut(|d| d.insert_temp(bulk_text_id, rows_to_bulk(rows)));
                }
                ctx.data_mut(|d| d.insert_temp(bulk_id, bulk));
            }
        });
    });
    ui.add_space(4.0);

    if bulk {
        let mut text: String = ctx
            .data(|d| d.get_temp::<String>(bulk_text_id))
            .unwrap_or_else(|| rows_to_bulk(rows));
        let before = text.clone();
        let editor_id = format!("{id_salt}_bulk_editor");
        ui.allocate_ui(egui::vec2(ui.available_width(), 260.0), |ui| {
            render_code_view(
                ui,
                &mut text,
                CodeView {
                    id_salt: &editor_id,
                    syntax: Syntax::Plain,
                    wrap: true,
                    hint: "Content-Type: application/json\n// X-Disabled: value",
                    search: "",
                },
            );
        });
        if text != before {
            *rows = bulk_to_rows(&text);
            ensure_trailing_empty_row(rows);
        }
        ctx.data_mut(|d| d.insert_temp(bulk_text_id, text));
        return;
    }

    // ── Tabel ──
    let check_w = 30.0;
    let action_w = 56.0;
    let total_w = ui.available_width();
    let key_w = ((total_w - check_w - action_w) * 0.4).max(80.0);
    let value_w = (total_w - check_w - action_w - key_w).max(80.0);

    let suggest_id = egui::Id::new((id_salt, "suggest"));
    let mut focused_key: Option<(usize, egui::Rect)> = None;
    let mut to_remove: Vec<usize> = Vec::new();
    let last = rows.len().saturating_sub(1);

    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            // Header
            let (head, _) = ui.allocate_exact_size(egui::vec2(total_w, 26.0), egui::Sense::hover());
            let painter = ui.painter().clone();
            painter.rect_filled(
                head,
                egui::CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: 0,
                    se: 0,
                },
                style::nav_track(&ctx),
            );
            let head_font = egui::FontId::proportional(11.0);
            painter.text(
                egui::pos2(head.left() + check_w + 8.0, head.center().y),
                egui::Align2::LEFT_CENTER,
                "KEY",
                head_font.clone(),
                muted,
            );
            painter.text(
                egui::pos2(head.left() + check_w + key_w + 8.0, head.center().y),
                egui::Align2::LEFT_CENTER,
                "VALUE",
                head_font,
                muted,
            );

            for (idx, (key, value, enabled)) in rows.iter_mut().enumerate() {
                let is_placeholder = idx == last && key.is_empty() && value.is_empty();
                let (row_rect, _) =
                    ui.allocate_exact_size(egui::vec2(total_w, row_h), egui::Sense::hover());
                let hovered = ui.rect_contains_pointer(row_rect);

                if hovered {
                    painter.rect_filled(row_rect, 0.0, hover_tint(&ctx));
                } else if idx % 2 == 1 {
                    painter.rect_filled(row_rect, 0.0, stripe_tint(&ctx));
                }
                painter.hline(
                    row_rect.x_range(),
                    row_rect.top(),
                    egui::Stroke::new(1.0, border),
                );

                let x0 = row_rect.left();
                let check_rect = egui::Rect::from_min_size(
                    egui::pos2(x0, row_rect.top()),
                    egui::vec2(check_w, row_h),
                );
                let key_rect = egui::Rect::from_min_size(
                    egui::pos2(x0 + check_w, row_rect.top()),
                    egui::vec2(key_w, row_h),
                );
                let value_rect = egui::Rect::from_min_size(
                    egui::pos2(x0 + check_w + key_w, row_rect.top()),
                    egui::vec2(value_w, row_h),
                );
                let action_rect = egui::Rect::from_min_max(
                    egui::pos2(value_rect.right(), row_rect.top()),
                    row_rect.max,
                );
                for x in [key_rect.left(), value_rect.left(), action_rect.left()] {
                    painter.vline(x, row_rect.y_range(), egui::Stroke::new(1.0, border));
                }

                if !is_placeholder {
                    ui.put(
                        egui::Rect::from_center_size(check_rect.center(), egui::vec2(18.0, 18.0)),
                        egui::Checkbox::without_text(enabled),
                    )
                    .on_hover_text(if *enabled { "Disable" } else { "Enable" });
                }

                let text_color = if *enabled { strong } else { muted };
                let hint_color = muted.gamma_multiply(0.7);
                let key_resp = ui.put(
                    key_rect.shrink2(egui::vec2(8.0, 3.0)),
                    egui::TextEdit::singleline(key)
                        .id_salt((id_salt, idx, "k"))
                        .frame(egui::Frame::NONE)
                        .font(mono.clone())
                        .text_color(text_color)
                        .vertical_align(egui::Align::Center)
                        .desired_width(f32::INFINITY)
                        .hint_text(egui::RichText::new(opts.key_hint).color(hint_color)),
                );
                if key_resp.has_focus() {
                    focused_key = Some((idx, key_rect));
                    focus_ring(&painter, key_rect, &ctx);
                }

                let sensitive = opts.mask_sensitive && is_sensitive_key(key);
                let reveal_id = egui::Id::new((id_salt, idx, "reveal"));
                let revealed = ctx.data(|d| d.get_temp::<bool>(reveal_id)).unwrap_or(false);
                let value_resp = ui.put(
                    value_rect.shrink2(egui::vec2(8.0, 3.0)),
                    egui::TextEdit::singleline(value)
                        .id_salt((id_salt, idx, "v"))
                        .frame(egui::Frame::NONE)
                        .font(mono.clone())
                        .text_color(text_color)
                        .password(sensitive && !revealed)
                        .vertical_align(egui::Align::Center)
                        .desired_width(f32::INFINITY)
                        .hint_text(egui::RichText::new(opts.value_hint).color(hint_color)),
                );
                if value_resp.has_focus() {
                    focus_ring(&painter, value_rect, &ctx);
                }

                // Aksi: toggle tampilkan secret + hapus baris (muncul saat hover).
                let btn_size = egui::vec2(22.0, 22.0);
                if sensitive {
                    let eye = if revealed {
                        egui_icons::icons::ICON_VISIBILITY_OFF
                    } else {
                        egui_icons::icons::ICON_VISIBILITY
                    };
                    let r = egui::Rect::from_center_size(
                        egui::pos2(action_rect.left() + 16.0, action_rect.center().y),
                        btn_size,
                    );
                    if ui
                        .put(
                            r,
                            egui::Button::new(eye.rich_text().size(14.0).color(muted))
                                .frame_when_inactive(false),
                        )
                        .on_hover_text(if revealed { "Hide value" } else { "Show value" })
                        .clicked()
                    {
                        ctx.data_mut(|d| d.insert_temp(reveal_id, !revealed));
                    }
                }
                if hovered && !is_placeholder {
                    let r = egui::Rect::from_center_size(
                        egui::pos2(action_rect.right() - 16.0, action_rect.center().y),
                        btn_size,
                    );
                    if ui
                        .put(
                            r,
                            egui::Button::new(
                                egui_icons::icons::ICON_CLOSE
                                    .rich_text()
                                    .size(14.0)
                                    .color(style::theme_danger(&ctx)),
                            )
                            .frame_when_inactive(false),
                        )
                        .on_hover_text("Remove row")
                        .clicked()
                    {
                        to_remove.push(idx);
                    }
                }
            }
        });

    for idx in to_remove.iter().rev() {
        rows.remove(*idx);
    }

    if !opts.suggestions.is_empty() {
        render_key_suggestions(ui, rows, suggest_id, focused_key, opts.suggestions, &mono);
    }

    ensure_trailing_empty_row(rows);
}

/// Popup autocomplete untuk kolom key yang sedang difokus.
fn render_key_suggestions(
    ui: &mut egui::Ui,
    rows: &mut [(String, String, bool)],
    suggest_id: egui::Id,
    focused_key: Option<(usize, egui::Rect)>,
    suggestions: &[&str],
    font: &egui::FontId,
) {
    let ctx = ui.ctx().clone();
    let rect_id = suggest_id.with("area_rect");
    let prev_rect: Option<egui::Rect> = ctx.data(|d| d.get_temp(rect_id));
    let prev_open: Option<(usize, egui::Rect)> = ctx.data(|d| d.get_temp(suggest_id));

    // Popup tetap terbuka saat pointer di atasnya, supaya klik pada saran
    // tidak hilang ketika field key kehilangan fokus di frame yang sama.
    let pointer_on_popup = prev_rect.is_some_and(|r| {
        ctx.input(|i| i.pointer.latest_pos())
            .is_some_and(|p| r.contains(p))
    });
    let open = focused_key.or(if pointer_on_popup { prev_open } else { None });

    let clear = |ctx: &egui::Context| {
        ctx.data_mut(|d| {
            d.remove::<(usize, egui::Rect)>(suggest_id);
            d.remove::<egui::Rect>(rect_id);
        });
    };

    let Some((row_idx, anchor)) = open.filter(|(i, _)| *i < rows.len()) else {
        clear(&ctx);
        return;
    };
    let matches = key_suggestions(&rows[row_idx].0, suggestions);
    if matches.is_empty() {
        clear(&ctx);
        return;
    }

    let mut chosen = None;
    let area = egui::Area::new(suggest_id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 2.0))
        .show(&ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(anchor.width().max(180.0));
                for s in &matches {
                    if ui
                        .selectable_label(false, egui::RichText::new(*s).font(font.clone()))
                        .clicked()
                    {
                        chosen = Some(*s);
                    }
                }
            });
        });

    if let Some(s) = chosen {
        rows[row_idx].0 = s.to_string();
        clear(&ctx);
    } else {
        ctx.data_mut(|d| {
            d.insert_temp(suggest_id, (row_idx, anchor));
            d.insert_temp(rect_id, area.response.rect);
        });
    }
}

fn focus_ring(painter: &egui::Painter, rect: egui::Rect, ctx: &egui::Context) {
    painter.rect_stroke(
        rect.shrink(1.5),
        4.0,
        egui::Stroke::new(
            1.0,
            ctx.global_style().visuals.widgets.active.bg_stroke.color,
        ),
        egui::StrokeKind::Inside,
    );
}

fn hover_tint(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 8)
    }
}

fn stripe_tint(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 4)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 5)
    }
}

// ─── Fungsi murni ────────────────────────────────────────────────────────────

/// Jumlah baris yang benar-benar dikirim (aktif dan key tidak kosong).
pub fn active_count(rows: &[(String, String, bool)]) -> usize {
    rows.iter()
        .filter(|(k, _, en)| *en && !k.trim().is_empty())
        .count()
}

/// Pastikan selalu ada tepat satu baris kosong di paling bawah sebagai
/// tempat mengetik entri baru.
pub fn ensure_trailing_empty_row(rows: &mut Vec<(String, String, bool)>) {
    let is_empty = |r: &(String, String, bool)| r.0.is_empty() && r.1.is_empty();
    while rows.len() >= 2 && is_empty(&rows[rows.len() - 1]) && is_empty(&rows[rows.len() - 2]) {
        rows.pop();
    }
    if rows.last().is_none_or(|r| !is_empty(r)) {
        rows.push((String::new(), String::new(), true));
    }
}

/// Ubah baris ke format bulk edit: `key: value` per baris, baris nonaktif
/// diawali `// `. Baris kosong diabaikan.
pub fn rows_to_bulk(rows: &[(String, String, bool)]) -> String {
    rows.iter()
        .filter(|(k, v, _)| !k.is_empty() || !v.is_empty())
        .map(|(k, v, en)| {
            if *en {
                format!("{k}: {v}")
            } else {
                format!("// {k}: {v}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Kebalikan `rows_to_bulk`. Dipisah pada `:` pertama; baris tanpa `:`
/// menjadi key dengan value kosong.
pub fn bulk_to_rows(text: &str) -> Vec<(String, String, bool)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (enabled, body) = match line.strip_prefix("//") {
                Some(rest) => (false, rest.trim_start()),
                None => (true, line),
            };
            let (k, v) = match body.split_once(':') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => (body.trim(), ""),
            };
            if k.is_empty() && v.is_empty() {
                return None;
            }
            Some((k.to_string(), v.to_string(), enabled))
        })
        .collect()
}

/// Apakah key header/param terlihat membawa secret (token, password, cookie...).
pub fn is_sensitive_key(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
    ];
    const PARTS: &[&str] = &[
        "token", "secret", "password", "passwd", "api-key", "apikey", "api_key", "session",
    ];
    let k = key.trim().to_ascii_lowercase();
    EXACT.contains(&k.as_str()) || PARTS.iter().any(|p| k.contains(p))
}

/// Saran autocomplete untuk `typed`: yang diawali prefix lebih dulu, lalu yang
/// mengandung teks; kosong bila sudah sama persis. Maksimal 8.
pub fn key_suggestions<'a>(typed: &str, candidates: &[&'a str]) -> Vec<&'a str> {
    let t = typed.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Vec::new();
    }
    let lower: Vec<String> = candidates.iter().map(|c| c.to_ascii_lowercase()).collect();
    if lower.contains(&t) {
        return Vec::new();
    }
    let mut out: Vec<&str> = candidates
        .iter()
        .zip(&lower)
        .filter(|(_, l)| l.starts_with(&t))
        .map(|(c, _)| *c)
        .collect();
    out.extend(
        candidates
            .iter()
            .zip(&lower)
            .filter(|(_, l)| !l.starts_with(&t) && l.contains(&t))
            .map(|(c, _)| *c),
    );
    out.truncate(8);
    out
}

/// Ukuran byte yang mudah dibaca: `512 B`, `1.5 KB`, `2.3 MB`.
pub fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Durasi yang mudah dibaca: `842 ms`, `1.24 s`, `1m 05s`.
pub fn format_duration(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.2} s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Kelas status HTTP untuk pewarnaan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusClass {
    Info,
    Success,
    Redirect,
    ClientError,
    ServerError,
}

pub fn status_class(status: u16) -> StatusClass {
    match status {
        200..=299 => StatusClass::Success,
        300..=399 => StatusClass::Redirect,
        400..=499 => StatusClass::ClientError,
        500.. => StatusClass::ServerError,
        _ => StatusClass::Info,
    }
}

pub fn status_color(ctx: &egui::Context, status: u16) -> egui::Color32 {
    match status_class(status) {
        StatusClass::Success => style::theme_success(ctx),
        StatusClass::Redirect | StatusClass::Info => style::theme_info(ctx),
        StatusClass::ClientError => style::theme_warning(ctx),
        StatusClass::ServerError => style::theme_danger(ctx),
    }
}

/// Petunjuk singkat untuk error jaringan umum dari reqwest.
pub fn error_hint(err: &str) -> Option<&'static str> {
    let e = err.to_ascii_lowercase();
    if e.contains("relative url") || e.contains("builder error") || e.contains("invalid url") {
        Some("The URL looks invalid. Include the scheme, e.g. https://api.example.com/path")
    } else if e.contains("dns")
        || e.contains("lookup address")
        || e.contains("name or service not known")
    {
        Some("The host name could not be resolved. Check the URL and your network or DNS.")
    } else if e.contains("timed out") || e.contains("timeout") {
        Some("The server did not respond in time. Check that it is reachable.")
    } else if e.contains("certificate") || e.contains("tls") || e.contains("ssl") {
        Some("TLS handshake failed. The server certificate may be invalid or self-signed.")
    } else if e.contains("connection refused") {
        Some("Nothing is listening at that address and port.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(k: &str, v: &str, en: bool) -> (String, String, bool) {
        (k.to_string(), v.to_string(), en)
    }

    #[test]
    fn trailing_row_added_and_deduplicated() {
        let mut rows = vec![row("a", "1", true)];
        ensure_trailing_empty_row(&mut rows);
        assert_eq!(rows.len(), 2);
        assert!(rows[1].0.is_empty() && rows[1].2);

        let mut rows = vec![row("a", "1", true), row("", "", true), row("", "", true)];
        ensure_trailing_empty_row(&mut rows);
        assert_eq!(rows.len(), 2);

        let mut rows = Vec::new();
        ensure_trailing_empty_row(&mut rows);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn bulk_roundtrip_keeps_disabled_rows() {
        let rows = vec![
            row("Content-Type", "application/json", true),
            row("X-Debug", "1", false),
            row("", "", true),
        ];
        let text = rows_to_bulk(&rows);
        assert_eq!(text, "Content-Type: application/json\n// X-Debug: 1");
        assert_eq!(bulk_to_rows(&text), rows[..2].to_vec());
    }

    #[test]
    fn bulk_parses_values_with_colons_and_blank_lines() {
        let rows = bulk_to_rows("\nurl: http://x:8080/a\n\nflag\n//   off:v\n");
        assert_eq!(
            rows,
            vec![
                row("url", "http://x:8080/a", true),
                row("flag", "", true),
                row("off", "v", false),
            ]
        );
    }

    #[test]
    fn active_count_ignores_disabled_and_empty() {
        let rows = vec![
            row("a", "1", true),
            row("b", "2", false),
            row(" ", "x", true),
        ];
        assert_eq!(active_count(&rows), 1);
    }

    #[test]
    fn sensitive_keys_detected() {
        for k in [
            "Authorization",
            "cookie",
            "X-API-Key",
            "x-auth-token",
            "client_secret",
            "apikey",
        ] {
            assert!(is_sensitive_key(k), "{k}");
        }
        for k in ["Accept", "Content-Type", "page", "X-Request-ID"] {
            assert!(!is_sensitive_key(k), "{k}");
        }
    }

    #[test]
    fn suggestions_prefix_first_and_hide_exact() {
        let s = key_suggestions("con", COMMON_HEADERS);
        assert_eq!(s[0], "Connection");
        assert!(s.contains(&"Content-Type"));
        assert!(key_suggestions("accept", COMMON_HEADERS).is_empty());
        assert!(key_suggestions("", COMMON_HEADERS).is_empty());
        // "type" hanya cocok lewat contains.
        assert_eq!(
            key_suggestions("type", COMMON_HEADERS),
            vec!["Content-Type"]
        );
    }

    #[test]
    fn formats_are_readable() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(format_duration(842), "842 ms");
        assert_eq!(format_duration(1240), "1.24 s");
        assert_eq!(format_duration(65_000), "1m 05s");
    }

    #[test]
    fn status_classes() {
        assert_eq!(status_class(204), StatusClass::Success);
        assert_eq!(status_class(301), StatusClass::Redirect);
        assert_eq!(status_class(404), StatusClass::ClientError);
        assert_eq!(status_class(503), StatusClass::ServerError);
        assert_eq!(status_class(101), StatusClass::Info);
    }

    #[test]
    fn error_hints() {
        assert!(error_hint("error sending request: dns error: failed to lookup address").is_some());
        assert!(error_hint("builder error: relative URL without a base").is_some());
        assert!(error_hint("operation timed out").is_some());
        assert!(error_hint("something odd").is_none());
    }

    #[test]
    fn match_ranges_case_insensitive_non_overlapping() {
        assert_eq!(match_ranges("aAaA", "aa"), vec![0..2, 2..4]);
        assert_eq!(match_ranges("Hello hello", "HELLO"), vec![0..5, 6..11]);
        assert!(match_ranges("abc", "").is_empty());
        assert!(match_ranges("abc", "z").is_empty());
    }

    #[test]
    fn highlight_splits_sections_at_match_bounds() {
        let mut job = egui::text::LayoutJob::default();
        // Warna berbeda supaya `append` tidak menggabungkan kedua section.
        let fmt = |color| egui::TextFormat {
            color,
            ..Default::default()
        };
        job.append("abc", 0.0, fmt(egui::Color32::WHITE));
        job.append("def", 0.0, fmt(egui::Color32::BLACK));
        let bg = egui::Color32::RED;
        // Match melintasi batas dua section: "cd".
        let out = highlight_matches(job, &match_ranges("abcdef", "cd"), bg);
        let pieces: Vec<(Range<usize>, bool)> = out
            .sections
            .iter()
            .map(|s| {
                (
                    s.byte_range.start.into()..s.byte_range.end.into(),
                    s.format.background == bg,
                )
            })
            .collect();
        assert_eq!(
            pieces,
            vec![(0..2, false), (2..3, true), (3..4, true), (4..6, false)]
        );
    }
}
