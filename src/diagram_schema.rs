//! Sinkronisasi skema live untuk diagram ERD tanpa memblokir UI thread.
//!
//! Alur buka diagram: layout tersimpan (cache JSON lokal) ditampilkan dulu,
//! lalu [`fetch_schema_snapshot`] berjalan di runtime tokio dan hasilnya
//! digabung lewat [`merge_schema`] saat tiba di UI thread.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::models::enums::DatabasePool;
use crate::models::structs::{
    ConnectionConfig, DiagramColumn, DiagramEdge, DiagramGroup, DiagramNode, DiagramState,
    ForeignKey,
};

/// Batas tunggu pool koneksi yang sedang dibuat di background.
const POOL_WAIT: Duration = Duration::from_secs(20);
/// Batas waktu tiap query metadata.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Skema live satu database. `None` berarti pengambilan bagian tersebut
/// gagal, sehingga data cache untuk bagian itu dipertahankan apa adanya.
#[derive(Default)]
pub struct SchemaSnapshot {
    pub foreign_keys: Option<Vec<ForeignKey>>,
    pub columns: Option<HashMap<String, Vec<DiagramColumn>>>,
    pub tables: Option<Vec<String>>,
    /// Layout bersama dari tabel `diagram_by_tabular` (bila ada).
    pub shared_state: Option<DiagramState>,
}

/// Semua yang dibutuhkan task background; tidak meminjam `Tabular`.
pub struct SchemaFetchRequest {
    pub conn: ConnectionConfig,
    pub db_name: String,
    /// Pool yang sudah siap di UI thread (bila ada).
    pub pool: Option<DatabasePool>,
    /// Tempat pool hasil koneksi background muncul.
    pub shared_pools: Arc<Mutex<HashMap<i64, DatabasePool>>>,
    /// Cache SQLite lokal untuk write-through foreign key.
    pub cache_pool: Option<Arc<sqlx::SqlitePool>>,
}

async fn wait_for_pool(
    conn_id: i64,
    shared: &Mutex<HashMap<i64, DatabasePool>>,
) -> Option<DatabasePool> {
    let deadline = Instant::now() + POOL_WAIT;
    loop {
        if let Some(pool) = shared.lock().ok().and_then(|m| m.get(&conn_id).cloned()) {
            return Some(pool);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn timed<T>(fut: impl std::future::Future<Output = Option<T>>) -> Option<T> {
    tokio::time::timeout(QUERY_TIMEOUT, fut).await.ok().flatten()
}

/// Ambil FK, kolom, daftar tabel, dan layout bersama secara paralel.
pub async fn fetch_schema_snapshot(req: SchemaFetchRequest) -> Result<SchemaSnapshot, String> {
    let started = Instant::now();
    let conn_id = req.conn.id.ok_or("Connection has no id")?;
    let pool = match req.pool {
        Some(p) => p,
        None => wait_for_pool(conn_id, &req.shared_pools)
            .await
            .ok_or_else(|| format!("Connection '{}' is not ready", req.conn.name))?,
    };
    let db = req.db_name.as_str();
    let conn = &req.conn;

    let fks = timed(async {
        match &pool {
            DatabasePool::MySQL(p) => crate::driver_mysql::fetch_mysql_foreign_keys(p, db).await.ok(),
            DatabasePool::PostgreSQL(p) => crate::driver_postgres::fetch_postgres_foreign_keys(p).await.ok(),
            DatabasePool::SQLite(p) => crate::driver_sqlite::fetch_sqlite_foreign_keys(p).await.ok(),
            // Fetcher MsSQL mengembalikan daftar kosong saat gagal; kosong
            // diperlakukan sebagai "tidak diketahui" supaya edge cache aman.
            DatabasePool::MsSQL(_) => {
                Some(crate::connection::metadata::fetch_mssql_foreign_keys(conn, db).await)
                    .filter(|k| !k.is_empty())
            }
            _ => None,
        }
    });
    let columns = timed(async {
        match &pool {
            DatabasePool::MySQL(p) => crate::driver_mysql::fetch_mysql_columns(p, db).await.ok(),
            DatabasePool::PostgreSQL(p) => crate::driver_postgres::fetch_postgres_columns(p).await.ok(),
            DatabasePool::SQLite(p) => crate::driver_sqlite::fetch_sqlite_columns(p).await.ok(),
            _ => None,
        }
    });
    let tables = timed(async {
        match &pool {
            DatabasePool::MySQL(p) => crate::driver_mysql::list_mysql_tables(p, db, "table").await,
            DatabasePool::PostgreSQL(_) => crate::driver_postgres::list_postgres_tables(conn, db, "table").await,
            DatabasePool::SQLite(p) => crate::driver_sqlite::list_sqlite_tables(p, "table").await,
            DatabasePool::MsSQL(p) => crate::driver_mssql::list_mssql_tables(p, "table").await,
            _ => None,
        }
    });
    let shared_state = timed(async {
        crate::diagram_storage::load_diagram_from_database(&pool, db, None)
            .await
            .ok()
            .flatten()
    });

    let (foreign_keys, columns, tables, shared_state) =
        tokio::join!(fks, columns, tables, shared_state);

    if let (Some(cache), Some(keys)) = (&req.cache_pool, &foreign_keys) {
        crate::connection::metadata::write_foreign_key_cache(cache, conn_id, db, keys).await;
    }

    log::info!(
        "[DIAGRAM_PERF] schema {}/{}: {} tables, {} fks, {} column sets, shared layout: {} ({:?})",
        req.conn.name,
        db,
        tables.as_ref().map_or(0, Vec::len),
        foreign_keys.as_ref().map_or(0, Vec::len),
        columns.as_ref().map_or(0, HashMap::len),
        shared_state.is_some(),
        started.elapsed()
    );

    Ok(SchemaSnapshot {
        foreign_keys,
        columns,
        tables,
        shared_state,
    })
}

/// Rapikan state yang baru dimuat dari penyimpanan: buang isi link lama dan
/// migrasikan diagram lama hasil "Add Tables" menjadi link database.
pub fn prepare_stored_state(state: &mut DiagramState, conn_id: i64, db_name: &str) {
    crate::diagram_links::strip_linked(state);
    let migrated =
        crate::diagram_links::migrate_legacy_foreign_nodes(state, conn_id, db_name, |i| {
            crate::diagram_view::GROUP_COLORS
                [(i * 3 + 5) % crate::diagram_view::GROUP_COLORS.len()]
        });
    if migrated > 0 {
        // Simpan segera supaya `link_id` hasil migrasi stabil.
        state.save_requested = true;
        log::info!("[DIAGRAM_LINK] migrated {migrated} legacy database(s) in '{db_name}' to links");
    }
}

fn table_prefix(name: &str) -> &str {
    name.split('_').next().unwrap_or(name)
}

/// Gabungkan skema live ke state diagram: tambah tabel baru, buang tabel
/// yang sudah tidak ada, dan segarkan kolom serta FK. Posisi node tersimpan
/// dipertahankan; auto-layout hanya untuk diagram yang masih kosong.
pub fn merge_schema(
    state: &mut DiagramState,
    snapshot: &SchemaSnapshot,
    conn_id: i64,
    db_name: &str,
    conn_name: Option<&str>,
) {
    let fks: &[ForeignKey] = snapshot.foreign_keys.as_deref().unwrap_or_default();

    let mut table_names: HashSet<String> = HashSet::new();
    for fk in fks {
        table_names.insert(fk.table_name.clone());
        table_names.insert(fk.referenced_table_name.clone());
    }
    if let Some(tables) = &snapshot.tables {
        table_names.extend(tables.iter().cloned());
    }

    // Edge selalu mengikuti FK skema terkini, kecuali FK gagal diambil.
    if snapshot.foreign_keys.is_some() {
        state.edges = fks
            .iter()
            .map(|fk| DiagramEdge {
                source: fk.table_name.clone(),
                target: fk.referenced_table_name.clone(),
                label: String::new(),
            })
            .collect();
    }

    // Group berdasarkan prefix nama tabel; group yang sudah ada dibiarkan.
    let mut groups_map: HashMap<&str, usize> = HashMap::new();
    for table in &table_names {
        *groups_map.entry(table_prefix(table)).or_default() += 1;
    }
    let mut existing_group_ids: HashSet<String> =
        state.groups.iter().map(|g| g.id.clone()).collect();
    let colors = crate::diagram_view::GROUP_COLORS;
    let mut color_idx = 0;
    let mut prefixes: Vec<&str> = groups_map
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(p, _)| *p)
        .collect();
    // Urutan stabil supaya warna group tidak berubah-ubah antar pembukaan.
    prefixes.sort_unstable();
    for prefix in prefixes {
        let group_id = format!("group_{prefix}");
        if existing_group_ids.insert(group_id.clone()) {
            let mut chars = prefix.chars();
            let title = chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default();
            state.groups.push(DiagramGroup {
                id: group_id,
                title,
                color: colors[color_idx % colors.len()],
                manual_pos: None,
            });
            color_idx += 1;
        }
    }

    // Buang node tabel yang sudah tidak ada. Node `detached` (hasil impor)
    // dipertahankan; tanpa daftar tabel yang valid tidak ada yang dibuang.
    let tables_known = snapshot.tables.as_ref().is_some_and(|t| !t.is_empty());
    if tables_known {
        state
            .nodes
            .retain(|n| n.detached || crate::diagram_links::is_linked_id(&n.id) || table_names.contains(&n.id));
    }

    let is_init = state.nodes.is_empty();
    let existing_node_ids: HashSet<String> = state.nodes.iter().map(|n| n.id.clone()).collect();
    let mut new_tables: Vec<&String> = table_names
        .iter()
        .filter(|t| !existing_node_ids.contains(*t))
        .collect();
    new_tables.sort();
    for table in new_tables {
        let hash: u64 = table.bytes().fold(5381, |acc, c| {
            acc.wrapping_shl(5).wrapping_add(acc).wrapping_add(c as u64)
        });
        let target_group = format!("group_{}", table_prefix(table));
        let has_group = existing_group_ids.contains(&target_group);
        state.nodes.push(DiagramNode {
            id: table.clone(),
            title: table.clone(),
            pos: eframe::egui::pos2((hash % 800) as f32 + 100.0, ((hash / 800) % 600) as f32 + 100.0),
            size: eframe::egui::vec2(150.0, 100.0), // Default, will be auto-sized
            group_ids: if has_group { vec![target_group.clone()] } else { Vec::new() },
            group_id: has_group.then_some(target_group),
            database_name: Some(db_name.to_string()),
            connection_id: Some(conn_id),
            connection_name: conn_name.map(str::to_string),
            ..Default::default()
        });
    }

    // Segarkan kolom + metadata + FK semua node tabel yang ada di skema.
    for node in &mut state.nodes {
        if crate::diagram_links::is_linked_id(&node.id) {
            continue;
        }
        if node.database_name.is_none() {
            node.database_name = Some(db_name.to_string());
            node.connection_id = Some(conn_id);
        }
        if !table_names.contains(&node.id) {
            continue;
        }
        node.detached = false;
        if let Some(cols) = snapshot.columns.as_ref().and_then(|c| c.get(&node.id)) {
            node.columns = cols.iter().map(|c| c.name.clone()).collect();
            node.column_meta = cols.clone();
        }
        if snapshot.foreign_keys.is_some() {
            node.foreign_keys = fks
                .iter()
                .filter(|fk| fk.table_name == node.id)
                .cloned()
                .collect();
        }
    }
    // Relasi virtual ke tabel yang sudah tidak ada ikut dibuang; relasi ke
    // tabel link database dibiarkan sampai link-nya selesai dimuat.
    crate::diagram_links::prune_virtual_relations(state);

    if is_init && !state.nodes.is_empty() {
        crate::diagram_view::perform_auto_layout(state);
    }
}

/// Sidik layout yang bisa diedit user: posisi node & group host, relasi
/// virtual, dan link database. Pan/zoom sengaja tidak ikut.
pub fn layout_fingerprint(state: &DiagramState) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in state.nodes.iter().filter(|n| !crate::diagram_links::is_linked_id(&n.id)) {
        n.id.hash(&mut h);
        n.pos.x.to_bits().hash(&mut h);
        n.pos.y.to_bits().hash(&mut h);
        n.group_ids.hash(&mut h);
    }
    for g in state.groups.iter().filter(|g| !crate::diagram_links::is_linked_id(&g.id)) {
        g.id.hash(&mut h);
        g.title.hash(&mut h);
        g.manual_pos.map(|p| (p.x.to_bits(), p.y.to_bits())).hash(&mut h);
    }
    state.virtual_relations.len().hash(&mut h);
    for l in &state.linked_databases {
        l.link_id.hash(&mut h);
        l.offset.x.to_bits().hash(&mut h);
        l.offset.y.to_bits().hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: eframe::egui::pos2(10.0, 20.0),
            ..Default::default()
        }
    }

    fn fk(table: &str, parent: &str) -> ForeignKey {
        ForeignKey {
            constraint_name: format!("fk_{table}_{parent}"),
            table_name: table.into(),
            column_name: format!("{parent}_id"),
            referenced_table_name: parent.into(),
            referenced_column_name: "id".into(),
        }
    }

    fn snapshot(tables: &[&str], fks: Vec<ForeignKey>) -> SchemaSnapshot {
        SchemaSnapshot {
            foreign_keys: Some(fks),
            columns: None,
            tables: Some(tables.iter().map(|t| t.to_string()).collect()),
            shared_state: None,
        }
    }

    #[test]
    fn merge_adds_new_tables_and_keeps_saved_positions() {
        let mut state = DiagramState {
            nodes: vec![node("users")],
            ..Default::default()
        };
        merge_schema(&mut state, &snapshot(&["users", "orders"], vec![fk("orders", "users")]), 1, "shop", None);

        assert_eq!(state.nodes.len(), 2);
        let users = state.nodes.iter().find(|n| n.id == "users").unwrap();
        assert_eq!(users.pos, eframe::egui::pos2(10.0, 20.0));
        assert_eq!(state.edges.len(), 1);
        let orders = state.nodes.iter().find(|n| n.id == "orders").unwrap();
        assert_eq!(orders.foreign_keys.len(), 1);
        assert_eq!(orders.database_name.as_deref(), Some("shop"));
    }

    #[test]
    fn merge_drops_missing_tables_but_keeps_detached() {
        let mut detached = node("imported");
        detached.detached = true;
        let mut state = DiagramState {
            nodes: vec![node("users"), node("gone"), detached],
            ..Default::default()
        };
        merge_schema(&mut state, &snapshot(&["users"], vec![]), 1, "shop", None);

        let ids: Vec<&str> = state.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["users", "imported"]);
    }

    #[test]
    fn merge_without_table_list_removes_nothing() {
        let mut state = DiagramState {
            nodes: vec![node("users"), node("orders")],
            edges: vec![DiagramEdge {
                source: "orders".into(),
                target: "users".into(),
                label: String::new(),
            }],
            ..Default::default()
        };
        merge_schema(&mut state, &SchemaSnapshot::default(), 1, "shop", None);

        assert_eq!(state.nodes.len(), 2);
        // FK gagal diambil: edge cache dipertahankan.
        assert_eq!(state.edges.len(), 1);
    }

    #[test]
    fn merge_on_empty_state_creates_prefix_groups() {
        let mut state = DiagramState::default();
        merge_schema(&mut state, &snapshot(&["user_a", "user_b", "misc"], vec![]), 1, "shop", Some("local"));

        assert_eq!(state.nodes.len(), 3);
        assert!(state.groups.iter().any(|g| g.id == "group_user" && g.title == "User"));
        let a = state.nodes.iter().find(|n| n.id == "user_a").unwrap();
        assert_eq!(a.group_id.as_deref(), Some("group_user"));
        assert_eq!(a.connection_name.as_deref(), Some("local"));
    }

    #[test]
    fn fingerprint_tracks_node_moves_but_not_pan() {
        let mut state = DiagramState {
            nodes: vec![node("users")],
            ..Default::default()
        };
        let base = layout_fingerprint(&state);
        state.pan = eframe::egui::vec2(50.0, 50.0);
        state.zoom = 2.0;
        assert_eq!(layout_fingerprint(&state), base);
        state.nodes[0].pos.x += 1.0;
        assert_ne!(layout_fingerprint(&state), base);
    }
}
