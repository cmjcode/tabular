use eframe::egui;
use crate::models::structs::DiagramState;

pub fn render_diagram(ui: &mut egui::Ui, state: &mut DiagramState) {
    let painter = ui.painter();
    let rect = ui.available_rect_before_wrap();
    
    // Handle Pan and Zoom
    let response = ui.interact(rect, ui.id().with("diagram_bg"), egui::Sense::drag());
    
    // Pan with middle mouse or drag on background
    if response.dragged() {
        state.pan += response.drag_delta();
    }

    // Zoom with Ctrl/Cmd + Scroll or Keys
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
    });

    // Removed scroll zoom to fix compilation error (and user only requested keys)

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
        // formula: screen_pos = rect.min + pan + node_pos * zoom
        // target: rect.min + view_center = rect.min + pan + content_center * zoom
        // view_center = pan + content_center * zoom
        // pan = view_center - content_center * zoom
        
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
                painter.add(bezier);
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
        }

        // Draw Shadow/Border
        painter.rect_filled(
            node_rect.expand(2.0 * scale), 
            5.0 * scale, 
            egui::Color32::from_black_alpha(50)
        );
        
        let fill_color = egui::Color32::from_rgb(30, 30, 35);
        painter.rect_filled(
            node_rect, 
            4.0 * scale, 
            fill_color
        );
        // Corrected rect_stroke args
        painter.rect_stroke(
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
        painter.rect_filled(
            header_rect, 
            4.0 * scale,
            egui::Color32::from_rgb(50, 50, 60)
        );
        
        // Title
        painter.text(
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
             painter.text(
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
    let iterations = 300;
    let repulsion_force = 600000.0; // Increased base repulsion
    let spring_length = 350.0; // Increased spring length
    let attraction_constant = 0.05; // Reduced attraction slightly
    let center_gravity = 0.02; // Reduced center pull to allow expansion
    let prefix_attraction = 0.08; // Increased prefix attraction to keep groups tight despite higher repulsion
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

                // COLLISION AVOIDANCE (Rectangle Overlap)
                // We define a "radius" for each as half the diagonal, or better, use Projected overlap.
                // Simple approach: Treat as circles with radius = max(w,h)/2 for safety
                // Or: Add extra force if bounding boxes overlap.
                
                let size_i = state.nodes[i].size;
                let size_j = state.nodes[j].size;
                
                // Effective radius approximation (avg width/height) + margin
                let r_i = (size_i.x + size_i.y) / 3.5; 
                let r_j = (size_j.x + size_j.y) / 3.5;
                let min_dist = r_i + r_j + 50.0; // 50.0 margin
                
                if dist < min_dist {
                     // Strong push if inside comfort zone
                     // Exponential push-back
                     force_scalar += 500_000.0 * (min_dist - dist) / min_dist; 
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
