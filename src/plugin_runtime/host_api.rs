use serde::{Deserialize, Serialize};

/// Safe data representation of a column schema passed to WebAssembly plugins
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginColumnSchema {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_auto_increment: bool,
    pub default_value: Option<String>,
    pub comment: Option<String>,
}

/// Safe data representation of table schema passed to WebAssembly plugins
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginTableSchema {
    pub table_name: String,
    pub schema_name: Option<String>,
    pub database_type: String,
    pub columns: Vec<PluginColumnSchema>,
    pub total_rows: usize,
}

/// Safe data representation of selected rows passed to WebAssembly plugins
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSelectionData {
    pub table_name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_selected: usize,
}

/// Safe export payload emitted by WebAssembly plugins to the host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginExportPayload {
    pub format: String,
    pub filename_suggestion: String,
    pub content_type: String,
    pub text_content: Option<String>,
    pub binary_base64: Option<String>,
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

/// Log message emitted by a plugin
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginLogEntry {
    pub level: PluginLogLevel,
    pub message: String,
    pub timestamp_millis: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginLogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl From<i32> for PluginLogLevel {
    fn from(val: i32) -> Self {
        match val {
            0 => PluginLogLevel::Debug,
            1 => PluginLogLevel::Info,
            2 => PluginLogLevel::Warn,
            _ => PluginLogLevel::Error,
        }
    }
}
