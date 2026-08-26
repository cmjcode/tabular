# Native Database Backup & Restore Wizard

Tabular provides a native database backup and restore engine designed for safe snapshots, automated compression, and intuitive progress streaming.

---

## 📦 Engine Overview

```
+-----------------------------------------------------------------------------------------+
|                                BACKUP & RESTORE ENGINE                                  |
+-----------------------------------------------------------------------------------------+
| [PostgreSQL]  -> Detects `pg_dump` & `pg_restore` binaries with stream pipeline         |
| [MySQL/Maria] -> Detects `mysqldump` & `mysql` binaries with transaction flags          |
| [SQLite]      -> Pure-Rust in-process backup using `libsqlite3_sys::sqlite3_backup_*`   |
+-----------------------------------------------------------------------------------------+
```

### Key Modules:
- **`src/backup_restore.rs`**: Core engine handling process execution, stdin/stdout stream pipes, compression (`.sql.gz`, `.tar`, `.dump`), and byte count telemetry.
- **`src/dialog_backup_restore.rs`**: Interactive wizard UI for configuring dump/restore options, file destinations, and live console monitoring.

---

## 💾 Backup Wizard Configuration

Open the Backup Dialog by right-clicking any database node in the sidebar tree and selecting **"💾 Backup Database..."**.

### Options:
1. **Dump Scope**:
   - **Schema & Data**: Complete dump (default).
   - **Schema Only**: DDL definitions without table rows (`--schema-only` / `--no-data`).
   - **Data Only**: Table inserts without structural definitions (`--data-only`).
2. **Advanced Flags**:
   - **Include Triggers & Procedures**: Back up stored routines, triggers, and views.
   - **Single Transaction**: Use transactional snapshots (`--single-transaction` / `--serializable-deferrable`) to avoid locking active workloads.
   - **Drop Objects Before Creation**: Adds `DROP TABLE IF EXISTS` headers for cleaner restoration.
3. **Output Formats**:
   - Plain SQL (`.sql`)
   - Compressed Gzip SQL (`.sql.gz`)
   - PostgreSQL Custom Archive (`.dump` / `.tar`)
   - Native SQLite Database (`.sqlite`)

---

## 📥 Restore Wizard Configuration

Open the Restore Dialog by right-clicking a database and selecting **"📥 Restore Database..."**.

### Features:
- **Automatic Format Detection**: Automatically inspects file extension and header magic bytes to select the correct restore pipeline (`psql`, `pg_restore`, `mysql`, or SQLite copy).
- **Error Resilience**: Option to **"Stop on Error"** or **"Continue on Errors (Log Warnings)"**.
- **Live Progress Monitor**: Real-time elapsed time, transferred bytes, and scrolling stdout/stderr console output.
