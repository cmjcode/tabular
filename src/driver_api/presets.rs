//! Preset engine yang berjalan di atas driver builtin (item L2/L3).
//!
//! Preset bukan plugin: ia hanya mengisi tipe builtin, port default, dan
//! nama saat membuat koneksi baru. Koneksi yang tersimpan tetap bertipe
//! builtin sehingga semua fitur engine itu berlaku.

use crate::models::enums::DatabaseType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub based_on: DatabaseType,
    pub default_port: &'static str,
    pub note: &'static str,
}

pub fn presets() -> Vec<EnginePreset> {
    use DatabaseType::*;
    vec![
        EnginePreset {
            id: "mariadb",
            name: "MariaDB",
            based_on: MySQL,
            default_port: "3306",
            note: "MySQL wire protocol",
        },
        EnginePreset {
            id: "tidb",
            name: "TiDB",
            based_on: MySQL,
            default_port: "4000",
            note: "MySQL wire protocol",
        },
        EnginePreset {
            id: "oceanbase",
            name: "OceanBase (MySQL mode)",
            based_on: MySQL,
            default_port: "2881",
            note: "MySQL wire protocol",
        },
        EnginePreset {
            id: "databend",
            name: "Databend",
            based_on: MySQL,
            default_port: "3307",
            note: "MySQL handler",
        },
        EnginePreset {
            id: "cockroachdb",
            name: "CockroachDB",
            based_on: PostgreSQL,
            default_port: "26257",
            note: "PostgreSQL wire protocol",
        },
        EnginePreset {
            id: "redshift",
            name: "Amazon Redshift",
            based_on: PostgreSQL,
            default_port: "5439",
            note: "PostgreSQL wire protocol",
        },
        EnginePreset {
            id: "yugabytedb",
            name: "YugabyteDB",
            based_on: PostgreSQL,
            default_port: "5433",
            note: "PostgreSQL wire protocol",
        },
        EnginePreset {
            id: "timescaledb",
            name: "TimescaleDB",
            based_on: PostgreSQL,
            default_port: "5432",
            note: "PostgreSQL extension",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_ids_are_unique_and_based_on_sql_builtins() {
        let all = presets();
        let mut ids: Vec<_> = all.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for p in all {
            assert!(matches!(
                p.based_on,
                DatabaseType::MySQL | DatabaseType::PostgreSQL
            ));
            assert!(p.default_port.parse::<u16>().is_ok());
        }
    }
}
