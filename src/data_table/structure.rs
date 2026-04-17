use log::{debug};
use crate::{connection, driver_mssql, models, window_egui};

pub(crate) fn load_structure_info_for_current_table(tabular: &mut window_egui::Tabular) {
    // Determine current target
    let Some(conn_id) = tabular.current_connection_id else {
        return;
    };
    let active_tab_db = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .unwrap_or_default();
    if let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    {
        // Infer actual table name from current UI state (avoids using captions like "Query Results")
        let table_guess = infer_current_table_name(tabular);
        if table_guess.trim().is_empty() {
            // Nothing to load if we can't determine a concrete table
            return;
        }
        let database = if !active_tab_db.is_empty() {
            active_tab_db.clone()
        } else {
            conn.database.clone()
        };

        // Short-circuit: if target unchanged and relevant subview data is already loaded, do nothing
        let target = (conn_id, database.clone(), table_guess.clone());
        if !tabular.request_structure_refresh
            && tabular
                .last_structure_target
                .as_ref()
                .map(|t| t == &target)
                .unwrap_or(false)
        {
            match tabular.structure_sub_view {
                models::structs::StructureSubView::Columns
                    if !tabular.structure_columns.is_empty() =>
                {
                    debug!(
                        "✅ Structure (columns) already loaded in-memory for {}/{} (skip reload)",
                        database, table_guess
                    );
                    return;
                }
                models::structs::StructureSubView::Indexes
                    if !tabular.structure_indexes.is_empty() =>
                {
                    debug!(
                        "✅ Structure (indexes) already loaded in-memory for {}/{} (skip reload)",
                        database, table_guess
                    );
                    return;
                }
                _ => {}
            }
        }

        // Reset current in-memory structure before (re)loading
        tabular.structure_columns.clear();
        tabular.structure_indexes.clear();
        tabular.structure_selected_row = None;
        tabular.structure_selected_cell = None;
        tabular.structure_sel_anchor = None;

        // Branch: if user explicitly requested refresh, force live fetch and update cache
        if tabular.request_structure_refresh {
            if let Some(cols) = crate::connection::fetch_columns_from_database(
                conn_id,
                &database,
                &table_guess,
                &conn,
            ) {
                crate::cache_data::save_columns_to_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                    &cols,
                );
                debug!(
                    "🔄 Manual refresh: loaded live structure from server for {}/{} ({} columns)",
                    database,
                    table_guess,
                    cols.len()
                );
                for (name, dtype) in cols {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name,
                            data_type: dtype,
                            ..Default::default()
                        });
                }
            }
        } else {
            // 1) Try to populate from cache immediately for instant UI
            let mut had_struct_cache = false;
            if let Some(cols) =
                crate::cache_data::get_columns_from_cache(tabular, conn_id, &database, &table_guess)
                && !cols.is_empty()
            {
                debug!(
                    "📦 Showing cached structure for {}/{} ({} columns)",
                    database,
                    table_guess,
                    cols.len()
                );
                for (name, dtype) in cols {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name,
                            data_type: dtype,
                            ..Default::default()
                        });
                }
                had_struct_cache = true;
            }

            // 2) Only fetch live structure if no cache yet
            if !had_struct_cache
                && let Some(cols) = crate::connection::fetch_columns_from_database(
                    conn_id,
                    &database,
                    &table_guess,
                    &conn,
                )
            {
                // Keep cache updated with latest structure
                crate::cache_data::save_columns_to_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                    &cols,
                );
                debug!(
                    "🌐 Loaded live structure from server for {}/{} ({} columns)",
                    database,
                    table_guess,
                    cols.len()
                );
                for (name, dtype) in cols {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name,
                            data_type: dtype,
                            ..Default::default()
                        });
                }
            }
        }

        // Detailed index metadata: only when Indexes subview is visible
        if tabular.structure_sub_view == models::structs::StructureSubView::Indexes {
            if tabular.request_structure_refresh {
                // Force live fetch and update cache
                let idx =
                    fetch_index_details_for_table(tabular, conn_id, &conn, &database, &table_guess);
                crate::cache_data::save_indexes_to_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                    &idx,
                );
                tabular.structure_indexes = idx;
            } else {
                // Try cache first for instant display
                if let Some(cached) = crate::cache_data::get_indexes_from_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                ) {
                    if !cached.is_empty() {
                        tabular.structure_indexes = cached;
                    } else {
                        let idx = fetch_index_details_for_table(
                            tabular,
                            conn_id,
                            &conn,
                            &database,
                            &table_guess,
                        );
                        if !idx.is_empty() {
                            crate::cache_data::save_indexes_to_cache(
                                tabular,
                                conn_id,
                                &database,
                                &table_guess,
                                &idx,
                            );
                        }
                        tabular.structure_indexes = idx;
                    }
                } else {
                    let idx = fetch_index_details_for_table(
                        tabular,
                        conn_id,
                        &conn,
                        &database,
                        &table_guess,
                    );
                    if !idx.is_empty() {
                        crate::cache_data::save_indexes_to_cache(
                            tabular,
                            conn_id,
                            &database,
                            &table_guess,
                            &idx,
                        );
                    }
                    tabular.structure_indexes = idx;
                }
            }
        }

        // Fetch and cache partitions whenever structure is refreshed (always, not just for sidebar)
        if tabular.request_structure_refresh {
            // Force live fetch of partitions and update cache
            if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(conn_id)).cloned() {
                let partitions = fetch_partition_details_for_table(
                    tabular,
                    conn_id,
                    &connection,
                    &database,
                    &table_guess,
                );
                if !partitions.is_empty() {
                    crate::cache_data::save_partitions_to_cache(
                        tabular,
                        conn_id,
                        &database,
                        &table_guess,
                        &partitions,
                    );
                    debug!(
                        "✅ Refreshed partition cache for {}/{} ({} partitions)",
                        database,
                        table_guess,
                        partitions.len()
                    );
                }
            }
        } else {
            // Background fetch: seed partition cache if empty
            if crate::cache_data::get_partitions_from_cache(tabular, conn_id, &database, &table_guess).is_none()
                && let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(conn_id)).cloned() {
                    let partitions = fetch_partition_details_for_table(
                        tabular,
                        conn_id,
                        &connection,
                        &database,
                        &table_guess,
                    );
                    if !partitions.is_empty() {
                        crate::cache_data::save_partitions_to_cache(
                            tabular,
                            conn_id,
                            &database,
                            &table_guess,
                            &partitions,
                        );
                        debug!(
                            "✅ Seeded partition cache for {}/{} ({} partitions)",
                            database,
                            table_guess,
                            partitions.len()
                        );
                    }
            }
        }

        // Remember last loaded structure target and clear refresh request
        tabular.last_structure_target = Some((conn_id, database, table_guess));
        tabular.request_structure_refresh = false;
    }
}

// Fetch partition details for a table per database type
pub fn fetch_partition_details_for_table(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    table_name: &str,
) -> Vec<models::structs::PartitionStructInfo> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = crate::connection::get_or_create_connection_pool(tabular, connection_id).await {
                    // First get partition names
                    let names_q = "SELECT PARTITION_NAME FROM INFORMATION_SCHEMA.PARTITIONS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND PARTITION_NAME IS NOT NULL AND SUBPARTITION_NAME IS NULL ORDER BY PARTITION_ORDINAL_POSITION";
                    let partition_names: Vec<String> = sqlx::query_as::<_, (String,)>(names_q)
                        .bind(database_name)
                        .bind(table_name)
                        .fetch_all(mysql_pool.as_ref())
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(n,)| n)
                        .collect();
                    
                    // Get partition type from SHOW CREATE TABLE
                    let show_q = format!("SHOW CREATE TABLE `{}`", table_name.replace("`", "``"));
                    let partition_type = sqlx::query_as::<_, (String, String)>(&show_q)
                        .fetch_optional(mysql_pool.as_ref())
                        .await
                        .ok()
                        .flatten()
                        .and_then(|(_, create_sql)| {
                            // Parse for PARTITION BY <TYPE>
                            if let Some(partition_idx) = create_sql.to_uppercase().find("PARTITION BY") {
                                let after_partition = &create_sql[partition_idx + 12..];
                                after_partition
                                    .split_whitespace()
                                    .next()
                                    .map(|s| s.to_uppercase())
                            } else {
                                None
                            }
                        });
                    
                    partition_names.into_iter()
                        .map(|name| models::structs::PartitionStructInfo {
                            name,
                            partition_type: partition_type.clone(),
                            partition_expression: None,
                            subpartition_type: None,
                        })
                        .collect()
                } else { Vec::new() }
            })
        }
        models::enums::DatabaseType::PostgreSQL => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = crate::connection::get_or_create_connection_pool(tabular, connection_id).await {
                    // Get partition info from PostgreSQL
                    let q = "SELECT \n  c.relname AS partition_name,\n  CASE \n    WHEN p.relkind = 'p' THEN 'RANGE'\n    WHEN p.relkind = 'r' THEN (SELECT partstrat FROM pg_partitioned_table WHERE partrelid = p.oid LIMIT 1)\n    ELSE NULL\n  END AS partition_type\nFROM pg_class p\nJOIN pg_class c ON c.relfilenode = p.relfilenode OR (p.oid IN (SELECT partrelid FROM pg_partitioned_table WHERE partkeylen > 0))\nWHERE p.relname = $1 AND p.relkind IN ('p', 'r')\nORDER BY c.relname";
                    match sqlx::query_as::<_, (String, Option<String>)>(q)
                        .bind(table_name)
                        .fetch_all(pg_pool.as_ref())
                        .await {
                            Ok(rows) => rows.into_iter()
                                .map(|(name, ptype)| models::structs::PartitionStructInfo {
                                    name,
                                    partition_type: ptype,
                                    partition_expression: None,
                                    subpartition_type: None,
                                })
                                .collect(),
                            Err(_) => Vec::new(),
                        }
                } else { Vec::new() }
            })
        }
        _ => Vec::new(),
    }
}

// Execute a manual data refresh for current table and update row cache
pub(crate) fn refresh_current_table_data(tabular: &mut window_egui::Tabular) {
    // Stay in browse mode so spreadsheet shortcuts remain enabled after refreshes
    tabular.is_table_browse_mode = true;
    if tabular.use_server_pagination && !tabular.current_base_query.is_empty() {
        tabular.current_page = 0;
        debug!("🔄 Manual refresh: server pagination first page reloaded");
        tabular.execute_paginated_query();
        return;
    }

    if let Some(conn_id) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id)
    {
        let table = infer_current_table_name(tabular);
        if table.is_empty() {
            return;
        }
        let db_name = tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.database_name.clone())
            .unwrap_or_default();
        let db_type = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.connection_type.clone());
        if let Some(ct) = db_type {
            let query = match ct {
                models::enums::DatabaseType::MySQL => {
                    if db_name.is_empty() {
                        format!("SELECT * FROM `{}` LIMIT 100", table)
                    } else {
                        format!("USE `{}`;\nSELECT * FROM `{}` LIMIT 100", db_name, table)
                    }
                }
                models::enums::DatabaseType::PostgreSQL => {
                    if db_name.is_empty() {
                        format!("SELECT * FROM \"{}\" LIMIT 100", table)
                    } else {
                        format!("SELECT * FROM \"{}\".\"{}\" LIMIT 100", db_name, table)
                    }
                }
                models::enums::DatabaseType::SQLite => {
                    format!("SELECT * FROM `{}` LIMIT 100", table)
                }
                models::enums::DatabaseType::MsSQL => {
                    driver_mssql::build_mssql_select_query(db_name.clone(), table.clone())
                }
                _ => String::new(),
            };
            if !query.is_empty()
                && let Some((headers, data)) =
                    connection::execute_query_with_connection(tabular, conn_id, query)
            {
                tabular.current_table_headers = headers;
                tabular.current_table_data = data.clone();
                tabular.all_table_data = data;
                tabular.total_rows = tabular.all_table_data.len();
                tabular.current_page = 0;
                if let Some(active_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                    active_tab.result_headers = tabular.current_table_headers.clone();
                    active_tab.result_rows = tabular.current_table_data.clone();
                    active_tab.result_all_rows = tabular.all_table_data.clone();
                    active_tab.result_table_name = tabular.current_table_name.clone();
                    active_tab.is_table_browse_mode = true;
                    active_tab.current_page = tabular.current_page;
                    active_tab.page_size = tabular.page_size;
                    active_tab.total_rows = tabular.total_rows;
                }
                // Save refreshed first page to cache
                let snapshot: Vec<Vec<String>> =
                    tabular.all_table_data.iter().take(100).cloned().collect();
                let headers_clone = tabular.current_table_headers.clone();
                crate::cache_data::save_table_rows_to_cache(
                    tabular,
                    conn_id,
                    &db_name,
                    &table,
                    &headers_clone,
                    &snapshot,
                );
                debug!(
                    "💾 Cached first 100 rows after manual refresh for {}/{}",
                    db_name, table
                );
            }
        }
    }
}

// Detailed index metadata loader per database
fn fetch_index_details_for_table(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    table_name: &str,
) -> Vec<models::structs::IndexStructInfo> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = crate::connection::get_or_create_connection_pool(tabular, connection_id).await {
                        let q = r#"SELECT INDEX_NAME, GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX) AS COLS, MIN(NON_UNIQUE) AS NON_UNIQUE, GROUP_CONCAT(DISTINCT INDEX_TYPE) AS TYPES FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? GROUP BY INDEX_NAME ORDER BY INDEX_NAME"#;
                        match sqlx::query(q).bind(database_name).bind(table_name).fetch_all(mysql_pool.as_ref()).await {
                            Ok(rows) => { use sqlx::Row; rows.into_iter().map(|r| {
                                let name: String = r.get("INDEX_NAME");
                                let cols_str: Option<String> = r.try_get("COLS").ok();
                                let non_unique: Option<i64> = r.try_get("NON_UNIQUE").ok();
                                let types: Option<String> = r.try_get("TYPES").ok();
                                let columns = cols_str.unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                                let unique = matches!(non_unique, Some(0));
                                let method = types.and_then(|t| t.split(',').next().map(|m| m.trim().to_string())).filter(|s| !s.is_empty());
                                models::structs::IndexStructInfo { name, method, unique, columns }
                            }).collect() }
                            Err(_) => Vec::new(),
                        }
                    } else { Vec::new() }
                })
        }
        models::enums::DatabaseType::PostgreSQL => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                    if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = crate::connection::get_or_create_connection_pool(tabular, connection_id).await {
                        let q = r#"SELECT idx.relname AS index_name, pg_get_indexdef(i.indexrelid) AS index_def, i.indisunique AS is_unique FROM pg_class t JOIN pg_index i ON t.oid = i.indrelid JOIN pg_class idx ON idx.oid = i.indexrelid JOIN pg_namespace n ON n.oid = t.relnamespace WHERE t.relname = $1 AND n.nspname='public' ORDER BY idx.relname"#;
                        match sqlx::query(q).bind(table_name).fetch_all(pg_pool.as_ref()).await {
                            Ok(rows) => { use sqlx::Row; rows.into_iter().map(|r| {
                                let name: String = r.get("index_name");
                                let def: String = r.get("index_def");
                                let unique: bool = r.get("is_unique");
                                let method = def.split(" USING ").nth(1).and_then(|rest| rest.split_whitespace().next()).and_then(|m| if m.starts_with('('){None}else{Some(m.trim_matches('(').trim_matches(')').to_string())});
                                let columns: Vec<String> = if let Some(start) = def.rfind('(') { if let Some(end_rel) = def[start+1..].find(')') { def[start+1..start+1+end_rel].split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect() } else { Vec::new() } } else { Vec::new() };
                                models::structs::IndexStructInfo { name, method, unique, columns }
                            }).collect() }
                            Err(_) => Vec::new(),
                        }
                    } else { Vec::new() }
                })
        }
        models::enums::DatabaseType::MsSQL => {
            use tiberius::{AuthMethod, Config};
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let host = connection.host.clone();
            let port: u16 = connection.port.parse().unwrap_or(1433);
            let user = connection.username.clone();
            let pass = connection.password.clone();
            let db = database_name.to_string();
            let tbl = table_name.to_string();
            let rt_res = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let mut config = Config::new(); config.host(host.clone()); config.port(port); config.authentication(AuthMethod::sql_server(user.clone(), pass.clone())); config.trust_cert(); if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?; tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    let parse = |name: &str| -> (Option<String>, String) { if let Some((s,t)) = name.split_once('.') { (Some(s.trim_matches(['[',']']).to_string()), t.trim_matches(['[',']']).to_string()) } else { (None, name.trim_matches(['[',']']).to_string()) } };
                    let (_schema_opt, table_only) = parse(&tbl);
                    let q = format!("SELECT i.name AS index_name, i.is_unique, i.type_desc, STUFF((SELECT ','+c.name FROM sys.index_columns ic2 JOIN sys.columns c ON c.object_id=ic2.object_id AND c.column_id=ic2.column_id WHERE ic2.object_id=i.object_id AND ic2.index_id=i.index_id ORDER BY ic2.key_ordinal FOR XML PATH(''), TYPE).value('.','NVARCHAR(MAX)'),1,1,'') AS columns FROM sys.indexes i INNER JOIN sys.objects o ON o.object_id=i.object_id WHERE o.name='{}' AND i.name IS NOT NULL ORDER BY i.name", table_only.replace("'","''"));
                    let mut stream = client.simple_query(q).await.map_err(|e| e.to_string())?; use futures_util::TryStreamExt; use tiberius::QueryItem; let mut list = Vec::new();
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let QueryItem::Row(r) = item { let name: Option<&str> = r.get(0); let is_unique: Option<bool> = r.get(1); let type_desc: Option<&str> = r.get(2); let cols: Option<&str> = r.get(3); if let Some(nm)=name { list.push(models::structs::IndexStructInfo { name: nm.to_string(), method: type_desc.map(|s| s.to_string()), unique: is_unique.unwrap_or(false), columns: cols.unwrap_or("").split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect() }); } } }
                    Ok::<_, String>(list)
                });
            rt_res.unwrap_or_default()
        }
        models::enums::DatabaseType::SQLite => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Some(models::enums::DatabasePool::SQLite(sqlite_pool)) =
                    crate::connection::get_or_create_connection_pool(tabular, connection_id).await
                {
                    use sqlx::Row;
                    let list_query =
                        format!("PRAGMA index_list('{}')", table_name.replace("'", "''"));
                    match sqlx::query(&list_query)
                        .fetch_all(sqlite_pool.as_ref())
                        .await
                    {
                        Ok(rows) => {
                            let mut infos = Vec::new();
                            for r in rows {
                                let name_opt: Option<String> = r.try_get("name").ok().flatten();
                                let unique_flag: Option<i64> = r.try_get("unique").ok().flatten();
                                if let Some(nm) = name_opt {
                                    let info_q =
                                        format!("PRAGMA index_info('{}')", nm.replace("'", "''"));
                                    let mut cols_vec = Vec::new();
                                    if let Ok(crows) =
                                        sqlx::query(&info_q).fetch_all(sqlite_pool.as_ref()).await
                                    {
                                        for cr in crows {
                                            if let Ok(Some(coln)) =
                                                cr.try_get::<Option<String>, _>("name")
                                            {
                                                cols_vec.push(coln);
                                            }
                                        }
                                    }
                                    infos.push(models::structs::IndexStructInfo {
                                        name: nm,
                                        method: None,
                                        unique: matches!(unique_flag, Some(0)),
                                        columns: cols_vec,
                                    });
                                }
                            }
                            infos
                        }
                        Err(_) => Vec::new(),
                    }
                } else {
                    Vec::new()
                }
            })
        }
        models::enums::DatabaseType::MongoDB => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Some(models::enums::DatabasePool::MongoDB(client)) =
                    crate::connection::get_or_create_connection_pool(tabular, connection_id).await
                {
                    match client
                        .database(database_name)
                        .collection::<mongodb::bson::Document>(table_name)
                        .list_index_names()
                        .await
                    {
                        Ok(names) => names
                            .into_iter()
                            .map(|n| models::structs::IndexStructInfo {
                                name: n,
                                method: None,
                                unique: false,
                                columns: Vec::new(),
                            })
                            .collect(),
                        Err(_) => Vec::new(),
                    }
                } else {
                    Vec::new()
                }
            })
        }
        _ => Vec::new(),
    }
}

pub(crate) fn infer_current_table_name(tabular: &mut window_egui::Tabular) -> String {
    // Priority 0: Check metadata
    if let Some(meta) = &tabular.current_column_metadata {
        // Try to find a valid table name from any column
        for col in meta {
            if let Some(t) = &col.table_name
                && !t.is_empty()
            {
                return t.clone();
            }
        }
    }

    // Priority 1: if current_table_name starts with "Table:" extract
    if tabular.current_table_name.starts_with("Table:")
        || tabular.current_table_name.starts_with("View:")
    {
        let after = tabular
            .current_table_name
            .split_once(':')
            .map(|x| x.1)
            .unwrap_or("")
            .trim();
        let mut cut = after.to_string();
        if let Some(p) = cut.find('(') {
            cut = cut[..p].trim().to_string();
        }
        if !cut.is_empty() {
            return cut;
        }
    }
    // Priority 2: active tab title pattern
    let ttitle = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.title.clone())
        .unwrap_or_default();
    let mut table_guess = if ttitle.contains(':') {
        ttitle.split(':').nth(1).unwrap_or("").trim().to_string()
    } else {
        String::new()
    };
    if let Some(p) = table_guess.find('(') {
        table_guess = table_guess[..p].trim().to_string();
    }
    table_guess
}

