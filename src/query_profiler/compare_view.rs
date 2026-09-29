//! View Metrics (C2) dan Compare (C3) untuk Visual Execution Profiler.

use super::compare::{DeltaStatus, PlanDiff, diff_plans};
use super::graph::{
    CompareAction, CompareContext, ProfilerViewMode, QueryProfilerState, get_cost_color,
};
use super::history::default_baseline;
use super::{ExplainNode, PlanMetric, metric_rows, parse_explain};
use eframe::egui::{self, Color32};

const GOOD: Color32 = Color32::from_rgb(76, 175, 80);
const BAD: Color32 = Color32::from_rgb(229, 83, 83);

fn format_metric(metric: PlanMetric, v: f64) -> String {
    match metric {
        PlanMetric::SelfCost => format!("{v:.2}"),
        PlanMetric::SelfTime => format!("{v:.3} ms"),
        PlanMetric::Rows => format!("{}", v.round() as u64),
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Metrics (bar chart)
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn render_metrics_view(
    ui: &mut egui::Ui,
    root: &ExplainNode,
    state: &mut QueryProfilerState,
) {
    let has_timing = root.actual_total_time.is_some();
    if state.metric == PlanMetric::SelfTime && !has_timing {
        state.metric = PlanMetric::SelfCost;
    }

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Metric").strong());
        for m in PlanMetric::ALL {
            let enabled = m != PlanMetric::SelfTime || has_timing;
            let resp = ui.add_enabled(
                enabled,
                egui::Button::selectable(state.metric == m, m.label()),
            );
            let resp = if enabled {
                resp
            } else {
                resp.on_disabled_hover_text("Run EXPLAIN ANALYZE to collect timings.")
            };
            if resp.clicked() {
                state.metric = m;
            }
        }
        ui.label(
            egui::RichText::new(
                "Self = the node minus its children. Click a bar to open it in the graph.",
            )
            .small()
            .weak(),
        );
    });
    ui.add_space(6.0);

    let rows = metric_rows(root, state.metric);
    let max = rows.iter().map(|r| r.value).fold(0.0_f64, f64::max);
    if rows.is_empty() || max <= 0.0 {
        ui.label(egui::RichText::new("No data for this metric in the plan.").weak());
        return;
    }

    let label_w = 300.0_f32.min(ui.available_width() * 0.4);
    let value_w = 110.0;
    let row_h = 22.0;
    egui::ScrollArea::vertical()
        .id_salt("profiler_metrics_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for r in &rows {
                let full_w = ui.available_width();
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(full_w, row_h), egui::Sense::click());
                let painter = ui.painter_at(rect);
                if resp.hovered() {
                    painter.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let text_color = ui.visuals().text_color();
                painter.text(
                    egui::pos2(rect.left() + 4.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    truncate_chars(&format!("#{} {}", r.node_id, r.label), 44),
                    egui::FontId::proportional(12.0),
                    text_color,
                );
                let bar_max_w = (full_w - label_w - value_w - 12.0).max(20.0);
                let frac = (r.value / max) as f32;
                let bar = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + label_w, rect.top() + 4.0),
                    egui::vec2((bar_max_w * frac).max(2.0), row_h - 8.0),
                );
                painter.rect_filled(bar, 3.0, get_cost_color(r.share * 100.0));
                painter.text(
                    egui::pos2(bar.right() + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!(
                        "{}  ({:.1}%)",
                        format_metric(state.metric, r.value),
                        r.share * 100.0
                    ),
                    egui::FontId::monospace(11.5),
                    text_color,
                );
                let resp = resp.on_hover_text(&r.label);
                if resp.clicked() {
                    state.selected_node_id = Some(r.node_id);
                    state.view_mode = ProfilerViewMode::VisualGraph;
                }
            }
        });
}

// ─────────────────────────────────────────────────────────────────────────────
// Compare
// ─────────────────────────────────────────────────────────────────────────────

fn delta_color(delta: f64) -> Color32 {
    if delta < -1e-9 {
        GOOD
    } else if delta > 1e-9 {
        BAD
    } else {
        Color32::GRAY
    }
}

fn pct_change(base: f64, cur: f64) -> String {
    if base.abs() < 1e-12 {
        return String::new();
    }
    format!(" ({:+.1}%)", (cur - base) / base * 100.0)
}

pub(super) fn render_compare_view(
    ui: &mut egui::Ui,
    root: &ExplainNode,
    compare: Option<&CompareContext<'_>>,
    state: &mut QueryProfilerState,
) -> Option<CompareAction> {
    let Some(ctx) = compare else {
        ui.label(
            egui::RichText::new(
                "Plan history is kept for EXPLAIN runs from a query tab with an active connection.",
            )
            .weak(),
        );
        return None;
    };
    let mut action = None;

    let current = ctx
        .current_id
        .and_then(|id| ctx.snapshots.iter().find(|s| s.id == id));
    let others: Vec<_> = ctx
        .snapshots
        .iter()
        .filter(|s| Some(s.id) != ctx.current_id)
        .collect();

    // Baseline yang dipilih harus masih ada di riwayat.
    if state
        .compare_baseline
        .is_some_and(|id| !others.iter().any(|s| s.id == id))
    {
        state.compare_baseline = None;
    }
    let baseline_id = state
        .compare_baseline
        .or_else(|| default_baseline(ctx.snapshots, ctx.current_id));
    let baseline = baseline_id.and_then(|id| others.iter().find(|s| s.id == id).copied());

    ui.horizontal_wrapped(|ui| {
        if let Some(cur) = current {
            let (label, hint) = if cur.pinned {
                (
                    "Unpin this plan",
                    "Pinned plans are kept and used as the default baseline.",
                )
            } else {
                (
                    "Pin this plan",
                    "Keep this plan as the baseline for future runs.",
                )
            };
            if ui
                .button(format!(
                    "{} {label}",
                    egui_icons::icons::ICON_PUSH_PIN.codepoint
                ))
                .on_hover_text(hint)
                .clicked()
            {
                action = Some(CompareAction::SetPinned {
                    id: cur.id,
                    pinned: !cur.pinned,
                });
            }
            ui.separator();
        }

        ui.label(egui::RichText::new("Baseline").strong());
        let describe = |s: &super::history::PlanSnapshot| {
            let dur = s
                .duration_ms
                .map(|d| format!(" · {d:.2} ms"))
                .unwrap_or_default();
            let pin = if s.pinned { " · pinned" } else { "" };
            format!("{} · cost {:.2}{dur}{pin}", s.captured_at, s.total_cost)
        };
        let selected = baseline
            .map(&describe)
            .unwrap_or_else(|| "No earlier plan".to_string());
        ui.add_enabled_ui(!others.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("profiler_compare_baseline")
                .selected_text(selected)
                .width(320.0)
                .show_ui(ui, |ui| {
                    for s in &others {
                        if ui
                            .selectable_label(baseline_id == Some(s.id), describe(s))
                            .clicked()
                        {
                            state.compare_baseline = Some(s.id);
                        }
                    }
                });
        });
        if let Some(b) = baseline
            && ui
                .small_button(if b.pinned {
                    "Unpin baseline"
                } else {
                    "Pin baseline"
                })
                .clicked()
        {
            action = Some(CompareAction::SetPinned {
                id: b.id,
                pinned: !b.pinned,
            });
        }
    });
    ui.add_space(8.0);

    let Some(base) = baseline else {
        ui.label(
            egui::RichText::new(
                "Run the same EXPLAIN again (after adding an index, rewriting the query, …) to see per-node deltas here.",
            )
            .weak(),
        );
        return action;
    };
    let Some((base_root, _)) = parse_explain(&base.raw_plan) else {
        ui.colored_label(BAD, "The baseline plan could not be parsed.");
        return action;
    };

    let diff = diff_plans(&base_root, root);
    render_diff_summary(ui, &diff);
    ui.add_space(6.0);
    render_diff_table(ui, &diff);
    action
}

fn render_diff_summary(ui: &mut egui::Ui, diff: &PlanDiff) {
    ui.horizontal_wrapped(|ui| {
        let dc = diff.current_cost - diff.base_cost;
        ui.label(egui::RichText::new("Total cost").strong());
        // Panah hanya ada di font monospace.
        ui.label(
            egui::RichText::new(format!("{:.2} → {:.2}", diff.base_cost, diff.current_cost))
                .family(egui::FontFamily::Monospace),
        );
        ui.colored_label(
            delta_color(dc),
            format!("{dc:+.2}{}", pct_change(diff.base_cost, diff.current_cost)),
        );
        if let (Some(bt), Some(ct)) = (diff.base_time_ms, diff.current_time_ms) {
            ui.separator();
            ui.label(egui::RichText::new("Time").strong());
            ui.label(
                egui::RichText::new(format!("{bt:.2} → {ct:.2} ms"))
                    .family(egui::FontFamily::Monospace),
            );
            ui.colored_label(
                delta_color(ct - bt),
                format!("{:+.2} ms{}", ct - bt, pct_change(bt, ct)),
            );
        }
        ui.separator();
        if diff.shape_changed {
            ui.colored_label(Color32::from_rgb(255, 183, 77), "⚠ Plan shape changed");
        } else {
            ui.label(egui::RichText::new("Same plan shape").weak());
        }
    });
}

fn render_diff_table(ui: &mut egui::Ui, diff: &PlanDiff) {
    egui::ScrollArea::both()
        .id_salt("profiler_compare_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("profiler_compare_grid")
                .striped(true)
                .spacing(egui::vec2(14.0, 4.0))
                .show(ui, |ui| {
                    for h in [
                        "Node",
                        "Status",
                        "Cost",
                        "Cost diff",
                        "Time diff",
                        "Rows diff",
                    ] {
                        ui.label(egui::RichText::new(h).strong());
                    }
                    ui.end_row();

                    for r in &diff.rows {
                        let indent = "   ".repeat(r.depth);
                        ui.label(format!("{indent}{}", truncate_chars(&r.label, 60)))
                            .on_hover_text(&r.label);
                        let (status, color) = match r.status {
                            DeltaStatus::Same => ("same", Color32::GRAY),
                            DeltaStatus::Changed => ("changed", Color32::from_rgb(100, 181, 246)),
                            DeltaStatus::Added => ("added", Color32::from_rgb(255, 183, 77)),
                            DeltaStatus::Removed => ("removed", BAD),
                        };
                        ui.colored_label(color, status);

                        let cost = match (r.base, r.current) {
                            (Some(b), Some(c)) => {
                                format!("{:.2} → {:.2}", b.total_cost, c.total_cost)
                            }
                            (None, Some(c)) => format!("{:.2}", c.total_cost),
                            (Some(b), None) => format!("{:.2}", b.total_cost),
                            (None, None) => String::new(),
                        };
                        ui.label(egui::RichText::new(cost).family(egui::FontFamily::Monospace));

                        match r.cost_delta() {
                            Some(d) => {
                                ui.colored_label(delta_color(d), format!("{d:+.2}"));
                            }
                            None => {
                                ui.label("");
                            }
                        }
                        match r.time_delta() {
                            Some(d) => {
                                ui.colored_label(delta_color(d), format!("{d:+.3} ms"));
                            }
                            None => {
                                ui.label("");
                            }
                        }
                        match r.rows_delta() {
                            Some(d) => {
                                ui.label(format!("{d:+}"));
                            }
                            None => {
                                ui.label("");
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}
