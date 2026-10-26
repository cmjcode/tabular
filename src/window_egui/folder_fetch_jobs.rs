//! Isi folder sidebar (tabel, view, procedure, koleksi, ...) dimuat di latar.
//!
//! Sebelumnya cache yang kosong membuat loader memanggil database secara
//! blocking di thread UI tanpa batas waktu: koneksi lambat atau VPN yang putus
//! membekukan seluruh jendela. Sekarang loader meminta [`Tabular::folder_fetch`],
//! menampilkan "Loading…", dan hasilnya ditulis ke cache metadata. Setelah itu
//! folder dimuat ulang lewat jalur cache yang sama seperti biasa.
//!
//! Hasil kosong dan kegagalan diingat sebentar supaya loader tidak meminta
//! ulang setiap frame (cache kosong = cache miss).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::Tabular;
use crate::models::enums::{DatabasePool, NodeType};
use crate::models::structs::TreeNode;

/// Batas waktu satu pengambilan isi folder (termasuk menunggu pool).
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Folder yang ternyata kosong tidak diminta ulang selama ini.
const EMPTY_TTL: Duration = Duration::from_secs(30);
/// Kegagalan ditampilkan selama ini sebelum boleh dicoba lagi.
const FAILURE_TTL: Duration = Duration::from_secs(15);

/// Cara mengambil isi satu folder.
#[derive(Clone, Debug)]
pub(crate) enum FolderFetchPlan {
    MySql {
        with_comments: bool,
    },
    Postgres {
        with_comments: bool,
    },
    /// SQL katalog PostgreSQL (function, procedure, trigger, ...).
    PostgresSql(String),
    MsSqlTables,
    MsSqlObjects,
    MongoCollections,
}

/// Status yang dipakai loader untuk mengisi node folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderFetchStatus {
    /// Sedang diambil di latar; tampilkan placeholder.
    Loading,
    /// Baru saja diambil dan kosong.
    RecentlyEmpty,
    /// Baru saja gagal; tampilkan pesannya.
    RecentlyFailed(String),
    /// Tidak bisa dijalankan di latar (tanpa runtime/koneksi); loader memakai
    /// jalur lamanya.
    Unavailable,
}

/// (connection id, database, table_type cache).
type FetchKey = (i64, String, String);

struct FetchDone {
    key: FetchKey,
    folder_type: NodeType,
    with_comments: bool,
    result: Result<Vec<(String, Option<String>)>, String>,
}

#[derive(Default)]
pub(crate) struct FolderFetchState {
    pending: HashSet<FetchKey>,
    recent: HashMap<FetchKey, (Instant, Option<String>)>,
    channel: Option<(mpsc::Sender<FetchDone>, mpsc::Receiver<FetchDone>)>,
}

/// Node placeholder di dalam folder (tidak bisa dibuka).
pub(crate) fn placeholder_node(text: impl Into<String>) -> TreeNode {
    let mut node = TreeNode::new(text.into(), NodeType::Column);
    node.is_loaded = true;
    node
}

async fn run_plan(
    plan: FolderFetchPlan,
    conn: crate::models::structs::ConnectionConfig,
    direct_pool: Option<DatabasePool>,
    shared_pools: std::sync::Arc<std::sync::Mutex<HashMap<i64, DatabasePool>>>,
    database: String,
    table_type: String,
) -> Result<Vec<(String, Option<String>)>, String> {
    let conn_id = conn.id.unwrap_or_default();
    let plain = |items: Option<Vec<String>>| -> Result<Vec<(String, Option<String>)>, String> {
        items
            .map(|v| v.into_iter().map(|name| (name, None)).collect())
            .ok_or_else(|| format!("could not list {table_type} objects"))
    };
    match plan {
        FolderFetchPlan::Postgres { with_comments } => {
            if with_comments {
                crate::driver_postgres::list_postgres_tables_with_comments(
                    &conn,
                    &database,
                    &table_type,
                )
                .await
                .ok_or_else(|| format!("could not list {table_type} objects"))
            } else {
                plain(
                    crate::driver_postgres::list_postgres_tables(&conn, &database, &table_type)
                        .await,
                )
            }
        }
        FolderFetchPlan::PostgresSql(sql) => {
            let pool = direct_pool.or_else(|| {
                crate::connection::pool::lock_or_recover(&shared_pools)
                    .get(&conn_id)
                    .cloned()
            });
            let sets =
                crate::schema_objects::exec::run_in_database(&conn, pool, Some(&database), &sql)
                    .await?;
            Ok(sets
                .first()
                .map(|s| s.first_column())
                .unwrap_or_default()
                .into_iter()
                .map(|name| (name, None))
                .collect())
        }
        plan => {
            let pool =
                super::app_impl::wait_for_connection_pool(direct_pool, shared_pools, conn_id)
                    .await?;
            match (plan, pool) {
                (FolderFetchPlan::MySql { with_comments }, DatabasePool::MySQL(p)) => {
                    if with_comments {
                        crate::driver_mysql::list_mysql_tables_with_comments(
                            &p,
                            &database,
                            &table_type,
                        )
                        .await
                        .ok_or_else(|| format!("could not list {table_type} objects"))
                    } else {
                        plain(
                            crate::driver_mysql::list_mysql_tables(&p, &database, &table_type)
                                .await,
                        )
                    }
                }
                (FolderFetchPlan::MsSqlTables, DatabasePool::MsSQL(p)) => {
                    plain(crate::driver_mssql::list_mssql_tables(&p, &database, &table_type).await)
                }
                (FolderFetchPlan::MsSqlObjects, DatabasePool::MsSQL(p)) => {
                    plain(crate::driver_mssql::list_mssql_objects(&p, &database, &table_type).await)
                }
                (FolderFetchPlan::MongoCollections, DatabasePool::MongoDB(client)) => client
                    .database(&database)
                    .list_collection_names()
                    .await
                    .map(|names| names.into_iter().map(|n| (n, None)).collect())
                    .map_err(|e| e.to_string()),
                _ => Err("the connection pool has an unexpected type".to_string()),
            }
        }
    }
}

impl Tabular {
    /// Minta isi folder diambil di latar. Lihat [`FolderFetchStatus`].
    /// `force` (refresh) mengabaikan hasil kosong/gagal yang diingat.
    pub(crate) fn folder_fetch(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_type: &str,
        folder_type: NodeType,
        plan: FolderFetchPlan,
        force: bool,
    ) -> FolderFetchStatus {
        let key: FetchKey = (
            connection_id,
            database_name.to_string(),
            table_type.to_string(),
        );
        if self.folder_fetch_state.pending.contains(&key) {
            return FolderFetchStatus::Loading;
        }
        if force {
            self.folder_fetch_state.recent.remove(&key);
        } else if let Some((at, error)) = self.folder_fetch_state.recent.get(&key) {
            let ttl = if error.is_some() {
                FAILURE_TTL
            } else {
                EMPTY_TTL
            };
            if at.elapsed() < ttl {
                return match error {
                    Some(e) => FolderFetchStatus::RecentlyFailed(e.clone()),
                    None => FolderFetchStatus::RecentlyEmpty,
                };
            }
        }
        let Some(runtime) = self.runtime.clone() else {
            return FolderFetchStatus::Unavailable;
        };
        let Some(conn) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .cloned()
        else {
            return FolderFetchStatus::Unavailable;
        };
        if !matches!(plan, FolderFetchPlan::Postgres { .. }) {
            crate::connection::ensure_background_pool_creation(self, connection_id);
        }
        let direct_pool = self.connection_pools.get(&connection_id).cloned();
        let shared_pools = self.shared_connection_pools.clone();
        let sender = self
            .folder_fetch_state
            .channel
            .get_or_insert_with(mpsc::channel)
            .0
            .clone();
        let wake = self.result_wake_hook();
        let with_comments = matches!(
            plan,
            FolderFetchPlan::MySql {
                with_comments: true
            } | FolderFetchPlan::Postgres {
                with_comments: true
            }
        );
        self.folder_fetch_state.pending.insert(key.clone());
        log::debug!("[TREE-FETCH] start {key:?} plan={plan:?}");
        runtime.spawn(async move {
            let work = run_plan(
                plan,
                conn,
                direct_pool,
                shared_pools,
                key.1.clone(),
                key.2.clone(),
            );
            let result = match tokio::time::timeout(FETCH_TIMEOUT, work).await {
                Ok(result) => result,
                Err(_) => Err(format!(
                    "the database did not respond within {}s",
                    FETCH_TIMEOUT.as_secs()
                )),
            };
            let _ = sender.send(FetchDone {
                key,
                folder_type,
                with_comments,
                result,
            });
            if let Some(wake) = wake {
                wake();
            }
        });
        FolderFetchStatus::Loading
    }

    /// Terapkan hasil pengambilan folder yang sudah selesai. Dipanggil tiap frame.
    pub(crate) fn poll_folder_fetches(&mut self) {
        let Some((_, receiver)) = &self.folder_fetch_state.channel else {
            return;
        };
        let finished: Vec<FetchDone> = receiver.try_iter().collect();
        for done in finished {
            let (connection_id, database, table_type) = done.key.clone();
            self.folder_fetch_state.pending.remove(&done.key);
            match done.result {
                Ok(items) if items.is_empty() => {
                    self.folder_fetch_state
                        .recent
                        .insert(done.key.clone(), (Instant::now(), None));
                }
                Ok(items) => {
                    self.folder_fetch_state.recent.remove(&done.key);
                    if done.with_comments {
                        let rows: Vec<(String, String, Option<String>)> = items
                            .into_iter()
                            .map(|(name, comment)| (name, table_type.clone(), comment))
                            .collect();
                        crate::cache_data::save_tables_with_comments_to_cache(
                            self,
                            connection_id,
                            &database,
                            &rows,
                        );
                    } else {
                        let rows: Vec<(String, String)> = items
                            .into_iter()
                            .map(|(name, _)| (name, table_type.clone()))
                            .collect();
                        crate::cache_data::save_tables_to_cache(
                            self,
                            connection_id,
                            &database,
                            &rows,
                        );
                    }
                }
                Err(error) => {
                    log::warn!("[TREE-FETCH] {:?} failed: {error}", done.key);
                    self.toasts.error(format!(
                        "Could not load {table_type} list of \"{database}\": {error}"
                    ));
                    self.folder_fetch_state
                        .recent
                        .insert(done.key.clone(), (Instant::now(), Some(error)));
                }
            }
            self.reload_folder_nodes(connection_id, &database, &done.folder_type);
        }
    }

    /// Muat ulang (dari cache) setiap node folder yang cocok di sidebar.
    fn reload_folder_nodes(&mut self, connection_id: i64, database: &str, folder_type: &NodeType) {
        let default_db = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .map(|c| c.database.clone())
            .unwrap_or_default();
        fn walk(
            tabular: &mut Tabular,
            nodes: &mut [TreeNode],
            connection_id: i64,
            database: &str,
            default_db: &str,
            folder_type: &NodeType,
        ) {
            for node in nodes.iter_mut() {
                let node_db = node.database_name.as_deref().unwrap_or(default_db);
                if node.node_type == *folder_type
                    && node.connection_id == Some(connection_id)
                    && node_db == database
                {
                    node.is_loaded = false;
                    tabular.load_folder_content(connection_id, node, folder_type.clone(), false);
                } else {
                    walk(
                        tabular,
                        &mut node.children,
                        connection_id,
                        database,
                        default_db,
                        folder_type,
                    );
                }
            }
        }
        let mut tree = std::mem::take(&mut self.items_tree);
        walk(
            self,
            &mut tree,
            connection_id,
            database,
            &default_db,
            folder_type,
        );
        self.items_tree = tree;
        let mut filtered = std::mem::take(&mut self.filtered_items_tree);
        walk(
            self,
            &mut filtered,
            connection_id,
            database,
            &default_db,
            folder_type,
        );
        self.filtered_items_tree = filtered;
    }

    /// Isi node folder untuk status latar. `Some(children)` bila loader harus
    /// berhenti di sini; `None` (tidak tersedia di latar) = pakai jalur lama.
    pub(crate) fn folder_fetch_children(status: FolderFetchStatus) -> Option<Vec<TreeNode>> {
        match status {
            FolderFetchStatus::Loading => Some(vec![placeholder_node("Loading…")]),
            FolderFetchStatus::RecentlyEmpty => Some(Vec::new()),
            FolderFetchStatus::RecentlyFailed(error) => {
                Some(vec![placeholder_node(format!("Failed to load: {error}"))])
            }
            FolderFetchStatus::Unavailable => None,
        }
    }
}
