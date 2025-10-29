//! MySQL database executor

use async_trait::async_trait;
use log::debug;
use sqlx::{Column, Row};
use std::sync::Arc;

use crate::models::enums::DatabaseType;
use crate::query_ast::errors::QueryAstError;
use crate::query_ast::executor::{DatabaseExecutor, QueryResult, SqlFeature};

pub struct MySqlExecutor {
    // Executor is stateless - pools are managed externally
}

impl MySqlExecutor {
    pub fn new() -> Self {
        Self {}
    }

    /// Get MySQL pool from global connection pools
    fn get_pool(connection_id: i64) -> Result<Arc<sqlx::MySqlPool>, QueryAstError> {
        // This will be accessed from global state in actual implementation
        // For now, return error - will be wired in integration
        Err(QueryAstError::Execution {
            query: format!("connection_id: {}", connection_id),
            reason: "Pool lookup not yet wired to global state".to_string(),
        })
    }
}

impl Default for MySqlExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseExecutor for MySqlExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::MySQL
    }

    async fn execute_query(
        &self,
        sql: &str,
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        debug!(
            "MySqlExecutor: executing query on connection {}",
            connection_id
        );
        debug!("SQL: {}", sql);

        // Get pool from global registry (will be wired later)
        let pool = Self::get_pool(connection_id)?;

        // Switch database if specified
        if let Some(db) = database_name {
            let use_sql = format!("USE `{}`", db.replace('`', "``"));
            sqlx::query(&use_sql)
                .execute(&*pool)
                .await
                .map_err(|e| QueryAstError::Execution {
                    query: use_sql,
                    reason: e.to_string(),
                })?;
        }

        // Execute the main query
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
            // No rows - try to infer from query
            Vec::new()
        };

        // Convert rows to Vec<Vec<String>>
        let data: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                let mut row_data = Vec::new();
                for i in 0..row.columns().len() {
                    // Try to get value as different types, fallback to empty string
                    let value = if let Ok(v) = row.try_get::<Option<String>, _>(i) {
                        v.unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
                        v.map(|n| n.to_string()).unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
                        v.map(|n| n.to_string()).unwrap_or_default()
                    } else if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
                        v.map(|b| if b { "1" } else { "0" }.to_string())
                            .unwrap_or_default()
                    } else {
                        // Fallback for other types
                        String::new()
                    };
                    row_data.push(value);
                }
                row_data
            })
            .collect();

        debug!(
            "MySqlExecutor: query returned {} rows, {} columns",
            data.len(),
            headers.len()
        );

        Ok((headers, data))
    }

    fn supports_feature(&self, feature: SqlFeature) -> bool {
        match feature {
            SqlFeature::WindowFunctions => true, // MySQL 8.0+
            SqlFeature::Cte => true,             // MySQL 8.0+
            SqlFeature::FullOuterJoin => false,  // Not supported natively
            SqlFeature::JsonOperators => true,   // MySQL 5.7+
        }
    }

    fn validate_query(&self, sql: &str) -> Result<(), QueryAstError> {
        // Basic validation
        let trimmed = sql.trim().to_uppercase();

        // Check for dangerous operations (DROP, DELETE without WHERE, etc.)
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
        let executor = MySqlExecutor::new();
        assert_eq!(executor.database_type(), DatabaseType::MySQL);
    }

    #[test]
    fn test_feature_support() {
        let executor = MySqlExecutor::new();
        assert!(executor.supports_feature(SqlFeature::WindowFunctions));
        assert!(executor.supports_feature(SqlFeature::Cte));
        assert!(!executor.supports_feature(SqlFeature::FullOuterJoin));
        assert!(executor.supports_feature(SqlFeature::JsonOperators));
    }

    #[test]
    fn test_query_validation() {
        let executor = MySqlExecutor::new();

        // Should pass
        assert!(executor.validate_query("SELECT * FROM users").is_ok());

        // Should fail
        assert!(executor.validate_query("DROP TABLE users").is_err());
    }
}
