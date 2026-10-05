use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::config;
use crate::plugin_runtime::engine::{PluginExecutionContext, WasmPluginEngine};
use crate::plugin_runtime::host_api::{
    PluginExportPayload, PluginLogEntry, PluginSelectionData, PluginTableSchema,
};
use crate::plugin_runtime::templates::{
    OrmTarget, WAT_ORM_STARTER, WAT_PARQUET_STARTER, generate_duckdb_script, generate_orm_code,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginCategory {
    Export,
    OrmCodeGen,
    DataTransform,
    Analytics,
    Custom,
}

impl PluginCategory {
    pub fn display_name(&self) -> &'static str {
        match self {
            PluginCategory::Export => "Export & Storage",
            PluginCategory::OrmCodeGen => "ORM & Models",
            PluginCategory::DataTransform => "Data Transformation",
            PluginCategory::Analytics => "Analytics",
            PluginCategory::Custom => "Custom Wasm",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub category: PluginCategory,
    pub icon: String,
    pub is_builtin: bool,
    pub wat_content: Option<String>,
    pub wasm_file_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginModalTab {
    PluginsCatalog,
    StarterTemplates,
    CustomWasmRunner,
    ExecutionOutput,
    DatabaseDrivers,
}

#[derive(Debug, Clone)]
pub struct PluginModalState {
    pub is_open: bool,
    pub active_tab: PluginModalTab,
    pub selected_plugin_id: String,
    pub selected_orm_target: OrmTarget,
    pub parquet_output_path: String,
    pub custom_wat_code: String,
    pub custom_wasm_file: Option<PathBuf>,
    pub search_query: String,
    pub filter_category: Option<PluginCategory>,
    pub is_running: bool,
    pub execution_output: Option<String>,
    pub execution_logs: Vec<PluginLogEntry>,
    pub execution_exports: Vec<PluginExportPayload>,
    pub error_message: Option<String>,
    pub status_message: Option<String>,
    /// Daftar driver engine terpasang; `None` = perlu dipindai ulang.
    pub installed_drivers: Option<Vec<crate::driver_api::manifest::InstalledDriver>>,
    /// Sidecar yang sedang menunggu konfirmasi persetujuan pengguna.
    pub pending_sidecar_approval: Option<String>,
    /// Eksekusi plugin yang sedang berjalan di thread kerja.
    pub pending_run: Option<PendingPluginRun>,
}

/// Hasil satu eksekusi plugin.
pub type PluginRunResult = Result<PluginExecutionContext, String>;

/// Eksekusi plugin di thread kerja. Kode Wasm pengguna tidak pernah jalan di
/// thread UI: modul yang lambat atau berputar tanpa henti hanya menahan
/// thread ini sampai anggaran fuel-nya habis, sementara jendela tetap hidup.
#[derive(Debug, Clone)]
pub struct PendingPluginRun {
    /// Nama yang ditampilkan di indikator sibuk.
    pub label: String,
    /// Pesan status bila eksekusi berhasil.
    pub success_message: String,
    pub started_at: std::time::Instant,
    slot: std::sync::Arc<std::sync::Mutex<Option<PluginRunResult>>>,
}

impl PendingPluginRun {
    /// Jalankan `run` di thread kerja. `on_done` dipanggil setelah hasil
    /// tersimpan (dipakai untuk meminta repaint).
    pub fn spawn(
        label: String,
        success_message: String,
        run: impl FnOnce() -> PluginRunResult + Send + 'static,
        on_done: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let worker_slot = slot.clone();
        std::thread::Builder::new()
            .name("tabular-plugin".to_string())
            .spawn(move || {
                // Panik di dalam plugin host tidak boleh membuat UI menunggu
                // selamanya: laporkan sebagai galat.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                    .unwrap_or_else(|_| Err("Plugin execution panicked".to_string()));
                *worker_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
                on_done();
            })
            .map_err(|e| format!("Could not start the plugin thread: {e}"))?;
        Ok(Self {
            label,
            success_message,
            started_at: std::time::Instant::now(),
            slot,
        })
    }

    /// Ambil hasil bila sudah selesai.
    pub fn take_result(&self) -> Option<PluginRunResult> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl Default for PluginModalState {
    fn default() -> Self {
        Self {
            is_open: false,
            active_tab: PluginModalTab::PluginsCatalog,
            selected_plugin_id: "builtin_parquet_duckdb".to_string(),
            selected_orm_target: OrmTarget::RustDiesel,
            parquet_output_path: "export.parquet".to_string(),
            custom_wat_code: WAT_PARQUET_STARTER.to_string(),
            custom_wasm_file: None,
            search_query: String::new(),
            filter_category: None,
            is_running: false,
            execution_output: None,
            execution_logs: Vec::new(),
            execution_exports: Vec::new(),
            error_message: None,
            status_message: None,
            installed_drivers: None,
            pending_sidecar_approval: None,
            pending_run: None,
        }
    }
}

/// Murah di-clone (engine berbagi state, manifest kecil), supaya salinannya
/// bisa dibawa ke thread kerja; lihat [`PendingPluginRun`].
#[derive(Clone)]
pub struct PluginManager {
    engine: WasmPluginEngine,
    plugins: HashMap<String, PluginManifest>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    pub fn new() -> Self {
        let engine = WasmPluginEngine::new();
        let mut manager = Self {
            engine,
            plugins: HashMap::new(),
        };
        manager.register_builtin_plugins();
        manager.load_plugins_from_disk();
        manager
    }

    /// Register default starter and powerhouse plugins
    fn register_builtin_plugins(&mut self) {
        // 1. Apache Parquet & DuckDB Plugin
        self.plugins.insert(
            "builtin_parquet_duckdb".to_string(),
            PluginManifest {
                id: "builtin_parquet_duckdb".to_string(),
                name: "Apache Parquet & DuckDB Pipeline".to_string(),
                version: "1.0.0".to_string(),
                author: "Tabular Core".to_string(),
                description: "Generates high-performance Snappy-compressed Parquet export scripts and in-memory DuckDB schemas for analytical workloads.".to_string(),
                category: PluginCategory::Export,
                icon: egui_icons::icons::MDI_DATABASE_EXPORT.codepoint.to_string(),
                is_builtin: true,
                wat_content: Some(WAT_PARQUET_STARTER.to_string()),
                wasm_file_path: None,
            },
        );

        // 2. Rust Diesel ORM Generator
        self.plugins.insert(
            "builtin_orm_diesel".to_string(),
            PluginManifest {
                id: "builtin_orm_diesel".to_string(),
                name: "Rust Diesel Models Generator".to_string(),
                version: "1.0.0".to_string(),
                author: "Tabular Core".to_string(),
                description: "Generates Diesel table! schemas, Queryable, Selectable, and Insertable model structs with complete type mappings.".to_string(),
                category: PluginCategory::OrmCodeGen,
                icon: egui_icons::icons::MDI_LANGUAGE_RUST.codepoint.to_string(),
                is_builtin: true,
                wat_content: Some(WAT_ORM_STARTER.to_string()),
                wasm_file_path: None,
            },
        );

        // 3. Rust SeaORM Entity Generator
        self.plugins.insert(
            "builtin_orm_seaorm".to_string(),
            PluginManifest {
                id: "builtin_orm_seaorm".to_string(),
                name: "Rust SeaORM Entity Generator".to_string(),
                version: "1.0.0".to_string(),
                author: "Tabular Core".to_string(),
                description: "Generates async SeaORM Entity Models with Relations, PrimaryKeys, and ActiveModelBehavior.".to_string(),
                category: PluginCategory::OrmCodeGen,
                icon: egui_icons::icons::MDI_LANGUAGE_RUST.codepoint.to_string(),
                is_builtin: true,
                wat_content: Some(WAT_ORM_STARTER.to_string()),
                wasm_file_path: None,
            },
        );

        // 4. TypeScript Prisma Model Generator
        self.plugins.insert(
            "builtin_orm_prisma".to_string(),
            PluginManifest {
                id: "builtin_orm_prisma".to_string(),
                name: "TypeScript Prisma Schema Generator".to_string(),
                version: "1.0.0".to_string(),
                author: "Tabular Core".to_string(),
                description: "Generates Prisma Schema model definitions with @id, autoincrement, default values, and column maps.".to_string(),
                category: PluginCategory::OrmCodeGen,
                icon: egui_icons::icons::MDI_LANGUAGE_TYPESCRIPT.codepoint.to_string(),
                is_builtin: true,
                wat_content: Some(WAT_ORM_STARTER.to_string()),
                wasm_file_path: None,
            },
        );

        // 5. TypeScript TypeORM Entity Generator
        self.plugins.insert(
            "builtin_orm_typeorm".to_string(),
            PluginManifest {
                id: "builtin_orm_typeorm".to_string(),
                name: "TypeScript TypeORM Entity Generator".to_string(),
                version: "1.0.0".to_string(),
                author: "Tabular Core".to_string(),
                description: "Generates TypeORM @Entity() classes with @PrimaryGeneratedColumn, @Column, and TypeScript interfaces.".to_string(),
                category: PluginCategory::OrmCodeGen,
                icon: egui_icons::icons::MDI_LANGUAGE_TYPESCRIPT.codepoint.to_string(),
                is_builtin: true,
                wat_content: Some(WAT_ORM_STARTER.to_string()),
                wasm_file_path: None,
            },
        );

        // 6. Python SQLAlchemy 2.0 Model Generator
        self.plugins.insert(
            "builtin_orm_sqlalchemy2".to_string(),
            PluginManifest {
                id: "builtin_orm_sqlalchemy2".to_string(),
                name: "Python SQLAlchemy 2.0 Model Generator".to_string(),
                version: "1.0.0".to_string(),
                author: "Tabular Core".to_string(),
                description: "Generates modern Python 3.10+ SQLAlchemy 2.0 type-annotated Mapped[T] and mapped_column definitions.".to_string(),
                category: PluginCategory::OrmCodeGen,
                icon: egui_icons::icons::MDI_LANGUAGE_PYTHON.codepoint.to_string(),
                is_builtin: true,
                wat_content: Some(WAT_ORM_STARTER.to_string()),
                wasm_file_path: None,
            },
        );
    }

    /// Load user-installed .wasm or .wat plugins from ~/.tabular/plugins directory
    ///
    /// Disabled on iOS. `UIFileSharingEnabled` lets anyone drop files into the
    /// app's Documents folder from the Files app — which is how a user brings
    /// their own `.db` in, so it stays on — but that same door would let a
    /// `.wasm` module in, and executing downloaded code is exactly what App
    /// Store Review Guideline 2.5.2 prohibits. Built-in plugins are compiled
    /// into the binary and keep working.
    pub fn load_plugins_from_disk(&mut self) {
        if cfg!(target_os = "ios") {
            return;
        }

        let plugins_dir = config::get_data_dir().join("plugins");
        if !plugins_dir.exists() {
            let _ = std::fs::create_dir_all(&plugins_dir);
            return;
        }

        if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let file_stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("custom_plugin")
                        .to_string();

                    if ext.eq_ignore_ascii_case("wasm") {
                        self.plugins.insert(
                            format!("custom_{}", file_stem),
                            PluginManifest {
                                id: format!("custom_{}", file_stem),
                                name: format!("User Plugin: {}", file_stem),
                                version: "1.0.0".to_string(),
                                author: "Local User".to_string(),
                                description: format!(
                                    "Custom WebAssembly plugin loaded from {:?}",
                                    path
                                ),
                                category: PluginCategory::Custom,
                                icon: egui_icons::icons::MDI_FILE_CODE.codepoint.to_string(),
                                is_builtin: false,
                                wat_content: None,
                                wasm_file_path: Some(path),
                            },
                        );
                    } else if ext.eq_ignore_ascii_case("wat") {
                        if let Ok(wat_content) = std::fs::read_to_string(&path) {
                            self.plugins.insert(
                                format!("custom_{}", file_stem),
                                PluginManifest {
                                    id: format!("custom_{}", file_stem),
                                    name: format!("WAT Plugin: {}", file_stem),
                                    version: "1.0.0".to_string(),
                                    author: "Local User".to_string(),
                                    description: format!(
                                        "Custom WebAssembly Text plugin loaded from {:?}",
                                        path
                                    ),
                                    category: PluginCategory::Custom,
                                    icon: egui_icons::icons::ICON_DESCRIPTION.codepoint.to_string(),
                                    is_builtin: false,
                                    wat_content: Some(wat_content),
                                    wasm_file_path: None,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    pub fn get_plugins(&self) -> Vec<&PluginManifest> {
        let mut list: Vec<&PluginManifest> = self.plugins.values().collect();
        list.sort_by(|a, b| {
            b.is_builtin
                .cmp(&a.is_builtin)
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    pub fn get_plugin(&self, id: &str) -> Option<&PluginManifest> {
        self.plugins.get(id)
    }

    /// Execute a plugin or starter template against given table schema & selection
    pub fn execute_plugin(
        &self,
        plugin_id: &str,
        schema: &PluginTableSchema,
        selection: Option<&PluginSelectionData>,
        orm_target: Option<OrmTarget>,
        parquet_output_path: Option<&str>,
    ) -> Result<PluginExecutionContext, String> {
        let mut ctx = PluginExecutionContext::new(Some(schema.clone()), selection.cloned());

        // Fast-path template generators with Wasm runtime verification
        match plugin_id {
            "builtin_parquet_duckdb" => {
                // Generate script
                let script = generate_duckdb_script(schema, selection, parquet_output_path);
                // Also run through Wasm sandboxed engine to verify host APIs
                if let Some(wat) = self
                    .plugins
                    .get(plugin_id)
                    .and_then(|p| p.wat_content.as_deref())
                {
                    let _ = self
                        .engine
                        .execute(wat.as_bytes(), "tabular_main", ctx.clone());
                }

                ctx.result_output = Some(script.clone());
                ctx.captured_exports.push(PluginExportPayload {
                    format: "duckdb".to_string(),
                    filename_suggestion: format!("{}_duckdb_export.sql", schema.table_name),
                    content_type: "application/sql".to_string(),
                    text_content: Some(script),
                    binary_base64: None,
                    metadata: None,
                });
                return Ok(ctx);
            }
            "builtin_orm_diesel" => {
                let code = generate_orm_code(schema, OrmTarget::RustDiesel);
                ctx.result_output = Some(code.clone());
                ctx.captured_exports.push(PluginExportPayload {
                    format: "rust".to_string(),
                    filename_suggestion: format!("{}_diesel.rs", schema.table_name),
                    content_type: "text/rust".to_string(),
                    text_content: Some(code),
                    binary_base64: None,
                    metadata: None,
                });
                return Ok(ctx);
            }
            "builtin_orm_seaorm" => {
                let code = generate_orm_code(schema, OrmTarget::RustSeaOrm);
                ctx.result_output = Some(code.clone());
                ctx.captured_exports.push(PluginExportPayload {
                    format: "rust".to_string(),
                    filename_suggestion: format!("{}_seaorm.rs", schema.table_name),
                    content_type: "text/rust".to_string(),
                    text_content: Some(code),
                    binary_base64: None,
                    metadata: None,
                });
                return Ok(ctx);
            }
            "builtin_orm_prisma" => {
                let code = generate_orm_code(schema, OrmTarget::TypeScriptPrisma);
                ctx.result_output = Some(code.clone());
                ctx.captured_exports.push(PluginExportPayload {
                    format: "prisma".to_string(),
                    filename_suggestion: format!("{}.prisma", schema.table_name),
                    content_type: "text/plain".to_string(),
                    text_content: Some(code),
                    binary_base64: None,
                    metadata: None,
                });
                return Ok(ctx);
            }
            "builtin_orm_typeorm" => {
                let code = generate_orm_code(schema, OrmTarget::TypeScriptTypeOrm);
                ctx.result_output = Some(code.clone());
                ctx.captured_exports.push(PluginExportPayload {
                    format: "typescript".to_string(),
                    filename_suggestion: format!("{}.entity.ts", schema.table_name),
                    content_type: "application/typescript".to_string(),
                    text_content: Some(code),
                    binary_base64: None,
                    metadata: None,
                });
                return Ok(ctx);
            }
            "builtin_orm_sqlalchemy2" => {
                let code = generate_orm_code(schema, OrmTarget::PythonSqlAlchemy2);
                ctx.result_output = Some(code.clone());
                ctx.captured_exports.push(PluginExportPayload {
                    format: "python".to_string(),
                    filename_suggestion: format!("{}_models.py", schema.table_name),
                    content_type: "text/x-python".to_string(),
                    text_content: Some(code),
                    binary_base64: None,
                    metadata: None,
                });
                return Ok(ctx);
            }
            _ => {}
        }

        // Custom plugin execution
        if let Some(plugin) = self.plugins.get(plugin_id) {
            if let Some(ref wat) = plugin.wat_content {
                return self.engine.execute(wat.as_bytes(), "tabular_main", ctx);
            } else if let Some(ref path) = plugin.wasm_file_path {
                let bytes = std::fs::read(path)
                    .map_err(|e| format!("Failed to read WASM plugin file: {}", e))?;
                return self.engine.execute(&bytes, "tabular_main", ctx);
            }
        }

        // Target-based fallback if passed directly
        if let Some(target) = orm_target {
            let code = generate_orm_code(schema, target);
            ctx.result_output = Some(code.clone());
            ctx.captured_exports.push(PluginExportPayload {
                format: target.language().to_string(),
                filename_suggestion: format!("{}.{}", schema.table_name, target.language()),
                content_type: "text/plain".to_string(),
                text_content: Some(code),
                binary_base64: None,
                metadata: None,
            });
            return Ok(ctx);
        }

        Err(format!(
            "Plugin '{}' not found or has no executable bytecode",
            plugin_id
        ))
    }

    /// Execute raw WAT or WASM bytecode supplied by user
    pub fn execute_raw(
        &self,
        bytecode: &[u8],
        entrypoint: &str,
        schema: Option<&PluginTableSchema>,
        selection: Option<&PluginSelectionData>,
    ) -> Result<PluginExecutionContext, String> {
        let ctx = PluginExecutionContext::new(schema.cloned(), selection.cloned());
        self.engine.execute(bytecode, entrypoint, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::engine::PluginLimits;

    fn wait(run: &PendingPluginRun) -> PluginRunResult {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(result) = run.take_result() {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "plugin thread did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn runaway_plugin_finishes_on_the_worker_thread_with_an_error() {
        let engine = WasmPluginEngine::with_limits(PluginLimits {
            fuel: 500_000,
            ..Default::default()
        });
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        let run = PendingPluginRun::spawn(
            "loop".to_string(),
            "ok".to_string(),
            move || {
                assert_ne!(std::thread::current().id(), caller);
                engine.execute(
                    b"(module (func (export \"tabular_main\") (loop $l (br $l))))",
                    "tabular_main",
                    PluginExecutionContext::default(),
                )
            },
            move || {
                let _ = done_tx.send(());
            },
        )
        .unwrap();
        // Pemanggil tidak ikut menunggu: hasil datang lewat slot.
        let err = wait(&run).unwrap_err();
        assert!(err.contains("out of fuel"), "{err}");
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok()
        );
        assert!(run.take_result().is_none());
    }

    #[test]
    fn panicking_run_is_reported_instead_of_hanging() {
        let run =
            PendingPluginRun::spawn("boom".to_string(), String::new(), || panic!("boom"), || {})
                .unwrap();
        assert_eq!(wait(&run).unwrap_err(), "Plugin execution panicked");
    }

    #[test]
    fn manager_clone_runs_builtin_plugins() {
        let manager = PluginManager::new();
        let schema = PluginTableSchema {
            table_name: "t".to_string(),
            schema_name: None,
            database_type: "SQLite".to_string(),
            columns: Vec::new(),
            total_rows: 0,
        };
        let worker = manager.clone();
        let run = PendingPluginRun::spawn(
            "duckdb".to_string(),
            String::new(),
            move || worker.execute_plugin("builtin_parquet_duckdb", &schema, None, None, None),
            || {},
        )
        .unwrap();
        assert!(wait(&run).unwrap().result_output.is_some());
    }
}
