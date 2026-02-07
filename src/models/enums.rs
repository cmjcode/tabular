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

// Special DBA quick view context (used to apply post-processing without embedding markers in SQL)
#[derive(Clone, PartialEq, Debug)]
pub enum DBASpecialMode {
    ReplicationStatus,
    MasterStatus,
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
}

#[derive(Debug, Clone)]
pub enum BackgroundResult {
    RefreshComplete {
        connection_id: i64,
        success: bool,
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
    SqlitePathPicked { path: String },
    // Result from background database fetch
    DatabasesFetched {
        connection_id: i64,
        databases: Vec<String>,
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
}

impl DatabaseType {
    /// Returns the icon emoji for this database type
    pub fn icon(&self) -> &'static str {
        match self {
            DatabaseType::MySQL => "🐬",
            DatabaseType::PostgreSQL => "🐘",
            DatabaseType::SQLite => "📄",
            DatabaseType::Redis => "🔴",
            DatabaseType::MsSQL => "💠",
            DatabaseType::MongoDB => "🍃",
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
    MsSQL(deadpool_tiberius::Pool),
    MongoDB(Arc<MongoClient>),
}
