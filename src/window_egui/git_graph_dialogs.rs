//! Dialog Git Graph: input/konfirmasi aksi riwayat, kelola remote, dan detail
//! tag. Semua memakai gaya modal aplikasi (backdrop + kartu).

use eframe::egui;

use super::git_graph_jobs::{GraphDialog, GraphState, RemoteForm, rev_label};
use super::git_graph_view::Act;
use super::style;
use crate::git::history_ops::{ForceMode, ResetMode};
use crate::window_egui::Tabular;

fn remote_combo(ui: &mut egui::Ui, id: &str, remotes: &[String], value: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .show_ui(ui, |ui| {
            for r in remotes {
                ui.selectable_value(value, r.clone(), r);
            }
        });
}

fn text_field(ui: &mut egui::Ui, value: &mut String, hint: &str, focus: bool) {
    let resp = style::render_text_field(
        ui,
        egui::TextEdit::singleline(value)
            .hint_text(egui::RichText::new(hint).color(style::nav_text_muted(ui.ctx()))),
        f32::INFINITY,
        None,
    );
    if focus && ui.memory(|m| m.focused().is_none()) {
        resp.request_focus();
    }
}

fn parent_choice(ui: &mut egui::Ui, parents: usize, value: &mut u32) {
    ui.horizontal(|ui| {
        ui.label("Parent:");
        for p in 1..=parents as u32 {
            ui.radio_value(value, p, format!("{p}"));
        }
    });
    ui.label(
        egui::RichText::new(
            "This is a merge commit. Choose the parent the changes are relative to (1 is usually the branch that was merged into).",
        )
        .size(11.5)
        .color(style::theme_muted_text(ui.ctx())),
    );
}

/// Judul dialog dan label tombol konfirmasi.
fn labels(d: &GraphDialog) -> (&'static str, &'static str) {
    match d {
        GraphDialog::CreateBranch { .. } => ("Create Branch", "Create Branch"),
        GraphDialog::AddTag { .. } => ("Add Tag", "Add Tag"),
        GraphDialog::CheckoutCommit { .. } => ("Checkout Commit", "Checkout"),
        GraphDialog::CheckoutBranch { .. } => ("Checkout Branch", "Checkout"),
        GraphDialog::Merge { .. } => ("Merge", "Merge"),
        GraphDialog::Rebase { .. } => ("Rebase", "Rebase"),
        GraphDialog::CherryPick { .. } => ("Cherry Pick", "Cherry Pick"),
        GraphDialog::Revert { .. } => ("Revert Commit", "Revert"),
        GraphDialog::DropCommit { .. } => ("Drop Commit", "Drop"),
        GraphDialog::Reset { .. } => ("Reset Branch", "Reset"),
        GraphDialog::RenameBranch { .. } => ("Rename Branch", "Rename"),
        GraphDialog::DeleteBranch { .. } => ("Delete Branch", "Delete"),
        GraphDialog::DeleteRemoteBranch { .. } => ("Delete Remote Branch", "Delete"),
        GraphDialog::PushBranch { .. } => ("Push Branch", "Push"),
        GraphDialog::PullInto { .. } => ("Pull Branch", "Pull"),
        GraphDialog::FetchInto { .. } => ("Fetch into Local Branch", "Fetch"),
        GraphDialog::DeleteTag { .. } => ("Delete Tag", "Delete"),
        GraphDialog::PushTag { .. } => ("Push Tag", "Push"),
        GraphDialog::StashApply { pop: true, .. } => ("Pop Stash", "Pop"),
        GraphDialog::StashApply { .. } => ("Apply Stash", "Apply"),
        GraphDialog::StashDrop { .. } => ("Drop Stash", "Drop"),
        GraphDialog::StashBranch { .. } => ("Create Branch from Stash", "Create Branch"),
        GraphDialog::StashPush { .. } => ("Stash Changes", "Stash"),
        GraphDialog::ResetUncommitted { .. } => ("Reset Uncommitted Changes", "Reset"),
        GraphDialog::CleanUntracked { .. } => ("Clean Untracked Files", "Clean"),
        GraphDialog::RemoveRemote { .. } => ("Remove Remote", "Remove"),
    }
}

/// Pilihan "juga di remote" untuk hapus branch/tag.
fn also_remote(ui: &mut egui::Ui, id: &str, remotes: &[String], remote: &mut Option<String>) {
    if remotes.is_empty() {
        return;
    }
    let mut on = remote.is_some();
    ui.horizontal(|ui| {
        ui.checkbox(&mut on, "Also delete it on remote");
        if on {
            let r = remote.get_or_insert_with(|| remotes[0].clone());
            remote_combo(ui, id, remotes, r);
        }
    });
    if !on {
        *remote = None;
    }
}

fn body(ui: &mut egui::Ui, d: &mut GraphDialog, remotes: &[String], current: &str) {
    let muted = style::theme_muted_text(ui.ctx());
    let cb = if current.is_empty() {
        "the current branch".to_string()
    } else {
        format!("\"{current}\"")
    };
    match d {
        GraphDialog::CreateBranch {
            at,
            name,
            checkout,
            force,
        } => {
            ui.label(format!("Create a branch at {}:", rev_label(at)));
            text_field(ui, name, "branch-name", true);
            ui.checkbox(checkout, "Check out the new branch");
            ui.checkbox(force, "Overwrite an existing branch with the same name");
        }
        GraphDialog::AddTag {
            at,
            name,
            annotated,
            message,
            push,
            remote,
            force,
        } => {
            ui.label(format!("Add a tag to commit {}:", rev_label(at)));
            text_field(ui, name, "v1.0.0", true);
            ui.horizontal(|ui| {
                ui.radio_value(annotated, true, "Annotated");
                ui.radio_value(annotated, false, "Lightweight");
            });
            if *annotated {
                ui.add(
                    egui::TextEdit::multiline(message)
                        .hint_text("Message")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            }
            if !remotes.is_empty() {
                ui.horizontal(|ui| {
                    ui.checkbox(push, "Push to remote");
                    if *push {
                        remote_combo(ui, "gg_tag_remote", remotes, remote);
                    }
                });
            }
            ui.checkbox(force, "Replace an existing tag with the same name");
        }
        GraphDialog::CheckoutCommit { hash } => {
            ui.label(format!(
                "Check out commit {}? You will be in \"detached HEAD\" state; create a branch to keep new commits.",
                rev_label(hash)
            ));
        }
        GraphDialog::CheckoutBranch {
            name, local_name, ..
        } => {
            ui.label(format!("Create a local branch that tracks \"{name}\":"));
            text_field(ui, local_name, "local-branch", true);
        }
        GraphDialog::Merge { what, opt, .. } => {
            ui.label(format!("Merge {what} into {cb}?"));
            ui.checkbox(
                &mut opt.no_ff,
                "Create a new commit even if fast-forward is possible",
            );
            ui.checkbox(&mut opt.squash, "Squash commits");
            ui.checkbox(&mut opt.no_commit, "Don't commit (stage the result only)");
        }
        GraphDialog::Rebase {
            what, ignore_date, ..
        } => {
            ui.label(format!("Rebase {cb} on {what}?"));
            ui.checkbox(
                ignore_date,
                "Ignore date (use the current time for rebased commits)",
            );
            ui.label(
                egui::RichText::new("Rebasing rewrites the history of the current branch.")
                    .size(11.5)
                    .color(muted),
            );
        }
        GraphDialog::CherryPick { hash, parents, opt } => {
            ui.label(format!("Cherry pick commit {} onto {cb}?", rev_label(hash)));
            if *parents > 1 {
                let mut m = opt.mainline.unwrap_or(1);
                parent_choice(ui, *parents, &mut m);
                opt.mainline = Some(m);
            }
            ui.checkbox(
                &mut opt.record_origin,
                "Record origin (append \"cherry picked from commit …\")",
            );
            ui.checkbox(&mut opt.no_commit, "Don't commit (stage the changes only)");
        }
        GraphDialog::Revert {
            hash,
            parents,
            mainline,
        } => {
            ui.label(format!(
                "Revert commit {}? A new commit undoes its changes.",
                rev_label(hash)
            ));
            if *parents > 1 {
                parent_choice(ui, *parents, mainline);
            }
        }
        GraphDialog::DropCommit { hash } => {
            ui.label(format!(
                "Drop commit {} from {cb}? This rewrites history and cannot be undone from Tabular.",
                rev_label(hash)
            ));
        }
        GraphDialog::Reset { hash, mode } => {
            ui.label(format!("Reset {cb} to commit {}?", rev_label(hash)));
            for m in ResetMode::ALL {
                ui.radio_value(mode, m, m.label());
            }
        }
        GraphDialog::RenameBranch { old, new } => {
            ui.label(format!("Rename branch \"{old}\" to:"));
            text_field(ui, new, "new-name", true);
        }
        GraphDialog::DeleteBranch {
            name,
            force,
            remote,
        } => {
            ui.label(format!("Delete branch \"{name}\"?"));
            ui.checkbox(force, "Force delete (the branch has unmerged commits)");
            also_remote(ui, "gg_del_remote", remotes, remote);
        }
        GraphDialog::DeleteRemoteBranch { remote, branch } => {
            ui.label(format!(
                "Delete branch \"{branch}\" on remote \"{remote}\"? Other people lose access to it."
            ));
        }
        GraphDialog::PushBranch {
            branch,
            remote,
            set_upstream,
            force,
        } => {
            ui.label(format!("Push branch \"{branch}\" to:"));
            remote_combo(ui, "gg_push_remote", remotes, remote);
            ui.checkbox(set_upstream, "Set upstream (track the remote branch)");
            ui.label("Push mode:");
            ui.radio_value(force, ForceMode::None, "Normal");
            ui.radio_value(
                force,
                ForceMode::Lease,
                "Force with lease (refuses if the remote changed since your last fetch)",
            );
            ui.radio_value(
                force,
                ForceMode::Force,
                "Force (overwrites the remote branch)",
            );
        }
        GraphDialog::PullInto {
            remote,
            branch,
            opt,
        } => {
            ui.label(format!("Pull \"{remote}/{branch}\" into {cb}?"));
            ui.checkbox(
                &mut opt.no_ff,
                "Create a new commit even if fast-forward is possible",
            );
            ui.checkbox(&mut opt.squash, "Squash commits");
        }
        GraphDialog::FetchInto {
            remote,
            branch,
            local,
            force,
        } => {
            ui.label(format!(
                "Update a local branch from \"{remote}/{branch}\" without checking it out:"
            ));
            text_field(ui, local, "local-branch", true);
            ui.checkbox(force, "Force (allow a non fast-forward update)");
        }
        GraphDialog::DeleteTag { name, remote } => {
            ui.label(format!("Delete tag \"{name}\"?"));
            also_remote(ui, "gg_deltag_remote", remotes, remote);
        }
        GraphDialog::PushTag { name, remote } => {
            ui.label(format!("Push tag \"{name}\" to:"));
            remote_combo(ui, "gg_pushtag_remote", remotes, remote);
        }
        GraphDialog::StashApply {
            selector,
            pop,
            index,
        } => {
            ui.label(if *pop {
                format!("Apply {selector} and remove it from the stash list?")
            } else {
                format!("Apply {selector} to the working tree?")
            });
            ui.checkbox(
                index,
                "Reinstate the index (restore staged changes as staged)",
            );
        }
        GraphDialog::StashDrop { selector } => {
            ui.label(format!("Drop {selector}? Its changes will be lost."));
        }
        GraphDialog::StashBranch { selector, name } => {
            ui.label(format!(
                "Create a branch from {selector}, check it out, and apply the stash:"
            ));
            text_field(ui, name, "branch-name", true);
        }
        GraphDialog::StashPush { message, untracked } => {
            ui.label("Stash the uncommitted changes:");
            text_field(ui, message, "Message (optional)", true);
            ui.checkbox(untracked, "Include untracked files");
        }
        GraphDialog::ResetUncommitted { mode } => {
            ui.label("Reset uncommitted changes to HEAD?");
            ui.radio_value(
                mode,
                ResetMode::Mixed,
                "Mixed - unstage everything, keep the files as they are",
            );
            ui.radio_value(
                mode,
                ResetMode::Hard,
                "Hard - discard all changes to tracked files",
            );
        }
        GraphDialog::CleanUntracked { dirs } => {
            ui.label("Remove all untracked files from disk? This cannot be undone.");
            ui.checkbox(dirs, "Also remove untracked directories");
        }
        GraphDialog::RemoveRemote { name } => {
            ui.label(format!(
                "Remove remote \"{name}\"? Its remote-tracking branches are removed too."
            ));
        }
    }
}

fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    width: f32,
    close: &mut bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    style::render_modal_backdrop(ctx, &format!("{id}_backdrop"), true);
    egui::Window::new(id)
        .id(egui::Id::new(id))
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(width)
        .show(ctx, |ui| {
            style::render_modal_header(ui, title, close);
            add(ui)
        })
        .and_then(|r| r.inner)
}

/// Render dialog yang terbuka (aksi, remote, detail tag).
pub(super) fn render(
    t: &mut Tabular,
    ctx: &egui::Context,
    g: &mut GraphState,
    acts: &mut Vec<Act>,
) {
    if let Some(mut d) = g.dialog.take() {
        let remotes = g.refs.remotes.clone();
        let current = g.refs.head_branch.clone().unwrap_or_default();
        let danger = d.is_dangerous();
        let (title, label) = labels(&d);
        let mut close = false;
        let mut cancel = false;
        let mut confirm = false;
        modal(ctx, "git_graph_dialog", title, 440.0, &mut close, |ui| {
            style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_min_width(400.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                body(ui, &mut d, &remotes, &current);
            });
            ui.add_space(8.0);
            // `horizontal` membatasi tinggi baris tombol; tanpa itu layout
            // kanan-ke-kiri mengambil seluruh tinggi jendela.
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = if danger {
                        style::btn_danger_ctx(ui.ctx(), label)
                    } else {
                        style::btn_primary_ctx(ui.ctx(), label)
                    };
                    if ui.add(btn).clicked() {
                        confirm = true;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        });
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if confirm {
            acts.push(Act::Execute(d));
        } else if !close {
            g.dialog = Some(d);
        }
        // Dialog aksi di atas dialog Remotes: jangan gambar keduanya.
        return;
    }

    if g.remotes_open {
        render_remotes(t, ctx, g, acts);
    }

    if let Some(tag) = g.tag_view.clone() {
        let mut close = false;
        modal(
            ctx,
            "git_graph_tag",
            &format!("Tag {}", tag.name),
            420.0,
            &mut close,
            |ui| {
                style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.set_min_width(380.0);
                    egui::Grid::new("gg_tag_grid")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Object");
                            ui.label(egui::RichText::new(rev_label(&tag.hash)).monospace());
                            ui.end_row();
                            ui.label("Tagger");
                            ui.label(format!("{} {}", tag.tagger, tag.email));
                            ui.end_row();
                            ui.label("Date");
                            ui.label(super::git_view::format_unix(tag.time));
                            ui.end_row();
                        });
                    ui.separator();
                    ui.label(&tag.message);
                });
            },
        );
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            g.tag_view = None;
        }
    }
}

fn render_remotes(t: &mut Tabular, ctx: &egui::Context, g: &mut GraphState, acts: &mut Vec<Act>) {
    let busy = t.git.is_busy(&g.key);
    let mut close = false;
    let remotes = g.remotes.clone();
    let form = &mut g.remote_form;
    modal(
        ctx,
        "git_graph_remotes",
        "Remotes",
        560.0,
        &mut close,
        |ui| {
            style::modal_card_frame(ui.ctx()).show(ui, |ui| {
            ui.set_min_width(520.0);
            if remotes.is_empty() {
                ui.label(
                    egui::RichText::new("No remotes configured.")
                        .color(style::theme_muted_text(ui.ctx())),
                );
            }
            for r in &remotes {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&r.name).strong());
                        ui.label(egui::RichText::new(format!("fetch: {}", r.fetch_url)).size(11.5));
                        if r.push_url != r.fetch_url {
                            ui.label(
                                egui::RichText::new(format!("push: {}", r.push_url)).size(11.5),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Remove…"))
                            .clicked()
                        {
                            acts.push(Act::Dialog(GraphDialog::RemoveRemote {
                                name: r.name.clone(),
                            }));
                        }
                        if ui.button("Edit").clicked() {
                            *form = RemoteForm {
                                editing: Some(r.name.clone()),
                                name: r.name.clone(),
                                url: r.fetch_url.clone(),
                                push_url: if r.push_url == r.fetch_url {
                                    String::new()
                                } else {
                                    r.push_url.clone()
                                },
                            };
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("Prune"))
                            .on_hover_text(
                                "Remove remote-tracking branches that no longer exist on the remote",
                            )
                            .clicked()
                        {
                            acts.push(Act::PruneRemote(r.name.clone()));
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Fetch")).clicked() {
                            acts.push(Act::FetchRemote(Some(r.name.clone())));
                        }
                    });
                });
                ui.separator();
            }
            ui.label(
                egui::RichText::new(if form.editing.is_some() {
                    "Edit remote"
                } else {
                    "Add remote"
                })
                .strong(),
            );
            egui::Grid::new("gg_remote_form")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    text_field(ui, &mut form.name, "origin", false);
                    ui.end_row();
                    ui.label("Fetch URL");
                    text_field(ui, &mut form.url, "https://github.com/org/app.git", false);
                    ui.end_row();
                    ui.label("Push URL");
                    text_field(ui, &mut form.push_url, "Same as fetch URL", false);
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                let ok = !form.name.trim().is_empty() && !form.url.trim().is_empty() && !busy;
                if ui
                    .add_enabled(ok, style::btn_primary_ctx(ui.ctx(), "Save Remote"))
                    .clicked()
                {
                    acts.push(Act::SaveRemote);
                }
                if form.editing.is_some() && ui.button("New remote").clicked() {
                    *form = RemoteForm::default();
                }
            });
        });
        },
    );
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        g.remotes_open = false;
    }
}
