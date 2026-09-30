//! Sidebar tab "Git": pemilih repository (gabungan folder dari Diagram, HTTP
//! API, Project, dan folder yang ditambahkan di Git), header branch dengan
//! Fetch/Pull/Push, serta sub-tab Changes / Branches / History / Review.

use eframe::egui;
use egui_icons::icons as i;

use super::git_jobs::{self, CloneDialog, DiffSource, GitSubMenu, SidebarConfirm};
use super::git_view::{self, ConfirmOutcome};
use crate::git::repos::{LinkKind, RepoEntry};
use crate::git::review::{MergeRequest, Provider};
use crate::git::status::FileChange;
use crate::window_egui::{PrefTab, Tabular, style};

fn muted(ui: &egui::Ui) -> egui::Color32 {
    style::theme_muted_text(ui.ctx())
}

fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon).size(15.0)).frame(false),
    )
    .on_hover_text(tooltip)
    .clicked()
}

/// Render isi sidebar Git.
pub fn render_git_sidebar(t: &mut Tabular, ui: &mut egui::Ui) {
    if !t.git.repos_loaded {
        git_jobs::reload_repos(t);
    }
    render_repo_picker(t, ui);
    ui.add_space(4.0);

    let segments = [
        style::NavSegment {
            key: "Changes",
            icon: i::ICON_SOURCE_COMMIT.codepoint,
            label: "Changes",
        },
        style::NavSegment {
            key: "Branches",
            icon: i::ICON_SOURCE_BRANCH.codepoint,
            label: "Branches",
        },
        style::NavSegment {
            key: "History",
            icon: i::ICON_HISTORY.codepoint,
            label: "History",
        },
        style::NavSegment {
            key: "Review",
            icon: i::ICON_SOURCE_PULL.codepoint,
            label: "Review",
        },
    ];
    if let Some(key) =
        style::render_segmented_nav(ui, "git_sub_nav", &segments, t.git.sub.key(), 32.0)
    {
        let next = GitSubMenu::from_key(key);
        if next != t.git.sub {
            t.git.sub = next;
            match next {
                GitSubMenu::Review if t.git.mrs_loaded_at.is_none() => git_jobs::load_mrs(t),
                GitSubMenu::History if t.git.commits.is_empty() => git_jobs::refresh_log(t, true),
                GitSubMenu::Changes => git_jobs::refresh_status(t),
                _ => {}
            }
        }
    }
    ui.add_space(4.0);

    if t.git.sub == GitSubMenu::Review {
        render_review(t, ui);
    } else if t.git.active().is_some_and(|r| r.path.is_some()) {
        render_branch_header(t, ui);
        ui.add_space(2.0);
        match t.git.sub {
            GitSubMenu::Changes => render_changes(t, ui),
            GitSubMenu::Branches => render_branches(t, ui),
            GitSubMenu::History => render_history(t, ui),
            GitSubMenu::Review => {}
        }
    }
    render_dialogs(t, ui.ctx());
}

// ─── Repository ─────────────────────────────────────────────────────────────

fn link_summary(entry: &RepoEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    for kind in [LinkKind::Diagram, LinkKind::HttpFolder, LinkKind::Project] {
        let labels: Vec<&str> = entry
            .links
            .iter()
            .filter(|l| l.kind == kind)
            .map(|l| l.label.as_str())
            .collect();
        if !labels.is_empty() {
            parts.push(format!("{}: {}", kind.label(), labels.join(", ")));
        }
    }
    parts.join(" · ")
}

fn render_repo_picker(t: &mut Tabular, ui: &mut egui::Ui) {
    let active_label = t
        .git
        .active()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "Select repository".to_string());
    let mut pick: Option<String> = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(i::ICON_SOURCE_REPOSITORY.codepoint).size(16.0));
        // Tombol kanan dulu, lalu combo mengisi sisa lebar. Menghitung lebar
        // combo dari konstanta membuat panel sidebar melebar tiap frame.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(
                egui::RichText::new(i::ICON_ADD.codepoint).size(15.0),
                |ui| {
                    repo_menu(t, ui);
                },
            );
            if icon_button(
                ui,
                i::ICON_REFRESH.codepoint,
                "Reload repositories and status",
                !t.git.is_busy(),
            ) {
                git_jobs::reload_repos(t);
            }
            let combo_w = (ui.available_width() - 12.0).max(60.0);
            egui::ComboBox::from_id_salt("git_repo_picker")
                .width(combo_w)
                .selected_text(egui::RichText::new(&active_label).strong())
                .show_ui(ui, |ui| {
                    ui.set_min_width(260.0);
                    if t.git.repos.is_empty() {
                        ui.label(egui::RichText::new("No repositories yet").color(muted(ui)));
                    }
                    for r in &t.git.repos {
                        let selected = t.git.store.active.as_deref() == Some(r.key.as_str());
                        let mut text = r.name.clone();
                        if r.path.is_none() {
                            text.push_str("  (not on this computer)");
                        }
                        let hover = match (&r.path, &r.url) {
                            (Some(p), Some(u)) => {
                                format!("{}\n{}\n{}", p.display(), u, link_summary(r))
                            }
                            (Some(p), None) => format!("{}\n{}", p.display(), link_summary(r)),
                            (None, Some(u)) => format!("{u}\n{}", link_summary(r)),
                            (None, None) => r.key.clone(),
                        };
                        if ui
                            .selectable_label(selected, text)
                            .on_hover_text(hover.trim())
                            .clicked()
                        {
                            pick = Some(r.key.clone());
                        }
                    }
                });
        });
    });
    if let Some(k) = pick {
        git_jobs::select_repo(t, &k);
    }

    let Some(entry) = t.git.active().cloned() else {
        if t.git.repos.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Add a local folder, clone a repository, or set a repository on a diagram group or an HTTP API folder. They all show up here.",
                )
                .color(muted(ui)),
            );
        }
        return;
    };
    let summary = link_summary(&entry);
    if !summary.is_empty() {
        ui.label(
            egui::RichText::new(format!("Linked to {summary}"))
                .size(10.5)
                .color(muted(ui)),
        )
        .on_hover_text("Diagram groups, HTTP API folders and projects that use this repository");
    }
    match &entry.path {
        None => {
            ui.add_space(4.0);
            style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
                ui.label("This repository is not on this computer yet.");
                ui.horizontal(|ui| {
                    if let Some(url) = entry.url.clone()
                        && ui.button("Clone…").clicked()
                    {
                        t.git.clone_dialog = Some(CloneDialog {
                            dest: crate::git::repos::default_clone_dir(&url)
                                .to_string_lossy()
                                .to_string(),
                            url,
                            entry_key: Some(entry.key.clone()),
                        });
                    }
                    if ui.button("Choose folder…").clicked()
                        && let Some(dir) = crate::rfd::FileDialog::new()
                            .set_title("Select the local clone")
                            .pick_folder()
                    {
                        git_jobs::add_folder(t, &dir);
                        git_jobs::link_folder_to_items(t, &entry.key);
                    }
                });
            });
        }
        Some(_) => {
            let missing = entry.links_without_folder().count();
            if missing > 0 {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{missing} linked item(s) have no local folder."
                        ))
                        .size(10.5)
                        .color(style::theme_warning(ui.ctx())),
                    );
                    if ui.small_button("Use this folder").clicked() {
                        git_jobs::link_folder_to_items(t, &entry.key);
                    }
                });
            }
        }
    }
}

/// Menu tambah repository (dipakai juga oleh tombol ➕ di bawah sidebar).
pub fn repo_menu(t: &mut Tabular, ui: &mut egui::Ui) {
    ui.set_min_width(200.0);
    if ui
        .button(format!(
            "{} Add local folder…",
            i::ICON_FOLDER_OPEN.codepoint
        ))
        .clicked()
    {
        ui.close();
        if let Some(dir) = crate::rfd::FileDialog::new()
            .set_title("Select a git repository")
            .pick_folder()
        {
            git_jobs::add_folder(t, &dir);
        }
    }
    if ui
        .button(format!("{} Clone repository…", i::ICON_DOWNLOAD.codepoint))
        .clicked()
    {
        ui.close();
        t.git.clone_dialog = Some(CloneDialog::default());
    }
    if ui
        .button(format!("{} Init repository…", i::ICON_ADD.codepoint))
        .clicked()
    {
        ui.close();
        if let Some(dir) = crate::rfd::FileDialog::new()
            .set_title("Folder for the new repository")
            .pick_folder()
        {
            git_jobs::init_repo(t, dir);
        }
    }
    let manual = t
        .git
        .active()
        .filter(|r| r.links.iter().any(|l| l.kind == LinkKind::Manual))
        .map(|r| r.key.clone());
    if let Some(key) = manual {
        ui.separator();
        if ui.button("Remove from list").clicked() {
            ui.close();
            t.git.confirm = Some(SidebarConfirm::RemoveRepo(key));
        }
    }
    if let Some(path) = t.git.active_path()
        && ui.button("Reveal in file manager").clicked()
    {
        ui.close();
        if let Err(e) = crate::url_opener::open_folder(&path) {
            t.toasts.error(e);
        }
    }
    ui.separator();
    if ui.button("Git settings…").clicked() {
        ui.close();
        t.settings_active_pref_tab = PrefTab::Git;
        t.show_settings_window = true;
    }
}

// ─── Header branch ──────────────────────────────────────────────────────────

fn render_branch_header(t: &mut Tabular, ui: &mut egui::Ui) {
    let busy = t.git.busy.clone();
    let st = t.git.status.clone();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(i::ICON_SOURCE_BRANCH.codepoint).size(15.0));
        let head = st
            .as_ref()
            .map(|s| s.head_label())
            .unwrap_or_else(|| "…".to_string());
        ui.label(egui::RichText::new(head).strong());
        if let Some(s) = &st {
            if s.ahead > 0 {
                ui.label(
                    egui::RichText::new(format!("{}{}", i::ICON_ARROW_UPWARD.codepoint, s.ahead))
                        .size(11.5),
                )
                .on_hover_text(format!("{} commit(s) to push", s.ahead));
            }
            if s.behind > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "{}{}",
                        i::ICON_ARROW_DOWNWARD.codepoint,
                        s.behind
                    ))
                    .size(11.5),
                )
                .on_hover_text(format!("{} commit(s) to pull", s.behind));
            }
            if s.upstream.is_none() && s.branch.is_some() {
                ui.label(
                    egui::RichText::new("no upstream")
                        .size(10.5)
                        .color(muted(ui)),
                );
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let idle = busy.is_none();
            if icon_button(
                ui,
                i::ICON_UPLOAD.codepoint,
                "Push",
                idle && st.as_ref().is_some_and(|s| s.branch.is_some()),
            ) {
                git_jobs::push(t);
            }
            if icon_button(
                ui,
                i::ICON_DOWNLOAD.codepoint,
                "Pull",
                idle && st.as_ref().is_some_and(|s| s.upstream.is_some()),
            ) {
                git_jobs::pull(t);
            }
            if icon_button(ui, i::ICON_SYNC.codepoint, "Fetch all remotes", idle) {
                git_jobs::fetch(t);
            }
        });
    });
    if let Some(label) = busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(format!("{label}…")).size(11.5));
            if matches!(label.as_str(), "Fetch" | "Pull" | "Push" | "Clone")
                && ui.small_button("Cancel").clicked()
            {
                git_jobs::cancel_op(t);
            }
        });
    }
    if let Some(e) = t.git.status_error.clone() {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
    }
}

// ─── Changes ────────────────────────────────────────────────────────────────

enum RowAction {
    Open(FileChange, DiffSource),
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Confirm(SidebarConfirm),
}

fn change_section(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    files: &[FileChange],
    source: DiffSource,
    actions: &mut Vec<RowAction>,
    busy: bool,
) {
    if files.is_empty() {
        return;
    }
    let paths = || files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    let header = egui::CollapsingHeader::new(
        egui::RichText::new(format!("{title}  {}", files.len()))
            .strong()
            .size(12.0),
    )
    .id_salt(id)
    .default_open(true);
    let resp = header.show(ui, |ui| {
        for f in files {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let (icon, tip) = match source {
                    DiffSource::Staged => (i::ICON_REMOVE.codepoint, "Unstage"),
                    _ => (i::ICON_ADD.codepoint, "Stage"),
                };
                if icon_button(ui, icon, tip, !busy) {
                    actions.push(match source {
                        DiffSource::Staged => RowAction::Unstage(vec![f.path.clone()]),
                        _ => RowAction::Stage(vec![f.path.clone()]),
                    });
                }
                let color = git_view::kind_color(ui.ctx(), f.kind);
                let resp = git_view::file_row(ui, &f.path, f.kind.letter(), color, false);
                if resp.clicked() {
                    actions.push(RowAction::Open(f.clone(), source));
                }
                resp.context_menu(|ui| {
                    if ui.button("Open changes").clicked() {
                        actions.push(RowAction::Open(f.clone(), source));
                        ui.close();
                    }
                    match source {
                        DiffSource::Staged => {
                            if ui.button("Unstage").clicked() {
                                actions.push(RowAction::Unstage(vec![f.path.clone()]));
                                ui.close();
                            }
                        }
                        DiffSource::Untracked => {
                            if ui.button("Stage").clicked() {
                                actions.push(RowAction::Stage(vec![f.path.clone()]));
                                ui.close();
                            }
                            if ui.button("Discard file…").clicked() {
                                actions.push(RowAction::Confirm(SidebarConfirm::DiscardUntracked(
                                    vec![f.path.clone()],
                                )));
                                ui.close();
                            }
                        }
                        DiffSource::Worktree | DiffSource::Conflict => {
                            let label = if source == DiffSource::Conflict {
                                "Mark as resolved (stage)"
                            } else {
                                "Stage"
                            };
                            if ui.button(label).clicked() {
                                actions.push(RowAction::Stage(vec![f.path.clone()]));
                                ui.close();
                            }
                            if source == DiffSource::Worktree
                                && ui.button("Discard changes…").clicked()
                            {
                                actions.push(RowAction::Confirm(SidebarConfirm::DiscardTracked(
                                    vec![f.path.clone()],
                                )));
                                ui.close();
                            }
                        }
                    }
                    if ui.button("Copy path").clicked() {
                        ui.ctx().copy_text(f.path.clone());
                        ui.close();
                    }
                });
            });
        }
    });
    resp.header_response.context_menu(|ui| match source {
        DiffSource::Staged => {
            if ui.button("Unstage all").clicked() {
                actions.push(RowAction::Unstage(paths()));
                ui.close();
            }
        }
        DiffSource::Untracked => {
            if ui.button("Stage all").clicked() {
                actions.push(RowAction::Stage(paths()));
                ui.close();
            }
            if ui.button("Discard all untracked…").clicked() {
                actions.push(RowAction::Confirm(
                    SidebarConfirm::DiscardUntracked(paths()),
                ));
                ui.close();
            }
        }
        DiffSource::Worktree => {
            if ui.button("Stage all").clicked() {
                actions.push(RowAction::Stage(paths()));
                ui.close();
            }
            if ui.button("Discard all changes…").clicked() {
                actions.push(RowAction::Confirm(SidebarConfirm::DiscardTracked(paths())));
                ui.close();
            }
        }
        DiffSource::Conflict => {}
    });
}

fn render_changes(t: &mut Tabular, ui: &mut egui::Ui) {
    let busy = t.git.is_busy();
    let st = t.git.status.clone().unwrap_or_default();

    // Kotak pesan commit.
    let edit_id = egui::Id::new("git_commit_message");
    let hint_color = style::nav_text_muted(ui.ctx());
    let resp = ui.add_sized(
        [ui.available_width(), 64.0],
        egui::TextEdit::multiline(&mut t.git.commit_message)
            .id(edit_id)
            .hint_text(egui::RichText::new("Message (Ctrl+Enter to commit)").color(hint_color)),
    );
    let submit =
        resp.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let label = if t.git.amend {
            "Amend commit"
        } else if st.staged.is_empty() {
            "Commit all"
        } else {
            "Commit"
        };
        let has_changes = !st.is_clean() || t.git.amend;
        let tip = if st.staged.is_empty() {
            "Nothing staged: all changes will be committed"
        } else {
            "Commit staged changes"
        };
        if ui
            .add_enabled(
                !busy && has_changes,
                style::btn_primary_ctx(ui.ctx(), format!("{} {label}", i::ICON_CHECK.codepoint)),
            )
            .on_hover_text(tip)
            .clicked()
            || (submit && !busy && has_changes)
        {
            git_jobs::commit(t);
        }
        ui.checkbox(&mut t.git.amend, "Amend");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if t.git.commit_ai_busy {
                ui.spinner();
            } else if icon_button(
                ui,
                i::ICON_AUTO_AWESOME.codepoint,
                "Generate commit message with AI",
                !st.is_clean(),
            ) {
                git_jobs::generate_commit_message(t);
            }
        });
    });
    ui.add_space(4.0);

    if st.is_clean() && t.git.status.is_some() {
        ui.label(egui::RichText::new("No changes. Working tree clean.").color(muted(ui)));
        return;
    }
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("git_changes_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            change_section(
                ui,
                "git_sec_conflicts",
                "Merge Conflicts",
                &st.conflicted,
                DiffSource::Conflict,
                &mut actions,
                busy,
            );
            change_section(
                ui,
                "git_sec_staged",
                "Staged Changes",
                &st.staged,
                DiffSource::Staged,
                &mut actions,
                busy,
            );
            change_section(
                ui,
                "git_sec_changes",
                "Changes",
                &st.unstaged,
                DiffSource::Worktree,
                &mut actions,
                busy,
            );
            change_section(
                ui,
                "git_sec_untracked",
                "Untracked",
                &st.untracked,
                DiffSource::Untracked,
                &mut actions,
                busy,
            );
        });
    for a in actions {
        match a {
            RowAction::Open(f, src) => git_jobs::open_file_diff(t, &f, src),
            RowAction::Stage(p) => git_jobs::stage(t, p),
            RowAction::Unstage(p) => git_jobs::unstage(t, p),
            RowAction::Confirm(c) => t.git.confirm = Some(c),
        }
    }
}

// ─── Branches ───────────────────────────────────────────────────────────────

fn render_branches(t: &mut Tabular, ui: &mut egui::Ui) {
    let busy = t.git.is_busy();
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    !busy && !t.git.new_branch.trim().is_empty(),
                    egui::Button::new("Create"),
                )
                .on_hover_text("Create the branch from HEAD and switch to it")
                .clicked()
            {
                git_jobs::create_branch(t);
            }
            let w = (ui.available_width() - 4.0).max(60.0);
            style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut t.git.new_branch).hint_text(
                    egui::RichText::new("new-branch-name").color(style::nav_text_muted(ui.ctx())),
                ),
                w,
                Some(i::ICON_SOURCE_BRANCH_PLUS.codepoint),
            );
        });
    });
    git_view::filter_field(ui, &mut t.git.branch_filter, "Filter branches");
    let q = t.git.branch_filter.trim().to_lowercase();
    let dirty = t
        .git
        .status
        .as_ref()
        .is_some_and(|s| !s.staged.is_empty() || !s.unstaged.is_empty());
    let branches = t.git.branches.clone();
    let mut action: Option<SidebarConfirm> = None;
    egui::ScrollArea::vertical()
        .id_salt("git_branches_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for remote in [false, true] {
                let list: Vec<_> = branches
                    .iter()
                    .filter(|b| {
                        b.is_remote == remote
                            && (q.is_empty() || b.name.to_lowercase().contains(&q))
                    })
                    .collect();
                if list.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!(
                        "{}  {}",
                        if remote { "Remote" } else { "Local" },
                        list.len()
                    ))
                    .strong()
                    .size(12.0),
                )
                .id_salt(("git_branch_sec", remote))
                .default_open(!remote)
                .show(ui, |ui| {
                    for b in list {
                        let strong = style::nav_text_strong(ui.ctx());
                        let weak = style::nav_text_muted(ui.ctx());
                        let mut text = egui::text::LayoutJob::default();
                        let prefix = if b.is_head {
                            format!("{} ", i::ICON_CHECK.codepoint)
                        } else {
                            "    ".to_string()
                        };
                        text.append(
                            &prefix,
                            0.0,
                            egui::TextFormat {
                                color: style::theme_success(ui.ctx()),
                                ..Default::default()
                            },
                        );
                        text.append(
                            &b.name,
                            0.0,
                            egui::TextFormat {
                                color: strong,
                                ..Default::default()
                            },
                        );
                        let mut meta = git_view::relative_time(b.time);
                        if !b.track.is_empty() {
                            meta = format!("{} · {meta}", b.track);
                        }
                        text.append(
                            &format!("  {meta}"),
                            0.0,
                            egui::TextFormat {
                                color: weak,
                                font_id: egui::FontId::proportional(10.5),
                                ..Default::default()
                            },
                        );
                        let resp = ui
                            .add(
                                egui::Button::selectable(false, text)
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                            )
                            .on_hover_text(format!(
                                "{} {}\n{}",
                                b.oid,
                                b.subject,
                                b.upstream.as_deref().unwrap_or("")
                            ));
                        if resp.double_clicked() && !b.is_head && !busy {
                            action = Some(SidebarConfirm::Checkout {
                                name: b.name.clone(),
                                remote,
                            });
                        }
                        resp.context_menu(|ui| {
                            if !b.is_head
                                && ui
                                    .add_enabled(!busy, egui::Button::new("Checkout"))
                                    .clicked()
                            {
                                action = Some(SidebarConfirm::Checkout {
                                    name: b.name.clone(),
                                    remote,
                                });
                                ui.close();
                            }
                            if !remote && !b.is_head {
                                if ui
                                    .add_enabled(!busy, egui::Button::new("Remove branch…"))
                                    .clicked()
                                {
                                    action = Some(SidebarConfirm::RemoveBranch {
                                        name: b.name.clone(),
                                        force: false,
                                    });
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        !busy,
                                        egui::Button::new("Force remove (unmerged)…"),
                                    )
                                    .clicked()
                                {
                                    action = Some(SidebarConfirm::RemoveBranch {
                                        name: b.name.clone(),
                                        force: true,
                                    });
                                    ui.close();
                                }
                            }
                            if ui.button("Copy name").clicked() {
                                ui.ctx().copy_text(b.name.clone());
                                ui.close();
                            }
                        });
                    }
                });
            }
            ui.label(
                egui::RichText::new("Double-click a branch to check it out.")
                    .size(10.5)
                    .color(muted(ui)),
            );
        });
    match action {
        // Checkout langsung bila working tree bersih; konfirmasi bila tidak.
        Some(SidebarConfirm::Checkout { name, remote }) if !dirty => {
            git_jobs::checkout(t, name, remote)
        }
        Some(c) => t.git.confirm = Some(c),
        None => {}
    }
}

// ─── History ────────────────────────────────────────────────────────────────

fn render_history(t: &mut Tabular, ui: &mut egui::Ui) {
    git_view::filter_field(ui, &mut t.git.history_filter, "Filter loaded commits");
    let q = t.git.history_filter.trim().to_lowercase();
    let mut open = None;
    let mut more = false;
    egui::ScrollArea::vertical()
        .id_salt("git_history_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if t.git.commits.is_empty() {
                if t.git.commits_loading {
                    ui.spinner();
                } else {
                    ui.label(egui::RichText::new("No commits yet.").color(muted(ui)));
                }
            }
            for c in &t.git.commits {
                if !q.is_empty()
                    && !c.subject.to_lowercase().contains(&q)
                    && !c.author.to_lowercase().contains(&q)
                    && !c.hash.starts_with(&q)
                {
                    continue;
                }
                let resp = ui
                    .vertical(|ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&c.subject).strong().size(12.5))
                                .truncate(),
                        );
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&c.short)
                                    .family(egui::FontFamily::Monospace)
                                    .size(11.0)
                                    .color(style::theme_info(ui.ctx())),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{} · {}",
                                        c.author,
                                        git_view::relative_time(c.time)
                                    ))
                                    .size(11.0)
                                    .color(muted(ui)),
                                )
                                .truncate(),
                            );
                        });
                        if !c.refs.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&c.refs)
                                        .size(10.5)
                                        .color(style::theme_success(ui.ctx())),
                                )
                                .truncate(),
                            );
                        }
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    open = Some(c.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button("Copy hash").clicked() {
                        ui.ctx().copy_text(c.hash.clone());
                        ui.close();
                    }
                    if ui.button("Copy message").clicked() {
                        ui.ctx().copy_text(c.subject.clone());
                        ui.close();
                    }
                });
                ui.add_space(3.0);
            }
            if !t.git.commits_done && !t.git.commits.is_empty() {
                if t.git.commits_loading {
                    ui.spinner();
                } else if ui.button("Load more").clicked() {
                    more = true;
                }
            }
        });
    if let Some(c) = open {
        git_jobs::open_commit(t, &c);
    }
    if more {
        git_jobs::refresh_log(t, false);
    }
}

// ─── Review ─────────────────────────────────────────────────────────────────

fn render_review(t: &mut Tabular, ui: &mut egui::Ui) {
    let access = t.git.provider_access();
    if access.github_token.is_none() && access.gitlab_token.is_none() {
        style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
            ui.label(egui::RichText::new("Merge Review").strong());
            ui.label("Add a GitHub or GitLab access token to list pull requests and merge requests assigned to you.");
            if ui.button("Set tokens…").clicked() {
                t.settings_active_pref_tab = PrefTab::Git;
                t.show_settings_window = true;
            }
        });
        return;
    }
    let mut reload = false;
    ui.horizontal(|ui| {
        if ui
            .selectable_label(!t.git.mrs_active_repo, "Assigned to me")
            .clicked()
            && t.git.mrs_active_repo
        {
            t.git.mrs_active_repo = false;
            reload = true;
        }
        if ui
            .add_enabled(
                t.git.active().is_some(),
                egui::Button::selectable(t.git.mrs_active_repo, "This repository"),
            )
            .clicked()
            && !t.git.mrs_active_repo
        {
            t.git.mrs_active_repo = true;
            reload = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if t.git.mrs_loading {
                ui.spinner();
            } else if icon_button(ui, i::ICON_REFRESH.codepoint, "Refresh", true) {
                reload = true;
            }
        });
    });
    if reload {
        git_jobs::load_mrs(t);
    }
    git_view::filter_field(ui, &mut t.git.mr_filter, "Filter by title, repo, author");
    for e in &t.git.mr_errors {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
    }
    let q = t.git.mr_filter.trim().to_lowercase();
    let list: Vec<MergeRequest> = t
        .git
        .mrs
        .iter()
        .filter(|m| {
            q.is_empty()
                || m.title.to_lowercase().contains(&q)
                || m.repo_full_name.to_lowercase().contains(&q)
                || m.author.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    if list.is_empty() && !t.git.mrs_loading && t.git.mrs_loaded_at.is_some() {
        ui.label(egui::RichText::new("No open merge requests.").color(muted(ui)));
    }
    let mut open = None;
    egui::ScrollArea::vertical()
        .id_salt("git_review_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for provider in [Provider::GitHub, Provider::GitLab] {
                let mine: Vec<&MergeRequest> =
                    list.iter().filter(|m| m.provider == provider).collect();
                if mine.is_empty() {
                    continue;
                }
                let icon = match provider {
                    Provider::GitHub => i::ICON_GITHUB.codepoint,
                    Provider::GitLab => i::ICON_GITLAB.codepoint,
                };
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!("{icon} {}  {}", provider.label(), mine.len()))
                        .strong()
                        .size(12.0),
                )
                .id_salt(("git_mr_provider", provider.label()))
                .default_open(true)
                .show(ui, |ui| {
                    let mut repos: Vec<&str> =
                        mine.iter().map(|m| m.repo_full_name.as_str()).collect();
                    repos.sort_unstable();
                    repos.dedup();
                    for repo in repos {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {repo}",
                                i::ICON_SOURCE_REPOSITORY.codepoint
                            ))
                            .size(11.5)
                            .color(muted(ui)),
                        );
                        for m in mine.iter().filter(|m| m.repo_full_name == repo) {
                            if mr_row(ui, m).clicked() {
                                open = Some((*m).clone());
                            }
                        }
                    }
                });
            }
        });
    if let Some(m) = open {
        git_jobs::open_merge_request(t, &m);
    }
}

fn mr_row(ui: &mut egui::Ui, m: &MergeRequest) -> egui::Response {
    let resp = ui
        .vertical(|ui| {
            ui.horizontal(|ui| {
                let color = if m.is_draft {
                    style::theme_muted_text(ui.ctx())
                } else {
                    style::theme_success(ui.ctx())
                };
                ui.label(egui::RichText::new(i::ICON_SOURCE_PULL.codepoint).color(color));
                ui.add(
                    egui::Label::new(egui::RichText::new(&m.title).strong().size(12.5)).truncate(),
                );
            });
            let age = git_view::relative_iso(&m.created_at);
            let mut meta = format!("{} · {}", m.display_number(), m.author);
            if !age.is_empty() {
                meta.push_str(&format!(" · {age}"));
            }
            if m.is_draft {
                meta.push_str(" · draft");
            }
            ui.add(
                egui::Label::new(egui::RichText::new(meta).size(11.0).color(muted(ui))).truncate(),
            );
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let tip = if m.source_branch.is_empty() {
        m.title.clone()
    } else {
        format!("{}\n{} → {}", m.title, m.source_branch, m.target_branch)
    };
    resp.on_hover_text(tip)
}

// ─── Dialog ─────────────────────────────────────────────────────────────────

fn render_dialogs(t: &mut Tabular, ctx: &egui::Context) {
    if let Some(c) = t.git.confirm.clone() {
        let (title, msg, label) = match &c {
            SidebarConfirm::DiscardTracked(p) => (
                "Discard changes",
                format!(
                    "Discard changes in {}? This cannot be undone.",
                    describe_paths(p)
                ),
                "Discard",
            ),
            SidebarConfirm::DiscardUntracked(p) => (
                "Discard untracked files",
                format!(
                    "Remove {} from disk? This cannot be undone.",
                    describe_paths(p)
                ),
                "Remove files",
            ),
            SidebarConfirm::RemoveBranch { name, force } => (
                "Remove branch",
                if *force {
                    format!(
                        "Force-remove local branch \"{name}\"? Unmerged commits on it will be lost."
                    )
                } else {
                    format!("Remove local branch \"{name}\"?")
                },
                "Remove",
            ),
            SidebarConfirm::Checkout { name, .. } => (
                "Checkout with local changes",
                format!(
                    "You have uncommitted changes. Git carries them over to \"{name}\" when possible, or refuses the checkout if they conflict."
                ),
                "Checkout",
            ),
            SidebarConfirm::RemoveRepo(_) => (
                "Remove repository",
                "Remove this folder from the Git list? Files on disk are not touched.".to_string(),
                "Remove",
            ),
        };
        let danger = !matches!(c, SidebarConfirm::Checkout { .. });
        match git_view::confirm_dialog(ctx, "git_sidebar_confirm", title, &msg, label, danger) {
            ConfirmOutcome::Pending => {}
            ConfirmOutcome::Cancelled => t.git.confirm = None,
            ConfirmOutcome::Confirmed => {
                t.git.confirm = None;
                git_jobs::confirm_sidebar(t, c);
            }
        }
    }

    if let Some(mut dlg) = t.git.clone_dialog.clone() {
        let mut start = false;
        let mut cancel = false;
        style::render_modal_backdrop(ctx, "git_clone_backdrop", true);
        egui::Window::new("Clone repository")
            .id(egui::Id::new("git_clone_dialog"))
            .title_bar(false)
            .frame(style::modal_window_frame(ctx))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(460.0)
            .show(ctx, |ui| {
                let mut close = false;
                style::render_modal_header(ui, "Clone repository", &mut close);
                if close {
                    cancel = true;
                }
                style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.label("Repository URL");
                    let before = dlg.url.clone();
                    style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut dlg.url)
                            .hint_text("https://github.com/org/app.git or git@host:org/app.git"),
                        f32::INFINITY,
                        None,
                    );
                    // Isi tujuan otomatis selama user belum mengubahnya.
                    let auto_before = crate::git::repos::default_clone_dir(&before).to_string_lossy().to_string();
                    if dlg.url != before && (dlg.dest.is_empty() || dlg.dest == auto_before) {
                        dlg.dest = crate::git::repos::default_clone_dir(&dlg.url).to_string_lossy().to_string();
                    }
                    ui.add_space(6.0);
                    ui.label("Destination folder");
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 80.0).max(120.0);
                        style::render_text_field(ui, egui::TextEdit::singleline(&mut dlg.dest), w, None);
                        if ui.button("Browse…").clicked()
                            && let Some(dir) = crate::rfd::FileDialog::new().set_title("Parent folder").pick_folder()
                        {
                            let name = crate::git::repos::default_clone_dir(&dlg.url)
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "repository".to_string());
                            dlg.dest = dir.join(name).to_string_lossy().to_string();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Authentication uses your git credential helper or SSH key. Diagram groups and HTTP API folders that use this URL get the new folder automatically.",
                        )
                        .size(11.0)
                        .color(style::theme_muted_text(ui.ctx())),
                    );
                });
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(style::btn_primary_ctx(ui.ctx(), "Clone")).clicked() {
                        start = true;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if start {
            t.git.clone_dialog = None;
            git_jobs::start_clone(t, dlg);
        } else if cancel {
            t.git.clone_dialog = None;
        } else {
            t.git.clone_dialog = Some(dlg);
        }
    }
}

fn describe_paths(p: &[String]) -> String {
    match p {
        [one] => format!("\"{one}\""),
        many => format!("{} files", many.len()),
    }
}
