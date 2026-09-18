use crate::models::structs::{DiagramNode, DiagramState, RelationOrigin, VirtualRelation};
use crate::rfd;
use eframe::egui;
use serde::{Deserialize, Serialize};

/// Palet warna group (tanpa duplikat), dipakai menu warna, grouping otomatis,
/// dan group hasil impor Mermaid.
pub const GROUP_COLORS: [egui::Color32; 20] = [
    egui::Color32::from_rgb(100, 149, 237), // Cornflower Blue
    egui::Color32::from_rgb(60, 179, 113),  // Medium Sea Green
    egui::Color32::from_rgb(205, 92, 92),   // Indian Red
    egui::Color32::from_rgb(218, 165, 32),  // Goldenrod
    egui::Color32::from_rgb(147, 112, 219), // Medium Purple
    egui::Color32::from_rgb(70, 130, 180),  // Steel Blue
    egui::Color32::from_rgb(255, 127, 80),  // Coral
    egui::Color32::from_rgb(255, 105, 180), // Hot Pink
    egui::Color32::from_rgb(0, 206, 209),   // Dark Turquoise
    egui::Color32::from_rgb(123, 104, 238), // Medium Slate Blue
    egui::Color32::from_rgb(50, 205, 50),   // Lime Green
    egui::Color32::from_rgb(255, 165, 0),   // Orange
    egui::Color32::from_rgb(106, 90, 205),  // Slate Blue
    egui::Color32::from_rgb(255, 99, 71),   // Tomato
    egui::Color32::from_rgb(64, 224, 208),  // Turquoise
    egui::Color32::from_rgb(238, 130, 238), // Violet
    egui::Color32::from_rgb(255, 215, 0),   // Gold
    egui::Color32::from_rgb(0, 250, 154),   // Medium Spring Green
    egui::Color32::from_rgb(138, 43, 226),  // Blue Violet
    egui::Color32::from_rgb(255, 140, 0),   // Dark Orange
];

pub const MIN_ZOOM: f32 = 0.5;
pub const MAX_ZOOM: f32 = 1.5;
pub const DEFAULT_ZOOM: f32 = 1.0;

/// Aksi dari toolbar diagram yang butuh state aplikasi (toast, vault, database).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramAction {
    /// Simpan diagram (ke disk lokal dan otomatis ke Obsidian vault bila aktif).
    Save,
    /// Simpan skema sebagai catatan Mermaid di vault Obsidian.
    SaveToVault,
    /// Simpan seluruh state diagram ke tabel `diagram_by_tabular` di database target.
    SaveToDatabase,
    /// Muat ulang diagram dari tabel `diagram_by_tabular` di database target.
    LoadFromDatabase,
    Info(String),
    Error(String),
}

/// Tulis file secara atomik: tulis ke `.tmp` lalu rename, supaya crash di
/// tengah penulisan tidak meninggalkan file setengah jadi.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn export_json(state: &DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .save_file()?;
    Some(
        match serde_json::to_vec_pretty(state)
            .map_err(|e| e.to_string())
            .and_then(|bytes| write_atomic(&path, &bytes).map_err(|e| e.to_string()))
        {
            Ok(()) => DiagramAction::Info(format!("Diagram exported to {}", path.display())),
            Err(e) => DiagramAction::Error(format!("Export failed: {e}")),
        },
    )
}

fn export_mermaid(state: &DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("Mermaid", &["mmd", "mermaid"])
        .add_filter("Markdown", &["md"])
        .save_file()?;
    let model = crate::diagram_mermaid::ErModel::from_diagram(state);
    let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");
    let text = if is_md {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Schema");
        crate::diagram_mermaid::schema_note_markdown(stem, &model)
    } else {
        model.to_mermaid(Default::default())
    };
    Some(match write_atomic(&path, text.as_bytes()) {
        Ok(()) => DiagramAction::Info(format!("Mermaid exported to {}", path.display())),
        Err(e) => DiagramAction::Error(format!("Export failed: {e}")),
    })
}

fn import_json(state: &mut DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()?;
    let result = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<DiagramState>(&bytes).map_err(|e| e.to_string())
        });
    Some(match result {
        Ok(new_state) => {
            *state = new_state;
            state.dragging_node = None;
            state.last_mouse_pos = None;
            state.save_requested = true;
            DiagramAction::Info("Diagram imported".to_string())
        }
        Err(e) => DiagramAction::Error(format!("Import failed: {e}")),
    })
}

fn import_mermaid(state: &mut DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("Mermaid / Markdown", &["mmd", "mermaid", "md", "txt"])
        .pick_file()?;
    let parsed = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::diagram_mermaid::parse_mermaid_er(&text));
    Some(match parsed {
        Ok(parsed) => {
            for w in &parsed.warnings {
                log::warn!("Mermaid import {}: {w}", path.display());
            }
            let stats = crate::diagram_mermaid::merge_into_state(state, &parsed.model);
            state.save_requested = true;
            let mut msg = format!(
                "Mermaid imported: {} new, {} updated tables, {} new relations",
                stats.added_tables, stats.updated_tables, stats.added_relations
            );
            if !parsed.warnings.is_empty() {
                msg.push_str(&format!(
                    " ({} lines skipped, see log)",
                    parsed.warnings.len()
                ));
            }
            DiagramAction::Info(msg)
        }
        Err(e) => DiagramAction::Error(format!("Mermaid import failed: {e}")),
    })
}

/// Pusatkan posisi semua node diagram ke tengah area tampilan (viewport).
pub fn center_diagram(state: &mut DiagramState, view_size: egui::Vec2) {
    if state.nodes.is_empty() {
        state.pan = egui::Vec2::ZERO;
        return;
    }

    // Hitung bounding box dari seluruh node tabel
    let mut min_pos = state.nodes[0].pos;
    let mut max_pos = state.nodes[0].pos + state.nodes[0].size;

    for node in &state.nodes {
        min_pos = min_pos.min(node.pos);
        max_pos = max_pos.max(node.pos + node.size);
    }

    let content_center = min_pos + (max_pos - min_pos) / 2.0;
    let view_center = view_size / 2.0;

    // Geser pan agar titik tengah konten tepat di tengah viewport
    state.pan = view_center - content_center.to_vec2() * state.zoom;
}

pub fn render_diagram(ui: &mut egui::Ui, state: &mut DiagramState) -> Option<DiagramAction> {
    let mut action: Option<DiagramAction> = None;
    let rect = ui.available_rect_before_wrap();
    // Semua gambar & interaksi dibatasi ke area diagram, supaya node/group
    // yang digeser ke atas tidak menutupi tab bar.
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));

    // Handle Pan and Zoom
    let response = ui.interact(
        rect,
        ui.id().with("diagram_bg"),
        egui::Sense::click_and_drag(),
    );

    // Pan with middle mouse or drag on background
    if response.dragged() {
        state.pan += response.drag_delta();
    }

    // Context Menu for Background
    response.context_menu(|ui| {
        if ui.button("Add Group").clicked() {
            ui.close();
            // Store the click position in Diagram coordinates
            if let Some(mouse_pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                let diagram_vec = (mouse_pos - rect.min - state.pan) / state.zoom;
                let diagram_pos = egui::pos2(diagram_vec.x, diagram_vec.y);
                state.add_group_popup = Some(diagram_pos);
                state.new_group_buffer.clear();
            }
        }
        ui.separator();
        if ui
            .checkbox(&mut state.prevent_overlap, "Prevent table overlap")
            .clicked()
        {
            if state.prevent_overlap {
                resolve_node_overlaps(&mut state.nodes, 20.0);
            }
            state.save_requested = true;
        }
        if ui.button("↔ Resolve Overlaps Now").clicked() {
            ui.close();
            resolve_node_overlaps(&mut state.nodes, 20.0);
            state.save_requested = true;
        }
        if ui.button("⚡ Auto Arrange Diagram").clicked() {
            ui.close();
            perform_auto_layout(state);
            state.save_requested = true;
        }
    });

    // Zoom & Shortcut Input Handling
    ui.input_mut(|i| {
        // Zoom In (Cmd + / Cmd =)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals)
        {
            state.zoom = (state.zoom * 1.15).min(MAX_ZOOM);
        }
        // Zoom Out (Cmd -)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus) {
            state.zoom = (state.zoom / 1.15).max(MIN_ZOOM);
        }
        // Reset Zoom (Cmd 0)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0) {
            state.zoom = DEFAULT_ZOOM;
        }

        // Mouse Wheel Zoom / Trackpad scroll (damped to prevent runaway zooming)
        let scroll_delta = i.smooth_scroll_delta.y;
        if scroll_delta != 0.0 {
            let zoom_factor = (1.0 + scroll_delta * 0.001).clamp(0.85, 1.15);
            state.zoom *= zoom_factor;
        }

        // Trackpad pinch gesture
        let zoom_delta = i.zoom_delta();
        if zoom_delta != 1.0 {
            state.zoom *= zoom_delta;
        }

        // Clamp zoom strictly within bounded range [0.25, 2.0]
        state.zoom = state.zoom.clamp(MIN_ZOOM, MAX_ZOOM);

        // Save Shortcut (Cmd + S)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) {
            state.save_requested = true;
            action = Some(DiagramAction::Save);
        }

        // Search Shortcut (Cmd + F)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::F) {
            state.show_search = !state.show_search;
            if state.show_search {
                state.search_query.clear();
            }
        }
    });

    // Handle Initial Centering
    if !state.is_centered && !state.nodes.is_empty() {
        center_diagram(state, rect.size());
        state.is_centered = true;
    }

    // Scale helper
    let scale = state.zoom;
    let pan = state.pan;

    let to_screen = move |pos: egui::Pos2| -> egui::Pos2 { rect.min + pan + pos.to_vec2() * scale };

    if state.show_grid {
        draw_grid(ui, rect, pan, scale);
    }

    // Draw Groups (Containers)
    let mut _group_rename_request: Option<(usize, String)> = None;
    let mut _group_delete_request: Option<String> = None;
    let mut group_drag_delta: Option<(String, egui::Vec2)> = None;

    // 1. Calculate Group Bounds (requires immutable access to nodes and groups)
    let mut group_bounds: Vec<(usize, String, egui::Rect, egui::Color32, String)> = Vec::new(); // (index, id, rect, color, title)

    let shift_held = ui.input(|i| i.modifiers.shift);
    for (idx, group) in state.groups.iter().enumerate() {
        let group_nodes: Vec<&DiagramNode> = state
            .nodes
            .iter()
            .filter(|n| n.is_in_group(&group.id))
            .filter(|n| {
                if Some(n.id.clone()) == state.dragging_node {
                    !shift_held
                } else {
                    true
                }
            })
            .collect();

        if group_nodes.is_empty() {
            // Handle Empty Groups with manual_pos
            if let Some(pos) = group.manual_pos {
                let size = egui::vec2(400.0, 300.0);
                let rect = egui::Rect::from_min_size(to_screen(pos), size * scale);
                group_bounds.push((
                    idx,
                    group.id.clone(),
                    rect,
                    group.color,
                    group.title.clone(),
                ));
            }
            continue;
        }

        let mut min_pos = group_nodes[0].pos;
        let mut max_pos = group_nodes[0].pos + group_nodes[0].size;

        for node in &group_nodes {
            min_pos = min_pos.min(node.pos);
            max_pos = max_pos.max(node.pos + node.size);
        }

        // Padding
        let padding = 20.0;
        let top_offset = (idx as f32 % 5.0) * 4.0;
        min_pos -= egui::vec2(padding, padding + 30.0 + top_offset);
        max_pos += egui::vec2(padding, padding);

        let min_screen = to_screen(min_pos);
        let max_screen = to_screen(max_pos);
        let rect = egui::Rect::from_min_max(min_screen, max_screen);

        group_bounds.push((
            idx,
            group.id.clone(),
            rect,
            group.color,
            group.title.clone(),
        ));
    }

    // 2. Render Groups (requires mutable access to groups for Rename, but NOT nodes)
    // We used state.nodes in step 1, now we are done with nodes.
    // But we need to update state.groups.

    for (idx, group_id, group_rect, color, _) in &group_bounds {
        // Retrieve mutable reference to group
        // We know it exists because we just got it from state.groups
        // But we can't iterate state.groups directly while modifying?
        // Actually we can iterate indices.

        let idx = *idx;
        let group_rect = *group_rect;
        let color = *color;

        // CAUTION: TextEdit needs `&mut String`.
        // We can get `&mut state.groups[idx]`

        let group = &mut state.groups[idx];

        let is_group_search_match = state.show_search
            && state.search_groups
            && !state.search_query.is_empty()
            && group.title.to_lowercase().contains(&state.search_query.to_lowercase());

        if is_group_search_match {
            ui.painter().rect_filled(
                group_rect.expand(6.0 * scale),
                12.0 * scale,
                egui::Color32::from_rgb(255, 0, 0).linear_multiply(0.35),
            );
        }

        // Draw Background
        ui.painter()
            .rect_filled(group_rect, 8.0 * scale, color.linear_multiply(0.1));
        let border_color = if is_group_search_match {
            egui::Color32::from_rgb(255, 0, 0)
        } else {
            color.linear_multiply(0.5)
        };
        let border_width = if is_group_search_match { 2.5 * scale } else { 1.0 * scale };
        ui.painter().rect_stroke(
            group_rect,
            8.0 * scale,
            egui::Stroke::new(border_width, border_color),
            egui::StrokeKind::Middle,
        );

        // Header Rect
        let title_rect =
            egui::Rect::from_min_size(group_rect.min, egui::vec2(group_rect.width(), 30.0 * scale));

        let title_fill = if is_group_search_match {
            egui::Color32::from_rgb(200, 30, 30)
        } else {
            color.linear_multiply(0.8)
        };
        ui.painter()
            .rect_filled(title_rect, 8.0 * scale, title_fill);

        let is_renaming = state.renaming_group.as_deref() == Some(group_id);

        if is_renaming {
            let edit_rect = title_rect.shrink(2.0);
            let response = ui
                .scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut group.title)
                            .frame(egui::Frame::NONE)
                            .text_color(egui::Color32::WHITE)
                            .font(egui::FontId::proportional(16.0 * scale)),
                    )
                })
                .inner;

            if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                state.renaming_group = None;
            } else {
                response.request_focus();
            }
        } else {
            ui.painter().text(
                title_rect.center(),
                egui::Align2::CENTER_CENTER,
                &group.title,
                egui::FontId::proportional(16.0 * scale),
                egui::Color32::WHITE,
            );

            // Interaction
            let interact_rect = title_rect;
            let response = ui.interact(
                interact_rect,
                ui.id().with("group_header").with(idx),
                egui::Sense::click_and_drag(),
            );

            if response.dragged() {
                let delta = response.drag_delta() / scale;
                group_drag_delta = Some((group_id.clone(), delta));
            }

            response.context_menu(|ui| {
                if ui.button("Rename Container").clicked() {
                    ui.close();
                    _group_rename_request = Some((idx, group_id.clone()));
                }
                if ui.button("Delete Group").clicked() {
                    ui.close();
                    _group_delete_request = Some(group_id.clone());
                }

                ui.horizontal(|ui| {
                    ui.label("Color:");
                    egui::ScrollArea::horizontal()
                        .max_width(200.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let colors = GROUP_COLORS;

                                for &c in &colors {
                                    let (response, painter) = ui.allocate_painter(
                                        egui::vec2(20.0, 20.0),
                                        egui::Sense::click(),
                                    );
                                    let rect = response.rect;
                                    painter.rect_filled(rect, 4.0, c);
                                    if response.hovered() {
                                        painter.rect_stroke(
                                            rect,
                                            4.0,
                                            egui::Stroke::new(2.0, egui::Color32::WHITE),
                                            egui::StrokeKind::Middle,
                                        );
                                    }
                                    if response.clicked() {
                                        group.color = c;
                                        ui.close();
                                    }
                                }
                            });
                        });
                });
            });
        }
    }

    // Apply rename request (workaround for borrow checker)
    if let Some((_, gid)) = _group_rename_request {
        state.renaming_group = Some(gid);
    }
    // Apply group deletion request
    if let Some(del_gid) = _group_delete_request {
        state.groups.retain(|g| g.id != del_gid);
        for node in &mut state.nodes {
            node.remove_from_group(&del_gid);
        }
        state.save_requested = true;
    }
    // Apply deferred group move
    if let Some((group_id, delta)) = group_drag_delta {
        // Move nodes belonging to group
        for node in &mut state.nodes {
            if node.is_in_group(&group_id) {
                node.pos += delta;
            }
        }

        // Move group manual_pos if it exists (for empty groups)
        if let Some(group) = state.groups.iter_mut().find(|g| g.id == group_id)
            && let Some(pos) = &mut group.manual_pos
        {
            *pos += delta;
        }
    }

    // Draw edges (relationships)
    let mut clicked_edge = None;
    let _pointer_pos = ui.input(|i| i.pointer.interact_pos());
    let pointer_down = ui.input(|i| i.pointer.primary_clicked());

    // Background interaction to clear selection
    if ui.input(|i| i.pointer.primary_clicked()) && !ui.ui_contains_pointer() {
        // This check is tricky because ui.interact covers the whole rect.
        // Reliance on the button click logic below is safer.
    }
    // Better: If we click the background rect (handled at start of function), we clear selection.
    // However, the background interact response is at line 8. We need to check it there?
    // Actually, we can check if any edge or node was clicked this frame. If not, and background was clicked, clear.
    // But `response.dragged()` consumes click? No, drag is different.

    // Let's implement hit testing first.

    for edge in &state.edges {
        // Resolve source and target nodes
        let src_node = state.nodes.iter().find(|n| n.id == edge.source);
        let dst_node = state.nodes.iter().find(|n| n.id == edge.target);

        if let (Some(src), Some(dst)) = (src_node, dst_node) {
            let src_rect_size = src.size * scale;
            let dst_rect_size = dst.size * scale;

            let src_pos = to_screen(src.pos) + egui::vec2(src_rect_size.x, src_rect_size.y / 2.0); // right side
            let dst_pos = to_screen(dst.pos) + egui::vec2(0.0, dst_rect_size.y / 2.0); // left side

            // Determine if selected
            let is_selected =
                state.selected_edge.as_ref() == Some(&(edge.source.clone(), edge.target.clone()));

            // Determine if highlighted by column
            let is_highlighted_by_col = if let Some((sel_table, sel_col)) = &state.selected_column {
                src.foreign_keys.iter().any(|fk| {
                    fk.referenced_table_name == edge.target
                        && ((fk.table_name == *sel_table && fk.column_name == *sel_col)
                            || (fk.referenced_table_name == *sel_table
                                && fk.referenced_column_name == *sel_col))
                })
            } else {
                false
            };

            let is_active = is_selected || is_highlighted_by_col;

            // Determine base color from source group
            let mut base_color = egui::Color32::from_gray(100);
            if let Some(group_id) = src.group_ids.first().or(src.group_id.as_ref())
                && let Some(group) = state.groups.iter().find(|g| &g.id == group_id)
            {
                base_color = group.color.linear_multiply(0.8); // Slight transparency
            }

            let color = if is_active {
                egui::Color32::from_rgb(255, 215, 0) // Gold
            } else {
                base_color
            };

            let width = if is_active { 3.0 * scale } else { 1.0 * scale };
            let stroke = egui::Stroke::new(width, color);

            // Cubic bezier for smooth connection
            let control_scale = (dst_pos.x - src_pos.x).abs().max(50.0 * scale) * 0.5;
            let control1 = src_pos + egui::vec2(control_scale, 0.0);
            let control2 = dst_pos - egui::vec2(control_scale, 0.0);

            let points = [src_pos, control1, control2, dst_pos];
            let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
                points,
                false,
                egui::Color32::TRANSPARENT,
                stroke,
            );

            // Hit detection (Check hover first)
            let mut is_hovered = false;
            // Sampling kurva hanya bila pointer di dekat bounding box edge.
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos())
                && egui::Rect::from_points(&points).expand(20.0).contains(pos)
            {
                let num_samples = 30;
                for i in 0..=num_samples {
                    let t = i as f32 / num_samples as f32;
                    let p = bezier.sample(t);
                    if p.distance(pos) < 20.0 {
                        // Increased tolerance
                        is_hovered = true;
                        break;
                    }
                }
            }

            if is_hovered {
                if pointer_down {
                    clicked_edge = Some((edge.source.clone(), edge.target.clone()));
                }
                if !is_selected {
                    // Hover feedback
                    let hover_stroke =
                        egui::Stroke::new(2.0 * scale, egui::Color32::from_gray(180));
                    ui.painter()
                        .add(egui::epaint::CubicBezierShape::from_points_stroke(
                            points,
                            false,
                            egui::Color32::TRANSPARENT,
                            hover_stroke,
                        ));
                }
            }

            ui.painter().add(bezier);
        }
    }

    let virtual_clicked = draw_virtual_relations(ui, state, rect, &to_screen, pointer_down);
    let edge_was_clicked = clicked_edge.is_some() || virtual_clicked;
    if let Some(edge) = clicked_edge {
        state.selected_edge = Some(edge);
    } else if pointer_down {
        // If clicked but not on any edge, check if we clicked a node later.
        // If not node either, we clear.
        // Simplified: We handle clear at the start or via background response if possible.
        // Actually, let's defer clearing to ensure we don't clear when clicking a node.
    }

    // Draw nodes
    let mut dragging_node_id = None;
    let mut drag_delta = egui::Vec2::ZERO;
    let mut drag_stopped_node_id: Option<String> = None;
    let mut node_clicked = false;
    let mut column_clicked_request: Option<(String, String)> = None;

    // Snapshot nodes untuk deteksi tabrakan saat dragging
    let nodes_snapshot = state.nodes.clone();

    // For manual interaction & relation search:
    let selected_column = state.selected_column.clone();
    let sel_col_is_pk = selected_column.as_ref().is_some_and(|(sel_table, sel_col)| {
        state
            .nodes
            .iter()
            .find(|n| &n.id == sel_table)
            .and_then(|n| n.column_info(sel_col))
            .is_some_and(|c| c.is_pk)
    });
    let shift_down = ui.input(|i| i.modifiers.shift);
    let ctrl_down = ui.input(|i| i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd);
    let mut link_request: Option<VirtualRelation> = None;
    let mut remove_node_request: Option<String> = None;
    let mut search_relations_for_column: Option<(String, String)> = None;

    let available_groups: Vec<(String, String, egui::Color32)> = state
        .groups
        .iter()
        .map(|g| (g.id.clone(), g.title.clone(), g.color))
        .collect();
    let mut empty_group_retention: Option<(String, egui::Pos2)> = None;
    let mut add_group_at_pos: Option<egui::Pos2> = None;

    for node in &mut state.nodes {
        node.ensure_groups_migrated();

        // Estimate height based on columns
        let header_height_unscaled = 24.0;
        let item_height_unscaled = 16.0;
        let content_height_unscaled = node.columns.len() as f32 * item_height_unscaled;
        let node_height_unscaled = header_height_unscaled + content_height_unscaled + 8.0; // padding
        let node_width = if node.column_meta.is_empty() {
            180.0
        } else {
            240.0
        };
        node.size = egui::vec2(node_width, node_height_unscaled);

        let node_size_scaled = node.size * scale;
        let node_pos_screen = to_screen(node.pos);
        let node_rect = egui::Rect::from_min_size(node_pos_screen, node_size_scaled);

        // Interact
        let node_id = ui.id().with("node").with(&node.id);
        let node_response = ui.interact(node_rect, node_id, egui::Sense::click_and_drag());

        if node_response.clicked() {
            node_clicked = true;
        }

        let mut toggle_group: Option<(String, bool)> = None;
        let mut new_group_for_node = false;
        node_response.context_menu(|ui| {
            ui.label(egui::RichText::new(&node.title).strong());
            ui.separator();

            if node.detached {
                ui.label(
                    egui::RichText::new("This table is not in the database (imported)").weak(),
                );
                if ui.button("Remove from diagram").clicked() {
                    ui.close();
                    remove_node_request = Some(node.id.clone());
                }
                ui.separator();
            }

            ui.label(egui::RichText::new("Groups:").small().weak());
            if available_groups.is_empty() {
                ui.label(egui::RichText::new("No groups created yet").weak());
            } else {
                for (gid, gtitle, gcolor) in &available_groups {
                    let in_group = node.is_in_group(gid);
                    let (prefix, action_label) = if in_group {
                        ("✓", format!("Remove from {}", gtitle))
                    } else {
                        ("➕", format!("Add to {}", gtitle))
                    };
                    let button = egui::Button::new(
                        egui::RichText::new(format!("{} {}", prefix, action_label)).color(
                            if in_group {
                                *gcolor
                            } else {
                                ui.visuals().text_color()
                            },
                        ),
                    );
                    if ui.add(button).clicked() {
                        toggle_group = Some((gid.clone(), !in_group));
                        ui.close();
                    }
                }
            }
            ui.separator();
            if ui.button("➕ New Group…").clicked() {
                new_group_for_node = true;
                ui.close();
            }
        });

        if let Some((gid, add)) = toggle_group {
            if add {
                node.add_to_group(gid);
            } else {
                node.remove_from_group(&gid);
                empty_group_retention = Some((gid, node.pos));
            }
            state.save_requested = true;
        }
        if new_group_for_node {
            add_group_at_pos = Some(node.pos + egui::vec2(node.size.x + 20.0, 0.0));
        }

        if node_response.dragged() {
            dragging_node_id = Some(node.id.clone());
            drag_delta = node_response.drag_delta();

            // Track globally for drop detection
            state.dragging_node = Some(node.id.clone());
        } else if node_response.drag_stopped() {
            let shift_held = ui.input(|i| i.modifiers.shift);
            if shift_held {
                // Check drop target
                if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    for (_, gid, rect, _, _) in &group_bounds {
                        if rect.contains(pointer_pos) {
                            node.add_to_group(gid.clone());
                            state.save_requested = true;
                            break;
                        }
                    }
                }
            }
            drag_stopped_node_id = Some(node.id.clone());
            state.dragging_node = None;
        }

        // Check if this node is part of the selected relationship
        let is_selected_edge_node = if let Some((s, t)) = &state.selected_edge {
            node.id == *s || node.id == *t
        } else {
            false
        };

        // Check if node matches search
        let is_search_match = if state.show_search && !state.search_query.is_empty() {
            let query = state.search_query.to_lowercase();
            (state.search_tables && node.title.to_lowercase().contains(&query))
                || (state.search_columns
                    && node
                        .columns
                        .iter()
                        .any(|c| c.to_lowercase().contains(&query)))
        } else {
            false
        };

        // Deteksi apakah node yang sedang di-drag sedang bertabrakan dengan tabel lain
        let is_colliding_drag = state.prevent_overlap
            && state.dragging_node.as_deref() == Some(&node.id)
            && check_single_node_collision(&nodes_snapshot, &node.id, 20.0);

        let is_glow = is_selected_edge_node || is_search_match || is_colliding_drag;

        // Draw Shadow/Border
        if is_glow {
            // Glow effect
            let glow_color = if is_search_match {
                egui::Color32::from_rgb(255, 0, 0) // Bright Red
            } else if is_colliding_drag {
                egui::Color32::from_rgb(255, 140, 0) // Warning Amber
            } else {
                egui::Color32::from_rgb(255, 215, 0) // Gold
            };

            ui.painter().rect_filled(
                node_rect.expand(6.0 * scale),
                12.0 * scale,
                glow_color.linear_multiply(0.5),
            );
        } else {
            ui.painter().rect_filled(
                node_rect.expand(2.0 * scale),
                5.0 * scale,
                egui::Color32::from_black_alpha(50),
            );
        }

        let fill_color = egui::Color32::from_rgb(30, 30, 35);
        ui.painter().rect_filled(node_rect, 4.0 * scale, fill_color);
        // Corrected rect_stroke args
        let border_color = if is_search_match {
            egui::Color32::from_rgb(255, 0, 0)
        } else if is_colliding_drag {
            egui::Color32::from_rgb(255, 140, 0)
        } else if is_selected_edge_node {
            egui::Color32::from_rgb(255, 215, 0)
        } else {
            egui::Color32::from_gray(60)
        };

        let border_width = if is_glow { 2.0 * scale } else { 1.0 * scale };

        ui.painter().rect_stroke(
            node_rect,
            4.0 * scale,
            egui::Stroke::new(border_width, border_color),
            egui::StrokeKind::Middle,
        );

        // Header
        let header_height = header_height_unscaled * scale;
        let header_rect = egui::Rect::from_min_size(
            node_pos_screen,
            egui::vec2(node_rect.width(), header_height),
        );

        // Simplified rounding to avoid compilation error
        // Header ungu untuk tabel yang tidak ada di database.
        let header_fill = if node.detached {
            egui::Color32::from_rgb(72, 52, 100)
        } else {
            egui::Color32::from_rgb(50, 50, 60)
        };
        ui.painter()
            .rect_filled(header_rect, 4.0 * scale, header_fill);
        if node.detached {
            ui.painter().text(
                egui::pos2(header_rect.right() - 6.0 * scale, header_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "not in DB",
                egui::FontId::proportional(9.0 * scale),
                egui::Color32::from_gray(190),
            );
        }

        // Group dots on header
        let mut dot_x = header_rect.left() + 8.0 * scale;
        let mut member_group_names = Vec::new();
        for gid in &node.group_ids {
            if let Some((_, title, color)) = available_groups.iter().find(|(id, _, _)| id == gid) {
                member_group_names.push(title.as_str());
                ui.painter().circle_filled(
                    egui::pos2(dot_x, header_rect.center().y),
                    3.5 * scale,
                    *color,
                );
                ui.painter().circle_stroke(
                    egui::pos2(dot_x, header_rect.center().y),
                    3.5 * scale,
                    egui::Stroke::new(1.0 * scale, egui::Color32::from_black_alpha(120)),
                );
                dot_x += 9.0 * scale;
            }
        }

        if !member_group_names.is_empty() {
            node_response.on_hover_text(format!(
                "Table: {}\nGroups: {}\n(Right-click to manage groups)",
                node.title,
                member_group_names.join(", ")
            ));
        } else {
            node_response.on_hover_text(format!(
                "Table: {}\n(Right-click to add to a group)",
                node.title
            ));
        }

        // Title
        ui.painter().text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            &node.title,
            egui::FontId::proportional(14.0 * scale),
            egui::Color32::WHITE,
        );

        // Columns
        let item_height = item_height_unscaled * scale;
        let mut y_offset = header_height + 4.0 * scale;

        for (col_idx, col) in node.columns.iter().enumerate() {
            // `column_meta` biasanya sejajar dengan `columns`; cari linear hanya bila tidak.
            let info = node
                .column_meta
                .get(col_idx)
                .filter(|m| m.name == *col)
                .or_else(|| node.column_info(col));
            let is_pk = info.is_some_and(|c| c.is_pk);
            let is_fk = node.is_fk_column(col);

            let col_pos_screen = node_pos_screen + egui::vec2(0.0, y_offset);
            let col_rect = egui::Rect::from_min_size(
                col_pos_screen,
                egui::vec2(node_rect.width(), item_height),
            );

            let col_id = ui.id().with("col").with(&node.id).with(col);
            let mut response = ui.interact(col_rect, col_id, egui::Sense::click());

            let is_selected_col = selected_column
                .as_ref()
                .is_some_and(|(t, c)| *t == node.id && c == col);

            // Context menu saat klik kanan pada kolom
            response.context_menu(|ui| {
                ui.label(egui::RichText::new(format!("{}.{}", node.id, col)).strong());
                if let Some(c_type) = info.map(|c| c.type_name.as_str()).filter(|t| !t.is_empty()) {
                    ui.label(egui::RichText::new(format!("Type: {c_type}")).weak().small());
                }
                ui.separator();

                if ui.button("🔍 Search relation").clicked() {
                    ui.close();
                    search_relations_for_column = Some((node.id.clone(), col.clone()));
                }

                ui.separator();
                if is_selected_col {
                    if ui.button("Deselect column").clicked() {
                        ui.close();
                        column_clicked_request = Some((String::new(), String::new()));
                    }
                } else if ui.button("🔗 Select for manual relation (Ctrl+Click)").clicked() {
                    ui.close();
                    column_clicked_request = Some((node.id.clone(), col.clone()));
                }
            });

            // Tooltip interaktif saat ada kolom yang sedang dipilih dari tabel lain
            if let Some((sel_table, sel_col)) = selected_column.as_ref() {
                if *sel_table != node.id {
                    response = response.on_hover_text(format!(
                        "Ctrl+Click to link with {sel_table}.{sel_col}"
                    ));
                }
            }

            let is_link_target_hover = (ctrl_down || shift_down)
                && response.hovered()
                && selected_column
                    .as_ref()
                    .is_some_and(|(t, _)| *t != node.id);

            if response.clicked() {
                let modifier_active = ctrl_down || shift_down;
                match selected_column.as_ref() {
                    // Ada kolom terpilih di tabel lain:
                    Some((sel_table, sel_col)) if *sel_table != node.id => {
                        if modifier_active {
                            // Ctrl+klik kolom tabel kedua -> buat relasi manual!
                            let this_is_pk = is_pk;
                            let sel_is_pk = sel_col_is_pk;

                            let (child_table, child_col, parent_table, parent_col) =
                                if this_is_pk && !sel_is_pk {
                                    (sel_table.clone(), sel_col.clone(), node.id.clone(), col.clone())
                                } else if sel_is_pk && !this_is_pk {
                                    (node.id.clone(), col.clone(), sel_table.clone(), sel_col.clone())
                                } else {
                                    (sel_table.clone(), sel_col.clone(), node.id.clone(), col.clone())
                                };

                            link_request = Some(VirtualRelation {
                                child: child_table,
                                child_column: child_col,
                                parent: parent_table,
                                parent_column: parent_col,
                                origin: RelationOrigin::Manual,
                            });
                        } else {
                            column_clicked_request = Some((node.id.clone(), col.clone()));
                        }
                    }
                    // Kolom pada tabel yang sama:
                    Some((sel_table, sel_col)) if *sel_table == node.id => {
                        if sel_col == col && modifier_active {
                            // Deselect saat Ctrl+klik kolom yang sama
                            column_clicked_request = Some((String::new(), String::new()));
                        } else {
                            column_clicked_request = Some((node.id.clone(), col.clone()));
                        }
                    }
                    _ => column_clicked_request = Some((node.id.clone(), col.clone())),
                }
            }

            let is_col_search_match = state.show_search
                && state.search_columns
                && !state.search_query.is_empty()
                && col.to_lowercase().contains(&state.search_query.to_lowercase());

            if is_selected_col {
                // Highlight jelas kolom sumber terpilih (emas dengan border)
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(255, 215, 0).linear_multiply(0.35),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.5 * scale, egui::Color32::from_rgb(255, 215, 0)),
                    egui::StrokeKind::Inside,
                );
            } else if is_link_target_hover {
                // Highlight kolom target saat di-hover dengan Ctrl (cyan terang)
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(0, 200, 220).linear_multiply(0.25),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.5 * scale, egui::Color32::from_rgb(0, 200, 220)),
                    egui::StrokeKind::Inside,
                );
            } else if is_col_search_match {
                // Highlight kolom pencarian (merah lembut dengan aksen)
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(255, 60, 60).linear_multiply(0.25),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.0 * scale, egui::Color32::from_rgb(255, 80, 80)),
                    egui::StrokeKind::Inside,
                );
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(col_rect, 0.0, egui::Color32::from_white_alpha(10));
            }

            let name_color = if is_col_search_match {
                egui::Color32::WHITE
            } else if is_pk {
                egui::Color32::from_rgb(255, 215, 0)
            } else if is_fk {
                egui::Color32::from_rgb(200, 200, 100)
            } else {
                egui::Color32::LIGHT_GRAY
            };
            ui.painter().text(
                node_pos_screen + egui::vec2(8.0 * scale, y_offset),
                egui::Align2::LEFT_TOP,
                col,
                egui::FontId::monospace(12.0 * scale),
                name_color,
            );

            // Badge kunci + tipe di sisi kanan (redup supaya nama tetap dominan).
            let mut right = String::new();
            if is_pk {
                right.push_str("PK ");
            }
            if is_fk {
                right.push_str("FK ");
            }
            if let Some(ty) = info.map(|c| c.type_name.as_str()).filter(|t| !t.is_empty()) {
                right.extend(ty.chars().take(14));
                if ty.chars().count() > 14 {
                    right.push('…');
                }
            }
            if !right.is_empty() {
                ui.painter().text(
                    egui::pos2(node_rect.right() - 8.0 * scale, col_pos_screen.y),
                    egui::Align2::RIGHT_TOP,
                    right.trim_end(),
                    egui::FontId::monospace(10.0 * scale),
                    egui::Color32::from_gray(130),
                );
            }
            y_offset += item_height;
        }
    }

    if let Some((gid, pos)) = empty_group_retention {
        let has_other_members = state.nodes.iter().any(|n| n.is_in_group(&gid));
        if !has_other_members {
            if let Some(g) = state.groups.iter_mut().find(|g| g.id == gid) {
                if g.manual_pos.is_none() {
                    g.manual_pos = Some(pos);
                }
            }
        }
    }
    if let Some(pos) = add_group_at_pos {
        state.add_group_popup = Some(pos);
        state.new_group_buffer.clear();
    }

    // Clear selection if clicked on background (and not on an edge or node)
    // We check `response` from the beginning of the function (passed down? no it was `ui.interact(rect...)`)
    // We need to check if the main rect was clicked, and ensure no edge/node was clicked.
    if ui.input(|i| i.pointer.primary_clicked())
        && !node_clicked
        && !edge_was_clicked
        && column_clicked_request.is_none()
    {
        // But wait, `ui.interact` for background handles drag. Does it also report click?
        // We can check if the pointer is within the clip rect and nothing else claimed it?
        // Simpler: If the background response was clicked?
        // Accessing `response` from top of function might be hard unless we passed it.
        // Let's rely on global input.
        if ui.rect_contains_pointer(rect) {
            state.selected_edge = None;
            state.selected_column = None;
            state.selected_virtual = None;
        }
    }

    if let Some(req) = column_clicked_request {
        if req.0.is_empty() {
            state.selected_column = None;
        } else {
            state.selected_column = Some(req);
        }
    }
    if let Some(rel) = link_request {
        let label = format!(
            "Linked {}.{} → {}.{}",
            rel.child, rel.child_column, rel.parent, rel.parent_column
        );
        if crate::diagram_relations::add_virtual_relation(state, rel) {
            state.save_requested = true;
            action = Some(DiagramAction::Info(label));
        }
        state.selected_column = None;
    }
    if let Some((table, column)) = search_relations_for_column {
        let suggestions =
            crate::diagram_relations::suggest_relations_for_column(state, &table, &column);
        state.relation_suggestions = Some(suggestions.into_iter().map(|s| (s, true)).collect());
    }
    if let Some(id) = remove_node_request {
        state.nodes.retain(|n| n.id != id);
        state
            .virtual_relations
            .retain(|r| r.child != id && r.parent != id);
        state.selected_virtual = None;
        state.save_requested = true;
    }
    // Delete / Backspace menghapus relasi virtual terpilih (bila tidak sedang mengetik).
    if let Some(idx) = state.selected_virtual
        && ui.memory(|m| m.focused().is_none())
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        remove_virtual(state, idx);
    }

    if let Some(id) = dragging_node_id
        && let Some(node) = state.nodes.iter_mut().find(|n| n.id == id)
    {
        node.pos += drag_delta / scale;
    }

    if let Some(id) = drag_stopped_node_id {
        if state.prevent_overlap {
            resolve_dragged_node_overlap(&mut state.nodes, &id, 20.0);
            state.save_requested = true;
        }
    }

    // Floating Toolbar: Zoom & Navigasi, Grid, Layout, Relasi, Sync, Save, Import & Export.
    let toolbar_id = ui.id().with("diagram_floating_toolbar_width");
    let measured_width: f32 = ui.data(|d| d.get_temp(toolbar_id)).unwrap_or(960.0);
    let toolbar_width = measured_width.max(960.0);
    let toolbar_height = 36.0;
    let toolbar_rect = egui::Rect::from_min_size(
        rect.right_bottom() + egui::vec2(-toolbar_width - 16.0, -toolbar_height - 16.0),
        egui::vec2(toolbar_width, toolbar_height),
    );

    let card_fill = ui.visuals().window_fill;
    let card_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().rect_filled(toolbar_rect, 6.0, card_fill);
    ui.painter()
        .rect_stroke(toolbar_rect, 6.0, card_stroke, egui::StrokeKind::Middle);

    let toolbar_res = ui.scope_builder(
        egui::UiBuilder::new().max_rect(toolbar_rect.shrink(4.0)),
        |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 0.0);

                // --- 1. Zoom & Navigasi ---
                if ui
                    .button(egui_icons::icons::ICON_REMOVE.codepoint)
                    .on_hover_text("Zoom Out (Cmd -)")
                    .clicked()
                {
                    state.zoom = (state.zoom / 1.15).max(MIN_ZOOM);
                }

                let zoom_text = format!("{:.0}%", state.zoom * 100.0);
                if ui
                    .button(egui::RichText::new(zoom_text).monospace().size(11.5))
                    .on_hover_text("Reset Zoom to 100% (Cmd 0)")
                    .clicked()
                {
                    state.zoom = DEFAULT_ZOOM;
                }

                if ui
                    .button(egui_icons::icons::ICON_ADD.codepoint)
                    .on_hover_text("Zoom In (Cmd +)")
                    .clicked()
                {
                    state.zoom = (state.zoom * 1.15).min(MAX_ZOOM);
                }

                if ui
                    .button(format!(
                        "{} Center",
                        egui_icons::icons::ICON_FILTER_CENTER_FOCUS.codepoint
                    ))
                    .on_hover_text("Move diagram to center of view")
                    .clicked()
                {
                    center_diagram(state, rect.size());
                }

                ui.separator();

                // --- 2. Grid & Anti-Overlap Toggles ---
                if ui
                    .selectable_label(
                        state.show_grid,
                        format!("{} Grid", egui_icons::icons::ICON_GRID_ON.codepoint),
                    )
                    .on_hover_text("Show or hide the background grid")
                    .clicked()
                {
                    state.show_grid = !state.show_grid;
                    state.save_requested = true;
                }

                if ui
                    .selectable_label(
                        state.prevent_overlap,
                        format!("{} No Overlap", egui_icons::icons::ICON_DASHBOARD.codepoint),
                    )
                    .on_hover_text("Prevent tables from overlapping (auto-separates on drop and drag)")
                    .clicked()
                {
                    state.prevent_overlap = !state.prevent_overlap;
                    if state.prevent_overlap {
                        resolve_node_overlaps(&mut state.nodes, 20.0);
                    }
                    state.save_requested = true;
                }

                ui.separator();

                // --- 3. Layout Menu ---
                ui.menu_button(
                    format!("{} Layout", egui_icons::icons::ICON_VIEW_MODULE.codepoint),
                    |ui| {
                        if ui
                            .checkbox(&mut state.prevent_overlap, "Prevent table overlap")
                            .on_hover_text("When enabled, tables will not overlap when moved or organized")
                            .clicked()
                        {
                            if state.prevent_overlap {
                                resolve_node_overlaps(&mut state.nodes, 20.0);
                            }
                            state.save_requested = true;
                        }
                        ui.separator();
                        if ui.button("⚡ Auto Arrange All (Smart Layout)").clicked() {
                            ui.close();
                            perform_auto_layout(state);
                            state.save_requested = true;
                        }
                        if ui.button("↔ Resolve Overlaps Now").clicked() {
                            ui.close();
                            resolve_node_overlaps(&mut state.nodes, 20.0);
                            state.save_requested = true;
                        }
                    },
                );

                ui.separator();

                // --- 3. Relations & Database Sync ---
                ui.menu_button(
                    format!("{} Relations", egui_icons::icons::ICON_LINK.codepoint),
                    |ui| {
                        if ui.button("Suggest from similar column names…").clicked() {
                            ui.close();
                            let suggestions = crate::diagram_relations::suggest_relations(state);
                            state.relation_suggestions =
                                Some(suggestions.into_iter().map(|s| (s, true)).collect());
                        }
                        if let Some((sel_table, sel_col)) = &state.selected_column {
                            if ui
                                .button(format!("Search relations for {sel_table}.{sel_col}…"))
                                .clicked()
                            {
                                ui.close();
                                let suggestions =
                                    crate::diagram_relations::suggest_relations_for_column(
                                        state, sel_table, sel_col,
                                    );
                                state.relation_suggestions =
                                    Some(suggestions.into_iter().map(|s| (s, true)).collect());
                            }
                        }
                        let removable = state
                            .virtual_relations
                            .iter()
                            .filter(|r| r.origin != RelationOrigin::Imported)
                            .count();
                        if ui
                            .add_enabled(
                                removable > 0,
                                egui::Button::new(format!(
                                    "Remove suggested & manual relations ({removable})"
                                )),
                            )
                            .clicked()
                        {
                            ui.close();
                            state
                                .virtual_relations
                                .retain(|r| r.origin == RelationOrigin::Imported);
                            state.selected_virtual = None;
                            state.save_requested = true;
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "Manual link: Ctrl+click a column, then Ctrl+click\nthe target column in another table.\nRight-click a column for automatic search.\nSelect a dashed line and press Delete to remove it.",
                            )
                            .weak()
                            .small(),
                        );
                    },
                );

                ui.menu_button(
                    format!("{} Sync", egui_icons::icons::ICON_SYNC.codepoint),
                    |ui| {
                        if ui.button("Save to Database (diagram_by_tabular)").clicked() {
                            ui.close();
                            action = Some(DiagramAction::SaveToDatabase);
                        }
                        if ui.button("Load from Database (diagram_by_tabular)").clicked() {
                            ui.close();
                            action = Some(DiagramAction::LoadFromDatabase);
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "Saves custom groups, virtual relations, and node layout\ninto table `diagram_by_tabular` in this database\nfor team & multi-device sync.",
                            )
                            .weak()
                            .small(),
                        );
                    },
                );

                ui.separator();

                // --- 4. File / Persistence (Save, Import, Export) ---
                if ui
                    .button(format!("{} Save", egui_icons::icons::ICON_SAVE.codepoint))
                    .on_hover_text(
                        "Save diagram layout (Cmd S) - default saves to Obsidian vault if enabled",
                    )
                    .clicked()
                {
                    state.save_requested = true;
                    action = Some(DiagramAction::Save);
                }

                ui.menu_button(
                    format!("{} Import", egui_icons::icons::ICON_UPLOAD.codepoint),
                    |ui| {
                        if ui.button("Diagram layout (JSON)…").clicked() {
                            ui.close();
                            action = import_json(state);
                        }
                        if ui.button("Mermaid erDiagram (.mmd / .md)…").clicked() {
                            ui.close();
                            action = import_mermaid(state);
                        }
                    },
                );

                ui.menu_button(
                    format!("{} Export", egui_icons::icons::ICON_DOWNLOAD.codepoint),
                    |ui| {
                        if ui.button("Diagram layout (JSON)…").clicked() {
                            ui.close();
                            action = export_json(state);
                        }
                        if ui.button("Mermaid erDiagram (.mmd / .md)…").clicked() {
                            ui.close();
                            action = export_mermaid(state);
                        }
                        if ui.button("Copy Mermaid to clipboard").clicked() {
                            ui.close();
                            let text = crate::diagram_mermaid::ErModel::from_diagram(state)
                                .to_mermaid(Default::default());
                            ui.ctx().copy_text(text);
                            action = Some(DiagramAction::Info(
                                "Mermaid copied to clipboard".to_string(),
                            ));
                        }
                    },
                );
            });
        },
    );

    let actual_content_width = toolbar_res.response.rect.width() + 20.0;
    if (actual_content_width - measured_width).abs() > 4.0 {
        ui.data_mut(|d| d.insert_temp(toolbar_id, actual_content_width));
    }

    // Render Search Box
    if state.show_search {
        // Two-row card layout: search field + close button on row 1, filter checkboxes on row 2
        let search_rect =
            egui::Rect::from_min_size(rect.min + egui::vec2(20.0, 20.0), egui::vec2(295.0, 70.0));

        let card_fill = ui.visuals().window_fill;
        let card_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter().rect_filled(search_rect, 6.0, card_fill);
        ui.painter()
            .rect_stroke(search_rect, 6.0, card_stroke, egui::StrokeKind::Middle);

        ui.scope_builder(
            egui::UiBuilder::new().max_rect(search_rect.shrink(6.0)),
            |ui| {
                ui.vertical(|ui| {
                    let mut search_changed = false;

                    // Baris 1: Field pencarian + tombol tutup
                    ui.horizontal(|ui| {
                        let response = crate::window_egui::style::render_search_field(
                            ui,
                            &mut state.search_query,
                            "Search diagram…",
                            240.0,
                        );

                        // Auto-focus if empty (just opened or cleared)
                        if state.search_query.is_empty() && !response.has_focus() {
                            response.request_focus();
                        }

                        if response.changed() {
                            search_changed = true;
                        }

                        if ui.button("X").clicked() {
                            state.show_search = false;
                            state.search_query.clear();
                        }
                    });

                    ui.add_space(2.0);

                    // Baris 2: Checkbox filter (Table, Column, Group)
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let cb_tbl = ui
                            .checkbox(&mut state.search_tables, "Table")
                            .on_hover_text("Search table names");
                        let cb_col = ui
                            .checkbox(&mut state.search_columns, "Column")
                            .on_hover_text("Search column names");
                        let cb_grp = ui
                            .checkbox(&mut state.search_groups, "Group")
                            .on_hover_text("Search group container names");

                        if cb_tbl.changed() || cb_col.changed() || cb_grp.changed() {
                            search_changed = true;
                        }
                    });

                    if search_changed {
                        let query = crate::search_match::SearchQuery::new(&state.search_query);
                        if !query.is_empty() {
                            // Hitung skor terbaik untuk node tabel / kolom
                            let mut best_node: Option<(f32, egui::Pos2)> = None;
                            if state.search_tables || state.search_columns {
                                for node in &state.nodes {
                                    let score = match (state.search_tables, state.search_columns) {
                                        (true, true) => query.best_score(
                                            std::iter::once(node.title.as_str())
                                                .chain(node.columns.iter().map(String::as_str)),
                                        ),
                                        (true, false) => query.score(&node.title),
                                        (false, true) => {
                                            query.best_score(node.columns.iter().map(String::as_str))
                                        }
                                        (false, false) => None,
                                    };
                                    if let Some(score) = score
                                        && best_node.is_none_or(|(best_score, _)| score > best_score)
                                    {
                                        let node_center = node.pos + node.size / 2.0;
                                        best_node = Some((score, node_center));
                                    }
                                }
                            }

                            // Hitung skor terbaik untuk group container
                            let mut best_group: Option<(f32, egui::Pos2)> = None;
                            if state.search_groups {
                                for group in &state.groups {
                                    if let Some(score) = query.score(&group.title) {
                                        if best_group.is_none_or(|(best_score, _)| score > best_score) {
                                            let group_nodes: Vec<&DiagramNode> = state
                                                .nodes
                                                .iter()
                                                .filter(|n| n.is_in_group(&group.id))
                                                .collect();

                                            let group_center = if !group_nodes.is_empty() {
                                                let mut min_pos = group_nodes[0].pos;
                                                let mut max_pos =
                                                    group_nodes[0].pos + group_nodes[0].size;
                                                for n in &group_nodes {
                                                    min_pos = min_pos.min(n.pos);
                                                    max_pos = max_pos.max(n.pos + n.size);
                                                }
                                                min_pos + (max_pos - min_pos) / 2.0
                                            } else if let Some(pos) = group.manual_pos {
                                                pos + egui::vec2(200.0, 150.0)
                                            } else {
                                                egui::Pos2::ZERO
                                            };

                                            best_group = Some((score, group_center));
                                        }
                                    }
                                }
                            }

                            // Pilih kecocokan dengan skor tertinggi antara node atau group
                            let best_match = match (best_node, best_group) {
                                (Some(n), Some(g)) => {
                                    if g.0 > n.0 {
                                        Some(g.1)
                                    } else {
                                        Some(n.1)
                                    }
                                }
                                (Some(n), None) => Some(n.1),
                                (None, Some(g)) => Some(g.1),
                                (None, None) => None,
                            };

                            let target_pan = best_match.map(|center| {
                                let view_center = rect.size() / 2.0;
                                view_center - center.to_vec2() * state.zoom
                            });

                            if let Some(pan) = target_pan {
                                state.pan = pan;
                                state.is_centered = true; // Ensure we don't auto-center back
                            }
                        }
                    }
                });
            },
        );

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.show_search = false;
        }
    }

    // Render "Add Group" Popup
    if let Some(pos) = state.add_group_popup {
        let mut open = true;
        let window_pos = to_screen(pos);

        egui::Window::new("New Group")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_pos(window_pos)
            .show(ui.ctx(), |ui| {
                ui.label("Enter group name:");
                let text_res = ui.text_edit_singleline(&mut state.new_group_buffer);
                if text_res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    // Trigger save
                } else {
                    text_res.request_focus();
                }

                ui.horizontal(|ui| {
                    if ui.button("Save").clicked()
                        || (ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && !state.new_group_buffer.is_empty())
                    {
                        let timestamp = chrono::Utc::now().to_rfc3339();
                        let digest = md5::compute(timestamp);
                        let group_id = format!("{:x}", digest);
                        let color = egui::Color32::from_rgb(100, 149, 237); // Default Blue

                        let new_group = crate::models::structs::DiagramGroup {
                            id: group_id,
                            title: state.new_group_buffer.clone(),
                            color,
                            manual_pos: Some(pos),
                        };

                        state.groups.push(new_group);
                        state.add_group_popup = None;
                        state.new_group_buffer.clear();
                    }
                    if ui.button("Cancel").clicked() {
                        state.add_group_popup = None;
                    }
                });
            });

        if !open {
            state.add_group_popup = None;
        }
    }

    if let Some(a) = render_relation_suggestions(ui.ctx(), state) {
        action = Some(a);
    }

    action
}

/// Grid latar mengikuti pan & zoom; tiap garis ke-5 lebih tegas.
fn draw_grid(ui: &egui::Ui, rect: egui::Rect, pan: egui::Vec2, scale: f32) {
    let spacing = 40.0 * scale;
    if spacing < 6.0 {
        return;
    }
    let base = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let minor = egui::Stroke::new(1.0, base.linear_multiply(0.25));
    let major = egui::Stroke::new(1.0, base.linear_multiply(0.55));
    let origin = rect.min + pan;
    let painter = ui.painter();

    let first = ((rect.left() - origin.x) / spacing).floor() as i64;
    let last = ((rect.right() - origin.x) / spacing).ceil() as i64;
    for k in first..=last {
        let x = origin.x + k as f32 * spacing;
        let stroke = if k % 5 == 0 { major } else { minor };
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            stroke,
        );
    }
    let first = ((rect.top() - origin.y) / spacing).floor() as i64;
    let last = ((rect.bottom() - origin.y) / spacing).ceil() as i64;
    for k in first..=last {
        let y = origin.y + k as f32 * spacing;
        let stroke = if k % 5 == 0 { major } else { minor };
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            stroke,
        );
    }
}

/// Titik tengah vertikal baris kolom (koordinat diagram), mengikuti layout
/// node di `render_diagram`: header 24, padding 4, tinggi baris 16.
fn column_anchor_y(node: &DiagramNode, column: &str) -> f32 {
    match node.columns.iter().position(|c| c == column) {
        Some(i) => node.pos.y + 24.0 + 4.0 + i as f32 * 16.0 + 8.0,
        None => node.pos.y + node.size.y / 2.0,
    }
}

fn remove_virtual(state: &mut DiagramState, idx: usize) {
    if idx < state.virtual_relations.len() {
        state.virtual_relations.remove(idx);
        state.save_requested = true;
    }
    state.selected_virtual = None;
}

/// Gambar relasi virtual sebagai garis putus-putus dari baris kolom child ke
/// baris kolom parent. Mengembalikan `true` bila salah satunya diklik.
fn draw_virtual_relations(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    rect: egui::Rect,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    pointer_down: bool,
) -> bool {
    let scale = state.zoom;
    let hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p));
    // Klik di atas node milik node, bukan garis di bawahnya.
    let over_node = hover.is_some_and(|p| {
        state
            .nodes
            .iter()
            .any(|n| egui::Rect::from_min_size(to_screen(n.pos), n.size * scale).contains(p))
    });

    let mut clicked: Option<usize> = None;
    let mut remove: Option<usize> = None;
    for (idx, rel) in state.virtual_relations.iter().enumerate() {
        let (Some(child), Some(parent)) = (
            state.nodes.iter().find(|n| n.id == rel.child),
            state.nodes.iter().find(|n| n.id == rel.parent),
        ) else {
            continue;
        };
        // Keluar dari sisi yang menghadap tabel tujuan.
        let parent_is_right =
            parent.pos.x + parent.size.x / 2.0 >= child.pos.x + child.size.x / 2.0;
        let (cx, px, dir) = if parent_is_right {
            (child.pos.x + child.size.x, parent.pos.x, 1.0)
        } else {
            (child.pos.x, parent.pos.x + parent.size.x, -1.0)
        };
        let start = to_screen(egui::pos2(cx, column_anchor_y(child, &rel.child_column)));
        let end = to_screen(egui::pos2(px, column_anchor_y(parent, &rel.parent_column)));
        let bend = (end.x - start.x).abs().max(60.0 * scale) * 0.5;
        let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
            [
                start,
                start + egui::vec2(bend * dir, 0.0),
                end - egui::vec2(bend * dir, 0.0),
                end,
            ],
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::NONE,
        );
        let points: Vec<egui::Pos2> = (0..=24).map(|i| bezier.sample(i as f32 / 24.0)).collect();

        let hovered = !over_node
            && hover.is_some_and(|p| {
                egui::Rect::from_points(&points).expand(8.0).contains(p)
                    && points.iter().any(|q| q.distance(p) < 8.0)
            });
        if hovered && pointer_down {
            clicked = Some(idx);
        }
        let selected = state.selected_virtual == Some(idx);
        let base = match rel.origin {
            RelationOrigin::Imported => egui::Color32::from_rgb(147, 112, 219),
            RelationOrigin::Inferred | RelationOrigin::Manual => {
                egui::Color32::from_rgb(0, 190, 200)
            }
        };
        let (color, width) = if selected {
            (egui::Color32::from_rgb(255, 215, 0), 2.5)
        } else if hovered {
            (base, 2.5)
        } else {
            (base.linear_multiply(0.85), 1.5)
        };
        ui.painter().extend(egui::Shape::dashed_line(
            &points,
            egui::Stroke::new(width * scale.max(0.5), color),
            6.0 * scale,
            4.0 * scale,
        ));
        ui.painter().circle_filled(end, 3.0 * scale, color);

        if selected || hovered {
            let mid = bezier.sample(0.5);
            let origin = match rel.origin {
                RelationOrigin::Inferred => "suggested",
                RelationOrigin::Manual => "manual",
                RelationOrigin::Imported => "imported",
            };
            ui.painter().text(
                mid - egui::vec2(0.0, 10.0),
                egui::Align2::CENTER_BOTTOM,
                format!("{} → {} ({origin})", rel.child_column, rel.parent_column),
                egui::FontId::proportional(11.0),
                color,
            );
            if selected {
                let btn = egui::Rect::from_center_size(
                    mid + egui::vec2(0.0, 12.0),
                    egui::vec2(64.0, 20.0),
                );
                if ui.put(btn, egui::Button::new("Remove").small()).clicked() {
                    remove = Some(idx);
                }
            }
        }
    }

    if let Some(idx) = remove {
        remove_virtual(state, idx);
        return true;
    }
    if let Some(idx) = clicked {
        state.selected_virtual = Some(idx);
        state.selected_edge = None;
        return true;
    }
    false
}

/// Jendela daftar saran relasi; user mencentang lalu menambahkan.
fn render_relation_suggestions(
    ctx: &egui::Context,
    state: &mut DiagramState,
) -> Option<DiagramAction> {
    let mut suggestions = state.relation_suggestions.take()?;
    let mut open = true;
    let mut close = false;
    let mut result = None;

    egui::Window::new("Suggested relations")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .show(ctx, |ui| {
            if suggestions.is_empty() {
                ui.label("No relations found. Columns such as `customer_id`, `id_customer` or a column that matches another table's primary key are detected automatically.");
                if ui.button("Close").clicked() {
                    close = true;
                }
                return;
            }
            ui.label(
                egui::RichText::new(
                    "Based on column names and types. Accepted relations are saved with the diagram and shown as dashed lines.",
                )
                .weak(),
            );

            // Filter pencarian nama kolom atau tabel
            let mut filter_text: String = ctx.data_mut(|d| {
                d.get_temp(egui::Id::new("rel_suggest_filter")).unwrap_or_default()
            });
            ui.horizontal(|ui| {
                ui.label("🔍 Filter:");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut filter_text)
                        .hint_text("Filter by table or column name..."),
                );
                if edit.changed() {
                    ctx.data_mut(|d| {
                        d.insert_temp(egui::Id::new("rel_suggest_filter"), filter_text.clone())
                    });
                }
                if !filter_text.is_empty() && ui.small_button("✖").clicked() {
                    filter_text.clear();
                    ctx.data_mut(|d| {
                        d.insert_temp(egui::Id::new("rel_suggest_filter"), String::new())
                    });
                }
            });

            let filter_lower = filter_text.trim().to_lowercase();

            ui.horizontal(|ui| {
                if ui.small_button("Select all").clicked() {
                    for (s, on) in suggestions.iter_mut() {
                        if filter_lower.is_empty()
                            || s.relation.child.to_lowercase().contains(&filter_lower)
                            || s.relation.child_column.to_lowercase().contains(&filter_lower)
                            || s.relation.parent.to_lowercase().contains(&filter_lower)
                            || s.relation.parent_column.to_lowercase().contains(&filter_lower)
                            || s.reason.to_lowercase().contains(&filter_lower)
                        {
                            *on = true;
                        }
                    }
                }
                if ui.small_button("Select none").clicked() {
                    for (s, on) in suggestions.iter_mut() {
                        if filter_lower.is_empty()
                            || s.relation.child.to_lowercase().contains(&filter_lower)
                            || s.relation.child_column.to_lowercase().contains(&filter_lower)
                            || s.relation.parent.to_lowercase().contains(&filter_lower)
                            || s.relation.parent_column.to_lowercase().contains(&filter_lower)
                            || s.reason.to_lowercase().contains(&filter_lower)
                        {
                            *on = false;
                        }
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                let mut displayed_count = 0;
                for (s, on) in suggestions.iter_mut() {
                    let r = &s.relation;
                    if !filter_lower.is_empty()
                        && !r.child.to_lowercase().contains(&filter_lower)
                        && !r.child_column.to_lowercase().contains(&filter_lower)
                        && !r.parent.to_lowercase().contains(&filter_lower)
                        && !r.parent_column.to_lowercase().contains(&filter_lower)
                        && !s.reason.to_lowercase().contains(&filter_lower)
                    {
                        continue;
                    }
                    displayed_count += 1;
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            on,
                            format!("{}.{} → {}.{}", r.child, r.child_column, r.parent, r.parent_column),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:.0}% · {}", s.score * 100.0, s.reason))
                                .weak()
                                .small(),
                        );
                    });
                }
                if displayed_count == 0 && !filter_lower.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("No suggestions matching \"{filter_text}\""))
                            .italics()
                            .weak(),
                    );
                }
            });
            ui.separator();
            let chosen = suggestions.iter().filter(|(_, on)| *on).count();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(chosen > 0, egui::Button::new(format!("Add {chosen} relation(s)")))
                    .clicked()
                {
                    let mut added = 0;
                    for (s, on) in &suggestions {
                        if *on && crate::diagram_relations::add_virtual_relation(state, s.relation.clone()) {
                            added += 1;
                        }
                    }
                    state.save_requested = true;
                    result = Some(DiagramAction::Info(format!("Added {added} relation(s)")));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if open && !close {
        state.relation_suggestions = Some(suggestions);
    }
    result
}

/// Cek apakah ada pasangan tabel yang saling tumpang tindih dalam batas padding.
pub fn check_nodes_overlap(nodes: &[DiagramNode], padding: f32) -> bool {
    let half_pad = padding.max(0.0) / 2.0;
    for (i, node) in nodes.iter().enumerate() {
        let rect_i = egui::Rect::from_min_size(node.pos, node.size).expand(half_pad);
        for other_node in &nodes[(i + 1)..] {
            let rect_j =
                egui::Rect::from_min_size(other_node.pos, other_node.size).expand(half_pad);
            let inter = rect_i.intersect(rect_j);
            if inter.width() > 0.0 && inter.height() > 0.0 {
                return true;
            }
        }
    }
    false
}

/// Cek apakah tabel spesifik saat ini bertabrakan dengan tabel lain dalam diagram.
pub fn check_single_node_collision(nodes: &[DiagramNode], node_id: &str, padding: f32) -> bool {
    let Some(target) = nodes.iter().find(|n| n.id == node_id) else {
        return false;
    };
    let half_pad = padding.max(0.0) / 2.0;
    let target_rect = egui::Rect::from_min_size(target.pos, target.size).expand(half_pad);
    for other in nodes.iter().filter(|n| n.id != node_id) {
        let other_rect = egui::Rect::from_min_size(other.pos, other.size).expand(half_pad);
        let inter = target_rect.intersect(other_rect);
        if inter.width() > 0.0 && inter.height() > 0.0 {
            return true;
        }
    }
    false
}

/// Pisahkan semua tabel yang bertumpukan secara iteratif menggunakan AABB collision resolution.
/// Menjamin tidak ada dua tabel yang tumpang tindih dengan jarak minimal `padding`.
pub fn resolve_node_overlaps(nodes: &mut [DiagramNode], padding: f32) {
    let node_count = nodes.len();
    if node_count < 2 {
        return;
    }

    let half_pad = padding.max(0.0) / 2.0;
    let max_iterations = 40;

    for _ in 0..max_iterations {
        let mut any_collision = false;

        for i in 0..node_count {
            for j in (i + 1)..node_count {
                let rect_i =
                    egui::Rect::from_min_size(nodes[i].pos, nodes[i].size).expand(half_pad);
                let rect_j =
                    egui::Rect::from_min_size(nodes[j].pos, nodes[j].size).expand(half_pad);

                let inter = rect_i.intersect(rect_j);
                if inter.width() > 0.0 && inter.height() > 0.0 {
                    any_collision = true;
                    let overlap_w = inter.width();
                    let overlap_h = inter.height();

                    // Dorong pada sumbu irisan terkecil agar pergeseran seminimal mungkin
                    let push = if overlap_w < overlap_h {
                        let dir = if rect_i.center().x <= rect_j.center().x {
                            -1.0
                        } else {
                            1.0
                        };
                        egui::vec2(dir * (overlap_w / 2.0 + 1.0), 0.0)
                    } else {
                        let dir = if rect_i.center().y <= rect_j.center().y {
                            -1.0
                        } else {
                            1.0
                        };
                        egui::vec2(0.0, dir * (overlap_h / 2.0 + 1.0))
                    };

                    nodes[i].pos += push;
                    nodes[j].pos -= push;
                }
            }
        }

        if !any_collision {
            break;
        }
    }
}

/// Pisahkan tabel yang baru selesai digeser agar tidak tumpang tindih dengan tabel lain.
/// Memprioritaskan posisi tabel lain tetap stabil di tempatnya.
pub fn resolve_dragged_node_overlap(nodes: &mut [DiagramNode], dragged_id: &str, padding: f32) {
    let half_pad = padding.max(0.0) / 2.0;
    let max_single_passes = 25;

    let Some(dragged_idx) = nodes.iter().position(|n| n.id == dragged_id) else {
        return;
    };

    let mut still_colliding = false;
    for _ in 0..max_single_passes {
        let dragged_rect = egui::Rect::from_min_size(
            nodes[dragged_idx].pos,
            nodes[dragged_idx].size,
        )
        .expand(half_pad);

        // Cari rintangan terdekat yang bertabrakan
        let mut min_push: Option<egui::Vec2> = None;
        let mut min_dist_sq = f32::MAX;

        for (j, other) in nodes.iter().enumerate() {
            if j == dragged_idx {
                continue;
            }
            let other_rect = egui::Rect::from_min_size(other.pos, other.size).expand(half_pad);
            let inter = dragged_rect.intersect(other_rect);
            if inter.width() > 0.0 && inter.height() > 0.0 {
                let overlap_w = inter.width();
                let overlap_h = inter.height();

                // Hitung vektor dorong untuk mengeluarkan dragged_node dari obstacle
                let (dir_x, dist_x) = if dragged_rect.center().x <= other_rect.center().x {
                    (-1.0, overlap_w + 1.0)
                } else {
                    (1.0, overlap_w + 1.0)
                };
                let (dir_y, dist_y) = if dragged_rect.center().y <= other_rect.center().y {
                    (-1.0, overlap_h + 1.0)
                } else {
                    (1.0, overlap_h + 1.0)
                };

                let push = if dist_x < dist_y {
                    egui::vec2(dir_x * dist_x, 0.0)
                } else {
                    egui::vec2(0.0, dir_y * dist_y)
                };

                let dist_sq = push.length_sq();
                if dist_sq < min_dist_sq {
                    min_dist_sq = dist_sq;
                    min_push = Some(push);
                }
            }
        }

        if let Some(push) = min_push {
            nodes[dragged_idx].pos += push;
            still_colliding = true;
        } else {
            still_colliding = false;
            break;
        }
    }

    // Jika ruang sangat sempit dan dragged_node masih terjepit di antara beberapa tabel,
    // jalankan relaksasi global untuk memberi ruang.
    if still_colliding {
        resolve_node_overlaps(nodes, padding);
    }
}

pub fn perform_auto_layout(state: &mut DiagramState) {
    let iterations = 1000; // Increased iterations for better convergence
    let repulsion_force = 800_000.0; // Stronger base repulsion
    let spring_length = 400.0; // Longer edges
    let attraction_constant = 0.04;
    let center_gravity = 0.01; // Weaker gravity to allow expansion
    let prefix_attraction = 0.05; // Reduced prefix attraction to prevent clumping
    let delta_time = 0.1;

    let node_count = state.nodes.len();
    if node_count == 0 {
        return;
    }

    // Helper to get prefix (e.g., "user" from "user_data")
    let get_prefix = |name: &str| -> String { name.split('_').next().unwrap_or(name).to_string() };

    // Pre-calculate prefixes
    let prefixes: Vec<String> = state.nodes.iter().map(|n| get_prefix(&n.id)).collect();

    for _ in 0..iterations {
        let mut forces = vec![egui::Vec2::ZERO; node_count];

        // 1. Repulsion (between every pair)
        for i in 0..node_count {
            for j in (i + 1)..node_count {
                // Calculate center-to-center distance
                let center_i = state.nodes[i].pos + state.nodes[i].size / 2.0;
                let center_j = state.nodes[j].pos + state.nodes[j].size / 2.0;
                let diff = center_i - center_j;
                let mut dist = diff.length();
                if dist < 1.0 {
                    dist = 1.0;
                } // Avoid zero division

                let mut force_scalar = repulsion_force / (dist * dist);

                // Boost repulsion if prefixes are different
                if prefixes[i] != prefixes[j] {
                    force_scalar *= 5.0; // Stronger group separation
                }

                // COLLISION AVOIDANCE
                // Use actual bounding boxes + margin
                let size_i = state.nodes[i].size;
                let size_j = state.nodes[j].size;

                // Effective radius for fast check
                let r_i = size_i.length() / 2.0;
                let r_j = size_j.length() / 2.0;
                let min_dist_circle = r_i + r_j + 100.0; // generous margin

                if dist < min_dist_circle {
                    // Check for actual Box Overlap for stronger push
                    let delta = diff.abs();
                    let combined_half_size = (size_i + size_j) / 2.0 + egui::vec2(50.0, 50.0); // 50px padding

                    if delta.x < combined_half_size.x && delta.y < combined_half_size.y {
                        // Overlap detected! Explosive force.
                        force_scalar += 2_000_000.0;
                    } else {
                        // Near miss, gentle push
                        force_scalar += 100_000.0 * (min_dist_circle - dist) / min_dist_circle;
                    }
                }

                let force_dir = diff / dist;
                let force = force_dir * force_scalar;

                forces[i] += force;
                forces[j] -= force;
            }
        }

        // 2. Attraction (Edges / Foreign Keys)
        for edge in &state.edges {
            if let Some(src_idx) = state.nodes.iter().position(|n| n.id == edge.source)
                && let Some(dst_idx) = state.nodes.iter().position(|n| n.id == edge.target)
            {
                let diff = state.nodes[src_idx].pos - state.nodes[dst_idx].pos;
                let dist = diff.length();

                if dist > 0.0 {
                    let force_scalar = (dist - spring_length) * attraction_constant;
                    let force_dir = diff / dist;
                    let force = force_dir * force_scalar;

                    forces[src_idx] -= force;
                    forces[dst_idx] += force;
                }
            }
        }

        // 3. Prefix Attraction (Group by name similarity)
        for i in 0..node_count {
            for j in (i + 1)..node_count {
                if prefixes[i] == prefixes[j] {
                    let diff = state.nodes[i].pos - state.nodes[j].pos;
                    let dist = diff.length();
                    if dist > 0.0 {
                        let force_scalar = (dist - (spring_length * 0.8)) * prefix_attraction; // Check if effective
                        let force_dir = diff / dist;
                        let force = force_dir * force_scalar;

                        forces[i] -= force;
                        forces[j] += force;
                    }
                }
            }
        }

        // Catatan: Group boleh tumpang tindih (groups are allowed to overlap),
        // sehingga tidak ada tolakan paksa antar group bounds di sini.

        // 4. Center Gravity (Pull to 0,0) + Apply Forces
        for (node, force) in state.nodes.iter_mut().zip(forces.iter_mut()) {
            if state.dragging_node.as_deref() == Some(&node.id) {
                continue;
            } // Don't move dragged node

            // Weaker center pull
            let center_pull = egui::Vec2::ZERO - node.pos.to_vec2();
            *force += center_pull * center_gravity;

            // Limit max force to prevent explosion
            let max_force = 1000.0;
            if force.length() > max_force {
                *force = force.normalized() * max_force;
            }

            node.pos += *force * delta_time;
        }
    }

    // STRICT COLLISION RESOLUTION (Post-Process)
    // Pastikan semua tabel terpisah sempurna dengan padding aman
    resolve_node_overlaps(&mut state.nodes, 20.0);

    // Normalize coordinates to be positive and start at somewhat reasonable position
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    for node in &state.nodes {
        if node.pos.x < min_x {
            min_x = node.pos.x;
        }
        if node.pos.y < min_y {
            min_y = node.pos.y;
        }
    }

    for node in &mut state.nodes {
        node.pos.x -= min_x - 50.0;
        node.pos.y -= min_y - 50.0;
    }
}

// ============================================================================
// VISUAL EXPLAIN PLAN VIEWER (PostgreSQL / MySQL / Text fallback)
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExplainPlanNode {
    pub node_type: String,
    pub relation_name: Option<String>,
    pub index_name: Option<String>,
    pub alias: Option<String>,
    pub startup_cost: f64,
    pub total_cost: f64,
    pub plan_rows: u64,
    pub plan_width: u64,
    pub actual_startup_time: Option<f64>,
    pub actual_total_time: Option<f64>,
    pub actual_rows: Option<u64>,
    pub actual_loops: Option<u64>,
    pub children: Vec<ExplainPlanNode>,
}

impl ExplainPlanNode {
    pub fn parse(raw_plan: &str) -> Option<Self> {
        let trimmed = raw_plan.trim();
        if (trimmed.starts_with('[') || trimmed.starts_with('{'))
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(node) = Self::parse_json_value(&v)
        {
            return Some(node);
        }

        // Fallback: parse plain text EXPLAIN output lines
        Self::parse_text_lines(trimmed)
    }

    fn parse_json_value(v: &serde_json::Value) -> Option<Self> {
        if let Some(arr) = v.as_array()
            && let Some(first) = arr.first()
        {
            return Self::parse_json_value(first);
        }
        if let Some(obj) = v.as_object() {
            if let Some(plan) = obj.get("Plan") {
                return Self::parse_pg_node(plan);
            }
            if let Some(qb) = obj.get("query_block") {
                return Self::parse_mysql_qb(qb);
            }
            if obj.contains_key("Node Type") {
                return Self::parse_pg_node(v);
            }
        }
        None
    }

    fn parse_pg_node(v: &serde_json::Value) -> Option<Self> {
        let node_type = v.get("Node Type")?.as_str()?.to_string();
        let relation_name = v
            .get("Relation Name")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let index_name = v
            .get("Index Name")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let alias = v
            .get("Alias")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        let startup_cost = v
            .get("Startup Cost")
            .and_then(|n| n.as_f64())
            .unwrap_or(0.0);
        let total_cost = v.get("Total Cost").and_then(|n| n.as_f64()).unwrap_or(0.0);
        let plan_rows = v.get("Plan Rows").and_then(|n| n.as_u64()).unwrap_or(0);
        let plan_width = v.get("Plan Width").and_then(|n| n.as_u64()).unwrap_or(0);

        let actual_startup_time = v.get("Actual Startup Time").and_then(|n| n.as_f64());
        let actual_total_time = v.get("Actual Total Time").and_then(|n| n.as_f64());
        let actual_rows = v.get("Actual Rows").and_then(|n| n.as_u64());
        let actual_loops = v.get("Actual Loops").and_then(|n| n.as_u64());

        let mut children = Vec::new();
        if let Some(plans) = v.get("Plans").and_then(|p| p.as_array()) {
            for child_val in plans {
                if let Some(child_node) = Self::parse_pg_node(child_val) {
                    children.push(child_node);
                }
            }
        }

        Some(ExplainPlanNode {
            node_type,
            relation_name,
            index_name,
            alias,
            startup_cost,
            total_cost,
            plan_rows,
            plan_width,
            actual_startup_time,
            actual_total_time,
            actual_rows,
            actual_loops,
            children,
        })
    }

    fn parse_mysql_qb(v: &serde_json::Value) -> Option<Self> {
        let mut children = Vec::new();
        let mut node_type = "Query Block".to_string();
        let mut total_cost = 0.0;

        if let Some(cost) = v
            .get("cost_info")
            .and_then(|c| c.get("query_cost"))
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<f64>().ok())
        {
            total_cost = cost;
        }

        if let Some(nl) = v.get("nested_loop").and_then(|n| n.as_array()) {
            node_type = "Nested Loop Join".to_string();
            for item in nl {
                if let Some(t) = item.get("table")
                    && let Some(cn) = Self::parse_mysql_table(t)
                {
                    children.push(cn);
                }
            }
        } else if let Some(t) = v.get("table") {
            return Self::parse_mysql_table(t);
        }

        Some(ExplainPlanNode {
            node_type,
            relation_name: None,
            index_name: None,
            alias: None,
            startup_cost: 0.0,
            total_cost,
            plan_rows: 0,
            plan_width: 0,
            actual_startup_time: None,
            actual_total_time: None,
            actual_rows: None,
            actual_loops: None,
            children,
        })
    }

    fn parse_mysql_table(v: &serde_json::Value) -> Option<Self> {
        let table_name = v
            .get("table_name")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let access_type = v
            .get("access_type")
            .and_then(|s| s.as_str())
            .unwrap_or("ALL");
        let node_type = match access_type {
            "ALL" => "Seq Scan (Full Table Scan)".to_string(),
            "ref" | "eq_ref" | "const" => "Index Scan".to_string(),
            "range" => "Index Range Scan".to_string(),
            other => format!("{} Scan", other),
        };
        let rows = v
            .get("rows_examined_per_scan")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let cost = v
            .get("cost_info")
            .and_then(|c| c.get("prefix_cost"))
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let key = v.get("key").and_then(|s| s.as_str()).map(|s| s.to_string());

        Some(ExplainPlanNode {
            node_type,
            relation_name: table_name,
            index_name: key,
            alias: None,
            startup_cost: 0.0,
            total_cost: cost,
            plan_rows: rows,
            plan_width: 0,
            actual_startup_time: None,
            actual_total_time: None,
            actual_rows: None,
            actual_loops: None,
            children: Vec::new(),
        })
    }

    fn parse_text_lines(text: &str) -> Option<Self> {
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return None;
        }
        let first_line = lines[0];
        Some(ExplainPlanNode {
            node_type: first_line.to_string(),
            relation_name: None,
            index_name: None,
            alias: None,
            startup_cost: 0.0,
            total_cost: 1.0,
            plan_rows: 1,
            plan_width: 0,
            actual_startup_time: None,
            actual_total_time: None,
            actual_rows: None,
            actual_loops: None,
            children: Vec::new(),
        })
    }

    pub fn max_cost(&self) -> f64 {
        let mut max_c = self.total_cost;
        for child in &self.children {
            max_c = max_c.max(child.max_cost());
        }
        max_c
    }

    pub fn max_duration(&self) -> f64 {
        let mut max_d = self.actual_total_time.unwrap_or(0.0);
        for child in &self.children {
            max_d = max_d.max(child.max_duration());
        }
        max_d
    }
}

pub fn render_explain_plan_viewer(ui: &mut egui::Ui, raw_plan: &str) {
    crate::query_profiler::render_query_profiler(ui, raw_plan);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_postgres_explain_json() {
        let pg_json = r#"[
          {
            "Plan": {
              "Node Type": "Nested Loop",
              "Startup Cost": 0.29,
              "Total Cost": 16.34,
              "Plan Rows": 1,
              "Plan Width": 244,
              "Actual Startup Time": 0.035,
              "Actual Total Time": 0.042,
              "Plans": [
                {
                  "Node Type": "Index Scan",
                  "Relation Name": "users",
                  "Index Name": "users_pkey",
                  "Startup Cost": 0.15,
                  "Total Cost": 8.17,
                  "Plan Rows": 1,
                  "Plan Width": 120
                }
              ]
            }
          }
        ]"#;

        let node = ExplainPlanNode::parse(pg_json).expect("should parse PostgreSQL EXPLAIN JSON");
        assert_eq!(node.node_type, "Nested Loop");
        assert_eq!(node.total_cost, 16.34);
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].node_type, "Index Scan");
        assert_eq!(node.children[0].relation_name.as_deref(), Some("users"));
        assert_eq!(node.children[0].index_name.as_deref(), Some("users_pkey"));
    }

    #[test]
    fn test_parse_mysql_explain_json() {
        let mysql_json = r#"{
          "query_block": {
            "select_id": 1,
            "cost_info": {
              "query_cost": "2.50"
            },
            "table": {
              "table_name": "orders",
              "access_type": "ALL",
              "rows_examined_per_scan": 100,
              "cost_info": {
                "prefix_cost": "2.50"
              }
            }
          }
        }"#;

        let node = ExplainPlanNode::parse(mysql_json).expect("should parse MySQL EXPLAIN JSON");
        assert_eq!(node.relation_name.as_deref(), Some("orders"));
        assert!(node.node_type.contains("Seq Scan"));
        assert_eq!(node.total_cost, 2.50);
        assert_eq!(node.plan_rows, 100);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_zoom_constants() {
        assert!(MIN_ZOOM > 0.0);
        assert!(MAX_ZOOM > MIN_ZOOM);
        assert!(DEFAULT_ZOOM >= MIN_ZOOM && DEFAULT_ZOOM <= MAX_ZOOM);
    }

    #[test]
    fn test_center_diagram_empty() {
        let mut state = DiagramState {
            pan: egui::vec2(100.0, 50.0),
            ..Default::default()
        };
        center_diagram(&mut state, egui::vec2(800.0, 600.0));
        assert_eq!(state.pan, egui::Vec2::ZERO);
    }

    #[test]
    fn test_center_diagram_with_nodes() {
        let mut state = DiagramState::default();
        state.nodes.push(crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "users".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
        });

        // Bounding box: min (100, 100), max (300, 200), center (200, 150)
        // Viewport size: (800, 600), view center: (400, 300)
        // Expected pan = (400, 300) - (200, 150) * 1.0 = (200, 150)
        center_diagram(&mut state, egui::vec2(800.0, 600.0));
        assert_eq!(state.pan, egui::vec2(200.0, 150.0));
    }

    #[test]
    fn test_check_nodes_overlap_detection() {
        let node_a = crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "table_a".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
        };

        // Node B bertumpukan langsung dengan Node A
        let mut node_b = node_a.clone();
        node_b.id = "table_b".to_string();
        node_b.pos = egui::pos2(150.0, 120.0);

        let nodes = vec![node_a.clone(), node_b];
        assert!(check_nodes_overlap(&nodes, 20.0));
        assert!(check_single_node_collision(&nodes, "table_a", 20.0));

        // Node C berada jauh di posisi aman (tidak bertumpukan)
        let mut node_c = node_a.clone();
        node_c.id = "table_c".to_string();
        node_c.pos = egui::pos2(500.0, 500.0);

        let non_overlapping = vec![node_a, node_c];
        assert!(!check_nodes_overlap(&non_overlapping, 20.0));
        assert!(!check_single_node_collision(&non_overlapping, "table_a", 20.0));
    }

    #[test]
    fn test_resolve_node_overlaps_separates_nodes() {
        let node_a = crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "table_a".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
        };

        let mut node_b = node_a.clone();
        node_b.id = "table_b".to_string();
        node_b.pos = egui::pos2(120.0, 110.0); // Sengaja tumpang tindih

        let mut nodes = vec![node_a.clone(), node_b.clone()];
        assert!(check_nodes_overlap(&nodes, 20.0));

        // Jalankan resolusi tumpang tindih
        resolve_node_overlaps(&mut nodes, 20.0);

        // Setelah dipisahkan, tidak boleh lagi ada yang tumpang tindih
        assert!(!check_nodes_overlap(&nodes, 20.0));
    }

    #[test]
    fn test_resolve_dragged_node_overlap_leaves_stationary_node_in_place() {
        let node_a = crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "table_a".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
        };

        // Node B di-drop tepat menimpa Node A
        let mut node_b = node_a.clone();
        node_b.id = "table_b".to_string();
        node_b.pos = egui::pos2(150.0, 100.0);

        let mut nodes = vec![node_a.clone(), node_b];
        assert!(check_nodes_overlap(&nodes, 20.0));

        // Selesaikan overlap khusus untuk node_b yang di-drag
        resolve_dragged_node_overlap(&mut nodes, "table_b", 20.0);

        // table_a harus tetap stabil di posisi aslinya (100.0, 100.0)
        assert_eq!(nodes[0].pos, egui::pos2(100.0, 100.0));

        // Dan kedua tabel sudah tidak lagi tumpang tindih
        assert!(!check_nodes_overlap(&nodes, 20.0));
    }

    #[test]
    fn test_diagram_state_prevent_overlap_default() {
        let state = DiagramState::default();
        assert!(state.prevent_overlap);

        // JSON tanpa properti prevent_overlap harus mendefaultkan ke true
        let json_data = r#"{"nodes":[],"edges":[],"groups":[],"pan":[0.0,0.0],"zoom":1.0,"is_centered":false}"#;
        let deserialized: DiagramState = serde_json::from_str(json_data).expect("should deserialize");
        assert!(deserialized.prevent_overlap);
    }

    #[test]
    fn test_diagram_search_filter_flags() {
        let mut state = DiagramState::default();
        assert!(state.search_tables);
        assert!(state.search_columns);
        assert!(state.search_groups);

        // JSON deserialization harus mendefaultkan search flags ke true
        let json_data = r#"{"nodes":[],"edges":[],"groups":[],"pan":[0.0,0.0],"zoom":1.0,"is_centered":false}"#;
        let deserialized: DiagramState = serde_json::from_str(json_data).expect("should deserialize");
        assert!(deserialized.search_tables);
        assert!(deserialized.search_columns);
        assert!(deserialized.search_groups);

        // Uji fleksibilitas filter pencarian (bisa salah satu, kombinasi, atau semua)
        let table_title = "users";
        let col_name = "email";
        let group_title = "Auth Group";
        let q = "user";

        state.search_tables = true;
        state.search_columns = false;
        state.search_groups = false;
        assert!(state.search_tables && table_title.contains(q));
        assert!(!(state.search_columns && col_name.contains("mail")));
        assert!(!(state.search_groups && group_title.to_lowercase().contains("auth")));

        state.search_tables = false;
        state.search_columns = true;
        assert!(!(state.search_tables && table_title.contains(q)));
        assert!(state.search_columns && col_name.contains("mail"));

        state.search_columns = false;
        state.search_groups = true;
        assert!(state.search_groups && group_title.to_lowercase().contains("auth"));
    }
}

