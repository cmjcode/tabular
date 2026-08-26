use wasmi::{Caller, Config, Engine, Linker, Module, Store};
use crate::plugin_runtime::host_api::{
    PluginExportPayload, PluginLogEntry, PluginLogLevel, PluginSelectionData, PluginTableSchema,
};

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
        let cached_schema_json = schema
            .as_ref()
            .and_then(|s| serde_json::to_string(s).ok());
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
fn read_guest_memory(caller: &Caller<PluginExecutionContext>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    if ptr < 0 || len < 0 {
        return None;
    }
    let ptr = ptr as usize;
    let len = len as usize;
    let memory = caller.get_export("memory")?.into_memory()?;
    let memory_slice = memory.data(caller);
    if ptr + len > memory_slice.len() {
        return None;
    }
    Some(memory_slice[ptr..ptr + len].to_vec())
}

/// Helper function to read a UTF-8 string from guest memory safely
fn read_guest_string(caller: &Caller<PluginExecutionContext>, ptr: i32, len: i32) -> Option<String> {
    let bytes = read_guest_memory(caller, ptr, len)?;
    String::from_utf8(bytes).ok()
}

/// Helper function to write bytes to guest memory safely
fn write_guest_memory(
    caller: &mut Caller<PluginExecutionContext>,
    ptr: i32,
    bytes: &[u8],
) -> Option<usize> {
    if ptr < 0 {
        return None;
    }
    let ptr = ptr as usize;
    let memory = caller.get_export("memory")?.into_memory()?;
    let memory_slice = memory.data_mut(caller);
    if ptr + bytes.len() > memory_slice.len() {
        return None;
    }
    memory_slice[ptr..ptr + bytes.len()].copy_from_slice(bytes);
    Some(bytes.len())
}

/// WebAssembly Sandboxed Plugin Engine
pub struct WasmPluginEngine {
    engine: Engine,
}

impl Default for WasmPluginEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmPluginEngine {
    pub fn new() -> Self {
        let mut config = Config::default();
        config.wasm_tail_call(true);
        let engine = Engine::new(&config);
        Self { engine }
    }

    /// Setup the Linker with safe host APIs exposed to plugins
    fn create_linker(&self) -> Result<Linker<PluginExecutionContext>, String> {
        let mut linker = Linker::new(&self.engine);

        // Host API: tabular_get_table_schema_len() -> i32
        linker
            .func_wrap(
                "env",
                "tabular_get_table_schema_len",
                |caller: Caller<PluginExecutionContext>| -> i32 {
                    caller
                        .data()
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
                |mut caller: Caller<PluginExecutionContext>, ptr: i32, max_len: i32| -> i32 {
                    if let Some(json_bytes) = caller
                        .data()
                        .cached_schema_json
                        .as_ref()
                        .map(|s| s.as_bytes().to_vec())
                    {
                        let to_write = std::cmp::min(json_bytes.len(), max_len as usize);
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
                |caller: Caller<PluginExecutionContext>| -> i32 {
                    caller
                        .data()
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
                |mut caller: Caller<PluginExecutionContext>, ptr: i32, max_len: i32| -> i32 {
                    if let Some(json_bytes) = caller
                        .data()
                        .cached_selection_json
                        .as_ref()
                        .map(|s| s.as_bytes().to_vec())
                    {
                        let to_write = std::cmp::min(json_bytes.len(), max_len as usize);
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
                |mut caller: Caller<PluginExecutionContext>,
                 format_ptr: i32,
                 format_len: i32,
                 payload_ptr: i32,
                 payload_len: i32|
                 -> i32 {
                    let format = read_guest_string(&caller, format_ptr, format_len)
                        .unwrap_or_else(|| "text".to_string());
                    let payload = read_guest_string(&caller, payload_ptr, payload_len)
                        .unwrap_or_default();

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

                    caller.data_mut().captured_exports.push(export);
                    1
                },
            )
            .map_err(|e| format!("Failed to bind tabular_export_data: {}", e))?;

        // Host API: tabular_log(level: i32, msg_ptr: i32, msg_len: i32) -> i32
        linker
            .func_wrap(
                "env",
                "tabular_log",
                |mut caller: Caller<PluginExecutionContext>,
                 level: i32,
                 msg_ptr: i32,
                 msg_len: i32|
                 -> i32 {
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

                    caller.data_mut().captured_logs.push(PluginLogEntry {
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
                |mut caller: Caller<PluginExecutionContext>, ptr: i32, len: i32| -> i32 {
                    if let Some(res) = read_guest_string(&caller, ptr, len) {
                        caller.data_mut().result_output = Some(res);
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
        let mut store = Store::new(&self.engine, context);

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| format!("Failed to instantiate and start module: {}", e))?;

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

        func.call(&mut store, ())
            .map_err(|e| format!("Plugin runtime execution error: {}", e))?;

        Ok(store.into_data())
    }
}
