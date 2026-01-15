use eframe::egui;
use crate::models::structs::{DiagramState, DiagramNode};

pub fn render_diagram(ui: &mut egui::Ui, state: &mut DiagramState) {
    let rect = ui.available_rect_before_wrap();
    
    // Handle Pan and Zoom
    let response = ui.interact(rect, ui.id().with("diagram_bg"), egui::Sense::drag());
    
    // Pan with middle mouse or drag on background
    if response.dragged() {
        state.pan += response.drag_delta();
    }

    // Zoom with Ctrl/Cmd + Scroll or Keys
    ui.input_mut(|i| {
        // Zoom In
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals) {
            state.zoom *= 1.1;
        }
        // Zoom Out
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus) {
            state.zoom /= 1.1;
        }

        // Clamp zoom
        if state.zoom < 0.1 { state.zoom = 0.1; }
        if state.zoom > 5.0 { state.zoom = 5.0; }

        // Save Shortcut (Cmd + S)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) {
            state.save_requested = true;
        }
    });

    // Handle Initial Centering
    if !state.is_centered && !state.nodes.is_empty() {
        // Calculate bounding box of nodes
        let mut min_pos = state.nodes[0].pos;
        let mut max_pos = state.nodes[0].pos + state.nodes[0].size;
        
        for node in &state.nodes {
            min_pos = min_pos.min(node.pos);
            max_pos = max_pos.max(node.pos + node.size);
        }
        
        let content_center = min_pos + (max_pos - min_pos) / 2.0;
        let view_center = rect.size() / 2.0;
        
        // Calculate target pan to align content_center with view_center
        state.pan = view_center - content_center.to_vec2() * state.zoom;
        state.is_centered = true;
    }

    // Clip to rect
    let _clip_rect = ui.clip_rect();
    
    // Scale helper
    let scale = state.zoom;
    
    let to_screen = |pos: egui::Pos2| -> egui::Pos2 {
        rect.min + state.pan + pos.to_vec2() * scale
    };

    // Draw Groups (Containers)
    let mut _group_rename_request: Option<(usize, String)> = None;
    let mut group_drag_delta: Option<(String, egui::Vec2)> = None;

    // 1. Calculate Group Bounds (requires immutable access to nodes and groups)
    let mut group_bounds: Vec<(usize, String, egui::Rect, egui::Color32, String)> = Vec::new(); // (index, id, rect, color, title)
    
    for (idx, group) in state.groups.iter().enumerate() {
         let group_nodes: Vec<&DiagramNode> = state.nodes.iter()
            .filter(|n| n.group_id.as_deref() == Some(&group.id))
            .filter(|n| Some(n.id.clone()) != state.dragging_node) // Exclude if being dragged
            .collect();
            
        if group_nodes.is_empty() { continue; }

        let mut min_pos = group_nodes[0].pos;
        let mut max_pos = group_nodes[0].pos + group_nodes[0].size;
        
        for node in &group_nodes {
            min_pos = min_pos.min(node.pos);
            max_pos = max_pos.max(node.pos + node.size);
        }
        
        // Padding
        let padding = 20.0;
        min_pos -= egui::vec2(padding, padding + 30.0);
        max_pos += egui::vec2(padding, padding);
        
        let min_screen = to_screen(min_pos);
        let max_screen = to_screen(max_pos);
        let rect = egui::Rect::from_min_max(min_screen, max_screen);
        
        group_bounds.push((idx, group.id.clone(), rect, group.color, group.title.clone()));
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
        
        // Draw Background
        ui.painter().rect_filled(
            group_rect, 
            8.0 * scale, 
            color.linear_multiply(0.1)
        );
        ui.painter().rect_stroke(
            group_rect, 
            8.0 * scale, 
            egui::Stroke::new(1.0 * scale, color.linear_multiply(0.5)),
            egui::StrokeKind::Middle
        );

        // Header Rect
        let title_rect = egui::Rect::from_min_size(
            group_rect.min, 
            egui::vec2(group_rect.width(), 30.0 * scale)
        );
        
        ui.painter().rect_filled(
            title_rect, 
            8.0 * scale,
            color.linear_multiply(0.8)
        );

        let is_renaming = state.renaming_group.as_deref() == Some(group_id);

        if is_renaming {
             let edit_rect = title_rect.shrink(2.0);
             let response = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                 ui.add(egui::TextEdit::singleline(&mut group.title)
                    .frame(false)
                    .text_color(egui::Color32::WHITE)
                    .font(egui::FontId::proportional(16.0 * scale)))
             }).inner;
             
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
                egui::Color32::WHITE
            );

             // Interaction
             let interact_rect = title_rect;
             let response = ui.interact(interact_rect, ui.id().with("group_header").with(idx), egui::Sense::click_and_drag());
             
             if response.dragged() {
                 let delta = response.drag_delta() / scale;
                 group_drag_delta = Some((group_id.clone(), delta));
             }

             response.context_menu(|ui| {
                 if ui.button("Rename Container").clicked() {
                     ui.close();
                     // Activate renaming logic only if NOT already renaming
                     // We need to set state flag.
                     // But we have a mutable borrow on `group` (part of state).
                     // Can we assign to `state.renaming_group`?
                     // `state` is borrowed mutably to get `group`.
                     // Rust might complain about splitting borrow.
                      _group_rename_request = Some((idx, group_id.clone()));
                 }
             });
        }
    }
    
    // Apply rename request (workaround for borrow checker)
    if let Some((_, gid)) = _group_rename_request {
        state.renaming_group = Some(gid);
    } // Apply deferred group move
    if let Some((group_id, delta)) = group_drag_delta {
        for node in &mut state.nodes {
            if node.group_id.as_deref() == Some(&group_id) {
                node.pos += delta;
            }
        }
    }

    // Draw edges (relationships)
    for edge in &state.edges {
        if let Some(src) = state.nodes.iter().find(|n| n.id == edge.source) {
            if let Some(dst) = state.nodes.iter().find(|n| n.id == edge.target) {
                let src_rect_size = src.size * scale;
                let dst_rect_size = dst.size * scale;
                
                let src_pos = to_screen(src.pos) + egui::vec2(src_rect_size.x, src_rect_size.y / 2.0); // right side
                let dst_pos = to_screen(dst.pos) + egui::vec2(0.0, dst_rect_size.y / 2.0); // left side
                
                let color = egui::Color32::from_gray(100);
                let stroke = egui::Stroke::new(1.0 * scale, color); // Scale stroke too
                
                // Cubic bezier for smooth connection
                let control_scale = (dst_pos.x - src_pos.x).abs().max(50.0 * scale) * 0.5;
                let control1 = src_pos + egui::vec2(control_scale, 0.0);
                let control2 = dst_pos - egui::vec2(control_scale, 0.0);
                
                let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
                    [src_pos, control1, control2, dst_pos],
                    false,
                    egui::Color32::TRANSPARENT,
                    stroke,
                );
                ui.painter().add(bezier);
            }
        }
    }

    // Draw nodes
    let mut dragging_node_id = None;
    let mut drag_delta = egui::Vec2::ZERO;

    // For manual interaction:
    let _mouse_pos = ui.input(|i| i.pointer.hover_pos());
    
    for node in &mut state.nodes {
        // Estimate height based on columns
        let header_height_unscaled = 24.0;
        let item_height_unscaled = 16.0;
        let content_height_unscaled = node.columns.len() as f32 * item_height_unscaled;
        let node_height_unscaled = header_height_unscaled + content_height_unscaled + 8.0; // padding
        node.size = egui::vec2(180.0, node_height_unscaled);

        let node_size_scaled = node.size * scale;
        let node_pos_screen = to_screen(node.pos);
        let node_rect = egui::Rect::from_min_size(node_pos_screen, node_size_scaled);
        
        // Interact
        let node_id = ui.id().with("node").with(&node.id);
        let node_response = ui.interact(node_rect, node_id, egui::Sense::click_and_drag());
        
        if node_response.dragged() {
            dragging_node_id = Some(node.id.clone());
            drag_delta = node_response.drag_delta();
            
            // Track globally for drop detection
            state.dragging_node = Some(node.id.clone());
        } else if node_response.drag_stopped() {
             // Check drop target
             let node_center_screen = node_rect.center();
             let mut new_group_id = None;
             
             // Check against group bounds (calculated earlier)
             for (_, gid, rect, _, _) in &group_bounds {
                 if rect.contains(node_center_screen) {
                     new_group_id = Some(gid.clone());
                     break; 
                 }
             }
             
             node.group_id = new_group_id;
             state.dragging_node = None;
        }

        // Draw Shadow/Border
        ui.painter().rect_filled(
            node_rect.expand(2.0 * scale), 
            5.0 * scale, 
            egui::Color32::from_black_alpha(50)
        );
        
        let fill_color = egui::Color32::from_rgb(30, 30, 35);
        ui.painter().rect_filled(
            node_rect, 
            4.0 * scale, 
            fill_color
        );
        // Corrected rect_stroke args
        ui.painter().rect_stroke(
            node_rect, 
            4.0 * scale, 
            egui::Stroke::new(1.0 * scale, egui::Color32::from_gray(60)),
            egui::StrokeKind::Middle,
        );
        
        // Header
        let header_height = header_height_unscaled * scale;
        let header_rect = egui::Rect::from_min_size(
            node_pos_screen, 
            egui::vec2(node_rect.width(), header_height)
        );
        
        // Simplified rounding to avoid compilation error
        ui.painter().rect_filled(
            header_rect, 
            4.0 * scale,
            egui::Color32::from_rgb(50, 50, 60)
        );
        
        // Title
        ui.painter().text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            &node.title, 
            egui::FontId::proportional(14.0 * scale),
            egui::Color32::WHITE
        );
        
        // Columns
        let item_height = item_height_unscaled * scale;
        let mut y_offset = header_height + 4.0 * scale;
        
        for col in &node.columns {
            let is_pk = node.foreign_keys.iter().any(|fk| fk.column_name == *col && fk.table_name == node.id);
             // Just name for now, maybe type later.
             // Truncate if too long?
             let text_pos = node_pos_screen + egui::vec2(8.0 * scale, y_offset);
             ui.painter().text(
                text_pos,
                egui::Align2::LEFT_TOP,
                col, // name is just the string
                egui::FontId::monospace(12.0 * scale),
                if is_pk { egui::Color32::from_rgb(200, 200, 100) } else { egui::Color32::LIGHT_GRAY }
            );
            y_offset += item_height;
        }
    }

    if let Some(id) = dragging_node_id {
        if let Some(node) = state.nodes.iter_mut().find(|n| n.id == id) {
            node.pos += drag_delta;
        }
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
    if node_count == 0 { return; }

    // Helper to get prefix (e.g., "user" from "user_data")
    let get_prefix = |name: &str| -> String {
        name.split('_').next().unwrap_or(name).to_string()
    };

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
                if dist < 1.0 { dist = 1.0; } // Avoid zero division

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
            if let Some(src_idx) = state.nodes.iter().position(|n| n.id == edge.source) {
                if let Some(dst_idx) = state.nodes.iter().position(|n| n.id == edge.target) {
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

        // 3b. Group Overlap Resolution (Push entire groups apart)
        // Re-calculate group bounds every iteration as nodes move
        let mut group_bounds: std::collections::HashMap<String, egui::Rect> = std::collections::HashMap::new();
        
        // Calculate bounds
        for (_, node) in state.nodes.iter().enumerate() {
            if let Some(gid) = &node.group_id {
                let rect = egui::Rect::from_min_size(node.pos, node.size);
                group_bounds.entry(gid.clone())
                    .and_modify(|r| *r = r.union(rect))
                    .or_insert(rect);
            }
        }

        let group_ids: Vec<String> = group_bounds.keys().cloned().collect();
        let group_padding = 40.0; // Margin between groups

        for i in 0..group_ids.len() {
            for j in (i + 1)..group_ids.len() {
                let g1_id = &group_ids[i];
                let g2_id = &group_ids[j];
                
                if let (Some(r1), Some(r2)) = (group_bounds.get(g1_id), group_bounds.get(g2_id)) {
                    let r1_padded = r1.expand(group_padding);
                    let r2_padded = r2.expand(group_padding);
                    
                    let intersection = r1_padded.intersect(r2_padded);
                    if intersection.width() > 0.0 && intersection.height() > 0.0 {
                         let overlap_w = intersection.width();
                         let overlap_h = intersection.height();
                         
                         // Push apart on axis of least overlap
                         let push_vec = if overlap_w < overlap_h {
                             if r1.center().x < r2.center().x {
                                 egui::vec2(-overlap_w, 0.0)
                             } else {
                                 egui::vec2(overlap_w, 0.0)
                             }
                         } else {
                             if r1.center().y < r2.center().y {
                                 egui::vec2(0.0, -overlap_h)
                             } else {
                                 egui::vec2(0.0, overlap_h)
                             }
                         } * 0.1; // Gentle push per iteration

                         // Apply to all nodes in group 1
                         for (idx, node) in state.nodes.iter().enumerate() {
                             if node.group_id.as_deref() == Some(g1_id) {
                                 forces[idx] += push_vec * 5.0; // Stronger group push
                             }
                         }
                         // Apply inverse to all nodes in group 2
                         for (idx, node) in state.nodes.iter().enumerate() {
                             if node.group_id.as_deref() == Some(g2_id) {
                                 forces[idx] -= push_vec * 5.0;
                             }
                         }
                    }
                }
            }
        }

        // 4. Center Gravity (Pull to 0,0) + Apply Forces
        for i in 0..node_count {
            let center_pull = egui::Vec2::ZERO - state.nodes[i].pos.to_vec2();
            forces[i] += center_pull * center_gravity;
            
            // Limit max force to prevent explosion
            let max_force = 1000.0;
            if forces[i].length() > max_force {
               forces[i] = forces[i].normalized() * max_force;
            }

            state.nodes[i].pos += forces[i] * delta_time;
        }
    }

    // STRICT COLLISION RESOLUTION (Post-Process)
    // Run a few passes to strictly separate overlapping rectangles
    let collision_iterations = 20;
    for _ in 0..collision_iterations {
        let mut resolved = true;
        for i in 0..node_count {
            for j in (i + 1)..node_count {
                let rect_i = egui::Rect::from_min_size(state.nodes[i].pos, state.nodes[i].size);
                let rect_j = egui::Rect::from_min_size(state.nodes[j].pos, state.nodes[j].size);
                
                // Expand rects slightly for padding
                let padding = 10.0;
                let padded_i = rect_i.expand(padding);
                let padded_j = rect_j.expand(padding);

                let intersection = padded_i.intersect(padded_j); // Returns Rect, not Option
                if intersection.width() > 0.0 && intersection.height() > 0.0 {
                        resolved = false;
                        let overlap_w = intersection.width();
                        let overlap_h = intersection.height();
                        
                        // Push apart on the axis of least overlap
                        let move_vec = if overlap_w < overlap_h {
                            // Move X
                            if rect_i.center().x < rect_j.center().x {
                                egui::vec2(-overlap_w / 2.0 - 1.0, 0.0)
                            } else {
                                egui::vec2(overlap_w / 2.0 + 1.0, 0.0)
                            }
                        } else {
                            // Move Y
                            if rect_i.center().y < rect_j.center().y {
                                egui::vec2(0.0, -overlap_h / 2.0 - 1.0)
                            } else {
                                egui::vec2(0.0, overlap_h / 2.0 + 1.0)
                            }
                        };
                        
                        state.nodes[i].pos += move_vec;
                        state.nodes[j].pos -= move_vec;
                    }
            }
        }
        if resolved { break; }
    }
    
    // Normalize coordinates to be positive and start at somewhat reasonable position
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    for node in &state.nodes {
        if node.pos.x < min_x { min_x = node.pos.x; }
        if node.pos.y < min_y { min_y = node.pos.y; }
    }
    
    for node in &mut state.nodes {
        node.pos.x -= min_x - 50.0;
        node.pos.y -= min_y - 50.0;
    }
}
