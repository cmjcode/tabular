use crate::{models, modules, ssh_tunnel, window_egui::Tabular};
use log::debug;
use mongodb::Client as MongoClient;
use redis::{Client, aio::ConnectionManager};
use sqlx::{
    mysql::MySqlPoolOptions, postgres::PgPoolOptions, sqlite::SqlitePoolOptions,
};
use once_cell::sync::Lazy;
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
const DRIVER_TIMEOUT: Duration = Duration::from_secs(10);

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
static CANCEL_FLAGS: Lazy<Mutex<HashMap<i64, Arc<AtomicBool>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Register a fresh, un-cancelled flag for a new attempt, replacing any flag
/// left over from a previous one.
pub(crate) fn begin_connect_attempt(connection_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut flags) = CANCEL_FLAGS.lock() {
        flags.insert(connection_id, flag.clone());
    }
    flag
}

/// Ask the in-flight attempt for this connection to unwind. No-op if nothing is
/// running.
pub(crate) fn signal_connect_cancel(connection_id: i64) {
    if let Ok(flags) = CANCEL_FLAGS.lock()
        && let Some(flag) = flags.get(&connection_id)
    {
        flag.store(true, Ordering::SeqCst);
    }
}

fn current_cancel_flag(connection_id: i64) -> Option<Arc<AtomicBool>> {
    CANCEL_FLAGS.lock().ok()?.get(&connection_id).cloned()
}

/// True if this connection's current attempt has been cancelled.
pub(crate) fn connect_was_cancelled(connection_id: i64) -> bool {
    current_cancel_flag(connection_id).is_some_and(|f| f.load(Ordering::SeqCst))
}

/// Forget a connection's flag once no attempt is outstanding.
fn end_connect_attempt(connection_id: i64) {
    if let Ok(mut flags) = CANCEL_FLAGS.lock() {
        flags.remove(&connection_id);
    }
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
            return Err("SSH host tidak boleh kosong".to_string());
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
            return Err("Database host tidak boleh kosong".to_string());
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
        "Gagal terhubung ke host [{}:{}]: Jaringan/Internet tidak terjangkau (Host Offline).",
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
        Ok(Err(e)) => Err(format!("Jaringan/Internet tidak terhubung ({})", e)),
        Err(_) => Err(format!(
            "DNS tidak merespons dalam {} detik",
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
        .map_err(|e| format!("Gagal resolve host '{}': {}", host, e))?;

    if socket_addrs.is_empty() {
        return Err(format!("Host '{}' tidak valid", host));
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
                    "Gagal resolve host '{}': Jaringan/Internet tidak terhubung ({})",
                    host, e
                ));
            }
            Err(_) => {
                return Err(format!(
                    "Gagal resolve host '{}': DNS tidak merespons dalam {} detik",
                    host,
                    DNS_TIMEOUT.as_secs()
                ));
            }
        };

    if socket_addrs.is_empty() {
        return Err(format!("Host '{}' tidak valid", host));
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
        let succeeded = tabular
            .shared_connection_pools
            .lock()
            .map(|pools| pools.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
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
            || tabular
                .shared_connection_pools
                .lock()
                .is_ok_and(|pools| pools.contains_key(&connection_id));

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
        let started = *tabular.pending_started_at.entry(connection_id).or_insert(now);

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
                        "Koneksi tidak merespons dalam {} detik dan dihentikan. Silakan coba hubungkan ulang.",
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
) -> Option<models::enums::DatabasePool> {
    let attempt = async {
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            create_connection_pool_for_config_inner(connection),
        )
        .await
        {
            Ok(pool) => pool,
            Err(_) => {
                debug!(
                    "⏰ Connect timed out after {}s for connection {:?}",
                    CONNECT_TIMEOUT.as_secs(),
                    connection.id
                );
                None
            }
        }
    };

    // Saved connections can be cancelled; ad-hoc configs without an id (test
    // dialogs, temporary pools) have nothing to key a flag on.
    let Some(flag) = connection.id.and_then(current_cancel_flag) else {
        return attempt.await;
    };

    tokio::select! {
        pool = attempt => pool,
        _ = wait_for_cancel(flag) => {
            // Dropping `attempt` here tears down the half-open socket instead of
            // leaving it to run to completion in the background.
            debug!("🚫 Connect cancelled for connection {:?}", connection.id);
            None
        }
    }
}

async fn create_connection_pool_for_config_inner(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    if let Err(e) = check_host_reachability_async(connection, 2500).await {
        debug!(
            "❌ Fast-fail TCP reachability check failed for connection {:?}: {}",
            connection.id, e
        );
        return None;
    }

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
                    return None;
                }
            };
            let _encoded_username = modules::url_encode(&connection.username);
            let _encoded_password = modules::url_encode(&connection.password);
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                _encoded_username, _encoded_password, target_host, target_port, connection.database
            );

            let mut last_err: Option<sqlx::Error> = None;

            for attempt in 1..=2u8 {
                let start = std::time::Instant::now();
                let (min_conns, test_before, acquire_secs) = match attempt {
                    1 => (0u32, false, 5u64),
                    _ => (1u32, true, 5u64),
                };

                let pool_result = MySqlPoolOptions::new()
                    .max_connections(10)
                    .min_connections(min_conns)
                    .acquire_timeout(std::time::Duration::from_secs(acquire_secs))
                    .idle_timeout(std::time::Duration::from_secs(600))
                    .max_lifetime(std::time::Duration::from_secs(1800))
                    .test_before_acquire(test_before)
                    .after_connect(|conn, _| {
                        Box::pin(async move {
                            let _ = sqlx::query("SET SESSION wait_timeout = 600")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION interactive_timeout = 600")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION net_read_timeout = 120")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION net_write_timeout = 120")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'")
                                .execute(&mut *conn)
                                .await;
                            Ok(())
                        })
                    })
                    .connect(&connection_string)
                    .await;

                match pool_result {
                    Ok(pool) => {
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "✅ Created MySQL connection pool (attempt {}, {} ms) for connection {:?}",
                            attempt, elapsed, connection.id
                        );
                        return Some(models::enums::DatabasePool::MySQL(Arc::new(pool)));
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
            }
            None
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
                    return None;
                }
            };
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                target_host,
                target_port,
                connection.database
            );

            let pool_result = PgPoolOptions::new()
                .max_connections(15)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .max_lifetime(std::time::Duration::from_secs(1800))
                .test_before_acquire(false)
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => {
                    let database_pool = models::enums::DatabasePool::PostgreSQL(Arc::new(pool));
                    Some(database_pool)
                }
                Err(e) => {
                    debug!("Failed to create PostgreSQL pool: {}", e);
                    None
                }
            }
        }
        models::enums::DatabaseType::SQLite => {
            let connection_string = format!("sqlite:{}", connection.host);

            let pool_result = SqlitePoolOptions::new()
                .max_connections(5)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .max_lifetime(std::time::Duration::from_secs(1800))
                .test_before_acquire(false)
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => {
                    let database_pool = models::enums::DatabasePool::SQLite(Arc::new(pool));
                    Some(database_pool)
                }
                Err(e) => {
                    debug!("Failed to create SQLite pool: {}", e);
                    None
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
                    return None;
                }
            };
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", target_host, target_port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, target_host, target_port
                )
            };

            debug!(
                "Creating new Redis connection manager for: {}",
                connection.name
            );
            match Client::open(connection_string) {
                // ConnectionManager retries internally and has no timeout of its own.
                Ok(client) => {
                    match tokio::time::timeout(DRIVER_TIMEOUT, ConnectionManager::new(client)).await
                    {
                        Ok(Ok(manager)) => {
                            let database_pool =
                                models::enums::DatabasePool::Redis(Arc::new(manager));
                            Some(database_pool)
                        }
                        Ok(Err(e)) => {
                            debug!("Failed to create Redis connection manager: {}", e);
                            None
                        }
                        Err(_) => {
                            debug!(
                                "Redis connection manager timed out after {}s",
                                DRIVER_TIMEOUT.as_secs()
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to create Redis client: {}", e);
                    None
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
                    return None;
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
                    Some(pool)
                }
                _ => {
                    debug!("Failed to create MongoDB client (timeout or error)");
                    None
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
                    return None;
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
                Ok(Ok(pool)) => Some(models::enums::DatabasePool::MsSQL(Arc::new(pool))),
                Ok(Err(e)) => {
                    debug!("MsSQL pool creation failed: {}", e);
                    None
                }
                Err(_) => {
                    debug!(
                        "MsSQL pool creation timed out after {}s",
                        DRIVER_TIMEOUT.as_secs()
                    );
                    None
                }
            }
        }
        models::enums::DatabaseType::ApiHttp => {
            // API-HTTP connections do not use a database pool
            None
        }
    }
}

/// Create a database pool (legacy / refresh path). Delegates to create_connection_pool_for_config.
///
/// Bounded by [`CONNECT_TIMEOUT`], same as the primary path.
#[allow(dead_code)]
pub(crate) async fn create_database_pool(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    match tokio::time::timeout(CONNECT_TIMEOUT, create_database_pool_inner(connection)).await {
        Ok(pool) => pool,
        Err(_) => {
            debug!(
                "⏰ Connect (refresh path) timed out after {}s for connection {:?}",
                CONNECT_TIMEOUT.as_secs(),
                connection.id
            );
            None
        }
    }
}

async fn create_database_pool_inner(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            return create_connection_pool_for_config(connection).await;
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
                    return None;
                }
            };
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                target_host,
                target_port,
                connection.database
            );

            match PgPoolOptions::new()
                .max_connections(3)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .connect(&connection_string)
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::PostgreSQL(Arc::new(pool))),
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::SQLite => {
            let connection_string = format!("sqlite:{}", connection.host);

            match SqlitePoolOptions::new()
                .max_connections(3)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .connect(&connection_string)
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::SQLite(Arc::new(pool))),
                Err(_e) => None,
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
                    return None;
                }
            };
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", target_host, target_port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, target_host, target_port
                )
            };

            match Client::open(connection_string) {
                Ok(client) => {
                    match tokio::time::timeout(DRIVER_TIMEOUT, ConnectionManager::new(client)).await
                    {
                        Ok(Ok(manager)) => {
                            Some(models::enums::DatabasePool::Redis(Arc::new(manager)))
                        }
                        _ => None,
                    }
                }
                Err(_e) => None,
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
                    return None;
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
                    .max_connections(5) // smaller size for temp/check connections
                    .build(),
            )
            .await
            {
                Ok(Ok(pool)) => Some(models::enums::DatabasePool::MsSQL(Arc::new(pool))),
                Ok(Err(e)) => {
                    debug!("MsSQL temp pool creation failed: {}", e);
                    None
                }
                Err(_) => {
                    debug!(
                        "MsSQL temp pool creation timed out after {}s",
                        DRIVER_TIMEOUT.as_secs()
                    );
                    None
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
                    return None;
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
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                MongoClient::with_uri_str(uri),
            )
            .await
            {
                Ok(Ok(client)) => Some(models::enums::DatabasePool::MongoDB(Arc::new(client))),
                _ => None,
            }
        }
        models::enums::DatabaseType::ApiHttp => None,
    }
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
        Ok(pool) => pool,
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
                COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys \
         FROM connections WHERE id = ?"
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
    let ssh_auth_method: String = row.try_get("ssh_auth_method").unwrap_or_else(|_| "key".to_string());
    let ssh_private_key: String = row.try_get("ssh_private_key").unwrap_or_default();
    let ssh_password: String = row.try_get("ssh_password").unwrap_or_default();
    let ssh_accept_unknown_host_keys: i64 = row.try_get("ssh_accept_unknown_host_keys").unwrap_or(0);

    Some(models::structs::ConnectionConfig {
        id,
        name,
        host,
        port,
        username,
        password,
        database,
        connection_type: match conn_type_str.as_str() {
            "MySQL" => models::enums::DatabaseType::MySQL,
            "PostgreSQL" => models::enums::DatabaseType::PostgreSQL,
            "Redis" => models::enums::DatabaseType::Redis,
            "MsSQL" => models::enums::DatabaseType::MsSQL,
            "MongoDB" => models::enums::DatabaseType::MongoDB,
            _ => models::enums::DatabaseType::SQLite,
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
        custom_views: Vec::new(),
        replication_master_id: None,
    })
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
                COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys \
         FROM connections WHERE id = ?"
    )
    .bind(connection_id)
    .fetch_optional(cache_pool)
    .await
    .map_err(|e| format!("Failed to read connection from SQLite: {}", e))?;

    let row = match row_opt {
        Some(r) => r,
        None => return Err(format!("Connection ID {} not found in local store", connection_id)),
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

    let connection = models::structs::ConnectionConfig {
        id: Some(id),
        name,
        host,
        port,
        username,
        password,
        database: database_name,
        connection_type: match connection_type.as_str() {
            "MySQL" => models::enums::DatabaseType::MySQL,
            "PostgreSQL" => models::enums::DatabaseType::PostgreSQL,
            "Redis" => models::enums::DatabaseType::Redis,
            "MsSQL" => models::enums::DatabaseType::MsSQL,
            "MongoDB" => models::enums::DatabaseType::MongoDB,
            _ => models::enums::DatabaseType::SQLite,
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
        custom_views: Vec::new(),
        replication_master_id: None,
    };

    match create_connection_pool_for_config(&connection).await {
        Some(pool) => Ok(pool),
        // Distinguish a deliberate cancel from a genuine failure, so the sidebar
        // doesn't tell the user to check credentials they never got to use.
        None if connect_was_cancelled(connection_id) => {
            Err("Percobaan koneksi dibatalkan.".to_string())
        }
        None => Err("Failed to connect to database server. Please check host, port, credentials, or network.".to_string()),
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
    begin_connect_attempt(connection_id);

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
                Some(pool) => {
                    debug!(
                        "✅ Background: Successfully created pool for connection {}",
                        connection_id
                    );
                    if let Ok(mut shared_pools) = shared_pools.lock() {
                        shared_pools.insert(connection_id, pool);
                    }
                }
                None => {
                    debug!(
                        "❌ Background: Failed to create pool for connection {}",
                        connection_id
                    );
                }
            }
        });
    }
}

/// Ensure a background pool creation is in progress. No-op if pool already exists or pending.
pub(crate) fn ensure_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
    let has_pool = tabular.connection_pools.contains_key(&connection_id)
        || tabular
            .shared_connection_pools
            .lock()
            .map(|p| p.contains_key(&connection_id))
            .unwrap_or(false);
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

    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    if let Ok(shared_pools) = tabular.shared_connection_pools.lock()
        && let Some(shared_pool) = shared_pools.get(&connection_id)
    {
        debug!(
            "✅ Using background-created connection pool for connection {}",
            connection_id
        );
        let pool = shared_pool.clone();
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

    if let Some(pool) = tabular.connection_pools.get(&connection_id) {
        return Some(pool.clone());
    }

    let shared = tabular
        .shared_connection_pools
        .lock()
        .ok()
        .and_then(|pools| pools.get(&connection_id).cloned());

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

/// Non-blocking version. Returns None immediately if pool is currently being created.
pub(crate) async fn try_get_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    cleanup_completed_background_pools(tabular);
    cleanup_stuck_pending_connections(tabular);

    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    if tabular.pending_connection_pools.contains(&connection_id) {
        debug!(
            "⏳ Connection pool creation in progress for connection {}, skipping for now",
            connection_id
        );
        return None;
    }

    get_or_create_connection_pool(tabular, connection_id).await
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
            tokio::time::sleep(std::time::Duration::from_millis(
                500 + attempt as u64 * 200,
            ))
            .await;
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
    tabular.connection_pools.remove(&connection_id);
    clear_pending_state(tabular, connection_id);
    end_connect_attempt(connection_id);

    if let Ok(mut shared_pools) = tabular.shared_connection_pools.lock() {
        shared_pools.remove(&connection_id);
    }

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

    debug!("🚫 Cancelling connect attempt for connection {}", connection_id);

    signal_connect_cancel(connection_id);
    clear_pending_state(tabular, connection_id);
    tabular.refreshing_connections.remove(&connection_id);
    tabular.connection_errors.insert(
        connection_id,
        "Percobaan koneksi dibatalkan oleh pengguna.".to_string(),
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
    fn watchdog_age_stays_above_the_connect_timeout() {
        // The watchdog must not reclaim an attempt that is still within its own
        // connect budget, or it would cancel connections that are about to land.
        assert!(PENDING_POOL_MAX_AGE > CONNECT_TIMEOUT);
    }
}
