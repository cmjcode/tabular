//! Riwayat sesi AI Assistant (K6).
//!
//! Setiap percakapan di panel AI disimpan otomatis setelah giliran selesai,
//! untuk backend HTTP API maupun CLI agent. Sesi bisa dibuka lagi (pesan
//! dipulihkan; untuk CLI agent id sesi/percakapan native ikut dipulihkan
//! sehingga agent melanjutkan konteksnya), diganti nama, dihapus, atau
//! dibersihkan semua.
//!
//! Disimpan di tabel `ai_chat_sessions` di `connections.db`, hanya lokal
//! (tidak ikut sync/export) karena teks pesan bisa berisi SQL dan data.
//! Retensi dibatasi [`MAX_SESSIONS`] sesi terbaru. Headless.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::ai_tool_calling::{ToolCallRecord, ToolCallStatus};
use crate::config::{ChatTarget, CliAgentKind};
use crate::models::structs::{AgentSession, AiChatMessage, AiChatRole};

/// Jumlah sesi yang disimpan; yang lebih lama dihapus saat menyimpan.
pub const MAX_SESSIONS: i64 = 200;
/// Panjang judul otomatis (karakter).
const TITLE_CHARS: usize = 60;

/// Bentuk pesan yang disimpan (tanpa state UI seperti live edit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
}

impl StoredMessage {
    pub fn from_chat(m: &AiChatMessage) -> Self {
        Self {
            role: match m.role {
                AiChatRole::User => "user",
                AiChatRole::Assistant => "assistant",
            }
            .to_string(),
            text: m.text.clone(),
            agent_label: m.agent_label.clone(),
            error: m.error.clone(),
            usage: m.usage.clone(),
            tool_calls: m.tool_calls.clone(),
        }
    }

    pub fn into_chat(self) -> AiChatMessage {
        let tool_calls = self
            .tool_calls
            .into_iter()
            .map(|mut r| {
                // Giliran yang terputus: jangan tampilkan tombol Approve lagi.
                if matches!(
                    r.status,
                    ToolCallStatus::AwaitingApproval | ToolCallStatus::Running
                ) {
                    r.status = ToolCallStatus::Failed;
                    r.result_summary = "Interrupted.".to_string();
                }
                r
            })
            .collect();
        AiChatMessage {
            role: if self.role == "user" {
                AiChatRole::User
            } else {
                AiChatRole::Assistant
            },
            text: self.text,
            agent_label: self.agent_label,
            error: self.error,
            usage: self.usage,
            tool_calls,
            ..Default::default()
        }
    }
}

/// Satu sesi lengkap.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatSessionRecord {
    /// 0 = belum disimpan.
    pub id: i64,
    pub title: String,
    pub target: ChatTarget,
    /// Id sesi native CLI (`--resume` / `--conversation`) beserta kind-nya.
    pub native_session: Option<AgentSession>,
    pub messages: Vec<StoredMessage>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Baris daftar riwayat (tanpa isi pesan).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatSessionSummary {
    pub id: i64,
    pub title: String,
    pub target: ChatTarget,
    pub message_count: i64,
    pub updated_at: i64,
}

/// Judul otomatis dari pesan user pertama (baris pertama, dipotong).
pub fn auto_title(messages: &[StoredMessage]) -> String {
    let first = messages
        .iter()
        .find(|m| m.role == "user" && !m.text.trim().is_empty())
        .map(|m| m.text.trim())
        .unwrap_or("New chat");
    let line = first.lines().next().unwrap_or(first).trim();
    if line.chars().count() <= TITLE_CHARS {
        line.to_string()
    } else {
        let head: String = line.chars().take(TITLE_CHARS - 1).collect();
        format!("{head}…")
    }
}

fn kind_key(kind: CliAgentKind) -> String {
    ChatTarget::Cli(kind).as_string()
}

fn kind_from_key(key: &str) -> Option<CliAgentKind> {
    match key.parse::<ChatTarget>() {
        Ok(ChatTarget::Cli(k)) => Some(k),
        _ => None,
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ai_chat_sessions (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            title TEXT NOT NULL, \
            target TEXT NOT NULL, \
            native_kind TEXT, \
            native_session_id TEXT, \
            messages_json TEXT NOT NULL, \
            message_count INTEGER NOT NULL DEFAULT 0, \
            created_at INTEGER NOT NULL, \
            updated_at INTEGER NOT NULL)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_ai_chat_sessions_updated \
         ON ai_chat_sessions(updated_at DESC)",
    )
    .execute(pool)
    .await
    .map(|_| ())
}

/// Simpan sesi (insert bila `id == 0`, selain itu update; judul yang sudah
/// ada, mis. hasil rename, tidak ditimpa). Lalu pangkas ke [`MAX_SESSIONS`].
/// Mengembalikan id.
pub async fn save(pool: &SqlitePool, s: &ChatSessionRecord) -> Result<i64, sqlx::Error> {
    ensure_table(pool).await?;
    let json = serde_json::to_string(&s.messages).unwrap_or_else(|_| "[]".into());
    let count = s.messages.len() as i64;
    let (nk, nid) = match &s.native_session {
        Some(n) => (Some(kind_key(n.kind)), Some(n.id.clone())),
        None => (None, None),
    };
    let title = if s.title.trim().is_empty() {
        auto_title(&s.messages)
    } else {
        s.title.clone()
    };
    let id = if s.id == 0 {
        sqlx::query(
            "INSERT INTO ai_chat_sessions (title, target, native_kind, native_session_id, \
             messages_json, message_count, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&title)
        .bind(s.target.as_string())
        .bind(nk)
        .bind(nid)
        .bind(json)
        .bind(count)
        .bind(s.created_at)
        .bind(s.updated_at)
        .execute(pool)
        .await?
        .last_insert_rowid()
    } else {
        let res = sqlx::query(
            "UPDATE ai_chat_sessions SET target = ?, native_kind = ?, native_session_id = ?, \
             messages_json = ?, message_count = ?, updated_at = ? WHERE id = ?",
        )
        .bind(s.target.as_string())
        .bind(nk.clone())
        .bind(nid.clone())
        .bind(&json)
        .bind(count)
        .bind(s.updated_at)
        .bind(s.id)
        .execute(pool)
        .await?;
        if res.rows_affected() == 0 {
            // Sesi terhapus (mis. "Clear all") saat masih terbuka: simpan ulang.
            sqlx::query(
                "INSERT INTO ai_chat_sessions (title, target, native_kind, native_session_id, \
                 messages_json, message_count, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&title)
            .bind(s.target.as_string())
            .bind(nk)
            .bind(nid)
            .bind(json)
            .bind(count)
            .bind(s.created_at)
            .bind(s.updated_at)
            .execute(pool)
            .await?
            .last_insert_rowid()
        } else {
            s.id
        }
    };
    prune(pool, MAX_SESSIONS).await?;
    Ok(id)
}

/// Hapus sesi di luar `keep` sesi terbaru.
pub async fn prune(pool: &SqlitePool, keep: i64) -> Result<u64, sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query(
        "DELETE FROM ai_chat_sessions WHERE id NOT IN \
         (SELECT id FROM ai_chat_sessions ORDER BY updated_at DESC, id DESC LIMIT ?)",
    )
    .bind(keep)
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
}

/// Daftar sesi terbaru lebih dulu.
pub async fn list(pool: &SqlitePool, limit: i64) -> Result<Vec<ChatSessionSummary>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows: Vec<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, title, target, message_count, updated_at FROM ai_chat_sessions \
         ORDER BY updated_at DESC, id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, title, target, message_count, updated_at)| ChatSessionSummary {
                id,
                title,
                target: target.parse().unwrap_or_default(),
                message_count,
                updated_at,
            },
        )
        .collect())
}

type SessionRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
);

pub async fn load(pool: &SqlitePool, id: i64) -> Result<Option<ChatSessionRecord>, sqlx::Error> {
    ensure_table(pool).await?;
    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT id, title, target, native_kind, native_session_id, messages_json, \
         created_at, updated_at FROM ai_chat_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, title, target, nk, nid, json, created_at, updated_at)| ChatSessionRecord {
            id,
            title,
            target: target.parse().unwrap_or_default(),
            native_session: match (nk.as_deref().and_then(kind_from_key), nid) {
                (Some(kind), Some(id)) if !id.is_empty() => Some(AgentSession { kind, id }),
                _ => None,
            },
            messages: serde_json::from_str(&json).unwrap_or_default(),
            created_at,
            updated_at,
        },
    ))
}

pub async fn rename(pool: &SqlitePool, id: i64, title: &str) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query("UPDATE ai_chat_sessions SET title = ? WHERE id = ?")
        .bind(title.trim())
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query("DELETE FROM ai_chat_sessions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn clear_all(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query("DELETE FROM ai_chat_sessions")
        .execute(pool)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    fn msg(role: &str, text: &str) -> StoredMessage {
        StoredMessage {
            role: role.into(),
            text: text.into(),
            agent_label: None,
            error: None,
            usage: None,
            tool_calls: Vec::new(),
        }
    }

    fn session(updated_at: i64) -> ChatSessionRecord {
        ChatSessionRecord {
            target: ChatTarget::Cli(CliAgentKind::ClaudeCode),
            native_session: Some(AgentSession {
                kind: CliAgentKind::ClaudeCode,
                id: "sess-1".into(),
            }),
            messages: vec![
                msg("user", "why is\nthis slow?"),
                msg("assistant", "Index."),
            ],
            created_at: updated_at,
            updated_at,
            ..Default::default()
        }
    }

    #[test]
    fn title_uses_first_user_line() {
        assert_eq!(auto_title(&session(1).messages), "why is");
        assert_eq!(auto_title(&[]), "New chat");
        let long = auto_title(&[msg("user", &"a".repeat(100))]);
        assert_eq!(long.chars().count(), TITLE_CHARS);
    }

    #[test]
    fn stored_message_roundtrip_marks_interrupted_calls() {
        let mut m = AiChatMessage {
            role: AiChatRole::Assistant,
            text: "hi".into(),
            ..Default::default()
        };
        m.tool_calls.push(ToolCallRecord {
            call_id: "c".into(),
            status: ToolCallStatus::AwaitingApproval,
            ..Default::default()
        });
        let back = StoredMessage::from_chat(&m).into_chat();
        assert_eq!(back.role, AiChatRole::Assistant);
        assert_eq!(back.text, "hi");
        assert_eq!(back.tool_calls[0].status, ToolCallStatus::Failed);
    }

    #[tokio::test]
    async fn save_load_rename_delete_clear() {
        let pool = pool().await;
        let mut s = session(10);
        let id = save(&pool, &s).await.unwrap();
        let loaded = load(&pool, id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "why is");
        assert_eq!(loaded.native_session, s.native_session);
        assert_eq!(loaded.messages, s.messages);
        assert_eq!(loaded.target, s.target);

        rename(&pool, id, "  Slow query  ").await.unwrap();
        s.id = id;
        s.updated_at = 20;
        s.messages.push(msg("user", "more"));
        save(&pool, &s).await.unwrap();
        let loaded = load(&pool, id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "Slow query");
        assert_eq!(loaded.messages.len(), 3);

        let api = ChatSessionRecord {
            target: ChatTarget::Api,
            messages: vec![msg("user", "hello")],
            created_at: 30,
            updated_at: 30,
            ..Default::default()
        };
        let id2 = save(&pool, &api).await.unwrap();
        let listed = list(&pool, 10).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, id2);
        assert_eq!(listed[1].message_count, 3);
        assert!(
            load(&pool, id2)
                .await
                .unwrap()
                .unwrap()
                .native_session
                .is_none()
        );

        delete(&pool, id2).await.unwrap();
        assert_eq!(list(&pool, 10).await.unwrap().len(), 1);
        clear_all(&pool).await.unwrap();
        assert!(list(&pool, 10).await.unwrap().is_empty());
        // Update ke sesi yang sudah terhapus menyimpan ulang sebagai baris baru.
        let id3 = save(&pool, &s).await.unwrap();
        assert_ne!(id3, id);
        assert_eq!(list(&pool, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prune_keeps_newest() {
        let pool = pool().await;
        for t in 0..5 {
            save(&pool, &session(t)).await.unwrap();
        }
        assert_eq!(prune(&pool, 3).await.unwrap(), 2);
        let listed = list(&pool, 10).await.unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].updated_at, 4);
        assert_eq!(listed[2].updated_at, 2);
    }
}
