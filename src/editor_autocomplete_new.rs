//! Glue autocomplete SQL: menghubungkan engine murni (`crate::autocomplete`)
//! dengan cache metadata `Tabular` dan popup egui.
//!
//! Alur per keystroke:
//! 1. `analyze` menentukan klausa, `Expect`, dan scope di posisi kursor.
//! 2. Metadata tabel/kolom/FK yang dibutuhkan diambil dari cache in-memory
//!    (cache miss memicu warming di background — tidak pernah blocking).
//! 3. `complete` menghasilkan kandidat terurut; hasilnya disalin ke state popup.
use crate::autocomplete::{self, Catalog, ColumnMeta, Dialect, Expect, ItemKind};
use crate::models::enums::AutocompleteKind;
use crate::query_tools;
use crate::window_egui::Tabular;
use eframe::egui;

use std::collections::{HashMap, HashSet};

/// Keyword cadangan untuk tab non-SQL saat autocomplete dipanggil manual.
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "DROP", "ALTER", "ADD", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP",
    "BY", "ORDER", "LIMIT", "OFFSET", "AND", "OR", "NOT", "NULL", "AS", "DISTINCT", "COUNT", "SUM",
    "AVG", "MIN", "MAX", "LIKE", "IN", "IS", "BETWEEN", "UNION", "ALL",
];

fn current_prefix(text: &str, cursor: usize) -> (String, usize) {
    if text.is_empty() {
        return (String::new(), cursor);
    }
    let bytes = text.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_alphanumeric() || matches!(c, '_' | ':' | '@' | '$' | '.') {
            start -= 1;
        } else {
            break;
        }
    }
    (text[start..cursor.min(text.len())].to_string(), start)
}

fn active_connection_and_db(app: &Tabular) -> Option<(i64, String)> {
    app.query_tabs.get(app.active_tab_index).and_then(|tab| {
        tab.connection_id
            .map(|cid| (cid, tab.database_name.clone().unwrap_or_default()))
    })
}
fn collect_tables_from_tree(
    nodes: &[crate::models::structs::TreeNode],
    target_cid: Option<i64>,
    target_db: Option<&str>,
    out: &mut Vec<String>,
) {
    for node in nodes {
        let matches_conn = target_cid.is_none()
            || node.connection_id.is_none()
            || node.connection_id == target_cid;
        let matches_db = target_db.is_none()
            || node.database_name.is_none()
            || node.database_name.as_deref() == target_db;
        if (node.node_type == crate::models::enums::NodeType::Table
            || node.node_type == crate::models::enums::NodeType::View)
            && matches_conn
            && matches_db
        {
            if !node.name.is_empty() && !out.contains(&node.name) {
                out.push(node.name.clone());
            }
        }
        collect_tables_from_tree(&node.children, target_cid, target_db, out);
    }
}

fn collect_columns_from_tree(
    nodes: &[crate::models::structs::TreeNode],
    target_cid: i64,
    target_table: &str,
    out: &mut Vec<String>,
) {
    for node in nodes {
        if (node.connection_id.is_none() || node.connection_id == Some(target_cid))
            && (node.node_type == crate::models::enums::NodeType::Table
                || node.node_type == crate::models::enums::NodeType::View)
            && node.name.eq_ignore_ascii_case(target_table)
        {
            for child in &node.children {
                if child.node_type == crate::models::enums::NodeType::Column {
                    if !child.name.is_empty() && !out.contains(&child.name) {
                        out.push(child.name.clone());
                    }
                }
            }
            return;
        }
        collect_columns_from_tree(&node.children, target_cid, target_table, out);
    }
}

fn get_cached_tables(app: &Tabular, cid: i64, db: &str) -> Option<Vec<String>> {
    // 1. In-memory check first (0ms, non-blocking)
    if let Some(tbls) = app.autocomplete_tables_mem.get(&(cid, db.to_string())) {
        if !tbls.is_empty() {
            return Some(tbls.clone());
        }
    }
    if !db.is_empty() {
        if let Some(tbls) = app.autocomplete_tables_mem.get(&(cid, String::new())) {
            if !tbls.is_empty() {
                return Some(tbls.clone());
            }
        }
    }

    // 2. Extract from in-memory items_tree without I/O
    let mut tree_tables = Vec::new();
    collect_tables_from_tree(
        &app.items_tree,
        Some(cid),
        if db.is_empty() { None } else { Some(db) },
        &mut tree_tables,
    );
    if !tree_tables.is_empty() {
        tree_tables.sort_unstable();
        tree_tables.dedup();
        return Some(tree_tables);
    }

    // 3. Lazy background warm from SQLite table_cache (non-blocking)
    if let (Some(rt), Some(db_pool)) = (app.runtime.clone(), app.db_pool.clone()) {
        let warm_tx = app.autocomplete_warm_sender.clone();
        let db_name = db.to_string();
        rt.spawn(async move {
            let rows = if db_name.is_empty() {
                sqlx::query_as::<_, (String,)>(
                    "SELECT DISTINCT table_name FROM table_cache WHERE connection_id = ? AND table_type IN ('table', 'view') ORDER BY table_name",
                )
                .bind(cid)
                .fetch_all(db_pool.as_ref())
                .await
            } else {
                sqlx::query_as::<_, (String,)>(
                    "SELECT table_name FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type IN ('table', 'view') ORDER BY table_name",
                )
                .bind(cid)
                .bind(&db_name)
                .fetch_all(db_pool.as_ref())
                .await
            };
            if let Ok(rows) = rows {
                let tables: Vec<String> = rows.into_iter().map(|(t,)| t).collect();
                if !tables.is_empty() {
                    let _ = warm_tx.send(crate::window_egui::AutocompleteWarmResult::Tables {
                        connection_id: cid,
                        database_name: db_name,
                        tables,
                    });
                }
            }
        });
    }

    None
}

pub(crate) fn get_all_tables(app: &Tabular) -> Vec<String> {
    // 1. In-memory check
    let mut all = Vec::new();
    for tbls in app.autocomplete_tables_mem.values() {
        for t in tbls {
            if !all.contains(t) {
                all.push(t.clone());
            }
        }
    }
    if !all.is_empty() {
        all.sort_unstable();
        all.dedup();
        return all;
    }

    // 2. Extract from in-memory items_tree
    collect_tables_from_tree(&app.items_tree, None, None, &mut all);
    if !all.is_empty() {
        all.sort_unstable();
        all.dedup();
        return all;
    }

    // 3. Lazy background warm (non-blocking)
    if let (Some(rt), Some(db_pool)) = (app.runtime.clone(), app.db_pool.clone()) {
        let warm_tx = app.autocomplete_warm_sender.clone();
        rt.spawn(async move {
            if let Ok(rows) = sqlx::query_as::<_, (i64, String, String)>(
                "SELECT connection_id, database_name, table_name FROM table_cache WHERE table_type IN ('table', 'view') ORDER BY table_name",
            )
            .fetch_all(db_pool.as_ref())
            .await {
                let mut map: std::collections::HashMap<(i64, String), Vec<String>> = std::collections::HashMap::new();
                for (cid, db, tbl) in rows {
                    map.entry((cid, db)).or_default().push(tbl);
                }
                for ((cid, db), tables) in map {
                    let _ = warm_tx.send(crate::window_egui::AutocompleteWarmResult::Tables {
                        connection_id: cid,
                        database_name: db,
                        tables,
                    });
                }
            }
        });
    }

    all
}

/// Kolom satu tabel dari cache in-memory (urutan ordinal dipertahankan).
/// Cache miss memicu warming di background; hasilnya tersedia di keystroke berikutnya.
fn get_cached_columns(app: &mut Tabular, cid: i64, db: &str, table: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for t in [table.to_string()] {
        let key = (cid, t.to_ascii_lowercase());
        if let Some(cols) = app.autocomplete_cols_mem.get(&key) {
            for c in cols {
                if !out.contains(c) {
                    out.push(c.clone());
                }
            }
            continue;
        }

        // 2) Check if columns are already in items_tree
        let mut tree_cols = Vec::new();
        collect_columns_from_tree(&app.items_tree, cid, &t, &mut tree_cols);
        if !tree_cols.is_empty() {
            for c in &tree_cols {
                if !out.contains(c) {
                    out.push(c.clone());
                }
            }
            app.autocomplete_cols_mem.insert(key.clone(), tree_cols);
            continue;
        }

        // 3) If missing in memory: mark warmed and insert placeholder immediately to prevent repeated lookups
        if !app.autocomplete_cols_warmed.contains(&key) {
            app.autocomplete_cols_warmed.insert(key.clone());
            app.autocomplete_cols_mem.entry(key.clone()).or_default();

            if let (Some(rt), Some(db_pool)) = (app.runtime.clone(), app.db_pool.clone()) {
                let warm_tx = app.autocomplete_warm_sender.clone();
                let t_clone = t.clone();
                let db_clone = db.to_string();
                let conn_opt = app.connections.iter().find(|c| c.id == Some(cid)).cloned();
                let pool_opt = app.shared_db_pool.read().ok().and_then(|g| g.clone());

                rt.spawn(async move {
                    // Query SQLite column_cache in background
                    let cached = if db_clone.is_empty() {
                        sqlx::query_as::<_, (String, String)>(
                            "SELECT column_name, data_type FROM column_cache WHERE connection_id = ? AND table_name = ? COLLATE NOCASE ORDER BY ordinal_position",
                        )
                        .bind(cid)
                        .bind(&t_clone)
                        .fetch_all(db_pool.as_ref())
                        .await
                    } else {
                        sqlx::query_as::<_, (String, String)>(
                            "SELECT column_name, data_type FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? COLLATE NOCASE ORDER BY ordinal_position",
                        )
                        .bind(cid)
                        .bind(&db_clone)
                        .bind(&t_clone)
                        .fetch_all(db_pool.as_ref())
                        .await
                    };

                    let mut cols: Vec<(String, String)> = match cached {
                        Ok(rows) if !rows.is_empty() => rows,
                        _ => Vec::new(),
                    };

                    // If SQLite had nothing and we have live connection, fetch live
                    if cols.is_empty() && let (Some(conn), Some(_pool)) = (conn_opt, pool_opt) {
                        let try_dbs: Vec<String> = if db_clone.is_empty() {
                            vec![conn.database.clone()]
                        } else {
                            vec![db_clone.clone()]
                        };
                        for table_db in &try_dbs {
                            if let Some(fetched) =
                                crate::connection::fetch_columns_from_database(cid, table_db, &t_clone, &conn)
                                && !fetched.is_empty()
                            {
                                if let Ok(mut tx) = db_pool.begin().await {
                                    for (i, (column_name, data_type)) in fetched.iter().enumerate() {
                                        let _ = sqlx::query(
                                            "INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)",
                                        )
                                        .bind(cid)
                                        .bind(table_db)
                                        .bind(&t_clone)
                                        .bind(column_name)
                                        .bind(data_type)
                                        .bind(i as i64)
                                        .execute(&mut *tx)
                                        .await;
                                    }
                                    let _ = tx.commit().await;
                                }
                                cols = fetched;
                                break;
                            }
                        }
                    }

                    if !cols.is_empty() {
                        let col_names: Vec<String> = cols.iter().map(|(c, _)| c.clone()).collect();
                        let _ = warm_tx.send(crate::window_egui::AutocompleteWarmResult::Columns {
                            connection_id: cid,
                            table_name: t_clone.to_ascii_lowercase(),
                            columns: col_names,
                            types: cols,
                        });
                    }
                });
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Collect ForeignKey metadata for autocomplete. Prefers in-memory
/// `autocomplete_fks_mem` for the active connection (instant 0ms response);
/// lazily warms that cache once per connection per session in background
/// without freezing the UI thread; finally falls back to any FK data already
/// loaded into an open ERD diagram.
fn collect_loaded_fks(app: &mut Tabular) -> Vec<crate::models::structs::ForeignKey> {
    if let Some((cid, db)) = active_connection_and_db(app) {
        // 1. In-memory check (0ms, non-blocking)
        if let Some(fks) = app.autocomplete_fks_mem.get(&(cid, db.clone())) {
            if !fks.is_empty() {
                return fks.clone();
            }
        }
        if !db.is_empty() {
            if let Some(fks) = app.autocomplete_fks_mem.get(&(cid, String::new())) {
                if !fks.is_empty() {
                    return fks.clone();
                }
            }
        }

        // 2. Lazy one-shot warm in background without freezing the UI thread
        if !app.fk_cache_warmed.contains(&cid) {
            app.fk_cache_warmed.insert(cid);
            let warm_tx = app.autocomplete_warm_sender.clone();
            if let (Some(rt), Some(pool), Some(db_pool)) = (
                app.runtime.clone(),
                app.connection_pools.get(&cid).cloned(),
                app.db_pool.clone(),
            ) {
                let db_name = db.clone();
                rt.spawn(async move {
                    // Try reading from SQLite cache first in background
                    let cached_fks = if db_name.is_empty() {
                        sqlx::query_as::<_, (String, String, String, String, String)>(
                            "SELECT table_name, column_name, referenced_table_name, referenced_column_name, constraint_name FROM foreign_key_cache WHERE connection_id = ?",
                        )
                        .bind(cid)
                        .fetch_all(db_pool.as_ref())
                        .await
                    } else {
                        sqlx::query_as::<_, (String, String, String, String, String)>(
                            "SELECT table_name, column_name, referenced_table_name, referenced_column_name, constraint_name FROM foreign_key_cache WHERE connection_id = ? AND database_name = ?",
                        )
                        .bind(cid)
                        .bind(&db_name)
                        .fetch_all(db_pool.as_ref())
                        .await
                    };

                    let mut keys: Vec<crate::models::structs::ForeignKey> = match cached_fks {
                        Ok(rows) if !rows.is_empty() => rows
                            .into_iter()
                            .map(|(table_name, column_name, referenced_table_name, referenced_column_name, constraint_name)| {
                                crate::models::structs::ForeignKey {
                                    constraint_name,
                                    table_name,
                                    column_name,
                                    referenced_table_name,
                                    referenced_column_name,
                                }
                            })
                            .collect(),
                        _ => Vec::new(),
                    };

                    // If SQLite cache had nothing, fetch live from driver
                    if keys.is_empty() {
                        match pool {
                            crate::models::enums::DatabasePool::MySQL(p) => {
                                if let Ok(k) = crate::driver_mysql::fetch_mysql_foreign_keys(&p, &db_name).await {
                                    keys = k;
                                }
                            }
                            crate::models::enums::DatabasePool::PostgreSQL(p) => {
                                if let Ok(k) = crate::driver_postgres::fetch_postgres_foreign_keys(&p).await {
                                    keys = k;
                                }
                            }
                            crate::models::enums::DatabasePool::SQLite(p) => {
                                if let Ok(k) = crate::driver_sqlite::fetch_sqlite_foreign_keys(&p).await {
                                    keys = k;
                                }
                            }
                            _ => {}
                        }

                        // Write to SQLite cache in a single atomic transaction
                        if !keys.is_empty() {
                            if let Ok(mut tx) = db_pool.begin().await {
                                let _ = sqlx::query(
                                    "DELETE FROM foreign_key_cache WHERE connection_id = ? AND database_name = ?",
                                )
                                .bind(cid)
                                .bind(&db_name)
                                .execute(&mut *tx)
                                .await;

                                for fk in &keys {
                                    let _ = sqlx::query(
                                        "INSERT OR REPLACE INTO foreign_key_cache (connection_id, database_name, table_name, column_name, referenced_table_name, referenced_column_name, constraint_name) VALUES (?, ?, ?, ?, ?, ?, ?)",
                                    )
                                    .bind(cid)
                                    .bind(&db_name)
                                    .bind(&fk.table_name)
                                    .bind(&fk.column_name)
                                    .bind(&fk.referenced_table_name)
                                    .bind(&fk.referenced_column_name)
                                    .bind(&fk.constraint_name)
                                    .execute(&mut *tx)
                                    .await;
                                }
                                let _ = tx.commit().await;
                            }
                        }
                    }

                    // Send back to main UI thread
                    let _ = warm_tx.send(crate::window_egui::AutocompleteWarmResult::ForeignKeys {
                        connection_id: cid,
                        database_name: db_name,
                        keys,
                    });
                });
            }
        }
    }

    // 3. Fallback: FKs from any open diagram state (already in memory).
    app.query_tabs
        .iter()
        .filter_map(|tab| tab.diagram_state.as_ref())
        .flat_map(|ds| ds.nodes.iter())
        .flat_map(|n| n.foreign_keys.iter().cloned())
        .collect()
}

/// Katalog metadata dari cache `Tabular` untuk satu kali pemanggilan engine.
struct AppCatalog {
    tables: Vec<String>,
    /// Key: nama tabel lowercase.
    columns: HashMap<String, Vec<ColumnMeta>>,
    fks: Vec<crate::models::structs::ForeignKey>,
    usage: HashMap<String, u32>,
}

impl Catalog for AppCatalog {
    fn tables(&self) -> &[String] {
        &self.tables
    }

    fn columns(&self, table: &str) -> Option<&[ColumnMeta]> {
        self.columns
            .get(&table.to_ascii_lowercase())
            .map(|v| v.as_slice())
    }

    fn foreign_keys(&self) -> &[crate::models::structs::ForeignKey] {
        &self.fks
    }

    fn usage(&self, label: &str) -> u32 {
        self.usage.get(label).copied().unwrap_or(0)
    }
}

/// Koneksi aktif: milik tab dulu, lalu koneksi global aplikasi.
fn active_connection(app: &Tabular) -> (Option<i64>, String) {
    let cid = app
        .query_tabs
        .get(app.active_tab_index)
        .and_then(|t| t.connection_id)
        .or(app.current_connection_id);
    let db = active_connection_and_db(app)
        .map(|(_, d)| d)
        .unwrap_or_default();
    (cid, db)
}

/// Dialek SQL koneksi; `None` untuk koneksi non-SQL (Redis, MongoDB, HTTP).
fn dialect_for(app: &Tabular, cid: Option<i64>) -> Option<Dialect> {
    use crate::models::enums::DatabaseType;
    let Some(conn) = cid.and_then(|cid| app.connections.iter().find(|c| c.id == Some(cid))) else {
        return Some(Dialect::Generic);
    };
    match conn.connection_type {
        DatabaseType::MySQL => Some(Dialect::MySql),
        DatabaseType::PostgreSQL => Some(Dialect::Postgres),
        DatabaseType::SQLite => Some(Dialect::Sqlite),
        DatabaseType::MsSQL => Some(Dialect::MsSql),
        DatabaseType::Redis | DatabaseType::MongoDB | DatabaseType::ApiHttp => None,
    }
}

/// Kumpulkan metadata yang dibutuhkan hasil analisis (hanya dari memori).
fn build_catalog(
    app: &mut Tabular,
    cid: Option<i64>,
    db: &str,
    analysis: &autocomplete::Analysis,
) -> AppCatalog {
    let tables = match cid {
        Some(c) => get_cached_tables(app, c, db).unwrap_or_else(|| get_all_tables(app)),
        None => get_all_tables(app),
    };
    let mut columns = HashMap::new();
    if let Some(c) = cid {
        let qualifier = analysis.qualifier.last().map(|s| s.to_ascii_lowercase());
        for t in analysis.referenced_tables() {
            // Qualifier yang bukan tabel di scope hanya dimuat bila memang nama tabel
            // yang dikenal — hindari fetch ke DB untuk nama schema atau typo.
            let qualifier_only = qualifier.as_deref() == Some(t.as_str())
                && !analysis
                    .scope
                    .iter()
                    .any(|s| s.name.eq_ignore_ascii_case(&t));
            if qualifier_only && !tables.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                continue;
            }
            if let Some(names) = get_cached_columns(app, c, db, &t) {
                let metas = names
                    .into_iter()
                    .map(|name| {
                        let data_type = app
                            .autocomplete_col_types_mem
                            .get(&(c, t.clone(), name.to_ascii_lowercase()))
                            .cloned();
                        ColumnMeta { name, data_type }
                    })
                    .collect();
                columns.insert(t, metas);
            }
        }
    }
    let fks = if cid.is_some() {
        collect_loaded_fks(app)
    } else {
        Vec::new()
    };
    AppCatalog {
        tables,
        columns,
        fks,
        usage: app.autocomplete_usage.clone(),
    }
}

fn map_kind(kind: ItemKind) -> AutocompleteKind {
    match kind {
        ItemKind::Table | ItemKind::Cte => AutocompleteKind::Table,
        ItemKind::Column => AutocompleteKind::Column,
        ItemKind::Alias => AutocompleteKind::Alias,
        ItemKind::Keyword | ItemKind::Value => AutocompleteKind::Syntax,
        ItemKind::Operator => AutocompleteKind::Operator,
        ItemKind::Function => AutocompleteKind::Function,
        ItemKind::JoinCondition => AutocompleteKind::Join,
        ItemKind::Template => AutocompleteKind::Snippet,
    }
}

fn kind_icon(kind: Option<AutocompleteKind>) -> &'static str {
    match kind {
        Some(AutocompleteKind::Table) => "📦",
        Some(AutocompleteKind::Column) => "🏷️",
        Some(AutocompleteKind::Syntax) => "⚡",
        Some(AutocompleteKind::Function) => "🧩",
        Some(AutocompleteKind::Snippet) => "📄",
        Some(AutocompleteKind::Parameter) => "🔧",
        Some(AutocompleteKind::Join) => "🔗",
        Some(AutocompleteKind::Alias) => "🔖",
        Some(AutocompleteKind::Operator) => "=",
        None => "•",
    }
}

fn clear_popup(app: &mut Tabular) {
    app.show_autocomplete = false;
    app.autocomplete_suggestions.clear();
    app.autocomplete_kinds.clear();
    app.autocomplete_notes.clear();
    app.autocomplete_payloads.clear();
    app.autocomplete_prefix.clear();
    app.last_autocomplete_trigger_len = 0;
}

/// Terapkan hasil warming metadata dari background ke cache in-memory.
fn drain_warm_results(app: &mut Tabular) {
    let Some(rx) = app.autocomplete_warm_receiver.as_ref() else {
        return;
    };
    let results: Vec<_> = rx.try_iter().collect();
    for res in results {
        match res {
            crate::window_egui::AutocompleteWarmResult::ForeignKeys {
                connection_id,
                database_name,
                keys,
            } => {
                app.autocomplete_fks_mem
                    .insert((connection_id, database_name), keys);
            }
            crate::window_egui::AutocompleteWarmResult::Columns {
                connection_id,
                table_name,
                columns,
                types,
            } => {
                app.autocomplete_cols_mem
                    .insert((connection_id, table_name.clone()), columns);
                for (cn, ct) in types {
                    app.autocomplete_col_types_mem.insert(
                        (connection_id, table_name.clone(), cn.to_ascii_lowercase()),
                        ct,
                    );
                }
            }
            crate::window_egui::AutocompleteWarmResult::Tables {
                connection_id,
                database_name,
                tables,
            } => {
                app.autocomplete_tables_mem
                    .insert((connection_id, database_name), tables);
            }
        }
    }
}

pub fn update_autocomplete(app: &mut Tabular) {
    drain_warm_results(app);

    // Throttle supaya tidak bekerja berat di setiap keystroke
    let now = std::time::Instant::now();
    if let Some(last) = app.autocomplete_last_update
        && now.saturating_duration_since(last)
            < std::time::Duration::from_millis(app.autocomplete_debounce_ms)
    {
        return;
    }
    app.autocomplete_last_update = Some(now);
    refresh(app, false);
}

/// Hitung ulang saran di posisi kursor. `force` (Ctrl+Space) melewati aturan
/// pemicu otomatis dan jatuh ke saran umum bila konteks tidak menghasilkan apa pun.
fn refresh(app: &mut Tabular, force: bool) {
    let text = app.editor.text.clone();
    let mut cursor = app.cursor_position.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (pref, pref_start) = current_prefix(&text, cursor);
    let prev_char = text[..cursor].chars().next_back();

    if !force && matches!(prev_char, Some(';') | Some('*')) {
        clear_popup(app);
        return;
    }
    // Saran untuk prefix ini sudah tampil
    if !force
        && app.show_autocomplete
        && app.autocomplete_prefix == pref
        && app.last_autocomplete_trigger_len == pref.len()
    {
        return;
    }

    let (cid, db) = active_connection(app);
    let dialect = dialect_for(app, cid);
    let mut analysis = dialect.map(|d| autocomplete::analyze(&text, cursor, d));

    if !force {
        let expect = analysis.as_ref().map(|a| &a.expect);
        let triggered = if pref.is_empty() {
            // Tanpa prefix: hanya setelah spasi/koma/kurung di posisi yang jelas butuh
            // tabel atau kolom (mis. `FROM |`, `WHERE |`, `ON |`, `SELECT a, |`).
            let soft_boundary =
                matches!(prev_char, Some(c) if c.is_whitespace() || c == ',' || c == '(');
            soft_boundary
                && matches!(
                    expect,
                    Some(Expect::Table | Expect::Column | Expect::JoinCondition)
                )
        } else {
            let before_prefix = text[..pref_start].chars().next_back();
            pref.contains('.')
                || pref.len() >= 2
                || pref.starts_with([':', '@', '$'])
                || matches!(before_prefix, Some(c) if c.is_whitespace())
        };
        if !triggered {
            clear_popup(app);
            return;
        }
    }

    let casing = app.advanced_editor.keyword_casing;
    let mut items = Vec::new();
    if let (Some(d), Some(a)) = (dialect, analysis.as_mut()) {
        let cat = build_catalog(app, cid, &db, a);
        let opts = autocomplete::Options { dialect: d, casing };
        items = autocomplete::complete(a, &cat, opts);
        if items.is_empty() && force && a.expect != Expect::None {
            a.expect = Expect::Generic;
            items = autocomplete::complete(a, &cat, opts);
        }
    }

    let mut labels = Vec::new();
    let mut kinds = Vec::new();
    let mut notes = Vec::new();
    let mut payloads = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push =
        |label: String, kind: AutocompleteKind, note: Option<String>, payload: Option<String>| {
            if seen.insert(label.clone()) {
                labels.push(label);
                kinds.push(kind);
                notes.push(note);
                payloads.push(payload);
            }
        };

    for it in items {
        let payload = (it.insert != it.label).then_some(it.insert);
        push(it.label, map_kind(it.kind), it.detail, payload);
    }

    for param in query_tools::parameter_candidates(&pref) {
        push(
            param.label.to_string(),
            AutocompleteKind::Parameter,
            Some(param.note.to_string()),
            Some(param.template.to_string()),
        );
    }

    // Snippet hanya saat user mengetik kata di posisi "awal klausa"
    let expect = analysis.as_ref().map(|a| a.expect.clone());
    let snippet_ok = !pref.is_empty()
        && !pref.contains('.')
        && analysis.as_ref().is_some_and(|a| a.qualifier.is_empty())
        && matches!(
            expect,
            Some(
                Expect::StatementStart
                    | Expect::AfterTable { .. }
                    | Expect::AfterExpr
                    | Expect::AfterSelectItem
                    | Expect::Column
                    | Expect::Generic
            )
        );
    if snippet_ok {
        let snippet_context = match analysis.as_ref().map(|a| a.clause) {
            Some(autocomplete::Clause::SelectList) => query_tools::SnippetContext::SelectList,
            Some(autocomplete::Clause::From) => query_tools::SnippetContext::FromClause,
            Some(
                autocomplete::Clause::Where
                | autocomplete::Clause::JoinOn
                | autocomplete::Clause::Having,
            ) => query_tools::SnippetContext::WhereClause,
            _ => query_tools::SnippetContext::Any,
        };
        for snippet in query_tools::snippet_candidates(&pref, snippet_context) {
            push(
                snippet.label.to_string(),
                AutocompleteKind::Snippet,
                Some(snippet.note.to_string()),
                Some(snippet.template.to_string()),
            );
        }
    }

    // Tab non-SQL yang dipanggil manual: keyword dasar sebagai cadangan
    if dialect.is_none() && force {
        let pl = pref.to_ascii_lowercase();
        for kw in SQL_KEYWORDS
            .iter()
            .filter(|k| k.to_ascii_lowercase().starts_with(&pl))
        {
            let s = match casing {
                crate::models::enums::KeywordCasing::Lower => kw.to_ascii_lowercase(),
                _ => kw.to_string(),
            };
            push(s, AutocompleteKind::Syntax, Some("keyword".into()), None);
        }
    }

    if labels.is_empty() {
        clear_popup(app);
        return;
    }
    app.autocomplete_suggestions = labels;
    app.autocomplete_kinds = kinds;
    app.autocomplete_notes = notes;
    app.autocomplete_payloads = payloads;
    app.selected_autocomplete_index = 0;
    app.show_autocomplete = true;
    app.autocomplete_prefix = pref.clone();
    app.last_autocomplete_trigger_len = pref.len();
}

/// Siapkan teks sisipan: buang spasi penutup bila karakter setelah kursor sudah
/// spasi, lalu hapus penanda kursor dan kembalikan offset caret-nya.
fn prepare_insert(raw: &str, next_is_space: bool) -> (String, usize) {
    let mut s = raw.to_string();
    if next_is_space && s.ends_with(' ') && !s.contains(autocomplete::CURSOR_MARK) {
        s.pop();
    }
    match s.find(autocomplete::CURSOR_MARK) {
        Some(p) => {
            s.remove(p);
            (s, p)
        }
        None => {
            let len = s.len();
            (s, len)
        }
    }
}

pub fn accept_current_suggestion(app: &mut Tabular) {
    if !app.show_autocomplete {
        return;
    }
    let idx = app.selected_autocomplete_index;
    let Some(display) = app.autocomplete_suggestions.get(idx).cloned() else {
        return;
    };
    let cursor = app.cursor_position.min(app.editor.text.len());
    let (pref, mut start) = current_prefix(&app.editor.text, cursor);
    // `alias.kol|` → hanya segmen setelah titik terakhir yang diganti
    if let Some(dot) = pref.rfind('.') {
        start += dot + 1;
    }
    let raw = app
        .autocomplete_payloads
        .get(idx)
        .cloned()
        .flatten()
        .unwrap_or_else(|| display.clone());
    let next_is_space = app.editor.text[cursor..]
        .chars()
        .next()
        .is_some_and(|c| c.is_whitespace());
    let (replacement, caret) = prepare_insert(&raw, next_is_space);
    app.editor.apply_single_replace(start..cursor, &replacement);
    app.cursor_position = start + caret;
    app.multi_selection
        .set_primary_range(app.cursor_position, app.cursor_position);
    app.pending_cursor_set = Some(app.cursor_position);
    app.autocomplete_expected_cursor = Some(app.cursor_position);
    app.autocomplete_protection_frames = app.autocomplete_protection_frames.max(8);
    app.editor_focus_boost_frames = app.editor_focus_boost_frames.max(6);
    *app.autocomplete_usage.entry(display).or_insert(0) += 1;
    app.show_autocomplete = false;
    app.autocomplete_suggestions.clear();
    app.autocomplete_kinds.clear();
    app.autocomplete_notes.clear();
    app.autocomplete_payloads.clear();
}

pub fn navigate(app: &mut Tabular, delta: i32) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let len = app.autocomplete_suggestions.len();
    if delta > 0 {
        app.selected_autocomplete_index = (app.selected_autocomplete_index + 1) % len;
    } else if app.selected_autocomplete_index == 0 {
        app.selected_autocomplete_index = len - 1;
    } else {
        app.selected_autocomplete_index -= 1;
    }
}

pub fn render_autocomplete(app: &mut Tabular, ui: &mut egui::Ui, pos: egui::Pos2) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let metrics =
        crate::window_egui::device_profile::DeviceUiMetrics::compute(ui.ctx(), app.ui_mode);
    let screen = ui.ctx().content_rect();
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let small_font_id = egui::TextStyle::Small.resolve(ui.style());
    let row_height = if metrics.is_touch { 32.0 } else { 22.0 };

    let suggestions = app.autocomplete_suggestions.clone();
    let kinds = app.autocomplete_kinds.clone();
    let notes = app.autocomplete_notes.clone();
    let mut max_label_px: f32 = 0.0;
    let mut max_note_px: f32 = 0.0;

    ui.ctx().fonts_mut(|f| {
        for (idx, s) in suggestions.iter().enumerate() {
            let g = f.layout_no_wrap(s.clone(), font_id.clone(), egui::Color32::WHITE);
            max_label_px = max_label_px.max(g.size().x + 24.0);

            if let Some(Some(note)) = notes.get(idx) {
                let ng =
                    f.layout_no_wrap(note.clone(), small_font_id.clone(), egui::Color32::WHITE);
                max_note_px = max_note_px.max(ng.size().x);
            }
        }
    });

    let base_width = max_label_px + max_note_px + 24.0;
    let popup_w = (base_width + 40.0).clamp(300.0, (screen.width() - 32.0).max(300.0));

    let entry_count = suggestions.len() as f32;
    let total_content_h = entry_count * row_height + 8.0;

    let screen_h = screen.height();
    let desired_cap = (screen_h * 0.55).max(120.0);
    let desired_h = total_content_h.min(desired_cap);

    let margin = 8.0;
    let space_below = (screen.bottom() - pos.y - margin).max(0.0);
    let space_above = (pos.y - screen.top() - margin).max(0.0);
    let show_above = space_below < desired_h && space_above > space_below;
    let max_h = desired_h.min(if show_above { space_above } else { space_below });

    let mut popup_pos = pos;
    if show_above {
        popup_pos.y = (pos.y - max_h - 4.0).max(screen.top());
    }
    if popup_pos.x + popup_w > screen.right() {
        popup_pos.x = (screen.right() - popup_w).max(screen.left());
    }

    egui::Area::new(egui::Id::new("autocomplete_popup"))
        .fixed_pos(popup_pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let bg_fill = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(30, 30, 35)
            } else {
                egui::Color32::from_rgb(250, 250, 250)
            };
            let stroke_color = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(80, 80, 90)
            } else {
                egui::Color32::from_rgb(180, 180, 190)
            };

            egui::Frame::new()
                .fill(bg_fill)
                .stroke(egui::Stroke::new(1.0, stroke_color))
                .corner_radius(egui::CornerRadius::same(if metrics.is_touch {
                    6_u8
                } else {
                    4_u8
                }))
                .shadow(eframe::epaint::Shadow {
                    offset: [0, 6],
                    blur: 10,
                    spread: 1,
                    color: egui::Color32::from_black_alpha(100),
                })
                .inner_margin(egui::Margin::symmetric(0, 4))
                .show(ui, |ui| {
                    ui.set_min_width(popup_w);
                    ui.set_max_width(popup_w);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                    let suggestions = suggestions.clone();
                    let kinds = kinds.clone();
                    let notes = notes.clone();

                    egui::ScrollArea::vertical()
                        .max_height(max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                            for (i, s) in suggestions.iter().enumerate() {
                                let selected = i == app.selected_autocomplete_index;
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), row_height),
                                    egui::Sense::click(),
                                );

                                if ui.is_rect_visible(rect) {
                                    if selected {
                                        let sel_color = if ui.visuals().dark_mode {
                                            egui::Color32::from_rgb(55, 115, 170)
                                        } else {
                                            egui::Color32::from_rgb(90, 145, 215)
                                        };
                                        ui.painter().rect_filled(rect, 0.0, sel_color);
                                    } else if response.hovered() {
                                        let hover_color = if ui.visuals().dark_mode {
                                            egui::Color32::from_rgb(45, 45, 52)
                                        } else {
                                            egui::Color32::from_rgb(235, 238, 245)
                                        };
                                        ui.painter().rect_filled(rect, 0.0, hover_color);
                                    }

                                    let text_color = if selected {
                                        egui::Color32::WHITE
                                    } else if ui.visuals().dark_mode {
                                        egui::Color32::from_rgb(225, 225, 230)
                                    } else {
                                        egui::Color32::from_rgb(25, 25, 35)
                                    };

                                    let icon = kind_icon(kinds.get(i).copied());

                                    // Left: Icon + Suggestion text
                                    ui.painter().text(
                                        egui::pos2(rect.left() + 6.0, rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        icon,
                                        small_font_id.clone(),
                                        text_color,
                                    );
                                    ui.painter().text(
                                        egui::pos2(rect.left() + 26.0, rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        s,
                                        font_id.clone(),
                                        text_color,
                                    );

                                    // Right: Note / description
                                    if let Some(Some(note)) = notes.get(i) {
                                        let note_color = if selected {
                                            egui::Color32::from_rgb(215, 225, 240)
                                        } else if ui.visuals().dark_mode {
                                            egui::Color32::from_rgb(135, 140, 150)
                                        } else {
                                            egui::Color32::from_rgb(105, 110, 125)
                                        };
                                        ui.painter().text(
                                            egui::pos2(rect.right() - 8.0, rect.center().y),
                                            egui::Align2::RIGHT_CENTER,
                                            note,
                                            small_font_id.clone(),
                                            note_color,
                                        );
                                    }
                                }

                                if response.clicked() {
                                    app.selected_autocomplete_index = i;
                                    accept_current_suggestion(app);
                                    let id = egui::Id::new("sql_editor");
                                    if let Some(mut state) =
                                        egui::text_edit::TextEditState::load(ui.ctx(), id)
                                    {
                                        use egui::text::{CCursor, CCursorRange};
                                        state.cursor.set_char_range(Some(CCursorRange::one(
                                            CCursor::new(app.cursor_position),
                                        )));
                                        state.store(ui.ctx(), id);
                                    }
                                    ui.memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                                    app.editor_focus_boost_frames =
                                        app.editor_focus_boost_frames.max(6);
                                }
                            }
                        });
                });
        });
}

pub fn trigger_manual(app: &mut Tabular) {
    drain_warm_results(app);
    app.autocomplete_last_update = Some(std::time::Instant::now());
    refresh(app, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_prefix_includes_qualifier() {
        assert_eq!(current_prefix("SELECT u.na", 11), ("u.na".to_string(), 7));
        assert_eq!(
            current_prefix("WHERE id = :us", 14),
            (":us".to_string(), 11)
        );
        assert_eq!(current_prefix("", 0), (String::new(), 0));
    }

    #[test]
    fn test_prepare_insert_cursor_mark_and_spacing() {
        let m = autocomplete::CURSOR_MARK;
        // penanda kursor dihapus, caret di posisinya
        assert_eq!(
            prepare_insert(&format!("COUNT({m})"), false),
            ("COUNT()".to_string(), 6)
        );
        // spasi penutup dibuang bila setelah kursor sudah ada spasi
        assert_eq!(prepare_insert("FROM ", true), ("FROM".to_string(), 4));
        assert_eq!(prepare_insert("FROM ", false), ("FROM ".to_string(), 5));
        // template dengan penanda tidak dipangkas
        assert_eq!(
            prepare_insert(&format!("BETWEEN {m} AND "), true),
            ("BETWEEN  AND ".to_string(), 8)
        );
    }

    #[test]
    fn test_map_kind_covers_join_and_alias() {
        assert_eq!(map_kind(ItemKind::JoinCondition), AutocompleteKind::Join);
        assert_eq!(map_kind(ItemKind::Cte), AutocompleteKind::Table);
        assert_eq!(map_kind(ItemKind::Template), AutocompleteKind::Snippet);
        assert_eq!(kind_icon(Some(AutocompleteKind::Join)), "🔗");
    }

    #[test]
    fn test_collect_tables_from_tree() {
        use crate::models::enums::NodeType;
        use crate::models::structs::TreeNode;

        let mut table1 = TreeNode::new("users".to_string(), NodeType::Table);
        table1.connection_id = Some(1);
        table1.database_name = Some("mydb".to_string());

        let mut view1 = TreeNode::new("v_active_users".to_string(), NodeType::View);
        view1.connection_id = Some(1);
        view1.database_name = Some("mydb".to_string());

        let mut other_db_table = TreeNode::new("other_users".to_string(), NodeType::Table);
        other_db_table.connection_id = Some(1);
        other_db_table.database_name = Some("otherdb".to_string());

        let mut other_conn_table = TreeNode::new("remote_users".to_string(), NodeType::Table);
        other_conn_table.connection_id = Some(2);
        other_conn_table.database_name = Some("mydb".to_string());

        let mut root = TreeNode::new("root".to_string(), NodeType::Connection);
        root.children = vec![table1, view1, other_db_table, other_conn_table];

        let mut out = Vec::new();
        collect_tables_from_tree(&[root], Some(1), Some("mydb"), &mut out);

        assert_eq!(out, vec!["users", "v_active_users"]);
    }

    #[test]
    fn test_collect_columns_from_tree() {
        use crate::models::enums::NodeType;
        use crate::models::structs::TreeNode;

        let col1 = TreeNode::new("id".to_string(), NodeType::Column);
        let col2 = TreeNode::new("email".to_string(), NodeType::Column);
        let col3 = TreeNode::new("name".to_string(), NodeType::Column);

        let mut table = TreeNode::new("customers".to_string(), NodeType::Table);
        table.connection_id = Some(1);
        table.children = vec![col1, col2, col3];

        let mut root = TreeNode::new("root".to_string(), NodeType::Connection);
        root.children = vec![table];

        let root_slice = std::slice::from_ref(&root);

        let mut cols = Vec::new();
        // Case-insensitive match check
        collect_columns_from_tree(root_slice, 1, "CUSTOMERS", &mut cols);
        assert_eq!(cols, vec!["id", "email", "name"]);

        // Unknown table returns empty without error
        let mut unknown_cols = Vec::new();
        collect_columns_from_tree(root_slice, 1, "nonexistent", &mut unknown_cols);
        assert!(unknown_cols.is_empty());
    }

    #[test]
    fn test_collect_loaded_fks_memory_lookup() {
        use crate::models::structs::ForeignKey;
        use std::collections::HashMap;

        let mut mem_fks: HashMap<(i64, String), Vec<ForeignKey>> = HashMap::new();
        mem_fks.insert(
            (1, "mydb".to_string()),
            vec![
                ForeignKey {
                    constraint_name: "fk_orders_customer".to_string(),
                    table_name: "orders".to_string(),
                    column_name: "customer_id".to_string(),
                    referenced_table_name: "customers".to_string(),
                    referenced_column_name: "id".to_string(),
                },
                ForeignKey {
                    constraint_name: "fk_items_order".to_string(),
                    table_name: "order_items".to_string(),
                    column_name: "order_id".to_string(),
                    referenced_table_name: "orders".to_string(),
                    referenced_column_name: "id".to_string(),
                },
            ],
        );

        // Verify that memory map lookup by (connection_id, db) is instant
        let key = (1, "mydb".to_string());
        let all_fks = mem_fks.get(&key).expect("FKs must be found in memory");
        assert_eq!(all_fks.len(), 2);
        assert_eq!(all_fks[0].table_name, "orders");
        assert_eq!(all_fks[0].referenced_table_name, "customers");
        assert_eq!(all_fks[1].table_name, "order_items");
    }
}
