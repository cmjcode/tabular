//! MS SQL Server database executor

use async_trait::async_trait;

use log::debug;

use crate::models::enums::DatabaseType;
use crate::query_ast::errors::QueryAstError;
use crate::query_ast::executor::{DatabaseExecutor, QueryResult, SqlFeature};

pub struct MssqlExecutor {
    // Executor is stateless - pools are managed externally
}

impl MssqlExecutor {
    pub fn new() -> Self {
        Self {}
    }

    // config helper removed
}

impl Default for MssqlExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseExecutor for MssqlExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::MsSQL
    }

    async fn execute_query(
        &self,
        sql: &str,
        _database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        debug!(
            "MssqlExecutor: executing query on connection {}",
            connection_id
        );
        // TODO: Wire up with deadpool from global state or pass pool into executor.
        // For now, this path is not fully active as AST execution for MsSQL is experimental.
        Err(QueryAstError::Execution {
            query: sql.to_string(),
            reason: "MssqlExecutor not yet updated to use connection pool".to_string(),
        })
    }

    fn supports_feature(&self, feature: SqlFeature) -> bool {
        match feature {
            SqlFeature::WindowFunctions => true, // SQL Server 2012+
            SqlFeature::Cte => true,             // SQL Server 2005+
            SqlFeature::FullOuterJoin => true,   // Fully supported
            SqlFeature::JsonOperators => true,   // SQL Server 2016+
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
        let executor = MssqlExecutor::new();
        assert_eq!(executor.database_type(), DatabaseType::MsSQL);
    }

    #[test]
    fn test_feature_support() {
        let executor = MssqlExecutor::new();
        assert!(executor.supports_feature(SqlFeature::WindowFunctions));
        assert!(executor.supports_feature(SqlFeature::Cte));
        assert!(executor.supports_feature(SqlFeature::FullOuterJoin));
        assert!(executor.supports_feature(SqlFeature::JsonOperators));
    }

    #[test]
    fn test_query_validation() {
        let executor = MssqlExecutor::new();

        assert!(executor.validate_query("SELECT * FROM users").is_ok());
        assert!(executor.validate_query("DROP TABLE users").is_err());
    }
}
