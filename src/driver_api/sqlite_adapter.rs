//! Adapter SQLite di dalam proses untuk trait [`EngineSession`].
//!
//! Koneksi SQLite biasa tetap memakai jalur builtin. Adapter ini membuktikan
//! bahwa API driver cukup untuk engine sungguhan dan menjadi fixture test
//! jalur generik plugin (pool, eksekusi, pohon objek) tanpa Wasm/sidecar.

use super::{
    ColumnInfo, ConnectParams, DriverError, DriverResult, EngineCapabilities, EngineDescriptor,
    EngineDriver, EngineSession, ExecuteOutput, ExecuteRequest, StandardFields, TableInfo,
    TableKind,
};
use sqlx::{Column, Row, SqlitePool, ValueRef};
use std::sync::Arc;

pub struct SqliteEngineDriver {
    descriptor: EngineDescriptor,
}

impl SqliteEngineDriver {
    /// `id` harus bukan id builtin (mis. `"sqlite-adapter"`).
    pub fn new(id: &str) -> Self {
        Self {
            descriptor: EngineDescriptor {
                id: id.to_string(),
                name: "SQLite (driver API)".to_string(),
                icon: None,
                default_port: None,
                standard_fields: StandardFields {
                    host: false,
                    port: false,
                    username: false,
                    password: false,
                    database: true,
                },
                options: vec![],
                capabilities: EngineCapabilities {
                    databases: false,
                    sql_dialect: Some("sqlite".to_string()),
                    ssh_tunnel: false,
                    tls: false,
                    ..Default::default()
                },
            },
        }
    }
}

impl EngineDriver for SqliteEngineDriver {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn connect(&self, params: ConnectParams) -> DriverResult<Arc<dyn EngineSession>> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| DriverError::Connect(format!("no async runtime: {e}")))?;
        let url = if params.database.is_empty() || params.database == ":memory:" {
            "sqlite::memory:".to_string()
        } else {
            format!("sqlite://{}?mode=rwc", params.database)
        };
        let pool = handle
            .block_on(SqlitePool::connect(&url))
            .map_err(|e| DriverError::Connect(e.to_string()))?;
        Ok(Arc::new(SqliteSession { pool, handle }))
    }
}

pub struct SqliteSession {
    pool: SqlitePool,
    handle: tokio::runtime::Handle,
}

impl SqliteSession {
    pub fn from_pool(pool: SqlitePool, handle: tokio::runtime::Handle) -> Self {
        Self { pool, handle }
    }

    fn query_strings(&self, sql: &'static str, bind: &[&str]) -> DriverResult<Vec<Vec<Option<String>>>> {
        let mut q = sqlx::query(sql);
        for b in bind {
            q = q.bind(*b);
        }
        let rows = self
            .handle
            .block_on(q.fetch_all(&self.pool))
            .map_err(|e| DriverError::Query(e.to_string()))?;
        Ok(rows.iter().map(row_to_cells).collect())
    }
}

fn row_to_cells(row: &sqlx::sqlite::SqliteRow) -> Vec<Option<String>> {
    (0..row.columns().len())
        .map(|i| {
            let is_null = row.try_get_raw(i).map(|v| v.is_null()).unwrap_or(true);
            if is_null {
                return None;
            }
            if let Ok(v) = row.try_get::<i64, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<f64, _>(i) {
                return Some(v.to_string());
            }
            if let Ok(v) = row.try_get::<String, _>(i) {
                return Some(v);
            }
            row.try_get::<Vec<u8>, _>(i)
                .ok()
                .map(|b| format!("0x{}", hex::encode(b)))
        })
        .collect()
}

/// Statement yang mengembalikan baris (bukan hanya affected rows).
fn returns_rows(sql: &str) -> bool {
    let head = sql
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(head.as_str(), "select" | "with" | "pragma" | "values" | "explain")
        || sql.to_ascii_lowercase().contains(" returning ")
}

impl EngineSession for SqliteSession {
    fn list_databases(&self) -> DriverResult<Vec<String>> {
        Ok(vec!["main".to_string()])
    }

    fn list_schemas(&self, _database: Option<&str>) -> DriverResult<Vec<String>> {
        Ok(vec![])
    }

    fn list_tables(
        &self,
        _database: Option<&str>,
        _schema: Option<&str>,
    ) -> DriverResult<Vec<TableInfo>> {
        let rows = self.query_strings(
            "SELECT name, type FROM sqlite_master \
             WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
            &[],
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let name = r.first()?.clone()?;
                let kind = match r.get(1).cloned().flatten().as_deref() {
                    Some("view") => TableKind::View,
                    _ => TableKind::Table,
                };
                Some(TableInfo { name, kind })
            })
            .collect())
    }

    fn list_columns(
        &self,
        _database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> DriverResult<Vec<ColumnInfo>> {
        let rows = self.query_strings(
            "SELECT name, type, \"notnull\", pk FROM pragma_table_info(?) ORDER BY cid",
            &[table],
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(ColumnInfo {
                    name: r.first()?.clone()?,
                    data_type: r.get(1).cloned().flatten().unwrap_or_default(),
                    nullable: r.get(2).cloned().flatten().as_deref() != Some("1"),
                    primary_key: r.get(3).cloned().flatten().is_some_and(|v| v != "0"),
                })
            })
            .collect())
    }

    fn execute(&self, request: &ExecuteRequest) -> DriverResult<ExecuteOutput> {
        if !returns_rows(&request.query) {
            let done = self
                .handle
                .block_on(sqlx::query(sqlx::AssertSqlSafe(request.query.clone())).execute(&self.pool))
                .map_err(|e| DriverError::Query(e.to_string()))?;
            return Ok(ExecuteOutput {
                affected_rows: Some(done.rows_affected()),
                ..Default::default()
            });
        }
        let rows = self
            .handle
            .block_on(sqlx::query(sqlx::AssertSqlSafe(request.query.clone())).fetch_all(&self.pool))
            .map_err(|e| DriverError::Query(e.to_string()))?;
        let headers = rows
            .first()
            .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
            .unwrap_or_default();
        let truncated = request.max_rows > 0 && rows.len() > request.max_rows;
        let limit = if request.max_rows == 0 {
            rows.len()
        } else {
            request.max_rows
        };
        Ok(ExecuteOutput {
            headers,
            rows: rows.iter().take(limit).map(row_to_cells).collect(),
            affected_rows: None,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver_api::run_blocking;

    #[tokio::test(flavor = "multi_thread")]
    async fn adapter_lists_objects_and_runs_queries() {
        let driver = SqliteEngineDriver::new("sqlite-adapter");
        let session = run_blocking(move || driver.connect(ConnectParams::default()))
            .await
            .unwrap();

        let s = session.clone();
        let out = run_blocking(move || {
            s.execute(&ExecuteRequest {
                query: "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, note TEXT)"
                    .into(),
                database: None,
                schema: None,
                max_rows: 10,
                job_id: 1,
            })?;
            s.execute(&ExecuteRequest {
                query: "INSERT INTO t (name, note) VALUES ('a', NULL), ('b', 'x'), ('c', 'y')"
                    .into(),
                database: None,
                schema: None,
                max_rows: 10,
                job_id: 2,
            })
        })
        .await
        .unwrap();
        assert_eq!(out.affected_rows, Some(3));

        let s = session.clone();
        let (tables, cols, rows) = run_blocking(move || {
            let tables = s.list_tables(None, None)?;
            let cols = s.list_columns(None, None, "t")?;
            let rows = s.execute(&ExecuteRequest {
                query: "SELECT id, name, note FROM t ORDER BY id".into(),
                database: None,
                schema: None,
                max_rows: 2,
                job_id: 3,
            })?;
            Ok((tables, cols, rows))
        })
        .await
        .unwrap();

        assert_eq!(tables, vec![TableInfo { name: "t".into(), kind: TableKind::Table }]);
        assert_eq!(cols.len(), 3);
        assert!(cols[0].primary_key);
        assert!(!cols[1].nullable);
        assert_eq!(rows.headers, vec!["id", "name", "note"]);
        assert_eq!(rows.rows.len(), 2);
        assert_eq!(rows.rows[0][2], None);
        assert!(rows.truncated);
    }

    #[test]
    fn detects_row_returning_statements() {
        assert!(returns_rows("  SELECT 1"));
        assert!(returns_rows("with x as (select 1) select * from x"));
        assert!(returns_rows("select(1)"));
        assert!(returns_rows("INSERT INTO t VALUES (1) RETURNING id"));
        assert!(!returns_rows("UPDATE t SET a = 1"));
    }
}
