# DBA Process Monitoring, User Management & Enterprise Security

Tabular includes a suite of DBA tools for live server inspection, connection management, privilege configuration, and high-security networking.

---

## 🚦 1. Real-Time Processlist & Deadlock Monitor

Located in `src/dba_monitor.rs`, this panel provides live visibility into active database sessions and lock contention.

### Capabilities:
- **Multi-Engine Polling**:
  - **PostgreSQL**: `pg_stat_activity` combined with `pg_locks`.
  - **MySQL**: `information_schema.processlist` and `performance_schema.data_locks`.
  - **SQL Server**: `sys.dm_exec_requests`, `sys.dm_exec_sessions`, and `sys.dm_tran_locks`.
- **Lock Dependency Tree**: Displays visual parent-child hierarchies when a transaction is blocked waiting on another transaction holding an exclusive lock.
- **1-Click Query Termination**:
  - PostgreSQL: Calls `pg_cancel_backend(pid)` or `pg_terminate_backend(pid)`.
  - MySQL: Calls `KILL <connection_id>` or `KILL QUERY <connection_id>`.
  - SQL Server: Calls `KILL <spid>`.
- **Auto-Refresh Rate**: Configurable interval (1s, 2s, 5s, 10s) with manual pause.

---

## 👥 2. User & Privileges Management GUI

Located in `src/user_manager.rs`, this graphical interface replaces manual `CREATE USER` and `GRANT` scripts.

### Capabilities:
- **User Account Management**:
  - List existing database users and assigned roles.
  - Create new users with strong password hashing (`SCRAM-SHA-256`, `caching_sha2_password`).
  - Alter passwords and lock/unlock accounts.
- **Granular Privilege Matrix**:
  - Visual checkbox matrix for database-level and table-level permissions:
    - Data Privileges: `SELECT`, `INSERT`, `UPDATE`, `DELETE`
    - Schema Privileges: `CREATE`, `ALTER`, `DROP`, `INDEX`
    - Routine Privileges: `EXECUTE`
  - Automatic generation and preview of the resulting `GRANT` / `REVOKE` DDL statements before execution.

---

## 🛡️ 3. Enterprise Security: mTLS & Multi-Hop SSH Bastion

Located in `src/ssh_tunnel.rs` and `src/connection/pool.rs`:

### mTLS (Mutual TLS):
- Configure custom **CA Certificate** (`ca.pem` / `ca.crt`).
- Configure **Client Certificate** (`client-cert.pem`) and **Private Key** (`client-key.pem`).
- Support for encrypted private keys with interactive passphrase prompts.

### Multi-Hop SSH Bastion Jump:
- Connect to isolated databases residing in private VPCs via intermediate bastion jump hosts (`Local Machine -> Bastion 1 -> Bastion 2 -> Database`).
- Supports SSH key authentication, SSH agent forwarding, and password credentials.
