//! Sidebar tab "Git" bergaya Source Control VS Code: semua repository milik
//! project Tabular yang dipilih tampil sebagai section yang bisa dilipat.
//! Setiap section punya header (branch, ahead/behind, jumlah perubahan, aksi
//! cepat) dan sub-tab sendiri: Changes / Branches / History / Review.
//!
//! Repository dikumpulkan dari folder Diagram, HTTP API, Project, dan folder
//! yang ditambahkan di Git (lihat [`git_jobs::collect_sources`]); repository di
//! luar project aktif ada di section "Other repositories".

use eframe::egui;
use egui_icons::icons as i;

use super::git_graph_paint::{self as paint, Geometry, NodeKind};
use super::git_jobs::{self, CloneDialog, DiffSource, GitSubMenu, SidebarConfirm};
use super::git_view::{self, ConfirmOutcome};
use crate::git::history_ops::RepoState;
use crate::git::repos::{LinkKind, RepoEntry};
use crate::git::review::{MergeRequest, Provider};
use crate::git::status::FileChange;
use crate::window_egui::{PrefTab, Tabular, style};

/// Tinggi maksimum daftar di dalam satu section (sisanya di-scroll).
const LIST_MAX_H: f32 = 340.0;

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
    render_header(t, ui);
    ui.add_space(4.0);
    let (mine, others) = git_jobs::visible_repos(t);
    let project = git_jobs::active_project(t);
    egui::ScrollArea::vertical()
        .id_salt("git_sidebar_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if t.git.clone_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Cloning…").size(11.5));
                    if ui.small_button("Cancel").clicked() {
                        t.git
                            .clone_cancel
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                });
            }
            if mine.is_empty() {
                ui.add_space(6.0);
                let msg = match &project {
                    Some((_, name)) => format!(
                        "No repositories in project \"{name}\" yet. Add a local folder or clone one with the + button, or set a repository URL on the project, a diagram group, or an HTTP API folder."
                    ),
                    None => "Add a local folder, clone a repository, or set a repository on a diagram group or an HTTP API folder. They all show up here.".to_string(),
                };
                ui.label(egui::RichText::new(msg).color(muted(ui)));
            }
            for r in &mine {
                repo_section(t, ui, r, project.is_some());
            }
            if !others.is_empty() {
                ui.add_space(6.0);
                let open = t.git.show_other_repos;
                let chevron = if open {
                    i::ICON_EXPAND_MORE.codepoint
                } else {
                    i::ICON_CHEVRON_RIGHT.codepoint
                };
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "{chevron} Other repositories  {}",
                                others.len()
                            ))
                            .size(12.0)
                            .color(muted(ui)),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Repositories that are not linked to the selected project")
                    .clicked()
                {
                    t.git.show_other_repos = !open;
                }
                if open {
                    for r in &others {
                        repo_section(t, ui, r, project.is_some());
                    }
                }
            }
        });
    render_dialogs(t, ui.ctx());
}

fn render_header(t: &mut Tabular, ui: &mut egui::Ui) {
    let title = git_jobs::active_project(t)
        .map(|(_, n)| n)
        .unwrap_or_else(|| "All repositories".to_string());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(i::ICON_WORK.codepoint).size(15.0));
        // Tombol kanan dulu; judul mengisi sisa lebar (rata kiri, terpotong).
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(
                egui::RichText::new(i::ICON_ADD.codepoint).size(15.0),
                |ui| repo_menu(t, ui),
            );
            if icon_button(
                ui,
                i::ICON_REFRESH.codepoint,
                "Reload repositories and status",
                true,
            ) {
                git_jobs::reload_repos(t);
            }
            if icon_button(
                ui,
                i::ICON_SYNC.codepoint,
                "Fetch all repositories of this project",
                true,
            ) {
                git_jobs::fetch_all(t);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&title).strong().size(13.0)).truncate(),
                )
                .on_hover_text("Repositories of the project selected in the project switcher");
            });
        });
    });
}

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

/// Menu tambah repository (dipakai juga oleh tombol ➕ di bawah sidebar).
pub fn repo_menu(t: &mut Tabular, ui: &mut egui::Ui) {
    ui.set_min_width(220.0);
    if let Some((_, name)) = git_jobs::active_project(t) {
        ui.label(
            egui::RichText::new(format!("Adds to project \"{name}\""))
                .size(11.0)
                .color(muted(ui)),
        );
    }
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
    ui.separator();
    if ui.button("Git settings…").clicked() {
        ui.close();
        t.settings_active_pref_tab = PrefTab::Git;
        t.show_settings_window = true;
    }
}

// ─── Section repository ─────────────────────────────────────────────────────

fn repo_section(t: &mut Tabular, ui: &mut egui::Ui, entry: &RepoEntry, has_project: bool) {
    let key = entry.key.clone();
    render_repo_header(t, ui, entry, has_project);
    if !t.git.ui_mut(&key).expanded {
        return;
    }
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 2,
            top: 2,
            bottom: 8,
        })
        .show(ui, |ui| {
            if entry.path.is_none() {
                render_missing_folder(t, ui, entry);
                return;
            }
            let summary = link_summary(entry);
            if !summary.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("Linked to {summary}"))
                            .size(10.5)
                            .color(muted(ui)),
                    )
                    .truncate(),
                )
                .on_hover_text(summary);
            }
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
                        git_jobs::link_folder_to_items(t, &key);
                    }
                });
            }
            render_sub_nav(t, ui, &key);
            render_status_lines(t, ui, &key);
            match t.git.ui_mut(&key).sub {
                GitSubMenu::Changes => render_changes(t, ui, &key),
                GitSubMenu::Branches => render_branches(t, ui, &key),
                GitSubMenu::History => render_history(t, ui, &key),
                GitSubMenu::Review => render_review(t, ui, &key),
            }
        });
}

fn render_repo_header(t: &mut Tabular, ui: &mut egui::Ui, entry: &RepoEntry, has_project: bool) {
    let key = entry.key.clone();
    let (expanded, status, busy) = {
        let u = t.git.ui_mut(&key);
        (u.expanded, u.status.clone(), u.busy.clone())
    };
    let ctx = ui.ctx().clone();
    let h = 26.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, style::nav_track(&ctx));
    }
    let painter = ui.painter();
    let strong = style::nav_text_strong(&ctx);
    let weak = style::nav_text_muted(&ctx);
    let chevron = if expanded {
        i::ICON_EXPAND_MORE.codepoint
    } else {
        i::ICON_CHEVRON_RIGHT.codepoint
    };
    let mut x = rect.left() + 2.0;
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        chevron,
        egui::FontId::proportional(15.0),
        weak,
    );
    x += 18.0;
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        i::ICON_SOURCE_REPOSITORY.codepoint,
        egui::FontId::proportional(14.0),
        if entry.path.is_some() { strong } else { weak },
    );
    x += 20.0;

    // Tombol kanan (dari kanan ke kiri).
    let mut bx = rect.right() - 2.0;
    let mut button = |ui: &mut egui::Ui, icon: &str, tip: &str, enabled: bool| -> bool {
        let r = egui::Rect::from_min_size(
            egui::pos2(bx - 22.0, rect.top() + 2.0),
            egui::vec2(22.0, h - 4.0),
        );
        bx -= 22.0;
        ui.put(
            r,
            egui::Button::new(egui::RichText::new(icon).size(14.0)).frame(false),
        )
        .on_hover_text(tip)
        .clicked()
            && enabled
    };
    let has_path = entry.path.is_some();
    let idle = busy.is_none();
    if has_path {
        if button(ui, i::ICON_ACCOUNT_TREE.codepoint, "Open Git Graph", true) {
            super::git_graph_jobs::open_graph(t, &key);
        }
        if button(ui, i::ICON_SYNC.codepoint, "Fetch", idle) {
            git_jobs::fetch(t, &key);
        }
    }
    let right_edge = bx - 4.0;

    // Badge: jumlah perubahan, ahead/behind.
    let painter = ui.painter();
    let mut rx = right_edge;
    if let Some(label) = &busy {
        let g = painter.layout_no_wrap(format!("{label}…"), egui::FontId::proportional(10.5), weak);
        rx -= g.size().x;
        painter.galley(egui::pos2(rx, rect.center().y - g.size().y / 2.0), g, weak);
        rx -= 6.0;
    } else if let Some(s) = &status {
        let n = s.change_count();
        if n > 0 {
            let g = painter.layout_no_wrap(
                n.to_string(),
                egui::FontId::proportional(10.5),
                egui::Color32::WHITE,
            );
            let w = g.size().x + 10.0;
            let br = egui::Rect::from_center_size(
                egui::pos2(rx - w / 2.0, rect.center().y),
                egui::vec2(w, 16.0),
            );
            painter.rect_filled(br, 8.0, style::theme_info(&ctx));
            painter.galley(
                egui::pos2(
                    br.center().x - g.size().x / 2.0,
                    br.center().y - g.size().y / 2.0,
                ),
                g,
                egui::Color32::WHITE,
            );
            rx = br.left() - 6.0;
        }
        let mut sync = String::new();
        if s.behind > 0 {
            sync.push_str(&format!(
                "{}{} ",
                i::ICON_ARROW_DOWNWARD.codepoint,
                s.behind
            ));
        }
        if s.ahead > 0 {
            sync.push_str(&format!("{}{}", i::ICON_ARROW_UPWARD.codepoint, s.ahead));
        }
        if !sync.is_empty() {
            let g = painter.layout_no_wrap(
                sync.trim().to_string(),
                egui::FontId::proportional(11.0),
                weak,
            );
            rx -= g.size().x;
            painter.galley(egui::pos2(rx, rect.center().y - g.size().y / 2.0), g, weak);
            rx -= 6.0;
        }
    }

    // Nama + branch, dipotong sebelum badge.
    let clip = egui::Rect::from_x_y_ranges(x..=rx.max(x), rect.y_range());
    let p = ui.painter().with_clip_rect(clip);
    let name_g = p.layout_no_wrap(
        entry.name.clone(),
        egui::FontId::proportional(13.0),
        if has_path { strong } else { weak },
    );
    let nw = name_g.size().x;
    p.galley(
        egui::pos2(x, rect.center().y - name_g.size().y / 2.0),
        name_g,
        strong,
    );
    let branch = match (&status, has_path) {
        (Some(s), _) => s.head_label(),
        (None, false) => "not on this computer".to_string(),
        (None, true) => String::new(),
    };
    if !branch.is_empty() {
        p.text(
            egui::pos2(x + nw + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{} {branch}", i::MDI_SOURCE_BRANCH.codepoint),
            egui::FontId::proportional(11.0),
            weak,
        );
    }

    let hover = match (&entry.path, &entry.url) {
        (Some(p), Some(u)) => format!("{}\n{u}", p.display()),
        (Some(p), None) => p.display().to_string(),
        (None, Some(u)) => u.clone(),
        (None, None) => entry.key.clone(),
    };
    let resp = resp.on_hover_text(hover);
    if resp.clicked() {
        git_jobs::set_expanded(t, &key, !expanded);
    }
    resp.context_menu(|ui| repo_context_menu(t, ui, entry, has_project, status.as_ref()));
}

fn repo_context_menu(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    entry: &RepoEntry,
    has_project: bool,
    status: Option<&crate::git::status::RepoStatus>,
) {
    ui.set_min_width(220.0);
    let key = entry.key.clone();
    let idle = !t.git.is_busy(&key);
    if let Some(path) = entry.path.clone() {
        if ui.button("Open Git Graph").clicked() {
            super::git_graph_jobs::open_graph(t, &key);
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(idle, egui::Button::new("Fetch")).clicked() {
            git_jobs::fetch(t, &key);
            ui.close();
        }
        let can_pull = status.is_some_and(|s| s.upstream.is_some());
        if ui
            .add_enabled(idle && can_pull, egui::Button::new("Pull"))
            .clicked()
        {
            git_jobs::pull(t, &key);
            ui.close();
        }
        let can_push = status.is_some_and(|s| s.branch.is_some());
        if ui
            .add_enabled(idle && can_push, egui::Button::new("Push"))
            .clicked()
        {
            git_jobs::push(t, &key);
            ui.close();
        }
        ui.separator();
        if has_project
            && let Some((pid, pname)) = git_jobs::active_project(t)
            && !entry.in_project(&pid)
            && ui.button(format!("Add to project \"{pname}\"")).clicked()
        {
            git_jobs::add_to_active_project(t, &key);
            ui.close();
        }
        if entry.links_without_folder().count() > 0
            && ui.button("Use this folder for linked items").clicked()
        {
            git_jobs::link_folder_to_items(t, &key);
            ui.close();
        }
        if ui.button("Reveal in file manager").clicked() {
            if let Err(e) = crate::url_opener::open_folder(&path) {
                t.toasts.error(e);
            }
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            ui.ctx().copy_text(path.display().to_string());
            ui.close();
        }
    } else if let Some(url) = entry.url.clone()
        && ui.button("Clone…").clicked()
    {
        t.git.clone_dialog = Some(CloneDialog {
            dest: crate::git::repos::default_clone_dir(&url)
                .to_string_lossy()
                .to_string(),
            url,
            entry_key: Some(key.clone()),
        });
        ui.close();
    }
    if entry.links.iter().any(|l| l.kind == LinkKind::Manual) {
        ui.separator();
        if ui.button("Remove from list…").clicked() {
            t.git.confirm = Some((key.clone(), SidebarConfirm::RemoveRepo));
            ui.close();
        }
    }
}

fn render_missing_folder(t: &mut Tabular, ui: &mut egui::Ui, entry: &RepoEntry) {
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

fn render_sub_nav(t: &mut Tabular, ui: &mut egui::Ui, key: &str) {
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
    let current = t.git.ui_mut(key).sub;
    let id = format!("git_sub_nav_{key}");
    if let Some(k) = style::render_segmented_nav(ui, &id, &segments, current.key(), 30.0) {
        git_jobs::set_sub(t, key, GitSubMenu::from_key(k));
    }
    ui.add_space(4.0);
}

/// Operasi berjalan, state merge/rebase, dan error repository.
fn render_status_lines(t: &mut Tabular, ui: &mut egui::Ui, key: &str) {
    let (busy, state, err) = {
        let u = t.git.ui_mut(key);
        (u.busy.clone(), u.repo_state, u.status_error.clone())
    };
    if let Some(label) = busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(format!("{label}…")).size(11.5));
            if matches!(label.as_str(), "Fetch" | "Pull" | "Push")
                && ui.small_button("Cancel").clicked()
            {
                git_jobs::cancel_op(t, key);
            }
        });
    }
    if state != RepoState::Clean {
        style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
            ui.label(egui::RichText::new(state.label()).strong());
            ui.label(
                egui::RichText::new("Resolve the conflicts, stage the files, then continue.")
                    .size(11.5),
            );
            ui.horizontal(|ui| {
                if ui.button("Continue").clicked() {
                    git_jobs::continue_operation(t, key);
                }
                if ui.button("Abort…").clicked() {
                    t.git.confirm = Some((key.to_string(), SidebarConfirm::AbortOperation(state)));
                }
            });
        });
    }
    if let Some(e) = err {
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

#[allow(clippy::too_many_arguments)]
fn change_section(
    ui: &mut egui::Ui,
    key: &str,
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
    .id_salt((id, key))
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

fn render_changes(t: &mut Tabular, ui: &mut egui::Ui, key: &str) {
    let busy = t.git.is_busy(key);
    let (st, has_status, ai_busy) = {
        let u = t.git.ui_mut(key);
        (
            u.status.clone().unwrap_or_default(),
            u.status.is_some(),
            u.commit_ai_busy,
        )
    };

    // Kotak pesan commit.
    let hint_color = style::nav_text_muted(ui.ctx());
    let (submit, amend) = {
        let u = t.git.ui_mut(key);
        let resp = ui.add_sized(
            [ui.available_width(), 58.0],
            egui::TextEdit::multiline(&mut u.commit_message)
                .id(egui::Id::new(("git_commit_message", key)))
                .hint_text(egui::RichText::new("Message (Ctrl+Enter to commit)").color(hint_color)),
        );
        let submit = resp.has_focus()
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        (submit, u.amend)
    };
    ui.horizontal(|ui| {
        let label = if amend {
            "Amend commit"
        } else if st.staged.is_empty() {
            "Commit all"
        } else {
            "Commit"
        };
        let has_changes = !st.is_clean() || amend;
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
            git_jobs::commit(t, key);
        }
        ui.checkbox(&mut t.git.ui_mut(key).amend, "Amend");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ai_busy {
                ui.spinner();
            } else if icon_button(
                ui,
                i::ICON_AUTO_AWESOME.codepoint,
                "Generate commit message with AI",
                !st.is_clean(),
            ) {
                git_jobs::generate_commit_message(t, key);
            }
        });
    });
    ui.add_space(4.0);

    if st.is_clean() && has_status {
        ui.label(egui::RichText::new("No changes. Working tree clean.").color(muted(ui)));
        return;
    }
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt(("git_changes_scroll", key))
        .max_height(LIST_MAX_H)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (id, title, files, source) in [
                (
                    "git_sec_conflicts",
                    "Merge Conflicts",
                    &st.conflicted,
                    DiffSource::Conflict,
                ),
                (
                    "git_sec_staged",
                    "Staged Changes",
                    &st.staged,
                    DiffSource::Staged,
                ),
                (
                    "git_sec_changes",
                    "Changes",
                    &st.unstaged,
                    DiffSource::Worktree,
                ),
                (
                    "git_sec_untracked",
                    "Untracked",
                    &st.untracked,
                    DiffSource::Untracked,
                ),
            ] {
                change_section(ui, key, id, title, files, source, &mut actions, busy);
            }
        });
    for a in actions {
        match a {
            RowAction::Open(f, src) => git_jobs::open_file_diff(t, key, &f, src),
            RowAction::Stage(p) => git_jobs::stage(t, key, p),
            RowAction::Unstage(p) => git_jobs::unstage(t, key, p),
            RowAction::Confirm(c) => t.git.confirm = Some((key.to_string(), c)),
        }
    }
}

// ─── Branches ───────────────────────────────────────────────────────────────

fn render_branches(t: &mut Tabular, ui: &mut egui::Ui, key: &str) {
    let busy = t.git.is_busy(key);
    let can_create = !t.git.ui_mut(key).new_branch.trim().is_empty();
    let mut create = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!busy && can_create, egui::Button::new("Create"))
                .on_hover_text("Create the branch from HEAD and switch to it")
                .clicked()
            {
                create = true;
            }
            let w = (ui.available_width() - 4.0).max(60.0);
            style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut t.git.ui_mut(key).new_branch).hint_text(
                    egui::RichText::new("new-branch-name").color(style::nav_text_muted(ui.ctx())),
                ),
                w,
                Some(i::ICON_SOURCE_BRANCH_PLUS.codepoint),
            );
        });
    });
    if create {
        git_jobs::create_branch(t, key);
    }
    git_view::filter_field(ui, &mut t.git.ui_mut(key).branch_filter, "Filter branches");
    let (q, dirty, branches) = {
        let u = t.git.ui_mut(key);
        (
            u.branch_filter.trim().to_lowercase(),
            u.status
                .as_ref()
                .is_some_and(|s| !s.staged.is_empty() || !s.unstaged.is_empty()),
            u.branches.clone(),
        )
    };
    let mut action: Option<SidebarConfirm> = None;
    egui::ScrollArea::vertical()
        .id_salt(("git_branches_scroll", key))
        .max_height(LIST_MAX_H)
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
                .id_salt(("git_branch_sec", remote, key))
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
            git_jobs::checkout(t, key, name, remote)
        }
        Some(c) => t.git.confirm = Some((key.to_string(), c)),
        None => {}
    }
}

// ─── History ────────────────────────────────────────────────────────────────

fn render_history(t: &mut Tabular, ui: &mut egui::Ui, key: &str) {
    ui.horizontal(|ui| {
        if ui
            .button(format!("{} Open Git Graph", i::ICON_ACCOUNT_TREE.codepoint))
            .on_hover_text("All branches with graph, details, compare and actions")
            .clicked()
        {
            super::git_graph_jobs::open_graph(t, key);
        }
    });
    git_view::filter_field(
        ui,
        &mut t.git.ui_mut(key).history_filter,
        "Filter loaded commits",
    );
    let style_kind = t.git.store.settings.graph_style;
    let (q, commits, rows, done, loading) = {
        let u = t.git.ui_mut(key);
        (
            u.history_filter.trim().to_lowercase(),
            u.commits.clone(),
            u.commit_rows.clone(),
            u.commits_done,
            u.commits_loading,
        )
    };
    let max_lanes = rows.iter().map(|r| r.width).max().unwrap_or(1).min(6);
    let lane_w = 10.0;
    let graph_w = Geometry::width(max_lanes, lane_w);
    let mut open = None;
    let mut more = false;
    egui::ScrollArea::vertical()
        .id_salt(("git_history_scroll", key))
        .max_height(LIST_MAX_H + 60.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if commits.is_empty() {
                if loading {
                    ui.spinner();
                } else {
                    ui.label(egui::RichText::new("No commits yet.").color(muted(ui)));
                }
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let ctx = ui.ctx().clone();
            let bg = ui.visuals().panel_fill;
            for (idx, c) in commits.iter().enumerate() {
                if !q.is_empty()
                    && !c.subject.to_lowercase().contains(&q)
                    && !c.author.to_lowercase().contains(&q)
                    && !c.hash.starts_with(&q)
                {
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 36.0),
                    egui::Sense::click(),
                );
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 3.0, style::nav_track(&ctx));
                }
                // Graf mini hanya berarti tanpa filter.
                let text_x = if q.is_empty() {
                    if let Some(row) = rows.get(idx) {
                        let geo = Geometry {
                            left: rect.left(),
                            lane_w,
                            style: style_kind,
                        };
                        let clip =
                            egui::Rect::from_min_size(rect.min, egui::vec2(graph_w, rect.height()));
                        paint::paint_row(
                            &ui.painter().with_clip_rect(clip),
                            rect,
                            &geo,
                            row,
                            if idx == 0 {
                                NodeKind::Head
                            } else {
                                NodeKind::Commit
                            },
                            bg,
                        );
                    }
                    rect.left() + graph_w + 2.0
                } else {
                    rect.left() + 4.0
                };
                let clip = egui::Rect::from_x_y_ranges(text_x..=rect.right() - 2.0, rect.y_range());
                let p = ui.painter().with_clip_rect(clip);
                p.text(
                    egui::pos2(text_x, rect.top() + 10.0),
                    egui::Align2::LEFT_CENTER,
                    super::git_graph_text::plain_line(&c.subject),
                    egui::FontId::proportional(12.5),
                    style::nav_text_strong(&ctx),
                );
                let mut meta = format!(
                    "{}  {} · {}",
                    c.short,
                    c.author,
                    git_view::relative_time(c.time)
                );
                if !c.refs.is_empty() {
                    meta.push_str(&format!("  ({})", c.refs));
                }
                p.text(
                    egui::pos2(text_x, rect.top() + 26.0),
                    egui::Align2::LEFT_CENTER,
                    meta,
                    egui::FontId::proportional(10.5),
                    style::nav_text_muted(&ctx),
                );
                let resp = resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("{}\n{} <{}>", c.subject, c.author, c.email));
                if resp.clicked() {
                    open = Some(c.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button("Open in Git Graph").clicked() {
                        super::git_graph_jobs::open_graph(t, key);
                        ui.close();
                    }
                    if ui.button("Copy hash").clicked() {
                        ui.ctx().copy_text(c.hash.clone());
                        ui.close();
                    }
                    if ui.button("Copy message").clicked() {
                        ui.ctx().copy_text(c.subject.clone());
                        ui.close();
                    }
                });
            }
            if !done && !commits.is_empty() {
                ui.add_space(4.0);
                if loading {
                    ui.spinner();
                } else if ui.button("Load more").clicked() {
                    more = true;
                }
            }
        });
    if let Some(c) = open {
        git_jobs::open_commit(t, key, &c);
    }
    if more {
        git_jobs::refresh_log(t, key, false);
    }
}

// ─── Review ─────────────────────────────────────────────────────────────────

fn render_review(t: &mut Tabular, ui: &mut egui::Ui, key: &str) {
    let access = t.git.provider_access();
    if access.github_token.is_none() && access.gitlab_token.is_none() {
        style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
            ui.label(egui::RichText::new("Merge Review").strong());
            ui.label(
                "Add a GitHub or GitLab access token to list pull requests and merge requests.",
            );
            if ui.button("Set tokens…").clicked() {
                t.settings_active_pref_tab = PrefTab::Git;
                t.show_settings_window = true;
            }
        });
        return;
    }
    let mine = t.git.ui_mut(key).mrs_mine;
    let mut reload = false;
    ui.horizontal(|ui| {
        if ui.selectable_label(!mine, "This repository").clicked() && mine {
            t.git.ui_mut(key).mrs_mine = false;
            reload = t.git.ui_mut(key).mrs_loaded_at.is_none();
        }
        if ui.selectable_label(mine, "Assigned to me").clicked() && !mine {
            t.git.ui_mut(key).mrs_mine = true;
            reload = t.git.mrs_loaded_at.is_none();
        }
        let loading = if t.git.ui_mut(key).mrs_mine {
            t.git.mrs_loading
        } else {
            t.git.ui_mut(key).mrs_loading
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if loading {
                ui.spinner();
            } else if icon_button(ui, i::ICON_REFRESH.codepoint, "Refresh", true) {
                reload = true;
            }
        });
    });
    let mine = t.git.ui_mut(key).mrs_mine;
    if reload {
        git_jobs::load_mrs(t, if mine { None } else { Some(key.to_string()) });
    }
    git_view::filter_field(ui, &mut t.git.mr_filter, "Filter by title, repo, author");
    let (list_src, errors, loaded, loading) = if mine {
        (
            t.git.mrs.clone(),
            t.git.mr_errors.clone(),
            t.git.mrs_loaded_at.is_some(),
            t.git.mrs_loading,
        )
    } else {
        let u = t.git.ui_mut(key);
        (
            u.mrs.clone(),
            u.mr_errors.clone(),
            u.mrs_loaded_at.is_some(),
            u.mrs_loading,
        )
    };
    for e in &errors {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
    }
    let q = t.git.mr_filter.trim().to_lowercase();
    let list: Vec<MergeRequest> = list_src
        .into_iter()
        .filter(|m| {
            q.is_empty()
                || m.title.to_lowercase().contains(&q)
                || m.repo_full_name.to_lowercase().contains(&q)
                || m.author.to_lowercase().contains(&q)
        })
        .collect();
    if list.is_empty() && !loading && loaded {
        ui.label(egui::RichText::new("No open merge requests.").color(muted(ui)));
    }
    let mut open = None;
    egui::ScrollArea::vertical()
        .id_salt(("git_review_scroll", key))
        .max_height(LIST_MAX_H)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for provider in [Provider::GitHub, Provider::GitLab] {
                let of: Vec<&MergeRequest> =
                    list.iter().filter(|m| m.provider == provider).collect();
                if of.is_empty() {
                    continue;
                }
                let icon = match provider {
                    Provider::GitHub => i::ICON_GITHUB.codepoint,
                    Provider::GitLab => i::ICON_GITLAB.codepoint,
                };
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!("{icon} {}  {}", provider.label(), of.len()))
                        .strong()
                        .size(12.0),
                )
                .id_salt(("git_mr_provider", provider.label(), key))
                .default_open(true)
                .show(ui, |ui| {
                    let mut repos: Vec<&str> =
                        of.iter().map(|m| m.repo_full_name.as_str()).collect();
                    repos.sort_unstable();
                    repos.dedup();
                    for repo in repos {
                        if mine {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} {repo}",
                                    i::ICON_SOURCE_REPOSITORY.codepoint
                                ))
                                .size(11.5)
                                .color(muted(ui)),
                            );
                        }
                        for m in of.iter().filter(|m| m.repo_full_name == repo) {
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

/// Dialog konfirmasi sidebar dan dialog clone. Dipanggil juga oleh Git Graph
/// saat sidebar Git tidak tampil.
pub(crate) fn render_dialogs(t: &mut Tabular, ctx: &egui::Context) {
    if let Some((key, c)) = t.git.confirm.clone() {
        let repo = t
            .git
            .entry(&key)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let (title, msg, label) = match &c {
            SidebarConfirm::DiscardTracked(p) => (
                "Discard changes",
                format!(
                    "Discard changes in {} ({repo})? This cannot be undone.",
                    describe_paths(p)
                ),
                "Discard",
            ),
            SidebarConfirm::DiscardUntracked(p) => (
                "Discard untracked files",
                format!(
                    "Remove {} from disk ({repo})? This cannot be undone.",
                    describe_paths(p)
                ),
                "Remove files",
            ),
            SidebarConfirm::RemoveBranch { name, force } => (
                "Remove branch",
                if *force {
                    format!(
                        "Force-remove local branch \"{name}\" in {repo}? Unmerged commits on it will be lost."
                    )
                } else {
                    format!("Remove local branch \"{name}\" in {repo}?")
                },
                "Remove",
            ),
            SidebarConfirm::Checkout { name, .. } => (
                "Checkout with local changes",
                format!(
                    "{repo} has uncommitted changes. Git carries them over to \"{name}\" when possible, or refuses the checkout if they conflict."
                ),
                "Checkout",
            ),
            SidebarConfirm::RemoveRepo => (
                "Remove repository",
                format!("Remove {repo} from the Git list? Files on disk are not touched."),
                "Remove",
            ),
            SidebarConfirm::AbortOperation(st) => (
                "Abort operation",
                format!(
                    "{} in {repo}. Abort it and return to the state before it started?",
                    st.label()
                ),
                "Abort",
            ),
        };
        let danger = !matches!(c, SidebarConfirm::Checkout { .. });
        match git_view::confirm_dialog(ctx, "git_sidebar_confirm", title, &msg, label, danger) {
            ConfirmOutcome::Pending => {}
            ConfirmOutcome::Cancelled => t.git.confirm = None,
            ConfirmOutcome::Confirmed => {
                t.git.confirm = None;
                git_jobs::confirm_sidebar(t, &key, c);
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
                    let auto_before = crate::git::repos::default_clone_dir(&before)
                        .to_string_lossy()
                        .to_string();
                    if dlg.url != before && (dlg.dest.is_empty() || dlg.dest == auto_before) {
                        dlg.dest = crate::git::repos::default_clone_dir(&dlg.url)
                            .to_string_lossy()
                            .to_string();
                    }
                    ui.add_space(6.0);
                    ui.label("Destination folder");
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 80.0).max(120.0);
                        style::render_text_field(
                            ui,
                            egui::TextEdit::singleline(&mut dlg.dest),
                            w,
                            None,
                        );
                        if ui.button("Browse…").clicked()
                            && let Some(dir) = crate::rfd::FileDialog::new()
                                .set_title("Parent folder")
                                .pick_folder()
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
                            "Authentication uses your git credential helper or SSH key. The clone is added to the selected project, and diagram groups and HTTP API folders that use this URL get the new folder automatically.",
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
