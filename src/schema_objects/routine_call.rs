//! Template pemanggilan procedure/function ("Execute…" di sidebar).
//!
//! Alurnya: [`params_query`] mengambil daftar parameter dari katalog, lalu
//! [`call_template`] menyusun `CALL`/`EXEC`/`SELECT` dengan placeholder
//! `:nama` untuk setiap parameter masukan. Saat dijalankan, editor meminta
//! nilainya lewat dialog parameter sehingga nilai di-escape per dialek.

use super::catalog::RoutineKind;
use super::sql::{quote_ident, quote_qualified, split_qualified, sql_literal};
use crate::models::enums::DatabaseType;

/// Arah parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    In,
    Out,
    InOut,
    Variadic,
}

impl ParamMode {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_uppercase().as_str() {
            "OUT" | "O" | "1" => ParamMode::Out,
            "INOUT" | "B" => ParamMode::InOut,
            "VARIADIC" | "V" => ParamMode::Variadic,
            _ => ParamMode::In,
        }
    }

    fn is_input(self) -> bool {
        !matches!(self, ParamMode::Out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineParam {
    /// Nama tanpa `@` (MsSQL). Bisa kosong untuk parameter PostgreSQL tanpa nama.
    pub name: String,
    pub data_type: String,
    pub mode: ParamMode,
}

/// Info routine dari query katalog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutineInfo {
    pub params: Vec<RoutineParam>,
    /// MsSQL: function bernilai tabel (dipanggil `SELECT * FROM f(...)`).
    pub table_valued: bool,
}

/// Nama routine tanpa daftar argumen (`s.fn(integer, text)` → `s.fn`).
fn base_name(name: &str) -> &str {
    name.split_once('(').map_or(name, |(n, _)| n).trim()
}

/// Query katalog parameter. Kolom: nama, tipe, mode, [flag table-valued].
/// `None` bila engine tidak mendukung routine.
pub fn params_query(db: &DatabaseType, kind: RoutineKind, name: &str) -> Option<String> {
    if !matches!(kind, RoutineKind::Procedure | RoutineKind::Function) {
        return None;
    }
    match db {
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT COALESCE(pr.parameter_name, ''), pr.data_type, pr.parameter_mode \
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             JOIN information_schema.parameters pr \
               ON pr.specific_schema = n.nspname AND pr.specific_name = p.proname || '_' || p.oid \
             WHERE (CASE WHEN n.nspname = 'public' THEN '' ELSE n.nspname || '.' END \
                    || p.proname || '(' || oidvectortypes(p.proargtypes) || ')') = {} \
             ORDER BY pr.ordinal_position",
            sql_literal(db, name)
        )),
        DatabaseType::MySQL => {
            let (schema, routine) = split_qualified(name, "");
            let schema_cond = if schema.is_empty() {
                "DATABASE()".to_string()
            } else {
                sql_literal(db, &schema)
            };
            Some(format!(
                "SELECT PARAMETER_NAME, DTD_IDENTIFIER, PARAMETER_MODE \
                 FROM information_schema.PARAMETERS \
                 WHERE SPECIFIC_SCHEMA = {} AND SPECIFIC_NAME = {} AND ORDINAL_POSITION > 0 \
                 ORDER BY ORDINAL_POSITION",
                schema_cond,
                sql_literal(db, &routine)
            ))
        }
        DatabaseType::MsSQL => {
            let (schema, routine) = split_qualified(name, "dbo");
            let object = sql_literal(
                db,
                &format!("{}.{}", quote_ident(db, &schema), quote_ident(db, &routine)),
            );
            Some(format!(
                "SELECT p.name, TYPE_NAME(p.user_type_id) + CASE \
                   WHEN TYPE_NAME(p.user_type_id) IN ('varchar','char','varbinary','binary') \
                     THEN '(' + CASE WHEN p.max_length = -1 THEN 'max' ELSE CAST(p.max_length AS varchar(10)) END + ')' \
                   WHEN TYPE_NAME(p.user_type_id) IN ('nvarchar','nchar') \
                     THEN '(' + CASE WHEN p.max_length = -1 THEN 'max' ELSE CAST(p.max_length / 2 AS varchar(10)) END + ')' \
                   WHEN TYPE_NAME(p.user_type_id) IN ('decimal','numeric') \
                     THEN '(' + CAST(p.precision AS varchar(3)) + ',' + CAST(p.scale AS varchar(3)) + ')' \
                   ELSE '' END, \
                 CASE WHEN p.is_output = 1 THEN 'OUT' ELSE 'IN' END, \
                 CAST(OBJECTPROPERTY(o.id, 'IsTableFunction') AS varchar(1)) \
                 FROM (SELECT OBJECT_ID({object}) AS id) o \
                 LEFT JOIN sys.parameters p ON p.object_id = o.id AND p.parameter_id > 0 \
                 ORDER BY p.parameter_id"
            ))
        }
        _ => None,
    }
}

/// Decode hasil [`params_query`]. Baris tanpa tipe (LEFT JOIN kosong) dilewati.
pub fn parse_params(db: &DatabaseType, rows: &[Vec<String>]) -> RoutineInfo {
    let clean = |s: Option<&String>| {
        s.map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && !crate::models::structs::is_null_cell(v) && v != "NULL")
    };
    let mut info = RoutineInfo::default();
    for row in rows {
        if matches!(db, DatabaseType::MsSQL) && clean(row.get(3)).as_deref() == Some("1") {
            info.table_valued = true;
        }
        let Some(data_type) = clean(row.get(1)) else {
            continue;
        };
        let name = clean(row.first()).unwrap_or_default();
        let name = name.trim_start_matches('@').to_string();
        let mode = ParamMode::parse(&clean(row.get(2)).unwrap_or_default());
        info.params.push(RoutineParam {
            name,
            data_type,
            mode,
        });
    }
    info
}

/// Nama placeholder yang aman untuk pemindai parameter (`:nama`).
fn placeholder(param: &RoutineParam, index: usize) -> String {
    let cleaned: String = param
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('_');
    if cleaned.is_empty() || cleaned.starts_with(|c: char| c.is_ascii_digit()) {
        format!(":p{}", index + 1)
    } else {
        format!(":{cleaned}")
    }
}

fn type_comment(params: &[RoutineParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mode = match p.mode {
                ParamMode::In => "IN",
                ParamMode::Out => "OUT",
                ParamMode::InOut => "INOUT",
                ParamMode::Variadic => "VARIADIC",
            };
            let name = if p.name.is_empty() {
                format!("${}", i + 1)
            } else {
                p.name.clone()
            };
            format!("--   {name} {mode} {}", p.data_type)
        })
        .collect();
    format!("-- Parameters:\n{}\n", lines.join("\n"))
}

/// Susun SQL pemanggilan dengan placeholder `:nama` untuk parameter masukan.
pub fn call_template(
    db: &DatabaseType,
    kind: RoutineKind,
    name: &str,
    info: &RoutineInfo,
) -> Option<String> {
    let base = base_name(name);
    let callee = quote_qualified(db, base);
    let params = &info.params;
    let header = format!(
        "-- {} {}. Run it to be asked for the parameter values.\n{}",
        kind.label(),
        base,
        type_comment(params)
    );
    let body = match (db, kind) {
        (DatabaseType::PostgreSQL, RoutineKind::Procedure) => {
            // Argumen OUT procedure PostgreSQL wajib diisi NULL.
            let args: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if p.mode.is_input() {
                        placeholder(p, i)
                    } else {
                        "NULL".to_string()
                    }
                })
                .collect();
            format!("CALL {}({});", callee, args.join(", "))
        }
        (DatabaseType::PostgreSQL, _) => {
            let args: Vec<String> = params
                .iter()
                .enumerate()
                .filter(|(_, p)| p.mode.is_input())
                .map(|(i, p)| placeholder(p, i))
                .collect();
            format!("SELECT * FROM {}({});", callee, args.join(", "))
        }
        (DatabaseType::MySQL, RoutineKind::Procedure) => {
            // OUT/INOUT MySQL lewat variabel sesi lalu dibaca dengan SELECT.
            let mut pre = Vec::new();
            let mut outs = Vec::new();
            let args: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let var = format!("@{}", placeholder(p, i).trim_start_matches(':'));
                    match p.mode {
                        ParamMode::Out => {
                            outs.push(var.clone());
                            var
                        }
                        ParamMode::InOut => {
                            pre.push(format!("SET {} = {};", var, placeholder(p, i)));
                            outs.push(var.clone());
                            var
                        }
                        _ => placeholder(p, i),
                    }
                })
                .collect();
            let mut sql = String::new();
            for line in pre {
                sql.push_str(&line);
                sql.push('\n');
            }
            sql.push_str(&format!("CALL {}({});", callee, args.join(", ")));
            if !outs.is_empty() {
                sql.push_str(&format!("\nSELECT {};", outs.join(", ")));
            }
            sql
        }
        (DatabaseType::MySQL, _) => {
            let args: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| placeholder(p, i))
                .collect();
            format!("SELECT {}({});", callee, args.join(", "))
        }
        (DatabaseType::MsSQL, RoutineKind::Procedure) => {
            let mut declares = Vec::new();
            let mut outs = Vec::new();
            let args: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let at = format!("@{}", p.name);
                    if p.mode.is_input() && p.mode != ParamMode::InOut {
                        format!("{at} = {}", placeholder(p, i))
                    } else {
                        let var = format!("@out_{}", placeholder(p, i).trim_start_matches(':'));
                        let init = if p.mode == ParamMode::InOut {
                            format!(" = {}", placeholder(p, i))
                        } else {
                            String::new()
                        };
                        declares.push(format!("DECLARE {var} {}{init};", p.data_type));
                        outs.push(format!("{var} AS {}", quote_ident(db, &p.name)));
                        format!("{at} = {var} OUTPUT")
                    }
                })
                .collect();
            let mut sql = String::new();
            for d in declares {
                sql.push_str(&d);
                sql.push('\n');
            }
            if args.is_empty() {
                sql.push_str(&format!("EXEC {};", callee));
            } else {
                sql.push_str(&format!("EXEC {}\n    {};", callee, args.join(",\n    ")));
            }
            if !outs.is_empty() {
                sql.push_str(&format!("\nSELECT {};", outs.join(", ")));
            }
            sql
        }
        (DatabaseType::MsSQL, _) => {
            let args: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| placeholder(p, i))
                .collect();
            if info.table_valued {
                format!("SELECT * FROM {}({});", callee, args.join(", "))
            } else {
                format!("SELECT {}({});", callee, args.join(", "))
            }
        }
        _ => return None,
    };
    Some(format!("{header}{body}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, ty: &str, mode: ParamMode) -> RoutineParam {
        RoutineParam {
            name: name.into(),
            data_type: ty.into(),
            mode,
        }
    }

    fn info(params: Vec<RoutineParam>) -> RoutineInfo {
        RoutineInfo {
            params,
            table_valued: false,
        }
    }

    #[test]
    fn postgres_function_uses_select_from_and_skips_out() {
        let sql = call_template(
            &DatabaseType::PostgreSQL,
            RoutineKind::Function,
            "billing.total_for(integer, date)",
            &info(vec![
                p("customer_id", "integer", ParamMode::In),
                p("since", "date", ParamMode::In),
                p("total", "numeric", ParamMode::Out),
            ]),
        )
        .unwrap();
        assert!(sql.contains("SELECT * FROM \"billing\".\"total_for\"(:customer_id, :since);"));
        assert_eq!(
            crate::query_params::names(&sql),
            vec![":customer_id", ":since"]
        );
    }

    #[test]
    fn postgres_procedure_passes_null_for_out() {
        let sql = call_template(
            &DatabaseType::PostgreSQL,
            RoutineKind::Procedure,
            "close_month(integer)",
            &info(vec![
                p("month_id", "integer", ParamMode::In),
                p("", "integer", ParamMode::Out),
            ]),
        )
        .unwrap();
        assert!(sql.contains("CALL \"close_month\"(:month_id, NULL);"));
    }

    #[test]
    fn mysql_procedure_reads_out_params_via_session_vars() {
        let sql = call_template(
            &DatabaseType::MySQL,
            RoutineKind::Procedure,
            "sp_stock",
            &info(vec![
                p("sku", "varchar(20)", ParamMode::In),
                p("qty", "int", ParamMode::Out),
                p("cnt", "int", ParamMode::InOut),
            ]),
        )
        .unwrap();
        assert!(sql.contains("SET @cnt = :cnt;"));
        assert!(sql.contains("CALL `sp_stock`(:sku, @qty, @cnt);"));
        assert!(sql.contains("SELECT @qty, @cnt;"));
        assert_eq!(crate::query_params::names(&sql), vec![":cnt", ":sku"]);
    }

    #[test]
    fn mssql_procedure_declares_outputs() {
        let sql = call_template(
            &DatabaseType::MsSQL,
            RoutineKind::Procedure,
            "dbo.usp_order",
            &info(vec![
                p("OrderId", "int", ParamMode::In),
                p("Status", "nvarchar(20)", ParamMode::Out),
            ]),
        )
        .unwrap();
        assert!(sql.contains("DECLARE @out_Status nvarchar(20);"));
        assert!(sql.contains("@OrderId = :OrderId"));
        assert!(sql.contains("@Status = @out_Status OUTPUT"));
        assert!(sql.contains("SELECT @out_Status AS [Status];"));
    }

    #[test]
    fn mssql_table_function_and_no_params() {
        let tvf = RoutineInfo {
            params: vec![],
            table_valued: true,
        };
        let sql =
            call_template(&DatabaseType::MsSQL, RoutineKind::Function, "dbo.f", &tvf).unwrap();
        assert!(sql.contains("SELECT * FROM [dbo].[f]();"));
        let sql = call_template(
            &DatabaseType::MsSQL,
            RoutineKind::Procedure,
            "dbo.p",
            &RoutineInfo::default(),
        )
        .unwrap();
        assert!(sql.contains("EXEC [dbo].[p];"));
    }

    #[test]
    fn parse_rows_skips_empty_left_join_and_strips_at() {
        let rows = vec![
            vec!["@a".into(), "int".into(), "IN".into(), "1".into()],
            vec![String::new(), String::new(), "IN".into(), "1".into()],
        ];
        let info = parse_params(&DatabaseType::MsSQL, &rows);
        assert!(info.table_valued);
        assert_eq!(info.params, vec![p("a", "int", ParamMode::In)]);
    }

    #[test]
    fn unsupported_engine_or_kind() {
        assert!(params_query(&DatabaseType::SQLite, RoutineKind::Procedure, "x").is_none());
        assert!(params_query(&DatabaseType::MySQL, RoutineKind::View, "x").is_none());
    }

    #[test]
    fn unnamed_params_get_positional_placeholders() {
        let sql = call_template(
            &DatabaseType::PostgreSQL,
            RoutineKind::Function,
            "f(integer)",
            &info(vec![p("", "integer", ParamMode::In)]),
        )
        .unwrap();
        assert!(sql.contains("\"f\"(:p1)"));
    }
}
