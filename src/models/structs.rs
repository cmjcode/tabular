use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, mpsc};

use crate::models::{self, enums::NodeType};

// ─── HTTP Client types ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub enum HttpMethod {
    #[default]
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    HEAD,
    OPTIONS,
}

impl HttpMethod {
    pub fn label(&self) -> &'static str {
        match self {
            HttpMethod::GET => "GET",
            HttpMethod::POST => "POST",
            HttpMethod::PUT => "PUT",
            HttpMethod::DELETE => "DELETE",
            HttpMethod::PATCH => "PATCH",
            HttpMethod::HEAD => "HEAD",
            HttpMethod::OPTIONS => "OPTIONS",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum HttpBodyType {
    // Form Data
    UrlEncoded,
    MultiPart,
    // Text Content
    GraphQL,
    Json,
    Xml,
    OtherText,
    // Other
    BinaryFile,
    #[default]
    NoBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum HttpAuthType {
    ApiKey,
    AwsSignature,
    BasicAuth,
    BearerToken,
    JwtBearer,
    OAuth1,
    OAuth2,
    NtlmAuth,
    InheritParent,
    #[default]
    NoAuth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum HttpRequestTab {
    #[default]
    Body,
    Params,
    Headers,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum HttpResponseTab {
    #[default]
    Body,
    Headers,
    Raw,
    /// Penjelasan response dari AI.
    Ai,
}

fn default_http_split_ratio() -> f32 {
    0.5
}

/// Target language/tool for the "Copy as Code" request-export dialog.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum CodeLang {
    #[default]
    Curl,
    Python,
    JavaScript,
    NodeJs,
    Go,
    Php,
    Rust,
}

impl CodeLang {
    pub fn label(&self) -> &'static str {
        match self {
            CodeLang::Curl => "cURL",
            CodeLang::Python => "Python",
            CodeLang::JavaScript => "JavaScript (fetch)",
            CodeLang::NodeJs => "Node.js (axios)",
            CodeLang::Go => "Go",
            CodeLang::Php => "PHP",
            CodeLang::Rust => "Rust (reqwest)",
        }
    }

    pub fn all() -> [CodeLang; 7] {
        [
            CodeLang::Curl,
            CodeLang::Python,
            CodeLang::JavaScript,
            CodeLang::NodeJs,
            CodeLang::Go,
            CodeLang::Php,
            CodeLang::Rust,
        ]
    }
}

/// Sent from the background thread back to the UI thread.
pub struct HttpClientResponse {
    pub status: u16,
    pub status_text: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
    pub time_ms: u128,
    pub size_bytes: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpClientState {
    pub url: String,
    pub method: HttpMethod,

    // Request config tabs
    pub active_tab: HttpRequestTab,

    // Body
    pub body_type: HttpBodyType,
    pub body_text: String,
    pub form_data: Vec<(String, String, bool)>, // (key, value, enabled)

    // Params
    pub params: Vec<(String, String, bool)>,

    // Headers
    pub headers: Vec<(String, String, bool)>,

    // Auth
    pub auth_type: HttpAuthType,
    pub bearer_token: String,
    pub basic_user: String,
    pub basic_pass: String,
    pub api_key_name: String,
    pub api_key_value: String,
    pub api_key_in_header: bool, // true = Header, false = Query Param

    // Response
    pub response_status: Option<u16>,
    pub response_status_text: String,
    pub response_body: String,
    pub response_headers: Vec<(String, String)>,
    pub response_time_ms: Option<u128>,
    pub response_size_bytes: Option<usize>,
    pub response_error: Option<String>,
    pub response_tab: HttpResponseTab,
    pub is_loading: bool,

    // ── Layout editor ────────────────────────────────────────────────────────
    /// Porsi panel request (0..1) terhadap panel response; bisa digeser user.
    #[serde(default = "default_http_split_ratio")]
    pub split_ratio: f32,
    /// true = request di atas, response di bawah.
    #[serde(default)]
    pub layout_vertical: bool,
    /// Wrap baris panjang pada body response.
    #[serde(default = "default_true")]
    pub response_wrap: bool,
    /// Teks pencarian di body response (transient).
    #[serde(skip)]
    pub response_search: String,
    /// Waktu mulai request yang sedang berjalan, untuk timer live.
    #[serde(skip)]
    pub request_started: Option<std::time::Instant>,
    /// Bantuan AI (prompt, jawaban tertunda, penjelasan). Tidak disimpan.
    #[serde(skip)]
    pub ai: crate::http_ai::HttpAiState,

    /// Channel receiver from background HTTP thread (Arc so Clone works).
    /// Skipped during serialization — recreated at runtime.
    #[serde(skip)]
    pub response_receiver: Option<Arc<Mutex<mpsc::Receiver<HttpClientResponse>>>>,

    // ── Collection / Saved-request sidebar ───────────────────────────────────
    /// All imported/saved workspaces. Skipped from JSON (loaded separately).
    #[serde(skip)]
    pub workspaces: Vec<crate::http_collection::HttpWorkspace>,

    /// UI state for the collection panel.
    pub collection_panel: crate::http_collection::CollectionPanelState,

    /// If this state comes from or was saved to a collection request, store its tracking metadata.
    #[serde(skip)]
    pub saved_request_id: Option<String>,
    #[serde(skip)]
    pub saved_workspace_id: Option<String>,
    #[serde(skip)]
    pub saved_folder_id: Option<String>,

    /// Transient flag: show the "Save Request" dialog.
    #[serde(skip)]
    pub show_save_dialog: bool,

    /// Name typed in the save-request dialog.
    #[serde(skip)]
    pub save_dialog_name: String,

    /// Transient flag: show the "Copy as Code" dialog.
    #[serde(skip)]
    pub show_code_dialog: bool,

    /// Language currently selected in the "Copy as Code" dialog.
    #[serde(skip)]
    pub code_dialog_lang: CodeLang,
}

impl Default for HttpClientState {
    fn default() -> Self {
        Self {
            url: String::new(),
            method: HttpMethod::GET,
            active_tab: HttpRequestTab::Body,
            body_type: HttpBodyType::NoBody,
            body_text: String::new(),
            form_data: vec![("".to_string(), "".to_string(), true)],
            params: vec![("".to_string(), "".to_string(), true)],
            headers: vec![("Accept".to_string(), "*/*".to_string(), true)],
            auth_type: HttpAuthType::NoAuth,
            bearer_token: String::new(),
            basic_user: String::new(),
            basic_pass: String::new(),
            api_key_name: String::new(),
            api_key_value: String::new(),
            api_key_in_header: true,
            response_status: None,
            response_status_text: String::new(),
            response_body: String::new(),
            response_headers: Vec::new(),
            response_time_ms: None,
            response_size_bytes: None,
            response_error: None,
            response_tab: HttpResponseTab::Body,
            is_loading: false,
            split_ratio: default_http_split_ratio(),
            layout_vertical: false,
            response_wrap: true,
            response_search: String::new(),
            request_started: None,
            ai: Default::default(),
            response_receiver: None,
            workspaces: Vec::new(),
            collection_panel: crate::http_collection::CollectionPanelState::default(),
            saved_request_id: None,
            saved_workspace_id: None,
            saved_folder_id: None,
            show_save_dialog: false,
            save_dialog_name: String::new(),
            show_code_dialog: false,
            code_dialog_lang: CodeLang::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RedisBrowserTypeFilter {
    #[default]
    All,
    String,
    Hash,
    List,
    Set,
    SortedSet,
    Stream,
    Other,
}

impl RedisBrowserTypeFilter {
    pub fn label(&self) -> &'static str {
        match self {
            RedisBrowserTypeFilter::All => "All Key Types",
            RedisBrowserTypeFilter::String => "Strings",
            RedisBrowserTypeFilter::Hash => "Hashes",
            RedisBrowserTypeFilter::List => "Lists",
            RedisBrowserTypeFilter::Set => "Sets",
            RedisBrowserTypeFilter::SortedSet => "Sorted Sets",
            RedisBrowserTypeFilter::Stream => "Streams",
            RedisBrowserTypeFilter::Other => "Other",
        }
    }

    pub fn matches_type(&self, key_type: &str) -> bool {
        match self {
            RedisBrowserTypeFilter::All => true,
            RedisBrowserTypeFilter::String => key_type.eq_ignore_ascii_case("string"),
            RedisBrowserTypeFilter::Hash => key_type.eq_ignore_ascii_case("hash"),
            RedisBrowserTypeFilter::List => key_type.eq_ignore_ascii_case("list"),
            RedisBrowserTypeFilter::Set => key_type.eq_ignore_ascii_case("set"),
            RedisBrowserTypeFilter::SortedSet => {
                key_type.eq_ignore_ascii_case("zset") || key_type.eq_ignore_ascii_case("sorted_set")
            }
            RedisBrowserTypeFilter::Stream => key_type.eq_ignore_ascii_case("stream"),
            RedisBrowserTypeFilter::Other => ![
                "string",
                "hash",
                "list",
                "set",
                "zset",
                "sorted_set",
                "stream",
            ]
            .iter()
            .any(|candidate| key_type.eq_ignore_ascii_case(candidate)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RedisBrowserKeyEntry {
    pub key_name: String,
    pub key_type: String,
    pub ttl_label: String,
    pub size_label: String,
}

#[derive(Clone, Debug, Default)]
pub struct RedisBrowserPreview {
    pub key_name: String,
    pub key_type: String,
    pub database_name: String,
    pub ttl_label: String,
    pub size_label: String,
    pub length_label: String,
    pub json_text: String,
}

#[derive(Clone, Debug, Default)]
pub struct RedisBrowserState {
    pub available_keyspaces: Vec<String>,
    pub keyspace_label: String,
    pub keys: Vec<RedisBrowserKeyEntry>,
    pub filter_text: String,
    pub type_filter: RedisBrowserTypeFilter,
    pub remote_search_in_progress: bool,
    pub last_remote_search: Option<String>,
    pub auto_refresh_enabled: bool,
    pub auto_refresh_interval_seconds: u32,
    pub auto_refresh_last_run: Option<std::time::Instant>,
    pub selected_key: Option<String>,
    pub selected_key_type: Option<String>,
    pub preview: Option<RedisBrowserPreview>,
    pub last_error: Option<String>,
    pub status_text: String,
}

#[derive(Clone)]
pub struct TreeNode {
    pub name: String,
    pub children: Vec<TreeNode>,
    pub is_expanded: bool,
    pub(crate) node_type: NodeType,
    pub connection_id: Option<i64>,    // For connection nodes
    pub is_loaded: bool,               // For tracking if tables/columns are loaded
    pub database_name: Option<String>, // For storing database context
    pub file_path: Option<String>,     // For query files
    pub table_name: Option<String>,    // For storing table context for subfolders/items
    pub query: Option<String>,         // For storing custom view queries
}

impl TreeNode {
    pub fn new(name: String, node_type: NodeType) -> Self {
        Self {
            name,
            children: Vec::new(),
            is_expanded: false,
            node_type,
            connection_id: None,
            is_loaded: true, // Regular nodes are always loaded
            database_name: None,
            file_path: None,
            table_name: None,
            query: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_children(name: String, node_type: NodeType, children: Vec<TreeNode>) -> Self {
        Self {
            name,
            children,
            is_expanded: false,
            node_type,
            connection_id: None,
            is_loaded: true,
            database_name: None,
            file_path: None,
            table_name: None,
            query: None,
        }
    }

    pub fn new_connection(name: String, connection_id: i64) -> Self {
        Self {
            name,
            children: Vec::new(),
            is_expanded: false,
            node_type: NodeType::Connection,
            connection_id: Some(connection_id),
            is_loaded: false, // Connection nodes need to load tables
            database_name: None,
            file_path: None,
            table_name: None,
            query: None,
        }
    }

    /// Recursively auto-expand all nested folders in the tree node hierarchy.
    pub fn expand_all_folders(&mut self) {
        if self.node_type.is_folder() {
            self.is_expanded = true;
        }
        for child in &mut self.children {
            child.expand_all_folders();
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForeignKey {
    pub constraint_name: String,
    pub table_name: String,
    pub column_name: String,
    pub referenced_table_name: String,
    pub referenced_column_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ForeignKeyRelation {
    pub source_col: String,
    pub target_table: String,
    pub target_col: String,
    pub target_schema: Option<String>,
}

impl ForeignKeyRelation {
    pub fn new(
        source_col: impl Into<String>,
        target_table: impl Into<String>,
        target_col: impl Into<String>,
        target_schema: Option<String>,
    ) -> Self {
        Self {
            source_col: source_col.into(),
            target_table: target_table.into(),
            target_col: target_col.into(),
            target_schema,
        }
    }
}

/// Zero-copy & memory-efficient cell representation for large payloads (JSON, BLOB, Text)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum CellValue {
    #[default]
    Null,
    Text(String),
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Json(String),
    Bytes(Arc<[u8]>),
}

/// Type alias for SQL values and query parameters
pub type SqlValue = CellValue;

impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CellValue::Null => write!(f, "NULL"),
            CellValue::Text(s) => write!(f, "{}", s),
            CellValue::Number(n) => write!(f, "{}", n),
            CellValue::Integer(i) => write!(f, "{}", i),
            CellValue::Boolean(b) => write!(f, "{}", b),
            CellValue::Json(j) => write!(f, "{}", j),
            CellValue::Bytes(b) => write!(f, "<BLOB {} bytes>", b.len()),
        }
    }
}

impl CellValue {
    pub fn is_null(&self) -> bool {
        matches!(self, CellValue::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            CellValue::Text(s) | CellValue::Json(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            CellValue::Null => "NULL".to_string(),
            CellValue::Text(s) => s.clone(),
            CellValue::Number(n) => n.to_string(),
            CellValue::Integer(i) => i.to_string(),
            CellValue::Boolean(b) => b.to_string(),
            CellValue::Json(j) => j.clone(),
            CellValue::Bytes(b) => format!("<BLOB {} bytes>", b.len()),
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            CellValue::Bytes(b) => Some(b.as_ref()),
            CellValue::Text(s) | CellValue::Json(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        if s.eq_ignore_ascii_case("null") {
            CellValue::Null
        } else {
            CellValue::Text(s.to_string())
        }
    }
}

impl From<String> for CellValue {
    fn from(s: String) -> Self {
        CellValue::Text(s)
    }
}

impl From<&str> for CellValue {
    fn from(s: &str) -> Self {
        CellValue::Text(s.to_string())
    }
}

impl From<i64> for CellValue {
    fn from(i: i64) -> Self {
        CellValue::Integer(i)
    }
}

impl From<i32> for CellValue {
    fn from(i: i32) -> Self {
        CellValue::Integer(i as i64)
    }
}

impl From<f64> for CellValue {
    fn from(f: f64) -> Self {
        CellValue::Number(f)
    }
}

impl From<bool> for CellValue {
    fn from(b: bool) -> Self {
        CellValue::Boolean(b)
    }
}

impl From<Vec<u8>> for CellValue {
    fn from(v: Vec<u8>) -> Self {
        CellValue::Bytes(Arc::from(v.into_boxed_slice()))
    }
}

impl From<Arc<[u8]>> for CellValue {
    fn from(b: Arc<[u8]>) -> Self {
        CellValue::Bytes(b)
    }
}

impl From<Option<String>> for CellValue {
    fn from(opt: Option<String>) -> Self {
        match opt {
            Some(s) => CellValue::Text(s),
            None => CellValue::Null,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomView {
    pub name: String,
    pub query: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramGroup {
    pub id: String,
    pub title: String,
    #[serde(with = "serde_color")]
    pub color: eframe::egui::Color32,
    #[serde(default)]
    #[serde(with = "serde_option_pos2")]
    pub manual_pos: Option<eframe::egui::Pos2>, // For empty groups or manual overriding
                                                // nodes are linked by group_id in DiagramNode
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramNode {
    pub id: String, // usually table name
    pub title: String,
    #[serde(with = "serde_pos2")]
    pub pos: eframe::egui::Pos2,
    #[serde(with = "serde_vec2")]
    pub size: eframe::egui::Vec2,
    pub columns: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>, // FKs originating from this table
    #[serde(default)]
    pub group_ids: Vec<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    /// Tipe/PK/nullable per kolom. Kosong untuk file diagram lama atau engine
    /// yang belum mendukung; `columns` tetap sumber urutan nama kolom.
    #[serde(default)]
    pub column_meta: Vec<DiagramColumn>,
    /// Tabel yang tidak ada di database (mis. hasil impor Mermaid). Tidak
    /// dibuang saat diagram disinkronkan ulang dengan skema database.
    #[serde(default)]
    pub detached: bool,
    /// Nama database asal tabel ini (opsional untuk backward compatibility).
    #[serde(default)]
    pub database_name: Option<String>,
    /// ID koneksi asal tabel ini.
    #[serde(default)]
    pub connection_id: Option<i64>,
    /// Nama label koneksi asal (misal "Production Postgres").
    #[serde(default)]
    pub connection_name: Option<String>,
}

impl Default for DiagramNode {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            pos: eframe::egui::pos2(0.0, 0.0),
            size: eframe::egui::vec2(150.0, 100.0),
            columns: Vec::new(),
            foreign_keys: Vec::new(),
            group_ids: Vec::new(),
            group_id: None,
            column_meta: Vec::new(),
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        }
    }
}

impl DiagramNode {
    /// Metadata kolom berdasarkan nama, bila tersedia.
    pub fn column_info(&self, name: &str) -> Option<&DiagramColumn> {
        self.column_meta.iter().find(|c| c.name == name)
    }

    /// Kolom ini sumber foreign key dari tabel ini.
    pub fn is_fk_column(&self, name: &str) -> bool {
        self.foreign_keys
            .iter()
            .any(|fk| fk.column_name == name && fk.table_name == self.id)
    }

    /// Cek apakah tabel ini tergabung dalam group dengan ID tertentu.
    pub fn is_in_group(&self, group_id: &str) -> bool {
        self.group_ids.iter().any(|g| g == group_id) || self.group_id.as_deref() == Some(group_id)
    }

    /// Tambahkan tabel ke suatu group bila belum ada.
    pub fn add_to_group(&mut self, group_id: String) {
        self.ensure_groups_migrated();
        if !self.group_ids.contains(&group_id) {
            self.group_ids.push(group_id);
        }
        self.group_id = self.group_ids.first().cloned();
    }

    /// Hapus tabel dari suatu group.
    pub fn remove_from_group(&mut self, group_id: &str) {
        self.ensure_groups_migrated();
        self.group_ids.retain(|g| g != group_id);
        self.group_id = self.group_ids.first().cloned();
    }

    /// Migrasikan `group_id` tunggal lama ke `group_ids` bila perlu.
    pub fn ensure_groups_migrated(&mut self) {
        if self.group_ids.is_empty() {
            if let Some(gid) = &self.group_id {
                self.group_ids.push(gid.clone());
            }
        } else if self.group_id.is_none() {
            self.group_id = self.group_ids.first().cloned();
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramColumn {
    pub name: String,
    #[serde(default)]
    pub type_name: String,
    #[serde(default)]
    pub is_pk: bool,
    #[serde(default = "default_true")]
    pub nullable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramEdge {
    pub source: String,
    pub target: String,
    pub label: String,
}

/// Asal relasi yang tidak berasal dari foreign key database.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelationOrigin {
    /// Disarankan dari kemiripan nama kolom lalu diterima user.
    Inferred,
    /// Dibuat manual (Shift+klik kolom).
    Manual,
    /// Berasal dari impor Mermaid.
    Imported,
}

/// Relasi `child.child_column -> parent.parent_column` tanpa FK di database.
/// Disimpan di file diagram, jadi tetap ada saat diagram dibuka ulang.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualRelation {
    pub child: String,
    pub child_column: String,
    pub parent: String,
    pub parent_column: String,
    pub origin: RelationOrigin,
}

/// Status materialisasi sebuah link database (runtime saja).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LinkStatus {
    /// Belum dimuat sejak diagram dibuka.
    #[default]
    Pending,
    /// Isi kontainer sudah dimuat dari diagram sumber.
    Loaded,
    /// Gagal dimuat (koneksi tidak ditemukan / offline). Relasi lintas
    /// database ke link ini dibiarkan dorman, tidak dibuang.
    Failed(String),
}

/// Referensi ke diagram database lain yang ditampilkan sebagai kontainer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkedDatabase {
    /// Namespace stabil untuk id node/group/relasi (`{link_id}::{table}`).
    /// Tidak bergantung pada `connection_id` supaya relasi lintas database
    /// tetap valid saat diagram dibuka di mesin lain.
    pub link_id: String,
    /// ID koneksi lokal; hanya valid di mesin pembuatnya.
    #[serde(default)]
    pub connection_id: Option<i64>,
    /// Nama koneksi, dipakai sebagai fallback resolusi antar mesin.
    #[serde(default)]
    pub connection_name: String,
    pub database_name: String,
    /// Posisi pojok kiri atas kontainer di kanvas host.
    #[serde(with = "serde_pos2")]
    pub offset: eframe::egui::Pos2,
    #[serde(with = "serde_color")]
    pub color: eframe::egui::Color32,
    #[serde(skip)]
    pub status: LinkStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagramState {
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    pub groups: Vec<DiagramGroup>,
    #[serde(with = "serde_vec2")]
    pub pan: eframe::egui::Vec2,
    pub zoom: f32,
    #[serde(skip)]
    pub dragging_node: Option<String>,
    #[serde(skip)]
    pub dragging_offset: eframe::egui::Vec2,
    #[serde(skip)]
    pub last_mouse_pos: Option<eframe::egui::Pos2>,
    pub is_centered: bool,
    #[serde(skip)]
    pub save_requested: bool,
    #[serde(skip)]
    pub renaming_group: Option<String>,
    #[serde(skip)]
    pub selected_edge: Option<(String, String)>, // (source_id, target_id)
    #[serde(skip)]
    pub selected_column: Option<(String, String)>, // (table_name, column_name)
    #[serde(skip)]
    pub add_group_popup: Option<eframe::egui::Pos2>, // Popup for adding group
    #[serde(skip)]
    pub new_group_buffer: String,
    #[serde(skip)]
    pub search_query: String,
    #[serde(skip)]
    pub show_search: bool,
    #[serde(skip, default = "default_true")]
    pub search_tables: bool,
    #[serde(skip, default = "default_true")]
    pub search_columns: bool,
    #[serde(skip, default = "default_true")]
    pub search_groups: bool,
    /// Tampilkan grid latar.
    #[serde(default = "default_true")]
    pub show_grid: bool,
    /// Mencegah tabel tumpang tindih (anti-overlap / collision avoidance).
    #[serde(default = "default_true")]
    pub prevent_overlap: bool,
    /// Tampilkan garis relasi / link kolom antar tabel.
    #[serde(default = "default_true")]
    pub show_relations: bool,
    /// Relasi tanpa FK database (disarankan, manual, atau hasil impor).
    #[serde(default)]
    pub virtual_relations: Vec<VirtualRelation>,
    #[serde(skip)]
    pub selected_virtual: Option<usize>,
    /// Jendela saran relasi yang sedang terbuka: (saran, dicentang).
    #[serde(skip)]
    pub relation_suggestions: Option<Vec<(crate::diagram_relations::RelationSuggestion, bool)>>,
    /// Judul / target kolom pencarian relasi (misal "devices.imei" atau "imei").
    #[serde(skip)]
    pub relation_suggestions_title: Option<String>,
    /// Teks input pencarian relasi berdasarkan nama kolom.
    #[serde(skip)]
    pub relation_column_search_query: String,
    /// Mode navigasi Hand Tool (geser kanvas bebas tanpa memindahkan tabel).
    #[serde(skip)]
    pub hand_tool: bool,
    /// Database lain yang di-link ke diagram ini. Hanya referensinya yang
    /// disimpan; isi kontainernya dimaterialisasi ulang dari diagram sumber.
    #[serde(default)]
    pub linked_databases: Vec<LinkedDatabase>,
    /// Relasi virtual bawaan diagram sumber (read-only, tidak disimpan).
    /// Relasi yang dibuat di diagram gabungan tetap di `virtual_relations`.
    #[serde(skip)]
    pub linked_relations: Vec<VirtualRelation>,
    /// Modal dialog "Link Database" yang sedang aktif.
    #[serde(skip)]
    pub show_link_modal: bool,
    /// ID koneksi yang dipilih dalam modal dialog.
    #[serde(skip)]
    pub link_modal_conn: Option<i64>,
    /// Nama database yang dipilih dalam modal dialog.
    #[serde(skip)]
    pub link_modal_db: String,
    /// Bila `Some`, modal mengganti koneksi link yang sudah ada (relink)
    /// sehingga id node dan relasi lintas database tetap utuh.
    #[serde(skip)]
    pub link_modal_relink: Option<String>,
    /// Daftar database koneksi terpilih di modal (dimuat sekali per koneksi).
    #[serde(skip)]
    pub link_modal_db_options: Vec<String>,
    /// Koneksi asal `link_modal_db_options`; beda dengan koneksi terpilih
    /// berarti daftar perlu dimuat ulang.
    #[serde(skip)]
    pub link_modal_db_options_for: Option<i64>,
    /// Paksa muat ulang daftar database langsung dari server.
    #[serde(skip)]
    pub link_modal_db_reload: bool,
    /// Judul kustom dokumen diagram (opsional).
    #[serde(default)]
    pub diagram_title: Option<String>,
    /// Remote ID jika diagram ini disinkronkan ke server.
    #[serde(default)]
    pub remote_id: Option<String>,
    /// Skema live sedang diambil di background; tampilan masih dari cache.
    #[serde(skip)]
    pub schema_syncing: bool,
    /// Sidik layout saat tab dibuka. Beda dengan sidik terkini berarti user
    /// sudah mengedit, jadi layout bersama dari `diagram_by_tabular` tidak
    /// boleh menimpanya.
    #[serde(skip)]
    pub layout_baseline: Option<u64>,
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            pan: eframe::egui::Vec2::ZERO,
            zoom: 1.0,
            dragging_node: None,
            dragging_offset: eframe::egui::Vec2::ZERO,
            last_mouse_pos: None,
            is_centered: false,
            save_requested: false,
            renaming_group: None,
            selected_edge: None,
            selected_column: None,
            add_group_popup: None,
            new_group_buffer: String::new(),
            search_query: String::new(),
            show_search: false,
            search_tables: true,
            search_columns: true,
            search_groups: true,
            show_grid: true,
            prevent_overlap: true,
            show_relations: true,
            virtual_relations: Vec::new(),
            selected_virtual: None,
            relation_suggestions: None,
            relation_suggestions_title: None,
            relation_column_search_query: String::new(),
            hand_tool: false,
            linked_databases: Vec::new(),
            linked_relations: Vec::new(),
            show_link_modal: false,
            link_modal_conn: None,
            link_modal_db: String::new(),
            link_modal_relink: None,
            link_modal_db_options: Vec::new(),
            link_modal_db_options_for: None,
            link_modal_db_reload: false,
            diagram_title: None,
            remote_id: None,
            schema_syncing: false,
            layout_baseline: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColumnMetadata {
    pub name: String,
    pub type_name: String,
    pub table_name: Option<String>, // Source table name if available
    pub original_name: Option<String>, // Original column name if aliased
    pub is_primary_key: bool,
}

/// Jenis pernyataan SQL (SELECT, INSERT, UPDATE, DELETE, DDL, dll.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StatementType {
    #[default]
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
    Transaction,
    Show,
    Other,
}

impl StatementType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::Ddl => "DDL",
            Self::Transaction => "TRANSACTION",
            Self::Show => "SHOW",
            Self::Other => "QUERY",
        }
    }

    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Insert | Self::Update | Self::Delete | Self::Ddl)
    }

    pub fn is_select(&self) -> bool {
        matches!(self, Self::Select)
    }

    /// Deteksi jenis pernyataan SQL dari string SQL dengan mengabaikan komentar dan spasi
    pub fn from_sql(sql: &str) -> Self {
        let trimmed = sql.trim();
        let bytes = trimmed.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        // Lewati komentar SQL dan spasi awal
        while i < len {
            // Lewati spasi
            while i < len && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r' || bytes[i] == b'\n') {
                i += 1;
            }
            if i >= len {
                break;
            }

            // Lewati komentar satu baris -- atau #
            if (i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-') || bytes[i] == b'#' {
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }

            // Lewati komentar blok /* ... */
            if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                }
                continue;
            }

            break;
        }

        if i >= len {
            return Self::Other;
        }

        // Ambil kata kunci pertama (alfanumerik)
        let word_start = i;
        while i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            i += 1;
        }
        let word = &trimmed[word_start..i];

        if word.eq_ignore_ascii_case("select") || word.eq_ignore_ascii_case("with") {
            Self::Select
        } else if word.eq_ignore_ascii_case("insert") || word.eq_ignore_ascii_case("upsert") || word.eq_ignore_ascii_case("replace") {
            Self::Insert
        } else if word.eq_ignore_ascii_case("update") {
            Self::Update
        } else if word.eq_ignore_ascii_case("delete") {
            Self::Delete
        } else if word.eq_ignore_ascii_case("create")
            || word.eq_ignore_ascii_case("alter")
            || word.eq_ignore_ascii_case("drop")
            || word.eq_ignore_ascii_case("truncate")
            || word.eq_ignore_ascii_case("rename")
        {
            Self::Ddl
        } else if word.eq_ignore_ascii_case("begin")
            || word.eq_ignore_ascii_case("commit")
            || word.eq_ignore_ascii_case("rollback")
            || word.eq_ignore_ascii_case("start")
            || word.eq_ignore_ascii_case("savepoint")
        {
            Self::Transaction
        } else if word.eq_ignore_ascii_case("show")
            || word.eq_ignore_ascii_case("describe")
            || word.eq_ignore_ascii_case("desc")
            || word.eq_ignore_ascii_case("explain")
        {
            Self::Show
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResult {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub all_rows: Vec<Vec<String>>,
    pub table_name: String,
    pub column_metadata: Option<Vec<ColumnMetadata>>, // Metadata for columns (table source, etc.)
    pub current_page: usize,
    pub page_size: usize,
    pub total_rows: usize,
    pub query_message: String,
    pub query_message_is_error: bool,
    pub execution_time_ms: u128,
    pub explain_plan_json: Option<String>,
    #[serde(default)]
    pub pinned_columns: HashSet<String>,
    #[serde(default)]
    pub executed_sql: String,
    #[serde(default)]
    pub statement_type: StatementType,
    #[serde(default)]
    pub affected_rows: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct QueryTab {
    /// Identitas tab yang stabil. Berbeda dengan index di `query_tabs`, id ini
    /// tidak berubah saat tab diurutkan ulang atau tab lain ditutup, sehingga
    /// hasil query async bisa dikembalikan ke tab yang menjalankannya.
    pub id: usize,
    pub title: String,
    pub content: String,
    pub file_path: Option<String>,
    pub is_saved: bool,
    pub is_modified: bool,
    pub connection_id: Option<i64>, // Each tab can have its own database connection
    pub database_name: Option<String>, // Each tab can have its own database selection
    pub schema_name: Option<String>, // Active schema / search_path
    pub has_executed_query: bool,   // Track if this tab has ever executed a query
    // NEW: per-tab result state so switching tabs restores its own data
    pub result_headers: Vec<String>,
    pub result_rows: Vec<Vec<String>>, // current page (or all rows if client side)
    pub result_all_rows: Vec<Vec<String>>, // full dataset for client pagination
    pub result_table_name: String,     // caption/status e.g. Table: ... or Query Results
    pub result_column_metadata: Option<Vec<ColumnMetadata>>, // Metadata for result columns

    // MULTI-RESULT SUPPORT
    pub results: Vec<QueryResult>,
    pub active_result_index: usize,

    pub is_table_browse_mode: bool, // was this produced by table browse
    pub current_page: usize,
    pub page_size: usize,
    pub total_rows: usize,
    pub base_query: String, // Store the base query (without LIMIT/OFFSET) for pagination
    // DBA quick view special post-processing mode (Replication Status, Master Status, etc.)
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub object_ddl: Option<String>, // Optional DDL (e.g., ALTER VIEW) for browsed objects
    pub explain_plan_json: Option<String>, // Parsed/raw EXPLAIN plan output JSON
    // Query execution message (similar to TablePlus message tab)
    pub query_message: String,        // Message text (success/error)
    pub query_message_is_error: bool, // Whether the message is an error or success

    // Diagram state for "Diagrams" tab
    pub diagram_state: Option<DiagramState>,
    pub should_run_on_open: bool,

    // HTTP client state (Some(_) means this tab is an HTTP client)
    pub http_client_state: Option<HttpClientState>,
    pub redis_browser_state: Option<RedisBrowserState>,
    // DBA Process & Lock monitor state
    pub dba_monitor_state: Option<DbaMonitorState>,
    // User & Privileges Manager state
    pub user_manager_state: Option<crate::user_manager::UserManagerState>,

    // Manual-commit (transaction) mode — see connection/session.rs
    pub tx_mode: bool,
    pub tx_active: bool,
    pub session: Option<crate::connection::session::SessionHandle>,
    pub pinned_columns: HashSet<String>,
    pub is_pinned: bool,
    pub last_executed_sql: String,
    pub last_statement_type: StatementType,
    pub last_affected_rows: Option<usize>,
}

// ─── AI Assistant chat ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AiChatRole {
    #[default]
    User,
    Assistant,
}

pub use crate::agent::harness::{ProgressStatus, ProgressStep};

/// Satu gelembung di transkrip panel AI.
#[derive(Clone, Debug, Default)]
pub struct AiChatMessage {
    pub role: AiChatRole,
    /// Markdown mentah dari model (blok live edit ikut tampil di sini).
    pub text: String,
    /// Masih menerima delta dari backend.
    pub streaming: bool,
    /// Nama tool yang dipanggil agent, untuk indikator aktivitas.
    pub tool_activity: Vec<String>,
    /// Edit editor yang dihasilkan pesan ini (Apply / Revert).
    pub edits: Vec<crate::agent::live_edit::LiveEditRecord>,
    pub error: Option<String>,
    /// Ringkasan token/biaya dari backend, bila ada.
    pub usage: Option<String>,
    /// Tahapan kemajuan / aktivitas yang dijalankan agent pada giliran ini.
    pub progress_steps: Vec<crate::agent::harness::ProgressStep>,
    /// Nama backend/agent yang menjawab; hanya diisi untuk role Assistant.
    pub agent_label: Option<String>,
}

/// Sesi CLI aktif untuk melanjutkan percakapan (agy --conversation / claude --resume).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub kind: crate::config::CliAgentKind,
    pub id: String,
}

#[derive(Default)]
pub struct McpStatus {
    /// None = belum diperiksa; Some(true) = MCP Tabular terdaftar di CLI global.
    pub registered: Option<bool>,
    pub message: Option<String>,
    pub receiver: Option<std::sync::mpsc::Receiver<Result<bool, String>>>,
}


/// Cache badge skema di header panel AI. Sumbernya query SQLite yang blocking,
/// jadi hanya dihitung ulang saat koneksi/database berubah atau cache kedaluwarsa.
#[derive(Clone, Debug)]
pub struct AiSchemaBadge {
    /// (connection id, nama database tab aktif)
    pub key: (Option<i64>, String),
    pub table_count: usize,
    /// Potongan konteks skema untuk tooltip.
    pub preview: String,
    pub computed_at: std::time::Instant,
}

/// Blok live edit yang sedang di-stream ke sebuah tab.
#[derive(Clone, Debug)]
pub struct ActiveLiveEdit {
    pub tab_id: usize,
    pub tab_title: String,
    pub mode: crate::agent::live_edit::LiveEditMode,
    /// Isi tab saat blok dimulai (untuk Revert dan mode selection/append).
    pub original: String,
    /// Seleksi (byte) saat blok dimulai; hanya berarti untuk tab aktif.
    pub selection: (usize, usize),
    /// Isi terakhir yang kami tulis; bila tab berubah di luar itu, edit dibatalkan.
    pub last_applied: String,
    /// Edit tidak lagi ditulis ke tab (auto-apply mati, tab hilang, atau diubah user).
    pub aborted: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProcessInfo {
    pub pid: i64,
    pub user: String,
    pub db: String,
    pub host: String,
    pub state: String,
    pub duration_secs: f64,
    pub wait_event: Option<String>,
    pub query: String,
    pub is_blocking: bool,
    pub blocked_by: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct DbaMonitorState {
    pub processes: Vec<ProcessInfo>,
    pub is_loading: bool,
    pub auto_refresh: bool,
    pub refresh_interval_secs: u64,
    pub last_refreshed: Option<std::time::Instant>,
    pub selected_tab: models::enums::DbaMonitorTab,
    pub filter_state: models::enums::ProcessStateFilter,
    pub search_text: String,
    pub selected_pid: Option<i64>,
    pub confirm_action: Option<(i64, bool)>, // (pid, is_cancel_only)
    pub status_message: Option<(String, bool)>, // (message, is_error)
}

impl Default for DbaMonitorState {
    fn default() -> Self {
        Self {
            processes: Vec::new(),
            is_loading: false,
            auto_refresh: true,
            refresh_interval_secs: 3,
            last_refreshed: None,
            selected_tab: models::enums::DbaMonitorTab::Processlist,
            filter_state: models::enums::ProcessStateFilter::All,
            search_text: String::new(),
            selected_pid: None,
            confirm_action: None,
            status_message: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorColorTheme {
    GithubDark,
    GithubLight,
    Gruvbox,
}

#[derive(Clone)]
pub struct AdvancedEditor {
    pub show_line_numbers: bool,
    pub theme: EditorColorTheme,
    pub font_size: f32,
    #[allow(dead_code)]
    pub tab_size: usize,
    #[allow(dead_code)]
    pub auto_indent: bool,
    #[allow(dead_code)]
    pub show_whitespace: bool,
    pub word_wrap: bool,
    // Number of visible rows the editor should aim to display; set dynamically to fill height
    pub desired_rows: usize,
    pub find_text: String,
    pub replace_text: String,
    pub show_find_replace: bool,
    pub show_replace_row: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
    pub in_selection: bool,
    pub selection_range: Option<(usize, usize)>,
    pub current_match_index: usize,
    pub match_count: usize,
    pub regex_error: Option<String>,
    pub focus_find_input: bool,
    pub keyword_casing: crate::models::enums::KeywordCasing,
    pub highlight_active_line: bool,
}

impl Default for AdvancedEditor {
    fn default() -> Self {
        Self {
            show_line_numbers: true,
            theme: EditorColorTheme::GithubDark,
            font_size: 14.0,
            tab_size: 4,
            auto_indent: true,
            show_whitespace: false,
            word_wrap: false,
            desired_rows: 25,
            find_text: String::new(),
            replace_text: String::new(),
            show_find_replace: false,
            show_replace_row: false,
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            in_selection: false,
            selection_range: None,
            current_match_index: 0,
            match_count: 0,
            regex_error: None,
            focus_find_input: false,
            keyword_casing: crate::models::enums::KeywordCasing::default(),
            highlight_active_line: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: Option<i64>,
    pub query: String,
    pub connection_id: i64,
    pub connection_name: String,
    pub executed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: Option<i64>,
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub connection_type: models::enums::DatabaseType,
    pub folder: Option<String>, // Custom folder name
    pub ssh_enabled: bool,
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_username: String,
    pub ssh_auth_method: models::enums::SshAuthMethod,
    pub ssh_private_key: String,
    pub ssh_password: String,
    pub ssh_accept_unknown_host_keys: bool,
    #[serde(default)]
    pub ssh_jump_host: String,
    #[serde(default)]
    pub ssl_enabled: bool,
    #[serde(default)]
    pub ssl_ca_cert: String,
    #[serde(default)]
    pub ssl_client_cert: String,
    #[serde(default)]
    pub ssl_client_key: String,
    #[serde(default)]
    pub ssl_key_passphrase: String,
    #[serde(default = "default_true")]
    pub ssl_verify_server: bool,
    #[serde(default)]
    pub custom_views: Vec<CustomView>,
    #[serde(default)]
    pub replication_master_id: Option<i64>,
}

fn default_true() -> bool {
    true
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            host: "localhost".to_string(),
            port: "3306".to_string(),
            username: String::new(),
            password: String::new(),
            database: String::new(),
            connection_type: models::enums::DatabaseType::MySQL,
            folder: None, // No custom folder by default
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: "22".to_string(),
            ssh_username: String::new(),
            ssh_auth_method: models::enums::SshAuthMethod::Key,
            ssh_private_key: String::new(),
            ssh_password: String::new(),
            ssh_accept_unknown_host_keys: false,
            ssh_jump_host: String::new(),
            ssl_enabled: false,
            ssl_ca_cert: String::new(),
            ssl_client_cert: String::new(),
            ssl_client_key: String::new(),
            ssl_key_passphrase: String::new(),
            ssl_verify_server: true,
            custom_views: Vec::new(),
            replication_master_id: None,
        }
    }
}

impl ConnectionConfig {
    pub fn display_name(&self) -> String {
        match &self.folder {
            Some(folder) if !folder.trim().is_empty() => {
                format!("{}/{}", folder.trim(), self.name)
            }
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExpansionRequest {
    pub node_type: models::enums::NodeType,
    pub connection_id: i64,
    pub database_name: Option<String>,
    /// When true, the relevant table_cache entries are deleted before reloading,
    /// forcing a live fetch from the database server (used by "Refresh Tables").
    pub force_clear_cache: bool,
}

// UI state for Create/Edit Index modal
#[derive(Clone, Debug, PartialEq)]
pub enum IndexDialogMode {
    Create,
    Edit,
}

#[derive(Clone, Debug)]
pub struct IndexDialogState {
    pub mode: IndexDialogMode,
    pub connection_id: i64,
    pub database_name: Option<String>, // For PG schema or MsSQL db context
    pub table_name: String,
    pub existing_index_name: Option<String>,
    pub index_name: String,
    pub columns: String, // comma-separated list the user can edit
    pub unique: bool,
    pub method: Option<String>, // e.g., btree/hash for PG, BTREE/HASH for MySQL
    pub db_type: crate::models::enums::DatabaseType,
}

// Bottom panel view mode for a selected table
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TableBottomView {
    #[default]
    Data,
    Structure,
    Query,
    Messages,
    Explain,
}

// Simplified column info for Structure tab (can be extended later per RDBMS)
#[derive(Clone, Debug, Default)]
pub struct ColumnStructInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: Option<bool>,
    pub default_value: Option<String>,
    pub extra: Option<String>,
    pub comment: Option<String>,
}

// Simplified index info shown in Structure -> Indexes
#[derive(Clone, Debug, Default)]
pub struct IndexStructInfo {
    pub name: String,
    pub method: Option<String>, // algorithm / type (btree, hash, etc.)
    pub unique: bool,
    pub columns: Vec<String>,
}

// Simplified partition info shown in Structure -> Partitions
#[derive(Clone, Debug, Default)]
pub struct PartitionStructInfo {
    pub name: String,
    pub partition_type: Option<String>, // RANGE, LIST, HASH, etc.
    pub partition_expression: Option<String>, // PARTITION BY expression
    pub subpartition_type: Option<String>, // For composite partitioning
}

// Sub view inside Structure (so kita tidak render dua tabel sekaligus)
#[derive(Clone, Debug, PartialEq, Default)]
pub enum StructureSubView {
    #[default]
    Columns,
    Indexes,
}

// Spreadsheet editing structures
#[derive(Clone, Debug, PartialEq)]
pub enum SpreadsheetOperationType {
    Update,
    Insert,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellEditOperation {
    Update {
        row_index: usize,
        col_index: usize,
        old_value: String,
        new_value: String,
    },
    InsertRow {
        row_index: usize,
        values: Vec<String>,
    },
    DeleteRow {
        row_index: usize,
        values: Vec<String>, // Store original values for undo
    },
}

#[derive(Clone, Debug)]
pub struct SpreadsheetOperation {
    pub operation_type: SpreadsheetOperationType,
    pub table_name: String,
    pub row_index: usize,
    pub column_name: String,
    pub old_value: String,
    pub new_value: String,
    pub primary_key_values: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct SpreadsheetState {
    pub editing_cell: Option<(usize, usize)>, // (row, col) being edited
    pub cell_edit_text: String,               // Text being edited in the cell
    pub pending_operations: Vec<CellEditOperation>, // Unsaved changes
    pub is_dirty: bool,                       // Whether there are unsaved changes
    pub primary_key_columns: Vec<String>,     // Primary key column names for generating SQL
    pub enum_options: Option<Vec<String>>,    // If editing an ENUM, available options
}

#[derive(Clone, Debug)]
pub struct TableColumnDefinition {
    pub name: String,
    pub data_type: String,
    pub allow_null: bool,
    pub default_value: String,
    pub is_primary_key: bool,
}

impl TableColumnDefinition {
    pub fn blank(index: usize) -> Self {
        let base_name = if index == 0 {
            "id".to_string()
        } else {
            format!("column_{}", index + 1)
        };
        Self {
            name: base_name,
            data_type: String::new(),
            allow_null: true,
            default_value: String::new(),
            is_primary_key: index == 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TableIndexDefinition {
    pub name: String,
    pub columns: String,
    pub unique: bool,
}

impl TableIndexDefinition {
    pub fn blank(index: usize) -> Self {
        Self {
            name: format!("idx_{}", index + 1),
            columns: String::new(),
            unique: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateTableWizardStep {
    Basics,
    Columns,
    Indexes,
    Review,
}

impl CreateTableWizardStep {
    pub fn all_steps() -> [Self; 4] {
        [Self::Basics, Self::Columns, Self::Indexes, Self::Review]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Basics => "Basics",
            Self::Columns => "Columns",
            Self::Indexes => "Indexes",
            Self::Review => "Review",
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Basics => Some(Self::Columns),
            Self::Columns => Some(Self::Indexes),
            Self::Indexes => Some(Self::Review),
            Self::Review => None,
        }
    }

    pub fn previous(self) -> Option<Self> {
        match self {
            Self::Basics => None,
            Self::Columns => Some(Self::Basics),
            Self::Indexes => Some(Self::Columns),
            Self::Review => Some(Self::Indexes),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateTableWizardState {
    pub connection_id: i64,
    pub db_type: models::enums::DatabaseType,
    pub database_name: Option<String>,
    pub table_name: String,
    pub columns: Vec<TableColumnDefinition>,
    pub indexes: Vec<TableIndexDefinition>,
    pub current_step: CreateTableWizardStep,
}

impl CreateTableWizardState {
    pub fn new(
        connection_id: i64,
        db_type: models::enums::DatabaseType,
        database_name: Option<String>,
    ) -> Self {
        let mut first_column = TableColumnDefinition::blank(0);
        first_column.data_type = match db_type {
            models::enums::DatabaseType::PostgreSQL => "SERIAL".to_string(),
            models::enums::DatabaseType::SQLite => "INTEGER".to_string(),
            models::enums::DatabaseType::MySQL => "INT".to_string(),
            models::enums::DatabaseType::MsSQL => "INT".to_string(),
            _ => String::new(),
        };
        first_column.allow_null = false;
        first_column.is_primary_key = true;

        Self {
            connection_id,
            db_type,
            database_name,
            table_name: String::new(),
            columns: vec![first_column],
            indexes: Vec::new(),
            current_step: CreateTableWizardStep::Basics,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplicationDialogState {
    pub target_connection_id: i64,
    pub source_connection_id: Option<i64>,
    pub error: Option<String>,
    pub is_executing: bool,
    // Manual credentials override
    pub replication_user: String,
    pub replication_password: String,
}

impl ReplicationDialogState {
    pub fn new(target_connection_id: i64) -> Self {
        Self {
            target_connection_id,
            source_connection_id: None,
            error: None,
            is_executing: false,
            replication_user: String::new(),
            replication_password: String::new(),
        }
    }
}

/// Type alias for the complex tuple returned by render_tree_node_with_table_expansion
pub type RenderTreeNodeResult = (
    Option<models::structs::ExpansionRequest>,
    Option<(usize, i64, String)>,
    Option<i64>,
    Option<(i64, String, models::enums::NodeType, Option<String>)>,
    Option<i64>,
    Option<(String, String, String)>,
    Option<String>,
    Option<String>,
    Option<(i64, String)>,
    Option<(i64, models::enums::NodeType)>,
    Option<(i64, String, Option<String>, Option<String>)>,
    Option<(i64, Option<String>, Option<String>)>,
    // New: request to open Structure view for Alter Table (connection_id, database, table_name)
    Option<(i64, Option<String>, String)>,
    // Request to open Add Replication dialog (connection_id)
    Option<i64>,
    // New: request to drop a MongoDB collection (connection_id, database_name, collection_name)
    Option<(i64, String, String)>,
    // New: request to drop a table (connection_id, database_name, table_name, stmt)
    Option<(i64, String, String, String)>,
    // New: request to open Create Table wizard (connection_id, optional database/schema)
    Option<(i64, Option<String>)>,
    // New: request to open ALTER script for stored procedure (connection_id, database, procedure_name)
    Option<(i64, Option<String>, String)>,
    // New: request to generate CREATE TABLE script (connection_id, database, table_name)
    Option<(i64, Option<String>, String)>,
    Option<(i64, String)>,
    // New: request to open "Add Custom View" dialog (connection_id)
    Option<i64>,
    // New: request to execute Custom View (connection_id, view_name, query)
    Option<(i64, String, String)>,
    // New: request to delete Custom View (connection_id, view_name)
    Option<(i64, String)>,
    // New: request to edit Custom View (connection_id, view_name, query)
    Option<(i64, String, String)>,
    // New: request to open Import CSV wizard (connection_id, database_name, table_name)
    Option<(i64, Option<String>, String)>,
    // New: request to copy table DDL to clipboard (connection_id, database_name, table_name)
    Option<(i64, Option<String>, String)>,
    // New: request to open Schema Diff dialog prefilled with (connection_id, database_name)
    Option<(i64, String)>,
    // New: request to open Backup dialog prefilled with (connection_id, database_name)
    Option<(i64, String)>,
    // New: request to open Restore dialog prefilled with (connection_id, database_name)
    Option<(i64, String)>,
    // New: request to open Copy Database dialog prefilled with (connection_id, database_name)
    Option<(i64, String)>,
);

// ── CSV Import Wizard ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum CsvImportStatus {
    #[default]
    Idle,
    Importing,
    Done(usize),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct CsvColumnMapping {
    pub csv_header: String,
    pub target_column: String, // "__skip__" = skip this column
}

#[derive(Clone, Debug)]
pub struct CsvImportState {
    pub connection_id: i64,
    pub database_name: Option<String>,
    pub table_name: String,
    pub db_type: crate::models::enums::DatabaseType,
    pub file_path: Option<std::path::PathBuf>,
    pub delimiter: char,
    pub has_header_row: bool,
    pub null_value: String,
    pub preview_headers: Vec<String>,
    pub preview_rows: Vec<Vec<String>>,
    pub table_columns: Vec<String>,
    pub column_mappings: Vec<CsvColumnMapping>,
    pub status: CsvImportStatus,
    pub progress_message: String,
}

// ── Schema Diff ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum SchemaDiffStatus {
    #[default]
    Idle,
    Running,
    Done,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct ColumnDiff {
    pub name: String,
    pub left_type: Option<String>,
    pub right_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TableDiff {
    pub table_name: String,
    pub status: DiffStatus,
    pub column_diffs: Vec<ColumnDiff>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiffStatus {
    Added,
    Removed,
    Modified,
    Same,
}

#[derive(Clone, Debug)]
pub struct SchemaDiffResult {
    pub diffs: Vec<TableDiff>,
}

#[derive(Clone, Debug)]
pub struct SchemaDiffState {
    pub left_conn_id: i64,
    pub left_db: String,
    pub right_conn_id: i64,
    pub right_db: String,
    pub status: SchemaDiffStatus,
    pub result: Option<SchemaDiffResult>,
    pub show_same: bool,
    pub filter_text: String,
}

impl SchemaDiffState {
    pub fn new(
        conn_id: i64,
        db_name: String,
        connections: &[crate::models::structs::ConnectionConfig],
    ) -> Self {
        let right_conn_id = connections
            .iter()
            .find(|c| c.id != Some(conn_id))
            .and_then(|c| c.id)
            .unwrap_or(conn_id);
        let right_db = connections
            .iter()
            .find(|c| c.id == Some(right_conn_id))
            .map(|c| c.database.clone())
            .unwrap_or_default();
        Self {
            left_conn_id: conn_id,
            left_db: db_name,
            right_conn_id,
            right_db,
            status: SchemaDiffStatus::Idle,
            result: None,
            show_same: false,
            filter_text: String::new(),
        }
    }
}

mod serde_color {
    use eframe::egui::Color32;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(color: &Color32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let array = [color.r(), color.g(), color.b(), color.a()];
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(4)?;
        tup.serialize_element(&array[0])?;
        tup.serialize_element(&array[1])?;
        tup.serialize_element(&array[2])?;
        tup.serialize_element(&array[3])?;
        tup.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Color32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: [u8; 4] = Deserialize::deserialize(deserializer)?;
        Ok(Color32::from_rgba_premultiplied(
            opt[0], opt[1], opt[2], opt[3],
        ))
    }
}

mod serde_pos2 {
    use eframe::egui::Pos2;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(pos: &Pos2, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&pos.x)?;
        tup.serialize_element(&pos.y)?;
        tup.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Pos2, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: [f32; 2] = Deserialize::deserialize(deserializer)?;
        Ok(Pos2::new(opt[0], opt[1]))
    }
}

mod serde_vec2 {
    use eframe::egui::Vec2;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(vec: &Vec2, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&vec.x)?;
        tup.serialize_element(&vec.y)?;
        tup.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec2, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: [f32; 2] = Deserialize::deserialize(deserializer)?;
        Ok(Vec2::new(opt[0], opt[1]))
    }
}

mod serde_option_pos2 {
    use eframe::egui::Pos2;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(pos: &Option<Pos2>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(p) = pos {
            use serde::ser::SerializeTuple;
            let mut tup = serializer.serialize_tuple(2)?;
            tup.serialize_element(&p.x)?;
            tup.serialize_element(&p.y)?;
            tup.end()
        } else {
            serializer.serialize_none()
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Pos2>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<[f32; 2]> = Deserialize::deserialize(deserializer)?;
        Ok(opt.map(|arr| Pos2::new(arr[0], arr[1])))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterOperator {
    #[default]
    Equal,
    NotEqual,
    Equals,
    NotEquals,
    Like,
    ILike,
    In,
    IsNull,
    IsNotNull,
    Between,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Contains,
    StartsWith,
    EndsWith,
}

impl FilterOperator {
    pub fn label(&self) -> &'static str {
        match self {
            FilterOperator::Equal | FilterOperator::Equals => "= (equals)",
            FilterOperator::NotEqual | FilterOperator::NotEquals => "!= (not equals)",
            FilterOperator::Like => "LIKE (pattern)",
            FilterOperator::ILike => "ILIKE (case insensitive)",
            FilterOperator::In => "IN (...)",
            FilterOperator::IsNull => "IS NULL",
            FilterOperator::IsNotNull => "IS NOT NULL",
            FilterOperator::Between => "BETWEEN (val1 AND val2)",
            FilterOperator::GreaterThan => "> (greater than)",
            FilterOperator::LessThan => "< (less than)",
            FilterOperator::GreaterThanOrEqual => ">= (greater or equal)",
            FilterOperator::LessThanOrEqual => "<= (less or equal)",
            FilterOperator::Contains => "contains (LIKE %...%)",
            FilterOperator::StartsWith => "starts with (LIKE ...%)",
            FilterOperator::EndsWith => "ends with (LIKE %...)",
        }
    }

    pub fn all() -> &'static [FilterOperator] {
        &[
            FilterOperator::Equal,
            FilterOperator::NotEqual,
            FilterOperator::Like,
            FilterOperator::ILike,
            FilterOperator::In,
            FilterOperator::IsNull,
            FilterOperator::IsNotNull,
            FilterOperator::Between,
            FilterOperator::GreaterThan,
            FilterOperator::LessThan,
            FilterOperator::GreaterThanOrEqual,
            FilterOperator::LessThanOrEqual,
            FilterOperator::Contains,
            FilterOperator::StartsWith,
            FilterOperator::EndsWith,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterGroup {
    #[default]
    #[serde(alias = "AND", alias = "and")]
    And,
    #[serde(alias = "OR", alias = "or")]
    Or,
}

impl FilterGroup {
    pub const AND: Self = Self::And;
    pub const OR: Self = Self::Or;

    pub fn as_sql(&self) -> &'static str {
        match self {
            FilterGroup::And => "AND",
            FilterGroup::Or => "OR",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            FilterGroup::And => "AND",
            FilterGroup::Or => "OR",
        }
    }

    pub fn all() -> &'static [FilterGroup] {
        &[FilterGroup::And, FilterGroup::Or]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct FilterCondition {
    pub column: String,
    pub operator: FilterOperator,
    pub value: String,
    #[serde(default)]
    pub value2: Option<String>,
}

impl FilterCondition {
    pub fn new(
        column: impl Into<String>,
        operator: FilterOperator,
        value: impl Into<String>,
    ) -> Self {
        Self {
            column: column.into(),
            operator,
            value: value.into(),
            value2: None,
        }
    }

    pub fn between(
        column: impl Into<String>,
        val1: impl Into<String>,
        val2: impl Into<String>,
    ) -> Self {
        Self {
            column: column.into(),
            operator: FilterOperator::Between,
            value: val1.into(),
            value2: Some(val2.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct VisualFilterState {
    pub conditions: Vec<FilterCondition>,
    pub match_all: bool, // true = AND, false = OR
    pub is_open: bool,
    #[serde(default)]
    pub group: FilterGroup,
    #[serde(default)]
    pub pinned_columns: HashSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_display_name_with_folder() {
        let conn = ConnectionConfig {
            name: "58".to_string(),
            folder: Some("FOX".to_string()),
            ..Default::default()
        };
        assert_eq!(conn.display_name(), "FOX/58");
    }

    #[test]
    fn test_connection_display_name_without_folder() {
        let conn = ConnectionConfig {
            name: "My Database".to_string(),
            folder: None,
            ..Default::default()
        };
        assert_eq!(conn.display_name(), "My Database");
    }

    #[test]
    fn test_connection_display_name_with_empty_folder() {
        let conn = ConnectionConfig {
            name: "Staging".to_string(),
            folder: Some("   ".to_string()),
            ..Default::default()
        };
        assert_eq!(conn.display_name(), "Staging");
    }

    #[test]
    fn test_filter_operator_labels_and_all() {
        let ops = FilterOperator::all();
        assert!(ops.contains(&FilterOperator::Equal));
        assert!(ops.contains(&FilterOperator::NotEqual));
        assert!(ops.contains(&FilterOperator::Like));
        assert!(ops.contains(&FilterOperator::ILike));
        assert!(ops.contains(&FilterOperator::In));
        assert!(ops.contains(&FilterOperator::IsNull));
        assert!(ops.contains(&FilterOperator::IsNotNull));
        assert!(ops.contains(&FilterOperator::Between));
        assert!(ops.contains(&FilterOperator::GreaterThan));
        assert!(ops.contains(&FilterOperator::LessThan));

        assert_eq!(FilterOperator::Equal.label(), "= (equals)");
        assert_eq!(FilterOperator::Between.label(), "BETWEEN (val1 AND val2)");
    }

    #[test]
    fn test_filter_group() {
        assert_eq!(FilterGroup::And.as_sql(), "AND");
        assert_eq!(FilterGroup::Or.as_sql(), "OR");
        assert_eq!(FilterGroup::AND, FilterGroup::And);
        assert_eq!(FilterGroup::OR, FilterGroup::Or);
        assert_eq!(FilterGroup::all(), &[FilterGroup::And, FilterGroup::Or]);
    }

    #[test]
    fn test_filter_condition_constructors() {
        let cond1 = FilterCondition::new("age", FilterOperator::GreaterThan, "25");
        assert_eq!(cond1.column, "age");
        assert_eq!(cond1.operator, FilterOperator::GreaterThan);
        assert_eq!(cond1.value, "25");
        assert_eq!(cond1.value2, None);

        let cond2 = FilterCondition::between("created_at", "2026-01-01", "2026-12-31");
        assert_eq!(cond2.column, "created_at");
        assert_eq!(cond2.operator, FilterOperator::Between);
        assert_eq!(cond2.value, "2026-01-01");
        assert_eq!(cond2.value2, Some("2026-12-31".to_string()));
    }

    #[test]
    fn test_foreign_key_relation() {
        let fk = ForeignKeyRelation::new("user_id", "users", "id", Some("public".to_string()));
        assert_eq!(fk.source_col, "user_id");
        assert_eq!(fk.target_table, "users");
        assert_eq!(fk.target_col, "id");
        assert_eq!(fk.target_schema, Some("public".to_string()));
    }

    #[test]
    fn test_cell_value_enum_and_conversions() {
        let val_null = CellValue::Null;
        assert!(val_null.is_null());
        assert_eq!(val_null.to_display_string(), "NULL");
        assert_eq!(format!("{}", val_null), "NULL");

        let val_text: CellValue = "hello world".into();
        assert!(!val_text.is_null());
        assert_eq!(val_text.as_str(), Some("hello world"));
        assert_eq!(val_text.to_display_string(), "hello world");

        let val_int: CellValue = 42i64.into();
        assert_eq!(val_int.to_display_string(), "42");

        let val_num: CellValue = 123.45.into();
        assert_eq!(val_num.to_display_string(), "123.45");

        let val_bool: CellValue = true.into();
        assert_eq!(val_bool.to_display_string(), "true");

        let bytes_vec = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let val_bytes: CellValue = bytes_vec.into();
        assert_eq!(val_bytes.as_bytes(), Some(&[0xDE, 0xAD, 0xBE, 0xEF][..]));
        assert_eq!(val_bytes.to_display_string(), "<BLOB 4 bytes>");

        let val_opt: CellValue = Some("dynamic".to_string()).into();
        assert_eq!(val_opt.as_str(), Some("dynamic"));

        let val_none: CellValue = None.into();
        assert!(val_none.is_null());

        let from_lossy = CellValue::from_str_lossy("null");
        assert!(from_lossy.is_null());
        let from_lossy_str = CellValue::from_str_lossy("active");
        assert_eq!(from_lossy_str.as_str(), Some("active"));
    }

    #[test]
    fn test_pinned_columns() {
        let mut pinned = HashSet::new();
        pinned.insert("id".to_string());
        pinned.insert("name".to_string());

        let state = VisualFilterState {
            conditions: Vec::new(),
            match_all: true,
            is_open: false,
            group: FilterGroup::And,
            pinned_columns: pinned.clone(),
        };

        assert!(state.pinned_columns.contains("id"));
        assert!(state.pinned_columns.contains("name"));
    }

    #[test]
    fn test_query_tab_pinning() {
        let mut tab = QueryTab {
            id: 1,
            title: "Test Tab".to_string(),
            content: "SELECT 1;".to_string(),
            file_path: None,
            is_saved: false,
            is_modified: false,
            connection_id: None,
            database_name: None,
            schema_name: None,
            has_executed_query: false,
            result_headers: Vec::new(),
            result_rows: Vec::new(),
            result_all_rows: Vec::new(),
            result_table_name: String::new(),
            result_column_metadata: None,
            results: Vec::new(),
            active_result_index: 0,
            is_table_browse_mode: false,
            current_page: 0,
            page_size: 100,
            total_rows: 0,
            base_query: String::new(),
            dba_special_mode: None,
            object_ddl: None,
            explain_plan_json: None,
            query_message: String::new(),
            query_message_is_error: false,
            diagram_state: None,
            should_run_on_open: false,
            http_client_state: None,
            redis_browser_state: None,
            dba_monitor_state: None,
            user_manager_state: None,
            tx_mode: false,
            tx_active: false,
            session: None,
            pinned_columns: HashSet::new(),
            is_pinned: false,
            last_executed_sql: String::new(),
            last_statement_type: StatementType::Select,
            last_affected_rows: None,
        };

        assert!(!tab.is_pinned);
        tab.is_pinned = true;
        assert!(tab.is_pinned);
    }

    #[test]
    fn test_statement_type_from_sql() {
        assert_eq!(StatementType::from_sql("SELECT * FROM users"), StatementType::Select);
        assert_eq!(StatementType::from_sql("  -- comment\nSELECT 1"), StatementType::Select);
        assert_eq!(StatementType::from_sql("/* block */ WITH cte AS (...) SELECT 1"), StatementType::Select);
        assert_eq!(StatementType::from_sql("INSERT INTO t VALUES (1)"), StatementType::Insert);
        assert_eq!(StatementType::from_sql("UPDATE t SET a = 1"), StatementType::Update);
        assert_eq!(StatementType::from_sql("DELETE FROM t WHERE a = 1"), StatementType::Delete);
        assert_eq!(StatementType::from_sql("CREATE TABLE foo (id INT)"), StatementType::Ddl);
        assert_eq!(StatementType::from_sql("ALTER TABLE foo ADD COLUMN bar TEXT"), StatementType::Ddl);
        assert_eq!(StatementType::from_sql("DROP TABLE foo"), StatementType::Ddl);
        assert_eq!(StatementType::from_sql("TRUNCATE foo"), StatementType::Ddl);
        assert_eq!(StatementType::from_sql("BEGIN;"), StatementType::Transaction);
        assert_eq!(StatementType::from_sql("COMMIT;"), StatementType::Transaction);
        assert_eq!(StatementType::from_sql("SHOW TABLES;"), StatementType::Show);
        assert_eq!(StatementType::from_sql("EXPLAIN SELECT 1;"), StatementType::Show);
    }
}
