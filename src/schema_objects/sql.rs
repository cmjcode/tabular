//! Builder SQL murni untuk operasi objek skema. Tidak ada I/O di sini sehingga
//! semua fungsi bisa diuji langsung.

use crate::models::enums::DatabaseType;

/// Kutip satu identifier sesuai dialek engine.
pub fn quote_ident(db: &DatabaseType, name: &str) -> String {
    match db {
        DatabaseType::MySQL => format!("`{}`", name.replace('`', "``")),
        DatabaseType::MsSQL => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Kutip nama yang mungkin berkualifikasi (`schema.table`). Nama yang sudah
/// mengandung karakter kutip (mis. `[dbo].[t]` dari tree MsSQL) dibiarkan apa adanya.
pub fn quote_qualified(db: &DatabaseType, name: &str) -> String {
    if name.contains('[') || name.contains('`') || name.contains('"') {
        return name.to_string();
    }
    name.split('.')
        .map(|part| quote_ident(db, part))
        .collect::<Vec<_>>()
        .join(".")
}

/// Literal string SQL (kutip tunggal di-escape). MsSQL memakai prefix `N`.
pub fn sql_literal(db: &DatabaseType, value: &str) -> String {
    let escaped = value.replace('\'', "''");
    let escaped = if matches!(db, DatabaseType::MySQL) {
        escaped.replace('\\', "\\\\")
    } else {
        escaped
    };
    if matches!(db, DatabaseType::MsSQL) {
        format!("N'{}'", escaped)
    } else {
        format!("'{}'", escaped)
    }
}

/// Buang kutip MsSQL/ANSI/MySQL dari nama berkualifikasi dan pecah jadi
/// `(schema, name)`. Schema default dipakai bila nama tidak berkualifikasi.
pub fn split_qualified(name: &str, default_schema: &str) -> (String, String) {
    let parts: Vec<String> = split_name_parts(name);
    match parts.len() {
        0 => (default_schema.to_string(), String::new()),
        1 => (default_schema.to_string(), parts[0].clone()),
        n => (parts[n - 2].clone(), parts[n - 1].clone()),
    }
}

fn split_name_parts(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut closing: Option<char> = None;
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        match closing {
            Some(close) => {
                if c == close {
                    // Kutip ganda di dalam identifier ("" atau ]]) = karakter literal.
                    if chars.peek() == Some(&close) {
                        current.push(close);
                        chars.next();
                    } else {
                        closing = None;
                    }
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '[' => closing = Some(']'),
                '"' => closing = Some('"'),
                '`' => closing = Some('`'),
                '.' => parts.push(std::mem::take(&mut current)),
                _ => current.push(c),
            },
        }
    }
    parts.push(current);
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

fn require_name(new_name: &str) -> Result<&str, String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        Err("New name must not be empty".to_string())
    } else {
        Ok(trimmed)
    }
}

/// Jenis objek yang bisa di-rename dari sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Table,
    View,
    MaterializedView,
    Database,
    Schema,
}

impl RenameTarget {
    pub fn label(&self) -> &'static str {
        match self {
            RenameTarget::Table => "table",
            RenameTarget::View => "view",
            RenameTarget::MaterializedView => "materialized view",
            RenameTarget::Database => "database",
            RenameTarget::Schema => "schema",
        }
    }
}

/// SQL untuk mengganti nama objek. `mysql_tables` hanya dipakai untuk rename
/// database MySQL (tidak ada `RENAME DATABASE`): tabel dipindah satu per satu.
pub fn rename_sql(
    db: &DatabaseType,
    target: RenameTarget,
    old_name: &str,
    new_name: &str,
    mysql_tables: &[String],
) -> Result<String, String> {
    let new_name = require_name(new_name)?;
    let old_q = quote_qualified(db, old_name);
    let new_q = quote_ident(db, new_name);
    match (target, db) {
        (RenameTarget::Table | RenameTarget::View, DatabaseType::MySQL) => {
            Ok(format!("RENAME TABLE {} TO {};", old_q, new_q))
        }
        (RenameTarget::Table, DatabaseType::PostgreSQL | DatabaseType::SQLite) => {
            Ok(format!("ALTER TABLE {} RENAME TO {};", old_q, new_q))
        }
        (RenameTarget::View, DatabaseType::PostgreSQL) => {
            Ok(format!("ALTER VIEW {} RENAME TO {};", old_q, new_q))
        }
        (RenameTarget::MaterializedView, DatabaseType::PostgreSQL) => Ok(format!(
            "ALTER MATERIALIZED VIEW {} RENAME TO {};",
            old_q, new_q
        )),
        (RenameTarget::Table | RenameTarget::View, DatabaseType::MsSQL) => {
            let (schema, name) = split_qualified(old_name, "dbo");
            let object = format!("{}.{}", quote_ident(db, &schema), quote_ident(db, &name));
            Ok(format!(
                "EXEC sp_rename {}, {};",
                sql_literal(db, &object),
                sql_literal(db, new_name)
            ))
        }
        (RenameTarget::Database, DatabaseType::PostgreSQL) => Ok(format!(
            "ALTER DATABASE {} RENAME TO {};",
            quote_ident(db, old_name),
            new_q
        )),
        (RenameTarget::Database, DatabaseType::MsSQL) => Ok(format!(
            "ALTER DATABASE {} MODIFY NAME = {};",
            quote_ident(db, old_name),
            new_q
        )),
        (RenameTarget::Database, DatabaseType::MySQL) => {
            let old_db = quote_ident(db, old_name);
            let mut sql = format!(
                "-- MySQL has no RENAME DATABASE: tables are moved into a new schema.\n\
                 -- Views, routines, triggers and events are NOT moved; recreate them first.\n\
                 CREATE DATABASE {};\n",
                new_q
            );
            if !mysql_tables.is_empty() {
                let moves: Vec<String> = mysql_tables
                    .iter()
                    .map(|t| {
                        format!(
                            "{}.{} TO {}.{}",
                            old_db,
                            quote_ident(db, t),
                            new_q,
                            quote_ident(db, t)
                        )
                    })
                    .collect();
                sql.push_str(&format!("RENAME TABLE\n  {};\n", moves.join(",\n  ")));
            }
            sql.push_str(&format!(
                "-- Drop the old database after verifying the move:\n-- DROP DATABASE {};",
                old_db
            ));
            Ok(sql)
        }
        (RenameTarget::Schema, DatabaseType::PostgreSQL) => Ok(format!(
            "ALTER SCHEMA {} RENAME TO {};",
            quote_ident(db, old_name),
            new_q
        )),
        (RenameTarget::View, DatabaseType::SQLite) => Err(
            "SQLite cannot rename a view; drop it and create it again with the new name"
                .to_string(),
        ),
        (t, d) => Err(format!(
            "Renaming a {} is not supported for {:?}",
            t.label(),
            d
        )),
    }
}

/// Apakah engine mendukung rename target ini dari sidebar.
pub fn supports_rename(db: &DatabaseType, target: RenameTarget) -> bool {
    rename_sql(db, target, "a", "b", &[]).is_ok()
}

/// SQL untuk membaca comment tabel saat ini (satu baris, satu kolom).
pub fn table_comment_query(db: &DatabaseType, database: &str, table: &str) -> Option<String> {
    match db {
        DatabaseType::MySQL => Some(format!(
            "SELECT TABLE_COMMENT FROM information_schema.TABLES WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {}",
            sql_literal(db, database),
            sql_literal(db, table)
        )),
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT COALESCE(obj_description(to_regclass({})::oid, 'pg_class'), '')",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MsSQL => {
            let (schema, name) = split_qualified(table, "dbo");
            Some(format!(
                "SELECT CAST(ep.value AS NVARCHAR(4000)) FROM sys.extended_properties ep \
                 WHERE ep.major_id = OBJECT_ID({}) AND ep.minor_id = 0 AND ep.name = N'MS_Description'",
                sql_literal(
                    db,
                    &format!("{}.{}", quote_ident(db, &schema), quote_ident(db, &name))
                )
            ))
        }
        _ => None,
    }
}

/// SQL untuk mengganti comment tabel/view. Comment kosong menghapus comment.
pub fn comment_table_sql(
    db: &DatabaseType,
    table: &str,
    is_view: bool,
    comment: &str,
) -> Result<String, String> {
    let table_q = quote_qualified(db, table);
    match db {
        DatabaseType::MySQL => {
            if is_view {
                return Err("MySQL views do not support comments".to_string());
            }
            Ok(format!(
                "ALTER TABLE {} COMMENT = {};",
                table_q,
                sql_literal(db, comment)
            ))
        }
        DatabaseType::PostgreSQL => {
            let value = if comment.is_empty() {
                "NULL".to_string()
            } else {
                sql_literal(db, comment)
            };
            let kind = if is_view { "VIEW" } else { "TABLE" };
            Ok(format!("COMMENT ON {} {} IS {};", kind, table_q, value))
        }
        DatabaseType::MsSQL => {
            let (schema, name) = split_qualified(table, "dbo");
            let object = format!("{}.{}", quote_ident(db, &schema), quote_ident(db, &name));
            let level1 = if is_view { "VIEW" } else { "TABLE" };
            let args = format!(
                "@level0type = N'SCHEMA', @level0name = {}, @level1type = N'{}', @level1name = {}",
                sql_literal(db, &schema),
                level1,
                sql_literal(db, &name)
            );
            let exists = format!(
                "EXISTS (SELECT 1 FROM sys.extended_properties WHERE major_id = OBJECT_ID({}) AND minor_id = 0 AND name = N'MS_Description')",
                sql_literal(db, &object)
            );
            if comment.is_empty() {
                Ok(format!(
                    "IF {} EXEC sp_dropextendedproperty @name = N'MS_Description', {};",
                    exists, args
                ))
            } else {
                let value = sql_literal(db, comment);
                Ok(format!(
                    "IF {exists}\n  EXEC sp_updateextendedproperty @name = N'MS_Description', @value = {value}, {args};\nELSE\n  EXEC sp_addextendedproperty @name = N'MS_Description', @value = {value}, {args};"
                ))
            }
        }
        _ => Err(format!("Table comments are not supported for {:?}", db)),
    }
}

/// Operasi maintenance tabel/database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaintenanceOp {
    Vacuum,
    VacuumFull,
    VacuumAnalyze,
    Analyze,
    Reindex,
    Optimize,
    Check,
    Repair,
    UpdateStatistics,
    RebuildIndexes,
    ReorganizeIndexes,
    IntegrityCheck,
}

impl MaintenanceOp {
    pub fn label(&self) -> &'static str {
        match self {
            MaintenanceOp::Vacuum => "VACUUM",
            MaintenanceOp::VacuumFull => "VACUUM FULL",
            MaintenanceOp::VacuumAnalyze => "VACUUM ANALYZE",
            MaintenanceOp::Analyze => "ANALYZE",
            MaintenanceOp::Reindex => "REINDEX",
            MaintenanceOp::Optimize => "OPTIMIZE",
            MaintenanceOp::Check => "CHECK",
            MaintenanceOp::Repair => "REPAIR",
            MaintenanceOp::UpdateStatistics => "Update Statistics",
            MaintenanceOp::RebuildIndexes => "Rebuild Indexes",
            MaintenanceOp::ReorganizeIndexes => "Reorganize Indexes",
            MaintenanceOp::IntegrityCheck => "Integrity Check",
        }
    }

    /// Operasi yang mengunci tabel lama atau menulis ulang data.
    pub fn is_heavy(&self) -> bool {
        matches!(
            self,
            MaintenanceOp::VacuumFull
                | MaintenanceOp::Optimize
                | MaintenanceOp::Repair
                | MaintenanceOp::RebuildIndexes
                | MaintenanceOp::Reindex
        )
    }
}

/// Operasi maintenance per tabel yang tersedia untuk engine ini.
pub fn table_maintenance_ops(db: &DatabaseType) -> &'static [MaintenanceOp] {
    use MaintenanceOp::*;
    match db {
        DatabaseType::PostgreSQL => &[Vacuum, VacuumAnalyze, VacuumFull, Analyze, Reindex],
        DatabaseType::MySQL => &[Optimize, Analyze, Check, Repair],
        DatabaseType::SQLite => &[Analyze, Reindex],
        DatabaseType::MsSQL => &[
            UpdateStatistics,
            RebuildIndexes,
            ReorganizeIndexes,
            IntegrityCheck,
        ],
        _ => &[],
    }
}

/// Operasi maintenance tingkat database yang tersedia untuk engine ini.
pub fn database_maintenance_ops(db: &DatabaseType) -> &'static [MaintenanceOp] {
    use MaintenanceOp::*;
    match db {
        DatabaseType::PostgreSQL => &[VacuumAnalyze, Analyze],
        DatabaseType::SQLite => &[Vacuum, Analyze, IntegrityCheck],
        DatabaseType::MsSQL => &[UpdateStatistics, IntegrityCheck],
        _ => &[],
    }
}

/// SQL maintenance. `table = None` berarti seluruh database.
pub fn maintenance_sql(
    db: &DatabaseType,
    op: MaintenanceOp,
    table: Option<&str>,
    database: &str,
) -> Result<String, String> {
    use MaintenanceOp::*;
    let t = table.map(|t| quote_qualified(db, t));
    let target = t.as_deref().map(|t| format!(" {}", t)).unwrap_or_default();
    let unsupported = || {
        Err(format!(
            "{} is not available for {:?} {}",
            op.label(),
            db,
            if table.is_some() {
                "tables"
            } else {
                "databases"
            }
        ))
    };
    match db {
        DatabaseType::PostgreSQL => match op {
            Vacuum => Ok(format!("VACUUM{};", target)),
            VacuumFull => Ok(format!("VACUUM FULL{};", target)),
            VacuumAnalyze => Ok(format!("VACUUM (ANALYZE){};", target)),
            Analyze => Ok(format!("ANALYZE{};", target)),
            Reindex => match &t {
                Some(t) => Ok(format!("REINDEX TABLE {};", t)),
                None => Ok(format!("REINDEX DATABASE {};", quote_ident(db, database))),
            },
            _ => unsupported(),
        },
        DatabaseType::MySQL => match (&t, op) {
            (Some(t), Optimize) => Ok(format!("OPTIMIZE TABLE {};", t)),
            (Some(t), Analyze) => Ok(format!("ANALYZE TABLE {};", t)),
            (Some(t), Check) => Ok(format!("CHECK TABLE {};", t)),
            (Some(t), Repair) => Ok(format!("REPAIR TABLE {};", t)),
            _ => unsupported(),
        },
        DatabaseType::SQLite => match (&t, op) {
            (None, Vacuum) => Ok("VACUUM;".to_string()),
            (_, Analyze) => Ok(format!("ANALYZE{};", target)),
            (Some(t), Reindex) => Ok(format!("REINDEX {};", t)),
            (None, IntegrityCheck) => Ok("PRAGMA integrity_check;".to_string()),
            _ => unsupported(),
        },
        DatabaseType::MsSQL => match (&t, op) {
            (Some(t), UpdateStatistics) => Ok(format!("UPDATE STATISTICS {};", t)),
            (None, UpdateStatistics) => Ok("EXEC sp_updatestats;".to_string()),
            (Some(t), RebuildIndexes) => Ok(format!("ALTER INDEX ALL ON {} REBUILD;", t)),
            (Some(t), ReorganizeIndexes) => Ok(format!("ALTER INDEX ALL ON {} REORGANIZE;", t)),
            (Some(_), IntegrityCheck) => {
                let (schema, name) = split_qualified(table.unwrap_or_default(), "dbo");
                Ok(format!(
                    "DBCC CHECKTABLE ({});",
                    sql_literal(
                        db,
                        &format!("{}.{}", quote_ident(db, &schema), quote_ident(db, &name))
                    )
                ))
            }
            (None, IntegrityCheck) => Ok(format!("DBCC CHECKDB ({});", sql_literal(db, database))),
            _ => unsupported(),
        },
        _ => unsupported(),
    }
}

/// `REFRESH MATERIALIZED VIEW` (PostgreSQL). `CONCURRENTLY` butuh unique index
/// dan tidak bisa dipakai bersama `WITH NO DATA`.
pub fn refresh_matview_sql(name: &str, concurrently: bool, with_data: bool) -> String {
    let db = DatabaseType::PostgreSQL;
    let mut sql = String::from("REFRESH MATERIALIZED VIEW ");
    if concurrently && with_data {
        sql.push_str("CONCURRENTLY ");
    }
    sql.push_str(&quote_qualified(&db, name));
    if !with_data {
        sql.push_str(" WITH NO DATA");
    }
    sql.push(';');
    sql
}

/// Hak akses schema PostgreSQL yang bisa diberikan lewat dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaGrant {
    pub role: String,
    pub usage: bool,
    pub create: bool,
}

/// `CREATE SCHEMA` PostgreSQL dengan owner dan grant opsional.
pub fn create_schema_sql(
    name: &str,
    owner: &str,
    grants: &[SchemaGrant],
) -> Result<String, String> {
    let db = DatabaseType::PostgreSQL;
    let name = require_name(name)?;
    let mut sql = format!("CREATE SCHEMA {}", quote_ident(&db, name));
    if !owner.trim().is_empty() {
        sql.push_str(&format!(
            " AUTHORIZATION {}",
            quote_ident(&db, owner.trim())
        ));
    }
    sql.push(';');
    sql.push_str(&schema_grants_sql(name, grants));
    Ok(sql)
}

/// Ubah owner / grant schema PostgreSQL yang sudah ada.
pub fn alter_schema_sql(
    name: &str,
    new_owner: Option<&str>,
    grants: &[SchemaGrant],
    revokes: &[String],
) -> String {
    let db = DatabaseType::PostgreSQL;
    let schema = quote_ident(&db, name);
    let mut sql = String::new();
    if let Some(owner) = new_owner.map(str::trim).filter(|o| !o.is_empty()) {
        sql.push_str(&format!(
            "ALTER SCHEMA {} OWNER TO {};",
            schema,
            quote_ident(&db, owner)
        ));
    }
    for role in revokes.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
        sql.push_str(&format!(
            "\nREVOKE ALL ON SCHEMA {} FROM {};",
            schema,
            grantee(role)
        ));
    }
    sql.push_str(&schema_grants_sql(name, grants));
    sql.trim_start().to_string()
}

fn grantee(role: &str) -> String {
    if role.eq_ignore_ascii_case("public") {
        "PUBLIC".to_string()
    } else {
        quote_ident(&DatabaseType::PostgreSQL, role)
    }
}

fn schema_grants_sql(name: &str, grants: &[SchemaGrant]) -> String {
    let db = DatabaseType::PostgreSQL;
    let mut sql = String::new();
    for grant in grants {
        let role = grant.role.trim();
        if role.is_empty() {
            continue;
        }
        let mut privs = Vec::new();
        if grant.usage {
            privs.push("USAGE");
        }
        if grant.create {
            privs.push("CREATE");
        }
        if privs.is_empty() {
            continue;
        }
        sql.push_str(&format!(
            "\nGRANT {} ON SCHEMA {} TO {};",
            privs.join(", "),
            quote_ident(&db, name),
            grantee(role)
        ));
    }
    sql
}

/// `DROP SCHEMA` PostgreSQL.
pub fn drop_schema_sql(name: &str, cascade: bool) -> String {
    format!(
        "DROP SCHEMA {}{};",
        quote_ident(&DatabaseType::PostgreSQL, name),
        if cascade { " CASCADE" } else { "" }
    )
}

/// `DROP VIEW` / `DROP MATERIALIZED VIEW`.
pub fn drop_view_sql(db: &DatabaseType, name: &str, materialized: bool) -> String {
    let kind = if materialized {
        "MATERIALIZED VIEW"
    } else {
        "VIEW"
    };
    format!("DROP {} IF EXISTS {};", kind, quote_qualified(db, name))
}

fn quote_column_list(db: &DatabaseType, columns: &str) -> Result<String, String> {
    let cols: Vec<String> = columns
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| quote_ident(db, c))
        .collect();
    if cols.is_empty() {
        Err("At least one column is required".to_string())
    } else {
        Ok(cols.join(", "))
    }
}

/// Aksi referensial yang valid untuk ON DELETE / ON UPDATE.
pub const FK_ACTIONS: [&str; 5] = [
    "NO ACTION",
    "RESTRICT",
    "CASCADE",
    "SET NULL",
    "SET DEFAULT",
];

/// Definisi foreign key baru dari form structure editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignKeyDraft {
    pub name: String,
    /// Kolom dipisah koma.
    pub columns: String,
    pub ref_table: String,
    /// Kolom referensi dipisah koma.
    pub ref_columns: String,
    pub on_delete: String,
    pub on_update: String,
}

pub fn add_foreign_key_sql(
    db: &DatabaseType,
    table: &str,
    fk: &ForeignKeyDraft,
) -> Result<String, String> {
    if matches!(db, DatabaseType::SQLite) {
        return Err(
            "SQLite cannot add a foreign key to an existing table; recreate the table from its DDL"
                .to_string(),
        );
    }
    if fk.ref_table.trim().is_empty() {
        return Err("Referenced table is required".to_string());
    }
    let cols = quote_column_list(db, &fk.columns)?;
    let ref_cols = quote_column_list(db, &fk.ref_columns)?;
    if cols.split(", ").count() != ref_cols.split(", ").count() {
        return Err("Column and referenced column counts must match".to_string());
    }
    let mut sql = format!("ALTER TABLE {} ADD ", quote_qualified(db, table));
    if !fk.name.trim().is_empty() {
        sql.push_str(&format!("CONSTRAINT {} ", quote_ident(db, fk.name.trim())));
    }
    sql.push_str(&format!(
        "FOREIGN KEY ({}) REFERENCES {} ({})",
        cols,
        quote_qualified(db, fk.ref_table.trim()),
        ref_cols
    ));
    for (clause, action) in [("ON DELETE", &fk.on_delete), ("ON UPDATE", &fk.on_update)] {
        let action = action.trim().to_ascii_uppercase();
        if action.is_empty() || action == "NO ACTION" {
            continue;
        }
        if !FK_ACTIONS.contains(&action.as_str()) {
            return Err(format!("Unknown referential action: {}", action));
        }
        // SQL Server tidak mengenal RESTRICT (NO ACTION setara).
        if matches!(db, DatabaseType::MsSQL) && action == "RESTRICT" {
            continue;
        }
        sql.push_str(&format!(" {} {}", clause, action));
    }
    sql.push(';');
    Ok(sql)
}

/// Jenis constraint yang di-drop dari structure editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintKind {
    ForeignKey,
    Check,
}

pub fn drop_constraint_sql(
    db: &DatabaseType,
    table: &str,
    kind: ConstraintKind,
    name: &str,
) -> Result<String, String> {
    let table_q = quote_qualified(db, table);
    let name_q = quote_ident(db, name);
    match (db, kind) {
        (DatabaseType::SQLite, _) => {
            Err("SQLite cannot drop a constraint; recreate the table from its DDL".to_string())
        }
        (DatabaseType::MySQL, ConstraintKind::ForeignKey) => Ok(format!(
            "ALTER TABLE {} DROP FOREIGN KEY {};",
            table_q, name_q
        )),
        (DatabaseType::MySQL, ConstraintKind::Check) => {
            Ok(format!("ALTER TABLE {} DROP CHECK {};", table_q, name_q))
        }
        _ => Ok(format!(
            "ALTER TABLE {} DROP CONSTRAINT {};",
            table_q, name_q
        )),
    }
}

pub fn add_check_sql(
    db: &DatabaseType,
    table: &str,
    name: &str,
    expr: &str,
) -> Result<String, String> {
    if matches!(db, DatabaseType::SQLite) {
        return Err(
            "SQLite cannot add a check constraint to an existing table; recreate the table from its DDL"
                .to_string(),
        );
    }
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("Check expression is required".to_string());
    }
    let expr = expr
        .strip_prefix('(')
        .and_then(|e| e.strip_suffix(')'))
        .unwrap_or(expr);
    let constraint = if name.trim().is_empty() {
        String::new()
    } else {
        format!("CONSTRAINT {} ", quote_ident(db, name.trim()))
    };
    Ok(format!(
        "ALTER TABLE {} ADD {}CHECK ({});",
        quote_qualified(db, table),
        constraint,
        expr
    ))
}

pub fn drop_trigger_sql(db: &DatabaseType, table: &str, trigger: &str) -> String {
    match db {
        DatabaseType::PostgreSQL => format!(
            "DROP TRIGGER {} ON {};",
            quote_ident(db, trigger),
            quote_qualified(db, table)
        ),
        DatabaseType::MsSQL => {
            let (schema, _) = split_qualified(table, "dbo");
            format!(
                "DROP TRIGGER {}.{};",
                quote_ident(db, &schema),
                quote_ident(db, trigger)
            )
        }
        _ => format!("DROP TRIGGER {};", quote_ident(db, trigger)),
    }
}

/// Template `CREATE TRIGGER` per engine untuk dibuka di editor.
pub fn trigger_template(db: &DatabaseType, table: &str) -> String {
    let (_, bare) = split_qualified(table, "");
    let table_q = quote_qualified(db, table);
    match db {
        DatabaseType::PostgreSQL => format!(
            "CREATE OR REPLACE FUNCTION {fn_name}() RETURNS trigger AS $$\nBEGIN\n    -- NEW / OLD are available here\n    RETURN NEW;\nEND;\n$$ LANGUAGE plpgsql;\n\nCREATE TRIGGER {trg}\n    BEFORE INSERT OR UPDATE ON {table}\n    FOR EACH ROW EXECUTE FUNCTION {fn_name}();",
            fn_name = quote_ident(db, &format!("{}_trigger_fn", bare)),
            trg = quote_ident(db, &format!("{}_trigger", bare)),
            table = table_q
        ),
        DatabaseType::MySQL => format!(
            "CREATE TRIGGER {trg}\n    BEFORE INSERT ON {table}\n    FOR EACH ROW\nBEGIN\n    -- SET NEW.column = ...;\nEND;",
            trg = quote_ident(db, &format!("{}_before_insert", bare)),
            table = table_q
        ),
        DatabaseType::MsSQL => {
            let (schema, _) = split_qualified(table, "dbo");
            format!(
                "CREATE TRIGGER {schema}.{trg}\n    ON {table}\n    AFTER INSERT, UPDATE\nAS\nBEGIN\n    SET NOCOUNT ON;\n    -- inserted / deleted are available here\nEND;",
                schema = quote_ident(db, &schema),
                trg = quote_ident(db, &format!("{}_after_change", bare)),
                table = table_q
            )
        }
        _ => format!(
            "CREATE TRIGGER {trg}\n    AFTER INSERT ON {table}\n    FOR EACH ROW\nBEGIN\n    -- statements\nEND;",
            trg = quote_ident(db, &format!("{}_after_insert", bare)),
            table = table_q
        ),
    }
}

/// `ADD COLUMN ... GENERATED ALWAYS AS (expr)` / computed column MsSQL.
pub fn add_generated_column_sql(
    db: &DatabaseType,
    table: &str,
    column: &str,
    data_type: &str,
    expr: &str,
    stored: bool,
) -> Result<String, String> {
    let column = require_name(column)?;
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("Generation expression is required".to_string());
    }
    let table_q = quote_qualified(db, table);
    let col_q = quote_ident(db, column);
    let data_type = data_type.trim();
    match db {
        DatabaseType::MsSQL => Ok(format!(
            "ALTER TABLE {} ADD {} AS ({}){};",
            table_q,
            col_q,
            expr,
            if stored { " PERSISTED" } else { "" }
        )),
        _ => {
            if data_type.is_empty() {
                return Err("Data type is required".to_string());
            }
            let kind = match (db, stored) {
                // PostgreSQL hanya mendukung STORED; SQLite hanya VIRTUAL lewat ALTER.
                (DatabaseType::PostgreSQL, _) => "STORED",
                (DatabaseType::SQLite, true) => {
                    return Err(
                        "SQLite can only add VIRTUAL generated columns to an existing table"
                            .to_string(),
                    );
                }
                (_, true) => "STORED",
                (_, false) => "VIRTUAL",
            };
            Ok(format!(
                "ALTER TABLE {} ADD COLUMN {} {} GENERATED ALWAYS AS ({}) {};",
                table_q, col_q, data_type, expr, kind
            ))
        }
    }
}

/// Perubahan tipe buatan user PostgreSQL (enum, composite, domain, range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeEdit {
    /// `position`: `Some((true, label))` = BEFORE label, `Some((false, label))` = AFTER label.
    AddEnumValue {
        value: String,
        position: Option<(bool, String)>,
    },
    RenameEnumValue {
        from: String,
        to: String,
    },
    AddAttribute {
        name: String,
        data_type: String,
    },
    DropAttribute {
        name: String,
        cascade: bool,
    },
    RenameAttribute {
        from: String,
        to: String,
    },
    /// `None` = DROP DEFAULT.
    SetDomainDefault(Option<String>),
    SetDomainNotNull(bool),
    AddDomainCheck {
        name: String,
        expr: String,
    },
    DropDomainConstraint {
        name: String,
    },
    RenameType {
        to: String,
    },
    DropType {
        cascade: bool,
    },
}

/// SQL `ALTER TYPE` / `ALTER DOMAIN` / `DROP` untuk [`TypeEdit`]. `is_domain`
/// memilih kata kunci `DOMAIN` karena domain tidak bisa diubah lewat `ALTER TYPE`.
pub fn alter_type_sql(name: &str, is_domain: bool, edit: &TypeEdit) -> Result<String, String> {
    let db = DatabaseType::PostgreSQL;
    let target = quote_qualified(&db, name);
    let kw = if is_domain { "DOMAIN" } else { "TYPE" };
    let lit = |v: &str| sql_literal(&db, v);
    let nonempty = |v: &str, what: &str| -> Result<(), String> {
        if v.trim().is_empty() {
            Err(format!("{} must not be empty", what))
        } else {
            Ok(())
        }
    };
    match edit {
        TypeEdit::AddEnumValue { value, position } => {
            nonempty(value, "Value")?;
            let pos = match position {
                Some((before, label)) if !label.is_empty() => format!(
                    " {} {}",
                    if *before { "BEFORE" } else { "AFTER" },
                    lit(label)
                ),
                _ => String::new(),
            };
            Ok(format!(
                "ALTER TYPE {} ADD VALUE IF NOT EXISTS {}{};",
                target,
                lit(value),
                pos
            ))
        }
        TypeEdit::RenameEnumValue { from, to } => {
            nonempty(from, "Current value")?;
            nonempty(to, "New value")?;
            Ok(format!(
                "ALTER TYPE {} RENAME VALUE {} TO {};",
                target,
                lit(from),
                lit(to)
            ))
        }
        TypeEdit::AddAttribute { name, data_type } => {
            nonempty(name, "Attribute name")?;
            nonempty(data_type, "Data type")?;
            Ok(format!(
                "ALTER TYPE {} ADD ATTRIBUTE {} {};",
                target,
                quote_ident(&db, name.trim()),
                data_type.trim()
            ))
        }
        TypeEdit::DropAttribute { name, cascade } => {
            nonempty(name, "Attribute name")?;
            Ok(format!(
                "ALTER TYPE {} DROP ATTRIBUTE {}{};",
                target,
                quote_ident(&db, name.trim()),
                if *cascade { " CASCADE" } else { "" }
            ))
        }
        TypeEdit::RenameAttribute { from, to } => {
            nonempty(from, "Attribute name")?;
            nonempty(to, "New name")?;
            Ok(format!(
                "ALTER TYPE {} RENAME ATTRIBUTE {} TO {};",
                target,
                quote_ident(&db, from.trim()),
                quote_ident(&db, to.trim())
            ))
        }
        TypeEdit::SetDomainDefault(expr) => match expr.as_deref().map(str::trim) {
            Some(e) if !e.is_empty() => Ok(format!("ALTER DOMAIN {} SET DEFAULT {};", target, e)),
            _ => Ok(format!("ALTER DOMAIN {} DROP DEFAULT;", target)),
        },
        TypeEdit::SetDomainNotNull(not_null) => Ok(format!(
            "ALTER DOMAIN {} {} NOT NULL;",
            target,
            if *not_null { "SET" } else { "DROP" }
        )),
        TypeEdit::AddDomainCheck { name, expr } => {
            nonempty(expr, "Check expression")?;
            let constraint = if name.trim().is_empty() {
                String::new()
            } else {
                format!("CONSTRAINT {} ", quote_ident(&db, name.trim()))
            };
            Ok(format!(
                "ALTER DOMAIN {} ADD {}CHECK ({});",
                target,
                constraint,
                expr.trim()
            ))
        }
        TypeEdit::DropDomainConstraint { name } => {
            nonempty(name, "Constraint name")?;
            Ok(format!(
                "ALTER DOMAIN {} DROP CONSTRAINT {};",
                target,
                quote_ident(&db, name.trim())
            ))
        }
        TypeEdit::RenameType { to } => {
            let to = require_name(to)?;
            Ok(format!(
                "ALTER {} {} RENAME TO {};",
                kw,
                target,
                quote_ident(&db, to)
            ))
        }
        TypeEdit::DropType { cascade } => Ok(format!(
            "DROP {} {}{};",
            kw,
            target,
            if *cascade { " CASCADE" } else { "" }
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: DatabaseType = DatabaseType::PostgreSQL;

    #[test]
    fn structure_constraint_statements() {
        let fk = ForeignKeyDraft {
            name: "fk_order_user".into(),
            columns: "user_id".into(),
            ref_table: "users".into(),
            ref_columns: "id".into(),
            on_delete: "cascade".into(),
            on_update: "NO ACTION".into(),
        };
        assert_eq!(
            add_foreign_key_sql(&PG, "orders", &fk).unwrap(),
            "ALTER TABLE \"orders\" ADD CONSTRAINT \"fk_order_user\" FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\") ON DELETE CASCADE;"
        );
        let bad = ForeignKeyDraft {
            ref_columns: "id, tenant_id".into(),
            ..fk.clone()
        };
        assert!(add_foreign_key_sql(&PG, "orders", &bad).is_err());
        assert!(add_foreign_key_sql(&LITE, "orders", &fk).is_err());
        assert_eq!(
            drop_constraint_sql(&MY, "orders", ConstraintKind::ForeignKey, "fk").unwrap(),
            "ALTER TABLE `orders` DROP FOREIGN KEY `fk`;"
        );
        assert_eq!(
            drop_constraint_sql(&MS, "[dbo].[orders]", ConstraintKind::Check, "ck").unwrap(),
            "ALTER TABLE [dbo].[orders] DROP CONSTRAINT [ck];"
        );
        assert_eq!(
            add_check_sql(&PG, "orders", "", "(qty > 0)").unwrap(),
            "ALTER TABLE \"orders\" ADD CHECK (qty > 0);"
        );
        assert_eq!(
            drop_trigger_sql(&PG, "orders", "trg"),
            "DROP TRIGGER \"trg\" ON \"orders\";"
        );
        assert_eq!(
            drop_trigger_sql(&MS, "[sales].[orders]", "trg"),
            "DROP TRIGGER [sales].[trg];"
        );
        assert!(trigger_template(&PG, "public.orders").contains("EXECUTE FUNCTION"));
    }

    #[test]
    fn generated_column_statements() {
        assert_eq!(
            add_generated_column_sql(&PG, "t", "total", "numeric", "qty * price", false).unwrap(),
            "ALTER TABLE \"t\" ADD COLUMN \"total\" numeric GENERATED ALWAYS AS (qty * price) STORED;"
        );
        assert_eq!(
            add_generated_column_sql(&MY, "t", "total", "decimal(10,2)", "qty * price", false)
                .unwrap(),
            "ALTER TABLE `t` ADD COLUMN `total` decimal(10,2) GENERATED ALWAYS AS (qty * price) VIRTUAL;"
        );
        assert_eq!(
            add_generated_column_sql(&MS, "t", "total", "", "qty * price", true).unwrap(),
            "ALTER TABLE [t] ADD [total] AS (qty * price) PERSISTED;"
        );
        assert!(add_generated_column_sql(&LITE, "t", "x", "int", "a+1", true).is_err());
    }

    #[test]
    fn alter_type_statements() {
        assert_eq!(
            alter_type_sql(
                "mood",
                false,
                &TypeEdit::AddEnumValue {
                    value: "meh".into(),
                    position: Some((true, "happy".into()))
                }
            )
            .unwrap(),
            "ALTER TYPE \"mood\" ADD VALUE IF NOT EXISTS 'meh' BEFORE 'happy';"
        );
        assert_eq!(
            alter_type_sql(
                "s.addr",
                false,
                &TypeEdit::AddAttribute {
                    name: "zip".into(),
                    data_type: "varchar(10)".into()
                }
            )
            .unwrap(),
            "ALTER TYPE \"s\".\"addr\" ADD ATTRIBUTE \"zip\" varchar(10);"
        );
        assert_eq!(
            alter_type_sql("email", true, &TypeEdit::SetDomainDefault(None)).unwrap(),
            "ALTER DOMAIN \"email\" DROP DEFAULT;"
        );
        assert_eq!(
            alter_type_sql("email", true, &TypeEdit::DropType { cascade: true }).unwrap(),
            "DROP DOMAIN \"email\" CASCADE;"
        );
        assert!(
            alter_type_sql(
                "mood",
                false,
                &TypeEdit::RenameEnumValue {
                    from: "a".into(),
                    to: " ".into()
                }
            )
            .is_err()
        );
    }
    const MY: DatabaseType = DatabaseType::MySQL;
    const MS: DatabaseType = DatabaseType::MsSQL;
    const LITE: DatabaseType = DatabaseType::SQLite;

    #[test]
    fn quoting_escapes_per_dialect() {
        assert_eq!(quote_ident(&MY, "a`b"), "`a``b`");
        assert_eq!(quote_ident(&PG, "a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_ident(&MS, "a]b"), "[a]]b]");
        assert_eq!(quote_qualified(&PG, "s.t"), "\"s\".\"t\"");
        assert_eq!(quote_qualified(&MS, "[dbo].[t]"), "[dbo].[t]");
        assert_eq!(sql_literal(&MY, "it's \\"), "'it''s \\\\'");
        assert_eq!(sql_literal(&MS, "x"), "N'x'");
    }

    #[test]
    fn split_qualified_handles_quotes() {
        assert_eq!(
            split_qualified("[dbo].[my.table]", "dbo"),
            ("dbo".to_string(), "my.table".to_string())
        );
        assert_eq!(
            split_qualified("orders", "public"),
            ("public".to_string(), "orders".to_string())
        );
        assert_eq!(
            split_qualified("\"a\"\"b\".c", ""),
            ("a\"b".to_string(), "c".to_string())
        );
    }

    #[test]
    fn rename_table_per_engine() {
        assert_eq!(
            rename_sql(&MY, RenameTarget::Table, "a", "b", &[]).unwrap(),
            "RENAME TABLE `a` TO `b`;"
        );
        assert_eq!(
            rename_sql(&PG, RenameTarget::Table, "a", "b", &[]).unwrap(),
            "ALTER TABLE \"a\" RENAME TO \"b\";"
        );
        assert_eq!(
            rename_sql(&MS, RenameTarget::Table, "[dbo].[a]", "b", &[]).unwrap(),
            "EXEC sp_rename N'[dbo].[a]', N'b';"
        );
        assert!(rename_sql(&PG, RenameTarget::Table, "a", "  ", &[]).is_err());
        assert!(!supports_rename(&LITE, RenameTarget::View));
        assert!(!supports_rename(&LITE, RenameTarget::Database));
    }

    #[test]
    fn rename_mysql_database_moves_tables() {
        let sql = rename_sql(
            &MY,
            RenameTarget::Database,
            "old",
            "new",
            &["t1".to_string(), "t2".to_string()],
        )
        .unwrap();
        assert!(sql.contains("CREATE DATABASE `new`;"));
        assert!(sql.contains("`old`.`t1` TO `new`.`t1`,\n  `old`.`t2` TO `new`.`t2`;"));
        assert!(sql.contains("-- DROP DATABASE `old`;"));
    }

    #[test]
    fn table_comment_sql() {
        assert_eq!(
            comment_table_sql(&PG, "t", false, "").unwrap(),
            "COMMENT ON TABLE \"t\" IS NULL;"
        );
        assert_eq!(
            comment_table_sql(&MY, "t", false, "it's").unwrap(),
            "ALTER TABLE `t` COMMENT = 'it''s';"
        );
        let ms = comment_table_sql(&MS, "[sales].[t]", false, "x").unwrap();
        assert!(ms.contains("sp_updateextendedproperty"));
        assert!(ms.contains("@level0name = N'sales'"));
        assert!(comment_table_sql(&LITE, "t", false, "x").is_err());
    }

    #[test]
    fn maintenance_ops_are_valid_for_their_engine() {
        for db in [PG, MY, LITE, MS] {
            for op in table_maintenance_ops(&db) {
                assert!(
                    maintenance_sql(&db, *op, Some("t"), "d").is_ok(),
                    "{:?} {:?}",
                    db,
                    op
                );
            }
            for op in database_maintenance_ops(&db) {
                assert!(
                    maintenance_sql(&db, *op, None, "d").is_ok(),
                    "{:?} {:?}",
                    db,
                    op
                );
            }
        }
        assert_eq!(
            maintenance_sql(&PG, MaintenanceOp::VacuumAnalyze, Some("t"), "d").unwrap(),
            "VACUUM (ANALYZE) \"t\";"
        );
        assert!(maintenance_sql(&MY, MaintenanceOp::Vacuum, Some("t"), "d").is_err());
    }

    #[test]
    fn refresh_matview_variants() {
        assert_eq!(
            refresh_matview_sql("mv", true, true),
            "REFRESH MATERIALIZED VIEW CONCURRENTLY \"mv\";"
        );
        assert_eq!(
            refresh_matview_sql("s.mv", true, false),
            "REFRESH MATERIALIZED VIEW \"s\".\"mv\" WITH NO DATA;"
        );
    }

    #[test]
    fn schema_create_and_alter() {
        let grants = vec![
            SchemaGrant {
                role: "app".into(),
                usage: true,
                create: false,
            },
            SchemaGrant {
                role: "public".into(),
                usage: true,
                create: true,
            },
        ];
        assert_eq!(
            create_schema_sql("sales", "owner1", &grants).unwrap(),
            "CREATE SCHEMA \"sales\" AUTHORIZATION \"owner1\";\nGRANT USAGE ON SCHEMA \"sales\" TO \"app\";\nGRANT USAGE, CREATE ON SCHEMA \"sales\" TO PUBLIC;"
        );
        assert_eq!(
            alter_schema_sql("sales", Some("bob"), &[], &["app".into()]),
            "ALTER SCHEMA \"sales\" OWNER TO \"bob\";\nREVOKE ALL ON SCHEMA \"sales\" FROM \"app\";"
        );
        assert_eq!(drop_schema_sql("s", true), "DROP SCHEMA \"s\" CASCADE;");
    }
}
