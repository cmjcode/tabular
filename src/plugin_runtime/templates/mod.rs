pub mod orm_models;
pub mod parquet_duckdb;

pub use orm_models::{OrmTarget, WAT_ORM_STARTER, generate_orm_code};
pub use parquet_duckdb::{WAT_PARQUET_STARTER, generate_duckdb_script, map_to_duckdb_type};
