// MsSQL driver module now built unconditionally (feature flag removed)
use crate::models;
use crate::window_egui; // for Tabular type

use mssql_client::{Client, Config, Credentials, Ready, SqlValue};

// MsSQL connectivity is provided by mssql-client (praxiomlabs rust-mssql-driver)
// with pooling from mssql-driver-pool. The helpers below centralize config,
// connection, and dynamic value-to-string conversion for the whole app.

/// Batas waktu per perintah di tingkat driver. `mssql-client` 0.20 memakai
/// 30 detik secara bawaan, yang memotong query panjang tanpa mempedulikan
/// pengaturan query-timeout user (0 = tanpa batas). Batas yang sebenarnya
/// ditegakkan aplikasi (`tokio::time::timeout` + ATTENTION di eksekutor);
/// nilai ini hanya plafon longgar supaya perintah di luar eksekutor (metadata,
/// tes koneksi) tidak menggantung selamanya. Pengaturan user tidak terjangkau
/// dari sini karena config dibuat sekali per pool.
const MSSQL_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Key `plugin_options` untuk mode enkripsi koneksi MsSQL.
pub(crate) const OPT_ENCRYPTION: &str = "mssql_encryption";
/// Key `plugin_options` untuk versi TDS yang diminta saat login.
pub(crate) const OPT_TDS_VERSION: &str = "mssql_tds_version";

/// Mode enkripsi koneksi MsSQL.
///
/// TLS di `mssql-client` memakai rustls yang minimal TLS 1.2, sedangkan SQL
/// Server 2005/2008/2008 R2 (dan 2012/2014 tanpa patch) hanya bisa TLS
/// 1.0/1.1. Server seperti itu hanya bisa dijangkau lewat `Disabled`, sama
/// seperti jTDS dengan `ssl=off` (bawaannya).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MssqlEncryption {
    /// Seluruh sesi dienkripsi TLS (bawaan, perilaku lama).
    #[default]
    Tls,
    /// TLS hanya untuk paket login, data setelahnya plaintext (`Encrypt=false`).
    LoginOnly,
    /// Tanpa TLS sama sekali; kredensial terkirim plaintext. Untuk server lama.
    Disabled,
}

impl MssqlEncryption {
    pub(crate) const ALL: [MssqlEncryption; 3] = [Self::Tls, Self::LoginOnly, Self::Disabled];

    pub(crate) fn as_key(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::LoginOnly => "login_only",
            Self::Disabled => "disabled",
        }
    }

    pub(crate) fn from_key(key: &str) -> Self {
        match key.trim().to_ascii_lowercase().as_str() {
            "login_only" | "false" | "no" | "optional" => Self::LoginOnly,
            "disabled" | "no_tls" | "off" | "none" => Self::Disabled,
            _ => Self::Tls,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tls => "Full TLS (default)",
            Self::LoginOnly => "Login only",
            Self::Disabled => "Disabled (legacy server)",
        }
    }
}

/// Versi TDS yang bisa dipilih user; `None` = bawaan driver (7.4, server
/// lama otomatis menawar turun).
pub(crate) const TDS_VERSION_CHOICES: [(&str, &str); 6] = [
    ("", "Auto"),
    ("7.4", "7.4 (SQL Server 2012+)"),
    ("7.3b", "7.3B (SQL Server 2008 R2)"),
    ("7.3a", "7.3A (SQL Server 2008)"),
    ("7.2", "7.2 (SQL Server 2005)"),
    ("7.1", "7.1 (SQL Server 2000)"),
];

/// Opsi koneksi MsSQL yang tersimpan di `ConnectionConfig::plugin_options`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MssqlOptions {
    pub encryption: MssqlEncryption,
    pub tds_version: Option<mssql_client::TdsVersion>,
}

impl MssqlOptions {
    pub(crate) fn from_options(options: &std::collections::BTreeMap<String, String>) -> Self {
        let encryption = options
            .get(OPT_ENCRYPTION)
            .map(|v| MssqlEncryption::from_key(v))
            .unwrap_or_default();
        let tds_version = options
            .get(OPT_TDS_VERSION)
            .and_then(|v| parse_tds_version(v));
        Self {
            encryption,
            tds_version,
        }
    }

    pub(crate) fn from_connection(conn: &models::structs::ConnectionConfig) -> Self {
        Self::from_options(&conn.plugin_options)
    }

    fn apply(self, mut config: Config) -> Config {
        if let Some(v) = self.tds_version {
            config = config.tds_version(v);
        }
        match self.encryption {
            MssqlEncryption::Tls => config,
            MssqlEncryption::LoginOnly => config.encrypt(false),
            MssqlEncryption::Disabled => config.no_tls(true),
        }
    }
}

/// "7.1" / "7.3b" -> `TdsVersion`. TDS 8.0 sengaja ditolak: mode strict
/// mewajibkan TLS dan tidak relevan untuk server lama.
fn parse_tds_version(value: &str) -> Option<mssql_client::TdsVersion> {
    use mssql_client::TdsVersion;
    match value.trim().to_ascii_lowercase().as_str() {
        "7.1" => Some(TdsVersion::V7_1),
        "7.2" => Some(TdsVersion::V7_2),
        "7.3" | "7.3a" => Some(TdsVersion::V7_3A),
        "7.3b" => Some(TdsVersion::V7_3B),
        "7.4" => Some(TdsVersion::V7_4),
        _ => None,
    }
}

/// Build a Config for a direct (non-pooled) MsSQL connection.
/// Mirrors the app-wide defaults: SQL auth + trusted server certificate.
pub(crate) fn mssql_config(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
    options: MssqlOptions,
) -> Config {
    let mut config = Config::new()
        .host(host)
        .port(port)
        .credentials(Credentials::sql_server(
            username.to_string(),
            password.to_string(),
        ))
        .connect_timeout(std::time::Duration::from_secs(10))
        .trust_server_certificate(true);
    if let Some(db) = database
        && !db.is_empty()
    {
        config = config.database(db.to_string());
    }
    // Dua-duanya diisi: driver membaca field lama, `timeouts` adalah API barunya.
    config.command_timeout = MSSQL_COMMAND_TIMEOUT;
    config.timeouts.command_timeout = MSSQL_COMMAND_TIMEOUT;
    options.apply(config)
}

/// Tambahkan saran mode legacy pada error yang khas server lama: handshake
/// TLS gagal (server hanya TLS 1.0/1.1) atau server menolak tanpa enkripsi.
pub(crate) fn explain_connect_error(err: &str, options: MssqlOptions) -> String {
    let lower = err.to_ascii_lowercase();
    let tls_failed = lower.contains("tls")
        || lower.contains("handshake")
        || lower.contains("protocol version")
        || lower.contains("alert")
        || lower.contains("connection reset")
        || lower.contains("unexpected eof");
    if options.encryption != MssqlEncryption::Disabled && tls_failed {
        format!(
            "{err}\n\nOlder SQL Server versions (2008 R2 and earlier, or unpatched 2012/2014) \
             only support TLS 1.0/1.1, which Tabular does not allow. Set Encryption to \
             \"Disabled (legacy server)\" in the connection settings (like jTDS with ssl=off)."
        )
    } else if options.encryption == MssqlEncryption::Disabled
        && (lower.contains("encryption") || lower.contains("no_tls"))
    {
        format!(
            "{err}\n\nThe server requires encryption (Force Encryption is on), so an \
             unencrypted connection is not possible. Try Encryption = \"Full TLS\"."
        )
    } else {
        err.to_string()
    }
}

/// Open a one-off MsSQL connection (no pool).
pub(crate) async fn connect_mssql(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
    options: MssqlOptions,
) -> Result<Client<Ready>, String> {
    Client::connect(mssql_config(
        host, port, username, password, database, options,
    ))
    .await
    .map_err(|e| explain_connect_error(&e.to_string(), options))
}

/// Nama kolom bantu nomor baris untuk paginasi tanpa `OFFSET/FETCH`. Kolom
/// ini dibuang dari hasil oleh [`run_query_limited`] /
/// [`run_query_multi_limited`] sehingga tidak pernah tampil di grid/export.
pub(crate) const ROW_NUMBER_COLUMN: &str = "__tabular_rn";

/// Versi mayor server per connection id (11 = SQL Server 2012). Diisi sekali
/// saat pool dibuat; dibaca saat menyusun SQL paginasi.
static SERVER_MAJOR: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, u32>>> =
    std::sync::LazyLock::new(Default::default);

/// Versi mayor pertama yang mendukung `OFFSET ... FETCH` (SQL Server 2012).
const FIRST_OFFSET_FETCH_MAJOR: u32 = 11;

pub(crate) fn remember_server_major(connection_id: i64, major: u32) {
    if let Ok(mut map) = SERVER_MAJOR.lock() {
        map.insert(connection_id, major);
    }
}

/// `true` bila server koneksi ini diketahui lebih tua dari SQL Server 2012
/// (tidak punya `OFFSET/FETCH`). Versi yang belum terdeteksi dianggap modern.
pub(crate) fn is_legacy_server(connection_id: i64) -> bool {
    SERVER_MAJOR
        .lock()
        .ok()
        .and_then(|map| map.get(&connection_id).copied())
        .is_some_and(|major| major < FIRST_OFFSET_FETCH_MAJOR)
}

/// "10.50.6000.34" -> 10.
pub(crate) fn parse_product_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.trim().parse().ok()
}

/// Baca `SERVERPROPERTY('ProductVersion')` lewat pool. Berlaku sejak SQL
/// Server 2000. `None` bila gagal (dianggap server modern).
pub(crate) async fn detect_server_major(pool: &mssql_driver_pool::Pool) -> Option<u32> {
    let rows = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        pooled_query(
            pool,
            "SELECT CAST(SERVERPROPERTY('ProductVersion') AS NVARCHAR(128))",
        ),
    )
    .await
    .ok()?
    .map_err(|e| log::debug!("[MSSQL] version probe failed: {e}"))
    .ok()?;
    // Pakai konversi grid: `get_string` mengembalikan None untuk hasil CAST
    // dari sql_variant di SQL Server 2008 R2.
    let version = row_values_to_strings(rows.first()?).into_iter().next()?;
    let major = parse_product_major(&version);
    if major.is_none() {
        log::debug!("[MSSQL] unrecognized ProductVersion {version:?}");
    }
    major
}

/// Indeks kolom [`ROW_NUMBER_COLUMN`] di header, dan buang dari header.
fn take_row_number_column(headers: &mut Vec<String>) -> Option<usize> {
    let idx = headers.iter().position(|h| h == ROW_NUMBER_COLUMN)?;
    headers.remove(idx);
    Some(idx)
}

/// `true` bila salah satu item ORDER BY berupa nomor kolom (`1`, `2 DESC`),
/// yang tidak boleh dipakai di `ROW_NUMBER() OVER (ORDER BY ...)`.
fn order_has_ordinal(order_expr: &str) -> bool {
    order_expr.split(',').any(|item| {
        item.split_whitespace()
            .next()
            .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))
    })
}

/// Paginasi untuk SQL Server < 2012 (tanpa `OFFSET/FETCH`).
///
/// - Halaman pertama dari query sederhana: `SELECT TOP n ...`, `ORDER BY`
///   user tetap utuh (berlaku sejak SQL Server 2000).
/// - Halaman berikutnya: `ROW_NUMBER() OVER (ORDER BY <urutan user>)`
///   disisipkan ke SELECT utama lalu dibungkus `WHERE rn BETWEEN` (SQL
///   Server 2005+). Query `DISTINCT` atau `UNION` dibungkus sebagai derived
///   table supaya nomor baris tidak merusak hasilnya.
///
/// `select_part` sudah tanpa `TOP` (lihat
/// [`sanitize_mssql_select_for_pagination`]); `extra_order` = klausa
/// ` ORDER BY ...` tambahan bila query belum punya urutan sendiri.
pub(crate) fn legacy_paginate(
    select_part: &str,
    extra_order: Option<&str>,
    offset: usize,
    limit: usize,
) -> String {
    let sql = select_part.trim();
    let tokens = crate::connection::sql::top_level_tokens(sql);
    let pos = |t: &str| t.as_ptr() as usize - sql.as_ptr() as usize;

    // ORDER BY terakhir di level teratas = urutan seluruh query.
    let order_at = tokens
        .windows(2)
        .filter(|w| w[0].eq_ignore_ascii_case("ORDER") && w[1].eq_ignore_ascii_case("BY"))
        .map(|w| (pos(w[0]), pos(w[1]) + w[1].len()))
        .next_back();
    let (body, order_expr) = match order_at {
        Some((start, end)) => (sql[..start].trim_end(), Some(sql[end..].trim().to_string())),
        None => (sql, None),
    };
    let order_expr = order_expr.filter(|o| !o.is_empty()).or_else(|| {
        let extra = extra_order?.trim();
        let rest = extra
            .get(..8)
            .filter(|head| head.eq_ignore_ascii_case("ORDER BY"))
            .map(|_| extra[8..].trim())
            .unwrap_or(extra);
        (!rest.is_empty()).then(|| rest.to_string())
    });

    let Some(select_idx) = tokens.iter().position(|t| t.eq_ignore_ascii_case("SELECT")) else {
        return sql.to_string();
    };
    let select_start = pos(tokens[select_idx]);
    let mut insert_at = select_start + "SELECT".len();
    let mut distinct = false;
    if let Some(next) = tokens.get(select_idx + 1)
        && (next.eq_ignore_ascii_case("DISTINCT") || next.eq_ignore_ascii_case("ALL"))
    {
        distinct = next.eq_ignore_ascii_case("DISTINCT");
        insert_at = pos(next) + next.len();
    }
    let set_op = tokens.iter().any(|t| {
        t.eq_ignore_ascii_case("UNION")
            || t.eq_ignore_ascii_case("EXCEPT")
            || t.eq_ignore_ascii_case("INTERSECT")
    });
    let order_clause = order_expr
        .as_deref()
        .map(|o| format!(" ORDER BY {o}"))
        .unwrap_or_default();

    if offset == 0 && !set_op {
        return format!(
            "{} TOP {limit}{}{order_clause}",
            &body[..insert_at],
            &body[insert_at..]
        );
    }

    let over = order_expr
        .as_deref()
        .filter(|o| !order_has_ordinal(o))
        .unwrap_or("(SELECT NULL)");
    let prefix = &body[..select_start];
    let inner = if distinct || set_op {
        format!(
            "SELECT ROW_NUMBER() OVER (ORDER BY {over}) AS [{ROW_NUMBER_COLUMN}], [__tabular_q].* \
             FROM ({}) AS [__tabular_q]",
            &body[select_start..]
        )
    } else {
        format!(
            "{} ROW_NUMBER() OVER (ORDER BY {over}) AS [{ROW_NUMBER_COLUMN}],{}",
            &body[select_start..insert_at],
            &body[insert_at..]
        )
    };
    format!(
        "{prefix}SELECT * FROM ({inner}) AS [__tabular_p] WHERE [{ROW_NUMBER_COLUMN}] BETWEEN {} AND {} \
         ORDER BY [{ROW_NUMBER_COLUMN}]",
        offset + 1,
        offset + limit
    )
}

/// Convert a dynamic SqlValue into the display string used by the data grid.
pub(crate) fn sql_value_to_string(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => crate::models::structs::NULL_CELL.to_string(),
        SqlValue::Bool(v) => v.to_string(),
        SqlValue::TinyInt(v) => v.to_string(),
        SqlValue::SmallInt(v) => v.to_string(),
        SqlValue::Int(v) => v.to_string(),
        SqlValue::BigInt(v) => v.to_string(),
        SqlValue::Float(v) => v.to_string(),
        SqlValue::Double(v) => v.to_string(),
        SqlValue::String(s) => s.clone(),
        SqlValue::Binary(b) => format!("0x{}", hex::encode(b)),
        SqlValue::Decimal(d) | SqlValue::Money(d) | SqlValue::SmallMoney(d) => d.to_string(),
        SqlValue::Uuid(u) => u.to_string(),
        SqlValue::Date(d) => d.format("%Y-%m-%d").to_string(),
        SqlValue::Time(t) => t.format("%H:%M:%S%.f").to_string(),
        SqlValue::DateTime(dt) | SqlValue::SmallDateTime(dt) => {
            dt.format("%Y-%m-%d %H:%M:%S%.f").to_string()
        }
        SqlValue::DateTimeOffset(dto) => dto.format("%Y-%m-%d %H:%M:%S%.f %:z").to_string(),
        SqlValue::Xml(x) => x.clone(),
        // Tvp is send-only and SqlValue is #[non_exhaustive]
        other => format!("{:?}", other),
    }
}

/// Convert every column of a row into display strings.
/// Nilai satu kolom sebagai teks; `None` untuk NULL atau indeks di luar
/// jangkauan. Pengganti `Row::get_string`, yang di `mssql-client` 0.20
/// mengembalikan string kosong untuk NVARCHAR dari SQL Server 2008 R2.
pub(crate) fn cell_string(row: &mssql_client::Row, index: usize) -> Option<String> {
    match row.get_raw(index)? {
        SqlValue::Null => None,
        v => Some(sql_value_to_string(&v)),
    }
}

pub(crate) fn row_values_to_strings(row: &mssql_client::Row) -> Vec<String> {
    (0..row.len())
        .map(|i| match row.get_raw(i) {
            Some(v) => sql_value_to_string(&v),
            None => crate::models::structs::NULL_CELL.to_string(),
        })
        .collect()
}

/// Run a single-statement query through the shared pool and collect all rows.
pub(crate) async fn pooled_query(
    pool: &mssql_driver_pool::Pool,
    sql: &str,
) -> Result<Vec<mssql_client::Row>, String> {
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let client = conn
        .client_mut()
        .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
    let stream = client.query(sql, &[]).await.map_err(|e| e.to_string())?;
    stream.collect_all().await.map_err(|e| e.to_string())
}

/// Daftar database untuk sidebar dari `sys.databases`. Database sistem
/// (master/model/msdb/tempdb) hanya ikut bila diminta lewat preferensi, atau
/// bila tidak ada database user sama sekali supaya koneksi tetap bisa dipakai.
pub(crate) async fn list_mssql_databases(
    pool: &mssql_driver_pool::Pool,
) -> Result<Vec<String>, String> {
    let rows = pooled_query(pool, "SELECT name FROM sys.databases ORDER BY name").await?;
    let names: Vec<String> = rows
        .iter()
        .filter_map(|row| row_values_to_strings(row).into_iter().next())
        .filter(|name| !name.is_empty())
        .collect();
    Ok(visible_mssql_databases(names))
}

fn visible_mssql_databases(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    let is_system = |name: &str| {
        crate::schema_objects::catalog::is_system_database(
            &models::enums::DatabaseType::MsSQL,
            name,
        )
    };
    let has_user_db = names.iter().any(|n| !is_system(n));
    if crate::schema_objects::show_system_objects() || !has_user_db {
        // Database user dulu, sistem di belakang (sama seperti sebelumnya).
        let (user, system): (Vec<String>, Vec<String>) =
            names.into_iter().partition(|n| !is_system(n));
        user.into_iter().chain(system).collect()
    } else {
        names.into_iter().filter(|n| !is_system(n)).collect()
    }
}

/// Muat ulang cache metadata MsSQL (dipanggil Refresh). Hanya daftar
/// database: tabel/view dimuat saat node database dibuka. `false` bila gagal,
/// supaya cache lama tetap dipakai.
pub(crate) async fn fetch_mssql_data(
    connection_id: i64,
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    cache_pool: &sqlx::SqlitePool,
) -> bool {
    let databases = match list_mssql_databases(&pool).await {
        Ok(list) => list,
        Err(e) => {
            log::warn!("[MSSQL] connection {connection_id}: failed to list databases: {e}");
            return false;
        }
    };
    log::info!(
        "[MSSQL] connection {connection_id}: cached {} databases",
        databases.len()
    );
    match crate::cache_data::write_databases_cache(cache_pool, connection_id, &databases).await {
        Ok(()) => true,
        Err(e) => {
            log::warn!("[MSSQL] connection {connection_id}: failed to save database list: {e}");
            false
        }
    }
}

pub(crate) fn load_mssql_structure(
    connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Similar to other drivers: show Databases + DBA Views folders
    let mut main_children = Vec::new();

    // Databases folder with loading marker
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(connection_id);
    let loading_node = models::structs::TreeNode::new(
        "Loading databases...".to_string(),
        models::enums::NodeType::Database,
    );
    databases_folder.children.push(loading_node);
    main_children.push(databases_folder);

    // DBA Views folder with standard children
    let mut dba_folder = models::structs::TreeNode::new(
        "DBA Views".to_string(),
        models::enums::NodeType::DBAViewsFolder,
    );
    dba_folder.connection_id = Some(connection_id);

    let mut dba_children = Vec::new();

    for (name, node_type, query) in
        crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::MsSQL)
    {
        let mut dba_node = models::structs::TreeNode::new(name.to_string(), node_type);
        dba_node.connection_id = Some(connection_id);
        dba_node.is_loaded = false;
        dba_node.query = Some(query.to_string());
        dba_children.push(dba_node);
    }

    dba_folder.children = dba_children;
    main_children.push(dba_folder);

    node.children = main_children;
}

/// Fetch MsSQL tables or views for a specific database (synchronous wrapper like other drivers)
pub(crate) fn fetch_tables_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    // Pakai runtime aplikasi yang berumur panjang: pool yang dibuat (dan
    // di-cache) di dalam runtime sekali pakai rusak begitu runtime itu di-drop.
    let rt = tabular.get_runtime();
    rt.block_on(async {
        // Get or create pool
        let pool_enum =
            crate::connection::get_or_create_connection_pool(tabular, connection_id).await?;
        let pool = match pool_enum {
            crate::models::enums::DatabasePool::MsSQL(p) => p,
            _ => return None,
        };
        list_mssql_tables(&pool, database_name, table_type).await
    })
}

/// Daftar tabel / view MsSQL (`[schema].[name]`) lewat pool yang sudah ada.
/// Aman dipanggil dari task async (tanpa runtime baru).
pub(crate) async fn list_mssql_tables(
    pool: &mssql_driver_pool::Pool,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    // Get a connection from the pool
    let mut conn = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            log::debug!("MsSQL pool get error: {}", e);
            return None;
        }
    };
    let client = conn.client_mut()?;

    // Choose query based on type (include schema for views)
    // Tanpa prefiks database, query membaca database default koneksi untuk
    // setiap node database di sidebar.
    let db = mssql_db_prefix(database_name);
    let query = match table_type {
        // Include schema for tables (some objects not in dbo)
        "table" => format!(
            "SELECT TABLE_SCHEMA, TABLE_NAME FROM {db}INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_TYPE='BASE TABLE' ORDER BY TABLE_NAME"
        ),
        // Include schema for views so we can build fully-qualified names
        "view" => format!(
            "SELECT TABLE_SCHEMA, TABLE_NAME FROM {db}INFORMATION_SCHEMA.VIEWS ORDER BY TABLE_NAME"
        ),
        _ => {
            log::debug!("Unsupported MsSQL table_type: {}", table_type);
            return None;
        }
    };

    let stream = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.query(&query, &[]),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            log::warn!("[MSSQL] listing {table_type}s in {database_name:?} failed: {e}");
            return None;
        }
        Err(_) => {
            log::debug!("MsSQL list query timeout");
            return None;
        }
    };

    let mut items = Vec::new();
    for row in stream.collect_all().await.ok()? {
        // `get_string` bisa None di server lama; konversi grid selalu terisi.
        if let [s, n, ..] = row_values_to_strings(&row).as_slice() {
            items.push(format!("[{}].[{}]", s, n));
        }
    }
    Some(items)
}

/// `[db].` untuk mengkualifikasi katalog (`[db].sys.objects`); kosong bila
/// database tidak disebut (pakai database aktif koneksi).
fn mssql_db_prefix(database_name: &str) -> String {
    let db = database_name.trim();
    if db.is_empty() {
        String::new()
    } else {
        format!("[{}].", db.replace(']', "]]"))
    }
}

/// Fetch MsSQL objects for a specific database by type: procedure | function | trigger
pub(crate) fn fetch_objects_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    object_type: &str,
) -> Option<Vec<String>> {
    // Runtime aplikasi, bukan runtime sekali pakai (lihat fungsi di atas).
    let rt = tabular.get_runtime();
    rt.block_on(async {
        // Get or create pool
        let pool_enum =
            crate::connection::get_or_create_connection_pool(tabular, connection_id).await?;
        let pool = match pool_enum {
            crate::models::enums::DatabasePool::MsSQL(p) => p,
            _ => return None,
        };
        list_mssql_objects(&pool, database_name, object_type).await
    })
}

/// Daftar procedure/function/trigger MsSQL lewat pool yang sudah ada. Aman
/// dipanggil dari task async (tanpa `&mut Tabular`).
pub(crate) async fn list_mssql_objects(
    pool: &mssql_driver_pool::Pool,
    database_name: &str,
    object_type: &str,
) -> Option<Vec<String>> {
    {
        let mut conn = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                log::debug!("MsSQL pool get error: {}", e);
                return None;
            }
        };
        let client = conn.client_mut()?;

        let db = mssql_db_prefix(database_name);
        let query = match object_type {
            // Stored procedures
            "procedure" => {
                // sys.procedures excludes system procedures when filtered by is_ms_shipped = 0
                format!(
                    "SELECT s.name AS schema_name, p.name AS object_name \
                 FROM {db}sys.procedures p \
                 JOIN {db}sys.schemas s ON p.schema_id = s.schema_id \
                 WHERE ISNULL(p.is_ms_shipped,0) = 0 \
                 ORDER BY p.name"
                )
            }
            // Functions: scalar, inline table-valued, multi-statement table-valued, and CLR variants
            "function" => format!(
                "SELECT s.name AS schema_name, o.name AS object_name \
                 FROM {db}sys.objects o \
                 JOIN {db}sys.schemas s ON o.schema_id = s.schema_id \
                 WHERE o.type IN ('FN','IF','TF','AF','FS','FT') \
                 ORDER BY o.name"
            ),
            // Triggers: list DML triggers attached to user tables
            "trigger" => {
                format!(
                    "SELECT ss.name AS schema_name, t.name AS table_name, tr.name AS trigger_name \
                 FROM {db}sys.triggers tr \
                 JOIN {db}sys.tables t ON tr.parent_id = t.object_id \
                 JOIN {db}sys.schemas ss ON t.schema_id = ss.schema_id \
                 WHERE t.type = 'U' \
                 ORDER BY tr.name"
                )
            }
            _ => {
                log::debug!("Unsupported MsSQL object_type: {}", object_type);
                return None;
            }
        };

        let stream = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.query(&query, &[]),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                log::warn!("[MSSQL] listing {object_type}s in {database_name:?} failed: {e}");
                return None;
            }
            Err(_) => {
                log::debug!("MsSQL object list query timeout");
                return None;
            }
        };

        let mut items = Vec::new();
        for row in stream.collect_all().await.ok()? {
            match object_type {
                "procedure" | "function" => {
                    if let [s, n, ..] = row_values_to_strings(&row).as_slice() {
                        items.push(format!("[{}].[{}]", s, n));
                    }
                }
                "trigger" => {
                    if let [s, t, tr, ..] = row_values_to_strings(&row).as_slice() {
                        items.push(format!("[{}].[{}].[{}]", s, t, tr));
                    }
                }
                _ => {}
            }
        }
        Some(items)
    }
}

/// Execute a query using the shared connection pool (mssql-driver-pool)
pub(crate) async fn execute_query(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    // Acquire a client from the pool and delegate to the common runner
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let client = conn
        .client_mut()
        .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
    run_query(client, query).await
}

pub(crate) async fn run_query(
    client: &mut Client<Ready>,
    query: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    run_query_limited(client, query, usize::MAX)
        .await
        .map(|(headers, rows, _truncated)| (headers, rows))
}

/// Seperti [`run_query`], tetapi berhenti mengonversi baris setelah `max_rows`
/// dan melaporkan apakah hasil terpotong.
///
/// Catatan: `mssql-client` 0.20 membaca seluruh respons dari jaringan di dalam
/// `query_multiple` (baris disimpan mentah, didekode malas). Batas ini mencegah
/// konversi ke `Vec<Vec<String>>` — bagian yang paling boros memori — tetapi
/// tidak bisa menghentikan transfernya; itu butuh API streaming di driver.
pub(crate) async fn run_query_limited(
    client: &mut Client<Ready>,
    query: &str,
    max_rows: usize,
) -> Result<(Vec<String>, Vec<Vec<String>>, bool), String> {
    let mut headers: Vec<String> = Vec::new();
    let mut data: Vec<Vec<String>> = Vec::new();
    let mut truncated = false;

    // query_multiple handles batches like "USE [db]; SELECT ..." — like the
    // previous tiberius stream, headers follow the latest result set with
    // columns while rows accumulate across result sets.
    let mut stream = client
        .query_multiple(query, &[])
        .await
        .map_err(|e| e.to_string())?;

    let mut rn_idx: Option<usize> = None;
    'sets: loop {
        if let Some(cols) = stream.columns()
            && !cols.is_empty()
        {
            headers = cols.iter().map(|c| c.name.clone()).collect();
            rn_idx = take_row_number_column(&mut headers);
        }
        while let Some(row) = stream.next_row().await.map_err(|e| e.to_string())? {
            if data.len() >= max_rows {
                truncated = true;
                break 'sets;
            }
            let mut values = row_values_to_strings(&row);
            if let Some(i) = rn_idx.filter(|i| *i < values.len()) {
                values.remove(i);
            }
            data.push(values);
        }
        if !stream.next_result().await.map_err(|e| e.to_string())? {
            break;
        }
    }
    Ok((headers, data, truncated))
}

/// Satu result set: header, baris, dan penanda terpotong.
pub(crate) type MssqlResultSet = (Vec<String>, Vec<Vec<String>>, bool);

/// Seperti [`run_query`], tetapi setiap result set yang punya kolom dikembalikan
/// terpisah (bukan digabung di bawah header terakhir). Batch tanpa result set
/// (DDL, `EXEC` tanpa `SELECT`) mengembalikan vektor kosong.
pub(crate) async fn run_query_multi(
    client: &mut Client<Ready>,
    query: &str,
) -> Result<Vec<(Vec<String>, Vec<Vec<String>>)>, String> {
    run_query_multi_limited(client, query, usize::MAX)
        .await
        .map(|sets| {
            sets.into_iter()
                .map(|(headers, rows, _truncated)| (headers, rows))
                .collect()
        })
}

/// Seperti [`run_query_multi`], dengan batas `max_rows` per result set. Baris
/// di luar batas dilewati tanpa dikonversi dan result set ditandai terpotong.
pub(crate) async fn run_query_multi_limited(
    client: &mut Client<Ready>,
    query: &str,
    max_rows: usize,
) -> Result<Vec<MssqlResultSet>, String> {
    let mut sets: Vec<MssqlResultSet> = Vec::new();
    let mut stream = client
        .query_multiple(query, &[])
        .await
        .map_err(|e| e.to_string())?;

    loop {
        let mut headers: Option<Vec<String>> = stream
            .columns()
            .filter(|cols| !cols.is_empty())
            .map(|cols| cols.iter().map(|c| c.name.clone()).collect());
        let rn_idx = headers.as_mut().and_then(take_row_number_column);
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(row) = stream.next_row().await.map_err(|e| e.to_string())? {
            if rows.len() >= max_rows {
                // Sisa baris result set ini dibuang; lanjut ke result set berikutnya.
                truncated = true;
                break;
            }
            let mut values = row_values_to_strings(&row);
            if let Some(i) = rn_idx.filter(|i| *i < values.len()) {
                values.remove(i);
            }
            rows.push(values);
        }
        if let Some(headers) = headers {
            sets.push((headers, rows, truncated));
        }
        if !stream.next_result().await.map_err(|e| e.to_string())? {
            break;
        }
    }
    Ok(sets)
}

/// Kegagalan eksekusi MsSQL yang dibatasi waktu.
#[derive(Debug)]
pub(crate) enum MssqlExecError {
    /// Batas waktu tercapai; koneksi sudah dibuang (tidak kembali ke pool).
    Timeout,
    /// Gagal mengambil koneksi dari pool (jaringan/tunnel/pool tertutup).
    Connection(String),
    /// Error dari server atau driver saat menjalankan statement.
    Query(String),
}

/// Batas waktu untuk mengirim paket ATTENTION saat membatalkan query.
const MSSQL_CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Jalankan batch pada satu koneksi pool dengan batas baris dan batas waktu
/// opsional. `split` menentukan apakah result set dipisah
/// ([`run_query_multi_limited`]) atau digabung ([`run_query_limited`]).
///
/// Saat timeout: paket ATTENTION dikirim supaya server berhenti mengerjakan
/// batch, lalu koneksi di-detach dan ditutup. Koneksi yang masih punya respons
/// tertunda tidak boleh kembali ke pool karena pemakai berikutnya akan membaca
/// sisa respons query lama.
/// Cancel handle batch yang sedang berjalan, per job id, supaya tombol
/// Cancel bisa mengirim ATTENTION ke server. Sebelumnya hanya timeout yang
/// membatalkan di server; cancel user cuma membuang future-nya.
static CANCEL_HANDLES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<u64, mssql_client::cancel::CancelHandle>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Hapus pendaftaran cancel handle saat batch selesai (juga saat task di-abort).
struct CancelRegistration(Option<u64>);

impl Drop for CancelRegistration {
    fn drop(&mut self) {
        if let Some(job_id) = self.0 {
            crate::connection::pool::lock_or_recover(&CANCEL_HANDLES).remove(&job_id);
        }
    }
}

/// Ambil cancel handle job yang masih berjalan (sekali pakai).
pub(crate) fn take_cancel_handle(job_id: u64) -> Option<mssql_client::cancel::CancelHandle> {
    crate::connection::pool::lock_or_recover(&CANCEL_HANDLES).remove(&job_id)
}

/// Bentuk keluaran [`execute_bounded`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum BoundedMode {
    /// Semua result set digabung menjadi satu.
    Merged,
    /// Tiap result set dikembalikan terpisah.
    Split,
    /// Batch tanpa result set: kembalikan jumlah baris yang terkena.
    Count,
}

async fn execute_bounded(
    pool: &mssql_driver_pool::Pool,
    query: &str,
    max_rows: usize,
    timeout: Option<std::time::Duration>,
    mode: BoundedMode,
    job_id: Option<u64>,
) -> Result<(Vec<MssqlResultSet>, Option<u64>), MssqlExecError> {
    let mut conn = pool
        .get()
        .await
        .map_err(|e| MssqlExecError::Connection(e.to_string()))?;
    let client = conn
        .client_mut()
        .ok_or_else(|| MssqlExecError::Connection("MsSQL pooled connection unavailable".into()))?;
    let cancel = client.cancel_handle();
    let _registration = CancelRegistration(job_id.inspect(|id| {
        crate::connection::pool::lock_or_recover(&CANCEL_HANDLES).insert(*id, cancel.clone());
    }));

    let run = async {
        match mode {
            BoundedMode::Split => run_query_multi_limited(client, query, max_rows)
                .await
                .map(|sets| (sets, None)),
            BoundedMode::Merged => run_query_limited(client, query, max_rows)
                .await
                .map(|set| (vec![set], None)),
            BoundedMode::Count => client
                .execute(query, &[])
                .await
                .map(|affected| (Vec::new(), Some(affected)))
                .map_err(|e| e.to_string()),
        }
    };
    let Some(limit) = timeout else {
        return run.await.map_err(MssqlExecError::Query);
    };
    match tokio::time::timeout(limit, run).await {
        Ok(result) => result.map_err(MssqlExecError::Query),
        Err(_) => {
            match tokio::time::timeout(MSSQL_CANCEL_TIMEOUT, cancel.cancel()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("[MSSQL] Failed to send query cancel: {}", e),
                Err(_) => log::warn!("[MSSQL] Query cancel timed out"),
            }
            // Detach lalu drop: koneksi ditutup, bukan dikembalikan ke pool.
            drop(conn.detach());
            Err(MssqlExecError::Timeout)
        }
    }
}

/// Jalankan batch lewat pool dengan batas baris dan batas waktu; semua result
/// set digabung seperti [`run_query`].
pub(crate) async fn execute_query_bounded(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
    max_rows: usize,
    timeout: Option<std::time::Duration>,
    job_id: Option<u64>,
) -> Result<MssqlResultSet, MssqlExecError> {
    let (mut sets, _) = execute_bounded(
        pool.as_ref(),
        query,
        max_rows,
        timeout,
        BoundedMode::Merged,
        job_id,
    )
    .await?;
    Ok(sets.pop().unwrap_or_default())
}

/// Jalankan batch yang hanya mengubah data (tanpa SELECT) dan kembalikan
/// jumlah baris yang terkena, dengan batas waktu dan cancel yang sama.
pub(crate) async fn execute_write_bounded(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
    timeout: Option<std::time::Duration>,
    job_id: Option<u64>,
) -> Result<u64, MssqlExecError> {
    let (_, affected) =
        execute_bounded(pool.as_ref(), query, 0, timeout, BoundedMode::Count, job_id).await?;
    Ok(affected.unwrap_or(0))
}

/// Jalankan batch lewat pool dengan batas baris per result set dan batas waktu;
/// setiap result set dikembalikan terpisah.
pub(crate) async fn execute_query_multi_bounded(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
    max_rows: usize,
    timeout: Option<std::time::Duration>,
    job_id: Option<u64>,
) -> Result<Vec<MssqlResultSet>, MssqlExecError> {
    execute_bounded(
        pool.as_ref(),
        query,
        max_rows,
        timeout,
        BoundedMode::Split,
        job_id,
    )
    .await
    .map(|(sets, _)| sets)
}

/// Jalankan batch lewat pool dan kembalikan semua result set secara terpisah.
pub(crate) async fn execute_query_multi(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
) -> Result<Vec<(Vec<String>, Vec<Vec<String>>)>, String> {
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let client = conn
        .client_mut()
        .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
    run_query_multi(client, query).await
}

// Helper: Remove TOP clauses from MsSQL SELECT for pagination compatibility
//
// Semua offset dihitung pada byte teks asli dengan perbandingan ASCII
// case-insensitive. Versi lama mencari posisi di salinan `to_lowercase()` lalu
// memotong teks asli dengan offset itu; lowercase Unicode bisa mengubah panjang
// byte sehingga slicing panic di tengah karakter (mis. `İ`, emoji).
pub(crate) fn sanitize_mssql_select_for_pagination(select_part: &str) -> String {
    let bytes = select_part.as_bytes();
    let starts_with_ci = |pos: usize, word: &[u8]| {
        bytes
            .get(pos..pos + word.len())
            .is_some_and(|slice| slice.eq_ignore_ascii_case(word))
    };
    let skip_whitespace = |mut pos: usize| {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        pos
    };

    // Pattern: SELECT [whitespace] TOP [whitespace] number/expression [whitespace]
    let Some(select_pos) = crate::connection::sql::find_ascii_ci(select_part, "select", 0) else {
        return select_part.to_string();
    };
    let select_end = select_pos + "select".len();

    let top_pos = skip_whitespace(select_end);
    if !starts_with_ci(top_pos, b"top") {
        return select_part.to_string();
    }
    let top_end = top_pos + "top".len();
    // `TOP` harus diikuti spasi atau `(`; kalau tidak, ini awal identifier
    // seperti `topic` dan tidak boleh dipotong.
    let after_top = skip_whitespace(top_end);
    let opens_paren = bytes.get(after_top) == Some(&b'(');
    if after_top == top_end && !opens_paren {
        return select_part.to_string();
    }

    let mut value_end = after_top;
    if opens_paren {
        // Ekspresi dalam kurung seperti TOP (100) atau TOP (@n)
        value_end += 1;
        while value_end < bytes.len() && bytes[value_end] != b')' {
            value_end += 1;
        }
        if value_end < bytes.len() {
            value_end += 1; // sertakan `)` penutup
        }
    } else {
        while value_end < bytes.len()
            && (bytes[value_end].is_ascii_digit() || bytes[value_end] == b'%')
        {
            value_end += 1;
        }
    }

    // Keyword PERCENT opsional setelah nilai TOP
    let percent_pos = skip_whitespace(value_end);
    if starts_with_ci(percent_pos, b"percent")
        && bytes
            .get(percent_pos + "percent".len())
            .is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
    {
        value_end = percent_pos + "percent".len();
    }
    value_end = skip_whitespace(value_end);

    // `select_end` dan `value_end` selalu jatuh tepat setelah byte ASCII (atau
    // di akhir teks), jadi keduanya batas karakter yang sah. `get` dipakai
    // supaya asumsi itu tidak pernah bisa menjadi panic.
    match (select_part.get(..select_end), select_part.get(value_end..)) {
        (Some(head), Some(tail)) => format!("{} {}", head, tail.trim_start()),
        _ => select_part.to_string(),
    }
}

// Helper: build MsSQL SELECT ensuring database context and proper quoting.
// db_name: selected database (can be empty -> fallback to object-provided or omit USE)
// raw_name: could be formats: table, [schema].[object], schema.object, [db].[schema].[object], db.schema.object
pub(crate) fn build_mssql_select_query(db_name: String, raw_name: String) -> String {
    // Normalize raw name: remove trailing semicolons/spaces
    let cleaned = raw_name.trim().trim_end_matches(';').to_string();

    // Split by '.' ignoring brackets segments
    // Strategy: remove outer brackets then split, re-wrap each part with []
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_bracket = false;
    for ch in cleaned.chars() {
        match ch {
            '[' => {
                in_bracket = true;
                current.push(ch);
            }
            ']' => {
                in_bracket = false;
                current.push(ch);
            }
            '.' if !in_bracket => {
                parts.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    // Remove surrounding brackets from each part and re-apply sanitized
    let mut plain_parts: Vec<String> = parts
        .into_iter()
        .map(|p| {
            let p2 = p.trim();
            let p2 = p2.strip_prefix('[').unwrap_or(p2);
            let p2 = p2.strip_suffix(']').unwrap_or(p2);
            p2.to_string()
        })
        .collect();

    // Decide final composition
    // Cases by length: 1=object, 2=schema.object, 3=db.schema.object
    // If db_name provided, override database part.
    let (database_part, schema_part, object_part) = match plain_parts.len() {
        3 => {
            let obj = plain_parts.pop().unwrap();
            let schema = plain_parts.pop().unwrap();
            let db = if !db_name.is_empty() {
                db_name.clone()
            } else {
                plain_parts.pop().unwrap()
            };
            (db, schema, obj)
        }
        2 => {
            let obj = plain_parts.pop().unwrap();
            let schema = plain_parts.pop().unwrap();
            let db = if !db_name.is_empty() {
                db_name.clone()
            } else {
                String::new()
            };
            (db, schema, obj)
        }
        1 => {
            let obj = plain_parts.pop().unwrap();
            let db = db_name.clone();
            (db, "dbo".to_string(), obj)
        }
        _ => (db_name.clone(), "dbo".to_string(), cleaned),
    };

    // Build fully qualified name with brackets
    let fq = if database_part.is_empty() {
        format!("[{}].[{}]", schema_part, object_part)
    } else {
        format!("[{}].[{}].[{}]", database_part, schema_part, object_part)
    };

    // If database part present, prepend USE to ensure context
    if database_part.is_empty() {
        format!("SELECT TOP 100 * FROM {};", fq)
    } else {
        format!("USE [{}];\nSELECT TOP 100 * FROM {};", database_part, fq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(pairs: &[(&str, &str)]) -> MssqlOptions {
        MssqlOptions::from_options(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn mssql_options_default_keeps_full_tls() {
        let o = opts(&[]);
        assert_eq!(o, MssqlOptions::default());
        let c = mssql_config("h", 1433, "u", "p", None, o);
        assert!(c.encrypt);
        assert!(!c.no_tls);
        assert_eq!(c.tds_version, mssql_client::TdsVersion::V7_4);
    }

    #[test]
    fn mssql_options_legacy_disables_tls_and_sets_tds() {
        let o = opts(&[(OPT_ENCRYPTION, "disabled"), (OPT_TDS_VERSION, "7.3b")]);
        assert_eq!(o.encryption, MssqlEncryption::Disabled);
        let c = mssql_config("h", 1433, "u", "p", Some("db"), o);
        assert!(c.no_tls);
        assert!(!c.encrypt);
        assert_eq!(c.tds_version, mssql_client::TdsVersion::V7_3B);

        let login = mssql_config(
            "h",
            1433,
            "u",
            "p",
            None,
            opts(&[(OPT_ENCRYPTION, "login_only")]),
        );
        assert!(!login.encrypt);
        assert!(!login.no_tls);
    }

    #[test]
    fn mssql_encryption_keys_round_trip() {
        for mode in MssqlEncryption::ALL {
            assert_eq!(MssqlEncryption::from_key(mode.as_key()), mode);
        }
        assert_eq!(
            MssqlEncryption::from_key("no_tls"),
            MssqlEncryption::Disabled
        );
        assert_eq!(MssqlEncryption::from_key("garbage"), MssqlEncryption::Tls);
        // TDS 8.0 dan nilai tak dikenal diabaikan (pakai bawaan driver).
        assert_eq!(opts(&[(OPT_TDS_VERSION, "8.0")]).tds_version, None);
        for (key, _) in TDS_VERSION_CHOICES.iter().filter(|(k, _)| !k.is_empty()) {
            assert!(parse_tds_version(key).is_some(), "{key}");
        }
    }

    #[test]
    fn connect_error_suggests_legacy_mode() {
        let msg = explain_connect_error(
            "TLS handshake failed: unexpected EOF",
            MssqlOptions::default(),
        );
        assert!(msg.contains("Disabled (legacy server)"), "{msg}");
        let legacy = MssqlOptions {
            encryption: MssqlEncryption::Disabled,
            tds_version: None,
        };
        let msg = explain_connect_error("server requires encryption", legacy);
        assert!(msg.contains("Force Encryption"), "{msg}");
        assert_eq!(
            explain_connect_error("Login failed for user 'sa'", legacy),
            "Login failed for user 'sa'"
        );
    }

    #[test]
    fn visible_databases_hide_system_unless_alone() {
        crate::schema_objects::set_show_system_objects(false);
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            visible_mssql_databases(names(&["tempdb", "Sales", "master", "AP"])),
            names(&["AP", "Sales"])
        );
        assert_eq!(
            visible_mssql_databases(names(&["msdb", "master"])),
            names(&["master", "msdb"])
        );
    }

    #[test]
    fn product_version_major() {
        assert_eq!(parse_product_major("10.50.6000.34"), Some(10));
        assert_eq!(parse_product_major(" 9.00.5000.00"), Some(9));
        assert_eq!(parse_product_major("16.0.1000.6"), Some(16));
        assert_eq!(parse_product_major(""), None);
    }

    #[test]
    fn legacy_server_flag_per_connection() {
        remember_server_major(-9001, 10);
        remember_server_major(-9002, 15);
        assert!(is_legacy_server(-9001));
        assert!(!is_legacy_server(-9002));
        assert!(!is_legacy_server(-9003));
    }

    #[test]
    fn legacy_first_page_uses_top() {
        assert_eq!(
            legacy_paginate("SELECT * FROM [dbo].[t] ORDER BY [id]", None, 0, 100),
            "SELECT TOP 100 * FROM [dbo].[t] ORDER BY [id]"
        );
        assert_eq!(
            legacy_paginate("SELECT DISTINCT a FROM t", Some(" ORDER BY [a]"), 0, 10),
            "SELECT DISTINCT TOP 10 a FROM t ORDER BY [a]"
        );
    }

    #[test]
    fn legacy_next_page_uses_row_number() {
        assert_eq!(
            legacy_paginate(
                "SELECT * FROM [t] WHERE x = 'order by' ORDER BY [id] DESC",
                None,
                100,
                50
            ),
            "SELECT * FROM (SELECT ROW_NUMBER() OVER (ORDER BY [id] DESC) AS [__tabular_rn], * \
             FROM [t] WHERE x = 'order by') AS [__tabular_p] WHERE [__tabular_rn] BETWEEN 101 AND 150 \
             ORDER BY [__tabular_rn]"
        );
        // Urutan tambahan dari primary key, dan ordinal yang tidak bisa dipakai di OVER.
        let sql = legacy_paginate("SELECT a, b FROM t", Some(" ORDER BY [a], [b]"), 10, 10);
        assert!(sql.contains("OVER (ORDER BY [a], [b])"), "{sql}");
        let sql = legacy_paginate("SELECT a FROM t ORDER BY 1", None, 10, 10);
        assert!(sql.contains("OVER (ORDER BY (SELECT NULL))"), "{sql}");
        assert!(!sql.contains("OFFSET"), "{sql}");
    }

    #[test]
    fn legacy_distinct_union_and_cte_are_wrapped() {
        let sql = legacy_paginate("SELECT DISTINCT a FROM t ORDER BY a", None, 20, 10);
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT ROW_NUMBER() OVER (ORDER BY a) AS [__tabular_rn], [__tabular_q].* \
             FROM (SELECT DISTINCT a FROM t) AS [__tabular_q]) AS [__tabular_p] \
             WHERE [__tabular_rn] BETWEEN 21 AND 30 ORDER BY [__tabular_rn]"
        );
        let sql = legacy_paginate("SELECT a FROM t UNION SELECT a FROM u", None, 0, 10);
        assert!(
            sql.contains("FROM (SELECT a FROM t UNION SELECT a FROM u) AS [__tabular_q]"),
            "{sql}"
        );
        let sql = legacy_paginate(
            "WITH c AS (SELECT a FROM t ORDER BY a OFFSET 0 ROWS) SELECT a FROM c ORDER BY a",
            None,
            5,
            5,
        );
        assert!(
            sql.starts_with("WITH c AS (SELECT a FROM t ORDER BY a OFFSET 0 ROWS) SELECT * FROM (SELECT ROW_NUMBER() OVER (ORDER BY a)"),
            "{sql}"
        );
    }

    #[test]
    fn row_number_column_is_removed_from_headers() {
        let mut headers = vec![ROW_NUMBER_COLUMN.to_string(), "a".to_string()];
        assert_eq!(take_row_number_column(&mut headers), Some(0));
        assert_eq!(headers, vec!["a".to_string()]);
        let mut plain = vec!["a".to_string()];
        assert_eq!(take_row_number_column(&mut plain), None);
    }

    #[test]
    fn sanitize_removes_top_clause_variants() {
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP 100 * FROM t"),
            "SELECT * FROM t"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("select  top(50)  a, b from t"),
            "select a, b from t"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP 10 PERCENT a FROM t"),
            "SELECT a FROM t"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP (@n) a FROM t"),
            "SELECT a FROM t"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT a FROM t"),
            "SELECT a FROM t"
        );
    }

    #[test]
    fn sanitize_leaves_identifiers_starting_with_top_alone() {
        // Dulu `topic` terpotong menjadi `ic`.
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT topic, top_score FROM t"),
            "SELECT topic, top_score FROM t"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP 5 percentile FROM t"),
            "SELECT percentile FROM t"
        );
    }

    #[test]
    fn sanitize_handles_non_ascii_without_panicking() {
        // Regresi: offset dari teks lowercase dipakai untuk memotong teks asli.
        assert_eq!(
            sanitize_mssql_select_for_pagination("İİİ SELECT TOP 10 'café' AS x FROM t"),
            "İİİ SELECT 'café' AS x FROM t"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP 3 N'—😀' AS emoji FROM [tabél]"),
            "SELECT N'—😀' AS emoji FROM [tabél]"
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP (😀"),
            "SELECT "
        );
        assert_eq!(
            sanitize_mssql_select_for_pagination("SELECT TOP İ"),
            "SELECT İ"
        );
    }

    #[test]
    fn sanitize_and_builder_never_panic_on_odd_input() {
        for input in crate::connection::sql::odd_sql_inputs() {
            let _ = sanitize_mssql_select_for_pagination(&input);
            let _ = build_mssql_select_query(input.clone(), input.clone());
            let _ = build_mssql_select_query(String::new(), input);
        }
    }
}
