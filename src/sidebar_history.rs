use log::{debug, error, warn};

use crate::{models, sidebar_database, window_egui};

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
    let max_length = 55;
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

pub(crate) fn load_query_history(tabular: &mut window_egui::Tabular) {
    crate::log_startup_step("sidebar_history::load_query_history started");
    let rt = tabular.get_runtime();
    if let Some(pool) = &tabular.db_pool {
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
                        Some(history_items)
                    }
                    Err(e) => {
                        if sidebar_database::is_sqlite_corrupt(&e) {
                            warn!("⚠️ [load_query_history] SQLite corruption detected when loading history");
                        } else {
                            debug!("Failed to load query history: {}", e);
                        }
                        None
                    }
                }
            });

        if let Some(items) = result {
            tabular.history_items = items;
            crate::log_startup_step(&format!(
                "sidebar_history: loaded {} history items, refreshing tree",
                tabular.history_items.len()
            ));
            refresh_history_tree(tabular);
            crate::log_startup_step("sidebar_history: history tree refreshed");
        } else if let Some(ref pool) = tabular.db_pool {
            crate::log_startup_step(
                "sidebar_history: query_history failed, checking corruption recovery",
            );
            // Test pool health; if corrupt, reset database file while preserving RAM
            let check = rt.block_on(async {
                sqlx::query("SELECT 1 FROM query_history LIMIT 1")
                    .execute(pool.as_ref())
                    .await
            });
            if let Err(e) = check {
                sidebar_database::check_and_recover_sqlite_corruption(tabular, &e);
            }
            crate::log_startup_step("sidebar_history: corruption recovery check finished");
        }
    }
    crate::log_startup_step("sidebar_history::load_query_history finished");
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
    crate::editor_autocomplete::learn_executed_query(tabular, connection_id, trimmed);

    let connection_name = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Connection {}", connection_id));

    let now_str = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // --- RAM upsert: update timestamp + bubble to top if duplicate ---
    if let Some(pos) = tabular
        .history_items
        .iter()
        .position(|h| h.query == trimmed && h.connection_id == connection_id)
    {
        // Update existing entry's timestamp and move it to the front
        tabular.history_items[pos].executed_at = now_str.clone();
        let item = tabular.history_items.remove(pos);
        tabular.history_items.insert(0, item);
        debug!("[save_query_to_history] Duplicate query found — updated timestamp, moved to top");
    } else {
        // New entry: insert at the front
        let new_item = models::structs::HistoryItem {
            id: None,
            query: trimmed.to_string(),
            connection_id,
            connection_name: connection_name.clone(),
            executed_at: now_str.clone(),
        };
        tabular.history_items.insert(0, new_item);
        debug!("[save_query_to_history] New query added to history");
    }
    refresh_history_tree(tabular);

    // --- SQLite upsert: UPDATE if exists, else INSERT ---
    // Daftar di RAM di atas adalah sumber kebenaran untuk UI; penulisan ke
    // SQLite dijalankan di runtime latar supaya UI thread tidak menunggu
    // disk pada setiap query yang selesai.
    if let Some(pool) = &tabular.db_pool {
        let pool = pool.clone();
        let query_text = trimmed.to_string();
        let conn_name = connection_name;
        let now = now_str;

        let rt = tabular.get_runtime();
        rt.spawn(async move {
            // Satu penulisan pada satu waktu: dua simpan beruntun untuk query
            // yang sama tidak boleh sama-sama lolos UPDATE lalu INSERT ganda.
            let _guard = HISTORY_WRITE_LOCK.lock().await;
            match upsert_history_row(&pool, &query_text, connection_id, &conn_name, &now).await {
                Ok((action, rows)) => {
                    debug!("[HISTORY] {} {} row(s) in query_history", action, rows);
                }
                Err(e) if sidebar_database::is_sqlite_corrupt(&e) => {
                    warn!(
                        "[HISTORY] query_history not saved, SQLite corruption detected (recovered on next history load): {}",
                        e
                    );
                }
                Err(e) => {
                    warn!("[HISTORY] failed to save query to history: {}", e);
                }
            }
        });
    } else {
        warn!("⚠️ [save_query_to_history] Cannot save query history: db_pool is None");
    }
}

/// Menjaga urutan penulisan history yang dijalankan di runtime latar.
static HISTORY_WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// UPDATE baris history yang sama (query + koneksi) atau INSERT baris baru,
/// lalu pangkas ke 150 entri terbaru. Mengembalikan aksi dan jumlah baris.
async fn upsert_history_row(
    pool: &sqlx::SqlitePool,
    query_text: &str,
    connection_id: i64,
    connection_name: &str,
    executed_at: &str,
) -> Result<(&'static str, u64), sqlx::Error> {
    // Try to update existing row first
    let updated = sqlx::query(
        "UPDATE query_history SET executed_at = ?, connection_name = ?
         WHERE query_text = ? AND connection_id = ?",
    )
    .bind(executed_at)
    .bind(connection_name)
    .bind(query_text)
    .bind(connection_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() > 0 {
        // Row existed — updated successfully, no INSERT needed
        return Ok(("updated", updated.rows_affected()));
    }

    // No existing row → INSERT new entry
    let inserted = sqlx::query(
        "INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, ?, ?)",
    )
    .bind(query_text)
    .bind(connection_id)
    .bind(connection_name)
    .execute(pool)
    .await?;

    // Clean up old entries beyond the 150 limit
    let _ = sqlx::query(
        "DELETE FROM query_history WHERE id NOT IN (
            SELECT id FROM query_history ORDER BY executed_at DESC LIMIT 150
        )",
    )
    .execute(pool)
    .await;
    Ok(("inserted", inserted.rows_affected()))
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
            hist_node.file_path = Some(format!(
                "{}||{}||{}",
                item.connection_name, item.executed_at, item.query
            ));
            date_node.children.push(hist_node);
        }

        tabular.history_tree.push(date_node);
    }

    // Apply search filter if text is present
    filter_history_tree(tabular);
}

/// Filter history tree based on search text
pub(crate) fn filter_history_tree(tabular: &mut window_egui::Tabular) {
    let search_text = tabular.history_search_text.trim();
    if search_text.is_empty() {
        // Clear filtered tree if search is empty
        tabular.filtered_history_tree.clear();
        return;
    }

    tabular.filtered_history_tree.clear();
    let query = crate::search_match::SearchQuery::new(search_text);

    for date_node in &tabular.history_tree {
        let mut filtered_date_node = date_node.clone();
        filtered_date_node.children.clear();

        // If the date folder itself matches the search text, keep all items in this folder
        let folder_matches = query.matches(&date_node.name);

        if folder_matches {
            filtered_date_node.children = date_node.children.clone();
            filtered_date_node.is_expanded = true;
            tabular.filtered_history_tree.push(filtered_date_node);
        } else {
            for item_node in &date_node.children {
                // Search in query text and connection name
                let connection_name = item_node
                    .connection_id
                    .and_then(|id| {
                        tabular
                            .connections
                            .iter()
                            .find(|c| c.id == Some(id))
                            .map(|c| c.name.clone())
                    })
                    .unwrap_or_default();

                if query.matches_any([item_node.name.as_str(), connection_name.as_str()]) {
                    filtered_date_node.children.push(item_node.clone());
                }
            }

            // Only add date node if it has matching items
            if !filtered_date_node.children.is_empty() {
                filtered_date_node.is_expanded = true;
                tabular.filtered_history_tree.push(filtered_date_node);
            }
        }
    }
}

/// Delete all saved query history rows and reset the in-memory/UI state.
pub(crate) fn clear_query_history(tabular: &mut window_egui::Tabular) {
    let rt = tabular.get_runtime();
    if let Some(pool) = &tabular.db_pool {
        let result = rt.block_on(async {
            sqlx::query("DELETE FROM query_history")
                .execute(pool.as_ref())
                .await
        });

        if let Err(e) = result {
            error!("Failed to clear query history: {}", e);
            tabular
                .toasts
                .error("Failed to clear query history".to_string());
            return;
        }
    }

    tabular.history_items.clear();
    tabular.history_tree.clear();
    tabular.filtered_history_tree.clear();
    tabular.history_search_text.clear();
    tabular.toasts.success("Query history cleared".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::NodeType;
    use crate::models::structs::TreeNode;

    #[test]
    fn test_filter_history_tree() {
        let mut tabular = window_egui::Tabular::default();

        let mut item1 = TreeNode::new("SELECT * FROM users;".to_string(), NodeType::QueryHistItem);
        item1.connection_id = Some(1);

        let mut item2 = TreeNode::new(
            "UPDATE orders SET done = 1;".to_string(),
            NodeType::QueryHistItem,
        );
        item2.connection_id = Some(1);

        let mut today_folder = TreeNode::new("Today".to_string(), NodeType::HistoryDateFolder);
        today_folder.children = vec![item1, item2];

        tabular.history_tree = vec![today_folder];

        // 1. Search for query text "users" -> only matching query is shown
        tabular.history_search_text = "users".to_string();
        filter_history_tree(&mut tabular);
        assert_eq!(tabular.filtered_history_tree.len(), 1);
        assert_eq!(tabular.filtered_history_tree[0].name, "Today");
        assert_eq!(tabular.filtered_history_tree[0].children.len(), 1);
        assert_eq!(
            tabular.filtered_history_tree[0].children[0].name,
            "SELECT * FROM users;"
        );

        // 2. Search for folder name "Today" -> all items in folder should be kept
        tabular.history_search_text = "today".to_string();
        filter_history_tree(&mut tabular);
        assert_eq!(tabular.filtered_history_tree.len(), 1);
        assert_eq!(tabular.filtered_history_tree[0].name, "Today");
        assert!(tabular.filtered_history_tree[0].is_expanded);
        assert_eq!(tabular.filtered_history_tree[0].children.len(), 2);
        assert_eq!(
            tabular.filtered_history_tree[0].children[0].name,
            "SELECT * FROM users;"
        );
        assert_eq!(
            tabular.filtered_history_tree[0].children[1].name,
            "UPDATE orders SET done = 1;"
        );

        // 3. Clear search
        tabular.history_search_text = "".to_string();
        filter_history_tree(&mut tabular);
        assert!(tabular.filtered_history_tree.is_empty());

        // 4. Search with whitespace only -> should treat as empty and clear filtered tree
        tabular.history_search_text = "   ".to_string();
        filter_history_tree(&mut tabular);
        assert!(tabular.filtered_history_tree.is_empty());

        // 5. Search with untrimmed query -> should trim and match correctly
        tabular.history_search_text = "  users  ".to_string();
        filter_history_tree(&mut tabular);
        assert_eq!(tabular.filtered_history_tree.len(), 1);
        assert_eq!(tabular.filtered_history_tree[0].name, "Today");
        assert_eq!(tabular.filtered_history_tree[0].children.len(), 1);
        assert_eq!(
            tabular.filtered_history_tree[0].children[0].name,
            "SELECT * FROM users;"
        );
    }

    #[tokio::test]
    async fn upsert_history_row_updates_instead_of_duplicating() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::query(
            "CREATE TABLE query_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                query_text TEXT NOT NULL,
                connection_id INTEGER NOT NULL,
                connection_name TEXT NOT NULL,
                executed_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
        )
        .execute(&pool)
        .await
        .expect("create table");

        let first = upsert_history_row(&pool, "SELECT 1", 7, "local", "2026-01-01 10:00:00")
            .await
            .expect("insert");
        assert_eq!(first, ("inserted", 1));
        let second = upsert_history_row(&pool, "SELECT 1", 7, "renamed", "2026-01-02 11:00:00")
            .await
            .expect("update");
        assert_eq!(second, ("updated", 1));
        // Koneksi lain = baris lain.
        upsert_history_row(&pool, "SELECT 1", 8, "other", "2026-01-03 12:00:00")
            .await
            .expect("insert other connection");

        let rows: Vec<(String, i64, String, String)> = sqlx::query_as(
            "SELECT query_text, connection_id, connection_name, executed_at
             FROM query_history ORDER BY connection_id",
        )
        .fetch_all(&pool)
        .await
        .expect("select");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].2, "renamed");
        assert_eq!(rows[0].3, "2026-01-02 11:00:00");
        assert_eq!(rows[1].1, 8);
    }
}
