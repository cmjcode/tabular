//! Host sidecar: driver sebagai proses terpisah (desktop saja).
//!
//! Protokol: JSON-RPC 2.0, satu objek JSON per baris lewat stdin/stdout.
//! Baris stdout yang bukan JSON diabaikan (dicatat di log debug); stderr
//! diteruskan ke log. Method:
//! - `initialize {abi}` -> `{abi_version: 1}`
//! - `connect ConnectParams` -> `{session: "<id>"}`
//! - `list_databases {session}`, `list_schemas {session, database}`,
//!   `list_tables {session, database, schema}`,
//!   `list_columns {session, database, schema, table}`
//! - `execute {session, request: ExecuteRequest}` -> `ExecuteOutput`
//! - `cancel {session, job_id}`, `close {session}`
//!
//! Satu proses melayani semua sesi driver itu dan harus menerima request
//! secara paralel (terutama `cancel` saat `execute` berjalan). Binary sidecar
//! tidak ter-sandbox; ia hanya dimuat setelah pengguna menyetujui hash-nya
//! (lihat `manifest::approve_sidecar`).

use super::manifest::DRIVER_ABI;
use super::wasm_host::decode;
use super::{
    ColumnInfo, ConnectParams, DriverError, DriverResult, EngineDescriptor, EngineDriver,
    EngineSession, ExecuteOutput, ExecuteRequest, TableInfo,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const RESPAWN_BACKOFF: Duration = Duration::from_secs(2);

type Reply = DriverResult<Value>;
type PendingMap = Arc<Mutex<HashMap<u64, mpsc::Sender<Reply>>>>;

struct SidecarProcess {
    generation: u64,
    stdin: Mutex<ChildStdin>,
    pending: PendingMap,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    child: Mutex<Child>,
}

impl SidecarProcess {
    fn send(&self, method: &str, params: Value) -> DriverResult<mpsc::Receiver<Reply>> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(DriverError::Plugin("sidecar process is not running".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| DriverError::Plugin("sidecar state poisoned".into()))?
            .insert(id, tx);
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let write = self
            .stdin
            .lock()
            .map_err(|_| DriverError::Plugin("sidecar stdin poisoned".into()))
            .and_then(|mut stdin| {
                writeln!(stdin, "{line}")
                    .and_then(|_| stdin.flush())
                    .map_err(|e| DriverError::Plugin(format!("cannot write to sidecar: {e}")))
            });
        if let Err(e) = write {
            if let Ok(mut p) = self.pending.lock() {
                p.remove(&id);
            }
            return Err(e);
        }
        Ok(rx)
    }

    fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Reply {
        let rx = self.send(method, params)?;
        match timeout {
            Some(t) => rx
                .recv_timeout(t)
                .map_err(|_| DriverError::Plugin(format!("sidecar did not answer '{method}'")))?,
            None => rx
                .recv()
                .map_err(|_| DriverError::Plugin("sidecar process exited".into()))?,
        }
    }
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn reply_from_line(line: &str) -> Option<(u64, Reply)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value.get("id")?.as_u64()?;
    if let Some(err) = value.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| err.to_string());
        return Some((id, Err(DriverError::Query(msg))));
    }
    Some((id, Ok(value.get("result").cloned().unwrap_or(Value::Null))))
}

struct Shared {
    descriptor: EngineDescriptor,
    path: PathBuf,
    args: Vec<String>,
    process: Mutex<Option<Arc<SidecarProcess>>>,
    last_spawn: Mutex<Option<Instant>>,
    generation: AtomicU64,
}

impl Shared {
    fn spawn(&self) -> DriverResult<Arc<SidecarProcess>> {
        let mut command = Command::new(&self.path);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = self.path.parent() {
            command.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| {
            DriverError::Plugin(format!(
                "cannot start sidecar '{}': {e}",
                self.path.display()
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DriverError::Plugin("sidecar stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Plugin("sidecar stdout unavailable".into()))?;
        let stderr = child.stderr.take();

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let id = self.descriptor.id.clone();

        {
            let pending = pending.clone();
            let alive = alive.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let trimmed = line.trim();
                    if !trimmed.starts_with('{') {
                        if !trimmed.is_empty() {
                            log::debug!("[DRIVER-PLUGIN:{id}] stdout: {trimmed}");
                        }
                        continue;
                    }
                    match reply_from_line(trimmed) {
                        Some((req_id, reply)) => {
                            let tx = pending.lock().ok().and_then(|mut p| p.remove(&req_id));
                            if let Some(tx) = tx {
                                let _ = tx.send(reply);
                            }
                        }
                        None => log::debug!("[DRIVER-PLUGIN:{id}] ignored line: {trimmed}"),
                    }
                }
                alive.store(false, Ordering::SeqCst);
                if let Ok(mut p) = pending.lock() {
                    for (_, tx) in p.drain() {
                        let _ = tx.send(Err(DriverError::Plugin("sidecar process exited".into())));
                    }
                }
                log::warn!("[DRIVER-PLUGIN:{id}] sidecar process exited");
            });
        }
        if let Some(stderr) = stderr {
            let id = id.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("[DRIVER-PLUGIN:{id}] stderr: {line}");
                }
            });
        }

        let process = Arc::new(SidecarProcess {
            generation: self.generation.fetch_add(1, Ordering::SeqCst) + 1,
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            alive,
            child: Mutex::new(child),
        });
        let init = process.request(
            "initialize",
            json!({ "abi": DRIVER_ABI }),
            Some(INIT_TIMEOUT),
        )?;
        let version = init.get("abi_version").and_then(Value::as_i64);
        if version != Some(1) {
            return Err(DriverError::Protocol(format!(
                "sidecar reported ABI version {version:?}, expected 1"
            )));
        }
        log::info!("[DRIVER-PLUGIN:{id}] sidecar started");
        Ok(process)
    }

    /// Proses yang hidup, atau spawn baru (dengan jeda antar-restart).
    fn process(&self) -> DriverResult<Arc<SidecarProcess>> {
        let poisoned = || DriverError::Plugin("sidecar state poisoned".into());
        let mut slot = self.process.lock().map_err(|_| poisoned())?;
        if let Some(p) = slot.as_ref()
            && p.alive.load(Ordering::SeqCst)
        {
            return Ok(p.clone());
        }
        let mut last = self.last_spawn.lock().map_err(|_| poisoned())?;
        if slot.is_some()
            && let Some(at) = *last
            && at.elapsed() < RESPAWN_BACKOFF
        {
            return Err(DriverError::Plugin(
                "sidecar crashed; restarting shortly".into(),
            ));
        }
        *last = Some(Instant::now());
        *slot = None;
        let process = self.spawn()?;
        *slot = Some(process.clone());
        Ok(process)
    }
}

/// Driver engine dari executable sidecar. Proses baru dijalankan saat koneksi
/// pertama dibuka, bukan saat dimuat.
pub struct SidecarDriver {
    shared: Arc<Shared>,
}

impl SidecarDriver {
    pub fn new(path: PathBuf, args: Vec<String>, descriptor: EngineDescriptor) -> Self {
        Self {
            shared: Arc::new(Shared {
                descriptor,
                path,
                args,
                process: Mutex::new(None),
                last_spawn: Mutex::new(None),
                generation: AtomicU64::new(0),
            }),
        }
    }
}

pub struct SidecarSession {
    shared: Arc<Shared>,
    params: ConnectParams,
    /// (generasi proses, id sesi di sidecar).
    state: Mutex<Option<(u64, String)>>,
}

impl SidecarSession {
    /// Pastikan sesi ada di proses yang sedang hidup; setelah sidecar
    /// di-restart, sesi dibuka ulang dengan parameter yang sama.
    fn session(&self) -> DriverResult<(Arc<SidecarProcess>, String)> {
        let process = self.shared.process()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| DriverError::Plugin("sidecar session poisoned".into()))?;
        if let Some((generation, id)) = state.as_ref()
            && *generation == process.generation
        {
            return Ok((process, id.clone()));
        }
        let params =
            serde_json::to_value(&self.params).map_err(|e| DriverError::Protocol(e.to_string()))?;
        let reply = process
            .request("connect", params, None)
            .map_err(|e| match e {
                DriverError::Query(m) => DriverError::Connect(m),
                other => other,
            })?;
        let id = reply
            .get("session")
            .and_then(Value::as_str)
            .ok_or_else(|| DriverError::Protocol("connect reply has no 'session'".into()))?
            .to_string();
        *state = Some((process.generation, id.clone()));
        Ok((process, id))
    }

    fn call(&self, method: &str, mut params: Value) -> DriverResult<Value> {
        let (process, id) = self.session()?;
        params["session"] = Value::String(id);
        process.request(method, params, None)
    }
}

impl EngineDriver for SidecarDriver {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.shared.descriptor
    }

    fn connect(&self, params: ConnectParams) -> DriverResult<Arc<dyn EngineSession>> {
        let session = SidecarSession {
            shared: self.shared.clone(),
            params,
            state: Mutex::new(None),
        };
        session.session()?;
        Ok(Arc::new(session))
    }
}

impl EngineSession for SidecarSession {
    fn list_databases(&self) -> DriverResult<Vec<String>> {
        decode("list_databases", self.call("list_databases", json!({}))?)
    }

    fn list_schemas(&self, database: Option<&str>) -> DriverResult<Vec<String>> {
        decode(
            "list_schemas",
            self.call("list_schemas", json!({ "database": database }))?,
        )
    }

    fn list_tables(
        &self,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> DriverResult<Vec<TableInfo>> {
        decode(
            "list_tables",
            self.call(
                "list_tables",
                json!({ "database": database, "schema": schema }),
            )?,
        )
    }

    fn list_columns(
        &self,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> DriverResult<Vec<ColumnInfo>> {
        decode(
            "list_columns",
            self.call(
                "list_columns",
                json!({ "database": database, "schema": schema, "table": table }),
            )?,
        )
    }

    fn execute(&self, request: &ExecuteRequest) -> DriverResult<ExecuteOutput> {
        decode(
            "execute",
            self.call("execute", json!({ "request": request }))?,
        )
    }

    fn cancel(&self, job_id: u64) -> DriverResult<()> {
        if !self.shared.descriptor.capabilities.cancel {
            return Err(DriverError::Unsupported("cancel".into()));
        }
        self.call("cancel", json!({ "job_id": job_id })).map(|_| ())
    }

    fn close(&self) {
        let current = self.state.lock().ok().and_then(|s| s.clone());
        let Some((generation, id)) = current else {
            return;
        };
        let process = self.shared.process.lock().ok().and_then(|p| p.clone());
        if let Some(process) = process
            && process.generation == generation
        {
            let _ = process.send("close", json!({ "session": id }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_ENV: &str = "TABULAR_FAKE_SIDECAR";

    /// Sidecar palsu: dijalankan ulang oleh test lewat binary test itu sendiri.
    #[test]
    #[ignore = "dijalankan sebagai proses sidecar oleh test lain"]
    fn fake_sidecar_main() {
        if std::env::var(FAKE_ENV).is_err() {
            return;
        }
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        // libtest sudah mencetak "test ... " tanpa newline; tutup barisnya
        // supaya balasan pertama berada di baris sendiri.
        let _ = writeln!(out);
        for line in stdin.lock().lines().map_while(Result::ok) {
            let Ok(req) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = req["id"].clone();
            let p = &req["params"];
            let result: Result<Value, &str> = match req["method"].as_str().unwrap_or("") {
                "initialize" => Ok(json!({ "abi_version": 1 })),
                "connect" if p["password"] == "bad" => Err("authentication failed"),
                "connect" => Ok(json!({ "session": "s1" })),
                "list_databases" => Ok(json!(["db1", "db2"])),
                "list_tables" => Ok(json!([{ "name": "t", "kind": "view" }])),
                "execute" => {
                    let q = p["request"]["query"].as_str().unwrap_or("");
                    if q == "crash" {
                        std::process::exit(3);
                    }
                    Ok(json!({ "headers": ["q", "session"], "rows": [[q, p["session"]]] }))
                }
                "cancel" | "close" => Ok(Value::Null),
                _ => Err("unknown method"),
            };
            let reply = match result {
                Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                Err(m) => {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": 1, "message": m } })
                }
            };
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }

    fn fake_driver() -> SidecarDriver {
        // Binary test menjalankan dirinya sendiri, hanya test sidecar palsu.
        let exe = std::env::current_exe().unwrap();
        let descriptor: EngineDescriptor =
            serde_json::from_str(r#"{"id":"fake-sidecar","name":"Fake"}"#).unwrap();
        SidecarDriver::new(
            exe,
            vec![
                "driver_api::sidecar::tests::fake_sidecar_main".into(),
                "--exact".into(),
                "--ignored".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            descriptor,
        )
    }

    fn req(q: &str) -> ExecuteRequest {
        ExecuteRequest {
            query: q.into(),
            database: None,
            schema: None,
            max_rows: 10,
            job_id: 1,
        }
    }

    fn with_fake_env() {
        // SAFETY: test ini hanya menulis nilai yang sama; proses anak
        // mewarisinya saat di-spawn.
        unsafe { std::env::set_var(FAKE_ENV, "1") };
    }

    #[test]
    fn sidecar_roundtrip_and_restart() {
        with_fake_env();
        let driver = fake_driver();
        let session = driver.connect(ConnectParams::default()).unwrap();
        assert_eq!(session.list_databases().unwrap(), vec!["db1", "db2"]);
        let tables = session.list_tables(None, None).unwrap();
        assert_eq!(tables[0].kind, super::super::TableKind::View);
        let out = session.execute(&req("select 1")).unwrap();
        assert_eq!(
            out.rows,
            vec![vec![Some("select 1".into()), Some("s1".into())]]
        );

        // Proses mati di tengah query: request gagal, lalu proses dijalankan
        // ulang dan sesi dibuka kembali otomatis.
        assert!(session.execute(&req("crash")).is_err());
        std::thread::sleep(RESPAWN_BACKOFF + Duration::from_millis(200));
        let out = session.execute(&req("after restart")).unwrap();
        assert_eq!(out.rows[0][0], Some("after restart".into()));
    }

    #[test]
    fn sidecar_connect_error_is_reported() {
        with_fake_env();
        let driver = fake_driver();
        let err = driver
            .connect(ConnectParams {
                password: "bad".into(),
                ..Default::default()
            })
            .err()
            .unwrap();
        assert_eq!(err, DriverError::Connect("authentication failed".into()));
    }

    #[test]
    fn missing_executable_is_a_plugin_error() {
        let descriptor: EngineDescriptor =
            serde_json::from_str(r#"{"id":"nope","name":"Nope"}"#).unwrap();
        let driver = SidecarDriver::new(
            PathBuf::from("/definitely/not/here/tabular-sidecar"),
            vec![],
            descriptor,
        );
        let err = driver.connect(ConnectParams::default()).err().unwrap();
        assert!(matches!(err, DriverError::Plugin(_)), "{err:?}");
    }

    #[test]
    fn parses_jsonrpc_replies() {
        let (id, r) = reply_from_line(r#"{"jsonrpc":"2.0","id":4,"result":[1]}"#).unwrap();
        assert_eq!((id, r.unwrap()), (4, json!([1])));
        let (_, r) =
            reply_from_line(r#"{"jsonrpc":"2.0","id":5,"error":{"code":1,"message":"x"}}"#)
                .unwrap();
        assert_eq!(r, Err(DriverError::Query("x".into())));
        assert!(reply_from_line("running 1 test").is_none());
    }
}
