//! MS SQL Server database executor

use async_trait::async_trait;
use futures_util::TryStreamExt;
use log::debug;
use std::sync::Arc;

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

    /// Get MS SQL config wrapper from global connection pools
    fn get_config(
        connection_id: i64,
    ) -> Result<Arc<crate::driver_mssql::MssqlConfigWrapper>, QueryAstError> {
        // This will be accessed from global state in actual implementation
        Err(QueryAstError::Execution {
            query: format!("connection_id: {}", connection_id),
            reason: "Pool lookup not yet wired to global state".to_string(),
        })
    }
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
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        debug!(
            "MssqlExecutor: executing query on connection {}",
            connection_id
        );
        debug!("SQL: {}", sql);

        // Get config from global registry
        let config_wrapper = Self::get_config(connection_id)?;

        // Build tiberius config
        let mut config = tiberius::Config::new();
        config.host(&config_wrapper.host);
        config.port(config_wrapper.port);
        config.authentication(tiberius::AuthMethod::sql_server(
            &config_wrapper.username,
            &config_wrapper.password,
        ));
        config.trust_cert();

        if !config_wrapper.database.is_empty() {
            config.database(&config_wrapper.database);
        }

        // Create TCP connection
        let tcp = tokio::net::TcpStream::connect((&config_wrapper.host[..], config_wrapper.port))
            .await
            .map_err(|e| QueryAstError::Execution {
                query: sql.to_string(),
                reason: format!("Failed to connect TCP: {}", e),
            })?;

        let _ = tcp.set_nodelay(true);

        // Create tiberius client
        use tokio_util::compat::TokioAsyncWriteCompatExt;
        let mut client = tiberius::Client::connect(config, tcp.compat_write())
            .await
            .map_err(|e| QueryAstError::Execution {
                query: sql.to_string(),
                reason: format!("Failed to connect: {}", e),
            })?;

        // Switch database if specified
        let final_sql = if let Some(db) = database_name {
            format!("USE [{}];\n{}", db.replace(']', "]]"), sql)
        } else {
            sql.to_string()
        };

        // Execute the query
        let mut stream =
            client
                .query(&final_sql, &[])
                .await
                .map_err(|e| QueryAstError::Execution {
                    query: final_sql.clone(),
                    reason: e.to_string(),
                })?;

        let mut all_rows = Vec::new();
        let mut headers = Vec::new();

        // Process result sets
        while let Some(item) = stream
            .try_next()
            .await
            .map_err(|e| QueryAstError::Execution {
                query: final_sql.clone(),
                reason: e.to_string(),
            })?
        {
            match item {
                tiberius::QueryItem::Metadata(meta) => {
                    // Extract column names
                    headers = meta
                        .columns()
                        .iter()
                        .map(|col| col.name().to_string())
                        .collect();
                }
                tiberius::QueryItem::Row(row) => {
                    let mut row_data = Vec::new();
                    for i in 0..row.len() {
                        // Try to convert each column to string
                        let value = if let Some(v) = row.get::<&str, _>(i) {
                            v.to_string()
                        } else if let Some(v) = row.get::<i32, _>(i) {
                            v.to_string()
                        } else if let Some(v) = row.get::<i64, _>(i) {
                            v.to_string()
                        } else if let Some(v) = row.get::<f64, _>(i) {
                            v.to_string()
                        } else if let Some(v) = row.get::<bool, _>(i) {
                            if v { "1" } else { "0" }.to_string()
                        } else {
                            String::new()
                        };
                        row_data.push(value);
                    }
                    all_rows.push(row_data);
                }
            }
        }

        debug!(
            "MssqlExecutor: query returned {} rows, {} columns",
            all_rows.len(),
            headers.len()
        );

        Ok((headers, all_rows))
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
