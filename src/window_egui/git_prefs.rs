//! Halaman Preferences › Git: token GitHub/GitLab (keychain), URL GitLab,
//! bahasa review AI, interval refresh, mode pull/fetch, dan tampilan Git Graph.

use std::sync::OnceLock;

use eframe::egui;

use super::preferences::{Tone, callout, hint, page_header, row, section, toggle_row};
use crate::git::history::CommitOrder;
use crate::git::repos::{AvatarSource, DateFormat, GraphStyle};
use crate::git::review::{self, GITHUB_TOKEN_SECRET, GITLAB_TOKEN_SECRET};
use crate::window_egui::{Tabular, style};

/// Lokasi binary git (dicari sekali per proses).
fn git_binary() -> Option<&'static std::path::Path> {
    static BIN: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    BIN.get_or_init(|| crate::agent::harness::resolve_binary("git"))
        .as_deref()
}

/// Baris token: status, input tersamar, Save/Clear. Mengembalikan true bila
/// keychain berubah.
fn token_row(
    ui: &mut egui::Ui,
    label: &str,
    hint_text: &str,
    saved: bool,
    draft: &mut String,
    secret_name: &str,
    toasts: &mut super::notifications::ToastManager,
) -> bool {
    let mut changed = false;
    row(ui, label, Some(hint_text), |ui| {
        ui.vertical(|ui| {
            let tone_color = if saved {
                style::theme_success(ui.ctx())
            } else {
                style::theme_muted_text(ui.ctx())
            };
            ui.label(
                egui::RichText::new(if saved {
                    "Saved in the OS keychain"
                } else {
                    "Not set"
                })
                .size(11.5)
                .color(tone_color),
            );
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 120.0).max(120.0);
                style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(draft)
                        .password(true)
                        .hint_text(if saved {
                            "Enter a new token to replace it"
                        } else {
                            "Paste token"
                        }),
                    w,
                    None,
                );
                if ui
                    .add_enabled(!draft.trim().is_empty(), egui::Button::new("Save"))
                    .clicked()
                {
                    if crate::secrets::set_secret(secret_name, draft.trim()) {
                        toasts.success(format!("{label} saved"));
                        changed = true;
                    } else {
                        toasts.error(format!("Could not save the {label} to the keychain"));
                    }
                    // Draft tidak disimpan di memori lebih lama dari perlu.
                    draft.clear();
                }
                if ui.add_enabled(saved, egui::Button::new("Clear")).clicked() {
                    crate::secrets::delete_secret(secret_name);
                    toasts.info(format!("{label} removed"));
                    changed = true;
                }
            });
        });
    });
    changed
}

pub fn render(t: &mut Tabular, ui: &mut egui::Ui) {
    page_header(
        ui,
        "Git",
        "Local repositories use the git installed on this computer. Merge Review talks to GitHub and GitLab with the tokens below.",
    );

    match git_binary() {
        Some(p) => hint(ui, format!("git: {}", p.display())),
        None => callout(ui, Tone::Warning, |ui| {
            ui.label(
                "git was not found in PATH. Install git to use the local repository features.",
            );
        }),
    }
    ui.add_space(8.0);

    let access = t.git.provider_access();
    let mut store_changed = false;
    let mut tokens_changed = false;

    section(ui, "Merge Review access", |ui| {
        let git = &mut t.git;
        tokens_changed |= token_row(
            ui,
            "GitHub token",
            "Classic token with the repo scope, or a fine-grained token with Pull requests: read & write.",
            access.github_token.is_some(),
            &mut git.github_token_draft,
            GITHUB_TOKEN_SECRET,
            &mut t.toasts,
        );
        ui.horizontal(|ui| {
            if ui.link("Create a GitHub token").clicked() {
                let _ = crate::url_opener::open_url("https://github.com/settings/tokens");
            }
        });
        super::preferences::divider(ui);
        row(
            ui,
            "GitLab URL",
            Some("gitlab.com or your self-hosted GitLab"),
            |ui| {
                let resp = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut git.store.settings.gitlab_url)
                        .hint_text(review::DEFAULT_GITLAB_URL),
                    f32::INFINITY,
                    None,
                );
                if resp.lost_focus() {
                    git.store.settings.gitlab_url =
                        review::normalized_gitlab_url(&git.store.settings.gitlab_url);
                    store_changed = true;
                    tokens_changed = true;
                }
            },
        );
        tokens_changed |= token_row(
            ui,
            "GitLab token",
            "Personal access token with the api scope.",
            access.gitlab_token.is_some(),
            &mut git.gitlab_token_draft,
            GITLAB_TOKEN_SECRET,
            &mut t.toasts,
        );
        let url = format!(
            "{}/-/user_settings/personal_access_tokens",
            review::normalized_gitlab_url(&git.store.settings.gitlab_url)
        );
        if ui.link("Create a GitLab token").clicked() {
            let _ = crate::url_opener::open_url(&url);
        }
    });

    section(ui, "AI review", |ui| {
        let settings = &mut t.git.store.settings;
        row(
            ui,
            "Review language",
            Some("Language of the AI review and its headings"),
            |ui| {
                egui::ComboBox::from_id_salt("git_pref_language")
                    .selected_text(&settings.review_language)
                    .show_ui(ui, |ui| {
                        for l in review::prompt::LANGUAGES {
                            if ui
                                .selectable_value(&mut settings.review_language, l.to_string(), l)
                                .changed()
                            {
                                store_changed = true;
                            }
                        }
                    });
            },
        );
        hint(
            ui,
            "The review uses the AI backend chosen in AI Assistant (API key or CLI agent).",
        );
    });

    section(ui, "Merge requests", |ui| {
        let settings = &mut t.git.store.settings;
        row(
            ui,
            "Auto refresh",
            Some("Minutes between list refreshes; 0 turns it off"),
            |ui| {
                if ui
                    .add(
                        egui::DragValue::new(&mut settings.auto_refresh_min)
                            .range(0..=120)
                            .suffix(" min"),
                    )
                    .changed()
                {
                    store_changed = true;
                }
            },
        );
        if toggle_row(
            ui,
            &mut settings.show_closed,
            "Include merged and closed",
            Some("Only for the \"This repository\" list"),
        ) {
            store_changed = true;
        }
    });

    section(ui, "Repository", |ui| {
        let settings = &mut t.git.store.settings;
        if toggle_row(
            ui,
            &mut settings.pull_rebase,
            "Pull with rebase",
            Some("Off: pull only fast-forwards and stops if the branches diverged"),
        ) {
            store_changed = true;
        }
        store_changed |= toggle_row(
            ui,
            &mut settings.fetch_prune,
            "Prune on fetch",
            Some("Remove remote-tracking branches that were deleted on the remote"),
        );
        store_changed |= toggle_row(
            ui,
            &mut settings.fetch_prune_tags,
            "Prune tags on fetch",
            Some("Remove local tags that no longer exist on the remote"),
        );
    });

    section(ui, "Git Graph", |ui| {
        let settings = &mut t.git.store.settings;
        row(ui, "Commit ordering", None, |ui| {
            egui::ComboBox::from_id_salt("git_pref_graph_order")
                .selected_text(settings.graph_order.label())
                .show_ui(ui, |ui| {
                    for o in CommitOrder::ALL {
                        store_changed |= ui
                            .selectable_value(&mut settings.graph_order, o, o.label())
                            .changed();
                    }
                });
        });
        row(ui, "Graph style", None, |ui| {
            store_changed |= ui
                .radio_value(&mut settings.graph_style, GraphStyle::Rounded, "Rounded")
                .changed();
            store_changed |= ui
                .radio_value(&mut settings.graph_style, GraphStyle::Angular, "Angular")
                .changed();
        });
        row(ui, "Date format", None, |ui| {
            store_changed |= ui
                .radio_value(
                    &mut settings.date_format,
                    DateFormat::DateTime,
                    "Date & time",
                )
                .changed();
            store_changed |= ui
                .radio_value(&mut settings.date_format, DateFormat::Date, "Date")
                .changed();
            store_changed |= ui
                .radio_value(&mut settings.date_format, DateFormat::Relative, "Relative")
                .changed();
        });
        row(
            ui,
            "Author avatars",
            Some("Gravatar sends an MD5 hash of each author email to gravatar.com"),
            |ui| {
                store_changed |= ui
                    .radio_value(&mut settings.avatars, AvatarSource::Initials, "Initials")
                    .changed();
                store_changed |= ui
                    .radio_value(&mut settings.avatars, AvatarSource::Gravatar, "Gravatar")
                    .changed();
            },
        );
        row(
            ui,
            "Commits per page",
            Some("More are loaded while you scroll"),
            |ui| {
                store_changed |= ui
                    .add(
                        egui::DragValue::new(&mut settings.graph_page)
                            .range(50..=5000)
                            .speed(10),
                    )
                    .changed();
            },
        );
        super::preferences::divider(ui);
        for (value, label, help) in [
            (
                &mut settings.show_remote_branches,
                "Show remote branches",
                None,
            ),
            (&mut settings.show_tags, "Show tags", None),
            (&mut settings.show_stashes, "Show stashes", None),
            (
                &mut settings.show_uncommitted,
                "Show uncommitted changes",
                None,
            ),
            (
                &mut settings.mute_merges,
                "Mute merge commits",
                Some("Show merge commit messages in a dimmer color"),
            ),
            (
                &mut settings.first_parent,
                "Only follow the first parent",
                Some("Hide commits that were merged in from other branches"),
            ),
            (
                &mut settings.show_reflog,
                "Include reflog commits",
                Some("Also show commits that are only referenced by reflogs"),
            ),
            (
                &mut settings.check_signatures,
                "Check commit signatures",
                Some("Runs gpg for signed commits when their details are opened"),
            ),
        ] {
            store_changed |= toggle_row(ui, value, label, help);
        }
        super::preferences::divider(ui);
        row(
            ui,
            "Issue link URL",
            Some("Use $1 for the issue number. Empty: link to GitHub/GitLab issues automatically"),
            |ui| {
                let resp = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut settings.issue_url)
                        .hint_text("https://jira.example.com/browse/$1"),
                    f32::INFINITY,
                    None,
                );
                store_changed |= resp.lost_focus();
            },
        );
        row(
            ui,
            "Issue pattern",
            Some("Regular expression; group 1 replaces $1. Empty: #123"),
            |ui| {
                let resp = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut settings.issue_regex)
                        .hint_text(r"([A-Z]+-\d+)"),
                    f32::INFINITY,
                    None,
                );
                store_changed |= resp.lost_focus();
            },
        );
    });

    if tokens_changed {
        t.git.invalidate_access();
        t.git.mrs_loaded_at = None;
    }
    if store_changed {
        if let Err(e) = t.git.save_store() {
            t.toasts.error(e);
        }
        super::git_graph_jobs::refresh_open_graphs(t);
    }
}
