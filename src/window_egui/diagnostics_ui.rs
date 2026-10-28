//! Tampilan diagnostik live di Query Editor: garis bergelombang di bawah
//! teks yang salah, penanda di gutter, tooltip berisi pesan dan saran, serta
//! panel Problems di pojok kanan bawah editor.
//!
//! Penanda gutter digambar dengan painter (bukan glyph) supaya tidak pernah
//! tampil sebagai kotak kosong; ikon teks memakai `egui_icons`.

use eframe::egui;
use egui::text::CCursor;

use super::Tabular;
use super::diagnostics_jobs;
use crate::query_tools::{LintMessage, LintSeverity};

/// Batas penanda yang digambar per frame (teks besar dengan ribuan error).
const MAX_PAINTED: usize = 400;

fn color(ctx: &egui::Context, severity: &LintSeverity) -> egui::Color32 {
    match severity {
        LintSeverity::Error => super::style::theme_danger(ctx),
        LintSeverity::Warning => super::style::theme_warning(ctx),
        LintSeverity::Info => super::style::theme_info(ctx),
    }
}

pub(crate) fn icon(severity: &LintSeverity) -> &'static str {
    match severity {
        LintSeverity::Error => egui_icons::icons::ICON_ERROR.codepoint,
        LintSeverity::Warning => egui_icons::icons::ICON_WARNING.codepoint,
        LintSeverity::Info => egui_icons::icons::ICON_INFO.codepoint,
    }
}

fn rank(severity: &LintSeverity) -> u8 {
    match severity {
        LintSeverity::Error => 2,
        LintSeverity::Warning => 1,
        LintSeverity::Info => 0,
    }
}

/// Offset byte (urut naik) → index karakter, satu kali jalan.
fn char_indices_for(text: &str, offsets: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(offsets.len());
    let mut chars = 0usize;
    let mut byte = 0usize;
    let mut iter = text.char_indices().peekable();
    for &target in offsets {
        while byte < target {
            match iter.next() {
                Some((_, c)) => {
                    byte += c.len_utf8();
                    chars += 1;
                }
                None => break,
            }
        }
        out.push(chars);
    }
    out
}

fn char_to_byte(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
}

fn wavy(painter: &egui::Painter, x0: f32, x1: f32, y: f32, color: egui::Color32) {
    let x1 = x1.max(x0 + 4.0);
    let (step, amp) = (2.0, 1.4);
    let mut pts = Vec::with_capacity(((x1 - x0) / step) as usize + 2);
    let mut x = x0;
    let mut up = true;
    while x < x1 {
        pts.push(egui::pos2(x, if up { y - amp } else { y + amp }));
        x += step;
        up = !up;
    }
    pts.push(egui::pos2(x1, if up { y - amp } else { y + amp }));
    painter.add(egui::Shape::line(pts, egui::Stroke::new(1.1, color)));
}

/// Gambar penanda di atas galley editor. Dipanggil setelah TextEdit dirender.
pub(crate) fn paint(
    tabular: &Tabular,
    ui: &egui::Ui,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    clip: egui::Rect,
    response: &egui::Response,
    gutter: Option<egui::Rect>,
) {
    let items = &tabular.diagnostics.items;
    if items.is_empty() {
        return;
    }
    let text = tabular.editor.text.as_str();
    let ctx = ui.ctx().clone();
    let painter = ui.painter().with_clip_rect(clip);
    let with_span: Vec<&LintMessage> = items
        .iter()
        .filter(|m| m.span.as_ref().is_some_and(|r| r.end <= text.len()))
        .take(MAX_PAINTED)
        .collect();
    let mut offsets: Vec<usize> = with_span
        .iter()
        .flat_map(|m| {
            let r = m.span.clone().unwrap_or_default();
            [r.start, r.end]
        })
        .collect();
    let mut order: Vec<usize> = (0..offsets.len()).collect();
    order.sort_by_key(|&i| offsets[i]);
    let sorted: Vec<usize> = order.iter().map(|&i| offsets[i]).collect();
    let chars = char_indices_for(text, &sorted);
    for (k, &i) in order.iter().enumerate() {
        offsets[i] = chars[k];
    }
    // Baris galley → severity tertinggi (untuk gutter) + pesan.
    let mut gutter_rows: std::collections::BTreeMap<usize, (u8, Vec<&LintMessage>)> =
        Default::default();
    for (n, m) in with_span.iter().enumerate() {
        let (mut sc, mut ec) = (offsets[2 * n], offsets[2 * n + 1]);
        if ec <= sc {
            // Rentang kosong: tandai satu karakter (atau karakter sebelumnya di akhir teks).
            if sc > 0 && sc >= text.chars().count() {
                sc -= 1;
            }
            ec = sc + 1;
        }
        let start = galley.layout_from_cursor(CCursor::new(sc));
        let end = galley.layout_from_cursor(CCursor::new(ec));
        let col = color(&ctx, &m.severity);
        for row_idx in start.row..=end.row.min(galley.rows.len().saturating_sub(1)) {
            let Some(row) = galley.rows.get(row_idx) else {
                break;
            };
            let rect = row.rect().translate(galley_pos.to_vec2());
            if rect.bottom() < clip.top() || rect.top() > clip.bottom() {
                continue;
            }
            let x0 = if row_idx == start.row {
                galley_pos.x + galley.pos_from_cursor(CCursor::new(sc)).min.x
            } else {
                rect.left()
            };
            let x1 = if row_idx == end.row {
                galley_pos.x + galley.pos_from_cursor(CCursor::new(ec)).min.x
            } else {
                rect.right()
            };
            wavy(&painter, x0, x1, rect.bottom() - 1.0, col);
        }
        let entry = gutter_rows.entry(start.row).or_insert((0, Vec::new()));
        entry.0 = entry.0.max(rank(&m.severity) + 1);
        entry.1.push(m);
    }

    // Penanda gutter + tooltip baris.
    if let Some(gutter) = gutter {
        let gpainter = ui
            .painter()
            .with_clip_rect(gutter.intersect(clip.expand2(egui::vec2(gutter.width() + 4.0, 0.0))));
        for (row_idx, (level, msgs)) in &gutter_rows {
            let Some(row) = galley.rows.get(*row_idx) else {
                continue;
            };
            let rect = row.rect().translate(galley_pos.to_vec2());
            if rect.bottom() < clip.top() || rect.top() > clip.bottom() {
                continue;
            }
            let center = egui::pos2(gutter.left() + 7.0, rect.center().y);
            let sev = match level {
                3 => LintSeverity::Error,
                2 => LintSeverity::Warning,
                _ => LintSeverity::Info,
            };
            let col = color(&ctx, &sev);
            match sev {
                LintSeverity::Warning => {
                    let r = 4.0;
                    gpainter.add(egui::Shape::convex_polygon(
                        vec![
                            center + egui::vec2(0.0, -r),
                            center + egui::vec2(r, r * 0.8),
                            center + egui::vec2(-r, r * 0.8),
                        ],
                        col,
                        egui::Stroke::NONE,
                    ));
                }
                _ => {
                    gpainter.circle_filled(center, 3.5, col);
                }
            }
            let hit =
                egui::Rect::from_center_size(center, egui::vec2(14.0, rect.height().max(12.0)));
            let resp = ui.interact(
                hit,
                ui.id().with(("diag_gutter", *row_idx)),
                egui::Sense::hover(),
            );
            if resp.hovered() {
                let msgs = msgs.clone();
                resp.on_hover_ui(|ui| tooltip_body(ui, &msgs, tabular));
            }
        }
    }

    // Tooltip saat pointer di atas teks yang ditandai.
    if let Some(pointer) = response.hover_pos() {
        let cursor = galley.cursor_from_pos(pointer - galley_pos);
        let row = galley.layout_from_cursor(cursor).row;
        let on_row = galley.rows.get(row).is_some_and(|r| {
            r.rect()
                .translate(galley_pos.to_vec2())
                .expand(1.0)
                .contains(pointer)
        });
        if on_row {
            let byte = char_to_byte(text, cursor.index.0);
            let hits: Vec<&LintMessage> = with_span
                .iter()
                .copied()
                .filter(|m| {
                    m.span
                        .as_ref()
                        .is_some_and(|r| r.start <= byte && byte < r.end.max(r.start + 1))
                })
                .collect();
            if !hits.is_empty() {
                response
                    .clone()
                    .on_hover_ui_at_pointer(|ui| tooltip_body(ui, &hits, tabular));
            }
        }
    }
}

fn tooltip_body(ui: &mut egui::Ui, msgs: &[&LintMessage], tabular: &Tabular) {
    ui.set_max_width(420.0);
    for (i, m) in msgs.iter().take(6).enumerate() {
        if i > 0 {
            ui.separator();
        }
        let col = color(ui.ctx(), &m.severity);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(icon(&m.severity)).color(col));
            ui.label(egui::RichText::new(&m.message).strong());
        });
        if let Some(hint) = &m.hint {
            ui.label(egui::RichText::new(hint).small().weak());
        }
        if let Some(first) = m.suggestions.first() {
            let shown = if first.is_empty() {
                "remove it".to_string()
            } else {
                first.clone()
            };
            ui.label(
                egui::RichText::new(format!(
                    "{} Quick fix ({}): {shown}",
                    egui_icons::icons::ICON_LIGHTBULB.codepoint,
                    tabular.keymap.label(crate::keymap::Action::QuickFix)
                ))
                .small()
                .color(super::style::theme_accent(ui.ctx())),
            );
        }
        ui.label(egui::RichText::new(m.source.label()).small().weak());
    }
}

/// Aksi dari panel Problems (dijalankan setelah panel selesai digambar).
enum PanelAction {
    Toggle,
    Close,
    Goto(usize),
    Fix(LintMessage, String),
    SetEnabled(bool),
}

/// Ringkasan jumlah problem di pojok kanan bawah editor dan daftar Problems.
pub(crate) fn render_problems(tabular: &mut Tabular, ui: &egui::Ui, clip: egui::Rect) {
    let (errors, warnings, infos) = tabular.diagnostics.counts();
    let open = tabular.diagnostics.panel_open;
    if errors + warnings + infos == 0 && !open {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut actions: Vec<PanelAction> = Vec::new();
    // Kiri bawah area teks: kanan bawah sudah dipakai tombol editor.
    let anchor = clip.left_bottom() + egui::vec2(8.0, -8.0);
    egui::Area::new(ui.id().with("diag_problems"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(anchor)
        .constrain_to(clip)
        .show(&ctx, |ui| {
            if open {
                problems_list(ui, tabular, &mut actions);
                ui.add_space(4.0);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let frame = egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(8, 3));
                let resp = frame
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (n, sev) in [
                                (errors, LintSeverity::Error),
                                (warnings, LintSeverity::Warning),
                                (infos, LintSeverity::Info),
                            ] {
                                if n > 0 {
                                    ui.label(
                                        egui::RichText::new(icon(&sev))
                                            .color(color(ui.ctx(), &sev))
                                            .small(),
                                    );
                                    ui.label(egui::RichText::new(n.to_string()).small());
                                }
                            }
                            if errors + warnings + infos == 0 {
                                ui.label(egui::RichText::new("No problems").small().weak());
                            }
                        });
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_text(format!(
                        "Problems in this query. Next problem: {}",
                        tabular.keymap.label(crate::keymap::Action::NextProblem)
                    ));
                if resp.clicked() {
                    actions.push(PanelAction::Toggle);
                }
            });
        });
    for action in actions {
        match action {
            PanelAction::Toggle => tabular.diagnostics.panel_open = !tabular.diagnostics.panel_open,
            PanelAction::Close => tabular.diagnostics.panel_open = false,
            PanelAction::Goto(pos) => diagnostics_jobs::move_cursor(tabular, pos),
            PanelAction::Fix(item, s) => {
                diagnostics_jobs::apply_quick_fix(tabular, &item, &s);
            }
            PanelAction::SetEnabled(on) => {
                tabular.live_diagnostics = on;
                tabular.prefs_dirty = true;
                tabular.try_save_prefs();
            }
        }
    }
}

fn problems_list(ui: &mut egui::Ui, tabular: &Tabular, actions: &mut Vec<PanelAction>) {
    let ctx = ui.ctx().clone();
    egui::Frame::popup(ui.style()).show(ui, |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Problems ({})", tabular.diagnostics.items.len()))
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if super::style::render_close_icon_button(ui).clicked() {
                    actions.push(PanelAction::Close);
                }
                let mut on = tabular.live_diagnostics;
                if ui.checkbox(&mut on, "Check while typing").changed() {
                    actions.push(PanelAction::SetEnabled(on));
                }
            });
        });
        ui.separator();
        if tabular.diagnostics.items.is_empty() {
            ui.label(egui::RichText::new("No problems found.").weak());
            return;
        }
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for m in &tabular.diagnostics.items {
                    let pos = m.span.as_ref().map_or(0, |r| r.start);
                    let (line, col) = tabular.editor.offset_to_line_col(pos);
                    let resp = ui
                        .horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(icon(&m.severity))
                                    .color(color(&ctx, &m.severity)),
                            );
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(&m.message).wrap());
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Ln {}, Col {}  ·  {}",
                                        line + 1,
                                        col + 1,
                                        m.source.label()
                                    ))
                                    .small()
                                    .weak(),
                                );
                                if let Some(hint) = &m.hint {
                                    ui.label(egui::RichText::new(hint).small().italics().weak());
                                }
                                if !m.suggestions.is_empty() {
                                    ui.horizontal_wrapped(|ui| {
                                        for s in m.suggestions.iter().take(3) {
                                            let label = if s.is_empty() {
                                                "Remove".to_string()
                                            } else {
                                                format!("Use {s}")
                                            };
                                            if ui
                                                .small_button(format!(
                                                    "{} {label}",
                                                    egui_icons::icons::ICON_LIGHTBULB.codepoint
                                                ))
                                                .clicked()
                                            {
                                                actions
                                                    .push(PanelAction::Fix(m.clone(), s.clone()));
                                            }
                                        }
                                    });
                                }
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.clicked() {
                        actions.push(PanelAction::Goto(pos));
                    }
                    ui.add_space(2.0);
                }
            });
    });
}
