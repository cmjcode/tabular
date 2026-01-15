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

    // Zoom with scroll (if supported by global input, tricky in simple widget, so maybe just simple controls for now)
    // For now, let's just stick to pan.

    // Clip to rect
    let _clip_rect = ui.clip_rect();
    // clip_rect.expand(state.zoom); // Simple zoom scaling not implemented yet for drawing
    
    let to_screen = |pos: egui::Pos2| -> egui::Pos2 {
        rect.min + state.pan + pos.to_vec2()
    };
    
    // Draw edges (relationships)
    for edge in &state.edges {
        if let Some(src) = state.nodes.iter().find(|n| n.id == edge.source) {
            if let Some(dst) = state.nodes.iter().find(|n| n.id == edge.target) {
                let src_pos = to_screen(src.pos) + egui::vec2(src.size.x, src.size.y / 2.0); // right side
                let dst_pos = to_screen(dst.pos) + egui::vec2(0.0, dst.size.y / 2.0); // left side
                
                let color = egui::Color32::from_gray(100);
                let stroke = egui::Stroke::new(1.0, color);
                
                // Cubic bezier for smooth connection
                let control_scale = (dst_pos.x - src_pos.x).abs().max(50.0) * 0.5;
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

    for node in &mut state.nodes {
        let screen_pos = to_screen(node.pos);
        // Estimate height based on columns
        let header_height = 24.0;
        let item_height = 16.0;
        let content_height = node.columns.len() as f32 * item_height;
        let node_height = header_height + content_height + 8.0; // padding
        node.size = egui::vec2(180.0, node_height);

        let node_rect = egui::Rect::from_min_size(screen_pos, node.size);
        
        // Window-like styling
        let frame_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(60));
        let fill_color = egui::Color32::from_rgb(30, 30, 35);
        let header_color = egui::Color32::from_rgb(50, 50, 60);

        // Interact
        let node_id = ui.id().with("node").with(&node.id);
        let node_response = ui.interact(node_rect, node_id, egui::Sense::click_and_drag());
        
        if node_response.dragged() {
            dragging_node_id = Some(node.id.clone());
            drag_delta = node_response.drag_delta();
        }

        // Draw shadow
        painter.rect_filled(node_rect.expand(2.0), 4.0, egui::Color32::from_black_alpha(50));
        
        // Draw body
        painter.rect_filled(node_rect, 4.0, fill_color);
        painter.rect_stroke(node_rect, 4.0, frame_stroke, egui::StrokeKind::Middle);

        // Draw Header
        let header_rect = egui::Rect::from_min_size(screen_pos, egui::vec2(node.size.x, header_height));
        painter.rect_filled(header_rect, 4.0f32, header_color);
        painter.rect_stroke(header_rect, 4.0f32, frame_stroke, egui::StrokeKind::Middle);
        
        painter.text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            &node.title,
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );

        // Draw Columns
        let mut cursor = screen_pos + egui::vec2(8.0, header_height + 4.0);
        for col in &node.columns {
            let is_pk = node.foreign_keys.iter().any(|fk| fk.column_name == *col && fk.table_name == node.id); // Simple heuristic, better to have explicit PK flag in DiagramNode
            
            // Just simple text for now
            painter.text(
                cursor,
                egui::Align2::LEFT_TOP,
                col,
                egui::FontId::monospace(12.0),
                if is_pk { egui::Color32::from_rgb(200, 200, 100) } else { egui::Color32::LIGHT_GRAY },
            );
            cursor.y += item_height;
        }
    }

    if let Some(id) = dragging_node_id {
        if let Some(node) = state.nodes.iter_mut().find(|n| n.id == id) {
            node.pos += drag_delta;
        }
    }
}
