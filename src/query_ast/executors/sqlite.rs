//! SQLite database executor

use async_trait::async_trait;
use log::debug;
use sqlx::{Column, Row};
use std::sync::Arc;

use crate::models::enums::DatabaseType;
use crate::query_ast::errors::QueryAstError;
use crate::query_ast::executor::{DatabaseExecutor, QueryResult, SqlFeature};

pub struct SqliteExecutor {
    // Executor is stateless - pools are managed externally
}

impl SqliteExecutor {
    pub fn new() -> Self {
        Self {}
    }

    /// Get SQLite pool from global connection pools
    fn get_pool(connection_id: i64) -> Result<Arc<sqlx::SqlitePool>, QueryAstError> {
        // This will be accessed from global state in actual implementation
        Err(QueryAstError::Execution {
            query: format!("connection_id: {}", connection_id),
            reason: "Pool lookup not yet wired to global state".to_string(),
        })
    }
}

impl Default for SqliteExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseExecutor for SqliteExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::SQLite
    }

    async fn execute_query(
        &self,
        sql: &str,
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        debug!(
            "SqliteExecutor: executing query on connection {}",
            connection_id
        );
        debug!("SQL: {}", sql);

        // Get pool from global registry
        let pool = Self::get_pool(connection_id)?;

        // SQLite supports ATTACH DATABASE for multiple databases
        if let Some(db) = database_name {
            debug!("SQLite: database context '{}' (handled via file path)", db);
            // In SQLite, database is typically the file path
            // Multiple databases can be attached with ATTACH DATABASE
            // For now, we assume the connection is already to the right file
        }

        // Execute the query
        let rows =
            sqlx::query(sql)
                .fetch_all(&*pool)
                .await
                .map_err(|e| QueryAstError::Execution {
                    query: sql.to_string(),
                    reason: e.to_string(),
                })?;

        // Extract headers
        let headers = if let Some(first_row) = rows.first() {
            first_row
                .columns()
                .iter()
                .map(|col| col.name().to_string())
                .collect()
        } else {
            Vec::new()
        };

        // Convert rows to Vec<Vec<String>>
        let data: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                let mut row_data = Vec::new();
                for i in 0..row.columns().len() {
                    let value = if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                        v.unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<i32>, _>(i) {
                        v.map(|n| n.to_string()).unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
                        v.map(|n| n.to_string()).unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
                        v.map(|n| n.to_string()).unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
                        v.map(|b| if b { "1" } else { "0" }.to_string())
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    row_data.push(value);
                }
                row_data
            })
            .collect();

        debug!(
            "SqliteExecutor: query returned {} rows, {} columns",
            data.len(),
            headers.len()
        );

        Ok((headers, data))
    }

    fn supports_feature(&self, feature: SqlFeature) -> bool {
        match feature {
            SqlFeature::WindowFunctions => true, // SQLite 3.25.0+
            SqlFeature::Cte => true,             // SQLite 3.8.3+
            SqlFeature::FullOuterJoin => false,  // Not supported natively
            SqlFeature::JsonOperators => true,   // JSON1 extension
        }
    }

    fn validate_query(&self, sql: &str) -> Result<(), QueryAstError> {
        let trimmed = sql.trim().to_uppercase();

        // Block dangerous operations
        if trimmed.starts_with("DROP ") {
            return Err(QueryAstError::Semantic(
                "DROP statements are not allowed through AST executor".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_creation() {
        let executor = SqliteExecutor::new();
        assert_eq!(executor.database_type(), DatabaseType::SQLite);
    }

    #[test]
    fn test_feature_support() {
        let executor = SqliteExecutor::new();
        assert!(executor.supports_feature(SqlFeature::WindowFunctions));
        assert!(executor.supports_feature(SqlFeature::Cte));
        assert!(!executor.supports_feature(SqlFeature::FullOuterJoin));
        assert!(executor.supports_feature(SqlFeature::JsonOperators));
    }

    #[test]
    fn test_query_validation() {
        let executor = SqliteExecutor::new();

        assert!(executor.validate_query("SELECT * FROM users").is_ok());
        assert!(executor.validate_query("DROP TABLE users").is_err());
    }
}
