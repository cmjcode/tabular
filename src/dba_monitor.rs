use std::collections::{HashMap, HashSet};

use eframe::egui;
use sqlx::{Column, Row};
use crate::models::enums::{DatabasePool, DatabaseType, DbaMonitorTab, ProcessStateFilter};
use crate::models::structs::{DbaMonitorState, ProcessInfo};

/// Action triggered from the DBA Monitor UI
#[derive(Debug, Clone, PartialEq)]
pub enum DbaAction {
    Refresh,
    CancelQuery(i64),
    KillProcess(i64),
    OpenInSqlTab(String),
}

/// Fetch processlist asynchronously directly from the connection pool
pub async fn fetch_dba_processes(
    pool: &DatabasePool,
    db_type: &DatabaseType,
) -> Result<Vec<ProcessInfo>, String> {
    let query = get_processlist_query(db_type);
    log::info!("[DBA-MONITOR] Fetching processes for db_type={:?}...", db_type);
    eprintln!("[DBA-MONITOR] Fetching processes for db_type={:?}...", db_type);
    match (db_type, pool) {
        (DatabaseType::PostgreSQL, DatabasePool::PostgreSQL(pg_pool)) => {
            let fut = sqlx::query(sqlx::AssertSqlSafe(query)).fetch_all(&**pg_pool);
            let rows = tokio::time::timeout(std::time::Duration::from_secs(5), fut)
                .await
                .map_err(|_| "Fetching PostgreSQL processes timed out after 5s".to_string())?
                .map_err(|e| e.to_string())?;

            let mut header_names = Vec::new();
            if let Some(first) = rows.first() {
                header_names = first.columns().iter().map(|c| c.name().to_string()).collect();
            }
            let mut string_rows = Vec::new();
            for r in rows {
                let mut row_vals = Vec::new();
                for (idx, _col) in r.columns().iter().enumerate() {
                    let val_str: String = if let Ok(s) = r.try_get::<String, _>(idx) {
                        s
                    } else if let Ok(i) = r.try_get::<i64, _>(idx) {
                        i.to_string()
                    } else if let Ok(i) = r.try_get::<i32, _>(idx) {
                        i.to_string()
                    } else if let Ok(f) = r.try_get::<f64, _>(idx) {
                        f.to_string()
                    } else {
                        String::new()
                    };
                    row_vals.push(val_str);
                }
                string_rows.push(row_vals);
            }
            log::info!("[DBA-MONITOR] PostgreSQL processes fetched: {} rows", string_rows.len());
            eprintln!("[DBA-MONITOR] PostgreSQL processes fetched: {} rows", string_rows.len());
            Ok(parse_processlist_rows(&header_names, &string_rows, db_type))
        }
        (DatabaseType::MySQL, DatabasePool::MySQL(my_pool)) => {
            let fut = sqlx::query(sqlx::AssertSqlSafe(query)).fetch_all(&**my_pool);
            let rows = tokio::time::timeout(std::time::Duration::from_secs(5), fut)
                .await
                .map_err(|_| "Fetching MySQL processlist timed out after 5s".to_string())?
                .map_err(|e| e.to_string())?;

            let mut header_names = Vec::new();
            if let Some(first) = rows.first() {
                header_names = first.columns().iter().map(|c| c.name().to_string()).collect();
            }
            let mut string_rows = Vec::new();
            for r in rows {
                let mut row_vals = Vec::new();
                for (idx, _col) in r.columns().iter().enumerate() {
                    let val_str: String = if let Ok(s) = r.try_get::<String, _>(idx) {
                        s
                    } else if let Ok(i) = r.try_get::<i64, _>(idx) {
                        i.to_string()
                    } else if let Ok(i) = r.try_get::<u64, _>(idx) {
                        i.to_string()
                    } else if let Ok(i) = r.try_get::<i32, _>(idx) {
                        i.to_string()
                    } else if let Ok(f) = r.try_get::<f64, _>(idx) {
                        f.to_string()
                    } else {
                        String::new()
                    };
                    row_vals.push(val_str);
                }
                string_rows.push(row_vals);
            }
            log::info!("[DBA-MONITOR] MySQL processes fetched: {} rows", string_rows.len());
            eprintln!("[DBA-MONITOR] MySQL processes fetched: {} rows", string_rows.len());
            Ok(parse_processlist_rows(&header_names, &string_rows, db_type))
        }
        (DatabaseType::SQLite, DatabasePool::SQLite(sq_pool)) => {
            let fut = sqlx::query(sqlx::AssertSqlSafe(query)).fetch_all(&**sq_pool);
            let rows = tokio::time::timeout(std::time::Duration::from_secs(5), fut)
                .await
                .map_err(|_| "Fetching SQLite processes timed out after 5s".to_string())?
                .map_err(|e| e.to_string())?;

            let mut header_names = Vec::new();
            if let Some(first) = rows.first() {
                header_names = first.columns().iter().map(|c| c.name().to_string()).collect();
            }
            let mut string_rows = Vec::new();
            for r in rows {
                let mut row_vals = Vec::new();
                for (idx, _col) in r.columns().iter().enumerate() {
                    let val_str: String = if let Ok(s) = r.try_get::<String, _>(idx) {
                        s
                    } else if let Ok(i) = r.try_get::<i64, _>(idx) {
                        i.to_string()
                    } else {
                        String::new()
                    };
                    row_vals.push(val_str);
                }
                string_rows.push(row_vals);
            }
            log::info!("[DBA-MONITOR] SQLite processes fetched: {} rows", string_rows.len());
            eprintln!("[DBA-MONITOR] SQLite processes fetched: {} rows", string_rows.len());
            Ok(parse_processlist_rows(&header_names, &string_rows, db_type))
        }
        _ => Err("Database engine not supported for live process monitor".to_string()),
    }
}

/// Execute a cancel or kill command on the database pool
pub async fn execute_dba_command(
    pool: &DatabasePool,
    query: &str,
) -> Result<(), String> {
    let query_owned = query.to_string();
    match pool {
        DatabasePool::PostgreSQL(pg_pool) => {
            sqlx::query(sqlx::AssertSqlSafe(query_owned))
                .execute(&**pg_pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        DatabasePool::MySQL(my_pool) => {
            sqlx::query(sqlx::AssertSqlSafe(query_owned))
                .execute(&**my_pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        DatabasePool::SQLite(sq_pool) => {
            sqlx::query(sqlx::AssertSqlSafe(query_owned))
                .execute(&**sq_pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        _ => Err("Unsupported database pool for DBA command".to_string()),
    }
}

/// SQL query generator for fetching process list and lock status
pub fn get_processlist_query(db_type: &DatabaseType) -> &'static str {
    match db_type {
        DatabaseType::PostgreSQL => {
            r#"SELECT 
    a.pid, 
    COALESCE(a.usename, '') AS usename, 
    COALESCE(a.datname, '') AS datname, 
    COALESCE(CAST(a.client_addr AS TEXT), 'local') AS client_addr, 
    COALESCE(a.state, 'unknown') AS state, 
    COALESCE(EXTRACT(EPOCH FROM (now() - a.query_start)), 0.0) AS duration_secs, 
    COALESCE(a.wait_event_type || ': ' || a.wait_event, '') AS wait_event, 
    COALESCE(a.query, '') AS query_text,
    CASE WHEN EXISTS (
        SELECT 1 FROM pg_locks bl 
        JOIN pg_locks bld ON bl.locktype = bld.locktype 
          AND bl.database IS NOT DISTINCT FROM bld.database 
          AND bl.relation IS NOT DISTINCT FROM bld.relation 
          AND bl.page IS NOT DISTINCT FROM bld.page 
          AND bl.tuple IS NOT DISTINCT FROM bld.tuple 
          AND bl.virtualxid IS NOT DISTINCT FROM bld.virtualxid 
          AND bl.transactionid IS NOT DISTINCT FROM bld.transactionid 
          AND bl.classid IS NOT DISTINCT FROM bld.classid 
          AND bl.objid IS NOT DISTINCT FROM bld.objid 
          AND bl.objsubid IS NOT DISTINCT FROM bld.objsubid 
        WHERE bl.pid = a.pid AND bl.granted AND NOT bld.granted AND bld.pid <> a.pid
    ) THEN 1 ELSE 0 END AS is_blocking,
    (
        SELECT bld_p.pid FROM pg_locks my_l
        JOIN pg_locks bld_l ON my_l.locktype = bld_l.locktype 
          AND my_l.database IS NOT DISTINCT FROM bld_l.database 
          AND my_l.relation IS NOT DISTINCT FROM bld_l.relation 
          AND my_l.page IS NOT DISTINCT FROM bld_l.page 
          AND my_l.tuple IS NOT DISTINCT FROM bld_l.tuple 
          AND my_l.virtualxid IS NOT DISTINCT FROM bld_l.virtualxid 
          AND my_l.transactionid IS NOT DISTINCT FROM bld_l.transactionid 
          AND my_l.classid IS NOT DISTINCT FROM bld_l.classid 
          AND my_l.objid IS NOT DISTINCT FROM bld_l.objid 
          AND my_l.objsubid IS NOT DISTINCT FROM bld_l.objsubid 
        JOIN pg_stat_activity bld_p ON bld_l.pid = bld_p.pid
        WHERE my_l.pid = a.pid AND NOT my_l.granted AND bld_l.granted AND bld_l.pid <> a.pid
        LIMIT 1
    ) AS blocked_by
FROM pg_stat_activity a
WHERE a.pid <> pg_backend_pid()
ORDER BY is_blocking DESC, duration_secs DESC;"#
        }
        DatabaseType::MySQL => {
            r#"SELECT 
    ID, 
    COALESCE(USER, '') AS USER, 
    COALESCE(HOST, '') AS HOST, 
    COALESCE(DB, '') AS DB, 
    COALESCE(COMMAND, '') AS COMMAND, 
    COALESCE(TIME, 0) AS TIME, 
    COALESCE(STATE, '') AS STATE, 
    COALESCE(INFO, '') AS INFO
FROM information_schema.PROCESSLIST 
WHERE ID <> CONNECTION_ID() 
ORDER BY TIME DESC;"#
        }
        DatabaseType::MsSQL => {
            r#"SELECT 
    r.session_id, 
    COALESCE(s.login_name, '') AS login_name, 
    COALESCE(DB_NAME(r.database_id), '') AS db_name, 
    COALESCE(s.host_name, '') AS host_name, 
    COALESCE(r.status, s.status, '') AS status, 
    COALESCE(r.total_elapsed_time / 1000.0, 0.0) AS duration_secs, 
    COALESCE(r.wait_type, '') AS wait_event, 
    COALESCE(t.text, '') AS query_text, 
    COALESCE(r.blocking_session_id, 0) AS blocking_session_id
FROM sys.dm_exec_sessions s
LEFT JOIN sys.dm_exec_requests r ON s.session_id = r.session_id
OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t 
WHERE s.session_id <> @@SPID AND s.is_user_process = 1
ORDER BY duration_secs DESC;"#
        }
        DatabaseType::SQLite => {
            r#"SELECT 1 AS pid, 'main' AS user, 'main' AS db, 'local' AS host, 'active' AS state, 0.0 AS duration_secs, '' AS wait_event, 'PRAGMA database_list;' AS query_text, 0 AS is_blocking, NULL AS blocked_by;"#
        }
        _ => "SELECT 1 WHERE 1=0;",
    }
}

/// Generate SQL statement to cancel a query
pub fn get_cancel_query(db_type: &DatabaseType, pid: i64) -> Option<String> {
    match db_type {
        DatabaseType::PostgreSQL => Some(format!("SELECT pg_cancel_backend({});", pid)),
        DatabaseType::MySQL => Some(format!("KILL QUERY {};", pid)),
        DatabaseType::MsSQL => Some(format!("KILL {};", pid)), // MSSQL kills session
        _ => None,
    }
}

/// Generate SQL statement to kill / terminate a process
pub fn get_kill_query(db_type: &DatabaseType, pid: i64) -> Option<String> {
    match db_type {
        DatabaseType::PostgreSQL => Some(format!("SELECT pg_terminate_backend({});", pid)),
        DatabaseType::MySQL => Some(format!("KILL {};", pid)),
        DatabaseType::MsSQL => Some(format!("KILL {};", pid)),
        _ => None,
    }
}

/// Parse raw database query result into standard `Vec<ProcessInfo>`
pub fn parse_processlist_rows(
    headers: &[String],
    rows: &[Vec<String>],
    db_type: &DatabaseType,
) -> Vec<ProcessInfo> {
    let mut header_map = HashMap::new();
    for (i, h) in headers.iter().enumerate() {
        header_map.insert(h.to_lowercase(), i);
    }

    let mut result = Vec::new();

    for row in rows {
        let get_val = |key: &str| -> String {
            if let Some(&idx) = header_map.get(key) {
                row.get(idx).cloned().unwrap_or_default()
            } else {
                String::new()
            }
        };

        match db_type {
            DatabaseType::PostgreSQL => {
                let pid = get_val("pid").parse::<i64>().unwrap_or(0);
                if pid == 0 {
                    continue;
                }
                let user = get_val("usename");
                let db = get_val("datname");
                let host = get_val("client_addr");
                let state = get_val("state");
                let duration_secs = get_val("duration_secs").parse::<f64>().unwrap_or(0.0);
                let wait_event_raw = get_val("wait_event");
                let wait_event = if wait_event_raw.trim().is_empty() || wait_event_raw == ":" {
                    None
                } else {
                    Some(wait_event_raw)
                };
                let query = get_val("query_text");
                let is_blocking = get_val("is_blocking") == "1";
                let blocked_by = get_val("blocked_by").parse::<i64>().ok().filter(|&id| id > 0);

                result.push(ProcessInfo {
                    pid,
                    user,
                    db,
                    host,
                    state,
                    duration_secs,
                    wait_event,
                    query,
                    is_blocking,
                    blocked_by,
                });
            }
            DatabaseType::MySQL => {
                let pid = get_val("id").parse::<i64>().unwrap_or(0);
                if pid == 0 {
                    continue;
                }
                let user = get_val("user");
                let db = get_val("db");
                let host = get_val("host");
                let command = get_val("command");
                let state_raw = get_val("state");
                let duration_secs = get_val("time").parse::<f64>().unwrap_or(0.0);
                let query = get_val("info");

                let state = if !state_raw.is_empty() {
                    state_raw
                } else {
                    command.clone()
                };

                let is_waiting = state.to_lowercase().contains("lock") || state.to_lowercase().contains("waiting");

                result.push(ProcessInfo {
                    pid,
                    user,
                    db,
                    host,
                    state,
                    duration_secs,
                    wait_event: if is_waiting { Some("Locked/Waiting".to_string()) } else { None },
                    query,
                    is_blocking: false,
                    blocked_by: None,
                });
            }
            DatabaseType::MsSQL => {
                let pid = get_val("session_id").parse::<i64>().unwrap_or(0);
                if pid == 0 {
                    continue;
                }
                let user = get_val("login_name");
                let db = get_val("db_name");
                let host = get_val("host_name");
                let state = get_val("status");
                let duration_secs = get_val("duration_secs").parse::<f64>().unwrap_or(0.0);
                let wait_event_raw = get_val("wait_event");
                let wait_event = if wait_event_raw.trim().is_empty() {
                    None
                } else {
                    Some(wait_event_raw)
                };
                let query = get_val("query_text");
                let blocking_id = get_val("blocking_session_id").parse::<i64>().unwrap_or(0);
                let blocked_by = if blocking_id > 0 && blocking_id != pid {
                    Some(blocking_id)
                } else {
                    None
                };

                result.push(ProcessInfo {
                    pid,
                    user,
                    db,
                    host,
                    state,
                    duration_secs,
                    wait_event,
                    query,
                    is_blocking: false, // Calculated in lock tree pass
                    blocked_by,
                });
            }
            _ => {
                let pid = get_val("pid").parse::<i64>().unwrap_or(1);
                result.push(ProcessInfo {
                    pid,
                    user: get_val("user"),
                    db: get_val("db"),
                    host: get_val("host"),
                    state: get_val("state"),
                    duration_secs: get_val("duration_secs").parse::<f64>().unwrap_or(0.0),
                    wait_event: None,
                    query: get_val("query_text"),
                    is_blocking: false,
                    blocked_by: None,
                });
            }
        }
    }

    // Post-process blocking flags for MySQL/MSSQL if blocked_by is present
    let blocked_ids: HashSet<i64> = result.iter().filter_map(|p| p.blocked_by).collect();
    for p in &mut result {
        if blocked_ids.contains(&p.pid) {
            p.is_blocking = true;
        }
    }

    result
}

/// Hierarchical Node for Lock & Deadlock Tree
#[derive(Debug, Clone)]
pub struct LockTreeNode {
    pub info: ProcessInfo,
    pub children: Vec<LockTreeNode>,
}

/// Build hierarchy of blockers and blocked queries
pub fn build_lock_tree(processes: &[ProcessInfo]) -> (Vec<LockTreeNode>, Vec<ProcessInfo>) {
    let mut by_pid: HashMap<i64, ProcessInfo> = HashMap::new();
    let mut children_map: HashMap<i64, Vec<i64>> = HashMap::new();

    for p in processes {
        by_pid.insert(p.pid, p.clone());
        if let Some(blocker_pid) = p.blocked_by {
            children_map.entry(blocker_pid).or_default().push(p.pid);
        }
    }

    // Find root blockers (processes that are blocking others and are not blocked themselves, or blocking with no parent)
    let mut root_blocker_pids = Vec::new();
    for p in processes {
        if p.is_blocking && p.blocked_by.is_none() {
            root_blocker_pids.push(p.pid);
        }
    }

    fn build_sub_tree(
        pid: i64,
        by_pid: &HashMap<i64, ProcessInfo>,
        children_map: &HashMap<i64, Vec<i64>>,
        visited: &mut HashSet<i64>,
    ) -> Option<LockTreeNode> {
        if visited.contains(&pid) {
            return None; // Prevent infinite cycle in deadlock
        }
        visited.insert(pid);

        let info = by_pid.get(&pid)?.clone();
        let mut children = Vec::new();

        if let Some(child_pids) = children_map.get(&pid) {
            for &child_pid in child_pids {
                if let Some(child_node) = build_sub_tree(child_pid, by_pid, children_map, visited) {
                    children.push(child_node);
                }
            }
        }

        Some(LockTreeNode { info, children })
    }

    let mut trees = Vec::new();
    let mut visited = HashSet::new();

    for root_pid in root_blocker_pids {
        if let Some(tree) = build_sub_tree(root_pid, &by_pid, &children_map, &mut visited) {
            trees.push(tree);
        }
    }

    // Unattached blocked processes (e.g. blocker not found in active list)
    let orphan_blocked: Vec<ProcessInfo> = processes
        .iter()
        .filter(|p| p.blocked_by.is_some() && !visited.contains(&p.pid))
        .cloned()
        .collect();

    (trees, orphan_blocked)
}

/// Format duration in seconds to clean readable string (e.g. "0.45s", "12.8s", "03m 15s")
pub fn format_duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{:.2}s", secs)
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else {
        let mins = (secs / 60.0).floor() as u64;
        let rem_secs = (secs % 60.0).floor() as u64;
        format!("{:02}m {:02}s", mins, rem_secs)
    }
}

/// Render the DBA Process Monitor interactive view inside a tab
pub fn render_dba_monitor(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    db_type: Option<&DatabaseType>,
    conn_name: &str,
    to_execute: &mut Option<DbaAction>,
) {
    ui.vertical(|ui| {
        // --- 1. Top Bar: Header, Badges, Metrics & Controls ---
        render_header_and_metrics(ui, state, db_type, conn_name, to_execute);
        ui.add_space(6.0);

        // --- 2. Filter & Navigation Bar ---
        render_navigation_and_filters(ui, state, to_execute);
        ui.add_space(4.0);

        // --- 3. Main Body Content (Processlist or Lock Tree) ---
        egui::Frame::group(ui.style())
            .fill(ui.visuals().window_fill())
            .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                match state.selected_tab {
                    DbaMonitorTab::Processlist => {
                        render_processlist_table(ui, state, to_execute);
                    }
                    DbaMonitorTab::LockTree => {
                        render_lock_tree_view(ui, state, to_execute);
                    }
                }
            });

        // --- 4. Bottom Panel: Executed SQL Query & Diagnostics ---
        ui.add_space(4.0);
        render_executed_query_panel(ui, state, db_type, to_execute);

        // --- 5. Confirmation Modal for Kill / Cancel ---
        render_confirm_modal(ui, state, to_execute);
    });
}

fn render_executed_query_panel(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    db_type: Option<&DatabaseType>,
    to_execute: &mut Option<DbaAction>,
) {
    let active_db = db_type.cloned().unwrap_or(DatabaseType::PostgreSQL);
    let query = get_processlist_query(&active_db);

    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} Executed SQL Query (DBA Live Monitoring)", egui_icons::icons::ICON_TERMINAL.codepoint))
                        .strong()
                        .size(13.0)
                        .color(ui.visuals().strong_text_color()),
                );

                // Status badge
                if state.is_loading {
                    ui.label(
                        egui::RichText::new("⏳ Fetching live data...")
                            .color(egui::Color32::from_rgb(220, 180, 50))
                            .size(11.0),
                    );
                } else if let Some((err_msg, true)) = &state.status_message {
                    ui.label(
                        egui::RichText::new(format!("⚠️ Error: {}", err_msg))
                            .color(egui::Color32::from_rgb(255, 100, 100))
                            .size(11.0),
                    );
                } else {
                    let total = state.processes.len();
                    let badge_text = if total == 0 {
                        "⚠️ 0 active processes retrieved (No current running queries or insufficient privileges)".to_string()
                    } else {
                        format!("✅ {} processes retrieved", total)
                    };
                    let badge_col = if total == 0 {
                        egui::Color32::from_rgb(200, 150, 40)
                    } else {
                        egui::Color32::from_rgb(40, 180, 80)
                    };
                    ui.label(
                        egui::RichText::new(badge_text)
                            .color(badge_col)
                            .size(11.0),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("⚡ Open in SQL Tab / Test Query").clicked() {
                        *to_execute = Some(DbaAction::OpenInSqlTab(query.to_string()));
                    }

                    if ui.small_button("📋 Copy SQL Query").clicked() {
                        ui.ctx().copy_text(query.to_string());
                    }

                    if ui.small_button("🔄 Re-run").clicked() {
                        *to_execute = Some(DbaAction::Refresh);
                    }
                });
            });

            ui.separator();
            let mut query_str = query;
            egui::ScrollArea::vertical()
                .id_salt("dba_query_scroll")
                .max_height(100.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut query_str)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(3),
                    );
                });
        });
}

fn render_header_and_metrics(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    db_type: Option<&DatabaseType>,
    conn_name: &str,
    to_execute: &mut Option<DbaAction>,
) {
    ui.horizontal(|ui| {
        // Title & Database Badge
        ui.heading(
            egui::RichText::new(format!("{} Live DBA Process Monitor", egui_icons::icons::ICON_MONITORING.codepoint))
                .strong()
                .size(16.0),
        );

        ui.add_space(8.0);
        let engine_label = match db_type {
            Some(DatabaseType::PostgreSQL) => "PostgreSQL",
            Some(DatabaseType::MySQL) => "MySQL",
            Some(DatabaseType::MsSQL) => "SQL Server",
            Some(DatabaseType::SQLite) => "SQLite",
            _ => "Database",
        };
        ui.label(
            egui::RichText::new(format!("{} {} — {}", egui_icons::icons::ICON_STORAGE.codepoint, engine_label, conn_name))
                .color(egui::Color32::from_rgb(100, 180, 240))
                .size(12.0),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Manual Refresh Button
            if ui.button(format!("{} Refresh Now", egui_icons::icons::ICON_REFRESH.codepoint)).clicked() {
                *to_execute = Some(DbaAction::Refresh);
            }

            ui.add_space(4.0);

            // Interval Selector
            egui::ComboBox::from_id_salt("dba_refresh_interval")
                .selected_text(format!("{}s", state.refresh_interval_secs))
                .width(55.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.refresh_interval_secs, 1, "1s");
                    ui.selectable_value(&mut state.refresh_interval_secs, 2, "2s");
                    ui.selectable_value(&mut state.refresh_interval_secs, 3, "3s");
                    ui.selectable_value(&mut state.refresh_interval_secs, 5, "5s");
                    ui.selectable_value(&mut state.refresh_interval_secs, 10, "10s");
                });

            ui.label(egui::RichText::new("Interval:").size(11.0).color(egui::Color32::GRAY));

            // Auto-refresh Toggle
            let refresh_btn = if state.auto_refresh {
                egui::Button::new(
                    egui::RichText::new(format!("{} Polling", egui_icons::icons::ICON_FIBER_MANUAL_RECORD.codepoint))
                        .color(egui::Color32::from_rgb(50, 205, 50))
                )
            } else {
                egui::Button::new(
                    egui::RichText::new(format!("{} Paused", egui_icons::icons::ICON_PAUSE.codepoint))
                        .color(egui::Color32::from_rgb(220, 150, 50))
                )
            };
            if ui.add(refresh_btn).clicked() {
                state.auto_refresh = !state.auto_refresh;
            }

            if state.is_loading {
                ui.spinner();
            }
        });
    });

    ui.add_space(4.0);

    // --- Metric Cards Bar ---
    let total_count = state.processes.len();
    let active_count = state
        .processes
        .iter()
        .filter(|p| p.state.to_lowercase().contains("active") || p.state.to_lowercase().contains("running"))
        .count();
    let blocked_count = state
        .processes
        .iter()
        .filter(|p| p.is_blocking || p.blocked_by.is_some() || p.state.to_lowercase().contains("lock") || p.state.to_lowercase().contains("wait"))
        .count();
    let slow_count = state.processes.iter().filter(|p| p.duration_secs > 5.0).count();

    ui.horizontal(|ui| {
        metric_card(ui, &format!("{} Total Sessions", egui_icons::icons::ICON_PERSON.codepoint), &total_count.to_string(), egui::Color32::from_rgb(140, 160, 220));
        metric_card(ui, &format!("{} Active Queries", egui_icons::icons::ICON_BOLT.codepoint), &active_count.to_string(), egui::Color32::from_rgb(80, 200, 120));
        metric_card(
            ui,
            &format!("{} Blocked / Locks", egui_icons::icons::ICON_BLOCK.codepoint),
            &blocked_count.to_string(),
            if blocked_count > 0 {
                egui::Color32::from_rgb(240, 80, 80)
            } else {
                egui::Color32::from_rgb(120, 120, 120)
            },
        );
        metric_card(
            ui,
            &format!("{} Slow (> 5s)", egui_icons::icons::ICON_HOURGLASS_EMPTY.codepoint),
            &slow_count.to_string(),
            if slow_count > 0 {
                egui::Color32::from_rgb(240, 160, 50)
            } else {
                egui::Color32::from_rgb(120, 120, 120)
            },
        );

        if let Some((msg, is_err)) = &state.status_message {
            ui.add_space(10.0);
            let col = if *is_err {
                egui::Color32::from_rgb(240, 80, 80)
            } else {
                egui::Color32::from_rgb(80, 200, 120)
            };
            ui.label(egui::RichText::new(msg).color(col).size(11.0));
        }
    });
}

fn metric_card(ui: &mut egui::Ui, title: &str, value: &str, accent_color: egui::Color32) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).size(11.0).color(egui::Color32::GRAY));
                ui.label(egui::RichText::new(value).size(13.0).strong().color(accent_color));
            });
        });
}

fn render_navigation_and_filters(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    _to_execute: &mut Option<DbaAction>,
) {
    ui.horizontal(|ui| {
        // Tab Pills
        if ui
            .selectable_label(
                state.selected_tab == DbaMonitorTab::Processlist,
                format!("{} Processlist ({})", egui_icons::icons::ICON_DNS.codepoint, state.processes.len()),
            )
            .clicked()
        {
            state.selected_tab = DbaMonitorTab::Processlist;
        }

        let blocked_count = state.processes.iter().filter(|p| p.is_blocking || p.blocked_by.is_some()).count();
        if ui
            .selectable_label(
                state.selected_tab == DbaMonitorTab::LockTree,
                format!("{} Deadlock & Lock Tree ({})", egui_icons::icons::ICON_ACCOUNT_TREE.codepoint, blocked_count),
            )
            .clicked()
        {
            state.selected_tab = DbaMonitorTab::LockTree;
        }

        ui.separator();

        // State Filter Pills
        ui.label(egui::RichText::new("Filter:").size(11.0).color(egui::Color32::GRAY));
        if ui
            .selectable_label(state.filter_state == ProcessStateFilter::All, "All")
            .clicked()
        {
            state.filter_state = ProcessStateFilter::All;
        }
        if ui
            .selectable_label(state.filter_state == ProcessStateFilter::ActiveOnly, format!("{} Active", egui_icons::icons::ICON_PLAY_ARROW.codepoint))
            .clicked()
        {
            state.filter_state = ProcessStateFilter::ActiveOnly;
        }
        if ui
            .selectable_label(state.filter_state == ProcessStateFilter::BlockedOnly, format!("{} Blocked", egui_icons::icons::ICON_BLOCK.codepoint))
            .clicked()
        {
            state.filter_state = ProcessStateFilter::BlockedOnly;
        }
        if ui
            .selectable_label(state.filter_state == ProcessStateFilter::IdleOnly, format!("{} Idle", egui_icons::icons::ICON_PAUSE.codepoint))
            .clicked()
        {
            state.filter_state = ProcessStateFilter::IdleOnly;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.search_text)
                    .hint_text(format!("{} Search PID, user, db, query...", egui_icons::icons::ICON_SEARCH.codepoint))
                    .desired_width(220.0),
            );
        });
    });
}

fn filter_process(p: &ProcessInfo, state: &DbaMonitorState) -> bool {
    // 1. State filter
    let state_lower = p.state.to_lowercase();
    let matches_state = match state.filter_state {
        ProcessStateFilter::All => true,
        ProcessStateFilter::ActiveOnly => state_lower.contains("active") || state_lower.contains("running") || state_lower.contains("execut"),
        ProcessStateFilter::BlockedOnly => p.is_blocking || p.blocked_by.is_some() || state_lower.contains("lock") || state_lower.contains("wait"),
        ProcessStateFilter::IdleOnly => state_lower.contains("idle") || state_lower.contains("sleep"),
    };
    if !matches_state {
        return false;
    }

    // 2. Search text filter
    let search = state.search_text.trim().to_lowercase();
    if search.is_empty() {
        return true;
    }

    p.pid.to_string().contains(&search)
        || p.user.to_lowercase().contains(&search)
        || p.db.to_lowercase().contains(&search)
        || p.host.to_lowercase().contains(&search)
        || p.query.to_lowercase().contains(&search)
        || p.state.to_lowercase().contains(&search)
}

fn render_processlist_table(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    _to_execute: &mut Option<DbaAction>,
) {
    let filtered_processes: Vec<&ProcessInfo> = state.processes.iter().filter(|p| filter_process(p, state)).collect();

    if filtered_processes.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            ui.label(egui::RichText::new("No matching active processes found.").color(egui::Color32::GRAY));
            ui.add_space(30.0);
        });
        return;
    }

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("dba_processlist_grid")
                .striped(true)
                .spacing([10.0, 6.0])
                .min_col_width(50.0)
                .show(ui, |ui| {
                    // Table Header
                    ui.label(egui::RichText::new("PID").strong());
                    ui.label(egui::RichText::new("User").strong());
                    ui.label(egui::RichText::new("Database").strong());
                    ui.label(egui::RichText::new("Host").strong());
                    ui.label(egui::RichText::new("State").strong());
                    ui.label(egui::RichText::new("Duration").strong());
                    ui.label(egui::RichText::new("Wait Event").strong());
                    ui.label(egui::RichText::new("Query Preview").strong());
                    ui.label(egui::RichText::new("Actions").strong());
                    ui.end_row();

                    let mut pid_to_confirm_cancel = None;
                    let mut pid_to_confirm_kill = None;

                    for p in filtered_processes {
                        let is_selected = state.selected_pid == Some(p.pid);

                        // PID
                        let pid_text = egui::RichText::new(p.pid.to_string())
                            .monospace()
                            .strong()
                            .color(if is_selected {
                                egui::Color32::from_rgb(100, 200, 255)
                            } else {
                                ui.visuals().text_color()
                            });
                        if ui.selectable_label(is_selected, pid_text).clicked() {
                            state.selected_pid = Some(p.pid);
                        }

                        // User & DB & Host
                        ui.label(egui::RichText::new(&p.user).color(egui::Color32::from_rgb(180, 180, 220)));
                        ui.label(egui::RichText::new(&p.db).color(egui::Color32::from_rgb(140, 210, 210)));
                        ui.label(egui::RichText::new(&p.host).size(11.0).color(egui::Color32::GRAY));

                        // State Pill
                        let state_lower = p.state.to_lowercase();
                        let (state_bg, state_fg) = if p.is_blocking || p.blocked_by.is_some() || state_lower.contains("lock") {
                            (egui::Color32::from_rgb(180, 40, 40), egui::Color32::WHITE)
                        } else if state_lower.contains("active") || state_lower.contains("running") {
                            (egui::Color32::from_rgb(40, 140, 60), egui::Color32::WHITE)
                        } else if state_lower.contains("idle in transaction") {
                            (egui::Color32::from_rgb(180, 130, 30), egui::Color32::WHITE)
                        } else {
                            (egui::Color32::from_rgb(70, 70, 70), egui::Color32::LIGHT_GRAY)
                        };

                        ui.horizontal(|ui| {
                            let label = if p.is_blocking {
                                format!("⛔ BLOCKER: {}", p.state)
                            } else if p.blocked_by.is_some() {
                                format!("⏳ BLOCKED by {}", p.blocked_by.unwrap())
                            } else {
                                p.state.clone()
                            };

                            let frame = egui::Frame::group(ui.style())
                                .fill(state_bg)
                                .inner_margin(egui::Margin::symmetric(4, 1));
                            frame.show(ui, |ui| {
                                ui.label(egui::RichText::new(label).color(state_fg).size(10.0).strong());
                            });
                        });

                        // Duration
                        let dur_col = if p.duration_secs > 30.0 {
                            egui::Color32::from_rgb(240, 80, 80)
                        } else if p.duration_secs > 5.0 {
                            egui::Color32::from_rgb(240, 180, 50)
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.label(egui::RichText::new(format_duration(p.duration_secs)).monospace().color(dur_col));

                        // Wait Event
                        let wait_text = p.wait_event.as_deref().unwrap_or("-");
                        ui.label(egui::RichText::new(wait_text).size(11.0).color(egui::Color32::GRAY));

                        // Query Preview
                        let clean_query: String = p.query.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
                        let truncated = if clean_query.len() > 60 {
                            format!("{}...", &clean_query[..57])
                        } else if clean_query.is_empty() {
                            "-".to_string()
                        } else {
                            clean_query
                        };

                        let mut query_resp = ui.add(
                            egui::Label::new(egui::RichText::new(truncated).monospace().size(11.0))
                                .sense(egui::Sense::click()),
                        );
                        if query_resp.clicked() {
                            state.selected_pid = Some(p.pid);
                        }
                        if !p.query.is_empty() {
                            query_resp = query_resp.on_hover_ui(|ui| {
                                ui.set_max_width(500.0);
                                ui.label(egui::RichText::new("Full Query:").strong());
                                ui.add(egui::Label::new(egui::RichText::new(&p.query).monospace().size(11.0)));
                            });
                        }

                        // Actions
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(egui::RichText::new(format!("{} Cancel", egui_icons::icons::ICON_CANCEL.codepoint)).size(10.0))).on_hover_text("Cancel current running query").clicked() {
                                pid_to_confirm_cancel = Some(p.pid);
                            }
                            if ui.add(egui::Button::new(egui::RichText::new(format!("{} Kill", egui_icons::icons::ICON_DELETE_FOREVER.codepoint)).color(egui::Color32::from_rgb(240, 80, 80)).size(10.0))).on_hover_text("Terminate connection").clicked() {
                                pid_to_confirm_kill = Some(p.pid);
                            }
                        });

                        ui.end_row();
                    }

                    if let Some(pid) = pid_to_confirm_cancel {
                        state.confirm_action = Some((pid, true));
                    }
                    if let Some(pid) = pid_to_confirm_kill {
                        state.confirm_action = Some((pid, false));
                    }
                });
        });
}

fn render_lock_tree_view(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    _to_execute: &mut Option<DbaAction>,
) {
    let (trees, orphans) = build_lock_tree(&state.processes);

    if trees.is_empty() && orphans.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(egui::RichText::new(format!("{} No lock contention or deadlocks detected!", egui_icons::icons::ICON_CHECK_CIRCLE.codepoint)).color(egui::Color32::from_rgb(80, 200, 120)).size(14.0));
            ui.label(egui::RichText::new("All database sessions are running smoothly without blocking each other.").color(egui::Color32::GRAY).size(12.0));
            ui.add_space(40.0);
        });
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label(egui::RichText::new(format!("{} Active Lock Dependencies & Bottlenecks", egui_icons::icons::ICON_WARNING.codepoint)).strong().color(egui::Color32::from_rgb(240, 100, 100)));
        ui.add_space(6.0);

        for tree in &trees {
            render_tree_node(ui, tree, 0, state);
            ui.add_space(6.0);
        }

        if !orphans.is_empty() {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(format!("{} Other Waiting Sessions (Waiting for external / transaction locks)", egui_icons::icons::ICON_HOURGLASS_EMPTY.codepoint)).strong().color(egui::Color32::from_rgb(220, 160, 50)));
            for p in &orphans {
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().faint_bg_color)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("PID {}", p.pid)).strong().monospace());
                            ui.label(format!("User: {} | DB: {}", p.user, p.db));
                            ui.label(egui::RichText::new(format!("Waiting: {}", format_duration(p.duration_secs))).color(egui::Color32::from_rgb(240, 160, 50)));
                            if let Some(event) = &p.wait_event {
                                ui.label(egui::RichText::new(event).size(11.0).color(egui::Color32::GRAY));
                            }
                            if ui.button(format!("{} Kill", egui_icons::icons::ICON_DELETE_FOREVER.codepoint)).clicked() {
                                state.confirm_action = Some((p.pid, false));
                            }
                        });
                    });
            }
        }
    });
}

fn render_tree_node(ui: &mut egui::Ui, node: &LockTreeNode, depth: usize, state: &mut DbaMonitorState) {
    let is_root = depth == 0;
    let bg_color = if is_root {
        egui::Color32::from_rgb(60, 20, 20)
    } else {
        egui::Color32::from_rgb(35, 35, 45)
    };

    let p = &node.info;
    let indent = (depth as f32) * 20.0;

    ui.horizontal(|ui| {
        if indent > 0.0 {
            ui.add_space(indent);
            ui.label(egui::RichText::new(format!("{} ", egui_icons::icons::ICON_CHEVRON_RIGHT.codepoint)).color(egui::Color32::from_rgb(240, 100, 100)));
        }

        egui::Frame::group(ui.style())
            .fill(bg_color)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if is_root {
                        ui.label(egui::RichText::new(format!("{} ROOT BLOCKER", egui_icons::icons::ICON_ERROR.codepoint)).strong().color(egui::Color32::from_rgb(255, 80, 80)));
                    }
                    ui.label(egui::RichText::new(format!("PID: {}", p.pid)).monospace().strong());
                    ui.label(format!("User: {} | DB: {}", p.user, p.db));
                    ui.label(egui::RichText::new(format_duration(p.duration_secs)).monospace());
                    ui.label(egui::RichText::new(&p.state).size(11.0).color(egui::Color32::GRAY));

                    let clean_query: String = p.query.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
                    let short_query = if clean_query.len() > 40 {
                        format!("{}...", &clean_query[..37])
                    } else {
                        clean_query
                    };
                    ui.label(egui::RichText::new(short_query).monospace().size(11.0).color(egui::Color32::LIGHT_GRAY));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new(format!("{} Terminate", egui_icons::icons::ICON_DELETE_FOREVER.codepoint)).color(egui::Color32::from_rgb(255, 100, 100))).clicked() {
                            state.confirm_action = Some((p.pid, false));
                        }
                        if ui.button(format!("{} Cancel", egui_icons::icons::ICON_CANCEL.codepoint)).clicked() {
                            state.confirm_action = Some((p.pid, true));
                        }
                    });
                });
            });
    });

    for child in &node.children {
        render_tree_node(ui, child, depth + 1, state);
    }
}

fn render_confirm_modal(
    ui: &mut egui::Ui,
    state: &mut DbaMonitorState,
    to_execute: &mut Option<DbaAction>,
) {
    if let Some((pid, is_cancel)) = state.confirm_action {
        let action_name = if is_cancel { "Cancel Query" } else { "Kill Process / Session" };
        let action_verb = if is_cancel { "Cancel" } else { "Kill" };

        let proc_info = state.processes.iter().find(|p| p.pid == pid).cloned();

        egui::Window::new(format!("{} Confirm {}", egui_icons::icons::ICON_WARNING.codepoint, action_name))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Are you sure you want to {} for PID {}?",
                            action_verb.to_lowercase(),
                            pid
                        ))
                        .strong()
                        .size(13.0),
                    );
                    ui.add_space(4.0);

                    if let Some(p) = proc_info {
                        ui.label(format!("• User: {}", p.user));
                        ui.label(format!("• Database: {}", p.db));
                        ui.label(format!("• Duration: {}", format_duration(p.duration_secs)));
                        if !p.query.is_empty() {
                            ui.label(egui::RichText::new("• Query:").size(11.0));
                            egui::Frame::group(ui.style())
                                .fill(ui.visuals().faint_bg_color)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(&p.query).monospace().size(11.0));
                                });
                        }
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("No, Keep Running").clicked() {
                            state.confirm_action = None;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let confirm_btn = egui::Button::new(
                                egui::RichText::new(format!("Yes, {}", action_verb))
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(200, 40, 40));

                            if ui.add(confirm_btn).clicked() {
                                if is_cancel {
                                    *to_execute = Some(DbaAction::CancelQuery(pid));
                                } else {
                                    *to_execute = Some(DbaAction::KillProcess(pid));
                                }
                                state.confirm_action = None;
                            }
                        });
                    });
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_queries_postgres() {
        let q = get_processlist_query(&DatabaseType::PostgreSQL);
        assert!(q.contains("pg_stat_activity"));
        assert!(q.contains("pg_locks"));

        let cancel_q = get_cancel_query(&DatabaseType::PostgreSQL, 1234).unwrap();
        assert_eq!(cancel_q, "SELECT pg_cancel_backend(1234);");

        let kill_q = get_kill_query(&DatabaseType::PostgreSQL, 1234).unwrap();
        assert_eq!(kill_q, "SELECT pg_terminate_backend(1234);");
    }

    #[test]
    fn test_get_queries_mysql() {
        let q = get_processlist_query(&DatabaseType::MySQL);
        assert!(q.contains("information_schema.PROCESSLIST"));

        let cancel_q = get_cancel_query(&DatabaseType::MySQL, 5678).unwrap();
        assert_eq!(cancel_q, "KILL QUERY 5678;");

        let kill_q = get_kill_query(&DatabaseType::MySQL, 5678).unwrap();
        assert_eq!(kill_q, "KILL 5678;");
    }

    #[test]
    fn test_parse_and_lock_tree() {
        let headers = vec![
            "pid".to_string(),
            "usename".to_string(),
            "datname".to_string(),
            "client_addr".to_string(),
            "state".to_string(),
            "duration_secs".to_string(),
            "wait_event".to_string(),
            "query_text".to_string(),
            "is_blocking".to_string(),
            "blocked_by".to_string(),
        ];

        let rows = vec![
            vec![
                "101".to_string(),
                "admin".to_string(),
                "mydb".to_string(),
                "127.0.0.1".to_string(),
                "active".to_string(),
                "45.2".to_string(),
                "".to_string(),
                "UPDATE accounts SET balance = 100 WHERE id = 1;".to_string(),
                "1".to_string(),
                "".to_string(),
            ],
            vec![
                "102".to_string(),
                "app_user".to_string(),
                "mydb".to_string(),
                "127.0.0.1".to_string(),
                "waiting".to_string(),
                "30.1".to_string(),
                "Lock:transactionid".to_string(),
                "SELECT * FROM accounts WHERE id = 1 FOR UPDATE;".to_string(),
                "0".to_string(),
                "101".to_string(),
            ],
        ];

        let procs = parse_processlist_rows(&headers, &rows, &DatabaseType::PostgreSQL);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 101);
        assert!(procs[0].is_blocking);
        assert_eq!(procs[1].blocked_by, Some(101));

        let (trees, orphans) = build_lock_tree(&procs);
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].info.pid, 101);
        assert_eq!(trees[0].children.len(), 1);
        assert_eq!(trees[0].children[0].info.pid, 102);
        assert_eq!(orphans.len(), 0);
    }
}
