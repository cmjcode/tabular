//! Menu konteks Git Graph: commit, branch lokal, branch remote, tag, stash,
//! dan baris "Uncommitted Changes". Setiap item menghasilkan [`Act`]; aksi
//! yang butuh input/konfirmasi membuka [`GraphDialog`].

use eframe::egui;

use super::git_graph_jobs::{GraphDialog, GraphItem, GraphState, rev_label};
use super::git_graph_view::{Act, PillRef};
use crate::git::history_ops::{ForceMode, MergeOptions, PickOptions, ResetMode};
use crate::window_egui::Tabular;

fn item(ui: &mut egui::Ui, label: &str, acts: &mut Vec<Act>, act: impl FnOnce() -> Act) {
    if ui.button(label).clicked() {
        acts.push(act());
        ui.close();
    }
}

fn item_if(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    acts: &mut Vec<Act>,
    act: impl FnOnce() -> Act,
) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        acts.push(act());
        ui.close();
    }
}

fn has_provider(t: &mut Tabular, key: &str) -> bool {
    let host = t.git.provider_access().gitlab_host();
    crate::git::review::remote_from_key(key, &host).is_some()
}

/// Dialog checkout branch remote sebagai branch lokal yang tracking.
pub(super) fn checkout_remote_dialog(full: &str) -> GraphDialog {
    let local = full.split_once('/').map_or(full, |(_, b)| b).to_string();
    GraphDialog::CheckoutBranch {
        name: full.to_string(),
        remote: true,
        local_name: local,
    }
}

pub(super) fn commit_menu(
    _t: &mut Tabular,
    ui: &mut egui::Ui,
    g: &GraphState,
    idx: usize,
    it: &GraphItem,
    acts: &mut Vec<Act>,
) {
    ui.set_min_width(260.0);
    let h = it.info.hash.clone();
    let parents = it.info.parents.len();
    let on_branch = g.refs.head_branch.is_some();
    let is_head = g.refs.head.as_deref() == Some(h.as_str());
    let remote = g.default_remote();
    item(ui, "Add Tag…", acts, || {
        Act::Dialog(GraphDialog::AddTag {
            at: h.clone(),
            name: String::new(),
            annotated: true,
            message: String::new(),
            push: false,
            remote: remote.clone(),
            force: false,
        })
    });
    item(ui, "Create Branch…", acts, || {
        Act::Dialog(GraphDialog::CreateBranch {
            at: h.clone(),
            name: String::new(),
            checkout: true,
            force: false,
        })
    });
    ui.separator();
    item(ui, "Checkout…", acts, || {
        Act::Dialog(GraphDialog::CheckoutCommit { hash: h.clone() })
    });
    item_if(ui, on_branch && !is_head, "Cherry Pick…", acts, || {
        Act::Dialog(GraphDialog::CherryPick {
            hash: h.clone(),
            parents,
            opt: PickOptions {
                mainline: (parents > 1).then_some(1),
                ..Default::default()
            },
        })
    });
    item(ui, "Revert…", acts, || {
        Act::Dialog(GraphDialog::Revert {
            hash: h.clone(),
            parents,
            mainline: 1,
        })
    });
    item_if(ui, on_branch && parents == 1, "Drop…", acts, || {
        Act::Dialog(GraphDialog::DropCommit { hash: h.clone() })
    });
    ui.separator();
    item_if(
        ui,
        on_branch && !is_head,
        "Merge into current branch…",
        acts,
        || {
            Act::Dialog(GraphDialog::Merge {
                rev: h.clone(),
                what: format!("commit {}", rev_label(&h)),
                opt: MergeOptions {
                    no_ff: true,
                    ..Default::default()
                },
            })
        },
    );
    item_if(
        ui,
        on_branch && !is_head,
        "Rebase current branch on this Commit…",
        acts,
        || {
            Act::Dialog(GraphDialog::Rebase {
                onto: h.clone(),
                what: format!("commit {}", rev_label(&h)),
                ignore_date: false,
            })
        },
    );
    item_if(
        ui,
        on_branch,
        "Reset current branch to this Commit…",
        acts,
        || {
            Act::Dialog(GraphDialog::Reset {
                hash: h.clone(),
                mode: ResetMode::Mixed,
            })
        },
    );
    ui.separator();
    let has_selection = g.selected.as_deref().is_some_and(|s| s != h.as_str());
    item_if(ui, has_selection, "Compare with Selected", acts, || {
        Act::Compare(idx)
    });
    item(ui, "Compare with Working Tree", acts, || {
        Act::CompareWorking(idx)
    });
    ui.separator();
    item(ui, "Copy Commit Hash", acts, || {
        Act::Copy(h.clone(), "Commit hash")
    });
    item(ui, "Copy Commit Subject", acts, || {
        Act::Copy(it.info.subject.clone(), "Commit subject")
    });
    item(ui, "Create Archive…", acts, || Act::Archive(h.clone()));
}

pub(super) fn ref_menu(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    g: &GraphState,
    it: &GraphItem,
    r: &PillRef,
    acts: &mut Vec<Act>,
) {
    ui.set_min_width(260.0);
    let remote = g.default_remote();
    let current = g.refs.head_branch.clone().unwrap_or_default();
    let on_branch = g.refs.head_branch.is_some();
    match r {
        PillRef::Local {
            name,
            current: is_current,
        } => {
            let name = name.clone();
            item_if(ui, !is_current, "Checkout Branch", acts, || {
                Act::Checkout(name.clone())
            });
            item(ui, "Rename Branch…", acts, || {
                Act::Dialog(GraphDialog::RenameBranch {
                    old: name.clone(),
                    new: name.clone(),
                })
            });
            item_if(ui, !is_current, "Delete Branch…", acts, || {
                Act::Dialog(GraphDialog::DeleteBranch {
                    name: name.clone(),
                    force: false,
                    remote: None,
                })
            });
            ui.separator();
            item_if(
                ui,
                !is_current && on_branch,
                &format!("Merge into current branch ({current})…"),
                acts,
                || {
                    Act::Dialog(GraphDialog::Merge {
                        rev: name.clone(),
                        what: format!("branch {name}"),
                        opt: MergeOptions {
                            no_ff: true,
                            ..Default::default()
                        },
                    })
                },
            );
            item_if(
                ui,
                !is_current && on_branch,
                "Rebase current branch on Branch…",
                acts,
                || {
                    Act::Dialog(GraphDialog::Rebase {
                        onto: name.clone(),
                        what: format!("branch {name}"),
                        ignore_date: false,
                    })
                },
            );
            item_if(
                ui,
                !g.refs.remotes.is_empty(),
                "Push Branch…",
                acts,
                || {
                    Act::Dialog(GraphDialog::PushBranch {
                        branch: name.clone(),
                        remote: remote.clone(),
                        set_upstream: true,
                        force: ForceMode::None,
                    })
                },
            );
            if has_provider(t, &g.key) {
                item(ui, "Create Pull Request", acts, || {
                    Act::CreatePr(name.clone())
                });
            }
            ui.separator();
            item(ui, "Create Archive…", acts, || Act::Archive(name.clone()));
            item(ui, "Select in Branches Filter", acts, || {
                Act::SetBranches(vec![name.clone()])
            });
            item(ui, "Copy Branch Name", acts, || {
                Act::Copy(name.clone(), "Branch name")
            });
        }
        PillRef::Remote(full) => {
            let (rem, branch) = full
                .split_once('/')
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap_or_else(|| (remote.clone(), full.clone()));
            item(ui, "Checkout Branch…", acts, || {
                Act::Dialog(checkout_remote_dialog(full))
            });
            item(ui, "Delete Remote Branch…", acts, || {
                Act::Dialog(GraphDialog::DeleteRemoteBranch {
                    remote: rem.clone(),
                    branch: branch.clone(),
                })
            });
            item(ui, "Fetch into local branch…", acts, || {
                Act::Dialog(GraphDialog::FetchInto {
                    remote: rem.clone(),
                    branch: branch.clone(),
                    local: branch.clone(),
                    force: false,
                })
            });
            ui.separator();
            item_if(
                ui,
                on_branch,
                &format!("Merge into current branch ({current})…"),
                acts,
                || {
                    Act::Dialog(GraphDialog::Merge {
                        rev: full.clone(),
                        what: format!("remote branch {full}"),
                        opt: MergeOptions {
                            no_ff: true,
                            ..Default::default()
                        },
                    })
                },
            );
            item_if(ui, on_branch, "Pull into current branch…", acts, || {
                Act::Dialog(GraphDialog::PullInto {
                    remote: rem.clone(),
                    branch: branch.clone(),
                    opt: MergeOptions::default(),
                })
            });
            if has_provider(t, &g.key) {
                item(ui, "Create Pull Request", acts, || {
                    Act::CreatePr(branch.clone())
                });
            }
            ui.separator();
            item(ui, "Create Archive…", acts, || Act::Archive(full.clone()));
            item(ui, "Select in Branches Filter", acts, || {
                Act::SetBranches(vec![full.clone()])
            });
            item(ui, "Copy Branch Name", acts, || {
                Act::Copy(full.clone(), "Branch name")
            });
        }
        PillRef::Tag { name, annotated } => {
            item_if(ui, *annotated, "View Details", acts, || {
                Act::ShowTag(name.clone())
            });
            item(ui, "Delete Tag…", acts, || {
                Act::Dialog(GraphDialog::DeleteTag {
                    name: name.clone(),
                    remote: None,
                })
            });
            item_if(ui, !g.refs.remotes.is_empty(), "Push Tag…", acts, || {
                Act::Dialog(GraphDialog::PushTag {
                    name: name.clone(),
                    remote: remote.clone(),
                })
            });
            ui.separator();
            item(ui, "Create Archive…", acts, || Act::Archive(name.clone()));
            item(ui, "Copy Tag Name", acts, || {
                Act::Copy(name.clone(), "Tag name")
            });
        }
        PillRef::Stash => stash_menu(ui, it, acts),
    }
}

pub(super) fn stash_menu(ui: &mut egui::Ui, it: &GraphItem, acts: &mut Vec<Act>) {
    ui.set_min_width(240.0);
    let Some(s) = &it.stash else {
        return;
    };
    let sel = s.selector.clone();
    item(ui, "Apply Stash…", acts, || {
        Act::Dialog(GraphDialog::StashApply {
            selector: sel.clone(),
            pop: false,
            index: false,
        })
    });
    item(ui, "Pop Stash…", acts, || {
        Act::Dialog(GraphDialog::StashApply {
            selector: sel.clone(),
            pop: true,
            index: false,
        })
    });
    item(ui, "Drop Stash…", acts, || {
        Act::Dialog(GraphDialog::StashDrop {
            selector: sel.clone(),
        })
    });
    item(ui, "Create Branch from Stash…", acts, || {
        Act::Dialog(GraphDialog::StashBranch {
            selector: sel.clone(),
            name: String::new(),
        })
    });
    ui.separator();
    item(ui, "Copy Stash Name", acts, || {
        Act::Copy(sel.clone(), "Stash name")
    });
    item(ui, "Copy Stash Hash", acts, || {
        Act::Copy(s.hash.clone(), "Stash hash")
    });
}

pub(super) fn uncommitted_menu(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.set_min_width(260.0);
    item(ui, "Stash uncommitted changes…", acts, || {
        Act::Dialog(GraphDialog::StashPush {
            message: String::new(),
            untracked: true,
        })
    });
    item(ui, "Reset uncommitted changes…", acts, || {
        Act::Dialog(GraphDialog::ResetUncommitted {
            mode: ResetMode::Mixed,
        })
    });
    item(ui, "Clean untracked files…", acts, || {
        Act::Dialog(GraphDialog::CleanUntracked { dirs: false })
    });
    ui.separator();
    item(ui, "Open in Changes sidebar", acts, || Act::OpenChanges);
}
