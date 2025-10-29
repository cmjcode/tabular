//! Database-agnostic query executor trait
//!
//! This layer abstracts query execution across different database types,
//! making it easy to add new databases without touching core logic.

use super::errors::QueryAstError;
use crate::models::enums::DatabaseType;

/// Result of query execution: (headers, rows)
pub type QueryResult = (Vec<String>, Vec<Vec<String>>);

/// Trait for database-agnostic query execution
///
/// Implement this trait for each database type to enable seamless integration
/// with the AST layer. The executor is responsible for:
/// - Executing emitted SQL
/// - Handling connection pooling
/// - Converting results to standard format
/// - Database-specific optimizations
#[async_trait::async_trait]
pub trait DatabaseExecutor: Send + Sync {
    /// Get the database type this executor handles
    fn database_type(&self) -> DatabaseType;

    /// Execute a query and return results
    ///
    /// # Arguments
    /// * `sql` - The SQL query to execute (already emitted for this DB type)
    /// * `database_name` - Optional database/schema name to USE
    /// * `connection_id` - Connection identifier for pool lookup
    ///
    /// # Returns
    /// Result containing (headers, rows) on success
    async fn execute_query(
        &self,
        sql: &str,
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError>;

    /// Check if this executor supports a specific SQL feature
    /// Used by the emitter to generate appropriate SQL
    fn supports_feature(&self, feature: SqlFeature) -> bool {
        // Default implementations
        match feature {
            SqlFeature::WindowFunctions => {
                matches!(
                    self.database_type(),
                    DatabaseType::PostgreSQL | DatabaseType::MySQL | DatabaseType::MsSQL
                )
            }
            SqlFeature::Cte => {
                matches!(
                    self.database_type(),
                    DatabaseType::PostgreSQL
                        | DatabaseType::MySQL
                        | DatabaseType::MsSQL
                        | DatabaseType::SQLite
                )
            }
            SqlFeature::FullOuterJoin => {
                matches!(
                    self.database_type(),
                    DatabaseType::PostgreSQL | DatabaseType::MsSQL
                )
            }
            SqlFeature::JsonOperators => {
                matches!(
                    self.database_type(),
                    DatabaseType::PostgreSQL | DatabaseType::MySQL
                )
            }
        }
    }

    /// Get dialect-specific pagination strategy
    fn pagination_strategy(&self) -> PaginationStrategy {
        match self.database_type() {
            DatabaseType::MsSQL => PaginationStrategy::TopOffset,
            _ => PaginationStrategy::LimitOffset,
        }
    }

    /// Validate query before execution (optional hook)
    fn validate_query(&self, _sql: &str) -> Result<(), QueryAstError> {
        Ok(())
    }
}

/// SQL features that may not be supported by all databases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlFeature {
    WindowFunctions,
    Cte,
    FullOuterJoin,
    JsonOperators,
}

/// Pagination implementation strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaginationStrategy {
    /// LIMIT n OFFSET m (PostgreSQL, MySQL, SQLite)
    LimitOffset,
    /// SELECT TOP n ... OFFSET m (MS SQL Server)
    TopOffset,
    /// MongoDB-style skip/limit
    SkipLimit,
}

/// Registry of database executors
///
/// Allows dynamic dispatch to the right executor based on database type
pub struct ExecutorRegistry {
    executors: std::collections::HashMap<DatabaseType, Box<dyn DatabaseExecutor>>,
}

impl ExecutorRegistry {
    pub fn new() -> Self {
        Self {
            executors: std::collections::HashMap::new(),
        }
    }

    /// Register an executor for a database type
    pub fn register(&mut self, executor: Box<dyn DatabaseExecutor>) {
        let db_type = executor.database_type();
        self.executors.insert(db_type, executor);
    }

    /// Get executor for a database type
    pub fn get(&self, db_type: &DatabaseType) -> Option<&dyn DatabaseExecutor> {
        self.executors.get(db_type).map(|b| &**b)
    }

    /// Create a default registry with all built-in executors
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();

        // Register all executors (always available, not feature-gated)
        registry.register(Box::new(crate::query_ast::executors::MySqlExecutor::new()));
        registry.register(Box::new(
            crate::query_ast::executors::PostgresExecutor::new(),
        ));
        registry.register(Box::new(crate::query_ast::executors::SqliteExecutor::new()));
        registry.register(Box::new(crate::query_ast::executors::MssqlExecutor::new()));
        registry.register(Box::new(crate::query_ast::executors::MongoDbExecutor::new()));
        registry.register(Box::new(crate::query_ast::executors::RedisExecutor::new()));

        registry
    }
}

impl Default for ExecutorRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Execute a query using the AST pipeline with automatic executor selection
///
/// This is the main entry point for AST-based query execution.
/// It compiles the query to a logical plan, applies optimizations,
/// emits database-specific SQL, and executes it.
pub async fn execute_ast_query(
    raw_sql: &str,
    db_type: &DatabaseType,
    connection_id: i64,
    database_name: Option<&str>,
    pagination: Option<(u64, u64)>,
    inject_auto_limit: bool,
    registry: &ExecutorRegistry,
) -> Result<QueryResult, QueryAstError> {
    // 1. Compile to logical plan and emit SQL
    let (emitted_sql, _headers) =
        super::compile_single_select(raw_sql, db_type, pagination, inject_auto_limit)?;

    // 2. Get executor for this database type
    let executor = registry
        .get(db_type)
        .ok_or(QueryAstError::Unsupported("database type not registered"))?;

    // 3. Execute the query
    executor
        .execute_query(&emitted_sql, database_name, connection_id)
        .await
}
