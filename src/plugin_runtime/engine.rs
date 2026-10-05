use crate::plugin_runtime::host_api::{
    PluginExportPayload, PluginLogEntry, PluginLogLevel, PluginSelectionData, PluginTableSchema,
};
use wasmi::errors::{ErrorKind, InstantiationError, MemoryError, TableError};
use wasmi::{
    Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, TrapCode,
};

/// Batas sumber daya satu eksekusi plugin. Nilai bawaan sama dengan host
/// plugin driver (`driver_api::wasm_host`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginLimits {
    /// Anggaran instruksi; habis = eksekusi dihentikan. Diisi ulang sebelum
    /// instansiasi (fungsi `start`) dan sebelum entrypoint dipanggil.
    pub fuel: u64,
    /// Ukuran maksimum satu linear memory, dalam byte.
    pub memory_bytes: usize,
    /// Jumlah maksimum elemen satu tabel.
    pub table_elements: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            fuel: 10_000_000_000,
            memory_bytes: 256 << 20,
            table_elements: 1_000_000,
        }
    }
}

/// Isi `Store`: konteks plugin dan pembatas sumber dayanya.
struct HostData {
    ctx: PluginExecutionContext,
    limits: StoreLimits,
}

/// Sandboxed Execution Context holding input data and capturing outputs
#[derive(Debug, Clone, Default)]
pub struct PluginExecutionContext {
    pub table_schema: Option<PluginTableSchema>,
    pub selection_data: Option<PluginSelectionData>,
    pub cached_schema_json: Option<String>,
    pub cached_selection_json: Option<String>,
    pub captured_exports: Vec<PluginExportPayload>,
    pub captured_logs: Vec<PluginLogEntry>,
    pub result_output: Option<String>,
    pub error_message: Option<String>,
}

impl PluginExecutionContext {
    pub fn new(schema: Option<PluginTableSchema>, selection: Option<PluginSelectionData>) -> Self {
        let cached_schema_json = schema.as_ref().and_then(|s| serde_json::to_string(s).ok());
        let cached_selection_json = selection
            .as_ref()
            .and_then(|s| serde_json::to_string(s).ok());

        Self {
            table_schema: schema,
            selection_data: selection,
            cached_schema_json,
            cached_selection_json,
            captured_exports: Vec::new(),
            captured_logs: Vec::new(),
            result_output: None,
            error_message: None,
        }
    }
}

/// Helper function to read a slice of bytes from guest memory safely
fn read_guest_memory(caller: &Caller<HostData>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    if ptr < 0 || len < 0 {
        return None;
    }
    let ptr = ptr as usize;
    let len = len as usize;
    let memory = caller.get_export("memory")?.into_memory()?;
    let memory_slice = memory.data(caller);
    memory_slice
        .get(ptr..ptr.checked_add(len)?)
        .map(<[u8]>::to_vec)
}

/// Helper function to read a UTF-8 string from guest memory safely
fn read_guest_string(caller: &Caller<HostData>, ptr: i32, len: i32) -> Option<String> {
    let bytes = read_guest_memory(caller, ptr, len)?;
    String::from_utf8(bytes).ok()
}

/// Helper function to write bytes to guest memory safely
fn write_guest_memory(caller: &mut Caller<HostData>, ptr: i32, bytes: &[u8]) -> Option<usize> {
    if ptr < 0 {
        return None;
    }
    let ptr = ptr as usize;
    let memory = caller.get_export("memory")?.into_memory()?;
    let memory_slice = memory.data_mut(caller);
    memory_slice
        .get_mut(ptr..ptr.checked_add(bytes.len())?)?
        .copy_from_slice(bytes);
    Some(bytes.len())
}

/// WebAssembly Sandboxed Plugin Engine
#[derive(Clone)]
pub struct WasmPluginEngine {
    engine: Engine,
    limits: PluginLimits,
}

impl Default for WasmPluginEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmPluginEngine {
    pub fn new() -> Self {
        Self::with_limits(PluginLimits::default())
    }

    /// Engine dengan batas sumber daya tertentu. Fuel selalu dihitung, jadi
    /// modul yang berputar tanpa henti pasti berhenti.
    pub fn with_limits(limits: PluginLimits) -> Self {
        let mut config = Config::default();
        config.wasm_tail_call(true);
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        Self { engine, limits }
    }

    pub fn limits(&self) -> PluginLimits {
        self.limits
    }

    /// Pesan galat yang bisa dibaca untuk kegagalan di `phase`: habis fuel
    /// dan batas memori/tabel disebut apa adanya, bukan sebagai trap mentah.
    fn describe_error(&self, phase: &str, error: &wasmi::Error) -> String {
        let out_of_fuel = error.as_trap_code() == Some(TrapCode::OutOfFuel)
            || matches!(
                error.kind(),
                ErrorKind::Memory(MemoryError::OutOfFuel { .. })
                    | ErrorKind::Table(TableError::OutOfFuel { .. })
                    | ErrorKind::Fuel(_)
            );
        if out_of_fuel {
            return format!(
                "Plugin stopped: it exceeded its CPU budget ({} instructions) and was out of fuel. \
                 The module may contain an infinite loop.",
                self.limits.fuel
            );
        }
        let memory_denied = |e: &MemoryError| {
            matches!(
                e,
                MemoryError::ResourceLimiterDeniedAllocation
                    | MemoryError::OutOfBoundsGrowth
                    | MemoryError::OutOfSystemMemory
            )
        };
        let table_denied = |e: &TableError| {
            matches!(
                e,
                TableError::ResourceLimiterDeniedAllocation | TableError::GrowOutOfBounds
            )
        };
        let over_limit = match error.kind() {
            ErrorKind::Memory(e) => memory_denied(e),
            ErrorKind::Table(e) => table_denied(e),
            ErrorKind::Instantiation(InstantiationError::FailedToInstantiateMemory(e)) => {
                memory_denied(e)
            }
            ErrorKind::Instantiation(InstantiationError::FailedToInstantiateTable(e)) => {
                table_denied(e)
            }
            ErrorKind::Instantiation(
                InstantiationError::TooManyInstances
                | InstantiationError::TooManyTables
                | InstantiationError::TooManyMemories,
            ) => true,
            _ => false,
        };
        if over_limit {
            return format!(
                "Plugin stopped: it exceeded its memory limit ({} MiB) or instance limits: {}",
                self.limits.memory_bytes >> 20,
                error
            );
        }
        format!("{phase}: {error}")
    }

    /// Setup the Linker with safe host APIs exposed to plugins
    fn create_linker(&self) -> Result<Linker<HostData>, String> {
        let mut linker = Linker::new(&self.engine);

        // Host API: tabular_get_table_schema_len() -> i32
        linker
            .func_wrap(
                "env",
                "tabular_get_table_schema_len",
                |caller: Caller<HostData>| -> i32 {
                    caller
                        .data()
                        .ctx
                        .cached_schema_json
                        .as_ref()
                        .map(|s| s.len() as i32)
                        .unwrap_or(0)
                },
            )
            .map_err(|e| format!("Failed to bind tabular_get_table_schema_len: {}", e))?;

        // Host API: tabular_get_table_schema_data(ptr: i32, max_len: i32) -> i32
        linker
            .func_wrap(
                "env",
                "tabular_get_table_schema_data",
                |mut caller: Caller<HostData>, ptr: i32, max_len: i32| -> i32 {
                    if let Some(json_bytes) = caller
                        .data()
                        .ctx
                        .cached_schema_json
                        .as_ref()
                        .map(|s| s.as_bytes().to_vec())
                    {
                        let to_write = json_bytes.len().min(usize::try_from(max_len).unwrap_or(0));
                        if let Some(written) =
                            write_guest_memory(&mut caller, ptr, &json_bytes[..to_write])
                        {
                            return written as i32;
                        }
                    }
                    0
                },
            )
            .map_err(|e| format!("Failed to bind tabular_get_table_schema_data: {}", e))?;

        // Host API: tabular_get_selected_rows_len() -> i32
        linker
            .func_wrap(
                "env",
                "tabular_get_selected_rows_len",
                |caller: Caller<HostData>| -> i32 {
                    caller
                        .data()
                        .ctx
                        .cached_selection_json
                        .as_ref()
                        .map(|s| s.len() as i32)
                        .unwrap_or(0)
                },
            )
            .map_err(|e| format!("Failed to bind tabular_get_selected_rows_len: {}", e))?;

        // Host API: tabular_get_selected_rows_data(ptr: i32, max_len: i32) -> i32
        linker
            .func_wrap(
                "env",
                "tabular_get_selected_rows_data",
                |mut caller: Caller<HostData>, ptr: i32, max_len: i32| -> i32 {
                    if let Some(json_bytes) = caller
                        .data()
                        .ctx
                        .cached_selection_json
                        .as_ref()
                        .map(|s| s.as_bytes().to_vec())
                    {
                        let to_write = json_bytes.len().min(usize::try_from(max_len).unwrap_or(0));
                        if let Some(written) =
                            write_guest_memory(&mut caller, ptr, &json_bytes[..to_write])
                        {
                            return written as i32;
                        }
                    }
                    0
                },
            )
            .map_err(|e| format!("Failed to bind tabular_get_selected_rows_data: {}", e))?;

        // Host API: tabular_export_data(format_ptr: i32, format_len: i32, payload_ptr: i32, payload_len: i32) -> i32
        linker
            .func_wrap(
                "env",
                "tabular_export_data",
                |mut caller: Caller<HostData>,
                 format_ptr: i32,
                 format_len: i32,
                 payload_ptr: i32,
                 payload_len: i32|
                 -> i32 {
                    let format = read_guest_string(&caller, format_ptr, format_len)
                        .unwrap_or_else(|| "text".to_string());
                    let payload =
                        read_guest_string(&caller, payload_ptr, payload_len).unwrap_or_default();

                    let (content_type, filename_ext) = match format.to_lowercase().as_str() {
                        "parquet" => ("application/vnd.apache.parquet", "parquet"),
                        "duckdb" | "sql" => ("application/sql", "sql"),
                        "json" => ("application/json", "json"),
                        "prisma" => ("text/plain", "prisma"),
                        "typescript" | "ts" => ("application/typescript", "ts"),
                        "python" | "py" => ("text/x-python", "py"),
                        "rust" | "rs" => ("text/rust", "rs"),
                        _ => ("text/plain", "txt"),
                    };

                    let table_name = caller
                        .data()
                        .ctx
                        .table_schema
                        .as_ref()
                        .map(|s| s.table_name.clone())
                        .unwrap_or_else(|| "export".to_string());

                    let export = PluginExportPayload {
                        format: format.clone(),
                        filename_suggestion: format!("{}_{}.{}", table_name, format, filename_ext),
                        content_type: content_type.to_string(),
                        text_content: Some(payload),
                        binary_base64: None,
                        metadata: None,
                    };

                    caller.data_mut().ctx.captured_exports.push(export);
                    1
                },
            )
            .map_err(|e| format!("Failed to bind tabular_export_data: {}", e))?;

        // Host API: tabular_log(level: i32, msg_ptr: i32, msg_len: i32) -> i32
        linker
            .func_wrap(
                "env",
                "tabular_log",
                |mut caller: Caller<HostData>, level: i32, msg_ptr: i32, msg_len: i32| -> i32 {
                    let msg = read_guest_string(&caller, msg_ptr, msg_len).unwrap_or_default();
                    let lvl = PluginLogLevel::from(level);
                    match lvl {
                        PluginLogLevel::Debug => log::debug!("[WasmPlugin] {}", msg),
                        PluginLogLevel::Info => log::info!("[WasmPlugin] {}", msg),
                        PluginLogLevel::Warn => log::warn!("[WasmPlugin] {}", msg),
                        PluginLogLevel::Error => log::error!("[WasmPlugin] {}", msg),
                    }

                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    caller.data_mut().ctx.captured_logs.push(PluginLogEntry {
                        level: lvl,
                        message: msg,
                        timestamp_millis: now,
                    });
                    1
                },
            )
            .map_err(|e| format!("Failed to bind tabular_log: {}", e))?;

        // Host API: tabular_set_result(ptr: i32, len: i32) -> i32
        linker
            .func_wrap(
                "env",
                "tabular_set_result",
                |mut caller: Caller<HostData>, ptr: i32, len: i32| -> i32 {
                    if let Some(res) = read_guest_string(&caller, ptr, len) {
                        caller.data_mut().ctx.result_output = Some(res);
                        1
                    } else {
                        0
                    }
                },
            )
            .map_err(|e| format!("Failed to bind tabular_set_result: {}", e))?;

        Ok(linker)
    }

    /// Executes a WebAssembly binary (.wasm) or WebAssembly text format (.wat) with the given context
    pub fn execute(
        &self,
        wasm_or_wat_bytes: &[u8],
        entrypoint: &str,
        context: PluginExecutionContext,
    ) -> Result<PluginExecutionContext, String> {
        let module = Module::new(&self.engine, wasm_or_wat_bytes)
            .map_err(|e| format!("Failed to parse WebAssembly module: {}", e))?;

        let linker = self.create_linker()?;
        let data = HostData {
            ctx: context,
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory_bytes)
                .table_elements(self.limits.table_elements)
                .instances(1)
                .memories(1)
                .tables(4)
                .build(),
        };
        let mut store = Store::new(&self.engine, data);
        store.limiter(|data| &mut data.limits);
        // Fungsi `start` modul juga kode tamu: beri anggaran yang sama.
        store
            .set_fuel(self.limits.fuel)
            .map_err(|e| format!("Failed to set the plugin fuel budget: {}", e))?;

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| self.describe_error("Failed to instantiate and start module", &e))?;

        // Attempt entrypoint function resolution
        // Check for specified entrypoint first (e.g., "tabular_main", "run", etc.)
        let func = instance
            .get_typed_func::<(), ()>(&store, entrypoint)
            .or_else(|_| instance.get_typed_func::<(), ()>(&store, "tabular_main"))
            .or_else(|_| instance.get_typed_func::<(), ()>(&store, "run"))
            .or_else(|_| instance.get_typed_func::<(), ()>(&store, "main"))
            .map_err(|e| {
                format!(
                    "Module does not export entrypoint '{}' or fallback ('tabular_main', 'run', 'main'): {}",
                    entrypoint, e
                )
            })?;

        store
            .set_fuel(self.limits.fuel)
            .map_err(|e| format!("Failed to set the plugin fuel budget: {}", e))?;
        func.call(&mut store, ())
            .map_err(|e| self.describe_error("Plugin runtime execution error", &e))?;

        Ok(store.into_data().ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> WasmPluginEngine {
        WasmPluginEngine::with_limits(PluginLimits {
            fuel: 200_000,
            memory_bytes: 2 * 65_536,
            ..Default::default()
        })
    }

    #[test]
    fn infinite_loop_runs_out_of_fuel() {
        let wat = r#"(module
            (memory (export "memory") 1)
            (func (export "tabular_main") (loop $spin (br $spin))))"#;
        let started = std::time::Instant::now();
        let err = small()
            .execute(
                wat.as_bytes(),
                "tabular_main",
                PluginExecutionContext::default(),
            )
            .unwrap_err();
        assert!(err.contains("out of fuel"), "{err}");
        assert!(err.contains("200000 instructions"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn infinite_loop_in_start_function_runs_out_of_fuel() {
        let wat = r#"(module
            (func $spin (loop $l (br $l)))
            (start $spin)
            (func (export "tabular_main")))"#;
        let err = small()
            .execute(
                wat.as_bytes(),
                "tabular_main",
                PluginExecutionContext::default(),
            )
            .unwrap_err();
        assert!(err.contains("out of fuel"), "{err}");
    }

    #[test]
    fn memory_is_limited() {
        // Memori awal di atas batas: instansiasi ditolak.
        let big = r#"(module (memory (export "memory") 3) (func (export "tabular_main")))"#;
        let err = small()
            .execute(
                big.as_bytes(),
                "tabular_main",
                PluginExecutionContext::default(),
            )
            .unwrap_err();
        assert!(err.contains("memory limit"), "{err}");

        // `memory.grow` melewati batas mengembalikan -1; modul ini melaporkan
        // hasilnya lewat `tabular_set_result` ("Y" = tumbuh, "N" = ditolak).
        let grow = r#"(module
            (import "env" "tabular_set_result" (func $set (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "YN")
            (func (export "tabular_main")
                (drop (call $set
                    (select (i32.const 1) (i32.const 0)
                        (i32.eq (memory.grow (i32.const 5)) (i32.const -1)))
                    (i32.const 1)))))"#;
        let ctx = small()
            .execute(
                grow.as_bytes(),
                "tabular_main",
                PluginExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(ctx.result_output.as_deref(), Some("N"));
        // Dalam batas (1 + 1 halaman <= 2 halaman) tetap boleh.
        let ok = grow.replace("(i32.const 5)", "(i32.const 1)");
        let ctx = small()
            .execute(
                ok.as_bytes(),
                "tabular_main",
                PluginExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(ctx.result_output.as_deref(), Some("Y"));
    }

    #[test]
    fn host_calls_reject_out_of_range_guest_pointers() {
        // Pointer/len negatif atau di luar memori tidak boleh membuat host panik.
        let wat = r#"(module
            (import "env" "tabular_set_result" (func $set (param i32 i32) (result i32)))
            (import "env" "tabular_get_table_schema_data" (func $get (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "tabular_main")
                (drop (call $set (i32.const 2147483647) (i32.const 2147483647)))
                (drop (call $set (i32.const -1) (i32.const 4)))
                (drop (call $get (i32.const 65530) (i32.const -1)))
                (drop (call $get (i32.const 65530) (i32.const 1000)))))"#;
        let schema = crate::plugin_runtime::host_api::PluginTableSchema {
            table_name: "t".to_string(),
            schema_name: None,
            database_type: "SQLite".to_string(),
            columns: Vec::new(),
            total_rows: 0,
        };
        let ctx = small()
            .execute(
                wat.as_bytes(),
                "tabular_main",
                PluginExecutionContext::new(Some(schema), None),
            )
            .unwrap();
        assert!(ctx.result_output.is_none());
    }

    #[test]
    fn default_limits_match_the_driver_host() {
        let limits = WasmPluginEngine::new().limits();
        assert_eq!(limits.fuel, 10_000_000_000);
        assert_eq!(limits.memory_bytes, 256 << 20);
    }
}
