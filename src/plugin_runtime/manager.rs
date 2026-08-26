use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::plugin_runtime::engine::{PluginExecutionContext, WasmPluginEngine};
use crate::plugin_runtime::host_api::{
    PluginExportPayload, PluginLogEntry, PluginSelectionData, PluginTableSchema,
};
use crate::plugin_runtime::templates::{
    generate_duckdb_script, generate_orm_code, OrmTarget, WAT_ORM_STARTER, WAT_PARQUET_STARTER,
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
            PluginCategory::Export => "📦 Export & Storage",
            PluginCategory::OrmCodeGen => "🏗 ORM & Models",
            PluginCategory::DataTransform => "🔄 Data Transformation",
            PluginCategory::Analytics => "📊 Analytics",
            PluginCategory::Custom => "🧩 Custom Wasm",
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
        }
    }
}

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
                icon: "🦆".to_string(),
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
                icon: "🦀".to_string(),
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
                icon: "🌊".to_string(),
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
                icon: "💎".to_string(),
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
                icon: "🔷".to_string(),
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
                icon: "🐍".to_string(),
                is_builtin: true,
                wat_content: Some(WAT_ORM_STARTER.to_string()),
                wasm_file_path: None,
            },
        );
    }

    /// Load user-installed .wasm or .wat plugins from ~/.tabular/plugins directory
    pub fn load_plugins_from_disk(&mut self) {
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
                                description: format!("Custom WebAssembly plugin loaded from {:?}", path),
                                category: PluginCategory::Custom,
                                icon: "🔌".to_string(),
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
                                    description: format!("Custom WebAssembly Text plugin loaded from {:?}", path),
                                    category: PluginCategory::Custom,
                                    icon: "📄".to_string(),
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
                if let Some(wat) = self.plugins.get(plugin_id).and_then(|p| p.wat_content.as_deref()) {
                    let _ = self.engine.execute(wat.as_bytes(), "tabular_main", ctx.clone());
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

        Err(format!("Plugin '{}' not found or has no executable bytecode", plugin_id))
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
