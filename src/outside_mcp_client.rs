//! Client MCP untuk server luar (K5): menjalankan server stdio sebagai child
//! process lewat `rmcp::transport::TokioChildProcess`, membaca daftar tool,
//! dan memanggil tool dengan batas waktu.
//!
//! Tidak dikompilasi di iOS (tidak ada proses anak). Headless: hanya menerima
//! [`OutsideMcpServer`] dengan env yang sudah di-`unseal`.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::outside_mcp::OutsideMcpServer;

/// Batas waktu handshake `initialize`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Batas waktu satu `tools/call`.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Stderr server yang disimpan untuk pesan error.
const MAX_STDERR_BYTES: usize = 4096;

type Client = RunningService<RoleClient, ()>;

/// Tool hasil `tools/list`: (nama, deskripsi, JSON Schema input).
pub type DiscoveredTool = (String, String, Value);

struct Connected {
    client: Client,
    stderr: Arc<Mutex<String>>,
}

fn stderr_tail(buf: &Arc<Mutex<String>>) -> String {
    buf.lock().map(|s| s.trim().to_string()).unwrap_or_default()
}

async fn connect(server: &OutsideMcpServer) -> Result<Connected, String> {
    let command = server.command.trim();
    if command.is_empty() {
        return Err("No command configured for this MCP server.".to_string());
    }
    // Aplikasi GUI di macOS hanya mewarisi PATH minimal; `npx`, `uvx`, `node`
    // sering ada di /opt/homebrew/bin atau ~/.nvm.
    let bin = crate::agent::harness::resolve_binary(command)
        .unwrap_or_else(|| std::path::PathBuf::from(command));
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&server.args)
        .env("PATH", crate::agent::harness::augmented_path())
        .kill_on_drop(true);
    for e in &server.env {
        if !e.key.trim().is_empty() {
            cmd.env(e.key.trim(), &e.value);
        }
    }
    let (transport, stderr) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start `{}`: {e}", bin.display()))?;

    let buf = Arc::new(Mutex::new(String::new()));
    if let Some(mut stderr) = stderr {
        let buf = buf.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 1024];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut s) = buf.lock() {
                            s.push_str(&String::from_utf8_lossy(&chunk[..n]));
                            if s.len() > MAX_STDERR_BYTES {
                                let mut cut = s.len() - MAX_STDERR_BYTES;
                                while !s.is_char_boundary(cut) {
                                    cut += 1;
                                }
                                s.drain(..cut);
                            }
                        }
                    }
                }
            }
        });
    }

    match tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport)).await {
        Ok(Ok(client)) => Ok(Connected {
            client,
            stderr: buf,
        }),
        Ok(Err(e)) => {
            // Beri waktu singkat agar stderr sempat terbaca.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let tail = stderr_tail(&buf);
            Err(if tail.is_empty() {
                format!("MCP handshake failed: {e}")
            } else {
                format!("MCP handshake failed: {e}\n{tail}")
            })
        }
        Err(_) => Err(format!(
            "The MCP server did not answer within {} s.",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

async fn list_tools_async(client: &Client) -> Result<Vec<DiscoveredTool>, String> {
    let tools = tokio::time::timeout(CALL_TIMEOUT, client.list_all_tools())
        .await
        .map_err(|_| "tools/list timed out.".to_string())?
        .map_err(|e| format!("tools/list failed: {e}"))?;
    Ok(tools
        .into_iter()
        .map(|t| {
            (
                t.name.to_string(),
                t.description.map(|d| d.to_string()).unwrap_or_default(),
                Value::Object((*t.input_schema).clone()),
            )
        })
        .collect())
}

/// Jalankan server, baca daftar tool, lalu hentikan. Untuk tombol
/// "Test / List tools" di Settings. Harus dipanggil dari luar runtime tokio.
pub fn list_tools_blocking(
    rt: &tokio::runtime::Handle,
    server: &OutsideMcpServer,
) -> Result<Vec<DiscoveredTool>, String> {
    rt.block_on(async {
        let conn = connect(server).await?;
        let result = list_tools_async(&conn.client).await;
        if let Err(e) = conn.client.cancel().await {
            log::debug!("[AI] MCP client shutdown: {e}");
        }
        result
    })
}

/// Koneksi MCP yang dibuka sesuai kebutuhan selama satu giliran chat dan
/// ditutup saat pool di-drop.
pub struct McpSessionPool {
    rt: tokio::runtime::Handle,
    servers: HashMap<i64, OutsideMcpServer>,
    clients: HashMap<i64, Connected>,
}

impl McpSessionPool {
    pub fn new(rt: tokio::runtime::Handle, servers: Vec<OutsideMcpServer>) -> Self {
        Self {
            rt,
            servers: servers.into_iter().map(|s| (s.id, s)).collect(),
            clients: HashMap::new(),
        }
    }

    /// Panggil `tool` di server `server_id`. Mengembalikan hasil MCP yang
    /// sudah diserialisasi ke JSON (lihat `ai_tool_calling::mcp_result_to_text`).
    pub fn call(&mut self, server_id: i64, tool: &str, arguments: Value) -> Result<Value, String> {
        if !self.clients.contains_key(&server_id) {
            let server = self
                .servers
                .get(&server_id)
                .ok_or_else(|| "Unknown MCP server.".to_string())?;
            let conn = self.rt.block_on(connect(server))?;
            self.clients.insert(server_id, conn);
        }
        let Some(conn) = self.clients.get(&server_id) else {
            return Err("MCP server is not connected.".to_string());
        };
        let mut params = CallToolRequestParams::new(tool.to_string());
        params.arguments = match arguments {
            Value::Object(m) => Some(m),
            Value::Null => None,
            other => {
                return Err(format!(
                    "Tool arguments must be a JSON object, got: {other}"
                ));
            }
        };
        let result = self.rt.block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, conn.client.call_tool(params)).await
        });
        match result {
            Ok(Ok(res)) => serde_json::to_value(&res)
                .map_err(|e| format!("Could not read the tool result: {e}")),
            Ok(Err(e)) => {
                let tail = stderr_tail(&conn.stderr);
                // Koneksi mungkin rusak; buka ulang di pemanggilan berikutnya.
                self.drop_client(server_id);
                Err(if tail.is_empty() {
                    format!("Tool call failed: {e}")
                } else {
                    format!("Tool call failed: {e}\n{tail}")
                })
            }
            Err(_) => {
                self.drop_client(server_id);
                Err(format!(
                    "The tool did not finish within {} s.",
                    CALL_TIMEOUT.as_secs()
                ))
            }
        }
    }

    fn drop_client(&mut self, server_id: i64) {
        if let Some(conn) = self.clients.remove(&server_id) {
            // Drop child process harus terjadi di dalam konteks runtime.
            let _guard = self.rt.enter();
            if let Err(e) = self.rt.block_on(conn.client.cancel()) {
                log::debug!("[AI] MCP client shutdown: {e}");
            }
        }
    }
}

impl Drop for McpSessionPool {
    fn drop(&mut self) {
        let ids: Vec<i64> = self.clients.keys().copied().collect();
        for id in ids {
            self.drop_client(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uji manual terhadap server MCP nyata, mis. `tabular mcp`:
    /// `TABULAR_MCP_TEST_BIN=target/debug/tabular cargo test --lib outside_mcp_client -- --ignored`
    #[test]
    #[ignore = "butuh binary MCP server nyata lewat TABULAR_MCP_TEST_BIN"]
    fn lists_and_calls_tools_of_real_server() {
        let Ok(bin) = std::env::var("TABULAR_MCP_TEST_BIN") else {
            return;
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let server = OutsideMcpServer {
            id: 1,
            name: "tabular".into(),
            command: bin,
            args: vec!["mcp".into()],
            enabled: true,
            ..Default::default()
        };
        let tools = list_tools_blocking(rt.handle(), &server).expect("list tools");
        assert!(tools.iter().any(|(n, _, _)| n == "format_sql"));
        let mut pool = McpSessionPool::new(rt.handle().clone(), vec![server]);
        let res = pool
            .call(1, "format_sql", serde_json::json!({ "sql": "select 1" }))
            .expect("call");
        let (text, is_err) = crate::ai_tool_calling::mcp_result_to_text(&res);
        assert!(!is_err, "{text}");
        assert!(text.to_ascii_uppercase().contains("SELECT"), "{text}");
        drop(pool);
    }
}
