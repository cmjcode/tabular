use crate::models;
use log::debug;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex};
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

/// One lock per tunnel key. Two attempts on the *same* connection still
/// serialize, but attempts on different connections no longer queue behind each
/// other — previously the registry lock was held across `spawn_tunnel`, so a
/// tunnel with `ConnectTimeout=15` stalled every other SSH connection for 15s.
static KEY_LOCKS: Lazy<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn key_lock(key: &str) -> Arc<Mutex<()>> {
    let mut locks = KEY_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    locks
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn lock_registry()
-> Result<std::sync::MutexGuard<'static, HashMap<String, TunnelProcess>>, String> {
    TUNNELS
        .lock()
        .map_err(|_| "Failed to lock SSH tunnel registry".to_string())
}

/// `TunnelProcess::terminate` waits on the child, so it must never run on a
/// caller thread that could be the UI thread.
fn terminate_detached(process: TunnelProcess) {
    std::thread::spawn(move || process.terminate());
}

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
            "tmp:{}@{}:{}:{}:jump[{}]->{:?}:{}:{}",
            connection.ssh_username.trim(),
            connection.ssh_host.trim(),
            connection.ssh_port.trim(),
            connection.ssh_auth_method.as_db_value(),
            connection.ssh_jump_host.trim(),
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

/// Builds the argument list for standard OpenSSH client, supporting multi-hop jump hosts.
pub fn build_ssh_args(
    connection: &models::structs::ConnectionConfig,
    local_port: u16,
    ssh_port: &str,
) -> Result<Vec<String>, String> {
    let remote_port = parse_remote_port(connection)?;
    let use_password = matches!(
        connection.ssh_auth_method,
        models::enums::SshAuthMethod::Password
    );

    let mut args = Vec::new();
    args.push("-N".to_string());
    args.push("-o".to_string());
    args.push("ExitOnForwardFailure=yes".to_string());
    args.push("-o".to_string());
    args.push("ServerAliveInterval=30".to_string());
    args.push("-o".to_string());
    args.push("ServerAliveCountMax=3".to_string());
    args.push("-o".to_string());
    args.push("ConnectTimeout=15".to_string());

    if use_password {
        args.push("-o".to_string());
        args.push("BatchMode=no".to_string());
        args.push("-o".to_string());
        args.push("PreferredAuthentications=password".to_string());
        args.push("-o".to_string());
        args.push("PubkeyAuthentication=no".to_string());
    } else {
        args.push("-o".to_string());
        args.push("BatchMode=yes".to_string());
    }

    if connection.ssh_accept_unknown_host_keys {
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=no".to_string());
        args.push("-o".to_string());
        args.push("UserKnownHostsFile=/dev/null".to_string());
    }

    // Enterprise Multi-Hop Jump Host Support (ProxyJump / -J)
    let jump_host = connection.ssh_jump_host.trim();
    if !jump_host.is_empty() {
        args.push("-J".to_string());
        args.push(jump_host.to_string());
    }

    args.push("-L".to_string());
    args.push(format!(
        "{}:{}:{}",
        local_port,
        connection.host.trim(),
        remote_port
    ));

    args.push("-p".to_string());
    args.push(ssh_port.to_string());

    if !use_password && !connection.ssh_private_key.trim().is_empty() {
        args.push("-i".to_string());
        args.push(connection.ssh_private_key.trim().to_string());
    }

    args.push(format!(
        "{}@{}",
        connection.ssh_username.trim(),
        connection.ssh_host.trim()
    ));

    Ok(args)
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

    let ssh_args = build_ssh_args(connection, local_port, ssh_port)?;

    let binary = if use_password { "sshpass" } else { "ssh" };
    let mut command = Command::new(binary);

    if use_password {
        command.arg("-p").arg(connection.ssh_password.trim());
        command.arg("ssh");
    }

    for arg in &ssh_args {
        command.arg(arg);
    }

    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());

    debug!(
        "Starting SSH tunnel for key {} -> {}:{} via {}:{} (jump: {})",
        key,
        connection.host.trim(),
        remote_port,
        connection.ssh_host.trim(),
        ssh_port,
        if connection.ssh_jump_host.trim().is_empty() { "none" } else { connection.ssh_jump_host.trim() }
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

    // Held for the whole attempt so two callers don't spawn duplicate tunnels for
    // the same key; scoped per key so unrelated connections stay unaffected.
    let key_guard = key_lock(&key);
    let _key_guard = key_guard.lock().unwrap_or_else(|e| e.into_inner());

    // Short critical section: reuse a live tunnel, or evict a dead one.
    let mut dead: Option<TunnelProcess> = None;
    {
        let mut registry = lock_registry()?;
        let mut evict = false;
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
                    evict = true;
                }
            }
        }
        if evict {
            dead = registry.remove(&key);
        }
    }

    // Reaping the dead process happens outside the registry lock.
    if let Some(process) = dead {
        terminate_detached(process);
    }

    let local_port = allocate_local_port()?;
    let ssh_port = parse_ssh_port(&connection.ssh_port);
    // `spawn_tunnel` sleeps ~250ms waiting for ssh to report an early failure, and
    // ssh itself may take up to `ConnectTimeout`. The registry lock stays free
    // throughout.
    let process = spawn_tunnel(connection, local_port, &ssh_port, &key)?;
    let port = process.local_port();
    lock_registry()?.insert(key, process);
    Ok(port)
}

pub fn ensure_tunnel(connection: &models::structs::ConnectionConfig) -> Result<u16, String> {
    if !connection.ssh_enabled {
        return Err("SSH tunnel is not enabled for this connection".to_string());
    }
    ensure_tunnel_internal(connection)
}

pub fn shutdown_for_connection(connection: &models::structs::ConnectionConfig) {
    let Ok(key) = make_key(connection) else {
        return;
    };
    shutdown_key(key);
}

pub fn shutdown_by_id(connection_id: i64) {
    shutdown_key(format!("id:{connection_id}"));
}

/// Remove and kill a tunnel without ever blocking the caller. Disconnect is
/// driven from the UI thread, and the registry may be busy while another
/// connection is spawning its tunnel — waiting on it would freeze the app.
fn shutdown_key(key: String) {
    match TUNNELS.try_lock() {
        Ok(mut registry) => {
            let removed = registry.remove(&key);
            drop(registry);
            if let Some(process) = removed {
                debug!("Shutting down SSH tunnel for key {}", key);
                terminate_detached(process);
            }
        }
        Err(_) => {
            std::thread::spawn(move || {
                let Ok(mut registry) = TUNNELS.lock() else {
                    return;
                };
                let removed = registry.remove(&key);
                drop(registry);
                if let Some(process) = removed {
                    debug!("Shutting down SSH tunnel for key {}", key);
                    process.terminate();
                }
            });
        }
    }
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
                // Terminate off-thread so reaping children doesn't hold the registry.
                terminate_detached(process);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::{DatabaseType, SshAuthMethod};

    #[test]
    fn test_build_ssh_args_basic_key() {
        let mut conn = models::structs::ConnectionConfig::default();
        conn.host = "192.168.1.100".to_string();
        conn.port = "5432".to_string();
        conn.connection_type = DatabaseType::PostgreSQL;
        conn.ssh_enabled = true;
        conn.ssh_host = "bastion.example.com".to_string();
        conn.ssh_port = "2222".to_string();
        conn.ssh_username = "ubuntu".to_string();
        conn.ssh_auth_method = SshAuthMethod::Key;
        conn.ssh_private_key = "/home/user/.ssh/id_ed25519".to_string();

        let args = build_ssh_args(&conn, 54321, "2222").unwrap();
        assert!(args.contains(&"-N".to_string()));
        assert!(args.contains(&"-L".to_string()));
        assert!(args.contains(&"54321:192.168.1.100:5432".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/home/user/.ssh/id_ed25519".to_string()));
        assert!(args.contains(&"ubuntu@bastion.example.com".to_string()));
    }

    #[test]
    fn test_build_ssh_args_jump_host() {
        let mut conn = models::structs::ConnectionConfig::default();
        conn.host = "db-internal.lan".to_string();
        conn.port = "3306".to_string();
        conn.connection_type = DatabaseType::MySQL;
        conn.ssh_enabled = true;
        conn.ssh_host = "private-app-server.lan".to_string();
        conn.ssh_port = "22".to_string();
        conn.ssh_username = "deploy".to_string();
        conn.ssh_auth_method = SshAuthMethod::Key;
        conn.ssh_private_key = "/keys/app.pem".to_string();
        conn.ssh_jump_host = "bastion-gateway.corp.com:2222".to_string();

        let args = build_ssh_args(&conn, 33060, "22").unwrap();
        assert!(args.contains(&"-J".to_string()));
        assert!(args.contains(&"bastion-gateway.corp.com:2222".to_string()));
        assert!(args.contains(&"33060:db-internal.lan:3306".to_string()));
        assert!(args.contains(&"deploy@private-app-server.lan".to_string()));
    }
}

