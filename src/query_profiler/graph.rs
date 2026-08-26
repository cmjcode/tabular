use super::{parse_explain, ExplainNode, ExplainSummary, ProfilerWarning};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfilerViewMode {
    #[default]
    VisualGraph,
    TreeList,
    Advisor,
    RawPlan,
}

/// Persistent GUI state for Query Profiler
#[derive(Debug, Clone)]
pub struct QueryProfilerState {
    pub view_mode: ProfilerViewMode,
    pub selected_node_id: Option<usize>,
    pub search_query: String,
    pub zoom: f32,
    pub pan_offset: Vec2,
    pub is_dragging: bool,
    pub collapsed_nodes: HashSet<usize>,
}

impl Default for QueryProfilerState {
    fn default() -> Self {
        Self {
            view_mode: ProfilerViewMode::VisualGraph,
            selected_node_id: None,
            search_query: String::new(),
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            is_dragging: false,
            collapsed_nodes: HashSet::new(),
        }
    }
}

/// Entry point to render the full Visual Execution Profiler & Query Intelligence interface
pub fn render_query_profiler(ui: &mut egui::Ui, raw_plan: &str) {
    let state_id = ui.id().with("query_profiler_state");
    let mut state = ui.data_mut(|d| {
        d.get_temp::<QueryProfilerState>(state_id)
            .unwrap_or_default()
    });

    let parse_result = parse_explain(raw_plan);

    egui::Frame::NONE
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            if let Some((root, summary)) = parse_result {
                render_profiler_header(ui, &summary, &mut state);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                match state.view_mode {
                    ProfilerViewMode::VisualGraph => {
                        render_graph_canvas_and_inspector(ui, &root, &summary, &mut state);
                    }
                    ProfilerViewMode::TreeList => {
                        render_tree_list_view(ui, &root, &summary, &mut state);
                    }
                    ProfilerViewMode::Advisor => {
                        render_advisor_view(ui, &root, &summary, &mut state);
                    }
                    ProfilerViewMode::RawPlan => {
                        render_raw_plan_view(ui, raw_plan);
                    }
                }
            } else {
                render_raw_fallback(ui, raw_plan);
            }
        });

    ui.data_mut(|d| d.insert_temp(state_id, state));
}

// ─────────────────────────────────────────────────────────────────────────────
// Header & Metrics Bar
// ─────────────────────────────────────────────────────────────────────────────

fn render_profiler_header(
    ui: &mut egui::Ui,
    summary: &ExplainSummary,
    state: &mut QueryProfilerState,
) {
    ui.horizontal(|ui| {
        ui.heading(
            egui::RichText::new("⚡ Visual Execution Profiler")
                .strong()
                .color(if ui.visuals().dark_mode {
                    Color32::from_rgb(129, 212, 250)
                } else {
                    Color32::from_rgb(2, 136, 209)
                }),
        );

        ui.add_space(10.0);

        // Engine badge
        let engine_bg = if ui.visuals().dark_mode {
            Color32::from_rgb(38, 50, 56)
        } else {
            Color32::from_rgb(236, 239, 241)
        };
        egui::Frame::NONE
            .fill(engine_bg)
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(summary.engine.display_name())
                        .size(11.0)
                        .strong()
                        .color(if ui.visuals().dark_mode {
                            Color32::from_rgb(176, 190, 197)
                        } else {
                            Color32::from_rgb(69, 90, 100)
                        }),
                );
            });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // View Mode Selector
            ui.selectable_value(&mut state.view_mode, ProfilerViewMode::RawPlan, "📝 Raw");
            ui.selectable_value(
                &mut state.view_mode,
                ProfilerViewMode::Advisor,
                &format!("💡 Advisor ({})", summary.warnings_count),
            );
            ui.selectable_value(&mut state.view_mode, ProfilerViewMode::TreeList, "🌲 Tree List");
            ui.selectable_value(&mut state.view_mode, ProfilerViewMode::VisualGraph, "📊 Graph");
        });
    });

    ui.add_space(8.0);

    // Summary Metric Tiles Bar
    ui.horizontal_wrapped(|ui| {
        let is_dark = ui.visuals().dark_mode;
        let card_bg = if is_dark {
            Color32::from_rgb(26, 30, 38)
        } else {
            Color32::from_rgb(245, 247, 250)
        };
        let border_stroke = Stroke::new(
            1.0,
            if is_dark {
                Color32::from_rgb(45, 50, 62)
            } else {
                Color32::from_rgb(220, 225, 235)
            },
        );

        // Tile 1: Total Duration
        render_metric_card(
            ui,
            card_bg,
            border_stroke,
            "⏱ Duration",
            &if summary.total_duration_ms > 0.0 {
                format!("{:.2} ms", summary.total_duration_ms)
            } else {
                "N/A".to_string()
            },
            Color32::from_rgb(255, 183, 77),
        );

        // Tile 2: Total Cost
        render_metric_card(
            ui,
            card_bg,
            border_stroke,
            "💰 Max Cost",
            &format!("{:.2}", summary.total_cost),
            Color32::from_rgb(129, 199, 132),
        );

        // Tile 3: Rows Output
        render_metric_card(
            ui,
            card_bg,
            border_stroke,
            "📦 Est / Act Rows",
            &format!("{}", summary.total_rows),
            Color32::from_rgb(100, 181, 246),
        );

        // Tile 4: Buffer Hit Rate
        let buf_color = if summary.buffer_hit_rate >= 90.0 {
            Color32::from_rgb(129, 199, 132)
        } else if summary.buffer_hit_rate >= 75.0 {
            Color32::from_rgb(255, 213, 79)
        } else {
            Color32::from_rgb(229, 115, 115)
        };
        render_metric_card(
            ui,
            card_bg,
            border_stroke,
            "🧠 Buffer Hit Rate",
            &format!("{:.1}%", summary.buffer_hit_rate),
            buf_color,
        );

        // Tile 5: Bottlenecks & Warnings
        let warn_color = if summary.warnings_count > 0 || summary.bottlenecks_count > 0 {
            Color32::from_rgb(244, 67, 54)
        } else {
            Color32::from_rgb(129, 199, 132)
        };
        render_metric_card(
            ui,
            card_bg,
            border_stroke,
            "⚠️ Bottlenecks / Warnings",
            &format!("{} / {}", summary.bottlenecks_count, summary.warnings_count),
            warn_color,
        );
    });
}

fn render_metric_card(
    ui: &mut egui::Ui,
    bg: Color32,
    border: Stroke,
    title: &str,
    value: &str,
    val_color: Color32,
) {
    egui::Frame::NONE
        .fill(bg)
        .stroke(border)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).size(10.0).color(Color32::GRAY));
                ui.label(
                    egui::RichText::new(value)
                        .size(13.0)
                        .strong()
                        .color(val_color),
                );
            });
        });
}

// ─────────────────────────────────────────────────────────────────────────────
// Visual Graph Canvas & Layout
// ─────────────────────────────────────────────────────────────────────────────

struct NodeLayout {
    id: usize,
    rect: Rect,
    children_indices: Vec<usize>,
}

fn render_graph_canvas_and_inspector(
    ui: &mut egui::Ui,
    root: &ExplainNode,
    _summary: &ExplainSummary,
    state: &mut QueryProfilerState,
) {
    // Toolbar: Search filter, zoom buttons, reset
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("🔍").size(12.0));
        ui.add(
            egui::TextEdit::singleline(&mut state.search_query)
                .hint_text("Search table, index, or operation...")
                .desired_width(220.0),
        );

        ui.separator();

        if ui.button("➕ Zoom In").clicked() {
            state.zoom = (state.zoom * 1.15).min(2.5);
        }
        if ui.button("➖ Zoom Out").clicked() {
            state.zoom = (state.zoom / 1.15).max(0.4);
        }
        if ui.button("🔄 Reset View").clicked() {
            state.zoom = 1.0;
            state.pan_offset = Vec2::ZERO;
            state.selected_node_id = None;
        }

        ui.label(
            egui::RichText::new(format!("Scale: {:.0}%", state.zoom * 100.0))
                .size(11.0)
                .color(Color32::GRAY),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if state.selected_node_id.is_some() && ui.button("✕ Close Inspector").clicked() {
                state.selected_node_id = None;
            }
        });
    });

    ui.add_space(4.0);

    let inspector_open = state.selected_node_id.is_some();

    if inspector_open {
        let available_size = ui.available_size();
        let inspector_width = 340.0f32.min(available_size.x * 0.45);
        let canvas_width = available_size.x - inspector_width - 8.0;

        ui.horizontal(|ui| {
            // Left: Canvas
            ui.allocate_ui(egui::vec2(canvas_width, available_size.y), |ui| {
                render_canvas_viewport(ui, root, state);
            });

            ui.separator();

            // Right: Inspector
            ui.allocate_ui(egui::vec2(inspector_width, available_size.y), |ui| {
                if let Some(sel_id) = state.selected_node_id {
                    if let Some(selected_node) = root.find_node_by_id(sel_id) {
                        render_node_inspector_drawer(ui, selected_node);
                    }
                }
            });
        });
    } else {
        render_canvas_viewport(ui, root, state);
    }
}

fn render_canvas_viewport(
    ui: &mut egui::Ui,
    root: &ExplainNode,
    state: &mut QueryProfilerState,
) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );

    // Pan interaction
    if response.dragged_by(egui::PointerButton::Primary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        state.pan_offset += response.drag_delta();
    }

    // Scroll zoom interaction
    let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
    if response.hovered() && scroll_delta.abs() > 0.1 {
        let zoom_factor = if scroll_delta > 0.0 { 1.05 } else { 0.95 };
        state.zoom = (state.zoom * zoom_factor).clamp(0.4, 2.5);
    }

    // 1. Calculate Hierarchical Tree Layout
    let node_width = 240.0 * state.zoom;
    let node_height = 95.0 * state.zoom;
    let h_spacing = 30.0 * state.zoom;
    let v_spacing = 60.0 * state.zoom;

    let mut layouts: Vec<NodeLayout> = Vec::new();
    let center_x = response.rect.center().x + state.pan_offset.x;
    let start_y = response.rect.top() + 30.0 + state.pan_offset.y;

    calculate_tree_layout(
        root,
        Pos2::new(center_x, start_y),
        node_width,
        node_height,
        h_spacing,
        v_spacing,
        &mut layouts,
    );

    // 2. Draw Connector Lines
    let is_dark = ui.visuals().dark_mode;
    let line_color = if is_dark {
        Color32::from_rgb(90, 100, 120)
    } else {
        Color32::from_rgb(180, 190, 205)
    };

    for layout in &layouts {
        let parent_bottom = Pos2::new(layout.rect.center().x, layout.rect.bottom());
        for &child_idx in &layout.children_indices {
            if let Some(child_layout) = layouts.get(child_idx) {
                let child_top = Pos2::new(child_layout.rect.center().x, child_layout.rect.top());
                
                // Draw smooth bezier curve
                let cp1 = Pos2::new(parent_bottom.x, parent_bottom.y + v_spacing * 0.4);
                let cp2 = Pos2::new(child_top.x, child_top.y - v_spacing * 0.4);
                let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                    [parent_bottom, cp1, cp2, child_top],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(2.0 * state.zoom, line_color),
                );
                painter.add(shape);
            }
        }
    }

    // 3. Draw Nodes & Handle Clicks
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let clicked = response.clicked();

    for layout in &layouts {
        if let Some(node) = root.find_node_by_id(layout.id) {
            let is_selected = state.selected_node_id == Some(node.id);
            let is_hovered = pointer_pos.map(|p| layout.rect.contains(p)).unwrap_or(false);

            if clicked && is_hovered {
                state.selected_node_id = Some(node.id);
            }

            render_node_card(
                &painter,
                node,
                layout.rect,
                is_selected,
                is_hovered,
                state.zoom,
                is_dark,
                &state.search_query,
            );
        }
    }
}

fn calculate_tree_layout(
    node: &ExplainNode,
    pos: Pos2,
    node_w: f32,
    node_h: f32,
    h_spacing: f32,
    v_spacing: f32,
    layouts: &mut Vec<NodeLayout>,
) -> (f32, usize) {
    let my_idx = layouts.len();
    layouts.push(NodeLayout {
        id: node.id,
        rect: Rect::from_min_size(pos, Vec2::new(node_w, node_h)),
        children_indices: Vec::new(),
    });

    if node.children.is_empty() {
        return (node_w, my_idx);
    }

    let mut children_indices = Vec::new();
    let mut total_subtree_width = 0.0;
    let mut child_widths = Vec::new();

    // First pass: compute subtree width of all children
    for (i, child) in node.children.iter().enumerate() {
        // Dummy layout computation
        let (cw, _) = compute_subtree_width(child, node_w, h_spacing);
        child_widths.push(cw);
        total_subtree_width += cw;
        if i > 0 {
            total_subtree_width += h_spacing;
        }
    }

    // Second pass: position children centered under parent
    let mut cur_x = pos.x + (node_w / 2.0) - (total_subtree_width / 2.0);
    let child_y = pos.y + node_h + v_spacing;

    for (child, &cw) in node.children.iter().zip(child_widths.iter()) {
        let child_x = cur_x + (cw / 2.0) - (node_w / 2.0);
        let (_, c_idx) = calculate_tree_layout(
            child,
            Pos2::new(child_x, child_y),
            node_w,
            node_h,
            h_spacing,
            v_spacing,
            layouts,
        );
        children_indices.push(c_idx);
        cur_x += cw + h_spacing;
    }

    layouts[my_idx].children_indices = children_indices;
    (total_subtree_width.max(node_w), my_idx)
}

fn compute_subtree_width(node: &ExplainNode, node_w: f32, h_spacing: f32) -> (f32, ()) {
    if node.children.is_empty() {
        return (node_w, ());
    }
    let mut total = 0.0;
    for (i, child) in node.children.iter().enumerate() {
        let (cw, _) = compute_subtree_width(child, node_w, h_spacing);
        total += cw;
        if i > 0 {
            total += h_spacing;
        }
    }
    (total.max(node_w), ())
}

fn render_node_card(
    painter: &egui::Painter,
    node: &ExplainNode,
    rect: Rect,
    is_selected: bool,
    is_hovered: bool,
    zoom: f32,
    is_dark: bool,
    search_query: &str,
) {
    let mut card_bg = if is_dark {
        Color32::from_rgb(30, 33, 42)
    } else {
        Color32::from_rgb(250, 252, 255)
    };

    let mut border_color = if is_dark {
        Color32::from_rgb(55, 60, 75)
    } else {
        Color32::from_rgb(210, 218, 230)
    };

    // Bottleneck / High Cost highlight
    if node.is_bottleneck {
        border_color = Color32::from_rgb(244, 67, 54);
        card_bg = if is_dark {
            Color32::from_rgb(45, 25, 28)
        } else {
            Color32::from_rgb(255, 235, 238)
        };
    }

    if is_selected {
        border_color = Color32::from_rgb(33, 150, 243);
    } else if is_hovered {
        border_color = Color32::from_rgb(100, 181, 246);
    }

    // Search query match highlight
    if !search_query.trim().is_empty() {
        let q = search_query.to_lowercase();
        let matches = node.node_type.to_lowercase().contains(&q)
            || node.relation_name.as_deref().unwrap_or("").to_lowercase().contains(&q)
            || node.index_name.as_deref().unwrap_or("").to_lowercase().contains(&q);
        if matches {
            border_color = Color32::from_rgb(255, 215, 0);
        }
    }

    // Draw Card Background & Border
    painter.rect(
        rect,
        6.0 * zoom,
        card_bg,
        Stroke::new(if is_selected || node.is_bottleneck { 2.0 } else { 1.0 }, border_color),
        egui::StrokeKind::Outside,
    );

    let pad = 8.0 * zoom;
    let mut cur_y = rect.top() + pad;

    // Operation Badge & Title
    let (badge_text, badge_color) = get_badge_info(&node.node_type);
    painter.text(
        Pos2::new(rect.left() + pad, cur_y),
        egui::Align2::LEFT_TOP,
        badge_text,
        egui::FontId::proportional(9.0 * zoom),
        badge_color,
    );

    if let Some(ref rel) = node.relation_name {
        painter.text(
            Pos2::new(rect.right() - pad, cur_y),
            egui::Align2::RIGHT_TOP,
            truncate_str(rel, 16),
            egui::FontId::proportional(10.0 * zoom),
            if is_dark {
                Color32::from_rgb(129, 212, 250)
            } else {
                Color32::from_rgb(2, 136, 209)
            },
        );
    }

    cur_y += 14.0 * zoom;

    // Node Type Name
    painter.text(
        Pos2::new(rect.left() + pad, cur_y),
        egui::Align2::LEFT_TOP,
        truncate_str(&node.node_type, 22),
        egui::FontId::proportional(11.0 * zoom),
        if is_dark { Color32::WHITE } else { Color32::BLACK },
    );

    cur_y += 16.0 * zoom;

    // Cost & Timing row
    let cost_str = format!("Cost: {:.1} ({:.0}%)", node.total_cost, node.cost_percentage);
    painter.text(
        Pos2::new(rect.left() + pad, cur_y),
        egui::Align2::LEFT_TOP,
        cost_str,
        egui::FontId::proportional(9.5 * zoom),
        if is_dark { Color32::LIGHT_GRAY } else { Color32::DARK_GRAY },
    );

    let rows_str = format!("Rows: {}", node.actual_rows.unwrap_or(node.plan_rows));
    painter.text(
        Pos2::new(rect.right() - pad, cur_y),
        egui::Align2::RIGHT_TOP,
        rows_str,
        egui::FontId::proportional(9.5 * zoom),
        if is_dark { Color32::LIGHT_GRAY } else { Color32::DARK_GRAY },
    );

    cur_y += 16.0 * zoom;

    // Cost % Progress Bar
    let bar_rect = Rect::from_min_size(
        Pos2::new(rect.left() + pad, cur_y),
        Vec2::new(rect.width() - 2.0 * pad, 4.0 * zoom),
    );
    let bar_bg = if is_dark {
        Color32::from_rgb(45, 50, 65)
    } else {
        Color32::from_rgb(220, 225, 235)
    };
    painter.rect_filled(bar_rect, 2.0 * zoom, bar_bg);

    let fill_w = (bar_rect.width() * (node.cost_percentage / 100.0)).clamp(0.0, bar_rect.width());
    if fill_w > 0.0 {
        let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_rect.height()));
        let fill_color = get_cost_color(node.cost_percentage);
        painter.rect_filled(fill_rect, 2.0 * zoom, fill_color);
    }

    cur_y += 8.0 * zoom;

    // Warning indicators
    if !node.warnings.is_empty() {
        let warn_text = format!("⚠️ {} warning(s)", node.warnings.len());
        painter.text(
            Pos2::new(rect.left() + pad, cur_y),
            egui::Align2::LEFT_TOP,
            warn_text,
            egui::FontId::proportional(9.0 * zoom),
            Color32::from_rgb(255, 152, 0),
        );
    } else if let Some(time) = node.actual_total_time {
        let time_str = format!("⏱ {:.2} ms", time);
        painter.text(
            Pos2::new(rect.left() + pad, cur_y),
            egui::Align2::LEFT_TOP,
            time_str,
            egui::FontId::proportional(9.0 * zoom),
            Color32::from_rgb(255, 183, 77),
        );
    }
}

fn get_badge_info(node_type: &str) -> (&'static str, Color32) {
    let lower = node_type.to_lowercase();
    if lower.contains("index") {
        ("INDEX SCAN", Color32::from_rgb(76, 175, 80))
    } else if lower.contains("seq scan") || lower.contains("full table") || lower.contains("table scan") {
        ("SEQ SCAN", Color32::from_rgb(255, 112, 67))
    } else if lower.contains("join") || lower.contains("nested loop") {
        ("JOIN", Color32::from_rgb(33, 150, 243))
    } else if lower.contains("sort") {
        ("SORT", Color32::from_rgb(171, 71, 188))
    } else if lower.contains("aggregate") || lower.contains("group") {
        ("AGGREGATE", Color32::from_rgb(0, 172, 193))
    } else {
        ("EXECUTE", Color32::from_rgb(120, 144, 156))
    }
}

fn get_cost_color(pct: f32) -> Color32 {
    if pct >= 75.0 {
        Color32::from_rgb(244, 67, 54) // Red
    } else if pct >= 45.0 {
        Color32::from_rgb(255, 152, 0) // Orange
    } else if pct >= 20.0 {
        Color32::from_rgb(255, 213, 79) // Yellow
    } else {
        Color32::from_rgb(76, 175, 80) // Green
    }
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Node Inspector Drawer
// ─────────────────────────────────────────────────────────────────────────────

fn render_node_inspector_drawer(ui: &mut egui::Ui, node: &ExplainNode) {
    egui::ScrollArea::vertical()
        .id_salt("profiler_node_inspector_scroll")
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.heading(
                    egui::RichText::new(&node.node_type)
                        .size(15.0)
                        .strong(),
                );

                if let Some(ref rel) = node.relation_name {
                    ui.label(
                        egui::RichText::new(format!("Relation: {}", rel))
                            .color(Color32::from_rgb(100, 181, 246))
                            .strong(),
                    );
                }
                if let Some(ref idx) = node.index_name {
                    ui.label(
                        egui::RichText::new(format!("Index: {}", idx))
                            .color(Color32::from_rgb(129, 199, 132))
                            .strong(),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Warnings section in drawer
                if !node.warnings.is_empty() {
                    ui.label(
                        egui::RichText::new("⚠️ Identified Issues & Optimization Tips")
                            .strong()
                            .color(Color32::from_rgb(255, 152, 0)),
                    );
                    ui.add_space(4.0);

                    for warn in &node.warnings {
                        render_warning_box(ui, warn);
                        ui.add_space(4.0);
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                }

                // Cost & Timing Breakdown
                ui.label(egui::RichText::new("📊 Cost & Execution Metrics").strong());
                ui.add_space(4.0);

                egui::Grid::new("node_metrics_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Total Cost:");
                        ui.label(format!("{:.2} ({:.1}%)", node.total_cost, node.cost_percentage));
                        ui.end_row();

                        ui.label("Startup Cost:");
                        ui.label(format!("{:.2}", node.startup_cost));
                        ui.end_row();

                        if let Some(time) = node.actual_total_time {
                            ui.label("Actual Time:");
                            ui.label(format!("{:.3} ms ({:.1}%)", time, node.time_percentage));
                            ui.end_row();
                        }

                        ui.label("Plan Rows:");
                        ui.label(format!("{}", node.plan_rows));
                        ui.end_row();

                        if let Some(act) = node.actual_rows {
                            ui.label("Actual Rows:");
                            ui.label(format!("{}", act));
                            ui.end_row();
                        }

                        if let Some(loops) = node.actual_loops {
                            ui.label("Loops:");
                            ui.label(format!("{}", loops));
                            ui.end_row();
                        }
                    });

                // Buffer I/O Details
                if node.buffer_hit.is_some() || node.buffer_read.is_some() || node.temp_written_blocks.is_some() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("🧠 Buffer & I/O Stats").strong());
                    ui.add_space(4.0);

                    egui::Grid::new("node_buffer_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .striped(true)
                        .show(ui, |ui| {
                            if let Some(hit) = node.buffer_hit {
                                ui.label("Shared Hit (RAM):");
                                ui.label(format!("{} blocks", hit));
                                ui.end_row();
                            }
                            if let Some(read) = node.buffer_read {
                                ui.label("Shared Read (Disk):");
                                ui.label(format!("{} blocks", read));
                                ui.end_row();
                            }
                            if let Some(temp_w) = node.temp_written_blocks {
                                ui.label("Temp Written:");
                                ui.label(format!("{} blocks", temp_w));
                                ui.end_row();
                            }
                        });
                }

                // Predicates & Filters
                if node.filter.is_some() || node.index_cond.is_some() || node.hash_cond.is_some() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("🔍 Filters & Conditions").strong());
                    ui.add_space(4.0);

                    if let Some(ref f) = node.filter {
                        ui.label("Filter:");
                        ui.code(f);
                        if let Some(removed) = node.rows_removed_by_filter {
                            ui.label(
                                egui::RichText::new(format!("Rows Removed: {}", removed))
                                    .size(11.0)
                                    .color(Color32::from_rgb(255, 183, 77)),
                            );
                        }
                    }
                    if let Some(ref ic) = node.index_cond {
                        ui.label("Index Cond:");
                        ui.code(ic);
                    }
                    if let Some(ref hc) = node.hash_cond {
                        ui.label("Hash Cond:");
                        ui.code(hc);
                    }
                }
            });
        });
}

fn render_warning_box(ui: &mut egui::Ui, warn: &ProfilerWarning) {
    let (r, g, b) = warn.severity.badge_color();
    let color = Color32::from_rgb(r, g, b);

    let is_dark = ui.visuals().dark_mode;
    let bg = if is_dark {
        Color32::from_rgb(35, 28, 30)
    } else {
        Color32::from_rgb(255, 243, 224)
    };

    egui::Frame::NONE
        .fill(bg)
        .stroke(Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(warn.severity.title_prefix())
                            .size(10.0)
                            .strong()
                            .color(color),
                    );
                    ui.label(egui::RichText::new(&warn.title).size(11.5).strong());
                });

                ui.add_space(3.0);
                ui.label(egui::RichText::new(&warn.description).size(11.0));

                if let Some(ref rec) = warn.recommendation {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("💡 Advice: {}", rec))
                            .size(11.0)
                            .color(Color32::from_rgb(76, 175, 80))
                            .strong(),
                    );
                }
            });
        });
}

// ─────────────────────────────────────────────────────────────────────────────
// Tree List & Advisor Views
// ─────────────────────────────────────────────────────────────────────────────

fn render_tree_list_view(
    ui: &mut egui::Ui,
    root: &ExplainNode,
    _summary: &ExplainSummary,
    state: &mut QueryProfilerState,
) {
    egui::ScrollArea::both()
        .id_salt("profiler_tree_list_scroll")
        .show(ui, |ui| {
            render_tree_list_node(ui, root, 0, state);
        });
}

fn render_tree_list_node(
    ui: &mut egui::Ui,
    node: &ExplainNode,
    depth: usize,
    state: &mut QueryProfilerState,
) {
    let is_selected = state.selected_node_id == Some(node.id);
    let (badge_text, badge_color) = get_badge_info(&node.node_type);

    ui.horizontal(|ui| {
        if depth > 0 {
            ui.add_space((depth as f32) * 18.0);
            ui.label(egui::RichText::new("└─").color(Color32::GRAY));
        }

        let is_dark = ui.visuals().dark_mode;
        let card_bg = if is_selected {
            if is_dark { Color32::from_rgb(40, 50, 70) } else { Color32::from_rgb(225, 238, 255) }
        } else if node.is_bottleneck {
            if is_dark { Color32::from_rgb(45, 25, 28) } else { Color32::from_rgb(255, 235, 238) }
        } else if is_dark {
            Color32::from_rgb(28, 30, 38)
        } else {
            Color32::from_rgb(248, 250, 252)
        };

        let card_stroke = Stroke::new(
            1.0,
            if is_selected {
                Color32::from_rgb(33, 150, 243)
            } else if node.is_bottleneck {
                Color32::from_rgb(244, 67, 54)
            } else if is_dark {
                Color32::from_rgb(48, 52, 65)
            } else {
                Color32::from_rgb(220, 225, 235)
            },
        );

        let response = egui::Frame::NONE
            .fill(card_bg)
            .stroke(card_stroke)
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_min_width(550.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(badge_text)
                            .size(10.0)
                            .strong()
                            .color(badge_color),
                    );

                    ui.label(
                        egui::RichText::new(&node.node_type)
                            .size(12.0)
                            .strong(),
                    );

                    if let Some(ref rel) = node.relation_name {
                        ui.label(
                            egui::RichText::new(format!("on {}", rel))
                                .color(Color32::from_rgb(100, 181, 246))
                                .size(11.5),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !node.warnings.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("⚠️ {} issues", node.warnings.len()))
                                    .color(Color32::from_rgb(255, 152, 0))
                                    .size(10.5),
                            );
                        }

                        if let Some(time) = node.actual_total_time {
                            ui.label(
                                egui::RichText::new(format!("{:.2} ms", time))
                                    .color(Color32::from_rgb(255, 183, 77))
                                    .size(11.0),
                            );
                        }

                        ui.label(
                            egui::RichText::new(format!("Cost: {:.1}", node.total_cost))
                                .color(get_cost_color(node.cost_percentage))
                                .size(11.0)
                                .strong(),
                        );
                    });
                });
            });

        if response.response.interact(egui::Sense::click()).clicked() {
            state.selected_node_id = Some(node.id);
        }
    });

    ui.add_space(3.0);

    for child in &node.children {
        render_tree_list_node(ui, child, depth + 1, state);
    }
}

fn render_advisor_view(
    ui: &mut egui::Ui,
    root: &ExplainNode,
    summary: &ExplainSummary,
    _state: &mut QueryProfilerState,
) {
    let mut all_nodes = Vec::new();
    root.collect_all_nodes(&mut all_nodes);

    let mut all_warnings: Vec<(&ExplainNode, &ProfilerWarning)> = Vec::new();
    for n in &all_nodes {
        for w in &n.warnings {
            all_warnings.push((n, w));
        }
    }

    // Sort by severity descending
    all_warnings.sort_by(|a, b| b.1.severity.cmp(&a.1.severity));

    egui::ScrollArea::vertical()
        .id_salt("profiler_advisor_scroll")
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.heading(
                    egui::RichText::new("💡 Query Intelligence & Optimization Advisor")
                        .size(15.0)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "Identified {} actionable recommendation(s) across {} execution nodes.",
                        all_warnings.len(),
                        summary.bottlenecks_count
                    ))
                    .color(Color32::GRAY)
                    .size(11.5),
                );

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                if all_warnings.is_empty() {
                    ui.label(
                        egui::RichText::new("✅ Excellent! No major performance anti-patterns detected.")
                            .color(Color32::from_rgb(76, 175, 80))
                            .strong(),
                    );
                } else {
                    for (node, warn) in all_warnings {
                        ui.label(
                            egui::RichText::new(format!("Operator: {} (ID #{})", node.node_type, node.id))
                                .size(11.0)
                                .color(Color32::GRAY),
                        );
                        ui.add_space(2.0);
                        render_warning_box(ui, warn);
                        ui.add_space(10.0);
                    }
                }
            });
        });
}

fn render_raw_plan_view(ui: &mut egui::Ui, raw_plan: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Raw EXPLAIN Output:").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("📋 Copy Raw Plan").clicked() {
                ui.ctx().copy_text(raw_plan.to_string());
            }
        });
    });

    ui.add_space(6.0);
    egui::ScrollArea::both()
        .id_salt("profiler_raw_scroll")
        .show(ui, |ui| {
            ui.code(raw_plan);
        });
}

fn render_raw_fallback(ui: &mut egui::Ui, raw_plan: &str) {
    ui.colored_label(
        Color32::from_rgb(255, 183, 77),
        "⚠️ Could not parse structured EXPLAIN tree. Showing raw output:",
    );
    ui.add_space(6.0);
    render_raw_plan_view(ui, raw_plan);
}
