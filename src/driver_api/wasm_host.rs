//! Host Wasm untuk plugin driver, ABI `tabular-driver-v1`.
//!
//! Export guest:
//! - `memory`
//! - `tdrv_abi_version() -> i32` (harus 1)
//! - `tdrv_alloc(len: i32) -> i32`, `tdrv_free(ptr: i32, len: i32)`
//! - `tdrv_call(method_ptr, method_len, payload_ptr, payload_len) -> i64`:
//!   mengembalikan `(ptr << 32) | len` berisi JSON `{"ok": ..}` atau
//!   `{"err": ".."}`. Host membebaskan input dan respons lewat `tdrv_free`.
//!
//! Import host (modul `tabular`):
//! - `log(level, ptr, len)`
//! - `http(ptr, len) -> i32`: jalankan [`HttpRequest`], simpan
//!   `{"ok": HttpResponse}` atau `{"err": ..}` di buffer host, kembalikan
//!   panjangnya.
//! - `take(ptr, len) -> i32`: salin buffer host ke memori guest.
//! - `now_ms() -> i64`
//!
//! Method: `connect`, `list_databases`, `list_schemas`, `list_tables`,
//! `list_columns`, `execute`, `cancel`, `close`. `execute` boleh membalas
//! `{"output": ExecuteOutput}` atau `{"http_table": HttpTableRequest}`; yang
//! kedua dijalankan dan di-parse host secara native.

use super::http::{self, HttpPolicy, HttpRequest, HttpTableRequest};
use super::{
    ColumnInfo, ConnectParams, DriverError, DriverResult, EngineDescriptor, EngineDriver,
    EngineSession, ExecuteOutput, ExecuteRequest, TableInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Condvar, Mutex};
use wasmi::{
    Caller, Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

const ABI_VERSION: i32 = 1;
/// Anggaran instruksi per panggilan guest (waktu tunggu HTTP tidak dihitung).
const FUEL_PER_CALL: u64 = 10_000_000_000;
const MEMORY_LIMIT: usize = 256 << 20;
/// Maksimum instance paralel per sesi (query, pohon objek, cancel).
const MAX_INSTANCES: usize = 4;
const MAX_GUEST_MESSAGE: usize = 64 << 20;

struct HostState {
    engine_id: String,
    policy: HttpPolicy,
    pending: Option<Vec<u8>>,
    limits: StoreLimits,
}

struct Inner {
    descriptor: EngineDescriptor,
    engine: Engine,
    module: Module,
    linker: Linker<HostState>,
    http_hosts: Vec<String>,
}

/// Driver engine dari satu modul Wasm.
pub struct WasmDriver {
    inner: Arc<Inner>,
}

fn guest_bytes(caller: &Caller<HostState>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    if ptr < 0 || len < 0 || len as usize > MAX_GUEST_MESSAGE {
        return None;
    }
    let memory = caller.get_export("memory")?.into_memory()?;
    let data = memory.data(caller);
    let (start, end) = (ptr as usize, ptr as usize + len as usize);
    data.get(start..end).map(<[u8]>::to_vec)
}

fn envelope_err(msg: impl std::fmt::Display) -> Vec<u8> {
    json!({ "err": msg.to_string() }).to_string().into_bytes()
}

fn create_linker(engine: &Engine) -> DriverResult<Linker<HostState>> {
    let mut linker = Linker::new(engine);
    let bind_err = |e: wasmi::errors::LinkerError| DriverError::Plugin(e.to_string());

    linker
        .func_wrap(
            "tabular",
            "log",
            |caller: Caller<HostState>, level: i32, ptr: i32, len: i32| {
                let msg = guest_bytes(&caller, ptr, len)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let id = &caller.data().engine_id;
                match level {
                    0 => log::debug!("[DRIVER-PLUGIN:{id}] {msg}"),
                    1 => log::info!("[DRIVER-PLUGIN:{id}] {msg}"),
                    2 => log::warn!("[DRIVER-PLUGIN:{id}] {msg}"),
                    _ => log::error!("[DRIVER-PLUGIN:{id}] {msg}"),
                }
            },
        )
        .map_err(bind_err)?;

    linker
        .func_wrap(
            "tabular",
            "http",
            |mut caller: Caller<HostState>, ptr: i32, len: i32| -> i32 {
                let reply = match guest_bytes(&caller, ptr, len)
                    .ok_or_else(|| DriverError::Protocol("bad request buffer".into()))
                    .and_then(|b| {
                        serde_json::from_slice::<HttpRequest>(&b)
                            .map_err(|e| DriverError::Protocol(format!("bad HTTP request: {e}")))
                    })
                    .and_then(|req| http::execute(&caller.data().policy, &req))
                {
                    Ok(resp) => json!({ "ok": resp }).to_string().into_bytes(),
                    Err(e) => envelope_err(e),
                };
                let n = reply.len() as i32;
                caller.data_mut().pending = Some(reply);
                n
            },
        )
        .map_err(bind_err)?;

    linker
        .func_wrap(
            "tabular",
            "take",
            |mut caller: Caller<HostState>, ptr: i32, len: i32| -> i32 {
                let Some(buf) = caller.data_mut().pending.take() else {
                    return -1;
                };
                if ptr < 0 || len < 0 || (len as usize) < buf.len() {
                    return -1;
                }
                let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory())
                else {
                    return -1;
                };
                let start = ptr as usize;
                match memory
                    .data_mut(&mut caller)
                    .get_mut(start..start + buf.len())
                {
                    Some(dst) => {
                        dst.copy_from_slice(&buf);
                        buf.len() as i32
                    }
                    None => -1,
                }
            },
        )
        .map_err(bind_err)?;

    linker
        .func_wrap("tabular", "now_ms", || -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        })
        .map_err(bind_err)?;

    Ok(linker)
}

impl WasmDriver {
    /// Kompilasi modul (`.wasm` atau teks `.wat`) dan periksa ABI-nya.
    pub fn load(
        bytes: &[u8],
        descriptor: EngineDescriptor,
        http_hosts: Vec<String>,
    ) -> DriverResult<Self> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes)
            .map_err(|e| DriverError::Plugin(format!("invalid Wasm module: {e}")))?;
        let linker = create_linker(&engine)?;
        let inner = Arc::new(Inner {
            descriptor,
            engine,
            module,
            linker,
            http_hosts,
        });
        // Instansiasi sekali untuk memeriksa export dan versi ABI.
        WasmInstance::new(&inner, HttpPolicy::default())?;
        Ok(Self { inner })
    }
}

struct WasmInstance {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    free: TypedFunc<(i32, i32), ()>,
    call: TypedFunc<(i32, i32, i32, i32), i64>,
}

impl WasmInstance {
    fn new(inner: &Inner, policy: HttpPolicy) -> DriverResult<Self> {
        let state = HostState {
            engine_id: inner.descriptor.id.clone(),
            policy,
            pending: None,
            limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build(),
        };
        let mut store = Store::new(&inner.engine, state);
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|e| DriverError::Plugin(e.to_string()))?;
        let instance = inner
            .linker
            .instantiate_and_start(&mut store, &inner.module)
            .map_err(|e| DriverError::Plugin(format!("cannot instantiate plugin: {e}")))?;
        let missing =
            |name: &str| DriverError::Protocol(format!("plugin does not export '{name}'"));
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| missing("memory"))?;
        let version = instance
            .get_typed_func::<(), i32>(&store, "tdrv_abi_version")
            .map_err(|_| missing("tdrv_abi_version"))?
            .call(&mut store, ())
            .map_err(|e| DriverError::Plugin(e.to_string()))?;
        if version != ABI_VERSION {
            return Err(DriverError::Protocol(format!(
                "plugin ABI version {version} is not supported (expected {ABI_VERSION})"
            )));
        }
        Ok(Self {
            alloc: instance
                .get_typed_func(&store, "tdrv_alloc")
                .map_err(|_| missing("tdrv_alloc"))?,
            free: instance
                .get_typed_func(&store, "tdrv_free")
                .map_err(|_| missing("tdrv_free"))?,
            call: instance
                .get_typed_func(&store, "tdrv_call")
                .map_err(|_| missing("tdrv_call"))?,
            memory,
            store,
        })
    }

    fn trap(&self, e: wasmi::Error) -> DriverError {
        if self.store.get_fuel().is_ok_and(|f| f == 0) {
            DriverError::Plugin("plugin exceeded its CPU budget".into())
        } else {
            DriverError::Plugin(format!("plugin trapped: {e}"))
        }
    }

    fn write(&mut self, bytes: &[u8]) -> DriverResult<(i32, i32)> {
        let len = i32::try_from(bytes.len())
            .map_err(|_| DriverError::Protocol("message too large".into()))?;
        let ptr = self
            .alloc
            .call(&mut self.store, len)
            .map_err(|e| self.trap(e))?;
        let start = usize::try_from(ptr)
            .map_err(|_| DriverError::Protocol("tdrv_alloc returned a negative pointer".into()))?;
        let dst = self
            .memory
            .data_mut(&mut self.store)
            .get_mut(start..start + bytes.len())
            .ok_or_else(|| DriverError::Protocol("tdrv_alloc returned an invalid range".into()))?;
        dst.copy_from_slice(bytes);
        Ok((ptr, len))
    }

    fn invoke(&mut self, method: &str, payload: &Value) -> DriverResult<Value> {
        self.store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|e| DriverError::Plugin(e.to_string()))?;
        let payload = payload.to_string();
        let (mp, ml) = self.write(method.as_bytes())?;
        let (pp, pl) = self.write(payload.as_bytes())?;
        let packed = self
            .call
            .call(&mut self.store, (mp, ml, pp, pl))
            .map_err(|e| self.trap(e))?;
        let _ = self.free.call(&mut self.store, (mp, ml));
        let _ = self.free.call(&mut self.store, (pp, pl));

        let ptr = (packed as u64 >> 32) as usize;
        let len = (packed as u64 & 0xffff_ffff) as usize;
        if len > MAX_GUEST_MESSAGE {
            return Err(DriverError::Protocol("plugin response too large".into()));
        }
        let bytes = self
            .memory
            .data(&self.store)
            .get(ptr..ptr + len)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| DriverError::Protocol("plugin returned an invalid buffer".into()))?;
        let _ = self.free.call(&mut self.store, (ptr as i32, len as i32));
        parse_envelope(&bytes)
    }
}

/// Uraikan `{"ok": ..}` / `{"err": ".."}` dari guest atau sidecar.
pub(crate) fn parse_envelope(bytes: &[u8]) -> DriverResult<Value> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| DriverError::Protocol(format!("plugin returned invalid JSON: {e}")))?;
    envelope_value(value)
}

pub(crate) fn envelope_value(mut value: Value) -> DriverResult<Value> {
    if let Some(err) = value.get("err") {
        let msg = err
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| err.to_string());
        return Err(DriverError::Query(msg));
    }
    value
        .get_mut("ok")
        .map(Value::take)
        .ok_or_else(|| DriverError::Protocol("plugin response has neither 'ok' nor 'err'".into()))
}

/// Balasan `execute` dari plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteReply {
    Output(ExecuteOutput),
    HttpTable(HttpTableRequest),
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(method: &str, v: Value) -> DriverResult<T> {
    serde_json::from_value(v)
        .map_err(|e| DriverError::Protocol(format!("invalid '{method}' reply: {e}")))
}

struct Pool {
    idle: Vec<WasmInstance>,
    total: usize,
}

pub struct WasmSession {
    inner: Arc<Inner>,
    params: ConnectParams,
    policy: HttpPolicy,
    pool: Mutex<Pool>,
    available: Condvar,
}

/// Instance yang sedang dipinjam; kembali ke pool saat di-drop, kecuali
/// guest trap (state-nya tidak bisa dipercaya lagi).
struct Lease<'a> {
    session: &'a WasmSession,
    instance: Option<WasmInstance>,
    healthy: bool,
}

impl Lease<'_> {
    fn invoke(&mut self, method: &str, payload: &Value) -> DriverResult<Value> {
        let instance = self
            .instance
            .as_mut()
            .ok_or_else(|| DriverError::Plugin("instance released".into()))?;
        let result = instance.invoke(method, payload);
        if matches!(
            result,
            Err(DriverError::Plugin(_) | DriverError::Protocol(_))
        ) {
            self.healthy = false;
        }
        result
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let Ok(mut pool) = self.session.pool.lock() else {
            return;
        };
        match self.instance.take() {
            Some(instance) if self.healthy => pool.idle.push(instance),
            _ => pool.total = pool.total.saturating_sub(1),
        }
        self.session.available.notify_one();
    }
}

impl WasmSession {
    fn new_instance(&self) -> DriverResult<WasmInstance> {
        let mut instance = WasmInstance::new(&self.inner, self.policy.clone())?;
        let params =
            serde_json::to_value(&self.params).map_err(|e| DriverError::Protocol(e.to_string()))?;
        instance
            .invoke("connect", &params)
            .map_err(|e| match e {
                DriverError::Query(m) => DriverError::Connect(m),
                other => other,
            })?;
        Ok(instance)
    }

    fn acquire(&self) -> DriverResult<Lease<'_>> {
        let poisoned = || DriverError::Plugin("instance pool poisoned".into());
        let mut pool = self.pool.lock().map_err(|_| poisoned())?;
        loop {
            if let Some(instance) = pool.idle.pop() {
                return Ok(Lease {
                    session: self,
                    instance: Some(instance),
                    healthy: true,
                });
            }
            if pool.total < MAX_INSTANCES {
                pool.total += 1;
                drop(pool);
                return match self.new_instance() {
                    Ok(instance) => Ok(Lease {
                        session: self,
                        instance: Some(instance),
                        healthy: true,
                    }),
                    Err(e) => {
                        if let Ok(mut pool) = self.pool.lock() {
                            pool.total = pool.total.saturating_sub(1);
                        }
                        self.available.notify_one();
                        Err(e)
                    }
                };
            }
            pool = self.available.wait(pool).map_err(|_| poisoned())?;
        }
    }

    fn call(&self, method: &str, payload: Value) -> DriverResult<Value> {
        self.acquire()?.invoke(method, &payload)
    }
}

impl EngineDriver for WasmDriver {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.inner.descriptor
    }

    fn connect(&self, params: ConnectParams) -> DriverResult<Arc<dyn EngineSession>> {
        let policy = HttpPolicy::new(&self.inner.http_hosts, &params.host);
        let session = WasmSession {
            inner: self.inner.clone(),
            params,
            policy,
            pool: Mutex::new(Pool {
                idle: Vec::new(),
                total: 0,
            }),
            available: Condvar::new(),
        };
        // Instance pertama memvalidasi kredensial lewat `connect`.
        drop(session.acquire()?);
        Ok(Arc::new(session))
    }
}

impl EngineSession for WasmSession {
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
        let payload =
            serde_json::to_value(request).map_err(|e| DriverError::Protocol(e.to_string()))?;
        // Instance sudah dilepas sebelum request tabel berjalan di host.
        let reply: ExecuteReply = decode("execute", self.call("execute", payload)?)?;
        match reply {
            ExecuteReply::Output(output) => Ok(output),
            ExecuteReply::HttpTable(table) => {
                http::execute_table(&self.policy, &table, request.max_rows)
            }
        }
    }

    fn cancel(&self, job_id: u64) -> DriverResult<()> {
        if !self.inner.descriptor.capabilities.cancel {
            return Err(DriverError::Unsupported("cancel".into()));
        }
        self.call("cancel", json!({ "job_id": job_id })).map(|_| ())
    }

    fn close(&self) {
        let idle = match self.pool.lock() {
            Ok(mut pool) => std::mem::take(&mut pool.idle),
            Err(_) => return,
        };
        for mut instance in idle {
            let _ = instance.invoke("close", &json!({}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> EngineDescriptor {
        serde_json::from_str(r#"{"id":"fake","name":"Fake"}"#).unwrap()
    }

    /// Guest WAT minimal: `tdrv_call` selalu membalas `response`.
    fn constant_guest(response: &str) -> String {
        let escaped = response.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            r#"(module
                (memory (export "memory") 4)
                (global $heap (mut i32) (i32.const 65536))
                (data (i32.const 16) "{escaped}")
                (func (export "tdrv_abi_version") (result i32) (i32.const 1))
                (func (export "tdrv_alloc") (param $n i32) (result i32) (local $p i32)
                  (local.set $p (global.get $heap))
                  (global.set $heap (i32.add (global.get $heap) (local.get $n)))
                  (local.get $p))
                (func (export "tdrv_free") (param i32 i32))
                (func (export "tdrv_call") (param i32 i32 i32 i32) (result i64)
                  (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {len}))))"#,
            len = response.len()
        )
    }

    /// Guest yang membalas `connect` dengan ok:null, dan untuk method lain
    /// meneruskan request HTTP tetap ke host lalu mengembalikan balasan host.
    fn http_forwarding_guest(request_json: &str) -> String {
        let escaped = request_json.replace('\\', "\\\\").replace('"', "\\\"");
        let ok_null = r#"{"ok":null}"#;
        format!(
            r#"(module
                (import "tabular" "http" (func $http (param i32 i32) (result i32)))
                (import "tabular" "take" (func $take (param i32 i32) (result i32)))
                (memory (export "memory") 4)
                (global $heap (mut i32) (i32.const 65536))
                (data (i32.const 16) "{escaped}")
                (data (i32.const 4096) "{ok_null_escaped}")
                (func $alloc (export "tdrv_alloc") (param $n i32) (result i32) (local $p i32)
                  (local.set $p (global.get $heap))
                  (global.set $heap (i32.add (global.get $heap) (local.get $n)))
                  (local.get $p))
                (func (export "tdrv_abi_version") (result i32) (i32.const 1))
                (func (export "tdrv_free") (param i32 i32))
                (func (export "tdrv_call") (param i32 i32 i32 i32) (result i64)
                  (local $n i32) (local $p i32)
                  (if (i32.eq (local.get 1) (i32.const 7))
                    (then (return (i64.or (i64.shl (i64.const 4096) (i64.const 32))
                                          (i64.const {ok_len})))))
                  (local.set $n (call $http (i32.const 16) (i32.const {len})))
                  (local.set $p (call $alloc (local.get $n)))
                  (drop (call $take (local.get $p) (local.get $n)))
                  (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32))
                          (i64.extend_i32_u (local.get $n)))))"#,
            len = request_json.len(),
            ok_null_escaped = ok_null.replace('"', "\\\""),
            ok_len = ok_null.len()
        )
    }

    fn request(query: &str) -> ExecuteRequest {
        ExecuteRequest {
            query: query.into(),
            database: None,
            schema: None,
            max_rows: 100,
            job_id: 7,
        }
    }

    #[test]
    fn rejects_module_without_driver_exports() {
        let err = WasmDriver::load(
            b"(module (memory (export \"memory\") 1))",
            descriptor(),
            vec![],
        )
        .err()
        .unwrap();
        assert!(err.to_string().contains("tdrv_abi_version"), "{err}");
    }

    #[test]
    fn rejects_wrong_abi_version() {
        let wat = constant_guest(r#"{"ok":null}"#).replace(
            "(result i32) (i32.const 1))",
            "(result i32) (i32.const 2))",
        );
        let err = WasmDriver::load(wat.as_bytes(), descriptor(), vec![])
            .err()
            .unwrap();
        assert!(err.to_string().contains("ABI version 2"), "{err}");
    }

    #[test]
    fn executes_through_guest_and_reports_output() {
        let wat = constant_guest(r#"{"ok":{"output":{"headers":["x"],"rows":[["1"],[null]]}}}"#);
        let driver = WasmDriver::load(wat.as_bytes(), descriptor(), vec![]).unwrap();
        let session = driver.connect(ConnectParams::default()).unwrap();
        let out = session.execute(&request("select 1")).unwrap();
        assert_eq!(out.headers, vec!["x"]);
        assert_eq!(out.rows, vec![vec![Some("1".into())], vec![None]]);
    }

    #[test]
    fn guest_error_on_connect_becomes_connect_error() {
        let wat = constant_guest(r#"{"err":"authentication failed"}"#);
        let driver = WasmDriver::load(wat.as_bytes(), descriptor(), vec![]).unwrap();
        let err = driver.connect(ConnectParams::default()).err().unwrap();
        assert_eq!(err, DriverError::Connect("authentication failed".into()));
    }

    #[test]
    fn host_http_enforces_allowlist() {
        let req = r#"{"method":"GET","url":"http://evil.example/steal"}"#;
        let wat = http_forwarding_guest(req);
        let driver = WasmDriver::load(
            wat.as_bytes(),
            descriptor(),
            vec!["{connection.host}".into()],
        )
        .unwrap();
        let session = driver
            .connect(ConnectParams {
                host: "db.internal".into(),
                ..Default::default()
            })
            .unwrap();
        // "list_databases" (bukan "connect") diteruskan guest ke host HTTP.
        let err = session.list_databases().err().unwrap();
        assert!(
            err.to_string().contains("not in the plugin's allowed hosts"),
            "{err}"
        );
    }

    #[test]
    fn instances_are_shared_across_threads() {
        let wat = constant_guest(r#"{"ok":{"output":{"headers":[],"rows":[]}}}"#);
        let driver = WasmDriver::load(wat.as_bytes(), descriptor(), vec![]).unwrap();
        let session = driver.connect(ConnectParams::default()).unwrap();
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let s = session.clone();
                std::thread::spawn(move || s.execute(&request("select 1")).is_ok())
            })
            .collect();
        assert!(handles.into_iter().all(|h| h.join().unwrap()));
    }

    #[test]
    fn cancel_requires_capability() {
        let wat = constant_guest(r#"{"ok":null}"#);
        let driver = WasmDriver::load(wat.as_bytes(), descriptor(), vec![]).unwrap();
        let session = driver.connect(ConnectParams::default()).unwrap();
        assert!(matches!(session.cancel(1), Err(DriverError::Unsupported(_))));
    }

    #[test]
    fn envelope_parsing() {
        assert_eq!(parse_envelope(br#"{"ok":[1]}"#).unwrap(), json!([1]));
        assert_eq!(
            parse_envelope(br#"{"err":"boom"}"#),
            Err(DriverError::Query("boom".into()))
        );
        assert!(parse_envelope(br#"{"x":1}"#).is_err());
        assert!(parse_envelope(b"nope").is_err());
    }
}
