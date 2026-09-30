//! Konfigurasi MCP server luar untuk AI Assistant (K5).
//!
//! User mendaftarkan MCP server stdio (command + args + env). Tabular
//! menjalankannya sebagai child process, membaca daftar tool-nya, lalu user
//! memilih tool mana yang boleh dipanggil model (allowlist, default tidak ada)
//! dan apakah tiap tool butuh konfirmasi sebelum jalan (default ya).
//!
//! Disimpan di tabel `ai_mcp_servers` di `connections.db`, hanya lokal
//! (tidak ikut sync atau export), mengikuti pola `connection_env`. Nilai env
//! yang namanya terlihat rahasia (`*TOKEN*`, `*KEY*`, `*SECRET*`, …) disimpan
//! lewat [`crate::secrets`]; kolom hanya berisi sentinel.
//!
//! Kode di sini headless; client MCP ada di `outside_mcp_client`, UI di
//! `window_egui::ai_mcp_ui`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::ai_tool_calling::{self, ToolDef};

/// Satu tool yang pernah dibaca dari server, beserta kebijakannya.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolPolicy {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema input terakhir yang dibaca lewat "List tools".
    #[serde(default)]
    pub schema: Value,
    /// Model boleh memanggil tool ini.
    #[serde(default)]
    pub allowed: bool,
    /// Tampilkan kartu Approve / Deny sebelum menjalankan.
    #[serde(default = "default_true")]
    pub confirm: bool,
}

fn default_true() -> bool {
    true
}

/// Satu pasangan env var. `value` berisi nilai asli di memori; saat disimpan,
/// nilai rahasia diganti sentinel (lihat [`seal_env`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Satu MCP server luar (transport stdio).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OutsideMcpServer {
    /// 0 = belum disimpan.
    pub id: i64,
    /// Nama unik, juga prefiks nama tool (`<name>__<tool>`).
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<EnvVar>,
    pub enabled: bool,
    pub tools: Vec<ToolPolicy>,
    /// Namespace nama secret untuk env rahasia (stabil walau nama diganti).
    pub secret_ns: String,
}

impl OutsideMcpServer {
    pub fn allowed_tools(&self) -> impl Iterator<Item = &ToolPolicy> {
        self.tools.iter().filter(|t| t.allowed)
    }
}

/// Tool yang siap ditawarkan ke model beserta asal-usulnya.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolBinding {
    pub def: ToolDef,
    pub server_id: i64,
    pub server_name: String,
    pub tool: String,
    pub confirm: bool,
}

/// Semua tool yang diizinkan dari server yang aktif. Nama berprefiks yang
/// bentrok (setelah sanitasi) hanya dipakai sekali.
pub fn build_bindings(servers: &[OutsideMcpServer]) -> Vec<ToolBinding> {
    let mut out: Vec<ToolBinding> = Vec::new();
    for s in servers.iter().filter(|s| s.enabled) {
        for t in s.allowed_tools() {
            let name = ai_tool_calling::prefixed_tool_name(&s.name, &t.name);
            if out.iter().any(|b| b.def.name == name) {
                log::warn!("[AI] duplicate MCP tool name {name}, skipped");
                continue;
            }
            out.push(ToolBinding {
                def: ToolDef {
                    name,
                    description: if t.description.is_empty() {
                        format!("Tool `{}` from MCP server `{}`.", t.name, s.name)
                    } else {
                        t.description.clone()
                    },
                    parameters: t.schema.clone(),
                },
                server_id: s.id,
                server_name: s.name.clone(),
                tool: t.name.clone(),
                confirm: t.confirm,
            });
        }
    }
    out
}

/// Gabungkan hasil "List tools" dengan kebijakan lama: tool yang sudah ada
/// mempertahankan `allowed`/`confirm`; tool baru default tidak diizinkan dan
/// butuh konfirmasi; tool yang hilang dari server dibuang.
pub fn merge_discovered(
    old: &[ToolPolicy],
    discovered: Vec<(String, String, Value)>,
) -> Vec<ToolPolicy> {
    discovered
        .into_iter()
        .map(|(name, description, schema)| {
            let prev = old.iter().find(|p| p.name == name);
            ToolPolicy {
                allowed: prev.is_some_and(|p| p.allowed),
                confirm: prev.is_none_or(|p| p.confirm),
                name,
                description,
                schema,
            }
        })
        .collect()
}

/// Nama env yang kemungkinan berisi rahasia.
pub fn env_key_looks_secret(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    [
        "TOKEN",
        "KEY",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "PASS",
        "AUTH",
        "CREDENTIAL",
        "PAT",
    ]
    .iter()
    .any(|w| k.contains(w))
}

fn env_secret_name(ns: &str, key: &str) -> String {
    format!("mcp:{ns}:env:{key}")
}

/// Pindahkan nilai env rahasia ke secret store; kembalikan env untuk kolom
/// database (nilai rahasia diganti sentinel bila berhasil disimpan).
pub fn seal_env(ns: &str, env: &[EnvVar]) -> Vec<EnvVar> {
    env.iter()
        .map(|e| {
            if env_key_looks_secret(&e.key) && !e.value.is_empty() {
                EnvVar {
                    key: e.key.clone(),
                    value: crate::secrets::store_or_keep(&env_secret_name(ns, &e.key), &e.value),
                }
            } else {
                e.clone()
            }
        })
        .collect()
}

/// Kebalikan [`seal_env`]: ganti sentinel dengan nilai asli.
pub fn unseal_env(ns: &str, env: &[EnvVar]) -> Vec<EnvVar> {
    env.iter()
        .map(|e| EnvVar {
            key: e.key.clone(),
            value: crate::secrets::resolve_readonly(&env_secret_name(ns, &e.key), &e.value),
        })
        .collect()
}

/// Hapus secret env milik server (dipanggil saat server dihapus atau env
/// dibuang).
pub fn delete_env_secrets(ns: &str, keys: &[String]) {
    for k in keys {
        crate::secrets::delete_secret(&env_secret_name(ns, k));
    }
}

/// Namespace secret baru yang cukup unik untuk satu mesin.
pub fn new_secret_ns() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

// ─── Penyimpanan ────────────────────────────────────────────────────────────

async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS ai_mcp_servers (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            name TEXT NOT NULL UNIQUE, \
            command TEXT NOT NULL, \
            args_json TEXT NOT NULL DEFAULT '[]', \
            env_json TEXT NOT NULL DEFAULT '[]', \
            enabled INTEGER NOT NULL DEFAULT 1, \
            tools_json TEXT NOT NULL DEFAULT '[]', \
            secret_ns TEXT NOT NULL DEFAULT '')",
    )
    .execute(pool)
    .await
    .map(|_| ())
}

type ServerRow = (i64, String, String, String, String, i64, String, String);

/// Semua server, urut nama. Nilai env masih dalam bentuk tersimpan
/// (sentinel); pakai [`unseal_env`] sebelum menjalankan proses.
pub async fn load_all(pool: &SqlitePool) -> Result<Vec<OutsideMcpServer>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows: Vec<ServerRow> = sqlx::query_as(
        "SELECT id, name, command, args_json, env_json, enabled, tools_json, secret_ns \
         FROM ai_mcp_servers ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, command, args, env, enabled, tools, secret_ns)| OutsideMcpServer {
                id,
                name,
                command,
                args: serde_json::from_str(&args).unwrap_or_default(),
                env: serde_json::from_str(&env).unwrap_or_default(),
                enabled: enabled != 0,
                tools: serde_json::from_str(&tools).unwrap_or_default(),
                secret_ns,
            },
        )
        .collect())
}

/// Simpan server (insert bila `id == 0`, selain itu update). `env` harus
/// sudah di-[`seal_env`]. Mengembalikan id.
pub async fn save(pool: &SqlitePool, s: &OutsideMcpServer) -> Result<i64, sqlx::Error> {
    ensure_table(pool).await?;
    let args = serde_json::to_string(&s.args).unwrap_or_else(|_| "[]".into());
    let env = serde_json::to_string(&s.env).unwrap_or_else(|_| "[]".into());
    let tools = serde_json::to_string(&s.tools).unwrap_or_else(|_| "[]".into());
    if s.id == 0 {
        let res = sqlx::query(
            "INSERT INTO ai_mcp_servers \
             (name, command, args_json, env_json, enabled, tools_json, secret_ns) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(s.name.trim())
        .bind(&s.command)
        .bind(args)
        .bind(env)
        .bind(i64::from(s.enabled))
        .bind(tools)
        .bind(&s.secret_ns)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    } else {
        sqlx::query(
            "UPDATE ai_mcp_servers SET name = ?, command = ?, args_json = ?, env_json = ?, \
             enabled = ?, tools_json = ?, secret_ns = ? WHERE id = ?",
        )
        .bind(s.name.trim())
        .bind(&s.command)
        .bind(args)
        .bind(env)
        .bind(i64::from(s.enabled))
        .bind(tools)
        .bind(&s.secret_ns)
        .bind(s.id)
        .execute(pool)
        .await?;
        Ok(s.id)
    }
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query("DELETE FROM ai_mcp_servers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn server(name: &str) -> OutsideMcpServer {
        OutsideMcpServer {
            name: name.into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            env: vec![EnvVar {
                key: "LOG_LEVEL".into(),
                value: "debug".into(),
            }],
            enabled: true,
            tools: vec![
                ToolPolicy {
                    name: "search_issues".into(),
                    description: "Search".into(),
                    schema: json!({ "type": "object" }),
                    allowed: true,
                    confirm: false,
                },
                ToolPolicy {
                    name: "create_issue".into(),
                    allowed: false,
                    confirm: true,
                    ..Default::default()
                },
            ],
            secret_ns: "ns1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn bindings_only_include_allowed_tools_of_enabled_servers() {
        let mut off = server("off");
        off.enabled = false;
        let b = build_bindings(&[server("github"), off]);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].def.name, "github__search_issues");
        assert_eq!(b[0].tool, "search_issues");
        assert!(!b[0].confirm);
    }

    #[test]
    fn merge_keeps_policy_and_defaults_new_tools_to_denied_with_confirm() {
        let old = server("gh").tools;
        let merged = merge_discovered(
            &old,
            vec![
                ("search_issues".into(), "new desc".into(), json!({})),
                ("list_prs".into(), String::new(), json!({})),
            ],
        );
        assert_eq!(merged.len(), 2);
        assert!(merged[0].allowed && !merged[0].confirm);
        assert_eq!(merged[0].description, "new desc");
        assert!(!merged[1].allowed && merged[1].confirm);
    }

    #[test]
    fn secret_env_detection() {
        assert!(env_key_looks_secret("GITHUB_PERSONAL_ACCESS_TOKEN"));
        assert!(env_key_looks_secret("api_key"));
        assert!(!env_key_looks_secret("LOG_LEVEL"));
    }

    #[tokio::test]
    async fn persists_updates_and_deletes_in_sqlite() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let mut s = server("github");
        let id = save(&pool, &s).await.unwrap();
        assert!(id > 0);
        s.id = id;
        s.enabled = false;
        s.args.push("--x".into());
        assert_eq!(save(&pool, &s).await.unwrap(), id);
        let other = save(&pool, &server("alpha")).await.unwrap();
        let all = load_all(&pool).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1], s);
        // Nama unik.
        assert!(save(&pool, &server("alpha")).await.is_err());
        delete(&pool, other).await.unwrap();
        assert_eq!(load_all(&pool).await.unwrap().len(), 1);
    }
}
