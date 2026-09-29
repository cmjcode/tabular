//! SDK guest untuk plugin driver Tabular (ABI `tabular-driver-v1`).
//!
//! Implementasikan [`Driver`] lalu panggil [`export_driver!`] sekali di crate
//! `cdylib` plugin. Makro itu membuat semua export yang dibutuhkan host
//! (`tdrv_abi_version`, `tdrv_alloc`, `tdrv_free`, `tdrv_call`).
//!
//! Plugin tidak punya socket; akses jaringan lewat [`http`], dan host hanya
//! mengizinkan host yang tercantum di `permissions.http_hosts` manifest.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsParams {
    pub enabled: bool,
    pub verify_server: bool,
    pub ca_cert: String,
    pub client_cert: String,
    pub client_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectParams {
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub database: String,
    pub tls: TlsParams,
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    #[default]
    Table,
    View,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    #[serde(default)]
    pub kind: TableKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub query: String,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    pub max_rows: usize,
    pub job_id: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecuteOutput {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub affected_rows: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Format respons tabel yang di-parse host secara native.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableFormat {
    JsonCompact,
    TsvWithNames,
    CsvWithNames,
    Ndjson,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpTableRequest {
    pub request: HttpRequest,
    pub format: TableFormat,
}

/// Balasan `execute`: hasil langsung, atau request yang dijalankan dan
/// di-parse host (disarankan untuk hasil besar).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteReply {
    Output(ExecuteOutput),
    HttpTable(HttpTableRequest),
}

pub type DriverResult<T> = Result<T, String>;

/// Satu koneksi engine. Host membuat beberapa instance plugin per koneksi
/// (masing-masing dengan `connect` sendiri) untuk kerja paralel.
pub trait Driver: Default {
    fn connect(&mut self, params: ConnectParams) -> DriverResult<()>;
    fn list_databases(&mut self) -> DriverResult<Vec<String>> {
        Ok(Vec::new())
    }
    fn list_schemas(&mut self, _database: Option<String>) -> DriverResult<Vec<String>> {
        Ok(Vec::new())
    }
    fn list_tables(
        &mut self,
        database: Option<String>,
        schema: Option<String>,
    ) -> DriverResult<Vec<TableInfo>>;
    fn list_columns(
        &mut self,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    ) -> DriverResult<Vec<ColumnInfo>>;
    fn execute(&mut self, request: ExecuteRequest) -> DriverResult<ExecuteReply>;
    fn cancel(&mut self, _job_id: u64) -> DriverResult<()> {
        Err("cancel is not supported".into())
    }
    fn close(&mut self) {}
}

#[cfg(target_arch = "wasm32")]
mod host {
    #[link(wasm_import_module = "tabular")]
    extern "C" {
        pub fn log(level: i32, ptr: *const u8, len: i32);
        pub fn http(ptr: *const u8, len: i32) -> i32;
        pub fn take(ptr: *mut u8, len: i32) -> i32;
        pub fn now_ms() -> i64;
    }
}

/// Level log: 0 debug, 1 info, 2 warn, 3 error.
pub fn log(level: i32, message: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        host::log(level, message.as_ptr(), message.len() as i32)
    };
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("[{level}] {message}");
}

pub fn now_ms() -> i64 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { host::now_ms() };
    #[cfg(not(target_arch = "wasm32"))]
    0
}

/// Jalankan request HTTP lewat host.
pub fn http(request: &HttpRequest) -> DriverResult<HttpResponse> {
    #[cfg(target_arch = "wasm32")]
    {
        let payload = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let len = unsafe { host::http(payload.as_ptr(), payload.len() as i32) };
        if len < 0 {
            return Err("host rejected the HTTP request".into());
        }
        let mut buf = vec![0u8; len as usize];
        let got = unsafe { host::take(buf.as_mut_ptr(), len) };
        if got != len {
            return Err("host HTTP buffer mismatch".into());
        }
        let mut value: Value = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
        if let Some(err) = value.get("err") {
            return Err(err.as_str().unwrap_or("HTTP error").to_string());
        }
        serde_json::from_value(value["ok"].take()).map_err(|e| e.to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = request;
        Err("host HTTP is only available inside Tabular".into())
    }
}

/// Percent-encode komponen URL (RFC 3986 unreserved dibiarkan).
pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn arg_str(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Jalankan satu method. Dipakai oleh [`export_driver!`]; tersedia publik
/// supaya driver bisa diuji di native tanpa host Wasm.
pub fn dispatch<D: Driver>(driver: &mut D, method: &str, payload: Value) -> Value {
    fn ok<T: Serialize>(v: DriverResult<T>) -> Value {
        match v.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())) {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "err": e }),
        }
    }
    fn parse<T: serde::de::DeserializeOwned>(payload: Value) -> DriverResult<T> {
        serde_json::from_value(payload).map_err(|e| format!("invalid payload: {e}"))
    }
    match method {
        "connect" => ok(parse(payload).and_then(|p| driver.connect(p))),
        "list_databases" => ok(driver.list_databases()),
        "list_schemas" => ok(driver.list_schemas(arg_str(&payload, "database"))),
        "list_tables" => ok(driver.list_tables(
            arg_str(&payload, "database"),
            arg_str(&payload, "schema"),
        )),
        "list_columns" => match arg_str(&payload, "table") {
            Some(table) => ok(driver.list_columns(
                arg_str(&payload, "database"),
                arg_str(&payload, "schema"),
                table,
            )),
            None => json!({ "err": "missing 'table'" }),
        },
        "execute" => ok(parse(payload).and_then(|r| driver.execute(r))),
        "cancel" => ok(driver.cancel(payload.get("job_id").and_then(Value::as_u64).unwrap_or(0))),
        "close" => {
            driver.close();
            json!({ "ok": null })
        }
        other => json!({ "err": format!("unknown method '{other}'") }),
    }
}

#[doc(hidden)]
pub fn __alloc(len: i32) -> i32 {
    let buf = vec![0u8; len.max(0) as usize].into_boxed_slice();
    Box::into_raw(buf) as *mut u8 as usize as i32
}

#[doc(hidden)]
/// # Safety
/// `ptr`/`len` harus berasal dari [`__alloc`] atau balasan `tdrv_call`.
pub unsafe fn __free(ptr: i32, len: i32) {
    if ptr == 0 || len < 0 {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(ptr as usize as *mut u8, len as usize);
    drop(Box::from_raw(slice));
}

#[doc(hidden)]
/// # Safety
/// Pointer input berasal dari host lewat `tdrv_alloc`.
pub unsafe fn __call<D: Driver>(
    driver: &mut D,
    method_ptr: i32,
    method_len: i32,
    payload_ptr: i32,
    payload_len: i32,
) -> i64 {
    let method = std::slice::from_raw_parts(method_ptr as usize as *const u8, method_len as usize);
    let payload =
        std::slice::from_raw_parts(payload_ptr as usize as *const u8, payload_len as usize);
    let method = std::str::from_utf8(method).unwrap_or("");
    let payload: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
    let reply = dispatch(driver, method, payload).to_string().into_bytes();
    let len = reply.len();
    let ptr = Box::into_raw(reply.into_boxed_slice()) as *mut u8 as usize as u64;
    ((ptr << 32) | len as u64) as i64
}

/// Buat export ABI untuk tipe driver `$ty` (harus `Driver + Default`).
#[macro_export]
macro_rules! export_driver {
    ($ty:ty) => {
        thread_local! {
            static __TDRV_DRIVER: ::std::cell::RefCell<$ty> =
                ::std::cell::RefCell::new(<$ty as ::std::default::Default>::default());
        }

        #[no_mangle]
        pub extern "C" fn tdrv_abi_version() -> i32 {
            1
        }

        #[no_mangle]
        pub extern "C" fn tdrv_alloc(len: i32) -> i32 {
            $crate::__alloc(len)
        }

        #[no_mangle]
        pub unsafe extern "C" fn tdrv_free(ptr: i32, len: i32) {
            $crate::__free(ptr, len)
        }

        #[no_mangle]
        pub unsafe extern "C" fn tdrv_call(mp: i32, ml: i32, pp: i32, pl: i32) -> i64 {
            __TDRV_DRIVER.with(|d| $crate::__call(&mut *d.borrow_mut(), mp, ml, pp, pl))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Echo {
        db: String,
    }

    impl Driver for Echo {
        fn connect(&mut self, p: ConnectParams) -> DriverResult<()> {
            self.db = p.database;
            Ok(())
        }
        fn list_tables(&mut self, _: Option<String>, _: Option<String>) -> DriverResult<Vec<TableInfo>> {
            Ok(vec![TableInfo { name: self.db.clone(), kind: TableKind::View }])
        }
        fn list_columns(&mut self, _: Option<String>, _: Option<String>, t: String) -> DriverResult<Vec<ColumnInfo>> {
            Err(format!("no columns for {t}"))
        }
        fn execute(&mut self, r: ExecuteRequest) -> DriverResult<ExecuteReply> {
            Ok(ExecuteReply::Output(ExecuteOutput {
                headers: vec!["q".into()],
                rows: vec![vec![Some(r.query)]],
                ..Default::default()
            }))
        }
    }

    #[test]
    fn dispatch_roundtrip() {
        let mut d = Echo::default();
        assert_eq!(dispatch(&mut d, "connect", json!({"database": "db1"})), json!({"ok": null}));
        assert_eq!(
            dispatch(&mut d, "list_tables", json!({})),
            json!({"ok": [{"name": "db1", "kind": "view"}]})
        );
        assert_eq!(
            dispatch(&mut d, "list_columns", json!({"table": "t"})),
            json!({"err": "no columns for t"})
        );
        let out = dispatch(&mut d, "execute", json!({"query": "select 1", "max_rows": 5, "job_id": 1}));
        assert_eq!(out["ok"]["output"]["rows"][0][0], "select 1");
        assert!(dispatch(&mut d, "nope", json!({}))["err"].is_string());
        assert!(dispatch(&mut d, "cancel", json!({"job_id": 1}))["err"].is_string());
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
