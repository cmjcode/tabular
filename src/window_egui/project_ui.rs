//! UI Project: switcher di header kanan, dialog New/Edit Project, filter
//! tree per project, share project ke team, dan sinkronisasi manifest.
//!
//! Logika data ada di [`crate::project`] dan [`crate::project_memory`]; modul
//! ini hanya menghubungkannya ke state `Tabular`.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use eframe::egui;
use egui_icons::icons;

use crate::connection_env::Environment;
use crate::models::structs::{ConnectionConfig, TreeNode};
use crate::project::{self, EnvVar, Project, ProjectEnv};
use crate::project_memory::{self, MemoryEntry};
use crate::sync::sync_projects::{self, MergeAction, PulledProject};
use crate::window_egui::{Tabular, style};

/// Lebar dialog project.
const DIALOG_WIDTH: f32 = 600.0;

#[derive(Default)]
pub struct ProjectsState {
    pub loaded: bool,
    pub list: Vec<Project>,
    pub ui: project::UiState,
    pub dialog: Option<ProjectDialog>,
    /// Minta sinkronisasi project pada tick sync berikutnya.
    pub sync_trigger: bool,
    pub push_rx: Option<Receiver<Result<usize, String>>>,
    pub pull_rx: Option<Receiver<Result<Vec<PulledProject>, String>>>,
    pub share_rx: Option<Receiver<Result<String, String>>>,
    /// Team pilihan per project di panel Collaborations.
    pub share_team_choice: HashMap<String, String>,
    /// Cache variabel HTTP (dengan secret) per `(project id, updated_at,
    /// environment)` supaya keychain tidak dibaca setiap frame.
    vars_cache: Option<((String, i64, String), HashMap<String, String>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogMode {
    Create,
    Edit(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogTab {
    General,
    Environments,
    Memory,
}

pub struct ProjectDialog {
    pub mode: DialogMode,
    pub draft: Project,
    /// Nama sebelum diedit (untuk rename folder).
    pub original: Option<Project>,
    pub tab: DialogTab,
    pub create_connection_folder: bool,
    pub create_query_folder: bool,
    pub create_http_workspace: bool,
    pub selected_env: usize,
    pub new_env_name: String,
    pub memory: Vec<MemoryEntry>,
    pub error: Option<String>,
    pub confirm_delete: bool,
}

// ─── Akses state ──────────────────────────────────────────────────────────────

fn app_dir() -> std::path::PathBuf {
    crate::directory::get_app_data_dir()
}

pub fn ensure_loaded(t: &mut Tabular) {
    if t.projects.loaded {
        return;
    }
    t.projects.loaded = true;
    let dir = app_dir();
    t.projects.list = project::load_all(&dir);
    t.projects.ui = project::load_ui_state(&dir);
    if let Some(id) = &t.projects.ui.active_id
        && !t.projects.list.iter().any(|p| &p.id == id)
    {
        t.projects.ui.active_id = None;
    }
}

fn save_ui_state(t: &Tabular) {
    if let Err(e) = project::save_ui_state(&app_dir(), &t.projects.ui) {
        log::warn!("[PROJECT] Cannot save project UI state: {}", e);
    }
}

pub fn active(t: &Tabular) -> Option<&Project> {
    let id = t.projects.ui.active_id.as_deref()?;
    t.projects.list.iter().find(|p| p.id == id)
}

/// Project pemilik koneksi `conn`.
pub fn project_for_connection<'a>(t: &'a Tabular, conn: &ConnectionConfig) -> Option<&'a Project> {
    t.projects.list.iter().find(|p| p.owns_connection(conn))
}

/// Project yang relevan untuk konteks saat ini: pemilik koneksi tab aktif,
/// atau project yang dipilih di switcher.
pub fn current_project(t: &Tabular) -> Option<&Project> {
    t.query_tabs
        .get(t.active_tab_index)
        .and_then(|tab| tab.connection_id)
        .and_then(|id| t.connections.iter().find(|c| c.id == Some(id)))
        .and_then(|c| project_for_connection(t, c))
        .or_else(|| active(t))
}

/// Variabel untuk query SQL di koneksi `conn`: environment aktif project
/// pemilik koneksi, tanpa secret (hasil substitusi masuk riwayat query).
pub fn sql_vars_for_connection(t: &Tabular, conn: &ConnectionConfig) -> HashMap<String, String> {
    let Some(p) = project_for_connection(t, conn) else {
        return HashMap::new();
    };
    match p.active_environment() {
        Some(env) => p.env_vars_with(&env.name, |_| None),
        None => HashMap::new(),
    }
}

/// Variabel untuk request HTTP: environment aktif project pemilik workspace
/// request, atau project yang dipilih di switcher; termasuk secret dari
/// keychain (nilai hanya dipakai di request, tidak disimpan).
pub fn http_vars_for_workspace(t: &mut Tabular, ws_id: Option<&str>) -> HashMap<String, String> {
    if !t.projects.loaded || t.projects.list.is_empty() {
        return HashMap::new();
    }
    let Some(p) = ws_id
        .and_then(|id| project_of_workspace(t, id))
        .or_else(|| active(t))
    else {
        return HashMap::new();
    };
    let Some(env) = p.active_environment() else {
        return HashMap::new();
    };
    let key = (p.id.clone(), p.updated_at, env.name.clone());
    if let Some((k, vars)) = &t.projects.vars_cache
        && *k == key
    {
        return vars.clone();
    }
    let vars = p.active_vars();
    t.projects.vars_cache = Some((key, vars.clone()));
    vars
}

/// Seksi system prompt AI untuk project saat ini (kosong bila tidak ada).
pub fn prompt_section(t: &Tabular, mcp_available: bool) -> String {
    let Some(p) = current_project(t) else {
        return String::new();
    };
    let memory = project_memory::list(&app_dir(), &p.id);
    project_memory::prompt_section(p, &memory, mcp_available, 6_000)
}

fn replace_project(t: &mut Tabular, p: Project) {
    match t.projects.list.iter_mut().find(|x| x.id == p.id) {
        Some(slot) => *slot = p,
        None => t.projects.list.push(p),
    }
    t.projects.list.sort_by_key(|p| p.name.to_lowercase());
}

fn persist(t: &mut Tabular, p: &Project) -> bool {
    t.projects.vars_cache = None;
    match project::save(&app_dir(), p) {
        Ok(()) => {
            if t.sync_account.is_some() {
                t.projects.sync_trigger = true;
            }
            true
        }
        Err(e) => {
            t.toasts
                .error(format!("Could not save project '{}': {}", p.name, e));
            false
        }
    }
}

pub fn set_active(t: &mut Tabular, id: Option<String>) {
    t.projects.ui.active_id = id;
    save_ui_state(t);
}

/// Ganti environment aktif project, perbarui tanda warna koneksi, dan
/// pindahkan tab aktif ke koneksi environment baru bila tab itu milik project.
pub fn set_active_env(t: &mut Tabular, project_id: &str, env_name: &str) {
    let Some(mut p) = t.projects.list.iter().find(|p| p.id == project_id).cloned() else {
        return;
    };
    p.active_env = Some(env_name.to_string());
    p.touch();
    persist(t, &p);
    replace_project(t, p.clone());
    apply_environment_marks(t, &p);

    let current = t
        .query_tabs
        .get(t.active_tab_index)
        .and_then(|tab| tab.connection_id)
        .and_then(|id| t.connections.iter().find(|c| c.id == Some(id)).cloned());
    if let Some(cur) = current.filter(|c| p.owns_connection(c))
        && let Some(target_id) = p.connection_for_env(env_name, &t.connections, Some(&cur))
        && Some(target_id) != cur.id
    {
        let target_db = t
            .connections
            .iter()
            .find(|c| c.id == Some(target_id))
            .map(|c| c.database.clone())
            .filter(|d| !d.trim().is_empty());
        let db = target_db.or_else(|| {
            t.query_tabs
                .get(t.active_tab_index)
                .and_then(|tab| tab.database_name.clone())
        });
        t.set_active_tab_connection_with_database(Some(target_id), db);
        t.toasts
            .info(format!("Switched tab to {} connection", env_name));
    }
}

/// Tulis tanda environment koneksi sesuai peta koneksi per environment.
fn apply_environment_marks(t: &mut Tabular, p: &Project) {
    for (id, env) in p.environment_marks(&t.connections) {
        if t.platform_ui.connection_envs.get(&id) != Some(&env) {
            t.set_connection_environment(id, Some(env));
        }
    }
}

// ─── Filter tree ──────────────────────────────────────────────────────────────

/// Pisahkan node root yang ditampilkan dari yang disembunyikan. Node
/// tersembunyi menyimpan index aslinya untuk [`merge_roots`].
pub fn split_roots(
    tree: Vec<TreeNode>,
    keep: impl Fn(&TreeNode) -> bool,
) -> (Vec<TreeNode>, Vec<(usize, TreeNode)>) {
    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    for (i, node) in tree.into_iter().enumerate() {
        if keep(&node) {
            visible.push(node);
        } else {
            hidden.push((i, node));
        }
    }
    (visible, hidden)
}

/// Gabungkan kembali hasil [`split_roots`] dengan urutan semula.
pub fn merge_roots(visible: Vec<TreeNode>, hidden: Vec<(usize, TreeNode)>) -> Vec<TreeNode> {
    let mut out = visible;
    for (i, node) in hidden {
        let at = i.min(out.len());
        out.insert(at, node);
    }
    out
}

fn filter_project(t: &Tabular) -> Option<&Project> {
    if t.projects.ui.filter_only {
        active(t)
    } else {
        None
    }
}

/// Split tree koneksi untuk filter project (tanpa filter: semua tampil).
pub fn split_connection_roots(
    t: &Tabular,
    tree: Vec<TreeNode>,
) -> (Vec<TreeNode>, Vec<(usize, TreeNode)>) {
    match filter_project(t) {
        Some(p) => {
            let root = p
                .connection_folder
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string();
            split_roots(tree, |n| n.name == root)
        }
        None => (tree, Vec::new()),
    }
}

/// Split tree query untuk filter project.
pub fn split_query_roots(
    t: &Tabular,
    tree: Vec<TreeNode>,
) -> (Vec<TreeNode>, Vec<(usize, TreeNode)>) {
    match filter_project(t) {
        Some(p) => {
            let root = p.query_folder.clone();
            split_roots(tree, |n| n.name == root)
        }
        None => (tree, Vec::new()),
    }
}

/// Workspace HTTP ditampilkan di sidebar.
pub fn http_workspace_visible(t: &Tabular, ws_id: &str) -> bool {
    match filter_project(t) {
        Some(p) => p.http_workspace_id.as_deref() == Some(ws_id),
        None => true,
    }
}

/// Nama project pemilik workspace HTTP (untuk label di sidebar).
pub fn project_of_workspace<'a>(t: &'a Tabular, ws_id: &str) -> Option<&'a Project> {
    t.projects
        .list
        .iter()
        .find(|p| p.http_workspace_id.as_deref() == Some(ws_id))
}

// ─── Switcher ─────────────────────────────────────────────────────────────────

fn env_pill(ui: &mut egui::Ui, env: &ProjectEnv) -> egui::Response {
    let color = env
        .environment()
        .map(Environment::color)
        .unwrap_or_else(|| ui.visuals().weak_text_color());
    let text = env
        .environment()
        .map(|e| e.short().to_string())
        .unwrap_or_else(|| env.name.to_uppercase().chars().take(6).collect());
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(10.5)
                .strong()
                .color(egui::Color32::WHITE),
        )
        .fill(color)
        .corner_radius(4.0)
        .min_size(egui::vec2(0.0, 18.0)),
    )
    .on_hover_text(format!("Environment: {} (click to switch)", env.name))
}

/// Aksi yang dipilih user dari switcher project pada satu frame.
#[derive(Default)]
struct SwitcherAction {
    open_create: bool,
    open_edit: Option<String>,
    select: Option<Option<String>>,
    select_env: Option<(String, String)>,
    toggle_filter: bool,
    go_share: bool,
}

/// Panjang maksimum nama project di header sebelum dipotong.
const HEADER_NAME_MAX_CHARS: usize = 24;

/// Switcher project di header kanan: tombol menu project + pill environment.
///
/// Urutan widget mengikuti arah layout `ui`: pada layout kanan-ke-kiri (header)
/// pill environment ditambahkan lebih dulu agar tetap tampil di kanan nama project.
pub fn render_switcher(t: &mut Tabular, ui: &mut egui::Ui) {
    ensure_loaded(t);
    let mut act = SwitcherAction::default();
    let active_proj = active(t).cloned();
    let right_to_left = ui.layout().main_dir() == egui::Direction::RightToLeft;

    if right_to_left {
        switcher_env_pill(ui, active_proj.as_ref(), &mut act);
        switcher_menu(t, ui, active_proj.as_ref(), &mut act);
    } else {
        switcher_menu(t, ui, active_proj.as_ref(), &mut act);
        switcher_env_pill(ui, active_proj.as_ref(), &mut act);
    }

    let SwitcherAction {
        open_create,
        open_edit,
        select,
        select_env,
        toggle_filter,
        go_share,
    } = act;
    if let Some(id) = select {
        set_active(t, id);
    }
    if let Some((pid, env)) = select_env {
        set_active_env(t, &pid, &env);
    }
    if toggle_filter {
        t.projects.ui.filter_only = !t.projects.ui.filter_only;
        save_ui_state(t);
    }
    if open_create {
        open_create_dialog(t, None);
    }
    if let Some(id) = open_edit {
        open_edit_dialog(t, &id);
    }
    if go_share {
        t.selected_menu = "Collaborations".to_string();
        t.selected_collab_sub_menu = "Projects".to_string();
        crate::sync::ui_teams::refresh_teams(t);
    }
}

/// Tombol menu project (ikon + nama + chevron) beserta isi menunya.
fn switcher_menu(
    t: &Tabular,
    ui: &mut egui::Ui,
    active_proj: Option<&Project>,
    act: &mut SwitcherAction,
) {
    let full_name = active_proj.map_or("All projects", |p| p.name.as_str());
    let mut label: String = full_name.chars().take(HEADER_NAME_MAX_CHARS).collect();
    if label.chars().count() < full_name.chars().count() {
        label.push('…');
    }
    let menu_text = egui::RichText::new(format!(
        "{}  {}  {}",
        icons::ICON_WORKSPACES.codepoint,
        label,
        icons::ICON_EXPAND_MORE.codepoint
    ))
    .strong();
    let resp = ui.menu_button(menu_text, |ui| {
        ui.set_min_width(220.0);
        if ui
            .selectable_label(active_proj.is_none(), "All projects")
            .clicked()
        {
            act.select = Some(None);
            ui.close();
        }
        if !t.projects.list.is_empty() {
            ui.separator();
        }
        for p in &t.projects.list {
            let is_active = active_proj.is_some_and(|a| a.id == p.id);
            let mut text = p.name.clone();
            if p.owner_id.is_some() {
                text.push_str("  (shared)");
            }
            if ui.selectable_label(is_active, text).clicked() {
                act.select = Some(Some(p.id.clone()));
                ui.close();
            }
        }
        ui.separator();
        if ui
            .button(format!("{}  New project…", icons::ICON_ADD.codepoint))
            .clicked()
        {
            act.open_create = true;
            ui.close();
        }
        if let Some(p) = active_proj {
            if ui
                .button(format!("{}  Edit project…", icons::ICON_EDIT.codepoint))
                .clicked()
            {
                act.open_edit = Some(p.id.clone());
                ui.close();
            }
            let mut only = t.projects.ui.filter_only;
            if ui.checkbox(&mut only, "Show only this project").changed() {
                act.toggle_filter = true;
                ui.close();
            }
            if ui
                .button(format!("{}  Share with team…", icons::ICON_SHARE.codepoint))
                .clicked()
            {
                act.go_share = true;
                ui.close();
            }
        }
    });
    resp.response.on_hover_text(format!("Project: {full_name}"));
}

/// Pill environment aktif project; klik untuk memilih environment lain.
fn switcher_env_pill(ui: &mut egui::Ui, active_proj: Option<&Project>, act: &mut SwitcherAction) {
    let Some(p) = active_proj else {
        return;
    };
    let Some(env) = p.active_environment().cloned() else {
        return;
    };
    let pill = env_pill(ui, &env);
    egui::Popup::menu(&pill).show(|ui| {
        ui.set_min_width(160.0);
        for e in &p.environments {
            let is_active = e.name == env.name;
            let color = e
                .environment()
                .map(Environment::color)
                .unwrap_or_else(|| ui.visuals().text_color());
            let text = egui::RichText::new(format!("{}  {}", icons::ICON_CIRCLE.codepoint, e.name))
                .color(color);
            if ui.selectable_label(is_active, text).clicked() {
                act.select_env = Some((p.id.clone(), e.name.clone()));
                ui.close();
            }
        }
    });
}

// ─── Dialog ───────────────────────────────────────────────────────────────────

/// Buka dialog New Project. `adopt` = `(jenis, path)` folder yang sudah ada
/// untuk diangkat menjadi project (`connection`, `query`, atau `http`).
pub fn open_create_dialog(t: &mut Tabular, adopt: Option<(&str, &str)>) {
    ensure_loaded(t);
    let name = adopt
        .map(|(_, path)| {
            path.trim_start_matches('/')
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default();
    let mut draft = Project::new(&name);
    if let Some(("http", ws_id)) = adopt {
        draft.http_workspace_id = Some(ws_id.to_string());
        if let Some(ws) = t.yaak_workspaces.iter().find(|w| w.id == ws_id) {
            draft = Project {
                http_workspace_id: Some(ws.id.clone()),
                ..Project::new(&ws.name)
            };
        }
    }
    t.projects.dialog = Some(ProjectDialog {
        mode: DialogMode::Create,
        draft,
        original: None,
        tab: DialogTab::General,
        create_connection_folder: true,
        create_query_folder: true,
        create_http_workspace: true,
        selected_env: 0,
        new_env_name: String::new(),
        memory: Vec::new(),
        error: None,
        confirm_delete: false,
    });
}

/// "Convert to Project…": buka dialog New Project untuk folder yang sudah
/// ada, atau dialog Edit bila folder itu sudah milik sebuah project.
pub fn convert_folder(t: &mut Tabular, kind: &str, path: &str) {
    ensure_loaded(t);
    let existing = t
        .projects
        .list
        .iter()
        .find(|p| match kind {
            "connection" => p.connection_folder == path,
            "query" => p.query_folder == path,
            "http" => p.http_workspace_id.as_deref() == Some(path),
            _ => false,
        })
        .map(|p| p.id.clone());
    match existing {
        Some(id) => open_edit_dialog(t, &id),
        None => open_create_dialog(t, Some((kind, path))),
    }
}

pub fn open_edit_dialog(t: &mut Tabular, id: &str) {
    ensure_loaded(t);
    let Some(p) = t.projects.list.iter().find(|p| p.id == id).cloned() else {
        return;
    };
    let mut draft = p.clone();
    draft.load_pending_secrets(crate::secrets::get_secret);
    t.projects.dialog = Some(ProjectDialog {
        mode: DialogMode::Edit(p.id.clone()),
        memory: project_memory::list(&app_dir(), &p.id),
        original: Some(p),
        draft,
        tab: DialogTab::General,
        create_connection_folder: false,
        create_query_folder: false,
        create_http_workspace: false,
        selected_env: 0,
        new_env_name: String::new(),
        error: None,
        confirm_delete: false,
    });
}

enum DialogOutcome {
    None,
    Close,
    Submit,
    Delete,
    DeleteMemory(String),
}

pub fn render_dialog(t: &mut Tabular, ctx: &egui::Context) {
    let Some(mut dlg) = t.projects.dialog.take() else {
        style::render_modal_backdrop(ctx, "project_dialog_backdrop", false);
        return;
    };
    style::render_modal_backdrop(ctx, "project_dialog_backdrop", true);

    let member_connections: Vec<ConnectionConfig> = dlg
        .draft
        .member_connections(&t.connections)
        .into_iter()
        .cloned()
        .collect();
    let ws_name = dlg
        .draft
        .http_workspace_id
        .as_deref()
        .and_then(|id| t.yaak_workspaces.iter().find(|w| w.id == id))
        .map(|w| w.name.clone());
    let read_only = dlg.draft.is_read_only();
    let is_edit = matches!(dlg.mode, DialogMode::Edit(_));
    let mut outcome = DialogOutcome::None;

    egui::Window::new("project_dialog")
        .id(egui::Id::new("project_dialog"))
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(DIALOG_WIDTH)
        .show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH);
            let mut close = false;
            let title = if is_edit { "Edit Project" } else { "New Project" };
            style::render_modal_header(ui, title, &mut close);
            if close {
                outcome = DialogOutcome::Close;
            }
            ui.add_space(6.0);

            if read_only {
                style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
                    ui.label("This project is shared with you read-only. Only its owner or an editor can change it.");
                });
                ui.add_space(6.0);
            }

            let segments = [
                style::NavSegment { key: "General", icon: icons::ICON_TUNE.codepoint, label: "General" },
                style::NavSegment { key: "Environments", icon: icons::ICON_LAYERS.codepoint, label: "Environments" },
                style::NavSegment { key: "Memory", icon: icons::ICON_PSYCHOLOGY.codepoint, label: "AI memory" },
            ];
            let visible_segments = if is_edit { &segments[..] } else { &segments[..2] };
            let current = match dlg.tab {
                DialogTab::General => "General",
                DialogTab::Environments => "Environments",
                DialogTab::Memory => "Memory",
            };
            if let Some(key) = style::render_segmented_nav(ui, "project_dialog_tabs", visible_segments, current, 30.0) {
                dlg.tab = match key {
                    "Environments" => DialogTab::Environments,
                    "Memory" => DialogTab::Memory,
                    _ => DialogTab::General,
                };
            }
            ui.add_space(8.0);

            egui::ScrollArea::vertical()
                .max_height(ctx.content_rect().height() * 0.6)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_enabled_ui(!read_only || dlg.tab == DialogTab::Memory, |ui| match dlg.tab {
                        DialogTab::General => render_general_tab(ui, &mut dlg, ws_name.as_deref()),
                        DialogTab::Environments => {
                            render_environments_tab(ui, &mut dlg, &member_connections)
                        }
                        DialogTab::Memory => {
                            if let Some(name) = render_memory_tab(ui, &dlg.memory, read_only) {
                                outcome = DialogOutcome::DeleteMemory(name);
                            }
                        }
                    });
                });

            if let Some(err) = &dlg.error {
                ui.add_space(6.0);
                ui.colored_label(style::theme_danger(ui.ctx()), err);
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if is_edit && dlg.draft.owner_id.is_none() {
                    if dlg.confirm_delete {
                        ui.label("Delete this project? Its folders stay.");
                        if ui.add(style::btn_danger_ctx(ui.ctx(), "Delete")).clicked() {
                            outcome = DialogOutcome::Delete;
                        }
                        if ui.button("Keep").clicked() {
                            dlg.confirm_delete = false;
                        }
                    } else if ui.add(style::btn_secondary("Delete project")).clicked() {
                        dlg.confirm_delete = true;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let submit_label = if is_edit { "Save" } else { "Create project" };
                    if ui
                        .add_enabled(!read_only, style::btn_primary_ctx(ui.ctx(), submit_label))
                        .clicked()
                    {
                        outcome = DialogOutcome::Submit;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        outcome = DialogOutcome::Close;
                    }
                });
            });
        });

    match outcome {
        DialogOutcome::None => t.projects.dialog = Some(dlg),
        DialogOutcome::Close => {}
        DialogOutcome::Submit => {
            let result = match dlg.mode.clone() {
                DialogMode::Create => create_project(t, &dlg),
                DialogMode::Edit(_) => save_project(t, &dlg),
            };
            if let Err(e) = result {
                dlg.error = Some(e);
                t.projects.dialog = Some(dlg);
            }
        }
        DialogOutcome::Delete => delete_project(t, &dlg.draft.id),
        DialogOutcome::DeleteMemory(name) => {
            match project_memory::delete(&app_dir(), &dlg.draft.id, &name) {
                Ok(_) => {
                    dlg.memory = project_memory::list(&app_dir(), &dlg.draft.id);
                    if t.sync_account.is_some() {
                        t.projects.sync_trigger = true;
                    }
                }
                Err(e) => dlg.error = Some(e.to_string()),
            }
            t.projects.dialog = Some(dlg);
        }
    }
}

fn render_general_tab(ui: &mut egui::Ui, dlg: &mut ProjectDialog, ws_name: Option<&str>) {
    let is_edit = matches!(dlg.mode, DialogMode::Edit(_));
    style::render_modal_card(ui, Some("Project"), None, |ui| {
        egui::Grid::new("project_general_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut dlg.draft.name).desired_width(f32::INFINITY),
                );
                ui.end_row();
                ui.label("Description");
                ui.add(
                    egui::TextEdit::multiline(&mut dlg.draft.description)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
                ui.label("Repository URL");
                let mut repo = dlg.draft.repo_url.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut repo)
                            .hint_text("https://github.com/org/repo (optional)")
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    dlg.draft.repo_url = (!repo.trim().is_empty()).then(|| repo.trim().to_string());
                }
                ui.end_row();
            });
    });
    ui.add_space(8.0);

    if is_edit {
        style::render_modal_card(
            ui,
            Some("Linked folders"),
            Some("Renaming the project renames these folders too."),
            |ui| {
                egui::Grid::new("project_links_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Connections");
                        ui.monospace(&dlg.draft.connection_folder);
                        ui.end_row();
                        ui.label("Queries");
                        ui.monospace(&dlg.draft.query_folder);
                        ui.end_row();
                        ui.label("HTTP");
                        ui.monospace(ws_name.unwrap_or("(none)"));
                        ui.end_row();
                    });
            },
        );
    } else {
        style::render_modal_card(
            ui,
            Some("Create folders"),
            Some("Folders get the project name. Existing folders with that name are reused."),
            |ui| {
                ui.checkbox(&mut dlg.create_connection_folder, "Connections folder");
                ui.checkbox(&mut dlg.create_query_folder, "Queries folder");
                ui.checkbox(&mut dlg.create_http_workspace, "HTTP collection");
            },
        );
    }
}

fn render_environments_tab(
    ui: &mut egui::Ui,
    dlg: &mut ProjectDialog,
    member_connections: &[ConnectionConfig],
) {
    let envs = &mut dlg.draft.environments;
    if dlg.selected_env >= envs.len() {
        dlg.selected_env = envs.len().saturating_sub(1);
    }
    let mut remove_env: Option<usize> = None;

    ui.horizontal_wrapped(|ui| {
        for (i, env) in envs.iter().enumerate() {
            let color = env
                .environment()
                .map(Environment::color)
                .unwrap_or_else(|| ui.visuals().text_color());
            let text = egui::RichText::new(&env.name).color(color).strong();
            if ui.selectable_label(dlg.selected_env == i, text).clicked() {
                dlg.selected_env = i;
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut dlg.new_env_name)
                .hint_text("New environment")
                .desired_width(130.0),
        );
        let name = dlg.new_env_name.trim().to_string();
        let can_add = !name.is_empty() && !envs.iter().any(|e| e.name.eq_ignore_ascii_case(&name));
        if ui
            .add_enabled(can_add, egui::Button::new(icons::ICON_ADD.rich_text()))
            .on_hover_text("Add environment")
            .clicked()
        {
            let kind = crate::connection_env::detect_from_name(&name);
            envs.push(ProjectEnv::new(&name, kind));
            dlg.selected_env = envs.len() - 1;
            dlg.new_env_name.clear();
        }
    });
    ui.add_space(8.0);

    let selected = dlg.selected_env;
    let env_count = envs.len();
    let Some(env) = envs.get_mut(selected) else {
        ui.label("Add an environment to start.");
        return;
    };

    style::render_modal_card(ui, Some("Environment"), None, |ui| {
        egui::Grid::new("project_env_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut env.name).desired_width(200.0));
                ui.end_row();
                ui.label("Type");
                let current = env
                    .kind
                    .as_deref()
                    .and_then(Environment::parse)
                    .map(|k| k.label())
                    .unwrap_or("Auto (from name)");
                egui::ComboBox::from_id_salt("project_env_kind")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(env.kind.is_none(), "Auto (from name)")
                            .clicked()
                        {
                            env.kind = None;
                        }
                        for k in Environment::ALL {
                            let text = egui::RichText::new(k.label()).color(k.color());
                            if ui
                                .selectable_label(env.kind.as_deref() == Some(k.key()), text)
                                .clicked()
                            {
                                env.kind = Some(k.key().to_string());
                            }
                        }
                    });
                ui.end_row();
            });
        if env_count > 1 && ui.add(style::btn_secondary("Remove environment")).clicked() {
            remove_env = Some(selected);
        }
    });
    ui.add_space(8.0);

    style::render_modal_card(
        ui,
        Some("Variables (.env)"),
        Some(
            "Use {{KEY}} in SQL and HTTP requests. Secret values stay in this computer's keychain and are never shared; SQL only uses non-secret values.",
        ),
        |ui| {
            let mut remove_var: Option<usize> = None;
            // Baris biasa dengan ukuran tetap: Grid memotong lebar TextEdit.
            const KEY_W: f32 = 170.0;
            const ROW_H: f32 = 22.0;
            let value_w = (ui.available_width() - KEY_W - 90.0).max(160.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [KEY_W, 14.0],
                    egui::Label::new(egui::RichText::new("Key").small().strong()),
                );
                ui.add_sized(
                    [value_w, 14.0],
                    egui::Label::new(egui::RichText::new("Value").small().strong()),
                );
                ui.label(egui::RichText::new("Secret").small().strong());
            });
            for (i, v) in env.variables.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [KEY_W, ROW_H],
                        egui::TextEdit::singleline(&mut v.key)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("KEY"),
                    );
                    if v.secret {
                        let val = v.pending_secret.get_or_insert_with(String::new);
                        ui.add_sized(
                            [value_w, ROW_H],
                            egui::TextEdit::singleline(val).password(true),
                        );
                    } else {
                        ui.add_sized(
                            [value_w, ROW_H],
                            egui::TextEdit::singleline(&mut v.value)
                                .font(egui::TextStyle::Monospace),
                        );
                    }
                    let was_secret = v.secret;
                    ui.checkbox(&mut v.secret, "")
                        .on_hover_text("Keep the value in this computer's keychain");
                    if v.secret != was_secret {
                        if v.secret {
                            v.pending_secret = Some(std::mem::take(&mut v.value));
                        } else {
                            v.value = v.pending_secret.take().unwrap_or_default();
                        }
                    }
                    if ui
                        .add(egui::Button::new(icons::ICON_DELETE.rich_text()).frame(false))
                        .on_hover_text("Remove variable")
                        .clicked()
                    {
                        remove_var = Some(i);
                    }
                });
            }
            if let Some(i) = remove_var {
                env.variables.remove(i);
            }
            if ui
                .add(style::btn_secondary(format!(
                    "{}  Variable",
                    icons::ICON_ADD.codepoint
                )))
                .clicked()
            {
                env.variables.push(EnvVar::default());
            }
        },
    );
    ui.add_space(8.0);

    style::render_modal_card(
        ui,
        Some("Connections"),
        Some(
            "Connections in the project folder that this environment uses. Switching environment moves the active query tab to the matching connection.",
        ),
        |ui| {
            if member_connections.is_empty() {
                ui.label(
                    egui::RichText::new("No connections in the project folder yet.")
                        .color(ui.visuals().weak_text_color()),
                );
            }
            for c in member_connections {
                let mut on = env.connections.contains(&c.name);
                if ui
                    .checkbox(
                        &mut on,
                        format!("{}  ({})", c.name, c.connection_type.as_db_str()),
                    )
                    .changed()
                {
                    if on {
                        env.connections.push(c.name.clone());
                    } else {
                        env.connections.retain(|n| n != &c.name);
                    }
                }
            }
        },
    );

    if let Some(i) = remove_env {
        let removed = dlg.draft.environments.remove(i);
        if dlg.draft.active_env.as_deref() == Some(removed.name.as_str()) {
            dlg.draft.active_env = dlg.draft.environments.first().map(|e| e.name.clone());
        }
        dlg.selected_env = 0;
    }
}

/// Mengembalikan nama entri yang diminta dihapus.
fn render_memory_tab(ui: &mut egui::Ui, memory: &[MemoryEntry], read_only: bool) -> Option<String> {
    let mut delete = None;
    ui.label(
        egui::RichText::new(
            "Facts the AI assistant and MCP agents saved about this project. They are shared with the team together with the project.",
        )
        .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(6.0);
    if memory.is_empty() {
        ui.label("No memory yet. Ask the AI assistant to remember something about this project.");
    }
    for e in memory {
        style::modal_card_frame(ui.ctx()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&e.name).strong().monospace());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !read_only
                        && ui
                            .add(egui::Button::new(icons::ICON_DELETE.rich_text()).frame(false))
                            .on_hover_text("Delete this memory")
                            .clicked()
                    {
                        delete = Some(e.name.clone());
                    }
                });
            });
            if !e.description.is_empty() {
                ui.label(egui::RichText::new(&e.description).italics());
            }
            egui::CollapsingHeader::new("Content")
                .id_salt(format!("project_memory_{}", e.name))
                .show(ui, |ui| {
                    ui.label(&e.body);
                });
        });
        ui.add_space(4.0);
    }
    delete
}

// ─── Aksi ─────────────────────────────────────────────────────────────────────

fn write_secrets(before: Option<&Project>, after: &Project) {
    for (name, value) in after.pending_secret_writes() {
        if value.is_empty() {
            crate::secrets::delete_secret(&name);
        } else if !crate::secrets::set_secret(&name, &value) {
            log::warn!("[PROJECT] Could not store secret {}", name);
        }
    }
    if let Some(old) = before {
        let keep = after.secret_names();
        for name in old.secret_names() {
            if !keep.contains(&name) {
                crate::secrets::delete_secret(&name);
            }
        }
    }
}

fn validate_envs(p: &Project) -> Result<(), String> {
    let mut names: Vec<String> = Vec::new();
    for env in &p.environments {
        let n = env.name.trim();
        if n.is_empty() {
            return Err("Environment name cannot be empty".into());
        }
        if names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            return Err(format!("Environment '{}' appears twice", n));
        }
        names.push(n.to_string());
        let mut keys: Vec<&str> = Vec::new();
        for v in &env.variables {
            let k = v.key.trim();
            if k.is_empty() {
                continue;
            }
            if k.contains("{{") || k.contains("}}") || k.contains(char::is_whitespace) {
                return Err(format!("Invalid variable name '{}' in {}", k, n));
            }
            if keys.contains(&k) {
                return Err(format!("Variable '{}' appears twice in {}", k, n));
            }
            keys.push(k);
        }
    }
    Ok(())
}

fn normalize(p: &mut Project) {
    for env in &mut p.environments {
        env.name = env.name.trim().to_string();
        env.variables.retain(|v| !v.key.trim().is_empty());
        for v in &mut env.variables {
            v.key = v.key.trim().to_string();
        }
    }
    if p.active_env.as_deref().is_none_or(|a| p.env(a).is_none()) {
        p.active_env = p.environments.first().map(|e| e.name.clone());
    }
}

fn create_project(t: &mut Tabular, dlg: &ProjectDialog) -> Result<(), String> {
    let mut p = dlg.draft.clone();
    p.name = project::validate_name(&p.name, &t.projects.list, None).map_err(|e| e.to_string())?;
    validate_envs(&p)?;
    normalize(&mut p);
    p.connection_folder = p.name.clone();
    p.query_folder = p.name.clone();
    p.description = p.description.trim().to_string();
    p.touch();

    if dlg.create_connection_folder {
        crate::sidebar_database::save_connection_folder(t, &p.connection_folder);
        crate::sidebar_database::refresh_connections_tree(t);
    }
    if dlg.create_query_folder {
        project::ensure_query_folder(&crate::directory::get_query_dir(), &p.query_folder)
            .map_err(|e| format!("Could not create the query folder: {}", e))?;
        crate::sidebar_query::load_queries_from_directory(t);
    }
    if dlg.create_http_workspace || p.http_workspace_id.is_some() {
        if project::ensure_http_workspace(&mut t.yaak_workspaces, &mut p)
            && let Err(e) = crate::http_collection::save_workspaces(&t.yaak_workspaces)
        {
            t.toasts.error(e);
        }
    }

    write_secrets(None, &p);
    if !persist(t, &p) {
        return Err("Could not save the project file".into());
    }
    log::info!("[PROJECT] Created project '{}'", p.name);
    t.toasts.success(format!("Project '{}' created", p.name));
    let id = p.id.clone();
    replace_project(t, p.clone());
    apply_environment_marks(t, &p);
    set_active(t, Some(id));
    Ok(())
}

fn save_project(t: &mut Tabular, dlg: &ProjectDialog) -> Result<(), String> {
    let old = dlg
        .original
        .clone()
        .ok_or_else(|| "Project not found".to_string())?;
    let mut p = dlg.draft.clone();
    p.name = project::validate_name(&p.name, &t.projects.list, Some(&p.id))
        .map_err(|e| e.to_string())?;
    validate_envs(&p)?;
    normalize(&mut p);
    p.description = p.description.trim().to_string();

    if p.name != old.name {
        if p.owner_id.is_some() {
            return Err("Only the owner can rename a shared project".into());
        }
        // Folder yang namanya mengikuti project ikut di-rename.
        if !old.connection_folder.contains('/') && old.connection_folder == old.name {
            crate::sidebar_database::rename_connection_folder(t, &old.connection_folder, &p.name)?;
            p.connection_folder = p.name.clone();
        }
        if old.query_folder == old.name {
            let exists = crate::directory::get_query_dir()
                .join(&old.query_folder)
                .is_dir();
            if exists {
                crate::sidebar_query::rename_query_folder(t, &old.query_folder, &p.name)?;
                crate::sidebar_query::load_queries_from_directory(t);
            }
            p.query_folder = p.name.clone();
        }
        if let Some(ws_id) = &p.http_workspace_id
            && t.yaak_workspaces
                .iter()
                .any(|w| &w.id == ws_id && w.name == old.name)
        {
            crate::http_collection::rename_workspace_in_workspaces(
                &mut t.yaak_workspaces,
                ws_id,
                &p.name,
            );
        }
        t.projects.ui.pending_remote_deletes.push(old.name.clone());
        save_ui_state(t);
        if let Some(team) = p.shared_team_id.clone() {
            t.toasts
                .info("Sharing the renamed project again with the team");
            share_project(t, &p, &team);
        }
    }

    p.touch();
    write_secrets(Some(&old), &p);
    if !persist(t, &p) {
        return Err("Could not save the project file".into());
    }
    replace_project(t, p.clone());
    apply_environment_marks(t, &p);
    t.toasts.success(format!("Project '{}' saved", p.name));
    Ok(())
}

fn delete_project(t: &mut Tabular, id: &str) {
    let Some(p) = t.projects.list.iter().find(|p| p.id == id).cloned() else {
        return;
    };
    if let Err(e) = project::delete(&app_dir(), &p) {
        t.toasts.error(format!("Could not delete project: {}", e));
        return;
    }
    t.projects.list.retain(|x| x.id != id);
    if p.owner_id.is_none() {
        t.projects.ui.pending_remote_deletes.push(p.name.clone());
    }
    if t.projects.ui.active_id.as_deref() == Some(id) {
        t.projects.ui.active_id = None;
    }
    save_ui_state(t);
    if t.sync_account.is_some() {
        t.projects.sync_trigger = true;
    }
    t.toasts.info(format!(
        "Project '{}' deleted. Its connection, query and HTTP folders were kept.",
        p.name
    ));
}

// ─── Share & sync ─────────────────────────────────────────────────────────────

/// Bagikan semua folder project dan manifest-nya ke `team_id` dalam satu
/// tugas berurutan, lalu pindahkan item yang sudah tersinkron ke Team key.
/// Dibuat berurutan supaya bootstrap Team key hanya terjadi sekali.
pub fn share_project(t: &mut Tabular, p: &Project, team_id: &str) {
    let Some(account) = t.sync_account.clone() else {
        t.toasts.error("Sign in to share projects");
        return;
    };
    let ws_name = p
        .http_workspace_id
        .as_deref()
        .and_then(|id| t.yaak_workspaces.iter().find(|w| w.id == id))
        .map(|w| w.name.clone());
    let targets = p.share_targets(ws_name.as_deref());
    let connections: Vec<ConnectionConfig> = p
        .member_connections(&t.connections)
        .into_iter()
        .cloned()
        .collect();
    let vault = t.vault.clone();
    let server = t.sync_server_url.clone();
    let team = team_id.to_string();
    let project_name = p.name.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let (key_tx, key_rx) = std::sync::mpsc::channel();

    crate::sync::spawn_async(async move {
        use crate::sync::{
            api_client, sync_connections, sync_http_requests, sync_queries, vault_sync,
        };
        let client = api_client::ApiClient::new(&server);
        let token = account.access_token.clone();
        for (kind, path) in &targets {
            let req = api_client::ShareFolderReq {
                resource_type: kind.to_string(),
                folder_path: path.clone(),
            };
            if let Err(e) = client.share_folder(&token, &team, &req).await {
                let _ = tx.send(Err(format!(
                    "Could not share {} folder '{}': {}",
                    kind, path, e
                )));
                return;
            }
        }
        if let Some(vault) = vault {
            let mut team_keys = HashMap::new();
            let key = async {
                let key = vault_sync::ensure_own_team_key(
                    &client,
                    &token,
                    &account.user_id,
                    &vault,
                    &team,
                    &mut team_keys,
                )
                .await?;
                vault_sync::grant_pending_team_key_envelopes(&client, &token, &team, &key).await?;
                Ok::<_, anyhow::Error>(key)
            }
            .await
            .map_err(|e| e.to_string());
            if let Ok(key) = &key {
                for (kind, path) in &targets {
                    match *kind {
                        "connection" => sync_connections::reencrypt_folder_to_server(
                            connections.clone(),
                            key.clone(),
                            path.clone(),
                            token.clone(),
                            server.clone(),
                        ),
                        "query" => sync_queries::reencrypt_folder_to_server(
                            key.clone(),
                            path.clone(),
                            token.clone(),
                            server.clone(),
                        ),
                        "http" => sync_http_requests::reencrypt_folder_to_server(
                            key.clone(),
                            path.clone(),
                            token.clone(),
                            server.clone(),
                        ),
                        _ => {}
                    }
                }
            }
            let _ = key_tx.send((team.clone(), key));
        }
        let _ = tx.send(Ok(project_name));
    });

    t.projects.share_rx = Some(rx);
    t.vault_team_bootstrap_receiver = Some(key_rx);
    let mut updated = p.clone();
    updated.shared_team_id = Some(team_id.to_string());
    if project::save(&app_dir(), &updated).is_ok() {
        replace_project(t, updated);
    }
}

/// Hentikan share project dari team: hapus semua folder bersama miliknya.
pub fn unshare_project(t: &mut Tabular, project_id: &str) {
    let Some(p) = t.projects.list.iter().find(|p| p.id == project_id).cloned() else {
        return;
    };
    let Some(team) = p.shared_team_id.clone() else {
        return;
    };
    let ws_name = p
        .http_workspace_id
        .as_deref()
        .and_then(|id| t.yaak_workspaces.iter().find(|w| w.id == id))
        .map(|w| w.name.clone());
    let targets = p.share_targets(ws_name.as_deref());
    let ids: Vec<String> = t
        .shared_folders_cache
        .iter()
        .filter(|f| {
            f.team_id == team
                && targets
                    .iter()
                    .any(|(k, path)| f.resource_type == *k && &f.folder_path == path)
        })
        .map(|f| f.id.clone())
        .collect();
    for id in ids {
        crate::sync::ui_teams::unshare_folder_action(t, &team, &id);
    }
    let mut updated = p;
    updated.shared_team_id = None;
    if project::save(&app_dir(), &updated).is_ok() {
        replace_project(t, updated);
    }
    t.toasts.info("Project is no longer shared");
}

/// Dipanggil setiap frame dari tick sync.
pub fn tick(t: &mut Tabular, ctx: &egui::Context) {
    ensure_loaded(t);
    render_dialog(t, ctx);

    if let Some(rx) = &t.projects.share_rx
        && let Ok(res) = rx.try_recv()
    {
        t.projects.share_rx = None;
        match res {
            Ok(name) => {
                t.toasts
                    .success(format!("Project '{}' shared with the team", name));
                crate::sync::ui_teams::refresh_all_shared_folders(t);
                t.projects.sync_trigger = true;
                t.sync_trigger_connections = true;
                t.sync_trigger_queries = true;
                t.sync_trigger_http = true;
            }
            Err(e) => t.toasts.error(e),
        }
    }

    if let Some(rx) = &t.projects.push_rx
        && let Ok(res) = rx.try_recv()
    {
        t.projects.push_rx = None;
        match res {
            Ok(n) => log::info!("[PROJECT] Pushed {} project(s)", n),
            Err(e) => log::warn!("[PROJECT] Project push failed: {}", e),
        }
    }

    if let Some(rx) = &t.projects.pull_rx
        && let Ok(res) = rx.try_recv()
    {
        t.projects.pull_rx = None;
        match res {
            Ok(pulled) => apply_pulled(t, pulled),
            Err(e) => log::warn!("[PROJECT] Project pull failed: {}", e),
        }
    }

    if t.projects.sync_trigger && t.projects.push_rx.is_none() && t.projects.pull_rx.is_none() {
        t.projects.sync_trigger = false;
        start_sync(t);
    }
}

fn start_sync(t: &mut Tabular) {
    let (Some(account), Some(vault)) = (t.sync_account.clone(), t.vault.clone()) else {
        return;
    };
    let token = account.access_token.clone();
    let server = t.sync_server_url.clone();

    // Hapus baris server milik project yang sudah dihapus / di-rename.
    let deletes = std::mem::take(&mut t.projects.ui.pending_remote_deletes);
    let live_names: Vec<String> = t
        .projects
        .list
        .iter()
        .filter(|p| p.owner_id.is_none())
        .map(|p| p.name.clone())
        .collect();
    let deletes: Vec<String> = deletes
        .into_iter()
        .filter(|n| !live_names.iter().any(|l| l.eq_ignore_ascii_case(n)))
        .collect();
    save_ui_state(t);
    if !deletes.is_empty() {
        let token = token.clone();
        let server = server.clone();
        crate::sync::spawn_async(async move {
            let client = crate::sync::api_client::ApiClient::new(&server);
            let Ok(remote) = client.list_projects(&token).await else {
                return;
            };
            for r in remote.iter().filter(|r| {
                r.access.as_deref().is_none_or(|a| a == "owner") && deletes.contains(&r.name)
            }) {
                if let Err(e) = client.delete_project(&token, &r.id).await {
                    log::warn!(
                        "[PROJECT] Could not delete remote project '{}': {}",
                        r.name,
                        e
                    );
                }
            }
        });
    }

    let (tx, rx) = std::sync::mpsc::channel();
    t.projects.push_rx = Some(rx);
    sync_projects::push_projects_to_server(
        app_dir(),
        vault.account_key.clone(),
        t.vault_team_keys.clone(),
        t.shared_folders_cache.clone(),
        token.clone(),
        server.clone(),
        tx,
    );
    let (tx2, rx2) = std::sync::mpsc::channel();
    t.projects.pull_rx = Some(rx2);
    sync_projects::pull_projects_from_server(
        vault.account_key.clone(),
        t.vault_team_keys.clone(),
        t.shared_folders_cache.clone(),
        token,
        server,
        tx2,
    );
}

/// Merge project hasil pull ke daftar lokal dan siapkan foldernya.
fn apply_pulled(t: &mut Tabular, pulled: Vec<PulledProject>) {
    let my_id = t.sync_account.as_ref().map(|a| a.user_id.clone());
    let dir = app_dir();
    let mut changed = 0usize;
    for item in pulled {
        let mut remote = item.shared.project.clone();
        let is_mine = my_id.as_deref() == Some(item.owner_id.as_str());
        let action = sync_projects::merge_action(&t.projects.list, &remote);
        if action == MergeAction::Skip {
            continue;
        }
        let local = t.projects.list.iter().find(|p| p.id == remote.id).cloned();
        // Field lokal dipertahankan / diisi dari metadata server.
        remote.shared_team_id = local.as_ref().and_then(|l| l.shared_team_id.clone());
        remote.active_env = local
            .as_ref()
            .and_then(|l| l.active_env.clone())
            .filter(|a| remote.env(a).is_some())
            .or(remote.active_env);
        if is_mine {
            remote.owner_id = None;
            remote.remote_id = None;
            remote.access = None;
        } else {
            remote.owner_id = Some(item.owner_id.clone());
            remote.remote_id = Some(item.remote_id.clone());
            remote.access = Some(item.access.clone());
        }
        // Workspace HTTP id lokal per mesin: cocokkan ulang lewat nama.
        if remote
            .http_workspace_id
            .as_deref()
            .is_none_or(|id| !t.yaak_workspaces.iter().any(|w| w.id == id))
        {
            remote.http_workspace_id = local.as_ref().and_then(|l| l.http_workspace_id.clone());
        }
        crate::sidebar_database::save_connection_folder(t, &remote.connection_folder);
        if let Err(e) =
            project::ensure_query_folder(&crate::directory::get_query_dir(), &remote.query_folder)
        {
            log::warn!(
                "[PROJECT] Cannot create query folder for '{}': {}",
                remote.name,
                e
            );
        }
        if project::ensure_http_workspace(&mut t.yaak_workspaces, &mut remote)
            && let Err(e) = crate::http_collection::save_workspaces(&t.yaak_workspaces)
        {
            log::warn!("[PROJECT] {}", e);
        }
        if let Err(e) = project::save(&dir, &remote) {
            log::warn!(
                "[PROJECT] Cannot save pulled project '{}': {}",
                remote.name,
                e
            );
            continue;
        }
        if let Err(e) = project_memory::replace_all(&dir, &remote.id, &item.shared.memory) {
            log::warn!("[PROJECT] Cannot save memory of '{}': {}", remote.name, e);
        }
        replace_project(t, remote);
        changed += 1;
    }
    if changed > 0 {
        log::info!("[PROJECT] Applied {} project(s) from the server", changed);
        crate::sidebar_database::refresh_connections_tree(t);
        crate::sidebar_query::load_queries_from_directory(t);
    }
}

// ─── Collaborations > Projects ────────────────────────────────────────────────

pub fn render_collab_projects(t: &mut Tabular, ui: &mut egui::Ui) {
    ensure_loaded(t);
    if t.sync_account.is_none() {
        ui.label("Sign in to share projects with your team.");
        return;
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Projects").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = t.projects.push_rx.is_some() || t.projects.pull_rx.is_some();
            if ui
                .add_enabled(!busy, egui::Button::new(icons::ICON_SYNC.rich_text()))
                .on_hover_text("Sync projects now")
                .clicked()
            {
                t.projects.sync_trigger = true;
            }
        });
    });
    if t.vault.is_none() {
        ui.colored_label(
            style::theme_warning(ui.ctx()),
            "Unlock your vault to sync projects.",
        );
    }
    ui.add_space(4.0);

    let mine: Vec<Project> = t
        .projects
        .list
        .iter()
        .filter(|p| p.owner_id.is_none())
        .cloned()
        .collect();
    let theirs: Vec<Project> = t
        .projects
        .list
        .iter()
        .filter(|p| p.owner_id.is_some())
        .cloned()
        .collect();
    let teams = t.teams.clone();
    let mut share: Option<(Project, String)> = None;
    let mut unshare: Option<String> = None;
    let mut edit: Option<String> = None;

    if mine.is_empty() {
        ui.label(
            egui::RichText::new(
                "No projects yet. Create one from the project menu above the Connections list.",
            )
            .color(ui.visuals().weak_text_color()),
        );
    }
    for p in &mine {
        style::modal_card_frame(ui.ctx()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(icons::ICON_WORKSPACES.rich_text());
                if ui.link(egui::RichText::new(&p.name).strong()).clicked() {
                    edit = Some(p.id.clone());
                }
            });
            match &p.shared_team_id {
                Some(team_id) => {
                    let team_name = teams
                        .iter()
                        .find(|x| &x.id == team_id)
                        .map(|x| x.name.clone())
                        .unwrap_or_else(|| team_id.clone());
                    ui.horizontal(|ui| {
                        ui.label(format!("Shared with {}", team_name));
                        if ui.add(style::btn_secondary("Stop sharing")).clicked() {
                            unshare = Some(p.id.clone());
                        }
                    });
                }
                None if teams.is_empty() => {
                    ui.label(
                        egui::RichText::new("Create a team under Teams to share this project.")
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                None => {
                    ui.horizontal(|ui| {
                        let choice = t
                            .projects
                            .share_team_choice
                            .entry(p.id.clone())
                            .or_insert_with(|| teams[0].id.clone());
                        let selected = teams
                            .iter()
                            .find(|x| &x.id == choice)
                            .map(|x| x.name.clone())
                            .unwrap_or_default();
                        egui::ComboBox::from_id_salt(format!("project_share_team_{}", p.id))
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for team in &teams {
                                    ui.selectable_value(choice, team.id.clone(), &team.name);
                                }
                            });
                        let team = choice.clone();
                        if ui
                            .add_enabled(
                                t.projects.share_rx.is_none(),
                                style::btn_primary_ctx(ui.ctx(), "Share"),
                            )
                            .clicked()
                        {
                            share = Some((p.clone(), team));
                        }
                    });
                }
            }
        });
        ui.add_space(4.0);
    }

    if !theirs.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Shared with me").strong());
        for p in &theirs {
            ui.horizontal(|ui| {
                ui.label(icons::ICON_WORKSPACES.rich_text());
                if ui.link(&p.name).clicked() {
                    edit = Some(p.id.clone());
                }
                ui.label(
                    egui::RichText::new(p.access.as_deref().unwrap_or("viewer"))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        }
    }

    if let Some((p, team)) = share {
        share_project(t, &p, &team);
    }
    if let Some(id) = unshare {
        unshare_project(t, &id);
    }
    if let Some(id) = edit {
        open_edit_dialog(t, &id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::NodeType;

    fn node(name: &str) -> TreeNode {
        TreeNode::new(name.to_string(), NodeType::CustomFolder)
    }

    #[test]
    fn split_and_merge_keep_order() {
        let tree = vec![node("Default"), node("Billing"), node("Shop"), node("Zeta")];
        let (visible, hidden) = split_roots(tree, |n| n.name == "Shop");
        assert_eq!(visible.len(), 1);
        assert_eq!(hidden.len(), 3);
        let merged = merge_roots(visible, hidden);
        let names: Vec<&str> = merged.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["Default", "Billing", "Shop", "Zeta"]);
    }

    #[test]
    fn env_validation() {
        let mut p = Project::new("Shop");
        assert!(validate_envs(&p).is_ok());
        p.environments[1].name = "development".into();
        assert!(validate_envs(&p).is_err());
        let mut q = Project::new("Shop");
        q.environments[0].variables = vec![
            EnvVar {
                key: "A".into(),
                ..Default::default()
            },
            EnvVar {
                key: "A".into(),
                ..Default::default()
            },
        ];
        assert!(validate_envs(&q).is_err());
        q.environments[0].variables[1].key = "B C".into();
        assert!(validate_envs(&q).is_err());
    }
}
