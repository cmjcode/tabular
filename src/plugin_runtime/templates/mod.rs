pub mod orm_models;
pub mod parquet_duckdb;

pub use orm_models::{
    generate_orm_code, OrmTarget, WAT_ORM_STARTER,
};
pub use parquet_duckdb::{
    generate_duckdb_script, map_to_duckdb_type, WAT_PARQUET_STARTER,
};
