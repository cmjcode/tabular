use log::debug;
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
                    return;
                }
                models::structs::StructureSubView::Indexes
                    if !tabular.structure_indexes.is_empty() =>
                {
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

        let is_refresh = tabular.request_structure_refresh;
        tabular.request_structure_refresh = false;

        let mut need_fetch = is_refresh;

        if !is_refresh {
            // 1) Try to populate from cache immediately for instant UI (0ms blocking)
            if let Some(cols) =
                crate::cache_data::get_columns_from_cache(tabular, conn_id, &database, &table_guess)
            {
                if !cols.is_empty() {
                    for (name, dtype) in cols {
                        tabular
                            .structure_columns
                            .push(models::structs::ColumnStructInfo {
                                name,
                                data_type: dtype,
                                ..Default::default()
                            });
                    }
                } else {
                    need_fetch = true;
                }
            } else {
                need_fetch = true;
            }

            if tabular.structure_sub_view == models::structs::StructureSubView::Indexes {
                if let Some(cached) = crate::cache_data::get_indexes_from_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                ) {
                    if !cached.is_empty() {
                        tabular.structure_indexes = cached;
                    } else {
                        need_fetch = true;
                    }
                } else {
                    need_fetch = true;
                }
            }
        }

        // 2) If not in cache or user requested manual refresh, dispatch background fetch non-blocking
        if need_fetch {
            tabular.is_refreshing_structure = true;
            if let Some(sender) = &tabular.background_sender {
                let _ = sender.send(models::enums::BackgroundTask::FetchTableStructure {
                    connection_id: conn_id,
                    database_name: database.clone(),
                    table_name: table_guess.clone(),
                });
            }
        }

        tabular.last_structure_target = Some((conn_id, database, table_guess));
    }
}

pub async fn fetch_partition_details_standalone_async(
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    table_name: &str,
) -> Vec<models::structs::PartitionStructInfo> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let (target_host, target_port) = match crate::connection::pool::resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(_) => return Vec::new(),
            };
            let encoded_username = crate::modules::url_encode(&connection.username);
            let encoded_password = crate::modules::url_encode(&connection.password);
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                encoded_username, encoded_password, target_host, target_port, database_name
            );
            if let Ok(pool) = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(3))
                .connect(&connection_string)
                .await
            {
                let names_q = "SELECT PARTITION_NAME FROM INFORMATION_SCHEMA.PARTITIONS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND PARTITION_NAME IS NOT NULL AND SUBPARTITION_NAME IS NULL ORDER BY PARTITION_ORDINAL_POSITION";
                let partition_names: Vec<String> = sqlx::query_as::<_, (String,)>(names_q)
                    .bind(database_name)
                    .bind(table_name)
                    .fetch_all(&pool)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(n,)| n)
                    .collect();

                let show_q = format!("SHOW CREATE TABLE `{}`", table_name.replace('`', "``"));
                let partition_type = sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(show_q.as_str()))
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|(_, create_sql)| {
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

                partition_names
                    .into_iter()
                    .map(|name| models::structs::PartitionStructInfo {
                        name,
                        partition_type: partition_type.clone(),
                        partition_expression: None,
                        subpartition_type: None,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match crate::connection::pool::resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(_) => return Vec::new(),
            };
            let encoded_username = crate::modules::url_encode(&connection.username);
            let encoded_password = crate::modules::url_encode(&connection.password);
            let connection_string = format!(
                "postgres://{}:{}@{}:{}/{}",
                encoded_username, encoded_password, target_host, target_port, database_name
            );
            if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(3))
                .connect(&connection_string)
                .await
            {
                let q = "SELECT \n  c.relname AS partition_name,\n  CASE \n    WHEN p.relkind = 'p' THEN 'RANGE'\n    WHEN p.relkind = 'r' THEN (SELECT partstrat FROM pg_partitioned_table WHERE partrelid = p.oid LIMIT 1)\n    ELSE NULL\n  END AS partition_type\nFROM pg_class p\nJOIN pg_class c ON c.relfilenode = p.relfilenode OR (p.oid IN (SELECT partrelid FROM pg_partitioned_table WHERE partkeylen > 0))\nWHERE p.relname = $1 AND p.relkind IN ('p', 'r')\nORDER BY c.relname";
                match sqlx::query_as::<_, (String, Option<String>)>(q)
                    .bind(table_name)
                    .fetch_all(&pool)
                    .await
                {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|(name, ptype)| models::structs::PartitionStructInfo {
                            name,
                            partition_type: ptype,
                            partition_expression: None,
                            subpartition_type: None,
                        })
                        .collect(),
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

pub async fn fetch_index_details_standalone_async(
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    table_name: &str,
) -> Vec<models::structs::IndexStructInfo> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let (target_host, target_port) = match crate::connection::pool::resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(_) => return Vec::new(),
            };
            let encoded_username = crate::modules::url_encode(&connection.username);
            let encoded_password = crate::modules::url_encode(&connection.password);
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                encoded_username, encoded_password, target_host, target_port, database_name
            );
            if let Ok(pool) = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(3))
                .connect(&connection_string)
                .await
            {
                let q = r#"SELECT INDEX_NAME, GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX) AS COLS, MIN(NON_UNIQUE) AS NON_UNIQUE, GROUP_CONCAT(DISTINCT INDEX_TYPE) AS TYPES FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? GROUP BY INDEX_NAME ORDER BY INDEX_NAME"#;
                match sqlx::query(q).bind(database_name).bind(table_name).fetch_all(&pool).await {
                    Ok(rows) => {
                        use sqlx::Row;
                        rows.into_iter().map(|r| {
                            let name: String = r.get("INDEX_NAME");
                            let cols_str: Option<String> = r.try_get("COLS").ok();
                            let non_unique: Option<i64> = r.try_get("NON_UNIQUE").ok();
                            let types: Option<String> = r.try_get("TYPES").ok();
                            let columns = cols_str.unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                            let unique = matches!(non_unique, Some(0));
                            let method = types.and_then(|t| t.split(',').next().map(|m| m.trim().to_string())).filter(|s| !s.is_empty());
                            models::structs::IndexStructInfo { name, method, unique, columns }
                        }).collect()
                    }
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match crate::connection::pool::resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(_) => return Vec::new(),
            };
            let encoded_username = crate::modules::url_encode(&connection.username);
            let encoded_password = crate::modules::url_encode(&connection.password);
            let connection_string = format!(
                "postgres://{}:{}@{}:{}/{}",
                encoded_username, encoded_password, target_host, target_port, database_name
            );
            if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(3))
                .connect(&connection_string)
                .await
            {
                let q = r#"SELECT idx.relname AS index_name, pg_get_indexdef(i.indexrelid) AS index_def, i.indisunique AS is_unique FROM pg_class t JOIN pg_index i ON t.oid = i.indrelid JOIN pg_class idx ON idx.oid = i.indexrelid JOIN pg_namespace n ON n.oid = t.relnamespace WHERE t.relname = $1 AND n.nspname='public' ORDER BY idx.relname"#;
                match sqlx::query(q).bind(table_name).fetch_all(&pool).await {
                    Ok(rows) => {
                        use sqlx::Row;
                        rows.into_iter().map(|r| {
                            let name: String = r.get("index_name");
                            let def: String = r.get("index_def");
                            let unique: bool = r.get("is_unique");
                            let method = def.split(" USING ").nth(1).and_then(|rest| rest.split_whitespace().next()).and_then(|m| if m.starts_with('('){None}else{Some(m.trim_matches('(').trim_matches(')').to_string())});
                            let columns: Vec<String> = if let Some(start) = def.rfind('(') { if let Some(end_rel) = def[start+1..].find(')') { def[start+1..start+1+end_rel].split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect() } else { Vec::new() } } else { Vec::new() };
                            models::structs::IndexStructInfo { name, method, unique, columns }
                        }).collect()
                    }
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            }
        }
        models::enums::DatabaseType::MsSQL => {
            let host = connection.host.clone();
            let port: u16 = connection.port.parse().unwrap_or(1433);
            let user = connection.username.clone();
            let pass = connection.password.clone();
            let db = database_name.to_string();
            let tbl = table_name.to_string();
            if let Ok(mut client) = crate::driver_mssql::connect_mssql(&host, port, &user, &pass, Some(&db)).await {
                let parse = |name: &str| -> (Option<String>, String) { if let Some((s,t)) = name.split_once('.') { (Some(s.trim_matches(['[',']']).to_string()), t.trim_matches(['[',']']).to_string()) } else { (None, name.trim_matches(['[',']']).to_string()) } };
                let (_schema_opt, table_only) = parse(&tbl);
                let q = format!("SELECT i.name AS index_name, i.is_unique, i.type_desc, STUFF((SELECT ','+c.name FROM sys.index_columns ic2 JOIN sys.columns c ON c.object_id=ic2.object_id AND c.column_id=ic2.column_id WHERE ic2.object_id=i.object_id AND ic2.index_id=i.index_id ORDER BY ic2.key_ordinal FOR XML PATH(''), TYPE).value('.','NVARCHAR(MAX)'),1,1,'') AS columns FROM sys.indexes i INNER JOIN sys.objects o ON o.object_id=i.object_id WHERE o.name='{}' AND i.name IS NOT NULL ORDER BY i.name", table_only.replace('\'',"''"));
                if let Ok(stream) = client.query(&q, &[]).await {
                    if let Ok(records) = stream.collect_all().await {
                        let mut list = Vec::new();
                        for r in records {
                            let name = r.get_string(0);
                            let is_unique: Option<bool> = r.try_get(1).ok().flatten();
                            let type_desc = r.get_string(2);
                            let cols = r.get_string(3);
                            if let Some(nm) = name {
                                list.push(models::structs::IndexStructInfo { name: nm, method: type_desc, unique: is_unique.unwrap_or(false), columns: cols.unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect() });
                            }
                        }
                        return list;
                    }
                }
            }
            Vec::new()
        }
        models::enums::DatabaseType::SQLite => {
            let sqlite_path = if !connection.database.trim().is_empty() {
                connection.database.trim()
            } else if !connection.host.trim().is_empty() && connection.host.trim() != "localhost" {
                connection.host.trim()
            } else {
                connection.database.trim()
            };
            let connection_string = if sqlite_path.starts_with("sqlite:") {
                sqlite_path.to_string()
            } else {
                format!("sqlite:{}", sqlite_path)
            };
            if let Ok(pool) = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(3))
                .connect(&connection_string)
                .await
            {
                use sqlx::Row;
                let list_query = format!("PRAGMA index_list('{}')", table_name.replace('\'', "''"));
                if let Ok(rows) = sqlx::query(sqlx::AssertSqlSafe(list_query.as_str())).fetch_all(&pool).await {
                    let mut infos = Vec::new();
                    for r in rows {
                        let name_opt: Option<String> = r.try_get("name").ok().flatten();
                        let unique_flag: Option<i64> = r.try_get("unique").ok().flatten();
                        if let Some(nm) = name_opt {
                            let info_q = format!("PRAGMA index_info('{}')", nm.replace('\'', "''"));
                            let mut cols_vec = Vec::new();
                            if let Ok(crows) = sqlx::query(sqlx::AssertSqlSafe(info_q.as_str())).fetch_all(&pool).await {
                                for cr in crows {
                                    if let Ok(Some(coln)) = cr.try_get::<Option<String>, _>("name") {
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
                    return infos;
                }
            }
            Vec::new()
        }
        models::enums::DatabaseType::MongoDB => {
            let client_opts = mongodb::options::ClientOptions::parse(&connection.host).await.ok();
            if let Some(opts) = client_opts {
                if let Ok(client) = mongodb::Client::with_options(opts) {
                    if let Ok(names) = client
                        .database(database_name)
                        .collection::<mongodb::bson::Document>(table_name)
                        .list_index_names()
                        .await
                    {
                        return names
                            .into_iter()
                            .map(|n| models::structs::IndexStructInfo {
                                name: n,
                                method: None,
                                unique: false,
                                columns: Vec::new(),
                            })
                            .collect();
                    }
                }
            }
            Vec::new()
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

