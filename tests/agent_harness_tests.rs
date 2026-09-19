//! Uji `agent::harness::spawn_stream` dengan binary CLI palsu (skrip shell)
//! yang memancarkan NDJSON ala `agy`, tanpa memerlukan agy/claude terpasang.

#![cfg(unix)]

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tabular::agent::harness::{AgentEvent, AgentRequest, CliAgentConfig, spawn_stream};
use tabular::config::CliAgentKind;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tabular-harness-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn request(cwd: PathBuf) -> AgentRequest {
    AgentRequest {
        system_prompt: "SYS".into(),
        user_prompt: "USER".into(),
        session_id: None,
        cwd,
        mcp_config: None,
    }
}

fn collect(rx: &std::sync::mpsc::Receiver<AgentEvent>, timeout: Duration) -> Vec<AgentEvent> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(ev) => {
                let done = matches!(ev, AgentEvent::Done { .. } | AgentEvent::Error(_));
                out.push(ev);
                if done {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    out
}

#[test]
fn fake_agy_streams_events_and_finishes() {
    let dir = temp_dir("stream");
    // Skrip mencetak argumen ke file supaya bisa diperiksa, lalu memancarkan NDJSON.
    let script = write_script(
        &dir,
        "fake-agy",
        r#"#!/bin/sh
printf '%s\n' "$@" > "$(dirname "$0")/args.txt"
echo '{"event":"init","conversation_id":"conv-42","init":{"model":"m"}}'
echo '{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"Hello"}}'
echo '{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool_call","tool_name":"call_mcp_tool"}}'
echo '{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":" world"}}'
echo '{"event":"result","result":{"status":"SUCCESS","response":"Hello world","duration_seconds":0.5,"usage":{"input_tokens":3,"output_tokens":2}}}'
"#,
    );
    let cfg = CliAgentConfig {
        kind: CliAgentKind::Antigravity,
        bin: script.to_string_lossy().to_string(),
        model: "test-model".into(),
        ..Default::default()
    };
    let (rx, _handle) = spawn_stream(&cfg, request(dir.clone())).expect("spawn");
    let events = collect(&rx, Duration::from_secs(10));

    assert_eq!(events[0], AgentEvent::Session("conv-42".into()));
    assert!(events.contains(&AgentEvent::TextDelta("Hello".into())));
    assert!(events.contains(&AgentEvent::ToolUse("call_mcp_tool".into())));
    match events.last().unwrap() {
        AgentEvent::Done { text, usage } => {
            assert_eq!(text, "Hello world");
            assert_eq!(usage.as_deref(), Some("3 in / 2 out tokens · 0.5s"));
        }
        other => panic!("unexpected last event {other:?}"),
    }

    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(
        args.contains("--print\nSYS\n\n---\n\nUSER\n"),
        "args: {args}"
    );
    assert!(args.contains("--output-format\nstream-json\n"));
    assert!(args.contains("--model\ntest-model\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nonzero_exit_without_result_becomes_error_with_login_hint() {
    let dir = temp_dir("fail");
    let script = write_script(
        &dir,
        "fake-agy",
        "#!/bin/sh\necho 'AUTHENTICATION_REQUIRED: session expired' >&2\nexit 3\n",
    );
    let cfg = CliAgentConfig {
        kind: CliAgentKind::Antigravity,
        bin: script.to_string_lossy().to_string(),
        ..Default::default()
    };
    let (rx, _handle) = spawn_stream(&cfg, request(dir.clone())).expect("spawn");
    let events = collect(&rx, Duration::from_secs(10));
    match events.last().unwrap() {
        AgentEvent::Error(msg) => assert!(msg.contains("not logged in"), "msg: {msg}"),
        other => panic!("unexpected {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_kills_a_hanging_cli() {
    let dir = temp_dir("cancel");
    let script = write_script(
        &dir,
        "fake-agy",
        "#!/bin/sh\necho '{\"event\":\"init\",\"conversation_id\":\"c\"}'\nsleep 30\n",
    );
    let cfg = CliAgentConfig {
        kind: CliAgentKind::Antigravity,
        bin: script.to_string_lossy().to_string(),
        ..Default::default()
    };
    let (rx, handle) = spawn_stream(&cfg, request(dir.clone())).expect("spawn");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        AgentEvent::Session("c".into())
    );
    let started = Instant::now();
    handle.cancel();
    let events = collect(&rx, Duration::from_secs(10));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "cancel did not stop the process"
    );
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Error("Stopped by user.".into()))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_binary_is_reported_before_spawn() {
    let cfg = CliAgentConfig {
        kind: CliAgentKind::Custom,
        bin: "/definitely/missing/tool".into(),
        extra_args: "{prompt}".into(),
        ..Default::default()
    };
    let err = spawn_stream(&cfg, request(std::env::temp_dir()))
        .err()
        .expect("should fail");
    assert!(err.contains("not found"), "err: {err}");
}
