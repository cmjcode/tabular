//! Dialog Data Compare & Sync (H11) dan saved comparisons (H12). Logika
//! pembanding ada di `crate::data_transfer::compare`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

use super::transfer_ui::{
    ComboPick, EndpointPick, Slot, card_row, error_label, filter_combo, fmt_count, modal,
    muted_label,
};
use super::{Tabular, style};
use crate::data_transfer::compare::{
    self, CompareOptions, CompareOutcome, CompareProgressHandle, DatabaseComparison, DiffKind,
    RowDiff, SyncParts,
};
use crate::data_transfer::compare_structure::{
    self, ColumnAttribute, ColumnDiff, ColumnDiffKind, DatabaseStructureComparison,
    StructureOptions, StructureSyncParts, TableStructureOutcome, count_executable_statements,
};
use crate::data_transfer::saved::{self, CompareMode, SavedComparison, SavedEndpoint};

/// Baris selisih yang digambar; sisanya tetap masuk skrip sinkronisasi.
const MAX_ROWS_SHOWN: usize = 300;
const MAX_CELL_CHARS: usize = 60;
/// Pilihan tabel yang berarti seluruh database dibandingkan.
const ALL_TABLES: &str = "All tables";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompareScriptKey {
    Data(u64, Option<usize>, SyncParts),
    Structure(u64, Option<usize>, StructureSyncParts),
}

pub(super) struct CompareDialog {
    mode: CompareMode,
    source: EndpointPick,
    /// Kosong di kedua sisi = bandingkan semua tabel database.
    source_table: String,
    target: EndpointPick,
    target_table: String,
    key_columns: String,
    ignore_columns: String,
    where_clause: String,
    row_limit: String,
    tolerant: bool,
    trim: bool,
    case_insensitive: bool,
    compare_nullability: bool,
    slot: Option<Slot<Result<CompareOutcome, String>>>,
    outcome: Option<CompareOutcome>,
    db_slot: Option<Slot<Result<DatabaseComparison, String>>>,
    db_outcome: Option<DatabaseComparison>,
    structure_slot: Option<Slot<Result<TableStructureOutcome, String>>>,
    structure_outcome: Option<TableStructureOutcome>,
    structure_db_slot: Option<Slot<Result<DatabaseStructureComparison, String>>>,
    structure_db_outcome: Option<DatabaseStructureComparison>,
    /// Tabel yang rinciannya ditampilkan; `None` = semua tabel.
    db_selected: Option<usize>,
    db_only_diff: bool,
    progress: CompareProgressHandle,
    cancel: Option<Arc<AtomicBool>>,
    /// Skrip sinkronisasi untuk `script_key`; disusun ulang hanya bila hasil,
    /// pilihan tabel, atau bagian skrip berubah, bukan tiap frame.
    script: Option<Vec<String>>,
    script_key: Option<CompareScriptKey>,
    generation: u64,
    error: Option<String>,
    show_source_only: bool,
    show_target_only: bool,
    show_changed: bool,
    show_struct_source_only: bool,
    show_struct_target_only: bool,
    show_struct_changed: bool,
    parts: SyncParts,
    structure_parts: StructureSyncParts,
    saved: Vec<SavedComparison>,
    saved_loaded: bool,
    saved_selected: Option<String>,
    save_name: String,
    confirm_apply: bool,
    apply_slot: Option<Slot<Result<usize, String>>>,
    apply_message: Option<Result<String, String>>,
    /// Form sumber/tujuan/opsi terlihat. Diciutkan setelah hasil keluar supaya
    /// tabel selisih dan skrip sinkronisasi muat tanpa menggulir.
    setup_open: bool,
}

impl CompareDialog {
    pub(super) fn new(
        conn_id: Option<i64>,
        database: Option<String>,
        table: Option<String>,
    ) -> Self {
        let defaults = CompareOptions::default();
        let table = table.unwrap_or_default();
        Self {
            mode: CompareMode::Data,
            source: EndpointPick::new(conn_id, database),
            source_table: table.clone(),
            target: EndpointPick::default(),
            target_table: table,
            key_columns: String::new(),
            ignore_columns: String::new(),
            where_clause: String::new(),
            row_limit: defaults
                .row_limit
                .map(|n| n.to_string())
                .unwrap_or_default(),
            tolerant: defaults.tolerant_values,
            trim: defaults.trim_trailing_space,
            case_insensitive: defaults.case_insensitive,
            compare_nullability: true,
            slot: None,
            outcome: None,
            db_slot: None,
            db_outcome: None,
            structure_slot: None,
            structure_outcome: None,
            structure_db_slot: None,
            structure_db_outcome: None,
            db_selected: None,
            db_only_diff: false,
            progress: CompareProgressHandle::default(),
            cancel: None,
            script: None,
            script_key: None,
            generation: 0,
            error: None,
            show_source_only: true,
            show_target_only: true,
            show_changed: true,
            show_struct_source_only: true,
            show_struct_target_only: true,
            show_struct_changed: true,
            parts: SyncParts::default(),
            structure_parts: StructureSyncParts::default(),
            saved: Vec::new(),
            saved_loaded: false,
            saved_selected: None,
            save_name: String::new(),
            confirm_apply: false,
            apply_slot: None,
            apply_message: None,
            setup_open: true,
        }
    }

    fn busy(&self) -> bool {
        self.slot.is_some()
            || self.db_slot.is_some()
            || self.structure_slot.is_some()
            || self.structure_db_slot.is_some()
            || self.apply_slot.is_some()
    }

    /// Tanpa tabel di kedua sisi: seluruh database dibandingkan.
    fn all_tables(&self) -> bool {
        self.source_table.trim().is_empty() && self.target_table.trim().is_empty()
    }

    fn clear_results(&mut self) {
        self.outcome = None;
        self.db_outcome = None;
        self.structure_outcome = None;
        self.structure_db_outcome = None;
        self.db_selected = None;
        self.generation += 1;
    }

    fn build_script(&self) -> Option<Vec<String>> {
        match self.mode {
            CompareMode::Data => {
                let one = |o: &CompareOutcome| {
                    compare::sync_statements(
                        &o.result,
                        &o.target_db,
                        &o.target_sql,
                        &o.kinds,
                        self.parts,
                    )
                };
                if let Some(outcome) = &self.outcome {
                    return Some(one(outcome));
                }
                let db = self.db_outcome.as_ref()?;
                let readable = |i: usize| db.tables.get(i).and_then(|t| t.outcome.as_ref().ok());
                Some(match self.db_selected {
                    Some(i) => readable(i).map(one).unwrap_or_default(),
                    None => (0..db.tables.len())
                        .filter_map(readable)
                        .flat_map(one)
                        .collect(),
                })
            }
            CompareMode::Structure => {
                if let Some(outcome) = &self.structure_outcome {
                    return Some(compare_structure::structure_sync_statements(
                        &outcome.result,
                        &outcome.source_db,
                        &outcome.target_db,
                        &outcome.target_sql,
                        self.structure_parts,
                    ));
                }
                let db = self.structure_db_outcome.as_ref()?;
                Some(compare_structure::database_structure_sync_statements(
                    db,
                    self.db_selected,
                    self.structure_parts,
                ))
            }
        }
    }

    fn refresh_script(&mut self) {
        let key = match self.mode {
            CompareMode::Data => {
                CompareScriptKey::Data(self.generation, self.db_selected, self.parts)
            }
            CompareMode::Structure => {
                CompareScriptKey::Structure(self.generation, self.db_selected, self.structure_parts)
            }
        };
        if self.script_key != Some(key) {
            self.script_key = Some(key);
            self.script = self.build_script();
        }
    }

    fn options(&self) -> Result<CompareOptions, String> {
        let list = |text: &str| -> Vec<String> {
            text.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let limit = self.row_limit.trim().replace([',', '_'], "");
        let row_limit = if limit.is_empty() {
            None
        } else {
            Some(
                limit
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| "Row limit must be a positive number".to_string())?,
            )
        };
        let where_clause = self.where_clause.trim();
        Ok(CompareOptions {
            key_columns: list(&self.key_columns),
            where_clause: (!where_clause.is_empty()).then(|| where_clause.to_string()),
            row_limit,
            ignore_columns: list(&self.ignore_columns),
            trim_trailing_space: self.trim,
            tolerant_values: self.tolerant,
            case_insensitive: self.case_insensitive,
        })
    }

    fn structure_options(&self) -> StructureOptions {
        let list = |text: &str| -> Vec<String> {
            text.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        StructureOptions {
            ignore_columns: list(&self.ignore_columns),
            case_insensitive_names: self.case_insensitive,
            compare_nullability: self.compare_nullability,
        }
    }

    fn problem(&self) -> Option<String> {
        if self.source.conn_id.is_none() || self.target.conn_id.is_none() {
            return Some("Choose source and target connections".to_string());
        }
        if self.source_table.trim().is_empty() != self.target_table.trim().is_empty() {
            return Some(format!(
                "Choose a table on both sides, or \"{ALL_TABLES}\" on both"
            ));
        }
        match self.mode {
            CompareMode::Data => self.options().err(),
            CompareMode::Structure => None,
        }
    }

    fn apply_saved(&mut self, item: &SavedComparison) {
        self.mode = item.mode;
        self.source = EndpointPick::new(
            Some(item.source.connection_id),
            item.source.database.clone(),
        );
        self.target = EndpointPick::new(
            Some(item.target.connection_id),
            item.target.database.clone(),
        );
        self.source_table = item.source.table.clone();
        self.target_table = item.target.table.clone();
        self.key_columns = item.options.key_columns.join(", ");
        self.ignore_columns = match item.mode {
            CompareMode::Data => item.options.ignore_columns.join(", "),
            CompareMode::Structure => item.structure_options.ignore_columns.join(", "),
        };
        self.where_clause = item.options.where_clause.clone().unwrap_or_default();
        self.row_limit = item
            .options
            .row_limit
            .map(|n| n.to_string())
            .unwrap_or_default();
        self.tolerant = item.options.tolerant_values;
        self.trim = item.options.trim_trailing_space;
        self.case_insensitive = match item.mode {
            CompareMode::Data => item.options.case_insensitive,
            CompareMode::Structure => item.structure_options.case_insensitive_names,
        };
        self.compare_nullability = item.structure_options.compare_nullability;
        self.save_name = item.name.clone();
        self.saved_selected = Some(item.name.clone());
        self.clear_results();
        self.error = None;
        self.apply_message = None;
        self.setup_open = true;
    }

    fn to_saved(&self) -> Option<SavedComparison> {
        let database = |pick: &EndpointPick| {
            let db = pick.database.trim();
            (!db.is_empty()).then(|| db.to_string())
        };
        Some(SavedComparison {
            name: self.save_name.trim().to_string(),
            source: SavedEndpoint {
                connection_id: self.source.conn_id?,
                database: database(&self.source),
                table: self.source_table.trim().to_string(),
            },
            target: SavedEndpoint {
                connection_id: self.target.conn_id?,
                database: database(&self.target),
                table: self.target_table.trim().to_string(),
            },
            mode: self.mode,
            options: self.options().unwrap_or_default(),
            structure_options: self.structure_options(),
            updated_at: String::new(),
        })
    }
}

fn clip(value: &str) -> String {
    // String kosong dibedakan dari sel yang tidak ada dan dari NULL.
    if value.is_empty() {
        return "(empty)".to_string();
    }
    let flat = value.replace(['\n', '\r'], " ");
    if flat.chars().count() > MAX_CELL_CHARS {
        let cut: String = flat.chars().take(MAX_CELL_CHARS).collect();
        format!("{cut}...")
    } else {
        flat
    }
}

/// Baris "Table" di grid endpoint: combobox berisi tabel endpoint, dengan
/// [`ALL_TABLES`] di puncak.
fn table_row(ui: &mut egui::Ui, pick: &EndpointPick, table: &mut String, salt: &str, width: f32) {
    ui.label(egui::RichText::new("Table").weak().small());
    ui.horizontal(|ui| {
        ui.add_enabled_ui(pick.conn_id.is_some(), |ui| {
            let text = if table.trim().is_empty() {
                ALL_TABLES.to_string()
            } else {
                table.clone()
            };
            match filter_combo(
                ui,
                (salt, "table"),
                &text,
                width,
                Some(ALL_TABLES),
                &pick.tables,
            ) {
                Some(ComboPick::Pinned) => table.clear(),
                Some(ComboPick::Item(i)) => *table = pick.tables[i].clone(),
                None => {}
            }
        });
        if pick.loading() {
            ui.add(egui::Spinner::new());
        }
    });
    ui.end_row();
}

/// Ringkasan per tabel untuk perbandingan database. Mengembalikan pilihan
/// baru bila sebuah baris diklik (`None` di dalamnya = semua tabel).
fn database_summary(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    db: &DatabaseComparison,
    selected: Option<usize>,
    only_diff: &mut bool,
) -> Option<Option<usize>> {
    let ok = style::theme_success(ctx);
    let warn = style::theme_warning(ctx);
    let bad = style::theme_danger(ctx);
    let different = db.tables.iter().filter(|t| t.differs()).count();
    let failed = db.tables.iter().filter(|t| t.outcome.is_err()).count();
    let identical = db.tables.len() - different - failed;
    let mut pick = None;
    style::render_modal_card(ui, Some("Tables"), None, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} compared: {} identical, {} different, {} failed",
                    fmt_count(db.tables.len() as u64),
                    fmt_count(identical as u64),
                    fmt_count(different as u64),
                    fmt_count(failed as u64)
                ))
                .strong(),
            );
            ui.checkbox(only_diff, "Hide identical tables");
        });
        if db.cancelled {
            error_label(ui, "Stopped before all tables were compared.");
        }
        muted_label(
            ui,
            "Click a table to see its rows and build its sync script.",
        );
        let number = |ui: &mut egui::Ui, n: usize, color: egui::Color32| {
            let text = egui::RichText::new(fmt_count(n as u64)).small();
            ui.label(if n > 0 {
                text.color(color)
            } else {
                text.weak()
            });
        };
        egui::ScrollArea::both()
            .id_salt("compare_db_scroll")
            .max_height(200.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("compare_db_grid")
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for head in [
                            "Table",
                            "Result",
                            "Only in source",
                            "Only in target",
                            "Changed",
                            "Identical",
                        ] {
                            ui.label(egui::RichText::new(head).small().strong());
                        }
                        ui.end_row();

                        if ui
                            .selectable_label(selected.is_none(), ALL_TABLES)
                            .clicked()
                        {
                            pick = Some(None);
                        }
                        ui.end_row();

                        for (i, table) in db.tables.iter().enumerate() {
                            if *only_diff && !table.differs() && table.outcome.is_ok() {
                                continue;
                            }
                            let name = if table
                                .source_table
                                .eq_ignore_ascii_case(&table.target_table)
                            {
                                table.source_table.clone()
                            } else {
                                format!("{} (target: {})", table.source_table, table.target_table)
                            };
                            if ui.selectable_label(selected == Some(i), name).clicked() {
                                pick = Some(Some(i));
                            }
                            match &table.outcome {
                                Ok(outcome) => {
                                    let result = &outcome.result;
                                    let (only_source, only_target, changed) = result.counts();
                                    let (text, color) = if result.diffs.is_empty() {
                                        ("Identical", ok)
                                    } else {
                                        ("Different", warn)
                                    };
                                    let text = if result.truncated {
                                        format!("{text} (row limit reached)")
                                    } else {
                                        text.to_string()
                                    };
                                    ui.label(egui::RichText::new(text).small().color(color));
                                    number(ui, only_source, ok);
                                    number(ui, only_target, bad);
                                    number(ui, changed, warn);
                                    number(ui, result.identical, ui.visuals().text_color());
                                }
                                Err(e) => {
                                    ui.label(egui::RichText::new(clip(e)).small().color(bad))
                                        .on_hover_text(e);
                                }
                            }
                            ui.end_row();
                        }
                        for (names, text) in [
                            (&db.source_only, "Missing in target"),
                            (&db.target_only, "Missing in source"),
                        ] {
                            for name in names {
                                ui.label(name);
                                ui.label(egui::RichText::new(text).small().color(bad));
                                ui.end_row();
                            }
                        }
                    });
            });
    });
    pick
}

fn diff_row(ui: &mut egui::Ui, ctx: &egui::Context, diff: &RowDiff, columns: usize) {
    let added = style::theme_success(ctx);
    let removed = style::theme_danger(ctx);
    let (label, color) = match diff.kind {
        DiffKind::OnlyInSource => ("Only in source", added),
        DiffKind::OnlyInTarget => ("Only in target", removed),
        // Kuning, supaya tidak tertukar dengan merah "Only in target".
        DiffKind::Changed => ("Changed", style::theme_warning(ctx)),
    };
    ui.label(egui::RichText::new(label).small().strong().color(color));
    for col in 0..columns {
        match diff.kind {
            DiffKind::OnlyInSource => {
                ui.label(
                    egui::RichText::new(clip(&diff.source[col]))
                        .small()
                        .color(added),
                );
            }
            DiffKind::OnlyInTarget => {
                ui.label(
                    egui::RichText::new(clip(&diff.target[col]))
                        .small()
                        .color(removed),
                );
            }
            DiffKind::Changed if diff.changed.contains(&col) => {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(clip(&diff.target[col]))
                            .small()
                            .strikethrough()
                            .color(removed),
                    )
                    .on_hover_text("Value in target");
                    ui.label(
                        egui::RichText::new(clip(&diff.source[col]))
                            .small()
                            .color(added),
                    )
                    .on_hover_text("Value in source");
                });
            }
            DiffKind::Changed => {
                ui.label(egui::RichText::new(clip(&diff.source[col])).small());
            }
        }
    }
    ui.end_row();
}

/// Ringkasan per tabel untuk perbandingan struktur database.
fn database_structure_summary(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    db: &DatabaseStructureComparison,
    selected: Option<usize>,
    only_diff: &mut bool,
) -> Option<Option<usize>> {
    let ok = style::theme_success(ctx);
    let warn = style::theme_warning(ctx);
    let bad = style::theme_danger(ctx);
    let different = db.tables.iter().filter(|t| t.differs()).count();
    let failed = db.tables.iter().filter(|t| t.outcome.is_err()).count();
    let identical = db.tables.len().saturating_sub(different + failed);
    let mut pick = None;
    style::render_modal_card(ui, Some("Tables"), None, |ui| {
        ui.horizontal(|ui| {
            let mut summary_text = format!(
                "{} compared: {} identical, {} different, {} failed",
                fmt_count(db.tables.len() as u64),
                fmt_count(identical as u64),
                fmt_count(different as u64),
                fmt_count(failed as u64)
            );
            if !db.source_only.is_empty() || !db.target_only.is_empty() {
                summary_text.push_str(&format!(
                    " ({} only in source, {} only in target)",
                    fmt_count(db.source_only.len() as u64),
                    fmt_count(db.target_only.len() as u64)
                ));
            }
            ui.label(egui::RichText::new(summary_text).strong());
            ui.checkbox(only_diff, "Hide identical tables");
        });
        if db.cancelled {
            error_label(ui, "Stopped before all tables were compared.");
        }
        muted_label(
            ui,
            "Click a table to see its column definitions and build its sync script.",
        );
        let number = |ui: &mut egui::Ui, n: usize, color: egui::Color32| {
            let text = egui::RichText::new(fmt_count(n as u64)).small();
            ui.label(if n > 0 {
                text.color(color)
            } else {
                text.weak()
            });
        };
        egui::ScrollArea::both()
            .id_salt("compare_db_struct_scroll")
            .max_height(200.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("compare_db_struct_grid")
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for head in [
                            "Table",
                            "Result",
                            "Only in source",
                            "Only in target",
                            "Changed",
                            "Identical",
                        ] {
                            ui.label(egui::RichText::new(head).small().strong());
                        }
                        ui.end_row();

                        if ui
                            .selectable_label(selected.is_none(), ALL_TABLES)
                            .clicked()
                        {
                            pick = Some(None);
                        }
                        ui.end_row();

                        for (i, table) in db.tables.iter().enumerate() {
                            if *only_diff && !table.differs() && table.outcome.is_ok() {
                                continue;
                            }
                            let name = if table
                                .source_table
                                .eq_ignore_ascii_case(&table.target_table)
                            {
                                table.source_table.clone()
                            } else {
                                format!("{} (target: {})", table.source_table, table.target_table)
                            };
                            if ui.selectable_label(selected == Some(i), name).clicked() {
                                pick = Some(Some(i));
                            }
                            match &table.outcome {
                                Ok(outcome) => {
                                    let result = &outcome.result;
                                    let (only_source, only_target, changed) = result.counts();
                                    let (text, color) = if result.diffs.is_empty() {
                                        ("Identical", ok)
                                    } else {
                                        ("Different", warn)
                                    };
                                    ui.label(egui::RichText::new(text).small().color(color));
                                    number(ui, only_source, ok);
                                    number(ui, only_target, bad);
                                    number(ui, changed, warn);
                                    number(ui, result.identical, ui.visuals().text_color());
                                }
                                Err(e) => {
                                    ui.label(egui::RichText::new(clip(e)).small().color(bad))
                                        .on_hover_text(e);
                                }
                            }
                            ui.end_row();
                        }
                        for (name, cols) in &db.source_only {
                            ui.label(name);
                            ui.label(egui::RichText::new("Missing in target").small().color(bad));
                            number(ui, cols.len(), ok);
                            ui.label("-");
                            ui.label("-");
                            ui.label("-");
                            ui.end_row();
                        }
                        for name in &db.target_only {
                            ui.label(name);
                            ui.label(egui::RichText::new("Missing in source").small().color(bad));
                            ui.label("-");
                            ui.label("-");
                            ui.label("-");
                            ui.label("-");
                            ui.end_row();
                        }
                    });
            });
    });
    pick
}

/// Render rincian perbedaan kolom untuk satu tabel dalam mode Structure.
fn render_structure_outcome(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    outcome: &TableStructureOutcome,
    show_source_only: &mut bool,
    show_target_only: &mut bool,
    show_changed: &mut bool,
) {
    let result = &outcome.result;
    let (only_source, only_target, changed) = result.counts();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} identical columns",
                fmt_count(result.identical as u64)
            ))
            .strong(),
        );
        ui.checkbox(
            show_source_only,
            format!("Only in source ({})", fmt_count(only_source as u64)),
        );
        ui.checkbox(
            show_target_only,
            format!("Only in target ({})", fmt_count(only_target as u64)),
        );
        ui.checkbox(
            show_changed,
            format!("Changed ({})", fmt_count(changed as u64)),
        );
    });
    muted_label(
        ui,
        format!(
            "Columns compared: {} source, {} target.",
            fmt_count(result.source_columns as u64),
            fmt_count(result.target_columns as u64)
        ),
    );

    let visible: Vec<&ColumnDiff> = result
        .diffs
        .iter()
        .filter(|d| match d.kind {
            ColumnDiffKind::OnlyInSource => *show_source_only,
            ColumnDiffKind::OnlyInTarget => *show_target_only,
            ColumnDiffKind::Changed => *show_changed,
        })
        .collect();

    if visible.is_empty() {
        ui.add_space(6.0);
        ui.label(if result.diffs.is_empty() {
            "No differences found."
        } else {
            "No differences match the selected filters."
        });
    } else {
        ui.add_space(4.0);
        egui::ScrollArea::both()
            .id_salt("compare_struct_diff_scroll")
            .max_height(230.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("compare_struct_diff_grid")
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for h in [
                            "Status",
                            "Column",
                            "Source type",
                            "Target type",
                            "Nullable",
                            "Key",
                        ] {
                            ui.label(egui::RichText::new(h).small().strong());
                        }
                        ui.end_row();

                        let ok = style::theme_success(ctx);
                        let warn = style::theme_warning(ctx);
                        let bad = style::theme_danger(ctx);

                        for diff in visible {
                            let (status_text, status_color) = match diff.kind {
                                ColumnDiffKind::OnlyInSource => (diff.kind.label(), ok),
                                ColumnDiffKind::OnlyInTarget => (diff.kind.label(), bad),
                                ColumnDiffKind::Changed => (diff.kind.label(), warn),
                            };
                            ui.label(
                                egui::RichText::new(status_text)
                                    .small()
                                    .strong()
                                    .color(status_color),
                            );

                            ui.label(
                                egui::RichText::new(&diff.name)
                                    .small()
                                    .strong()
                                    .family(egui::FontFamily::Monospace),
                            );

                            let src_type = diff
                                .source
                                .as_ref()
                                .map(|c| c.data_type.as_str())
                                .unwrap_or("-");
                            let dst_type = diff
                                .target
                                .as_ref()
                                .map(|c| c.data_type.as_str())
                                .unwrap_or("-");

                            let type_changed = diff.changed.contains(&ColumnAttribute::Type);
                            let src_type_txt = egui::RichText::new(src_type)
                                .small()
                                .family(egui::FontFamily::Monospace);
                            let dst_type_txt = egui::RichText::new(dst_type)
                                .small()
                                .family(egui::FontFamily::Monospace);
                            if type_changed {
                                ui.label(src_type_txt.color(warn));
                                ui.label(dst_type_txt.color(warn));
                            } else {
                                ui.label(src_type_txt);
                                ui.label(dst_type_txt);
                            }

                            let null_changed = diff.changed.contains(&ColumnAttribute::Nullable);
                            let null_str = |opt: Option<
                                &crate::data_transfer::types::SourceColumn,
                            >| match opt {
                                Some(c) if c.nullable => "NULL",
                                Some(_) => "NOT NULL",
                                None => "-",
                            };
                            let null_val = if null_changed {
                                format!(
                                    "{} -> {}",
                                    null_str(diff.source.as_ref()),
                                    null_str(diff.target.as_ref())
                                )
                            } else {
                                null_str(diff.source.as_ref().or(diff.target.as_ref())).to_string()
                            };
                            let null_txt = egui::RichText::new(null_val).small();
                            if null_changed {
                                ui.label(null_txt.color(warn));
                            } else {
                                ui.label(null_txt);
                            }

                            let pk_changed = diff.changed.contains(&ColumnAttribute::PrimaryKey);
                            let pk_str = |opt: Option<
                                &crate::data_transfer::types::SourceColumn,
                            >| match opt {
                                Some(c) if c.primary_key => "PK",
                                Some(_) => "-",
                                None => "-",
                            };
                            let pk_val = if pk_changed {
                                format!(
                                    "{} -> {}",
                                    pk_str(diff.source.as_ref()),
                                    pk_str(diff.target.as_ref())
                                )
                            } else {
                                pk_str(diff.source.as_ref().or(diff.target.as_ref())).to_string()
                            };
                            let pk_txt = egui::RichText::new(pk_val).small();
                            if pk_changed {
                                ui.label(pk_txt.color(warn));
                            } else {
                                ui.label(pk_txt);
                            }

                            ui.end_row();
                        }
                    });
            });
    }
}

impl Tabular {
    fn load_saved_comparisons(&mut self, dialog: &mut CompareDialog) {
        dialog.saved_loaded = true;
        let Some(pool) = self.db_pool.clone() else {
            return;
        };
        match self.get_runtime().block_on(saved::list(pool.as_ref())) {
            Ok(items) => dialog.saved = items,
            Err(e) => log::warn!("[TRANSFER] loading saved comparisons failed: {}", e),
        }
    }

    fn start_compare(&mut self, ctx: &egui::Context, dialog: &mut CompareDialog) {
        dialog.error = None;
        dialog.confirm_apply = false;
        let (Some(source), Some(target)) =
            (dialog.source.endpoint(self), dialog.target.endpoint(self))
        else {
            dialog.error = Some("Connection is no longer available".to_string());
            return;
        };
        match dialog.mode {
            CompareMode::Data => {
                let opts = match dialog.options() {
                    Ok(opts) => opts,
                    Err(e) => {
                        dialog.error = Some(e);
                        return;
                    }
                };
                if dialog.all_tables() {
                    let progress = CompareProgressHandle::default();
                    let cancel = Arc::new(AtomicBool::new(false));
                    dialog.progress = progress.clone();
                    dialog.cancel = Some(cancel.clone());
                    dialog.db_slot = Some(self.spawn_slot(ctx, async move {
                        compare::compare_databases(source, target, &opts, progress, cancel).await
                    }));
                    return;
                }
                let source_table = dialog.source_table.trim().to_string();
                let target_table = dialog.target_table.trim().to_string();
                dialog.slot = Some(self.spawn_slot(ctx, async move {
                    compare::compare_tables(source, target, &source_table, &target_table, &opts)
                        .await
                }));
            }
            CompareMode::Structure => {
                let opts = dialog.structure_options();
                if dialog.all_tables() {
                    let progress = CompareProgressHandle::default();
                    let cancel = Arc::new(AtomicBool::new(false));
                    dialog.progress = progress.clone();
                    dialog.cancel = Some(cancel.clone());
                    dialog.structure_db_slot = Some(self.spawn_slot(ctx, async move {
                        compare_structure::compare_database_structure(
                            source, target, &opts, progress, cancel,
                        )
                        .await
                    }));
                    return;
                }
                let source_table = dialog.source_table.trim().to_string();
                let target_table = dialog.target_table.trim().to_string();
                dialog.structure_slot = Some(self.spawn_slot(ctx, async move {
                    compare_structure::compare_table_structure(
                        source,
                        target,
                        &source_table,
                        &target_table,
                        &opts,
                    )
                    .await
                }));
            }
        }
    }

    pub(super) fn render_compare_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.transfer_ui.compare.take() else {
            return;
        };
        if !dialog.saved_loaded {
            self.load_saved_comparisons(&mut dialog);
        }
        dialog.source.sync_tables(self, ctx);
        dialog.target.sync_tables(self, ctx);

        if let Some(slot) = &dialog.slot
            && let Some(result) = slot.take()
        {
            dialog.slot = None;
            dialog.clear_results();
            match result {
                Ok(outcome) => {
                    dialog.outcome = Some(outcome);
                    dialog.error = None;
                    dialog.setup_open = false;
                }
                Err(e) => {
                    dialog.error = Some(e);
                    dialog.setup_open = true;
                }
            }
        }
        if let Some(slot) = &dialog.db_slot
            && let Some(result) = slot.take()
        {
            dialog.db_slot = None;
            dialog.cancel = None;
            // Banding ulang setelah apply: tabel yang sedang dilihat tetap terpilih.
            let selected = dialog.db_selected;
            dialog.clear_results();
            match result {
                Ok(db) => {
                    dialog.db_selected = selected.filter(|i| *i < db.tables.len());
                    dialog.db_outcome = Some(db);
                    dialog.error = None;
                    dialog.setup_open = false;
                }
                Err(e) => {
                    dialog.error = Some(e);
                    dialog.setup_open = true;
                }
            }
        }
        if let Some(slot) = &dialog.structure_slot
            && let Some(result) = slot.take()
        {
            dialog.structure_slot = None;
            dialog.clear_results();
            match result {
                Ok(outcome) => {
                    dialog.structure_outcome = Some(outcome);
                    dialog.error = None;
                    dialog.setup_open = false;
                }
                Err(e) => {
                    dialog.error = Some(e);
                    dialog.setup_open = true;
                }
            }
        }
        if let Some(slot) = &dialog.structure_db_slot
            && let Some(result) = slot.take()
        {
            dialog.structure_db_slot = None;
            dialog.cancel = None;
            let selected = dialog.db_selected;
            dialog.clear_results();
            match result {
                Ok(db) => {
                    dialog.db_selected = selected.filter(|i| *i < db.tables.len());
                    dialog.structure_db_outcome = Some(db);
                    dialog.error = None;
                    dialog.setup_open = false;
                }
                Err(e) => {
                    dialog.error = Some(e);
                    dialog.setup_open = true;
                }
            }
        }
        let mut rerun = false;
        if let Some(slot) = &dialog.apply_slot
            && let Some(result) = slot.take()
        {
            dialog.apply_slot = None;
            dialog.apply_message = Some(match result {
                Ok(count) => {
                    rerun = true;
                    Ok(format!(
                        "Applied {} statements to the target",
                        fmt_count(count as u64)
                    ))
                }
                Err(e) => Err(format!("Apply failed: {e}")),
            });
        }

        let busy = dialog.busy();
        let problem = dialog.problem();
        dialog.refresh_script();
        let count = dialog.script.as_ref().map_or(0, |stmts| match dialog.mode {
            CompareMode::Data => stmts.len(),
            CompareMode::Structure => count_executable_statements(stmts),
        });
        let all_tables = dialog.all_tables();
        let has_result = dialog.outcome.is_some()
            || dialog.db_outcome.is_some()
            || dialog.structure_outcome.is_some()
            || dialog.structure_db_outcome.is_some();
        let has_db_outcome = dialog.db_outcome.is_some() || dialog.structure_db_outcome.is_some();
        let side = |pick: &EndpointPick| {
            pick.endpoint(self)
                .map(|e| e.label())
                .unwrap_or_else(|| "database".to_string())
        };
        let (source_label, target_label) = if has_db_outcome {
            (side(&dialog.source), side(&dialog.target))
        } else {
            (
                dialog.source_table.trim().to_string(),
                dialog.target_table.trim().to_string(),
            )
        };
        let tables_before = (dialog.source_table.clone(), dialog.target_table.clone());
        let mut select_table: Option<Option<usize>> = None;
        let mut stop = false;
        let mut run = false;
        let mut done = false;
        let mut load_saved: Option<SavedComparison> = None;
        let mut save_current = false;
        let mut remove_saved: Option<String> = None;
        let mut open_script = false;
        let mut copy_script = false;
        let mut apply = false;

        let close = modal(
            ctx,
            "transfer_compare_dialog",
            "Compare",
            egui::vec2(900.0, 720.0),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("compare_body_scroll")
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 44.0)
                    .show(ui, |ui| {
                        if has_result {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{source_label}  vs  {target_label}"
                                    ))
                                    .strong(),
                                );
                                let label = if dialog.setup_open {
                                    "Hide Setup"
                                } else {
                                    "Edit Setup"
                                };
                                if ui.button(label).clicked() {
                                    dialog.setup_open = !dialog.setup_open;
                                }
                            });
                            ui.add_space(4.0);
                        }
                        let show_setup = dialog.setup_open || !has_result;
                        ui.add_enabled_ui(!busy, |ui| {
                            if !show_setup {
                                return;
                            }
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Saved").weak().small());
                                egui::ComboBox::from_id_salt("compare_saved_combo")
                                    .selected_text(
                                        dialog
                                            .saved_selected
                                            .clone()
                                            .unwrap_or_else(|| "Load a saved comparison".to_string()),
                                    )
                                    .width(220.0)
                                    .show_ui(ui, |ui| {
                                        if dialog.saved.is_empty() {
                                            ui.label("Nothing saved yet");
                                        }
                                        for item in &dialog.saved {
                                            if ui
                                                .selectable_label(
                                                    dialog.saved_selected.as_deref()
                                                        == Some(item.name.as_str()),
                                                    &item.name,
                                                )
                                                .clicked()
                                            {
                                                load_saved = Some(item.clone());
                                            }
                                        }
                                    });
                                if let Some(name) = dialog.saved_selected.clone()
                                    && ui.button("Remove").clicked()
                                {
                                    remove_saved = Some(name);
                                }
                                ui.add_space(12.0);
                                style::render_text_field(
                                    ui,
                                    egui::TextEdit::singleline(&mut dialog.save_name)
                                        .hint_text("Name to save as"),
                                    180.0,
                                    None,
                                );
                                if ui
                                    .add_enabled(
                                        !dialog.save_name.trim().is_empty() && problem.is_none(),
                                        egui::Button::new("Save"),
                                    )
                                    .clicked()
                                {
                                    save_current = true;
                                }
                            });
                            ui.add_space(6.0);

                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Mode").weak().small());
                                if ui.selectable_label(dialog.mode == CompareMode::Data, "Data").clicked()
                                    && dialog.mode != CompareMode::Data
                                {
                                    dialog.mode = CompareMode::Data;
                                    dialog.clear_results();
                                }
                                if ui.selectable_label(dialog.mode == CompareMode::Structure, "Structure").clicked()
                                    && dialog.mode != CompareMode::Structure
                                {
                                    dialog.mode = CompareMode::Structure;
                                    dialog.clear_results();
                                }
                            });
                            ui.add_space(6.0);

                            card_row(ui, ["Source", "Target"], |ui, side| {
                                let (pick, table, salt) = if side == 0 {
                                    (&mut dialog.source, &mut dialog.source_table, "compare_source")
                                } else {
                                    (&mut dialog.target, &mut dialog.target_table, "compare_target")
                                };
                                pick.ui_with(ui, self, salt, |ui, pick, width| {
                                    table_row(ui, pick, table, salt, width);
                                });
                                if let Some(e) = &pick.tables_error {
                                    error_label(ui, e);
                                }
                            });
                            ui.add_space(6.0);

                            style::render_modal_card(ui, Some("Options"), None, |ui| {
                                if dialog.mode == CompareMode::Data {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("Key columns").weak().small());
                                        style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut dialog.key_columns)
                                                .hint_text("Primary key (or: id, tenant_id)"),
                                            250.0,
                                            None,
                                        );
                                        ui.add_space(8.0);
                                        ui.label(egui::RichText::new("Ignore columns").weak().small());
                                        style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut dialog.ignore_columns)
                                                .hint_text("e.g. updated_at"),
                                            220.0,
                                            None,
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("Row filter (WHERE)").weak().small());
                                        style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut dialog.where_clause)
                                                .hint_text("Applied to both sides"),
                                            250.0,
                                            None,
                                        );
                                        ui.add_space(8.0);
                                        ui.label(egui::RichText::new("Row limit per side").weak().small());
                                        style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut dialog.row_limit)
                                                .hint_text("All"),
                                            110.0,
                                            None,
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut dialog.tolerant, "Ignore number/boolean/timestamp formatting")
                                            .on_hover_text(
                                                "Treats 1.0 and 1, t and 1, and timestamps with or \
                                                 without fractional zeros as equal",
                                            );
                                        ui.checkbox(&mut dialog.trim, "Ignore trailing spaces");
                                        ui.checkbox(&mut dialog.case_insensitive, "Ignore case");
                                    });
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("Ignore columns").weak().small());
                                        style::render_text_field(
                                            ui,
                                            egui::TextEdit::singleline(&mut dialog.ignore_columns)
                                                .hint_text("e.g. updated_at, temp_id"),
                                            260.0,
                                            None,
                                        );
                                        ui.add_space(8.0);
                                        ui.checkbox(&mut dialog.case_insensitive, "Ignore column name case");
                                        ui.checkbox(&mut dialog.compare_nullability, "Compare nullability");
                                    });
                                }
                            });
                        });

                        if dialog.slot.is_some() || dialog.structure_slot.is_some() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label(if dialog.structure_slot.is_some() {
                                    "Reading table structure..."
                                } else {
                                    "Reading both tables..."
                                });
                            });
                        }
                        if dialog.db_slot.is_some() || dialog.structure_db_slot.is_some() {
                            let progress = dialog
                                .progress
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .clone();
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label(if progress.tables_total == 0 {
                                    "Listing tables...".to_string()
                                } else {
                                    format!(
                                        "Compared {} of {} tables  (now: {})",
                                        progress.tables_done,
                                        progress.tables_total,
                                        progress.current_table
                                    )
                                });
                                if ui.button("Stop").clicked() {
                                    stop = true;
                                }
                            });
                        }
                        if let Some(e) = &dialog.error {
                            ui.add_space(6.0);
                            error_label(ui, e);
                        }

                        if let Some(db) = &dialog.db_outcome {
                            ui.add_space(8.0);
                            select_table = database_summary(
                                ui,
                                ctx,
                                db,
                                dialog.db_selected,
                                &mut dialog.db_only_diff,
                            );
                        }
                        if let Some(db) = &dialog.structure_db_outcome {
                            ui.add_space(8.0);
                            select_table = database_structure_summary(
                                ui,
                                ctx,
                                db,
                                dialog.db_selected,
                                &mut dialog.db_only_diff,
                            );
                        }
                        let active: Option<&CompareOutcome> =
                            dialog.outcome.as_ref().or_else(|| {
                                let db = dialog.db_outcome.as_ref()?;
                                db.tables.get(dialog.db_selected?)?.outcome.as_ref().ok()
                            });
                        let active_struct: Option<&TableStructureOutcome> =
                            dialog.structure_outcome.as_ref().or_else(|| {
                                let db = dialog.structure_db_outcome.as_ref()?;
                                db.tables.get(dialog.db_selected?)?.outcome.as_ref().ok()
                            });
                        if let Some(outcome) = active {
                            let result = &outcome.result;
                            let (only_source, only_target, changed) = result.counts();
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} identical",
                                        fmt_count(result.identical as u64)
                                    ))
                                    .strong(),
                                );
                                ui.checkbox(
                                    &mut dialog.show_source_only,
                                    format!("Only in source ({})", fmt_count(only_source as u64)),
                                );
                                ui.checkbox(
                                    &mut dialog.show_target_only,
                                    format!("Only in target ({})", fmt_count(only_target as u64)),
                                );
                                ui.checkbox(
                                    &mut dialog.show_changed,
                                    format!("Changed ({})", fmt_count(changed as u64)),
                                );
                            });
                            muted_label(
                                ui,
                                format!(
                                    "Matched on: {}. Rows read: {} source, {} target.",
                                    outcome.key_columns.join(", "),
                                    fmt_count(result.source_rows as u64),
                                    fmt_count(result.target_rows as u64)
                                ),
                            );
                            if result.truncated {
                                error_label(
                                    ui,
                                    "Row limit reached: only part of the data was compared, so \
                                     rows may be reported as missing.",
                                );
                            }
                            if result.duplicate_keys > 0 {
                                error_label(
                                    ui,
                                    &format!(
                                        "{} rows share a key with another row and were skipped; \
                                         choose key columns that identify rows uniquely.",
                                        fmt_count(result.duplicate_keys as u64)
                                    ),
                                );
                            }
                            if !result.source_only_columns.is_empty() {
                                muted_label(
                                    ui,
                                    format!(
                                        "Columns only in source (not compared): {}",
                                        result.source_only_columns.join(", ")
                                    ),
                                );
                            }
                            if !result.target_only_columns.is_empty() {
                                muted_label(
                                    ui,
                                    format!(
                                        "Columns only in target (not compared): {}",
                                        result.target_only_columns.join(", ")
                                    ),
                                );
                            }

                            let visible: Vec<&RowDiff> = result
                                .diffs
                                .iter()
                                .filter(|d| match d.kind {
                                    DiffKind::OnlyInSource => dialog.show_source_only,
                                    DiffKind::OnlyInTarget => dialog.show_target_only,
                                    DiffKind::Changed => dialog.show_changed,
                                })
                                .collect();
                            if visible.is_empty() {
                                ui.add_space(6.0);
                                ui.label(if result.diffs.is_empty() {
                                    "No differences found."
                                } else {
                                    "No differences match the selected filters."
                                });
                            } else {
                                ui.add_space(4.0);
                                egui::ScrollArea::both()
                                    .id_salt("compare_diff_scroll")
                                    .max_height(230.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        egui::Grid::new("compare_diff_grid")
                                            .striped(true)
                                            .spacing([14.0, 4.0])
                                            .show(ui, |ui| {
                                                ui.label(egui::RichText::new("Status").small().strong());
                                                for (i, column) in result.columns.iter().enumerate() {
                                                    let text = if result.key_indices.contains(&i) {
                                                        format!("{column} (key)")
                                                    } else {
                                                        column.clone()
                                                    };
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .small()
                                                            .strong()
                                                            .family(egui::FontFamily::Monospace),
                                                    );
                                                }
                                                ui.end_row();
                                                for diff in visible.iter().take(MAX_ROWS_SHOWN) {
                                                    diff_row(ui, ctx, diff, result.columns.len());
                                                }
                                            });
                                    });
                                if visible.len() > MAX_ROWS_SHOWN {
                                    muted_label(
                                        ui,
                                        format!(
                                            "Showing the first {} of {} differences; the script \
                                             covers all of them.",
                                            MAX_ROWS_SHOWN,
                                            fmt_count(visible.len() as u64)
                                        ),
                                    );
                                }
                            }
                        }
                        if let Some(outcome) = active_struct {
                            render_structure_outcome(
                                ui,
                                ctx,
                                outcome,
                                &mut dialog.show_struct_source_only,
                                &mut dialog.show_struct_target_only,
                                &mut dialog.show_struct_changed,
                            );
                        }
                        if has_result {
                            ui.add_space(8.0);
                            style::render_modal_card(
                                ui,
                                Some("Sync script (make target match source)"),
                                None,
                                |ui| {
                                    if dialog.mode == CompareMode::Data {
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut dialog.parts.insert_missing, "Insert missing rows");
                                            ui.checkbox(&mut dialog.parts.update_changed, "Update changed rows");
                                            ui.checkbox(
                                                &mut dialog.parts.delete_extra,
                                                "Delete rows only in target",
                                            );
                                        });
                                    } else {
                                        ui.horizontal(|ui| {
                                            ui.checkbox(
                                                &mut dialog.structure_parts.add_missing_columns,
                                                "Add missing columns",
                                            );
                                            ui.checkbox(
                                                &mut dialog.structure_parts.alter_changed_columns,
                                                "Alter changed columns",
                                            );
                                            ui.checkbox(
                                                &mut dialog.structure_parts.drop_extra_columns,
                                                "Drop columns only in target",
                                            );
                                            if dialog.structure_db_outcome.is_some() {
                                                ui.checkbox(
                                                    &mut dialog.structure_parts.create_missing_tables,
                                                    "Create missing tables",
                                                );
                                            }
                                        });
                                    }
                                    if active.is_none()
                                        && active_struct.is_none()
                                        && (dialog.db_outcome.is_some()
                                            || dialog.structure_db_outcome.is_some())
                                    {
                                        muted_label(
                                            ui,
                                            if dialog.mode == CompareMode::Data {
                                                "Covers every compared table, in table name order; \
                                                 foreign keys may need a different order."
                                            } else {
                                                "Covers every compared table; table creation and alterations \
                                                 are generated per target dialect."
                                            },
                                        );
                                    }
                                    ui.horizontal(|ui| {
                                        ui.add_enabled_ui(count > 0 && !busy, |ui| {
                                            if ui.button("Open Script in Editor").clicked() {
                                                open_script = true;
                                            }
                                            if ui.button("Copy Script").clicked() {
                                                copy_script = true;
                                            }
                                            if !dialog.confirm_apply
                                                && ui.button("Apply to Target...").clicked()
                                            {
                                                dialog.confirm_apply = true;
                                            }
                                        });
                                        muted_label(
                                            ui,
                                            format!("{} statements", fmt_count(count as u64)),
                                        );
                                    });
                                    if dialog.confirm_apply && count > 0 {
                                        let target_name = if let Some(o) = active {
                                            o.target_sql.as_str()
                                        } else if let Some(o) = active_struct {
                                            o.target_table.as_str()
                                        } else {
                                            target_label.as_str()
                                        };
                                        error_label(
                                            ui,
                                            &format!(
                                                "This runs {} statements on {} and cannot be undone.",
                                                fmt_count(count as u64),
                                                target_name,
                                            ),
                                        );
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add_enabled(
                                                    !busy,
                                                    style::btn_danger_ctx(
                                                        ctx,
                                                        format!("Run {} Statements", fmt_count(count as u64)),
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                apply = true;
                                            }
                                            if ui.button("Cancel").clicked() {
                                                dialog.confirm_apply = false;
                                            }
                                        });
                                    }
                                    if dialog.apply_slot.is_some() {
                                        ui.horizontal(|ui| {
                                            ui.add(egui::Spinner::new());
                                            ui.label("Applying to target...");
                                        });
                                    }
                                    match &dialog.apply_message {
                                        Some(Ok(message)) => {
                                            ui.label(
                                                egui::RichText::new(message)
                                                    .small()
                                                    .color(style::theme_success(ctx)),
                                            );
                                        }
                                        Some(Err(message)) => error_label(ui, message),
                                        None => {}
                                    }
                                },
                            );
                        }
                    });

                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let compare_btn_label = match (dialog.mode, all_tables) {
                        (CompareMode::Structure, true) => "Compare All Tables",
                        (CompareMode::Structure, false) => "Compare Structure",
                        (CompareMode::Data, true) => "Compare All Tables",
                        (CompareMode::Data, false) => "Compare",
                    };
                    if ui
                        .add_enabled(
                            problem.is_none() && !busy,
                            style::btn_primary_ctx(ctx, compare_btn_label),
                        )
                        .clicked()
                    {
                        run = true;
                    }
                    if ui.add(style::btn_secondary("Close")).clicked() {
                        done = true;
                    }
                    if let Some(problem) = &problem {
                        muted_label(ui, problem.clone());
                    }
                });
            },
        );
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }

        if stop && let Some(cancel) = &dialog.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(selection) = select_table {
            dialog.db_selected = selection;
            dialog.confirm_apply = false;
        }
        // Memilih tabel (atau "All tables") di satu sisi ikut mengisi sisi lain
        // selama sisi itu masih mengikuti: kasus umumnya nama tabelnya sama.
        if dialog.source_table != tables_before.0
            && (dialog.source_table.is_empty()
                || dialog.target_table.is_empty()
                || dialog.target_table == tables_before.0)
        {
            dialog.target_table = dialog.source_table.clone();
        } else if dialog.target_table != tables_before.1 && dialog.target_table.is_empty() {
            dialog.source_table.clear();
        }

        if let Some(item) = load_saved {
            let missing = [item.source.connection_id, item.target.connection_id]
                .iter()
                .any(|id| !self.connections.iter().any(|c| c.id == Some(*id)));
            dialog.apply_saved(&item);
            if missing {
                dialog.error = Some(
                    "A connection used by this saved comparison no longer exists; choose it again"
                        .to_string(),
                );
            }
        }
        if save_current
            && let Some(item) = dialog.to_saved()
            && let Some(pool) = self.db_pool.clone()
        {
            match self
                .get_runtime()
                .block_on(saved::save(pool.as_ref(), &item))
            {
                Ok(()) => {
                    dialog.saved_selected = Some(item.name.clone());
                    self.toasts
                        .success(format!("Saved comparison \"{}\"", item.name));
                    self.load_saved_comparisons(&mut dialog);
                }
                Err(e) => dialog.error = Some(format!("Could not save: {e}")),
            }
        }
        if let Some(name) = remove_saved
            && let Some(pool) = self.db_pool.clone()
        {
            match self
                .get_runtime()
                .block_on(saved::delete(pool.as_ref(), &name))
            {
                Ok(()) => {
                    dialog.saved_selected = None;
                    self.load_saved_comparisons(&mut dialog);
                }
                Err(e) => dialog.error = Some(format!("Could not remove: {e}")),
            }
        }
        if copy_script && let Some(statements) = &dialog.script {
            ctx.copy_text(statements.join("\n"));
            self.toasts.info("Sync script copied to clipboard");
        }
        if apply
            && let Some(statements) = dialog.script.clone()
            && let Some(target) = dialog.target.endpoint(self)
        {
            dialog.confirm_apply = false;
            dialog.apply_message = None;
            dialog.apply_slot = Some(self.spawn_slot(ctx, async move {
                let target = target.connect().await?;
                let exec_statements: Vec<String> = statements
                    .into_iter()
                    .filter(|s| !s.trim_start().starts_with("--"))
                    .collect();
                target.execute(&exec_statements).await?;
                Ok(exec_statements.len())
            }));
        }
        if open_script
            && let Some(statements) = &dialog.script
            && let Some(conn_id) = dialog.target.conn_id
        {
            let database = dialog.target.database.trim();
            let subject = match (
                &dialog.db_outcome,
                &dialog.structure_db_outcome,
                dialog.db_selected,
            ) {
                (Some(db), _, Some(i)) => db.tables.get(i).map(|t| t.target_table.clone()),
                (_, Some(db), Some(i)) => db.tables.get(i).map(|t| t.target_table.clone()),
                (Some(_), _, None) | (_, Some(_), None) => Some(target_label.clone()),
                (None, None, _) => None,
            }
            .unwrap_or_else(|| dialog.target_table.trim().to_string());
            let title = match dialog.mode {
                CompareMode::Data => format!("Sync {subject}"),
                CompareMode::Structure => format!("Sync Structure {subject}"),
            };
            crate::editor::create_new_tab_with_connection_and_database(
                self,
                title,
                statements.join("\n"),
                Some(conn_id),
                (!database.is_empty()).then(|| database.to_string()),
            );
            // Skrip ditinjau di editor; dialog ditutup supaya tab terlihat.
            return;
        }
        if run || rerun {
            self.start_compare(ctx, &mut dialog);
        }
        if busy || !(close || done) {
            self.transfer_ui.compare = Some(dialog);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_parsed_from_text_fields() {
        let mut dialog = CompareDialog::new(Some(1), Some("shop".into()), Some("orders".into()));
        dialog.key_columns = " id , tenant_id ,".to_string();
        dialog.ignore_columns = "updated_at".to_string();
        dialog.where_clause = "  id > 5 ".to_string();
        dialog.row_limit = "2,000".to_string();
        let opts = dialog.options().unwrap();
        assert_eq!(opts.key_columns, vec!["id", "tenant_id"]);
        assert_eq!(opts.ignore_columns, vec!["updated_at"]);
        assert_eq!(opts.where_clause.as_deref(), Some("id > 5"));
        assert_eq!(opts.row_limit, Some(2000));

        dialog.row_limit = "none".to_string();
        assert!(dialog.options().is_err());
        dialog.row_limit.clear();
        assert_eq!(dialog.options().unwrap().row_limit, None);
    }

    #[test]
    fn saved_comparison_roundtrips_through_dialog() {
        let mut dialog = CompareDialog::new(Some(1), Some("shop".into()), Some("orders".into()));
        assert!(dialog.problem().is_some());
        dialog.target = EndpointPick::new(Some(2), None);
        dialog.key_columns = "id".to_string();
        dialog.save_name = " nightly ".to_string();
        assert_eq!(dialog.problem(), None);
        let saved = dialog.to_saved().unwrap();
        assert_eq!(saved.name, "nightly");
        assert_eq!(saved.source.database.as_deref(), Some("shop"));
        assert_eq!(saved.target.database, None);

        let mut other = CompareDialog::new(None, None, None);
        other.apply_saved(&saved);
        assert_eq!(other.source.conn_id, Some(1));
        assert_eq!(other.target.conn_id, Some(2));
        assert_eq!(other.source_table, "orders");
        assert_eq!(other.key_columns, "id");
        assert_eq!(other.saved_selected.as_deref(), Some("nightly"));
    }

    #[test]
    fn structure_mode_saved_roundtrips_through_dialog() {
        let mut dialog = CompareDialog::new(Some(1), Some("shop".into()), Some("orders".into()));
        dialog.mode = CompareMode::Structure;
        dialog.target = EndpointPick::new(Some(2), None);
        dialog.ignore_columns = "created_at, updated_at".to_string();
        dialog.case_insensitive = true;
        dialog.compare_nullability = false;
        dialog.save_name = "struct_sync".to_string();
        // Mode structure does not require key_columns.
        assert_eq!(dialog.problem(), None);

        let saved = dialog.to_saved().unwrap();
        assert_eq!(saved.name, "struct_sync");
        assert_eq!(saved.mode, CompareMode::Structure);
        assert_eq!(
            saved.structure_options.ignore_columns,
            vec!["created_at", "updated_at"]
        );
        assert!(saved.structure_options.case_insensitive_names);
        assert!(!saved.structure_options.compare_nullability);

        let mut other = CompareDialog::new(None, None, None);
        other.apply_saved(&saved);
        assert_eq!(other.mode, CompareMode::Structure);
        assert_eq!(other.source.conn_id, Some(1));
        assert_eq!(other.target.conn_id, Some(2));
        assert_eq!(other.ignore_columns, "created_at, updated_at");
        assert!(other.case_insensitive);
        assert!(!other.compare_nullability);
        assert_eq!(other.saved_selected.as_deref(), Some("struct_sync"));
    }

    #[test]
    fn long_cells_are_clipped_on_char_boundaries() {
        assert_eq!(clip("a\nb"), "a b");
        assert_eq!(clip(""), "(empty)");
        let long = "é".repeat(100);
        let clipped = clip(&long);
        assert!(clipped.ends_with("..."));
        assert_eq!(clipped.chars().count(), MAX_CELL_CHARS + 3);
    }
}
