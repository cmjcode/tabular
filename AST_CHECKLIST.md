# Agnostic AST - Implementation Checklist

Checklist untuk melengkapi implementasi Agnostic AST di Tabular.

## ✅ Fase 1: Foundation (SELESAI)

- [x] Logical plan structure (`logical.rs`)
- [x] Parser dari SQL ke logical plan (`parser.rs`)
- [x] Basic emitter untuk semua database (`emitter/mod.rs`)
- [x] Rewrite rules (9 rules implemented)
- [x] Plan caching dengan multi-level fingerprinting
- [x] Error types dengan thiserror

## 🚧 Fase 2: Trait-Based Abstraction (PRIORITAS TINGGI)

### 2.1 Dialect Abstraction
- [x] Buat `SqlDialect` trait (`emitter/dialect.rs`)
- [ ] Implementasi dialect untuk semua database:
  - [x] MySQL
  - [x] PostgreSQL  
  - [x] SQLite
  - [x] MS SQL Server
  - [x] MongoDB (limited)
  - [x] Redis (limited)
- [ ] Update emitter untuk menggunakan dialect trait
- [ ] Test cases per dialect

### 2.2 Executor Abstraction
- [x] Buat `DatabaseExecutor` trait (`executor.rs`)
- [ ] Implementasi executor untuk semua database:
  - [ ] `MySqlExecutor`
  - [ ] `PostgresExecutor`
  - [ ] `SqliteExecutor`
  - [ ] `MssqlExecutor`
  - [ ] `MongoDbExecutor`
  - [ ] `RedisExecutor`
- [ ] Buat `ExecutorRegistry` untuk dynamic dispatch
- [ ] Integration dengan connection pool existing

### 2.3 Feature Detection
- [ ] Implementasi `supports_feature()` per executor
- [ ] Fallback logic untuk unsupported features
- [ ] Warning system untuk feature degradation
- [ ] Documentation per database capabilities

## 📝 Fase 3: Refactoring Existing Code (MEDIUM)

### 3.1 Driver Integration
- [ ] `driver_mysql.rs`: Gunakan AST pipeline
- [ ] `driver_postgres.rs`: Gunakan AST pipeline  
- [ ] `driver_sqlite.rs`: Gunakan AST pipeline
- [ ] `driver_mssql.rs`: Gunakan AST pipeline
- [ ] `driver_mongodb.rs`: Hybrid (AST + native)
- [ ] `driver_redis.rs`: Hybrid (AST + native)

### 3.2 Connection Module
- [ ] `connection.rs`: Route queries through executor registry
- [ ] Remove hardcoded match statements on DatabaseType
- [ ] Centralize query execution logic
- [ ] Better error propagation

### 3.3 Cache Data
- [ ] `cache_data.rs`: Use executor for metadata queries
- [ ] Standardize metadata format across databases
- [ ] Cache executor responses

## 🎨 Fase 4: Enhanced Emitter (MEDIUM)

### 4.1 Smart Emitter
- [ ] Detect database capabilities from executor
- [ ] Auto-fallback untuk unsupported features:
  - [ ] Window functions → subquery
  - [ ] FULL JOIN → LEFT + RIGHT UNION
  - [ ] CTE → inline subqueries
- [ ] Generate warnings for fallbacks

### 4.2 Dialect Enhancements
- [ ] JSON operators (PostgreSQL, MySQL)
- [ ] Array operations (PostgreSQL)
- [ ] Regex matching (per database syntax)
- [ ] Date/time functions (standardize)
- [ ] String functions (standardize)

### 4.3 Type System
- [ ] Type inference in logical plan
- [ ] Type checking before emission
- [ ] Auto-cast insertion where needed
- [ ] Type error messages

## 🧪 Fase 5: Testing & Validation (HIGH)

### 5.1 Unit Tests
- [ ] Parser tests (100+ cases)
- [ ] Rewrite tests (per rule)
- [ ] Emitter tests (per dialect)
- [ ] Executor tests (per database)
- [ ] Cache tests (hit/miss scenarios)

### 5.2 Integration Tests
- [ ] End-to-end query execution per database
- [ ] Cross-database query compatibility
- [ ] Performance benchmarks
- [ ] Memory leak detection
- [ ] Concurrent query stress test

### 5.3 Regression Tests
- [ ] Test suite dari production queries
- [ ] Known edge cases
- [ ] Database version compatibility
- [ ] Error handling scenarios

## 📊 Fase 6: Performance Optimization (LOW)

### 6.1 Query Optimization
- [ ] Cost-based join ordering
- [ ] Index hints (per database)
- [ ] Partition pruning
- [ ] Parallel query execution

### 6.2 Cache Optimization
- [ ] LRU eviction policy
- [ ] Cache warming strategies
- [ ] Memory usage monitoring
- [ ] Cache statistics dashboard

### 6.3 Code Optimization
- [ ] Profile hot paths
- [ ] Reduce allocations (use arena)
- [ ] Lazy evaluation where possible
- [ ] SIMD for data processing

## 📚 Fase 7: Documentation (ONGOING)

- [x] `AST_ARCHITECTURE.md` - Overview
- [x] `ADDING_NEW_DATABASE.md` - Guide
- [ ] Inline code documentation (rustdoc)
- [ ] Examples per database
- [ ] Performance tuning guide
- [ ] Troubleshooting guide
- [ ] Migration guide (legacy → AST)

## 🔄 Fase 8: Migration Path (MEDIUM)

### 8.1 Gradual Migration
- [ ] Feature flag untuk AST vs legacy
- [ ] A/B testing framework
- [ ] Telemetry untuk success rates
- [ ] Rollback mechanism

### 8.2 Backward Compatibility
- [ ] Support legacy driver API
- [ ] Deprecation warnings
- [ ] Migration tools/scripts
- [ ] Version negotiation

## 🎯 Quick Wins (Bisa Dikerjakan Dulu)

Sorted by impact/effort ratio:

### Week 1-2: Core Abstractions
1. ✅ Implementasi `SqlDialect` trait dengan 6 dialects
2. ✅ Implementasi `DatabaseExecutor` trait (skeleton)
3. [ ] Update `emitter/mod.rs` untuk pakai dialect trait
4. [ ] Basic tests untuk dialect

**Impact**: 🟢 High (foundation untuk semua fitur lain)  
**Effort**: 🟡 Medium (2-3 hari)

### Week 3-4: Executor Implementation
1. [ ] Implementasi `MySqlExecutor` (prioritas - paling banyak dipakai)
2. [ ] Implementasi `PostgresExecutor`
3. [ ] Implementasi `SqliteExecutor`
4. [ ] Registry dengan dynamic dispatch

**Impact**: 🟢 High (unlock AST execution)  
**Effort**: 🔴 High (5-7 hari)

### Week 5: Integration
1. [ ] Route `connection.rs` queries through executor registry
2. [ ] Update `window_egui.rs` untuk pakai AST pipeline
3. [ ] Feature flag untuk enable/disable AST
4. [ ] Basic error handling

**Impact**: 🟢 High (production-ready)  
**Effort**: 🟡 Medium (3-4 hari)

### Week 6-7: Testing & Polish
1. [ ] Integration tests per database
2. [ ] Performance benchmarks
3. [ ] Error message improvements
4. [ ] Documentation cleanup

**Impact**: 🟡 Medium (polish & stability)  
**Effort**: 🟡 Medium (4-5 hari)

### Week 8: MS SQL & Special Cases
1. [ ] Implementasi `MssqlExecutor`
2. [ ] MongoDB hybrid approach (AST + native)
3. [ ] Redis minimal support
4. [ ] Edge case handling

**Impact**: 🟡 Medium (completeness)  
**Effort**: 🟡 Medium (3-4 hari)

## 🚀 Quick Start (Mulai dari Mana?)

### Step 1: Update Emitter (1 hari)

Ganti `FlatEmitter` di `emitter/mod.rs` untuk pakai `SqlDialect` trait:

```rust
// OLD
impl FlatEmitter {
    fn quote_ident(&self, ident: &str) -> String {
        match self.dialect {
            DatabaseType::MySQL => format!("`{}`", ident),
            DatabaseType::PostgreSQL => format!("\"{}\"", ident),
            // ...
        }
    }
}

// NEW
impl FlatEmitter {
    fn new(dialect: Box<dyn SqlDialect>) -> Self {
        Self { dialect }
    }
    
    fn quote_ident(&self, ident: &str) -> String {
        self.dialect.quote_ident(ident)
    }
}
```

### Step 2: Implement MySqlExecutor (2-3 hari)

Buat `src/query_ast/executors/mysql.rs`:

```rust
pub struct MySqlExecutor;

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
        // Get pool from global registry
        let pool = get_mysql_pool(connection_id)?;
        
        // Switch database if needed
        if let Some(db) = database_name {
            sqlx::query(&format!("USE `{}`", db))
                .execute(&pool)
                .await?;
        }
        
        // Execute query
        let rows = sqlx::query(sql).fetch_all(&pool).await?;
        
        // Convert to standard format
        let headers = rows[0].columns().iter()
            .map(|c| c.name().to_string())
            .collect();
        
        let data = rows.iter()
            .map(|row| {
                (0..row.len())
                    .map(|i| row.get::<String, _>(i))
                    .collect()
            })
            .collect();
        
        Ok((headers, data))
    }
}
```

### Step 3: Wire Everything Together (1 hari)

Update `connection.rs` untuk route ke executor:

```rust
pub async fn execute_query_with_ast(
    connection_id: i64,
    query: &str,
    database_name: Option<&str>,
) -> Result<QueryResult, QueryAstError> {
    let registry = ExecutorRegistry::with_defaults();
    let db_type = get_database_type(connection_id)?;
    
    execute_ast_query(
        query,
        &db_type,
        connection_id,
        database_name,
        None, // pagination
        true, // inject_auto_limit
        &registry,
    ).await
}
```

## 📈 Success Metrics

Track these untuk measure progress:

- **Coverage**: % queries going through AST pipeline
- **Performance**: Query compilation time (target: < 5ms)
- **Cache Hit Rate**: % cached plans reused (target: > 80%)
- **Error Rate**: % queries failing AST pipeline (target: < 1%)
- **Memory**: AST overhead (target: < 10MB for 1000 plans)

## 🎓 Learning Resources

Untuk yang mau contribute:

1. **Rust Async**: https://rust-lang.github.io/async-book/
2. **sqlparser**: https://docs.rs/sqlparser/latest/sqlparser/
3. **Query Optimization**: "Database System Concepts" ch. 13-15
4. **Design Patterns**: Strategy, Visitor, Factory patterns

## 💡 Tips

- **Start small**: Test dengan 1-2 databases dulu
- **Iterate**: Jangan tunggu perfect, ship early & improve
- **Test driven**: Tulis test dulu baru implement
- **Profile**: Measure before optimizing
- **Document**: Jelaskan "why" bukan cuma "what"

## 🐛 Known Issues

Track issues yang perlu di-fix:

- [ ] CTE inlining bisa infinite loop pada recursive CTEs
- [ ] Window function frame clause belum di-parse lengkap  
- [ ] Subquery correlation detection kadang false positive
- [ ] MongoDB: Aggregation pipeline belum fully supported
- [ ] Redis: Hanya support key operations, bukan SQL

## 📞 Questions?

Buat issue di GitHub atau tanya di discussion forum.

---

**Last Updated**: 2025-10-06  
**Next Review**: Setiap end of week
