//! Dialog "Transfer To" (salin tabel antar koneksi, juga lintas engine) dan
//! "Export Objects as SQL" (pohon objek + opsi dump). Dibuka lewat
//! `transfer_ui::TransferAction`; logikanya di `crate::data_transfer`.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use super::transfer_ui::{
    EndpointPick, Slot, card_row, error_label, fmt_count, modal, muted_label, passphrase_fields,
};
use super::{Tabular, style};
use crate::data_transfer::formats;
use crate::data_transfer::object_export::{self, ObjectKind, ObjectRef, SqlExportOptions};
use crate::data_transfer::transfer::{
    self, ProgressHandle, TableTransfer, TransferOptions, TransferProgress, TransferSummary,
};
use crate::data_transfer::values::InsertLimits;
use crate::rfd;

fn new_progress() -> ProgressHandle {
    Arc::new(Mutex::new(TransferProgress::default()))
}

fn snapshot(progress: &ProgressHandle) -> TransferProgress {
    progress
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Angka opsional dari kolom teks; kosong = tanpa batas.
fn parse_limit(text: &str) -> Result<Option<u64>, String> {
    let trimmed = text.trim().replace([',', '_'], "");
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .map(Some)
        .ok_or_else(|| "Row limit must be a positive number".to_string())
}

/// Progres ringkas + beberapa baris log terakhir.
fn progress_panel(ui: &mut egui::Ui, progress: &TransferProgress, running: bool, unit: &str) {
    ui.horizontal(|ui| {
        if running {
            ui.add(egui::Spinner::new());
        }
        let mut line = format!(
            "{} of {} {} done, {} rows",
            progress.tables_done,
            progress.tables_total,
            unit,
            fmt_count(progress.rows_copied)
        );
        if running && !progress.current_table.is_empty() {
            line.push_str(&format!("  (now: {})", progress.current_table));
        }
        ui.label(line);
    });
    if !progress.log.is_empty() {
        egui::ScrollArea::vertical()
            .id_salt(("transfer_log", unit))
            .max_height(84.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in progress.log.iter().rev().take(60).rev() {
                    ui.label(
                        egui::RichText::new(line)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                }
            });
    }
}

// ─── Transfer To ────────────────────────────────────────────────────────────

pub(super) struct TransferDialog {
    source: EndpointPick,
    target: EndpointPick,
    selected: BTreeSet<String>,
    preselect: Option<String>,
    filter: String,
    target_table: String,
    create_table: bool,
    clear_target: bool,
    where_clause: String,
    row_limit: String,
    continue_on_error: bool,
    progress: Option<ProgressHandle>,
    cancel: Option<Arc<AtomicBool>>,
    slot: Option<Slot<Result<TransferSummary, String>>>,
    outcome: Option<Result<TransferSummary, String>>,
    error: Option<String>,
}

impl TransferDialog {
    pub(super) fn new(
        conn_id: Option<i64>,
        database: Option<String>,
        table: Option<String>,
    ) -> Self {
        Self {
            source: EndpointPick::new(conn_id, database),
            target: EndpointPick::default(),
            selected: BTreeSet::new(),
            target_table: table.clone().unwrap_or_default(),
            preselect: table,
            filter: String::new(),
            create_table: true,
            clear_target: false,
            where_clause: String::new(),
            row_limit: String::new(),
            continue_on_error: true,
            progress: None,
            cancel: None,
            slot: None,
            outcome: None,
            error: None,
        }
    }

    fn running(&self) -> bool {
        self.slot.is_some()
    }

    /// Pesan bila transfer belum bisa dimulai.
    fn problem(&self) -> Option<String> {
        if self.source.conn_id.is_none() {
            return Some("Choose a source connection".to_string());
        }
        if self.target.conn_id.is_none() {
            return Some("Choose a target connection".to_string());
        }
        if self.selected.is_empty() {
            return Some("Select at least one table".to_string());
        }
        let same_place = self.source.conn_id == self.target.conn_id
            && self.source.database.trim() == self.target.database.trim();
        if same_place {
            let renamed = self.selected.len() == 1
                && !self.target_table.trim().is_empty()
                && self.selected.first().map(String::as_str) != Some(self.target_table.trim());
            if !renamed {
                return Some(
                    "Source and target are the same; pick another target or table name".to_string(),
                );
            }
        }
        parse_limit(&self.row_limit).err()
    }
}

impl Tabular {
    pub(super) fn render_transfer_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.transfer_ui.transfer.take() else {
            return;
        };
        dialog.source.sync_tables(self, ctx);
        if let Some(wanted) = dialog.preselect.clone()
            && !dialog.source.loading()
            && (!dialog.source.tables.is_empty() || dialog.source.tables_error.is_some())
        {
            // Nama dari sidebar bisa berbeda bentuk (`dbo.t` vs `[dbo].[t]`);
            // pakai nama dari daftar bila ada yang cocok.
            let found = dialog
                .source
                .tables
                .iter()
                .find(|t| t.eq_ignore_ascii_case(&wanted) || t.ends_with(&format!(".{wanted}")))
                .cloned()
                .unwrap_or(wanted);
            dialog.selected.insert(found);
            dialog.preselect = None;
        }
        if let Some(slot) = &dialog.slot
            && let Some(result) = slot.take()
        {
            dialog.slot = None;
            match &result {
                Ok(summary) => {
                    let message = format!(
                        "Transferred {} rows into {} table(s)",
                        fmt_count(summary.rows_copied),
                        summary.tables_copied
                    );
                    if summary.failed.is_empty() {
                        self.toasts.success(message);
                    } else {
                        self.toasts
                            .warning(format!("{message}; {} failed", summary.failed.len()));
                    }
                }
                Err(e) => self.toasts.error(format!("Transfer failed: {e}")),
            }
            if let Some(conn_id) = dialog.target.conn_id {
                let database = dialog.target.database.trim().to_string();
                self.refresh_after_schema_change(
                    conn_id,
                    Some(if database.is_empty() {
                        "main"
                    } else {
                        &database
                    }),
                );
            }
            // Tabel baru di tujuan bisa jadi sumber transfer berikutnya.
            dialog.source.invalidate_tables();
            dialog.outcome = Some(result);
        }

        let running = dialog.running();
        let problem = dialog.problem();
        let mut start = false;
        let mut stop = false;
        let mut done = false;
        let close = modal(
            ctx,
            "transfer_tables_dialog",
            "Transfer Tables",
            egui::vec2(760.0, 680.0),
            |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    card_row(ui, ["Source", "Target"], |ui, side| {
                        if side == 0 {
                            if dialog.source.ui(ui, self, "transfer_source") {
                                dialog.selected.clear();
                            }
                        } else {
                            dialog.target.ui(ui, self, "transfer_target");
                        }
                    });
                    ui.add_space(6.0);

                    style::render_modal_card(ui, Some("Tables"), None, |ui| {
                        ui.horizontal(|ui| {
                            style::render_search_field(
                                ui,
                                &mut dialog.filter,
                                "Filter tables",
                                220.0,
                            );
                            if ui.button("All").clicked() {
                                dialog.selected = dialog.source.tables.iter().cloned().collect();
                            }
                            if ui.button("None").clicked() {
                                dialog.selected.clear();
                            }
                            muted_label(
                                ui,
                                format!(
                                    "{} of {} selected",
                                    dialog.selected.len(),
                                    dialog.source.tables.len()
                                ),
                            );
                        });
                        if dialog.source.loading() {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label("Loading tables...");
                            });
                        }
                        if let Some(e) = &dialog.source.tables_error {
                            error_label(ui, e);
                        }
                        let filter = dialog.filter.to_lowercase();
                        egui::ScrollArea::vertical()
                            .id_salt("transfer_table_list")
                            .max_height(150.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for table in &dialog.source.tables {
                                    if !filter.is_empty() && !table.to_lowercase().contains(&filter)
                                    {
                                        continue;
                                    }
                                    let mut on = dialog.selected.contains(table);
                                    if ui.checkbox(&mut on, table).changed() {
                                        if on {
                                            dialog.selected.insert(table.clone());
                                        } else {
                                            dialog.selected.remove(table);
                                        }
                                    }
                                }
                            });
                        if dialog.selected.len() == 1 {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Target table name").weak().small());
                                style::render_text_field(
                                    ui,
                                    egui::TextEdit::singleline(&mut dialog.target_table)
                                        .hint_text("Same as source"),
                                    240.0,
                                    None,
                                );
                            });
                        }
                    });
                    ui.add_space(6.0);

                    style::render_modal_card(ui, Some("Options"), None, |ui| {
                        ui.checkbox(
                            &mut dialog.create_table,
                            "Create target table if it does not exist",
                        );
                        ui.checkbox(
                            &mut dialog.clear_target,
                            "Delete all rows in the target table first",
                        );
                        if dialog.clear_target {
                            error_label(
                                ui,
                                "Existing rows in every selected target table will be removed.",
                            );
                        }
                        ui.checkbox(
                            &mut dialog.continue_on_error,
                            "Continue with the next table when one fails",
                        );
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Row filter (WHERE)").weak().small());
                            style::render_text_field(
                                ui,
                                egui::TextEdit::singleline(&mut dialog.where_clause)
                                    .hint_text("e.g. created_at >= '2026-01-01'"),
                                260.0,
                                None,
                            );
                            ui.label(egui::RichText::new("Row limit").weak().small());
                            style::render_text_field(
                                ui,
                                egui::TextEdit::singleline(&mut dialog.row_limit).hint_text("All"),
                                90.0,
                                None,
                            );
                        });
                        let (src, dst) = (dialog.source.db_type(self), dialog.target.db_type(self));
                        if let (Some(src), Some(dst)) = (src, dst)
                            && src != dst
                        {
                            muted_label(
                                ui,
                                format!(
                                    "Column types are approximated from {} to {}. Defaults, \
                                     auto-increment, indexes and foreign keys are not copied.",
                                    src.as_db_str(),
                                    dst.as_db_str()
                                ),
                            );
                        }
                    });
                });

                ui.add_space(6.0);
                if let Some(progress) = &dialog.progress {
                    progress_panel(ui, &snapshot(progress), running, "tables");
                }
                if let Some(Ok(summary)) = &dialog.outcome {
                    for (table, error) in &summary.failed {
                        error_label(ui, &format!("{table}: {error}"));
                    }
                }
                if let Some(Err(e)) = &dialog.outcome {
                    error_label(ui, e);
                }
                if let Some(e) = &dialog.error {
                    error_label(ui, e);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if ui.add(style::btn_danger_ctx(ctx, "Stop")).clicked() {
                            stop = true;
                        }
                    } else {
                        let label = if dialog.clear_target {
                            "Clear Target and Transfer"
                        } else {
                            "Start Transfer"
                        };
                        if ui
                            .add_enabled(problem.is_none(), style::btn_primary_ctx(ctx, label))
                            .clicked()
                        {
                            start = true;
                        }
                        if ui.add(style::btn_secondary("Close")).clicked() {
                            done = true;
                        }
                        if let Some(problem) = &problem {
                            muted_label(ui, problem.clone());
                        }
                    }
                });
            },
        );
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if stop && let Some(cancel) = &dialog.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if start {
            self.start_table_transfer(ctx, &mut dialog);
        }
        // Dialog tidak ditutup selama transfer berjalan: progresnya hanya ada di sini.
        if running || !(close || done) {
            self.transfer_ui.transfer = Some(dialog);
        }
    }

    fn start_table_transfer(&mut self, ctx: &egui::Context, dialog: &mut TransferDialog) {
        dialog.error = None;
        dialog.outcome = None;
        let (Some(source), Some(target)) =
            (dialog.source.endpoint(self), dialog.target.endpoint(self))
        else {
            dialog.error = Some("Connection is no longer available".to_string());
            return;
        };
        let row_limit = match parse_limit(&dialog.row_limit) {
            Ok(limit) => limit,
            Err(e) => {
                dialog.error = Some(e);
                return;
            }
        };
        let rename = dialog.target_table.trim();
        let tables: Vec<TableTransfer> = dialog
            .selected
            .iter()
            .map(|table| TableTransfer {
                source_table: table.clone(),
                target_table: if dialog.selected.len() == 1 && !rename.is_empty() {
                    rename.to_string()
                } else {
                    table.clone()
                },
            })
            .collect();
        let where_clause = dialog.where_clause.trim();
        let opts = TransferOptions {
            create_table: dialog.create_table,
            clear_target: dialog.clear_target,
            where_clause: (!where_clause.is_empty()).then(|| where_clause.to_string()),
            row_limit,
            continue_on_error: dialog.continue_on_error,
            ..Default::default()
        };
        let progress = new_progress();
        let cancel = Arc::new(AtomicBool::new(false));
        dialog.progress = Some(progress.clone());
        dialog.cancel = Some(cancel.clone());
        dialog.slot = Some(self.spawn_slot(
            ctx,
            transfer::transfer_tables(source, target, tables, opts, progress, cancel),
        ));
    }
}

// ─── Export Objects as SQL ──────────────────────────────────────────────────

pub(super) struct ObjectExportDialog {
    source: EndpointPick,
    objects: Vec<ObjectRef>,
    objects_key: Option<(i64, String)>,
    objects_slot: Option<Slot<Result<Vec<ObjectRef>, String>>>,
    objects_error: Option<String>,
    selected: HashSet<ObjectRef>,
    preselect: Option<(ObjectKind, String)>,
    filter: String,
    structure: bool,
    data: bool,
    drop_if_exists: bool,
    indexes_post_data: bool,
    max_insert_kb: u32,
    max_insert_rows: u32,
    row_limit: String,
    encrypt: bool,
    pass1: String,
    pass2: String,
    target: Option<PathBuf>,
    progress: Option<ProgressHandle>,
    cancel: Option<Arc<AtomicBool>>,
    slot: Option<Slot<Result<String, String>>>,
    error: Option<String>,
}

impl ObjectExportDialog {
    pub(super) fn new(
        conn_id: Option<i64>,
        database: Option<String>,
        preselect: Option<(ObjectKind, String)>,
    ) -> Self {
        Self {
            source: EndpointPick::new(conn_id, database),
            objects: Vec::new(),
            objects_key: None,
            objects_slot: None,
            objects_error: None,
            selected: HashSet::new(),
            preselect,
            filter: String::new(),
            structure: true,
            data: true,
            drop_if_exists: false,
            indexes_post_data: true,
            max_insert_kb: 1024,
            max_insert_rows: 500,
            row_limit: String::new(),
            encrypt: false,
            pass1: String::new(),
            pass2: String::new(),
            target: None,
            progress: None,
            cancel: None,
            slot: None,
            error: None,
        }
    }

    fn of_kind(&self, kind: ObjectKind) -> impl Iterator<Item = &ObjectRef> {
        self.objects.iter().filter(move |o| o.kind == kind)
    }
}

impl Tabular {
    fn sync_export_objects(&mut self, ctx: &egui::Context, dialog: &mut ObjectExportDialog) {
        if let Some(slot) = &dialog.objects_slot
            && let Some(result) = slot.take()
        {
            dialog.objects_slot = None;
            match result {
                Ok(objects) => {
                    dialog.objects = objects;
                    dialog.objects_error = None;
                    dialog.selected = match dialog.preselect.take() {
                        // Dibuka dari satu tabel: hanya tabel itu yang dicentang.
                        Some((kind, name)) => dialog
                            .objects
                            .iter()
                            .filter(|o| {
                                o.kind == kind
                                    && (o.name.eq_ignore_ascii_case(&name)
                                        || o.name.ends_with(&format!(".{name}")))
                            })
                            .cloned()
                            .collect(),
                        // Dibuka dari database: semua kecuali privilege.
                        None => dialog
                            .objects
                            .iter()
                            .filter(|o| o.kind != ObjectKind::Privileges)
                            .cloned()
                            .collect(),
                    };
                }
                Err(e) => {
                    dialog.objects.clear();
                    dialog.objects_error = Some(e);
                }
            }
        }
        let Some(conn_id) = dialog.source.conn_id else {
            return;
        };
        let key = (conn_id, dialog.source.database.trim().to_string());
        if dialog.objects_slot.is_some() || dialog.objects_key.as_ref() == Some(&key) {
            return;
        }
        dialog.objects_key = Some(key);
        dialog.objects.clear();
        dialog.selected.clear();
        if let Some(endpoint) = dialog.source.endpoint(self) {
            dialog.objects_slot = Some(self.spawn_slot(ctx, async move {
                let endpoint = endpoint.connect().await?;
                object_export::list_objects(&endpoint).await
            }));
        }
    }

    pub(super) fn render_object_export_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.transfer_ui.objects.take() else {
            return;
        };
        self.sync_export_objects(ctx, &mut dialog);

        if let Some(slot) = &dialog.slot
            && let Some(result) = slot.take()
        {
            dialog.slot = None;
            let written = result.and_then(|sql| {
                let target = dialog
                    .target
                    .clone()
                    .ok_or_else(|| "No target file".to_string())?;
                formats::write_bytes(
                    &target,
                    sql.into_bytes(),
                    dialog.encrypt.then_some(dialog.pass1.as_str()),
                )
            });
            match written {
                Ok((path, bytes)) => {
                    self.toasts.success(format!(
                        "Exported SQL to {} ({} bytes)",
                        path.display(),
                        fmt_count(bytes)
                    ));
                    return;
                }
                Err(e) => {
                    log::warn!("[TRANSFER] object export failed: {}", e);
                    dialog.error = Some(e);
                }
            }
        }

        let running = dialog.slot.is_some();
        let limit_error = parse_limit(&dialog.row_limit).err();
        let mut start = false;
        let mut stop = false;
        let mut done = false;
        let close = modal(
            ctx,
            "transfer_object_export_dialog",
            "Export Objects as SQL",
            egui::vec2(720.0, 680.0),
            |ui| {
                let mut pass_problem = None;
                ui.add_enabled_ui(!running, |ui| {
                    style::render_modal_card(ui, Some("Source"), None, |ui| {
                        dialog.source.ui(ui, self, "object_export_source");
                    });
                    ui.add_space(6.0);

                    style::render_modal_card(ui, Some("Objects"), None, |ui| {
                        ui.horizontal(|ui| {
                            style::render_search_field(
                                ui,
                                &mut dialog.filter,
                                "Filter objects",
                                220.0,
                            );
                            if ui.button("All").clicked() {
                                dialog.selected = dialog.objects.iter().cloned().collect();
                            }
                            if ui.button("None").clicked() {
                                dialog.selected.clear();
                            }
                            muted_label(
                                ui,
                                format!(
                                    "{} of {} selected",
                                    dialog.selected.len(),
                                    dialog.objects.len()
                                ),
                            );
                        });
                        if dialog.objects_slot.is_some() {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label("Loading objects...");
                            });
                        }
                        if let Some(e) = &dialog.objects_error {
                            error_label(ui, e);
                        }
                        let filter = dialog.filter.to_lowercase();
                        egui::ScrollArea::vertical()
                            .id_salt("object_export_tree")
                            .max_height(210.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for kind in ObjectKind::ALL {
                                    let items: Vec<ObjectRef> =
                                        dialog.of_kind(kind).cloned().collect();
                                    if items.is_empty() {
                                        continue;
                                    }
                                    let chosen = items
                                        .iter()
                                        .filter(|o| dialog.selected.contains(*o))
                                        .count();
                                    egui::CollapsingHeader::new(format!(
                                        "{} ({}/{})",
                                        kind.plural(),
                                        chosen,
                                        items.len()
                                    ))
                                    .id_salt(("object_export_kind", kind.plural()))
                                    .default_open(kind == ObjectKind::Table)
                                    .show(ui, |ui| {
                                        let mut all = chosen == items.len();
                                        if ui.checkbox(&mut all, "Select all").changed() {
                                            for item in &items {
                                                if all {
                                                    dialog.selected.insert(item.clone());
                                                } else {
                                                    dialog.selected.remove(item);
                                                }
                                            }
                                        }
                                        for item in &items {
                                            if !filter.is_empty()
                                                && !item.name.to_lowercase().contains(&filter)
                                            {
                                                continue;
                                            }
                                            let mut on = dialog.selected.contains(item);
                                            if ui.checkbox(&mut on, &item.name).changed() {
                                                if on {
                                                    dialog.selected.insert(item.clone());
                                                } else {
                                                    dialog.selected.remove(item);
                                                }
                                            }
                                        }
                                    });
                                }
                            });
                    });
                    ui.add_space(6.0);

                    style::render_modal_card(ui, Some("Options"), None, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut dialog.structure, "Structure");
                            ui.checkbox(&mut dialog.data, "Table data");
                            ui.checkbox(&mut dialog.drop_if_exists, "DROP IF EXISTS before CREATE");
                        });
                        ui.add_enabled(
                            dialog.structure && dialog.data,
                            egui::Checkbox::new(
                                &mut dialog.indexes_post_data,
                                "Write indexes and foreign keys after the data",
                            ),
                        );
                        ui.add_enabled_ui(dialog.data, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Max INSERT size").weak().small());
                                ui.add(
                                    egui::DragValue::new(&mut dialog.max_insert_kb)
                                        .range(1..=65_536)
                                        .suffix(" KB"),
                                );
                                ui.label("or");
                                ui.add(
                                    egui::DragValue::new(&mut dialog.max_insert_rows)
                                        .range(1..=100_000)
                                        .suffix(" rows"),
                                );
                                ui.add_space(12.0);
                                ui.label(egui::RichText::new("Rows per table").weak().small());
                                style::render_text_field(
                                    ui,
                                    egui::TextEdit::singleline(&mut dialog.row_limit)
                                        .hint_text("All"),
                                    90.0,
                                    None,
                                );
                            });
                        });
                        pass_problem = passphrase_fields(
                            ui,
                            &mut dialog.encrypt,
                            &mut dialog.pass1,
                            &mut dialog.pass2,
                        );
                    });
                });

                ui.add_space(6.0);
                if let Some(progress) = &dialog.progress {
                    progress_panel(ui, &snapshot(progress), running, "objects");
                }
                if let Some(e) = &dialog.error {
                    error_label(ui, e);
                }
                let problem = if dialog.selected.is_empty() {
                    Some("Select at least one object".to_string())
                } else if !dialog.structure && !dialog.data {
                    Some("Choose structure, data, or both".to_string())
                } else {
                    limit_error
                        .clone()
                        .or_else(|| pass_problem.map(str::to_string))
                };
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if ui.add(style::btn_danger_ctx(ctx, "Stop")).clicked() {
                            stop = true;
                        }
                    } else {
                        if ui
                            .add_enabled(
                                problem.is_none(),
                                style::btn_primary_ctx(ctx, "Export..."),
                            )
                            .clicked()
                        {
                            start = true;
                        }
                        if ui.add(style::btn_secondary("Close")).clicked() {
                            done = true;
                        }
                        if let Some(problem) = &problem {
                            muted_label(ui, problem.clone());
                        }
                    }
                });
            },
        );
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if stop && let Some(cancel) = &dialog.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if start {
            self.start_object_export(ctx, &mut dialog);
        }
        if running || !(close || done) {
            self.transfer_ui.objects = Some(dialog);
        }
    }

    fn start_object_export(&mut self, ctx: &egui::Context, dialog: &mut ObjectExportDialog) {
        dialog.error = None;
        let Some(endpoint) = dialog.source.endpoint(self) else {
            dialog.error = Some("Connection is no longer available".to_string());
            return;
        };
        let row_limit = match parse_limit(&dialog.row_limit) {
            Ok(limit) => limit,
            Err(e) => {
                dialog.error = Some(e);
                return;
            }
        };
        let default_name = if dialog.source.database.trim().is_empty() {
            endpoint.conn.name.replace(' ', "_")
        } else {
            dialog.source.database.trim().to_string()
        };
        let Some(target) = rfd::FileDialog::new()
            .add_filter("SQL files", &["sql"])
            .set_file_name(format!(
                "{}_{}.sql",
                default_name,
                chrono::Local::now().format("%Y%m%d_%H%M%S")
            ))
            .save_file()
        else {
            return;
        };
        dialog.target = Some(target);
        let objects: Vec<ObjectRef> = dialog.selected.iter().cloned().collect();
        let opts = SqlExportOptions {
            structure: dialog.structure,
            data: dialog.data,
            drop_if_exists: dialog.drop_if_exists,
            insert_limits: InsertLimits {
                max_rows: dialog.max_insert_rows as usize,
                max_bytes: dialog.max_insert_kb as usize * 1024,
            },
            indexes_post_data: dialog.indexes_post_data,
            row_limit,
            ..Default::default()
        };
        let progress = new_progress();
        let cancel = Arc::new(AtomicBool::new(false));
        dialog.progress = Some(progress.clone());
        dialog.cancel = Some(cancel.clone());
        dialog.slot = Some(self.spawn_slot(ctx, async move {
            object_export::export_sql(endpoint, &objects, &opts, &progress, &cancel).await
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_limit_parsing() {
        assert_eq!(parse_limit(""), Ok(None));
        assert_eq!(parse_limit(" 1,000 "), Ok(Some(1000)));
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("abc").is_err());
    }

    #[test]
    fn transfer_needs_distinct_target() {
        let mut dialog = TransferDialog::new(Some(1), Some("shop".into()), Some("orders".into()));
        assert_eq!(
            dialog.problem().as_deref(),
            Some("Choose a target connection")
        );
        dialog.target = EndpointPick::new(Some(1), Some("shop".into()));
        dialog.selected.insert("orders".to_string());
        assert!(
            dialog
                .problem()
                .unwrap()
                .starts_with("Source and target are the same")
        );
        dialog.target_table = "orders_copy".to_string();
        assert_eq!(dialog.problem(), None);
        dialog.target = EndpointPick::new(Some(2), Some("shop".into()));
        dialog.target_table = "orders".to_string();
        assert_eq!(dialog.problem(), None);
        dialog.row_limit = "x".to_string();
        assert!(dialog.problem().is_some());
    }
}
