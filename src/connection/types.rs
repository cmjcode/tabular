use crate::models;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct QueryExecutionOptions {
    pub connection_id: i64,
    pub connection: models::structs::ConnectionConfig,
    pub query: String,
    pub selected_database: Option<String>,
    pub use_server_pagination: bool,
    pub current_page: usize,
    pub page_size: usize,
    pub base_query: Option<String>,
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub save_to_history: bool,
    pub ast_enabled: bool,
}

#[derive(Clone)]
pub struct QueryJob {
    pub job_id: u64,
    pub options: QueryExecutionOptions,
    pub connection_pool: models::enums::DatabasePool,
    pub started_at: Instant,
}

#[derive(Clone, Debug)]
pub struct QueryJobStatus {
    pub job_id: u64,
    pub connection_id: i64,
    pub query_preview: String,
    pub started_at: Instant,
    pub completed: bool,
}

#[derive(Debug, Clone)]
pub struct QueryResultMessage {
    pub job_id: u64,
    pub connection_id: i64,
    pub success: bool,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub error: Option<String>,
    pub duration: std::time::Duration,
    pub query: String,
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub ast_debug_sql: Option<String>,
    pub ast_headers: Option<Vec<String>>,
    pub affected_rows: Option<usize>, // Number of affected rows for INSERT/UPDATE/DELETE
    pub column_metadata: Option<Vec<models::structs::ColumnMetadata>>,
}

#[derive(Debug, Clone)]
pub struct QueryJobOutput {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub ast_debug_sql: Option<String>,
    pub ast_headers: Option<Vec<String>>,
    pub column_metadata: Option<Vec<models::structs::ColumnMetadata>>,
}

#[derive(Debug)]
pub enum QueryPreparationError {
    ConnectionNotFound,
    PoolUnavailable,
    RuntimeUnavailable,
    UnsupportedDatabase,
}

#[derive(Debug)]
pub enum QueryExecutionError {
    Message(String),
}
