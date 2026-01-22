use eframe::egui;
use crate::models::structs::{DiagramState, DiagramNode};

pub fn render_diagram(ui: &mut egui::Ui, state: &mut DiagramState) {
    let rect = ui.available_rect_before_wrap();
    
    // Handle Pan and Zoom
    let response = ui.interact(rect, ui.id().with("diagram_bg"), egui::Sense::click_and_drag());
    
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
    });


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

        // Mouse Wheel Zoom
        let scroll_delta = i.raw_scroll_delta.y;
        if scroll_delta != 0.0 {
            let zoom_factor = 1.0 + scroll_delta * 0.001;
            state.zoom *= zoom_factor;
        }

        // Clamp zoom
        state.zoom = state.zoom.clamp(0.1, 5.0);

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
    let pan = state.pan;
    
    let to_screen = move |pos: egui::Pos2| -> egui::Pos2 {
        rect.min + pan + pos.to_vec2() * scale
    };

    // Draw Groups (Containers)
    let mut _group_rename_request: Option<(usize, String)> = None;
    let mut group_drag_delta: Option<(String, egui::Vec2)> = None;

    // 1. Calculate Group Bounds (requires immutable access to nodes and groups)
    let mut group_bounds: Vec<(usize, String, egui::Rect, egui::Color32, String)> = Vec::new(); // (index, id, rect, color, title)
    
    let shift_held = ui.input(|i| i.modifiers.shift);
    for (idx, group) in state.groups.iter().enumerate() {
         let group_nodes: Vec<&DiagramNode> = state.nodes.iter()
            .filter(|n| n.group_id.as_deref() == Some(&group.id))
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
                group_bounds.push((idx, group.id.clone(), rect, group.color, group.title.clone()));
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
             let response = ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
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
                 
                 
                 ui.horizontal(|ui| {
                     ui.label("Color:");
                     egui::ScrollArea::horizontal().max_width(200.0).show(ui, |ui| {
                         ui.horizontal(|ui| {
                             let colors = [
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
                             
                             for &c in &colors {
                                 let (response, painter) = ui.allocate_painter(egui::vec2(20.0, 20.0), egui::Sense::click());
                                 let rect = response.rect;
                                 painter.rect_filled(rect, 4.0, c);
                                 if response.hovered() {
                                     painter.rect_stroke(rect, 4.0, egui::Stroke::new(2.0, egui::Color32::WHITE), egui::StrokeKind::Middle);
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
    } // Apply deferred group move
    if let Some((group_id, delta)) = group_drag_delta {
        // Move nodes belonging to group
        for node in &mut state.nodes {
            if node.group_id.as_deref() == Some(&group_id) {
                node.pos += delta;
            }
        }
        
        // Move group manual_pos if it exists (for empty groups)
        if let Some(group) = state.groups.iter_mut().find(|g| g.id == group_id)
            && let Some(pos) = &mut group.manual_pos {
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
                let is_selected = state.selected_edge.as_ref() == Some(&(edge.source.clone(), edge.target.clone()));

                // Determine if highlighted by column
                let is_highlighted_by_col = if let Some((sel_table, sel_col)) = &state.selected_column {
                     src.foreign_keys.iter().any(|fk| 
                        fk.referenced_table_name == edge.target &&
                        (
                            (fk.table_name == *sel_table && fk.column_name == *sel_col) ||
                            (fk.referenced_table_name == *sel_table && fk.referenced_column_name == *sel_col)
                        )
                     )
                } else {
                    false
                };

                let is_active = is_selected || is_highlighted_by_col;

                // Determine base color from source group
                let mut base_color = egui::Color32::from_gray(100);
                if let Some(group_id) = &src.group_id
                    && let Some(group) = state.groups.iter().find(|g| &g.id == group_id) {
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
                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                   // Sample points to check distance
                   let num_samples = 30; // Increased samples for smoother detection
                   for i in 0..=num_samples {
                       let t = i as f32 / num_samples as f32;
                       let p = bezier.sample(t);
                       if p.distance(pos) < 20.0 { // Increased tolerance
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
                        let hover_stroke = egui::Stroke::new(2.0 * scale, egui::Color32::from_gray(180));
                         ui.painter().add(egui::epaint::CubicBezierShape::from_points_stroke(
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

    let edge_was_clicked = clicked_edge.is_some();
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
    let mut node_clicked = false;
    let mut column_clicked_request: Option<(String, String)> = None;

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
        
        if node_response.clicked() {
            node_clicked = true;
            // Selecting a node could perhaps select edges? For now, just prevent deselection.
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
                 // Use pointer position for better intuition
                 if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                     let mut new_group_id = None;
                     
                     // Check against group bounds (calculated earlier)
                     for (_, gid, rect, _, _) in &group_bounds {
                         if rect.contains(pointer_pos) {
                             new_group_id = Some(gid.clone());
                             break; 
                         }
                     }
                     
                     node.group_id = new_group_id;
                 }
             }
             state.dragging_node = None;
        }

         // Check if this node is part of the selected relationship
        let is_glow = if let Some((s, t)) = &state.selected_edge {
            node.id == *s || node.id == *t
        } else {
            false
        };

        // Draw Shadow/Border
        if is_glow {
             // Glow effect
             ui.painter().rect_filled(
                node_rect.expand(6.0 * scale), 
                12.0 * scale, 
                egui::Color32::from_rgb(255, 215, 0).linear_multiply(0.5) // Gold with transparency
            );
        } else {
            ui.painter().rect_filled(
                node_rect.expand(2.0 * scale), 
                5.0 * scale, 
                egui::Color32::from_black_alpha(50)
            );
        }
        
        let fill_color = egui::Color32::from_rgb(30, 30, 35);
        ui.painter().rect_filled(
            node_rect, 
            4.0 * scale, 
            fill_color
        );
        // Corrected rect_stroke args
        let border_color = if is_glow { egui::Color32::from_rgb(255, 215, 0) } else { egui::Color32::from_gray(60) };
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
            let is_fk = node.foreign_keys.iter().any(|fk| fk.column_name == *col && fk.table_name == node.id);
             
             // Interaction Rect
             let col_pos_screen = node_pos_screen + egui::vec2(0.0, y_offset);
             let col_rect = egui::Rect::from_min_size(
                 col_pos_screen,
                 egui::vec2(node_rect.width(), item_height)
             );

             let col_id = ui.id().with("col").with(&node.id).with(col);
             let response = ui.interact(col_rect, col_id, egui::Sense::click());
             
             if response.clicked() {
                 column_clicked_request = Some((node.id.clone(), col.clone()));
             }

             // Highlight if selected
             // We can't access state.selected_column here due to borrow of state.nodes
             // But we can check after loop? No, visual feedback needs to be here.
             // We can pass `selected_column` into the loop if we extract it before?
             // But we iterate `state.nodes`. 
             // We need to copy `selected_column` before the loop.
             // I'll do that in the previous chunk.
             
             if response.hovered() {
                 ui.painter().rect_filled(col_rect, 0.0, egui::Color32::from_white_alpha(10));
             }

             // Text
             let text_pos = node_pos_screen + egui::vec2(8.0 * scale, y_offset);
             ui.painter().text(
                text_pos,
                egui::Align2::LEFT_TOP,
                col, 
                egui::FontId::monospace(12.0 * scale),
                if is_fk { egui::Color32::from_rgb(200, 200, 100) } else { egui::Color32::LIGHT_GRAY }
            );
            y_offset += item_height;
        }
    }
    
    // Clear selection if clicked on background (and not on an edge or node)
    // We check `response` from the beginning of the function (passed down? no it was `ui.interact(rect...)`)
    // We need to check if the main rect was clicked, and ensure no edge/node was clicked.
    if ui.input(|i| i.pointer.primary_clicked()) && !node_clicked && !edge_was_clicked && column_clicked_request.is_none() {
        // But wait, `ui.interact` for background handles drag. Does it also report click?
        // We can check if the pointer is within the clip rect and nothing else claimed it?
        // Simpler: If the background response was clicked? 
         // Accessing `response` from top of function might be hard unless we passed it.
         // Let's rely on global input.
         if ui.rect_contains_pointer(rect) {
             state.selected_edge = None;
             state.selected_column = None;
         }
    }
    
    if let Some(req) = column_clicked_request {
        state.selected_column = Some(req);
    }

    if let Some(id) = dragging_node_id
        && let Some(node) = state.nodes.iter_mut().find(|n| n.id == id) {
            node.pos += drag_delta;
        }

    // Draw Toolbar (Export/Import)
    // Move it a bit closer to the right edge if requested, generally right_top aligned is standard.
    // Making it transparent and red text.
    let toolbar_width = 100.0;
    let toolbar_height = 40.0;
    let padding = -10.0;
    let toolbar_pos = rect.right_top() + egui::vec2(-toolbar_width, padding);
    let toolbar_rect = egui::Rect::from_min_size(toolbar_pos, egui::vec2(toolbar_width, toolbar_height));

    ui.scope_builder(egui::UiBuilder::new().max_rect(toolbar_rect), |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(20.0);
            // "Export" first because we are in right_to_left layout
            if ui.add(egui::Button::new(egui::RichText::new("Export").color(egui::Color32::from_rgb(255, 100, 100))).frame(false)).clicked()
                 && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).save_file()
                    && let Ok(file) = std::fs::File::create(path) {
                        let writer = std::io::BufWriter::new(file);
                        let _ = serde_json::to_writer_pretty(writer, state);
                    }

            ui.add_space(10.0);

            // "Import" second
            if ui.add(egui::Button::new(egui::RichText::new("Import").color(egui::Color32::from_rgb(255, 100, 100))).frame(false)).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file()
                    && let Ok(file) = std::fs::File::open(path) {
                        let reader = std::io::BufReader::new(file);
                        if let Ok(new_state) = serde_json::from_reader::<_, DiagramState>(reader) {
                            *state = new_state;
                            state.dragging_node = None;
                            state.last_mouse_pos = None;
                            state.save_requested = true;
                        }
                    }
            ui.add_space(10.0);

        });
    });

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
                    if ui.button("Save").clicked() || (ui.input(|i| i.key_pressed(egui::Key::Enter)) && !state.new_group_buffer.is_empty()) {
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
            if let Some(src_idx) = state.nodes.iter().position(|n| n.id == edge.source)
                && let Some(dst_idx) = state.nodes.iter().position(|n| n.id == edge.target) {
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

        // 3b. Group Overlap Resolution (Push entire groups apart)
        // Re-calculate group bounds every iteration as nodes move
        let mut group_bounds: std::collections::HashMap<String, egui::Rect> = std::collections::HashMap::new();
        
        // Calculate bounds
        for node in &state.nodes {
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
                         } else if r1.center().y < r2.center().y {
                             egui::vec2(0.0, -overlap_h)
                         } else {
                             egui::vec2(0.0, overlap_h)
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
    for (node, force) in state.nodes.iter_mut().zip(forces.iter_mut()) {
        if state.dragging_node.as_deref() == Some(&node.id) { continue; } // Don't move dragged node
        
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
