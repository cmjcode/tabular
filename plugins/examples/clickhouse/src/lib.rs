//! Driver ClickHouse untuk Tabular lewat HTTP interface.
//!
//! Hasil query dikirim sebagai `http_table` (format `JSONCompact`), sehingga
//! baris di-parse host secara native dan tidak melewati interpreter Wasm.

use tabular_driver_sdk::{
    export_driver, http, url_encode, ColumnInfo, ConnectParams, Driver, DriverResult,
    ExecuteReply, ExecuteRequest, HttpRequest, HttpTableRequest, TableFormat, TableInfo,
    TableKind,
};

#[derive(Default)]
pub struct ClickHouse {
    base_url: String,
    user: String,
    password: String,
    database: String,
    compression: bool,
}

/// Literal string ClickHouse: `'...'` dengan backslash dan kutip di-escape.
pub fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// `query_id` stabil per job supaya `cancel` bisa menarget query yang tepat.
pub fn query_id(job_id: u64) -> String {
    format!("tabular-{job_id}")
}

impl ClickHouse {
    fn headers(&self) -> Vec<(String, String)> {
        let mut h = vec![("X-ClickHouse-User".to_string(), self.user.clone())];
        if !self.password.is_empty() {
            h.push(("X-ClickHouse-Key".to_string(), self.password.clone()));
        }
        h
    }

    /// URL query dengan parameter setting ClickHouse.
    pub fn url(&self, database: Option<&str>, extra: &[(&str, String)]) -> String {
        let mut params: Vec<(String, String)> = Vec::new();
        let db = database.filter(|d| !d.is_empty()).unwrap_or(&self.database);
        if !db.is_empty() {
            params.push(("database".into(), db.to_string()));
        }
        if self.compression {
            params.push(("enable_http_compression".into(), "1".into()));
        }
        for (k, v) in extra {
            params.push((k.to_string(), v.clone()));
        }
        let query: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect();
        if query.is_empty() {
            format!("{}/", self.base_url)
        } else {
            format!("{}/?{}", self.base_url, query.join("&"))
        }
    }

    fn post(&self, database: Option<&str>, sql: &str) -> DriverResult<String> {
        let resp = http(&HttpRequest {
            method: "POST".into(),
            url: self.url(database, &[]),
            headers: self.headers(),
            body: Some(sql.to_string()),
            timeout_ms: Some(60_000),
        })?;
        if resp.status >= 400 {
            return Err(resp.body.trim().to_string());
        }
        Ok(resp.body)
    }

    /// Jalankan query metadata kecil dengan format TSV tanpa header.
    fn tsv(&self, sql: &str) -> DriverResult<Vec<Vec<String>>> {
        let body = self.post(None, &format!("{sql} FORMAT TabSeparated"))?;
        Ok(body
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\t').map(unescape_tsv).collect())
            .collect())
    }
}

fn unescape_tsv(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Driver for ClickHouse {
    fn connect(&mut self, p: ConnectParams) -> DriverResult<()> {
        let scheme = if p.tls.enabled { "https" } else { "http" };
        let port = p.port.unwrap_or(if p.tls.enabled { 8443 } else { 8123 });
        if p.host.trim().is_empty() {
            return Err("host is required".into());
        }
        self.base_url = format!("{scheme}://{}:{port}", p.host.trim());
        self.user = if p.username.is_empty() {
            "default".into()
        } else {
            p.username
        };
        self.password = p.password;
        self.database = p.database;
        self.compression = p.options.get("compression").map(String::as_str) == Some("true");
        // Validasi kredensial sekarang, bukan saat query pertama.
        self.post(None, "SELECT 1").map(|_| ())
    }

    fn list_databases(&mut self) -> DriverResult<Vec<String>> {
        Ok(self
            .tsv("SELECT name FROM system.databases ORDER BY name")?
            .into_iter()
            .filter_map(|r| r.into_iter().next())
            .collect())
    }

    fn list_tables(&mut self, database: Option<String>, _: Option<String>) -> DriverResult<Vec<TableInfo>> {
        let db = database.unwrap_or_else(|| self.database.clone());
        let rows = self.tsv(&format!(
            "SELECT name, engine FROM system.tables WHERE database = {} ORDER BY name",
            quote_literal(&db)
        ))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let name = it.next()?;
                let engine = it.next().unwrap_or_default();
                let kind = if engine.ends_with("View") {
                    TableKind::View
                } else {
                    TableKind::Table
                };
                Some(TableInfo { name, kind })
            })
            .collect())
    }

    fn list_columns(&mut self, database: Option<String>, _: Option<String>, table: String) -> DriverResult<Vec<ColumnInfo>> {
        let db = database.unwrap_or_else(|| self.database.clone());
        let rows = self.tsv(&format!(
            "SELECT name, type, is_in_primary_key FROM system.columns \
             WHERE database = {} AND table = {} ORDER BY position",
            quote_literal(&db),
            quote_literal(&table)
        ))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let name = it.next()?;
                let data_type = it.next().unwrap_or_default();
                let pk = it.next().as_deref() == Some("1");
                Some(ColumnInfo {
                    nullable: data_type.starts_with("Nullable("),
                    name,
                    data_type,
                    primary_key: pk,
                })
            })
            .collect())
    }

    fn execute(&mut self, r: ExecuteRequest) -> DriverResult<ExecuteReply> {
        let mut settings = vec![
            ("default_format", "JSONCompact".to_string()),
            ("query_id", query_id(r.job_id)),
        ];
        if r.max_rows > 0 {
            // Satu baris ekstra supaya host tahu hasil terpotong.
            settings.push(("max_result_rows", (r.max_rows + 1).to_string()));
            settings.push(("result_overflow_mode", "break".to_string()));
        }
        Ok(ExecuteReply::HttpTable(HttpTableRequest {
            request: HttpRequest {
                method: "POST".into(),
                url: self.url(r.database.as_deref(), &settings),
                headers: self.headers(),
                body: Some(r.query),
                timeout_ms: None,
            },
            format: TableFormat::JsonCompact,
        }))
    }

    fn cancel(&mut self, job_id: u64) -> DriverResult<()> {
        self.post(
            None,
            &format!(
                "KILL QUERY WHERE query_id = {} ASYNC",
                quote_literal(&query_id(job_id))
            ),
        )
        .map(|_| ())
    }
}

export_driver!(ClickHouse);

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> ClickHouse {
        ClickHouse {
            base_url: "http://ch.local:8123".into(),
            user: "default".into(),
            password: String::new(),
            database: "analytics".into(),
            compression: false,
        }
    }

    #[test]
    fn literal_escaping() {
        assert_eq!(quote_literal("a'b\\c"), "'a\\'b\\\\c'");
    }

    #[test]
    fn url_includes_database_and_settings() {
        let d = driver();
        assert_eq!(d.url(None, &[]), "http://ch.local:8123/?database=analytics");
        let u = d.url(Some("other"), &[("query_id", "tabular-5".into())]);
        assert_eq!(u, "http://ch.local:8123/?database=other&query_id=tabular-5");
    }

    #[test]
    fn execute_defers_to_host_table_parsing() {
        let mut d = driver();
        let reply = d
            .execute(ExecuteRequest {
                query: "SELECT 1".into(),
                database: None,
                schema: None,
                max_rows: 100,
                job_id: 9,
            })
            .unwrap();
        let ExecuteReply::HttpTable(t) = reply else {
            panic!("expected http_table");
        };
        assert!(matches!(t.format, TableFormat::JsonCompact));
        assert!(t.request.url.contains("query_id=tabular-9"));
        assert!(t.request.url.contains("max_result_rows=101"));
        assert_eq!(t.request.body.as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn tsv_unescape() {
        assert_eq!(unescape_tsv("a\\tb\\nc\\\\"), "a\tb\nc\\");
    }
}
