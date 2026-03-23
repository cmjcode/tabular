// connection/mod.rs
// Splits the original monolithic connection.rs into focused submodules.
//
// Module structure:
//   types    – shared data structures (QueryJob, QueryResultMessage, etc.)
//   sql      – SQL analysis helpers (pagination detection, simple-select check, etc.)
//   pool     – connection pool creation, caching, and lifecycle management
//   execute  – per-driver async query execution
//   metadata – schema/database/column discovery and row-cache prefetch
//   crud     – connection CRUD (update, remove, test) + background refresh
//   ui       – egui connection-selector popup

pub mod types;
pub mod sql;
pub mod pool;
pub mod execute;
pub mod metadata;
pub mod crud;
pub mod ui;

// ── Re-exports ────────────────────────────────────────────────────────────────
// Keep the same API surface that the rest of the crate expects.

// Types
pub(crate) use types::{
    QueryExecutionOptions, QueryJob, QueryJobOutput, QueryJobStatus, QueryPreparationError,
    QueryResultMessage,
};

// SQL utilities
pub(crate) use sql::{
    add_auto_limit_if_needed, query_contains_pagination, should_enable_auto_pagination,
};

// Pool management
pub(crate) use pool::{
    cleanup_connection_pool, ensure_background_pool_creation, get_or_create_connection_pool,
    get_or_create_connection_pool_with_retry, start_background_pool_creation,
    try_get_connection_pool, create_database_pool,
};

// Query execution
pub(crate) use execute::{
    execute_multiple_queries_concurrently, execute_query_with_connection,
    execute_table_query_sync, prepare_query_job, spawn_query_job,
};

// Metadata / schema discovery
pub use metadata::fetch_databases_background_task; // fully pub in original
pub(crate) use metadata::{
    fetch_columns_from_database,
    fetch_databases_from_connection_async, fetch_databases_from_connection_blocking,
    fetch_procedure_definition, fetch_table_definition, fetch_view_definition, get_foreign_keys,
};

// Connection CRUD + testing
pub(crate) use crud::{
    refresh_connection_background_async, remove_connection, test_database_connection,
    update_connection_in_database,
};

// UI
pub(crate) use ui::render_connection_selector;
