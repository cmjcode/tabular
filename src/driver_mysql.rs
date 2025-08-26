use log::{debug, error};
use sqlx::MySqlPool;
use sqlx::{Column, Row, SqlitePool};

use crate::{connection, models, window_egui};

// Helper function for final fallback when all type-specific conversions fail
fn get_value_as_string_fallback(
    row: &sqlx::mysql::MySqlRow,
    column_name: &str,
    type_name: &str,
) -> String {
    // Simple fallback: try string conversion with both column name and index
    // Try column index instead of name (more reliable)
    let column_index = match row
        .columns()
        .iter()
        .position(|col| col.name() == column_name)
    {
        Some(idx) => idx,
        None => {
            debug!("Column '{}' not found in row", column_name);
            return format!("[COLUMN_NOT_FOUND:{}]", column_name);
        }
    };

    // Try with column index first
    if let Ok(Some(val)) = row.try_get::<Option<String>, _>(column_index) {
        return val;
    }
    if let Ok(val) = row.try_get::<String, _>(column_index) {
        return val;
    }

    // Try with column name as fallback
    match row.try_get::<Option<String>, _>(column_name) {
        Ok(Some(val)) => val,
        Ok(None) => "NULL".to_string(),
        Err(_) => format!("[CONVERSION_ERROR:{}]", type_name),
    }
}

// New: index-first fallback to avoid ColumnNotFound issues
fn get_value_as_string_fallback_idx(
    row: &sqlx::mysql::MySqlRow,
    idx: usize,
    column_name: &str,
    type_name: &str,
) -> String {
    // Try String by index
    if let Ok(Some(val)) = row.try_get::<Option<String>, _>(idx) {
        return val;
    }
    if let Ok(val) = row.try_get::<String, _>(idx) {
        return val;
    }
    // Try bytes by index (attempt to decode to text if possible)
    if let Ok(Some(val)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return bytes_to_string_or_marker(val);
    }
    if let Ok(val) = row.try_get::<Vec<u8>, _>(idx) {
        return bytes_to_string_or_marker(val);
    }
    // Try some generic numeric types before giving up
    if let Ok(Some(val)) = row.try_get::<Option<i64>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<u64>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<i32>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<u32>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<f64>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx) {
        return val.to_rfc3339();
    }
    if let Ok(Some(val)) = row.try_get::<Option<rust_decimal::Decimal>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<chrono::NaiveDate>, _>(idx) {
        return val.to_string();
    }
    if let Ok(Some(val)) = row.try_get::<Option<chrono::NaiveTime>, _>(idx) {
        return val.to_string();
    }
    // Fall back to name-based fallback (may still fail, but gives a consistent marker)
    get_value_as_string_fallback(row, column_name, type_name)
}

// Heuristic: treat bytes as text if they are mostly printable
fn looks_textual(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    let mut printable = 0usize;
    for &b in bytes {
        if (0x20..=0x7E).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t' {
            printable += 1;
        }
    }
    (printable as f32) / (bytes.len() as f32) > 0.85
}

fn bytes_to_string_or_marker(bytes: Vec<u8>) -> String {
    // Trim trailing NULs often present in MySQL BINARY/VARBINARY padding
    let mut b = bytes;
    while matches!(b.last(), Some(0)) {
        b.pop();
    }
    if b.is_empty() {
        return String::new();
    }

    if looks_textual(&b) {
        String::from_utf8_lossy(&b).into_owned()
    } else {
        // Show as hex instead of a vague [BINARY:n bytes]
        let mut s = String::with_capacity(2 + b.len() * 2);
        s.push_str("0x");
        for byte in &b {
            use std::fmt::Write as _;
            let _ = write!(&mut s, "{:02X}", byte);
        }
        s
    }
}

// Helper function to convert MySQL rows to Vec<Vec<String>> with proper type checking
pub(crate) fn convert_mysql_rows_to_table_data(
    rows: Vec<sqlx::mysql::MySqlRow>,
) -> Vec<Vec<String>> {
    use sqlx::{Column, Row, TypeInfo};

    let mut table_data = Vec::new();

    for row in &rows {
        let mut row_data = Vec::new();
        let columns = row.columns();

        for (idx, column) in columns.iter().enumerate() {
            let column_name = column.name();
            let type_info = column.type_info();
            let type_name = type_info.name();
            let t = type_name.to_ascii_uppercase(); // case-insensitive match

            let value_str = match t.as_str() {
                // Integer types
                "TINYINT" => match row.try_get::<Option<i8>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },
                "SMALLINT" => match row.try_get::<Option<i16>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },
                "MEDIUMINT" | "INT" | "INTEGER" => match row.try_get::<Option<i32>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },
                "BIGINT" => match row.try_get::<Option<i64>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },

                // Unsigned integer types
                "TINYINT UNSIGNED" => match row.try_get::<Option<u8>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },
                "SMALLINT UNSIGNED" => match row.try_get::<Option<u16>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },
                "MEDIUMINT UNSIGNED" | "INT UNSIGNED" | "INTEGER UNSIGNED" => {
                    match row.try_get::<Option<u32>, _>(idx) {
                        Ok(Some(val)) => val.to_string(),
                        Ok(None) => "NULL".to_string(),
                        Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                    }
                }
                "BIGINT UNSIGNED" => {
                    // Prefer u64 for BIGINT UNSIGNED
                    match row.try_get::<Option<u64>, _>(idx) {
                        Ok(Some(val)) => val.to_string(),
                        Ok(None) => "NULL".to_string(),
                        Err(er) => {
                            debug!(
                                "BIGINT UNSIGNED conversion error for column '{}'",
                                column_name
                            );
                            error!("Error: {:?}", er);
                            // Try signed as a fallback (if fits) before string fallback
                            match row.try_get::<Option<i64>, _>(idx) {
                                Ok(Some(val)) => val.to_string(),
                                Ok(None) => "NULL".to_string(),
                                Err(_) => {
                                    get_value_as_string_fallback_idx(row, idx, column_name, &t)
                                }
                            }
                        }
                    }
                }

                // Floating point types
                ,
                "FLOAT" => match row.try_get::<Option<f32>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },
                "DOUBLE" | "REAL" => match row.try_get::<Option<f64>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },

                // Decimal types - use rust_decimal for proper handling
                "DECIMAL" | "NUMERIC" | "NEWDECIMAL" => {
                    if let Ok(Some(val)) = row.try_get::<Option<rust_decimal::Decimal>, _>(idx) {
                        val.to_string()
                    } else if let Ok(val) = row.try_get::<rust_decimal::Decimal, _>(idx) {
                        val.to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<String>, _>(idx) {
                        val
                    } else if let Ok(val) = row.try_get::<String, _>(idx) {
                        val
                    } else if let Ok(Some(val)) = row.try_get::<Option<f64>, _>(idx) {
                        val.to_string()
                    } else if let Ok(val) = row.try_get::<f64, _>(idx) {
                        val.to_string()
                    } else {
                        get_value_as_string_fallback_idx(row, idx, column_name, &t)
                    }
                }

                // String types
                ,
                "VARCHAR" | "CHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM"
                | "SET" | "VAR_STRING" | "STRING" => match row.try_get::<Option<String>, _>(idx) {
                    Ok(Some(val)) => val,
                    Ok(None) => "NULL".to_string(),
                    Err(_) => {
                        // Some drivers may expose these as bytes, try to decode
                        if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                            bytes_to_string_or_marker(bytes)
                        } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
                            bytes_to_string_or_marker(bytes)
                        } else {
                            get_value_as_string_fallback_idx(row, idx, column_name, &t)
                        }
                    }
                },

                // Binary types
                "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
                    match row.try_get::<Option<Vec<u8>>, _>(idx) {
                        Ok(Some(val)) => bytes_to_string_or_marker(val),
                        Ok(None) => "NULL".to_string(),
                        Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                    }
                }

                // Bit type (format as integer or bit-string)
                "BIT" => {
                    if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                        use std::fmt::Write as _;
                        let mut s = String::with_capacity(bytes.len() * 8 + 2);
                        s.push_str("0b");
                        for b in bytes {
                            let _ = write!(&mut s, "{:08b}", b);
                        }
                        s
                    } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
                        use std::fmt::Write as _;
                        let mut s = String::with_capacity(bytes.len() * 8 + 2);
                        s.push_str("0b");
                        for b in bytes {
                            let _ = write!(&mut s, "{:08b}", b);
                        }
                        s
                    } else if let Ok(Some(val)) = row.try_get::<Option<u64>, _>(idx) {
                        format!("0b{:b}", val)
                    } else {
                        get_value_as_string_fallback_idx(row, idx, column_name, &t)
                    }
                }

                // Date and time types
                "DATE" => match row.try_get::<Option<chrono::NaiveDate>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => match row.try_get::<Option<String>, _>(idx) {
                        Ok(Some(val)) => val,
                        Ok(None) => "NULL".to_string(),
                        Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                    },
                },
                "TIME" => match row.try_get::<Option<chrono::NaiveTime>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => match row.try_get::<Option<String>, _>(idx) {
                        Ok(Some(val)) => val,
                        Ok(None) => "NULL".to_string(),
                        Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                    },
                },
                "DATETIME" | "TIMESTAMP" => {
                    // Try chrono::NaiveDateTime first
                    if let Ok(Some(val)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) {
                        val.to_string()
                    } else if let Ok(Some(val)) =
                        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx)
                    {
                        val.to_rfc3339()
                    } else if let Ok(Some(val)) = row.try_get::<Option<String>, _>(idx) {
                        val
                    } else if let Ok(val) = row.try_get::<String, _>(idx) {
                        val
                    } else if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                        bytes_to_string_or_marker(bytes)
                    } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
                        bytes_to_string_or_marker(bytes)
                    } else {
                        get_value_as_string_fallback_idx(row, idx, column_name, &t)
                    }
                }
                "YEAR" => match row.try_get::<Option<i16>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                },

                // Boolean type
                "BOOLEAN" | "BOOL" => match row.try_get::<Option<bool>, _>(idx) {
                    Ok(Some(val)) => val.to_string(),
                    Ok(None) => "NULL".to_string(),
                    Err(_) => match row.try_get::<Option<i8>, _>(idx) {
                        Ok(Some(val)) => (val != 0).to_string(),
                        Ok(None) => "NULL".to_string(),
                        Err(_) => get_value_as_string_fallback_idx(row, idx, column_name, &t),
                    },
                },

                // JSON type
                "JSON" => {
                    if let Ok(Some(val)) = row.try_get::<Option<String>, _>(idx) {
                        val
                    } else if let Ok(val) = row.try_get::<String, _>(idx) {
                        val
                    } else if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                        bytes_to_string_or_marker(bytes)
                    } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
                        bytes_to_string_or_marker(bytes)
                    } else {
                        get_value_as_string_fallback_idx(row, idx, column_name, &t)
                    }
                }

                // Default
                _ => match row.try_get::<Option<String>, _>(idx) {
                    Ok(Some(val)) => val,
                    Ok(None) => "NULL".to_string(),
                    Err(_) => {
                        // If not directly convertible to String, try bytes -> text
                        if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                            bytes_to_string_or_marker(bytes)
                        } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
                            bytes_to_string_or_marker(bytes)
                        } else {
                            get_value_as_string_fallback_idx(row, idx, column_name, &t)
                        }
                    }
                },
            };

            row_data.push(value_str);
        }
        table_data.push(row_data);
    }

    table_data
}

pub(crate) async fn fetch_mysql_data(
    connection_id: i64,
    pool: &MySqlPool,
    cache_pool: &SqlitePool,
) -> bool {
    // Helper to decode a value in a row as String or bytes -> UTF8
    fn decode_cell(row: &sqlx::mysql::MySqlRow, idx: usize) -> Option<String> {
        if let Ok(s) = row.try_get::<String, _>(idx) {
            return Some(s);
        }
        if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
            return Some(s);
        }
        if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }
        if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }
        None
    }

    // Fetch databases via INFORMATION_SCHEMA and skip system schemas (robust to VARBINARY)
    let db_rows_res = sqlx::query("SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA")
        .fetch_all(pool)
        .await;

    let db_rows = match db_rows_res {
        Ok(r) => r,
        Err(e) => {
            debug!("MySQL fetch_mysql_data: failed to list schemata: {}", e);
            return false;
        }
    };

    for row in db_rows.into_iter() {
        let db_name = match decode_cell(&row, 0) {
            Some(v) => v,
            None => continue,
        };
        if ["information_schema", "performance_schema", "mysql", "sys"].contains(&db_name.as_str())
        {
            continue;
        }

        // Cache database
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
        )
        .bind(connection_id)
        .bind(&db_name)
        .execute(cache_pool)
        .await;

        // Fetch base tables using INFORMATION_SCHEMA
        let tables_res = sqlx::query(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME"
        )
        .bind(&db_name)
        .fetch_all(pool)
        .await;

        let table_rows = match tables_res {
            Ok(r) => r,
            Err(e) => {
                debug!(
                    "MySQL fetch_mysql_data: failed to list tables in {}: {}",
                    db_name, e
                );
                continue;
            }
        };

        for row in table_rows.into_iter() {
            let table_name = match decode_cell(&row, 0) {
                Some(v) => v,
                None => continue,
            };
            // Cache table (include table_type column we previously omitted) => table_type = 'table'
            if let Err(e) = sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)" )
                .bind(connection_id)
                .bind(&db_name)
                .bind(&table_name)
                .bind("table")
                .execute(cache_pool)
                .await {
                debug!("MySQL fetch_mysql_data: failed to cache table {}.{}: {}", db_name, table_name, e);
            }

            // Fetch columns using INFORMATION_SCHEMA
            let cols_res = sqlx::query(
                "SELECT COLUMN_NAME, DATA_TYPE, ORDINAL_POSITION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION"
            )
            .bind(&db_name)
            .bind(&table_name)
            .fetch_all(pool)
            .await;

            if let Ok(cols) = cols_res {
                let mut count = 0usize;
                for row_c in cols {
                    let col_name = decode_cell(&row_c, 0).unwrap_or_default();
                    let col_type = decode_cell(&row_c, 1).unwrap_or_default();
                    let ord: i64 = row_c.try_get(2).unwrap_or(0);
                    if let Err(e) = sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)" )
                        .bind(connection_id)
                        .bind(&db_name)
                        .bind(&table_name)
                        .bind(&col_name)
                        .bind(&col_type)
                        .bind(ord)
                        .execute(cache_pool)
                        .await {
                        debug!("MySQL fetch_mysql_data: failed to cache column {}.{}.{}: {}", db_name, table_name, col_name, e);
                    } else { count += 1; }
                }
                debug!(
                    "MySQL fetch_mysql_data: cached {} columns for {}.{}",
                    count, db_name, table_name
                );
            } else if let Err(e) = cols_res {
                debug!(
                    "MySQL fetch_mysql_data: failed to list columns in {}.{}: {}",
                    db_name, table_name, e
                );
            }
        }
    }

    true
}

pub(crate) fn fetch_tables_from_mysql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    // Build a temporary runtime (function is sync due to UI constraints)
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        let pool = connection::get_or_create_connection_pool(tabular, connection_id).await?;
        match pool {
            models::enums::DatabasePool::MySQL(mysql_pool) => {
                // Safe decoder for first column (handles VARBINARY)
                fn decode_row(row: &sqlx::mysql::MySqlRow) -> Option<String> {
                    if let Ok(s) = row.try_get::<String, _>(0) { return Some(s); }
                    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(0) { return Some(s); }
                    if let Ok(bytes) = row.try_get::<Vec<u8>, _>(0) { return Some(String::from_utf8_lossy(&bytes).to_string()); }
                    if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(0) { return Some(String::from_utf8_lossy(&bytes).to_string()); }
                    None
                }

                let query = match table_type {
                    "table" => "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
                    "view" => "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
                    "procedure" => "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = ? AND ROUTINE_TYPE = 'PROCEDURE' ORDER BY ROUTINE_NAME",
                    "function" => "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = ? AND ROUTINE_TYPE = 'FUNCTION' ORDER BY ROUTINE_NAME",
                    "trigger" => "SELECT TRIGGER_NAME FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_SCHEMA = ? ORDER BY TRIGGER_NAME",
                    "event" => "SELECT EVENT_NAME FROM INFORMATION_SCHEMA.EVENTS WHERE EVENT_SCHEMA = ? ORDER BY EVENT_NAME",
                    _ => { debug!("Unsupported table type: {}", table_type); return None; }
                };

                let rows_res = sqlx::query(query)
                    .bind(database_name)
                    .fetch_all(mysql_pool.as_ref())
                    .await;

                match rows_res {
                    Ok(rows) => {
                        let mut list: Vec<String> = rows.into_iter().filter_map(|r| decode_row(&r)).collect();
                        list.sort();
                        Some(list)
                    }
                    Err(e) => {
                        debug!("Error querying MySQL {} from database {}: {}", table_type, database_name, e);
                        None
                    }
                }
            }
            _ => None,
        }
    })
}

pub(crate) fn load_mysql_structure(
    connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    debug!(
        "Loading MySQL structure for connection ID: {}",
        connection_id
    );

    // Since we can't use block_on in an async context, we'll create a simple structure
    // and populate it with cached data or show a loading message

    // Create basic structure immediately
    let mut main_children = Vec::new();

    // 1. Databases folder
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(connection_id);
    databases_folder.is_loaded = false; // Will be loaded when expanded

    // 2. DBA Views folder
    let mut dba_folder = models::structs::TreeNode::new(
        "DBA Views".to_string(),
        models::enums::NodeType::DBAViewsFolder,
    );
    dba_folder.connection_id = Some(connection_id);

    let mut dba_children = Vec::new();

    // Users
    let mut users_folder =
        models::structs::TreeNode::new("Users".to_string(), models::enums::NodeType::UsersFolder);
    users_folder.connection_id = Some(connection_id);
    users_folder.is_loaded = false;
    dba_children.push(users_folder);

    // Privileges
    let mut priv_folder = models::structs::TreeNode::new(
        "Privileges".to_string(),
        models::enums::NodeType::PrivilegesFolder,
    );
    priv_folder.connection_id = Some(connection_id);
    priv_folder.is_loaded = false;
    dba_children.push(priv_folder);

    // Processes
    let mut proc_folder = models::structs::TreeNode::new(
        "Processes".to_string(),
        models::enums::NodeType::ProcessesFolder,
    );
    proc_folder.connection_id = Some(connection_id);
    proc_folder.is_loaded = false;
    dba_children.push(proc_folder);

    // Status
    let mut status_folder =
        models::structs::TreeNode::new("Status".to_string(), models::enums::NodeType::StatusFolder);
    status_folder.connection_id = Some(connection_id);
    status_folder.is_loaded = false;
    dba_children.push(status_folder);

    // User Active
    let mut metrics_user_active_folder = models::structs::TreeNode::new(
        "User Active".to_string(),
        models::enums::NodeType::MetricsUserActiveFolder,
    );
    metrics_user_active_folder.connection_id = Some(connection_id);
    metrics_user_active_folder.is_loaded = false;
    dba_children.push(metrics_user_active_folder);

    dba_folder.children = dba_children;

    main_children.push(databases_folder);
    main_children.push(dba_folder);

    node.children = main_children;

    // Trigger async loading in background (we'll need to implement this differently)
    // For now, we'll rely on the expansion mechanism to load databases when needed
}
