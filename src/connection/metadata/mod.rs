// connection/metadata/mod.rs
// Splits metadata.rs into focused sub-modules:
//   cache       – fetch_and_cache_all_data, row-cache helpers, prefetch
//   databases   – fetch_databases_* (blocking, async, background)
//   columns     – fetch_columns_from_database
//   ddl         – fetch_view_definition, fetch_procedure_definition,
//                 get_foreign_keys, fetch_table_definition

mod cache;
mod databases;
mod columns;
mod ddl;

// Re-export everything that the parent connection module (and the rest of the
// crate) expects to find at the `metadata::*` path.

pub(crate) use cache::fetch_and_cache_all_data;

pub use databases::fetch_databases_background_task; // fully pub in original
pub(crate) use databases::{
    fetch_databases_from_connection_async, fetch_databases_from_connection_blocking,
};

pub(crate) use columns::fetch_columns_from_database;

pub(crate) use ddl::{
    fetch_procedure_definition, fetch_table_definition, fetch_view_definition, get_foreign_keys,
};
