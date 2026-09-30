//! Operasi agent di luar jalur read-only (checklist K1, K11).
//!
//! - Rencana eksekusi statement tulis: setiap statement diklasifikasi lalu
//!   diputuskan dengan [`access::decide`] berdasarkan level akses koneksi.
//!   Rencana tidak menjalankan apa pun; server MCP yang meminta persetujuan
//!   (bila perlu) lalu memanggil [`HeadlessSession::execute_plan`].
//! - Tool tambahan yang tetap read-only: daftar tabel, detail tabel, DDL,
//!   contoh baris, jumlah baris, query yang sedang berjalan.
//!
//! Semua fungsi di sini headless (tanpa egui) dan hanya menerima data biasa.

use serde::Serialize;

use super::access::{self, AccessLevel, ConnectionAccess, Decision, StatementRisk};
use super::classify::{self, StatementKind};
use super::core::{
    AgentError, AgentQueryResult, ColumnDescription, ConnectionSummary, ForeignKeyDescription,
    HeadlessSession, IndexDescription, kind_label,
};
use crate::models::enums::DatabaseType;

/// Satu statement di dalam rencana.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PlannedStatement {
    pub statement: String,
    pub kind: StatementKind,
    /// `allow`, `needs_approval`, atau `deny`.
    pub decision: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Rencana eksekusi yang sudah diputuskan tetapi belum dijalankan.
#[derive(Debug, Clone)]
pub struct WritePlan {
    pub connection_id: i64,
    pub connection_name: String,
    pub database: Option<String>,
    pub level: AccessLevel,
    pub sql: String,
    pub statements: Vec<PlannedStatement>,
    pub decision: Decision,
}

impl WritePlan {
    /// Jenis statement paling "berat" di rencana, untuk log dan dialog.
    pub fn strongest_kind(&self) -> StatementKind {
        let rank = |k: StatementKind| match k {
            StatementKind::Read => 0,
            StatementKind::Write => 1,
            StatementKind::Ddl => 2,
            StatementKind::Admin => 3,
            StatementKind::Unknown => 4,
        };
        self.statements
            .iter()
            .map(|s| s.kind)
            .max_by_key(|k| rank(*k))
            .unwrap_or(StatementKind::Unknown)
    }
}

/// Gabungkan keputusan per statement: satu `Deny` menolak semuanya; bila ada
/// yang perlu persetujuan, seluruh batch disetujui sekaligus.
pub fn aggregate(decisions: &[Decision]) -> Decision {
    if let Some(Decision::Deny(r)) = decisions.iter().find(|d| matches!(d, Decision::Deny(_))) {
        return Decision::Deny(r.clone());
    }
    let mut reasons: Vec<&str> = decisions
        .iter()
        .filter_map(|d| match d {
            Decision::NeedsApproval(r) => Some(r.as_str()),
            _ => None,
        })
        .collect();
    reasons.dedup();
    if reasons.is_empty() {
        Decision::Allow
    } else {
        Decision::NeedsApproval(reasons.join("; "))
    }
}

/// Koneksi sebagaimana dilihat oleh satu klien agent.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AgentConnection {
    #[serde(flatten)]
    pub summary: ConnectionSummary,
    /// Level akses agent: read_only, ask, edit, agent.
    pub access: AccessLevel,
    /// `true` bila execute_statement bisa dipakai (mungkin dengan persetujuan).
    pub writes_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TableListEntry {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TableList {
    pub connection_id: i64,
    pub database: String,
    pub total: usize,
    pub tables: Vec<TableListEntry>,
    pub truncated: bool,
}

/// FK dari tabel lain yang menunjuk ke tabel ini.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct IncomingReference {
    pub table: String,
    pub column: String,
    pub references_column: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TableDetail {
    pub connection_id: i64,
    pub database: String,
    pub name: String,
    pub kind: String,
    pub columns: Vec<ColumnDescription>,
    pub foreign_keys: Vec<ForeignKeyDescription>,
    pub referenced_by: Vec<IncomingReference>,
    pub indexes: Vec<IndexDescription>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TableDdl {
    pub connection_id: i64,
    pub database: String,
    pub table: String,
    pub ddl: String,
    /// `server` (dari database) atau `cache` (disusun dari cache Tabular,
    /// tanpa default, constraint lain, atau opsi storage).
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RowCount {
    pub connection_id: i64,
    pub database: String,
    pub table: String,
    pub rows: i64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RunningQuery {
    pub pid: i64,
    pub user: String,
    pub database: String,
    pub state: String,
    pub duration_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_event: Option<String>,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<i64>,
    pub is_blocking: bool,
}

/// Kutip identifier sesuai dialek. `schema.table` dikutip per bagian.
pub fn quote_ident(db: &DatabaseType, name: &str) -> String {
    let quote_one = |part: &str| match db {
        DatabaseType::MySQL => format!("`{}`", part.replace('`', "``")),
        DatabaseType::MsSQL => format!("[{}]", part.replace(']', "]]")),
        _ => format!("\"{}\"", part.replace('"', "\"\"")),
    };
    match name.split_once('.') {
        Some((schema, table))
            if matches!(db, DatabaseType::PostgreSQL | DatabaseType::MsSQL)
                && !schema.is_empty()
                && !table.is_empty() =>
        {
            format!("{}.{}", quote_one(schema), quote_one(table))
        }
        _ => quote_one(name),
    }
}

/// `SELECT *` terbatas per dialek.
pub fn sample_sql(db: &DatabaseType, table: &str, limit: usize) -> String {
    let t = quote_ident(db, table);
    match db {
        DatabaseType::MsSQL => format!("SELECT TOP ({limit}) * FROM {t}"),
        _ => format!("SELECT * FROM {t} LIMIT {limit}"),
    }
}

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

impl HeadlessSession {
    /// Pengaturan akses koneksi (default read-only bila belum diatur).
    pub async fn connection_access(&self, id: i64) -> Result<ConnectionAccess, AgentError> {
        Ok(access::load(self.cache_pool(), id).await?)
    }

    /// Tolak bila koneksi diblokir atau klien tidak ada di allowlist.
    pub async fn ensure_access(
        &self,
        client: &str,
        id: i64,
    ) -> Result<ConnectionAccess, AgentError> {
        let acc = self.connection_access(id).await?;
        if acc.level == AccessLevel::Blocked {
            // Pesan sama dengan koneksi yang tidak ada supaya keberadaannya
            // tidak bocor ke agent.
            return Err(AgentError::ConnectionNotFound(id));
        }
        if !acc.client_allowed(client) {
            return Err(AgentError::Refused(format!(
                "MCP client \"{client}\" is not allowed to use connection {id}; the user can allow it \
                 in Tabular under Settings > Agent Access"
            )));
        }
        Ok(acc)
    }

    async fn is_production(&self, id: i64, name: &str) -> bool {
        let explicit = crate::connection_env::load_all(self.cache_pool())
            .await
            .ok()
            .and_then(|m| m.get(&id).copied());
        crate::connection_env::effective(explicit, name)
            == Some(crate::connection_env::Environment::Production)
    }

    /// Koneksi yang boleh dilihat klien ini, beserta level aksesnya.
    pub async fn list_connections_for(
        &self,
        client: &str,
    ) -> Result<Vec<AgentConnection>, AgentError> {
        let all = self.list_connections().await?;
        let access = access::load_all(self.cache_pool()).await?;
        let envs = crate::connection_env::load_all(self.cache_pool())
            .await
            .unwrap_or_default();
        Ok(all
            .into_iter()
            .filter_map(|summary| {
                let acc = access.get(&summary.id).cloned().unwrap_or_default();
                if acc.level == AccessLevel::Blocked || !acc.client_allowed(client) {
                    return None;
                }
                let environment =
                    crate::connection_env::effective(envs.get(&summary.id).copied(), &summary.name)
                        .map(|e| e.key());
                Some(AgentConnection {
                    writes_allowed: acc.level.allows_writes() && summary.supports_query,
                    access: acc.level,
                    environment,
                    summary,
                })
            })
            .collect())
    }

    /// Susun rencana eksekusi tanpa menjalankan apa pun.
    pub async fn plan_statement(
        &self,
        client: &str,
        id: i64,
        sql: &str,
        database: Option<&str>,
    ) -> Result<WritePlan, AgentError> {
        let acc = self.ensure_access(client, id).await?;
        let conn = self.load_connection(id).await?;
        if !matches!(
            conn.connection_type,
            DatabaseType::MySQL
                | DatabaseType::PostgreSQL
                | DatabaseType::SQLite
                | DatabaseType::MsSQL
                | DatabaseType::Redis
        ) {
            return Err(AgentError::Unsupported(
                id,
                kind_label(&conn.connection_type).to_string(),
            ));
        }
        let parts = classify::classify_query(&conn.connection_type, sql);
        if parts.is_empty() {
            return Err(AgentError::Refused("empty statement".to_string()));
        }
        let production = self.is_production(id, &conn.name).await;
        let mut decisions = Vec::new();
        let mut statements = Vec::new();
        for (statement, kind) in parts {
            let risk = StatementRisk {
                unsafe_dml: crate::safety_guard::analyze_safety(&statement).map(|r| {
                    format!(
                        "{} without WHERE on {}",
                        r.statement_type,
                        r.table_name.unwrap_or_else(|| "unknown table".to_string())
                    )
                }),
                destructive: access::is_destructive(&statement),
                production,
            };
            let d = access::decide(acc.level, kind, &risk);
            let (label, reason) = match &d {
                Decision::Allow => ("allow", None),
                Decision::NeedsApproval(r) => ("needs_approval", Some(r.clone())),
                Decision::Deny(r) => ("deny", Some(r.clone())),
            };
            statements.push(PlannedStatement {
                statement,
                kind,
                decision: label,
                reason,
            });
            decisions.push(d);
        }
        Ok(WritePlan {
            connection_id: id,
            connection_name: conn.name,
            database: database
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string),
            level: acc.level,
            sql: sql.to_string(),
            statements,
            decision: aggregate(&decisions),
        })
    }

    /// Jalankan rencana yang sudah diizinkan (atau disetujui user).
    pub async fn execute_plan(
        &self,
        plan: &WritePlan,
        max_rows: Option<usize>,
    ) -> Result<AgentQueryResult, AgentError> {
        if let Decision::Deny(r) = &plan.decision {
            return Err(AgentError::Refused(r.clone()));
        }
        log::info!(
            "[AGENT] executing {} statement(s) on connection {} (level {})",
            plan.statements.len(),
            plan.connection_id,
            plan.level.key()
        );
        self.execute_unchecked(
            plan.connection_id,
            &plan.sql,
            plan.database.as_deref(),
            max_rows,
        )
        .await
    }

    /// Nama tabel persis seperti di cache (pencocokan tanpa huruf besar/kecil),
    /// supaya tool yang menyusun SQL hanya memakai tabel yang benar-benar ada.
    async fn resolve_table(
        &self,
        id: i64,
        db: &str,
        table: &str,
    ) -> Result<(String, String), AgentError> {
        let tables = self.cached_tables(id, db).await?;
        let wanted = table.trim();
        tables
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(wanted))
            .ok_or_else(|| {
                AgentError::Refused(format!(
                    "table {wanted} not found in database {db}; call list_tables (or refresh_schema_cache if it was just created)"
                ))
            })
    }

    pub async fn list_tables(
        &self,
        id: i64,
        database: Option<&str>,
        pattern: Option<&str>,
        limit: Option<usize>,
    ) -> Result<TableList, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let mut tables = self.cached_tables(id, &db).await?;
        if tables.is_empty() {
            self.refresh_schema_cache(id).await?;
            tables = self.cached_tables(id, &db).await?;
        }
        if let Some(p) = pattern.map(str::trim).filter(|p| !p.is_empty()) {
            let p = p.to_ascii_lowercase();
            tables.retain(|(n, _)| n.to_ascii_lowercase().contains(&p));
        }
        let total = tables.len();
        let limit = limit.unwrap_or(500).clamp(1, 2000);
        Ok(TableList {
            connection_id: id,
            database: db,
            total,
            truncated: total > limit,
            tables: tables
                .into_iter()
                .take(limit)
                .map(|(name, kind)| TableListEntry { name, kind })
                .collect(),
        })
    }

    pub async fn describe_table(
        &self,
        id: i64,
        database: Option<&str>,
        table: &str,
    ) -> Result<TableDetail, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let (name, kind) = self.resolve_table(id, &db, table).await?;
        let pool = self.cache_pool();
        let cols: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT column_name, data_type, COALESCE(is_primary_key, 0) FROM column_cache \
             WHERE connection_id = ? AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE \
             ORDER BY ordinal_position",
        )
        .bind(id)
        .bind(&db)
        .bind(&name)
        .fetch_all(pool)
        .await?;
        let fks: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT column_name, referenced_table_name, referenced_column_name FROM foreign_key_cache \
             WHERE connection_id = ? AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE",
        )
        .bind(id)
        .bind(&db)
        .bind(&name)
        .fetch_all(pool)
        .await?;
        let incoming: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT table_name, column_name, referenced_column_name FROM foreign_key_cache \
             WHERE connection_id = ? AND database_name = ? COLLATE NOCASE \
               AND referenced_table_name = ? COLLATE NOCASE",
        )
        .bind(id)
        .bind(&db)
        .bind(&name)
        .fetch_all(pool)
        .await?;
        let indexes = self.cached_indexes(id, &db, &name).await;
        Ok(TableDetail {
            connection_id: id,
            database: db,
            name,
            kind,
            columns: cols
                .into_iter()
                .map(|(name, data_type, pk)| ColumnDescription {
                    name,
                    data_type,
                    primary_key: pk != 0,
                })
                .collect(),
            foreign_keys: fks
                .into_iter()
                .map(
                    |(column, references_table, references_column)| ForeignKeyDescription {
                        column,
                        references_table,
                        references_column,
                    },
                )
                .collect(),
            referenced_by: incoming
                .into_iter()
                .map(|(table, column, references_column)| IncomingReference {
                    table,
                    column,
                    references_column,
                })
                .collect(),
            indexes,
        })
    }

    /// DDL tabel: dari server untuk MySQL/SQLite, disusun dari cache untuk
    /// engine lain.
    pub async fn table_ddl(
        &self,
        id: i64,
        database: Option<&str>,
        table: &str,
    ) -> Result<TableDdl, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let (name, _) = self.resolve_table(id, &db, table).await?;
        let server_sql = match conn.connection_type {
            DatabaseType::MySQL => Some(format!(
                "SHOW CREATE TABLE {}.{}",
                quote_ident(&DatabaseType::MySQL, &db),
                quote_ident(&DatabaseType::MySQL, &name)
            )),
            DatabaseType::SQLite => Some(format!(
                "SELECT sql FROM sqlite_master WHERE name = {}",
                sql_string(&name)
            )),
            _ => None,
        };
        if let Some(q) = server_sql {
            let res = self.run_query(id, &q, Some(&db), Some(5)).await?;
            let ddl = res
                .rows
                .first()
                .and_then(|r| r.last())
                .cloned()
                .unwrap_or_default();
            if !ddl.trim().is_empty() {
                return Ok(TableDdl {
                    connection_id: id,
                    database: db,
                    table: name,
                    ddl,
                    source: "server",
                });
            }
        }
        let detail = self.describe_table(id, Some(&db), &name).await?;
        Ok(TableDdl {
            connection_id: id,
            database: db,
            table: name,
            ddl: ddl_from_detail(&conn.connection_type, &detail),
            source: "cache",
        })
    }

    pub async fn sample_rows(
        &self,
        id: i64,
        database: Option<&str>,
        table: &str,
        limit: Option<usize>,
    ) -> Result<AgentQueryResult, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let (name, _) = self.resolve_table(id, &db, table).await?;
        let limit = limit.unwrap_or(20).clamp(1, self.limits.max_rows.max(1));
        let sql = sample_sql(&conn.connection_type, &name, limit);
        self.run_query(id, &sql, Some(&db), Some(limit)).await
    }

    pub async fn count_rows(
        &self,
        id: i64,
        database: Option<&str>,
        table: &str,
    ) -> Result<RowCount, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let (name, _) = self.resolve_table(id, &db, table).await?;
        let sql = format!(
            "SELECT COUNT(*) FROM {}",
            quote_ident(&conn.connection_type, &name)
        );
        let res = self.run_query(id, &sql, Some(&db), Some(1)).await?;
        let rows = res
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.trim().parse::<i64>().ok())
            .ok_or_else(|| AgentError::Query("COUNT(*) returned no number".to_string()))?;
        Ok(RowCount {
            connection_id: id,
            database: db,
            table: name,
            rows,
        })
    }

    /// Query yang sedang berjalan di server (PostgreSQL, MySQL, SQL Server).
    pub async fn running_queries(&self, id: i64) -> Result<Vec<RunningQuery>, AgentError> {
        let (conn, pool) = self.pool_for(id).await?;
        if !matches!(
            conn.connection_type,
            DatabaseType::PostgreSQL | DatabaseType::MySQL | DatabaseType::MsSQL
        ) {
            return Err(AgentError::Unsupported(
                id,
                kind_label(&conn.connection_type).to_string(),
            ));
        }
        let procs = crate::dba_monitor::fetch_dba_processes(&pool, &conn.connection_type)
            .await
            .map_err(AgentError::Query)?;
        Ok(procs
            .into_iter()
            .map(|p| RunningQuery {
                pid: p.pid,
                user: p.user,
                database: p.db,
                state: p.state,
                duration_secs: p.duration_secs,
                wait_event: p.wait_event,
                query: p.query.chars().take(self.limits.max_cell_chars).collect(),
                blocked_by: p.blocked_by,
                is_blocking: p.is_blocking,
            })
            .collect())
    }

    /// SQL untuk membatalkan query / mengakhiri sesi. Dijalankan lewat
    /// [`Self::plan_statement`] sehingga selalu butuh persetujuan (admin).
    pub async fn cancel_sql(
        &self,
        id: i64,
        pid: i64,
        terminate: bool,
    ) -> Result<String, AgentError> {
        let conn = self.load_connection(id).await?;
        let sql = if terminate {
            crate::dba_monitor::get_kill_query(&conn.connection_type, pid)
        } else {
            crate::dba_monitor::get_cancel_query(&conn.connection_type, pid)
        };
        sql.ok_or_else(|| {
            AgentError::Unsupported(id, kind_label(&conn.connection_type).to_string())
        })
    }
}

/// DDL perkiraan dari cache (PostgreSQL/SQL Server/fallback).
pub fn ddl_from_detail(db: &DatabaseType, t: &TableDetail) -> String {
    let mut out = format!(
        "-- Reconstructed from Tabular's schema cache: no defaults, checks or storage options\n\
         CREATE TABLE {} (\n",
        quote_ident(db, &t.name)
    );
    let mut lines: Vec<String> = t
        .columns
        .iter()
        .map(|c| format!("  {} {}", quote_ident(db, &c.name), c.data_type))
        .collect();
    let pk: Vec<String> = t
        .columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| quote_ident(db, &c.name))
        .collect();
    if !pk.is_empty() {
        lines.push(format!("  PRIMARY KEY ({})", pk.join(", ")));
    }
    for fk in &t.foreign_keys {
        lines.push(format!(
            "  FOREIGN KEY ({}) REFERENCES {} ({})",
            quote_ident(db, &fk.column),
            quote_ident(db, &fk.references_table),
            quote_ident(db, &fk.references_column)
        ));
    }
    out.push_str(&lines.join(",\n"));
    out.push_str("\n);\n");
    for idx in &t.indexes {
        if idx.columns.is_empty() {
            continue;
        }
        let cols: Vec<String> = idx.columns.iter().map(|c| quote_ident(db, c)).collect();
        out.push_str(&format!(
            "CREATE {}INDEX {} ON {} ({});\n",
            if idx.unique { "UNIQUE " } else { "" },
            quote_ident(db, &idx.name),
            quote_ident(db, &t.name),
            cols.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_decisions() {
        assert_eq!(
            aggregate(&[Decision::Allow, Decision::Allow]),
            Decision::Allow
        );
        assert_eq!(
            aggregate(&[
                Decision::Allow,
                Decision::NeedsApproval("a".into()),
                Decision::NeedsApproval("a".into()),
                Decision::NeedsApproval("b".into()),
            ]),
            Decision::NeedsApproval("a; b".into())
        );
        assert_eq!(
            aggregate(&[
                Decision::NeedsApproval("a".into()),
                Decision::Deny("no".into())
            ]),
            Decision::Deny("no".into())
        );
    }

    #[test]
    fn quoting_per_dialect() {
        assert_eq!(quote_ident(&DatabaseType::MySQL, "a`b"), "`a``b`");
        assert_eq!(
            quote_ident(&DatabaseType::PostgreSQL, "public.Orders"),
            "\"public\".\"Orders\""
        );
        assert_eq!(quote_ident(&DatabaseType::MsSQL, "dbo.x]y"), "[dbo].[x]]y]");
        assert_eq!(
            quote_ident(&DatabaseType::SQLite, "we\"ird"),
            "\"we\"\"ird\""
        );
        assert_eq!(
            sample_sql(&DatabaseType::MsSQL, "dbo.t", 5),
            "SELECT TOP (5) * FROM [dbo].[t]"
        );
        assert_eq!(
            sample_sql(&DatabaseType::PostgreSQL, "t", 5),
            "SELECT * FROM \"t\" LIMIT 5"
        );
    }

    #[test]
    fn ddl_reconstruction() {
        let t = TableDetail {
            connection_id: 1,
            database: "shop".into(),
            name: "orders".into(),
            kind: "table".into(),
            columns: vec![
                ColumnDescription {
                    name: "id".into(),
                    data_type: "bigint".into(),
                    primary_key: true,
                },
                ColumnDescription {
                    name: "customer_id".into(),
                    data_type: "bigint".into(),
                    primary_key: false,
                },
            ],
            foreign_keys: vec![ForeignKeyDescription {
                column: "customer_id".into(),
                references_table: "customers".into(),
                references_column: "id".into(),
            }],
            referenced_by: vec![],
            indexes: vec![IndexDescription {
                name: "idx_orders_customer".into(),
                columns: vec!["customer_id".into()],
                unique: false,
                method: None,
            }],
        };
        let ddl = ddl_from_detail(&DatabaseType::PostgreSQL, &t);
        assert!(ddl.contains("CREATE TABLE \"orders\""));
        assert!(ddl.contains("PRIMARY KEY (\"id\")"));
        assert!(ddl.contains("REFERENCES \"customers\" (\"id\")"));
        assert!(
            ddl.contains("CREATE INDEX \"idx_orders_customer\" ON \"orders\" (\"customer_id\");")
        );
    }
}
