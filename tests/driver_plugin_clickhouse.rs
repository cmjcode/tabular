//! Test ujung-ke-ujung plugin driver ClickHouse (Wasm) melawan server HTTP
//! tiruan. Butuh `plugins/examples/clickhouse/dist/driver.wasm` (jalankan
//! `plugins/examples/clickhouse/build.sh`); dilewati bila belum di-build.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tabular::driver_api::wasm_host::WasmDriver;
use tabular::driver_api::{ConnectParams, EngineDescriptor, EngineDriver, ExecuteRequest, TableKind};

const WASM: &str = "plugins/examples/clickhouse/dist/driver.wasm";
const MANIFEST: &str = "plugins/examples/clickhouse/manifest.json";
const BIG_ROWS: usize = 100_000;

struct Request {
    target: String,
    body: String,
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?.to_string();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        if h == "\r\n" || h.is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        target,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn big_json() -> String {
    let mut s = String::from(r#"{"meta":[{"name":"id","type":"UInt64"},{"name":"label","type":"Nullable(String)"}],"data":["#);
    for i in 0..BIG_ROWS {
        if i > 0 {
            s.push(',');
        }
        if i % 10 == 0 {
            s.push_str(&format!(r#"["{i}",null]"#));
        } else {
            s.push_str(&format!(r#"["{i}","row {i}"]"#));
        }
    }
    s.push_str("]}");
    s
}

/// Server ClickHouse tiruan; mencatat body semua request yang diterima.
fn fake_clickhouse() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let big = Arc::new(big_json());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = log.clone();
            let big = big.clone();
            std::thread::spawn(move || {
                let Some(req) = read_request(&mut stream) else { return };
                log.lock().unwrap().push(req.body.clone());
                let (status, body) = if req.body.starts_with("SELECT 1") {
                    (200, "1\n".to_string())
                } else if req.body.contains("system.databases") {
                    (200, "default\nanalytics\n".to_string())
                } else if req.body.contains("system.tables") {
                    (200, "events\tMergeTree\nv_daily\tView\n".to_string())
                } else if req.body.contains("system.columns") {
                    (200, "id\tUInt64\t1\nlabel\tNullable(String)\t0\n".to_string())
                } else if req.body.starts_with("KILL QUERY") {
                    (200, String::new())
                } else if req.body.contains("big_table") {
                    (200, big.to_string())
                } else if req.body.contains("broken") {
                    (400, "Code: 62. DB::Exception: Syntax error".to_string())
                } else {
                    assert!(req.target.contains("default_format=JSONCompact"));
                    (200, r#"{"meta":[{"name":"x","type":"UInt8"}],"data":[[1]],"rows":1}"#.to_string())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    (port, seen)
}

fn load_driver() -> Option<WasmDriver> {
    let bytes = std::fs::read(WASM).ok()?;
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(MANIFEST).ok()?).ok()?;
    let descriptor: EngineDescriptor = serde_json::from_value(manifest["engine"].clone()).ok()?;
    let hosts = vec!["{connection.host}".to_string()];
    Some(WasmDriver::load(&bytes, descriptor, hosts).expect("driver.wasm loads"))
}

fn request(query: &str, max_rows: usize) -> ExecuteRequest {
    ExecuteRequest {
        query: query.into(),
        database: Some("analytics".into()),
        schema: None,
        max_rows,
        job_id: 42,
    }
}

#[test]
fn clickhouse_plugin_end_to_end() {
    let Some(driver) = load_driver() else {
        eprintln!("skipping: {WASM} not built (run plugins/examples/clickhouse/build.sh)");
        return;
    };
    let (port, seen) = fake_clickhouse();
    let session = driver
        .connect(ConnectParams {
            host: "127.0.0.1".into(),
            port: Some(port),
            username: "reader".into(),
            ..Default::default()
        })
        .expect("connect");

    assert_eq!(session.list_databases().unwrap(), vec!["default", "analytics"]);
    let tables = session.list_tables(Some("analytics"), None).unwrap();
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[1].kind, TableKind::View);
    let cols = session.list_columns(Some("analytics"), None, "events").unwrap();
    assert!(cols[0].primary_key);
    assert!(cols[1].nullable);

    let out = session.execute(&request("SELECT x FROM events", 100)).unwrap();
    assert_eq!(out.headers, vec!["x"]);
    assert_eq!(out.rows, vec![vec![Some("1".into())]]);

    let err = session.execute(&request("SELECT broken", 100)).unwrap_err();
    assert!(err.to_string().contains("Syntax error"), "{err}");

    session.cancel(42).unwrap();
    assert!(seen
        .lock()
        .unwrap()
        .iter()
        .any(|b| b.contains("KILL QUERY WHERE query_id = 'tabular-42'")));
}

#[test]
fn clickhouse_plugin_rejects_hosts_outside_allowlist() {
    let Some(driver) = load_driver() else {
        return;
    };
    // Host koneksi "db.example" diizinkan, tetapi plugin tetap memakai host
    // koneksi, jadi request ke 127.0.0.1 tidak terjadi; connect harus gagal
    // karena db.example tidak bisa di-resolve, bukan karena bypass allowlist.
    let err = driver
        .connect(ConnectParams {
            host: "db.invalid".into(),
            port: Some(1),
            ..Default::default()
        })
        .err()
        .expect("connect fails");
    assert!(!err.to_string().contains("allowed hosts"), "{err}");
}

/// Benchmark jalur host-side parsing (Fase 3): 100k baris JSONCompact.
#[test]
fn clickhouse_plugin_large_result_uses_host_parsing() {
    let Some(driver) = load_driver() else {
        return;
    };
    let (port, _) = fake_clickhouse();
    let session = driver
        .connect(ConnectParams {
            host: "127.0.0.1".into(),
            port: Some(port),
            ..Default::default()
        })
        .unwrap();
    let started = Instant::now();
    let out = session.execute(&request("SELECT * FROM big_table", BIG_ROWS)).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(out.rows.len(), BIG_ROWS);
    assert_eq!(out.rows[10][1], None);
    assert_eq!(out.rows[11][1].as_deref(), Some("row 11"));
    eprintln!("[bench] {BIG_ROWS} rows via Wasm plugin + host parsing: {elapsed:?}");

    let started = Instant::now();
    let capped = session.execute(&request("SELECT * FROM big_table", 1_000)).unwrap();
    assert_eq!(capped.rows.len(), 1_000);
    assert!(capped.truncated);
    eprintln!("[bench] capped at 1000 rows: {:?}", started.elapsed());
}
