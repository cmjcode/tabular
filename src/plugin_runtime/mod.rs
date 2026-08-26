pub mod engine;
pub mod host_api;
pub mod manager;
pub mod templates;
pub mod ui;

pub use engine::{PluginExecutionContext, WasmPluginEngine};
pub use host_api::{
    PluginColumnSchema, PluginExportPayload, PluginLogEntry, PluginLogLevel, PluginSelectionData,
    PluginTableSchema,
};
pub use manager::{
    PluginCategory, PluginManifest, PluginManager, PluginModalState, PluginModalTab,
};
pub use templates::{
    generate_duckdb_script, generate_orm_code, OrmTarget, WAT_ORM_STARTER, WAT_PARQUET_STARTER,
};
pub use ui::{extract_plugin_table_schema, render_plugin_modal};

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_schema() -> PluginTableSchema {
        PluginTableSchema {
            table_name: "customers".to_string(),
            schema_name: Some("public".to_string()),
            database_type: "PostgreSQL".to_string(),
            columns: vec![
                PluginColumnSchema {
                    name: "id".to_string(),
                    data_type: "BIGINT".to_string(),
                    is_nullable: false,
                    is_primary_key: true,
                    is_auto_increment: true,
                    default_value: None,
                    comment: None,
                },
                PluginColumnSchema {
                    name: "full_name".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: false,
                    is_primary_key: false,
                    is_auto_increment: false,
                    default_value: None,
                    comment: None,
                },
                PluginColumnSchema {
                    name: "email".to_string(),
                    data_type: "VARCHAR(255)".to_string(),
                    is_nullable: true,
                    is_primary_key: false,
                    is_auto_increment: false,
                    default_value: None,
                    comment: None,
                },
                PluginColumnSchema {
                    name: "balance".to_string(),
                    data_type: "NUMERIC(10, 2)".to_string(),
                    is_nullable: false,
                    is_primary_key: false,
                    is_auto_increment: false,
                    default_value: Some("0.0".to_string()),
                    comment: None,
                },
                PluginColumnSchema {
                    name: "created_at".to_string(),
                    data_type: "TIMESTAMP".to_string(),
                    is_nullable: false,
                    is_primary_key: false,
                    is_auto_increment: false,
                    default_value: Some("NOW()".to_string()),
                    comment: None,
                },
            ],
            total_rows: 250,
        }
    }

    #[test]
    fn test_duckdb_parquet_script_generation() {
        let schema = create_test_schema();
        let selection = PluginSelectionData {
            table_name: "customers".to_string(),
            headers: vec!["id".to_string(), "full_name".to_string(), "email".to_string()],
            rows: vec![
                vec!["1".to_string(), "Alice".to_string(), "alice@example.com".to_string()],
                vec!["2".to_string(), "Bob".to_string(), "null".to_string()],
            ],
            total_selected: 2,
        };

        let script = generate_duckdb_script(&schema, Some(&selection), Some("custom_customers.parquet"));
        assert!(script.contains("CREATE OR REPLACE TABLE \"customers\""));
        assert!(script.contains("\"id\" BIGINT NOT NULL PRIMARY KEY"));
        assert!(script.contains("\"full_name\" VARCHAR NOT NULL"));
        assert!(script.contains("COPY \"customers\" TO 'custom_customers.parquet' (FORMAT PARQUET, COMPRESSION 'SNAPPY'"));
        assert!(script.contains("INSERT INTO \"customers\" (\"id\", \"full_name\", \"email\")"));
        assert!(script.contains("read_parquet('custom_customers.parquet')"));
    }

    #[test]
    fn test_diesel_orm_generation() {
        let schema = create_test_schema();
        let code = generate_orm_code(&schema, OrmTarget::RustDiesel);
        assert!(code.contains("diesel::table!"));
        assert!(code.contains("pub struct Customers"));
        assert!(code.contains("pub id: i64"));
        assert!(code.contains("pub email: Option<String>"));
        assert!(code.contains("pub struct NewCustomers"));
    }

    #[test]
    fn test_seaorm_generation() {
        let schema = create_test_schema();
        let code = generate_orm_code(&schema, OrmTarget::RustSeaOrm);
        assert!(code.contains("#[sea_orm(table_name = \"customers\")]"));
        assert!(code.contains("#[sea_orm(primary_key)]"));
        assert!(code.contains("pub struct Model"));
        assert!(code.contains("pub email: Option<String>"));
        assert!(code.contains("impl ActiveModelBehavior for ActiveModel"));
    }

    #[test]
    fn test_prisma_generation() {
        let schema = create_test_schema();
        let code = generate_orm_code(&schema, OrmTarget::TypeScriptPrisma);
        assert!(code.contains("model Customers {"));
        assert!(code.contains("id               BigInt @id @default(autoincrement())"));
        assert!(code.contains("email            String?"));
        assert!(code.contains("balance          Decimal"));
        assert!(code.contains("created_at       DateTime"));
    }

    #[test]
    fn test_typeorm_generation() {
        let schema = create_test_schema();
        let code = generate_orm_code(&schema, OrmTarget::TypeScriptTypeOrm);
        assert!(code.contains("@Entity(\"customers\")"));
        assert!(code.contains("export class Customers {"));
        assert!(code.contains("@PrimaryGeneratedColumn()"));
        assert!(code.contains("id: number;"));
        assert!(code.contains("@Column({ nullable: true, name: \"email\" })"));
        assert!(code.contains("email?: string;"));
    }

    #[test]
    fn test_sqlalchemy2_generation() {
        let schema = create_test_schema();
        let code = generate_orm_code(&schema, OrmTarget::PythonSqlAlchemy2);
        assert!(code.contains("class Customers(Base):"));
        assert!(code.contains("__tablename__ = \"customers\""));
        assert!(code.contains("id: Mapped[int] = mapped_column(primary_key=True, autoincrement=True)"));
        assert!(code.contains("email: Mapped[Optional[str]] = mapped_column()"));
        assert!(code.contains("created_at: Mapped[datetime] = mapped_column()"));
    }

    #[test]
    fn test_sqlalchemy1_generation() {
        let schema = create_test_schema();
        let code = generate_orm_code(&schema, OrmTarget::PythonSqlAlchemy1);
        assert!(code.contains("class Customers(Base):"));
        assert!(code.contains("__tablename__ = \"customers\""));
        assert!(code.contains("id = Column(BigInteger, primary_key=True, nullable=False)"));
        assert!(code.contains("email = Column(String(255))"));
    }

    #[test]
    fn test_wasm_engine_execution_and_host_api() {
        let engine = WasmPluginEngine::new();
        let schema = create_test_schema();
        let selection = PluginSelectionData {
            table_name: "customers".to_string(),
            headers: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec!["1".to_string(), "Alice".to_string()]],
            total_selected: 1,
        };

        let ctx = PluginExecutionContext::new(Some(schema), Some(selection));

        // Execute starter parquet WAT
        let result = engine.execute(WAT_PARQUET_STARTER.as_bytes(), "tabular_main", ctx);
        assert!(result.is_ok(), "Execution error: {:?}", result.err());

        let finished_ctx = result.unwrap();
        assert_eq!(finished_ctx.captured_logs.len(), 1);
        assert_eq!(finished_ctx.captured_logs[0].level, PluginLogLevel::Info);
        assert!(finished_ctx.captured_logs[0].message.contains("Analyzing table schema"));

        assert_eq!(finished_ctx.captured_exports.len(), 1);
        assert_eq!(finished_ctx.captured_exports[0].format, "duckdb");
        assert!(finished_ctx.captured_exports[0].text_content.as_ref().unwrap().contains("COPY (SELECT * FROM current_table) TO 'export.parquet'"));
    }

    #[test]
    fn test_plugin_manager_lifecycle() {
        let manager = PluginManager::new();
        let plugins = manager.get_plugins();
        assert!(plugins.len() >= 6, "Expected at least 6 builtin plugins");

        let schema = create_test_schema();
        let res = manager.execute_plugin(
            "builtin_orm_diesel",
            &schema,
            None,
            None,
            None,
        );

        assert!(res.is_ok());
        let ctx = res.unwrap();
        assert!(ctx.result_output.is_some());
        assert!(ctx.result_output.unwrap().contains("diesel::table!"));
    }
}
