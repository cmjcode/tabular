//! Jembatan antara `ConnectionConfig` dan driver plugin.
//!
//! `ConnectionConfig` tidak pernah dikirim ke plugin; dari sini hanya
//! [`ConnectParams`] milik koneksi itu yang diteruskan.

use super::{ConnectParams, OptionKind, PluginPool, TlsParams, registry, run_blocking};
use crate::models::enums::DatabasePool;
use crate::models::structs::ConnectionConfig;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Pool plugin yang sedang hidup per id koneksi, supaya pemanggil yang hanya
/// memegang `ConnectionConfig` (mis. autocomplete kolom) tidak membuka sesi baru.
fn live_pools() -> &'static Mutex<HashMap<i64, Weak<PluginPool>>> {
    static LIVE: OnceLock<Mutex<HashMap<i64, Weak<PluginPool>>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn live_pool(connection_id: i64) -> Option<Arc<PluginPool>> {
    live_pools().lock().ok()?.get(&connection_id)?.upgrade()
}

fn remember_pool(connection_id: i64, pool: &Arc<PluginPool>) {
    if let Ok(mut map) = live_pools().lock() {
        map.retain(|_, weak| weak.strong_count() > 0);
        map.insert(connection_id, Arc::downgrade(pool));
    }
}

/// Nama secret-store untuk opsi plugin bertipe `secret`.
pub fn option_secret_name(connection_id: i64, key: &str) -> String {
    crate::secrets::connection_secret_name(connection_id, &format!("opt:{key}"))
}

/// Sebelum disimpan ke `connections.db`: opsi bertipe `secret` dipindah ke
/// secret store dan diganti sentinel. Bila driver tidak terpasang, nilai
/// disimpan apa adanya (yang sudah sentinel tetap sentinel).
pub fn protect_secret_options(
    connection_id: i64,
    engine_id: &str,
    options: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let Some(descriptor) = registry::descriptor(engine_id) else {
        return options.clone();
    };
    let mut stored = options.clone();
    for field in descriptor
        .options
        .iter()
        .filter(|f| f.kind == OptionKind::Secret)
    {
        if let Some(value) = options.get(&field.key) {
            let persisted = crate::secrets::store_or_keep(
                &option_secret_name(connection_id, &field.key),
                value,
            );
            stored.insert(field.key.clone(), persisted);
        }
    }
    stored
}

/// Setelah dibaca dari `connections.db`: sentinel diganti nilai aslinya.
pub fn resolve_secret_options(
    connection_id: i64,
    options: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    options
        .iter()
        .map(|(k, v)| {
            let value = if v == crate::secrets::SECRET_SENTINEL {
                crate::secrets::resolve_readonly(&option_secret_name(connection_id, k), v)
            } else {
                v.clone()
            };
            (k.clone(), value)
        })
        .collect()
}

/// Hapus secret opsi plugin saat koneksi dihapus.
pub fn delete_secret_options(connection_id: i64, options: &BTreeMap<String, String>) {
    for key in options.keys() {
        crate::secrets::delete_secret(&option_secret_name(connection_id, key));
    }
}

/// Uraikan kolom JSON `plugin_options`; nilai rusak dianggap kosong.
pub fn parse_options_json(json: &str) -> BTreeMap<String, String> {
    if json.trim().is_empty() {
        return BTreeMap::new();
    }
    serde_json::from_str(json).unwrap_or_else(|e| {
        log::warn!("[DRIVER-PLUGIN] Ignoring invalid plugin_options JSON: {e}");
        BTreeMap::new()
    })
}

pub fn options_json(options: &BTreeMap<String, String>) -> String {
    serde_json::to_string(options).unwrap_or_else(|_| "{}".to_string())
}

/// Baca `plugin_options` satu koneksi dan pulihkan nilai rahasianya. Kolom
/// yang belum ada (database lama, fixture test) menghasilkan map kosong.
pub async fn load_plugin_options(
    pool: &sqlx::SqlitePool,
    connection_id: i64,
) -> BTreeMap<String, String> {
    let json = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(plugin_options, '{}') FROM connections WHERE id = ?",
    )
    .bind(connection_id)
    .fetch_optional(pool)
    .await;
    match json {
        Ok(Some(json)) => resolve_secret_options(connection_id, &parse_options_json(&json)),
        Ok(None) => BTreeMap::new(),
        Err(e) => {
            log::debug!("[DRIVER-PLUGIN] plugin_options unavailable for {connection_id}: {e}");
            BTreeMap::new()
        }
    }
}

/// Isi `plugin_options` untuk semua koneksi plugin di daftar.
pub async fn fill_plugin_options(pool: &sqlx::SqlitePool, connections: &mut [ConnectionConfig]) {
    for conn in connections.iter_mut() {
        if let (Some(id), Some(_)) = (conn.id, conn.connection_type.plugin_id()) {
            conn.plugin_options = load_plugin_options(pool, id).await;
        }
    }
}

/// Simpan `plugin_options` koneksi (opsi rahasia dipindah ke secret store).
pub async fn save_plugin_options(
    pool: &sqlx::SqlitePool,
    connection: &ConnectionConfig,
    connection_id: i64,
) -> Result<(), sqlx::Error> {
    let Some(engine_id) = connection.connection_type.plugin_id() else {
        return Ok(());
    };
    let stored = protect_secret_options(connection_id, engine_id, &connection.plugin_options);
    sqlx::query("UPDATE connections SET plugin_options = ? WHERE id = ?")
        .bind(options_json(&stored))
        .bind(connection_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Susun parameter koneksi untuk plugin dari host/port efektif (sudah lewat
/// tunnel bila ada) dan field koneksi.
pub fn connect_params(
    connection: &ConnectionConfig,
    host: String,
    port: String,
    default_port: Option<u16>,
) -> ConnectParams {
    ConnectParams {
        host,
        port: port.trim().parse::<u16>().ok().or(default_port),
        username: connection.username.clone(),
        password: connection.password.clone(),
        database: connection.database.clone(),
        tls: TlsParams {
            enabled: connection.ssl_enabled,
            verify_server: connection.ssl_verify_server,
            ca_cert: connection.ssl_ca_cert.clone(),
            client_cert: connection.ssl_client_cert.clone(),
            client_key: connection.ssl_client_key.clone(),
        },
        options: connection.plugin_options.clone(),
    }
}

/// Buka sesi engine plugin untuk koneksi ini.
pub async fn create_plugin_pool(
    connection: &ConnectionConfig,
    engine_id: &str,
) -> Result<DatabasePool, String> {
    let driver = registry::driver(engine_id).map_err(|e| e.to_string())?;
    let descriptor = driver.descriptor().clone();
    let (host, port) = if connection.ssh_enabled {
        if !descriptor.capabilities.ssh_tunnel {
            return Err(format!("{} does not support SSH tunnels", descriptor.name));
        }
        crate::connection::pool::resolve_connection_target_async(connection).await?
    } else {
        (connection.host.clone(), connection.port.clone())
    };
    let params = connect_params(connection, host, port, descriptor.default_port);
    log::debug!(
        "[DRIVER-PLUGIN] Connecting {:?} with engine '{}'",
        connection.id,
        engine_id
    );
    let session = run_blocking(move || driver.connect(params))
        .await
        .map_err(|e| e.to_string())?;
    let pool = Arc::new(PluginPool {
        engine_id: engine_id.to_string(),
        capabilities: descriptor.capabilities,
        session,
    });
    if let Some(id) = connection.id {
        remember_pool(id, &pool);
    }
    Ok(DatabasePool::Plugin(pool))
}

/// Pool hidup untuk koneksi ini, atau buka sesi baru.
pub async fn pool_for(
    connection: &ConnectionConfig,
    engine_id: &str,
) -> Result<Arc<PluginPool>, String> {
    if let Some(pool) = connection.id.and_then(live_pool) {
        return Ok(pool);
    }
    match create_plugin_pool(connection, engine_id).await? {
        DatabasePool::Plugin(pool) => Ok(pool),
        _ => Err("unexpected pool type".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_params_use_effective_target_and_default_port() {
        let mut conn = ConnectionConfig {
            username: "u".into(),
            password: "p".into(),
            database: "d".into(),
            ssl_enabled: true,
            ..Default::default()
        };
        conn.plugin_options.insert("region".into(), "eu".into());
        let p = connect_params(&conn, "127.0.0.1".into(), "".into(), Some(8123));
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, Some(8123));
        assert_eq!(p.password, "p");
        assert!(p.tls.enabled);
        assert_eq!(p.options.get("region").map(String::as_str), Some("eu"));

        let p = connect_params(&conn, "h".into(), " 9000 ".into(), Some(8123));
        assert_eq!(p.port, Some(9000));
    }

    #[test]
    fn options_json_roundtrip_and_bad_input() {
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), "1".to_string());
        assert_eq!(parse_options_json(&options_json(&m)), m);
        assert!(parse_options_json("not json").is_empty());
        assert!(parse_options_json("").is_empty());
    }

    #[test]
    fn non_sentinel_options_resolve_unchanged() {
        let mut m = BTreeMap::new();
        m.insert("region".to_string(), "eu".to_string());
        assert_eq!(resolve_secret_options(1, &m), m);
    }
}
