use crate::models;
use log::debug;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct TunnelProcess {
    child: Child,
    stderr: Option<ChildStderr>,
    local_port: u16,
    last_used: Instant,
}

impl TunnelProcess {
    fn new(child: Child, stderr: Option<ChildStderr>, local_port: u16) -> Self {
        Self {
            child,
            stderr,
            local_port,
            last_used: Instant::now(),
        }
    }

    fn touch(&mut self) {
        self.last_used = Instant::now();
    }

    fn local_port(&self) -> u16 {
        self.local_port
    }

    fn check_alive(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr_msg = String::new();
                if let Some(stderr) = self.stderr.as_mut() {
                    let _ = stderr.read_to_string(&mut stderr_msg);
                }
                let detail = if stderr_msg.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr_msg.trim())
                };
                Err(format!(
                    "SSH tunnel exited with status {}{}",
                    status, detail
                ))
            }
            Ok(None) => Ok(()),
            Err(e) => Err(format!("Failed to poll SSH tunnel: {e}")),
        }
    }

    fn terminate(mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => (),
            Ok(None) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

static TUNNELS: Lazy<Mutex<HashMap<String, TunnelProcess>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn allocate_local_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("Failed to allocate local port: {e}"))
        .map(|listener| listener.local_addr().unwrap().port())
}

fn make_key(connection: &models::structs::ConnectionConfig) -> Result<String, String> {
    if let Some(id) = connection.id {
        Ok(format!("id:{id}"))
    } else {
        if connection.ssh_host.trim().is_empty()
            || connection.ssh_username.trim().is_empty()
            || connection.host.trim().is_empty()
        {
            return Err(
                "SSH tunnel requires SSH host, SSH username, and database host".to_string(),
            );
        }
        Ok(format!(
            "tmp:{}@{}:{}:{}->{:?}:{}:{}",
            connection.ssh_username.trim(),
            connection.ssh_host.trim(),
            connection.ssh_port.trim(),
            connection.ssh_auth_method.as_db_value(),
            connection.connection_type,
            connection.host.trim(),
            connection.port.trim()
        ))
    }
}

fn parse_remote_port(connection: &models::structs::ConnectionConfig) -> Result<u16, String> {
    connection
        .port
        .trim()
        .parse::<u16>()
        .map_err(|_| "Database port must be a valid number when using SSH tunnel".to_string())
}

fn parse_ssh_port(ssh_port: &str) -> String {
    let trimmed = ssh_port.trim();
    if trimmed.is_empty() {
        "22".to_string()
    } else {
        trimmed.to_string()
    }
}

fn spawn_tunnel(
    connection: &models::structs::ConnectionConfig,
    local_port: u16,
    ssh_port: &str,
    key: &str,
) -> Result<TunnelProcess, String> {
    let remote_port = parse_remote_port(connection)?;
    let use_password = matches!(
        connection.ssh_auth_method,
        models::enums::SshAuthMethod::Password
    );

    if use_password && connection.ssh_password.trim().is_empty() {
        return Err("SSH password cannot be empty when using password authentication".to_string());
    }

    let binary = if use_password { "sshpass" } else { "ssh" };
    let mut command = Command::new(binary);

    if use_password {
        command.arg("-p").arg(connection.ssh_password.trim());
        command.arg("ssh");
    }

    command.arg("-N");
    command.arg("-o").arg("ExitOnForwardFailure=yes");
    command.arg("-o").arg("ServerAliveInterval=30");
    command.arg("-o").arg("ServerAliveCountMax=3");
    command.arg("-o").arg("ConnectTimeout=15");
    if use_password {
        command.arg("-o").arg("BatchMode=no");
        command.arg("-o").arg("PreferredAuthentications=password");
        command.arg("-o").arg("PubkeyAuthentication=no");
    } else {
        command.arg("-o").arg("BatchMode=yes");
    }
    if connection.ssh_accept_unknown_host_keys {
        command.arg("-o").arg("StrictHostKeyChecking=no");
        command.arg("-o").arg("UserKnownHostsFile=/dev/null");
    }
    command.arg("-L").arg(format!(
        "{}:{}:{}",
        local_port,
        connection.host.trim(),
        remote_port
    ));
    command.arg("-p").arg(ssh_port);
    if !use_password && !connection.ssh_private_key.trim().is_empty() {
        command.arg("-i").arg(connection.ssh_private_key.trim());
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());
    command.arg(format!(
        "{}@{}",
        connection.ssh_username.trim(),
        connection.ssh_host.trim()
    ));

    debug!(
        "Starting SSH tunnel for key {} -> {}:{} via {}:{}",
        key,
        connection.host.trim(),
        remote_port,
        connection.ssh_host.trim(),
        ssh_port
    );

    let mut child = command.spawn().map_err(|e| {
        if use_password {
            format!("Failed to start sshpass process: {e}")
        } else {
            format!("Failed to start ssh process: {e}")
        }
    })?;
    let stderr = child.stderr.take();

    // Give ssh a brief moment to establish the tunnel and report errors.
    std::thread::sleep(Duration::from_millis(250));
    match child.try_wait() {
        Ok(Some(status)) => {
            let mut stderr_msg = String::new();
            if let Some(mut stderr_handle) = stderr {
                let _ = stderr_handle.read_to_string(&mut stderr_msg);
            }
            return Err(format!(
                "SSH tunnel exited immediately with status {}{}",
                status,
                if stderr_msg.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr_msg.trim())
                }
            ));
        }
        Ok(None) => {}
        Err(e) => {
            let _ = child.kill();
            return Err(format!("Failed to poll ssh process: {e}"));
        }
    }

    Ok(TunnelProcess::new(child, stderr, local_port))
}

fn ensure_tunnel_internal(connection: &models::structs::ConnectionConfig) -> Result<u16, String> {
    if connection.ssh_host.trim().is_empty() {
        return Err("SSH host cannot be empty".to_string());
    }
    if connection.ssh_username.trim().is_empty() {
        return Err("SSH username cannot be empty".to_string());
    }
    if connection.host.trim().is_empty() {
        return Err("Database host cannot be empty when using SSH".to_string());
    }
    if matches!(
        connection.ssh_auth_method,
        models::enums::SshAuthMethod::Password
    ) && connection.ssh_password.trim().is_empty()
    {
        return Err("SSH password cannot be empty when using password authentication".to_string());
    }

    let key = make_key(connection)?;
    let mut registry = TUNNELS
        .lock()
        .map_err(|_| "Failed to lock SSH tunnel registry".to_string())?;

    if let Some(process) = registry.get_mut(&key) {
        match process.check_alive() {
            Ok(()) => {
                process.touch();
                return Ok(process.local_port());
            }
            Err(err) => {
                debug!(
                    "SSH tunnel for key {} died. Removing and recreating: {}",
                    key, err
                );
                let old = registry.remove(&key);
                if let Some(process) = old {
                    process.terminate();
                }
            }
        }
    }

    let local_port = allocate_local_port()?;
    let ssh_port = parse_ssh_port(&connection.ssh_port);
    let process = spawn_tunnel(connection, local_port, &ssh_port, &key)?;
    let port = process.local_port();
    registry.insert(key.clone(), process);
    Ok(port)
}

pub fn ensure_tunnel(connection: &models::structs::ConnectionConfig) -> Result<u16, String> {
    if !connection.ssh_enabled {
        return Err("SSH tunnel is not enabled for this connection".to_string());
    }
    ensure_tunnel_internal(connection)
}

pub fn shutdown_for_connection(connection: &models::structs::ConnectionConfig) {
    let Ok(mut registry) = TUNNELS.lock() else {
        return;
    };
    let Ok(key) = make_key(connection) else {
        return;
    };
    let Some(process) = registry.remove(&key) else {
        return;
    };
    debug!("Shutting down SSH tunnel for key {}", key);
    process.terminate();
}

pub fn shutdown_by_id(connection_id: i64) {
    let key = format!("id:{connection_id}");
    let Ok(mut registry) = TUNNELS.lock() else {
        return;
    };
    let Some(process) = registry.remove(&key) else {
        return;
    };
    debug!("Shutting down SSH tunnel for key {}", key);
    process.terminate();
}

pub fn active_local_port(connection: &models::structs::ConnectionConfig) -> Option<u16> {
    let key = make_key(connection).ok()?;
    let mut registry = TUNNELS.lock().ok()?;
    let process = registry.get_mut(&key)?;
    if process.check_alive().is_ok() {
        process.touch();
        Some(process.local_port())
    } else {
        registry.remove(&key);
        None
    }
}

pub fn cleanup_idle_tunnels(max_idle: Duration) {
    if let Ok(mut registry) = TUNNELS.lock() {
        let now = Instant::now();
        let mut stale_keys = Vec::new();
        for (key, process) in registry.iter_mut() {
            if process.last_used + max_idle < now {
                stale_keys.push(key.clone());
            }
        }
        for key in stale_keys {
            if let Some(process) = registry.remove(&key) {
                debug!("Auto-closing idle SSH tunnel for key {}", key);
                process.terminate();
            }
        }
    }
}
