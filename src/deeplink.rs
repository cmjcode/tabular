//! Deep link `tabular://` dan DSN mentah (M1/M2).
//!
//! Modul ini headless: hanya parsing, resolusi koneksi, dan kotak masuk URL
//! yang diisi oleh argv, `tabular open`, listener single-instance, atau Apple
//! Event macOS. GUI mengosongkan kotak masuk tiap frame lewat
//! [`drain_incoming`] lalu menerjemahkan [`DeepLink`] menjadi aksi.
//!
//! Format yang didukung (lihat `docs/URL_SCHEME.md`):
//! - `tabular://open?connection=<id|nama>[&database=..][&table=..]`
//! - `tabular://query?connection=<id|nama>&sql=..[&database=..][&run=1]`
//! - `tabular://import?url=<DSN>[&name=..]` (alias `connect`)
//! - DSN langsung: `postgres://`, `mysql://`, `sqlite:///path`, `redis://`, ...
//!
//! Prinsip keamanan: deep link tidak pernah mengeksekusi SQL tanpa konfirmasi
//! pengguna; `run=1` hanya meminta GUI menampilkan dialog konfirmasi.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use crate::models::enums::DatabaseType;
use crate::models::structs::ConnectionConfig;

/// Skema URL terdaftar di OS.
pub const SCHEME: &str = "tabular";

/// Batas panjang input agar URL raksasa dari luar tidak membebani UI.
const MAX_INPUT_LEN: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeepLinkError {
    #[error("empty link")]
    Empty,
    #[error("link is too long ({0} bytes)")]
    TooLong(usize),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("unsupported URL scheme `{0}`")]
    UnsupportedScheme(String),
    #[error("unknown action `{0}` (expected open, query, or import)")]
    UnknownAction(String),
    #[error("missing parameter `{0}`")]
    MissingParam(&'static str),
}

/// Aksi yang diminta sebuah deep link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// Buka koneksi tersimpan, opsional langsung ke database/tabel.
    Open {
        connection: String,
        database: Option<String>,
        table: Option<String>,
    },
    /// Buka tab query baru berisi SQL. `run` hanya memicu dialog konfirmasi.
    Query {
        connection: String,
        sql: String,
        database: Option<String>,
        run: bool,
    },
    /// Isi form koneksi baru dari DSN (atau buka koneksi yang cocok).
    Import {
        dsn: ParsedDsn,
        name: Option<String>,
    },
}

impl DeepLink {
    /// Bentuk kanonik `tabular://...` untuk disalin, Handoff, atau diteruskan.
    /// DSN pada `Import` ditulis tanpa password.
    pub fn to_url(&self) -> String {
        let (action, pairs): (&str, Vec<(&str, String)>) = match self {
            DeepLink::Open {
                connection,
                database,
                table,
            } => {
                let mut p = vec![("connection", connection.clone())];
                if let Some(db) = database {
                    p.push(("database", db.clone()));
                }
                if let Some(t) = table {
                    p.push(("table", t.clone()));
                }
                ("open", p)
            }
            DeepLink::Query {
                connection,
                sql,
                database,
                run,
            } => {
                let mut p = vec![("connection", connection.clone())];
                if let Some(db) = database {
                    p.push(("database", db.clone()));
                }
                p.push(("sql", sql.clone()));
                if *run {
                    p.push(("run", "1".to_string()));
                }
                ("query", p)
            }
            DeepLink::Import { dsn, name } => {
                let mut p = vec![("url", dsn.to_dsn_string(false))];
                if let Some(n) = name {
                    p.push(("name", n.clone()));
                }
                ("import", p)
            }
        };
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish()
            .replace('+', "%20");
        format!("{SCHEME}://{action}?{query}")
    }
}

/// Parse deep link `tabular://` atau DSN mentah.
pub fn parse(input: &str) -> Result<DeepLink, DeepLinkError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(DeepLinkError::Empty);
    }
    if input.len() > MAX_INPUT_LEN {
        return Err(DeepLinkError::TooLong(input.len()));
    }
    let scheme = input
        .split_once(':')
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();

    if scheme != SCHEME {
        let dsn = parse_dsn(input)?;
        return Ok(DeepLink::Import { dsn, name: None });
    }

    let url = url::Url::parse(input).map_err(|e| DeepLinkError::InvalidUrl(e.to_string()))?;
    // `tabular://open?..` → host "open"; `tabular:open?..` → path "open".
    let action = url
        .host_str()
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| url.path().trim_matches('/').to_string())
        .to_ascii_lowercase();

    let param = |key: &str| -> Option<String> {
        url.query_pairs()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.into_owned())
            .filter(|v| !v.trim().is_empty())
    };

    match action.as_str() {
        "open" => Ok(DeepLink::Open {
            connection: param("connection").ok_or(DeepLinkError::MissingParam("connection"))?,
            database: param("database"),
            table: param("table"),
        }),
        "query" => Ok(DeepLink::Query {
            connection: param("connection").ok_or(DeepLinkError::MissingParam("connection"))?,
            sql: param("sql").ok_or(DeepLinkError::MissingParam("sql"))?,
            database: param("database"),
            run: matches!(param("run").as_deref(), Some("1" | "true" | "yes")),
        }),
        "import" | "connect" => {
            let raw = param("url").ok_or(DeepLinkError::MissingParam("url"))?;
            Ok(DeepLink::Import {
                dsn: parse_dsn(&raw)?,
                name: param("name"),
            })
        }
        other => Err(DeepLinkError::UnknownAction(other.to_string())),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DSN
// ─────────────────────────────────────────────────────────────────────────────

/// Hasil parsing connection string. Password ikut disimpan karena dipakai
/// untuk mengisi form; `Debug` menyamarkannya supaya aman untuk log.
#[derive(Clone, PartialEq, Eq)]
pub struct ParsedDsn {
    pub db_type: DatabaseType,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub ssl: bool,
}

impl std::fmt::Debug for ParsedDsn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedDsn")
            .field("db_type", &self.db_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .field("database", &self.database)
            .field("ssl", &self.ssl)
            .finish()
    }
}

fn default_port(db_type: &DatabaseType) -> &'static str {
    match db_type {
        DatabaseType::MySQL => "3306",
        DatabaseType::PostgreSQL => "5432",
        DatabaseType::MsSQL => "1433",
        DatabaseType::Redis => "6379",
        DatabaseType::MongoDB => "27017",
        DatabaseType::SQLite | DatabaseType::ApiHttp | DatabaseType::Plugin(_) => "",
    }
}

fn scheme_to_type(scheme: &str) -> Option<(DatabaseType, bool)> {
    Some(match scheme {
        "postgres" | "postgresql" | "pg" => (DatabaseType::PostgreSQL, false),
        "mysql" | "mariadb" | "mysqlx" => (DatabaseType::MySQL, false),
        "sqlite" | "sqlite3" | "file" => (DatabaseType::SQLite, false),
        "sqlserver" | "mssql" => (DatabaseType::MsSQL, false),
        "redis" => (DatabaseType::Redis, false),
        "rediss" => (DatabaseType::Redis, true),
        "mongodb" | "mongodb+srv" => (DatabaseType::MongoDB, false),
        _ => return None,
    })
}

/// Percent-decode satu komponen URL (bukan form-encoding: `+` tetap `+`).
fn decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

/// Parse DSN umum menjadi [`ParsedDsn`].
pub fn parse_dsn(input: &str) -> Result<ParsedDsn, DeepLinkError> {
    let input = input.trim();
    let input = input.strip_prefix("jdbc:").unwrap_or(input);
    let (scheme_raw, rest) = match input.split_once("://") {
        Some(pair) => pair,
        None => input
            .split_once(':')
            .ok_or_else(|| DeepLinkError::InvalidUrl("missing scheme".into()))?,
    };
    let scheme = scheme_raw.to_ascii_lowercase();
    let (db_type, scheme_ssl) =
        scheme_to_type(&scheme).ok_or_else(|| DeepLinkError::UnsupportedScheme(scheme.clone()))?;

    if db_type == DatabaseType::SQLite {
        // sqlite:///abs/path.db → "/abs/path.db"; sqlite:relative.db → "relative.db"
        let path = decode(rest.split('?').next().unwrap_or_default());
        if path.trim().is_empty() {
            return Err(DeepLinkError::MissingParam("path"));
        }
        return Ok(ParsedDsn {
            db_type,
            host: String::new(),
            port: String::new(),
            username: String::new(),
            password: String::new(),
            database: path,
            ssl: false,
        });
    }

    // Gaya JDBC SQL Server: sqlserver://host:1433;databaseName=db;user=sa;password=x
    if db_type == DatabaseType::MsSQL && rest.contains(';') {
        return Ok(parse_jdbc_sqlserver(rest));
    }

    // Skema netral agar `url` mem-parse authority dengan cara yang sama untuk
    // semua engine (mis. `mongodb+srv` bukan skema spesial).
    let url = url::Url::parse(&format!("dsn://{rest}"))
        .map_err(|e| DeepLinkError::InvalidUrl(e.to_string()))?;
    let host = url
        .host_str()
        .map(|h| h.trim_matches(['[', ']']).to_string())
        .unwrap_or_default();
    if host.is_empty() {
        return Err(DeepLinkError::MissingParam("host"));
    }
    let port = url
        .port()
        .map(|p| p.to_string())
        .unwrap_or_else(|| default_port(&db_type).to_string());
    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.into_owned())
    };
    let ssl = scheme_ssl
        || matches!(
            query("sslmode").as_deref(),
            Some("require" | "verify-ca" | "verify-full")
        )
        || matches!(query("ssl").as_deref(), Some("true" | "1"))
        || matches!(query("tls").as_deref(), Some("true" | "1"));
    let mut database = decode(url.path().trim_start_matches('/'));
    if database.is_empty() {
        database = query("dbname")
            .or_else(|| query("database"))
            .unwrap_or_default();
    }
    Ok(ParsedDsn {
        db_type,
        host,
        port,
        username: decode(url.username()),
        password: url.password().map(decode).unwrap_or_default(),
        database,
        ssl,
    })
}

fn parse_jdbc_sqlserver(rest: &str) -> ParsedDsn {
    let mut parts = rest.split(';');
    let authority = parts.next().unwrap_or_default();
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_string(), p.to_string())
        }
        _ => (authority.to_string(), "1433".to_string()),
    };
    let mut dsn = ParsedDsn {
        db_type: DatabaseType::MsSQL,
        host,
        port,
        username: String::new(),
        password: String::new(),
        database: String::new(),
        ssl: false,
    };
    for kv in parts {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "databasename" | "database" => dsn.database = v.trim().to_string(),
            "user" | "username" | "user id" | "uid" => dsn.username = v.trim().to_string(),
            "password" | "pwd" => dsn.password = v.to_string(),
            "encrypt" => dsn.ssl = v.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    dsn
}

impl ParsedDsn {
    /// Tulis ulang sebagai DSN. `with_password = false` untuk tampilan/Handoff.
    pub fn to_dsn_string(&self, with_password: bool) -> String {
        let scheme = match self.db_type {
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "mysql",
            DatabaseType::SQLite => return format!("sqlite://{}", self.database),
            DatabaseType::MsSQL => "sqlserver",
            DatabaseType::Redis if self.ssl => "rediss",
            DatabaseType::Redis => "redis",
            DatabaseType::MongoDB => "mongodb",
            DatabaseType::ApiHttp => "http",
            DatabaseType::Plugin(ref id) => id.as_str(),
        };
        let enc = |s: &str| {
            url::form_urlencoded::byte_serialize(s.as_bytes())
                .collect::<String>()
                .replace('+', "%20")
        };
        let mut auth = String::new();
        if !self.username.is_empty() {
            auth.push_str(&enc(&self.username));
        }
        if with_password && !self.password.is_empty() {
            auth.push(':');
            auth.push_str(&enc(&self.password));
        }
        if !auth.is_empty() {
            auth.push('@');
        }
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let port = if self.port.is_empty() {
            String::new()
        } else {
            format!(":{}", self.port)
        };
        let db = if self.database.is_empty() {
            String::new()
        } else {
            format!("/{}", enc(&self.database))
        };
        format!("{scheme}://{auth}{host}{port}{db}")
    }

    /// Nama koneksi usulan, mis. `app @ localhost`.
    pub fn suggested_name(&self) -> String {
        match self.db_type {
            DatabaseType::SQLite => std::path::Path::new(&self.database)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| self.database.clone()),
            _ if self.database.is_empty() => self.host.clone(),
            _ => format!("{} @ {}", self.database, self.host),
        }
    }

    /// Konfigurasi koneksi baru (belum disimpan) untuk mengisi form.
    pub fn to_connection_config(&self, name: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            id: None,
            name: name
                .map(str::to_string)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| self.suggested_name()),
            host: self.host.clone(),
            port: self.port.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            database: self.database.clone(),
            connection_type: self.db_type.clone(),
            ssl_enabled: self.ssl,
            ..ConnectionConfig::default()
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Resolusi koneksi
// ─────────────────────────────────────────────────────────────────────────────

/// Cari koneksi berdasarkan id numerik atau nama (tidak peka huruf besar).
/// Nama bertingkat `Folder/Nama` juga diterima.
pub fn resolve_connection<'a>(
    connections: &'a [ConnectionConfig],
    key: &str,
) -> Option<&'a ConnectionConfig> {
    let key = key.trim();
    if let Ok(id) = key.parse::<i64>()
        && let Some(c) = connections.iter().find(|c| c.id == Some(id))
    {
        return Some(c);
    }
    connections
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(key))
        .or_else(|| {
            connections
                .iter()
                .find(|c| c.display_name().eq_ignore_ascii_case(key))
        })
}

fn is_loopback(host: &str) -> bool {
    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1" | ""
    )
}

/// Koneksi tersimpan yang menunjuk ke server/database yang sama dengan DSN.
/// Dipakai `tabular open <dsn>` (mis. dari ddev) agar tidak membuat duplikat.
pub fn find_matching_connection(connections: &[ConnectionConfig], dsn: &ParsedDsn) -> Option<i64> {
    connections
        .iter()
        .find(|c| {
            if c.connection_type != dsn.db_type {
                return false;
            }
            if dsn.db_type == DatabaseType::SQLite {
                return c.database == dsn.database;
            }
            let same_host = c.host.eq_ignore_ascii_case(&dsn.host)
                || (is_loopback(&c.host) && is_loopback(&dsn.host));
            same_host
                && c.port.trim() == dsn.port.trim()
                && c.database == dsn.database
                && (dsn.username.is_empty() || c.username == dsn.username)
        })
        .and_then(|c| c.id)
}

// ─────────────────────────────────────────────────────────────────────────────
// Kotak masuk URL (argv, IPC single-instance, Apple Event)
// ─────────────────────────────────────────────────────────────────────────────

type Waker = Box<dyn Fn() + Send + Sync>;

fn inbox() -> &'static Mutex<VecDeque<String>> {
    static INBOX: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    INBOX.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn waker() -> &'static Mutex<Option<Waker>> {
    static WAKER: OnceLock<Mutex<Option<Waker>>> = OnceLock::new();
    WAKER.get_or_init(|| Mutex::new(None))
}

/// Antre URL masuk. Aman dipanggil dari thread mana pun (listener IPC,
/// handler Apple Event). Antrean dibatasi agar pengirim nakal tidak bisa
/// menumpuk memori tanpa batas.
pub fn push_incoming(url: impl Into<String>) {
    let url = url.into();
    if url.trim().is_empty() || url.len() > MAX_INPUT_LEN {
        log::warn!("[DEEPLINK] ignoring empty or oversized link");
        return;
    }
    if let Ok(mut q) = inbox().lock() {
        if q.len() >= 32 {
            q.pop_front();
        }
        q.push_back(url);
    }
    if let Ok(w) = waker().lock()
        && let Some(w) = w.as_ref()
    {
        w();
    }
}

/// Ambil semua URL yang menunggu (dipanggil GUI tiap frame).
pub fn drain_incoming() -> Vec<String> {
    inbox()
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

/// Pasang fungsi pembangun UI (biasanya `ctx.request_repaint`).
pub fn set_waker(f: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut w) = waker().lock() {
        *w = Some(Box::new(f));
    }
}

fn known() -> &'static Mutex<Vec<(i64, String)>> {
    static KNOWN: OnceLock<Mutex<Vec<(i64, String)>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(Vec::new()))
}

/// Snapshot nama koneksi untuk AppleScript (`connection names`). Diisi GUI
/// setiap daftar koneksi berubah; tidak memuat data rahasia.
pub fn set_known_connections(list: Vec<(i64, String)>) {
    if let Ok(mut g) = known().lock() {
        *g = list;
    }
}

pub fn known_connections() -> Vec<(i64, String)> {
    known().lock().map(|g| g.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_link() {
        let link = parse("tabular://open?connection=Prod%20DB&database=app&table=users").unwrap();
        assert_eq!(
            link,
            DeepLink::Open {
                connection: "Prod DB".into(),
                database: Some("app".into()),
                table: Some("users".into()),
            }
        );
    }

    #[test]
    fn parses_query_link_and_run_flag() {
        let link = parse("tabular://query?connection=3&sql=SELECT%201%3B&run=1").unwrap();
        assert_eq!(
            link,
            DeepLink::Query {
                connection: "3".into(),
                sql: "SELECT 1;".into(),
                database: None,
                run: true,
            }
        );
    }

    #[test]
    fn query_without_sql_is_rejected() {
        assert_eq!(
            parse("tabular://query?connection=3"),
            Err(DeepLinkError::MissingParam("sql"))
        );
    }

    #[test]
    fn unknown_action_is_rejected() {
        assert!(matches!(
            parse("tabular://drop?connection=1"),
            Err(DeepLinkError::UnknownAction(_))
        ));
    }

    #[test]
    fn roundtrips_through_to_url() {
        for link in [
            DeepLink::Open {
                connection: "a b".into(),
                database: None,
                table: Some("t".into()),
            },
            DeepLink::Query {
                connection: "7".into(),
                sql: "select * from x where a = 'b&c' + 1".into(),
                database: Some("db".into()),
                run: false,
            },
        ] {
            assert_eq!(parse(&link.to_url()).unwrap(), link);
        }
    }

    #[test]
    fn raw_dsn_becomes_import() {
        let link =
            parse("postgres://alice:p%40ss@db.example.com:6543/app?sslmode=require").unwrap();
        let DeepLink::Import { dsn, name } = link else {
            panic!("expected import");
        };
        assert_eq!(name, None);
        assert_eq!(dsn.db_type, DatabaseType::PostgreSQL);
        assert_eq!(dsn.host, "db.example.com");
        assert_eq!(dsn.port, "6543");
        assert_eq!(dsn.username, "alice");
        assert_eq!(dsn.password, "p@ss");
        assert_eq!(dsn.database, "app");
        assert!(dsn.ssl);
    }

    #[test]
    fn import_link_wraps_dsn() {
        let link =
            parse("tabular://import?url=mysql%3A%2F%2Fdb%3Adb%40127.0.0.1%3A32768%2Fdb&name=ddev")
                .unwrap();
        let DeepLink::Import { dsn, name } = link else {
            panic!("expected import");
        };
        assert_eq!(name.as_deref(), Some("ddev"));
        assert_eq!(dsn.db_type, DatabaseType::MySQL);
        assert_eq!(dsn.port, "32768");
        assert_eq!(dsn.password, "db");
    }

    #[test]
    fn dsn_defaults_and_variants() {
        let d = parse_dsn("mariadb://root@localhost").unwrap();
        assert_eq!((d.db_type, d.port.as_str()), (DatabaseType::MySQL, "3306"));

        let d = parse_dsn("sqlite:///Users/me/data/app.db").unwrap();
        assert_eq!(d.db_type, DatabaseType::SQLite);
        assert_eq!(d.database, "/Users/me/data/app.db");
        assert_eq!(d.suggested_name(), "app.db");

        let d = parse_dsn("rediss://:secret@cache.local:6380/2").unwrap();
        assert!(d.ssl);
        assert_eq!((d.password.as_str(), d.database.as_str()), ("secret", "2"));

        let d = parse_dsn("jdbc:sqlserver://sql.local:1444;databaseName=erp;user=sa;password=a")
            .unwrap();
        assert_eq!(d.db_type, DatabaseType::MsSQL);
        assert_eq!((d.host.as_str(), d.port.as_str()), ("sql.local", "1444"));
        assert_eq!((d.database.as_str(), d.username.as_str()), ("erp", "sa"));

        let d = parse_dsn("postgresql://[::1]:5433/x").unwrap();
        assert_eq!(d.host, "::1");

        assert!(matches!(
            parse_dsn("oracle://x"),
            Err(DeepLinkError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn debug_output_masks_password() {
        let d = parse_dsn("postgres://u:topsecret@h/db").unwrap();
        assert!(!format!("{d:?}").contains("topsecret"));
        assert!(!d.to_dsn_string(false).contains("topsecret"));
        assert!(d.to_dsn_string(true).contains("topsecret"));
    }

    fn conn(id: i64, name: &str, folder: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            id: Some(id),
            name: name.into(),
            folder: folder.map(str::to_string),
            host: "127.0.0.1".into(),
            port: "3306".into(),
            database: "db".into(),
            username: "db".into(),
            ..ConnectionConfig::default()
        }
    }

    #[test]
    fn resolves_by_id_name_and_folder_path() {
        let list = vec![conn(1, "Local", None), conn(2, "Prod", Some("Work"))];
        assert_eq!(resolve_connection(&list, "2").unwrap().name, "Prod");
        assert_eq!(resolve_connection(&list, "local").unwrap().id, Some(1));
        assert_eq!(resolve_connection(&list, "work/prod").unwrap().id, Some(2));
        assert!(resolve_connection(&list, "missing").is_none());
    }

    #[test]
    fn matching_connection_treats_loopback_hosts_as_equal() {
        let list = vec![conn(9, "ddev", None)];
        let dsn = parse_dsn("mysql://db:db@localhost:3306/db").unwrap();
        assert_eq!(find_matching_connection(&list, &dsn), Some(9));
        let other = parse_dsn("mysql://db:db@localhost:3307/db").unwrap();
        assert_eq!(find_matching_connection(&list, &other), None);
    }

    #[test]
    fn inbox_is_bounded_and_drains() {
        drain_incoming();
        for i in 0..40 {
            push_incoming(format!("tabular://open?connection={i}"));
        }
        let got = drain_incoming();
        assert_eq!(got.len(), 32);
        assert!(got[0].ends_with("=8"));
        assert!(drain_incoming().is_empty());
    }
}
