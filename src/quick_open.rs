use eframe::egui;
use log::info;
use std::collections::{HashMap, HashSet};

use crate::{
    directory, editor,
    models::{self, enums::NodeType, structs::ConnectionConfig},
    sidebar_query,
    window_egui::{self, Tabular},
};

/// Category / type of item searchable in Quick Open
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuickOpenKind {
    Table,
    View,
    Procedure,
    Function,
    SavedQuery,
    History,
    Connection,
    Command,
}

impl QuickOpenKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Table => "Table",
            Self::View => "View",
            Self::Procedure => "Procedure",
            Self::Function => "Function",
            Self::SavedQuery => "Saved Query",
            Self::History => "History",
            Self::Connection => "Connection",
            Self::Command => "Command",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Table => egui_icons::icons::MDI_TABLE.codepoint,
            Self::View => egui_icons::icons::ICON_VISIBILITY.codepoint,
            Self::Procedure => egui_icons::icons::MDI_PACKAGE_VARIANT.codepoint,
            Self::Function => egui_icons::icons::MDI_FUNCTION.codepoint,
            Self::SavedQuery => egui_icons::icons::ICON_DESCRIPTION.codepoint,
            Self::History => egui_icons::icons::ICON_HISTORY.codepoint,
            Self::Connection => egui_icons::icons::MDI_DATABASE.codepoint,
            Self::Command => egui_icons::icons::ICON_TERMINAL.codepoint,
        }
    }

    pub fn badge_color(&self, dark_mode: bool) -> (egui::Color32, egui::Color32) {
        // (background_color, text_color)
        if dark_mode {
            match self {
                Self::Table => (egui::Color32::from_rgb(22, 60, 42), egui::Color32::from_rgb(74, 222, 128)),
                Self::View => (egui::Color32::from_rgb(18, 48, 68), egui::Color32::from_rgb(56, 189, 248)),
                Self::Procedure => (egui::Color32::from_rgb(50, 25, 75), egui::Color32::from_rgb(192, 132, 252)),
                Self::Function => (egui::Color32::from_rgb(60, 45, 20), egui::Color32::from_rgb(251, 191, 36)),
                Self::SavedQuery => (egui::Color32::from_rgb(65, 40, 20), egui::Color32::from_rgb(251, 146, 60)),
                Self::History => (egui::Color32::from_rgb(45, 45, 55), egui::Color32::from_rgb(203, 213, 225)),
                Self::Connection => (egui::Color32::from_rgb(30, 40, 80), egui::Color32::from_rgb(129, 140, 248)),
                Self::Command => (egui::Color32::from_rgb(60, 25, 45), egui::Color32::from_rgb(244, 114, 182)),
            }
        } else {
            match self {
                Self::Table => (egui::Color32::from_rgb(220, 252, 231), egui::Color32::from_rgb(22, 101, 52)),
                Self::View => (egui::Color32::from_rgb(224, 242, 254), egui::Color32::from_rgb(7, 89, 133)),
                Self::Procedure => (egui::Color32::from_rgb(243, 232, 255), egui::Color32::from_rgb(107, 33, 168)),
                Self::Function => (egui::Color32::from_rgb(254, 243, 199), egui::Color32::from_rgb(146, 64, 14)),
                Self::SavedQuery => (egui::Color32::from_rgb(255, 237, 213), egui::Color32::from_rgb(154, 52, 18)),
                Self::History => (egui::Color32::from_rgb(241, 245, 249), egui::Color32::from_rgb(71, 85, 105)),
                Self::Connection => (egui::Color32::from_rgb(224, 231, 255), egui::Color32::from_rgb(55, 48, 163)),
                Self::Command => (egui::Color32::from_rgb(252, 231, 243), egui::Color32::from_rgb(157, 23, 77)),
            }
        }
    }
}

/// A single item discoverable via Quick Open (with pre-lowercased cache for fast matching)
#[derive(Clone, Debug)]
pub struct QuickOpenItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub title_lower: String,
    pub subtitle_lower: String,
    pub kind: QuickOpenKind,
    pub connection_id: Option<i64>,
    pub connection_name: Option<String>,
    pub database_name: Option<String>,
    pub table_name: Option<String>,
    pub file_path: Option<String>,
    pub sql_content: Option<String>,
    pub shortcut: Option<String>,
}

impl QuickOpenItem {
    pub fn new(
        id: String,
        title: String,
        subtitle: String,
        kind: QuickOpenKind,
        connection_id: Option<i64>,
        connection_name: Option<String>,
        database_name: Option<String>,
        table_name: Option<String>,
        file_path: Option<String>,
        sql_content: Option<String>,
        shortcut: Option<String>,
    ) -> Self {
        let title_lower = title.to_lowercase();
        let subtitle_lower = subtitle.to_lowercase();
        Self {
            id,
            title,
            subtitle,
            title_lower,
            subtitle_lower,
            kind,
            connection_id,
            connection_name,
            database_name,
            table_name,
            file_path,
            sql_content,
            shortcut,
        }
    }
}

/// State for Quick Open modal
#[derive(Clone, Debug)]
pub struct QuickOpenState {
    pub is_open: bool,
    pub query: String,
    pub selected_index: usize,
    pub active_category: Option<QuickOpenKind>,
    pub items: Vec<QuickOpenItem>,
    pub filtered_items: Vec<(usize, i32)>, // (item_idx, score)
    pub request_focus: bool,
    pub scroll_to_selected: bool,
}

impl Default for QuickOpenState {
    fn default() -> Self {
        Self {
            is_open: false,
            query: String::new(),
            selected_index: 0,
            active_category: None,
            items: Vec::new(),
            filtered_items: Vec::new(),
            request_focus: false,
            scroll_to_selected: false,
        }
    }
}

impl QuickOpenState {
    /// Invalidate items cache so next open will reload from database/tree
    pub fn invalidate(&mut self) {
        self.items.clear();
        self.filtered_items.clear();
    }

    /// Close the modal
    pub fn close(&mut self) {
        self.is_open = false;
        self.query.clear();
        self.selected_index = 0;
        self.filtered_items.clear();
        self.scroll_to_selected = false;
    }

    /// Navigate up or down in filtered results
    pub fn navigate(&mut self, delta: i32) {
        if self.filtered_items.is_empty() {
            self.selected_index = 0;
            return;
        }
        let count = self.filtered_items.len() as i32;
        let new_idx = (self.selected_index as i32 + delta).rem_euclid(count);
        self.selected_index = new_idx as usize;
        self.scroll_to_selected = true;
    }

    /// Cycle active category filter
    pub fn cycle_category(&mut self) {
        let categories = [
            None,
            Some(QuickOpenKind::Table),
            Some(QuickOpenKind::View),
            Some(QuickOpenKind::Procedure),
            Some(QuickOpenKind::SavedQuery),
            Some(QuickOpenKind::History),
            Some(QuickOpenKind::Connection),
            Some(QuickOpenKind::Command),
        ];

        let current_pos = categories.iter().position(|c| *c == self.active_category).unwrap_or(0);
        let next_pos = (current_pos + 1) % categories.len();
        self.active_category = categories[next_pos];
        self.refilter();
    }

    /// Set category filter directly
    pub fn set_category(&mut self, category: Option<QuickOpenKind>) {
        if self.active_category == category {
            self.active_category = None; // toggle off
        } else {
            self.active_category = category;
        }
        self.refilter();
    }

    /// Recalculate filtered items with smart fuzzy matching and scoring
    pub fn refilter(&mut self) {
        let raw_query = self.query.trim();

        // Check for quick prefix overrides e.g. "t:", "@table", ">", "h:", "c:", "q:"
        let (filter_kind, clean_query) = parse_query_prefix(raw_query);
        let effective_category = filter_kind.or(self.active_category);
        let clean_lower = clean_query.to_lowercase();

        let mut scored: Vec<(usize, i32)> = Vec::with_capacity(self.items.len().min(1024));

        for (idx, item) in self.items.iter().enumerate() {
            // Apply category filter if active
            if let Some(cat) = effective_category {
                if item.kind != cat {
                    // Match function/procedure under procedure filter
                    if !(cat == QuickOpenKind::Procedure && item.kind == QuickOpenKind::Function) {
                        continue;
                    }
                }
            }

            if clean_lower.is_empty() {
                // Default priority score when query is empty
                let base_score = match item.kind {
                    QuickOpenKind::Connection => 800,
                    QuickOpenKind::Table => 700,
                    QuickOpenKind::View => 650,
                    QuickOpenKind::Procedure | QuickOpenKind::Function => 600,
                    QuickOpenKind::SavedQuery => 500,
                    QuickOpenKind::History => 400,
                    QuickOpenKind::Command => 300,
                };
                scored.push((idx, base_score));
            } else if let Some(score) = score_fuzzy_match_fast(&clean_lower, item) {
                scored.push((idx, score));
            }
        }

        // Sort descending by score; if tied, sort by title length (shorter first)
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                let item_a = &self.items[a.0];
                let item_b = &self.items[b.0];
                item_a.title.len().cmp(&item_b.title.len())
            })
        });

        self.filtered_items = scored;
        if self.selected_index >= self.filtered_items.len() {
            self.selected_index = 0;
        }
    }
}

/// Parse search prefix shortcuts from input query
fn parse_query_prefix(query: &str) -> (Option<QuickOpenKind>, &str) {
    let lower = query.to_lowercase();
    let lower_str = lower.as_str();

    if let Some(rest) = lower_str.strip_prefix("t:").or_else(|| lower_str.strip_prefix("@table ")).or_else(|| lower_str.strip_prefix("@t ")) {
        return (Some(QuickOpenKind::Table), query[query.len() - rest.len()..].trim());
    }
    if let Some(rest) = lower_str.strip_prefix("v:").or_else(|| lower_str.strip_prefix("@view ")).or_else(|| lower_str.strip_prefix("@v ")) {
        return (Some(QuickOpenKind::View), query[query.len() - rest.len()..].trim());
    }
    if let Some(rest) = lower_str.strip_prefix("p:").or_else(|| lower_str.strip_prefix("@proc ")).or_else(|| lower_str.strip_prefix("@p ")) {
        return (Some(QuickOpenKind::Procedure), query[query.len() - rest.len()..].trim());
    }
    if let Some(rest) = lower_str.strip_prefix("q:").or_else(|| lower_str.strip_prefix("/").or_else(|| lower_str.strip_prefix("@query "))) {
        return (Some(QuickOpenKind::SavedQuery), query[query.len() - rest.len()..].trim());
    }
    if let Some(rest) = lower_str.strip_prefix("h:").or_else(|| lower_str.strip_prefix("?").or_else(|| lower_str.strip_prefix("@hist "))) {
        return (Some(QuickOpenKind::History), query[query.len() - rest.len()..].trim());
    }
    if let Some(rest) = lower_str.strip_prefix("c:").or_else(|| lower_str.strip_prefix("#").or_else(|| lower_str.strip_prefix("@conn "))) {
        return (Some(QuickOpenKind::Connection), query[query.len() - rest.len()..].trim());
    }
    if let Some(rest) = lower_str.strip_prefix(">").or_else(|| lower_str.strip_prefix("cmd:").or_else(|| lower_str.strip_prefix("@cmd "))) {
        return (Some(QuickOpenKind::Command), query[query.len() - rest.len()..].trim());
    }

    (None, query)
}

/// Fast fuzzy score for an item against pre-lowercased search query (no allocations)
fn score_fuzzy_match_fast(q_lower: &str, item: &QuickOpenItem) -> Option<i32> {
    if q_lower.is_empty() {
        return Some(0);
    }

    let title_lower = &item.title_lower;
    let subtitle_lower = &item.subtitle_lower;

    // Direct match against title
    if title_lower == q_lower {
        return Some(10000);
    }

    // Title starts with query
    if title_lower.starts_with(q_lower) {
        return Some(5000 - (item.title.len() as i32 * 5));
    }

    // Title contains exact substring
    if let Some(pos) = title_lower.find(q_lower) {
        let word_boundary_bonus = if pos == 0 || title_lower.as_bytes().get(pos.saturating_sub(1)).is_some_and(|&b| b == b'_' || b == b'.' || b == b' ') {
            1000
        } else {
            0
        };
        return Some(3000 + word_boundary_bonus - (pos as i32 * 20) - (item.title.len() as i32 * 2));
    }

    // Subtitle contains exact substring
    if subtitle_lower.contains(q_lower) {
        return Some(1500 - (item.subtitle.len() as i32));
    }

    // Subsequence fuzzy match on title
    if let Some(sub_score) = subsequence_fuzzy_fast(q_lower, &item.title, title_lower) {
        return Some(sub_score);
    }

    // Fallback: check sql content for history
    if let Some(sql) = &item.sql_content {
        let sql_lower = sql.to_lowercase();
        if sql_lower.contains(q_lower) {
            return Some(800);
        }
    }

    None
}

/// Fast subsequence and CamelHump matcher
fn subsequence_fuzzy_fast(query_lower: &str, target_orig: &str, target_lower: &str) -> Option<i32> {
    let q_bytes = query_lower.as_bytes();
    let orig_bytes = target_orig.as_bytes();
    let lower_bytes = target_lower.as_bytes();

    if q_bytes.is_empty() {
        return Some(0);
    }

    let mut qi = 0usize;
    let mut score = 500i32;
    let mut prev_matched_idx: Option<usize> = None;

    for (idx, &ch) in lower_bytes.iter().enumerate() {
        if qi >= q_bytes.len() {
            break;
        }
        if ch == q_bytes[qi] {
            // Distance penalty
            if let Some(prev) = prev_matched_idx {
                let dist = idx.saturating_sub(prev);
                if dist == 1 {
                    score += 50; // consecutive match bonus
                } else {
                    score -= (dist as i32) * 5;
                }
            }

            // Word boundary & CamelCase bonus
            let is_boundary = idx == 0 || orig_bytes.get(idx.saturating_sub(1)).is_some_and(|&c| c == b'_' || c == b'.' || c == b' ');
            let is_camel = orig_bytes.get(idx).is_some_and(|c| c.is_ascii_uppercase());
            if is_boundary || is_camel {
                score += 150;
            }

            prev_matched_idx = Some(idx);
            qi += 1;
        }
    }

    if qi == q_bytes.len() {
        Some(score.max(100))
    } else {
        None
    }
}

/// Load all searchable items from SQLite cache and RAM state
pub fn load_all_quick_open_items(tabular: &mut Tabular) -> Vec<QuickOpenItem> {
    let mut items = Vec::new();
    let mut seen_ids = HashSet::new();

    // Map connection ID -> ConnectionConfig for O(1) lookups
    let mut conn_map: HashMap<i64, ConnectionConfig> = HashMap::new();
    for conn in &tabular.connections {
        if let Some(id) = conn.id {
            conn_map.insert(id, conn.clone());
        }
    }

    // 1. CONNECTIONS
    for conn in &tabular.connections {
        let conn_id = conn.id.unwrap_or(0);
        let id = format!("conn_{}", conn_id);
        if seen_ids.insert(id.clone()) {
            let subtitle = format!(
                "{} • {}:{} • DB: {}",
                conn.connection_type.badge_label(),
                conn.host,
                conn.port,
                if conn.database.is_empty() { "default" } else { &conn.database }
            );
            items.push(QuickOpenItem::new(
                id,
                conn.name.clone(),
                subtitle,
                QuickOpenKind::Connection,
                Some(conn_id),
                Some(conn.name.clone()),
                Some(conn.database.clone()),
                None,
                None,
                None,
                Some("↵ Connect".to_string()),
            ));
        }
    }

    // 2. CACHED TABLES, VIEWS, PROCEDURES, FUNCTIONS FROM SQLITE
    if let Some(pool) = &tabular.db_pool {
        let pool_clone = pool.clone();
        let rt = tabular.get_runtime();
        let fetched = rt.block_on(async {
            sqlx::query_as::<_, (i64, String, String, String)>(
                "SELECT connection_id, database_name, table_name, table_type FROM table_cache ORDER BY table_name ASC"
            )
            .fetch_all(pool_clone.as_ref())
            .await
            .unwrap_or_default()
        });

        for (conn_id, db_name, tbl_name, tbl_type) in fetched {
            let kind = match tbl_type.to_lowercase().as_str() {
                "view" => QuickOpenKind::View,
                "procedure" => QuickOpenKind::Procedure,
                "function" => QuickOpenKind::Function,
                _ => QuickOpenKind::Table,
            };

            let id = format!("{}_{}_{}_{}", kind.label(), conn_id, db_name, tbl_name);
            if seen_ids.insert(id.clone()) {
                let conn_name = conn_map
                    .get(&conn_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| format!("Conn #{}", conn_id));

                let subtitle = format!(
                    "{} • {} • {}",
                    kind.label(),
                    db_name,
                    conn_name
                );

                items.push(QuickOpenItem::new(
                    id,
                    tbl_name.clone(),
                    subtitle,
                    kind,
                    Some(conn_id),
                    Some(conn_name),
                    Some(db_name),
                    Some(tbl_name),
                    None,
                    None,
                    Some("↵ Open".to_string()),
                ));
            }
        }
    }

    // Fallback/Supplementary: Traverse tabular.items_tree in RAM for active session objects
    fn scan_tree_nodes(
        nodes: &[models::structs::TreeNode],
        items: &mut Vec<QuickOpenItem>,
        seen_ids: &mut HashSet<String>,
        conn_map: &HashMap<i64, ConnectionConfig>,
    ) {
        for node in nodes {
            let kind = match node.node_type {
                NodeType::Table => Some(QuickOpenKind::Table),
                NodeType::View => Some(QuickOpenKind::View),
                NodeType::StoredProcedure => Some(QuickOpenKind::Procedure),
                NodeType::UserFunction => Some(QuickOpenKind::Function),
                NodeType::Connection => Some(QuickOpenKind::Connection),
                _ => None,
            };

            if let Some(k) = kind {
                let conn_id = node.connection_id;
                let db_name = node.database_name.clone().unwrap_or_default();
                let id = format!("{}_{:?}_{}_{}", k.label(), conn_id, db_name, node.name);

                if seen_ids.insert(id.clone()) {
                    let conn_name = conn_id
                        .and_then(|cid| conn_map.get(&cid).map(|c| c.name.clone()))
                        .unwrap_or_else(|| "Database".to_string());

                    let subtitle = if db_name.is_empty() {
                        format!("{} • {}", k.label(), conn_name)
                    } else {
                        format!("{} • {} • {}", k.label(), db_name, conn_name)
                    };

                    items.push(QuickOpenItem::new(
                        id,
                        node.name.clone(),
                        subtitle,
                        k,
                        conn_id,
                        Some(conn_name),
                        if db_name.is_empty() { None } else { Some(db_name) },
                        Some(node.name.clone()),
                        node.file_path.clone(),
                        None,
                        Some("↵ Open".to_string()),
                    ));
                }
            }

            scan_tree_nodes(&node.children, items, seen_ids, conn_map);
        }
    }
    scan_tree_nodes(&tabular.items_tree, &mut items, &mut seen_ids, &conn_map);

    // 3. SAVED QUERIES (From query tree & directory)
    fn scan_query_nodes(
        nodes: &[models::structs::TreeNode],
        items: &mut Vec<QuickOpenItem>,
        seen_ids: &mut HashSet<String>,
    ) {
        for node in nodes {
            if node.node_type == NodeType::Query {
                let id = format!("query_{}", node.file_path.as_deref().unwrap_or(&node.name));
                if seen_ids.insert(id.clone()) {
                    let subtitle = node.file_path.as_deref().unwrap_or("Saved SQL Query");
                    items.push(QuickOpenItem::new(
                        id,
                        node.name.clone(),
                        format!("Saved Query • {}", subtitle),
                        QuickOpenKind::SavedQuery,
                        node.connection_id,
                        None,
                        node.database_name.clone(),
                        None,
                        node.file_path.clone(),
                        None,
                        Some("↵ Open Query".to_string()),
                    ));
                }
            }
            scan_query_nodes(&node.children, items, seen_ids);
        }
    }
    scan_query_nodes(&tabular.queries_tree, &mut items, &mut seen_ids);

    // Also scan queries directory directly if queries_tree is shallow
    let query_dir = directory::get_query_dir();
    if let Ok(entries) = std::fs::read_dir(&query_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "sql") {
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("query.sql").to_string();
                let path_str = path.to_string_lossy().to_string();
                let id = format!("query_{}", path_str);
                if seen_ids.insert(id.clone()) {
                    items.push(QuickOpenItem::new(
                        id,
                        file_name,
                        format!("Saved Query • {}", path_str),
                        QuickOpenKind::SavedQuery,
                        None,
                        None,
                        None,
                        None,
                        Some(path_str),
                        None,
                        Some("↵ Open Query".to_string()),
                    ));
                }
            }
        }
    }

    // 4. QUERY HISTORY (From RAM or SQLite)
    for hist in &tabular.history_items {
        let clean_sql = hist.query.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        let preview_title = if clean_sql.len() > 60 {
            format!("{}…", &clean_sql[..60])
        } else {
            clean_sql.clone()
        };

        let id = format!("hist_{}_{}", hist.connection_id, hist.executed_at);
        if seen_ids.insert(id.clone()) {
            let subtitle = format!("History • {} • {}", hist.connection_name, hist.executed_at);
            items.push(QuickOpenItem::new(
                id,
                preview_title,
                subtitle,
                QuickOpenKind::History,
                Some(hist.connection_id),
                Some(hist.connection_name.clone()),
                None,
                None,
                None,
                Some(hist.query.clone()),
                Some("↵ Run / Edit".to_string()),
            ));
        }
    }

    // If history in RAM is empty, try loading from SQLite
    if tabular.history_items.is_empty() && let Some(pool) = &tabular.db_pool {
        let pool_clone = pool.clone();
        let rt = tabular.get_runtime();
        let history_rows = rt.block_on(async {
            sqlx::query_as::<_, (i64, String, i64, String, String)>(
                "SELECT id, query_text, connection_id, connection_name, executed_at FROM query_history ORDER BY executed_at DESC LIMIT 60"
            )
            .fetch_all(pool_clone.as_ref())
            .await
            .unwrap_or_default()
        });

        for (_hid, q_text, conn_id, conn_name, exec_at) in history_rows {
            let clean_sql = q_text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
            let preview_title = if clean_sql.len() > 60 {
                format!("{}…", &clean_sql[..60])
            } else {
                clean_sql.clone()
            };

            let id = format!("hist_{}_{}", conn_id, exec_at);
            if seen_ids.insert(id.clone()) {
                let subtitle = format!("History • {} • {}", conn_name, exec_at);
                items.push(QuickOpenItem::new(
                    id,
                    preview_title,
                    subtitle,
                    QuickOpenKind::History,
                    Some(conn_id),
                    Some(conn_name),
                    None,
                    None,
                    None,
                    Some(q_text),
                    Some("↵ Run / Edit".to_string()),
                ));
            }
        }
    }

    // 5. ACTIONS & COMMANDS
    let commands = [
        ("Query: Run", "Execute the current query or selection", "⌘ Enter"),
        ("Query: Format SQL", "Format and beautify SQL query", "⌘ Shift+F"),
        ("Query: Explain", "Inspect query execution plan", "⌘ Shift+E"),
        ("Query: New Tab", "Open a new query editor tab", "⌘T"),
        ("Query: Close Tab", "Close current editor tab", "⌘W"),
        ("Query: Save Tab", "Save query to file", "⌘S"),
        ("Editor: Go to Definition", "Navigate to symbol / table in tree", "F12"),
        ("Editor: Rename Symbol", "Rename table/column across query", "F2"),
        ("Editor: Toggle Find & Replace", "Open search and replace toolbar", "⌘F"),
        ("Editor: Toggle Word Wrap", "Wrap long SQL query lines", ""),
        ("Editor: Toggle Line Numbers", "Show or hide editor line numbers", ""),
        ("Data: Export CSV", "Export current result set to CSV", ""),
        ("Data: Export JSON", "Export current result set to JSON", ""),
        ("Data: Export SQL Inserts", "Export data as SQL INSERT statements", ""),
        ("Data: Export Markdown", "Export table results as Markdown", ""),
        ("Data: Import CSV", "Import CSV data into table", ""),
        ("Transaction: Begin / Toggle", "Toggle transactional execution mode", "⌘ Shift+T"),
        ("Transaction: Commit", "Commit pending transaction changes", ""),
        ("Transaction: Rollback", "Rollback pending transaction changes", ""),
        ("DBA: Live Process Monitor", "Open real-time processlist monitor & kill queries", ""),
        ("DBA: Deadlock & Lock Tree", "Inspect active lock dependencies and blocking hierarchy", ""),
        ("DBA: Manage Users & Privileges", "Open User & Role Management and Object Grants GUI", ""),
        ("DBA: Create New User", "Create database user account & assign permissions", ""),
        ("Plugins: Extensibility & Wasm Automation", "Run Wasm plugins, export Parquet/DuckDB & generate ORM models", "⌘⇧P"),
        ("View: Refresh", "Refresh active database or table", "⌘R"),
        ("Preferences: Color Theme", "Change editor and UI color palette", ""),
        ("Preferences: Settings", "Configure application settings", "⌘,"),
    ];

    for (cmd_title, cmd_sub, sc) in commands {
        let id = format!("cmd_{}", cmd_title);
        if seen_ids.insert(id.clone()) {
            items.push(QuickOpenItem::new(
                id,
                cmd_title.to_string(),
                format!("Command • {}", cmd_sub),
                QuickOpenKind::Command,
                None,
                None,
                None,
                None,
                None,
                Some(cmd_title.to_string()),
                if sc.is_empty() { None } else { Some(sc.to_string()) },
            ));
        }
    }

    items
}

/// Execute an item selected from Quick Open
pub fn execute_quick_open_item(tabular: &mut Tabular, item: &QuickOpenItem) {
    info!("🚀 Executing Quick Open Item: [{:?}] {}", item.kind, item.title);

    match item.kind {
        QuickOpenKind::Table | QuickOpenKind::View => {
            let conn_id = item.connection_id.unwrap_or(0);
            let db_name = item.database_name.clone().unwrap_or_default();
            let table_name = item.table_name.clone().unwrap_or_else(|| item.title.clone());
            let is_view = item.kind == QuickOpenKind::View;

            tabular.current_connection_id = Some(conn_id);
            if !is_view && tabular.table_bottom_view == models::structs::TableBottomView::Query {
                tabular.table_bottom_view = models::structs::TableBottomView::Data;
            }

            // Find connection info to generate appropriate query
            let conn_opt = tabular.connections.iter().find(|c| c.id == Some(conn_id)).cloned();

            if let Some(conn) = conn_opt {
                let tab_title = if is_view {
                    format!("View: {}", table_name)
                } else {
                    format!("Table: {}", table_name)
                };

                let query_content = match conn.connection_type {
                    models::enums::DatabaseType::MySQL => {
                        if !db_name.is_empty() {
                            format!("USE `{}`;\nSELECT * FROM `{}` LIMIT 100;", db_name, table_name)
                        } else {
                            format!("SELECT * FROM `{}` LIMIT 100;", table_name)
                        }
                    }
                    models::enums::DatabaseType::PostgreSQL => {
                        if !db_name.is_empty() {
                            format!("SELECT * FROM \"{}\".\"{}\" LIMIT 100;", db_name, table_name)
                        } else {
                            format!("SELECT * FROM \"{}\" LIMIT 100;", table_name)
                        }
                    }
                    models::enums::DatabaseType::MsSQL => {
                        crate::driver_mssql::build_mssql_select_query(db_name.clone(), table_name.clone())
                    }
                    models::enums::DatabaseType::Redis => {
                        format!("SCAN 0 MATCH *{}* COUNT 100", table_name)
                    }
                    models::enums::DatabaseType::MongoDB => {
                        format!("// Sample collection {}\ndb.{}.find().limit(100)", table_name, table_name)
                    }
                    models::enums::DatabaseType::SQLite | models::enums::DatabaseType::ApiHttp => {
                        format!("SELECT * FROM `{}` LIMIT 100;", table_name)
                    }
                };

                // Create new tab or switch to existing
                if let Some(existing_idx) = editor::find_tab_for_target(
                    tabular,
                    &tab_title,
                    conn_id,
                    if db_name.is_empty() { None } else { Some(&db_name) },
                ) {
                    editor::switch_to_tab(tabular, existing_idx);
                } else {
                    editor::create_new_tab_with_connection_and_database(
                        tabular,
                        tab_title.clone(),
                        query_content.clone(),
                        Some(conn_id),
                        if db_name.is_empty() { None } else { Some(db_name.clone()) },
                    );
                }

                // Execute query to populate data table
                if let Some((headers, data)) = crate::connection::execute_query_with_connection(
                    tabular,
                    conn_id,
                    query_content,
                ) {
                    tabular.current_table_headers = headers.clone();
                    tabular.current_table_data = data.clone();
                    tabular.all_table_data = data.clone();
                    tabular.current_table_name = tab_title.clone();
                    tabular.total_rows = tabular.all_table_data.len();
                    tabular.current_page = 0;
                    if let Some(active_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                        active_tab.result_headers = headers;
                        active_tab.result_rows = data.clone();
                        active_tab.result_all_rows = data;
                        active_tab.result_table_name = tab_title;
                        active_tab.is_table_browse_mode = tabular.is_table_browse_mode;
                        active_tab.current_page = tabular.current_page;
                        active_tab.page_size = tabular.page_size;
                        active_tab.total_rows = tabular.total_rows;
                    }
                }
            }
        }
        QuickOpenKind::Procedure | QuickOpenKind::Function => {
            let conn_id = item.connection_id.unwrap_or(0);
            let db_name = item.database_name.clone();
            let proc_name = item.table_name.clone().unwrap_or_else(|| item.title.clone());

            tabular.current_connection_id = Some(conn_id);
            let conn_opt = tabular.connections.iter().find(|c| c.id == Some(conn_id)).cloned();

            if let Some(conn) = conn_opt {
                let definition_opt = crate::connection::fetch_procedure_definition(&conn, db_name.as_deref(), &proc_name);
                let content = match definition_opt {
                    Some(sql) if !sql.trim().is_empty() => sql,
                    _ => format!("-- Stored Procedure: {}\n-- Database: {}\n-- Connection: {}\n\n", proc_name, db_name.as_deref().unwrap_or("default"), conn.name),
                };

                let tab_title = format!("Proc: {}", proc_name);
                editor::create_new_tab_with_connection_and_database(
                    tabular,
                    tab_title,
                    content,
                    Some(conn_id),
                    db_name,
                );
            }
        }
        QuickOpenKind::SavedQuery => {
            if let Some(file_path) = &item.file_path {
                let _ = sidebar_query::open_query_file(tabular, file_path);
            }
        }
        QuickOpenKind::History => {
            let query_sql = item.sql_content.clone().unwrap_or_default();
            let conn_id = item.connection_id;
            let tab_title = format!("Hist-{}", chrono::Local::now().format("%m%d %H:%M"));

            editor::create_new_tab_with_connection_and_database(
                tabular,
                tab_title,
                query_sql,
                conn_id,
                item.database_name.clone(),
            );
        }
        QuickOpenKind::Connection => {
            if let Some(conn_id) = item.connection_id {
                tabular.current_connection_id = Some(conn_id);
                // Expand connection node in tree
                for node in &mut tabular.items_tree {
                    if node.connection_id == Some(conn_id) {
                        node.is_expanded = true;
                        break;
                    }
                }
            }
        }
        QuickOpenKind::Command => {
            if let Some(cmd) = &item.sql_content {
                editor::execute_command(tabular, cmd);
            }
        }
    }
}

/// Helper function to open Quick Open modal (instant cached load)
pub fn open_quick_open(tabular: &mut Tabular) {
    if tabular.quick_open_state.items.is_empty() {
        let items = load_all_quick_open_items(tabular);
        tabular.quick_open_state.items = items;
    }
    tabular.quick_open_state.is_open = true;
    tabular.quick_open_state.query.clear();
    tabular.quick_open_state.selected_index = 0;
    tabular.quick_open_state.active_category = None;
    tabular.quick_open_state.request_focus = true;
    tabular.quick_open_state.scroll_to_selected = true;
    tabular.quick_open_state.refilter();
    tabular.show_command_palette = false;
}

/// Helper function to navigate Quick Open modal
pub fn navigate_quick_open(tabular: &mut Tabular, delta: i32) {
    tabular.quick_open_state.navigate(delta);
}

/// Helper function to cycle filter categories
pub fn cycle_filter_category(tabular: &mut Tabular) {
    tabular.quick_open_state.cycle_category();
}

/// Helper function to execute selected item
pub fn execute_selected_quick_open(tabular: &mut Tabular) {
    let selected_item = {
        if tabular.quick_open_state.filtered_items.is_empty()
            || tabular.quick_open_state.selected_index >= tabular.quick_open_state.filtered_items.len()
        {
            None
        } else {
            let item_idx = tabular.quick_open_state.filtered_items[tabular.quick_open_state.selected_index].0;
            tabular.quick_open_state.items.get(item_idx).cloned()
        }
    };

    if let Some(item) = selected_item {
        tabular.quick_open_state.close();
        execute_quick_open_item(tabular, &item);
    }
}

/// Render the Universal Quick Open Modal UI with 60 FPS Virtualized Scrolling
pub fn render_quick_open(tabular: &mut Tabular, ctx: &egui::Context) {
    let progress = window_egui::style::render_modal_backdrop(ctx, "quick_open_spotlight", tabular.quick_open_state.is_open);
    if progress <= 0.01 {
        return;
    }

    let screen_rect = ctx.content_rect();
    let modal_width = 680.0_f32.min(screen_rect.width() - 32.0);
    let modal_x = (screen_rect.width() - modal_width) / 2.0;
    let base_y = (screen_rect.height() * 0.12).max(60.0);
    let animated_y = base_y + (1.0 - progress) * -25.0;

    let is_dark = ctx.global_style().visuals.dark_mode;
    let bg_color = if is_dark {
        egui::Color32::from_rgb(22, 24, 33)
    } else {
        egui::Color32::from_rgb(255, 255, 255)
    };
    let border_color = if is_dark {
        egui::Color32::from_rgb(48, 54, 72)
    } else {
        egui::Color32::from_rgb(215, 222, 235)
    };
    let accent_color = window_egui::style::theme_accent(ctx);

    egui::Area::new(egui::Id::new("universal_quick_open_area"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(modal_x, animated_y))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(bg_color)
                .stroke(egui::Stroke::new(1.0, border_color))
                .corner_radius(egui::CornerRadius::same(14u8))
                .shadow(egui::Shadow {
                    offset: [0, 16],
                    blur: 36,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(150),
                })
                .inner_margin(egui::Margin::same(0))
                .show(ui, |ui| {
                    ui.set_width(modal_width);
                    ui.vertical(|ui| {
                        // ─── SEARCH INPUT ROW ───────────────────────────────────────
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(16, 14))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui_icons::icons::ICON_SEARCH.rich_text()
                                            .size(18.0)
                                            .color(accent_color),
                                    );
                                    ui.add_space(6.0);

                                    let text_edit = egui::TextEdit::singleline(&mut tabular.quick_open_state.query)
                                        .hint_text("Search tables, views, procedures, queries, history, connections... (⌘P / ⌘K)")
                                        .frame(egui::Frame::NONE)
                                        .font(egui::FontId::proportional(16.0));

                                    let resp = ui.add_sized([modal_width - 130.0, 28.0], text_edit);

                                    if tabular.quick_open_state.request_focus {
                                        resp.request_focus();
                                        tabular.quick_open_state.request_focus = false;
                                    }

                                    if resp.changed() {
                                        tabular.quick_open_state.refilter();
                                        tabular.quick_open_state.selected_index = 0;
                                        tabular.quick_open_state.scroll_to_selected = true;
                                    }

                                    // Result count badge
                                    let count = tabular.quick_open_state.filtered_items.len();
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let badge_bg = if is_dark {
                                            egui::Color32::from_rgb(36, 40, 56)
                                        } else {
                                            egui::Color32::from_rgb(235, 240, 250)
                                        };
                                        egui::Frame::new()
                                            .fill(badge_bg)
                                            .corner_radius(egui::CornerRadius::same(6u8))
                                            .inner_margin(egui::Margin::symmetric(8, 3))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("{} found", count))
                                                        .size(11.0)
                                                        .color(ui.visuals().text_color().linear_multiply(0.7)),
                                                );
                                            });
                                    });
                                });
                            });

                        // ─── CATEGORY FILTER PILLS ──────────────────────────────────
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(14, 6))
                            .fill(if is_dark {
                                egui::Color32::from_rgb(22, 25, 36)
                            } else {
                                egui::Color32::from_rgb(240, 243, 250)
                            })
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let categories: &[(Option<QuickOpenKind>, &str, &str)] = &[
                                        (None, "All", egui_icons::icons::ICON_SELECT_ALL.codepoint),
                                        (Some(QuickOpenKind::Table), "Tables", egui_icons::icons::MDI_TABLE.codepoint),
                                        (Some(QuickOpenKind::View), "Views", egui_icons::icons::ICON_VISIBILITY.codepoint),
                                        (Some(QuickOpenKind::Procedure), "Procedures", egui_icons::icons::MDI_PACKAGE_VARIANT.codepoint),
                                        (Some(QuickOpenKind::SavedQuery), "Saved Queries", egui_icons::icons::ICON_DESCRIPTION.codepoint),
                                        (Some(QuickOpenKind::History), "History", egui_icons::icons::ICON_HISTORY.codepoint),
                                        (Some(QuickOpenKind::Connection), "Connections", egui_icons::icons::MDI_DATABASE.codepoint),
                                        (Some(QuickOpenKind::Command), "Commands", egui_icons::icons::ICON_TERMINAL.codepoint),
                                    ];

                                    for (cat, label, icon) in categories {
                                        let is_active = tabular.quick_open_state.active_category == *cat;
                                        let chip_bg = if is_active {
                                            accent_color
                                        } else if is_dark {
                                            egui::Color32::from_rgb(38, 42, 58)
                                        } else {
                                            egui::Color32::from_rgb(230, 235, 245)
                                        };

                                        let chip_text_color = if is_active {
                                            egui::Color32::WHITE
                                        } else if is_dark {
                                            egui::Color32::from_rgb(210, 215, 230)
                                        } else {
                                            egui::Color32::from_rgb(60, 70, 90)
                                        };

                                        let frame = egui::Frame::new()
                                            .fill(chip_bg)
                                            .corner_radius(egui::CornerRadius::same(12u8))
                                            .inner_margin(egui::Margin::symmetric(9, 4));

                                        let chip_resp = frame.show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(*icon).size(11.5));
                                                ui.add_space(2.0);
                                                ui.label(
                                                    egui::RichText::new(*label)
                                                        .size(11.5)
                                                        .strong()
                                                        .color(chip_text_color),
                                                );
                                            });
                                        }).response;

                                        let chip_click = chip_resp.interact(egui::Sense::click());
                                        if chip_click.clicked() {
                                            tabular.quick_open_state.set_category(*cat);
                                        }
                                    }
                                });
                            });

                        // 1px Border line
                        let line_y = ui.cursor().min.y;
                        ui.painter().hline(
                            modal_x..=(modal_x + modal_width),
                            line_y,
                            egui::Stroke::new(1.0, border_color),
                        );
                        ui.add_space(2.0);

                        // ─── SEARCH RESULTS LIST (VIRTUALIZED WITH show_rows) ───────
                        let list_height = 360.0_f32.min(screen_rect.height() * 0.5);
                        let row_height = 44.0_f32;
                        let total_count = tabular.quick_open_state.filtered_items.len();

                        let mut item_to_execute: Option<QuickOpenItem> = None;

                        egui::ScrollArea::vertical()
                            .max_height(list_height)
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, total_count, |ui, row_range| {
                                if total_count == 0 {
                                    ui.add_space(36.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            egui::RichText::new("No matching items found")
                                                .size(14.0)
                                                .color(ui.visuals().text_color().linear_multiply(0.5)),
                                        );
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new("Tip: Try searching by table name, query text, or use prefix: t: (tables), v: (views), p: (procedures), q: (queries), h: (history), c: (connections)")
                                                .size(11.5)
                                                .color(ui.visuals().text_color().linear_multiply(0.4)),
                                        );
                                    });
                                    ui.add_space(36.0);
                                } else {
                                    // If keyboard navigation requested scrolling to the selected row
                                    if tabular.quick_open_state.scroll_to_selected {
                                        let sel_idx = tabular.quick_open_state.selected_index.min(total_count.saturating_sub(1));
                                        let target_y = ui.min_rect().min.y + (sel_idx as f32 * row_height);
                                        let target_rect = egui::Rect::from_min_size(
                                            egui::pos2(modal_x, target_y),
                                            egui::vec2(modal_width, row_height),
                                        );
                                        ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
                                        tabular.quick_open_state.scroll_to_selected = false;
                                    }

                                    for filtered_idx in row_range {
                                        let &(item_idx, _score) = match tabular.quick_open_state.filtered_items.get(filtered_idx) {
                                            Some(pair) => pair,
                                            None => continue,
                                        };
                                        let item = match tabular.quick_open_state.items.get(item_idx) {
                                            Some(it) => it,
                                            None => continue,
                                        };

                                        let is_selected = filtered_idx == tabular.quick_open_state.selected_index;
                                        let (badge_bg, badge_fg) = item.kind.badge_color(is_dark);

                                        let row_bg = if is_selected {
                                            if is_dark {
                                                egui::Color32::from_rgb(42, 48, 68)
                                            } else {
                                                egui::Color32::from_rgb(228, 236, 250)
                                            }
                                        } else {
                                            egui::Color32::TRANSPARENT
                                        };

                                        let item_frame = egui::Frame::new()
                                            .fill(row_bg)
                                            .corner_radius(egui::CornerRadius::same(8u8))
                                            .inner_margin(egui::Margin::symmetric(12, 6));

                                        let row_resp = item_frame.show(ui, |ui| {
                                            ui.set_height(row_height - 12.0);
                                            ui.horizontal(|ui| {
                                                // Category Badge
                                                egui::Frame::new()
                                                    .fill(badge_bg)
                                                    .corner_radius(egui::CornerRadius::same(5u8))
                                                    .inner_margin(egui::Margin::symmetric(6, 2))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!("{} {}", item.kind.icon(), item.kind.label()))
                                                                .size(10.5)
                                                                .strong()
                                                                .color(badge_fg),
                                                        );
                                                    });

                                                ui.add_space(6.0);

                                                // Title & Subtitle in vertical stack
                                                ui.vertical(|ui| {
                                                    let title_color = if is_selected {
                                                        if is_dark { egui::Color32::WHITE } else { egui::Color32::BLACK }
                                                    } else {
                                                        ui.visuals().text_color()
                                                    };

                                                    ui.label(
                                                        egui::RichText::new(&item.title)
                                                            .size(13.0)
                                                            .strong()
                                                            .color(title_color),
                                                    );

                                                    if !item.subtitle.is_empty() {
                                                        ui.label(
                                                            egui::RichText::new(&item.subtitle)
                                                                .size(10.5)
                                                                .color(ui.visuals().text_color().linear_multiply(0.6)),
                                                        );
                                                    }
                                                });

                                                // Shortcut / Action Hint on the far right
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if let Some(sc) = &item.shortcut {
                                                        window_egui::style::render_shortcut_badge(ui, sc);
                                                    }
                                                });
                                            });
                                        }).response;

                                        let clicked = row_resp.interact(egui::Sense::click()).clicked();
                                        if clicked {
                                            tabular.quick_open_state.selected_index = filtered_idx;
                                            item_to_execute = Some(item.clone());
                                            break;
                                        }
                                    }
                                }
                            });

                        if let Some(item) = item_to_execute {
                            tabular.quick_open_state.close();
                            execute_quick_open_item(tabular, &item);
                        }

                        // ─── FOOTER BAR ───────────────────────────────────────────
                        let footer_y = ui.cursor().min.y;
                        ui.painter().hline(
                            modal_x..=(modal_x + modal_width),
                            footer_y,
                            egui::Stroke::new(1.0, border_color),
                        );

                        egui::Frame::new()
                            .fill(if is_dark { egui::Color32::from_rgb(18, 20, 28) } else { egui::Color32::from_rgb(245, 247, 251) })
                            .corner_radius(egui::CornerRadius { nw: 0, ne: 0, sw: 14, se: 14 })
                            .inner_margin(egui::Margin::symmetric(16, 9))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Tabular Quick Open")
                                            .size(11.5)
                                            .strong()
                                            .color(accent_color),
                                    );

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        window_egui::style::render_shortcut_badge(ui, "Esc");
                                        ui.label(egui::RichText::new("Close").size(11.0).color(ui.visuals().text_color().linear_multiply(0.6)));
                                        ui.add_space(8.0);

                                        window_egui::style::render_shortcut_badge(ui, "Tab");
                                        ui.label(egui::RichText::new("Filter").size(11.0).color(ui.visuals().text_color().linear_multiply(0.6)));
                                        ui.add_space(8.0);

                                        window_egui::style::render_shortcut_badge(ui, "↵");
                                        ui.label(egui::RichText::new("Open / Execute").size(11.0).color(ui.visuals().text_color().linear_multiply(0.6)));
                                        ui.add_space(8.0);

                                        window_egui::style::render_shortcut_badge(ui, "↑↓");
                                        ui.label(egui::RichText::new("Navigate").size(11.0).color(ui.visuals().text_color().linear_multiply(0.6)));
                                    });
                                });
                            });
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query_prefix() {
        assert_eq!(parse_query_prefix("t:users"), (Some(QuickOpenKind::Table), "users"));
        assert_eq!(parse_query_prefix("@table orders"), (Some(QuickOpenKind::Table), "orders"));
        assert_eq!(parse_query_prefix("v:active_users"), (Some(QuickOpenKind::View), "active_users"));
        assert_eq!(parse_query_prefix("p:get_balance"), (Some(QuickOpenKind::Procedure), "get_balance"));
        assert_eq!(parse_query_prefix("q:monthly_report"), (Some(QuickOpenKind::SavedQuery), "monthly_report"));
        assert_eq!(parse_query_prefix("/monthly_report"), (Some(QuickOpenKind::SavedQuery), "monthly_report"));
        assert_eq!(parse_query_prefix("h:select *"), (Some(QuickOpenKind::History), "select *"));
        assert_eq!(parse_query_prefix("?select *"), (Some(QuickOpenKind::History), "select *"));
        assert_eq!(parse_query_prefix("c:prod_db"), (Some(QuickOpenKind::Connection), "prod_db"));
        assert_eq!(parse_query_prefix("#prod_db"), (Some(QuickOpenKind::Connection), "prod_db"));
        assert_eq!(parse_query_prefix(">format"), (Some(QuickOpenKind::Command), "format"));
        assert_eq!(parse_query_prefix("users"), (None, "users"));
    }

    #[test]
    fn test_fuzzy_scoring() {
        let item = QuickOpenItem::new(
            "tbl_1".to_string(),
            "customer_orders".to_string(),
            "Table • ecommerce • Postgres".to_string(),
            QuickOpenKind::Table,
            Some(1),
            Some("Postgres".to_string()),
            Some("ecommerce".to_string()),
            Some("customer_orders".to_string()),
            None,
            None,
            None,
        );

        // Exact match
        let exact_score = score_fuzzy_match_fast("customer_orders", &item);
        assert!(exact_score.is_some());
        assert!(exact_score.unwrap() >= 10000);

        // Prefix match
        let prefix_score = score_fuzzy_match_fast("cust", &item);
        assert!(prefix_score.is_some());
        assert!(prefix_score.unwrap() >= 4000);

        // Word boundary / substring match
        let word_score = score_fuzzy_match_fast("orders", &item);
        assert!(word_score.is_some());
        assert!(word_score.unwrap() >= 3000);

        // Subsequence match (CamelCase / acronym)
        let sub_score = score_fuzzy_match_fast("cord", &item);
        assert!(sub_score.is_some());

        // Non-matching
        let no_score = score_fuzzy_match_fast("xyz999", &item);
        assert!(no_score.is_none());
    }

    #[test]
    fn test_quick_open_state_navigation_and_filtering() {
        let mut state = QuickOpenState::default();
        state.items = vec![
            QuickOpenItem::new(
                "1".to_string(),
                "users".to_string(),
                "Table".to_string(),
                QuickOpenKind::Table,
                Some(1),
                None,
                None,
                Some("users".to_string()),
                None,
                None,
                None,
            ),
            QuickOpenItem::new(
                "2".to_string(),
                "orders".to_string(),
                "Table".to_string(),
                QuickOpenKind::Table,
                Some(1),
                None,
                None,
                Some("orders".to_string()),
                None,
                None,
                None,
            ),
            QuickOpenItem::new(
                "3".to_string(),
                "user_view".to_string(),
                "View".to_string(),
                QuickOpenKind::View,
                Some(1),
                None,
                None,
                Some("user_view".to_string()),
                None,
                None,
                None,
            ),
        ];

        state.refilter();
        assert_eq!(state.filtered_items.len(), 3);

        // Query filter
        state.query = "user".to_string();
        state.refilter();
        assert_eq!(state.filtered_items.len(), 2);
        assert_eq!(state.selected_index, 0);

        // Navigation
        state.navigate(1);
        assert_eq!(state.selected_index, 1);
        state.navigate(1);
        assert_eq!(state.selected_index, 0); // circular wrapping
        state.navigate(-1);
        assert_eq!(state.selected_index, 1);

        // Category filter
        state.set_category(Some(QuickOpenKind::View));
        assert_eq!(state.filtered_items.len(), 1);
        assert_eq!(state.items[state.filtered_items[0].0].title, "user_view");
    }
}
