use std::sync::Arc;

use mongodb::Client as MongoClient;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, PgPool, SqlitePool};

#[derive(Clone, PartialEq, Debug)]
pub enum NodeType {
    #[allow(dead_code)]
    Database,
    Table,
    Column,
    Query,
    QueryHistItem,
    // Folder/grouping node for query history by execution date (YYYY-MM-DD)
    HistoryDateFolder,
    Connection,
    DatabasesFolder,
    TablesFolder,
    ViewsFolder,
    StoredProceduresFolder,
    UserFunctionsFolder,
    TriggersFolder,
    EventsFolder,
    DBAViewsFolder,
    UsersFolder,
    PrivilegesFolder,
    ProcessesFolder,
    StatusFolder,
    BlockedQueriesFolder,
    MetricsUserActiveFolder,
    // MySQL specific DBA quick views
    ReplicationStatusFolder,
    MasterStatusFolder,
    View,
    StoredProcedure,
    UserFunction,
    Trigger,
    Event,
    // Objek skema PostgreSQL: materialized view dan tipe buatan user.
    MaterializedViewsFolder,
    MaterializedView,
    TypesFolder,
    UserType,
    MySQLFolder,      // Folder untuk koneksi MySQL
    MsSQLFolder,      // Folder untuk koneksi MsSQL
    MongoDBFolder,    // Folder untuk koneksi MongoDB
    PostgreSQLFolder, // Folder untuk koneksi PostgreSQL
    SQLiteFolder,     // Folder untuk koneksi SQLite
    RedisFolder,      // Folder untuk koneksi Redis
    CustomFolder,     // Folder custom yang bisa dinamai user
    CustomView,       // User defined DBA view
    QueryFolder,      // Folder untuk mengelompokkan query files
    // New table subfolders and items
    ColumnsFolder,
    IndexesFolder,
    PrimaryKeysFolder,
    PartitionsFolder,
    Index,
    DiagramsFolder,
    Diagram,
}

impl NodeType {
    pub fn is_folder(&self) -> bool {
        matches!(
            self,
            NodeType::HistoryDateFolder
                | NodeType::DatabasesFolder
                | NodeType::Database
                | NodeType::TablesFolder
                | NodeType::ViewsFolder
                | NodeType::StoredProceduresFolder
                | NodeType::UserFunctionsFolder
                | NodeType::TriggersFolder
                | NodeType::EventsFolder
                | NodeType::MaterializedViewsFolder
                | NodeType::TypesFolder
                | NodeType::DBAViewsFolder
                | NodeType::UsersFolder
                | NodeType::PrivilegesFolder
                | NodeType::ProcessesFolder
                | NodeType::StatusFolder
                | NodeType::BlockedQueriesFolder
                | NodeType::MetricsUserActiveFolder
                | NodeType::ReplicationStatusFolder
                | NodeType::MasterStatusFolder
                | NodeType::MySQLFolder
                | NodeType::MsSQLFolder
                | NodeType::MongoDBFolder
                | NodeType::PostgreSQLFolder
                | NodeType::SQLiteFolder
                | NodeType::RedisFolder
                | NodeType::CustomFolder
                | NodeType::QueryFolder
                | NodeType::ColumnsFolder
                | NodeType::IndexesFolder
                | NodeType::PrimaryKeysFolder
                | NodeType::PartitionsFolder
                | NodeType::DiagramsFolder
        )
    }
}

// Special DBA quick view context (used to apply post-processing without embedding markers in SQL)
#[derive(Clone, PartialEq, Debug)]
pub enum DBASpecialMode {
    ReplicationStatus,
    MasterStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStateFilter {
    All,
    ActiveOnly,
    BlockedOnly,
    IdleOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbaMonitorTab {
    Processlist,
    LockTree,
    /// Metrik server real-time + slow statement (checklist I3).
    Dashboard,
}

#[derive(Debug, Clone)]
pub enum BackgroundTask {
    RefreshConnection {
        connection_id: i64,
    },
    CheckForUpdates,
    StartPrefetch {
        connection_id: i64,
        show_progress: bool, // Whether to show progress in UI
    },
    // Ask UI thread to open SQLite file/folder picker for new connection
    PickSqlitePath,
    // Fetch databases in background
    FetchDatabases {
        connection_id: i64,
    },
    // Fetch Redis keys for a specific database in background
    FetchRedisKeys {
        connection_id: i64,
        database_name: String,
    },
    FetchRedisBrowserState {
        connection_id: i64,
        database_name: Option<String>,
    },
    SearchRedisBrowserKeys {
        connection_id: i64,
        database_name: String,
        search_text: String,
    },
    TestConnection {
        connection: Box<crate::models::structs::ConnectionConfig>,
    },
    EnsureConnectionPool {
        connection_id: i64,
    },
    FetchTableStructure {
        connection_id: i64,
        database_name: String,
        table_name: String,
    },
}

// Infrequent mpsc channel message (one per background task completion), so the
// size disparity between variants is not a hot-path concern.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum BackgroundResult {
    TableStructureFetched {
        connection_id: i64,
        database_name: String,
        table_name: String,
        columns: Option<Vec<(String, String)>>,
        columns_detail: Option<Vec<crate::models::structs::ColumnStructInfo>>,
        indexes: Option<Vec<crate::models::structs::IndexStructInfo>>,
        partitions: Option<Vec<crate::models::structs::PartitionStructInfo>>,
    },
    RefreshComplete {
        connection_id: i64,
        success: bool,
        /// Databases fetched during this refresh (populated only when success=true).
        /// Passed inline to avoid a second SQLite round-trip in the UI thread.
        databases: Vec<String>,
    },
    UpdateCheckComplete {
        result: Result<crate::self_update::UpdateInfo, String>,
    },
    PrefetchProgress {
        connection_id: i64,
        completed: usize,
        total: usize,
    },
    PrefetchComplete {
        connection_id: i64,
    },
    // Result from SQLite folder/file picker for new connection dialog
    SqlitePathPicked {
        path: String,
    },
    // Result from background database fetch
    DatabasesFetched {
        connection_id: i64,
        databases: Vec<String>,
    },
    // Result when background connection or database fetch fails
    ConnectionFailed {
        connection_id: i64,
        error_message: String,
    },
    // Result when async TestConnection task completes
    TestConnectionComplete {
        success: bool,
        message: String,
    },
    // Result from background Redis key fetch
    RedisKeysFetched {
        connection_id: i64,
        database_name: String,
        keys: Vec<(String, String)>, // (key_name, key_type)
    },
    RedisBrowserStateFetched {
        connection_id: i64,
        state: crate::models::structs::RedisBrowserState,
    },
    RedisBrowserSearchFetched {
        connection_id: i64,
        database_name: String,
        search_text: String,
        keys: Vec<(String, String)>,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub enum DatabaseType {
    MySQL,
    PostgreSQL,
    SQLite,
    Redis,
    MsSQL,
    MongoDB,
    ApiHttp,
    /// Engine dari plugin driver (ADR 0002), dengan id engine plugin.
    Plugin(String),
}

impl DatabaseType {
    /// Returns the icon emoji for this database type
    pub fn icon(&self) -> &'static str {
        match self {
            DatabaseType::MySQL => "🐬",
            DatabaseType::PostgreSQL => "🐘",
            DatabaseType::SQLite => "📄",
            DatabaseType::Redis => "🔴",
            DatabaseType::MsSQL => "🛢️",
            DatabaseType::MongoDB => "🍃",
            DatabaseType::ApiHttp => "🌐",
            DatabaseType::Plugin(_) => "🧩",
        }
    }

    /// Nilai yang disimpan di kolom `connections.connection_type`. Untuk varian
    /// builtin sama persis dengan output `Debug` supaya baris lama tetap
    /// terbaca; engine plugin disimpan sebagai `plugin:<id>`.
    pub fn as_db_str(&self) -> std::borrow::Cow<'static, str> {
        use std::borrow::Cow;
        match self {
            DatabaseType::MySQL => Cow::Borrowed("MySQL"),
            DatabaseType::PostgreSQL => Cow::Borrowed("PostgreSQL"),
            DatabaseType::SQLite => Cow::Borrowed("SQLite"),
            DatabaseType::Redis => Cow::Borrowed("Redis"),
            DatabaseType::MsSQL => Cow::Borrowed("MsSQL"),
            DatabaseType::MongoDB => Cow::Borrowed("MongoDB"),
            DatabaseType::ApiHttp => Cow::Borrowed("ApiHttp"),
            DatabaseType::Plugin(id) => Cow::Owned(format!("plugin:{id}")),
        }
    }

    /// Id yang dicadangkan untuk engine builtin; plugin tidak boleh memakainya.
    pub fn is_builtin_id(id: &str) -> bool {
        matches!(
            id.to_ascii_lowercase().as_str(),
            "mysql"
                | "mariadb"
                | "postgresql"
                | "postgres"
                | "sqlite"
                | "redis"
                | "mssql"
                | "sqlserver"
                | "mongodb"
                | "mongo"
                | "apihttp"
                | "http"
        )
    }

    /// Id engine plugin, atau `None` untuk engine builtin.
    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            DatabaseType::Plugin(id) => Some(id),
            _ => None,
        }
    }

    /// Nama engine untuk UI. Engine plugin memakai nama dari registry, atau
    /// id-nya bila driver belum terpasang.
    pub fn display_name(&self) -> String {
        match self {
            DatabaseType::Plugin(id) => crate::driver_api::registry::descriptor(id)
                .map(|d| d.name)
                .unwrap_or_else(|| id.clone()),
            other => other.badge_label().to_string(),
        }
    }

    /// Kebalikan dari [`Self::as_db_str`]. Nilai yang tidak dikenal (misalnya
    /// engine plugin yang belum terpasang, atau data dari versi lebih baru)
    /// menghasilkan `None`, bukan diam-diam dianggap SQLite.
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "MySQL" => Some(DatabaseType::MySQL),
            "PostgreSQL" => Some(DatabaseType::PostgreSQL),
            "SQLite" => Some(DatabaseType::SQLite),
            "Redis" => Some(DatabaseType::Redis),
            "MsSQL" => Some(DatabaseType::MsSQL),
            "MongoDB" => Some(DatabaseType::MongoDB),
            "ApiHttp" => Some(DatabaseType::ApiHttp),
            other => other
                .strip_prefix("plugin:")
                .filter(|id| crate::driver_api::EngineDescriptor::is_valid_id(id))
                .map(|id| DatabaseType::Plugin(id.to_string())),
        }
    }

    /// Returns a short uppercase label for the colored badge shown in the sidebar
    pub fn badge_label(&self) -> &'static str {
        match self {
            DatabaseType::MySQL => "MySQL",
            DatabaseType::PostgreSQL => "PostgreSQL",
            DatabaseType::SQLite => "SQLite",
            DatabaseType::Redis => "Redis",
            DatabaseType::MsSQL => "MsSQL",
            DatabaseType::MongoDB => "MongoDB",
            DatabaseType::ApiHttp => "API",
            DatabaseType::Plugin(_) => "Plugin",
        }
    }

    /// Returns whether this database type supports PostgreSQL/MsSQL style schemas distinct from databases
    pub fn supports_schemas(&self) -> bool {
        matches!(self, DatabaseType::PostgreSQL | DatabaseType::MsSQL)
    }

    /// Returns the brand RGB color for the sidebar badge (r, g, b)
    pub fn badge_color(&self) -> (u8, u8, u8) {
        match self {
            DatabaseType::MySQL => (0, 117, 143),       // MySQL teal-blue
            DatabaseType::PostgreSQL => (51, 103, 145), // PostgreSQL steel-blue
            DatabaseType::SQLite => (68, 130, 195),     // SQLite blue
            DatabaseType::Redis => (210, 56, 42),       // Redis red
            DatabaseType::MsSQL => (0, 164, 239),       // MS Azure blue
            DatabaseType::MongoDB => (0, 168, 80),      // MongoDB green
            DatabaseType::ApiHttp => (139, 79, 191),    // HTTP purple
            DatabaseType::Plugin(_) => (120, 120, 130), // plugin neutral gray
        }
    }
    /// Returns a filesystem-safe key for loading PNG icons, e.g. "mysql" → assets/db_icons/mysql.png
    pub fn icon_key(&self) -> &'static str {
        match self {
            DatabaseType::MySQL => "mysql",
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::SQLite => "sqlite",
            DatabaseType::Redis => "redis",
            DatabaseType::MsSQL => "mssql",
            DatabaseType::MongoDB => "mongodb",
            DatabaseType::ApiHttp => "apihttp",
            DatabaseType::Plugin(_) => "plugin",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutocompleteKind {
    Table,
    Column,
    Syntax,
    Snippet,
    Parameter,
    Function,
    /// Kondisi join siap pakai (`o.user_id = u.id`).
    Join,
    /// Alias tabel / alias SELECT.
    Alias,
    Operator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum KeywordCasing {
    #[default]
    Upper,
    Lower,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SshAuthMethod {
    #[default]
    Key,
    Password,
}

impl SshAuthMethod {
    pub fn as_db_value(&self) -> &'static str {
        match self {
            SshAuthMethod::Key => "key",
            SshAuthMethod::Password => "password",
        }
    }

    pub fn from_db_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "password" => SshAuthMethod::Password,
            _ => SshAuthMethod::Key,
        }
    }
}

// Enum untuk berbagai jenis database pool - sqlx pools are already thread-safe
#[derive(Clone)]
pub enum DatabasePool {
    MySQL(Arc<MySqlPool>),
    PostgreSQL(Arc<PgPool>),
    SQLite(Arc<SqlitePool>),
    Redis(Arc<ConnectionManager>),
    MsSQL(Arc<mssql_driver_pool::Pool>),
    MongoDB(Arc<MongoClient>),
    Plugin(Arc<crate::driver_api::PluginPool>),
}

impl DatabasePool {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DatabasePool::MySQL(_) => DatabaseType::MySQL,
            DatabasePool::PostgreSQL(_) => DatabaseType::PostgreSQL,
            DatabasePool::SQLite(_) => DatabaseType::SQLite,
            DatabasePool::Redis(_) => DatabaseType::Redis,
            DatabasePool::MsSQL(_) => DatabaseType::MsSQL,
            DatabasePool::MongoDB(_) => DatabaseType::MongoDB,
            DatabasePool::Plugin(p) => DatabaseType::Plugin(p.engine_id.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DatabaseType;

    const ALL_TYPES: [DatabaseType; 7] = [
        DatabaseType::MySQL,
        DatabaseType::PostgreSQL,
        DatabaseType::SQLite,
        DatabaseType::Redis,
        DatabaseType::MsSQL,
        DatabaseType::MongoDB,
        DatabaseType::ApiHttp,
    ];

    #[test]
    fn db_str_round_trips_for_every_builtin_type() {
        for ty in ALL_TYPES {
            assert_eq!(DatabaseType::from_db_str(&ty.as_db_str()), Some(ty.clone()));
        }
    }

    #[test]
    fn db_str_matches_legacy_debug_format() {
        // Penulis lama menyimpan `format!("{:?}", connection_type)`.
        for ty in ALL_TYPES {
            assert_eq!(ty.as_db_str(), format!("{ty:?}"));
        }
    }

    #[test]
    fn unknown_type_is_not_coerced_to_sqlite() {
        assert_eq!(DatabaseType::from_db_str("ClickHouse"), None);
        assert_eq!(DatabaseType::from_db_str("plugin:"), None);
        assert_eq!(DatabaseType::from_db_str("plugin:../x"), None);
        assert_eq!(DatabaseType::from_db_str(""), None);
    }

    #[test]
    fn plugin_type_round_trips() {
        let ty = DatabaseType::Plugin("clickhouse".into());
        assert_eq!(ty.as_db_str(), "plugin:clickhouse");
        assert_eq!(DatabaseType::from_db_str("plugin:clickhouse"), Some(ty.clone()));
        assert_eq!(ty.plugin_id(), Some("clickhouse"));
        // Driver tidak terpasang: nama jatuh ke id.
        assert_eq!(ty.display_name(), "clickhouse");
    }

    #[test]
    fn builtin_ids_are_reserved() {
        assert!(DatabaseType::is_builtin_id("PostgreSQL"));
        assert!(DatabaseType::is_builtin_id("sqlite"));
        assert!(!DatabaseType::is_builtin_id("clickhouse"));
    }
}
