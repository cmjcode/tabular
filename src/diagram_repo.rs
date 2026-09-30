//! UI penghubung group diagram dengan repository kode:
//! - modal "Group Repository" untuk mengisi URL git / folder lokal;
//! - jendela "Suggested tables" yang menampilkan kemajuan pemindaian dan
//!   hasil saran tabel dari [`crate::repo_scan`];
//! - popup "Search Table to Add" untuk memilih tabel diagram secara manual.
//!
//! Pemindaian sendiri berjalan di `window_egui::diagram` (butuh konfigurasi AI
//! dari `Tabular`); modul ini hanya membaca/menulis `DiagramState`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::agent::harness::ProgressStatus;
use crate::diagram_view::DiagramAction;
use crate::models::structs::{DiagramState, GroupRepoDraft};
use crate::window_egui::style;

/// Id data egui untuk pilihan "Arrange added tables inside the group".
fn arrange_pref_id() -> egui::Id {
    egui::Id::new("group_table_suggest_arrange")
}

pub(crate) fn accent_button(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .fill(style::theme_accent(ui.ctx()))
    .min_size(egui::vec2(0.0, 28.0))
}

/// Clone ke folder project yang berjalan di background, disimpan di data egui
/// selama modal repository terbuka.
#[derive(Clone)]
struct CloneTask {
    dest: String,
    /// `None` selama berjalan.
    result: Arc<Mutex<Option<Result<(), String>>>>,
    cancel: Arc<AtomicBool>,
}

fn clone_task_id(key: &str) -> egui::Id {
    egui::Id::new(("group_repo_clone", key))
}

fn start_clone(ctx: &egui::Context, key: &str, url: String, dest: String) {
    let task = CloneTask {
        dest: dest.clone(),
        result: Arc::new(Mutex::new(None)),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let (result, cancel, repaint) = (task.result.clone(), task.cancel.clone(), ctx.clone());
    std::thread::spawn(move || {
        let path = crate::repo_scan::expand_home(&dest);
        let r = crate::repo_scan::clone_into(&url, &path, &cancel).map_err(|e| e.to_string());
        if let Ok(mut slot) = result.lock() {
            *slot = Some(r);
        }
        repaint.request_repaint();
    });
    ctx.data_mut(|d| d.insert_temp(clone_task_id(key), task));
}

/// Isi ulang URL dari `.git/config` folder bila URL masih kosong atau hasil
/// deteksi sebelumnya (belum diubah user).
fn autofill_url(draft: &mut GroupRepoDraft) {
    if !(draft.url.trim().is_empty() || draft.url_auto) {
        return;
    }
    let path = draft.path.trim();
    let detected = (!path.is_empty())
        .then(|| crate::repo_scan::git_remote_url(&crate::repo_scan::expand_home(path)))
        .flatten();
    match detected {
        Some(url) => {
            draft.url = url;
            draft.url_auto = true;
        }
        None if draft.url_auto => {
            draft.url.clear();
            draft.url_auto = false;
        }
        None => {}
    }
}

/// Status clone "Clone into this folder" untuk satu modal repository
/// (group diagram atau folder HTTP API). `key` membedakan modal.
pub(crate) struct RepoCloneState {
    key: String,
    task: Option<CloneTask>,
    pub cloning: bool,
    last_error: Option<String>,
}

impl RepoCloneState {
    /// Baca status clone; clone yang selesai mengisi folder draft. Pesan info
    /// dikembalikan bila clone baru saja berhasil.
    pub(crate) fn poll(
        ctx: &egui::Context,
        key: &str,
        draft: &mut GroupRepoDraft,
    ) -> (Self, Option<String>) {
        let task_id = clone_task_id(key);
        let task: Option<CloneTask> = ctx.data(|d| d.get_temp(task_id));
        let task_result = task
            .as_ref()
            .and_then(|t| t.result.lock().ok().and_then(|r| r.clone()));
        let cloning = task.is_some() && task_result.is_none();
        let mut info = None;
        if let (Some(t), Some(r)) = (&task, task_result) {
            ctx.data_mut(|d| d.remove::<CloneTask>(task_id));
            match r {
                Ok(()) => {
                    draft.path = t.dest.clone();
                    autofill_url(draft);
                    info = Some(format!("Cloned into {}", t.dest));
                }
                Err(e) if e == "Cancelled" => {}
                Err(e) => {
                    ctx.data_mut(|d| d.insert_temp(task_id.with("error"), e));
                }
            }
        }
        let last_error: Option<String> = ctx.data(|d| d.get_temp(task_id.with("error")));
        (
            Self {
                key: key.to_string(),
                task,
                cloning,
                last_error,
            },
            info,
        )
    }

    /// Modal ditutup: hentikan clone yang masih berjalan dan buang status.
    pub(crate) fn finish(&self, ctx: &egui::Context) {
        if let Some(t) = &self.task
            && self.cloning
        {
            t.cancel.store(true, Ordering::SeqCst);
        }
        let task_id = clone_task_id(&self.key);
        ctx.data_mut(|d| {
            d.remove::<CloneTask>(task_id);
            d.remove::<String>(task_id.with("error"));
        });
    }
}

/// Validasi isi field repository.
pub(crate) struct RepoFieldsState {
    pub path_empty: bool,
    pub url_empty: bool,
    pub url_ok: bool,
}

impl RepoFieldsState {
    /// Boleh disimpan: ada folder atau URL, URL valid, dan tidak sedang clone.
    pub(crate) fn valid(&self, clone: &RepoCloneState) -> bool {
        (!self.path_empty || !self.url_empty) && self.url_ok && !clone.cloning
    }
}

/// Kartu "PROJECT FOLDER" + "GIT URL" yang dipakai modal repository group
/// diagram dan folder HTTP API. `shared_with` menjelaskan ke mana URL ikut
/// disimpan (mis. "the diagram").
pub(crate) fn render_repo_fields(
    ui: &mut egui::Ui,
    draft: &mut GroupRepoDraft,
    clone: &RepoCloneState,
    shared_with: &str,
) -> RepoFieldsState {
    let path_empty = draft.path.trim().is_empty();
    let url_empty = draft.url.trim().is_empty();
    let folder = crate::repo_scan::expand_home(draft.path.trim());
    let folder_exists = !path_empty && folder.is_dir();
    let url_parsed = crate::repo_scan::RepoSource::parse(&draft.url);
    let url_has_secret = crate::repo_scan::has_embedded_credentials(&draft.url);
    let url_ok = url_empty || (url_parsed.is_ok() && !url_has_secret);
    let task_id = clone_task_id(&clone.key);

    style::modal_card_frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("PROJECT FOLDER")
                    .small()
                    .strong()
                    .weak(),
            );
            ui.label(
                egui::RichText::new("personal, only on this computer")
                    .small()
                    .italics()
                    .weak(),
            );
        });
        ui.horizontal(|ui| {
            let browse_w = if cfg!(target_os = "ios") { 0.0 } else { 86.0 };
            let edit = style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut draft.path).hint_text("/path/to/project"),
                ui.available_width() - browse_w,
                Some(egui_icons::icons::ICON_FOLDER.codepoint),
            );
            if edit.changed() {
                autofill_url(draft);
            }
            #[cfg(not(target_os = "ios"))]
            if ui
                .add(egui::Button::new("Browse…").min_size(egui::vec2(0.0, 28.0)))
                .clicked()
            {
                let mut dialog =
                    crate::rfd::FileDialog::new().set_title("Select the project folder");
                if folder_exists {
                    dialog = dialog.set_directory(&folder);
                }
                if let Some(dir) = dialog.pick_folder() {
                    draft.path = dir.to_string_lossy().to_string();
                    autofill_url(draft);
                }
            }
        });
        if !path_empty {
            let (msg, color) = if folder_exists {
                (
                    "Folder found. Scans read it in place, including uncommitted changes.",
                    ui.visuals().weak_text_color(),
                )
            } else if matches!(url_parsed, Ok(crate::repo_scan::RepoSource::Remote(_))) {
                (
                    "Folder not found on this computer. Clone it here, or leave it: \
                     scans then use a private clone of the git URL.",
                    ui.visuals().warn_fg_color,
                )
            } else {
                (
                    "Folder not found on this computer. Add a git URL to clone it.",
                    ui.visuals().warn_fg_color,
                )
            };
            ui.label(egui::RichText::new(msg).small().color(color));

            let can_clone = !folder_exists
                && !cfg!(target_os = "ios")
                && matches!(url_parsed, Ok(crate::repo_scan::RepoSource::Remote(_)));
            if clone.cloning {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(egui::RichText::new("Cloning…").small());
                    if ui.small_button("Cancel").clicked()
                        && let Some(t) = &clone.task
                    {
                        t.cancel.store(true, Ordering::SeqCst);
                    }
                });
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            } else if can_clone {
                if let Some(e) = &clone.last_error {
                    ui.label(
                        egui::RichText::new(e)
                            .small()
                            .color(ui.visuals().error_fg_color),
                    );
                }
                if ui
                    .add(
                        egui::Button::new(format!(
                            "{} Clone into this folder",
                            egui_icons::icons::MDI_GIT.codepoint
                        ))
                        .min_size(egui::vec2(0.0, 26.0)),
                    )
                    .on_hover_text(format!(
                        "git clone {} {}",
                        crate::repo_scan::redact(draft.url.trim()),
                        draft.path.trim()
                    ))
                    .clicked()
                {
                    ui.ctx()
                        .data_mut(|d| d.remove::<String>(task_id.with("error")));
                    start_clone(
                        ui.ctx(),
                        &clone.key,
                        draft.url.trim().to_string(),
                        draft.path.trim().to_string(),
                    );
                }
            }
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("GIT URL").small().strong().weak());
            ui.label(
                egui::RichText::new(format!("shared with {shared_with}"))
                    .small()
                    .italics()
                    .weak(),
            );
            if draft.url_auto && !url_empty {
                ui.label(
                    egui::RichText::new("detected from .git/config")
                        .small()
                        .italics()
                        .weak(),
                );
            }
        });
        ui.horizontal(|ui| {
            let detect_w = if folder_exists { 76.0 } else { 0.0 };
            let edit = style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut draft.url)
                    .hint_text("https://github.com/org/app.git"),
                ui.available_width() - detect_w,
                Some(egui_icons::icons::MDI_GIT.codepoint),
            );
            if edit.changed() {
                draft.url_auto = false;
            }
            if folder_exists
                && ui
                    .add(egui::Button::new("Detect").min_size(egui::vec2(0.0, 28.0)))
                    .on_hover_text("Read the remote URL from the folder's .git/config")
                    .clicked()
            {
                draft.url.clear();
                draft.url_auto = true;
                autofill_url(draft);
            }
        });
        if !url_empty {
            let (msg, color) = match &url_parsed {
                Err(e) => (e.to_string(), ui.visuals().error_fg_color),
                Ok(_) if url_has_secret => (
                    format!(
                        "Remove the password or token from the URL. It is shared with \
                         everyone who opens {shared_with}; use SSH keys or a git credential \
                         helper instead."
                    ),
                    ui.visuals().error_fg_color,
                ),
                Ok(crate::repo_scan::RepoSource::Remote(_)) => (
                    "Cloned with depth 1 when needed; private repositories use your \
                     git credentials."
                        .to_string(),
                    ui.visuals().weak_text_color(),
                ),
                Ok(crate::repo_scan::RepoSource::Local(_)) => (
                    "This looks like a folder path. Put it in Project folder instead.".to_string(),
                    ui.visuals().warn_fg_color,
                ),
            };
            ui.label(egui::RichText::new(msg).small().color(color));
        } else if folder_exists {
            ui.label(
                egui::RichText::new("No git remote found in this folder.")
                    .small()
                    .weak(),
            );
        }
    });

    RepoFieldsState {
        path_empty,
        url_empty,
        url_ok,
    }
}

/// Modal pengaturan repository sebuah group: folder project dan/atau URL git.
pub fn render_group_repo_editor(
    ctx: &egui::Context,
    state: &mut DiagramState,
) -> Option<DiagramAction> {
    let mut draft = state.group_repo_editor.take()?;
    let gid = draft.group_id.clone();
    let Some(group) = state.groups.iter().find(|g| g.id == gid) else {
        return None; // group sudah dihapus
    };
    let group_title = group.title.clone();
    let had_repo = group.has_repository();

    let mut close = false;
    let mut save = false;
    let mut scan_after_save = false;
    let mut remove = false;

    let (clone, info) = RepoCloneState::poll(ctx, &gid, &mut draft);
    let result_action = info.map(DiagramAction::Info);

    style::render_modal_backdrop(ctx, "group_repo_editor_backdrop", true);
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(340.0, 560.0);

    egui::Window::new("Group Repository")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            style::render_modal_header(ui, format!("Repository for {group_title}"), &mut close);
            ui.label(
                egui::RichText::new(
                    "Link the code that uses this group's tables. The git URL is saved with the \
                     diagram and shared with everyone who opens it. The project folder is \
                     personal: it stays on this computer only. Scans use your folder when it \
                     exists, otherwise a private clone of the git URL. HTTP API folders with \
                     the same git URL are linked to this group.",
                )
                .weak()
                .small(),
            );
            ui.add_space(8.0);

            let fields = render_repo_fields(ui, &mut draft, &clone, "the diagram");

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if had_repo
                    && ui
                        .add(egui::Button::new("Remove").min_size(egui::vec2(0.0, 28.0)))
                        .on_hover_text("Unlink the folder and URL from this group")
                        .clicked()
                {
                    remove = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let valid = fields.valid(&clone);
                    if ui
                        .add_enabled(valid, accent_button(ui, "Save & Suggest Tables"))
                        .clicked()
                    {
                        save = true;
                        scan_after_save = true;
                    }
                    if ui
                        .add_enabled(
                            valid,
                            egui::Button::new("Save").min_size(egui::vec2(0.0, 28.0)),
                        )
                        .clicked()
                    {
                        save = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 28.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        });

    if close || save || remove {
        clone.finish(ctx);
    }
    if save || remove {
        let clean = |v: &str| {
            (!remove)
                .then(|| v.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        // Folder: personal, hanya di komputer ini. URL: ikut diagram (bersama).
        if let Err(e) =
            crate::diagram_repo_paths::set_local_repo_path(&gid, clean(&draft.path).as_deref())
        {
            return Some(DiagramAction::Error(e));
        }
        let url = clean(&draft.url);
        if let Some(g) = state.groups.iter_mut().find(|g| g.id == gid)
            && g.repo_url != url
        {
            g.repo_url = url;
            state.save_requested = true;
        }
        if scan_after_save {
            return Some(DiagramAction::SuggestGroupTables(gid));
        }
        return result_action;
    }
    if !close {
        state.group_repo_editor = Some(draft);
    }
    result_action
}

/// Jendela kemajuan + hasil saran tabel untuk sebuah group.
pub fn render_group_table_suggestions(
    ctx: &egui::Context,
    state: &mut DiagramState,
) -> Option<DiagramAction> {
    let mut sugg = state.group_table_suggestions.take()?;
    let gid = sugg.group_id.clone();
    let Some(group) = state.groups.iter().find(|g| g.id == gid) else {
        return None; // group dihapus: job ikut dibatalkan oleh poller
    };
    let repo_label = crate::repo_scan::choose_source(
        group.local_repo_path().as_deref(),
        group.shared_repo_url(),
    )
    .map(|src| match src {
        crate::repo_scan::RepoSource::Local(p) => p.display().to_string(),
        crate::repo_scan::RepoSource::Remote(u) => crate::repo_scan::redact(&u),
    })
    .unwrap_or_else(|_| "the linked repository".to_string());
    let members: HashSet<String> = state
        .nodes
        .iter()
        .filter(|n| n.is_in_group(&gid))
        .map(|n| n.id.clone())
        .collect();

    let mut close = false;
    let mut result = None;
    let mut add_ids: Option<Vec<String>> = None;
    let mut arrange: bool = ctx.data(|d| d.get_temp(arrange_pref_id())).unwrap_or(true);

    style::render_modal_backdrop(ctx, "group_table_suggestions_backdrop", true);
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(380.0, 720.0);
    let list_h = (screen.height() * 0.5).clamp(240.0, 480.0);

    egui::Window::new("Suggested tables")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            style::render_modal_header(
                ui,
                format!("Suggested tables for {}", sugg.group_title),
                &mut close,
            );
            ui.label(
                egui::RichText::new(format!(
                    "Tables from this diagram that the code in {repo_label} uses."
                ))
                .weak()
                .small(),
            );
            ui.add_space(8.0);

            if sugg.running {
                render_progress(ui, &sugg, true);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if sugg.cancel_requested {
                            "Cancelling…"
                        } else {
                            "Cancel"
                        };
                        if ui
                            .add_enabled(
                                !sugg.cancel_requested,
                                egui::Button::new(label).min_size(egui::vec2(0.0, 28.0)),
                            )
                            .clicked()
                        {
                            sugg.cancel_requested = true;
                        }
                    });
                });
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }

            if let Some(err) = &sugg.error {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {err}",
                        egui_icons::icons::ICON_ERROR.codepoint
                    ))
                    .color(ui.visuals().error_fg_color),
                );
                ui.add_space(4.0);
            }
            if let Some(note) = &sugg.note {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {note}",
                        egui_icons::icons::ICON_INFO.codepoint
                    ))
                    .small()
                    .color(ui.visuals().warn_fg_color),
                );
                ui.add_space(4.0);
            }

            let selectable: Vec<usize> = sugg
                .items
                .iter()
                .enumerate()
                .filter(|(_, (s, _))| !members.contains(&s.id))
                .map(|(i, _)| i)
                .collect();
            let chosen = selectable.iter().filter(|&&i| sugg.items[i].1).count();

            style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{chosen} of {} selected · {} already in group",
                            selectable.len(),
                            sugg.items.len() - selectable.len()
                        ))
                        .weak()
                        .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(chosen > 0, egui::Button::new("Select none").small())
                            .clicked()
                        {
                            for &i in &selectable {
                                sugg.items[i].1 = false;
                            }
                        }
                        if ui
                            .add_enabled(
                                chosen < selectable.len(),
                                egui::Button::new("Select all").small(),
                            )
                            .clicked()
                        {
                            for &i in &selectable {
                                sugg.items[i].1 = true;
                            }
                        }
                    });
                });
                ui.separator();

                if sugg.items.is_empty() {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 80.0),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "No tables from this diagram were found in the repository.",
                                )
                                .italics()
                                .weak(),
                            );
                        },
                    );
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(list_h)
                    // tinggi minimal saat bisa di-scroll: tanpa ini ScrollArea
                    // menyusut ke sisa tinggi window yang fixed_size-nya 0
                    .min_scrolled_height(list_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (s, on) in sugg.items.iter_mut() {
                            render_suggestion_row(ui, s, on, members.contains(&s.id));
                            ui.add_space(2.0);
                        }
                    });
            });

            if !sugg.unknown.is_empty() {
                ui.add_space(6.0);
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!(
                        "Mentioned in code but not in this diagram ({})",
                        sugg.unknown.len()
                    ))
                    .small()
                    .weak(),
                )
                .id_salt("group_suggest_unknown")
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(sugg.unknown.join(", ")).small().weak());
                });
            }
            if !sugg.progress.is_empty() {
                egui::CollapsingHeader::new(egui::RichText::new("Scan details").small().weak())
                    .default_open(sugg.error.is_some() || sugg.note.is_some())
                    .id_salt("group_suggest_details")
                    .show(ui, |ui| render_progress(ui, &sugg, false));
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut arrange, "Arrange added tables in the group")
                    .on_hover_text(
                        "Move the added tables next to the group. Tables that already belong \
                         to another group stay where they are.",
                    );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            chosen > 0,
                            accent_button(ui, format!("Add {chosen} table(s)")),
                        )
                        .clicked()
                    {
                        add_ids = Some(
                            selectable
                                .iter()
                                .filter(|&&i| sugg.items[i].1)
                                .map(|&i| sugg.items[i].0.id.clone())
                                .collect(),
                        );
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("Rescan").min_size(egui::vec2(0.0, 28.0)))
                        .on_hover_text("Fetch the repository again and repeat the analysis")
                        .clicked()
                    {
                        result = Some(DiagramAction::SuggestGroupTables(gid.clone()));
                    }
                    if ui
                        .add(egui::Button::new("Close").min_size(egui::vec2(0.0, 28.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        });

    ctx.data_mut(|d| d.insert_temp(arrange_pref_id(), arrange));

    if let Some(ids) = add_ids {
        let added = add_tables_to_group(state, &gid, &ids, arrange);
        result = Some(DiagramAction::Info(format!(
            "Added {added} table(s) to {}",
            sugg.group_title
        )));
    }
    if !close {
        state.group_table_suggestions = Some(sugg);
    }
    result
}

/// Satu baris kandidat di popup "Search Table to Add".
struct PickerRow {
    id: String,
    title: String,
    /// Judul group lain tempat tabel ini sudah tergabung.
    other_groups: Vec<String>,
    is_member: bool,
}

/// Cocokkan `query` (tanpa beda huruf besar/kecil) dengan judul atau id tabel.
fn picker_matches(query: &str, title: &str, id: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || title.to_lowercase().contains(&q) || id.to_lowercase().contains(&q)
}

/// Popup "Search Table to Add": daftar seluruh tabel diagram yang bisa dicari,
/// lalu tabel yang dicentang dimasukkan ke group. Enter tanpa centangan
/// menambahkan hasil teratas.
pub fn render_group_table_picker(
    ctx: &egui::Context,
    state: &mut DiagramState,
) -> Option<DiagramAction> {
    let mut picker = state.group_table_picker.take()?;
    let gid = picker.group_id.clone();
    if !state.groups.iter().any(|g| g.id == gid) {
        return None; // group sudah dihapus
    }

    let group_titles: std::collections::HashMap<&str, &str> = state
        .groups
        .iter()
        .map(|g| (g.id.as_str(), g.title.as_str()))
        .collect();
    // Tabel milik database yang di-link mengikuti diagram sumbernya.
    let mut rows: Vec<PickerRow> = state
        .nodes
        .iter()
        .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
        .filter(|n| picker_matches(&picker.query, &n.title, &n.id))
        .map(|n| {
            let mut other_groups: Vec<String> = n
                .group_ids
                .iter()
                .chain(n.group_id.as_ref())
                .filter(|g| **g != gid)
                .filter_map(|g| group_titles.get(g.as_str()).map(|t| t.to_string()))
                .collect();
            other_groups.dedup();
            PickerRow {
                id: n.id.clone(),
                title: n.title.clone(),
                other_groups,
                is_member: n.is_in_group(&gid),
            }
        })
        .collect();
    rows.sort_by_key(|r| (r.is_member, r.title.to_lowercase()));
    let total_tables = state
        .nodes
        .iter()
        .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
        .count();
    let shown_selectable: Vec<&str> = rows
        .iter()
        .filter(|r| !r.is_member)
        .map(|r| r.id.as_str())
        .collect();

    let mut close = false;
    let mut add_ids: Option<Vec<String>> = None;
    let mut arrange: bool = ctx.data(|d| d.get_temp(arrange_pref_id())).unwrap_or(true);

    style::render_modal_backdrop(ctx, "group_table_picker_backdrop", true);
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(340.0, 520.0);
    let list_h = (screen.height() * 0.5).clamp(200.0, 420.0);

    egui::Window::new("Search Table to Add")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            style::render_modal_header(
                ui,
                format!("Add tables to {}", picker.group_title),
                &mut close,
            );
            ui.add_space(6.0);

            let search = style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut picker.query).hint_text("Search tables…"),
                f32::INFINITY,
                Some(egui_icons::icons::ICON_SEARCH.codepoint),
            );
            if ui.memory(|m| m.focused().is_none()) {
                search.request_focus();
            }
            ui.add_space(6.0);

            style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let chosen = picker.selected.len();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{chosen} selected · {} of {total_tables} table(s) shown",
                            rows.len()
                        ))
                        .weak()
                        .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(chosen > 0, egui::Button::new("Clear").small())
                            .clicked()
                        {
                            picker.selected.clear();
                        }
                        let all_on = shown_selectable
                            .iter()
                            .all(|id| picker.selected.contains(*id));
                        if ui
                            .add_enabled(
                                !shown_selectable.is_empty() && !all_on,
                                egui::Button::new("Select shown").small(),
                            )
                            .clicked()
                        {
                            picker
                                .selected
                                .extend(shown_selectable.iter().map(|id| id.to_string()));
                        }
                    });
                });
                ui.separator();

                if rows.is_empty() {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 80.0),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            let msg = if total_tables == 0 {
                                "This diagram has no tables yet."
                            } else {
                                "No tables match your search."
                            };
                            ui.label(egui::RichText::new(msg).italics().weak());
                        },
                    );
                    return;
                }

                let weak = ui.visuals().weak_text_color();
                let row_h = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical()
                    .max_height(list_h)
                    .min_scrolled_height(list_h)
                    .auto_shrink([false, true])
                    .show_rows(ui, row_h, rows.len(), |ui, range| {
                        for r in &rows[range] {
                            ui.horizontal(|ui| {
                                ui.set_min_height(row_h);
                                if r.is_member {
                                    let mut always = true;
                                    ui.add_enabled(
                                        false,
                                        egui::Checkbox::new(&mut always, &r.title),
                                    )
                                    .on_disabled_hover_text("Already in this group");
                                    ui.label(
                                        egui::RichText::new("in group")
                                            .small()
                                            .italics()
                                            .color(weak),
                                    );
                                } else {
                                    let mut on = picker.selected.contains(&r.id);
                                    if ui.checkbox(&mut on, &r.title).changed() {
                                        if on {
                                            picker.selected.insert(r.id.clone());
                                        } else {
                                            picker.selected.remove(&r.id);
                                        }
                                    }
                                }
                                if !r.other_groups.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "also in {}",
                                                r.other_groups.join(", ")
                                            ))
                                            .small()
                                            .color(weak),
                                        )
                                        .truncate(),
                                    );
                                }
                            });
                        }
                    });
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut arrange, "Arrange added tables in the group")
                    .on_hover_text(
                        "Move the added tables next to the group. Tables that already belong \
                         to another group stay where they are.",
                    );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let chosen = picker.selected.len();
                    if ui
                        .add_enabled(
                            chosen > 0,
                            accent_button(ui, format!("Add {chosen} table(s)")),
                        )
                        .clicked()
                    {
                        add_ids = Some(picker.selected.iter().cloned().collect());
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 28.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });

            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && add_ids.is_none() {
                add_ids = if picker.selected.is_empty() {
                    shown_selectable.first().map(|id| vec![id.to_string()])
                } else {
                    Some(picker.selected.iter().cloned().collect())
                };
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

    ctx.data_mut(|d| d.insert_temp(arrange_pref_id(), arrange));

    let mut result = None;
    if let Some(ids) = add_ids {
        let added = add_tables_to_group(state, &gid, &ids, arrange);
        result = Some(DiagramAction::Info(format!(
            "Added {added} table(s) to {}",
            picker.group_title
        )));
        close = true;
    }
    if !close {
        state.group_table_picker = Some(picker);
    }
    result
}

/// Format durasi singkat: "42s" atau "3m 05s".
fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Tanpa event baru selama ini, tampilkan peringatan kemungkinan macet.
const STALL_WARNING: std::time::Duration = std::time::Duration::from_secs(45);

/// Jumlah langkah agent terakhir yang ditampilkan selama berjalan.
const LIVE_AGENT_STEPS: usize = 6;

fn step_icon(ui: &mut egui::Ui, status: ProgressStatus, live: bool) {
    match status {
        ProgressStatus::Active if live => {
            ui.add(egui::Spinner::new().size(14.0));
        }
        ProgressStatus::Active => {
            ui.label(egui::RichText::new("•").color(ui.visuals().weak_text_color()));
        }
        ProgressStatus::Done => {
            ui.label(
                egui_icons::icons::ICON_CHECK
                    .rich_text()
                    .color(egui::Color32::from_rgb(80, 180, 110)),
            );
        }
        ProgressStatus::Error => {
            ui.label(
                egui_icons::icons::ICON_ERROR
                    .rich_text()
                    .color(ui.visuals().error_fg_color),
            );
        }
    }
}

fn step_row(
    ui: &mut egui::Ui,
    step: &crate::agent::harness::ProgressStep,
    live: bool,
    indent: f32,
) {
    let weak = ui.visuals().weak_text_color();
    ui.horizontal(|ui| {
        ui.add_space(indent);
        step_icon(ui, step.status, live);
        let desc = egui::RichText::new(&step.description);
        let desc = if step.status == ProgressStatus::Done && indent > 0.0 {
            desc.color(weak)
        } else {
            desc
        };
        ui.add(egui::Label::new(desc).truncate());
        if let Some(detail) = &step.detail {
            let color = if step.status == ProgressStatus::Error {
                ui.visuals().error_fg_color
            } else {
                weak
            };
            ui.add(egui::Label::new(egui::RichText::new(detail).small().color(color)).truncate())
                .on_hover_text(detail);
        }
    });
}

/// Tahapan pemindaian (langkah pemindai + langkah agent di bawahnya) dan,
/// selama berjalan, baris status dengan durasi dan peringatan macet.
fn render_progress(
    ui: &mut egui::Ui,
    sugg: &crate::models::structs::GroupTableSuggestions,
    live: bool,
) {
    render_job_progress(
        ui,
        &sugg.progress,
        sugg.started_at,
        sugg.last_activity_at,
        sugg.elapsed,
        live,
    );
}

/// Daftar langkah kemajuan job repository (pemindai + langkah agent AI).
pub(crate) fn render_job_progress(
    ui: &mut egui::Ui,
    steps: &[crate::agent::harness::ProgressStep],
    started_at: Option<std::time::Instant>,
    last_activity_at: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    live: bool,
) {
    let weak = ui.visuals().weak_text_color();
    let (scan_steps, agent_steps): (Vec<_>, Vec<_>) = steps
        .iter()
        .partition(|s| s.tool_name.as_deref() == Some("repo_scan"));
    let agent_done = agent_steps
        .iter()
        .filter(|s| s.status == ProgressStatus::Done)
        .count();
    let agent_failed = agent_steps
        .iter()
        .filter(|s| s.status == ProgressStatus::Error)
        .count();

    if live {
        let now = std::time::Instant::now();
        let running_for = started_at.map(|t| now - t).unwrap_or_default();
        let idle = last_activity_at
            .or(started_at)
            .map(|t| now - t)
            .unwrap_or_default();
        let mut summary = format!("Working · {}", format_duration(running_for));
        if !agent_steps.is_empty() {
            summary.push_str(&format!(
                " · AI used {} tool(s), {agent_done} finished",
                agent_steps.len()
            ));
            if agent_failed > 0 {
                summary.push_str(&format!(", {agent_failed} failed"));
            }
        }
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(egui::RichText::new(summary).strong());
        });
        if idle >= STALL_WARNING {
            ui.label(
                egui::RichText::new(format!(
                    "{} No new activity for {}. The AI may still be thinking; cancel if it stays stuck.",
                    egui_icons::icons::ICON_WARNING.codepoint,
                    format_duration(idle)
                ))
                .small()
                .color(ui.visuals().warn_fg_color),
            );
        }
        ui.add_space(6.0);
        if steps.is_empty() {
            ui.label(egui::RichText::new("Starting…").weak());
            return;
        }
    } else if let Some(d) = elapsed {
        let mut summary = format!("Finished in {}", format_duration(d));
        if !agent_steps.is_empty() {
            summary.push_str(&format!(" · AI used {} tool(s)", agent_steps.len()));
            if agent_failed > 0 {
                summary.push_str(&format!(", {agent_failed} failed"));
            }
        }
        ui.label(egui::RichText::new(summary).small().color(weak));
    }

    // Langkah agent ditampilkan di bawah langkah pemindai yang aktif terakhir
    // (biasanya "Asking … to review the code").
    let anchor = scan_steps
        .iter()
        .rposition(|s| s.status == ProgressStatus::Active)
        .or_else(|| scan_steps.len().checked_sub(1));
    for (i, step) in scan_steps.iter().enumerate() {
        step_row(ui, step, live, 0.0);
        if Some(i) != anchor || agent_steps.is_empty() {
            continue;
        }
        let skip = if live {
            agent_steps.len().saturating_sub(LIVE_AGENT_STEPS)
        } else {
            0
        };
        if skip > 0 {
            let failed_hidden = agent_steps[..skip]
                .iter()
                .filter(|s| s.status == ProgressStatus::Error)
                .count();
            let mut text = format!("{skip} earlier step(s)");
            if failed_hidden > 0 {
                text.push_str(&format!(", {failed_hidden} failed"));
            }
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.label(egui::RichText::new(text).small().italics().color(weak));
            });
        }
        for s in &agent_steps[skip..] {
            step_row(ui, s, live, 22.0);
        }
    }
    // Tanpa langkah pemindai (seharusnya tidak terjadi): tampilkan langkah agent saja.
    if scan_steps.is_empty() {
        let skip = if live {
            agent_steps.len().saturating_sub(LIVE_AGENT_STEPS)
        } else {
            0
        };
        for s in &agent_steps[skip..] {
            step_row(ui, s, live, 0.0);
        }
    }
}

fn render_suggestion_row(
    ui: &mut egui::Ui,
    s: &crate::repo_scan::TableSuggestion,
    on: &mut bool,
    is_member: bool,
) {
    let weak = ui.visuals().weak_text_color();
    let pct = (s.confidence * 100.0).round();
    let pct_color = if pct >= 80.0 {
        egui::Color32::from_rgb(80, 180, 110)
    } else if pct >= 50.0 {
        egui::Color32::from_rgb(210, 160, 60)
    } else {
        weak
    };
    ui.horizontal(|ui| {
        if is_member {
            let mut always = true;
            ui.add_enabled(false, egui::Checkbox::without_text(&mut always))
                .on_disabled_hover_text("Already in this group");
        } else {
            ui.checkbox(on, "");
        }
        let title = egui::RichText::new(&s.title).strong();
        ui.label(if is_member { title.color(weak) } else { title });
        let (badge, tip) = if s.from_ai {
            ("AI", "Confirmed by the AI review")
        } else {
            (
                "TEXT",
                "Found by text search only; the AI did not confirm it",
            )
        };
        ui.label(egui::RichText::new(badge).small().strong().color(weak))
            .on_hover_text(tip);
        if is_member {
            ui.label(
                egui::RichText::new("in group")
                    .small()
                    .italics()
                    .color(weak),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{pct:.0}%"))
                    .small()
                    .strong()
                    .color(pct_color),
            );
        });
    });
    ui.indent(("group_suggest_row", &s.id), |ui| {
        ui.label(egui::RichText::new(&s.reason).small().color(weak));
        if !s.evidence.is_empty() {
            let shown: Vec<&str> = s.evidence.iter().take(3).map(String::as_str).collect();
            let more = s.evidence.len().saturating_sub(shown.len());
            let mut text = shown.join("   ");
            if more > 0 {
                text.push_str(&format!("   +{more}"));
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .small()
                        .family(egui::FontFamily::Monospace)
                        .color(weak),
                )
                .truncate(),
            )
            .on_hover_text(s.evidence.join("\n"));
        }
    });
}

/// Tambahkan tabel `ids` ke group `gid`. Bila `arrange`, tabel baru yang
/// belum punya group lain disusun dalam grid tepat di bawah isi group (atau
/// di posisi group kosong). Mengembalikan jumlah tabel yang benar-benar baru.
pub fn add_tables_to_group(
    state: &mut DiagramState,
    gid: &str,
    ids: &[String],
    arrange: bool,
) -> usize {
    let Some(group) = state.groups.iter().find(|g| g.id == gid) else {
        return 0;
    };
    let manual_pos = group.manual_pos;
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();

    // Batas isi group saat ini.
    let mut extent: Option<egui::Rect> = None;
    for n in state.nodes.iter().filter(|n| n.is_in_group(gid)) {
        let r = egui::Rect::from_min_size(n.pos, n.size);
        extent = Some(extent.map_or(r, |e| e.union(r)));
    }
    let anchor = match (extent, manual_pos) {
        (Some(e), _) => egui::pos2(e.min.x, e.max.y + 60.0),
        (None, Some(p)) => p + egui::vec2(20.0, 50.0),
        (None, None) => egui::pos2(0.0, 0.0),
    };

    let new_idx: Vec<usize> = state
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| wanted.contains(n.id.as_str()) && !n.is_in_group(gid))
        .map(|(i, _)| i)
        .collect();
    if new_idx.is_empty() {
        return 0;
    }
    // Hanya tabel yang belum punya group dipindahkan: memindahkan anggota group
    // lain akan merusak kotak group tersebut.
    let movable: Vec<usize> = new_idx
        .iter()
        .copied()
        .filter(|&i| {
            let n = &state.nodes[i];
            n.group_ids.is_empty() && n.group_id.is_none()
        })
        .collect();

    if arrange && !movable.is_empty() {
        let cols = (movable.len() as f32).sqrt().ceil().max(1.0) as usize;
        let cell = movable
            .iter()
            .fold(egui::Vec2::ZERO, |acc, &i| acc.max(state.nodes[i].size))
            + egui::vec2(40.0, 40.0);
        for (k, &i) in movable.iter().enumerate() {
            state.nodes[i].pos =
                anchor + egui::vec2((k % cols) as f32 * cell.x, (k / cols) as f32 * cell.y);
        }
    }
    for &i in &new_idx {
        state.nodes[i].add_to_group(gid.to_string());
    }
    if arrange && state.prevent_overlap {
        crate::diagram_view::resolve_all_overlaps(&mut state.nodes, 20.0, Some(gid));
    }
    state.save_requested = true;
    log::info!("[DIAGRAM] added {} table(s) to group {gid}", new_idx.len());
    new_idx.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramGroup, DiagramNode};

    #[test]
    fn picker_matches_title_or_id_case_insensitive() {
        assert!(picker_matches("", "orders", "public.orders"));
        assert!(picker_matches("  ORD ", "orders", "public.orders"));
        assert!(picker_matches("public.", "orders", "public.orders"));
        assert!(!picker_matches("users", "orders", "public.orders"));
    }

    fn node(id: &str, x: f32, y: f32, groups: &[&str]) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: egui::pos2(x, y),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: groups.iter().map(|g| g.to_string()).collect(),
            group_id: groups.first().map(|g| g.to_string()),
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        }
    }

    fn state() -> DiagramState {
        DiagramState {
            groups: vec![
                DiagramGroup {
                    id: "g".into(),
                    title: "Sales".into(),
                    color: egui::Color32::WHITE,
                    manual_pos: None,
                    repo_url: Some("https://example.com/app.git".into()),
                },
                DiagramGroup {
                    id: "other".into(),
                    title: "Other".into(),
                    color: egui::Color32::WHITE,
                    manual_pos: None,
                    repo_url: None,
                },
            ],
            nodes: vec![
                node("orders", 0.0, 0.0, &["g"]),
                node("customers", 5000.0, 5000.0, &[]),
                node("invoices", -3000.0, 800.0, &[]),
                node("audit", 900.0, 900.0, &["other"]),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn add_tables_arranges_free_tables_below_group() {
        let mut st = state();
        let ids = vec![
            "customers".to_string(),
            "invoices".to_string(),
            "audit".to_string(),
            "orders".to_string(), // sudah anggota: diabaikan
            "missing".to_string(),
        ];
        let added = add_tables_to_group(&mut st, "g", &ids, true);
        assert_eq!(added, 3);
        assert!(st.save_requested);
        let by_id = |id: &str| st.nodes.iter().find(|n| n.id == id).expect("node");
        for id in ["customers", "invoices", "audit"] {
            assert!(by_id(id).is_in_group("g"), "{id} should join the group");
        }
        // Tabel bebas dipindah ke bawah isi group (orders: y 0..100).
        assert!(by_id("customers").pos.y >= 160.0);
        assert!(by_id("customers").pos.x < 1000.0);
        assert!(by_id("invoices").pos.x > -1000.0);
        // Anggota group lain tidak dipindah.
        assert_eq!(by_id("audit").pos, egui::pos2(900.0, 900.0));
        assert!(by_id("audit").is_in_group("other"));
        assert!(!crate::diagram_view::check_nodes_overlap(&st.nodes, 0.0));
    }

    #[test]
    fn add_tables_without_arrange_keeps_positions() {
        let mut st = state();
        let added = add_tables_to_group(&mut st, "g", &["customers".to_string()], false);
        assert_eq!(added, 1);
        let c = st.nodes.iter().find(|n| n.id == "customers").expect("node");
        assert_eq!(c.pos, egui::pos2(5000.0, 5000.0));
        assert!(c.is_in_group("g"));
    }

    #[test]
    fn group_repository_fields_roundtrip_and_old_files_load() {
        let st = state();
        let json = serde_json::to_string(&st.groups[0]).expect("serialize");
        let back: DiagramGroup = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.shared_repo_url(), Some("https://example.com/app.git"));
        // Folder personal tidak pernah ikut diagram (file, DB, sync).
        assert!(!json.contains("path"), "{json}");

        // File diagram lama tanpa URL; `repo_path` dari build sebelumnya diabaikan.
        let mut old: serde_json::Value = serde_json::from_str(&json).expect("value");
        let obj = old.as_object_mut().expect("object");
        obj.remove("repo_url");
        obj.insert("repo_path".into(), serde_json::json!("/someone/else"));
        let legacy: DiagramGroup = serde_json::from_value(old).expect("legacy loads");
        assert!(legacy.shared_repo_url().is_none());
        // Group tanpa URL tidak menambah field ke file.
        let plain = serde_json::to_string(&legacy).expect("serialize");
        assert!(!plain.contains("repo_"), "{plain}");
    }

    #[test]
    fn add_tables_to_unknown_group_is_noop() {
        let mut st = state();
        assert_eq!(
            add_tables_to_group(&mut st, "nope", &["customers".to_string()], true),
            0
        );
        assert!(!st.save_requested);
    }
}
