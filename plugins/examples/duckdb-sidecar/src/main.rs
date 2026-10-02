//! Sidecar DuckDB untuk Tabular: JSON-RPC 2.0 lewat stdin/stdout, satu objek
//! per baris. Setiap request diproses di thread sendiri supaya `cancel` bisa
//! masuk saat `execute` berjalan. Log hanya ke stderr; stdout khusus protokol.

use duckdb::types::Value as DuckValue;
use duckdb::{Config, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

struct Session {
    /// Koneksi utama untuk query; di-clone per request supaya metadata dan
    /// query bisa berjalan bersamaan.
    conn: Mutex<Connection>,
    interrupts: Mutex<HashMap<u64, Arc<duckdb::InterruptHandle>>>,
}

#[derive(Default)]
struct State {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    next_id: AtomicU64,
}

type RpcResult = Result<Value, String>;

fn cell(value: DuckValue) -> Value {
    match value {
        DuckValue::Null => Value::Null,
        DuckValue::Text(s) => Value::String(s),
        DuckValue::Boolean(b) => Value::String(b.to_string()),
        DuckValue::TinyInt(v) => Value::String(v.to_string()),
        DuckValue::SmallInt(v) => Value::String(v.to_string()),
        DuckValue::Int(v) => Value::String(v.to_string()),
        DuckValue::BigInt(v) => Value::String(v.to_string()),
        DuckValue::HugeInt(v) => Value::String(v.to_string()),
        DuckValue::UTinyInt(v) => Value::String(v.to_string()),
        DuckValue::USmallInt(v) => Value::String(v.to_string()),
        DuckValue::UInt(v) => Value::String(v.to_string()),
        DuckValue::UBigInt(v) => Value::String(v.to_string()),
        DuckValue::Float(v) => Value::String(v.to_string()),
        DuckValue::Double(v) => Value::String(v.to_string()),
        DuckValue::Blob(b) => Value::String(format!("<{} bytes>", b.len())),
        other => Value::String(format!("{other:?}")),
    }
}

fn session<'a>(state: &'a State, params: &Value) -> Result<Arc<Session>, String> {
    let id = params["session"].as_str().ok_or("missing session")?;
    state
        .sessions
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .get(id)
        .cloned()
        .ok_or_else(|| format!("unknown session '{id}'"))
}

fn query_rows(conn: &Connection, sql: &str, max_rows: usize) -> Result<(Vec<String>, Vec<Vec<Value>>, bool), String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut truncated = false;
    let mut width = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if max_rows > 0 && out.len() == max_rows {
            truncated = true;
            break;
        }
        width = row.as_ref().column_count();
        let mut cells = Vec::with_capacity(width);
        for i in 0..width {
            cells.push(cell(row.get::<_, DuckValue>(i).map_err(|e| e.to_string())?));
        }
        out.push(cells);
    }
    drop(rows);
    let headers = stmt.column_names();
    let _ = width;
    Ok((headers, out, truncated))
}

fn strings(conn: &Connection, sql: &str, args: &[&str]) -> Result<Vec<Vec<String>>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let params: Vec<&dyn duckdb::ToSql> = args.iter().map(|a| a as &dyn duckdb::ToSql).collect();
    let mut rows = stmt.query(params.as_slice()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let n = row.as_ref().column_count();
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            r.push(row.get::<_, Option<String>>(i).map_err(|e| e.to_string())?.unwrap_or_default());
        }
        out.push(r);
    }
    Ok(out)
}

fn handle(state: &State, method: &str, params: Value) -> RpcResult {
    match method {
        "initialize" => Ok(json!({ "abi_version": 1 })),
        "connect" => {
            let path = params["database"].as_str().unwrap_or("").trim().to_string();
            let read_only = params["options"]["read_only"].as_str() == Some("true");
            let conn = if path.is_empty() || path == ":memory:" {
                Connection::open_in_memory()
            } else {
                let config = if read_only {
                    Config::default()
                        .access_mode(duckdb::AccessMode::ReadOnly)
                        .map_err(|e| e.to_string())?
                } else {
                    Config::default()
                };
                Connection::open_with_flags(&path, config)
            }
            .map_err(|e| e.to_string())?;
            let id = format!("s{}", state.next_id.fetch_add(1, Ordering::SeqCst) + 1);
            state.sessions.lock().map_err(|_| "state poisoned")?.insert(
                id.clone(),
                Arc::new(Session {
                    conn: Mutex::new(conn),
                    interrupts: Mutex::new(HashMap::new()),
                }),
            );
            Ok(json!({ "session": id }))
        }
        "list_databases" => {
            let s = session(state, &params)?;
            let conn = s.conn.lock().map_err(|_| "poisoned")?.try_clone().map_err(|e| e.to_string())?;
            let rows = strings(
                &conn,
                "SELECT database_name FROM duckdb_databases() WHERE NOT internal ORDER BY 1",
                &[],
            )?;
            Ok(json!(rows.into_iter().map(|r| r[0].clone()).collect::<Vec<_>>()))
        }
        "list_schemas" => {
            let s = session(state, &params)?;
            let conn = s.conn.lock().map_err(|_| "poisoned")?.try_clone().map_err(|e| e.to_string())?;
            let db = params["database"].as_str().unwrap_or("memory");
            let rows = strings(
                &conn,
                "SELECT schema_name FROM information_schema.schemata WHERE catalog_name = ? ORDER BY 1",
                &[db],
            )?;
            Ok(json!(rows.into_iter().map(|r| r[0].clone()).collect::<Vec<_>>()))
        }
        "list_tables" => {
            let s = session(state, &params)?;
            let conn = s.conn.lock().map_err(|_| "poisoned")?.try_clone().map_err(|e| e.to_string())?;
            let db = params["database"].as_str().unwrap_or("memory");
            let schema = params["schema"].as_str().unwrap_or("main");
            let rows = strings(
                &conn,
                "SELECT table_name, table_type FROM information_schema.tables \
                 WHERE table_catalog = ? AND table_schema = ? ORDER BY 1",
                &[db, schema],
            )?;
            Ok(json!(rows
                .into_iter()
                .map(|r| json!({
                    "name": r[0],
                    "kind": if r[1] == "VIEW" { "view" } else { "table" }
                }))
                .collect::<Vec<_>>()))
        }
        "list_columns" => {
            let s = session(state, &params)?;
            let conn = s.conn.lock().map_err(|_| "poisoned")?.try_clone().map_err(|e| e.to_string())?;
            let db = params["database"].as_str().unwrap_or("memory");
            let schema = params["schema"].as_str().unwrap_or("main");
            let table = params["table"].as_str().ok_or("missing table")?;
            let rows = strings(
                &conn,
                "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
                 WHERE table_catalog = ? AND table_schema = ? AND table_name = ? \
                 ORDER BY ordinal_position",
                &[db, schema, table],
            )?;
            Ok(json!(rows
                .into_iter()
                .map(|r| json!({
                    "name": r[0],
                    "data_type": r[1],
                    "nullable": r[2] == "YES",
                    "primary_key": false
                }))
                .collect::<Vec<_>>()))
        }
        "execute" => {
            let s = session(state, &params)?;
            let req = &params["request"];
            let sql = req["query"].as_str().ok_or("missing query")?;
            let max_rows = req["max_rows"].as_u64().unwrap_or(0) as usize;
            let job_id = req["job_id"].as_u64().unwrap_or(0);
            let conn = s.conn.lock().map_err(|_| "poisoned")?.try_clone().map_err(|e| e.to_string())?;
            if let Some(db) = req["database"].as_str().filter(|d| !d.is_empty()) {
                let _ = conn.execute_batch(&format!("USE \"{}\"", db.replace('"', "\"\"")));
            }
            s.interrupts
                .lock()
                .map_err(|_| "poisoned")?
                .insert(job_id, conn.interrupt_handle());
            let result = query_rows(&conn, sql, max_rows);
            if let Ok(mut m) = s.interrupts.lock() {
                m.remove(&job_id);
            }
            let (headers, rows, truncated) = result?;
            Ok(json!({ "headers": headers, "rows": rows, "truncated": truncated }))
        }
        "cancel" => {
            let s = session(state, &params)?;
            let job_id = params["job_id"].as_u64().unwrap_or(0);
            if let Some(h) = s.interrupts.lock().map_err(|_| "poisoned")?.get(&job_id) {
                h.interrupt();
            }
            Ok(Value::Null)
        }
        "close" => {
            if let Some(id) = params["session"].as_str() {
                state.sessions.lock().map_err(|_| "poisoned")?.remove(id);
            }
            Ok(Value::Null)
        }
        other => Err(format!("unknown method '{other}'")),
    }
}

fn main() {
    let state = Arc::new(State::default());
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            eprintln!("ignoring invalid request line");
            continue;
        };
        let (state, stdout) = (state.clone(), stdout.clone());
        std::thread::spawn(move || {
            let id = req["id"].clone();
            let method = req["method"].as_str().unwrap_or("").to_string();
            let reply = match handle(&state, &method, req["params"].clone()) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(message) => json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32000, "message": message }
                }),
            };
            if let Ok(mut out) = stdout.lock() {
                let _ = writeln!(out, "{reply}");
                let _ = out.flush();
            }
        });
    }
}
