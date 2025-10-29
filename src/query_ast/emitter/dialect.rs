//! Database dialect trait for extensible SQL emission
//!
//! Each database dialect implements this trait to provide
//! database-specific SQL generation logic.

use super::super::errors::QueryAstError;
use crate::models::enums::DatabaseType;

/// Trait for database-specific SQL dialect
pub trait SqlDialect: Send + Sync {
    /// Get the database type
    fn db_type(&self) -> DatabaseType;

    /// Quote an identifier (table/column name)
    fn quote_ident(&self, ident: &str) -> String;

    /// Quote a string literal
    fn quote_string(&self, s: &str) -> String {
        format!("'{}'", s.replace("'", "''"))
    }

    /// Emit boolean literal
    fn emit_boolean(&self, value: bool) -> String {
        if value { "TRUE" } else { "FALSE" }.to_string()
    }

    /// Emit NULL literal
    fn emit_null(&self) -> String {
        "NULL".to_string()
    }

    /// Emit LIMIT clause
    fn emit_limit(&self, limit: u64, offset: u64) -> String {
        if offset > 0 {
            format!(" LIMIT {} OFFSET {}", limit, offset)
        } else {
            format!(" LIMIT {}", limit)
        }
    }

    /// Emit DISTINCT keyword
    fn emit_distinct(&self) -> &'static str {
        "DISTINCT"
    }

    /// Emit JOIN keyword for given kind
    fn emit_join_kind(&self, kind: &super::super::logical::JoinKind) -> &'static str {
        use super::super::logical::JoinKind;
        match kind {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::Right => "RIGHT JOIN",
            JoinKind::Full => "FULL JOIN",
        }
    }

    /// Check if this dialect supports a feature
    fn supports_window_functions(&self) -> bool {
        !matches!(self.db_type(), DatabaseType::SQLite | DatabaseType::Redis)
    }

    fn supports_cte(&self) -> bool {
        !matches!(self.db_type(), DatabaseType::Redis)
    }

    fn supports_full_join(&self) -> bool {
        !matches!(self.db_type(), DatabaseType::MySQL | DatabaseType::SQLite)
    }

    /// Emit a cast expression (database-specific syntax)
    fn emit_cast(&self, expr: &str, target_type: &str) -> String {
        format!("CAST({} AS {})", expr, target_type)
    }

    /// Emit ILIKE (case-insensitive LIKE) - fallback to LIKE LOWER() if not supported
    fn emit_ilike(&self, expr: &str, pattern: &str, negated: bool) -> String {
        if matches!(self.db_type(), DatabaseType::PostgreSQL) {
            if negated {
                format!("{} NOT ILIKE {}", expr, pattern)
            } else {
                format!("{} ILIKE {}", expr, pattern)
            }
        } else {
            // Fallback: LOWER(expr) LIKE LOWER(pattern)
            let op = if negated { "NOT LIKE" } else { "LIKE" };
            format!("LOWER({}) {} LOWER({})", expr, op, pattern)
        }
    }

    /// Emit regex match expression (very database-specific)
    fn emit_regex_match(&self, expr: &str, pattern: &str) -> Result<String, QueryAstError> {
        match self.db_type() {
            DatabaseType::PostgreSQL => Ok(format!("{} ~ {}", expr, pattern)),
            DatabaseType::MySQL => Ok(format!("{} REGEXP {}", expr, pattern)),
            _ => Err(QueryAstError::Unsupported(
                "regex not supported by this database",
            )),
        }
    }

    /// Emit array literal (if supported)
    fn emit_array(&self, elements: &[String]) -> Result<String, QueryAstError> {
        match self.db_type() {
            DatabaseType::PostgreSQL => Ok(format!("ARRAY[{}]", elements.join(", "))),
            _ => Err(QueryAstError::Unsupported(
                "arrays not supported by this database",
            )),
        }
    }
}

/// MySQL dialect
pub struct MySqlDialect;

impl SqlDialect for MySqlDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::MySQL
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }
}

/// PostgreSQL dialect
pub struct PostgresDialect;

impl SqlDialect for PostgresDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::PostgreSQL
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

/// SQLite dialect
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::SQLite
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }

    fn supports_window_functions(&self) -> bool {
        true // SQLite 3.25.0+ supports window functions
    }
}

/// MS SQL Server dialect
pub struct MssqlDialect;

impl SqlDialect for MssqlDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::MsSQL
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("[{}]", ident.replace(']', "]]"))
    }

    fn emit_limit(&self, limit: u64, offset: u64) -> String {
        if offset > 0 {
            format!(" OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", offset, limit)
        } else {
            // For SELECT TOP, this is handled differently in the emitter
            // Return empty here and let emitter inject TOP after SELECT
            String::new()
        }
    }

    fn emit_boolean(&self, value: bool) -> String {
        if value { "1" } else { "0" }.to_string()
    }
}

/// MongoDB dialect (for SQL-like queries over MongoDB collections)
pub struct MongoDialect;

impl SqlDialect for MongoDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::MongoDB
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn supports_window_functions(&self) -> bool {
        false
    }

    fn supports_full_join(&self) -> bool {
        false
    }
}

/// Redis dialect (limited SQL support)
pub struct RedisDialect;

impl SqlDialect for RedisDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Redis
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn supports_window_functions(&self) -> bool {
        false
    }

    fn supports_cte(&self) -> bool {
        false
    }

    fn supports_full_join(&self) -> bool {
        false
    }
}

/// Get dialect for a database type
pub fn get_dialect(db_type: &DatabaseType) -> Box<dyn SqlDialect> {
    match db_type {
        DatabaseType::MySQL => Box::new(MySqlDialect),
        DatabaseType::PostgreSQL => Box::new(PostgresDialect),
        DatabaseType::SQLite => Box::new(SqliteDialect),
        DatabaseType::MsSQL => Box::new(MssqlDialect),
        DatabaseType::MongoDB => Box::new(MongoDialect),
        DatabaseType::Redis => Box::new(RedisDialect),
    }
}
