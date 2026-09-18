//! Backend CLI agent untuk panel AI Assistant.
//!
//! Menjalankan `agy` / `claude` / `gemini` / perintah custom dalam print mode
//! (`--output-format stream-json`), membaca stdout baris demi baris, dan
//! meneruskannya ke UI sebagai [`AgentEvent`] lewat `mpsc`. Tidak bergantung
//! pada egui sama sekali: UI hanya mem-poll `Receiver<AgentEvent>`.
//!
//! Akses database untuk agent disediakan oleh MCP server Tabular sendiri
//! (`tabular mcp`, lihat [`super::mcp`]); modul ini hanya memastikan server
//! itu dikenal oleh CLI (per-invocation untuk Claude Code, registrasi global
//! untuk agy / Gemini).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crate::config::CliAgentKind;

/// Nama MCP server Tabular di konfigurasi CLI (`mcp__tabular__*` di Claude Code).
pub const MCP_SERVER_NAME: &str = "tabular";

/// Pesan untuk build Mac App Store, tempat backend CLI tidak bisa dipakai.
pub const SANDBOX_UNAVAILABLE_MESSAGE: &str = "CLI agents are not available in the Mac App Store version of Tabular: the App Sandbox does not allow running tools installed on your Mac. Use the HTTP API backend, or install the direct-download version of Tabular.";

/// Apakah proses berjalan di dalam App Sandbox macOS (build Mac App Store).
///
/// Di sandbox, proses anak mewarisi sandbox yang sama: binary di `~/.local/bin`
/// tidak bisa dieksekusi, `HOME` dialihkan ke container sehingga sesi login
/// CLI tidak terlihat, dan konfigurasi MCP global tidak bisa ditulis. macOS
/// mengisi `APP_SANDBOX_CONTAINER_ID` untuk setiap proses yang di-sandbox.
pub fn is_app_sandboxed() -> bool {
    sandboxed_from_env(std::env::var_os("APP_SANDBOX_CONTAINER_ID").as_deref())
}

fn sandboxed_from_env(container_id: Option<&std::ffi::OsStr>) -> bool {
    cfg!(target_os = "macos") && container_id.is_some_and(|v| !v.is_empty())
}

/// Konfigurasi CLI yang disalin dari preferensi user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliAgentConfig {
    pub kind: CliAgentKind,
    /// Path binary; kosong berarti cari `kind.default_binary()` di PATH.
    pub bin: String,
    pub model: String,
    pub effort: String,
    /// Argumen tambahan; untuk `Custom` ini adalah template dengan placeholder
    /// `{prompt}`, `{system}`, `{model}`, `{session}`.
    pub extra_args: String,
}

impl CliAgentConfig {
    /// Nama/path binary yang efektif dipakai.
    pub fn effective_bin(&self) -> String {
        let trimmed = self.bin.trim();
        if trimmed.is_empty() {
            self.kind.default_binary().to_string()
        } else {
            trimmed.to_string()
        }
    }
}

/// Kejadian yang dikirim ke UI selama satu giliran percakapan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// Id sesi/percakapan dari CLI, dipakai untuk melanjutkan giliran berikutnya.
    Session(String),
    /// Potongan teks jawaban (streaming).
    TextDelta(String),
    /// Agent memanggil tool (nama tool), hanya untuk indikator aktivitas.
    ToolUse(String),
    /// Giliran selesai. `text` berisi jawaban lengkap (sama dengan gabungan
    /// delta bila ada), `usage` ringkasan token/biaya bila CLI melaporkannya.
    Done { text: String, usage: Option<String> },
    Error(String),
}

/// Satu permintaan ke CLI.
#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub system_prompt: String,
    pub user_prompt: String,
    /// Id sesi dari giliran sebelumnya (hanya dipakai bila `kind.supports_resume()`).
    pub session_id: Option<String>,
    /// Direktori kerja proses; harus direktori kosong khusus agar tool file
    /// milik CLI tidak menyentuh project apa pun.
    pub cwd: PathBuf,
    /// File JSON konfigurasi MCP (dipakai Claude Code lewat `--mcp-config`).
    pub mcp_config: Option<PathBuf>,
}

/// Pisahkan string argumen ala shell: spasi memisahkan, kutip tunggal/ganda
/// menggabungkan, backslash meng-escape karakter berikutnya.
pub fn split_args(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut has_token = false;

    for ch in input.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            has_token = true;
            continue;
        }
        match (quote, ch) {
            (_, '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(ch);
                has_token = true;
            }
            (None, c) if c.is_whitespace() => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            (None, c) => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(cur);
    }
    out
}

/// Apakah nama model sudah menyertakan tingkat effort (`…-low|-medium|-high`).
pub fn model_embeds_effort(model: &str) -> bool {
    matches!(
        model.trim().rsplit('-').next(),
        Some("low") | Some("medium") | Some("high")
    )
}

/// Gabungkan system + user prompt untuk CLI yang tidak punya flag system prompt.
fn combined_prompt(req: &AgentRequest) -> String {
    if req.system_prompt.trim().is_empty() {
        req.user_prompt.clone()
    } else {
        format!("{}\n\n---\n\n{}", req.system_prompt.trim_end(), req.user_prompt)
    }
}

/// Susun argumen baris perintah untuk `kind`. Dipisahkan dari [`spawn_stream`]
/// supaya bisa diuji tanpa menjalankan proses.
pub fn build_args(cfg: &CliAgentConfig, req: &AgentRequest) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let model = cfg.model.trim();
    let effort = cfg.effort.trim();
    let session = req
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match cfg.kind {
        CliAgentKind::Antigravity => {
            args.push("--print".into());
            args.push(combined_prompt(req));
            args.push("--output-format".into());
            args.push("stream-json".into());
            // Print mode tidak bisa menjawab prompt izin; MCP Tabular sendiri
            // sudah read-only, dan cwd adalah direktori kosong khusus.
            args.push("--dangerously-skip-permissions".into());
            if !model.is_empty() {
                args.push("--model".into());
                args.push(model.into());
            }
            // Model Gemini di agy sudah membawa tingkat effort di namanya
            // (`gemini-3.8-flash-high`); `--effort` yang berbeda ditolak agy.
            if !effort.is_empty() && !model_embeds_effort(model) {
                args.push("--effort".into());
                args.push(effort.into());
            }
            if let Some(id) = session {
                args.push("--conversation".into());
                args.push(id.into());
            }
            args.extend(split_args(&cfg.extra_args));
        }
        CliAgentKind::ClaudeCode => {
            args.push("-p".into());
            args.push(req.user_prompt.clone());
            args.push("--output-format".into());
            args.push("stream-json".into());
            args.push("--verbose".into());
            args.push("--include-partial-messages".into());
            if !req.system_prompt.trim().is_empty() {
                args.push("--append-system-prompt".into());
                args.push(req.system_prompt.clone());
            }
            if !model.is_empty() {
                args.push("--model".into());
                args.push(model.into());
            }
            if !effort.is_empty() {
                args.push("--effort".into());
                args.push(effort.into());
            }
            if let Some(id) = session {
                args.push("--resume".into());
                args.push(id.into());
            }
            if let Some(path) = &req.mcp_config {
                args.push("--mcp-config".into());
                args.push(path.to_string_lossy().to_string());
                args.push("--strict-mcp-config".into());
                // Hanya tool MCP Tabular yang diizinkan tanpa prompt; tool lain
                // (Bash, Edit, …) ditolak otomatis di print mode.
                args.push("--allowedTools".into());
                args.push(format!("mcp__{MCP_SERVER_NAME}"));
            }
            args.extend(split_args(&cfg.extra_args));
        }
        CliAgentKind::GeminiCli => {
            args.push("-p".into());
            args.push(combined_prompt(req));
            args.push("--output-format".into());
            args.push("stream-json".into());
            // Mode read-only; user bisa menimpa lewat extra args (yargs memakai
            // nilai terakhir).
            args.push("--approval-mode".into());
            args.push("plan".into());
            args.push("--allowed-mcp-server-names".into());
            args.push(MCP_SERVER_NAME.into());
            if !model.is_empty() {
                args.push("-m".into());
                args.push(model.into());
            }
            args.extend(split_args(&cfg.extra_args));
        }
        CliAgentKind::Custom => {
            let template = split_args(&cfg.extra_args);
            let mut prompt_used = false;
            for tok in template {
                let replaced = tok
                    .replace("{system}", &req.system_prompt)
                    .replace("{model}", model)
                    .replace("{session}", session.unwrap_or(""));
                if replaced.contains("{prompt}") {
                    prompt_used = true;
                    args.push(replaced.replace("{prompt}", &req.user_prompt));
                } else {
                    args.push(replaced);
                }
            }
            if !prompt_used {
                args.push(combined_prompt(req));
            }
        }
    }
    args
}

// ─── Parser stream-json ──────────────────────────────────────────────────────

/// Parser output CLI yang stateful: satu instance per giliran.
///
/// Setiap CLI punya bentuk NDJSON sendiri; parser toleran terhadap event yang
/// tidak dikenal (diabaikan) supaya perubahan kecil antar versi CLI tidak
/// mematahkan chat.
#[derive(Debug)]
pub struct StreamParser {
    kind: CliAgentKind,
    /// Teks yang sudah dikirim sebagai delta; dipakai sebagai jawaban akhir.
    text: String,
    saw_delta: bool,
    saw_stream_event: bool,
    saw_json: bool,
    finished: bool,
    /// Baris non-JSON dari CLI JSON (fallback bila tidak ada event sama sekali).
    raw_lines: String,
}

impl StreamParser {
    pub fn new(kind: CliAgentKind) -> Self {
        Self {
            kind,
            text: String::new(),
            saw_delta: false,
            saw_stream_event: false,
            saw_json: false,
            finished: false,
            raw_lines: String::new(),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    fn delta(&mut self, s: &str) -> AgentEvent {
        self.saw_delta = true;
        self.text.push_str(s);
        AgentEvent::TextDelta(s.to_string())
    }

    fn done(&mut self, fallback_text: Option<&str>, usage: Option<String>) -> AgentEvent {
        self.finished = true;
        let text = if self.saw_delta {
            self.text.clone()
        } else {
            fallback_text.unwrap_or("").to_string()
        };
        AgentEvent::Done { text, usage }
    }

    fn error(&mut self, msg: String) -> AgentEvent {
        self.finished = true;
        AgentEvent::Error(msg)
    }

    /// Proses satu baris stdout. Bisa menghasilkan 0..n event.
    pub fn feed_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || self.finished {
            return Vec::new();
        }
        if self.kind == CliAgentKind::Custom {
            return vec![self.delta(&format!("{line}\n"))];
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                log::debug!("[AGENT] non-JSON line from {:?}: {line}", self.kind);
                self.raw_lines.push_str(line);
                self.raw_lines.push('\n');
                return Vec::new();
            }
        };
        self.saw_json = true;
        match self.kind {
            CliAgentKind::Antigravity => self.feed_agy(&value),
            CliAgentKind::ClaudeCode => self.feed_claude(&value),
            CliAgentKind::GeminiCli => self.feed_gemini(&value),
            CliAgentKind::Custom => unreachable!(),
        }
    }

    /// Dipanggil saat stdout ditutup. `Some` bila giliran belum ditutup oleh
    /// event `result` (mis. CLI plain-text atau proses berhenti lebih awal).
    pub fn finish(&mut self) -> Option<AgentEvent> {
        if self.finished {
            return None;
        }
        if self.saw_delta {
            return Some(self.done(None, None));
        }
        if !self.saw_json && !self.raw_lines.trim().is_empty() {
            let raw = self.raw_lines.trim().to_string();
            return Some(self.done(Some(&raw), None));
        }
        None
    }

    fn feed_agy(&mut self, v: &serde_json::Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match v["event"].as_str().unwrap_or("") {
            "init" => {
                if let Some(id) = v["conversation_id"].as_str() {
                    out.push(AgentEvent::Session(id.to_string()));
                }
            }
            "step_update" => {
                let su = &v["step_update"];
                let step_type = su["step_type"].as_str().unwrap_or("");
                match step_type {
                    "agent_response" => {
                        if let Some(d) = su["text_delta"].as_str()
                            && !d.is_empty()
                        {
                            out.push(self.delta(d));
                        }
                    }
                    "user_input" | "" => {}
                    other => {
                        if su["state"].as_str() == Some("ACTIVE") {
                            let name = su["tool_name"]
                                .as_str()
                                .or_else(|| su["name"].as_str())
                                .unwrap_or(other);
                            out.push(AgentEvent::ToolUse(name.to_string()));
                        }
                    }
                }
            }
            "result" => {
                let r = &v["result"];
                let status = r["status"].as_str().unwrap_or("SUCCESS");
                if status.eq_ignore_ascii_case("SUCCESS") {
                    let usage = format_usage(&r["usage"], r["duration_seconds"].as_f64());
                    let response = r["response"].as_str().map(str::to_string);
                    out.push(self.done(response.as_deref(), usage));
                } else {
                    let detail = r["error"]
                        .as_str()
                        .or_else(|| r["response"].as_str())
                        .unwrap_or("");
                    out.push(self.error(format!("agy finished with status {status}: {detail}")));
                }
            }
            "error" => {
                let msg = v["error"]
                    .as_str()
                    .or_else(|| v["message"].as_str())
                    .unwrap_or("unknown error from agy")
                    .to_string();
                out.push(self.error(msg));
            }
            _ => {}
        }
        out
    }

    fn feed_claude(&mut self, v: &serde_json::Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match v["type"].as_str().unwrap_or("") {
            "system" => {
                if v["subtype"].as_str() == Some("init")
                    && let Some(id) = v["session_id"].as_str()
                {
                    out.push(AgentEvent::Session(id.to_string()));
                }
            }
            "stream_event" => {
                self.saw_stream_event = true;
                let ev = &v["event"];
                match ev["type"].as_str().unwrap_or("") {
                    "content_block_delta" => {
                        if ev["delta"]["type"].as_str() == Some("text_delta")
                            && let Some(t) = ev["delta"]["text"].as_str()
                            && !t.is_empty()
                        {
                            out.push(self.delta(t));
                        }
                    }
                    "content_block_start" => {
                        if ev["content_block"]["type"].as_str() == Some("tool_use")
                            && let Some(name) = ev["content_block"]["name"].as_str()
                        {
                            out.push(AgentEvent::ToolUse(name.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                // Tanpa --include-partial-messages hanya event ini yang membawa
                // teks; dengan flag itu, delta sudah dikirim lewat stream_event.
                if self.saw_stream_event {
                    return out;
                }
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for b in blocks {
                        match b["type"].as_str().unwrap_or("") {
                            "text" => {
                                if let Some(t) = b["text"].as_str()
                                    && !t.is_empty()
                                {
                                    out.push(self.delta(t));
                                }
                            }
                            "tool_use" => {
                                if let Some(name) = b["name"].as_str() {
                                    out.push(AgentEvent::ToolUse(name.to_string()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            "result" => {
                if v["is_error"].as_bool() == Some(true) {
                    let msg = v["result"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| {
                            v["errors"].as_array().map(|errs| {
                                errs.iter()
                                    .filter_map(|e| e.as_str())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            })
                        })
                        .unwrap_or_else(|| "claude returned an error".to_string());
                    out.push(self.error(msg));
                } else {
                    let mut usage = format_usage(&v["usage"], v["duration_ms"].as_f64().map(|ms| ms / 1000.0));
                    if let Some(cost) = v["total_cost_usd"].as_f64() {
                        let cost_txt = format!("${cost:.4}");
                        usage = Some(match usage {
                            Some(u) => format!("{u} · {cost_txt}"),
                            None => cost_txt,
                        });
                    }
                    let result = v["result"].as_str().map(str::to_string);
                    out.push(self.done(result.as_deref(), usage));
                }
            }
            _ => {}
        }
        out
    }

    fn feed_gemini(&mut self, v: &serde_json::Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match v["type"].as_str().unwrap_or("") {
            "init" => {
                if let Some(id) = v["session_id"].as_str() {
                    out.push(AgentEvent::Session(id.to_string()));
                }
            }
            "message" => {
                if v["role"].as_str() == Some("assistant")
                    && let Some(content) = v["content"].as_str()
                    && !content.is_empty()
                {
                    let is_delta = v["delta"].as_bool().unwrap_or(true);
                    if is_delta || !self.saw_delta {
                        out.push(self.delta(content));
                    }
                }
            }
            "tool_use" | "tool_call" => {
                let name = v["tool_name"]
                    .as_str()
                    .or_else(|| v["name"].as_str())
                    .unwrap_or("tool");
                out.push(AgentEvent::ToolUse(name.to_string()));
            }
            "result" => {
                let status = v["status"].as_str().unwrap_or("success");
                if status.eq_ignore_ascii_case("success") {
                    let usage = format_usage(&v["stats"], None);
                    let response = v["response"].as_str().map(str::to_string);
                    out.push(self.done(response.as_deref(), usage));
                } else {
                    let detail = v["error"]["message"]
                        .as_str()
                        .or_else(|| v["error"].as_str())
                        .unwrap_or("");
                    out.push(self.error(format!("gemini finished with status {status}: {detail}")));
                }
            }
            "error" => {
                let msg = v["message"]
                    .as_str()
                    .or_else(|| v["error"]["message"].as_str())
                    .unwrap_or("unknown error from gemini")
                    .to_string();
                out.push(self.error(msg));
            }
            _ => {}
        }
        out
    }
}

/// Ringkas objek usage (`input_tokens`, `output_tokens`, …) jadi satu baris.
fn format_usage(usage: &serde_json::Value, duration_secs: Option<f64>) -> Option<String> {
    let mut parts = Vec::new();
    let input = usage["input_tokens"].as_u64();
    let output = usage["output_tokens"].as_u64();
    if let (Some(i), Some(o)) = (input, output) {
        parts.push(format!("{i} in / {o} out tokens"));
    } else if let Some(total) = usage["total_tokens"].as_u64() {
        parts.push(format!("{total} tokens"));
    }
    if let Some(secs) = duration_secs {
        parts.push(format!("{secs:.1}s"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

// ─── Proses ──────────────────────────────────────────────────────────────────

/// Pegangan untuk menghentikan proses CLI yang sedang berjalan.
#[derive(Clone)]
pub struct CancelHandle {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.child.lock()
            && let Some(child) = guard.as_mut()
        {
            let pid = child.id();
            if let Err(e) = child.kill() {
                log::debug!("[AGENT] kill pid {pid} failed (already exited?): {e}");
            }
            // Proses CLI biasanya punya anak (node, MCP server). Group id sama
            // dengan pid karena `process_group(0)` saat spawn.
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg("--")
                    .arg(format!("-{pid}"))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Direktori kerja kosong untuk proses CLI (`{data_dir}/agent-workspace`).
pub fn agent_workspace_dir() -> PathBuf {
    let dir = crate::config::get_data_dir().join("agent-workspace");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("[AGENT] cannot create workspace dir {}: {e}", dir.display());
    }
    dir
}

/// Path binary Tabular sendiri, dipakai sebagai command MCP server.
pub fn tabular_exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "tabular".to_string())
}

/// JSON konfigurasi MCP yang dimengerti Claude Code / Cursor / agy.
pub fn mcp_config_json() -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            MCP_SERVER_NAME: {
                "command": tabular_exe(),
                "args": ["mcp"]
            }
        }
    })
}

/// Tulis file konfigurasi MCP ke workspace dan kembalikan path-nya.
pub fn write_mcp_config_file() -> Result<PathBuf, String> {
    let path = agent_workspace_dir().join("mcp-tabular.json");
    let body = serde_json::to_string_pretty(&mcp_config_json()).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Direktori tambahan yang dicari bila binary tidak ada di PATH proses. Aplikasi
/// yang diluncurkan dari Finder/Dock hanya mewarisi PATH sistem minimal.
fn extra_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        for rel in [
            ".local/bin",
            ".antigravity/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            ".cargo/bin",
        ] {
            dirs.push(home.join(rel));
        }
        // nvm: ~/.nvm/versions/node/<ver>/bin
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            for e in entries.flatten() {
                dirs.push(e.path().join("bin"));
            }
        }
    }
    dirs
}

/// PATH yang sudah ditambah [`extra_bin_dirs`], untuk diwariskan ke proses CLI
/// (agy/claude sendiri butuh `node`, `git`, dll. yang mungkin tidak ada di PATH
/// minimal aplikasi GUI).
pub fn augmented_path() -> std::ffi::OsString {
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in extra_bin_dirs() {
        if d.is_dir() && !paths.contains(&d) {
            paths.push(d);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

fn candidate_names(name: &str) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if cfg!(windows) {
        for ext in ["exe", "cmd", "bat"] {
            names.push(format!("{name}.{ext}"));
        }
    }
    names
}

/// Cari binary: path eksplisit → PATH → direktori umum → `$SHELL -lc command -v`.
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let direct = Path::new(name);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(extra_bin_dirs());
    for dir in dirs {
        for cand in candidate_names(name) {
            let p = dir.join(cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        if let Ok(out) = Command::new(shell)
            .arg("-lc")
            .arg(format!("command -v {name}"))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() && Path::new(&s).is_file() {
                return Some(PathBuf::from(s));
            }
        }
    }
    None
}

/// Deteksi pesan "belum login" dari output CLI dan ubah jadi instruksi.
pub fn detect_login_problem(kind: CliAgentKind, output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let hit = [
        "not logged in",
        "not authenticated",
        "unauthenticated",
        "authentication_required",
        "authentication required",
        "please log in",
        "please login",
        "please run /login",
        "login required",
        "invalid api key",
        "401",
    ]
    .iter()
    .any(|p| lower.contains(p));
    if !hit {
        return None;
    }
    let bin = kind.default_binary();
    Some(match kind {
        CliAgentKind::Antigravity => format!(
            "Antigravity session expired or not logged in. Open a terminal, run `{bin}` once and sign in with your Google account, then try again."
        ),
        CliAgentKind::ClaudeCode => format!(
            "Claude Code is not logged in. Open a terminal, run `{bin}` and complete `/login`, then try again."
        ),
        CliAgentKind::GeminiCli => format!(
            "Gemini CLI is not authenticated. Open a terminal, run `{bin}` once and sign in, then try again."
        ),
        CliAgentKind::Custom => "The CLI reported an authentication problem. Sign in from a terminal and try again.".to_string(),
    })
}

fn tail(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        let start = s.len() - max;
        let start = s.char_indices().map(|(i, _)| i).find(|&i| i >= start).unwrap_or(0);
        format!("…{}", &s[start..])
    }
}

/// Jalankan CLI dan alirkan event-nya. Proses hidup di thread terpisah; UI
/// mem-poll receiver dengan `try_recv()`.
pub fn spawn_stream(
    cfg: &CliAgentConfig,
    req: AgentRequest,
) -> Result<(mpsc::Receiver<AgentEvent>, CancelHandle), String> {
    if is_app_sandboxed() {
        return Err(SANDBOX_UNAVAILABLE_MESSAGE.to_string());
    }
    let bin_name = cfg.effective_bin();
    if bin_name.is_empty() {
        return Err("No CLI command configured. Open Settings → AI Assistant.".to_string());
    }
    let bin = resolve_binary(&bin_name).ok_or_else(|| {
        format!(
            "CLI `{bin_name}` not found. Install it, or set its full path in Settings → AI Assistant."
        )
    })?;
    if let Err(e) = std::fs::create_dir_all(&req.cwd) {
        return Err(format!("cannot create agent workspace {}: {e}", req.cwd.display()));
    }

    let args = build_args(cfg, &req);
    log::info!(
        "[AGENT] spawning {} ({:?}) with {} args in {}",
        bin.display(),
        cfg.kind,
        args.len(),
        req.cwd.display()
    );

    let mut cmd = Command::new(&bin);
    cmd.args(&args)
        .current_dir(&req.cwd)
        .env("PATH", augmented_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{}`: {e}", bin.display()))?;
    let stdout = child.stdout.take().ok_or("CLI stdout is not available")?;
    let stderr = child.stderr.take().ok_or("CLI stderr is not available")?;

    let (tx, rx) = mpsc::channel();
    let handle = CancelHandle {
        child: Arc::new(Mutex::new(Some(child))),
        cancelled: Arc::new(AtomicBool::new(false)),
    };

    let stderr_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    {
        let buf = Arc::clone(&stderr_buf);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut s = String::new();
            let _ = reader.read_to_string(&mut s);
            if !s.trim().is_empty() {
                log::debug!("[AGENT] stderr: {}", tail(&s, 2000));
            }
            if let Ok(mut g) = buf.lock() {
                *g = s;
            }
        });
    }

    let kind = cfg.kind;
    let reader_handle = handle.clone();
    std::thread::spawn(move || {
        let mut parser = StreamParser::new(kind);
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    log::debug!("[AGENT] stdout read error: {e}");
                    break;
                }
            };
            for ev in parser.feed_line(&line) {
                if tx.send(ev).is_err() {
                    // UI sudah tidak menunggu; hentikan proses agar tidak yatim.
                    reader_handle.cancel();
                    return;
                }
            }
        }

        let status = {
            let mut guard = reader_handle.child.lock().ok();
            guard.as_mut().and_then(|g| g.take()).and_then(|mut c| c.wait().ok())
        };
        if parser.is_finished() {
            return;
        }
        if reader_handle.is_cancelled() {
            let _ = tx.send(AgentEvent::Error("Stopped by user.".to_string()));
            return;
        }
        let stderr_text = stderr_buf.lock().map(|g| g.clone()).unwrap_or_default();
        let ok = status.map(|s| s.success()).unwrap_or(false);
        if let Some(ev) = parser.finish()
            && ok
        {
            let _ = tx.send(ev);
            return;
        }
        let msg = detect_login_problem(kind, &stderr_text).unwrap_or_else(|| {
            let code = status
                .and_then(|s| s.code())
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            let detail = tail(&stderr_text, 800);
            if detail.is_empty() {
                format!("CLI exited with code {code} without a result.")
            } else {
                format!("CLI exited with code {code}: {detail}")
            }
        });
        let _ = tx.send(AgentEvent::Error(msg));
    });

    Ok((rx, handle))
}

/// Jalankan `bin` dengan argumen dan kembalikan stdout (dan stderr bila gagal).
fn run_capture(bin: &Path, args: &[&str], timeout: std::time::Duration) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("PATH", augmented_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{}`: {e}", bin.display()))?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                return Err(format!(
                    "`{} {}` timed out after {}s",
                    bin.display(),
                    args.join(" "),
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("cannot read output: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "`{} {}` failed ({}): {}",
            bin.display(),
            args.join(" "),
            out.status,
            tail(&format!("{stdout}\n{stderr}"), 600)
        ))
    }
}

/// Apakah MCP server Tabular sudah terdaftar di konfigurasi global CLI.
pub fn check_mcp_registered(cfg: &CliAgentConfig) -> Result<bool, String> {
    let bin = resolve_binary(&cfg.effective_bin())
        .ok_or_else(|| format!("CLI `{}` not found", cfg.effective_bin()))?;
    let out = run_capture(&bin, &["mcp", "list"], std::time::Duration::from_secs(20))?;
    Ok(mcp_list_mentions_tabular(&out))
}

/// Apakah output `<cli> mcp list` menyebut server Tabular.
pub fn mcp_list_mentions_tabular(output: &str) -> bool {
    output.lines().any(|l| {
        let first = l.split_whitespace().next().map(|w| w.trim_end_matches(':'));
        first == Some(MCP_SERVER_NAME) || l.contains(&format!("{MCP_SERVER_NAME}:"))
    })
}

/// Daftarkan MCP server Tabular di konfigurasi global CLI.
pub fn register_mcp(cfg: &CliAgentConfig) -> Result<String, String> {
    let bin = resolve_binary(&cfg.effective_bin())
        .ok_or_else(|| format!("CLI `{}` not found", cfg.effective_bin()))?;
    let exe = tabular_exe();
    let args: Vec<&str> = match cfg.kind {
        CliAgentKind::Antigravity | CliAgentKind::ClaudeCode => {
            vec!["mcp", "add", MCP_SERVER_NAME, "--", &exe, "mcp"]
        }
        CliAgentKind::GeminiCli => vec!["mcp", "add", MCP_SERVER_NAME, &exe, "mcp"],
        CliAgentKind::Custom => {
            return Err("Register the Tabular MCP server manually for a custom command (see `tabular mcp --print-config`).".to_string());
        }
    };
    let out = run_capture(&bin, &args, std::time::Duration::from_secs(20))?;
    log::info!("[AGENT] registered MCP server via {}: {}", bin.display(), tail(&out, 300));
    Ok(out.trim().to_string())
}

/// Uji cepat: binary ada, versi bisa dibaca, dan satu prompt kecil dijawab.
pub fn test_connection(cfg: &CliAgentConfig) -> Result<String, String> {
    let bin = resolve_binary(&cfg.effective_bin()).ok_or_else(|| {
        format!(
            "CLI `{}` not found in PATH or common install locations.",
            cfg.effective_bin()
        )
    })?;
    let version = run_capture(&bin, &["--version"], std::time::Duration::from_secs(15))
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "(version unknown)".to_string());

    let req = AgentRequest {
        system_prompt: String::new(),
        user_prompt: "Reply with exactly the word OK and nothing else.".to_string(),
        session_id: None,
        cwd: agent_workspace_dir(),
        mcp_config: None,
    };
    let (rx, handle) = spawn_stream(cfg, req)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut text = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            handle.cancel();
            return Err(format!("{} · {version}\nTest prompt timed out after 120s.", bin.display()));
        }
        match rx.recv_timeout(remaining) {
            Ok(AgentEvent::TextDelta(d)) => text.push_str(&d),
            Ok(AgentEvent::Done { text: t, usage }) => {
                let reply = if text.is_empty() { t } else { text };
                let usage = usage.map(|u| format!(" ({u})")).unwrap_or_default();
                return Ok(format!(
                    "{} · {version}\nReply: {}{usage}",
                    bin.display(),
                    reply.trim()
                ));
            }
            Ok(AgentEvent::Error(e)) => return Err(format!("{} · {version}\n{e}", bin.display())),
            Ok(_) => {}
            Err(_) => {
                handle.cancel();
                return Err(format!("{} · {version}\nCLI stopped without a reply.", bin.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> AgentRequest {
        AgentRequest {
            system_prompt: "SYS".into(),
            user_prompt: "USER".into(),
            session_id: Some("abc".into()),
            cwd: PathBuf::from("/tmp"),
            mcp_config: Some(PathBuf::from("/tmp/mcp.json")),
        }
    }

    #[test]
    fn split_args_handles_quotes_and_escapes() {
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("  a  b "), vec!["a", "b"]);
        assert_eq!(split_args(r#"--x "hello world" 'it''s'"#), vec!["--x", "hello world", "its"]);
        assert_eq!(split_args(r"a\ b"), vec!["a b"]);
        assert_eq!(split_args(r#""""#), vec![""]);
    }

    #[test]
    fn agy_args_include_prompt_model_and_resume() {
        let cfg = CliAgentConfig {
            kind: CliAgentKind::Antigravity,
            model: "claude-sonnet-4-6".into(),
            effort: "low".into(),
            extra_args: "--sandbox".into(),
            ..Default::default()
        };
        let args = build_args(&cfg, &req());
        assert_eq!(args[0], "--print");
        assert!(args[1].starts_with("SYS\n\n---\n\nUSER"));
        assert!(args.windows(2).any(|w| w == ["--output-format", "stream-json"]));
        assert!(args.windows(2).any(|w| w == ["--model", "claude-sonnet-4-6"]));
        assert!(args.windows(2).any(|w| w == ["--effort", "low"]));
        assert!(args.windows(2).any(|w| w == ["--conversation", "abc"]));
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert_eq!(args.last().unwrap(), "--sandbox");
    }

    #[test]
    fn agy_skips_effort_when_model_name_embeds_it() {
        assert!(model_embeds_effort("gemini-3.8-flash-high"));
        assert!(model_embeds_effort("gemini-3.1-pro-low "));
        assert!(!model_embeds_effort("claude-sonnet-4-6"));
        assert!(!model_embeds_effort(""));

        let cfg = CliAgentConfig {
            kind: CliAgentKind::Antigravity,
            model: "gemini-3.8-flash-high".into(),
            effort: "medium".into(),
            ..Default::default()
        };
        let args = build_args(&cfg, &req());
        assert!(!args.contains(&"--effort".to_string()));

        let cfg = CliAgentConfig {
            kind: CliAgentKind::Antigravity,
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            ..Default::default()
        };
        let args = build_args(&cfg, &req());
        assert!(args.windows(2).any(|w| w == ["--effort", "medium"]));
    }

    #[test]
    fn claude_args_use_system_prompt_flag_and_mcp_config() {
        let cfg = CliAgentConfig {
            kind: CliAgentKind::ClaudeCode,
            model: "sonnet".into(),
            ..Default::default()
        };
        let args = build_args(&cfg, &req());
        assert_eq!(&args[..2], ["-p", "USER"]);
        assert!(args.windows(2).any(|w| w == ["--append-system-prompt", "SYS"]));
        assert!(args.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(args.windows(2).any(|w| w == ["--mcp-config", "/tmp/mcp.json"]));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args.windows(2).any(|w| w == ["--allowedTools", "mcp__tabular"]));
        assert!(args.contains(&"--include-partial-messages".to_string()));
    }

    #[test]
    fn claude_args_without_mcp_or_session() {
        let cfg = CliAgentConfig {
            kind: CliAgentKind::ClaudeCode,
            ..Default::default()
        };
        let mut r = req();
        r.session_id = None;
        r.mcp_config = None;
        let args = build_args(&cfg, &r);
        assert!(!args.contains(&"--resume".to_string()));
        assert!(!args.contains(&"--mcp-config".to_string()));
    }

    #[test]
    fn custom_template_substitutes_placeholders() {
        let cfg = CliAgentConfig {
            kind: CliAgentKind::Custom,
            bin: "mytool".into(),
            model: "m1".into(),
            extra_args: "run --model {model} --sys {system} {prompt}".into(),
            ..Default::default()
        };
        let args = build_args(&cfg, &req());
        assert_eq!(args, vec!["run", "--model", "m1", "--sys", "SYS", "USER"]);

        let cfg2 = CliAgentConfig {
            kind: CliAgentKind::Custom,
            extra_args: "chat".into(),
            ..Default::default()
        };
        let args2 = build_args(&cfg2, &req());
        assert_eq!(args2[0], "chat");
        assert!(args2[1].contains("USER"));
    }

    #[test]
    fn agy_parser_streams_and_finishes() {
        let mut p = StreamParser::new(CliAgentKind::Antigravity);
        let init = r#"{"event":"init","conversation_id":"1e16","init":{"model":"x","tools":[]}}"#;
        assert_eq!(p.feed_line(init), vec![AgentEvent::Session("1e16".into())]);
        let d1 = r#"{"event":"step_update","step_update":{"conversation_id":"1e16","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"OK"}}"#;
        assert_eq!(p.feed_line(d1), vec![AgentEvent::TextDelta("OK".into())]);
        let tool = r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool_call","tool_name":"call_mcp_tool"}}"#;
        assert_eq!(p.feed_line(tool), vec![AgentEvent::ToolUse("call_mcp_tool".into())]);
        let d2 = r#"{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"\n","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        assert_eq!(p.feed_line(d2), vec![AgentEvent::TextDelta("\n".into())]);
        let res = r#"{"event":"result","result":{"conversation_id":"1e16","status":"SUCCESS","response":"OK\n","duration_seconds":2.3,"usage":{"input_tokens":13564,"output_tokens":1}}}"#;
        let evs = p.feed_line(res);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Done { text, usage } => {
                assert_eq!(text, "OK\n");
                assert_eq!(usage.as_deref(), Some("13564 in / 1 out tokens · 2.3s"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(p.is_finished());
        assert!(p.finish().is_none());
    }

    #[test]
    fn agy_parser_reports_failed_status() {
        let mut p = StreamParser::new(CliAgentKind::Antigravity);
        let res = r#"{"event":"result","result":{"status":"ERROR","error":"quota exceeded"}}"#;
        assert_eq!(
            p.feed_line(res),
            vec![AgentEvent::Error("agy finished with status ERROR: quota exceeded".into())]
        );
    }

    #[test]
    fn claude_parser_prefers_stream_deltas_over_assistant_message() {
        let mut p = StreamParser::new(CliAgentKind::ClaudeCode);
        let init = r#"{"type":"system","subtype":"init","session_id":"s-1"}"#;
        assert_eq!(p.feed_line(init), vec![AgentEvent::Session("s-1".into())]);
        let start = r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"mcp__tabular__run_query"}}}"#;
        assert_eq!(p.feed_line(start), vec![AgentEvent::ToolUse("mcp__tabular__run_query".into())]);
        let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#;
        assert_eq!(p.feed_line(delta), vec![AgentEvent::TextDelta("Hel".into())]);
        let delta2 = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}}"#;
        p.feed_line(delta2);
        // Pesan assistant lengkap tidak boleh menggandakan teks.
        let asst = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]}}"#;
        assert!(p.feed_line(asst).is_empty());
        let res = r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","session_id":"s-1","total_cost_usd":0.01,"usage":{"input_tokens":10,"output_tokens":2},"duration_ms":1500}"#;
        let evs = p.feed_line(res);
        match &evs[0] {
            AgentEvent::Done { text, usage } => {
                assert_eq!(text, "Hello");
                assert_eq!(usage.as_deref(), Some("10 in / 2 out tokens · 1.5s · $0.0100"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn claude_parser_uses_assistant_text_when_no_partials() {
        let mut p = StreamParser::new(CliAgentKind::ClaudeCode);
        let asst = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"},{"type":"tool_use","name":"Read"}]}}"#;
        assert_eq!(
            p.feed_line(asst),
            vec![AgentEvent::TextDelta("Hi".into()), AgentEvent::ToolUse("Read".into())]
        );
        let err = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["boom"]}"#;
        assert_eq!(p.feed_line(err), vec![AgentEvent::Error("boom".into())]);
    }

    #[test]
    fn gemini_parser_handles_messages_and_result() {
        let mut p = StreamParser::new(CliAgentKind::GeminiCli);
        p.feed_line(r#"{"type":"init","session_id":"g1"}"#);
        assert_eq!(
            p.feed_line(r#"{"type":"message","role":"assistant","content":"Hi","delta":true}"#),
            vec![AgentEvent::TextDelta("Hi".into())]
        );
        assert_eq!(
            p.feed_line(r#"{"type":"tool_use","tool_name":"run_query"}"#),
            vec![AgentEvent::ToolUse("run_query".into())]
        );
        let evs = p.feed_line(r#"{"type":"result","status":"success","stats":{"total_tokens":42}}"#);
        assert_eq!(
            evs,
            vec![AgentEvent::Done { text: "Hi".into(), usage: Some("42 tokens".into()) }]
        );
    }

    #[test]
    fn custom_parser_streams_plain_lines() {
        let mut p = StreamParser::new(CliAgentKind::Custom);
        assert_eq!(p.feed_line("line one"), vec![AgentEvent::TextDelta("line one\n".into())]);
        assert_eq!(
            p.finish(),
            Some(AgentEvent::Done { text: "line one\n".into(), usage: None })
        );
    }

    #[test]
    fn json_parser_falls_back_to_raw_lines_when_no_events() {
        let mut p = StreamParser::new(CliAgentKind::Antigravity);
        assert!(p.feed_line("Some plain error text").is_empty());
        assert_eq!(
            p.finish(),
            Some(AgentEvent::Done { text: "Some plain error text".into(), usage: None })
        );
    }

    #[test]
    fn login_problem_detection() {
        assert!(detect_login_problem(CliAgentKind::Antigravity, "AUTHENTICATION_REQUIRED: expired").is_some());
        assert!(detect_login_problem(CliAgentKind::ClaudeCode, "Please run /login").is_some());
        assert!(detect_login_problem(CliAgentKind::ClaudeCode, "all good").is_none());
    }

    #[test]
    fn mcp_list_detection() {
        assert!(mcp_list_mentions_tabular("tabular: /Applications/Tabular.app/Contents/MacOS/tabular mcp"));
        assert!(mcp_list_mentions_tabular("  tabular  stdio  enabled"));
        assert!(!mcp_list_mentions_tabular("No MCP servers configured."));
        assert!(!mcp_list_mentions_tabular("other: npx something"));
    }

    #[test]
    fn sandbox_detection_from_env_value() {
        use std::ffi::OsStr;
        assert!(!sandboxed_from_env(None));
        assert!(!sandboxed_from_env(Some(OsStr::new(""))));
        assert_eq!(
            sandboxed_from_env(Some(OsStr::new("id.tabular.database"))),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn resolve_binary_rejects_missing_paths() {
        assert!(resolve_binary("").is_none());
        assert!(resolve_binary("/definitely/not/here/agy").is_none());
        assert!(resolve_binary("/bin/sh").is_some());
    }

    /// Memanggil `agy` sungguhan (butuh binary + login). Jalankan dengan
    /// `cargo test --lib -- --ignored real_agy`.
    #[test]
    #[ignore]
    fn real_agy_smoke() {
        let cfg = CliAgentConfig {
            kind: CliAgentKind::Antigravity,
            model: "gemini-3.8-flash-low".into(),
            effort: "low".into(),
            ..Default::default()
        };
        let out = test_connection(&cfg).expect("agy test_connection");
        eprintln!("{out}");
        assert!(out.contains("Reply: OK"), "unexpected reply: {out}");
    }
}
