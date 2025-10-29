//! MongoDB database executor
//!
//! Note: MongoDB doesn't natively support SQL. This executor handles basic
//! SELECT queries by translating them to MongoDB aggregation pipeline.
//! Complex SQL features are not supported.

use async_trait::async_trait;
use futures_util::TryStreamExt;
use log::{debug, warn};
use mongodb::bson::{Document, doc};
use std::sync::Arc;

use crate::models::enums::DatabaseType;
use crate::query_ast::errors::QueryAstError;
use crate::query_ast::executor::{DatabaseExecutor, QueryResult, SqlFeature};

pub struct MongoDbExecutor {
    // Executor is stateless - clients are managed externally
}

impl MongoDbExecutor {
    pub fn new() -> Self {
        Self {}
    }

    /// Get MongoDB client from global connection pools
    fn get_client(connection_id: i64) -> Result<Arc<mongodb::Client>, QueryAstError> {
        // This will be accessed from global state in actual implementation
        Err(QueryAstError::Execution {
            query: format!("connection_id: {}", connection_id),
            reason: "Pool lookup not yet wired to global state".to_string(),
        })
    }

    /// Parse very basic SELECT queries and convert to MongoDB operations
    /// Format: SELECT * FROM collection [WHERE field = value] [LIMIT n]
    fn parse_simple_select(
        sql: &str,
    ) -> Result<(String, Option<Document>, Option<i64>), QueryAstError> {
        let sql_upper = sql.to_uppercase();

        // Extract collection name after FROM
        let from_pos = sql_upper
            .find(" FROM ")
            .ok_or(QueryAstError::Unsupported("Missing FROM clause"))?;

        let after_from = &sql[from_pos + 6..];

        // Find collection name (until WHERE, LIMIT, or end)
        let collection = after_from
            .split_whitespace()
            .next()
            .ok_or(QueryAstError::Unsupported("Missing collection name"))?
            .trim_matches(|c| c == '`' || c == '"' || c == ';')
            .to_string();

        // Very basic WHERE parsing (field = 'value')
        let filter = if let Some(where_pos) = sql_upper.find(" WHERE ") {
            // This is extremely simplified - real implementation would need proper parsing
            let where_clause = &sql[where_pos + 7..];
            let parts: Vec<&str> = where_clause.split('=').take(2).collect();
            if parts.len() == 2 {
                let field = parts[0].trim().to_string();
                let value = parts[1]
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"' || c == ';')
                    .to_string();
                Some(doc! { field: value })
            } else {
                None
            }
        } else {
            None
        };

        // Extract LIMIT
        let limit = if let Some(limit_pos) = sql_upper.find(" LIMIT ") {
            let limit_str = &sql[limit_pos + 7..];

            limit_str
                .split_whitespace()
                .next()
                .and_then(|s| s.trim_matches(';').parse::<i64>().ok())
        } else {
            None
        };

        Ok((collection, filter, limit))
    }
}

impl Default for MongoDbExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseExecutor for MongoDbExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::MongoDB
    }

    async fn execute_query(
        &self,
        sql: &str,
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        debug!(
            "MongoDbExecutor: executing query on connection {}",
            connection_id
        );
        debug!("SQL: {}", sql);

        warn!("MongoDB: SQL support is limited. Consider using native MongoDB operations.");

        // Get client from global registry
        let client = Self::get_client(connection_id)?;

        // MongoDB requires database name
        let db_name = database_name
            .ok_or_else(|| QueryAstError::Semantic("MongoDB requires database name".to_string()))?;

        let database = client.database(db_name);

        // Try to parse the SQL (very basic support)
        let (collection_name, filter, limit) = Self::parse_simple_select(sql)?;

        let collection = database.collection::<Document>(&collection_name);

        // Build find options
        let mut find_options = mongodb::options::FindOptions::default();
        if let Some(l) = limit {
            find_options.limit = Some(l);
        }

        // Execute find query
        let cursor = if let Some(f) = filter {
            collection.find(f).with_options(find_options).await
        } else {
            collection.find(doc! {}).with_options(find_options).await
        }
        .map_err(|e| QueryAstError::Execution {
            query: sql.to_string(),
            reason: e.to_string(),
        })?;

        // Collect results
        let docs: Vec<Document> =
            cursor
                .try_collect()
                .await
                .map_err(|e| QueryAstError::Execution {
                    query: sql.to_string(),
                    reason: e.to_string(),
                })?;

        // Extract headers from first document
        let headers = if let Some(first_doc) = docs.first() {
            first_doc.keys().map(|k| k.to_string()).collect()
        } else {
            vec!["_id".to_string()]
        };

        // Convert documents to rows
        let data: Vec<Vec<String>> = docs
            .iter()
            .map(|doc| {
                headers
                    .iter()
                    .map(|header| {
                        doc.get(header)
                            .map(|v| format!("{}", v))
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect();

        debug!("MongoDbExecutor: query returned {} documents", data.len());

        Ok((headers, data))
    }

    fn supports_feature(&self, feature: SqlFeature) -> bool {
        match feature {
            SqlFeature::WindowFunctions => false, // Not applicable to MongoDB
            SqlFeature::Cte => false,             // Not applicable
            SqlFeature::FullOuterJoin => false,   // Use $lookup for joins
            SqlFeature::JsonOperators => true,    // Native JSON/BSON support
        }
    }

    fn validate_query(&self, sql: &str) -> Result<(), QueryAstError> {
        let trimmed = sql.trim().to_uppercase();

        // MongoDB through SQL is very limited
        if !trimmed.starts_with("SELECT ") {
            return Err(QueryAstError::Unsupported(
                "MongoDB executor only supports basic SELECT queries",
            ));
        }

        // Block operations that don't make sense for MongoDB
        if trimmed.contains(" JOIN ") {
            return Err(QueryAstError::Unsupported(
                "MongoDB doesn't support SQL JOINs. Use native $lookup instead.",
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
        let executor = MongoDbExecutor::new();
        assert_eq!(executor.database_type(), DatabaseType::MongoDB);
    }

    #[test]
    fn test_feature_support() {
        let executor = MongoDbExecutor::new();
        assert!(!executor.supports_feature(SqlFeature::WindowFunctions));
        assert!(!executor.supports_feature(SqlFeature::Cte));
        assert!(!executor.supports_feature(SqlFeature::FullOuterJoin));
        assert!(executor.supports_feature(SqlFeature::JsonOperators));
    }

    #[test]
    fn test_query_validation() {
        let executor = MongoDbExecutor::new();

        assert!(executor.validate_query("SELECT * FROM users").is_ok());
        assert!(
            executor
                .validate_query("UPDATE users SET name = 'x'")
                .is_err()
        );
        assert!(
            executor
                .validate_query("SELECT * FROM users JOIN orders")
                .is_err()
        );
    }

    #[test]
    fn test_simple_select_parsing() {
        let (coll, filter, limit) = MongoDbExecutor::parse_simple_select(
            "SELECT * FROM users WHERE name = 'john' LIMIT 10",
        )
        .unwrap();

        assert_eq!(coll, "users");
        assert!(filter.is_some());
        assert_eq!(limit, Some(10));
    }
}
