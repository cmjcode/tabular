//! UI fitur data grid (bagian B checklist TablePro): action bar, find bar,
//! penanda sel, menu konteks tambahan, dan jendela (review SQL, riwayat
//! commit, highlight rules, jump to column, FK picker, Row as JSON, pindah
//! kolom).

use super::grid_model::{self as gm, HighlightColor, HighlightRule, HighlightScope, QuickValue};
use super::grid_state::{
    self as gs, FkChild, GridRequests, MAX_FK_DEPTH, MoveColumnStatus, ReviewKind, RowJsonNode,
};
use crate::models::enums::DatabaseType;
use crate::models::structs::{FilterOperator, ForeignKey};
use crate::window_egui::{Tabular, style};
use eframe::egui;
use std::collections::{BTreeSet, HashMap, HashSet};

// ─── Penanda per frame ─────────────────────────────────────────────────────

/// Semua informasi penanda grid yang dihitung sekali per frame sebelum
/// baris dirender (render berjalan dengan borrow immutable atas `Tabular`).
pub(crate) struct GridMarks {
    pub deleted: HashSet<usize>,
    pub inserted: HashSet<usize>,
    pub updated: HashMap<(usize, usize), String>,
    pub find_hits: HashSet<(usize, usize)>,
    pub find_current: Option<(usize, usize)>,
    pub invisible_cols: HashMap<usize, usize>,
    pub show_invisibles: bool,
    pub rules: Vec<HighlightRule>,
}

impl GridMarks {
    pub(crate) fn compute(t: &mut Tabular) -> Self {
        gs::refresh_find_matches(t);
        let invisible_cols = gs::invisible_columns(t);
        let ops = &t.spreadsheet_state.pending_operations;
        let find = &t.grid_ext.find;
        Self {
            deleted: gm::pending_deleted_rows(ops),
            inserted: gm::pending_inserted_rows(ops),
            updated: gm::pending_updated_cells(ops),
            find_hits: find.matches.iter().copied().collect(),
            find_current: find.matches.get(find.current).copied(),
            invisible_cols,
            show_invisibles: !t.grid_ext.hide_invisibles,
            rules: t.grid_ext.highlight_rules.clone(),
        }
    }
}

pub(crate) fn tint(color: HighlightColor, dark: bool, strong: bool) -> egui::Color32 {
    let (r, g, b) = color.rgb();
    let alpha = match (dark, strong) {
        (true, false) => 34,
        (true, true) => 60,
        (false, false) => 45,
        (false, true) => 80,
    };
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

pub(crate) fn pending_delete_tint(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgba_unmultiplied(239, 68, 68, 38)
    } else {
        egui::Color32::from_rgba_unmultiplied(239, 68, 68, 45)
    }
}

pub(crate) fn updated_cell_tint(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgba_unmultiplied(234, 179, 8, 40)
    } else {
        egui::Color32::from_rgba_unmultiplied(234, 179, 8, 60)
    }
}

/// Latar sel sesuai status: diubah (kuning + garis kiri), cocok find, rule.
pub(crate) fn paint_cell_marks(
    ui: &egui::Ui,
    rect: egui::Rect,
    marks: &GridMarks,
    rule_color: Option<HighlightColor>,
    row: usize,
    col: usize,
) {
    let dark = ui.visuals().dark_mode;
    if let Some(color) = rule_color {
        ui.painter().rect_filled(rect, 0.0, tint(color, dark, true));
    }
    if marks.updated.contains_key(&(row, col)) {
        ui.painter().rect_filled(rect, 0.0, updated_cell_tint(dark));
        ui.painter().line_segment(
            [rect.left_top(), rect.left_bottom()],
            egui::Stroke::new(2.5, egui::Color32::from_rgb(234, 179, 8)),
        );
    }
    if marks.find_hits.contains(&(row, col)) {
        let current = marks.find_current == Some((row, col));
        let fill = if current {
            egui::Color32::from_rgba_unmultiplied(249, 115, 22, if dark { 110 } else { 130 })
        } else {
            egui::Color32::from_rgba_unmultiplied(250, 204, 21, if dark { 55 } else { 90 })
        };
        ui.painter().rect_filled(rect.shrink(1.0), 2.0, fill);
        if current {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                2.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(249, 115, 22)),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// Gambar teks sel (rata kiri, tengah vertikal) dengan penanda karakter tak
/// terlihat dan coretan untuk baris yang akan dihapus.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_cell_text(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    italics: bool,
    show_invisibles: bool,
    strike: bool,
) {
    let marker_color = style::theme_warning(ui.ctx());
    let mut job = egui::text::LayoutJob::default();
    let format = |c: egui::Color32| egui::TextFormat {
        font_id: font.clone(),
        color: c,
        italics,
        strikethrough: if strike {
            egui::Stroke::new(1.2, c)
        } else {
            egui::Stroke::NONE
        },
        ..Default::default()
    };
    if show_invisibles && gm::has_invisible(text) {
        for (piece, is_marker) in gm::segment_invisibles(text) {
            let c = if is_marker { marker_color } else { color };
            job.append(&piece, 0.0, format(c));
        }
    } else {
        // Tanpa penanda, newline/tab tetap diratakan agar sel satu baris.
        let flat: String = text
            .chars()
            .map(|c| {
                if c == '\n' || c == '\r' || c == '\t' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        job.append(&flat, 0.0, format(color));
    }
    job.wrap.max_rows = 1;
    job.wrap.max_width = (rect.width() - 8.0).max(8.0);
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    let pos = egui::pos2(rect.left() + 5.0, rect.center().y - galley.size().y * 0.5);
    ui.painter().galley(pos, galley, color);
}

// ─── Action bar & find bar ─────────────────────────────────────────────────

fn icon_button(ui: &mut egui::Ui, icon: &str, enabled: bool, tooltip: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon).size(14.0)).small(),
    )
    .on_hover_text(tooltip)
    .clicked()
}

/// Baris aksi di atas grid: find, kolom, rules, riwayat commit, undo/redo,
/// dan status perubahan pending (Discard / Review & Save).
pub(crate) fn render_grid_action_bar(t: &mut Tabular, ui: &mut egui::Ui, req: &mut GridRequests) {
    let ops = &t.spreadsheet_state.pending_operations;
    let summary = gm::summarize_ops(ops);
    let dirty = !ops.is_empty();
    let hidden = gs::hidden_columns(t);
    let has_custom_order = gs::column_order(t).is_some();
    let can_undo = !t.grid_ext.undo.is_empty();
    let can_redo = !t.grid_ext.redo.is_empty();
    let rules_count = t.grid_ext.highlight_rules.len();
    let rewind_count = t.grid_ext.rewind.len();
    let headers = t.current_table_headers.clone();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.add_space(2.0);

        let find_open = t.grid_ext.find.open;
        if ui
            .selectable_label(
                find_open,
                format!("{} Find", egui_icons::icons::ICON_SEARCH.codepoint),
            )
            .on_hover_text("Find in results (⌘F while the grid is focused)")
            .clicked()
        {
            if find_open {
                t.grid_ext.find.open = false;
            } else {
                req.open_find = true;
            }
        }

        let columns_label = if hidden.is_empty() {
            format!("{} Columns", egui_icons::icons::ICON_VIEW_COLUMN.codepoint)
        } else {
            format!(
                "{} Columns ({} hidden)",
                egui_icons::icons::ICON_VIEW_COLUMN.codepoint,
                hidden.len()
            )
        };
        ui.menu_button(columns_label, |ui| {
            ui.set_min_width(220.0);
            if ui.button("Jump to Column…   ⌘J").clicked() {
                req.open_jump = true;
                ui.close();
            }
            if !hidden.is_empty() && ui.button("Show All Columns").clicked() {
                req.show_all_columns = true;
                ui.close();
            }
            if has_custom_order && ui.button("Reset Column Order").clicked() {
                req.reset_order = true;
                ui.close();
            }
            let mut show_invisibles = !t.grid_ext.hide_invisibles;
            if ui
                .checkbox(&mut show_invisibles, "Show invisible characters")
                .on_hover_text("Render tab, CR, NBSP and zero-width characters as visible symbols")
                .changed()
            {
                t.grid_ext.hide_invisibles = !show_invisibles;
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for h in &headers {
                        let mut visible = !hidden.contains(h);
                        if ui.checkbox(&mut visible, h).changed() {
                            req.toggle_column = Some(h.clone());
                        }
                    }
                });
        });

        let rules_label = if rules_count > 0 {
            format!(
                "{} Rules ({})",
                egui_icons::icons::ICON_PALETTE.codepoint,
                rules_count
            )
        } else {
            format!("{} Rules", egui_icons::icons::ICON_PALETTE.codepoint)
        };
        if ui
            .selectable_label(t.grid_ext.show_rules_editor, rules_label)
            .on_hover_text("Highlight rows or cells by value")
            .clicked()
        {
            t.grid_ext.show_rules_editor = !t.grid_ext.show_rules_editor;
        }

        if rewind_count > 0
            && ui
                .selectable_label(
                    t.grid_ext.show_rewind,
                    format!(
                        "{} History ({})",
                        egui_icons::icons::ICON_HISTORY.codepoint,
                        rewind_count
                    ),
                )
                .on_hover_text("Committed grid changes; restore previous values")
                .clicked()
        {
            t.grid_ext.show_rewind = !t.grid_ext.show_rewind;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
            if dirty {
                if ui
                    .add(style::btn_primary_ctx(ui.ctx(), "Review & Save"))
                    .on_hover_text("Preview the SQL, then commit (⌘S)")
                    .clicked()
                {
                    req.review_save = true;
                }
                if ui
                    .add(style::btn_secondary("Discard"))
                    .on_hover_text("Revert all pending changes (Esc)")
                    .clicked()
                {
                    req.discard = true;
                }
                ui.label(
                    egui::RichText::new(summary.describe())
                        .color(style::theme_warning(ui.ctx()))
                        .strong(),
                );
            }
            if icon_button(
                ui,
                egui_icons::icons::ICON_REDO.codepoint,
                can_redo,
                "Redo (⌘⇧Z)",
            ) {
                req.redo = true;
            }
            if icon_button(
                ui,
                egui_icons::icons::ICON_UNDO.codepoint,
                can_undo,
                "Undo (⌘Z)",
            ) {
                req.undo = true;
            }
        });
    });
}

pub(crate) fn render_find_bar(t: &mut Tabular, ui: &mut egui::Ui) {
    if !t.grid_ext.find.open {
        return;
    }
    let browse = t.is_table_browse_mode;
    let mut step: isize = 0;
    let mut close = false;
    let mut server = false;
    let mut clear_server = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.add_space(2.0);
        let find = &mut t.grid_ext.find;
        let resp = ui.add(
            egui::TextEdit::singleline(&mut find.query)
                .hint_text("Find in loaded rows…")
                .desired_width(240.0),
        );
        if find.focus_request {
            resp.request_focus();
            find.focus_request = false;
        }
        if resp.changed() {
            find.current = 0;
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            step = if ui.input(|i| i.modifiers.shift) {
                -1
            } else {
                1
            };
            resp.request_focus();
        }
        let total = find.matches.len();
        let label = if find.query.is_empty() {
            String::new()
        } else if total == 0 {
            "No matches".to_string()
        } else {
            format!("{} of {}", find.current + 1, total)
        };
        ui.label(egui::RichText::new(label).weak());
        if icon_button(
            ui,
            egui_icons::icons::ICON_ARROW_UP.codepoint,
            total > 0,
            "Previous match (⇧Enter)",
        ) {
            step = -1;
        }
        if icon_button(
            ui,
            egui_icons::icons::ICON_ARROW_DOWN.codepoint,
            total > 0,
            "Next match (Enter)",
        ) {
            step = 1;
        }
        ui.toggle_value(&mut find.case_sensitive, "Aa")
            .on_hover_text("Match case");
        if browse {
            ui.separator();
            if find.server_filter_active {
                if ui
                    .button("Clear server search")
                    .on_hover_text("Remove the search filter and reload the table")
                    .clicked()
                {
                    clear_server = true;
                }
            } else if ui
                .add_enabled(
                    !find.query.trim().is_empty(),
                    egui::Button::new("Search All Rows"),
                )
                .on_hover_text(
                    "Search every column of the whole table on the server (WHERE col LIKE …)",
                )
                .clicked()
            {
                server = true;
            }
        }
        if icon_button(
            ui,
            egui_icons::icons::ICON_CLOSE.codepoint,
            true,
            "Close (Esc)",
        ) {
            close = true;
        }
    });
    if step != 0 {
        gs::refresh_find_matches(t);
        gs::find_step(t, step);
    }
    if server {
        gs::search_all_rows_on_server(t);
    }
    if clear_server {
        gs::clear_server_search(t);
    }
    if close {
        if t.grid_ext.find.server_filter_active {
            gs::clear_server_search(t);
        }
        t.grid_ext.find.open = false;
    }
}

// ─── Saved filters (B6) ────────────────────────────────────────────────────

/// Baris saved filter di bawah Filter Builder: terapkan, jadikan default,
/// hapus, dan simpan filter aktif dengan nama.
pub(crate) fn render_saved_filters_bar(t: &mut Tabular, ui: &mut egui::Ui) {
    if t.grid_ext.active_key.is_none() {
        return;
    }
    let saved = t.grid_ext.saved_filters.clone();
    let has_filter = !t.visual_filter.conditions.is_empty() || !t.sql_filter_text.trim().is_empty();
    let mut apply: Option<super::grid_prefs::SavedFilterPayload> = None;
    let mut remove: Option<String> = None;
    let mut toggle_default: Option<(String, bool)> = None;
    let mut save = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.label(egui::RichText::new("Saved filters:").small().weak());
        let label = if saved.is_empty() {
            format!(
                "{} None saved",
                egui_icons::icons::ICON_FILTER_LIST.codepoint
            )
        } else {
            format!(
                "{} {} saved",
                egui_icons::icons::ICON_FILTER_LIST.codepoint,
                saved.len()
            )
        };
        ui.menu_button(label, |ui| {
            ui.set_min_width(240.0);
            if saved.is_empty() {
                ui.label(egui::RichText::new("No saved filters for this table").weak());
            }
            for filter in &saved {
                ui.horizontal(|ui| {
                    let star = if filter.is_default {
                        egui_icons::icons::ICON_STAR.codepoint
                    } else {
                        egui_icons::icons::ICON_STAR_OUTLINE.codepoint
                    };
                    if icon_button(
                        ui,
                        star,
                        true,
                        "Default: apply automatically when this table opens",
                    ) {
                        toggle_default = Some((filter.name.clone(), !filter.is_default));
                    }
                    if ui.button(&filter.name).clicked() {
                        apply = Some(filter.payload.clone());
                        ui.close();
                    }
                    if icon_button(
                        ui,
                        egui_icons::icons::ICON_DELETE.codepoint,
                        true,
                        "Remove saved filter",
                    ) {
                        remove = Some(filter.name.clone());
                    }
                });
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut t.grid_ext.new_filter_name)
                .hint_text("Filter name")
                .desired_width(140.0),
        );
        ui.checkbox(&mut t.grid_ext.new_filter_default, "Default");
        if ui
            .add_enabled(
                has_filter && !t.grid_ext.new_filter_name.trim().is_empty(),
                egui::Button::new("Save Filter"),
            )
            .on_hover_text("Save the current conditions / WHERE text for this table")
            .clicked()
        {
            save = true;
        }
    });
    if let Some(payload) = apply {
        gs::apply_saved_filter(t, &payload);
    }
    if let Some((name, is_default)) = toggle_default {
        gs::set_filter_default(t, &name, is_default);
    }
    if let Some(name) = remove {
        gs::remove_saved_filter(t, &name);
    }
    if save {
        let name = t.grid_ext.new_filter_name.clone();
        let as_default = t.grid_ext.new_filter_default;
        gs::save_current_filter(t, &name, as_default);
        t.grid_ext.new_filter_name.clear();
        t.grid_ext.new_filter_default = false;
    }
}

// ─── Shortcut grid ─────────────────────────────────────────────────────────

/// Shortcut yang hanya berlaku saat grid fokus (bukan editor/teks lain).
pub(crate) fn handle_grid_shortcuts(t: &Tabular, ui: &egui::Ui, req: &mut GridRequests) {
    let grid_focused = t.table_recently_clicked
        && t.spreadsheet_state.editing_cell.is_none()
        && !ui.ctx().egui_wants_keyboard_input();
    if !grid_focused {
        return;
    }
    let cmd_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    ui.ctx().input_mut(|i| {
        if i.consume_key(cmd_shift, egui::Key::Z)
            || i.consume_key(egui::Modifiers::CTRL, egui::Key::Y)
        {
            req.redo = true;
        } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) {
            req.undo = true;
        }
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::J) {
            req.open_jump = true;
        }
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::F) {
            req.open_find = true;
        }
        // Hapus baris hanya di mode browse: di hasil query bebas tabel
        // tujuannya tidak diketahui sehingga DELETE tidak bisa dibuat.
        let delete_pressed = t.is_table_browse_mode
            && (i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Backspace));
        if delete_pressed {
            let rows: Vec<usize> = if !t.selected_rows.is_empty() {
                t.selected_rows.iter().copied().collect()
            } else {
                t.selected_row.into_iter().collect()
            };
            if !rows.is_empty() {
                req.toggle_delete_rows = Some(rows);
            }
        }
    });
}

// ─── Menu konteks ──────────────────────────────────────────────────────────

pub(crate) struct CellMenuCtx<'a> {
    pub row: usize,
    pub col: usize,
    pub column: &'a str,
    pub value: &'a str,
    pub browse: bool,
    pub fk: Option<&'a ForeignKey>,
    pub pending_delete: bool,
    pub pending_insert: bool,
    pub db_type: Option<&'a DatabaseType>,
    pub selected_rows: &'a BTreeSet<usize>,
}

/// Item tambahan menu klik-kanan sel: filter by value, set value, FK picker,
/// row JSON, highlight rule, sembunyikan kolom, hapus/pulihkan baris.
pub(crate) fn render_cell_menu_extras(
    ui: &mut egui::Ui,
    ctx: &CellMenuCtx<'_>,
    req: &mut GridRequests,
) {
    let null = gm::is_null_cell(ctx.value);
    let shown = gm::display_value(ctx.value).into_owned();
    let preview: String = shown.chars().take(24).collect();
    let preview = if shown.chars().count() > 24 {
        format!("{}…", preview)
    } else {
        preview
    };

    if ctx.browse && !ctx.pending_insert {
        ui.menu_button(
            format!(
                "{} Filter by this value",
                egui_icons::icons::ICON_FILTER_ALT.codepoint
            ),
            |ui| {
                ui.set_min_width(200.0);
                let mut push = |op: FilterOperator| {
                    req.filter_by = Some((ctx.column.to_string(), op, shown.clone()));
                };
                if null {
                    if ui.button(format!("{} IS NULL", ctx.column)).clicked() {
                        push(FilterOperator::IsNull);
                        ui.close();
                    }
                    if ui.button(format!("{} IS NOT NULL", ctx.column)).clicked() {
                        push(FilterOperator::IsNotNull);
                        ui.close();
                    }
                } else {
                    if ui
                        .button(format!("{} = '{}'", ctx.column, preview))
                        .clicked()
                    {
                        push(FilterOperator::Equal);
                        ui.close();
                    }
                    if ui
                        .button(format!("{} != '{}'", ctx.column, preview))
                        .clicked()
                    {
                        push(FilterOperator::NotEqual);
                        ui.close();
                    }
                    if ui
                        .button(format!("{} contains '{}'", ctx.column, preview))
                        .clicked()
                    {
                        push(FilterOperator::Contains);
                        ui.close();
                    }
                    if shown.parse::<f64>().is_ok() {
                        if ui.button(format!("{} > {}", ctx.column, preview)).clicked() {
                            push(FilterOperator::GreaterThan);
                            ui.close();
                        }
                        if ui.button(format!("{} < {}", ctx.column, preview)).clicked() {
                            push(FilterOperator::LessThan);
                            ui.close();
                        }
                    }
                    if ui.button(format!("{} IS NULL", ctx.column)).clicked() {
                        push(FilterOperator::IsNull);
                        ui.close();
                    }
                }
            },
        );
    }

    if !ctx.pending_delete {
        ui.menu_button(
            format!("{} Set Value", egui_icons::icons::ICON_TUNE.codepoint),
            |ui| {
                ui.set_min_width(200.0);
                let default_ok =
                    ctx.pending_insert || ctx.db_type.is_some_and(gm::default_allowed_in_update);
                let options = [
                    QuickValue::Null,
                    QuickValue::EmptyString,
                    QuickValue::Default,
                    QuickValue::Now,
                    QuickValue::Uuid,
                ];
                for option in options {
                    let enabled = option != QuickValue::Default || default_ok;
                    let resp = ui.add_enabled(enabled, egui::Button::new(option.label()));
                    let resp = if enabled {
                        resp
                    } else {
                        resp.on_disabled_hover_text(
                            "SQLite does not support SET column = DEFAULT on existing rows",
                        )
                    };
                    if resp.clicked() {
                        req.set_value = Some((ctx.row, ctx.col, option.cell_value()));
                        ui.close();
                    }
                }
            },
        );
        if let Some(fk) = ctx.fk
            && ui
                .button(format!(
                    "{} Pick from {}…",
                    egui_icons::icons::ICON_LINK.codepoint,
                    fk.referenced_table_name
                ))
                .clicked()
        {
            req.open_fk_picker = Some((ctx.row, ctx.col, fk.clone()));
            ui.close();
        }
    }

    if ui
        .button(format!(
            "{} View Row as JSON…",
            egui_icons::icons::ICON_DATA_OBJECT.codepoint
        ))
        .clicked()
    {
        req.open_row_json = Some(ctx.row);
        ui.close();
    }
    if ui
        .button(format!(
            "{} Highlight rows where {} = '{}'",
            egui_icons::icons::ICON_PALETTE.codepoint,
            ctx.column,
            preview
        ))
        .clicked()
    {
        req.rule_from_cell = Some((ctx.column.to_string(), ctx.value.to_string()));
        ui.close();
    }
    if ui
        .button(format!(
            "{} Hide Column '{}'",
            egui_icons::icons::ICON_VISIBILITY_OFF.codepoint,
            ctx.column
        ))
        .clicked()
    {
        req.hide_column = Some(ctx.column.to_string());
        ui.close();
    }

    if !ctx.browse {
        ui.separator();
        return;
    }
    if !ctx.pending_delete
        && ui
            .button(format!(
                "{} Duplicate Row",
                egui_icons::icons::ICON_CONTENT_COPY.codepoint
            ))
            .clicked()
    {
        req.duplicate_row = Some(ctx.row);
        ui.close();
    }
    let rows: Vec<usize> = if ctx.selected_rows.contains(&ctx.row) {
        ctx.selected_rows.iter().copied().collect()
    } else {
        vec![ctx.row]
    };
    let label = if ctx.pending_delete {
        format!("{} Restore Row", egui_icons::icons::ICON_RESTORE.codepoint)
    } else if rows.len() > 1 {
        format!(
            "{} Delete {} Selected Rows",
            egui_icons::icons::ICON_DELETE.codepoint,
            rows.len()
        )
    } else {
        format!("{} Delete Row", egui_icons::icons::ICON_DELETE.codepoint)
    };
    if ui.button(label).clicked() {
        req.toggle_delete_rows = Some(rows);
        ui.close();
    }
    ui.separator();
}

/// Item tambahan menu header kolom.
pub(crate) fn render_header_menu_extras(
    ui: &mut egui::Ui,
    column: &str,
    browse: bool,
    has_custom_order: bool,
    req: &mut GridRequests,
) {
    ui.separator();
    if ui
        .button(format!(
            "{} Hide Column",
            egui_icons::icons::ICON_VISIBILITY_OFF.codepoint
        ))
        .clicked()
    {
        req.hide_column = Some(column.to_string());
        ui.close();
    }
    if ui.button("Jump to Column…   ⌘J").clicked() {
        req.open_jump = true;
        ui.close();
    }
    if has_custom_order && ui.button("Reset Column Order").clicked() {
        req.reset_order = true;
        ui.close();
    }
    if browse
        && ui
            .button("Move Column in Table…")
            .on_hover_text("Change the physical column order (ALTER TABLE)")
            .clicked()
    {
        req.move_column_physical = Some(column.to_string());
        ui.close();
    }
}

// ─── Jendela ───────────────────────────────────────────────────────────────

pub(crate) fn render_grid_windows(t: &mut Tabular, ctx: &egui::Context) {
    render_review_window(t, ctx);
    render_rewind_window(t, ctx);
    render_rules_editor(t, ctx);
    render_jump_window(t, ctx);
    render_fk_picker(t, ctx);
    render_row_json_window(t, ctx);
    render_move_column_window(t, ctx);
}

fn sql_block(ui: &mut egui::Ui, sql: &str, max_height: f32) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .max_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let mut text = sql;
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                });
        });
}

fn render_review_window(t: &mut Tabular, ctx: &egui::Context) {
    let Some(review) = t.grid_ext.review.clone() else {
        return;
    };
    let mut open = true;
    let mut confirm = false;
    let mut cancel = false;
    let confirm_label = match review.kind {
        ReviewKind::SaveChanges => "Commit Changes",
        ReviewKind::Rewind { .. } => "Restore Values",
        ReviewKind::Ddl => "Execute",
    };
    egui::Window::new(&review.title)
        .id(egui::Id::new("grid_review_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(640.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(&review.summary).strong());
            for warning in &review.warnings {
                ui.label(
                    egui::RichText::new(format!("⚠ {}", warning))
                        .color(style::theme_warning(ui.ctx())),
                );
            }
            ui.add_space(4.0);
            sql_block(ui, &review.sql, 360.0);
            ui.label(
                egui::RichText::new(
                    "Values are escaped and inlined exactly as shown; statements run in this order.",
                )
                .small()
                .weak(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if review.kind == ReviewKind::SaveChanges {
                    ui.checkbox(
                        &mut t.grid_ext.skip_review,
                        "Don't show this review again (this session)",
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(style::btn_primary_ctx(ui.ctx(), confirm_label))
                        .on_hover_text("⌘Enter")
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button("Copy SQL").clicked() {
                        ui.ctx().copy_text(review.sql.clone());
                        t.toasts.info("SQL copied");
                    }
                });
            });
        });
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
        confirm = true;
    }
    if confirm {
        gs::confirm_review(t);
    } else if cancel || !open {
        t.grid_ext.review = None;
    }
}

fn render_rewind_window(t: &mut Tabular, ctx: &egui::Context) {
    if !t.grid_ext.show_rewind {
        return;
    }
    let mut open = true;
    let mut restore: Option<u64> = None;
    egui::Window::new("Committed Changes")
        .id(egui::Id::new("grid_rewind_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(620.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Grid changes committed in this session. Restoring runs the reverse statements shown before execution.",
                )
                .weak(),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                for entry in &t.grid_ext.rewind {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&entry.table).strong());
                            ui.label(egui::RichText::new(&entry.committed_at).weak());
                            ui.label(entry.summary.describe());
                            if entry.restored {
                                ui.label(
                                    egui::RichText::new("restored")
                                        .color(style::theme_success(ui.ctx())),
                                );
                            }
                        });
                        for note in &entry.notes {
                            ui.label(
                                egui::RichText::new(format!("⚠ {}", note))
                                    .small()
                                    .color(style::theme_warning(ui.ctx())),
                            );
                        }
                        egui::CollapsingHeader::new("Committed SQL")
                            .id_salt(("rewind_fwd", entry.id))
                            .show(ui, |ui| sql_block(ui, &entry.forward_sql, 160.0));
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    entry.rewind_sql.is_some(),
                                    style::btn_primary_ctx(ui.ctx(), "Restore Previous Values…"),
                                )
                                .clicked()
                            {
                                restore = Some(entry.id);
                            }
                            if let Some(sql) = &entry.rewind_sql
                                && ui.button("Copy Reverse SQL").clicked()
                            {
                                ui.ctx().copy_text(sql.clone());
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        });
    if let Some(id) = restore {
        gs::open_rewind_review(t, id);
    }
    if !open {
        t.grid_ext.show_rewind = false;
    }
}

fn color_swatch(ui: &mut egui::Ui, color: HighlightColor) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let (r, g, b) = color.rgb();
    ui.painter()
        .rect_filled(rect, 3.0, egui::Color32::from_rgb(r, g, b));
}

fn render_rules_editor(t: &mut Tabular, ctx: &egui::Context) {
    if !t.grid_ext.show_rules_editor {
        return;
    }
    let mut open = true;
    let mut persist = false;
    let headers = t.current_table_headers.clone();
    let table_label = t.grid_ext.active_key.as_ref().map(|k| k.table.clone());
    egui::Window::new("Highlight Rules")
        .id(egui::Id::new("grid_rules_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(640.0)
        .show(ctx, |ui| {
            match &table_label {
                Some(table) => ui.label(
                    egui::RichText::new(format!("Saved for table {}", table)).weak(),
                ),
                None => ui.label(
                    egui::RichText::new(
                        "Rules are saved per table when browsing a table; for query results they last for this session.",
                    )
                    .weak(),
                ),
            };
            ui.add_space(4.0);
            let mut remove: Option<usize> = None;
            let mut move_up: Option<usize> = None;
            let rules = &mut t.grid_ext.highlight_rules;
            for (i, rule) in rules.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    persist |= ui.checkbox(&mut rule.enabled, "").changed();
                    egui::ComboBox::from_id_salt(("rule_col", i))
                        .selected_text(&rule.column)
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for h in &headers {
                                persist |= ui
                                    .selectable_value(&mut rule.column, h.clone(), h)
                                    .changed();
                            }
                        });
                    egui::ComboBox::from_id_salt(("rule_op", i))
                        .selected_text(rule.operator.label())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for op in gm::highlight_operators() {
                                persist |= ui
                                    .selectable_value(&mut rule.operator, *op, op.label())
                                    .changed();
                            }
                        });
                    if !matches!(
                        rule.operator,
                        FilterOperator::IsNull | FilterOperator::IsNotNull
                    ) {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut rule.value)
                                .hint_text("value")
                                .desired_width(110.0),
                        );
                        persist |= resp.lost_focus();
                    }
                    color_swatch(ui, rule.color);
                    egui::ComboBox::from_id_salt(("rule_color", i))
                        .selected_text(rule.color.label())
                        .width(80.0)
                        .show_ui(ui, |ui| {
                            for c in HighlightColor::all() {
                                ui.horizontal(|ui| {
                                    color_swatch(ui, *c);
                                    persist |= ui
                                        .selectable_value(&mut rule.color, *c, c.label())
                                        .changed();
                                });
                            }
                        });
                    egui::ComboBox::from_id_salt(("rule_scope", i))
                        .selected_text(match rule.scope {
                            HighlightScope::Row => "Row",
                            HighlightScope::Cell => "Cell",
                        })
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            persist |= ui
                                .selectable_value(&mut rule.scope, HighlightScope::Row, "Row")
                                .changed();
                            persist |= ui
                                .selectable_value(&mut rule.scope, HighlightScope::Cell, "Cell")
                                .changed();
                        });
                    if i > 0
                        && icon_button(
                            ui,
                            egui_icons::icons::ICON_ARROW_UP.codepoint,
                            true,
                            "Move up (earlier rules win)",
                        )
                    {
                        move_up = Some(i);
                    }
                    if icon_button(ui, egui_icons::icons::ICON_CLOSE.codepoint, true, "Remove rule")
                    {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                rules.remove(i);
                persist = true;
            }
            if let Some(i) = move_up {
                rules.swap(i, i - 1);
                persist = true;
            }
            if rules.is_empty() {
                ui.label(
                    egui::RichText::new("No rules yet. Rules are evaluated top to bottom; the first match wins.")
                        .italics()
                        .weak(),
                );
            }
            ui.add_space(6.0);
            if ui.button("+ Add Rule").clicked() {
                rules.push(HighlightRule::new(headers.first().cloned().unwrap_or_default()));
                persist = true;
            }
        });
    if persist {
        gs::persist_highlight_rules(t);
    }
    if !open {
        gs::persist_highlight_rules(t);
        t.grid_ext.show_rules_editor = false;
    }
}

fn render_jump_window(t: &mut Tabular, ctx: &egui::Context) {
    if !t.grid_ext.jump.open {
        return;
    }
    let candidates = gs::jump_candidates(t);
    let hidden = gs::hidden_columns(t);
    let types = t.grid_ext.jump.types.clone();
    let mut chosen: Option<usize> = None;
    let mut open = true;
    let len = candidates.len();
    ctx.input_mut(|i| {
        let jump = &mut t.grid_ext.jump;
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) && len > 0 {
            jump.selected = (jump.selected + 1) % len;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) && len > 0 {
            jump.selected = (jump.selected + len - 1) % len;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            chosen = candidates
                .get(jump.selected.min(len.saturating_sub(1)))
                .copied();
        }
    });
    egui::Window::new("Jump to Column")
        .id(egui::Id::new("grid_jump_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
        .show(ctx, |ui| {
            let jump = &mut t.grid_ext.jump;
            let resp = ui.add(
                egui::TextEdit::singleline(&mut jump.query)
                    .hint_text("Jump to column…")
                    .desired_width(f32::INFINITY),
            );
            if jump.focus_request {
                resp.request_focus();
                jump.focus_request = false;
            }
            if resp.changed() {
                jump.selected = 0;
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (pos, &col) in candidates.iter().enumerate().take(200) {
                        let name = &t.current_table_headers[col];
                        let ty = types.get(col).map(String::as_str).unwrap_or("");
                        let selected = pos == t.grid_ext.jump.selected;
                        let text = format!(
                            "{}{}   {}   #{}",
                            name,
                            if hidden.contains(name) {
                                " (hidden)"
                            } else {
                                ""
                            },
                            ty,
                            col + 1
                        );
                        let resp = ui.selectable_label(selected, text);
                        if selected {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() {
                            chosen = Some(col);
                        }
                    }
                    if candidates.is_empty() {
                        ui.label(egui::RichText::new("No matching column").weak());
                    }
                });
        });
    if let Some(col) = chosen {
        gs::jump_to_column(t, col);
    } else if !open {
        t.grid_ext.jump.open = false;
    }
}

fn render_fk_picker(t: &mut Tabular, ctx: &egui::Context) {
    let Some(picker) = t.grid_ext.fk_picker.as_ref() else {
        return;
    };
    let title = format!(
        "Pick {}.{} for {}",
        picker.fk.referenced_table_name, picker.fk.referenced_column_name, picker.column
    );
    let mut open = true;
    let mut chosen: Option<String> = None;
    let mut refetch = false;
    egui::Window::new(title)
        .id(egui::Id::new("grid_fk_picker"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .default_height(420.0)
        .show(ctx, |ui| {
            let Some(picker) = t.grid_ext.fk_picker.as_mut() else {
                return;
            };
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut picker.search)
                        .hint_text("Search key or label…")
                        .desired_width(300.0),
                );
                if picker.focus_request {
                    resp.request_focus();
                    picker.focus_request = false;
                }
                if resp.changed() {
                    picker.search_changed_at = Some(std::time::Instant::now());
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    refetch = true;
                }
                if picker.loading {
                    ui.add(egui::Spinner::new());
                }
                if ui.button("Set NULL").clicked() {
                    chosen = Some("NULL".to_string());
                }
            });
            if let Some(at) = picker.search_changed_at {
                if at.elapsed() >= std::time::Duration::from_millis(350) {
                    refetch = true;
                } else {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(120));
                }
            }
            if let Some(err) = &picker.error {
                ui.label(egui::RichText::new(err).color(style::theme_danger(ui.ctx())));
            }
            ui.add_space(4.0);
            let key = picker.fk.referenced_column_name.clone();
            let key_idx = picker
                .headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(&key));
            let label_idx: Vec<usize> = picker
                .label_cols
                .iter()
                .filter_map(|c| picker.headers.iter().position(|h| h == c))
                .collect();
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("fk_picker_grid")
                    .striped(true)
                    .num_columns(1 + label_idx.len())
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(&key).strong());
                        for &i in &label_idx {
                            ui.label(egui::RichText::new(&picker.headers[i]).strong());
                        }
                        ui.end_row();
                        for row in &picker.rows {
                            let Some(value) = key_idx.and_then(|i| row.get(i)) else {
                                continue;
                            };
                            if ui
                                .selectable_label(false, egui::RichText::new(value).monospace())
                                .clicked()
                            {
                                chosen = Some(value.clone());
                            }
                            for &i in &label_idx {
                                let text = row.get(i).map(String::as_str).unwrap_or("");
                                let short: String = text.chars().take(60).collect();
                                if ui.selectable_label(false, short).clicked() {
                                    chosen = Some(value.clone());
                                }
                            }
                            ui.end_row();
                        }
                    });
                if !picker.loading && picker.rows.is_empty() && picker.error.is_none() {
                    ui.label(egui::RichText::new("No rows").weak());
                }
            });
            if picker.rows.len() >= gs::FK_PICKER_LIMIT {
                ui.label(
                    egui::RichText::new(format!(
                        "Showing the first {} rows; refine the search to narrow down.",
                        gs::FK_PICKER_LIMIT
                    ))
                    .small()
                    .weak(),
                );
            }
        });
    if let Some(value) = chosen {
        gs::fk_picker_choose(t, value);
    } else if !open {
        t.grid_ext.fk_picker = None;
    } else if refetch {
        gs::fk_picker_fetch(t);
    }
}

/// Render satu node baris; permintaan expand FK dikumpulkan ke `expand`.
fn render_row_node(
    ui: &mut egui::Ui,
    node: &RowJsonNode,
    path: &[usize],
    fks: &[ForeignKey],
    expand: &mut Option<(Vec<usize>, usize)>,
) {
    egui::Grid::new(("row_json_grid", path.to_vec()))
        .num_columns(2)
        .spacing(egui::vec2(12.0, 3.0))
        .show(ui, |ui| {
            for (i, col) in node.columns.iter().enumerate() {
                let raw = node.values.get(i).map(String::as_str).unwrap_or("NULL");
                ui.label(egui::RichText::new(col).strong());
                ui.horizontal(|ui| {
                    if gm::is_null_cell(raw) {
                        ui.label(egui::RichText::new("NULL").italics().weak());
                    } else {
                        let shown: String = gm::display_value(raw).chars().take(200).collect();
                        ui.label(egui::RichText::new(shown).monospace());
                    }
                    let fk = fk_for_node(fks, node, i);
                    if let Some(fk) = fk
                        && !gm::is_null_cell(raw)
                    {
                        match node.children.get(&i) {
                            None if path.len() < MAX_FK_DEPTH => {
                                if ui
                                    .small_button(format!(
                                        "{} {}",
                                        egui_icons::icons::ICON_CHEVRON_RIGHT.codepoint,
                                        fk.referenced_table_name
                                    ))
                                    .on_hover_text(format!(
                                        "Load {}.{} = {}",
                                        fk.referenced_table_name, fk.referenced_column_name, raw
                                    ))
                                    .clicked()
                                {
                                    *expand = Some((path.to_vec(), i));
                                }
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new(format!("max depth {}", MAX_FK_DEPTH))
                                        .small()
                                        .weak(),
                                );
                            }
                            Some(FkChild::Loading) => {
                                ui.add(egui::Spinner::new().size(12.0));
                            }
                            Some(FkChild::NotFound) => {
                                ui.label(
                                    egui::RichText::new("referenced row not found")
                                        .small()
                                        .color(style::theme_warning(ui.ctx())),
                                );
                            }
                            Some(FkChild::Failed(err)) => {
                                ui.label(
                                    egui::RichText::new(err)
                                        .small()
                                        .color(style::theme_danger(ui.ctx())),
                                );
                            }
                            Some(FkChild::Loaded(_)) => {}
                        }
                    }
                });
                ui.end_row();
                if let Some(FkChild::Loaded(child)) = node.children.get(&i) {
                    ui.label("");
                    ui.vertical(|ui| {
                        let mut child_path = path.to_vec();
                        child_path.push(i);
                        egui::CollapsingHeader::new(format!(
                            "{} {}",
                            egui_icons::icons::ICON_LINK.codepoint,
                            child.table
                        ))
                        .id_salt(("row_json_child", child_path.clone()))
                        .default_open(true)
                        .show(ui, |ui| {
                            render_row_node(ui, child, &child_path, fks, expand);
                        });
                    });
                    ui.end_row();
                }
            }
        });
}

fn fk_for_node<'a>(
    fks: &'a [ForeignKey],
    node: &RowJsonNode,
    col: usize,
) -> Option<&'a ForeignKey> {
    node.columns
        .get(col)
        .and_then(|c| gs::fk_for(fks, node.column_table(col), c))
}

fn render_row_json_window(t: &mut Tabular, ctx: &egui::Context) {
    let Some(viewer) = t.grid_ext.row_json.as_mut() else {
        return;
    };
    let mut open = viewer.open;
    let mut expand: Option<(Vec<usize>, usize)> = None;
    let title = if viewer.root.table.is_empty() {
        format!("{} as JSON", viewer.row_label)
    } else {
        format!("{} of {} as JSON", viewer.row_label, viewer.root.table)
    };
    egui::Window::new(title)
        .id(egui::Id::new("grid_row_json"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .default_height(520.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut viewer.show_raw_json, false, "Tree");
                ui.selectable_value(&mut viewer.show_raw_json, true, "JSON");
                ui.separator();
                if ui
                    .button(format!(
                        "{} Copy JSON",
                        egui_icons::icons::ICON_CONTENT_COPY.codepoint
                    ))
                    .clicked()
                {
                    let json =
                        serde_json::to_string_pretty(&viewer.root.to_json()).unwrap_or_default();
                    ui.ctx().copy_text(json);
                }
                if viewer.fks.is_empty() {
                    ui.label(
                        egui::RichText::new("No foreign keys cached for this database")
                            .small()
                            .weak(),
                    );
                }
            });
            ui.separator();
            egui::ScrollArea::both().show(ui, |ui| {
                if viewer.show_raw_json {
                    let json =
                        serde_json::to_string_pretty(&viewer.root.to_json()).unwrap_or_default();
                    sql_block(ui, &json, f32::INFINITY);
                } else {
                    render_row_node(ui, &viewer.root, &[], &viewer.fks, &mut expand);
                }
            });
        });
    viewer.open = open;
    if !open {
        t.grid_ext.row_json = None;
    } else if let Some((path, col)) = expand {
        gs::expand_row_json_fk(t, path, col);
    }
}

fn render_move_column_window(t: &mut Tabular, ctx: &egui::Context) {
    let Some(dialog) = t.grid_ext.move_column.as_mut() else {
        return;
    };
    let mut open = true;
    let mut review = false;
    egui::Window::new(format!("Move Column `{}`", dialog.column))
        .id(egui::Id::new("grid_move_column"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| match &dialog.status {
            MoveColumnStatus::Loading => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label("Reading table definition…");
                });
            }
            MoveColumnStatus::Unsupported(msg) => {
                ui.label(msg);
            }
            MoveColumnStatus::Failed(err) => {
                ui.label(egui::RichText::new(err).color(style::theme_danger(ui.ctx())));
            }
            MoveColumnStatus::Ready { definition } => {
                ui.label(egui::RichText::new(definition).monospace().small());
                ui.add_space(6.0);
                let selected = match &dialog.after {
                    Some(c) => format!("After {}", c),
                    None => "First".to_string(),
                };
                ui.horizontal(|ui| {
                    ui.label("Position:");
                    egui::ComboBox::from_id_salt("move_col_after")
                        .selected_text(selected)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut dialog.after, None, "First");
                            for c in &dialog.columns {
                                if c != &dialog.column {
                                    ui.selectable_value(
                                        &mut dialog.after,
                                        Some(c.clone()),
                                        format!("After {}", c),
                                    );
                                }
                            }
                        });
                });
                ui.add_space(6.0);
                if ui
                    .add(style::btn_primary_ctx(ui.ctx(), "Review ALTER TABLE…"))
                    .clicked()
                {
                    review = true;
                }
            }
        });
    if review {
        gs::review_move_column(t);
    } else if !open {
        t.grid_ext.move_column = None;
    }
}
