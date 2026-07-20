use log::{debug, error, info, warn};

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
        d if d == today => "Today".to_string(),
        d if d == yesterday => "Yesterday".to_string(),
        _ => {
            // Try to parse the date and format it nicely
            if let Ok(parsed_date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                format!("{}", parsed_date.format("%B %d, %Y"))
            } else {
                date_str.to_string()
            }
        }
    }
}

fn is_sqlite_corrupt(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        if db_err.code().is_some_and(|c| c.as_ref() == "11") {
            return true;
        }
        let msg = db_err.message().to_lowercase();
        return msg.contains("malformed") || msg.contains("disk image is malformed") || msg.contains("corrupt");
    }
    false
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
                        if is_sqlite_corrupt(&e) {
                            warn!("⚠️ [load_query_history] SQLite corruption detected when loading history — attempting table repair");
                            let _ = sqlx::query("VACUUM").execute(pool.as_ref()).await;
                            let _ = sqlx::query(
                                r#"
                                CREATE TABLE IF NOT EXISTS query_history (
                                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    query_text TEXT NOT NULL,
                                    connection_id INTEGER NOT NULL,
                                    connection_name TEXT NOT NULL,
                                    executed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                                    FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE
                                )
                                "#
                            )
                            .execute(pool.as_ref())
                            .await;
                        } else {
                            debug!("Failed to load query history: {}", e);
                        }
                        Vec::new()
                    }
                }
            });

        tabular.history_items = result;
        refresh_history_tree(tabular);
    }
}

async fn recover_and_retry_insert(
    pool: &sqlx::SqlitePool,
    query_text: &str,
    connection_id: i64,
    conn_name: &str,
) {
    warn!("⚠️ [save_query_to_history] SQLite database corruption detected — attempting recovery");

    // 1. Try VACUUM first to repair corrupted WAL / pages
    if sqlx::query("VACUUM").execute(pool).await.is_ok() {
        debug!("[save_query_to_history] VACUUM completed successfully");
    }

    // 2. Drop and recreate query_history table to repair corrupt table structure/indexes
    warn!("[save_query_to_history] Recreating query_history table to clear corrupted database image...");
    let _ = sqlx::query("DROP TABLE IF EXISTS query_history").execute(pool).await;
    let _ = sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS query_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            query_text TEXT NOT NULL,
            connection_id INTEGER NOT NULL,
            connection_name TEXT NOT NULL,
            executed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE
        )
        "#
    )
    .execute(pool)
    .await;

    // 3. Retry inserting query into history
    match sqlx::query(
        "INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, ?, ?)"
    )
    .bind(query_text)
    .bind(connection_id)
    .bind(conn_name)
    .execute(pool)
    .await
    {
        Ok(res) => {
            info!(
                "✅ [save_query_to_history] Recovery successful! Recorded query into SQLite (rows_affected={}): '{}'",
                res.rows_affected(),
                query_text
            );
        }
        Err(e) => {
            error!(
                "❌ [save_query_to_history] Insert failed even after SQLite corruption recovery attempt: {}",
                e
            );
        }
    }
}

pub(crate) fn save_query_to_history(
    tabular: &mut window_egui::Tabular,
    query: &str,
    connection_id: i64,
) {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        debug!("[save_query_to_history] Skipping empty query string");
        return;
    }

    let connection_name = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Connection {}", connection_id));

    if let Some(pool) = &tabular.db_pool {
        let pool = pool.clone();
        let query_text = trimmed.to_string();
        let conn_name = connection_name.clone();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            match sqlx::query(
                "INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, ?, ?)"
            )
            .bind(&query_text)
            .bind(connection_id)
            .bind(&conn_name)
            .execute(pool.as_ref())
            .await
            {
                Ok(res) => {
                    info!(
                        "✅ [save_query_to_history] Successfully recorded query into SQLite (rows_affected={}): '{}'",
                        res.rows_affected(),
                        query_text
                    );
                }
                Err(e) => {
                    if is_sqlite_corrupt(&e) {
                        recover_and_retry_insert(pool.as_ref(), &query_text, connection_id, &conn_name).await;
                    } else {
                        error!(
                            "❌ [save_query_to_history] Failed to insert query into SQLite query_history: {}",
                            e
                        );
                    }
                }
            }

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
    } else {
        warn!("⚠️ [save_query_to_history] Cannot save query history: db_pool is None");
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
            // Store connection info, timestamp, and original query in file_path field
            // Format: "connection_name||executed_at||original_query"
            hist_node.file_path = Some(format!("{}||{}||{}", item.connection_name, item.executed_at, item.query));
            date_node.children.push(hist_node);
        }

        tabular.history_tree.push(date_node);
    }

    // Apply search filter if text is present
    filter_history_tree(tabular);
}

/// Filter history tree based on search text
pub(crate) fn filter_history_tree(tabular: &mut window_egui::Tabular) {
    if tabular.history_search_text.is_empty() {
        // Clear filtered tree if search is empty
        tabular.filtered_history_tree.clear();
        return;
    }

    tabular.filtered_history_tree.clear();
    let search_lower = tabular.history_search_text.to_lowercase();

    for date_node in &tabular.history_tree {
        let mut filtered_date_node = date_node.clone();
        filtered_date_node.children.clear();

        for item_node in &date_node.children {
            // Search in query text and connection name
            let query_text = item_node.name.to_lowercase();
            let connection_name = item_node
                .connection_id
                .and_then(|id| {
                    tabular
                        .connections
                        .iter()
                        .find(|c| c.id == Some(id))
                        .map(|c| c.name.to_lowercase())
                })
                .unwrap_or_default();

            if query_text.contains(&search_lower) || connection_name.contains(&search_lower) {
                filtered_date_node.children.push(item_node.clone());
            }
        }

        // Only add date node if it has matching items
        if !filtered_date_node.children.is_empty() {
            tabular.filtered_history_tree.push(filtered_date_node);
        }
    }
}
