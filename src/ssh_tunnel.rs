use crate::models;
use log::debug;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
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

/// Batas waktu sampai port lokal tunnel menerima koneksi. Sama dengan
/// `ConnectTimeout` yang diberikan ke `ssh`.
const TUNNEL_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Batas satu percobaan `connect` ke port lokal saat menunggu tunnel siap.
const TUNNEL_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Jeda antar percobaan saat port lokal belum dibuka oleh `ssh`.
const TUNNEL_PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// Status proses tunnel sebuah koneksi di registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// Proses `ssh` masih berjalan.
    Alive,
    /// Proses `ssh` sudah keluar; entry-nya sudah dibuang dari registry.
    Dead,
    /// Tidak ada tunnel terdaftar untuk koneksi ini.
    Missing,
    /// Registry sedang dipakai thread lain; status tidak diketahui.
    Unknown,
}

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

/// Kunci registry. Mutex yang ter-poison tetap dipakai: isinya hanya daftar
/// proses anak, dan menganggapnya "kosong" justru membuat proses `ssh` bocor.
fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<String, TunnelProcess>> {
    TUNNELS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `TunnelProcess::terminate` waits on the child, so it must never run on a
/// caller thread that could be the UI thread.
fn terminate_detached(process: TunnelProcess) {
    std::thread::spawn(move || process.terminate());
}

fn allocate_local_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("Failed to allocate local port: {e}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read allocated local port: {e}"))
}

/// Baca stderr proses yang SUDAH keluar dan format sebagai akhiran pesan error.
fn stderr_detail(stderr: Option<&mut ChildStderr>) -> String {
    let mut message = String::new();
    if let Some(handle) = stderr {
        let _ = handle.read_to_string(&mut message);
    }
    if message.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", message.trim())
    }
}

/// Tunggu sampai port lokal tunnel menerima koneksi TCP.
///
/// `ssh -N -L` baru mendengarkan di port lokal setelah autentikasi selesai,
/// jadi "proses masih hidup setelah 250 ms" bukan tanda tunnel siap: driver
/// yang langsung konek akan mendapat "connection refused". Loop ini berhenti
/// saat (a) port menerima koneksi, (b) proses `ssh` keluar — stderr-nya
/// dikembalikan sebagai error, atau (c) `timeout` terlewati.
fn wait_until_ready(
    child: &mut Child,
    stderr: &mut Option<ChildStderr>,
    local_port: u16,
    timeout: Duration,
) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], local_port));
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "SSH tunnel exited with status {}{}",
                    status,
                    stderr_detail(stderr.as_mut())
                ));
            }
            Ok(None) => {}
            Err(e) => return Err(format!("Failed to poll ssh process: {e}")),
        }

        if TcpStream::connect_timeout(&addr, TUNNEL_PROBE_TIMEOUT).is_ok() {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "SSH tunnel did not become ready within {} seconds",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(TUNNEL_PROBE_INTERVAL);
    }
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

    let mut args: Vec<String> = [
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ConnectTimeout=15",
    ]
    .into_iter()
    .map(String::from)
    .collect();

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
        // `sshpass -e` membaca password dari env `SSHPASS` milik proses anak.
        // `-p <password>` menaruhnya di argv, yang terlihat oleh semua user
        // lewat `ps`.
        command.arg("-e");
        command.env("SSHPASS", connection.ssh_password.trim());
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
        if connection.ssh_jump_host.trim().is_empty() {
            "none"
        } else {
            connection.ssh_jump_host.trim()
        }
    );

    let mut child = command.spawn().map_err(|e| {
        if use_password {
            format!("Failed to start sshpass process: {e}")
        } else {
            format!("Failed to start ssh process: {e}")
        }
    })?;
    let mut stderr = child.stderr.take();

    if let Err(err) = wait_until_ready(&mut child, &mut stderr, local_port, TUNNEL_READY_TIMEOUT) {
        // Proses yang belum keluar (timeout / gagal polling) harus dimatikan
        // dan di-reap supaya tidak menjadi zombie.
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Err(err);
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
        let mut registry = lock_registry();
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
    // `spawn_tunnel` menunggu sampai port lokal benar-benar menerima koneksi
    // (paling lama `TUNNEL_READY_TIMEOUT`). Lock registry tetap bebas selama itu.
    let process = spawn_tunnel(connection, local_port, &ssh_port, &key)?;
    let port = process.local_port();
    lock_registry().insert(key, process);
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
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            let removed = poisoned.into_inner().remove(&key);
            if let Some(process) = removed {
                debug!("Shutting down SSH tunnel for key {}", key);
                terminate_detached(process);
            }
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            std::thread::spawn(move || {
                let mut registry = lock_registry();
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
    let mut registry = lock_registry();
    let process = registry.get_mut(&key)?;
    if process.check_alive().is_ok() {
        process.touch();
        Some(process.local_port())
    } else {
        let dead = registry.remove(&key);
        drop(registry);
        if let Some(process) = dead {
            terminate_detached(process);
        }
        None
    }
}

/// Status tunnel untuk koneksi tersimpan `connection_id`.
///
/// Tidak pernah memblokir: aman dipanggil dari thread UI. Bila registry sedang
/// dipakai thread lain hasilnya [`TunnelState::Unknown`]. Tunnel yang ternyata
/// sudah mati langsung dibuang dari registry supaya `ensure_tunnel` berikutnya
/// membuat yang baru.
pub fn tunnel_state_by_id(connection_id: i64) -> TunnelState {
    let key = format!("id:{connection_id}");
    let mut registry = match TUNNELS.try_lock() {
        Ok(registry) => registry,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return TunnelState::Unknown,
    };
    let Some(process) = registry.get_mut(&key) else {
        return TunnelState::Missing;
    };
    match process.check_alive() {
        Ok(()) => TunnelState::Alive,
        Err(err) => {
            debug!("SSH tunnel for key {} is dead: {}", key, err);
            // Proses sudah keluar dan sudah di-reap oleh `try_wait`, jadi
            // men-drop entry ini tidak memblokir.
            registry.remove(&key);
            TunnelState::Dead
        }
    }
}

/// True jika koneksi tersimpan `connection_id` punya tunnel yang prosesnya
/// masih berjalan.
pub fn is_tunnel_alive(connection_id: i64) -> bool {
    tunnel_state_by_id(connection_id) == TunnelState::Alive
}

/// Matikan SEMUA tunnel dan tunggu sampai setiap proses `ssh` benar-benar
/// keluar. Sinkron — dipanggil saat aplikasi (atau server MCP) berhenti, ketika
/// thread detached tidak lagi dijamin sempat berjalan. Tanpa ini proses `ssh`
/// menjadi yatim dan terus menahan port lokal setelah aplikasi ditutup.
///
/// Mengembalikan jumlah tunnel yang dimatikan.
pub fn shutdown_all() -> usize {
    let processes: Vec<(String, TunnelProcess)> = lock_registry().drain().collect();
    terminate_all(processes)
}

/// Kill + wait setiap proses secara sinkron. Dipisah dari [`shutdown_all`]
/// supaya bisa diuji tanpa menyentuh registry global.
fn terminate_all(processes: Vec<(String, TunnelProcess)>) -> usize {
    let count = processes.len();
    for (key, process) in processes {
        debug!("Shutting down SSH tunnel for key {} (shutdown_all)", key);
        process.terminate();
    }
    count
}

pub fn cleanup_idle_tunnels(max_idle: Duration) {
    let stale: Vec<(String, TunnelProcess)> = {
        let mut registry = lock_registry();
        let now = Instant::now();
        let stale_keys: Vec<String> = registry
            .iter()
            .filter(|(_, process)| process.last_used + max_idle < now)
            .map(|(key, _)| key.clone())
            .collect();
        stale_keys
            .into_iter()
            .filter_map(|key| registry.remove(&key).map(|process| (key, process)))
            .collect()
    };
    for (key, process) in stale {
        debug!("Auto-closing idle SSH tunnel for key {}", key);
        // Terminate off-thread so reaping children doesn't hold the registry.
        terminate_detached(process);
    }
}

#[cfg(test)]
// Test lebih mudah dibaca dengan pola Default lalu set field satu per satu.
#[allow(clippy::field_reassign_with_default)]
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
    fn allocated_port_is_usable() {
        let port = allocate_local_port().expect("free local port");
        assert_ne!(port, 0);
    }

    #[test]
    fn password_is_never_placed_on_the_command_line() {
        let mut conn = models::structs::ConnectionConfig::default();
        conn.host = "db.internal".to_string();
        conn.port = "5432".to_string();
        conn.ssh_host = "bastion.example.com".to_string();
        conn.ssh_username = "ubuntu".to_string();
        conn.ssh_auth_method = SshAuthMethod::Password;
        conn.ssh_password = "s3cr3t-pässword".to_string();

        let args = build_ssh_args(&conn, 40000, "22").unwrap();
        assert!(args.iter().all(|arg| !arg.contains("s3cr3t")));
        assert!(args.contains(&"PreferredAuthentications=password".to_string()));
    }

    /// Proses anak berumur panjang sebagai pengganti `ssh` di test.
    #[cfg(unix)]
    fn sleeper() -> Child {
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sleep")
    }

    #[cfg(unix)]
    #[test]
    fn readiness_wait_succeeds_once_the_port_accepts_connections() {
        // Listener mewakili `ssh -L` yang sudah siap.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut child = sleeper();
        let mut stderr = child.stderr.take();

        let result = wait_until_ready(&mut child, &mut stderr, port, Duration::from_secs(5));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(result, Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn readiness_wait_reports_stderr_when_the_process_exits() {
        let port = allocate_local_port().unwrap();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("echo 'Permission denied (publickey).' >&2; exit 255")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let mut stderr = child.stderr.take();

        let started = Instant::now();
        let err = wait_until_ready(&mut child, &mut stderr, port, Duration::from_secs(10))
            .expect_err("exited process must fail readiness");
        assert!(err.contains("Permission denied"), "{err}");
        assert!(err.contains("exited"), "{err}");
        // Harus berhenti begitu proses keluar, bukan menunggu timeout penuh.
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[cfg(unix)]
    #[test]
    fn readiness_wait_times_out_when_the_port_never_opens() {
        let port = allocate_local_port().unwrap();
        let mut child = sleeper();
        let mut stderr = child.stderr.take();

        let err = wait_until_ready(&mut child, &mut stderr, port, Duration::from_millis(400))
            .expect_err("closed port must time out");
        let _ = child.kill();
        let _ = child.wait();
        assert!(err.contains("did not become ready"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn terminate_all_kills_and_reaps_every_child() {
        let mut first = sleeper();
        let mut second = sleeper();
        let pids = [first.id(), second.id()];
        let processes = vec![
            ("a".to_string(), {
                let stderr = first.stderr.take();
                TunnelProcess::new(first, stderr, 1)
            }),
            ("b".to_string(), {
                let stderr = second.stderr.take();
                TunnelProcess::new(second, stderr, 2)
            }),
        ];

        assert_eq!(terminate_all(processes), 2);
        for pid in pids {
            // `kill -0` gagal bila proses sudah tidak ada (sudah di-reap).
            let alive = Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            assert!(!alive, "process {pid} should be gone");
        }
    }

    // Id negatif per test: TUNNELS bersifat global dan test berbagi proses.
    #[cfg(unix)]
    #[test]
    fn tunnel_state_tracks_the_child_process() {
        let id = -7101;
        let key = format!("id:{id}");
        assert_eq!(tunnel_state_by_id(id), TunnelState::Missing);
        assert!(!is_tunnel_alive(id));

        let mut child = sleeper();
        let stderr = child.stderr.take();
        lock_registry().insert(key.clone(), TunnelProcess::new(child, stderr, 1));
        assert_eq!(tunnel_state_by_id(id), TunnelState::Alive);
        assert!(is_tunnel_alive(id));

        // Matikan proses dari luar, seperti `ssh` yang putus sendiri.
        if let Some(process) = lock_registry().get_mut(&key) {
            let _ = process.child.kill();
            let _ = process.child.wait();
        }
        assert_eq!(tunnel_state_by_id(id), TunnelState::Dead);
        // Entry mati sudah dibuang, jadi berikutnya terbaca tidak ada.
        assert_eq!(tunnel_state_by_id(id), TunnelState::Missing);
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
