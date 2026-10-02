//! Tab Merge Review: detail PR/MR, daftar file, diff side-by-side, panel
//! review AI (backend AI Tabular), komentar, serta merge/close.
//! Diadaptasi dari panel webview ekstensi "GitMerge Review".

use eframe::egui;
use egui_icons::icons as i;

use super::git_jobs::{self, GitTabState, GitView, MrConfirm};
use super::git_view::{self, ConfirmOutcome};
use crate::git::review::{MergeMethod, MergeRequest, MrState, Provider, Recommendation};
use crate::window_egui::{PrefTab, Tabular, style};

fn provider_icon(p: Provider) -> &'static str {
    match p {
        Provider::GitHub => i::ICON_GITHUB.codepoint,
        Provider::GitLab => i::ICON_GITLAB.codepoint,
    }
}

fn state_badge(ui: &mut egui::Ui, mr: &MergeRequest) {
    let ctx = ui.ctx().clone();
    let (text, color) = match (mr.state, mr.is_draft) {
        (MrState::Open, true) => ("Draft", style::theme_muted_text(&ctx)),
        (MrState::Open, false) => ("Open", style::theme_success(&ctx)),
        (MrState::Merged, _) => ("Merged", egui::Color32::from_rgb(137, 87, 229)),
        (MrState::Closed, _) => ("Closed", style::theme_danger(&ctx)),
    };
    style::render_badge(ui, text, color, egui::Color32::WHITE);
}

fn recommendation_badge(ui: &mut egui::Ui, rec: Recommendation) {
    let ctx = ui.ctx().clone();
    let (icon, color) = match rec {
        Recommendation::Approve => ("✅", style::theme_success(&ctx)),
        Recommendation::ApproveWithSuggestions => ("⚠", style::theme_warning(&ctx)),
        Recommendation::RequestChanges => ("❌", style::theme_danger(&ctx)),
    };
    style::render_badge(
        ui,
        &format!("{icon} {}", rec.label()),
        color,
        egui::Color32::WHITE,
    );
}

pub fn render_mr(t: &mut Tabular, ui: &mut egui::Ui, tab_id: usize, st: &mut GitTabState) {
    let GitView::MergeRequest { mr } = st.view.clone() else {
        return;
    };
    render_header(t, ui, tab_id, st, &mr);
    ui.separator();

    if let Some(e) = st.error.clone() {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
        if ui.button("Retry").clicked() {
            reload(t, tab_id, st);
        }
        if st.mr_files.is_empty() {
            return;
        }
    }
    if st.loading && st.mr_files.is_empty() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading changed files…");
        });
        return;
    }

    let action_h = 132.0;
    let avail = ui.available_size();
    let body_h = (avail.y - action_h).max(160.0);
    let list_w = (avail.x * 0.22).clamp(180.0, 300.0);
    let ai_w = if st.show_ai {
        (avail.x * 0.30).clamp(260.0, 460.0)
    } else {
        0.0
    };
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(list_w, body_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(list_w, body_h));
                render_file_list(ui, tab_id, st);
            },
        );
        ui.separator();
        let diff_w = (ui.available_width() - ai_w - if st.show_ai { 10.0 } else { 0.0 }).max(200.0);
        ui.allocate_ui_with_layout(
            egui::vec2(diff_w, body_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(diff_w, body_h));
                ui.horizontal(|ui| {
                    if let Some(f) = st.selected_file.and_then(|i| st.mr_files.get(i)) {
                        ui.label(egui::RichText::new(&f.filename).strong());
                        if let Some(o) = &f.old_filename {
                            ui.label(
                                egui::RichText::new(format!("(from {o})"))
                                    .color(style::theme_muted_text(ui.ctx())),
                            );
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        git_view::layout_toggle(ui, st);
                    });
                });
                git_view::diff_body(ui, ("git_mr_diff", tab_id), st);
            },
        );
        if st.show_ai {
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ai_w, body_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(egui::vec2(ai_w, body_h));
                    render_ai_panel(t, ui, tab_id, st);
                },
            );
        }
    });
    ui.separator();
    render_actions(t, ui, tab_id, st, &mr);
    render_confirm(t, ui.ctx(), tab_id, st, &mr);
}

fn reload(t: &mut Tabular, tab_id: usize, st: &mut GitTabState) {
    git_jobs::reload_merge_request(t, tab_id, st);
}

fn render_header(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    tab_id: usize,
    st: &mut GitTabState,
    mr: &MergeRequest,
) {
    let muted = style::theme_muted_text(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(provider_icon(mr.provider)).size(18.0));
        ui.label(egui::RichText::new(&mr.title).strong().size(15.0));
        ui.label(
            egui::RichText::new(mr.display_number())
                .color(muted)
                .size(14.0),
        );
        state_badge(ui, mr);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ai_label = if st.show_ai { "Hide AI" } else { "AI Review" };
            if ui
                .button(format!("{} {ai_label}", i::ICON_AUTO_AWESOME.codepoint))
                .clicked()
            {
                st.show_ai = !st.show_ai;
            }
            if ui
                .button(i::ICON_OPEN_IN_NEW.codepoint)
                .on_hover_text(format!("Open in {}", mr.provider.label()))
                .clicked()
                && let Err(e) = crate::url_opener::open_url(&mr.url)
            {
                t.toasts.error(e);
            }
            if ui
                .add_enabled(!st.loading, egui::Button::new(i::ICON_REFRESH.codepoint))
                .on_hover_text("Reload")
                .clicked()
            {
                reload(t, tab_id, st);
            }
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&mr.repo_full_name).color(muted));
        if !mr.source_branch.is_empty() {
            ui.label(egui::RichText::new("·").color(muted));
            ui.label(
                egui::RichText::new(format!("{} → {}", mr.source_branch, mr.target_branch))
                    .family(egui::FontFamily::Monospace)
                    .size(12.0),
            );
        }
        ui.label(egui::RichText::new("·").color(muted));
        ui.label(egui::RichText::new(format!("by {}", mr.author)).color(muted));
        let updated = git_view::relative_iso(&mr.updated_at);
        if !updated.is_empty() {
            ui.label(egui::RichText::new(format!("· updated {updated}")).color(muted));
        }
        ui.label(egui::RichText::new("·").color(muted));
        super::git_diff_view::stat_label(ui, mr.additions, mr.deletions);
        ui.label(
            egui::RichText::new(format!(
                "{} files",
                st.mr_files.len().max(mr.changed_files_count as usize)
            ))
            .color(muted),
        );
        for l in &mr.labels {
            style::render_badge(
                ui,
                l,
                style::nav_track(ui.ctx()),
                style::nav_text_strong(ui.ctx()),
            );
        }
        if !mr.reviewers.is_empty() {
            ui.label(
                egui::RichText::new(format!("· reviewers: {}", mr.reviewers.join(", ")))
                    .color(muted),
            );
        }
    });
    if !mr.description.trim().is_empty() {
        egui::CollapsingHeader::new("Description")
            .id_salt(("git_mr_desc", tab_id))
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("git_mr_desc_scroll", tab_id))
                    .max_height(180.0)
                    .show(ui, |ui| {
                        egui_commonmark::CommonMarkViewer::new().show(
                            ui,
                            &mut t.git.md_cache,
                            &mr.description,
                        );
                    });
            });
    }
}

fn render_file_list(ui: &mut egui::Ui, tab_id: usize, st: &mut GitTabState) {
    git_view::filter_field(ui, &mut st.file_filter, "Filter files");
    let q = st.file_filter.trim().to_lowercase();
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt(("git_mr_files", tab_id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (idx, f) in st.mr_files.iter().enumerate() {
                if !q.is_empty() && !f.filename.to_lowercase().contains(&q) {
                    continue;
                }
                let color = match f.status {
                    crate::git::review::FileStatus::Added => style::theme_success(ui.ctx()),
                    crate::git::review::FileStatus::Deleted => style::theme_danger(ui.ctx()),
                    crate::git::review::FileStatus::Renamed
                    | crate::git::review::FileStatus::Copied => style::theme_info(ui.ctx()),
                    crate::git::review::FileStatus::Modified => style::theme_warning(ui.ctx()),
                };
                let resp = git_view::file_row(
                    ui,
                    &f.filename,
                    f.status.letter(),
                    color,
                    st.selected_file == Some(idx),
                )
                .on_hover_text(format!("+{} −{}", f.additions, f.deletions));
                if resp.clicked() {
                    clicked = Some(idx);
                }
            }
        });
    if let Some(idx) = clicked {
        git_jobs::select_mr_file(st, idx);
    }
}

fn render_ai_panel(t: &mut Tabular, ui: &mut egui::Ui, tab_id: usize, st: &mut GitTabState) {
    let ctx = ui.ctx().clone();
    let running = t.git.reviews.get(&tab_id).is_some_and(|r| r.is_running());
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} AI Review", i::ICON_AUTO_AWESOME.codepoint)).strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if running {
                if ui
                    .button(format!("{} Cancel", i::ICON_STOP.codepoint))
                    .clicked()
                {
                    git_jobs::cancel_review(t, tab_id);
                }
            } else {
                let label = if t.git.reviews.contains_key(&tab_id) {
                    "Analyze again"
                } else {
                    "Analyze with AI"
                };
                if ui
                    .add_enabled(!st.mr_files.is_empty(), style::btn_primary_ctx(&ctx, label))
                    .clicked()
                {
                    git_jobs::start_review(t, tab_id, st);
                }
            }
        });
    });
    let target = t.effective_default_target();
    let label = crate::ai_assistant::backend_label_for(t, target);
    let ready = crate::ai_assistant::backend_ready_for(t, target);
    ui.label(
        egui::RichText::new(format!(
            "Backend: {label} · language: {}",
            t.git.store.settings.review_language
        ))
        .size(11.0)
        .color(style::theme_muted_text(&ctx)),
    );
    if target == crate::config::ChatTarget::Api {
        ui.label(
            egui::RichText::new(
                "The diff is sent to your AI provider. Obvious secrets are redacted first.",
            )
            .size(10.5)
            .color(style::theme_muted_text(&ctx)),
        );
    }
    if let Err(e) = ready {
        ui.add_space(6.0);
        ui.colored_label(
            style::theme_warning(&ctx),
            format!("AI is not configured: {e}"),
        );
        if ui.button("Open AI settings").clicked() {
            t.settings_active_pref_tab = PrefTab::AiAssistant;
            t.show_settings_window = true;
        }
        return;
    }
    ui.add_space(4.0);
    let Some(run) = t.git.reviews.get_mut(&tab_id) else {
        ui.label(
            egui::RichText::new("Run an AI review of every changed file: summary, issues, code quality, security and a recommendation.")
                .color(style::theme_muted_text(&ctx)),
        );
        return;
    };
    ui.horizontal(|ui| {
        if run.is_running() {
            ui.spinner();
        }
        if !run.status_line.is_empty() {
            ui.label(
                egui::RichText::new(&run.status_line)
                    .size(11.0)
                    .color(style::theme_muted_text(&ctx)),
            );
        }
    });
    if let Some(rec) = run.recommendation {
        recommendation_badge(ui, rec);
    }
    if let Some(e) = &run.error {
        ui.colored_label(style::theme_danger(&ctx), e);
    }
    let mut copy_to_comment = false;
    egui::ScrollArea::vertical()
        .id_salt(("git_mr_ai", tab_id))
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 30.0)
        .stick_to_bottom(run.is_running())
        .show(ui, |ui| {
            egui_commonmark::CommonMarkViewer::new().show(ui, &mut run.md_cache, &run.text);
        });
    if !run.is_running() && !run.text.trim().is_empty() {
        ui.horizontal(|ui| {
            if ui
                .button(format!("{} Copy", i::ICON_CONTENT_COPY.codepoint))
                .clicked()
            {
                ui.ctx().copy_text(run.text.clone());
            }
            if ui.button("Use as comment").clicked() {
                copy_to_comment = true;
            }
        });
    }
    if copy_to_comment {
        st.comment = format!(
            "### AI review ({})\n\n{}",
            run.backend_label,
            run.text.trim()
        );
        st.comment_preview = false;
    }
}

fn render_actions(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    tab_id: usize,
    st: &mut GitTabState,
    mr: &MergeRequest,
) {
    let ctx = ui.ctx().clone();
    let busy = st.busy.is_some();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.comment_preview, false, "Write");
        ui.selectable_value(&mut st.comment_preview, true, "Preview");
        if let Some(b) = &st.busy {
            ui.spinner();
            ui.label(b);
        }
    });
    let comment_h = 58.0;
    if st.comment_preview {
        egui::ScrollArea::vertical()
            .id_salt(("git_mr_comment_preview", tab_id))
            .max_height(comment_h)
            .show(ui, |ui| {
                ui.set_min_height(comment_h);
                egui_commonmark::CommonMarkViewer::new().show(ui, &mut t.git.md_cache, &st.comment);
            });
    } else {
        egui::ScrollArea::vertical()
            .id_salt(("git_mr_comment", tab_id))
            .max_height(comment_h)
            .show(ui, |ui| {
                ui.add_sized(
                    [ui.available_width(), comment_h],
                    egui::TextEdit::multiline(&mut st.comment)
                        .hint_text("Leave a comment (Markdown)"),
                );
            });
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy && !st.comment.trim().is_empty(),
                style::btn_secondary(format!("{} Comment", i::ICON_CHAT.codepoint)),
            )
            .clicked()
        {
            let body = st.comment.trim().to_string();
            git_jobs::post_comment(t, tab_id, st, body);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let open = mr.state == MrState::Open;
            if ui
                .add_enabled(open && !busy, style::btn_danger_ctx(&ctx, "Close"))
                .on_hover_text("Close / reject this merge request")
                .clicked()
            {
                st.confirm = Some(MrConfirm::Close);
            }
            let merge_hint = if mr.is_draft {
                "Draft merge requests cannot be merged"
            } else {
                "Merge into the target branch"
            };
            if ui
                .add_enabled(
                    open && !busy && !mr.is_draft,
                    style::btn_success_ctx(&ctx, format!("{} Merge", i::ICON_CALL_MERGE.codepoint)),
                )
                .on_hover_text(merge_hint)
                .clicked()
            {
                st.confirm = Some(MrConfirm::Merge);
            }
            egui::ComboBox::from_id_salt(("git_merge_method", tab_id))
                .selected_text(st.merge_method.label())
                .show_ui(ui, |ui| {
                    for m in MergeMethod::ALL {
                        // GitLab tidak punya "rebase and merge" lewat endpoint merge.
                        if mr.provider == Provider::GitLab && m == MergeMethod::Rebase {
                            continue;
                        }
                        ui.selectable_value(&mut st.merge_method, m, m.label());
                    }
                });
        });
    });
}

fn render_confirm(
    t: &mut Tabular,
    ctx: &egui::Context,
    tab_id: usize,
    st: &mut GitTabState,
    mr: &MergeRequest,
) {
    let Some(action) = st.confirm else {
        return;
    };
    let (title, msg, label, danger) = match action {
        MrConfirm::Merge => (
            "Merge merge request",
            format!(
                "{} \"{}\" will be merged into {} using \"{}\".",
                mr.display_number(),
                mr.title,
                mr.target_branch,
                st.merge_method.label()
            ),
            "Merge",
            false,
        ),
        MrConfirm::Close => (
            "Close merge request",
            format!(
                "Close {} \"{}\" without merging?",
                mr.display_number(),
                mr.title
            ),
            "Close",
            true,
        ),
    };
    match git_view::confirm_dialog(
        ctx,
        &format!("git_mr_confirm_{tab_id}"),
        title,
        &msg,
        label,
        danger,
    ) {
        ConfirmOutcome::Pending => {}
        ConfirmOutcome::Cancelled => st.confirm = None,
        ConfirmOutcome::Confirmed => {
            st.confirm = None;
            git_jobs::mr_action(t, tab_id, st, action);
        }
    }
}
