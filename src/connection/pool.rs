use crate::{models, modules, ssh_tunnel, window_egui::Tabular};
use log::debug;
use mongodb::Client as MongoClient;
use once_cell::sync::Lazy;
use redis::aio::ConnectionManager;
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions, sqlite::SqlitePoolOptions};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Wall-clock ceiling for one full connect attempt, covering DNS, the TCP probe,
/// the SSH tunnel and the driver handshake. Without it a hung server keeps the
/// pool-creation task alive indefinitely and the connection stays wedged in
/// `pending_connection_pools` until the app restarts.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// DNS budget. Name resolution has no timeout of its own and can hang for ~30s
/// against an unresponsive resolver.
const DNS_TIMEOUT: Duration = Duration::from_secs(3);

/// Ceiling for a single driver handshake, matching the pre-existing MongoDB value.
pub(crate) const DRIVER_TIMEOUT: Duration = Duration::from_secs(10);

/// Batas waktu reset sesi (`after_release`) saat koneksi kembali ke pool.
/// Koneksi yang tidak bisa di-reset dalam waktu ini ditutup paksa supaya slot
/// pool tidak tertahan oleh query lama yang masih berjalan di server.
const SESSION_RESET_TIMEOUT: Duration = Duration::from_secs(5);

/// Kunci mutex dan tetap pakai isinya walau ter-poison. Semua registry di
/// modul ini hanya berisi map sederhana yang tetap konsisten setelah panic di
/// thread lain; membacanya sebagai "kosong" justru menyembunyikan pool yang
/// masih hidup (koneksi tampak terputus, tunnel dan pool bocor).
pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A pending pool creation older than this is treated as dead and released.
/// Deliberately a little past [`CONNECT_TIMEOUT`] so an attempt that is about to
/// report back on its own still gets the chance to.
const PENDING_POOL_MAX_AGE: Duration = Duration::from_secs(20);

/// How often an in-flight connect re-checks whether it has been cancelled. This
/// is the worst-case latency between the user asking to cancel and the attempt
/// actually unwinding.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Cancellation flags for in-flight connect attempts, keyed by connection id.
///
/// A process-global registry (same shape as `ssh_tunnel::TUNNELS`) rather than a
/// parameter, because connects are dispatched down two different paths — the
/// background worker thread and `runtime.spawn` — and only one of them can hand
/// a task handle back to the UI. Looking the flag up by `connection.id` reaches
/// both without changing the signature of every connect function.
static CANCEL_FLAGS: Lazy<Mutex<HashMap<i64, CancelEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

struct CancelEntry {
    flag: Arc<AtomicBool>,
    /// Kapan flag dinaikkan. Flag yang sudah dibatalkan dibiarkan di registry
    /// supaya attempt yang masih antre ikut melihatnya, tetapi hanya selama
    /// [`PENDING_POOL_MAX_AGE`]: setelah itu tidak mungkin ada attempt lama yang
    /// masih berjalan, dan flag basi tidak boleh membatalkan connect baru dari
    /// pemanggil yang tidak lewat `begin_connect_attempt` (agent, transfer data).
    cancelled_at: Option<std::time::Instant>,
}

/// Register a fresh, un-cancelled flag for a new attempt, replacing any flag
/// left over from a previous one.
pub(crate) fn begin_connect_attempt(connection_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    lock_or_recover(&CANCEL_FLAGS).insert(
        connection_id,
        CancelEntry {
            flag: flag.clone(),
            cancelled_at: None,
        },
    );
    flag
}

/// Ask the in-flight attempt for this connection to unwind. No-op if nothing is
/// running.
pub(crate) fn signal_connect_cancel(connection_id: i64) {
    if let Some(entry) = lock_or_recover(&CANCEL_FLAGS).get_mut(&connection_id) {
        entry.flag.store(true, Ordering::SeqCst);
        entry.cancelled_at.get_or_insert_with(std::time::Instant::now);
    }
}

fn current_cancel_flag(connection_id: i64) -> Option<Arc<AtomicBool>> {
    let mut flags = lock_or_recover(&CANCEL_FLAGS);
    let expired = flags
        .get(&connection_id)?
        .cancelled_at
        .is_some_and(|at| at.elapsed() > PENDING_POOL_MAX_AGE);
    if expired {
        flags.remove(&connection_id);
        return None;
    }
    flags.get(&connection_id).map(|entry| entry.flag.clone())
}

/// True if this connection's current attempt has been cancelled.
pub(crate) fn connect_was_cancelled(connection_id: i64) -> bool {
    current_cancel_flag(connection_id).is_some_and(|f| f.load(Ordering::SeqCst))
}

/// Forget a connection's flag once no attempt is outstanding.
fn end_connect_attempt(connection_id: i64) {
    lock_or_recover(&CANCEL_FLAGS).remove(&connection_id);
}

/// Resolves once the flag is raised. Raced against the connect attempt so that
/// cancelling drops the attempt's future instead of waiting for it to finish.
async fn wait_for_cancel(flag: Arc<AtomicBool>) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
    }
}

/// Host/port that a reachability probe should target: the SSH endpoint when
/// tunnelling, otherwise the database endpoint. `Ok(None)` means the probe does
/// not apply (SQLite, or a loopback host that is always reachable).
fn reachability_target(
    connection: &models::structs::ConnectionConfig,
) -> Result<Option<(String, String)>, String> {
    if connection.connection_type == models::enums::DatabaseType::SQLite {
        return Ok(None);
    }

    let (host, port_str) = if connection.ssh_enabled {
        let h = connection.ssh_host.trim();
        let p = if connection.ssh_port.trim().is_empty() {
            "22"
        } else {
            connection.ssh_port.trim()
        };
        if h.is_empty() {
            return Err("SSH host must not be empty".to_string());
        }
        (h, p)
    } else {
        let h = connection.host.trim();
        let p = if connection.port.trim().is_empty() {
            "3306"
        } else {
            connection.port.trim()
        };
        if h.is_empty() {
            return Err("Database host must not be empty".to_string());
        }
        (h, p)
    };

    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return Ok(None);
    }

    Ok(Some((host.to_string(), port_str.to_string())))
}

fn unreachable_error(host: &str, port_str: &str) -> String {
    format!(
        "Cannot reach host [{}:{}]: network unreachable (host offline).",
        host, port_str
    )
}

/// `ToSocketAddrs` cannot be interrupted, so resolve on a detached thread and
/// abandon the answer once the budget expires.
fn resolve_addrs_blocking(
    addr: &str,
    budget: Duration,
) -> Result<Vec<std::net::SocketAddr>, String> {
    use std::net::ToSocketAddrs;

    let (tx, rx) = std::sync::mpsc::channel();
    let owned = addr.to_string();
    std::thread::spawn(move || {
        let resolved = owned
            .to_socket_addrs()
            .map(|addrs| addrs.collect::<Vec<_>>())
            .map_err(|e| e.to_string());
        let _ = tx.send(resolved);
    });

    match rx.recv_timeout(budget) {
        Ok(Ok(addrs)) => Ok(addrs),
        Ok(Err(e)) => Err(format!("Network is not connected ({})", e)),
        Err(_) => Err(format!(
            "DNS did not respond within {} seconds",
            budget.as_secs()
        )),
    }
}

/// Check TCP reachability to host & port before attempting database driver connection.
/// Fails fast (within timeout_ms) if laptop has no network or host is unreachable.
///
/// Blocking variant, for callers that are genuinely synchronous. Async callers
/// must use [`check_host_reachability_async`] — this one cannot be cancelled by
/// `tokio::time::timeout` and would block a runtime worker thread.
#[allow(dead_code)]
pub(crate) fn check_host_reachability(
    connection: &models::structs::ConnectionConfig,
    timeout_ms: u64,
) -> Result<(), String> {
    use std::net::TcpStream;

    let Some((host, port_str)) = reachability_target(connection)? else {
        return Ok(());
    };

    let addr_str = format!("{}:{}", host, port_str);
    let socket_addrs = resolve_addrs_blocking(&addr_str, DNS_TIMEOUT)
        .map_err(|e| format!("Cannot resolve host '{}': {}", host, e))?;

    if socket_addrs.is_empty() {
        return Err(format!("Host '{}' is not valid", host));
    }

    // The budget covers the whole probe, not each address: a host with several
    // A-records used to multiply the wait by the number of addresses.
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    for addr in socket_addrs {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if TcpStream::connect_timeout(&addr, remaining).is_ok() {
            return Ok(());
        }
    }

    Err(unreachable_error(&host, &port_str))
}

/// Async reachability probe. Every step yields, so an enclosing
/// `tokio::time::timeout` (or a task abort) can actually cancel it.
#[allow(dead_code)]
pub(crate) async fn check_host_reachability_async(
    connection: &models::structs::ConnectionConfig,
    timeout_ms: u64,
) -> Result<(), String> {
    let Some((host, port_str)) = reachability_target(connection)? else {
        return Ok(());
    };

    let addr_str = format!("{}:{}", host, port_str);
    let socket_addrs =
        match tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host(addr_str)).await {
            Ok(Ok(addrs)) => addrs.collect::<Vec<_>>(),
            Ok(Err(e)) => {
                return Err(format!(
                    "Cannot resolve host '{}': network is not connected ({})",
                    host, e
                ));
            }
            Err(_) => {
                return Err(format!(
                    "Cannot resolve host '{}': DNS did not respond within {} seconds",
                    host,
                    DNS_TIMEOUT.as_secs()
                ));
            }
        };

    if socket_addrs.is_empty() {
        return Err(format!("Host '{}' is not valid", host));
    }

    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    for addr in socket_addrs {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(Ok(stream)) =
            tokio::time::timeout(remaining, tokio::net::TcpStream::connect(addr)).await
        {
            drop(stream);
            return Ok(());
        }
    }

    Err(unreachable_error(&host, &port_str))
}

/// Resolve the actual host/port to connect to, accounting for SSH tunnels.
///
/// Blocking variant. Async callers must use [`resolve_connection_target_async`]:
/// `ensure_tunnel` spawns an `ssh` process and waits on it.
pub(crate) fn resolve_connection_target(
    connection: &models::structs::ConnectionConfig,
) -> Result<(String, String), String> {
    if connection.ssh_enabled {
        match connection.connection_type {
            models::enums::DatabaseType::SQLite => {
                Err("SSH tunnel is not supported for SQLite connections".to_string())
            }
            _ => {
                let local_port = ssh_tunnel::ensure_tunnel(connection)?;
                Ok(("127.0.0.1".to_string(), local_port.to_string()))
            }
        }
    } else {
        Ok((connection.host.clone(), connection.port.clone()))
    }
}

/// Async counterpart of [`resolve_connection_target`]. Spawning the `ssh` child
/// and waiting for it to settle happens on a blocking thread so it never
/// occupies a runtime worker.
pub(crate) async fn resolve_connection_target_async(
    connection: &models::structs::ConnectionConfig,
) -> Result<(String, String), String> {
    if !connection.ssh_enabled {
        return Ok((connection.host.clone(), connection.port.clone()));
    }
    if connection.connection_type == models::enums::DatabaseType::SQLite {
        return Err("SSH tunnel is not supported for SQLite connections".to_string());
    }

    let conn = connection.clone();
    match tokio::task::spawn_blocking(move || ssh_tunnel::ensure_tunnel(&conn)).await {
        Ok(Ok(local_port)) => Ok(("127.0.0.1".to_string(), local_port.to_string())),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(format!("SSH tunnel task failed: {e}")),
    }
}

// Helper function to clean up completed background pools
pub(crate) fn cleanup_completed_background_pools(tabular: &mut Tabular) {
    let settled: Vec<i64> = {
        let succeeded = lock_or_recover(&tabular.shared_connection_pools)
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let failed = tabular.connection_errors.keys().copied();
        succeeded.into_iter().chain(failed).collect()
    };

    for connection_id in settled {
        // The attempt reported back one way or the other, so its cancellation
        // flag has nothing left to cancel.
        clear_pending_state(tabular, connection_id);
        end_connect_attempt(connection_id);
        tabular.refreshing_connections.remove(&connection_id);
    }
}

/// Drop every trace of a connection's pending status.
fn clear_pending_state(tabular: &mut Tabular, connection_id: i64) {
    tabular.pending_connection_pools.remove(&connection_id);
    tabular.pending_started_at.remove(&connection_id);
    tabular.pending_pool_log_last.remove(&connection_id);
}

// Force cleanup of stuck pending connections (safety net)
pub(crate) fn cleanup_stuck_pending_connections(tabular: &mut Tabular) {
    // Forget timestamps for connections that are no longer pending.
    tabular
        .pending_started_at
        .retain(|id, _| tabular.pending_connection_pools.contains(id));

    if tabular.pending_connection_pools.is_empty() {
        return;
    }

    let now = std::time::Instant::now();
    let stuck_connections: Vec<i64> = tabular.pending_connection_pools.iter().copied().collect();

    for connection_id in stuck_connections {
        let has_pool = tabular.connection_pools.contains_key(&connection_id)
            || lock_or_recover(&tabular.shared_connection_pools).contains_key(&connection_id);

        if has_pool {
            debug!(
                "🧹 Removing stuck pending status for connection {} (pool exists)",
                connection_id
            );
            clear_pending_state(tabular, connection_id);
            continue;
        }

        // Watchdog. A pool creation can die without ever reporting back: a
        // panicking worker thread, an aborted task, or a driver that outlives
        // its own timeout. The id would then sit in `pending_connection_pools`
        // forever, and because `get_or_create_connection_pool` short-circuits on
        // pending ids, the connection would stay dead until the app restarts.
        //
        // The start time is recorded lazily rather than at every insertion site,
        // so an id added through any path — now or in future code — is covered.
        let started = *tabular
            .pending_started_at
            .entry(connection_id)
            .or_insert(now);

        if now.duration_since(started) > PENDING_POOL_MAX_AGE {
            debug!(
                "⏰ Pool creation for connection {} exceeded {}s without reporting — releasing it",
                connection_id,
                PENDING_POOL_MAX_AGE.as_secs()
            );
            clear_pending_state(tabular, connection_id);
            tabular.refreshing_connections.remove(&connection_id);
            // Don't mask a more specific error the background task already reported.
            tabular
                .connection_errors
                .entry(connection_id)
                .or_insert_with(|| {
                    format!(
                        "The connection did not respond within {} seconds and was stopped. Please try connecting again.",
                        PENDING_POOL_MAX_AGE.as_secs()
                    )
                });
        }
    }
}

/// Create a new connection pool for the given connection configuration.
///
/// Bounded by [`CONNECT_TIMEOUT`]. Everything inside yields at `.await` points,
/// so this timeout — and an outer task abort — can genuinely cancel the attempt.
pub(crate) async fn create_connection_pool_for_config(
    connection: &models::structs::ConnectionConfig,
) -> Result<models::enums::DatabasePool, String> {
    let attempt = async {
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            create_connection_pool_for_config_inner(connection),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => {
                let msg = format!(
                    "Connect timed out after {}s for host {}:{}",
                    CONNECT_TIMEOUT.as_secs(),
                    connection.host,
                    connection.port
                );
                debug!("⏰ {}", msg);
                Err(msg)
            }
        }
    };

    // Saved connections can be cancelled; ad-hoc configs without an id (test
    // dialogs, temporary pools) have nothing to key a flag on.
    let Some(flag) = connection.id.and_then(current_cancel_flag) else {
        return attempt.await;
    };

    tokio::select! {
        res = attempt => res,
        _ = wait_for_cancel(flag) => {
            // Dropping `attempt` here tears down the half-open socket instead of
            // leaving it to run to completion in the background.
            debug!("🚫 Connect cancelled for connection {:?}", connection.id);
            Err("Connection attempt cancelled.".to_string())
        }
    }
}

async fn create_connection_pool_for_config_inner(
    connection: &models::structs::ConnectionConfig,
) -> Result<models::enums::DatabasePool, String> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let (target_host, target_port) = match resolve_connection_target_async(connection).await
            {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MySQL connection {:?}: {}",
                        connection.id, err
                    );
                    return Err(format!("Cannot resolve target host: {}", err));
                }
            };
            let connect_opts =
                mysql_connect_options(connection, &target_host, &target_port, None);
            let default_database = connection.database.trim().to_string();

            let mut last_err: Option<sqlx::Error> = None;

            for attempt in 1..=2u8 {
                let start = std::time::Instant::now();
                let (min_conns, test_before, acquire_secs) = match attempt {
                    1 => (0u32, false, 15u64),
                    _ => (1u32, true, 15u64),
                };

                let release_database = default_database.clone();
                let pool_result = MySqlPoolOptions::new()
                    .max_connections(10)
                    .min_connections(min_conns)
                    .acquire_timeout(std::time::Duration::from_secs(acquire_secs))
                    .idle_timeout(std::time::Duration::from_secs(600))
                    .max_lifetime(std::time::Duration::from_secs(1800))
                    .test_before_acquire(test_before)
                    .after_connect(|conn, _| {
                        Box::pin(async move {
                            // `sql_mode` sengaja TIDAK dipaksa: memaksa
                            // 'TRADITIONAL' mengganti mode server (mis.
                            // ONLY_FULL_GROUP_BY, ANSI_QUOTES) sehingga query
                            // berperilaku beda dari klien lain.
                            for statement in MYSQL_SESSION_SETUP {
                                if let Err(e) = sqlx::query(statement).execute(&mut *conn).await {
                                    log::warn!(
                                        "[POOL] MySQL session setup `{}` failed: {}",
                                        statement,
                                        e
                                    );
                                }
                            }
                            Ok(())
                        })
                    })
                    .after_release(move |conn, _| {
                        let default_database = release_database.clone();
                        Box::pin(async move {
                            finish_session_reset(
                                "MySQL",
                                tokio::time::timeout(
                                    SESSION_RESET_TIMEOUT,
                                    reset_mysql_session(conn, &default_database),
                                )
                                .await,
                            )
                        })
                    })
                    .connect_with(connect_opts.clone())
                    .await;

                match pool_result {
                    Ok(pool) => {
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "✅ Created MySQL connection pool (attempt {}, {} ms) for connection {:?}",
                            attempt, elapsed, connection.id
                        );
                        return Ok(models::enums::DatabasePool::MySQL(Arc::new(pool)));
                    }
                    Err(e) => {
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "❌ MySQL pool attempt {} failed after {} ms for connection {:?}: {:?}",
                            attempt, elapsed, connection.id, e
                        );
                        let is_timeout = matches!(e, sqlx::Error::PoolTimedOut)
                            || e.to_string().contains("timeout");
                        last_err = Some(e);
                        if !is_timeout || attempt == 2 {
                            break;
                        }
                    }
                }
            }

            if let Some(e) = last_err {
                debug!(
                    "❌ Failed to create MySQL pool for connection {:?} after retries: {:?}",
                    connection.id, e
                );
                Err(format!("MySQL connection failed: {}", e))
            } else {
                Err("MySQL connection failed: Unknown error".to_string())
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match resolve_connection_target_async(connection).await
            {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for PostgreSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return Err(format!("Cannot resolve target host: {}", err));
                }
            };
            let connect_opts =
                pg_connect_options(connection, &target_host, &target_port, &connection.database);

            let pool_result = PgPoolOptions::new()
                .max_connections(15)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(15))
                .idle_timeout(std::time::Duration::from_secs(600))
                .max_lifetime(std::time::Duration::from_secs(1800))
                .test_before_acquire(false)
                .after_release(|conn, _| {
                    Box::pin(async move {
                        finish_session_reset(
                            "PostgreSQL",
                            tokio::time::timeout(
                                SESSION_RESET_TIMEOUT,
                                reset_postgres_session(conn),
                            )
                            .await,
                        )
                    })
                })
                .connect_with(connect_opts)
                .await;

            match pool_result {
                Ok(pool) => {
                    let database_pool = models::enums::DatabasePool::PostgreSQL(Arc::new(pool));
                    Ok(database_pool)
                }
                Err(e) => {
                    debug!("Failed to create PostgreSQL pool: {}", e);
                    Err(format!("PostgreSQL connection failed: {}", e))
                }
            }
        }
        models::enums::DatabaseType::SQLite => {
            let sqlite_path = if !connection.database.trim().is_empty() {
                connection.database.trim()
            } else if !connection.host.trim().is_empty() && connection.host.trim() != "localhost" {
                connection.host.trim()
            } else {
                connection.database.trim()
            };
            let connection_string = if sqlite_path.starts_with("sqlite:") {
                sqlite_path.to_string()
            } else {
                format!("sqlite:{}", sqlite_path)
            };

            let pool_result = sqlite_pool_options()
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => {
                    let database_pool = models::enums::DatabasePool::SQLite(Arc::new(pool));
                    Ok(database_pool)
                }
                Err(e) => {
                    debug!("Failed to create SQLite pool: {}", e);
                    Err(format!("SQLite connection failed: {}", e))
                }
            }
        }
        models::enums::DatabaseType::Redis => {
            let (target_host, target_port) = match resolve_connection_target_async(connection).await
            {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for Redis connection {:?}: {}",
                        connection.id, err
                    );
                    return Err(format!("Cannot resolve target host: {}", err));
                }
            };
            debug!(
                "Creating new Redis connection manager for: {}",
                connection.name
            );
            // Manager bersama selalu berada di db default (0). Job yang butuh
            // db lain memakai manager terpisah, lihat `redis_manager_for_db`.
            match crate::driver_redis::open_redis_manager(
                &target_host,
                &target_port,
                &connection.username,
                &connection.password,
                None,
            )
            .await
            {
                Ok(manager) => Ok(models::enums::DatabasePool::Redis(Arc::new(manager))),
                Err(e) => {
                    debug!("Failed to create Redis connection manager: {}", e);
                    Err(format!("Redis connection failed: {}", e))
                }
            }
        }
        models::enums::DatabaseType::MongoDB => {
            let (target_host, target_port) = match resolve_connection_target_async(connection).await
            {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MongoDB connection {:?}: {}",
                        connection.id, err
                    );
                    return Err(format!("Cannot resolve target host: {}", err));
                }
            };
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", target_host, target_port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, target_host, target_port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, target_host, target_port
                )
            };
            debug!("Creating MongoDB client for URI: {}", uri);
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                MongoClient::with_uri_str(uri),
            )
            .await
            {
                Ok(Ok(client)) => {
                    let pool = models::enums::DatabasePool::MongoDB(Arc::new(client));
                    Ok(pool)
                }
                Ok(Err(e)) => {
                    debug!("Failed to create MongoDB client: {}", e);
                    Err(format!("MongoDB connection failed: {}", e))
                }
                Err(_) => {
                    debug!("Failed to create MongoDB client (timeout)");
                    Err("MongoDB connection timed out after 10s".to_string())
                }
            }
        }
        models::enums::DatabaseType::MsSQL => {
            let (target_host, target_port) = match resolve_connection_target_async(connection).await
            {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MsSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return Err(format!("Cannot resolve target host: {}", err));
                }
            };

            let client_config = crate::driver_mssql::mssql_config(
                &target_host,
                target_port.parse::<u16>().unwrap_or(1433),
                &connection.username,
                &connection.password,
                Some(&connection.database),
            );

            match tokio::time::timeout(
                DRIVER_TIMEOUT,
                mssql_driver_pool::Pool::builder()
                    .client_config(client_config)
                    .max_connections(20)
                    .build(),
            )
            .await
            {
                Ok(Ok(pool)) => Ok(models::enums::DatabasePool::MsSQL(Arc::new(pool))),
                Ok(Err(e)) => {
                    debug!("MsSQL pool creation failed: {}", e);
                    Err(format!("MsSQL connection failed: {}", e))
                }
                Err(_) => {
                    let msg = format!(
                        "MsSQL pool creation timed out after {}s",
                        DRIVER_TIMEOUT.as_secs()
                    );
                    debug!("{}", msg);
                    Err(msg)
                }
            }
        }
        models::enums::DatabaseType::ApiHttp => {
            // API-HTTP connections do not use a database pool
            Err("API-HTTP connections do not use a database pool".to_string())
        }
        models::enums::DatabaseType::Plugin(ref engine_id) => {
            crate::driver_api::connect::create_plugin_pool(connection, engine_id).await
        }
    }
}

/// SET sesi yang dijalankan pada setiap koneksi MySQL baru.
const MYSQL_SESSION_SETUP: [&str; 4] = [
    "SET SESSION wait_timeout = 600",
    "SET SESSION interactive_timeout = 600",
    "SET SESSION net_read_timeout = 120",
    "SET SESSION net_write_timeout = 120",
];

/// Opsi koneksi MySQL yang dipakai pool utama maupun koneksi sekali pakai,
/// supaya pengaturan SSL selalu sama. `host`/`port` adalah target yang sudah
/// di-resolve (ujung lokal tunnel SSH bila aktif). `database` `None` berarti
/// database default koneksi; string kosong berarti tanpa database.
pub(crate) fn mysql_connect_options(
    connection: &models::structs::ConnectionConfig,
    host: &str,
    port: &str,
    database: Option<&str>,
) -> sqlx::mysql::MySqlConnectOptions {
    let mut connect_opts = sqlx::mysql::MySqlConnectOptions::new()
        .host(host)
        .port(port.trim().parse::<u16>().unwrap_or(3306))
        .username(&connection.username)
        .password(&connection.password);

    let database = database.unwrap_or(&connection.database).trim();
    if !database.is_empty() {
        connect_opts = connect_opts.database(database);
    }

    if connection.ssl_enabled {
        let ssl_mode = if !connection.ssl_verify_server {
            sqlx::mysql::MySqlSslMode::Required
        } else if !connection.ssl_ca_cert.trim().is_empty() {
            sqlx::mysql::MySqlSslMode::VerifyCa
        } else {
            sqlx::mysql::MySqlSslMode::Required
        };
        connect_opts = connect_opts.ssl_mode(ssl_mode);

        if !connection.ssl_ca_cert.trim().is_empty() {
            connect_opts = connect_opts.ssl_ca(connection.ssl_ca_cert.trim());
        }
        if !connection.ssl_client_cert.trim().is_empty() {
            connect_opts = connect_opts.ssl_client_cert(connection.ssl_client_cert.trim());
        }
        if !connection.ssl_client_key.trim().is_empty() {
            connect_opts = connect_opts.ssl_client_key(connection.ssl_client_key.trim());
        }
    } else {
        connect_opts = connect_opts.ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
    }
    connect_opts
}

/// Opsi koneksi PostgreSQL untuk target yang sudah di-resolve. Kredensial dan
/// nama database diisi lewat builder (bukan URL), jadi karakter seperti `@`,
/// `/`, `:` atau `%` di password tidak perlu di-encode.
pub(crate) fn pg_connect_options(
    connection: &models::structs::ConnectionConfig,
    host: &str,
    port: &str,
    database: &str,
) -> sqlx::postgres::PgConnectOptions {
    let mut connect_opts = sqlx::postgres::PgConnectOptions::new()
        .host(host)
        .port(port.trim().parse::<u16>().unwrap_or(5432))
        .username(&connection.username)
        .password(&connection.password);

    if !database.trim().is_empty() {
        connect_opts = connect_opts.database(database.trim());
    }

    if connection.ssl_enabled {
        let ssl_mode = if !connection.ssl_verify_server {
            sqlx::postgres::PgSslMode::Require
        } else if !connection.ssl_ca_cert.trim().is_empty() {
            sqlx::postgres::PgSslMode::VerifyCa
        } else {
            sqlx::postgres::PgSslMode::Require
        };
        connect_opts = connect_opts.ssl_mode(ssl_mode);

        if !connection.ssl_ca_cert.trim().is_empty() {
            connect_opts = connect_opts.ssl_root_cert(connection.ssl_ca_cert.trim());
        }
        if !connection.ssl_client_cert.trim().is_empty() {
            connect_opts = connect_opts.ssl_client_cert(connection.ssl_client_cert.trim());
        }
        if !connection.ssl_client_key.trim().is_empty() {
            connect_opts = connect_opts.ssl_client_key(connection.ssl_client_key.trim());
        }
    } else {
        connect_opts = connect_opts.ssl_mode(sqlx::postgres::PgSslMode::Prefer);
    }
    connect_opts
}

/// Buka pool PostgreSQL sekali pakai (1 koneksi) ke `database` tertentu.
///
/// Dipakai jalur metadata yang butuh database selain database default pool
/// utama. Target di-resolve lewat [`resolve_connection_target_async`], jadi
/// tunnel SSH ikut dipakai, dan opsi SSL sama dengan pool utama. Pemanggil
/// wajib menutup pool (`pool.close().await`) setelah selesai.
pub(crate) async fn connect_postgres_once(
    connection: &models::structs::ConnectionConfig,
    database: &str,
    acquire_timeout: Duration,
) -> Result<sqlx::PgPool, String> {
    let (host, port) = resolve_connection_target_async(connection)
        .await
        .map_err(|e| format!("Cannot resolve target host: {e}"))?;
    let database = if database.trim().is_empty() {
        connection.database.as_str()
    } else {
        database
    };
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(acquire_timeout)
        .connect_with(pg_connect_options(connection, &host, &port, database))
        .await
        .map_err(|e| format!("PostgreSQL connection to database '{database}' failed: {e}"))
}

/// Buka pool MySQL sekali pakai (1 koneksi) ke `database` tertentu. Sama
/// seperti [`connect_postgres_once`]: lewat tunnel SSH bila aktif dan dengan
/// opsi SSL pool utama. Pemanggil sebaiknya menutup pool setelah selesai.
pub(crate) async fn connect_mysql_once(
    connection: &models::structs::ConnectionConfig,
    database: &str,
    acquire_timeout: Duration,
) -> Result<sqlx::MySqlPool, String> {
    let (host, port) = resolve_connection_target_async(connection)
        .await
        .map_err(|e| format!("Cannot resolve target host: {e}"))?;
    let database = if database.trim().is_empty() {
        None
    } else {
        Some(database)
    };
    MySqlPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(acquire_timeout)
        .connect_with(mysql_connect_options(connection, &host, &port, database))
        .await
        .map_err(|e| format!("MySQL connection failed: {e}"))
}

/// Opsi pool SQLite standar aplikasi, termasuk hook yang me-rollback transaksi
/// yang tertinggal saat koneksi dikembalikan ke pool.
pub(crate) fn sqlite_pool_options() -> SqlitePoolOptions {
    SqlitePoolOptions::new()
        .max_connections(5)
        .min_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .idle_timeout(std::time::Duration::from_secs(300))
        .max_lifetime(std::time::Duration::from_secs(1800))
        .test_before_acquire(false)
        .after_release(|conn, _| {
            Box::pin(async move {
                finish_session_reset(
                    "SQLite",
                    tokio::time::timeout(SESSION_RESET_TIMEOUT, reset_sqlite_session(conn)).await,
                )
            })
        })
}

/// Ubah hasil reset sesi menjadi keputusan `after_release`.
///
/// `Ok(true)`: koneksi bersih, boleh kembali ke pool. `Ok(false)`: server
/// menolak reset — koneksi ditutup baik-baik. `Err`: koneksi rusak atau reset
/// melewati batas waktu — sqlx menutupnya paksa tanpa handshake penutup.
fn finish_session_reset(
    engine: &str,
    outcome: Result<Result<bool, sqlx::Error>, tokio::time::error::Elapsed>,
) -> Result<bool, sqlx::Error> {
    match outcome {
        Ok(Ok(true)) => Ok(true),
        Ok(Ok(false)) => {
            log::warn!(
                "[POOL] {} session could not be reset; closing the connection",
                engine
            );
            Ok(false)
        }
        Ok(Err(e @ sqlx::Error::Database(_))) => {
            log::warn!(
                "[POOL] {} session reset failed, closing the connection: {}",
                engine,
                e
            );
            Ok(false)
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("{engine} session reset timed out"),
        ))),
    }
}

/// Reset sesi PostgreSQL sebelum koneksi kembali ke pool.
///
/// Tanpa ini, `BEGIN` tanpa `COMMIT` (atau transaksi yang gagal) ikut ke
/// pemakai koneksi berikutnya: statement mereka masuk ke transaksi orang lain,
/// lock tertahan tanpa batas, dan `SET search_path` / `statement_timeout` dari
/// job lama tetap berlaku.
///
/// `DISCARD ALL` sengaja tidak dipakai karena ikut menghapus prepared statement
/// yang masih dirujuk cache statement sqlx. Sesi manual-commit
/// (`connection/session.rs`) tidak terpengaruh: hook ini baru berjalan saat
/// `PoolConnection`-nya di-drop, yaitu setelah sesi itu selesai.
async fn reset_postgres_session(conn: &mut sqlx::PgConnection) -> Result<bool, sqlx::Error> {
    use sqlx::Row;

    // Satu round-trip untuk kasus umum (tidak ada transaksi terbuka). Di luar
    // transaksi eksplisit, waktu mulai transaksi (`now()`) sama persis dengan
    // waktu mulai statement; di dalam transaksi eksplisit `now()` menunjuk ke
    // `BEGIN` yang lebih lama. `ROLLBACK` tanpa syarat tidak dipakai karena
    // menulis WARNING ke log server pada setiap pelepasan koneksi.
    let probe = sqlx::raw_sql(
        "SELECT now() IS DISTINCT FROM statement_timestamp() AS in_transaction; RESET ALL",
    )
    .fetch_all(&mut *conn)
    .await;

    let needs_rollback = match probe {
        Ok(rows) => rows
            .first()
            .and_then(|row| row.try_get::<bool, _>(0).ok())
            .unwrap_or(true),
        // Transaksi berstatus gagal menolak semua statement (25P02), dan
        // engine kompatibel-PG mungkin tidak punya fungsi di atas: keduanya
        // ditangani dengan rollback eksplisit.
        Err(sqlx::Error::Database(_)) => true,
        Err(e) => return Err(e),
    };

    if needs_rollback {
        // `RESET ALL` di dalam transaksi ikut ter-rollback, jadi diulang.
        sqlx::raw_sql("ROLLBACK; RESET ALL")
            .execute(&mut *conn)
            .await?;
    }
    Ok(true)
}

/// Reset sesi MySQL sebelum koneksi kembali ke pool: rollback transaksi yang
/// tertinggal dan kembalikan database default (job bisa menjalankan `USE`).
/// `ROLLBACK` di luar transaksi adalah no-op tanpa warning di MySQL.
async fn reset_mysql_session(
    conn: &mut sqlx::MySqlConnection,
    default_database: &str,
) -> Result<bool, sqlx::Error> {
    use sqlx::Row;

    // Protokol teks (`raw_sql`): ROLLBACK dan USE tidak didukung protokol
    // prepared statement di semua versi server.
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await?;

    if default_database.is_empty() {
        // Pool tanpa database default: database yang terlanjur dipilih tidak
        // bisa "dilepas", jadi koneksi seperti itu ditutup.
        let row = sqlx::raw_sql("SELECT DATABASE()")
            .fetch_one(&mut *conn)
            .await?;
        let has_database = match row.try_get::<Option<String>, _>(0) {
            Ok(name) => name.is_some(),
            Err(_) => row
                .try_get::<Option<Vec<u8>>, _>(0)
                .map(|name| name.is_some())
                .unwrap_or(true),
        };
        return Ok(!has_database);
    }

    let use_statement = format!("USE `{}`", default_database.replace('`', "``"));
    sqlx::raw_sql(sqlx::AssertSqlSafe(use_statement))
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

/// Reset sesi SQLite: rollback transaksi yang tertinggal. Koneksi yang kembali
/// ke pool dengan transaksi tulis terbuka menahan lock RESERVED/EXCLUSIVE dan
/// membuat koneksi lain gagal dengan "database is locked".
///
/// `query_only` ikut dimatikan: job read-only (jalur baca agent) menyalakannya,
/// dan job yang di-drop di tengah jalan tidak sempat mematikannya sendiri.
async fn reset_sqlite_session(conn: &mut sqlx::SqliteConnection) -> Result<bool, sqlx::Error> {
    match sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await {
        Ok(_) => {}
        // Mode autocommit: tidak ada yang perlu di-rollback.
        Err(sqlx::Error::Database(e)) if e.message().contains("no transaction is active") => {}
        Err(e) => return Err(e),
    }
    sqlx::raw_sql("PRAGMA query_only=OFF")
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

/// Manager Redis khusus per (koneksi, indeks db).
///
/// `ConnectionManager` adalah satu soket yang di-multiplex: semua clone-nya
/// berbagi koneksi yang sama. `SELECT n` pada manager bersama mengganti db
/// untuk semua tab dan worker sidebar sekaligus, dan reconnect otomatis
/// mengembalikannya ke db 0. Karena itu pemakaian db tertentu memakai manager
/// sendiri yang indeks db-nya ada di info koneksi (ikut dipakai saat reconnect).
static REDIS_DB_MANAGERS: Lazy<Mutex<HashMap<(i64, i64), ConnectionManager>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Ambil (atau buat) manager Redis yang terikat ke `db` untuk koneksi ini.
/// Koneksi tanpa id (ad-hoc) tidak di-cache.
///
/// Harus dipanggil dari runtime berumur panjang: task latar manager hidup di
/// runtime tempat manager dibuat.
pub(crate) async fn redis_manager_for_db(
    connection: &models::structs::ConnectionConfig,
    db: i64,
) -> Result<ConnectionManager, String> {
    if let Some(id) = connection.id
        && let Some(manager) = lock_or_recover(&REDIS_DB_MANAGERS).get(&(id, db))
    {
        return Ok(manager.clone());
    }

    let (host, port) = resolve_connection_target_async(connection).await?;
    let manager = crate::driver_redis::open_redis_manager(
        &host,
        &port,
        &connection.username,
        &connection.password,
        Some(db),
    )
    .await?;

    if let Some(id) = connection.id {
        lock_or_recover(&REDIS_DB_MANAGERS).insert((id, db), manager.clone());
    }
    Ok(manager)
}

/// Buang semua manager Redis per-db milik sebuah koneksi.
fn evict_redis_db_managers(connection_id: i64) {
    lock_or_recover(&REDIS_DB_MANAGERS).retain(|(id, _), _| *id != connection_id);
}

/// True jika pool sudah ditutup dan tidak bisa membuka koneksi baru.
fn pool_is_closed(pool: &models::enums::DatabasePool) -> bool {
    match pool {
        models::enums::DatabasePool::MySQL(p) => p.is_closed(),
        models::enums::DatabasePool::PostgreSQL(p) => p.is_closed(),
        models::enums::DatabasePool::SQLite(p) => p.is_closed(),
        models::enums::DatabasePool::MsSQL(p) => p.is_closed(),
        _ => false,
    }
}

/// Buang pool yang sudah tidak bisa dipakai supaya pemakaian berikutnya
/// membangun ulang tunnel + pool, alih-alih terus gagal sampai aplikasi
/// di-restart. Pool dianggap tidak bisa dipakai bila:
///
/// - pool-nya sudah ditutup, atau
/// - koneksinya lewat tunnel SSH dan proses `ssh`-nya sudah mati/hilang: pool
///   masih menunjuk ke port lokal lama yang tidak lagi didengarkan siapa pun.
///
/// Mengembalikan `true` bila ada pool yang dibuang. Tidak pernah memblokir,
/// jadi aman dipanggil dari thread UI.
pub(crate) fn evict_unusable_pool(tabular: &mut Tabular, connection_id: i64) -> bool {
    // Selama connect masih berjalan belum ada pool untuk dinilai, dan tunnel
    // barunya tidak boleh ikut dimatikan.
    if tabular.pending_connection_pools.contains(&connection_id) {
        return false;
    }
    let pool = tabular
        .connection_pools
        .get(&connection_id)
        .cloned()
        .or_else(|| {
            lock_or_recover(&tabular.shared_connection_pools)
                .get(&connection_id)
                .cloned()
        });
    let Some(pool) = pool else {
        return false;
    };

    let reason = if pool_is_closed(&pool) {
        Some("the pool is closed")
    } else if pool_uses_ssh_tunnel(tabular, connection_id)
        && matches!(
            ssh_tunnel::tunnel_state_by_id(connection_id),
            ssh_tunnel::TunnelState::Dead | ssh_tunnel::TunnelState::Missing
        )
    {
        Some("its SSH tunnel is no longer running")
    } else {
        None
    };
    let Some(reason) = reason else {
        return false;
    };

    log::warn!(
        "[POOL] Dropping the pool of connection {} because {}; it will be rebuilt on next use",
        connection_id,
        reason
    );
    tabular.connection_pools.remove(&connection_id);
    lock_or_recover(&tabular.shared_connection_pools).remove(&connection_id);
    evict_redis_db_managers(connection_id);
    ssh_tunnel::shutdown_by_id(connection_id);
    true
}

/// True jika pool koneksi ini dibangun di atas tunnel SSH milik aplikasi
/// (engine builtin yang lewat `resolve_connection_target*`).
fn pool_uses_ssh_tunnel(tabular: &Tabular, connection_id: i64) -> bool {
    tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .is_some_and(|c| {
            c.ssh_enabled
                && matches!(
                    c.connection_type,
                    models::enums::DatabaseType::MySQL
                        | models::enums::DatabaseType::PostgreSQL
                        | models::enums::DatabaseType::Redis
                        | models::enums::DatabaseType::MongoDB
                        | models::enums::DatabaseType::MsSQL
                )
        })
}

/// Create a database pool (legacy / refresh path). Delegates to create_connection_pool_for_config.
#[allow(dead_code)]
pub(crate) async fn create_database_pool(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    create_connection_pool_for_config(connection).await.ok()
}

/// Try to create pool quickly (with short timeout); returns None if it times out.
async fn try_quick_pool_creation(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    let connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        create_connection_pool_for_config(&connection),
    )
    .await;

    match result {
        Ok(res) => res.ok(),
        Err(_) => {
            debug!(
                "⚡ Quick creation timed out for connection {}, will try in background",
                connection_id
            );
            None
        }
    }
}

pub(crate) async fn load_connection_by_id(
    connection_id: i64,
    cache_pool: &sqlx::SqlitePool,
) -> Option<models::structs::ConnectionConfig> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT id, name, host, port, username, password, database_name, connection_type, folder, \
                COALESCE(ssh_enabled, 0) AS ssh_enabled, \
                COALESCE(ssh_host, '') AS ssh_host, \
                COALESCE(ssh_port, '22') AS ssh_port, \
                COALESCE(ssh_username, '') AS ssh_username, \
                COALESCE(ssh_auth_method, 'key') AS ssh_auth_method, \
                COALESCE(ssh_private_key, '') AS ssh_private_key, \
                COALESCE(ssh_password, '') AS ssh_password, \
                COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys, \
                COALESCE(ssh_jump_host, '') AS ssh_jump_host, \
                COALESCE(ssl_enabled, 0) AS ssl_enabled, \
                COALESCE(ssl_ca_cert, '') AS ssl_ca_cert, \
                COALESCE(ssl_client_cert, '') AS ssl_client_cert, \
                COALESCE(ssl_client_key, '') AS ssl_client_key, \
                COALESCE(ssl_key_passphrase, '') AS ssl_key_passphrase, \
                COALESCE(ssl_verify_server, 1) AS ssl_verify_server \
         FROM connections WHERE id = ?",
    )
    .bind(connection_id)
    .fetch_optional(cache_pool)
    .await
    .ok()??;

    let id: Option<i64> = row.try_get("id").ok();
    let name: String = row.try_get("name").unwrap_or_default();
    let host: String = row.try_get("host").unwrap_or_default();
    let port: String = row.try_get("port").unwrap_or_default();
    let username: String = row.try_get("username").unwrap_or_default();
    let password: String = row.try_get("password").unwrap_or_default();
    let database: String = row.try_get("database_name").unwrap_or_default();
    let conn_type_str: String = row.try_get("connection_type").unwrap_or_default();
    let folder: Option<String> = row.try_get("folder").ok();
    let ssh_enabled: i64 = row.try_get("ssh_enabled").unwrap_or(0);
    let ssh_host: String = row.try_get("ssh_host").unwrap_or_default();
    let ssh_port: String = row.try_get("ssh_port").unwrap_or_else(|_| "22".to_string());
    let ssh_username: String = row.try_get("ssh_username").unwrap_or_default();
    let ssh_auth_method: String = row
        .try_get("ssh_auth_method")
        .unwrap_or_else(|_| "key".to_string());
    let ssh_private_key: String = row.try_get("ssh_private_key").unwrap_or_default();
    let ssh_password: String = row.try_get("ssh_password").unwrap_or_default();
    let ssh_accept_unknown_host_keys: i64 =
        row.try_get("ssh_accept_unknown_host_keys").unwrap_or(0);
    let ssh_jump_host: String = row.try_get("ssh_jump_host").unwrap_or_default();
    let ssl_enabled: i64 = row.try_get("ssl_enabled").unwrap_or(0);
    let ssl_ca_cert: String = row.try_get("ssl_ca_cert").unwrap_or_default();
    let ssl_client_cert: String = row.try_get("ssl_client_cert").unwrap_or_default();
    let ssl_client_key: String = row.try_get("ssl_client_key").unwrap_or_default();
    let ssl_key_passphrase: String = row.try_get("ssl_key_passphrase").unwrap_or_default();
    let ssl_verify_server: i64 = row.try_get("ssl_verify_server").unwrap_or(1);

    let password = if let Some(cid) = id {
        crate::secrets::resolve_readonly(
            &crate::secrets::connection_secret_name(cid, "password"),
            &password,
        )
    } else {
        password
    };
    let ssh_private_key = if let Some(cid) = id {
        crate::secrets::resolve_readonly(
            &crate::secrets::connection_secret_name(cid, "ssh_private_key"),
            &ssh_private_key,
        )
    } else {
        ssh_private_key
    };
    let ssh_password = if let Some(cid) = id {
        crate::secrets::resolve_readonly(
            &crate::secrets::connection_secret_name(cid, "ssh_password"),
            &ssh_password,
        )
    } else {
        ssh_password
    };
    let ssl_key_passphrase = if let Some(cid) = id {
        crate::secrets::resolve_readonly(
            &crate::secrets::connection_secret_name(cid, "ssl_key_passphrase"),
            &ssl_key_passphrase,
        )
    } else {
        ssl_key_passphrase
    };

    let mut connection = models::structs::ConnectionConfig {
        id,
        plugin_options: Default::default(),
        name,
        host,
        port,
        username,
        password,
        database,
        connection_type: match models::enums::DatabaseType::from_db_str(&conn_type_str) {
            Some(ty) => ty,
            None => {
                log::warn!(
                    "[CONNECTIONS] Connection {} has unknown type '{}'",
                    connection_id,
                    conn_type_str
                );
                return None;
            }
        },
        folder,
        ssh_enabled: ssh_enabled != 0,
        ssh_host,
        ssh_port,
        ssh_username,
        ssh_auth_method: match ssh_auth_method.as_str() {
            "password" => models::enums::SshAuthMethod::Password,
            _ => models::enums::SshAuthMethod::Key,
        },
        ssh_private_key,
        ssh_password,
        ssh_accept_unknown_host_keys: ssh_accept_unknown_host_keys != 0,
        ssh_jump_host,
        ssl_enabled: ssl_enabled != 0,
        ssl_ca_cert,
        ssl_client_cert,
        ssl_client_key,
        ssl_key_passphrase,
        ssl_verify_server: ssl_verify_server != 0,
        custom_views: Vec::new(),
        replication_master_id: None,
    };
    if let Some(cid) = id
        && connection.connection_type.plugin_id().is_some()
    {
        connection.plugin_options =
            crate::driver_api::connect::load_plugin_options(cache_pool, cid).await;
    }
    Some(connection)
}

pub(crate) async fn create_connection_pool_by_id(
    connection_id: i64,
    cache_pool: &sqlx::SqlitePool,
) -> Result<models::enums::DatabasePool, String> {
    use sqlx::Row;
    let row_opt = sqlx::query(
        "SELECT id, name, host, port, username, password, database_name, connection_type, folder, \
                COALESCE(ssh_enabled, 0) AS ssh_enabled, \
                COALESCE(ssh_host, '') AS ssh_host, \
                COALESCE(ssh_port, '22') AS ssh_port, \
                COALESCE(ssh_username, '') AS ssh_username, \
                COALESCE(ssh_auth_method, 'key') AS ssh_auth_method, \
                COALESCE(ssh_private_key, '') AS ssh_private_key, \
                COALESCE(ssh_password, '') AS ssh_password, \
                COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys, \
                COALESCE(ssh_jump_host, '') AS ssh_jump_host, \
                COALESCE(ssl_enabled, 0) AS ssl_enabled, \
                COALESCE(ssl_ca_cert, '') AS ssl_ca_cert, \
                COALESCE(ssl_client_cert, '') AS ssl_client_cert, \
                COALESCE(ssl_client_key, '') AS ssl_client_key, \
                COALESCE(ssl_key_passphrase, '') AS ssl_key_passphrase, \
                COALESCE(ssl_verify_server, 1) AS ssl_verify_server \
         FROM connections WHERE id = ?",
    )
    .bind(connection_id)
    .fetch_optional(cache_pool)
    .await
    .map_err(|e| format!("Failed to read connection from SQLite: {}", e))?;

    let row = match row_opt {
        Some(r) => r,
        None => {
            return Err(format!(
                "Connection ID {} not found in local store",
                connection_id
            ));
        }
    };

    let id = row.try_get::<i64, _>("id").unwrap_or(connection_id);
    let name = row.try_get::<String, _>("name").unwrap_or_default();
    let host = row.try_get::<String, _>("host").unwrap_or_default();
    let port = row
        .try_get::<String, _>("port")
        .unwrap_or_else(|_| "3306".to_string());
    let username = row.try_get::<String, _>("username").unwrap_or_default();
    let password = row.try_get::<String, _>("password").unwrap_or_default();
    let database_name = row
        .try_get::<String, _>("database_name")
        .unwrap_or_default();
    let connection_type = row
        .try_get::<String, _>("connection_type")
        .unwrap_or_else(|_| "SQLite".to_string());
    let folder = row.try_get::<Option<String>, _>("folder").unwrap_or(None);
    let ssh_enabled = row.try_get::<i64, _>("ssh_enabled").unwrap_or(0);
    let ssh_host = row.try_get::<String, _>("ssh_host").unwrap_or_default();
    let ssh_port = row
        .try_get::<String, _>("ssh_port")
        .unwrap_or_else(|_| "22".to_string());
    let ssh_username = row.try_get::<String, _>("ssh_username").unwrap_or_default();
    let ssh_auth_method = row
        .try_get::<String, _>("ssh_auth_method")
        .unwrap_or_else(|_| "key".to_string());
    let ssh_private_key = row
        .try_get::<String, _>("ssh_private_key")
        .unwrap_or_default();
    let ssh_password = row.try_get::<String, _>("ssh_password").unwrap_or_default();
    let ssh_accept_unknown_host_keys = row
        .try_get::<i64, _>("ssh_accept_unknown_host_keys")
        .unwrap_or(0);
    let ssh_jump_host = row
        .try_get::<String, _>("ssh_jump_host")
        .unwrap_or_default();
    let ssl_enabled = row.try_get::<i64, _>("ssl_enabled").unwrap_or(0);
    let ssl_ca_cert = row.try_get::<String, _>("ssl_ca_cert").unwrap_or_default();
    let ssl_client_cert = row
        .try_get::<String, _>("ssl_client_cert")
        .unwrap_or_default();
    let ssl_client_key = row
        .try_get::<String, _>("ssl_client_key")
        .unwrap_or_default();
    let ssl_key_passphrase = row
        .try_get::<String, _>("ssl_key_passphrase")
        .unwrap_or_default();
    let ssl_verify_server = row.try_get::<i64, _>("ssl_verify_server").unwrap_or(1);

    let password = crate::secrets::resolve_readonly(
        &crate::secrets::connection_secret_name(id, "password"),
        &password,
    );
    let ssh_private_key = crate::secrets::resolve_readonly(
        &crate::secrets::connection_secret_name(id, "ssh_private_key"),
        &ssh_private_key,
    );
    let ssh_password = crate::secrets::resolve_readonly(
        &crate::secrets::connection_secret_name(id, "ssh_password"),
        &ssh_password,
    );

    let mut connection = models::structs::ConnectionConfig {
        id: Some(id),
        plugin_options: Default::default(),
        name,
        host,
        port,
        username,
        password,
        database: database_name,
        connection_type: match models::enums::DatabaseType::from_db_str(&connection_type) {
            Some(ty) => ty,
            None => {
                return Err(format!(
                    "Unknown connection type '{}' for connection {}",
                    connection_type, id
                ));
            }
        },
        folder,
        ssh_enabled: ssh_enabled != 0,
        ssh_host,
        ssh_port,
        ssh_username,
        ssh_auth_method: models::enums::SshAuthMethod::from_db_value(&ssh_auth_method),
        ssh_private_key,
        ssh_password,
        ssh_accept_unknown_host_keys: ssh_accept_unknown_host_keys != 0,
        ssh_jump_host,
        ssl_enabled: ssl_enabled != 0,
        ssl_ca_cert,
        ssl_client_cert,
        ssl_client_key,
        ssl_key_passphrase,
        ssl_verify_server: ssl_verify_server != 0,
        custom_views: Vec::new(),
        replication_master_id: None,
    };
    if connection.connection_type.plugin_id().is_some() {
        connection.plugin_options =
            crate::driver_api::connect::load_plugin_options(cache_pool, id).await;
    }

    match create_connection_pool_for_config(&connection).await {
        // Disconnect/cancel bisa datang tepat setelah connect selesai. Pool
        // yang dikembalikan di sini akan dimasukkan pemanggil ke
        // `shared_connection_pools`, sehingga koneksi yang baru saja diputus
        // user "hidup lagi". Buang pool-nya dan laporkan sebagai batal.
        Ok(pool) if connect_was_cancelled(connection_id) => {
            debug!(
                "🚫 Discarding pool for connection {}: the attempt was cancelled",
                connection_id
            );
            drop(pool);
            end_connect_attempt(connection_id);
            ssh_tunnel::shutdown_by_id(connection_id);
            Err("Connection attempt cancelled.".to_string())
        }
        Ok(pool) => Ok(pool),
        Err(err) => {
            if connect_was_cancelled(connection_id) {
                end_connect_attempt(connection_id);
                Err("Connection attempt cancelled.".to_string())
            } else {
                Err(err)
            }
        }
    }
}

/// Start background pool creation without blocking the UI thread.
pub(crate) fn start_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
    tabular.pending_connection_pools.insert(connection_id);
    tabular
        .pending_started_at
        .insert(connection_id, std::time::Instant::now());
    // Arm cancellation before dispatch, so a cancel arriving while the task is
    // still queued is still seen by it.
    let cancel_flag = begin_connect_attempt(connection_id);

    if let Some(sender) = &tabular.background_sender {
        let _ = sender.send(models::enums::BackgroundTask::EnsureConnectionPool { connection_id });
        return;
    }

    let connection = match tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
    {
        Some(conn) => conn.clone(),
        None => {
            debug!(
                "❌ Connection {} not found for background creation",
                connection_id
            );
            tabular.pending_connection_pools.remove(&connection_id);
            return;
        }
    };

    if let Some(runtime) = &tabular.runtime {
        let rt = runtime.clone();
        let shared_pools = tabular.shared_connection_pools.clone();

        rt.spawn(async move {
            debug!(
                "🔄 Background: Creating pool for connection {}",
                connection_id
            );

            match create_connection_pool_for_config(&connection).await {
                Ok(pool) => {
                    // Flag milik attempt INI (bukan flag terbaru di registry):
                    // disconnect saat connect berjalan tidak boleh berakhir
                    // dengan pool yang dimasukkan kembali.
                    if cancel_flag.load(Ordering::SeqCst) {
                        debug!(
                            "🚫 Background: discarding pool for connection {} (attempt cancelled)",
                            connection_id
                        );
                        drop(pool);
                        ssh_tunnel::shutdown_by_id(connection_id);
                        return;
                    }
                    debug!(
                        "✅ Background: Successfully created pool for connection {}",
                        connection_id
                    );
                    lock_or_recover(&shared_pools).insert(connection_id, pool);
                }
                Err(err) => {
                    debug!(
                        "❌ Background: Failed to create pool for connection {}: {}",
                        connection_id, err
                    );
                }
            }
        });
    }
}

/// Ensure a background pool creation is in progress. No-op if pool already exists or pending.
pub(crate) fn ensure_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
    let has_pool = tabular.connection_pools.contains_key(&connection_id)
        || lock_or_recover(&tabular.shared_connection_pools).contains_key(&connection_id);
    if has_pool {
        return;
    }
    if tabular.pending_connection_pools.contains(&connection_id) {
        return;
    }
    tabular.pending_connection_pools.insert(connection_id);
    start_background_pool_creation(tabular, connection_id);
}

/// Get or create a connection pool, using cache, background tasks, or quick creation.
pub(crate) async fn get_or_create_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    cleanup_completed_background_pools(tabular);
    cleanup_stuck_pending_connections(tabular);
    evict_unusable_pool(tabular, connection_id);

    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    let shared_pool = lock_or_recover(&tabular.shared_connection_pools)
        .get(&connection_id)
        .cloned();
    if let Some(pool) = shared_pool {
        debug!(
            "✅ Using background-created connection pool for connection {}",
            connection_id
        );
        tabular.connection_pools.insert(connection_id, pool.clone());
        tabular.pending_connection_pools.remove(&connection_id);
        return Some(pool);
    }

    if tabular.pending_connection_pools.contains(&connection_id) {
        let now = std::time::Instant::now();
        let should_log = match tabular.pending_pool_log_last.get(&connection_id) {
            Some(last) => now.duration_since(*last) > std::time::Duration::from_secs(1),
            None => true,
        };
        if should_log {
            debug!(
                "⏳ Connection pool creation already in progress for connection {}",
                connection_id
            );
            tabular.pending_pool_log_last.insert(connection_id, now);
        }
        return None;
    }

    debug!(
        "🔄 Creating new connection pool for connection {}",
        connection_id
    );

    tabular.pending_connection_pools.insert(connection_id);
    tabular
        .pending_started_at
        .insert(connection_id, std::time::Instant::now());
    begin_connect_attempt(connection_id);

    match try_quick_pool_creation(tabular, connection_id).await {
        Some(pool) => {
            tabular.connection_pools.insert(connection_id, pool.clone());
            clear_pending_state(tabular, connection_id);
            end_connect_attempt(connection_id);
            debug!(
                "✅ Quickly created connection pool for connection {}",
                connection_id
            );
            Some(pool)
        }
        // A cancel that landed during the quick attempt must not be undone by
        // immediately queueing the same connect in the background.
        None if connect_was_cancelled(connection_id) => {
            debug!(
                "🚫 Quick attempt for connection {} was cancelled; not escalating to background",
                connection_id
            );
            clear_pending_state(tabular, connection_id);
            None
        }
        None => {
            start_background_pool_creation(tabular, connection_id);
            None
        }
    }
}

/// Pool lookup for callers running on the UI thread.
///
/// Returns a pool only if one is already established. It never performs a
/// connect itself — unlike [`get_or_create_connection_pool`], which can spend up
/// to the quick-attempt budget dialling the server — so it is safe to call while
/// painting a frame. When no pool is ready it starts background creation and
/// returns `None`; the caller should render a placeholder and pick the data up
/// on a later frame.
///
/// Declared `async` purely so it drops into the existing `block_on` call sites
/// unchanged; it never awaits.
pub(crate) async fn pool_if_connected_or_start(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    cleanup_completed_background_pools(tabular);
    cleanup_stuck_pending_connections(tabular);
    evict_unusable_pool(tabular, connection_id);

    if let Some(pool) = tabular.connection_pools.get(&connection_id) {
        return Some(pool.clone());
    }

    let shared = lock_or_recover(&tabular.shared_connection_pools)
        .get(&connection_id)
        .cloned();

    if let Some(pool) = shared {
        debug!(
            "✅ Promoting background-created pool for connection {}",
            connection_id
        );
        tabular.connection_pools.insert(connection_id, pool.clone());
        clear_pending_state(tabular, connection_id);
        end_connect_attempt(connection_id);
        return Some(pool);
    }

    // Don't re-dial a connection that already failed. Callers here are render
    // paths, so without this a dead server would be retried on every frame. The
    // error is cleared by an explicit Reconnect, which is what re-arms this.
    if tabular.connection_errors.contains_key(&connection_id) {
        return None;
    }

    ensure_background_pool_creation(tabular, connection_id);
    None
}

/// Retry-based pool retrieval. Waits between retries if pool is being created.
#[allow(dead_code)]
pub(crate) async fn get_or_create_connection_pool_with_retry(
    tabular: &mut Tabular,
    connection_id: i64,
    max_retries: u32,
) -> Option<models::enums::DatabasePool> {
    for attempt in 0..=max_retries {
        if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
            debug!(
                "✅ Using cached connection pool for connection {}",
                connection_id
            );
            return Some(cached_pool.clone());
        }

        if !tabular.pending_connection_pools.contains(&connection_id) {
            return get_or_create_connection_pool(tabular, connection_id).await;
        }

        if attempt < max_retries {
            debug!(
                "⏳ Waiting for connection pool creation (attempt {}/{})",
                attempt + 1,
                max_retries + 1
            );
            tokio::time::sleep(std::time::Duration::from_millis(500 + attempt as u64 * 200)).await;
        } else {
            debug!(
                "⏰ Max retries reached for connection pool {}",
                connection_id
            );
            break;
        }
    }

    None
}

/// Remove and clean up a connection pool (local cache, shared cache, SSH tunnels).
pub(crate) fn cleanup_connection_pool(tabular: &mut Tabular, connection_id: i64) {
    debug!(
        "🧹 Cleaning up connection pool for connection {}",
        connection_id
    );
    // Batalkan dulu connect yang masih berjalan. Tanpa sinyal ini attempt
    // tersebut tetap selesai dan memasukkan pool-nya ke
    // `shared_connection_pools`, sehingga koneksi yang baru diputus hidup lagi.
    let attempt_outstanding = tabular.pending_connection_pools.contains(&connection_id);
    signal_connect_cancel(connection_id);

    tabular.connection_pools.remove(&connection_id);
    clear_pending_state(tabular, connection_id);
    if !attempt_outstanding {
        // Tidak ada attempt yang perlu melihat flag batal. Bila ADA, flag
        // dibiarkan: attempt yang masih antre di worker harus tetap bisa
        // membacanya (lihat `create_connection_pool_by_id`), dan flag itu
        // kedaluwarsa sendiri (lihat `CancelEntry`).
        end_connect_attempt(connection_id);
    }

    lock_or_recover(&tabular.shared_connection_pools).remove(&connection_id);
    evict_redis_db_managers(connection_id);

    ssh_tunnel::shutdown_by_id(connection_id);
}

/// Cancel an in-flight connect attempt for `connection_id`.
///
/// Returns `true` if there was something to cancel. The UI state is released
/// immediately; the background task itself unwinds within
/// [`CANCEL_POLL_INTERVAL`], when the cancel watcher wins its race and the
/// half-open connect future is dropped.
pub(crate) fn cancel_connection_attempt(tabular: &mut Tabular, connection_id: i64) -> bool {
    let was_pending = tabular.pending_connection_pools.contains(&connection_id);
    if !was_pending {
        return false;
    }

    debug!(
        "🚫 Cancelling connect attempt for connection {}",
        connection_id
    );

    signal_connect_cancel(connection_id);
    clear_pending_state(tabular, connection_id);
    tabular.refreshing_connections.remove(&connection_id);
    tabular.connection_errors.insert(
        connection_id,
        "Connection attempt cancelled by the user.".to_string(),
    );

    // Tear down a tunnel the attempt may already have opened. Non-blocking, so
    // this is safe to call from the UI thread.
    ssh_tunnel::shutdown_by_id(connection_id);

    true
}

/// Cancel every in-flight connect attempt, e.g. on shutdown.
pub(crate) fn cancel_all_connection_attempts(tabular: &mut Tabular) {
    let pending: Vec<i64> = tabular.pending_connection_pools.iter().copied().collect();
    for connection_id in pending {
        cancel_connection_attempt(tabular, connection_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::enums::DatabaseType;
    use models::structs::ConnectionConfig;

    fn conn(connection_type: DatabaseType) -> ConnectionConfig {
        ConnectionConfig {
            connection_type,
            ..Default::default()
        }
    }

    #[test]
    fn sqlite_needs_no_reachability_probe() {
        let mut c = conn(DatabaseType::SQLite);
        c.host = "/tmp/some.db".to_string();
        assert_eq!(reachability_target(&c).unwrap(), None);
    }

    #[test]
    fn loopback_hosts_need_no_reachability_probe() {
        for host in ["localhost", "127.0.0.1", "::1"] {
            let mut c = conn(DatabaseType::MySQL);
            c.host = host.to_string();
            assert_eq!(reachability_target(&c).unwrap(), None, "host {host}");
        }
    }

    #[test]
    fn direct_connection_probes_database_endpoint() {
        let mut c = conn(DatabaseType::PostgreSQL);
        c.host = "db.example.com".to_string();
        c.port = "5432".to_string();
        assert_eq!(
            reachability_target(&c).unwrap(),
            Some(("db.example.com".to_string(), "5432".to_string()))
        );
    }

    #[test]
    fn ssh_connection_probes_the_ssh_endpoint_not_the_database() {
        let mut c = conn(DatabaseType::MySQL);
        c.host = "db.internal".to_string();
        c.port = "3306".to_string();
        c.ssh_enabled = true;
        c.ssh_host = "bastion.example.com".to_string();
        c.ssh_port = "2222".to_string();
        assert_eq!(
            reachability_target(&c).unwrap(),
            Some(("bastion.example.com".to_string(), "2222".to_string()))
        );
    }

    #[test]
    fn blank_ports_fall_back_to_defaults() {
        let mut direct = conn(DatabaseType::MySQL);
        direct.host = "db.example.com".to_string();
        direct.port = "  ".to_string();
        assert_eq!(
            reachability_target(&direct).unwrap(),
            Some(("db.example.com".to_string(), "3306".to_string()))
        );

        let mut tunnelled = conn(DatabaseType::MySQL);
        tunnelled.host = "db.internal".to_string();
        tunnelled.ssh_enabled = true;
        tunnelled.ssh_host = "bastion.example.com".to_string();
        tunnelled.ssh_port = String::new();
        assert_eq!(
            reachability_target(&tunnelled).unwrap(),
            Some(("bastion.example.com".to_string(), "22".to_string()))
        );
    }

    #[test]
    fn empty_hosts_are_rejected() {
        let mut direct = conn(DatabaseType::MySQL);
        direct.host = String::new();
        assert!(reachability_target(&direct).is_err());

        let mut tunnelled = conn(DatabaseType::MySQL);
        tunnelled.host = "db.internal".to_string();
        tunnelled.ssh_enabled = true;
        tunnelled.ssh_host = "   ".to_string();
        assert!(reachability_target(&tunnelled).is_err());
    }

    #[test]
    fn dns_resolution_gives_up_once_the_budget_expires() {
        // RFC 6761 reserves .invalid, so this never resolves. The point is that
        // the call returns rather than hanging the way `to_socket_addrs` could.
        let started = std::time::Instant::now();
        let result = resolve_addrs_blocking("nonexistent.invalid:3306", Duration::from_millis(300));
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "resolution should be bounded, took {:?}",
            started.elapsed()
        );
    }

    // Ids are namespaced per test: CANCEL_FLAGS is process-global and tests
    // share a process.
    #[test]
    fn cancel_flag_starts_clear_and_is_raised_by_signal() {
        let id = -9001;
        begin_connect_attempt(id);
        assert!(!connect_was_cancelled(id));

        signal_connect_cancel(id);
        assert!(connect_was_cancelled(id));

        end_connect_attempt(id);
    }

    #[test]
    fn a_new_attempt_clears_a_previous_cancel() {
        // Otherwise a connection cancelled once could never be reconnected.
        let id = -9002;
        begin_connect_attempt(id);
        signal_connect_cancel(id);
        assert!(connect_was_cancelled(id));

        begin_connect_attempt(id);
        assert!(!connect_was_cancelled(id));

        end_connect_attempt(id);
    }

    #[test]
    fn unknown_and_finished_connections_are_not_cancelled() {
        let id = -9003;
        assert!(!connect_was_cancelled(id));

        begin_connect_attempt(id);
        signal_connect_cancel(id);
        end_connect_attempt(id);
        // A stale flag must not make the next attempt look pre-cancelled.
        assert!(!connect_was_cancelled(id));
    }

    #[test]
    fn signalling_one_connection_does_not_cancel_another() {
        let (a, b) = (-9004, -9005);
        begin_connect_attempt(a);
        begin_connect_attempt(b);

        signal_connect_cancel(a);
        assert!(connect_was_cancelled(a));
        assert!(!connect_was_cancelled(b));

        end_connect_attempt(a);
        end_connect_attempt(b);
    }

    #[tokio::test]
    async fn cancel_watcher_resolves_once_the_flag_is_raised() {
        let flag = Arc::new(AtomicBool::new(false));
        let watcher = flag.clone();

        let started = std::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            watcher.store(true, Ordering::SeqCst);
        });

        // Would hang forever if the watcher ignored the flag.
        tokio::time::timeout(Duration::from_secs(5), wait_for_cancel(flag))
            .await
            .expect("cancel watcher should resolve after the flag is raised");

        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn cancelled_flag_survives_until_the_attempt_reads_it() {
        // Disconnect saat connect masih antre: flag harus tetap terbaca oleh
        // attempt yang baru mulai belakangan, tetapi tidak selamanya.
        let id = -9006;
        begin_connect_attempt(id);
        signal_connect_cancel(id);
        assert!(connect_was_cancelled(id));

        // Pura-pura flag dibatalkan jauh di masa lalu.
        if let Some(entry) = lock_or_recover(&CANCEL_FLAGS).get_mut(&id) {
            entry.cancelled_at = std::time::Instant::now()
                .checked_sub(PENDING_POOL_MAX_AGE + Duration::from_secs(1));
            assert!(entry.cancelled_at.is_some(), "clock too close to boot");
        }
        // Flag basi dibuang, jadi connect baru (agent, transfer data) yang
        // tidak lewat `begin_connect_attempt` tidak langsung batal.
        assert!(!connect_was_cancelled(id));
        assert!(current_cancel_flag(id).is_none());
    }

    #[test]
    fn poisoned_registry_lock_still_yields_its_contents() {
        let registry = Arc::new(Mutex::new(HashMap::from([(1_i64, "pool")])));
        let poisoner = registry.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().expect("first lock");
            panic!("poison the registry");
        })
        .join();
        assert!(registry.is_poisoned());
        // Tidak boleh terbaca "kosong" hanya karena thread lain panic.
        assert_eq!(lock_or_recover(&registry).get(&1), Some(&"pool"));
    }

    fn secured(connection_type: DatabaseType) -> ConnectionConfig {
        let mut c = conn(connection_type);
        c.username = "app".to_string();
        // Karakter yang merusak URL bila disisipkan tanpa encode.
        c.password = "p@ss/w:rd?#%".to_string();
        c.database = "main".to_string();
        c
    }

    #[test]
    fn postgres_options_take_target_database_and_ssl_from_config() {
        let mut c = secured(DatabaseType::PostgreSQL);
        let opts = pg_connect_options(&c, "127.0.0.1", "6543", "other");
        assert_eq!(opts.get_host(), "127.0.0.1");
        assert_eq!(opts.get_port(), 6543);
        assert_eq!(opts.get_username(), "app");
        assert_eq!(opts.get_database(), Some("other"));
        assert!(matches!(
            opts.get_ssl_mode(),
            sqlx::postgres::PgSslMode::Prefer
        ));

        c.ssl_enabled = true;
        c.ssl_verify_server = false;
        let opts = pg_connect_options(&c, "db.internal", "not-a-port", "");
        assert_eq!(opts.get_port(), 5432);
        assert!(matches!(
            opts.get_ssl_mode(),
            sqlx::postgres::PgSslMode::Require
        ));
    }

    #[test]
    fn mysql_options_take_target_database_and_ssl_from_config() {
        let mut c = secured(DatabaseType::MySQL);
        let opts = mysql_connect_options(&c, "127.0.0.1", "3307", None);
        assert_eq!(opts.get_host(), "127.0.0.1");
        assert_eq!(opts.get_port(), 3307);
        assert_eq!(opts.get_database(), Some("main"));
        assert!(matches!(
            opts.get_ssl_mode(),
            sqlx::mysql::MySqlSslMode::Disabled
        ));

        assert_eq!(
            mysql_connect_options(&c, "h", "3306", Some("other")).get_database(),
            Some("other")
        );
        assert_eq!(
            mysql_connect_options(&c, "h", "3306", Some("  ")).get_database(),
            None
        );

        c.ssl_enabled = true;
        let opts = mysql_connect_options(&c, "h", "3306", None);
        assert!(matches!(
            opts.get_ssl_mode(),
            sqlx::mysql::MySqlSslMode::Required
        ));
    }

    #[tokio::test]
    async fn session_reset_outcome_decides_the_fate_of_the_connection() {
        assert!(matches!(
            finish_session_reset("SQLite", Ok(Ok(true))),
            Ok(true)
        ));
        // Reset ditolak: tutup baik-baik.
        assert!(matches!(
            finish_session_reset("SQLite", Ok(Ok(false))),
            Ok(false)
        ));
        // Koneksi rusak: tutup paksa.
        let broken = sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        ));
        assert!(finish_session_reset("MySQL", Ok(Err(broken))).is_err());

        let elapsed = tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .expect_err("pending future must time out");
        let err = finish_session_reset("PostgreSQL", Err(elapsed)).expect_err("timeout");
        assert!(err.to_string().contains("session reset timed out"), "{err}");
    }

    #[tokio::test]
    async fn sqlite_reset_rolls_back_a_leftover_transaction() {
        use sqlx::Connection;
        let mut conn = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::query("CREATE TABLE t (id INTEGER)")
            .execute(&mut conn)
            .await
            .expect("create");

        // Tanpa transaksi terbuka: tidak ada yang dilakukan, koneksi tetap sah.
        assert!(matches!(reset_sqlite_session(&mut conn).await, Ok(true)));

        sqlx::raw_sql("BEGIN; INSERT INTO t VALUES (1)")
            .execute(&mut conn)
            .await
            .expect("open transaction");
        assert!(matches!(reset_sqlite_session(&mut conn).await, Ok(true)));

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM t")
            .fetch_one(&mut conn)
            .await
            .expect("count");
        assert_eq!(count, 0);
        // Transaksi baru bisa dimulai lagi di koneksi yang sama.
        sqlx::raw_sql("BEGIN; COMMIT")
            .execute(&mut conn)
            .await
            .expect("fresh transaction");
    }

    #[test]
    fn watchdog_age_stays_above_the_connect_timeout() {
        // The watchdog must not reclaim an attempt that is still within its own
        // connect budget, or it would cancel connections that are about to land.
        assert!(PENDING_POOL_MAX_AGE > CONNECT_TIMEOUT);
    }
}
