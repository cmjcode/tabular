//! Titik-titik GUI yang mencatat kejadian penting ke Engram
//! (`crate::engram::record`). Semua fungsi di sini murah: hanya menyusun
//! teks lalu menitipkannya ke worker latar; tidak ada I/O jaringan.

use crate::connection::types::QueryResultMessage;
use crate::engram::{self, ActivityKind};
use crate::models::enums::DatabaseType;
use crate::models::structs::ConnectionConfig;

use super::Tabular;
use super::project_ui;

impl Tabular {
    /// Nama project untuk koneksi `id`, atau project aktif.
    fn engram_project_for(&self, connection_id: Option<i64>) -> Option<String> {
        connection_id
            .and_then(|id| self.connections.iter().find(|c| c.id == Some(id)))
            .and_then(|c| project_ui::project_for_connection(self, c))
            .or_else(|| project_ui::current_project(self))
            .map(|p| p.name.clone())
    }

    /// Nama koneksi untuk teks kejadian.
    pub(crate) fn engram_connection_name(&self, id: i64) -> String {
        self.connections
            .iter()
            .find(|c| c.id == Some(id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("#{id}"))
    }

    /// Kejadian umum yang terkait project aktif.
    pub(crate) fn engram_activity(&self, kind: ActivityKind, text: impl Into<String>) {
        let project = self.engram_project_for(None);
        engram::record(kind, project.as_deref(), text);
    }

    /// Perubahan skema dan penulisan data yang berhasil dijalankan. Bacaan
    /// tidak dicatat; nilai data DML tidak pernah ikut.
    pub(crate) fn engram_record_query(&self, message: &QueryResultMessage) {
        if !message.success || message.query.trim().is_empty() {
            return;
        }
        let Some(conn) = self
            .connections
            .iter()
            .find(|c| c.id == Some(message.connection_id))
        else {
            return;
        };
        if matches!(
            conn.connection_type,
            DatabaseType::Redis | DatabaseType::MongoDB | DatabaseType::ApiHttp
        ) {
            return;
        }
        let database = message
            .tab_id
            .and_then(|id| self.query_tabs.iter().find(|t| t.id == id))
            .and_then(|t| t.database_name.clone())
            .filter(|d| !d.trim().is_empty())
            .or_else(|| Some(conn.database.clone()).filter(|d| !d.trim().is_empty()));
        let hash_is_comment = matches!(conn.connection_type, DatabaseType::MySQL);
        let segments = crate::connection::sql::split_sql_segments(&message.query, hash_is_comment);
        let single = segments.len() == 1;
        let project = self.engram_project_for(Some(message.connection_id));
        for seg in segments {
            let Some(sql) = message.query.get(seg.raw.start..seg.body_end) else {
                continue;
            };
            let rows = single
                .then_some(message.affected_rows)
                .flatten()
                .map(|n| n as u64);
            if let Some((kind, text)) =
                engram::describe_statement(sql, &conn.name, database.as_deref(), rows)
            {
                engram::record(kind, project.as_deref(), text);
            }
        }
    }

    /// Koneksi ditambah, diubah, atau dihapus. Tanpa username/password.
    pub(crate) fn engram_connection(&self, action: &str, conn: &ConnectionConfig) {
        let mut text = format!(
            "{action} the {} connection \"{}\"",
            conn.connection_type.display_name(),
            conn.name
        );
        if !conn.host.trim().is_empty() {
            text.push_str(&format!(" (host {}", conn.host.trim()));
            if !conn.database.trim().is_empty() {
                text.push_str(&format!(", database {}", conn.database.trim()));
            }
            text.push(')');
        }
        let project = project_ui::project_for_connection(self, conn)
            .or_else(|| project_ui::current_project(self))
            .map(|p| p.name.clone());
        engram::record(ActivityKind::Connection, project.as_deref(), text);
    }
}
