//! Redis database executor
//! 
//! Note: Redis doesn't support SQL. This executor provides minimal
//! query translation for very basic operations like KEYS, GET, etc.
//! Most Redis operations should use native commands.

use async_trait::async_trait;
use redis::AsyncCommands;
use std::sync::Arc;
use log::{debug, warn};

use crate::models::enums::{DatabaseType, DatabasePool};
use crate::query_ast::executor::{DatabaseExecutor, QueryResult, SqlFeature};
use crate::query_ast::errors::QueryAstError;

pub struct RedisExecutor {
    // Executor is stateless - connection managers are managed externally
}

impl RedisExecutor {
    pub fn new() -> Self {
        Self {}
    }
    
    /// Get Redis connection manager from global connection pools
    fn get_connection_manager(connection_id: i64) -> Result<Arc<redis::aio::ConnectionManager>, QueryAstError> {
        // This will be accessed from global state in actual implementation
        Err(QueryAstError::Execution {
            query: format!("connection_id: {}", connection_id),
            reason: "Pool lookup not yet wired to global state".to_string(),
        })
    }
    
    /// Try to parse SQL-like query and convert to Redis operations
    /// This is extremely limited - mostly for compatibility
    fn parse_query_type(sql: &str) -> Result<RedisOperation, QueryAstError> {
        let sql_upper = sql.to_uppercase();
        
        if sql_upper.contains("KEYS") || sql_upper.starts_with("SELECT * FROM") {
            // SELECT * FROM keys or KEYS pattern
            let pattern = if let Some(pos) = sql_upper.find("WHERE") {
                let where_clause = &sql[pos + 5..];
                where_clause.split('=').nth(1)
                    .map(|s| s.trim().trim_matches(|c| c == '\'' || c == '"' || c == ';').to_string())
                    .unwrap_or_else(|| "*".to_string())
            } else {
                "*".to_string()
            };
            Ok(RedisOperation::Keys(pattern))
        } else if sql_upper.starts_with("SELECT") {
            // Very basic SELECT key_name -> GET key_name
            Err(QueryAstError::Unsupported(
                "Redis executor doesn't support SQL SELECT. Use native Redis commands."
            ))
        } else {
            Err(QueryAstError::Unsupported(
                "Redis executor only supports KEYS operation through SQL"
            ))
        }
    }
}

impl Default for RedisExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum RedisOperation {
    Keys(String), // Pattern for KEYS command
}

#[async_trait]
impl DatabaseExecutor for RedisExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::Redis
    }
    
    async fn execute_query(
        &self,
        sql: &str,
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        debug!("RedisExecutor: executing query on connection {}", connection_id);
        debug!("SQL: {}", sql);
        
        warn!("Redis: SQL support is minimal. Use native Redis commands for full functionality.");
        
        // Get connection manager from global registry
        let manager = Self::get_connection_manager(connection_id)?;
        let mut conn = manager.clone();
        
        // Redis database selection (0-15 by default)
        if let Some(db) = database_name {
            if let Ok(db_num) = db.parse::<i64>() {
                redis::cmd("SELECT")
                    .arg(db_num)
                    .query_async::<_, ()>(&mut conn)
                    .await
                    .map_err(|e| QueryAstError::Execution {
                        query: format!("SELECT {}", db_num),
                        reason: e.to_string(),
                    })?;
            }
        }
        
        // Parse the query
        let operation = Self::parse_query_type(sql)?;
        
        match operation {
            RedisOperation::Keys(pattern) => {
                // Execute KEYS command
                let keys: Vec<String> = conn.keys(&pattern)
                    .await
                    .map_err(|e| QueryAstError::Execution {
                        query: sql.to_string(),
                        reason: e.to_string(),
                    })?;
                
                // Format as table: key | type
                let headers = vec!["key".to_string(), "type".to_string()];
                let mut data = Vec::new();
                
                for key in keys {
                    let key_type: String = conn.key_type(&key)
                        .await
                        .map_err(|e| QueryAstError::Execution {
                            query: format!("TYPE {}", key),
                            reason: e.to_string(),
                        })?;
                    data.push(vec![key, key_type]);
                }
                
                debug!("RedisExecutor: query returned {} keys", data.len());
                
                Ok((headers, data))
            }
        }
    }
    
    fn supports_feature(&self, feature: SqlFeature) -> bool {
        match feature {
            // Redis doesn't support any SQL features
            SqlFeature::WindowFunctions => false,
            SqlFeature::Cte => false,
            SqlFeature::FullOuterJoin => false,
            SqlFeature::JsonOperators => false, // Use Redis JSON module instead
        }
    }
    
    fn validate_query(&self, sql: &str) -> Result<(), QueryAstError> {
        let trimmed = sql.trim().to_uppercase();
        
        // Redis through SQL is extremely limited
        if !trimmed.contains("KEYS") && !trimmed.starts_with("SELECT * FROM KEYS") {
            return Err(QueryAstError::Unsupported(
                "Redis executor only supports KEYS operations through SQL. Use native Redis commands."
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
        let executor = RedisExecutor::new();
        assert_eq!(executor.database_type(), DatabaseType::Redis);
    }
    
    #[test]
    fn test_feature_support() {
        let executor = RedisExecutor::new();
        assert!(!executor.supports_feature(SqlFeature::WindowFunctions));
        assert!(!executor.supports_feature(SqlFeature::Cte));
        assert!(!executor.supports_feature(SqlFeature::FullOuterJoin));
        assert!(!executor.supports_feature(SqlFeature::JsonOperators));
    }
    
    #[test]
    fn test_query_validation() {
        let executor = RedisExecutor::new();
        
        // Only KEYS-related queries should pass
        assert!(executor.validate_query("SELECT * FROM KEYS").is_ok());
        assert!(executor.validate_query("KEYS *").is_ok());
        
        // Everything else should fail
        assert!(executor.validate_query("SELECT * FROM users").is_err());
        assert!(executor.validate_query("INSERT INTO x VALUES (1)").is_err());
    }
}
