//! Tab Git di area tengah: diff file lokal, detail commit, dan (lewat
//! `git_review_view`) merge request.

use eframe::egui;
use egui_icons::icons as i;

use super::git_diff_view;
use super::git_jobs::{self, DiffSource, GitTabState, GitView, MAX_DIFF_ROWS};
use crate::git::status::ChangeKind;
use crate::window_egui::{Tabular, style};

/// Waktu relatif singkat dari unix timestamp ("5m ago", "3d ago").
pub fn relative_time(unix: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let d = (now - unix).max(0);
    match d {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        86_400..=2_591_999 => format!("{}d ago", d / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", d / 2_592_000),
        _ => format!("{}y ago", d / 31_536_000),
    }
}

/// Waktu relatif dari string RFC 3339 (API GitHub/GitLab).
pub fn relative_iso(s: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| relative_time(d.timestamp()))
        .unwrap_or_default()
}

pub fn format_unix(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn kind_color(ctx: &egui::Context, kind: ChangeKind) -> egui::Color32 {
    match kind {
        ChangeKind::Added | ChangeKind::Untracked => style::theme_success(ctx),
        ChangeKind::Deleted => style::theme_danger(ctx),
        ChangeKind::Conflicted => style::theme_warning(ctx),
        ChangeKind::Renamed | ChangeKind::Copied => style::theme_info(ctx),
        _ => style::theme_warning(ctx).gamma_multiply(0.9),
    }
}

/// Render tab Git aktif. State diambil sementara dari tab supaya fungsi
/// render boleh memakai `&mut Tabular` (toast, job, AI).
pub fn render_active_git_tab(t: &mut Tabular, ui: &mut egui::Ui) {
    let idx = t.active_tab_index;
    let Some(tab) = t.query_tabs.get_mut(idx) else {
        return;
    };
    let tab_id = tab.id;
    let Some(mut st) = tab.git_state.take() else {
        return;
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| match &st.view {
            GitView::FileDiff { .. } => render_file_diff(t, ui, &mut st),
            GitView::Commit { .. } => render_commit(t, ui, tab_id, &mut st),
            GitView::MergeRequest { .. } => {
                super::git_review_view::render_mr(t, ui, tab_id, &mut st)
            }
        });
    // Tab bisa saja berganti selama render (mis. job membuka tab lain).
    if let Some(tab) = t.query_tabs.iter_mut().find(|q| q.id == tab_id)
        && tab.git_state.is_none()
    {
        tab.git_state = Some(st);
    }
}

/// Toggle side-by-side / unified.
pub fn layout_toggle(ui: &mut egui::Ui, st: &mut GitTabState) {
    ui.selectable_value(&mut st.side_by_side, true, "Side by side");
    ui.selectable_value(&mut st.side_by_side, false, "Unified");
}

/// Isi diff dengan status loading/error/biner.
pub fn diff_body(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    st: &mut GitTabState,
) {
    if let Some(e) = &st.error {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
        return;
    }
    if st.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading diff…");
        });
        return;
    }
    if st.binary {
        ui.label(
            egui::RichText::new("Binary file or diff not available.")
                .color(style::theme_muted_text(ui.ctx())),
        );
        return;
    }
    if st.rows.is_empty() {
        ui.label(
            egui::RichText::new("No changes to show.").color(style::theme_muted_text(ui.ctx())),
        );
        return;
    }
    let limit = (!st.show_all_rows).then_some(MAX_DIFF_ROWS);
    if git_diff_view::render_diff(ui, id, &st.rows, st.side_by_side, limit) {
        st.show_all_rows = true;
    }
}

fn render_file_diff(t: &mut Tabular, ui: &mut egui::Ui, st: &mut GitTabState) {
    let GitView::FileDiff {
        repo,
        path,
        orig,
        source,
    } = st.view.clone()
    else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(i::ICON_DIFFERENCE.codepoint).size(16.0));
        ui.label(egui::RichText::new(&path).strong().size(14.0));
        if let Some(o) = &orig {
            ui.label(
                egui::RichText::new(format!("(from {o})")).color(style::theme_muted_text(ui.ctx())),
            );
        }
        let tag = match source {
            DiffSource::Worktree => "Working tree",
            DiffSource::Staged => "Staged",
            DiffSource::Untracked => "Untracked",
            DiffSource::Conflict => "Conflict",
        };
        style::render_badge(
            ui,
            tag,
            style::nav_track(ui.ctx()),
            style::nav_text_strong(ui.ctx()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = t.git.is_busy();
            let same_repo = t.git.active_path().as_deref() == Some(repo.as_path());
            match source {
                DiffSource::Staged => {
                    if ui
                        .add_enabled(!busy && same_repo, egui::Button::new("Unstage"))
                        .clicked()
                    {
                        git_jobs::unstage(t, vec![path.clone()]);
                    }
                }
                _ => {
                    if ui
                        .add_enabled(!busy && same_repo, egui::Button::new("Stage"))
                        .clicked()
                    {
                        git_jobs::stage(t, vec![path.clone()]);
                    }
                }
            }
            if ui
                .button(i::ICON_FOLDER_OPEN.codepoint)
                .on_hover_text("Reveal in file manager")
                .clicked()
            {
                let full = repo.join(&path);
                let dir = full
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or(repo.clone());
                if let Err(e) = crate::url_opener::open_folder(&dir) {
                    t.toasts.error(e);
                }
            }
            ui.separator();
            layout_toggle(ui, st);
        });
    });
    if source == DiffSource::Conflict {
        ui.colored_label(
            style::theme_warning(ui.ctx()),
            "This file has merge conflicts. Resolve the markers in your editor, then stage it.",
        );
    }
    ui.separator();
    diff_body(ui, ("git_file_diff", &path), st);
}

fn render_commit(t: &mut Tabular, ui: &mut egui::Ui, tab_id: usize, st: &mut GitTabState) {
    let GitView::Commit { commit, .. } = st.view.clone() else {
        return;
    };
    let muted = style::theme_muted_text(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(i::ICON_COMMIT.codepoint).size(16.0));
        ui.label(egui::RichText::new(&commit.subject).strong().size(14.0));
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("{} <{}>", commit.author, commit.email)).color(muted));
        ui.label(egui::RichText::new("·").color(muted));
        ui.label(egui::RichText::new(format_unix(commit.time)).color(muted));
        ui.label(egui::RichText::new("·").color(muted));
        if ui
            .link(egui::RichText::new(&commit.short).family(egui::FontFamily::Monospace))
            .on_hover_text("Copy full hash")
            .clicked()
        {
            ui.ctx().copy_text(commit.hash.clone());
            t.toasts.info("Commit hash copied");
        }
        if commit.parents.len() > 1 {
            style::render_badge(
                ui,
                "merge",
                style::nav_track(ui.ctx()),
                style::nav_text_strong(ui.ctx()),
            );
        }
        if !commit.refs.is_empty() {
            ui.label(
                egui::RichText::new(&commit.refs)
                    .color(style::theme_info(ui.ctx()))
                    .size(11.5),
            );
        }
    });
    let body = st
        .commit_message
        .split_once('\n')
        .map(|(_, b)| b.trim())
        .unwrap_or("");
    if !body.is_empty() {
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt(("git_commit_body", tab_id))
            .max_height(90.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(body)
                        .family(egui::FontFamily::Monospace)
                        .size(12.0),
                );
            });
    }
    ui.separator();

    let mut clicked = None;
    let avail = ui.available_size();
    let list_w = (avail.x * 0.28).clamp(200.0, 360.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(list_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(list_w, avail.y));
                ui.label(
                    egui::RichText::new(format!("{} files changed", st.commit_files.len()))
                        .color(muted)
                        .size(11.5),
                );
                egui::ScrollArea::vertical()
                    .id_salt(("git_commit_files", tab_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (idx, f) in st.commit_files.iter().enumerate() {
                            let selected = st.selected_file == Some(idx);
                            let resp = file_row(
                                ui,
                                &f.path,
                                f.kind.letter(),
                                kind_color(ui.ctx(), f.kind),
                                selected,
                            );
                            if resp.clicked() {
                                clicked = Some(idx);
                            }
                        }
                    });
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.horizontal(|ui| {
                    if let Some(f) = st.selected_file.and_then(|i| st.commit_files.get(i)) {
                        ui.label(egui::RichText::new(&f.path).strong());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        layout_toggle(ui, st);
                    });
                });
                diff_body(ui, ("git_commit_diff", tab_id), st);
            },
        );
    });
    if let Some(idx) = clicked {
        git_jobs::load_commit_file_for(&mut t.git, tab_id, st, idx);
    }
}

/// Baris file dengan huruf status berwarna di kanan. Nama file tebal, folder redup.
pub fn file_row(
    ui: &mut egui::Ui,
    path: &str,
    letter: &str,
    color: egui::Color32,
    selected: bool,
) -> egui::Response {
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (d, n),
        None => ("", path),
    };
    let h = 22.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx().clone();
        if selected {
            ui.painter().rect_filled(rect, 3.0, style::nav_raised(&ctx));
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 3.0, style::nav_track(&ctx));
        }
        let font = egui::FontId::proportional(12.5);
        let letter_w = 16.0;
        let clip = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right() - letter_w - 4.0, rect.max.y),
        );
        let painter = ui.painter().with_clip_rect(clip);
        let name_g =
            painter.layout_no_wrap(name.to_string(), font.clone(), style::nav_text_strong(&ctx));
        let name_w = name_g.size().x;
        painter.galley(
            egui::pos2(rect.left() + 6.0, rect.center().y - name_g.size().y / 2.0),
            name_g,
            style::nav_text_strong(&ctx),
        );
        if !dir.is_empty() {
            painter.text(
                egui::pos2(rect.left() + 12.0 + name_w, rect.center().y),
                egui::Align2::LEFT_CENTER,
                dir,
                egui::FontId::proportional(11.0),
                style::nav_text_muted(&ctx),
            );
        }
        ui.painter().text(
            egui::pos2(rect.right() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            letter,
            egui::FontId::proportional(12.0),
            color,
        );
    }
    resp.on_hover_text(path)
}

/// Hasil dialog konfirmasi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Pending,
    Confirmed,
    Cancelled,
}

/// Dialog konfirmasi modal bergaya aplikasi (backdrop + kartu).
pub fn confirm_dialog(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    message: &str,
    confirm_label: &str,
    danger: bool,
) -> ConfirmOutcome {
    let mut out = ConfirmOutcome::Pending;
    let mut close = false;
    style::render_modal_backdrop(ctx, id, true);
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(380.0)
        .show(ctx, |ui| {
            style::render_modal_header(ui, title, &mut close);
            style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_min_width(340.0);
                ui.label(message);
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = if danger {
                        style::btn_danger_ctx(ui.ctx(), confirm_label)
                    } else {
                        style::btn_primary_ctx(ui.ctx(), confirm_label)
                    };
                    if ui.add(btn).clicked() {
                        out = ConfirmOutcome::Confirmed;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        out = ConfirmOutcome::Cancelled;
                    }
                });
            });
        });
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        out = ConfirmOutcome::Cancelled;
    }
    out
}

/// Kolom filter yang langsung berlaku saat mengetik.
pub fn filter_field(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    let muted = style::nav_text_muted(ui.ctx());
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(text).hint_text(egui::RichText::new(hint).color(muted)),
        f32::INFINITY,
        Some(i::ICON_SEARCH.codepoint),
    )
}
