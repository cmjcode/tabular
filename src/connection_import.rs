//! Impor koneksi dari klien database lain. Saat ini: DBeaver
//! (`data-sources.json`).
//!
//! Password tidak diimpor. DBeaver menyimpannya terenkripsi di file terpisah
//! (`credentials-config.json`); pengguna mengisi ulang password di Tabular,
//! sehingga rahasia tidak pernah dibaca dari aplikasi lain.

use crate::deeplink::{ParsedDsn, parse_dsn};
use crate::models::enums::DatabaseType;
use serde_json::Value;
use std::path::PathBuf;

/// Satu koneksi hasil impor (belum disimpan).
#[derive(Debug, Clone)]
pub struct ImportedConnection {
    pub name: String,
    /// Folder di DBeaver, bila ada.
    pub folder: Option<String>,
    pub dsn: ParsedDsn,
    /// Host SSH tunnel (`host:port`) bila tunnel aktif di DBeaver.
    pub ssh: Option<SshTunnel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTunnel {
    pub host: String,
    pub port: String,
    pub username: String,
}

/// Hasil membaca file impor.
#[derive(Debug, Default)]
pub struct ImportReport {
    pub connections: Vec<ImportedConnection>,
    /// `(nama, alasan)` koneksi yang tidak bisa diimpor.
    pub skipped: Vec<(String, String)>,
}

/// Lokasi `data-sources.json` default DBeaver di platform ini.
pub fn dbeaver_default_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()?.join("Library/DBeaverData")
    } else if cfg!(target_os = "windows") {
        dirs::config_dir()?.join("DBeaverData")
    } else {
        dirs::data_dir()?.join("DBeaverData")
    };
    Some(base.join("workspace6/General/.dbeaver/data-sources.json"))
}

fn provider_type(provider: &str, driver: &str) -> Option<DatabaseType> {
    let p = provider.to_ascii_lowercase();
    let d = driver.to_ascii_lowercase();
    Some(match p.as_str() {
        "postgresql" | "postgres" | "redshift" | "cockroachdb" | "yugabyte" | "timescale"
        | "greenplum" => DatabaseType::PostgreSQL,
        "mysql" | "mariadb" | "tidb" => DatabaseType::MySQL,
        "sqlite" => DatabaseType::SQLite,
        "sqlserver" | "mssql" => DatabaseType::MsSQL,
        "mongodb" | "mongo" => DatabaseType::MongoDB,
        "redis" => DatabaseType::Redis,
        "generic" if d.contains("sqlite") => DatabaseType::SQLite,
        _ => return None,
    })
}

fn default_port(db: &DatabaseType) -> &'static str {
    match db {
        DatabaseType::MySQL => "3306",
        DatabaseType::PostgreSQL => "5432",
        DatabaseType::MsSQL => "1433",
        DatabaseType::Redis => "6379",
        DatabaseType::MongoDB => "27017",
        _ => "",
    }
}

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn ssh_tunnel(config: &Value) -> Option<SshTunnel> {
    let handler = config.get("handlers")?.get("ssh_tunnel")?;
    if !handler
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let props = handler.get("properties").unwrap_or(&Value::Null);
    let host = text(props, "host");
    if host.is_empty() {
        return None;
    }
    let port = Some(text(props, "port"))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "22".to_string());
    let username = Some(text(handler, "user"))
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| text(props, "user"));
    Some(SshTunnel {
        host,
        port,
        username,
    })
}

/// Parse isi `data-sources.json` DBeaver.
pub fn parse_dbeaver(json: &str) -> Result<ImportReport, String> {
    let root: Value =
        serde_json::from_str(json).map_err(|e| format!("not a DBeaver data-sources file: {e}"))?;
    let connections = root
        .get("connections")
        .and_then(Value::as_object)
        .ok_or_else(|| "no \"connections\" section found".to_string())?;

    let mut report = ImportReport::default();
    for (id, entry) in connections {
        let name = Some(text(entry, "name"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.clone());
        let provider = text(entry, "provider");
        let driver = text(entry, "driver");
        let config = entry.get("configuration").unwrap_or(&Value::Null);

        let Some(db_type) = provider_type(&provider, &driver) else {
            report
                .skipped
                .push((name, format!("unsupported database type \"{provider}\"")));
            continue;
        };

        let host = text(config, "host");
        let database = text(config, "database");
        let url = text(config, "url");
        let dsn = if db_type == DatabaseType::SQLite {
            let path = if database.is_empty() {
                url.trim_start_matches("jdbc:sqlite:").to_string()
            } else {
                database
            };
            if path.is_empty() {
                report.skipped.push((name, "no database file".to_string()));
                continue;
            }
            ParsedDsn {
                db_type,
                host: String::new(),
                port: String::new(),
                username: String::new(),
                password: String::new(),
                database: path,
                ssl: false,
            }
        } else if !host.is_empty() {
            let port = Some(text(config, "port"))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| default_port(&db_type).to_string());
            ParsedDsn {
                db_type,
                host,
                port,
                username: text(config, "user"),
                password: String::new(),
                database,
                ssl: false,
            }
        } else if !url.is_empty() {
            match parse_dsn(&url) {
                Ok(mut dsn) => {
                    dsn.db_type = db_type;
                    dsn.password.clear();
                    dsn
                }
                Err(e) => {
                    report
                        .skipped
                        .push((name, format!("could not read URL: {e}")));
                    continue;
                }
            }
        } else {
            report.skipped.push((name, "no host or URL".to_string()));
            continue;
        };

        let folder = Some(text(entry, "folder")).filter(|f| !f.is_empty());
        report.connections.push(ImportedConnection {
            name,
            folder,
            dsn,
            ssh: ssh_tunnel(config),
        });
    }
    report.connections.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "folders": {"Work": {}},
      "connections": {
        "postgres-jdbc-1": {
          "provider": "postgresql", "driver": "postgres-jdbc", "name": "Shop PG",
          "folder": "Work", "save-password": true,
          "configuration": {
            "host": "db.internal", "port": "6432", "database": "shop",
            "url": "jdbc:postgresql://db.internal:6432/shop", "user": "app",
            "handlers": {"ssh_tunnel": {"enabled": true, "user": "deploy",
              "properties": {"host": "bastion.example", "port": 2222}}}
          }
        },
        "mysql8-2": {
          "provider": "mysql", "driver": "mysql8", "name": "Local MySQL",
          "configuration": {"url": "jdbc:mysql://localhost:3307/crm", "user": "root"}
        },
        "sqlite-3": {
          "provider": "generic", "driver": "sqlite_jdbc", "name": "Notes",
          "configuration": {"database": "/tmp/notes.db", "url": "jdbc:sqlite:/tmp/notes.db"}
        },
        "oracle-4": {
          "provider": "oracle", "driver": "oracle_thin", "name": "Legacy",
          "configuration": {"host": "ora", "port": "1521"}
        }
      }
    }"#;

    #[test]
    fn parses_supported_connections_and_skips_others() {
        let r = parse_dbeaver(SAMPLE).unwrap();
        let names: Vec<&str> = r.connections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Local MySQL", "Notes", "Shop PG"]);
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].0, "Legacy");

        let pg = r.connections.iter().find(|c| c.name == "Shop PG").unwrap();
        assert_eq!(pg.dsn.db_type, DatabaseType::PostgreSQL);
        assert_eq!(pg.dsn.host, "db.internal");
        assert_eq!(pg.dsn.port, "6432");
        assert_eq!(pg.dsn.database, "shop");
        assert_eq!(pg.dsn.username, "app");
        assert!(pg.dsn.password.is_empty());
        assert_eq!(pg.folder.as_deref(), Some("Work"));
        assert_eq!(
            pg.ssh,
            Some(SshTunnel {
                host: "bastion.example".into(),
                port: "2222".into(),
                username: "deploy".into()
            })
        );

        let my = r
            .connections
            .iter()
            .find(|c| c.name == "Local MySQL")
            .unwrap();
        assert_eq!(my.dsn.db_type, DatabaseType::MySQL);
        assert_eq!(my.dsn.port, "3307");
        assert_eq!(my.dsn.database, "crm");

        let lite = r.connections.iter().find(|c| c.name == "Notes").unwrap();
        assert_eq!(lite.dsn.db_type, DatabaseType::SQLite);
        assert_eq!(lite.dsn.database, "/tmp/notes.db");
    }

    #[test]
    fn rejects_non_dbeaver_json() {
        assert!(parse_dbeaver("[]").is_err());
        assert!(parse_dbeaver("nope").is_err());
    }
}
