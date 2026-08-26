# Tabular v0.13.0 Release Notes: Master Powerhouse Release

We are thrilled to announce **Tabular v0.13.0**, a monumental release bringing full feature parity and surpassing tools like DataGrip and TablePlus in key developer workflows — all while keeping Tabular's hallmark **pure Rust speed, instant startup (~0.1s), and lightweight memory footprint (~30-70 MB RAM)**.

---

## 🌟 Major Highlights & New Features

### 📊 1. Visual Query Execution Profiler
- **Interactive Node Graph**: Renders hierarchical execution plans for `EXPLAIN (ANALYZE, BUFFERS)` (Postgres), `EXPLAIN FORMAT=JSON` (MySQL), and ShowPlan XML (MSSQL).
- **Sugiyama Tree Layout Canvas**: Smooth panning, zooming, and node inspection.
- **Cost Percentage Heatmap**: Automatically highlights heavy nodes and bottlenecks in vibrant red.
- **Automated Heuristics**: Warns on large table Sequential Scans, Cartesian Products, and Disk Spills.

### ⚡ 2. High-Impact Data Grid & Multi-Tab Value Inspector
- **Server-Side GUI Filter Builder**: Add dynamic WHERE conditions (`=`, `!=`, `LIKE`, `IN`, `IS NULL`, `BETWEEN`, `>`, `<`) pushed directly to database queries.
- **1-Click Foreign Key Jump**: Hyperlinked foreign key cells to navigate parent/child relational records instantly.
- **Multi-Tab Inspector Modal**:
  - **JSON Tree & Formatter**: Collapsible syntax-highlighted tree view with search and sub-tree copying.
  - **Hex / Binary Editor**: Hex byte viewer with ASCII representation and decode options.
  - **Image Viewer**: Render PNG, JPEG, WebP, GIF, BMP, ICO, and SVG assets stored in BLOB fields.
  - **Virtual Scrolling Text**: Render megabyte-sized text payloads with zero UI lag.
- **Column Pinning & Freeze**: Lock important identifier columns while scrolling wide tables horizontally.

### 💾 3. Native Database Backup, Restore & Schema Migration
- **Native Backup & Restore Wizard**:
  - Direct integration with native CLI binaries (`pg_dump`, `pg_restore`, `mysqldump`, `mysql`).
  - Pure-Rust SQLite backup handler (`sqlite3_backup_*`).
  - Export to `.sql`, `.sql.gz`, `.tar`, and `.dump` with live progress telemetry.
- **Two-Way Schema Sync**: Structural schema diffing between databases with automated generation of forward `ALTER TABLE` and `ROLLBACK` migration scripts.

### 🚦 4. DBA Process Monitor & Enterprise Security
- **Real-Time Processlist & Deadlock Tree**: Live session monitoring, blocking transaction graphs, and 1-click `KILL PID` termination.
- **User & Privileges Management GUI**: Visual user creation, password management, and database/table grant matrices.
- **Enterprise Security**: mTLS (Custom CA, Client Certificates & Passphrase-protected Private Keys) and Multi-Hop SSH Bastion Tunneling.

### 🔌 5. WebAssembly (Wasm) Plugin Runtime & Custom Exporter SDK
- Sandboxed `wasmi` Wasm engine with Host API bindings (`selected_rows`, `table_schema`, `export_data`, `log`).
- Built-in starter plugins:
  - Apache Parquet & DuckDB export.
  - ORM Model Generator for Rust (Diesel/SeaORM), TypeScript (Prisma/TypeORM), and Python (SQLAlchemy).
- Interactive Plugin Manager Modal with catalog, runner, and artifact output inspection.

### 🔍 6. Universal Quick Open & Semantic Find & Replace
- **Universal Quick Open (`Cmd+P` / `Cmd+K`)**: Rapid fuzzy search across tables, views, procedures, saved queries, history, connections, and commands with category filter pills (`t:`, `v:`, `p:`, `q:`, `h:`, `c:`, `>`).
- **In-Editor Find & Replace**: Floating search panel supporting Regex, Case Matching, Whole Word, and In-Selection scope.

---

## 🛠️ Summary of Changes

```
v0.12.0  ──►  v0.13.0 (Visual Profiler, Server-side Filter Builder, FK Navigation, Backup & Restore, Two-Way Migration, DBA Monitor, Wasm Plugins, Quick Open)
```

---

## 📥 Upgrade Notes
- No breaking configuration changes. Existing SQLite cache and connection settings in `~/.tabular` are fully compatible.
- To use native database dumping for PostgreSQL or MySQL, ensure `pg_dump` and `mysqldump` are installed in your system PATH (or use pure-Rust SQLite backup).
