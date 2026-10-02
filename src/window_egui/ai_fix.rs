//! Jendela "Fix with AI" (K9): minta AI memperbaiki statement yang gagal,
//! tampilkan hasilnya sebagai diff per baris, lalu user memilih Apply
//! (ganti statement di tab), Copy, atau Cancel. Query tidak pernah dijalankan
//! otomatis.
//!
//! Prompt, ekstraksi SQL, dan diff ada di `crate::ai_query_fix` (headless).

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;

use super::Tabular;
use crate::ai_query_fix::{self, DiffKind, DiffLine};

/// Jumlah tabel maksimum dalam konteks skema prompt.
const MAX_SCHEMA_TABLES: usize = 25;

pub enum FixStatus {
    Loading { started: std::time::Instant },
    Ready { fixed: String, diff: Vec<DiffLine> },
    Failed(String),
}

/// State jendela Fix with AI (satu jendela untuk seluruh aplikasi).
pub struct AiFixState {
    /// `QueryTab::id` tempat query gagal.
    pub tab_id: usize,
    pub original: String,
    pub error: String,
    pub engine: String,
    pub status: FixStatus,
    rx: Option<Receiver<Result<String, String>>>,
    pub notice: Option<String>,
    /// `true` = jendela disembunyikan; permintaan tetap berjalan di background.
    hidden: bool,
    /// Entri permintaan ini di panel Background Processes.
    task_id: Option<u64>,
}

/// Nama engine koneksi tab aktif, bila ada.
pub(crate) fn active_engine(tabular: &Tabular) -> Option<&'static str> {
    let cid = tabular
        .query_tabs
        .get(tabular.active_tab_index)?
        .connection_id?;
    tabular
        .connections
        .iter()
        .find(|c| c.id == Some(cid))
        .map(|c| ai_query_fix::engine_label(&c.connection_type))
}

/// Statement yang gagal di tab aktif: statement dari lokasi error bila
/// driver melaporkannya, selain itu SQL terakhir yang dijalankan.
fn failing_sql(tabular: &Tabular) -> String {
    let active_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.id);
    if let Some((tab_id, loc)) = &tabular.last_error_location
        && Some(*tab_id) == active_id
        && !loc.statement.trim().is_empty()
    {
        return loc.statement.trim().to_string();
    }
    tabular.last_executed_sql.trim().to_string()
}

fn error_text(tabular: &Tabular) -> String {
    let msg = tabular
        .query_message
        .strip_prefix("Error: ")
        .unwrap_or(&tabular.query_message);
    if msg.trim().is_empty() {
        tabular.current_table_name.clone()
    } else {
        msg.to_string()
    }
}

/// Tombol "Fix with AI" boleh tampil: query terakhir di tab SQL aktif gagal.
pub fn can_fix(tabular: &Tabular) -> bool {
    tabular.query_message_is_error
        && tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .is_some_and(crate::ai_assistant::is_sql_tab)
        && !failing_sql(tabular).is_empty()
}

/// Mulai permintaan perbaikan untuk query gagal di tab aktif.
pub fn start_fix(tabular: &mut Tabular) {
    let Some(tab_id) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.id)
    else {
        return;
    };
    let original = failing_sql(tabular);
    if original.is_empty() {
        tabular
            .toasts
            .info("There is no failed statement to fix in this tab.");
        return;
    }
    let error = error_text(tabular);
    let engine = active_engine(tabular).unwrap_or("SQL").to_string();
    let mut state = AiFixState {
        tab_id,
        original: original.clone(),
        error: error.clone(),
        engine: engine.clone(),
        status: FixStatus::Loading {
            started: std::time::Instant::now(),
        },
        rx: None,
        notice: None,
        hidden: false,
        task_id: None,
    };

    let target = tabular.effective_chat_target();
    if let Err(msg) = crate::ai_assistant::backend_ready_for(tabular, target) {
        state.status = FixStatus::Failed(msg);
        tabular.ai_fix = Some(state);
        return;
    }
    let schema =
        crate::ai_assistant::build_schema_context_for_prompt(tabular, &original, MAX_SCHEMA_TABLES);
    let cfg = crate::ai_assistant::chat_backend_for(tabular, target);
    let rx = crate::ai_assistant::request_text(
        &cfg,
        ai_query_fix::fix_system_prompt(&engine),
        ai_query_fix::fix_user_prompt(&original, &error, &engine, &schema),
    );
    state.rx = Some(rx);
    tabular.ai_fix = Some(state);
    log::info!("[AI] meminta perbaikan query gagal ({engine})");
}

fn poll(state: &mut AiFixState) {
    let Some(rx) = &state.rx else { return };
    let status = match rx.try_recv() {
        Ok(Ok(reply)) => match ai_query_fix::extract_fixed_sql(&reply) {
            Some(fixed) => {
                let diff = ai_query_fix::line_diff(&state.original, &fixed);
                FixStatus::Ready { fixed, diff }
            }
            None => FixStatus::Failed("The AI returned an empty answer.".to_string()),
        },
        Ok(Err(e)) => {
            log::warn!("[AI] perbaikan query gagal: {e}");
            FixStatus::Failed(e)
        }
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => {
            FixStatus::Failed("The AI backend stopped without a reply.".to_string())
        }
    };
    state.status = status;
    state.rx = None;
}

/// Ganti statement asal di tab dengan hasil perbaikan. Mengembalikan `false`
/// bila statement asal sudah tidak ada (hasil lalu disalin ke clipboard).
fn apply_fix(tabular: &mut Tabular, tab_id: usize, original: &str, fixed: &str) -> bool {
    let Some(idx) = tabular.query_tabs.iter().position(|t| t.id == tab_id) else {
        return false;
    };
    let text = if idx == tabular.active_tab_index {
        tabular.editor.text.clone()
    } else {
        tabular.query_tabs[idx].content.clone()
    };
    let Some(new_text) = ai_query_fix::replace_statement(&text, original, fixed) else {
        return false;
    };
    crate::editor::ai_write_tab_content(tabular, idx, new_text, true);
    log::info!("[AI] perbaikan query diterapkan ke tab {tab_id}");
    true
}

enum Action {
    Apply,
    Copy,
    Retry,
    Close,
}

fn render_diff(ui: &mut egui::Ui, diff: &[DiffLine]) {
    let ctx = ui.ctx().clone();
    let removed = super::style::theme_danger(&ctx);
    let added = super::style::theme_success(&ctx);
    ui.spacing_mut().item_spacing.y = 0.0;
    for line in diff {
        let (prefix, fill, color) = match line.kind {
            DiffKind::Same => (
                ' ',
                egui::Color32::TRANSPARENT,
                ui.visuals().weak_text_color(),
            ),
            DiffKind::Removed => ('-', removed.linear_multiply(0.14), removed),
            DiffKind::Added => ('+', added.linear_multiply(0.14), added),
        };
        egui::Frame::NONE
            .fill(fill)
            .inner_margin(egui::Margin::symmetric(6, 1))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{prefix} {}", line.text))
                            .family(egui::FontFamily::Monospace)
                            .size(12.0)
                            .color(color),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
    }
}

/// Cerminkan permintaan ke panel Background Processes dan jalankan permintaan
/// panel. Mengembalikan `false` bila jendela tidak perlu digambar.
fn sync_background_task(tabular: &mut Tabular) -> bool {
    use super::background_tasks::{Snapshot, TaskOwner};

    let Some(state) = tabular.ai_fix.as_mut() else {
        tabular.background_tasks.retain_owner(TaskOwner::AiFix, &[]);
        return false;
    };
    let (started_at, result) = match &state.status {
        FixStatus::Loading { started } => (Some(*started), None),
        FixStatus::Ready { .. } => (None, Some(Ok("Fix ready to review".to_string()))),
        FixStatus::Failed(e) => (None, Some(Err(e.clone()))),
    };
    let subtitle = format!("{} error: {}", state.engine, state.error);
    let out = tabular.background_tasks.mirror(
        &mut state.task_id,
        Snapshot {
            owner: TaskOwner::AiFix,
            title: "Fix with AI",
            subtitle: &subtitle,
            steps: &[],
            started_at,
            last_activity_at: None,
            hidden: state.hidden,
            result,
        },
    );
    let seen: Vec<u64> = state.task_id.into_iter().collect();
    tabular
        .background_tasks
        .retain_owner(TaskOwner::AiFix, &seen);
    if out.show {
        state.hidden = false;
    }
    if out.finished_hidden {
        match &state.status {
            FixStatus::Failed(e) => tabular.toasts.error(format!("Fix with AI failed: {e}")),
            _ => tabular
                .toasts
                .success("Fix with AI is ready. Open it from Background Processes."),
        }
    }
    if out.cancel || out.dismissed {
        tabular.ai_fix = None;
        return false;
    }
    !state.hidden
}

/// Gambar jendela Fix with AI bila sedang terbuka.
pub fn render_ai_fix_window(tabular: &mut Tabular, ctx: &egui::Context) {
    let Some(state) = tabular.ai_fix.as_mut() else {
        // Jendela sudah ditutup: entri panelnya ikut hilang.
        sync_background_task(tabular);
        return;
    };
    poll(state);
    if matches!(state.status, FixStatus::Loading { .. }) {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
    if !sync_background_task(tabular) {
        return;
    }
    let Some(state) = tabular.ai_fix.as_mut() else {
        return;
    };

    let mut open = true;
    let mut action: Option<Action> = None;
    egui::Window::new("Fix with AI")
        .id(egui::Id::new("ai_fix_query_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(640.0)
        .default_height(420.0)
        .show(ctx, |ui| {
            let danger = super::style::theme_danger(ui.ctx());
            ui.label(
                egui::RichText::new(format!("{} error", state.engine))
                    .strong()
                    .size(12.0),
            );
            egui::ScrollArea::vertical()
                .id_salt("ai_fix_error")
                .max_height(60.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&state.error).color(danger).size(11.5));
                });
            ui.separator();

            match &state.status {
                FixStatus::Loading { started } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!(
                            "Asking AI for a corrected statement… {}s",
                            started.elapsed().as_secs()
                        ));
                    });
                }
                FixStatus::Failed(e) => {
                    ui.label(egui::RichText::new(e).color(danger));
                }
                FixStatus::Ready { fixed, diff } => {
                    if fixed.trim() == state.original.trim() {
                        ui.label(
                            egui::RichText::new(
                                "The AI returned the same statement. The error may come from data or permissions rather than the SQL.",
                            )
                            .weak(),
                        );
                    }
                    ui.label(
                        egui::RichText::new("Review the change. Nothing runs until you execute it.")
                            .weak()
                            .size(11.0),
                    );
                    ui.add_space(4.0);
                    let avail = (ui.available_height() - 48.0).max(120.0);
                    egui::ScrollArea::both()
                        .id_salt("ai_fix_diff")
                        .max_height(avail)
                        .auto_shrink([false, true])
                        .show(ui, |ui| render_diff(ui, diff));
                }
            }

            if let Some(notice) = &state.notice {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(notice).weak().size(11.0));
            }

            ui.separator();
            ui.horizontal(|ui| {
                let ready = matches!(state.status, FixStatus::Ready { .. });
                if ui
                    .add_enabled(ready, super::style::btn_primary_ctx(ui.ctx(), "Apply"))
                    .on_hover_text("Replace the failing statement in the tab. It is not executed.")
                    .clicked()
                {
                    action = Some(Action::Apply);
                }
                if ui
                    .add_enabled(ready, super::style::btn_secondary("Copy"))
                    .on_hover_text("Copy the corrected statement")
                    .clicked()
                {
                    action = Some(Action::Copy);
                }
                if matches!(state.status, FixStatus::Failed(_) | FixStatus::Ready { .. })
                    && ui.add(super::style::btn_secondary("Retry")).clicked()
                {
                    action = Some(Action::Retry);
                }
                if ui.add(super::style::btn_secondary("Cancel")).clicked() {
                    action = Some(Action::Close);
                }
                if matches!(state.status, FixStatus::Loading { .. })
                    && ui
                        .add(super::style::btn_secondary("Process in Background"))
                        .on_hover_text(
                            "Hide this window and keep working. Follow it in Background \
                             Processes at the bottom of the sidebar.",
                        )
                        .clicked()
                {
                    state.hidden = true;
                }
            });
        });

    if !open {
        action = Some(Action::Close);
    }
    match action {
        None => {}
        Some(Action::Close) => tabular.ai_fix = None,
        Some(Action::Copy) => {
            if let Some(FixStatus::Ready { fixed, .. }) = tabular.ai_fix.as_ref().map(|s| &s.status)
            {
                ctx.copy_text(fixed.clone());
                tabular.toasts.success("Corrected statement copied.");
            }
        }
        Some(Action::Retry) => {
            let tab_id = tabular.ai_fix.as_ref().map(|s| s.tab_id);
            let active_id = tabular
                .query_tabs
                .get(tabular.active_tab_index)
                .map(|t| t.id);
            if tab_id == active_id {
                start_fix(tabular);
            } else if let Some(s) = tabular.ai_fix.as_mut() {
                s.notice = Some("Switch back to the tab where the query failed to retry.".into());
            }
        }
        Some(Action::Apply) => {
            let Some((tab_id, original, fixed)) =
                tabular.ai_fix.as_ref().and_then(|s| match &s.status {
                    FixStatus::Ready { fixed, .. } => {
                        Some((s.tab_id, s.original.clone(), fixed.clone()))
                    }
                    _ => None,
                })
            else {
                return;
            };
            if apply_fix(tabular, tab_id, &original, &fixed) {
                tabular.ai_fix = None;
                tabular
                    .toasts
                    .success("Fix applied to the editor. Run it when you are ready.");
            } else {
                ctx.copy_text(fixed);
                if let Some(s) = tabular.ai_fix.as_mut() {
                    s.notice = Some(
                        "The failing statement is no longer in the tab, so the corrected statement was copied to the clipboard instead.".to_string(),
                    );
                }
            }
        }
    }
}
