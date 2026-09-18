use crate::window_egui::Tabular;
use eframe::egui;

use super::pool::ensure_background_pool_creation;

// Render a connection selector popup when the user tries to execute a query without a connection.
// Shows a simple modal listing available connections; selecting one assigns it to the active tab
// and (optionally) auto-executes the pending query captured earlier.
pub(crate) fn render_connection_selector(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_connection_selector {
        return;
    }

    // If no connections configured, show guidance with quick action
    if tabular.connections.is_empty() {
        crate::window_egui::style::render_modal_backdrop(
            ctx,
            "no_connections_backdrop",
            tabular.show_connection_selector,
        );
        let mut close_dialog = false;
        egui::Window::new("No Connections Available")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(crate::window_egui::style::modal_window_frame(ctx))
            .default_width(380.0)
            .show(ctx, |ui| {
                crate::window_egui::style::render_modal_header(
                    ui,
                    "No Connections Available",
                    &mut close_dialog,
                );
                ui.add_space(8.0);
                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.label("No saved connections yet. Add a connection first.");
                    ui.add_space(8.0);
                    if ui
                        .add(crate::window_egui::style::btn_primary_ctx(
                            ui.ctx(),
                            "Add New Connection",
                        ))
                        .clicked()
                    {
                        tabular.show_add_connection = true;
                        close_dialog = true;
                    }
                });
            });
        if close_dialog || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            tabular.show_connection_selector = false;
        }
        return;
    }

    // Keep a local filter text in temporary egui memory (per-session)
    let filter_id = egui::Id::new("conn_selector_filter");
    let mut filter_text = ctx
        .data(|d| d.get_temp::<String>(filter_id))
        .unwrap_or_default();

    crate::window_egui::style::render_modal_backdrop(
        ctx,
        "conn_selector_backdrop",
        tabular.show_connection_selector,
    );
    let mut close_dialog = false;
    egui::Window::new("Connection Selector")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(true)
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .default_width(460.0)
        .show(ctx, |ui| {
            crate::window_egui::style::render_modal_header(
                ui,
                "Connection Selector",
                &mut close_dialog,
            );
            ui.add_space(8.0);
            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                let r = crate::window_egui::style::render_search_field(
                    ui,
                    &mut filter_text,
                    "type host / database / connection name...",
                    f32::INFINITY,
                );
                if r.changed() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(filter_id, filter_text.clone()));
                }

                ui.add_space(8.0);

                let mut items: Vec<_> = tabular.connections.clone();
                if !filter_text.trim().is_empty() {
                    let f = filter_text.to_lowercase();
                    items.retain(|c| {
                        c.name.to_lowercase().contains(&f)
                            || c.host.to_lowercase().contains(&f)
                            || c.database.to_lowercase().contains(&f)
                            || format!("{:?}", c.connection_type)
                                .to_lowercase()
                                .contains(&f)
                    });
                }

                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for conn in items.iter() {
                            let title = format!(
                                "{} — {:?} @ {}:{}{}",
                                conn.name,
                                conn.connection_type,
                                conn.host,
                                conn.port,
                                if conn.database.is_empty() {
                                    "".to_string()
                                } else {
                                    format!(" / {}", conn.database)
                                }
                            );

                            let mut should_connect = false;
                            let lresp = ui.selectable_label(false, title);
                            if lresp.clicked() || lresp.double_clicked() {
                                should_connect = true;
                            }

                            if should_connect {
                                if let Some(id) = conn.id {
                                    if let Some(tab) =
                                        tabular.query_tabs.get_mut(tabular.active_tab_index)
                                    {
                                        tab.connection_id = Some(id);
                                        if (tab.database_name.is_none()
                                            || tab
                                                .database_name
                                                .as_deref()
                                                .unwrap_or("")
                                                .is_empty())
                                            && !conn.database.is_empty()
                                        {
                                            tab.database_name = Some(conn.database.clone());
                                        }
                                    }
                                    tabular.current_connection_id = Some(id);
                                    ensure_background_pool_creation(tabular, id);

                                    tabular.show_connection_selector = false;

                                    if tabular.auto_execute_after_connection {
                                        crate::editor::execute_query(tabular);
                                        tabular.auto_execute_after_connection = false;
                                        tabular.pending_query.clear();
                                    }
                                }
                                break;
                            }
                        }
                    });
            });
        });
    if close_dialog || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        tabular.show_connection_selector = false;
    }
}
