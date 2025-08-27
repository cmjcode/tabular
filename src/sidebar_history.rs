use log::debug;

use crate::{models, window_egui};

/// Format query text for display in the sidebar history
fn format_query_for_sidebar(query: &str, _connection_name: &str) -> String {
    // Remove extra whitespace and newlines
    let cleaned_query = query
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with("--"))
        .collect::<Vec<_>>()
        .join(" ");

    // Truncate if too long, with ellipsis
    let max_length = 80;
    if cleaned_query.len() > max_length {
        format!("{}...", &cleaned_query[0..max_length].trim())
    } else {
        cleaned_query
    }
}

/// Format date for better display in history folders
fn format_date_for_display(date_str: &str) -> String {
    // Check if it's today's date
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let yesterday = (chrono::Local::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    match date_str {
        d if d == today => "📅 Today".to_string(),
        d if d == yesterday => "📅 Yesterday".to_string(),
        _ => {
            // Try to parse the date and format it nicely
            if let Ok(parsed_date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                format!("📅 {}", parsed_date.format("%B %d, %Y"))
            } else {
                format!("📅 {}", date_str)
            }
        }
    }
}

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

pub(crate) fn save_query_to_history(
    tabular: &mut window_egui::Tabular,
    query: &str,
    connection_id: i64,
) {
    if let Some(pool) = &tabular.db_pool
        && let Some(connection) = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
    {
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

pub(crate) fn refresh_history_tree(tabular: &mut window_egui::Tabular) {
    tabular.history_tree.clear();

    // Kelompokkan berdasarkan tanggal (YYYY-MM-DD) dari field executed_at
    use std::collections::BTreeMap; // BTreeMap agar urutan tanggal terjaga (desc nanti kita balik)
    let mut grouped: BTreeMap<String, Vec<&models::structs::HistoryItem>> = BTreeMap::new();

    for item in &tabular.history_items {
        // Ambil 10 pertama (YYYY-MM-DD) jika format standar (2025-08-11T12:34:56Z / 2025-08-11 12:34:56 ...)
        let date_key = if item.executed_at.len() >= 10 {
            &item.executed_at[0..10]
        } else {
            &item.executed_at
        };
        grouped.entry(date_key.to_string()).or_default().push(item);
    }

    // Iterasi mundur (tanggal terbaru dulu)
    for (date, items) in grouped.iter().rev() {
        // Format date for better display
        let formatted_date = format_date_for_display(date);
        let mut date_node = models::structs::TreeNode::new(
            formatted_date,
            models::enums::NodeType::HistoryDateFolder,
        );
        date_node.is_expanded = true; // Expand default supaya user langsung lihat isinya

        for item in items {
            // Format query for better display in sidebar
            let formatted_query = format_query_for_sidebar(&item.query, &item.connection_name);
            let mut hist_node = models::structs::TreeNode::new(
                formatted_query,
                models::enums::NodeType::QueryHistItem,
            );
            hist_node.connection_id = Some(item.connection_id);
            // Store original query and connection info in file_path field (we'll use this to identify the actual query)
            // Format: "connection_name||original_query" for easy parsing
            hist_node.file_path = Some(format!("{}||{}", item.connection_name, item.query));
            date_node.children.push(hist_node);
        }

        tabular.history_tree.push(date_node);
    }
}
