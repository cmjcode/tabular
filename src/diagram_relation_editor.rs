//! Modal "Edit relation": dibuka dengan double-click garis relasi virtual.
//! Tabel kedua ujung tetap; user memilih ulang kolom child dan parent.

use eframe::egui;

use crate::models::structs::{
    DiagramNode, DiagramState, RelationEditDraft, RelationOrigin, VirtualRelation,
};
use crate::window_egui::style;

/// Buka modal edit untuk relasi virtual ke-`idx`.
pub fn open_relation_editor(state: &mut DiagramState, idx: usize) {
    let Some(rel) = state.virtual_relations.get(idx) else {
        return;
    };
    state.relation_editor = Some(RelationEditDraft {
        index: idx,
        original: rel.clone(),
        child_column: rel.child_column.clone(),
        parent_column: rel.parent_column.clone(),
        child_filter: String::new(),
        parent_filter: String::new(),
        error: None,
        scrolled: false,
    });
    state.selected_virtual = Some(idx);
    state.selected_edge = None;
}

/// Terapkan draft ke `state.virtual_relations`. Relasi hasil saran yang
/// diubah user menjadi relasi manual.
pub fn apply_relation_edit(
    state: &mut DiagramState,
    draft: &RelationEditDraft,
) -> Result<(), String> {
    if state.virtual_relations.get(draft.index) != Some(&draft.original) {
        return Err("This relation was changed or removed. Close and try again.".into());
    }
    let has_column = |table: &str, column: &str| {
        state
            .nodes
            .iter()
            .find(|n| n.id == table)
            .is_some_and(|n| n.columns.iter().any(|c| c == column))
    };
    if !has_column(&draft.original.child, &draft.child_column) {
        return Err(format!(
            "Column {} does not exist in {}.",
            draft.child_column, draft.original.child
        ));
    }
    if !has_column(&draft.original.parent, &draft.parent_column) {
        return Err(format!(
            "Column {} does not exist in {}.",
            draft.parent_column, draft.original.parent
        ));
    }
    if draft.child_column == draft.original.child_column
        && draft.parent_column == draft.original.parent_column
    {
        return Ok(());
    }
    let duplicate = state.virtual_relations.iter().enumerate().any(|(i, r)| {
        i != draft.index
            && r.child == draft.original.child
            && r.parent == draft.original.parent
            && r.child_column == draft.child_column
            && r.parent_column == draft.parent_column
    });
    if duplicate {
        return Err(format!(
            "{}.{} → {}.{} already exists.",
            draft.original.child, draft.child_column, draft.original.parent, draft.parent_column
        ));
    }

    let origin = match draft.original.origin {
        RelationOrigin::Inferred => RelationOrigin::Manual,
        other => other,
    };
    state.virtual_relations[draft.index] = VirtualRelation {
        child: draft.original.child.clone(),
        child_column: draft.child_column.clone(),
        parent: draft.original.parent.clone(),
        parent_column: draft.parent_column.clone(),
        origin,
    };
    state.selected_virtual = Some(draft.index);
    state.save_requested = true;
    Ok(())
}

/// Render modal edit relasi bila sedang terbuka.
pub fn render_relation_editor(ctx: &egui::Context, state: &mut DiagramState) {
    let Some(mut draft) = state.relation_editor.take() else {
        return;
    };
    // Relasi sudah hilang/berubah (mis. dihapus dari panel lain): tutup modal.
    if state.virtual_relations.get(draft.index) != Some(&draft.original) {
        return;
    }
    let child = state.nodes.iter().find(|n| n.id == draft.original.child);
    let parent = state.nodes.iter().find(|n| n.id == draft.original.parent);
    let (Some(child), Some(parent)) = (child, parent) else {
        return;
    };

    let mut close = false;
    let mut save = false;
    // Kolom terpilih di-scroll ke tengah hanya pada frame pertama.
    let first_frame = !draft.scrolled;
    draft.scrolled = true;
    style::render_modal_backdrop(ctx, "diagram_relation_editor_backdrop", true);
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(420.0, 720.0);
    let list_h = (screen.height() - 320.0).clamp(140.0, 360.0);

    egui::Window::new("Edit relation")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            style::render_modal_header(ui, "Edit relation", &mut close);
            ui.label(
                egui::RichText::new(format!(
                    "{}.{}  →  {}.{}",
                    child.title, draft.child_column, parent.title, draft.parent_column
                ))
                .monospace(),
            );
            if draft.original.origin == RelationOrigin::Inferred {
                ui.label(
                    egui::RichText::new(
                        "Saving changes turns this suggested relation into a manual one.",
                    )
                    .small()
                    .weak(),
                );
            }
            ui.add_space(8.0);

            ui.columns(2, |cols| {
                column_picker(
                    &mut cols[0],
                    "child",
                    &format!("{} (child)", child.title),
                    child,
                    &mut draft.child_column,
                    &mut draft.child_filter,
                    list_h,
                    first_frame,
                );
                column_picker(
                    &mut cols[1],
                    "parent",
                    &format!("{} (parent)", parent.title),
                    parent,
                    &mut draft.parent_column,
                    &mut draft.parent_filter,
                    list_h,
                    first_frame,
                );
            });

            if let Some(err) = &draft.error {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
            }

            ui.add_space(10.0);
            let changed = draft.child_column != draft.original.child_column
                || draft.parent_column != draft.original.parent_column;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        changed,
                        style::btn_primary_ctx(ui.ctx(), "Save").min_size(egui::vec2(72.0, 28.0)),
                    )
                    .on_hover_text("Save (Cmd/Ctrl+Enter)")
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
            if changed
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
            {
                save = true;
            }
        });

    if save {
        match apply_relation_edit(state, &draft) {
            Ok(()) => return,
            Err(e) => draft.error = Some(e),
        }
    }
    if !close {
        state.relation_editor = Some(draft);
    }
}

/// Daftar kolom satu tabel dengan kotak pencarian; klik baris memilih kolom.
fn column_picker(
    ui: &mut egui::Ui,
    salt: &str,
    heading: &str,
    node: &DiagramNode,
    selected: &mut String,
    filter: &mut String,
    list_h: f32,
    scroll_to_selected: bool,
) {
    ui.label(egui::RichText::new(heading).strong());
    ui.add_space(4.0);
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(filter).hint_text("Filter columns"),
        ui.available_width(),
        None,
    );
    ui.add_space(4.0);
    let needle = filter.trim().to_lowercase();
    egui::Frame::NONE
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("diagram_relation_editor_list", salt))
                .max_height(list_h)
                .min_scrolled_height(list_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut shown = 0;
                    for column in &node.columns {
                        if !needle.is_empty() && !column.to_lowercase().contains(&needle) {
                            continue;
                        }
                        shown += 1;
                        let is_selected = selected == column;
                        let meta = node.column_info(column);
                        let is_pk = meta.is_some_and(|m| m.is_pk);
                        let type_name = meta.map(|m| m.type_name.as_str()).unwrap_or("");
                        let resp = ui
                            .horizontal(|ui| {
                                ui.set_width(ui.available_width());
                                let r = ui.selectable_label(is_selected, column);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(egui::RichText::new(type_name).small().weak());
                                        if is_pk {
                                            ui.label(
                                                egui::RichText::new("PK")
                                                    .small()
                                                    .color(egui::Color32::from_rgb(255, 200, 60)),
                                            );
                                        }
                                    },
                                );
                                r
                            })
                            .inner;
                        if is_selected && scroll_to_selected {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                        if resp.clicked() {
                            *selected = column.clone();
                        }
                    }
                    if shown == 0 {
                        ui.label(egui::RichText::new("No matching column").weak());
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, columns: &[&str]) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        }
    }

    fn rel(child_col: &str, parent_col: &str, origin: RelationOrigin) -> VirtualRelation {
        VirtualRelation {
            child: "orders".into(),
            child_column: child_col.into(),
            parent: "customers".into(),
            parent_column: parent_col.into(),
            origin,
        }
    }

    fn fixture() -> DiagramState {
        DiagramState {
            nodes: vec![
                node("orders", &["id", "customer_code", "customer_id"]),
                node("customers", &["id", "code"]),
            ],
            virtual_relations: vec![
                rel("customer_code", "id", RelationOrigin::Inferred),
                rel("customer_id", "id", RelationOrigin::Manual),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn edit_changes_columns_and_marks_manual() {
        let mut state = fixture();
        open_relation_editor(&mut state, 0);
        let mut draft = state.relation_editor.clone().unwrap();
        draft.parent_column = "code".into();
        apply_relation_edit(&mut state, &draft).unwrap();
        let r = &state.virtual_relations[0];
        assert_eq!(r.child_column, "customer_code");
        assert_eq!(r.parent_column, "code");
        assert_eq!(r.origin, RelationOrigin::Manual);
        assert!(state.save_requested);
    }

    #[test]
    fn edit_rejects_duplicate_and_unknown_column() {
        let mut state = fixture();
        open_relation_editor(&mut state, 0);
        let mut draft = state.relation_editor.clone().unwrap();
        draft.child_column = "customer_id".into();
        assert!(apply_relation_edit(&mut state, &draft).is_err());
        draft.child_column = "missing".into();
        assert!(apply_relation_edit(&mut state, &draft).is_err());
        assert_eq!(state.virtual_relations[0].child_column, "customer_code");
        assert!(!state.save_requested);
    }

    #[test]
    fn edit_refuses_stale_draft() {
        let mut state = fixture();
        open_relation_editor(&mut state, 1);
        let mut draft = state.relation_editor.clone().unwrap();
        state.virtual_relations.remove(0);
        draft.parent_column = "code".into();
        assert!(apply_relation_edit(&mut state, &draft).is_err());
    }
}
