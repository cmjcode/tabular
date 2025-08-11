use log::debug;

use crate::{models, window_egui};

 pub(crate) fn load_query_history(tabular: &mut window_egui::Tabular) {
        if let Some(pool) = &tabular.db_pool {
            let rt = tokio::runtime::Runtime::new().unwrap();
            
            let result = rt.block_on(async {
                match sqlx::query_as::<_, (i64, String, i64, String, String)>(
                    "SELECT id, query_text, connection_id, connection_name, executed_at FROM query_history ORDER BY executed_at DESC LIMIT 100"
                )
                .fetch_all(pool.as_ref())
                .await
                {
                    Ok(rows) => {
                        let mut history_items = Vec::new();
                        for row in rows {
                            history_items.push(models::structs::HistoryItem {
                                id: Some(row.0),
                                query: row.1,
                                connection_id: row.2,
                                connection_name: row.3,
                                executed_at: row.4,
                            });
                        }
                        history_items
                    }
                    Err(e) => {
                        debug!("Failed to load query history: {}", e);
                        Vec::new()
                    }
                }
            });
            
            tabular.history_items = result;
            refresh_history_tree(tabular);
        }
    }

 pub(crate) fn save_query_to_history(tabular: &mut window_egui::Tabular, query: &str, connection_id: i64) {
        if let Some(pool) = &tabular.db_pool {
            if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(connection_id)) {
                let connection_name = connection.name.clone();

                
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let _ = sqlx::query(
                        "INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, ?, ?)"
                    )
                    .bind(query.to_string())
                    .bind(connection_id)
                    .bind(&connection_name)
                    .execute(pool.as_ref())
                    .await;
                    
                    // Clean up old history entries if we have more than 150 entries
                    let _ = sqlx::query(
                        "DELETE FROM query_history WHERE id NOT IN (
                            SELECT id FROM query_history ORDER BY executed_at DESC LIMIT 150
                        )"
                    )
                    .execute(pool.as_ref())
                    .await;
                });
                
                // Reload history to update UI
                load_query_history(tabular);
            }
        }
    }

 pub(crate) fn refresh_history_tree(tabular: &mut window_egui::Tabular) {
        tabular.history_tree.clear();

        // Kelompokkan berdasarkan tanggal (YYYY-MM-DD) dari field executed_at
        use std::collections::BTreeMap; // BTreeMap agar urutan tanggal terjaga (desc nanti kita balik)
        let mut grouped: BTreeMap<String, Vec<&models::structs::HistoryItem>> = BTreeMap::new();

        for item in &tabular.history_items {
            // Ambil 10 pertama (YYYY-MM-DD) jika format standar (2025-08-11T12:34:56Z / 2025-08-11 12:34:56 ...)
            let date_key = if item.executed_at.len() >= 10 { &item.executed_at[0..10] } else { &item.executed_at };
            grouped.entry(date_key.to_string()).or_default().push(item);
        }

        // Iterasi mundur (tanggal terbaru dulu)
        for (date, items) in grouped.iter().rev() {
            let mut date_node = models::structs::TreeNode::new(date.clone(), models::enums::NodeType::HistoryDateFolder);
            date_node.is_expanded = true; // Expand default supaya user langsung lihat isinya

            for item in items {
                let mut hist_node = models::structs::TreeNode::new(item.query.clone(), models::enums::NodeType::QueryHistItem);
                hist_node.connection_id = Some(item.connection_id);
                date_node.children.push(hist_node);
            }

            tabular.history_tree.push(date_node);
        }
    }
