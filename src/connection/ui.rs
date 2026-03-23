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
        let mut open = tabular.show_connection_selector;
        egui::Window::new("No Connections Available")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Belum ada koneksi tersimpan. Tambahkan koneksi terlebih dahulu.");
                ui.horizontal(|ui| {
                    if ui.button("Add new connection").clicked() {
                        tabular.show_add_connection = true;
                        tabular.show_connection_selector = false;
                    }
                });
            });
        if !open {
            tabular.show_connection_selector = false;
        }
        return;
    }

    // Keep a local filter text in temporary egui memory (per-session)
    let filter_id = egui::Id::new("conn_selector_filter");
    let mut filter_text = ctx
        .data(|d| d.get_temp::<String>(filter_id))
        .unwrap_or_default();

    let mut open = tabular.show_connection_selector;
    egui::Window::new("Connection Selector")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut filter_text)
                        .hint_text("type host / database / connection name...")
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(filter_id, filter_text.clone()));
                }
            });
            ui.separator();

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
                        ui.separator();

                        if should_connect {
                            if let Some(id) = conn.id {
                                if let Some(tab) =
                                    tabular.query_tabs.get_mut(tabular.active_tab_index)
                                {
                                    tab.connection_id = Some(id);
                                    if (tab.database_name.is_none()
                                        || tab.database_name.as_deref().unwrap_or("").is_empty())
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
    if !open {
        tabular.show_connection_selector = false;
    }
}
