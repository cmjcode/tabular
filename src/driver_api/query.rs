//! Helper SQL generik untuk engine plugin: kutip identifier, query pratinjau
//! tabel, dan pemetaan dialect.

use super::{EngineCapabilities, QueryLanguage, registry};

/// Capability engine plugin `id`; default bila driver belum terpasang.
pub fn capabilities(id: &str) -> EngineCapabilities {
    registry::descriptor(id)
        .map(|d| d.capabilities)
        .unwrap_or_default()
}

/// Dialect memakai backtick untuk identifier (MySQL, ClickHouse, dsb.).
fn uses_backticks(dialect: Option<&str>) -> bool {
    matches!(
        dialect.map(str::to_ascii_lowercase).as_deref(),
        Some("mysql" | "mariadb" | "clickhouse" | "databend" | "doris" | "starrocks")
    )
}

/// Kutip identifier (boleh `schema.table`) sesuai dialect engine.
pub fn quote_ident(dialect: Option<&str>, ident: &str) -> String {
    let (open, close) = if uses_backticks(dialect) {
        ('`', '`')
    } else if dialect.is_some_and(|d| d.eq_ignore_ascii_case("mssql")) {
        ('[', ']')
    } else {
        ('"', '"')
    };
    ident
        .split('.')
        .map(|part| {
            let escaped = part.replace(close, &format!("{close}{close}"));
            format!("{open}{escaped}{close}")
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Query pratinjau isi tabel untuk engine plugin.
pub fn preview_query(caps: &EngineCapabilities, database: &str, table: &str, limit: usize) -> String {
    let quoted = quote_ident(caps.sql_dialect.as_deref(), table);
    match &caps.preview_template {
        Some(template) => template
            .replace("{table}", &quoted)
            .replace("{raw_table}", table)
            .replace("{database}", database)
            .replace("{limit}", &limit.to_string()),
        None if caps.query_language == QueryLanguage::Sql => {
            format!("SELECT * FROM {quoted} LIMIT {limit};")
        }
        None => String::new(),
    }
}

/// Dialect SQL "keluarga" builtin yang paling dekat, untuk parser/formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialectFamily {
    MySql,
    Postgres,
    Sqlite,
    MsSql,
    Generic,
}

pub fn dialect_family(caps: &EngineCapabilities) -> Option<DialectFamily> {
    if caps.query_language != QueryLanguage::Sql {
        return None;
    }
    let dialect = caps.sql_dialect.as_deref().map(str::to_ascii_lowercase);
    Some(match dialect.as_deref() {
        Some("postgres" | "postgresql" | "redshift" | "cockroachdb") => DialectFamily::Postgres,
        Some("sqlite" | "libsql" | "turso" | "d1") => DialectFamily::Sqlite,
        Some("mssql") => DialectFamily::MsSql,
        _ if uses_backticks(dialect.as_deref()) => DialectFamily::MySql,
        _ => DialectFamily::Generic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_per_dialect() {
        assert_eq!(quote_ident(Some("clickhouse"), "db.t`x"), "`db`.`t``x`");
        assert_eq!(quote_ident(None, "public.My\"T"), "\"public\".\"My\"\"T\"");
        assert_eq!(quote_ident(Some("mssql"), "dbo.t]"), "[dbo].[t]]]");
    }

    #[test]
    fn preview_uses_template_or_sql_default() {
        let caps = EngineCapabilities {
            sql_dialect: Some("clickhouse".into()),
            ..Default::default()
        };
        assert_eq!(preview_query(&caps, "db", "events", 100), "SELECT * FROM `events` LIMIT 100;");

        let json = EngineCapabilities {
            query_language: QueryLanguage::Json,
            preview_template: Some(r#"{"index":"{raw_table}","size":{limit}}"#.into()),
            ..Default::default()
        };
        assert_eq!(preview_query(&json, "", "logs", 5), r#"{"index":"logs","size":5}"#);

        let text = EngineCapabilities {
            query_language: QueryLanguage::Text,
            ..Default::default()
        };
        assert_eq!(preview_query(&text, "", "k", 5), "");
    }

    #[test]
    fn maps_dialect_family() {
        let mut caps = EngineCapabilities::default();
        assert_eq!(dialect_family(&caps), Some(DialectFamily::Generic));
        caps.sql_dialect = Some("ClickHouse".into());
        assert_eq!(dialect_family(&caps), Some(DialectFamily::MySql));
        caps.sql_dialect = Some("cockroachdb".into());
        assert_eq!(dialect_family(&caps), Some(DialectFamily::Postgres));
        caps.query_language = QueryLanguage::Json;
        assert_eq!(dialect_family(&caps), None);
    }
}
