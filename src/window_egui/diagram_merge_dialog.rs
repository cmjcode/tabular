//! Popup merge diagram: tampil saat diagram diubah di database dan di lokal
//! pada entitas yang sama. User memilih versi per item (seperti resolve
//! konflik di git) atau semua sekaligus.

use eframe::egui;

use crate::diagram_sync::Side;
use crate::window_egui::style;

/// Keputusan dari popup pada frame ini.
enum MergeDecision {
    Apply(Option<Side>),
    Postpone,
}

impl super::Tabular {
    pub fn render_diagram_merge_dialog(&mut self, ctx: &egui::Context) {
        let tab_idx = self.active_tab_index;
        let Some(merge) = self
            .query_tabs
            .get_mut(tab_idx)
            .and_then(|t| t.diagram_state.as_mut())
            .and_then(|st| st.pending_merge.as_mut())
            .filter(|m| m.visible)
        else {
            return;
        };

        let mut close = false;
        let mut decision: Option<MergeDecision> = None;
        style::render_modal_backdrop(ctx, "diagram_merge_backdrop", true);

        egui::Window::new("Merge Diagram")
            .title_bar(false)
            .frame(style::modal_window_frame(ctx))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.set_min_width(540.0);
                style::render_modal_header(ui, "Diagram changed in the database", &mut close);
                ui.add_space(6.0);

                let rec = &merge.remote;
                let mut who = format!("Revision {}", rec.revision);
                if let Some(by) = &rec.updated_by {
                    who.push_str(&format!(" saved by {by}"));
                }
                if let Some(at) = &rec.updated_at {
                    who.push_str(&format!(" at {at}"));
                }
                ui.label(format!(
                    "{who} changes the same items as your local edits. Choose which version to keep for each item."
                ));
                if merge.result.auto_merged > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} other change(s) from the database were merged automatically.",
                            merge.result.auto_merged
                        ))
                        .weak(),
                    );
                }
                ui.add_space(8.0);

                style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(340.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let mut kinds: Vec<&'static str> = Vec::new();
                            for c in &merge.result.conflicts {
                                if !kinds.contains(&c.kind) {
                                    kinds.push(c.kind);
                                }
                            }
                            for kind in kinds {
                                ui.label(egui::RichText::new(kind).strong());
                                for (i, c) in merge
                                    .result
                                    .conflicts
                                    .iter_mut()
                                    .enumerate()
                                    .filter(|(_, c)| c.kind == kind)
                                {
                                    let mine = c.describe(Side::Local);
                                    let theirs = c.describe(Side::Remote);
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.set_width(330.0);
                                            ui.label(&c.label);
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Mine: {mine} · Database: {theirs}"
                                                ))
                                                .weak()
                                                .small(),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.push_id(("merge_choice", i), |ui| {
                                                    ui.selectable_value(
                                                        &mut c.choice,
                                                        Side::Remote,
                                                        "Database",
                                                    );
                                                    ui.selectable_value(
                                                        &mut c.choice,
                                                        Side::Local,
                                                        "Mine",
                                                    );
                                                });
                                            },
                                        );
                                    });
                                    ui.add_space(4.0);
                                }
                                ui.add_space(6.0);
                            }
                        });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(style::btn_secondary("Keep all mine"))
                        .on_hover_text("Keep your local version of every conflicting item")
                        .clicked()
                    {
                        decision = Some(MergeDecision::Apply(Some(Side::Local)));
                    }
                    if ui
                        .add(style::btn_secondary("Take all from database"))
                        .on_hover_text("Use the database version of every conflicting item")
                        .clicked()
                    {
                        decision = Some(MergeDecision::Apply(Some(Side::Remote)));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(style::btn_primary_ctx(ui.ctx(), "Apply merge"))
                            .on_hover_text("Apply the choices above and save the result")
                            .clicked()
                        {
                            decision = Some(MergeDecision::Apply(None));
                        }
                    });
                });
            });

        if close && decision.is_none() {
            decision = Some(MergeDecision::Postpone);
        }
        match decision {
            Some(MergeDecision::Apply(all)) => self.apply_diagram_merge(tab_idx, all),
            Some(MergeDecision::Postpone) => {
                merge.visible = false;
                self.toasts.info(
                    "Merge postponed. Changes stay local until you resolve it from the diagram toolbar.",
                );
            }
            None => {}
        }
    }
}
