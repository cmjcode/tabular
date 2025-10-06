# Adding a New Database to Tabular

This guide explains how to add support for a new database type using the Agnostic AST architecture.

## Overview

Tabular uses a layered architecture with clear separation of concerns:

```
Raw SQL → Parser → Logical Plan → Optimizer/Rewriter → Emitter → Executor
                       ↓              ↓                    ↓         ↓
                   DB-Agnostic   DB-Agnostic          DB-Specific  DB-Specific
```

## Steps to Add a New Database

### 1. Define Database Type

Add your database to `src/models/enums.rs`:

```rust
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub enum DatabaseType {
    MySQL,
    PostgreSQL,
    SQLite,
    Redis,
    MsSQL,
    MongoDB,
    YourNewDB, // <- Add here
}

pub enum DatabasePool {
    MySQL(Arc<MySqlPool>),
    PostgreSQL(Arc<PgPool>),
    SQLite(Arc<SqlitePool>),
    Redis(Arc<ConnectionManager>),
    MsSQL(Arc<MssqlConfigWrapper>),
    MongoDB(Arc<MongoClient>),
    YourNewDB(Arc<YourDbConnection>), // <- Add here
}
```

### 2. Implement SQL Dialect

Create `src/query_ast/emitter/your_db_dialect.rs`:

```rust
use super::dialect::SqlDialect;
use crate::models::enums::DatabaseType;

pub struct YourDbDialect;

impl SqlDialect for YourDbDialect {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::YourNewDB
    }
    
    fn quote_ident(&self, ident: &str) -> String {
        // Your database's identifier quoting
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
    
    fn emit_limit(&self, limit: u64, offset: u64) -> String {
        // Your database's LIMIT syntax
        if offset > 0 {
            format!(" LIMIT {} OFFSET {}", limit, offset)
        } else {
            format!(" LIMIT {}", limit)
        }
    }
    
    fn supports_window_functions(&self) -> bool {
        true // or false, depending on your DB
    }
    
    fn supports_cte(&self) -> bool {
        true // or false
    }
    
    fn supports_full_join(&self) -> bool {
        true // or false
    }
    
    // Override other methods as needed for your database
}
```

Update `src/query_ast/emitter/dialect.rs`:

```rust
pub fn get_dialect(db_type: &DatabaseType) -> Box<dyn SqlDialect> {
    match db_type {
        DatabaseType::MySQL => Box::new(MySqlDialect),
        DatabaseType::PostgreSQL => Box::new(PostgresDialect),
        // ... other databases ...
        DatabaseType::YourNewDB => Box::new(YourDbDialect), // <- Add here
    }
}
```

### 3. Implement Database Executor

Create `src/query_ast/executors/your_db_executor.rs`:

```rust
use async_trait::async_trait;
use crate::query_ast::executor::{DatabaseExecutor, QueryResult};
use crate::query_ast::errors::QueryAstError;
use crate::models::enums::DatabaseType;

pub struct YourDbExecutor {
    // Connection pool or client reference
}

impl YourDbExecutor {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl DatabaseExecutor for YourDbExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::YourNewDB
    }
    
    async fn execute_query(
        &self,
        sql: &str,
        database_name: Option<&str>,
        connection_id: i64,
    ) -> Result<QueryResult, QueryAstError> {
        // 1. Get connection from pool using connection_id
        // 2. Switch database if database_name is provided
        // 3. Execute SQL query
        // 4. Convert results to (Vec<String>, Vec<Vec<String>>)
        // 5. Return results
        
        todo!("Implement query execution for your database")
    }
    
    fn validate_query(&self, sql: &str) -> Result<(), QueryAstError> {
        // Optional: Add database-specific validation
        Ok(())
    }
}
```

Register in `src/query_ast/executor.rs`:

```rust
impl ExecutorRegistry {
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        
        // ... existing executors ...
        
        #[cfg(feature = "your_db")]
        registry.register(Box::new(super::executors::YourDbExecutor::new()));
        
        registry
    }
}
```

### 4. Create Driver Module

Create `src/driver_your_db.rs`:

```rust
use crate::models::enums::DatabaseType;
use crate::window_egui::Tabular;

/// Create connection pool for your database
pub async fn create_connection_pool(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
) -> Result<YourDbConnection, Box<dyn std::error::Error>> {
    // Implement connection creation
    todo!()
}

/// Fetch tables from your database
pub async fn fetch_tables_from_connection(
    tabular: &Tabular,
    connection_id: i64,
    database_name: Option<String>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Implement table listing
    todo!()
}

/// Fetch columns from a table
pub async fn fetch_columns_from_table(
    tabular: &Tabular,
    connection_id: i64,
    database_name: Option<String>,
    table_name: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    // Implement column listing (name, type)
    todo!()
}
```

### 5. Update Connection Module

In `src/connection.rs`, add your database to the connection creation logic:

```rust
pub async fn get_or_create_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Result<models::enums::DatabasePool, Box<dyn std::error::Error>> {
    // ... existing code ...
    
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => { /* ... */ }
        models::enums::DatabaseType::PostgreSQL => { /* ... */ }
        // ... other databases ...
        models::enums::DatabaseType::YourNewDB => {
            let client = driver_your_db::create_connection_pool(
                &connection.host,
                connection.port,
                &connection.username,
                &connection.password,
                connection.database.as_deref(),
            ).await?;
            
            let pool = models::enums::DatabasePool::YourNewDB(Arc::new(client));
            tabular.connection_pools.insert(connection_id, pool.clone());
            Ok(pool)
        }
    }
}
```

### 6. Add UI Support

In `src/sidebar_database.rs`, add folder icon and logic:

```rust
pub fn create_database_folders_from_connections(connections: &[models::structs::Connection]) -> Vec<models::structs::TreeNode> {
    // ... existing code ...
    
    models::enums::DatabaseType::YourNewDB => {
        let mut node = models::structs::TreeNode {
            name: format!("Your DB Connections ({})", count),
            node_type: models::enums::NodeType::YourNewDBFolder,
            // ... rest of initialization ...
        };
        node
    }
}
```

Add to `src/models/enums.rs`:

```rust
pub enum NodeType {
    // ... existing types ...
    YourNewDBFolder, // <- Add here
}
```

## Testing Your Integration

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_your_db_query() {
        let executor = YourDbExecutor::new();
        let result = executor.execute_query(
            "SELECT * FROM users LIMIT 10",
            None,
            1,
        ).await;
        
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_your_db_dialect() {
        let dialect = YourDbDialect;
        assert_eq!(dialect.quote_ident("table"), "\"table\"");
    }
}
```

### Integration Testing

1. Create a test connection in the UI
2. Test basic operations: connect, list tables, query data
3. Test AST features: filtering, sorting, pagination
4. Test error handling: invalid queries, connection failures

## Performance Considerations

### Connection Pooling

- Use connection pools for better performance
- Configure appropriate pool size (5-20 connections typically)
- Implement connection health checks

### Query Optimization

- The rewrite layer applies database-agnostic optimizations
- Add database-specific optimizations in your executor
- Consider query result caching for expensive queries

### Plan Caching

The AST layer automatically caches compiled plans. Ensure your SQL emission is deterministic for cache hits.

## Feature Flags

Add a feature flag for optional compilation:

In `Cargo.toml`:

```toml
[features]
default = ["mysql", "postgres", "sqlite"]
mysql = ["sqlx/mysql"]
postgres = ["sqlx/postgres"]
sqlite = ["sqlx/sqlite"]
your_db = ["your_db_driver"] # <- Add here
```

## Common Pitfalls

1. **Quote Identifiers**: Always use `dialect.quote_ident()`, never hardcode quotes
2. **NULL Handling**: Different databases handle NULL differently
3. **Type Mapping**: Map SQL types to your database's native types
4. **Transaction Support**: Implement proper transaction handling
5. **Error Messages**: Provide clear, actionable error messages

## Example: Adding CockroachDB

Here's a minimal example of adding CockroachDB (which is PostgreSQL-compatible):

```rust
// 1. Add to DatabaseType enum
pub enum DatabaseType {
    // ... existing ...
    CockroachDB,
}

// 2. Reuse PostgreSQL dialect (it's compatible!)
pub fn get_dialect(db_type: &DatabaseType) -> Box<dyn SqlDialect> {
    match db_type {
        // ... existing ...
        DatabaseType::CockroachDB => Box::new(PostgresDialect), // Reuse!
    }
}

// 3. Create executor (can extend PostgreSQL executor)
pub struct CockroachExecutor {
    pg_executor: PostgresExecutor, // Composition over inheritance!
}

#[async_trait]
impl DatabaseExecutor for CockroachExecutor {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::CockroachDB
    }
    
    async fn execute_query(&self, sql: &str, db: Option<&str>, conn_id: i64) 
        -> Result<QueryResult, QueryAstError> 
    {
        // Delegate to PostgreSQL executor since CockroachDB is wire-compatible
        self.pg_executor.execute_query(sql, db, conn_id).await
    }
}
```

## Performance Metrics

After adding your database, measure:

- Query compilation time (should be < 1ms for simple queries)
- Cache hit rate (should be > 80% for repeated queries)
- Execution overhead (AST layer adds < 5% overhead)

## Need Help?

- Check existing drivers (`driver_mysql.rs`, `driver_postgres.rs`)
- Review the executor trait in `query_ast/executor.rs`
- Look at dialect implementations in `query_ast/emitter/dialect.rs`
- See rewrite rules in `query_ast/rewrite.rs` for optimization ideas

## Benefits of This Architecture

✅ **Separation of Concerns**: Logic layer independent of database specifics
✅ **Reusability**: Share optimizations across all databases
✅ **Testability**: Mock executors for unit testing
✅ **Extensibility**: Add new databases without touching core logic
✅ **Performance**: Aggressive caching at every layer
✅ **Type Safety**: Rust's type system catches errors at compile time
