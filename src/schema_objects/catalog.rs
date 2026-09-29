//! Query katalog (read-only) per engine: daftar objek skema, source routine,
//! partisi, constraint, trigger, kolom generated. Semua query mengembalikan
//! kolom teks agar mudah di-decode oleh eksekutor headless.

use super::sql::{quote_ident, quote_qualified, split_qualified, sql_literal};
use crate::models::enums::DatabaseType;

const PG_SYSTEM_SCHEMA_FILTER: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'";

/// Objek milik extension (mis. PostGIS) tidak ditampilkan di sidebar.
fn pg_not_extension(oid_expr: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM pg_depend dep WHERE dep.objid = {} AND dep.deptype = 'e')",
        oid_expr
    )
}

/// Nama tampilan PostgreSQL: tanpa prefix untuk schema `public`.
fn pg_display(schema_expr: &str, name_expr: &str) -> String {
    format!(
        "(CASE WHEN {s} = 'public' THEN '' ELSE {s} || '.' END || {n})",
        s = schema_expr,
        n = name_expr
    )
}

/// Jenis objek PostgreSQL tambahan di sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PgObjectKind {
    MaterializedView,
    UserType,
    Function,
    Procedure,
    Trigger,
}

impl PgObjectKind {
    /// Kunci `table_type` di `table_cache`.
    pub fn cache_key(&self) -> &'static str {
        match self {
            PgObjectKind::MaterializedView => "matview",
            PgObjectKind::UserType => "type",
            PgObjectKind::Function => "function",
            PgObjectKind::Procedure => "procedure",
            PgObjectKind::Trigger => "trigger",
        }
    }

    pub fn from_cache_key(key: &str) -> Option<Self> {
        match key {
            "matview" => Some(PgObjectKind::MaterializedView),
            "type" => Some(PgObjectKind::UserType),
            "function" => Some(PgObjectKind::Function),
            "procedure" => Some(PgObjectKind::Procedure),
            "trigger" => Some(PgObjectKind::Trigger),
            _ => None,
        }
    }
}

fn pg_function_display() -> String {
    pg_display(
        "n.nspname",
        "p.proname || '(' || oidvectortypes(p.proargtypes) || ')'",
    )
}

fn pg_trigger_display() -> String {
    format!(
        "(t.tgname || ' on ' || {})",
        pg_display("n.nspname", "c.relname")
    )
}

fn pg_type_display() -> String {
    pg_display("n.nspname", "t.typname")
}

fn pg_relation_display() -> String {
    pg_display("n.nspname", "c.relname")
}

/// Query daftar objek PostgreSQL (satu kolom: nama tampilan).
pub fn pg_list_objects_sql(kind: PgObjectKind) -> String {
    match kind {
        PgObjectKind::MaterializedView => format!(
            "SELECT {d} FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind = 'm' AND {f} ORDER BY 1",
            d = pg_relation_display(),
            f = PG_SYSTEM_SCHEMA_FILTER
        ),
        PgObjectKind::UserType => format!(
            "SELECT {d} FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace \
             LEFT JOIN pg_class c ON c.oid = t.typrelid \
             WHERE {f} AND (t.typtype IN ('e', 'd', 'r') OR (t.typtype = 'c' AND c.relkind = 'c')) \
             AND {ext} ORDER BY 1",
            d = pg_type_display(),
            f = PG_SYSTEM_SCHEMA_FILTER,
            ext = pg_not_extension("t.oid")
        ),
        PgObjectKind::Function | PgObjectKind::Procedure => format!(
            "SELECT {d} FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE {f} AND p.prokind = '{k}' AND {ext} ORDER BY 1",
            d = pg_function_display(),
            f = PG_SYSTEM_SCHEMA_FILTER,
            k = if kind == PgObjectKind::Function {
                'f'
            } else {
                'p'
            },
            ext = pg_not_extension("p.oid")
        ),
        PgObjectKind::Trigger => format!(
            "SELECT {d} FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE NOT t.tgisinternal AND {f} ORDER BY 1",
            d = pg_trigger_display(),
            f = PG_SYSTEM_SCHEMA_FILTER
        ),
    }
}

/// Jenis objek yang source-nya bisa dilihat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutineKind {
    Procedure,
    Function,
    Trigger,
    Event,
    View,
    MaterializedView,
    UserType,
}

impl RoutineKind {
    pub fn label(&self) -> &'static str {
        match self {
            RoutineKind::Procedure => "Procedure",
            RoutineKind::Function => "Function",
            RoutineKind::Trigger => "Trigger",
            RoutineKind::Event => "Event",
            RoutineKind::View => "View",
            RoutineKind::MaterializedView => "Materialized View",
            RoutineKind::UserType => "Type",
        }
    }
}

/// Query source satu objek. Hasil: satu baris; kolom yang berisi source
/// ditunjuk oleh nama header (`Some`) atau kolom pertama (`None`).
pub fn routine_source_query(
    db: &DatabaseType,
    kind: RoutineKind,
    name: &str,
) -> Option<(String, Option<&'static str>)> {
    match db {
        DatabaseType::PostgreSQL => {
            let lit = sql_literal(db, name);
            let sql = match kind {
                RoutineKind::Function | RoutineKind::Procedure => format!(
                    "SELECT pg_get_functiondef(p.oid) FROM pg_proc p \
                     JOIN pg_namespace n ON n.oid = p.pronamespace WHERE {} = {} LIMIT 1",
                    pg_function_display(),
                    lit
                ),
                RoutineKind::Trigger => format!(
                    "SELECT pg_get_triggerdef(t.oid, true) || ';' FROM pg_trigger t \
                     JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE NOT t.tgisinternal AND {} = {} LIMIT 1",
                    pg_trigger_display(),
                    lit
                ),
                RoutineKind::View => format!(
                    "SELECT 'CREATE OR REPLACE VIEW ' || c.oid::regclass::text || E' AS\\n' || pg_get_viewdef(c.oid, true) \
                     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relkind = 'v' AND {} = {} LIMIT 1",
                    pg_relation_display(),
                    lit
                ),
                RoutineKind::MaterializedView => format!(
                    "SELECT 'CREATE MATERIALIZED VIEW ' || c.oid::regclass::text || E' AS\\n' \
                     || rtrim(pg_get_viewdef(c.oid, true), E' ;\\n') \
                     || CASE WHEN c.relispopulated THEN E'\\nWITH DATA;' ELSE E'\\nWITH NO DATA;' END \
                     || COALESCE((SELECT E'\\n\\n' || string_agg(pg_get_indexdef(i.indexrelid) || ';', E'\\n') \
                        FROM pg_index i WHERE i.indrelid = c.oid), '') \
                     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relkind = 'm' AND {} = {} LIMIT 1",
                    pg_relation_display(),
                    lit
                ),
                RoutineKind::UserType => return Some((pg_type_detail_sql(name), None)),
                RoutineKind::Event => return None,
            };
            Some((sql, None))
        }
        DatabaseType::MySQL => {
            let q = quote_qualified(db, name);
            match kind {
                RoutineKind::Procedure => Some((
                    format!("SHOW CREATE PROCEDURE {}", q),
                    Some("Create Procedure"),
                )),
                RoutineKind::Function => Some((
                    format!("SHOW CREATE FUNCTION {}", q),
                    Some("Create Function"),
                )),
                RoutineKind::Trigger => Some((
                    format!("SHOW CREATE TRIGGER {}", q),
                    Some("SQL Original Statement"),
                )),
                RoutineKind::Event => {
                    Some((format!("SHOW CREATE EVENT {}", q), Some("Create Event")))
                }
                RoutineKind::View => Some((format!("SHOW CREATE VIEW {}", q), Some("Create View"))),
                _ => None,
            }
        }
        DatabaseType::MsSQL => match kind {
            RoutineKind::MaterializedView | RoutineKind::UserType | RoutineKind::Event => None,
            _ => Some((
                format!(
                    "SELECT OBJECT_DEFINITION(OBJECT_ID({}))",
                    mssql_object_literal(name)
                ),
                None,
            )),
        },
        DatabaseType::SQLite => {
            let t = match kind {
                RoutineKind::Trigger => "trigger",
                RoutineKind::View => "view",
                _ => return None,
            };
            Some((
                format!(
                    "SELECT sql || ';' FROM sqlite_master WHERE type = '{}' AND name = {}",
                    t,
                    sql_literal(db, name)
                ),
                None,
            ))
        }
        _ => None,
    }
}

/// Query DDL tipe PostgreSQL (kolom 1: DDL, kolom 2: `typtype`).
pub fn pg_type_detail_sql(name: &str) -> String {
    let lit = sql_literal(&DatabaseType::PostgreSQL, name);
    format!(
        "SELECT CASE t.typtype \
           WHEN 'e' THEN 'CREATE TYPE ' || t.oid::regtype::text || ' AS ENUM (' || E'\\n    ' || \
             COALESCE((SELECT string_agg(quote_literal(e.enumlabel), E',\\n    ' ORDER BY e.enumsortorder) FROM pg_enum e WHERE e.enumtypid = t.oid), '') || E'\\n);' \
           WHEN 'c' THEN 'CREATE TYPE ' || t.oid::regtype::text || ' AS (' || E'\\n    ' || \
             COALESCE((SELECT string_agg(quote_ident(a.attname) || ' ' || format_type(a.atttypid, a.atttypmod), E',\\n    ' ORDER BY a.attnum) \
              FROM pg_attribute a WHERE a.attrelid = t.typrelid AND a.attnum > 0 AND NOT a.attisdropped), '') || E'\\n);' \
           WHEN 'd' THEN 'CREATE DOMAIN ' || t.oid::regtype::text || ' AS ' || format_type(t.typbasetype, t.typtypmod) \
             || CASE WHEN t.typdefault IS NOT NULL THEN ' DEFAULT ' || t.typdefault ELSE '' END \
             || CASE WHEN t.typnotnull THEN ' NOT NULL' ELSE '' END \
             || COALESCE((SELECT string_agg(E'\\n    CONSTRAINT ' || quote_ident(con.conname) || ' ' || pg_get_constraintdef(con.oid, true), '' ORDER BY con.conname) \
                FROM pg_constraint con WHERE con.contypid = t.oid), '') || ';' \
           WHEN 'r' THEN 'CREATE TYPE ' || t.oid::regtype::text || ' AS RANGE (subtype = ' \
             || COALESCE((SELECT format_type(r.rngsubtype, NULL) FROM pg_range r WHERE r.rngtypid = t.oid), '?') || ');' \
           ELSE '-- unsupported type kind ' || t.typtype::text END \
         || COALESCE(E'\\n\\nCOMMENT ON TYPE ' || t.oid::regtype::text || ' IS ' || quote_literal(obj_description(t.oid, 'pg_type')) || ';', '') AS ddl, \
         t.typtype::text AS kind \
         FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace WHERE {} = {} LIMIT 1",
        pg_type_display(),
        lit
    )
}

/// Label enum PostgreSQL untuk editor inline (satu kolom, urut).
pub fn pg_enum_labels_sql(name: &str) -> String {
    format!(
        "SELECT e.enumlabel FROM pg_enum e JOIN pg_type t ON t.oid = e.enumtypid \
         JOIN pg_namespace n ON n.oid = t.typnamespace WHERE {} = {} ORDER BY e.enumsortorder",
        pg_type_display(),
        sql_literal(&DatabaseType::PostgreSQL, name)
    )
}

/// Satu partisi tabel beserta batas dan perkiraan jumlah baris.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionInfo {
    pub name: String,
    pub bound: String,
    pub rows: Option<i64>,
}

impl PartitionInfo {
    /// Label ringkas untuk sidebar.
    pub fn label(&self) -> String {
        let mut label = self.name.clone();
        if !self.bound.trim().is_empty() {
            label.push_str(" · ");
            label.push_str(self.bound.trim());
        }
        if let Some(rows) = self.rows {
            label.push_str(&format!(" · ~{} rows", format_count(rows)));
        }
        label
    }
}

fn format_count(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{}", out) } else { out }
}

/// Query partisi (kolom: nama, batas, jumlah baris sebagai teks).
pub fn partitions_query(db: &DatabaseType, database: &str, table: &str) -> Option<String> {
    match db {
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT c.oid::regclass::text, COALESCE(pg_get_expr(c.relpartbound, c.oid), ''), \
             CASE WHEN c.reltuples < 0 THEN '' ELSE c.reltuples::bigint::text END \
             FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid \
             WHERE i.inhparent = to_regclass({}) ORDER BY 1",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MySQL => Some(format!(
            "SELECT PARTITION_NAME, \
             CONCAT(COALESCE(PARTITION_METHOD, ''), CASE WHEN PARTITION_DESCRIPTION IS NULL THEN '' ELSE CONCAT(' ', PARTITION_DESCRIPTION) END), \
             CAST(TABLE_ROWS AS CHAR) \
             FROM information_schema.PARTITIONS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} \
             AND PARTITION_NAME IS NOT NULL ORDER BY PARTITION_ORDINAL_POSITION",
            sql_literal(db, database),
            sql_literal(db, table)
        )),
        DatabaseType::MsSQL => Some(format!(
            "SELECT CAST(p.partition_number AS NVARCHAR(20)), \
             pf.name + CASE WHEN prv.value IS NULL THEN '' ELSE \
               (CASE WHEN pf.boundary_value_on_right = 1 THEN ' >= ' ELSE ' <= ' END) + CONVERT(NVARCHAR(200), prv.value) END, \
             CAST(p.rows AS NVARCHAR(30)) \
             FROM sys.partitions p \
             JOIN sys.indexes i ON i.object_id = p.object_id AND i.index_id = p.index_id \
             JOIN sys.partition_schemes ps ON ps.data_space_id = i.data_space_id \
             JOIN sys.partition_functions pf ON pf.function_id = ps.function_id \
             LEFT JOIN sys.partition_range_values prv ON prv.function_id = pf.function_id \
               AND prv.boundary_id = p.partition_number - CAST(pf.boundary_value_on_right AS INT) \
             WHERE p.object_id = OBJECT_ID({}) AND i.index_id IN (0, 1) ORDER BY p.partition_number",
            mssql_object_literal(table)
        )),
        _ => None,
    }
}

/// Ubah baris hasil [`partitions_query`] menjadi [`PartitionInfo`].
pub fn parse_partitions(rows: &[Vec<String>]) -> Vec<PartitionInfo> {
    rows.iter()
        .filter_map(|r| {
            let name = r.first()?.trim().to_string();
            if name.is_empty() || name == "NULL" {
                return None;
            }
            let bound = r
                .get(1)
                .filter(|b| b.as_str() != "NULL")
                .cloned()
                .unwrap_or_default();
            let rows = r.get(2).and_then(|v| v.trim().parse::<i64>().ok());
            Some(PartitionInfo { name, bound, rows })
        })
        .collect()
}

/// Database sistem yang disembunyikan kecuali toggle "Show system objects" aktif.
pub fn is_system_database(db: &DatabaseType, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match db {
        DatabaseType::MySQL => matches!(
            lower.as_str(),
            "information_schema" | "performance_schema" | "mysql" | "sys"
        ),
        DatabaseType::PostgreSQL => {
            matches!(lower.as_str(), "postgres" | "template0" | "template1")
        }
        DatabaseType::MsSQL => matches!(lower.as_str(), "master" | "model" | "msdb" | "tempdb"),
        _ => false,
    }
}

/// Schema sistem PostgreSQL.
pub fn is_system_schema(name: &str) -> bool {
    name == "pg_catalog"
        || name == "information_schema"
        || name.starts_with("pg_toast")
        || name.starts_with("pg_temp")
}

/// Daftar schema PostgreSQL (kolom: nama, owner, ACL). `include_system`
/// menampilkan juga `pg_catalog` dan `information_schema`.
pub fn pg_schemas_sql(include_system: bool) -> String {
    format!(
        "SELECT n.nspname, pg_get_userbyid(n.nspowner), COALESCE(array_to_string(n.nspacl, ', '), '') \
         FROM pg_namespace n WHERE n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%' {}ORDER BY 1",
        if include_system {
            ""
        } else {
            "AND n.nspname NOT IN ('pg_catalog', 'information_schema') "
        }
    )
}

/// Daftar role PostgreSQL (untuk pilihan owner/grantee).
pub fn pg_roles_sql() -> &'static str {
    "SELECT rolname FROM pg_roles WHERE rolname !~ '^pg_' ORDER BY 1"
}

/// Nama tabel dasar satu database MySQL (dipakai rename database).
pub fn mysql_base_tables_sql(database: &str) -> String {
    format!(
        "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = {} AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
        sql_literal(&DatabaseType::MySQL, database)
    )
}

/// Query foreign key tabel. Kolom: nama, kolom, tabel referensi, kolom
/// referensi, ON UPDATE, ON DELETE.
pub fn foreign_keys_query(db: &DatabaseType, database: &str, table: &str) -> Option<String> {
    match db {
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT con.conname, \
             (SELECT string_agg(a.attname, ', ' ORDER BY k.ord) FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord) \
               JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
             con.confrelid::regclass::text, \
             (SELECT string_agg(a.attname, ', ' ORDER BY k.ord) FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord) \
               JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum), \
             {upd}, {del} \
             FROM pg_constraint con WHERE con.contype = 'f' AND con.conrelid = to_regclass({t}) ORDER BY 1",
            upd = pg_fk_action("con.confupdtype"),
            del = pg_fk_action("con.confdeltype"),
            t = sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MySQL => Some(format!(
            "SELECT rc.CONSTRAINT_NAME, \
             GROUP_CONCAT(k.COLUMN_NAME ORDER BY k.ORDINAL_POSITION SEPARATOR ', '), \
             MAX(k.REFERENCED_TABLE_NAME), \
             GROUP_CONCAT(k.REFERENCED_COLUMN_NAME ORDER BY k.ORDINAL_POSITION SEPARATOR ', '), \
             MAX(rc.UPDATE_RULE), MAX(rc.DELETE_RULE) \
             FROM information_schema.REFERENTIAL_CONSTRAINTS rc \
             JOIN information_schema.KEY_COLUMN_USAGE k ON k.CONSTRAINT_SCHEMA = rc.CONSTRAINT_SCHEMA \
               AND k.CONSTRAINT_NAME = rc.CONSTRAINT_NAME AND k.TABLE_NAME = rc.TABLE_NAME \
             WHERE rc.CONSTRAINT_SCHEMA = {} AND rc.TABLE_NAME = {} \
             GROUP BY rc.CONSTRAINT_NAME ORDER BY 1",
            sql_literal(db, database),
            sql_literal(db, table)
        )),
        DatabaseType::SQLite => Some(format!(
            "SELECT 'fk_' || id, group_concat(\"from\", ', '), \"table\", group_concat(\"to\", ', '), \
             on_update, on_delete FROM pragma_foreign_key_list({}) GROUP BY id ORDER BY id",
            sql_literal(db, table)
        )),
        DatabaseType::MsSQL => Some(format!(
            "SELECT fk.name, \
             STUFF((SELECT ', ' + c.name FROM sys.foreign_key_columns fkc JOIN sys.columns c \
               ON c.object_id = fkc.parent_object_id AND c.column_id = fkc.parent_column_id \
               WHERE fkc.constraint_object_id = fk.object_id ORDER BY fkc.constraint_column_id FOR XML PATH('')), 1, 2, ''), \
             QUOTENAME(SCHEMA_NAME(rt.schema_id)) + '.' + QUOTENAME(rt.name), \
             STUFF((SELECT ', ' + c.name FROM sys.foreign_key_columns fkc JOIN sys.columns c \
               ON c.object_id = fkc.referenced_object_id AND c.column_id = fkc.referenced_column_id \
               WHERE fkc.constraint_object_id = fk.object_id ORDER BY fkc.constraint_column_id FOR XML PATH('')), 1, 2, ''), \
             REPLACE(fk.update_referential_action_desc, '_', ' '), \
             REPLACE(fk.delete_referential_action_desc, '_', ' ') \
             FROM sys.foreign_keys fk JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id \
             WHERE fk.parent_object_id = OBJECT_ID({}) ORDER BY fk.name",
            mssql_object_literal(table)
        )),
        _ => None,
    }
}

fn pg_fk_action(col: &str) -> String {
    format!(
        "CASE {} WHEN 'a' THEN 'NO ACTION' WHEN 'r' THEN 'RESTRICT' WHEN 'c' THEN 'CASCADE' \
         WHEN 'n' THEN 'SET NULL' WHEN 'd' THEN 'SET DEFAULT' ELSE '' END",
        col
    )
}

fn mssql_object_literal(table: &str) -> String {
    let db = DatabaseType::MsSQL;
    let (schema, obj) = split_qualified(table, "dbo");
    sql_literal(
        &db,
        &format!("{}.{}", quote_ident(&db, &schema), quote_ident(&db, &obj)),
    )
}

/// Query check constraint. Kolom: nama, ekspresi. SQLite menyimpan check
/// di dalam `CREATE TABLE` sehingga tidak ada query terpisah.
pub fn check_constraints_query(db: &DatabaseType, database: &str, table: &str) -> Option<String> {
    match db {
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT conname, pg_get_constraintdef(oid, true) FROM pg_constraint \
             WHERE contype = 'c' AND conrelid = to_regclass({}) ORDER BY 1",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MySQL => Some(format!(
            "SELECT cc.CONSTRAINT_NAME, cc.CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS cc \
             JOIN information_schema.TABLE_CONSTRAINTS tc ON tc.CONSTRAINT_SCHEMA = cc.CONSTRAINT_SCHEMA \
               AND tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME \
             WHERE tc.TABLE_SCHEMA = {} AND tc.TABLE_NAME = {} AND tc.CONSTRAINT_TYPE = 'CHECK' ORDER BY 1",
            sql_literal(db, database),
            sql_literal(db, table)
        )),
        DatabaseType::MsSQL => Some(format!(
            "SELECT name, definition FROM sys.check_constraints WHERE parent_object_id = OBJECT_ID({}) ORDER BY name",
            mssql_object_literal(table)
        )),
        _ => None,
    }
}

/// Query trigger per tabel. Kolom: nama, timing, event, definisi.
pub fn table_triggers_query(db: &DatabaseType, database: &str, table: &str) -> Option<String> {
    match db {
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT t.tgname, \
             CASE WHEN t.tgtype & 2 = 2 THEN 'BEFORE' WHEN t.tgtype & 64 = 64 THEN 'INSTEAD OF' ELSE 'AFTER' END, \
             concat_ws(' OR ', CASE WHEN t.tgtype & 4 = 4 THEN 'INSERT' END, CASE WHEN t.tgtype & 8 = 8 THEN 'DELETE' END, \
               CASE WHEN t.tgtype & 16 = 16 THEN 'UPDATE' END, CASE WHEN t.tgtype & 32 = 32 THEN 'TRUNCATE' END), \
             pg_get_triggerdef(t.oid, true) \
             FROM pg_trigger t WHERE t.tgrelid = to_regclass({}) AND NOT t.tgisinternal ORDER BY 1",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MySQL => Some(format!(
            "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, ACTION_STATEMENT \
             FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = {} AND EVENT_OBJECT_TABLE = {} ORDER BY 1",
            sql_literal(db, database),
            sql_literal(db, table)
        )),
        DatabaseType::SQLite => Some(format!(
            "SELECT name, '', '', sql FROM sqlite_master WHERE type = 'trigger' AND tbl_name = {} ORDER BY name",
            sql_literal(db, table)
        )),
        DatabaseType::MsSQL => Some(format!(
            "SELECT tr.name, CASE WHEN tr.is_instead_of_trigger = 1 THEN 'INSTEAD OF' ELSE 'AFTER' END, \
             STUFF((SELECT ', ' + te.type_desc FROM sys.trigger_events te WHERE te.object_id = tr.object_id FOR XML PATH('')), 1, 2, ''), \
             OBJECT_DEFINITION(tr.object_id) \
             FROM sys.triggers tr WHERE tr.parent_id = OBJECT_ID({}) ORDER BY tr.name",
            mssql_object_literal(table)
        )),
        _ => None,
    }
}

/// Query kolom generated/computed. Kolom: nama, ekspresi, jenis.
pub fn generated_columns_query(db: &DatabaseType, database: &str, table: &str) -> Option<String> {
    match db {
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT a.attname, pg_get_expr(d.adbin, d.adrelid), 'STORED' FROM pg_attribute a \
             JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = to_regclass({}) AND a.attgenerated = 's' AND NOT a.attisdropped ORDER BY a.attnum",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MySQL => Some(format!(
            "SELECT COLUMN_NAME, GENERATION_EXPRESSION, EXTRA FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} AND GENERATION_EXPRESSION <> '' ORDER BY ORDINAL_POSITION",
            sql_literal(db, database),
            sql_literal(db, table)
        )),
        DatabaseType::SQLite => Some(format!(
            "SELECT name, '(see DDL)', CASE hidden WHEN 2 THEN 'VIRTUAL' ELSE 'STORED' END \
             FROM pragma_table_xinfo({}) WHERE hidden IN (2, 3) ORDER BY cid",
            sql_literal(db, table)
        )),
        DatabaseType::MsSQL => Some(format!(
            "SELECT name, definition, CASE WHEN is_persisted = 1 THEN 'PERSISTED' ELSE 'VIRTUAL' END \
             FROM sys.computed_columns WHERE object_id = OBJECT_ID({}) ORDER BY column_id",
            mssql_object_literal(table)
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_cache_keys_roundtrip() {
        for kind in [
            PgObjectKind::MaterializedView,
            PgObjectKind::UserType,
            PgObjectKind::Function,
            PgObjectKind::Procedure,
            PgObjectKind::Trigger,
        ] {
            assert_eq!(PgObjectKind::from_cache_key(kind.cache_key()), Some(kind));
            assert!(pg_list_objects_sql(kind).starts_with("SELECT"));
        }
        assert!(pg_list_objects_sql(PgObjectKind::Procedure).contains("p.prokind = 'p'"));
    }

    #[test]
    fn routine_source_queries_escape_names() {
        let (sql, col) =
            routine_source_query(&DatabaseType::MySQL, RoutineKind::Function, "fn").unwrap();
        assert_eq!(sql, "SHOW CREATE FUNCTION `fn`");
        assert_eq!(col, Some("Create Function"));
        let (sql, _) = routine_source_query(
            &DatabaseType::PostgreSQL,
            RoutineKind::Function,
            "f'x(integer)",
        )
        .unwrap();
        assert!(sql.contains("'f''x(integer)'"));
        let (sql, _) =
            routine_source_query(&DatabaseType::MsSQL, RoutineKind::Procedure, "[sales].[p]")
                .unwrap();
        assert!(sql.contains("OBJECT_ID(N'[sales].[p]')"));
        assert!(routine_source_query(&DatabaseType::SQLite, RoutineKind::Function, "f").is_none());
    }

    #[test]
    fn partitions_parse_and_label() {
        let rows = vec![
            vec![
                "orders_2024".to_string(),
                "FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')".to_string(),
                "1234567".to_string(),
            ],
            vec!["p0".to_string(), "RANGE 100".to_string(), "".to_string()],
            vec!["NULL".to_string(), "".to_string(), "".to_string()],
        ];
        let parts = parse_partitions(&rows);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].rows, Some(1_234_567));
        assert_eq!(
            parts[0].label(),
            "orders_2024 · FOR VALUES FROM ('2024-01-01') TO ('2025-01-01') · ~1,234,567 rows"
        );
        assert_eq!(parts[1].label(), "p0 · RANGE 100");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
    }

    #[test]
    fn system_objects_detection() {
        assert!(is_system_database(
            &DatabaseType::MySQL,
            "performance_schema"
        ));
        assert!(is_system_database(&DatabaseType::PostgreSQL, "template1"));
        assert!(is_system_database(&DatabaseType::MsSQL, "TempDB"));
        assert!(!is_system_database(&DatabaseType::MySQL, "shop"));
        assert!(is_system_schema("pg_toast_temp_1"));
        assert!(!is_system_schema("public"));
        assert!(pg_schemas_sql(false).contains("NOT IN ('pg_catalog'"));
        assert!(!pg_schemas_sql(true).contains("NOT IN ('pg_catalog'"));
    }

    #[test]
    fn structure_queries_exist_where_supported() {
        for db in [
            DatabaseType::PostgreSQL,
            DatabaseType::MySQL,
            DatabaseType::SQLite,
            DatabaseType::MsSQL,
        ] {
            assert!(foreign_keys_query(&db, "d", "t").is_some());
            assert!(table_triggers_query(&db, "d", "t").is_some());
            assert!(generated_columns_query(&db, "d", "t").is_some());
        }
        assert!(check_constraints_query(&DatabaseType::SQLite, "d", "t").is_none());
    }
}
