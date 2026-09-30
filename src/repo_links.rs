//! Tautan repository antara folder HTTP API dan group diagram database.
//!
//! Keduanya bisa menyimpan URL git (dan folder project personal). Folder dan
//! group yang menunjuk repository yang sama dianggap satu project; kuncinya
//! adalah [`crate::repo_scan::repo_key`]. Endpoint yang di-generate dari
//! repository folder HTTP ditautkan ke tabel diagram lewat kunci ini.
//!
//! Modul ini murni (tanpa egui dan tanpa `Tabular`) supaya mudah dites. Membaca
//! file diagram dilakukan oleh [`load_diagram_files`], yang hanya butuh path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::http_collection::{HttpWorkspace, SavedRequest};
use crate::models::structs::{DiagramState, EndpointLink};

/// Group diagram yang punya repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRef {
    pub conn_id: i64,
    pub db_name: String,
    pub group_id: String,
    pub group_title: String,
    pub key: String,
}

/// Folder HTTP API yang punya repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRef {
    pub workspace_id: String,
    pub workspace_name: String,
    pub folder_id: String,
    pub folder_name: String,
    pub key: String,
}

/// Diagram yang ikut diindeks (dari tab terbuka atau file cache lokal).
#[derive(Debug, Clone)]
pub struct IndexedDiagram {
    pub conn_id: i64,
    pub db_name: String,
    /// Kunci repository milik group-group di diagram ini.
    pub keys: HashSet<String>,
    /// Nama tabel milik diagram ini (tanpa tabel database yang di-link).
    pub tables: Vec<String>,
}

/// Indeks kunci repository → group diagram dan folder HTTP API.
#[derive(Debug, Clone, Default)]
pub struct RepoIndex {
    pub groups: Vec<GroupRef>,
    pub folders: Vec<FolderRef>,
    pub diagrams: Vec<IndexedDiagram>,
}

impl RepoIndex {
    /// Tambahkan group-group ber-repository dari satu diagram. Diagram yang
    /// sama (conn, db) yang sudah diindeks dilewati, jadi tab terbuka (lebih
    /// baru) harus ditambahkan sebelum file cache.
    pub fn add_diagram(&mut self, conn_id: i64, db_name: &str, state: &DiagramState) {
        if self
            .diagrams
            .iter()
            .any(|d| d.conn_id == conn_id && d.db_name == db_name)
        {
            return;
        }
        let mut keys = HashSet::new();
        for g in &state.groups {
            if crate::diagram_links::is_linked_id(&g.id) {
                continue;
            }
            let Some(key) = g.repo_key() else { continue };
            keys.insert(key.clone());
            self.groups.push(GroupRef {
                conn_id,
                db_name: db_name.to_string(),
                group_id: g.id.clone(),
                group_title: g.title.clone(),
                key,
            });
        }
        let tables = state
            .nodes
            .iter()
            .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
            .map(|n| n.title.clone())
            .collect();
        self.diagrams.push(IndexedDiagram {
            conn_id,
            db_name: db_name.to_string(),
            keys,
            tables,
        });
    }

    /// Tambahkan semua folder HTTP API yang punya repository.
    pub fn add_workspaces(&mut self, workspaces: &[HttpWorkspace]) {
        for (ws, folder) in crate::http_collection::all_folders(workspaces) {
            let Some(key) = folder.repo_key() else {
                continue;
            };
            self.folders.push(FolderRef {
                workspace_id: ws.id.clone(),
                workspace_name: ws.name.clone(),
                folder_id: folder.id.clone(),
                folder_name: folder.name.clone(),
                key,
            });
        }
    }

    pub fn groups_for<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a GroupRef> + 'a {
        self.groups.iter().filter(move |g| g.key == key)
    }

    pub fn folders_for<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a FolderRef> + 'a {
        self.folders.iter().filter(move |f| f.key == key)
    }

    /// Diagram yang punya group dengan kunci `key`.
    pub fn diagrams_for<'a>(
        &'a self,
        key: &'a str,
    ) -> impl Iterator<Item = &'a IndexedDiagram> + 'a {
        self.diagrams.iter().filter(move |d| d.keys.contains(key))
    }

    /// Nama tabel unik dari semua diagram bertautan `key`, untuk prompt AI.
    pub fn tables_for(&self, key: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in self.diagrams_for(key) {
            for t in &d.tables {
                if seen.insert(t.to_ascii_lowercase()) {
                    out.push(t.clone());
                }
            }
        }
        out
    }
}

/// `(conn_id, nama aman database)` dari nama file cache diagram
/// `conn_{id}_{db}.json` (lihat `diagram_storage::local_diagram_file_name`).
pub fn parse_diagram_file_name(name: &str) -> Option<(i64, String)> {
    let rest = name.strip_prefix("conn_")?.strip_suffix(".json")?;
    let (id, db) = rest.split_once('_')?;
    Some((id.parse().ok()?, db.to_string()))
}

/// Nama database asli diagram: `database_name` terbanyak di node miliknya.
/// Nama file hanya menyimpan versi aman (non-alfanumerik jadi `_`).
pub fn guess_db_name(state: &DiagramState, safe_name: &str) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for n in &state.nodes {
        if crate::diagram_links::is_linked_id(&n.id) {
            continue;
        }
        let Some(db) = n.database_name.as_deref().filter(|d| !d.is_empty()) else {
            continue;
        };
        match counts.iter_mut().find(|(d, _)| d == db) {
            Some((_, c)) => *c += 1,
            None => counts.push((db.to_string(), 1)),
        }
    }
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect()
    };
    counts
        .into_iter()
        .filter(|(d, _)| safe(d) == safe_name)
        .max_by_key(|(_, c)| *c)
        .map(|(d, _)| d)
        .unwrap_or_else(|| safe_name.to_string())
}

/// Diagram tersimpan di folder cache `dir`: `(conn_id, db, path, state)`.
/// File yang rusak dilewati dengan peringatan di log.
pub fn load_diagram_files(dir: &Path) -> Vec<(i64, String, PathBuf, DiagramState)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((conn_id, safe_db)) = parse_diagram_file_name(&name) else {
            continue;
        };
        let path = entry.path();
        let state = match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<DiagramState>(&b).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[REPO_LINKS] skip unreadable diagram {}: {e}",
                    path.display()
                );
                continue;
            }
        };
        let db = guess_db_name(&state, &safe_db);
        out.push((conn_id, db, path, state));
    }
    out.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    out
}

/// Endpoint yang akan ditautkan ke tabel diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointTables {
    pub method: String,
    pub path: String,
    pub summary: String,
    pub request_id: Option<String>,
    pub source: Option<String>,
    pub tables: Vec<String>,
}

impl EndpointTables {
    /// Dari request tersimpan; path diambil dari URL (tanpa host/base URL).
    pub fn from_request(req: &SavedRequest) -> Self {
        Self {
            method: req.method.label().to_string(),
            path: req
                .route
                .clone()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| endpoint_path(&req.url)),
            summary: req.name.trim().to_string(),
            request_id: Some(req.id.clone()),
            source: req.source.clone(),
            tables: req.tables.clone(),
        }
    }
}

/// Path route dari URL request: `{{base_url}}/users/{id}?x=1` → `/users/{id}`.
pub fn endpoint_path(url: &str) -> String {
    crate::http_collection::extract_endpoint_url(url)
}

/// Id node tabel `name` di diagram, hanya tabel milik diagram ini. Nama
/// berskema (`public.users`) dicoba juga tanpa skema.
fn resolve_own_table(state: &DiagramState, name: &str) -> Option<String> {
    let found = crate::diagram_notes::resolve_table(&state.nodes, name).or_else(|| {
        let short = name.rsplit('.').next().filter(|s| *s != name)?;
        crate::diagram_notes::resolve_table(&state.nodes, short)
    })?;
    (!crate::diagram_links::is_linked_id(&found.id)).then(|| found.id.clone())
}

/// Hasil penerapan tautan endpoint ke satu diagram.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyStats {
    pub added: usize,
    pub updated: usize,
    /// Nama tabel yang tidak ditemukan di diagram.
    pub unresolved: usize,
}

impl ApplyStats {
    pub fn changed(&self) -> bool {
        self.added + self.updated > 0
    }
}

/// Tautkan `endpoints` ke tabel-tabel diagram. Link yang sudah ada (tabel +
/// method + path sama) diperbarui, bukan diduplikasi.
pub fn apply_endpoint_links(
    state: &mut DiagramState,
    key: Option<&str>,
    endpoints: &[EndpointTables],
) -> ApplyStats {
    let mut stats = ApplyStats::default();
    for ep in endpoints {
        let method = ep.method.trim().to_ascii_uppercase();
        let path = ep.path.trim();
        if method.is_empty() || path.is_empty() {
            continue;
        }
        let mut done: HashSet<String> = HashSet::new();
        for name in &ep.tables {
            let Some(table) = resolve_own_table(state, name) else {
                stats.unresolved += 1;
                continue;
            };
            if !done.insert(table.clone()) {
                continue;
            }
            let link = EndpointLink {
                table,
                method: method.clone(),
                path: path.to_string(),
                summary: ep.summary.clone(),
                request_id: ep.request_id.clone(),
                repo_key: key.map(str::to_string),
                source: ep.source.clone(),
            };
            match state
                .endpoint_links
                .iter_mut()
                .find(|l| l.same_endpoint(&link))
            {
                Some(existing) if *existing != link => {
                    *existing = link;
                    stats.updated += 1;
                }
                Some(_) => {}
                None => {
                    state.endpoint_links.push(link);
                    stats.added += 1;
                }
            }
        }
    }
    stats
}

/// Link endpoint milik tabel `table`, urut path lalu method.
pub fn links_for_table<'a>(state: &'a DiagramState, table: &str) -> Vec<&'a EndpointLink> {
    let mut out: Vec<&EndpointLink> = state
        .endpoint_links
        .iter()
        .filter(|l| l.table == table)
        .collect();
    out.sort_by(|a, b| {
        (a.path.as_str(), method_rank(&a.method)).cmp(&(b.path.as_str(), method_rank(&b.method)))
    });
    out
}

/// Jumlah endpoint per tabel (untuk badge).
pub fn endpoint_counts(state: &DiagramState) -> std::collections::HashMap<&str, usize> {
    let mut m = std::collections::HashMap::new();
    for l in &state.endpoint_links {
        *m.entry(l.table.as_str()).or_insert(0) += 1;
    }
    m
}

/// Urutan tampil method.
pub fn method_rank(method: &str) -> u8 {
    match method.to_ascii_uppercase().as_str() {
        "GET" => 0,
        "POST" => 1,
        "PUT" => 2,
        "PATCH" => 3,
        "DELETE" => 4,
        "HEAD" => 5,
        "OPTIONS" => 6,
        _ => 7,
    }
}

/// Buang link ke tabel yang sudah tidak ada di diagram.
pub fn prune_endpoint_links(state: &mut DiagramState) {
    if state.endpoint_links.is_empty() {
        return;
    }
    let ids: HashSet<&str> = state.nodes.iter().map(|n| n.id.as_str()).collect();
    // Diagram yang belum memuat node (skema masih diambil) tidak dipangkas.
    if ids.is_empty() {
        return;
    }
    let before = state.endpoint_links.len();
    let keep: Vec<EndpointLink> = state
        .endpoint_links
        .iter()
        .filter(|l| ids.contains(l.table.as_str()))
        .cloned()
        .collect();
    if keep.len() != before {
        state.endpoint_links = keep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramGroup, DiagramNode};

    fn node(id: &str, db: Option<&str>) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: eframe::egui::pos2(0.0, 0.0),
            size: eframe::egui::vec2(100.0, 100.0),
            columns: vec!["id".into()],
            foreign_keys: Vec::new(),
            group_ids: Vec::new(),
            group_id: None,
            column_meta: Vec::new(),
            detached: false,
            database_name: db.map(str::to_string),
            connection_id: Some(1),
            connection_name: None,
        }
    }

    fn state_with(nodes: &[&str]) -> DiagramState {
        DiagramState {
            nodes: nodes.iter().map(|n| node(n, Some("shop-db"))).collect(),
            ..Default::default()
        }
    }

    fn group(id: &str, url: Option<&str>) -> DiagramGroup {
        DiagramGroup {
            id: id.to_string(),
            title: format!("Group {id}"),
            color: eframe::egui::Color32::WHITE,
            manual_pos: None,
            repo_url: url.map(str::to_string),
        }
    }

    fn ep(method: &str, path: &str, tables: &[&str]) -> EndpointTables {
        EndpointTables {
            method: method.to_string(),
            path: path.to_string(),
            summary: String::new(),
            request_id: Some(format!("{method}{path}")),
            source: None,
            tables: tables.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn parses_diagram_file_names() {
        assert_eq!(
            parse_diagram_file_name("conn_12_shop_db.json"),
            Some((12, "shop_db".to_string()))
        );
        assert_eq!(parse_diagram_file_name("conn_x_db.json"), None);
        assert_eq!(parse_diagram_file_name("other.json"), None);
    }

    #[test]
    fn guesses_real_db_name_from_nodes() {
        let s = state_with(&["users"]);
        assert_eq!(guess_db_name(&s, "shop_db"), "shop-db");
        assert_eq!(guess_db_name(&s, "other"), "other");
    }

    #[test]
    fn index_matches_groups_and_folders_by_key() {
        let mut s = state_with(&["users", "orders"]);
        s.groups
            .push(group("g1", Some("git@github.com:Org/Shop.git")));
        s.groups.push(group("g2", None));
        let mut idx = RepoIndex::default();
        idx.add_diagram(1, "shop-db", &s);
        // Diagram yang sama dari file cache diabaikan.
        idx.add_diagram(1, "shop-db", &DiagramState::default());
        let ws = HttpWorkspace {
            id: "ws".into(),
            name: "API".into(),
            folders: vec![crate::http_collection::HttpFolder {
                id: "f1".into(),
                name: "Shop".into(),
                repo_url: Some("https://github.com/org/shop".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        idx.add_workspaces(std::slice::from_ref(&ws));
        let key = "github.com/org/shop";
        assert_eq!(idx.groups_for(key).count(), 1);
        assert_eq!(idx.folders_for(key).count(), 1);
        assert_eq!(idx.diagrams_for(key).count(), 1);
        assert_eq!(idx.tables_for(key), vec!["users", "orders"]);
        assert_eq!(idx.groups_for("github.com/org/other").count(), 0);
    }

    #[test]
    fn applies_links_without_duplicates() {
        let mut s = state_with(&["users", "orders"]);
        let eps = vec![
            ep("get", "/users/{id}", &["users", "public.users", "missing"]),
            ep("POST", "/orders", &["orders", "users"]),
        ];
        let stats = apply_endpoint_links(&mut s, Some("k"), &eps);
        assert_eq!(stats.added, 3);
        assert_eq!(stats.unresolved, 1);
        assert_eq!(s.endpoint_links[0].method, "GET");

        // Menerapkan ulang tidak menduplikasi; perubahan summary = update.
        let mut again = eps.clone();
        again[1].summary = "Create order".into();
        let stats = apply_endpoint_links(&mut s, Some("k"), &again);
        assert_eq!(stats.added, 0);
        assert_eq!(stats.updated, 2);
        assert_eq!(s.endpoint_links.len(), 3);
        assert_eq!(links_for_table(&s, "users").len(), 2);
        assert_eq!(endpoint_counts(&s).get("orders"), Some(&1));
    }

    #[test]
    fn prune_drops_links_to_removed_tables() {
        let mut s = state_with(&["users", "orders"]);
        apply_endpoint_links(&mut s, None, &[ep("GET", "/orders", &["orders"])]);
        s.nodes.retain(|n| n.id != "orders");
        prune_endpoint_links(&mut s);
        assert!(s.endpoint_links.is_empty());
    }

    #[test]
    fn endpoint_path_strips_base_url() {
        assert_eq!(endpoint_path("{{base_url}}/users/{id}?a=1"), "/users/{id}");
        assert_eq!(
            endpoint_path("http://localhost:3000/api/items"),
            "/api/items"
        );
    }
}
