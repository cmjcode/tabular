use std::sync::Arc;

use redis::aio::ConnectionManager;
use mongodb::Client as MongoClient;
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
    View,
    StoredProcedure,
    UserFunction,
    Trigger,
    Event,
    MySQLFolder,       // Folder untuk koneksi MySQL
    MsSQLFolder,      // Folder untuk koneksi MsSQL
    MongoDBFolder,    // Folder untuk koneksi MongoDB
    PostgreSQLFolder,  // Folder untuk koneksi PostgreSQL
    SQLiteFolder,      // Folder untuk koneksi SQLite
    RedisFolder,       // Folder untuk koneksi Redis
    CustomFolder,      // Folder custom yang bisa dinamai user
    QueryFolder,       // Folder untuk mengelompokkan query files
    // New table subfolders and items
    ColumnsFolder,
    IndexesFolder,
    PrimaryKeysFolder,
    Index,
}

#[derive(Debug, Clone)]
pub enum BackgroundTask {
    RefreshConnection { connection_id: i64 },
    CheckForUpdates,
}

#[derive(Debug, Clone)]
pub enum BackgroundResult {
    RefreshComplete { connection_id: i64, success: bool },
    UpdateCheckComplete { 
        result: Result<crate::self_update::UpdateInfo, String> 
    },
}


#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub enum DatabaseType {
    MySQL,
    PostgreSQL,
    SQLite,
    Redis,
    MsSQL,
    MongoDB,
}


// Enum untuk berbagai jenis database pool - sqlx pools are already thread-safe
#[derive(Clone)]
pub enum DatabasePool {
    MySQL(Arc<MySqlPool>),
    PostgreSQL(Arc<PgPool>),
    SQLite(Arc<SqlitePool>),
    Redis(Arc<ConnectionManager>),
    // For MsSQL we store a lightweight config (connections opened per query for now)
    MsSQL(Arc<crate::driver_mssql::MssqlConfigWrapper>),
    MongoDB(Arc<MongoClient>),
}
