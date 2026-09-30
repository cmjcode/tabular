//! Tab Git Graph (seperti ekstensi Git Graph untuk VS Code): toolbar, tabel
//! commit bergraf dengan label ref, panel detail/perbandingan commit, Find,
//! dan shortcut keyboard. Menu konteks ada di [`super::git_graph_menus`],
//! dialog di [`super::git_graph_dialogs`].
//!
//! Render bekerja pada `GraphState` yang dikeluarkan sementara dari
//! `t.git.graphs`; aksi dikumpulkan sebagai [`Act`] lalu dijalankan setelah
//! state dikembalikan, supaya job yang memakai `t.git.graphs` melihat state
//! terbaru.

use eframe::egui;
use egui_icons::icons as i;

use super::git_graph_jobs::{self as gj, GraphDialog, GraphState, RowKind};
use super::git_graph_paint::{self as paint, Geometry, NodeKind};
use super::git_graph_text::{self as text, IssueLinker};
use super::git_jobs::{self, GitSubMenu, SidebarConfirm};
use super::{git_avatar, git_diff_view, git_view, style};
use crate::git::history::{CommitOrder, DiffRange, FileStat};
use crate::git::history_ops::RepoState;
use crate::git::refs::{RefKind, RefLabel};
use crate::git::repos::{AvatarSource, DateFormat, GraphStyle};
use crate::window_egui::{PrefTab, Tabular};

const ROW_H: f32 = 24.0;
const LANE_W: f32 = 16.0;
const MIN_COL: f32 = 50.0;

/// Aksi dari UI Git Graph.
pub(super) enum Act {
    Select(usize),
    Compare(usize),
    CompareWorking(usize),
    Jump(String),
    CloseDetail,
    Dialog(GraphDialog),
    Execute(GraphDialog),
    Copy(String, &'static str),
    Archive(String),
    SelectFile(usize),
    OpenFileTab(usize),
    RevealFile(String),
    ToggleReview,
    Reload,
    LoadMore,
    Fetch,
    SetBranches(Vec<String>),
    ShowTag(String),
    OpenRemotes,
    SaveRemote,
    FetchRemote(Option<String>),
    PruneRemote(String),
    Checkout(String),
    OpenChanges,
    OpenRepo(String),
    SaveSettings { reload: bool },
    ContinueOp,
    AbortOp(RepoState),
    Cancel,
    CreatePr(String),
}

fn muted(ctx: &egui::Context) -> egui::Color32 {
    style::theme_muted_text(ctx)
}

fn tool_button(ui: &mut egui::Ui, icon: &str, tip: &str, on: bool) -> egui::Response {
    ui.add(
        egui::Button::selectable(on, egui::RichText::new(icon).size(16.0))
            .min_size(egui::vec2(26.0, 24.0)),
    )
    .on_hover_text(tip)
}

/// Render tab Git Graph `tab_id`.
pub fn render(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    tab_id: usize,
    key: &str,
    repo: &std::path::Path,
) {
    if !t.git.graphs.contains_key(&tab_id) {
        // Tab dipulihkan dari sesi lama: buat ulang state-nya.
        gj::attach(t, tab_id, key, repo);
    }
    let Some(mut g) = t.git.graphs.remove(&tab_id) else {
        return;
    };
    let mut acts = Vec::new();
    render_toolbar(t, ui, &mut g, &mut acts);
    render_banner(t, ui, &g, &mut acts);
    if g.find.open {
        render_find(ui, &mut g);
    }
    keyboard(ui, &mut g, &mut acts);
    if g.detail.is_some() {
        egui::Panel::bottom(egui::Id::new(("git_graph_detail", tab_id)))
            .resizable(true)
            .default_size(300.0)
            .min_size(140.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 6)))
            .show(ui, |ui| render_detail(t, ui, tab_id, &mut g, &mut acts));
    }
    render_table(t, ui, tab_id, &mut g, &mut acts);
    super::git_graph_dialogs::render(t, ui.ctx(), &mut g, &mut acts);
    if t.selected_menu != "Git" {
        // Konfirmasi abort memakai dialog sidebar; tampilkan juga di sini.
        super::git_sidebar::render_dialogs(t, ui.ctx());
    }
    let key = g.key.clone();
    t.git.graphs.insert(tab_id, g);
    let ctx = ui.ctx().clone();
    for a in acts {
        handle(t, &ctx, tab_id, &key, a);
    }
}

// ─── Toolbar ────────────────────────────────────────────────────────────────

fn render_toolbar(t: &mut Tabular, ui: &mut egui::Ui, g: &mut GraphState, acts: &mut Vec<Act>) {
    let repos: Vec<(String, String)> = {
        let (mine, other) = git_jobs::visible_repos(t);
        mine.into_iter()
            .chain(other)
            .filter(|r| r.path.is_some())
            .map(|r| (r.key, r.name))
            .collect()
    };
    let name = t
        .git
        .entry(&g.key)
        .map(|e| e.name.clone())
        .unwrap_or_default();
    let busy = t.git.ui(&g.key).and_then(|u| u.busy.clone());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(i::ICON_SOURCE_REPOSITORY.codepoint).size(16.0));
        egui::ComboBox::from_id_salt(("git_graph_repo", &g.key))
            .selected_text(egui::RichText::new(&name).strong())
            .show_ui(ui, |ui| {
                ui.set_min_width(200.0);
                for (k, n) in &repos {
                    if ui.selectable_label(*k == g.key, n).clicked() && *k != g.key {
                        acts.push(Act::OpenRepo(k.clone()));
                    }
                }
            });
        ui.separator();
        ui.label("Branches:");
        branch_filter(t, ui, g, acts);
        let mut show_remote = t.git.store.settings.show_remote_branches;
        if ui
            .checkbox(&mut show_remote, "Show Remote Branches")
            .changed()
        {
            t.git.store.settings.show_remote_branches = show_remote;
            acts.push(Act::SaveSettings { reload: true });
        }
        if g.loading {
            ui.spinner();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if tool_button(ui, i::ICON_REFRESH.codepoint, "Refresh (Ctrl/Cmd+R)", false).clicked() {
                acts.push(Act::Reload);
            }
            if ui
                .add_enabled(
                    busy.is_none(),
                    egui::Button::new(egui::RichText::new(i::ICON_SYNC.codepoint).size(16.0))
                        .min_size(egui::vec2(26.0, 24.0)),
                )
                .on_hover_text("Fetch from remote(s)")
                .clicked()
            {
                acts.push(Act::Fetch);
            }
            ui.menu_button(
                egui::RichText::new(i::ICON_TUNE.codepoint).size(16.0),
                |ui| settings_menu(t, ui, acts),
            )
            .response
            .on_hover_text("Graph settings");
            if tool_button(ui, i::ICON_LAN.codepoint, "Remotes", g.remotes_open).clicked() {
                acts.push(Act::OpenRemotes);
            }
            if tool_button(
                ui,
                i::ICON_SEARCH.codepoint,
                "Find (Ctrl/Cmd+F)",
                g.find.open,
            )
            .clicked()
            {
                g.find.open = !g.find.open;
                g.find.focus = g.find.open;
            }
            if let Some(label) = &busy {
                if ui.small_button("Cancel").clicked() {
                    acts.push(Act::Cancel);
                }
                ui.label(egui::RichText::new(format!("{label}…")).size(12.0));
                ui.spinner();
            }
        });
    });
    if let Some(e) = &g.error {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
    }
    ui.separator();
}

fn branch_filter(t: &Tabular, ui: &mut egui::Ui, g: &mut GraphState, acts: &mut Vec<Act>) {
    let label = match g.branches.as_slice() {
        [] => "Show All".to_string(),
        [one] => one.clone(),
        many => format!("{} branches", many.len()),
    };
    let show_remote = t.git.store.settings.show_remote_branches;
    egui::ComboBox::from_id_salt(("git_graph_branches", &g.key))
        .selected_text(label)
        .width(170.0)
        .height(420.0)
        .show_ui(ui, |ui| {
            ui.set_min_width(240.0);
            git_view::filter_field(ui, &mut g.branch_query, "Filter branches");
            let mut all = g.branches.is_empty();
            if ui.checkbox(&mut all, "Show All").changed() && all {
                acts.push(Act::SetBranches(Vec::new()));
            }
            ui.separator();
            let q = g.branch_query.to_lowercase();
            let mut selected = g.branches.clone();
            let mut changed = false;
            let remote: &[String] = if show_remote {
                g.refs.remote.as_slice()
            } else {
                &[]
            };
            for (title, list) in [("Local", g.refs.local.as_slice()), ("Remote", remote)] {
                if list.is_empty() {
                    continue;
                }
                ui.label(egui::RichText::new(title).size(11.0).color(muted(ui.ctx())));
                for b in list
                    .iter()
                    .filter(|b| q.is_empty() || b.to_lowercase().contains(&q))
                {
                    let mut on = selected.contains(b);
                    if ui.checkbox(&mut on, b).changed() {
                        changed = true;
                        if on {
                            selected.push(b.clone());
                        } else {
                            selected.retain(|x| x != b);
                        }
                    }
                }
            }
            if changed {
                acts.push(Act::SetBranches(selected));
            }
        });
}

fn settings_menu(t: &mut Tabular, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.set_min_width(260.0);
    let mut reload = false;
    let mut save = false;
    {
        let s = &mut t.git.store.settings;
        ui.label(egui::RichText::new("Commit ordering").strong());
        for o in CommitOrder::ALL {
            reload |= ui.radio_value(&mut s.graph_order, o, o.label()).changed();
        }
        ui.separator();
        ui.label(egui::RichText::new("Graph style").strong());
        ui.horizontal(|ui| {
            save |= ui
                .radio_value(&mut s.graph_style, GraphStyle::Rounded, "Rounded")
                .changed();
            save |= ui
                .radio_value(&mut s.graph_style, GraphStyle::Angular, "Angular")
                .changed();
        });
        ui.label(egui::RichText::new("Date format").strong());
        ui.horizontal(|ui| {
            save |= ui
                .radio_value(&mut s.date_format, DateFormat::DateTime, "Date & time")
                .changed();
            save |= ui
                .radio_value(&mut s.date_format, DateFormat::Date, "Date")
                .changed();
            save |= ui
                .radio_value(&mut s.date_format, DateFormat::Relative, "Relative")
                .changed();
        });
        ui.separator();
        ui.label(egui::RichText::new("Columns").strong());
        ui.horizontal(|ui| {
            save |= ui.checkbox(&mut s.show_date, "Date").changed();
            save |= ui.checkbox(&mut s.show_author, "Author").changed();
            save |= ui.checkbox(&mut s.show_commit, "Commit").changed();
        });
        ui.separator();
        reload |= ui.checkbox(&mut s.show_tags, "Show tags").changed();
        reload |= ui.checkbox(&mut s.show_stashes, "Show stashes").changed();
        reload |= ui
            .checkbox(&mut s.show_uncommitted, "Show uncommitted changes")
            .changed();
        reload |= ui
            .checkbox(
                &mut s.first_parent,
                "Only follow the first parent of commits",
            )
            .changed();
        reload |= ui
            .checkbox(
                &mut s.show_reflog,
                "Include commits only mentioned by reflogs",
            )
            .changed();
        save |= ui
            .checkbox(&mut s.mute_merges, "Mute merge commits")
            .changed();
    }
    ui.separator();
    if ui.button("More Git settings…").clicked() {
        t.settings_active_pref_tab = PrefTab::Git;
        t.show_settings_window = true;
        ui.close();
    }
    if reload || save {
        acts.push(Act::SaveSettings { reload });
    }
}

fn render_banner(t: &Tabular, ui: &mut egui::Ui, g: &GraphState, acts: &mut Vec<Act>) {
    let Some(st) = t.git.ui(&g.key).map(|u| u.repo_state) else {
        return;
    };
    if st == RepoState::Clean {
        return;
    }
    style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(i::ICON_WARNING.codepoint)
                    .color(style::theme_warning(ui.ctx())),
            );
            ui.label(format!(
                "{}. Resolve conflicts and stage the files, then continue.",
                st.label()
            ));
            if ui.button("Continue").clicked() {
                acts.push(Act::ContinueOp);
            }
            if ui.button("Abort…").clicked() {
                acts.push(Act::AbortOp(st));
            }
            if ui.button("Open Changes").clicked() {
                acts.push(Act::OpenChanges);
            }
        });
    });
}

// ─── Find ───────────────────────────────────────────────────────────────────

fn find_step(g: &mut GraphState, forward: bool) {
    let n = g.find.matches.len();
    if n == 0 {
        return;
    }
    g.find.current = if forward {
        (g.find.current + 1) % n
    } else {
        (g.find.current + n - 1) % n
    };
    g.scroll_to = Some(g.find.matches[g.find.current]);
}

fn render_find(ui: &mut egui::Ui, g: &mut GraphState) {
    ui.horizontal(|ui| {
        let muted = muted(ui.ctx());
        let resp = style::render_text_field(
            ui,
            egui::TextEdit::singleline(&mut g.find.text)
                .id(egui::Id::new(("git_graph_find", &g.key)))
                .hint_text(
                    egui::RichText::new("Find message, hash, author or branch").color(muted),
                ),
            300.0,
            Some(i::ICON_SEARCH.codepoint),
        );
        if g.find.focus {
            resp.request_focus();
            g.find.focus = false;
        }
        if resp.changed() {
            g.find.current = 0;
            g.update_find();
            if let Some(&first) = g.find.matches.first() {
                g.scroll_to = Some(first);
            }
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let back = ui.input(|i| i.modifiers.shift);
            find_step(g, !back);
            resp.request_focus();
        }
        let n = g.find.matches.len();
        let label = if let Some(e) = &g.find.error {
            e.lines().last().unwrap_or("Invalid pattern").to_string()
        } else if g.find.text.trim().is_empty() {
            String::new()
        } else if n == 0 {
            "No results".to_string()
        } else {
            format!("{} of {n}", g.find.current + 1)
        };
        ui.label(egui::RichText::new(label).size(12.0).color(muted));
        if ui
            .small_button(i::ICON_KEYBOARD_ARROW_UP.codepoint)
            .on_hover_text("Previous (Shift+Enter)")
            .clicked()
        {
            find_step(g, false);
        }
        if ui
            .small_button(i::ICON_KEYBOARD_ARROW_DOWN.codepoint)
            .on_hover_text("Next (Enter)")
            .clicked()
        {
            find_step(g, true);
        }
        let mut refresh = false;
        refresh |= ui
            .toggle_value(&mut g.find.case_sensitive, i::ICON_MATCH_CASE.codepoint)
            .on_hover_text("Match case")
            .changed();
        refresh |= ui
            .toggle_value(&mut g.find.regex, i::ICON_REGULAR_EXPRESSION.codepoint)
            .on_hover_text("Use regular expression")
            .changed();
        if refresh {
            g.update_find();
        }
        if ui
            .small_button(i::ICON_CLOSE.codepoint)
            .on_hover_text("Close (Esc)")
            .clicked()
        {
            g.find.open = false;
        }
    });
}

fn keyboard(ui: &mut egui::Ui, g: &mut GraphState, acts: &mut Vec<Act>) {
    let typing = ui.ctx().egui_wants_keyboard_input();
    let (cmd_f, cmd_r, cmd_h, esc, up, down) = ui.input(|i| {
        let c = i.modifiers.command;
        (
            c && i.key_pressed(egui::Key::F),
            c && i.key_pressed(egui::Key::R),
            c && i.key_pressed(egui::Key::H),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
        )
    });
    if cmd_f {
        g.find.open = true;
        g.find.focus = true;
    }
    if cmd_r {
        acts.push(Act::Reload);
    }
    if cmd_h && let Some(h) = g.head_index() {
        g.scroll_to = Some(h);
        acts.push(Act::Select(h));
    }
    if esc && g.dialog.is_none() {
        if g.find.open {
            g.find.open = false;
        } else if g.detail.is_some() {
            acts.push(Act::CloseDetail);
        }
    }
    if !typing && (up || down) && g.dialog.is_none() {
        let cur = g.selected.as_deref().and_then(|h| g.index_of(h));
        if let Some(cur) = cur {
            let next = if up {
                cur.checked_sub(1)
            } else {
                Some(cur + 1).filter(|n| *n < g.items.len())
            };
            if let Some(n) = next {
                g.scroll_to = Some(n);
                acts.push(Act::Select(n));
            }
        }
    }
}

// ─── Tabel ──────────────────────────────────────────────────────────────────

struct Cols {
    graph: f32,
    desc: f32,
    date: f32,
    author: f32,
    commit: f32,
}

fn columns(t: &Tabular, g: &GraphState, width: f32) -> Cols {
    let s = &t.git.store.settings;
    let [dw, aw, cw] = s.column_widths;
    let date = if s.show_date { dw.max(MIN_COL) } else { 0.0 };
    let author = if s.show_author { aw.max(MIN_COL) } else { 0.0 };
    let commit = if s.show_commit { cw.max(MIN_COL) } else { 0.0 };
    let graph = Geometry::width(g.max_lanes, LANE_W).clamp(40.0, width * 0.4);
    let desc = (width - graph - date - author - commit).max(120.0);
    Cols {
        graph,
        desc,
        date,
        author,
        commit,
    }
}

pub(super) fn format_date(fmt: DateFormat, unix: i64) -> String {
    match fmt {
        DateFormat::DateTime => git_view::format_unix(unix),
        DateFormat::Relative => git_view::relative_time(unix),
        DateFormat::Date => chrono::DateTime::from_timestamp(unix, 0)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%d %b %Y")
                    .to_string()
            })
            .unwrap_or_default(),
    }
}

fn render_header(t: &mut Tabular, ui: &mut egui::Ui, cols: &Cols, acts: &mut Vec<Act>) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
    let color = muted(ui.ctx());
    let font = egui::FontId::proportional(12.0);
    let mut x = rect.left();
    let mut boundaries = Vec::new();
    for (label, w, idx) in [
        ("Graph", cols.graph, None),
        ("Description", cols.desc, None),
        ("Date", cols.date, Some(0usize)),
        ("Author", cols.author, Some(1)),
        ("Commit", cols.commit, Some(2)),
    ] {
        if w <= 0.0 {
            continue;
        }
        if let Some(i) = idx {
            boundaries.push((x, i));
        }
        ui.painter().text(
            egui::pos2(x + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
        x += w;
    }
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, style::nav_track(ui.ctx())),
    );
    // Seret batas kiri kolom Date/Author/Commit untuk mengubah lebarnya.
    for (bx, idx) in boundaries {
        let handle = egui::Rect::from_center_size(
            egui::pos2(bx, rect.center().y),
            egui::vec2(8.0, rect.height()),
        );
        let resp = ui
            .interact(handle, ui.id().with(("gg_col", idx)), egui::Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if resp.hovered() || resp.dragged() {
            ui.painter().vline(
                bx,
                rect.y_range(),
                egui::Stroke::new(1.0, style::nav_text_muted(ui.ctx())),
            );
        }
        if resp.dragged() {
            let w = &mut t.git.store.settings.column_widths[idx];
            *w = (*w - resp.drag_delta().x).clamp(MIN_COL, 480.0);
        }
        if resp.drag_stopped() {
            acts.push(Act::SaveSettings { reload: false });
        }
    }
}

/// Gambar satu label ref; `None` bila tidak muat sebelum `max_x`.
fn ref_pill(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    max_x: f32,
    parts: &[(String, bool)],
    icon: &str,
    color: egui::Color32,
    strong: bool,
) -> Option<egui::Rect> {
    let font = egui::FontId::proportional(12.0);
    let ctx = ui.ctx().clone();
    let painter = ui.painter();
    let icon_g = painter.layout_no_wrap(icon.to_string(), font.clone(), egui::Color32::WHITE);
    let galleys: Vec<_> = parts
        .iter()
        .map(|(s, italic)| {
            let c = if *italic {
                style::nav_text_muted(&ctx)
            } else {
                style::nav_text_strong(&ctx)
            };
            (painter.layout_no_wrap(s.clone(), font.clone(), c), c)
        })
        .collect();
    let icon_w = icon_g.size().x + 8.0;
    let text_w: f32 = galleys.iter().map(|(g, _)| g.size().x + 10.0).sum::<f32>()
        + 2.0 * parts.len().saturating_sub(1) as f32;
    let w = icon_w + text_w;
    if pos.x + w > max_x {
        return None;
    }
    let h = 18.0;
    let rect = egui::Rect::from_min_size(egui::pos2(pos.x, pos.y - h / 2.0), egui::vec2(w, h));
    let bg = if strong {
        color.gamma_multiply(0.35)
    } else {
        style::nav_track(&ctx)
    };
    painter.rect(
        rect,
        3.0,
        bg,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    let icon_rect = egui::Rect::from_min_size(rect.min, egui::vec2(icon_w, h));
    painter.rect_filled(icon_rect, 3.0, color);
    let icon_size = icon_g.size();
    painter.galley(
        egui::pos2(
            icon_rect.center().x - icon_size.x / 2.0,
            rect.center().y - icon_size.y / 2.0,
        ),
        icon_g,
        egui::Color32::WHITE,
    );
    let mut x = icon_rect.right() + 5.0;
    for (idx, (g, c)) in galleys.into_iter().enumerate() {
        if idx > 0 {
            painter.vline(
                x - 3.0,
                rect.y_range().shrink(3.0),
                egui::Stroke::new(1.0, color),
            );
            x += 2.0;
        }
        let size = g.size();
        painter.galley(egui::pos2(x, rect.center().y - size.y / 2.0), g, c);
        x += size.x + 10.0;
    }
    Some(rect)
}

/// Label yang digambar: branch lokal digabung dengan remote bernama sama.
enum Pill {
    Local {
        name: String,
        remotes: Vec<String>,
        current: bool,
    },
    Remote(String),
    Tag {
        name: String,
        annotated: bool,
    },
    Stash(String),
}

fn pills_for(labels: &[RefLabel], stash: Option<&str>) -> Vec<Pill> {
    let mut out = Vec::new();
    let mut used_remote: Vec<&str> = Vec::new();
    for l in labels.iter().filter(|l| l.kind == RefKind::Head) {
        let mut remotes = Vec::new();
        for r in labels {
            if let Some((rem, b)) = r.remote_parts()
                && b == l.name
            {
                used_remote.push(&r.name);
                remotes.push(rem.to_string());
            }
        }
        out.push(Pill::Local {
            name: l.name.clone(),
            remotes,
            current: l.is_current,
        });
    }
    for l in labels
        .iter()
        .filter(|l| l.kind == RefKind::Remote && !used_remote.contains(&l.name.as_str()))
    {
        out.push(Pill::Remote(l.name.clone()));
    }
    for l in labels.iter().filter(|l| l.kind == RefKind::Tag) {
        out.push(Pill::Tag {
            name: l.name.clone(),
            annotated: l.annotated,
        });
    }
    if let Some(s) = stash {
        out.push(Pill::Stash(s.to_string()));
    }
    out
}

/// Ref yang diwakili sebuah label (untuk menu konteks).
pub(super) enum PillRef {
    Local { name: String, current: bool },
    Remote(String),
    Tag { name: String, annotated: bool },
    Stash,
}

fn pill_ref(p: &Pill) -> PillRef {
    match p {
        Pill::Local { name, current, .. } => PillRef::Local {
            name: name.clone(),
            current: *current,
        },
        Pill::Remote(n) => PillRef::Remote(n.clone()),
        Pill::Tag { name, annotated } => PillRef::Tag {
            name: name.clone(),
            annotated: *annotated,
        },
        Pill::Stash(_) => PillRef::Stash,
    }
}

fn render_table(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    tab_id: usize,
    g: &mut GraphState,
    acts: &mut Vec<Act>,
) {
    let cols = columns(t, g, ui.available_width());
    render_header(t, ui, &cols, acts);
    if g.items.is_empty() {
        ui.add_space(12.0);
        if g.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading commits…");
            });
        } else if g.error.is_none() {
            ui.label(egui::RichText::new("No commits yet.").color(muted(ui.ctx())));
        }
        return;
    }
    let settings = t.git.store.settings.clone();
    let ctx = ui.ctx().clone();
    let selected_bg = style::nav_raised(&ctx);
    let hover_bg = style::nav_track(&ctx);
    let panel_bg = ui.visuals().panel_fill;
    let find_bg = style::theme_warning(&ctx).gamma_multiply(0.18);
    let find_cur_bg = style::theme_warning(&ctx).gamma_multiply(0.4);
    let strong = style::nav_text_strong(&ctx);
    let weak = style::nav_text_muted(&ctx);
    let head = g.refs.head.clone();
    let current_match = g.find.matches.get(g.find.current).copied();
    let compare = g.detail.as_ref().and_then(|d| d.compare.clone());
    let n = g.items.len();

    ui.spacing_mut().item_spacing.y = 0.0;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("git_graph_rows", tab_id))
        .auto_shrink([false, false]);
    if let Some(idx) = g.scroll_to.take() {
        let viewport = ui.available_height();
        scroll = scroll.vertical_scroll_offset((idx as f32 * ROW_H - viewport / 3.0).max(0.0));
    }
    scroll.show_rows(ui, ROW_H, n, |ui, range| {
        if range.end + 30 >= n && !g.done && !g.loading {
            acts.push(Act::LoadMore);
        }
        for idx in range {
            let item = g.items[idx].clone();
            let row = g.rows.get(idx).cloned().unwrap_or_default();
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ROW_H),
                egui::Sense::click(),
            );
            let is_sel = g.selected.as_deref() == Some(item.info.hash.as_str())
                || compare.as_deref() == Some(item.info.hash.as_str());
            let bg = if is_sel {
                selected_bg
            } else if current_match == Some(idx) {
                find_cur_bg
            } else if g.find.matches.binary_search(&idx).is_ok() {
                find_bg
            } else if resp.hovered() {
                hover_bg
            } else {
                egui::Color32::TRANSPARENT
            };
            if bg != egui::Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, 0.0, bg);
            }

            // Graf.
            let geo = Geometry {
                left: rect.left(),
                lane_w: LANE_W,
                style: settings.graph_style,
            };
            let node = match item.kind {
                RowKind::Uncommitted => NodeKind::Uncommitted,
                RowKind::Stash => NodeKind::Stash,
                RowKind::Commit if head.as_deref() == Some(item.info.hash.as_str()) => {
                    NodeKind::Head
                }
                RowKind::Commit => NodeKind::Commit,
            };
            let graph_rect = egui::Rect::from_min_size(rect.min, egui::vec2(cols.graph, ROW_H));
            let fill = if bg == egui::Color32::TRANSPARENT {
                panel_bg
            } else {
                bg
            };
            paint::paint_row(
                &ui.painter().with_clip_rect(graph_rect),
                rect,
                &geo,
                &row,
                node,
                fill,
            );

            // Deskripsi: label ref lalu subjek.
            let desc_left = rect.left() + cols.graph;
            let desc_right = desc_left + cols.desc - 6.0;
            let mut x = desc_left + 4.0;
            let color = paint::lane_color(row.color);
            let labels = g.refs.labels(&item.info.hash).to_vec();
            let stash_sel = item.stash.as_ref().map(|s| s.selector.clone());
            for pill in pills_for(&labels, stash_sel.as_deref()) {
                let (parts, icon, strong_pill): (Vec<(String, bool)>, &str, bool) = match &pill {
                    Pill::Local {
                        name,
                        remotes,
                        current,
                    } => {
                        let mut p = vec![(name.clone(), false)];
                        p.extend(remotes.iter().map(|r| (r.clone(), true)));
                        (p, i::MDI_SOURCE_BRANCH.codepoint, *current)
                    }
                    Pill::Remote(n) => (vec![(n.clone(), true)], i::ICON_CLOUD.codepoint, false),
                    Pill::Tag { name, .. } => {
                        (vec![(name.clone(), false)], i::MDI_TAG.codepoint, false)
                    }
                    Pill::Stash(s) => (
                        vec![(s.clone(), false)],
                        i::ICON_INVENTORY_2.codepoint,
                        false,
                    ),
                };
                let Some(prect) = ref_pill(
                    ui,
                    egui::pos2(x, rect.center().y),
                    desc_right - 60.0,
                    &parts,
                    icon,
                    color,
                    strong_pill,
                ) else {
                    break;
                };
                x = prect.right() + 5.0;
                let pid = ui.id().with(("gg_pill", idx, &parts[0].0));
                let presp = ui
                    .interact(prect, pid, egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if presp.double_clicked() {
                    match &pill {
                        Pill::Local { name, current, .. } if !*current => {
                            acts.push(Act::Checkout(name.clone()))
                        }
                        Pill::Remote(n) => acts.push(Act::Dialog(
                            super::git_graph_menus::checkout_remote_dialog(n),
                        )),
                        _ => {}
                    }
                } else if presp.clicked() {
                    acts.push(Act::Select(idx));
                }
                let pref = pill_ref(&pill);
                presp.context_menu(|ui| {
                    super::git_graph_menus::ref_menu(t, ui, g, &item, &pref, acts)
                });
            }
            let subject = text::plain_line(&item.info.subject);
            let is_merge = item.info.parents.len() > 1;
            let subj_color = match item.kind {
                RowKind::Uncommitted => weak,
                _ if is_merge && settings.mute_merges => weak,
                _ => strong,
            };
            let clip = egui::Rect::from_x_y_ranges(x..=desc_right, rect.y_range());
            ui.painter().with_clip_rect(clip).text(
                egui::pos2(x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                subject,
                egui::FontId::proportional(13.0),
                subj_color,
            );

            // Date, Author, Commit.
            let mut cx = desc_left + cols.desc;
            let small = egui::FontId::proportional(12.5);
            if cols.date > 0.0 {
                let clip = egui::Rect::from_x_y_ranges(cx..=cx + cols.date - 6.0, rect.y_range());
                ui.painter().with_clip_rect(clip).text(
                    egui::pos2(cx + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    format_date(settings.date_format, item.info.time),
                    small.clone(),
                    weak,
                );
                cx += cols.date;
            }
            if cols.author > 0.0 {
                let mut ax = cx + 6.0;
                if item.kind != RowKind::Uncommitted {
                    let ar = egui::Rect::from_center_size(
                        egui::pos2(ax + 8.0, rect.center().y),
                        egui::vec2(16.0, 16.0),
                    );
                    git_avatar::paint(
                        ui,
                        &mut t.git.avatars,
                        settings.avatars,
                        ar,
                        &item.info.author,
                        &item.info.email,
                    );
                    ax += 22.0;
                }
                let clip = egui::Rect::from_x_y_ranges(ax..=cx + cols.author - 6.0, rect.y_range());
                ui.painter().with_clip_rect(clip).text(
                    egui::pos2(ax, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &item.info.author,
                    small.clone(),
                    weak,
                );
                cx += cols.author;
            }
            if cols.commit > 0.0 && item.kind != RowKind::Uncommitted {
                ui.painter().text(
                    egui::pos2(cx + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &item.info.short,
                    egui::FontId::monospace(12.0),
                    weak,
                );
            }

            if resp.clicked() {
                if ui.input(|i| i.modifiers.command) {
                    acts.push(Act::Compare(idx));
                } else {
                    acts.push(Act::Select(idx));
                }
            }
            let hint = if item.kind == RowKind::Uncommitted {
                "Click to view the changes. Ctrl/Cmd+click a commit to compare.".to_string()
            } else {
                format!(
                    "{}\n{} <{}>\n{}\nCtrl/Cmd+click to compare with the selected commit",
                    item.info.subject, item.info.author, item.info.email, item.info.hash
                )
            };
            let resp = resp.on_hover_text_at_pointer(hint);
            resp.context_menu(|ui| match item.kind {
                RowKind::Uncommitted => super::git_graph_menus::uncommitted_menu(ui, acts),
                RowKind::Stash => super::git_graph_menus::stash_menu(ui, &item, acts),
                RowKind::Commit => super::git_graph_menus::commit_menu(t, ui, g, idx, &item, acts),
            });
        }
        if g.loading && !g.done {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Loading more commits…").color(weak));
            });
        }
    });
}

// ─── Panel detail ───────────────────────────────────────────────────────────

fn issue_linker(t: &mut Tabular, key: &str) -> Option<IssueLinker> {
    let s = &t.git.store.settings;
    let (pattern, url) = (s.issue_regex.clone(), s.issue_url.clone());
    let template = if url.trim().is_empty() {
        let host = t.git.provider_access().gitlab_host();
        IssueLinker::template_for_key(key, &host)?
    } else {
        url
    };
    IssueLinker::new(&pattern, &template)
}

fn short(h: &str) -> String {
    if h == gj::UNCOMMITTED {
        "Working tree".to_string()
    } else {
        h.chars().take(8).collect()
    }
}

fn render_detail(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    tab_id: usize,
    g: &mut GraphState,
    acts: &mut Vec<Act>,
) {
    let key = g.key.clone();
    let linker = issue_linker(t, &key);
    let avatars = t.git.store.settings.avatars;
    let date_fmt = t.git.store.settings.date_format;
    let items = g.items.clone();
    let refs = g.refs.clone();
    let Some(d) = g.detail.as_mut() else {
        return;
    };
    let muted = muted(ui.ctx());

    ui.horizontal(|ui| {
        let title = match (&d.compare, &d.range) {
            (Some(_), DiffRange::Between { from, to }) => {
                format!("Comparing {} → {}", short(from), short(to))
            }
            (Some(_), DiffRange::WorkingTree { from }) => {
                format!("Comparing {} → working tree", short(from))
            }
            _ if d.primary == gj::UNCOMMITTED => "Uncommitted Changes".to_string(),
            _ => format!("Commit {}", short(&d.primary)),
        };
        ui.label(egui::RichText::new(title).strong().size(14.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button(i::ICON_CLOSE.codepoint)
                .on_hover_text("Close (Esc)")
                .clicked()
            {
                acts.push(Act::CloseDetail);
            }
            let review_label = if d.reviewing {
                format!("{} End Code Review", i::ICON_RATE_REVIEW.codepoint)
            } else {
                format!("{} Start Code Review", i::ICON_RATE_REVIEW.codepoint)
            };
            if ui
                .add(egui::Button::selectable(d.reviewing, review_label))
                .on_hover_text(
                    "Files you open are marked as reviewed; marks are kept for this commit range",
                )
                .clicked()
            {
                acts.push(Act::ToggleReview);
            }
            ui.selectable_value(&mut d.tree, false, i::ICON_FORMAT_LIST_BULLETED.codepoint)
                .on_hover_text("List view");
            ui.selectable_value(&mut d.tree, true, i::ICON_ACCOUNT_TREE.codepoint)
                .on_hover_text("Tree view");
        });
    });
    ui.separator();

    let avail = ui.available_size();
    let meta_w = (avail.x * 0.30).clamp(220.0, 460.0);
    let files_w = (avail.x * 0.24).clamp(200.0, 380.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(meta_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(meta_w, avail.y));
                egui::ScrollArea::vertical()
                    .id_salt(("gg_meta", tab_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        render_meta(
                            t,
                            ui,
                            d,
                            &items,
                            &refs,
                            linker.as_ref(),
                            avatars,
                            date_fmt,
                            acts,
                        )
                    });
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(files_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(files_w, avail.y));
                render_files(t, ui, tab_id, &key, d, acts);
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| render_file_diff(ui, tab_id, d, muted, acts),
        );
    });
}

fn render_file_diff(
    ui: &mut egui::Ui,
    tab_id: usize,
    d: &mut gj::DetailState,
    muted: egui::Color32,
    acts: &mut Vec<Act>,
) {
    let Some(fi) = d.selected_file else {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Select a file to view its changes.").color(muted));
        return;
    };
    let path = d
        .files
        .get(fi)
        .map(|f| f.change.path.clone())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&path).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button(i::ICON_OPEN_IN_NEW.codepoint)
                .on_hover_text("Open diff in a tab")
                .clicked()
            {
                acts.push(Act::OpenFileTab(fi));
            }
            ui.selectable_value(&mut d.side_by_side, true, "Side by side");
            ui.selectable_value(&mut d.side_by_side, false, "Unified");
        });
    });
    if let Some(e) = &d.diff_error {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
    } else if d.diff_loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading diff…");
        });
    } else if d.binary {
        ui.label(egui::RichText::new("Binary file or diff not available.").color(muted));
    } else if d.rows.is_empty() {
        ui.label(egui::RichText::new("No changes to show.").color(muted));
    } else {
        let limit = (!d.show_all_rows).then_some(git_jobs::MAX_DIFF_ROWS);
        if git_diff_view::render_diff(ui, ("gg_diff", tab_id), &d.rows, d.side_by_side, limit) {
            d.show_all_rows = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_meta(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    d: &gj::DetailState,
    items: &[gj::GraphItem],
    refs: &crate::git::refs::RefSet,
    linker: Option<&IssueLinker>,
    avatars: AvatarSource,
    date_fmt: DateFormat,
    acts: &mut Vec<Act>,
) {
    let muted = muted(ui.ctx());
    if let Some(e) = &d.error {
        ui.colored_label(style::theme_danger(ui.ctx()), e);
        return;
    }
    let (adds, dels) = totals(&d.files);
    let summary = format!(
        "{} files changed, {adds} insertions(+), {dels} deletions(-)",
        d.files.len()
    );
    if d.compare.is_some() {
        ui.label(summary);
        ui.add_space(4.0);
        for h in [Some(&d.primary), d.compare.as_ref()].into_iter().flatten() {
            if let Some(it) = items.iter().find(|i| i.info.hash == *h) {
                ui.horizontal_wrapped(|ui| {
                    if ui.link(egui::RichText::new(short(h)).monospace()).clicked() {
                        acts.push(Act::Jump(h.clone()));
                    }
                    ui.label(text::emojify(&it.info.subject));
                });
            }
        }
        return;
    }
    if d.primary == gj::UNCOMMITTED {
        ui.label(summary);
        ui.add_space(4.0);
        if ui.button("Open in Changes sidebar").clicked() {
            acts.push(Act::OpenChanges);
        }
        return;
    }
    let Some(c) = &d.details else {
        if d.loading {
            ui.spinner();
        }
        return;
    };
    let field = |ui: &mut egui::Ui, label: &str| {
        ui.label(egui::RichText::new(label).color(muted).size(12.0));
    };
    egui::Grid::new(("gg_meta_grid", &c.hash))
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            field(ui, "Commit");
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(short(&c.hash)).monospace());
                if ui
                    .small_button(i::ICON_CONTENT_COPY.codepoint)
                    .on_hover_text(&c.hash)
                    .clicked()
                {
                    acts.push(Act::Copy(c.hash.clone(), "Commit hash"));
                }
            });
            ui.end_row();
            field(ui, "Parents");
            ui.horizontal_wrapped(|ui| {
                if c.parents.is_empty() {
                    ui.label(egui::RichText::new("None (root commit)").color(muted));
                }
                for p in &c.parents {
                    if ui
                        .link(egui::RichText::new(short(p)).monospace())
                        .on_hover_text("Go to parent")
                        .clicked()
                    {
                        acts.push(Act::Jump(p.clone()));
                    }
                }
            });
            ui.end_row();
            field(ui, "Author");
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                git_avatar::paint(
                    ui,
                    &mut t.git.avatars,
                    avatars,
                    r,
                    &c.author,
                    &c.author_email,
                );
                ui.label(&c.author).on_hover_text(&c.author_email);
            });
            ui.end_row();
            field(ui, "Date");
            ui.label(format_date(date_fmt, c.author_time))
                .on_hover_text(git_view::format_unix(c.author_time));
            ui.end_row();
            if c.committer != c.author || c.commit_time != c.author_time {
                field(ui, "Committer");
                ui.label(&c.committer).on_hover_text(&c.committer_email);
                ui.end_row();
                field(ui, "Committed");
                ui.label(format_date(date_fmt, c.commit_time));
                ui.end_row();
            }
            if let Some(sig) = c.signature_label() {
                field(ui, "Signature");
                let color = match c.signature {
                    'G' => style::theme_success(ui.ctx()),
                    'B' => style::theme_danger(ui.ctx()),
                    _ => style::theme_warning(ui.ctx()),
                };
                ui.label(
                    egui::RichText::new(format!("{} {sig}", i::ICON_VERIFIED.codepoint))
                        .color(color),
                )
                .on_hover_text(format!("{}\nKey {}", c.signer, c.signing_key));
                ui.end_row();
            }
            let labels = refs.labels(&c.hash);
            if !labels.is_empty() {
                field(ui, "Refs");
                ui.horizontal_wrapped(|ui| {
                    for l in labels {
                        let icon = match l.kind {
                            RefKind::Head => i::MDI_SOURCE_BRANCH.codepoint,
                            RefKind::Remote => i::ICON_CLOUD.codepoint,
                            RefKind::Tag => i::MDI_TAG.codepoint,
                        };
                        let rt = egui::RichText::new(format!("{icon} {}", l.name)).size(12.0);
                        ui.label(if l.is_current { rt.strong() } else { rt });
                    }
                });
                ui.end_row();
            }
        });
    ui.separator();
    text::render_message(ui, &c.body, linker, 13.0);
}

fn totals(files: &[FileStat]) -> (u64, u64) {
    files.iter().fold((0, 0), |(a, d), f| {
        (a + f.additions.unwrap_or(0), d + f.deletions.unwrap_or(0))
    })
}

/// Pohon folder untuk tampilan file.
#[derive(Default)]
struct Dir {
    dirs: std::collections::BTreeMap<String, Dir>,
    files: Vec<usize>,
}

fn build_tree(files: &[FileStat]) -> Dir {
    let mut root = Dir::default();
    for (idx, f) in files.iter().enumerate() {
        let mut node = &mut root;
        let parts: Vec<&str> = f.change.path.split('/').collect();
        for p in &parts[..parts.len().saturating_sub(1)] {
            node = node.dirs.entry((*p).to_string()).or_default();
        }
        node.files.push(idx);
    }
    root
}

fn render_files(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    tab_id: usize,
    key: &str,
    d: &mut gj::DetailState,
    acts: &mut Vec<Act>,
) {
    let muted = muted(ui.ctx());
    if d.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading files…");
        });
        return;
    }
    let (adds, dels) = totals(&d.files);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} files", d.files.len()))
                .size(12.0)
                .color(muted),
        );
        ui.label(
            egui::RichText::new(format!("+{adds}"))
                .size(12.0)
                .color(style::theme_success(ui.ctx())),
        );
        ui.label(
            egui::RichText::new(format!("-{dels}"))
                .size(12.0)
                .color(style::theme_danger(ui.ctx())),
        );
    });
    let reviewed: Vec<bool> = d
        .files
        .iter()
        .map(|f| gj::is_reviewed(t, key, &d.range, &f.change.path))
        .collect();
    if d.reviewing {
        let done = reviewed.iter().filter(|r| **r).count();
        ui.label(
            egui::RichText::new(format!("Reviewed {done} of {}", d.files.len()))
                .size(11.5)
                .color(style::theme_info(ui.ctx())),
        );
    }
    egui::ScrollArea::vertical()
        .id_salt(("gg_files", tab_id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if d.tree {
                let tree = build_tree(&d.files);
                render_dir(ui, &tree, "", 0, d, &reviewed, acts);
            } else {
                for idx in 0..d.files.len() {
                    file_line(ui, d, idx, &reviewed, 0, false, acts);
                }
            }
        });
}

fn render_dir(
    ui: &mut egui::Ui,
    dir: &Dir,
    prefix: &str,
    depth: usize,
    d: &mut gj::DetailState,
    reviewed: &[bool],
    acts: &mut Vec<Act>,
) {
    for (name, sub) in &dir.dirs {
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let open = !d.collapsed.contains(&full);
        let icon = if open {
            i::ICON_EXPAND_MORE.codepoint
        } else {
            i::ICON_CHEVRON_RIGHT.codepoint
        };
        let resp = ui.add(
            egui::Button::new(
                egui::RichText::new(format!(
                    "{}{icon} {} {name}",
                    "    ".repeat(depth),
                    i::ICON_FOLDER.codepoint
                ))
                .size(12.5),
            )
            .frame(false),
        );
        if resp.clicked() {
            if open {
                d.collapsed.insert(full.clone());
            } else {
                d.collapsed.remove(&full);
            }
        }
        if open {
            render_dir(ui, sub, &full, depth + 1, d, reviewed, acts);
        }
    }
    for &idx in &dir.files {
        file_line(ui, d, idx, reviewed, depth, true, acts);
    }
}

fn file_line(
    ui: &mut egui::Ui,
    d: &gj::DetailState,
    idx: usize,
    reviewed: &[bool],
    depth: usize,
    name_only: bool,
    acts: &mut Vec<Act>,
) {
    let Some(f) = d.files.get(idx) else {
        return;
    };
    let ctx = ui.ctx().clone();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
    if d.selected_file == Some(idx) {
        ui.painter().rect_filled(rect, 3.0, style::nav_raised(&ctx));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, style::nav_track(&ctx));
    }
    let painter = ui.painter().with_clip_rect(rect);
    let mut x = rect.left() + 6.0 + depth as f32 * 14.0;
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        f.change.kind.letter(),
        egui::FontId::proportional(12.0),
        git_view::kind_color(&ctx, f.change.kind),
    );
    x += 14.0;
    let (dir, name) = match f.change.path.rsplit_once('/') {
        Some((d, n)) => (d, n),
        None => ("", f.change.path.as_str()),
    };
    let stats = match (f.additions, f.deletions) {
        (Some(a), Some(dl)) => format!("+{a} -{dl}"),
        _ => "bin".to_string(),
    };
    let weak = style::nav_text_muted(&ctx);
    let stats_g = painter.layout_no_wrap(stats, egui::FontId::proportional(11.0), weak);
    let right = rect.right() - 6.0 - stats_g.size().x;
    let sy = rect.center().y - stats_g.size().y / 2.0;
    painter.galley(egui::pos2(right, sy), stats_g, weak);
    let done = reviewed.get(idx).copied().unwrap_or(false);
    let text_clip = egui::Rect::from_x_y_ranges(x..=right - 18.0, rect.y_range());
    let tp = ui.painter().with_clip_rect(text_clip);
    let name_color = if done {
        weak
    } else {
        style::nav_text_strong(&ctx)
    };
    let ng = tp.layout_no_wrap(
        name.to_string(),
        egui::FontId::proportional(12.5),
        name_color,
    );
    let (nw, nh) = (ng.size().x, ng.size().y);
    tp.galley(egui::pos2(x, rect.center().y - nh / 2.0), ng, name_color);
    if !name_only && !dir.is_empty() {
        tp.text(
            egui::pos2(x + nw + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            dir,
            egui::FontId::proportional(11.0),
            weak,
        );
    }
    if done {
        ui.painter().text(
            egui::pos2(right - 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            i::ICON_CHECK_CIRCLE.codepoint,
            egui::FontId::proportional(12.0),
            style::theme_success(&ctx),
        );
    }
    if resp.double_clicked() {
        acts.push(Act::OpenFileTab(idx));
    } else if resp.clicked() {
        acts.push(Act::SelectFile(idx));
    }
    let path = f.change.path.clone();
    let resp = resp.on_hover_text(match &f.change.orig_path {
        Some(o) => format!("{o} → {path}"),
        None => path.clone(),
    });
    resp.context_menu(|ui| {
        if ui.button("View diff").clicked() {
            acts.push(Act::SelectFile(idx));
            ui.close();
        }
        if ui.button("Open diff in a tab").clicked() {
            acts.push(Act::OpenFileTab(idx));
            ui.close();
        }
        if ui.button("Reveal in file manager").clicked() {
            acts.push(Act::RevealFile(path.clone()));
            ui.close();
        }
        if ui.button("Copy relative path").clicked() {
            acts.push(Act::Copy(path.clone(), "Path"));
            ui.close();
        }
    });
}

// ─── Aksi ───────────────────────────────────────────────────────────────────

fn handle(t: &mut Tabular, ctx: &egui::Context, tab_id: usize, key: &str, a: Act) {
    match a {
        Act::Select(i) => gj::select(t, tab_id, i),
        Act::Compare(i) => gj::compare(t, tab_id, i),
        Act::CompareWorking(i) => gj::compare_with_working_tree(t, tab_id, i),
        Act::Jump(hash) => {
            let idx = t.git.graphs.get(&tab_id).and_then(|g| g.index_of(&hash));
            match idx {
                Some(i) => {
                    if let Some(g) = t.git.graphs.get_mut(&tab_id) {
                        g.scroll_to = Some(i);
                    }
                    gj::select(t, tab_id, i);
                }
                None => t.toasts.info("That commit is not loaded in the graph yet"),
            }
        }
        Act::CloseDetail => gj::close_detail(t, tab_id),
        Act::Dialog(d) => {
            if let Some(g) = t.git.graphs.get_mut(&tab_id) {
                g.dialog = Some(d);
            }
        }
        Act::Execute(d) => {
            if !gj::execute(t, tab_id, d.clone())
                && let Some(g) = t.git.graphs.get_mut(&tab_id)
            {
                g.dialog = Some(d);
            }
        }
        Act::Copy(s, what) => {
            ctx.copy_text(s);
            t.toasts.info(format!("{what} copied"));
        }
        Act::Archive(rev) => gj::archive(t, tab_id, rev),
        Act::SelectFile(i) => gj::select_file(t, tab_id, i),
        Act::OpenFileTab(i) => gj::open_file_tab(t, tab_id, i),
        Act::RevealFile(p) => {
            if let Some(repo) = t.git.path_of(key) {
                let full = repo.join(&p);
                let dir = full
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or(repo);
                if let Err(e) = crate::url_opener::open_folder(&dir) {
                    t.toasts.error(e);
                }
            }
        }
        Act::ToggleReview => gj::toggle_review(t, tab_id),
        Act::Reload => gj::reload(t, tab_id),
        Act::LoadMore => gj::load_more(t, tab_id),
        Act::Fetch => gj::fetch_remote(t, tab_id, None),
        Act::SetBranches(b) => gj::set_branches(t, tab_id, b),
        Act::ShowTag(n) => gj::show_tag(t, tab_id, n),
        Act::OpenRemotes => gj::load_remotes(t, tab_id),
        Act::SaveRemote => gj::save_remote(t, tab_id),
        Act::FetchRemote(r) => gj::fetch_remote(t, tab_id, r),
        Act::PruneRemote(r) => gj::prune_remote(t, tab_id, r),
        Act::Checkout(name) => git_jobs::checkout(t, key, name, false),
        Act::OpenChanges => {
            t.selected_menu = "Git".to_string();
            git_jobs::set_expanded(t, key, true);
            git_jobs::set_sub(t, key, GitSubMenu::Changes);
        }
        Act::OpenRepo(k) => gj::open_graph(t, &k),
        Act::SaveSettings { reload } => {
            if let Err(e) = t.git.save_store() {
                t.toasts.error(e);
            }
            if reload {
                gj::refresh_open_graphs(t);
            }
        }
        Act::ContinueOp => git_jobs::continue_operation(t, key),
        Act::AbortOp(st) => {
            t.git.confirm = Some((key.to_string(), SidebarConfirm::AbortOperation(st)));
        }
        Act::Cancel => git_jobs::cancel_op(t, key),
        Act::CreatePr(branch) => match gj::pull_request_url(t, key, &branch) {
            Some(u) => {
                if let Err(e) = crate::url_opener::open_url(&u) {
                    t.toasts.error(e);
                }
            }
            None => t
                .toasts
                .info("Pull requests need a GitHub or GitLab remote"),
        },
    }
}
