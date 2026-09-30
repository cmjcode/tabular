//! Kontrol akses agent per koneksi (checklist K1, K2 sebagian, K4).
//!
//! Semua state disimpan di `connections.db` pada tabel terpisah (bukan kolom
//! `connections`) supaya tidak ikut sync/import-export dan tidak menyentuh
//! skema milik GUI; konsekuensinya pengaturan ini lokal per mesin, sama seperti
//! environment koneksi (`crate::connection_env`).
//!
//! Tabel:
//! - `agent_connection_access`: level akses + allowlist klien per koneksi.
//! - `agent_clients`: klien MCP yang pernah terhubung (nama dari
//!   `clientInfo` saat initialize). Nama ini dilaporkan sendiri oleh klien,
//!   jadi allowlist adalah pagar kenyamanan, **bukan** batas keamanan.
//! - `agent_approvals`: antrean persetujuan statement tulis. Proses
//!   `tabular mcp` menulis baris `pending`, GUI menampilkan dialog dan mengisi
//!   keputusan; proses MCP menunggu dengan polling.
//! - `agent_activity`: log setiap panggilan tool/resource/prompt. Statement
//!   dicatat sebagai digest SHA-256, bukan teks (teks lengkap untuk query yang
//!   benar-benar dijalankan tetap ada di `query_history` bertanda `(agent)`).
//!
//! Modul ini headless: tidak bergantung pada egui dan dipakai bersama oleh
//! server MCP dan GUI.

use std::collections::HashMap;

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use super::classify::StatementKind;

/// Level akses agent untuk satu koneksi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    /// Koneksi disembunyikan dari agent; semua panggilan ditolak.
    Blocked,
    /// Hanya membaca (perilaku bawaan sejak awal).
    #[default]
    ReadOnly,
    /// Boleh menulis, tetapi setiap statement non-read harus disetujui user.
    Ask,
    /// DML (INSERT/UPDATE/DELETE) langsung jalan; DDL dan admin minta persetujuan.
    Edit,
    /// DML dan DDL langsung jalan; perintah admin/tak dikenal minta persetujuan.
    Agent,
}

impl AccessLevel {
    pub const ALL: [AccessLevel; 5] = [
        AccessLevel::Blocked,
        AccessLevel::ReadOnly,
        AccessLevel::Ask,
        AccessLevel::Edit,
        AccessLevel::Agent,
    ];

    pub fn key(self) -> &'static str {
        match self {
            AccessLevel::Blocked => "blocked",
            AccessLevel::ReadOnly => "read_only",
            AccessLevel::Ask => "ask",
            AccessLevel::Edit => "edit",
            AccessLevel::Agent => "agent",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|l| l.key().eq_ignore_ascii_case(key.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            AccessLevel::Blocked => "Blocked",
            AccessLevel::ReadOnly => "Read only",
            AccessLevel::Ask => "Ask",
            AccessLevel::Edit => "Edit",
            AccessLevel::Agent => "Agent",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            AccessLevel::Blocked => "Hidden from agents; every call is refused.",
            AccessLevel::ReadOnly => "Schema and SELECT only. Writes are refused.",
            AccessLevel::Ask => "Writes are allowed after you approve each statement in Tabular.",
            AccessLevel::Edit => {
                "INSERT/UPDATE/DELETE run directly; DDL and admin commands need your approval."
            }
            AccessLevel::Agent => {
                "Data and schema changes run directly; admin commands and risky statements need your approval."
            }
        }
    }

    pub fn allows_writes(self) -> bool {
        matches!(
            self,
            AccessLevel::Ask | AccessLevel::Edit | AccessLevel::Agent
        )
    }
}

/// Keputusan untuk satu statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Boleh jalan setelah user menyetujui; berisi alasan yang ditampilkan.
    NeedsApproval(String),
    Deny(String),
}

/// Fakta tambahan tentang statement yang memengaruhi keputusan.
#[derive(Debug, Clone, Default)]
pub struct StatementRisk {
    /// UPDATE/DELETE tanpa WHERE (dari `safety_guard`).
    pub unsafe_dml: Option<String>,
    /// DROP DATABASE/SCHEMA/TABLE, TRUNCATE, FLUSHALL, dan sejenisnya.
    pub destructive: bool,
    /// Koneksi bertanda (atau ditebak) Production.
    pub production: bool,
}

/// Aturan inti K1. Murni supaya mudah diuji.
pub fn decide(level: AccessLevel, kind: StatementKind, risk: &StatementRisk) -> Decision {
    if level == AccessLevel::Blocked {
        return Decision::Deny("this connection is blocked for agents in Tabular".to_string());
    }
    if kind == StatementKind::Read {
        return Decision::Allow;
    }
    if !level.allows_writes() {
        return Decision::Deny(format!(
            "agent access to this connection is read-only; {} statements are not allowed. \
             Ask the user to run it in the Tabular app or to raise the connection's agent access level.",
            kind_label(kind)
        ));
    }
    // Risiko tinggi selalu minta persetujuan, apa pun levelnya.
    if let Some(what) = &risk.unsafe_dml {
        return Decision::NeedsApproval(format!("{what} affects every row"));
    }
    if risk.destructive {
        return Decision::NeedsApproval("destructive statement (DROP/TRUNCATE)".to_string());
    }
    if risk.production {
        return Decision::NeedsApproval(format!(
            "{} statement on a Production connection",
            kind_label(kind)
        ));
    }
    let direct = match level {
        AccessLevel::Edit => kind == StatementKind::Write,
        AccessLevel::Agent => matches!(kind, StatementKind::Write | StatementKind::Ddl),
        _ => false,
    };
    if direct {
        Decision::Allow
    } else {
        Decision::NeedsApproval(format!("{} statement", kind_label(kind)))
    }
}

pub fn kind_label(kind: StatementKind) -> &'static str {
    match kind {
        StatementKind::Read => "read",
        StatementKind::Write => "write",
        StatementKind::Ddl => "DDL",
        StatementKind::Admin => "admin",
        StatementKind::Unknown => "unrecognized",
    }
}

/// Deteksi statement destruktif sederhana dari kata-kata awal statement
/// tunggal (sudah dipisah oleh `classify_query`).
pub fn is_destructive(statement: &str) -> bool {
    let words: Vec<String> = statement
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .filter(|w| !w.is_empty())
        .take(3)
        .map(|w| w.to_ascii_uppercase())
        .collect();
    match words.first().map(String::as_str) {
        Some("TRUNCATE") => true,
        Some("DROP") => matches!(
            words.get(1).map(String::as_str),
            Some("DATABASE" | "SCHEMA" | "TABLE" | "USER" | "ROLE" | "OWNED")
        ),
        Some("FLUSHALL" | "FLUSHDB") => true,
        _ => false,
    }
}

/// Digest SHA-256 (hex) untuk log aktivitas.
pub fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.trim().as_bytes()))
}

// ── Penyimpanan ─────────────────────────────────────────────────────────────

/// Pengaturan akses satu koneksi.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, schemars::JsonSchema)]
pub struct ConnectionAccess {
    pub level: AccessLevel,
    /// `None` = semua klien boleh; `Some(list)` = hanya klien bernama ini.
    pub allowed_clients: Option<Vec<String>>,
}

impl ConnectionAccess {
    pub fn client_allowed(&self, client: &str) -> bool {
        match &self.allowed_clients {
            None => true,
            Some(list) => list.iter().any(|c| c.eq_ignore_ascii_case(client)),
        }
    }
}

pub async fn ensure_tables(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    for sql in [
        "CREATE TABLE IF NOT EXISTS agent_connection_access (\
            connection_id INTEGER PRIMARY KEY, \
            level TEXT NOT NULL, \
            allowed_clients TEXT NULL)",
        "CREATE TABLE IF NOT EXISTS agent_clients (\
            name TEXT PRIMARY KEY, \
            version TEXT NOT NULL DEFAULT '', \
            first_seen DATETIME DEFAULT CURRENT_TIMESTAMP, \
            last_seen DATETIME DEFAULT CURRENT_TIMESTAMP, \
            sessions INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE IF NOT EXISTS agent_approvals (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP, \
            client TEXT NOT NULL, \
            connection_id INTEGER NOT NULL, \
            connection_name TEXT NOT NULL, \
            database_name TEXT NOT NULL DEFAULT '', \
            kind TEXT NOT NULL, \
            reason TEXT NOT NULL, \
            statement TEXT NOT NULL, \
            status TEXT NOT NULL DEFAULT 'pending', \
            seen_at DATETIME NULL, \
            decided_at DATETIME NULL)",
        "CREATE TABLE IF NOT EXISTS agent_activity (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            at DATETIME DEFAULT CURRENT_TIMESTAMP, \
            client TEXT NOT NULL, \
            category TEXT NOT NULL, \
            name TEXT NOT NULL, \
            connection_id INTEGER NULL, \
            statement_digest TEXT NULL, \
            statement_kind TEXT NULL, \
            outcome TEXT NOT NULL, \
            duration_ms INTEGER NOT NULL DEFAULT 0, \
            detail TEXT NOT NULL DEFAULT '')",
        "CREATE INDEX IF NOT EXISTS idx_agent_activity_at ON agent_activity(at)",
        "CREATE INDEX IF NOT EXISTS idx_agent_approvals_status ON agent_approvals(status)",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}

fn parse_clients(raw: Option<String>) -> Option<Vec<String>> {
    let raw = raw?;
    serde_json::from_str::<Vec<String>>(&raw).ok()
}

pub async fn load_all(pool: &SqlitePool) -> Result<HashMap<i64, ConnectionAccess>, sqlx::Error> {
    ensure_tables(pool).await?;
    let rows: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT connection_id, level, allowed_clients FROM agent_connection_access")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, level, clients)| {
            (
                id,
                ConnectionAccess {
                    level: AccessLevel::parse(&level).unwrap_or_default(),
                    allowed_clients: parse_clients(clients),
                },
            )
        })
        .collect())
}

pub async fn load(pool: &SqlitePool, connection_id: i64) -> Result<ConnectionAccess, sqlx::Error> {
    ensure_tables(pool).await?;
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT level, allowed_clients FROM agent_connection_access WHERE connection_id = ?",
    )
    .bind(connection_id)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|(level, clients)| ConnectionAccess {
            level: AccessLevel::parse(&level).unwrap_or_default(),
            allowed_clients: parse_clients(clients),
        })
        .unwrap_or_default())
}

pub async fn save(
    pool: &SqlitePool,
    connection_id: i64,
    access: &ConnectionAccess,
) -> Result<(), sqlx::Error> {
    ensure_tables(pool).await?;
    if *access == ConnectionAccess::default() {
        sqlx::query("DELETE FROM agent_connection_access WHERE connection_id = ?")
            .bind(connection_id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    let clients = access
        .allowed_clients
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap_or_else(|_| "[]".to_string()));
    sqlx::query(
        "INSERT INTO agent_connection_access (connection_id, level, allowed_clients) VALUES (?, ?, ?) \
         ON CONFLICT(connection_id) DO UPDATE SET level = excluded.level, \
         allowed_clients = excluded.allowed_clients",
    )
    .bind(connection_id)
    .bind(access.level.key())
    .bind(clients)
    .execute(pool)
    .await?;
    Ok(())
}

/// Klien MCP yang pernah terlihat.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct KnownClient {
    pub name: String,
    pub version: String,
    pub first_seen: String,
    pub last_seen: String,
    pub sessions: i64,
}

/// Catat klien yang baru melakukan initialize.
pub async fn record_client(
    pool: &SqlitePool,
    name: &str,
    version: &str,
) -> Result<(), sqlx::Error> {
    ensure_tables(pool).await?;
    sqlx::query(
        "INSERT INTO agent_clients (name, version, sessions) VALUES (?, ?, 1) \
         ON CONFLICT(name) DO UPDATE SET version = excluded.version, \
         last_seen = CURRENT_TIMESTAMP, sessions = sessions + 1",
    )
    .bind(name)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_clients(pool: &SqlitePool) -> Result<Vec<KnownClient>, sqlx::Error> {
    ensure_tables(pool).await?;
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT name, version, COALESCE(first_seen, ''), COALESCE(last_seen, ''), sessions \
         FROM agent_clients ORDER BY last_seen DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(name, version, first_seen, last_seen, sessions)| KnownClient {
                name,
                version,
                first_seen,
                last_seen,
                sessions,
            },
        )
        .collect())
}

/// "Forget": hapus klien dari daftar dan dari semua allowlist koneksi.
/// Allowlist yang menjadi kosong tetap kosong (tidak ada klien diizinkan),
/// bukan kembali ke "semua klien".
pub async fn forget_client(pool: &SqlitePool, name: &str) -> Result<(), sqlx::Error> {
    ensure_tables(pool).await?;
    sqlx::query("DELETE FROM agent_clients WHERE name = ? COLLATE NOCASE")
        .bind(name)
        .execute(pool)
        .await?;
    for (id, mut access) in load_all(pool).await? {
        if let Some(list) = access.allowed_clients.as_mut() {
            let before = list.len();
            list.retain(|c| !c.eq_ignore_ascii_case(name));
            if list.len() != before {
                save(pool, id, &access).await?;
            }
        }
    }
    Ok(())
}

// ── Antrean persetujuan ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

impl ApprovalStatus {
    pub fn key(self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "approved" => ApprovalStatus::Approved,
            "denied" => ApprovalStatus::Denied,
            "expired" => ApprovalStatus::Expired,
            _ => ApprovalStatus::Pending,
        }
    }
}

/// Permintaan persetujuan yang menunggu keputusan user.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub id: i64,
    pub created_at: String,
    pub client: String,
    pub connection_id: i64,
    pub connection_name: String,
    pub database: String,
    pub kind: String,
    pub reason: String,
    pub statement: String,
}

pub struct NewApproval<'a> {
    pub client: &'a str,
    pub connection_id: i64,
    pub connection_name: &'a str,
    pub database: &'a str,
    pub kind: &'a str,
    pub reason: &'a str,
    pub statement: &'a str,
}

/// Jumlah permintaan yang masih menunggu, tanpa menandai `seen_at`
/// (dipakai pemantau latar GUI untuk membangunkan UI).
pub async fn pending_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    ensure_tables(pool).await?;
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM agent_approvals WHERE status = 'pending'")
            .fetch_one(pool)
            .await?;
    Ok(n)
}

pub async fn request_approval(
    pool: &SqlitePool,
    req: &NewApproval<'_>,
) -> Result<i64, sqlx::Error> {
    ensure_tables(pool).await?;
    let res = sqlx::query(
        "INSERT INTO agent_approvals (client, connection_id, connection_name, database_name, kind, reason, statement) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(req.client)
    .bind(req.connection_id)
    .bind(req.connection_name)
    .bind(req.database)
    .bind(req.kind)
    .bind(req.reason)
    .bind(req.statement)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Status permintaan dan apakah GUI sudah menampilkannya.
pub async fn approval_status(
    pool: &SqlitePool,
    id: i64,
) -> Result<(ApprovalStatus, bool), sqlx::Error> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status, seen_at FROM agent_approvals WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row
        .map(|(s, seen)| (ApprovalStatus::parse(&s), seen.is_some()))
        .unwrap_or((ApprovalStatus::Expired, false)))
}

/// Tulis keputusan. Hanya permintaan yang masih `pending` yang berubah,
/// sehingga keputusan terlambat untuk permintaan kedaluwarsa diabaikan.
pub async fn decide_approval(
    pool: &SqlitePool,
    id: i64,
    status: ApprovalStatus,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE agent_approvals SET status = ?, decided_at = CURRENT_TIMESTAMP \
         WHERE id = ? AND status = 'pending'",
    )
    .bind(status.key())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

type ApprovalTuple = (
    i64,
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
);

/// Permintaan yang masih menunggu (untuk GUI). Sekaligus menandai `seen_at`
/// supaya proses MCP tahu GUI sedang berjalan dan menampilkannya.
pub async fn pending_approvals(pool: &SqlitePool) -> Result<Vec<ApprovalRequest>, sqlx::Error> {
    ensure_tables(pool).await?;
    // Proses MCP yang mati saat menunggu meninggalkan baris `pending`; setelah
    // batas tunggu MCP (180 dtk) plus jeda, permintaan itu tidak bisa lagi
    // dijalankan, jadi jangan ditampilkan.
    sqlx::query(
        "UPDATE agent_approvals SET status = 'expired', decided_at = CURRENT_TIMESTAMP \
         WHERE status = 'pending' AND created_at < datetime('now', '-200 seconds')",
    )
    .execute(pool)
    .await?;
    let rows: Vec<ApprovalTuple> = sqlx::query_as(
        "SELECT id, COALESCE(created_at, ''), client, connection_id, connection_name, database_name, \
                kind, reason, statement \
         FROM agent_approvals WHERE status = 'pending' ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    if !rows.is_empty() {
        sqlx::query(
            "UPDATE agent_approvals SET seen_at = CURRENT_TIMESTAMP \
             WHERE status = 'pending' AND seen_at IS NULL",
        )
        .execute(pool)
        .await?;
    }
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                created_at,
                client,
                connection_id,
                connection_name,
                database,
                kind,
                reason,
                statement,
            )| {
                ApprovalRequest {
                    id,
                    created_at,
                    client,
                    connection_id,
                    connection_name,
                    database,
                    kind,
                    reason,
                    statement,
                }
            },
        )
        .collect())
}

// ── Log aktivitas ───────────────────────────────────────────────────────────

/// Retensi log aktivitas.
const ACTIVITY_RETENTION_DAYS: i64 = 90;
const ACTIVITY_MAX_ROWS: i64 = 20_000;

#[derive(Debug, Clone, Default)]
pub struct ActivityEntry {
    pub client: String,
    /// `tool`, `resource`, `prompt`, `session`.
    pub category: String,
    pub name: String,
    pub connection_id: Option<i64>,
    pub statement_digest: Option<String>,
    pub statement_kind: Option<String>,
    /// `ok`, `error`, `refused`, `approved`, `denied`, `expired`.
    pub outcome: String,
    pub duration_ms: i64,
    pub detail: String,
}

pub async fn log_activity(pool: &SqlitePool, e: &ActivityEntry) -> Result<(), sqlx::Error> {
    ensure_tables(pool).await?;
    let detail: String = e.detail.chars().take(500).collect();
    sqlx::query(
        "INSERT INTO agent_activity (client, category, name, connection_id, statement_digest, \
         statement_kind, outcome, duration_ms, detail) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&e.client)
    .bind(&e.category)
    .bind(&e.name)
    .bind(e.connection_id)
    .bind(&e.statement_digest)
    .bind(&e.statement_kind)
    .bind(&e.outcome)
    .bind(e.duration_ms)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

/// Buang log lama (dipanggil sekali per sesi MCP).
pub async fn prune_activity(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    ensure_tables(pool).await?;
    sqlx::query("DELETE FROM agent_activity WHERE at < datetime('now', ?)")
        .bind(format!("-{ACTIVITY_RETENTION_DAYS} days"))
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM agent_activity WHERE id <= \
         (SELECT id FROM agent_activity ORDER BY id DESC LIMIT 1 OFFSET ?)",
    )
    .bind(ACTIVITY_MAX_ROWS)
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM agent_approvals WHERE status != 'pending' \
         AND created_at < datetime('now', '-7 days')",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ActivityRow {
    pub id: i64,
    pub at: String,
    pub client: String,
    pub category: String,
    pub name: String,
    pub connection_id: Option<i64>,
    pub statement_digest: Option<String>,
    pub statement_kind: Option<String>,
    pub outcome: String,
    pub duration_ms: i64,
    pub detail: String,
}

type ActivityTuple = (
    i64,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
    i64,
    String,
);

pub async fn recent_activity(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<ActivityRow>, sqlx::Error> {
    ensure_tables(pool).await?;
    let rows: Vec<ActivityTuple> = sqlx::query_as(
        "SELECT id, COALESCE(at, ''), client, category, name, connection_id, statement_digest, \
                statement_kind, outcome, duration_ms, detail \
         FROM agent_activity ORDER BY id DESC LIMIT ?",
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                at,
                client,
                category,
                name,
                connection_id,
                statement_digest,
                statement_kind,
                outcome,
                duration_ms,
                detail,
            )| ActivityRow {
                id,
                at,
                client,
                category,
                name,
                connection_id,
                statement_digest,
                statement_kind,
                outcome,
                duration_ms,
                detail,
            },
        )
        .collect())
}

pub async fn clear_activity(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    ensure_tables(pool).await?;
    sqlx::query("DELETE FROM agent_activity")
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn risk() -> StatementRisk {
        StatementRisk::default()
    }

    #[test]
    fn read_only_refuses_writes_and_allows_reads() {
        let r = risk();
        assert_eq!(
            decide(AccessLevel::ReadOnly, StatementKind::Read, &r),
            Decision::Allow
        );
        assert!(matches!(
            decide(AccessLevel::ReadOnly, StatementKind::Write, &r),
            Decision::Deny(_)
        ));
        assert!(matches!(
            decide(AccessLevel::Blocked, StatementKind::Read, &r),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn levels_escalate_as_documented() {
        let r = risk();
        use StatementKind::*;
        assert!(matches!(
            decide(AccessLevel::Ask, Write, &r),
            Decision::NeedsApproval(_)
        ));
        assert_eq!(decide(AccessLevel::Edit, Write, &r), Decision::Allow);
        assert!(matches!(
            decide(AccessLevel::Edit, Ddl, &r),
            Decision::NeedsApproval(_)
        ));
        assert_eq!(decide(AccessLevel::Agent, Ddl, &r), Decision::Allow);
        assert!(matches!(
            decide(AccessLevel::Agent, Admin, &r),
            Decision::NeedsApproval(_)
        ));
        assert!(matches!(
            decide(AccessLevel::Agent, Unknown, &r),
            Decision::NeedsApproval(_)
        ));
    }

    #[test]
    fn risky_statements_always_need_approval() {
        use StatementKind::*;
        let unsafe_dml = StatementRisk {
            unsafe_dml: Some("DELETE without WHERE on t".into()),
            ..risk()
        };
        assert!(matches!(
            decide(AccessLevel::Agent, Write, &unsafe_dml),
            Decision::NeedsApproval(_)
        ));
        let prod = StatementRisk {
            production: true,
            ..risk()
        };
        assert!(matches!(
            decide(AccessLevel::Edit, Write, &prod),
            Decision::NeedsApproval(_)
        ));
        // Read tetap bebas di Production.
        assert_eq!(decide(AccessLevel::Edit, Read, &prod), Decision::Allow);
    }

    #[test]
    fn destructive_detection() {
        assert!(is_destructive("TRUNCATE orders"));
        assert!(is_destructive("drop table users"));
        assert!(is_destructive("DROP DATABASE shop"));
        assert!(!is_destructive("DROP INDEX idx_a"));
        assert!(!is_destructive("DELETE FROM t WHERE id = 1"));
        assert!(is_destructive("FLUSHALL"));
    }

    #[test]
    fn levels_roundtrip() {
        for l in AccessLevel::ALL {
            assert_eq!(AccessLevel::parse(l.key()), Some(l));
        }
        assert_eq!(AccessLevel::parse("nope"), None);
    }

    async fn mem_pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory pool")
    }

    #[tokio::test]
    async fn access_persists_and_forget_prunes_allowlists() {
        let pool = mem_pool().await;
        let acc = ConnectionAccess {
            level: AccessLevel::Edit,
            allowed_clients: Some(vec!["claude-code".into(), "cursor".into()]),
        };
        save(&pool, 7, &acc).await.expect("save");
        assert_eq!(load(&pool, 7).await.expect("load"), acc);
        assert_eq!(
            load(&pool, 8).await.expect("load").level,
            AccessLevel::ReadOnly
        );

        record_client(&pool, "cursor", "1.0").await.expect("record");
        record_client(&pool, "cursor", "1.1").await.expect("record");
        let clients = list_clients(&pool).await.expect("clients");
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].sessions, 2);

        forget_client(&pool, "Cursor").await.expect("forget");
        assert!(list_clients(&pool).await.expect("clients").is_empty());
        let after = load(&pool, 7).await.expect("load");
        assert_eq!(after.allowed_clients, Some(vec!["claude-code".to_string()]));
        assert!(after.client_allowed("claude-code"));
        assert!(!after.client_allowed("cursor"));

        // Kembali ke default menghapus baris.
        save(&pool, 7, &ConnectionAccess::default())
            .await
            .expect("save");
        assert!(load_all(&pool).await.expect("all").is_empty());
    }

    #[tokio::test]
    async fn approvals_flow() {
        let pool = mem_pool().await;
        let id = request_approval(
            &pool,
            &NewApproval {
                client: "claude-code",
                connection_id: 1,
                connection_name: "local",
                database: "shop",
                kind: "write",
                reason: "write statement",
                statement: "DELETE FROM t WHERE id = 1",
            },
        )
        .await
        .expect("request");
        assert_eq!(
            approval_status(&pool, id).await.expect("status"),
            (ApprovalStatus::Pending, false)
        );
        let pending = pending_approvals(&pool).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert!(
            approval_status(&pool, id).await.expect("status").1,
            "seen_at set"
        );
        assert!(
            decide_approval(&pool, id, ApprovalStatus::Approved)
                .await
                .expect("decide")
        );
        // Keputusan kedua diabaikan.
        assert!(
            !decide_approval(&pool, id, ApprovalStatus::Denied)
                .await
                .expect("decide")
        );
        assert_eq!(
            approval_status(&pool, id).await.expect("status").0,
            ApprovalStatus::Approved
        );
    }

    #[tokio::test]
    async fn stale_pending_requests_expire() {
        let pool = mem_pool().await;
        ensure_tables(&pool).await.expect("tables");
        sqlx::query(
            "INSERT INTO agent_approvals (created_at, client, connection_id, connection_name, kind, reason, statement) \
             VALUES (datetime('now', '-10 minutes'), 'c', 1, 'x', 'write', 'r', 'DELETE FROM t')",
        )
        .execute(&pool)
        .await
        .expect("insert");
        assert_eq!(pending_count(&pool).await.expect("count"), 1);
        assert!(pending_approvals(&pool).await.expect("pending").is_empty());
        assert_eq!(pending_count(&pool).await.expect("count"), 0);
        assert_eq!(
            approval_status(&pool, 1).await.expect("status").0,
            ApprovalStatus::Expired
        );
    }

    #[tokio::test]
    async fn activity_log_roundtrip() {
        let pool = mem_pool().await;
        log_activity(
            &pool,
            &ActivityEntry {
                client: "cursor".into(),
                category: "tool".into(),
                name: "run_query".into(),
                connection_id: Some(1),
                statement_digest: Some(digest("SELECT 1")),
                statement_kind: Some("read".into()),
                outcome: "ok".into(),
                duration_ms: 12,
                detail: String::new(),
            },
        )
        .await
        .expect("log");
        prune_activity(&pool).await.expect("prune");
        let rows = recent_activity(&pool, 10).await.expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].statement_digest.as_deref().map(str::len), Some(64));
        clear_activity(&pool).await.expect("clear");
        assert!(recent_activity(&pool, 10).await.expect("rows").is_empty());
    }
}
