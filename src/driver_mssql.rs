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

/// Build a Config for a direct (non-pooled) MsSQL connection.
/// Mirrors the app-wide defaults: SQL auth + trusted server certificate.
pub(crate) fn mssql_config(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
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
    config
}

/// Open a one-off MsSQL connection (no pool).
pub(crate) async fn connect_mssql(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
) -> Result<Client<Ready>, String> {
    Client::connect(mssql_config(host, port, username, password, database))
        .await
        .map_err(|e| e.to_string())
}

/// Convert a dynamic SqlValue into the display string used by the data grid.
pub(crate) fn sql_value_to_string(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => "NULL".to_string(),
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
pub(crate) fn row_values_to_strings(row: &mssql_client::Row) -> Vec<String> {
    (0..row.len())
        .map(|i| match row.get_raw(i) {
            Some(v) => sql_value_to_string(&v),
            None => "NULL".to_string(),
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

pub(crate) async fn fetch_mssql_data(
    _connection_id: i64,
    _pool: std::sync::Arc<mssql_driver_pool::Pool>,
    _cache_pool: &sqlx::SqlitePool,
) -> bool {
    // TODO: implement metadata caching
    true
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
    _database_name: &str,
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
        list_mssql_tables(&pool, table_type).await
    })
}

/// Daftar tabel / view MsSQL (`[schema].[name]`) lewat pool yang sudah ada.
/// Aman dipanggil dari task async (tanpa runtime baru).
pub(crate) async fn list_mssql_tables(
    pool: &mssql_driver_pool::Pool,
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
    let query = match table_type {
        // Include schema for tables (some objects not in dbo)
        "table" => {
            "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE='BASE TABLE' ORDER BY TABLE_NAME"
        }
        // Include schema for views so we can build fully-qualified names
        "view" => {
            "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS ORDER BY TABLE_NAME"
        }
        _ => {
            log::debug!("Unsupported MsSQL table_type: {}", table_type);
            return None;
        }
    };

    let stream =
        match tokio::time::timeout(std::time::Duration::from_secs(10), client.query(query, &[]))
            .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                log::debug!("MsSQL list query error: {}", e);
                return None;
            }
            Err(_) => {
                log::debug!("MsSQL list query timeout");
                return None;
            }
        };

    let mut items = Vec::new();
    for row in stream.collect_all().await.ok()? {
        let schema = row.get_string(0);
        let name = row.get_string(1);
        if let (Some(s), Some(n)) = (schema, name) {
            items.push(format!("[{}].[{}]", s, n));
        }
    }
    Some(items)
}

/// Fetch MsSQL objects for a specific database by type: procedure | function | trigger
pub(crate) fn fetch_objects_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    _database_name: &str,
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

        let mut conn = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                log::debug!("MsSQL pool get error: {}", e);
                return None;
            }
        };
        let client = conn.client_mut()?;

        let query = match object_type {
            // Stored procedures
            "procedure" => {
                // sys.procedures excludes system procedures when filtered by is_ms_shipped = 0
                "SELECT s.name AS schema_name, p.name AS object_name \
                 FROM sys.procedures p \
                 JOIN sys.schemas s ON p.schema_id = s.schema_id \
                 WHERE ISNULL(p.is_ms_shipped,0) = 0 \
                 ORDER BY p.name"
                    .to_string()
            }
            // Functions: scalar, inline table-valued, multi-statement table-valued, and CLR variants
            "function" => "SELECT s.name AS schema_name, o.name AS object_name \
                 FROM sys.objects o \
                 JOIN sys.schemas s ON o.schema_id = s.schema_id \
                 WHERE o.type IN ('FN','IF','TF','AF','FS','FT') \
                 ORDER BY o.name"
                .to_string(),
            // Triggers: list DML triggers attached to user tables
            "trigger" => {
                "SELECT ss.name AS schema_name, t.name AS table_name, tr.name AS trigger_name \
                 FROM sys.triggers tr \
                 JOIN sys.tables t ON tr.parent_id = t.object_id \
                 JOIN sys.schemas ss ON t.schema_id = ss.schema_id \
                 WHERE t.type = 'U' \
                 ORDER BY tr.name"
                    .to_string()
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
                log::debug!("MsSQL object list query error: {}", e);
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
                    let schema = row.get_string(0);
                    let name = row.get_string(1);
                    if let (Some(s), Some(n)) = (schema, name) {
                        items.push(format!("[{}].[{}]", s, n));
                    }
                }
                "trigger" => {
                    let schema = row.get_string(0);
                    let table = row.get_string(1);
                    let trig = row.get_string(2);
                    if let (Some(s), Some(t), Some(tr)) = (schema, table, trig) {
                        items.push(format!("[{}].[{}].[{}]", s, t, tr));
                    }
                }
                _ => {}
            }
        }
        Some(items)
    })
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

    'sets: loop {
        if let Some(cols) = stream.columns()
            && !cols.is_empty()
        {
            headers = cols.iter().map(|c| c.name.clone()).collect();
        }
        while let Some(row) = stream.next_row().await.map_err(|e| e.to_string())? {
            if data.len() >= max_rows {
                truncated = true;
                break 'sets;
            }
            data.push(row_values_to_strings(&row));
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
        let headers: Option<Vec<String>> = stream
            .columns()
            .filter(|cols| !cols.is_empty())
            .map(|cols| cols.iter().map(|c| c.name.clone()).collect());
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(row) = stream.next_row().await.map_err(|e| e.to_string())? {
            if rows.len() >= max_rows {
                // Sisa baris result set ini dibuang; lanjut ke result set berikutnya.
                truncated = true;
                break;
            }
            rows.push(row_values_to_strings(&row));
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
async fn execute_bounded(
    pool: &mssql_driver_pool::Pool,
    query: &str,
    max_rows: usize,
    timeout: Option<std::time::Duration>,
    split: bool,
) -> Result<Vec<MssqlResultSet>, MssqlExecError> {
    let mut conn = pool
        .get()
        .await
        .map_err(|e| MssqlExecError::Connection(e.to_string()))?;
    let client = conn
        .client_mut()
        .ok_or_else(|| MssqlExecError::Connection("MsSQL pooled connection unavailable".into()))?;
    let cancel = client.cancel_handle();

    let run = async {
        if split {
            run_query_multi_limited(client, query, max_rows).await
        } else {
            run_query_limited(client, query, max_rows)
                .await
                .map(|set| vec![set])
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
) -> Result<MssqlResultSet, MssqlExecError> {
    let mut sets = execute_bounded(pool.as_ref(), query, max_rows, timeout, false).await?;
    Ok(sets.pop().unwrap_or_default())
}

/// Jalankan batch lewat pool dengan batas baris per result set dan batas waktu;
/// setiap result set dikembalikan terpisah.
pub(crate) async fn execute_query_multi_bounded(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
    max_rows: usize,
    timeout: Option<std::time::Duration>,
) -> Result<Vec<MssqlResultSet>, MssqlExecError> {
    execute_bounded(pool.as_ref(), query, max_rows, timeout, true).await
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
