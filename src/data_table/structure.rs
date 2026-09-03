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
        let target_changed = tabular
            .last_structure_target
            .as_ref()
            .map(|t| t != &target)
            .unwrap_or(true);

        if !tabular.request_structure_refresh && !target_changed {
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

        // Reset current in-memory structure only if target actually changed or refresh requested
        if target_changed || tabular.request_structure_refresh {
            tabular.structure_columns.clear();
            tabular.structure_indexes.clear();
            tabular.structure_selected_row = None;
            tabular.structure_selected_cell = None;
            tabular.structure_sel_anchor = None;
        }

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

            // Always populate indexes from cache if available so switching to Indexes tab is instant
            if let Some(cached) = crate::cache_data::get_indexes_from_cache(
                tabular,
                conn_id,
                &database,
                &table_guess,
            ) {
                if !cached.is_empty() {
                    tabular.structure_indexes = cached;
                } else if tabular.structure_sub_view == models::structs::StructureSubView::Indexes {
                    need_fetch = true;
                }
            } else if tabular.structure_sub_view == models::structs::StructureSubView::Indexes {
                need_fetch = true;
            }
        }

        // 1.5) Fallback seed: if columns are still empty, immediately populate from current_table_headers
        // so user never sees a blank structure view while background query runs
        if tabular.structure_columns.is_empty() && !tabular.current_table_headers.is_empty() {
            for col_name in &tabular.current_table_headers {
                if !col_name.is_empty() {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name: col_name.clone(),
                            data_type: "varchar(255)".to_string(),
                            ..Default::default()
                        });
                }
            }
        }

        // 1.6) Fallback seed for indexes: if structure_indexes is still empty, seed PRIMARY index if there's an 'id' column
        if tabular.structure_indexes.is_empty() {
            let pk_col = tabular.structure_columns.iter().find(|c| {
                c.name.eq_ignore_ascii_case("id")
                    || c.extra.as_deref().unwrap_or("").to_lowercase().contains("auto_increment")
            });
            if let Some(col) = pk_col {
                tabular.structure_indexes.push(models::structs::IndexStructInfo {
                    name: "PRIMARY".to_string(),
                    method: Some("BTREE".to_string()),
                    unique: true,
                    columns: vec![col.name.clone()],
                });
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
            let port_num = target_port.parse::<u16>().unwrap_or(3306);
            let clean_db = if !database_name.trim().is_empty() {
                database_name.trim().trim_matches(['`', '"', '[', ']']).to_string()
            } else if !connection.database.trim().is_empty() {
                connection.database.trim().trim_matches(['`', '"', '[', ']']).to_string()
            } else {
                String::new()
            };
            let clean_table = table_name.trim().trim_matches(['`', '"', '[', ']']).to_string();

            let mut connect_opts = sqlx::mysql::MySqlConnectOptions::new()
                .host(&target_host)
                .port(port_num)
                .username(&connection.username)
                .password(&connection.password);

            if !clean_db.is_empty() {
                connect_opts = connect_opts.database(&clean_db);
            }

            if connection.ssl_enabled {
                let ssl_mode = if !connection.ssl_verify_server {
                    sqlx::mysql::MySqlSslMode::Required
                } else if !connection.ssl_ca_cert.trim().is_empty() {
                    sqlx::mysql::MySqlSslMode::VerifyCa
                } else {
                    sqlx::mysql::MySqlSslMode::Required
                };
                connect_opts = connect_opts.ssl_mode(ssl_mode);
                if !connection.ssl_ca_cert.trim().is_empty() {
                    connect_opts = connect_opts.ssl_ca(connection.ssl_ca_cert.trim());
                }
            } else {
                connect_opts = connect_opts.ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
            }

            if let Ok(pool) = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect_with(connect_opts)
                .await
            {
                let find_col_idx = |row: &sqlx::mysql::MySqlRow, col_target: &str| -> Option<usize> {
                    use sqlx::Column;
                    use sqlx::Row;
                    row.columns()
                        .iter()
                        .position(|c| c.name().eq_ignore_ascii_case(col_target))
                };
                let get_str = |row: &sqlx::mysql::MySqlRow, col: &str| -> Option<String> {
                    use sqlx::Row;
                    let idx = find_col_idx(row, col)?;
                    if let Ok(s) = row.try_get::<String, _>(idx) {
                        return Some(s);
                    }
                    if let Ok(b) = row.try_get::<Vec<u8>, _>(idx) {
                        return Some(String::from_utf8_lossy(&b).to_string());
                    }
                    None
                };
                let get_num = |row: &sqlx::mysql::MySqlRow, col: &str| -> Option<i64> {
                    use sqlx::Row;
                    let idx = find_col_idx(row, col)?;
                    if let Ok(v) = row.try_get::<i64, _>(idx) { return Some(v); }
                    if let Ok(v) = row.try_get::<i32, _>(idx) { return Some(v as i64); }
                    if let Ok(v) = row.try_get::<i16, _>(idx) { return Some(v as i64); }
                    if let Ok(v) = row.try_get::<i8, _>(idx) { return Some(v as i64); }
                    if let Ok(v) = row.try_get::<u64, _>(idx) { return Some(v as i64); }
                    if let Ok(v) = row.try_get::<u32, _>(idx) { return Some(v as i64); }
                    if let Ok(v) = row.try_get::<String, _>(idx) { return v.parse::<i64>().ok(); }
                    None
                };

                // Method 1 (Primary): SHOW INDEX FROM `db`.`table`
                let show_q = if !clean_db.is_empty() {
                    format!("SHOW INDEX FROM `{}`.`{}`", clean_db.replace('`', ""), clean_table.replace('`', ""))
                } else {
                    format!("SHOW INDEX FROM `{}`", clean_table.replace('`', ""))
                };

                if let Ok(rows) = sqlx::query(sqlx::AssertSqlSafe(show_q.as_str())).fetch_all(&pool).await {
                    let mut map: std::collections::BTreeMap<String, (Option<String>, bool, Vec<(i64, String)>)> = std::collections::BTreeMap::new();
                    for r in rows {
                        let key_name = get_str(&r, "Key_name").unwrap_or_default();
                        if key_name.is_empty() { continue; }
                        let col_name = get_str(&r, "Column_name").unwrap_or_default();
                        let non_unique = get_num(&r, "Non_unique").unwrap_or(1);
                        let index_type = get_str(&r, "Index_type");
                        let seq = get_num(&r, "Seq_in_index").unwrap_or(0);

                        let entry = map.entry(key_name).or_insert_with(|| (index_type, non_unique == 0, Vec::new()));
                        if !col_name.is_empty() {
                            entry.2.push((seq, col_name));
                        }
                    }
                    if !map.is_empty() {
                        let mut list = Vec::new();
                        for (k_name, (itype, is_unique, mut cols)) in map {
                            cols.sort_by_key(|c| c.0);
                            let col_names: Vec<String> = cols.into_iter().map(|c| c.1).collect();
                            list.push(models::structs::IndexStructInfo {
                                name: k_name,
                                method: itype,
                                unique: is_unique,
                                columns: col_names,
                            });
                        }
                        list.sort_by(|a, b| {
                            if a.name == "PRIMARY" {
                                std::cmp::Ordering::Less
                            } else if b.name == "PRIMARY" {
                                std::cmp::Ordering::Greater
                            } else {
                                a.name.cmp(&b.name)
                            }
                        });
                        return list;
                    }
                }

                // Method 2 (Fallback): INFORMATION_SCHEMA.STATISTICS
                let q = r#"SELECT INDEX_NAME, COLUMN_NAME, SEQ_IN_INDEX, NON_UNIQUE, INDEX_TYPE FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX"#;
                if let Ok(rows) = sqlx::query(q).bind(&clean_db).bind(&clean_table).fetch_all(&pool).await {
                    let mut map: std::collections::BTreeMap<String, (Option<String>, bool, Vec<(i64, String)>)> = std::collections::BTreeMap::new();
                    for r in rows {
                        let key_name = get_str(&r, "INDEX_NAME").unwrap_or_default();
                        if key_name.is_empty() { continue; }
                        let col_name = get_str(&r, "COLUMN_NAME").unwrap_or_default();
                        let non_unique = get_num(&r, "NON_UNIQUE").unwrap_or(1);
                        let index_type = get_str(&r, "INDEX_TYPE");
                        let seq = get_num(&r, "SEQ_IN_INDEX").unwrap_or(0);

                        let entry = map.entry(key_name).or_insert_with(|| (index_type, non_unique == 0, Vec::new()));
                        if !col_name.is_empty() {
                            entry.2.push((seq, col_name));
                        }
                    }
                    if !map.is_empty() {
                        let mut list = Vec::new();
                        for (k_name, (itype, is_unique, mut cols)) in map {
                            cols.sort_by_key(|c| c.0);
                            let col_names: Vec<String> = cols.into_iter().map(|c| c.1).collect();
                            list.push(models::structs::IndexStructInfo {
                                name: k_name,
                                method: itype,
                                unique: is_unique,
                                columns: col_names,
                            });
                        }
                        list.sort_by(|a, b| {
                            if a.name == "PRIMARY" {
                                std::cmp::Ordering::Less
                            } else if b.name == "PRIMARY" {
                                std::cmp::Ordering::Greater
                            } else {
                                a.name.cmp(&b.name)
                            }
                        });
                        return list;
                    }
                }
            }
            Vec::new()
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
                let (schema_name, raw_table) = if let Some((s, t)) = table_name.split_once('.') {
                    (s.trim_matches('"'), t.trim_matches('"'))
                } else if !database_name.is_empty() && database_name != connection.database {
                    (database_name, table_name.trim_matches('"'))
                } else {
                    ("public", table_name.trim_matches('"'))
                };
                let q = r#"SELECT idx.relname AS index_name, pg_get_indexdef(i.indexrelid) AS index_def, i.indisunique AS is_unique FROM pg_class t JOIN pg_index i ON t.oid = i.indrelid JOIN pg_class idx ON idx.oid = i.indexrelid JOIN pg_namespace n ON n.oid = t.relnamespace WHERE t.relname = $1 AND (n.nspname = $2 OR $2 = '') ORDER BY idx.relname"#;
                match sqlx::query(q).bind(raw_table).bind(schema_name).fetch_all(&pool).await {
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
                let clean_table = table_name.trim_matches(['`', '"', '[', ']']).replace('\'', "''");
                let list_query = format!("PRAGMA index_list('{}')", clean_table);
                let mut infos = Vec::new();

                // 1) First check primary key from table_info
                let info_table_q = format!("PRAGMA table_info('{}')", clean_table);
                if let Ok(prows) = sqlx::query(sqlx::AssertSqlSafe(info_table_q.as_str())).fetch_all(&pool).await {
                    let mut pk_cols: Vec<(i64, String)> = Vec::new();
                    for pr in prows {
                        let pk_order: i64 = pr.try_get("pk").unwrap_or(0);
                        if pk_order > 0 {
                            if let Ok(col_name) = pr.try_get::<String, _>("name") {
                                pk_cols.push((pk_order, col_name));
                            }
                        }
                    }
                    if !pk_cols.is_empty() {
                        pk_cols.sort_by_key(|k| k.0);
                        let pk_col_names: Vec<String> = pk_cols.into_iter().map(|k| k.1).collect();
                        infos.push(models::structs::IndexStructInfo {
                            name: "PRIMARY".to_string(),
                            method: Some("PRIMARY KEY".to_string()),
                            unique: true,
                            columns: pk_col_names,
                        });
                    }
                }

                // 2) Check regular & unique indexes
                if let Ok(rows) = sqlx::query(sqlx::AssertSqlSafe(list_query.as_str())).fetch_all(&pool).await {
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
                            // Don't duplicate if already added as PRIMARY
                            let is_already_added = infos.iter().any(|existing| existing.name == nm || (existing.name == "PRIMARY" && existing.columns == cols_vec));
                            if !is_already_added {
                                infos.push(models::structs::IndexStructInfo {
                                    name: nm,
                                    method: None,
                                    unique: matches!(unique_flag, Some(1)),
                                    columns: cols_vec,
                                });
                            }
                        }
                    }
                }
                return infos;
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

fn extract_table_from_caption(caption: &str) -> Option<String> {
    let trimmed = caption.trim();
    if trimmed.is_empty() {
        return None;
    }
    let after = if let Some((_, rest)) = trimmed.split_once(':') {
        rest.trim()
    } else {
        trimmed
    };
    let mut cut = after.to_string();
    if let Some(p) = cut.find('(') {
        cut = cut[..p].trim().to_string();
    }
    let mut clean = cut
        .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']')
        .trim();
    if let Some(rest) = clean.strip_prefix("Loading ") {
        clean = rest.trim_end_matches('.').trim();
    }
    let clean = clean
        .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']')
        .trim();
    if !clean.is_empty()
        && !clean.starts_with("Query Results")
        && !clean.starts_with("Query executed")
        && !clean.starts_with("Running query")
        && !clean.starts_with("Connecting")
        && !clean.starts_with("Loading")
    {
        Some(clean.to_string())
    } else {
        None
    }
}

pub(crate) fn infer_current_table_name(tabular: &mut window_egui::Tabular) -> String {
    // Priority 1: Check active_tab.result_table_name (e.g. "Table: users (Database: mydb)")
    if let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index) {
        if let Some(t) = extract_table_from_caption(&tab.result_table_name) {
            return t;
        }
    }

    // Priority 2: Check tabular.current_table_name if it starts with Table: or View:
    if tabular.current_table_name.starts_with("Table:")
        || tabular.current_table_name.starts_with("View:")
    {
        if let Some(t) = extract_table_from_caption(&tabular.current_table_name) {
            return t;
        }
    }

    // Priority 3: Check active tab title (e.g. "Table: users")
    if let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index) {
        if let Some(t) = extract_table_from_caption(&tab.title) {
            if !t.starts_with("Query ")
                && !t.starts_with("New Tab")
                && !t.starts_with("Untitled")
                && !t.starts_with("Redis ")
            {
                return t;
            }
        }
    }

    // Priority 4: Check metadata only as fallback
    if let Some(meta) = &tabular.current_column_metadata {
        for col in meta {
            if let Some(t) = &col.table_name
                && !t.is_empty()
            {
                return t.clone();
            }
        }
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_table_from_caption_standard() {
        assert_eq!(
            extract_table_from_caption("Table: users (Database: mydb)"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_table_from_caption("View: active_orders (Database: store)"),
            Some("active_orders".to_string())
        );
        assert_eq!(
            extract_table_from_caption("Table: `products` (Database: shop)"),
            Some("products".to_string())
        );
        assert_eq!(
            extract_table_from_caption("Table: [customers] (Database: crm)"),
            Some("customers".to_string())
        );
        assert_eq!(
            extract_table_from_caption("Table: \"accounts\""),
            Some("accounts".to_string())
        );
        assert_eq!(
            extract_table_from_caption("Table: users"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_table_from_caption("Loading datalogs..."),
            Some("datalogs".to_string())
        );
    }

    #[test]
    fn test_extract_table_from_caption_query_results_ignored() {
        assert_eq!(
            extract_table_from_caption("Query Results (page 1 showing 100 rows)"),
            None
        );
        assert_eq!(
            extract_table_from_caption("Query executed successfully (0.02s)"),
            None
        );
        assert_eq!(
            extract_table_from_caption("Connecting… waiting for pool"),
            None
        );
        assert_eq!(extract_table_from_caption(""), None);
    }
}


