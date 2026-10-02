//! Ekspor SQL untuk objek database apa pun (H7): tabel (struktur + data),
//! view, materialized view, routine, trigger, event, tipe, dan privilege.
//! Ukuran tiap `INSERT` dibatasi dan index/foreign key bisa ditulis setelah
//! data (H6) supaya impor ulang tidak memelihara index per baris.

use std::sync::atomic::{AtomicBool, Ordering};

use super::catalog::{self, Endpoint};
use super::transfer::{ProgressHandle, TransferProgress};
use super::values::{InsertLimits, ValueKind, build_insert_batches, kind_from_type};
use crate::models::enums::DatabaseType;
use crate::schema_objects::catalog::{
    PgObjectKind, RoutineKind, pg_list_objects_sql, routine_source_query,
};
use crate::schema_objects::sql::{quote_ident, quote_qualified, split_qualified, sql_literal};

/// Jenis objek, dalam urutan penulisan di file hasil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectKind {
    Type,
    Table,
    View,
    MaterializedView,
    Function,
    Procedure,
    Trigger,
    Event,
    Privileges,
}

impl ObjectKind {
    pub const ALL: [ObjectKind; 9] = [
        ObjectKind::Type,
        ObjectKind::Table,
        ObjectKind::View,
        ObjectKind::MaterializedView,
        ObjectKind::Function,
        ObjectKind::Procedure,
        ObjectKind::Trigger,
        ObjectKind::Event,
        ObjectKind::Privileges,
    ];

    pub fn plural(self) -> &'static str {
        match self {
            ObjectKind::Type => "Types",
            ObjectKind::Table => "Tables",
            ObjectKind::View => "Views",
            ObjectKind::MaterializedView => "Materialized Views",
            ObjectKind::Function => "Functions",
            ObjectKind::Procedure => "Procedures",
            ObjectKind::Trigger => "Triggers",
            ObjectKind::Event => "Events",
            ObjectKind::Privileges => "Privileges",
        }
    }

    fn routine_kind(self) -> Option<RoutineKind> {
        match self {
            ObjectKind::Type => Some(RoutineKind::UserType),
            ObjectKind::View => Some(RoutineKind::View),
            ObjectKind::MaterializedView => Some(RoutineKind::MaterializedView),
            ObjectKind::Function => Some(RoutineKind::Function),
            ObjectKind::Procedure => Some(RoutineKind::Procedure),
            ObjectKind::Trigger => Some(RoutineKind::Trigger),
            ObjectKind::Event => Some(RoutineKind::Event),
            ObjectKind::Table | ObjectKind::Privileges => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    pub kind: ObjectKind,
    pub name: String,
}

/// Nama entri tunggal untuk [`ObjectKind::Privileges`].
pub const PRIVILEGES_ENTRY: &str = "All grants in this database";

const PG_USER_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'";

/// Query daftar objek satu jenis (satu kolom: nama). `None` = jenis itu tidak
/// ada di engine ini.
pub fn list_objects_sql(
    db: &DatabaseType,
    kind: ObjectKind,
    database: Option<&str>,
) -> Option<String> {
    let schema = || {
        database
            .map(|d| sql_literal(db, d))
            .unwrap_or_else(|| "DATABASE()".to_string())
    };
    match (db, kind) {
        (_, ObjectKind::Table) => catalog::list_tables_sql(db, database),
        (_, ObjectKind::Privileges) => None,
        (DatabaseType::MySQL, ObjectKind::View) => Some(format!(
            "SELECT TABLE_NAME FROM information_schema.VIEWS WHERE TABLE_SCHEMA = {} ORDER BY 1",
            schema()
        )),
        (DatabaseType::MySQL, ObjectKind::Function | ObjectKind::Procedure) => Some(format!(
            "SELECT ROUTINE_NAME FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = {} \
             AND ROUTINE_TYPE = '{}' ORDER BY 1",
            schema(),
            if kind == ObjectKind::Function {
                "FUNCTION"
            } else {
                "PROCEDURE"
            }
        )),
        (DatabaseType::MySQL, ObjectKind::Trigger) => Some(format!(
            "SELECT TRIGGER_NAME FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = {} \
             ORDER BY 1",
            schema()
        )),
        (DatabaseType::MySQL, ObjectKind::Event) => Some(format!(
            "SELECT EVENT_NAME FROM information_schema.EVENTS WHERE EVENT_SCHEMA = {} ORDER BY 1",
            schema()
        )),
        (DatabaseType::PostgreSQL, ObjectKind::View) => Some(format!(
            "SELECT (CASE WHEN n.nspname = 'public' THEN '' ELSE n.nspname || '.' END || c.relname) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind = 'v' AND {PG_USER_SCHEMAS} ORDER BY 1"
        )),
        (DatabaseType::PostgreSQL, ObjectKind::MaterializedView) => {
            Some(pg_list_objects_sql(PgObjectKind::MaterializedView))
        }
        (DatabaseType::PostgreSQL, ObjectKind::Type) => {
            Some(pg_list_objects_sql(PgObjectKind::UserType))
        }
        (DatabaseType::PostgreSQL, ObjectKind::Function) => {
            Some(pg_list_objects_sql(PgObjectKind::Function))
        }
        (DatabaseType::PostgreSQL, ObjectKind::Procedure) => {
            Some(pg_list_objects_sql(PgObjectKind::Procedure))
        }
        (DatabaseType::PostgreSQL, ObjectKind::Trigger) => {
            Some(pg_list_objects_sql(PgObjectKind::Trigger))
        }
        (
            DatabaseType::MsSQL,
            ObjectKind::View | ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger,
        ) => {
            let types = match kind {
                ObjectKind::View => "'V'",
                ObjectKind::Function => "'FN', 'IF', 'TF'",
                ObjectKind::Procedure => "'P'",
                _ => "'TR'",
            };
            Some(format!(
                "SELECT s.name + '.' + o.name FROM sys.objects o \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE o.type IN ({types}) AND o.is_ms_shipped = 0 ORDER BY 1"
            ))
        }
        (DatabaseType::SQLite, ObjectKind::View | ObjectKind::Trigger) => Some(format!(
            "SELECT name FROM sqlite_master WHERE type = '{}' ORDER BY name",
            if kind == ObjectKind::View {
                "view"
            } else {
                "trigger"
            }
        )),
        _ => None,
    }
}

fn privileges_sql(db: &DatabaseType, database: Option<&str>) -> Option<String> {
    match db {
        DatabaseType::MySQL => {
            let schema = database
                .map(|d| sql_literal(db, d))
                .unwrap_or_else(|| "DATABASE()".to_string());
            Some(format!(
                "SELECT CONCAT('GRANT ', PRIVILEGE_TYPE, ' ON `', TABLE_SCHEMA, '`.* TO ', GRANTEE, ';') \
                 FROM information_schema.SCHEMA_PRIVILEGES WHERE TABLE_SCHEMA = {schema} \
                 UNION ALL \
                 SELECT CONCAT('GRANT ', PRIVILEGE_TYPE, ' ON `', TABLE_SCHEMA, '`.`', TABLE_NAME, '` TO ', GRANTEE, ';') \
                 FROM information_schema.TABLE_PRIVILEGES WHERE TABLE_SCHEMA = {schema}"
            ))
        }
        DatabaseType::PostgreSQL => Some(
            "SELECT 'GRANT ' || privilege_type || ' ON ' || quote_ident(table_schema) || '.' \
             || quote_ident(table_name) || ' TO ' \
             || CASE WHEN grantee = 'PUBLIC' THEN 'PUBLIC' ELSE quote_ident(grantee) END || ';' \
             FROM information_schema.role_table_grants \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') AND grantee <> grantor \
             ORDER BY 1"
                .to_string(),
        ),
        DatabaseType::MsSQL => Some(
            "SELECT CASE WHEN p.state = 'W' THEN 'GRANT' ELSE p.state_desc END COLLATE DATABASE_DEFAULT \
             + ' ' + p.permission_name COLLATE DATABASE_DEFAULT \
             + CASE WHEN p.class = 1 THEN ' ON ' + QUOTENAME(SCHEMA_NAME(o.schema_id)) + '.' + QUOTENAME(o.name) ELSE '' END \
             + ' TO ' + QUOTENAME(u.name) \
             + CASE WHEN p.state = 'W' THEN ' WITH GRANT OPTION' ELSE '' END + ';' \
             FROM sys.database_permissions p \
             JOIN sys.database_principals u ON u.principal_id = p.grantee_principal_id \
             LEFT JOIN sys.objects o ON o.object_id = p.major_id AND p.class = 1 \
             WHERE p.class IN (0, 1) AND u.name NOT IN ('public', 'dbo', 'guest', 'sys', 'INFORMATION_SCHEMA') \
             AND (p.class = 0 OR o.is_ms_shipped = 0) ORDER BY 1"
                .to_string(),
        ),
        _ => None,
    }
}

/// Semua objek yang bisa diekspor dari `ep`, dikelompokkan per jenis. Jenis
/// yang query-nya gagal (hak akses kurang) dilewati.
pub async fn list_objects(ep: &Endpoint) -> Result<Vec<ObjectRef>, String> {
    let mut out = Vec::new();
    for kind in ObjectKind::ALL {
        if kind == ObjectKind::Privileges {
            if privileges_sql(ep.db_type(), ep.database.as_deref()).is_some() {
                out.push(ObjectRef {
                    kind,
                    name: PRIVILEGES_ENTRY.to_string(),
                });
            }
            continue;
        }
        let Some(sql) = list_objects_sql(ep.db_type(), kind, ep.database.as_deref()) else {
            continue;
        };
        match ep.query(&sql).await {
            Ok(set) => out.extend(
                set.first_column()
                    .into_iter()
                    .map(|name| ObjectRef { kind, name }),
            ),
            // Tabel wajib terbaca; jenis lain boleh gagal tanpa membatalkan.
            Err(e) if kind == ObjectKind::Table => return Err(e),
            Err(e) => log::warn!("[TRANSFER] listing {} failed: {}", kind.plural(), e),
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct SqlExportOptions {
    pub structure: bool,
    pub data: bool,
    /// Tulis `DROP TABLE/VIEW IF EXISTS` sebelum tiap `CREATE`.
    pub drop_if_exists: bool,
    pub insert_limits: InsertLimits,
    /// Tulis index dan foreign key setelah data.
    pub indexes_post_data: bool,
    /// Baris maksimum per tabel.
    pub row_limit: Option<u64>,
    pub page_rows: u64,
}

impl Default for SqlExportOptions {
    fn default() -> Self {
        Self {
            structure: true,
            data: true,
            drop_if_exists: false,
            insert_limits: InsertLimits::default(),
            indexes_post_data: true,
            row_limit: None,
            page_rows: 2000,
        }
    }
}

/// DDL satu tabel: `CREATE TABLE` dan statement yang boleh ditunda sampai
/// setelah data (index sekunder, foreign key).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableDdl {
    pub pre: Vec<String>,
    pub create: String,
    pub post_data: Vec<String>,
}

fn ensure_semicolon(sql: &str) -> String {
    let trimmed = sql.trim_end();
    if trimmed.ends_with(';') {
        trimmed.to_string()
    } else {
        format!("{trimmed};")
    }
}

/// Pisahkan index sekunder dan foreign key dari keluaran `SHOW CREATE TABLE`
/// MySQL menjadi `ALTER TABLE … ADD …`. Primary key tetap di `CREATE TABLE`.
/// Tabel dengan kolom `AUTO_INCREMENT` tanpa primary key tidak dipecah, karena
/// kolom itu wajib ber-index saat tabel dibuat.
pub fn split_mysql_create_table(ddl: &str, table_sql: &str) -> TableDdl {
    let whole = || TableDdl {
        pre: Vec::new(),
        create: ensure_semicolon(ddl),
        post_data: Vec::new(),
    };
    let lines: Vec<&str> = ddl.lines().collect();
    let Some(close) = lines.iter().rposition(|l| l.trim_start().starts_with(')')) else {
        return whole();
    };
    if lines.len() < 3 || close == 0 {
        return whole();
    }
    let has_auto_increment = lines[1..close]
        .iter()
        .any(|l| l.to_ascii_uppercase().contains("AUTO_INCREMENT"));
    let has_primary = lines[1..close].iter().any(|l| {
        l.trim_start()
            .to_ascii_uppercase()
            .starts_with("PRIMARY KEY")
    });
    if has_auto_increment && !has_primary {
        return whole();
    }
    let mut kept: Vec<String> = Vec::new();
    let mut post = Vec::new();
    for line in &lines[1..close] {
        let def = line.trim().trim_end_matches(',');
        let upper = def.to_ascii_uppercase();
        let deferred = upper.starts_with("KEY ")
            || upper.starts_with("UNIQUE KEY ")
            || upper.starts_with("FULLTEXT KEY ")
            || upper.starts_with("SPATIAL KEY ")
            || (upper.starts_with("CONSTRAINT ") && upper.contains(" FOREIGN KEY "));
        if deferred {
            post.push(format!("ALTER TABLE {table_sql} ADD {def};"));
        } else {
            kept.push(format!("  {def}"));
        }
    }
    if post.is_empty() || kept.is_empty() {
        return whole();
    }
    let mut create = String::new();
    create.push_str(lines[0]);
    create.push('\n');
    create.push_str(&kept.join(",\n"));
    create.push('\n');
    create.push_str(&lines[close..].join("\n"));
    TableDdl {
        pre: Vec::new(),
        create: ensure_semicolon(&create),
        post_data: post,
    }
}

/// Nama sequence dari default `nextval('seq'::regclass)`.
fn sequence_of_default(default: &str) -> Option<&str> {
    let rest = default.strip_prefix("nextval('")?;
    rest.split_once('\'').map(|(name, _)| name)
}

async fn postgres_table_ddl(ep: &Endpoint, table: &str) -> Result<TableDdl, String> {
    let db = ep.db_type();
    let regclass = format!("{}::regclass", sql_literal(db, &quote_qualified(db, table)));
    let columns = ep
        .query(&format!(
            "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), \
             CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END, \
             COALESCE(pg_get_expr(d.adbin, d.adrelid), ''), a.attidentity::text, \
             a.attgenerated::text \
             FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = {regclass} AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum"
        ))
        .await?;
    if columns.rows.is_empty() {
        return Err(format!("Table \"{table}\" not found"));
    }
    let mut ddl = TableDdl::default();
    let mut defs = Vec::new();
    for row in &columns.rows {
        let cell = |i: usize| row.get(i).map(String::as_str).unwrap_or("");
        let mut def = format!("  {} {}", quote_ident(db, cell(0)), cell(1));
        let default = cell(3);
        match (cell(4), cell(5)) {
            (_, "s") => def.push_str(&format!(" GENERATED ALWAYS AS ({default}) STORED")),
            ("a", _) => def.push_str(" GENERATED ALWAYS AS IDENTITY"),
            ("d", _) => def.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
            _ if !default.is_empty() && default != "NULL" => {
                if let Some(seq) = sequence_of_default(default) {
                    ddl.pre
                        .push(format!("CREATE SEQUENCE IF NOT EXISTS {seq};"));
                }
                def.push_str(&format!(" DEFAULT {default}"));
            }
            _ => {}
        }
        if cell(2) == "NO" {
            def.push_str(" NOT NULL");
        }
        defs.push(def);
    }
    let constraints = ep
        .query(&format!(
            "SELECT conname::text, contype::text, pg_get_constraintdef(oid) FROM pg_constraint \
             WHERE conrelid = {regclass} ORDER BY contype, conname"
        ))
        .await?;
    let table_sql = quote_qualified(db, table);
    for row in &constraints.rows {
        let (name, kind, def) = (
            row.first().map(String::as_str).unwrap_or(""),
            row.get(1).map(String::as_str).unwrap_or(""),
            row.get(2).map(String::as_str).unwrap_or(""),
        );
        if kind == "f" {
            // Foreign key selalu setelah data: urutan tabel tidak dijamin.
            ddl.post_data.push(format!(
                "ALTER TABLE {table_sql} ADD CONSTRAINT {} {def};",
                quote_ident(db, name)
            ));
        } else {
            defs.push(format!("  CONSTRAINT {} {def}", quote_ident(db, name)));
        }
    }
    ddl.create = format!("CREATE TABLE {table_sql} (\n{}\n);", defs.join(",\n"));
    let indexes = ep
        .query(&format!(
            "SELECT pg_get_indexdef(i.indexrelid) FROM pg_index i WHERE i.indrelid = {regclass} \
             AND NOT i.indisprimary AND NOT EXISTS \
             (SELECT 1 FROM pg_constraint c WHERE c.conindid = i.indexrelid) ORDER BY 1"
        ))
        .await?;
    ddl.post_data
        .extend(indexes.first_column().iter().map(|s| ensure_semicolon(s)));
    Ok(ddl)
}

async fn mssql_table_ddl(ep: &Endpoint, table: &str) -> Result<TableDdl, String> {
    let db = ep.db_type();
    let table_sql = quote_qualified(db, table);
    let columns = catalog::fetch_columns(ep, table).await?;
    // Disusun dari katalog kolom: default, identity, dan check constraint
    // tidak ikut (lihat docs/DATA_TRANSFER.md).
    let create = super::types::create_table_sql(db, db, &table_sql, &columns);
    let indexes = ep
        .query(&format!(
            "SELECT 'CREATE ' + CASE WHEN i.is_unique = 1 THEN 'UNIQUE ' ELSE '' END \
             + i.type_desc COLLATE DATABASE_DEFAULT + ' INDEX ' + QUOTENAME(i.name) + ' ON ' \
             + QUOTENAME(SCHEMA_NAME(o.schema_id)) + '.' + QUOTENAME(o.name) + ' (' \
             + STUFF((SELECT ', ' + QUOTENAME(c.name) + CASE WHEN ic.is_descending_key = 1 THEN ' DESC' ELSE '' END \
               FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
               WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 \
               ORDER BY ic.key_ordinal FOR XML PATH('')), 1, 2, '') + ');' \
             FROM sys.indexes i JOIN sys.objects o ON o.object_id = i.object_id \
             WHERE i.object_id = OBJECT_ID({}) AND i.is_primary_key = 0 AND i.type IN (1, 2) \
             AND i.is_unique_constraint = 0 ORDER BY 1",
            sql_literal(db, &table_sql)
        ))
        .await
        .map(|set| set.first_column())
        .unwrap_or_default();
    Ok(TableDdl {
        pre: Vec::new(),
        create,
        post_data: indexes,
    })
}

/// DDL tabel dari server.
pub async fn table_ddl(ep: &Endpoint, table: &str) -> Result<TableDdl, String> {
    let db = ep.db_type();
    match db {
        DatabaseType::MySQL => {
            let table_sql = quote_qualified(db, table);
            let set = ep.query(&format!("SHOW CREATE TABLE {table_sql}")).await?;
            let ddl = set
                .value_by_header(Some("Create Table"))
                .or_else(|| set.rows.first().and_then(|r| r.last()).map(String::as_str))
                .ok_or_else(|| format!("Table \"{table}\" not found"))?;
            Ok(split_mysql_create_table(ddl, &table_sql))
        }
        DatabaseType::SQLite => {
            let name = sql_literal(db, &split_qualified(table, "").1);
            let create = ep
                .query(&format!(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = {name}"
                ))
                .await?
                .first_value()
                .map(ensure_semicolon)
                .ok_or_else(|| format!("Table \"{table}\" not found"))?;
            let post_data = ep
                .query(&format!(
                    "SELECT sql FROM sqlite_master WHERE type = 'index' AND tbl_name = {name} \
                     AND sql IS NOT NULL ORDER BY name"
                ))
                .await?
                .first_column()
                .iter()
                .map(|s| ensure_semicolon(s))
                .collect();
            Ok(TableDdl {
                pre: Vec::new(),
                create,
                post_data,
            })
        }
        DatabaseType::PostgreSQL => postgres_table_ddl(ep, table).await,
        DatabaseType::MsSQL => mssql_table_ddl(ep, table).await,
        other => Err(format!(
            "{} tables cannot be exported as SQL",
            other.as_db_str()
        )),
    }
}

/// Source satu objek non-tabel, siap ditulis ke skrip.
async fn object_source(ep: &Endpoint, object: &ObjectRef) -> Result<String, String> {
    let db = ep.db_type();
    let kind = object
        .kind
        .routine_kind()
        .ok_or_else(|| "not a source object".to_string())?;
    let (sql, header) = routine_source_query(db, kind, &object.name)
        .ok_or_else(|| format!("{} are not available on this engine", object.kind.plural()))?;
    let set = ep.query(&sql).await?;
    let source = set
        .value_by_header(header)
        .ok_or_else(|| format!("source of \"{}\" is empty or not readable", object.name))?;
    let body = source.trim_end().trim_end_matches(';').to_string();
    let compound = matches!(
        object.kind,
        ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger | ObjectKind::Event
    );
    Ok(match db {
        // Body routine MySQL berisi `;`; klien butuh delimiter lain.
        DatabaseType::MySQL if compound => format!("DELIMITER ;;\n{body};;\nDELIMITER ;"),
        DatabaseType::MsSQL => format!("{body}\nGO"),
        _ => format!("{body};"),
    })
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!(
        "\n-- ----------------------------------------------------------\n-- {title}\n-- ----------------------------------------------------------\n\n"
    ));
}

fn with_progress(progress: &ProgressHandle, f: impl FnOnce(&mut TransferProgress)) {
    let mut guard = progress
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard);
}

async fn table_data_sql(
    ep: &Endpoint,
    table: &str,
    opts: &SqlExportOptions,
    progress: &ProgressHandle,
    cancel: &AtomicBool,
    out: &mut String,
) -> Result<u64, String> {
    let db = ep.db_type();
    let columns = catalog::fetch_columns(ep, table).await?;
    let table_sql = quote_qualified(db, table);
    let select: Vec<String> = columns
        .iter()
        .map(|c| catalog::select_expr(db, c))
        .collect();
    let order: Vec<String> = columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| quote_ident(db, &c.name))
        .collect();
    let column_sql: Vec<String> = columns.iter().map(|c| quote_ident(db, &c.name)).collect();
    let kinds: Vec<ValueKind> = columns
        .iter()
        .map(|c| kind_from_type(&c.data_type))
        .collect();
    let source_cols: Vec<usize> = (0..columns.len()).collect();
    let page = opts.page_rows.max(1);
    let mut written = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        let limit = match opts.row_limit {
            Some(max) if written >= max => break,
            Some(max) => page.min(max - written),
            None => page,
        };
        let sql = catalog::select_page_sql(db, &table_sql, &select, None, &order, limit, written);
        let mut rows = ep.query(&sql).await?.rows;
        if rows.is_empty() {
            break;
        }
        catalog::pad_rows(&mut rows, columns.len());
        let fetched = rows.len() as u64;
        for statement in build_insert_batches(
            db,
            &table_sql,
            &column_sql,
            &rows,
            &source_cols,
            &kinds,
            opts.insert_limits,
        ) {
            out.push_str(&statement);
            out.push('\n');
        }
        written += fetched;
        with_progress(progress, |p| p.rows_copied += fetched);
        if fetched < limit {
            break;
        }
    }
    Ok(written)
}

/// Susun skrip SQL untuk `objects`. Urutan: tipe, tabel (struktur), data,
/// index/foreign key, view, routine, trigger, event, privilege.
pub async fn export_sql(
    ep: Endpoint,
    objects: &[ObjectRef],
    opts: &SqlExportOptions,
    progress: &ProgressHandle,
    cancel: &AtomicBool,
) -> Result<String, String> {
    with_progress(progress, |p| p.tables_total = objects.len());
    let result = build_script(ep, objects, opts, progress, cancel).await;
    with_progress(progress, |p| {
        p.finished = true;
        p.current_table.clear();
        if let Err(e) = &result {
            p.error = Some(e.clone());
        }
    });
    result
}

async fn build_script(
    ep: Endpoint,
    objects: &[ObjectRef],
    opts: &SqlExportOptions,
    progress: &ProgressHandle,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if objects.is_empty() {
        return Err("No objects selected".to_string());
    }
    let ep = ep.connect().await?;
    let db = ep.db_type().clone();
    let mut sorted: Vec<&ObjectRef> = objects.iter().collect();
    sorted.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    let of_kind = |kind: ObjectKind| sorted.iter().copied().filter(move |o| o.kind == kind);
    let step = |name: &str| {
        with_progress(progress, |p| {
            p.current_table = name.to_string();
        })
    };
    let done = || with_progress(progress, |p| p.tables_done += 1);
    let warn = |out: &mut String, object: &ObjectRef, e: &str| {
        log::warn!("[TRANSFER] export of {} failed: {}", object.name, e);
        out.push_str(&format!(
            "-- {}: skipped ({})\n",
            object.name,
            e.replace('\n', " ")
        ));
        with_progress(progress, |p| p.log.push(format!("{}: {}", object.name, e)));
    };

    let mut out = format!(
        "-- Tabular SQL export\n-- Source: {}\n-- Engine: {}\n-- Exported at: {}\n",
        ep.label(),
        db.as_db_str(),
        chrono::Utc::now().to_rfc3339()
    );
    if matches!(db, DatabaseType::MySQL) {
        out.push_str("\nSET FOREIGN_KEY_CHECKS = 0;\n");
    }

    if opts.structure && of_kind(ObjectKind::Type).next().is_some() {
        section(&mut out, "Types");
        for object in of_kind(ObjectKind::Type) {
            step(&object.name);
            match object_source(&ep, object).await {
                Ok(src) => out.push_str(&format!("{src}\n\n")),
                Err(e) => warn(&mut out, object, &e),
            }
            done();
        }
    }

    let tables: Vec<&ObjectRef> = of_kind(ObjectKind::Table).collect();
    let mut post_data: Vec<String> = Vec::new();
    if opts.structure && !tables.is_empty() {
        section(&mut out, "Tables");
        for object in &tables {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
            step(&object.name);
            match table_ddl(&ep, &object.name).await {
                Ok(ddl) => {
                    if opts.drop_if_exists {
                        out.push_str(&format!(
                            "DROP TABLE IF EXISTS {};\n",
                            quote_qualified(&db, &object.name)
                        ));
                    }
                    for pre in &ddl.pre {
                        out.push_str(&format!("{pre}\n"));
                    }
                    out.push_str(&format!("{}\n\n", ddl.create));
                    // Index dan foreign key ditulis setelah semua tabel dibuat:
                    // foreign key bisa menunjuk tabel yang muncul belakangan.
                    post_data.extend(ddl.post_data);
                }
                Err(e) => warn(&mut out, object, &e),
            }
        }
    }
    let after_data = opts.indexes_post_data && opts.data;
    if !after_data && !post_data.is_empty() {
        section(&mut out, "Indexes and foreign keys");
        for stmt in post_data.drain(..) {
            out.push_str(&format!("{stmt}\n"));
        }
    }
    if opts.data && !tables.is_empty() {
        section(&mut out, "Data");
        for object in &tables {
            step(&object.name);
            let before = out.len();
            match table_data_sql(&ep, &object.name, opts, progress, cancel, &mut out).await {
                Ok(rows) => {
                    with_progress(progress, |p| {
                        p.log.push(format!("{}: {} rows", object.name, rows))
                    });
                    if out.len() > before {
                        out.push('\n');
                    }
                }
                Err(e) if e == "Cancelled" => return Err(e),
                Err(e) => warn(&mut out, object, &e),
            }
        }
    }
    for _ in &tables {
        done();
    }
    if !post_data.is_empty() {
        section(&mut out, "Indexes and foreign keys (post-data)");
        for stmt in &post_data {
            out.push_str(&format!("{stmt}\n"));
        }
    }

    if opts.structure {
        for kind in [
            ObjectKind::View,
            ObjectKind::MaterializedView,
            ObjectKind::Function,
            ObjectKind::Procedure,
            ObjectKind::Trigger,
            ObjectKind::Event,
        ] {
            if of_kind(kind).next().is_none() {
                continue;
            }
            section(&mut out, kind.plural());
            for object in of_kind(kind) {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled".to_string());
                }
                step(&object.name);
                match object_source(&ep, object).await {
                    Ok(src) => {
                        if opts.drop_if_exists && kind == ObjectKind::View {
                            out.push_str(&format!(
                                "DROP VIEW IF EXISTS {};\n",
                                quote_qualified(&db, &object.name)
                            ));
                        }
                        out.push_str(&format!("{src}\n\n"));
                    }
                    Err(e) => warn(&mut out, object, &e),
                }
                done();
            }
        }
        if of_kind(ObjectKind::Privileges).next().is_some()
            && let Some(sql) = privileges_sql(&db, ep.database.as_deref())
        {
            section(&mut out, "Privileges");
            step("privileges");
            match ep.query(&sql).await {
                Ok(set) => {
                    for grant in set.first_column() {
                        out.push_str(&format!("{grant}\n"));
                    }
                }
                Err(e) => out.push_str(&format!("-- privileges: skipped ({e})\n")),
            }
            done();
        }
    }

    if matches!(db, DatabaseType::MySQL) {
        out.push_str("\nSET FOREIGN_KEY_CHECKS = 1;\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::DatabasePool;
    use crate::models::structs::ConnectionConfig;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mysql_secondary_keys_move_to_post_data() {
        let ddl = "CREATE TABLE `orders` (\n  `id` int NOT NULL AUTO_INCREMENT,\n  `user_id` int NOT NULL,\n  `code` varchar(20) DEFAULT NULL,\n  PRIMARY KEY (`id`),\n  UNIQUE KEY `uq_code` (`code`),\n  KEY `idx_user` (`user_id`),\n  CONSTRAINT `fk_user` FOREIGN KEY (`user_id`) REFERENCES `users` (`id`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4";
        let split = split_mysql_create_table(ddl, "`orders`");
        assert_eq!(
            split.create,
            "CREATE TABLE `orders` (\n  `id` int NOT NULL AUTO_INCREMENT,\n  `user_id` int NOT NULL,\n  `code` varchar(20) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;"
        );
        assert_eq!(
            split.post_data,
            vec![
                "ALTER TABLE `orders` ADD UNIQUE KEY `uq_code` (`code`);",
                "ALTER TABLE `orders` ADD KEY `idx_user` (`user_id`);",
                "ALTER TABLE `orders` ADD CONSTRAINT `fk_user` FOREIGN KEY (`user_id`) REFERENCES `users` (`id`);",
            ]
        );
    }

    #[test]
    fn mysql_auto_increment_without_primary_key_is_not_split() {
        let ddl = "CREATE TABLE `t` (\n  `n` int NOT NULL AUTO_INCREMENT,\n  KEY `n` (`n`)\n) ENGINE=InnoDB";
        let split = split_mysql_create_table(ddl, "`t`");
        assert!(split.post_data.is_empty());
        assert!(split.create.contains("KEY `n` (`n`)"));
    }

    #[test]
    fn sequence_name_is_read_from_default() {
        assert_eq!(
            sequence_of_default("nextval('users_id_seq'::regclass)"),
            Some("users_id_seq")
        );
        assert_eq!(sequence_of_default("now()"), None);
    }

    #[test]
    fn object_listing_queries_exist_per_engine() {
        let my = DatabaseType::MySQL;
        assert!(
            list_objects_sql(&my, ObjectKind::Event, Some("shop"))
                .unwrap()
                .contains("EVENT_SCHEMA = 'shop'")
        );
        assert!(list_objects_sql(&DatabaseType::SQLite, ObjectKind::Procedure, None).is_none());
        assert!(list_objects_sql(&DatabaseType::PostgreSQL, ObjectKind::Type, None).is_some());
        assert!(privileges_sql(&DatabaseType::SQLite, None).is_none());
    }

    async fn sqlite_endpoint() -> Endpoint {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        Endpoint::new(
            ConnectionConfig {
                name: "mem".to_string(),
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(DatabasePool::SQLite(Arc::new(pool))),
            None,
        )
    }

    #[tokio::test]
    async fn sqlite_export_orders_structure_data_indexes_and_objects() {
        let ep = sqlite_endpoint().await;
        ep.query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL); \
             CREATE INDEX idx_users_name ON users (name); \
             CREATE VIEW v_users AS SELECT name FROM users; \
             INSERT INTO users VALUES (1, 'ann'), (2, 'o''neil');",
        )
        .await
        .unwrap();
        let objects = list_objects(&ep).await.unwrap();
        assert!(objects.contains(&ObjectRef {
            kind: ObjectKind::Table,
            name: "users".into()
        }));
        assert!(objects.contains(&ObjectRef {
            kind: ObjectKind::View,
            name: "v_users".into()
        }));

        let progress: ProgressHandle = Arc::new(Mutex::new(TransferProgress::default()));
        let cancel = AtomicBool::new(false);
        let opts = SqlExportOptions {
            drop_if_exists: true,
            ..Default::default()
        };
        let sql = export_sql(ep.clone(), &objects, &opts, &progress, &cancel)
            .await
            .unwrap();
        let create = sql.find("CREATE TABLE users").unwrap();
        let insert = sql.find("INSERT INTO \"users\"").unwrap();
        let index = sql.find("CREATE INDEX idx_users_name").unwrap();
        let view = sql.find("CREATE VIEW v_users").unwrap();
        assert!(create < insert && insert < index && index < view, "{sql}");
        assert!(sql.contains("DROP TABLE IF EXISTS \"users\";"));
        assert!(sql.contains("(1, 'ann'),\n(2, 'o''neil');"));
        assert!(progress.lock().unwrap().finished);

        // Skrip hasil bisa dijalankan di database kosong.
        let fresh = sqlite_endpoint().await;
        fresh.query(&sql).await.unwrap();
        let names = fresh
            .query("SELECT name FROM v_users ORDER BY 1")
            .await
            .unwrap();
        assert_eq!(names.first_column(), vec!["ann", "o'neil"]);

        // Tanpa post-data: index langsung setelah CREATE TABLE.
        let inline = SqlExportOptions {
            indexes_post_data: false,
            ..Default::default()
        };
        let progress: ProgressHandle = Arc::new(Mutex::new(TransferProgress::default()));
        let sql = export_sql(ep, &objects, &inline, &progress, &cancel)
            .await
            .unwrap();
        assert!(
            sql.find("CREATE INDEX idx_users_name").unwrap() < sql.find("INSERT INTO").unwrap()
        );
    }
}
