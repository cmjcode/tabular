//! Aksi objek skema dari sidebar: rename, comment, maintenance, materialized
//! view, manajemen schema PostgreSQL, edit tipe buatan user, dan tampil/ekspor
//! source routine.
//!
//! Sidebar hanya memasukkan [`SchemaAction`] ke antrean (`queue_action`); tiap
//! frame [`Tabular::render_schema_ui`] mengosongkan antrean, membuka dialog, dan
//! menjalankan SQL di latar belakang lewat `schema_objects::run_in_database`.
//! Setiap dialog menampilkan SQL yang akan dijalankan sebelum dieksekusi.

use super::Tabular;
use crate::models::enums::DatabaseType;
use crate::schema_objects::catalog::{self, RoutineKind};
use crate::schema_objects::sql::{self, MaintenanceOp, RenameTarget, SchemaGrant, TypeEdit};
use crate::schema_objects::{ResultSet, run_in_database};
use eframe::egui;
use std::sync::{Arc, Mutex};

pub type SqlOutcome = Result<Vec<ResultSet>, String>;

/// Slot hasil task SQL latar belakang; diisi sekali oleh task, diambil oleh UI.
#[derive(Clone, Default)]
pub struct TaskSlot(Arc<Mutex<Option<SqlOutcome>>>);

impl TaskSlot {
    fn set(&self, outcome: SqlOutcome) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some(outcome);
        }
    }

    pub(crate) fn take(&self) -> Option<SqlOutcome> {
        self.0.lock().ok().and_then(|mut guard| guard.take())
    }
}

/// Permintaan dari sidebar / structure editor.
#[derive(Clone, Debug)]
pub enum SchemaAction {
    Rename {
        conn_id: i64,
        database: Option<String>,
        target: RenameTarget,
        name: String,
    },
    EditComment {
        conn_id: i64,
        database: Option<String>,
        table: String,
        is_view: bool,
    },
    Maintenance {
        conn_id: i64,
        database: Option<String>,
        table: Option<String>,
    },
    RefreshMatView {
        conn_id: i64,
        database: Option<String>,
        name: String,
    },
    DropView {
        conn_id: i64,
        database: Option<String>,
        name: String,
        materialized: bool,
    },
    ViewSource {
        conn_id: i64,
        database: Option<String>,
        kind: RoutineKind,
        name: String,
        export: bool,
    },
    CreateSchema {
        conn_id: i64,
        database: Option<String>,
    },
    ManageSchemas {
        conn_id: i64,
        database: Option<String>,
    },
    EditType {
        conn_id: i64,
        database: Option<String>,
        name: String,
    },
    /// Balik preferensi "Show system databases/schemas".
    ToggleSystemObjects,
    /// SQL jadi dari structure editor (tambah/hapus FK, check, trigger, kolom
    /// generated); tetap ditampilkan dulu sebelum dieksekusi.
    RunSql {
        conn_id: i64,
        database: Option<String>,
        title: String,
        sql: String,
        destructive: bool,
    },
}

const QUEUE_ID: &str = "tabular_schema_object_actions";

/// Masukkan aksi ke antrean; diproses di frame yang sama oleh `render_schema_ui`.
pub fn queue_action(ctx: &egui::Context, action: SchemaAction) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<SchemaAction>>(egui::Id::new(QUEUE_ID))
            .push(action)
    });
}

fn drain_actions(ctx: &egui::Context) -> Vec<SchemaAction> {
    ctx.data_mut(|d| d.remove_temp::<Vec<SchemaAction>>(egui::Id::new(QUEUE_ID)))
        .unwrap_or_default()
}

/// State UI aksi skema milik `Tabular`.
#[derive(Default)]
pub struct SchemaUiState {
    dialog: Option<SchemaDialog>,
    fetches: Vec<SourceFetch>,
    /// State tab Foreign Keys / Checks / Triggers / Generated / DDL di Structure.
    pub(crate) structure: crate::data_table::structure_objects::StructureObjectsState,
}

struct SourceFetch {
    slot: TaskSlot,
    conn_id: i64,
    database: Option<String>,
    kind: RoutineKind,
    name: String,
    header: Option<&'static str>,
    export: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SchemaOp {
    ChangeOwner,
    Rename,
    Grant,
    Revoke,
    Drop,
}

impl SchemaOp {
    const ALL: [SchemaOp; 5] = [
        SchemaOp::ChangeOwner,
        SchemaOp::Rename,
        SchemaOp::Grant,
        SchemaOp::Revoke,
        SchemaOp::Drop,
    ];

    fn label(&self) -> &'static str {
        match self {
            SchemaOp::ChangeOwner => "Change owner",
            SchemaOp::Rename => "Rename",
            SchemaOp::Grant => "Grant privileges",
            SchemaOp::Revoke => "Revoke all from role",
            SchemaOp::Drop => "Drop schema",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeOp {
    AddEnumValue,
    RenameEnumValue,
    AddAttribute,
    DropAttribute,
    RenameAttribute,
    SetDefault,
    NotNull,
    AddCheck,
    DropConstraint,
    Rename,
    Drop,
}

impl TypeOp {
    fn label(&self) -> &'static str {
        match self {
            TypeOp::AddEnumValue => "Add value",
            TypeOp::RenameEnumValue => "Rename value",
            TypeOp::AddAttribute => "Add attribute",
            TypeOp::DropAttribute => "Drop attribute",
            TypeOp::RenameAttribute => "Rename attribute",
            TypeOp::SetDefault => "Set / drop default",
            TypeOp::NotNull => "NOT NULL",
            TypeOp::AddCheck => "Add check constraint",
            TypeOp::DropConstraint => "Drop constraint",
            TypeOp::Rename => "Rename type",
            TypeOp::Drop => "Drop type",
        }
    }

    /// Operasi yang berlaku untuk `typtype` PostgreSQL.
    fn for_kind(kind: &str) -> &'static [TypeOp] {
        use TypeOp::*;
        match kind {
            "e" => &[AddEnumValue, RenameEnumValue, Rename, Drop],
            "c" => &[AddAttribute, DropAttribute, RenameAttribute, Rename, Drop],
            "d" => &[SetDefault, NotNull, AddCheck, DropConstraint, Rename, Drop],
            _ => &[Rename, Drop],
        }
    }
}

enum DialogForm {
    Rename {
        target: RenameTarget,
        old_name: String,
        new_name: String,
        mysql_tables: Vec<String>,
    },
    Comment {
        table: String,
        is_view: bool,
        comment: String,
    },
    Maintenance {
        table: Option<String>,
        ops: &'static [MaintenanceOp],
        op: MaintenanceOp,
    },
    RefreshMatView {
        name: String,
        concurrently: bool,
        with_data: bool,
    },
    DropView {
        name: String,
        materialized: bool,
    },
    CreateSchema {
        name: String,
        owner: String,
        grants: Vec<SchemaGrant>,
        roles: Vec<String>,
    },
    ManageSchemas {
        schemas: Vec<Vec<String>>,
        roles: Vec<String>,
        selected: usize,
        op: SchemaOp,
        text: String,
        grant: SchemaGrant,
        cascade: bool,
    },
    Raw {
        title: String,
        sql: String,
        destructive: bool,
    },
    EditType {
        name: String,
        ddl: String,
        kind: String,
        labels: Vec<String>,
        op: TypeOp,
        a: String,
        b: String,
        before: bool,
        flag: bool,
    },
}

struct SchemaDialog {
    conn_id: i64,
    /// Database tempat SQL dijalankan (bisa berbeda dari database objek, mis.
    /// rename database MsSQL dijalankan di `master`).
    exec_database: Option<String>,
    /// Database objek (untuk refresh cache setelah sukses).
    object_database: Option<String>,
    db_type: DatabaseType,
    form: DialogForm,
    prepare: Option<TaskSlot>,
    blocking_error: Option<String>,
    run: Option<TaskSlot>,
    run_error: Option<String>,
    run_output: Vec<ResultSet>,
}

impl SchemaDialog {
    fn title(&self) -> String {
        match &self.form {
            DialogForm::Rename {
                target, old_name, ..
            } => format!("Rename {} {}", target.label(), old_name),
            DialogForm::Comment { table, .. } => format!("Comment on {}", table),
            DialogForm::Maintenance { table, .. } => match table {
                Some(t) => format!("Maintenance: {}", t),
                None => format!(
                    "Maintenance: database {}",
                    self.object_database.as_deref().unwrap_or("")
                ),
            },
            DialogForm::RefreshMatView { name, .. } => {
                format!("Refresh materialized view {}", name)
            }
            DialogForm::DropView { name, materialized } => format!(
                "Drop {} {}",
                if *materialized {
                    "materialized view"
                } else {
                    "view"
                },
                name
            ),
            DialogForm::CreateSchema { .. } => "Create schema".to_string(),
            DialogForm::ManageSchemas { .. } => format!(
                "Schemas in {}",
                self.object_database.as_deref().unwrap_or("database")
            ),
            DialogForm::EditType { name, .. } => format!("Edit type {}", name),
            DialogForm::Raw { title, .. } => title.clone(),
        }
    }

    fn is_destructive(&self) -> bool {
        match &self.form {
            DialogForm::DropView { .. } => true,
            DialogForm::Raw { destructive, .. } => *destructive,
            DialogForm::Rename { target, .. } => *target == RenameTarget::Database,
            DialogForm::Maintenance { op, .. } => op.is_heavy(),
            DialogForm::ManageSchemas { op, .. } => {
                matches!(op, SchemaOp::Drop | SchemaOp::Revoke)
            }
            DialogForm::EditType { op, .. } => {
                matches!(
                    op,
                    TypeOp::Drop | TypeOp::DropAttribute | TypeOp::DropConstraint
                )
            }
            _ => false,
        }
    }

    /// SQL yang akan dijalankan, atau alasan kenapa belum bisa.
    fn build_sql(&self) -> Result<String, String> {
        if let Some(err) = &self.blocking_error {
            return Err(err.clone());
        }
        let db = &self.db_type;
        match &self.form {
            DialogForm::Rename {
                target,
                old_name,
                new_name,
                mysql_tables,
            } => sql::rename_sql(db, *target, old_name, new_name, mysql_tables),
            DialogForm::Comment {
                table,
                is_view,
                comment,
            } => sql::comment_table_sql(db, table, *is_view, comment),
            DialogForm::Maintenance { table, op, .. } => sql::maintenance_sql(
                db,
                *op,
                table.as_deref(),
                self.object_database.as_deref().unwrap_or(""),
            ),
            DialogForm::RefreshMatView {
                name,
                concurrently,
                with_data,
            } => Ok(sql::refresh_matview_sql(name, *concurrently, *with_data)),
            DialogForm::DropView { name, materialized } => {
                Ok(sql::drop_view_sql(db, name, *materialized))
            }
            DialogForm::Raw { sql, .. } => Ok(sql.clone()),
            DialogForm::CreateSchema {
                name,
                owner,
                grants,
                ..
            } => sql::create_schema_sql(name, owner, grants),
            DialogForm::ManageSchemas {
                schemas,
                selected,
                op,
                text,
                grant,
                cascade,
                ..
            } => {
                let schema = schemas
                    .get(*selected)
                    .and_then(|r| r.first())
                    .ok_or_else(|| "Select a schema".to_string())?;
                match op {
                    SchemaOp::ChangeOwner => {
                        if text.trim().is_empty() {
                            Err("Choose the new owner".to_string())
                        } else {
                            Ok(sql::alter_schema_sql(schema, Some(text), &[], &[]))
                        }
                    }
                    SchemaOp::Rename => {
                        sql::rename_sql(db, RenameTarget::Schema, schema, text, &[])
                    }
                    SchemaOp::Grant => {
                        let sql =
                            sql::alter_schema_sql(schema, None, std::slice::from_ref(grant), &[]);
                        if sql.is_empty() {
                            Err("Choose a role and at least one privilege".to_string())
                        } else {
                            Ok(sql)
                        }
                    }
                    SchemaOp::Revoke => {
                        if text.trim().is_empty() {
                            Err("Choose the role to revoke from".to_string())
                        } else {
                            Ok(sql::alter_schema_sql(
                                schema,
                                None,
                                &[],
                                std::slice::from_ref(text),
                            ))
                        }
                    }
                    SchemaOp::Drop => Ok(sql::drop_schema_sql(schema, *cascade)),
                }
            }
            DialogForm::EditType {
                name,
                kind,
                op,
                a,
                b,
                before,
                flag,
                ..
            } => {
                let edit = match op {
                    TypeOp::AddEnumValue => TypeEdit::AddEnumValue {
                        value: a.clone(),
                        position: (!b.is_empty()).then(|| (*before, b.clone())),
                    },
                    TypeOp::RenameEnumValue => TypeEdit::RenameEnumValue {
                        from: b.clone(),
                        to: a.clone(),
                    },
                    TypeOp::AddAttribute => TypeEdit::AddAttribute {
                        name: a.clone(),
                        data_type: b.clone(),
                    },
                    TypeOp::DropAttribute => TypeEdit::DropAttribute {
                        name: a.clone(),
                        cascade: *flag,
                    },
                    TypeOp::RenameAttribute => TypeEdit::RenameAttribute {
                        from: a.clone(),
                        to: b.clone(),
                    },
                    TypeOp::SetDefault => TypeEdit::SetDomainDefault(Some(a.clone())),
                    TypeOp::NotNull => TypeEdit::SetDomainNotNull(*flag),
                    TypeOp::AddCheck => TypeEdit::AddDomainCheck {
                        name: a.clone(),
                        expr: b.clone(),
                    },
                    TypeOp::DropConstraint => TypeEdit::DropDomainConstraint { name: a.clone() },
                    TypeOp::Rename => TypeEdit::RenameType { to: a.clone() },
                    TypeOp::Drop => TypeEdit::DropType { cascade: *flag },
                };
                sql::alter_type_sql(name, kind == "d", &edit)
            }
        }
    }

    /// Isi form dari hasil query persiapan (comment saat ini, daftar role, …).
    fn apply_prepared(&mut self, outcome: SqlOutcome) {
        let sets = match outcome {
            Ok(sets) => sets,
            Err(e) => {
                self.blocking_error = Some(format!("Could not load details: {}", e));
                return;
            }
        };
        match &mut self.form {
            DialogForm::Rename { mysql_tables, .. } => {
                *mysql_tables = sets.first().map(|s| s.first_column()).unwrap_or_default();
            }
            DialogForm::Comment { comment, .. } => {
                *comment = sets
                    .first()
                    .and_then(|s| s.first_value())
                    .unwrap_or_default()
                    .to_string();
            }
            DialogForm::CreateSchema { roles, .. } => {
                *roles = sets.first().map(|s| s.first_column()).unwrap_or_default();
            }
            DialogForm::ManageSchemas {
                schemas,
                roles,
                selected,
                ..
            } => {
                *schemas = sets.first().map(|s| s.rows.clone()).unwrap_or_default();
                *roles = sets.get(1).map(|s| s.first_column()).unwrap_or_default();
                if *selected >= schemas.len() {
                    *selected = 0;
                }
            }
            DialogForm::EditType {
                ddl,
                kind,
                labels,
                op,
                ..
            } => {
                if let Some(first) = sets.first().and_then(|s| s.rows.first()) {
                    *ddl = first.first().cloned().unwrap_or_default();
                    *kind = first.get(1).cloned().unwrap_or_default();
                } else {
                    self.blocking_error = Some("Type not found".to_string());
                    return;
                }
                *labels = sets.get(1).map(|s| s.first_column()).unwrap_or_default();
                *op = TypeOp::for_kind(kind)[0];
            }
            _ => {}
        }
    }
}

/// Apa yang perlu disegarkan setelah SQL sukses.
enum AfterSuccess {
    Nothing,
    StructureChanged,
    RefreshFolders,
    RefreshConnection,
    ReloadDialog,
}

impl Tabular {
    fn pool_for(&self, conn_id: i64) -> Option<crate::models::enums::DatabasePool> {
        self.connection_pools.get(&conn_id).cloned().or_else(|| {
            self.shared_connection_pools
                .lock()
                .ok()
                .and_then(|p| p.get(&conn_id).cloned())
        })
    }

    /// Jalankan SQL di latar belakang pada `database` koneksi `conn_id`.
    pub(crate) fn spawn_schema_sql(
        &mut self,
        ctx: &egui::Context,
        conn_id: i64,
        database: Option<String>,
        sql: String,
    ) -> TaskSlot {
        let slot = TaskSlot::default();
        let Some(conn) = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()
        else {
            slot.set(Err("Connection not found".to_string()));
            return slot;
        };
        let pool = self.pool_for(conn_id);
        let runtime = self.get_runtime();
        let task_slot = slot.clone();
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let outcome = run_in_database(&conn, pool, database.as_deref(), &sql).await;
            if let Err(e) = &outcome {
                log::warn!("[SCHEMA] statement failed: {}", e);
            }
            task_slot.set(outcome);
            ctx.request_repaint();
        });
        slot
    }

    /// Versi blocking untuk loader tree (loader yang sudah ada juga sinkron).
    /// Dibatasi `timeout_secs`.
    pub(crate) fn run_schema_sql_blocking(
        &mut self,
        conn_id: i64,
        database: Option<&str>,
        sql: &str,
        timeout_secs: u64,
    ) -> SqlOutcome {
        let conn = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()
            .ok_or_else(|| "Connection not found".to_string())?;
        let pool = self.pool_for(conn_id);
        let runtime = self.get_runtime();
        runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                run_in_database(&conn, pool, database, sql),
            )
            .await
            .unwrap_or_else(|_| Err("Timed out".to_string()))
        })
    }

    fn connection_type_of(&self, conn_id: i64) -> Option<DatabaseType> {
        self.connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.connection_type.clone())
    }

    /// Dipanggil setiap frame dari `app_impl`.
    pub(crate) fn render_schema_ui(&mut self, ctx: &egui::Context) {
        for action in drain_actions(ctx) {
            self.start_schema_action(ctx, action);
        }
        self.poll_source_fetches();
        self.render_schema_dialog(ctx);
    }

    fn start_schema_action(&mut self, ctx: &egui::Context, action: SchemaAction) {
        match action {
            SchemaAction::ViewSource {
                conn_id,
                database,
                kind,
                name,
                export,
            } => {
                let Some(db_type) = self.connection_type_of(conn_id) else {
                    return;
                };
                let Some((query, header)) = catalog::routine_source_query(&db_type, kind, &name)
                else {
                    self.toasts.warning(format!(
                        "Viewing {} source is not supported for {:?}",
                        kind.label().to_lowercase(),
                        db_type
                    ));
                    return;
                };
                let slot = self.spawn_schema_sql(ctx, conn_id, database.clone(), query);
                self.schema_ui.fetches.push(SourceFetch {
                    slot,
                    conn_id,
                    database,
                    kind,
                    name,
                    header,
                    export,
                });
            }
            SchemaAction::ToggleSystemObjects => {
                let show = !self.show_system_objects;
                self.apply_show_system_objects(show);
            }
            other => self.open_schema_dialog(ctx, other),
        }
    }

    fn open_schema_dialog(&mut self, ctx: &egui::Context, action: SchemaAction) {
        let (conn_id, database) = match &action {
            SchemaAction::Rename {
                conn_id, database, ..
            }
            | SchemaAction::EditComment {
                conn_id, database, ..
            }
            | SchemaAction::Maintenance {
                conn_id, database, ..
            }
            | SchemaAction::RefreshMatView {
                conn_id, database, ..
            }
            | SchemaAction::DropView {
                conn_id, database, ..
            }
            | SchemaAction::ViewSource {
                conn_id, database, ..
            }
            | SchemaAction::CreateSchema { conn_id, database }
            | SchemaAction::ManageSchemas { conn_id, database }
            | SchemaAction::EditType {
                conn_id, database, ..
            }
            | SchemaAction::RunSql {
                conn_id, database, ..
            } => (*conn_id, database.clone()),
            SchemaAction::ToggleSystemObjects => return,
        };
        let Some(db_type) = self.connection_type_of(conn_id) else {
            return;
        };
        let default_db = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.database.clone())
            .unwrap_or_default();

        let mut exec_database = database.clone();
        let mut blocking_error = None;
        let mut prepare_sql: Option<String> = None;

        let form = match action {
            SchemaAction::Rename { target, name, .. } => {
                if !sql::supports_rename(&db_type, target) {
                    self.toasts.warning(format!(
                        "Renaming a {} is not supported for {:?}",
                        target.label(),
                        db_type
                    ));
                    return;
                }
                if target == RenameTarget::Database {
                    match db_type {
                        DatabaseType::PostgreSQL => {
                            // Harus dijalankan dari database lain; pool utama terikat
                            // ke database default koneksi.
                            exec_database = None;
                            if name == default_db || default_db.is_empty() {
                                blocking_error = Some(
                                    "This connection is bound to this database. Set another default database (for example postgres) in the connection settings, then rename it."
                                        .to_string(),
                                );
                            }
                        }
                        DatabaseType::MsSQL => exec_database = Some("master".to_string()),
                        DatabaseType::MySQL => {
                            exec_database = None;
                            prepare_sql = Some(catalog::mysql_base_tables_sql(&name));
                        }
                        _ => {}
                    }
                }
                DialogForm::Rename {
                    target,
                    new_name: sql::split_qualified(&name, "").1,
                    old_name: name,
                    mysql_tables: Vec::new(),
                }
            }
            SchemaAction::EditComment { table, is_view, .. } => {
                let Some(query) = sql::table_comment_query(
                    &db_type,
                    database.as_deref().unwrap_or(&default_db),
                    &table,
                ) else {
                    self.toasts.warning(format!(
                        "Table comments are not supported for {:?}",
                        db_type
                    ));
                    return;
                };
                prepare_sql = Some(query);
                DialogForm::Comment {
                    table,
                    is_view,
                    comment: String::new(),
                }
            }
            SchemaAction::Maintenance { table, .. } => {
                let ops = if table.is_some() {
                    sql::table_maintenance_ops(&db_type)
                } else {
                    sql::database_maintenance_ops(&db_type)
                };
                let Some(first) = ops.first().copied() else {
                    self.toasts
                        .warning(format!("No maintenance operations for {:?}", db_type));
                    return;
                };
                DialogForm::Maintenance {
                    table,
                    ops,
                    op: first,
                }
            }
            SchemaAction::RefreshMatView { name, .. } => DialogForm::RefreshMatView {
                name,
                concurrently: false,
                with_data: true,
            },
            SchemaAction::DropView {
                name, materialized, ..
            } => DialogForm::DropView { name, materialized },
            SchemaAction::CreateSchema { .. } => {
                prepare_sql = Some(catalog::pg_roles_sql().to_string());
                DialogForm::CreateSchema {
                    name: String::new(),
                    owner: String::new(),
                    grants: vec![SchemaGrant::default()],
                    roles: Vec::new(),
                }
            }
            SchemaAction::ManageSchemas { .. } => {
                prepare_sql = Some(manage_schemas_prepare_sql());
                DialogForm::ManageSchemas {
                    schemas: Vec::new(),
                    roles: Vec::new(),
                    selected: 0,
                    op: SchemaOp::ChangeOwner,
                    text: String::new(),
                    grant: SchemaGrant {
                        usage: true,
                        ..Default::default()
                    },
                    cascade: false,
                }
            }
            SchemaAction::EditType { name, .. } => {
                prepare_sql = Some(format!(
                    "{};\n{}",
                    catalog::pg_type_detail_sql(&name),
                    catalog::pg_enum_labels_sql(&name)
                ));
                DialogForm::EditType {
                    name,
                    ddl: String::new(),
                    kind: String::new(),
                    labels: Vec::new(),
                    op: TypeOp::Rename,
                    a: String::new(),
                    b: String::new(),
                    before: false,
                    flag: false,
                }
            }
            SchemaAction::RunSql {
                title,
                sql,
                destructive,
                ..
            } => DialogForm::Raw {
                title,
                sql,
                destructive,
            },
            SchemaAction::ViewSource { .. } | SchemaAction::ToggleSystemObjects => return,
        };

        let prepare =
            prepare_sql.map(|q| self.spawn_schema_sql(ctx, conn_id, exec_database.clone(), q));
        self.schema_ui.dialog = Some(SchemaDialog {
            conn_id,
            exec_database,
            object_database: database,
            db_type,
            form,
            prepare,
            blocking_error,
            run: None,
            run_error: None,
            run_output: Vec::new(),
        });
    }

    fn poll_source_fetches(&mut self) {
        if self.schema_ui.fetches.is_empty() {
            return;
        }
        let fetches = std::mem::take(&mut self.schema_ui.fetches);
        for fetch in fetches {
            let Some(outcome) = fetch.slot.take() else {
                self.schema_ui.fetches.push(fetch);
                continue;
            };
            let source = match outcome {
                Ok(sets) => sets
                    .first()
                    .and_then(|s| s.value_by_header(fetch.header))
                    .map(|s| s.trim_end().to_string()),
                Err(e) => {
                    self.toasts.error(format!(
                        "Could not load {} {}: {}",
                        fetch.kind.label().to_lowercase(),
                        fetch.name,
                        e
                    ));
                    continue;
                }
            };
            let Some(source) = source else {
                self.toasts.warning(format!(
                    "No source found for {} {} (it may be encrypted or you lack privileges)",
                    fetch.kind.label().to_lowercase(),
                    fetch.name
                ));
                continue;
            };
            if fetch.export {
                self.export_source_to_file(&fetch.name, &source);
            } else {
                let title = format!("{}: {}", fetch.kind.label(), fetch.name);
                crate::editor::create_new_tab_with_connection_and_database(
                    self,
                    title,
                    source,
                    Some(fetch.conn_id),
                    fetch.database.clone(),
                );
                self.table_bottom_view = crate::models::structs::TableBottomView::Query;
            }
        }
    }

    #[cfg(not(target_os = "ios"))]
    fn export_source_to_file(&mut self, name: &str, source: &str) {
        let file_name: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SQL", &["sql"])
            .set_file_name(format!("{}.sql", file_name.trim_matches('_')))
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, source) {
            Ok(()) => self.toasts.success(format!("Saved {}", path.display())),
            Err(e) => {
                log::warn!("[SCHEMA] export source failed: {}", e);
                self.toasts.error(format!("Could not save file: {}", e));
            }
        }
    }

    #[cfg(target_os = "ios")]
    fn export_source_to_file(&mut self, _name: &str, _source: &str) {
        self.toasts
            .warning("Saving files is not available on this device".to_string());
    }

    fn render_schema_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.schema_ui.dialog.take() else {
            return;
        };

        if let Some(slot) = &dialog.prepare
            && let Some(outcome) = slot.take()
        {
            dialog.prepare = None;
            dialog.apply_prepared(outcome);
        }

        let mut after: Option<AfterSuccess> = None;
        if let Some(slot) = &dialog.run
            && let Some(outcome) = slot.take()
        {
            dialog.run = None;
            match outcome {
                Ok(sets) => {
                    dialog.run_output = sets.into_iter().filter(|s| !s.rows.is_empty()).collect();
                    dialog.run_error = None;
                    after = Some(after_success(&dialog));
                }
                Err(e) => dialog.run_error = Some(e),
            }
        }

        let mut close = false;
        let mut execute: Option<String> = None;
        let mut open_in_editor: Option<String> = None;
        let title = dialog.title();

        super::style::render_modal_backdrop(ctx, "schema_action_backdrop", true);
        egui::Window::new(&title)
            .id(egui::Id::new("schema_action_dialog"))
            .collapsible(false)
            .resizable(true)
            .title_bar(false)
            .default_width(560.0)
            .frame(super::style::modal_window_frame(ctx))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                super::style::render_modal_header(ui, &title, &mut close);
                ui.add_space(6.0);
                super::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.set_min_width(520.0);
                    if dialog.prepare.is_some() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label("Loading details…");
                        });
                    }
                    render_form(ui, &mut dialog);
                });

                ui.add_space(8.0);
                let built = dialog.build_sql();
                match &built {
                    Ok(sql_text) => {
                        ui.label(egui::RichText::new("SQL to execute").strong());
                        egui::ScrollArea::vertical()
                            .id_salt("schema_action_sql")
                            .max_height(160.0)
                            .show(ui, |ui| {
                                let mut text = sql_text.as_str();
                                ui.add(
                                    egui::TextEdit::multiline(&mut text)
                                        .code_editor()
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(3),
                                );
                            });
                    }
                    Err(reason) => {
                        ui.colored_label(super::style::theme_warning(ui.ctx()), reason);
                    }
                }

                if let Some(err) = &dialog.run_error {
                    ui.add_space(6.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                if !dialog.run_output.is_empty() {
                    ui.add_space(6.0);
                    render_result_sets(ui, &dialog.run_output);
                }

                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let running = dialog.run.is_some();
                    let can_run = built.is_ok() && !running && dialog.prepare.is_none();
                    let label = if dialog.is_destructive() {
                        egui::RichText::new("Execute").color(ui.visuals().error_fg_color)
                    } else {
                        egui::RichText::new("Execute")
                    };
                    if ui.add_enabled(can_run, egui::Button::new(label)).clicked()
                        && let Ok(sql_text) = &built
                    {
                        execute = Some(sql_text.clone());
                    }
                    if ui
                        .add_enabled(built.is_ok(), egui::Button::new("Open in Editor"))
                        .on_hover_text("Open the SQL in a new query tab without running it")
                        .clicked()
                        && let Ok(sql_text) = &built
                    {
                        open_in_editor = Some(sql_text.clone());
                    }
                    let close_label = if dialog.run_output.is_empty() {
                        "Cancel"
                    } else {
                        "Close"
                    };
                    if ui.button(close_label).clicked() {
                        close = true;
                    }
                    if running {
                        ui.add(egui::Spinner::new());
                    }
                });
            });

        if let Some(sql_text) = execute {
            dialog.run_error = None;
            dialog.run_output.clear();
            dialog.run = Some(self.spawn_schema_sql(
                ctx,
                dialog.conn_id,
                dialog.exec_database.clone(),
                sql_text,
            ));
        }
        if let Some(sql_text) = open_in_editor {
            crate::editor::create_new_tab_with_connection_and_database(
                self,
                title.clone(),
                sql_text,
                Some(dialog.conn_id),
                dialog.exec_database.clone(),
            );
            close = true;
        }

        match after {
            Some(AfterSuccess::RefreshFolders) => {
                self.refresh_after_schema_change(dialog.conn_id, dialog.object_database.as_deref());
                self.toasts.success(format!("{}: done", title));
                if dialog.run_output.is_empty() {
                    close = true;
                }
            }
            Some(AfterSuccess::StructureChanged) => {
                self.schema_ui.structure.invalidate();
                crate::data_table::trigger_background_structure_refresh(self);
                self.toasts.success(format!("{}: done", title));
                close = true;
            }
            Some(AfterSuccess::RefreshConnection) => {
                self.refresh_connection(dialog.conn_id);
                self.toasts.success(format!("{}: done", title));
                close = true;
            }
            Some(AfterSuccess::ReloadDialog) => {
                self.toasts.success("Schema updated".to_string());
                if let DialogForm::ManageSchemas { text, .. } = &mut dialog.form {
                    text.clear();
                }
                dialog.prepare = Some(self.spawn_schema_sql(
                    ctx,
                    dialog.conn_id,
                    dialog.exec_database.clone(),
                    manage_schemas_prepare_sql(),
                ));
            }
            Some(AfterSuccess::Nothing) if dialog.run_output.is_empty() => {
                self.toasts.success(format!("{}: done", title));
                close = true;
            }
            Some(AfterSuccess::Nothing) => {}
            None => {}
        }

        if !close {
            self.schema_ui.dialog = Some(dialog);
        }
    }

    /// Simpan preferensi database/schema sistem lalu muat ulang daftar database
    /// koneksi yang sedang terbuka.
    pub(crate) fn apply_show_system_objects(&mut self, show: bool) {
        self.show_system_objects = show;
        crate::schema_objects::set_show_system_objects(show);
        self.prefs_dirty = true;
        self.try_save_prefs();
        let connected: Vec<i64> = self.connection_pools.keys().copied().collect();
        for conn_id in connected {
            self.refresh_connection(conn_id);
        }
    }

    /// Isi folder "Partitions" dengan batas partisi dan perkiraan jumlah baris
    /// langsung dari server. Bila gagal, daftar dari cache dipertahankan.
    pub(crate) fn load_partitions_folder(
        &mut self,
        conn_id: i64,
        folder: &mut crate::models::structs::TreeNode,
    ) {
        folder.is_loaded = true;
        let (Some(db_type), Some(table)) =
            (self.connection_type_of(conn_id), folder.table_name.clone())
        else {
            return;
        };
        let database = folder.database_name.clone().unwrap_or_default();
        let Some(query) = catalog::partitions_query(&db_type, &database, &table) else {
            return;
        };
        let db_arg = (!database.is_empty()).then_some(database.as_str());
        match self.run_schema_sql_blocking(conn_id, db_arg, &query, 10) {
            Ok(sets) => {
                let parts = sets
                    .first()
                    .map(|s| catalog::parse_partitions(&s.rows))
                    .unwrap_or_default();
                folder.children = if parts.is_empty() {
                    vec![crate::models::structs::TreeNode::new(
                        "(not partitioned)".to_string(),
                        crate::models::enums::NodeType::Column,
                    )]
                } else {
                    parts
                        .into_iter()
                        .map(|part| {
                            let mut n = crate::models::structs::TreeNode::new(
                                part.label(),
                                crate::models::enums::NodeType::Column,
                            );
                            n.connection_id = Some(conn_id);
                            n.database_name = folder.database_name.clone();
                            n.table_name = Some(part.name);
                            n
                        })
                        .collect()
                };
            }
            Err(e) => {
                log::warn!("[SCHEMA] partitions for {} failed: {}", table, e);
                if folder.children.is_empty() {
                    folder.children = vec![crate::models::structs::TreeNode::new(
                        "Failed to load partitions".to_string(),
                        crate::models::enums::NodeType::Column,
                    )];
                }
            }
        }
    }

    /// Kosongkan cache tabel database dan muat ulang folder yang terbuka.
    pub(crate) fn refresh_after_schema_change(&mut self, conn_id: i64, database: Option<&str>) {
        if let Some(db) = database {
            crate::cache_data::clear_tables_from_cache_for_db(self, conn_id, db);
        }
        self.refresh_all_table_folders(conn_id);
    }
}

fn manage_schemas_prepare_sql() -> String {
    format!(
        "{};\n{}",
        catalog::pg_schemas_sql(crate::schema_objects::show_system_objects()),
        catalog::pg_roles_sql()
    )
}

fn after_success(dialog: &SchemaDialog) -> AfterSuccess {
    match &dialog.form {
        DialogForm::Rename { target, .. } if *target == RenameTarget::Database => {
            AfterSuccess::RefreshConnection
        }
        DialogForm::Raw { .. } => AfterSuccess::StructureChanged,
        DialogForm::Rename { .. }
        | DialogForm::DropView { .. }
        | DialogForm::EditType { .. }
        | DialogForm::CreateSchema { .. } => AfterSuccess::RefreshFolders,
        DialogForm::ManageSchemas { .. } => AfterSuccess::ReloadDialog,
        _ => AfterSuccess::Nothing,
    }
}

/// Isian teks satu baris dengan pilihan cepat dari daftar (mis. role).
fn text_with_choices(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    hint: &str,
    choices: &[String],
) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(220.0),
        );
        if !choices.is_empty() {
            egui::ComboBox::from_id_salt(id)
                .selected_text("Pick…")
                .width(90.0)
                .show_ui(ui, |ui| {
                    for choice in choices {
                        if ui.selectable_label(value == choice, choice).clicked() {
                            *value = choice.clone();
                        }
                    }
                });
        }
    });
}

fn render_form(ui: &mut egui::Ui, dialog: &mut SchemaDialog) {
    let db_type = dialog.db_type.clone();
    match &mut dialog.form {
        DialogForm::Rename {
            target,
            new_name,
            mysql_tables,
            ..
        } => {
            ui.label("New name");
            ui.add(egui::TextEdit::singleline(new_name).desired_width(f32::INFINITY));
            if *target == RenameTarget::Database {
                ui.add_space(4.0);
                let note = match db_type {
                    DatabaseType::PostgreSQL => {
                        "PostgreSQL requires that nobody else is connected to the database."
                    }
                    DatabaseType::MsSQL => {
                        "SQL Server needs exclusive access; other sessions may block the rename."
                    }
                    DatabaseType::MySQL => {
                        "MySQL cannot rename a database. The script creates the new database and moves every table; views, routines, triggers and events stay behind."
                    }
                    _ => "",
                };
                ui.label(egui::RichText::new(note).small().weak());
                if db_type == DatabaseType::MySQL {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} tables will be moved.",
                            mysql_tables.len()
                        ))
                        .small(),
                    );
                }
            } else if db_type == DatabaseType::MsSQL {
                ui.label(
                    egui::RichText::new(
                        "sp_rename does not update code in views, procedures or triggers that reference the old name.",
                    )
                    .small()
                    .weak(),
                );
            }
        }
        DialogForm::Comment { comment, .. } => {
            ui.label("Comment (leave empty to remove)");
            ui.add(
                egui::TextEdit::multiline(comment)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
        }
        DialogForm::Maintenance { ops, op, .. } => {
            ui.label("Operation");
            egui::ComboBox::from_id_salt("schema_maintenance_op")
                .selected_text(op.label())
                .show_ui(ui, |ui| {
                    for candidate in ops.iter() {
                        ui.selectable_value(op, *candidate, candidate.label());
                    }
                });
            if op.is_heavy() {
                ui.label(
                    egui::RichText::new(
                        "This operation rewrites data and can lock the table for a long time.",
                    )
                    .small()
                    .color(super::style::theme_warning(ui.ctx())),
                );
            }
        }
        DialogForm::RefreshMatView {
            concurrently,
            with_data,
            ..
        } => {
            ui.checkbox(with_data, "WITH DATA (populate the view)");
            ui.add_enabled(
                *with_data,
                egui::Checkbox::new(concurrently, "CONCURRENTLY (no read lock)"),
            )
            .on_hover_text("Requires a unique index on the materialized view");
        }
        DialogForm::DropView { .. } => {
            ui.label("This cannot be undone. Objects that depend on it will make the drop fail.");
        }
        DialogForm::Raw { destructive, .. } => {
            if *destructive {
                ui.label("This cannot be undone.");
            } else {
                ui.label("Review the statement below before running it.");
            }
        }
        DialogForm::CreateSchema {
            name,
            owner,
            grants,
            roles,
        } => {
            egui::Grid::new("schema_create_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(name).desired_width(220.0));
                    ui.end_row();
                    ui.label("Owner");
                    text_with_choices(ui, "schema_create_owner", owner, "current user", roles);
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Privileges").strong());
            let mut remove: Option<usize> = None;
            for (i, grant) in grants.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    text_with_choices(
                        ui,
                        &format!("schema_create_grant_{}", i),
                        &mut grant.role,
                        "role or PUBLIC",
                        roles,
                    );
                    ui.checkbox(&mut grant.usage, "USAGE");
                    ui.checkbox(&mut grant.create, "CREATE");
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                grants.remove(i);
            }
            if ui.small_button("+ Add grant").clicked() {
                grants.push(SchemaGrant {
                    usage: true,
                    ..Default::default()
                });
            }
        }
        DialogForm::ManageSchemas {
            schemas,
            roles,
            selected,
            op,
            text,
            grant,
            cascade,
        } => {
            egui::ScrollArea::vertical()
                .id_salt("schema_manage_list")
                .max_height(180.0)
                .show(ui, |ui| {
                    egui::Grid::new("schema_manage_grid")
                        .num_columns(3)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Schema").strong());
                            ui.label(egui::RichText::new("Owner").strong());
                            ui.label(egui::RichText::new("Privileges").strong());
                            ui.end_row();
                            for (i, row) in schemas.iter().enumerate() {
                                let name = row.first().cloned().unwrap_or_default();
                                if ui.selectable_label(*selected == i, &name).clicked() {
                                    *selected = i;
                                }
                                ui.label(row.get(1).cloned().unwrap_or_default());
                                ui.label(
                                    egui::RichText::new(row.get(2).cloned().unwrap_or_default())
                                        .small()
                                        .weak(),
                                );
                                ui.end_row();
                            }
                        });
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Action");
                egui::ComboBox::from_id_salt("schema_manage_op")
                    .selected_text(op.label())
                    .show_ui(ui, |ui| {
                        for candidate in SchemaOp::ALL {
                            ui.selectable_value(op, candidate, candidate.label());
                        }
                    });
            });
            match op {
                SchemaOp::ChangeOwner => {
                    text_with_choices(ui, "schema_manage_owner", text, "new owner", roles)
                }
                SchemaOp::Rename => {
                    ui.add(egui::TextEdit::singleline(text).hint_text("new name"));
                }
                SchemaOp::Grant => {
                    ui.horizontal(|ui| {
                        text_with_choices(
                            ui,
                            "schema_manage_grant",
                            &mut grant.role,
                            "role or PUBLIC",
                            roles,
                        );
                        ui.checkbox(&mut grant.usage, "USAGE");
                        ui.checkbox(&mut grant.create, "CREATE");
                    });
                }
                SchemaOp::Revoke => {
                    text_with_choices(ui, "schema_manage_revoke", text, "role or PUBLIC", roles)
                }
                SchemaOp::Drop => {
                    ui.checkbox(cascade, "CASCADE (also drop every object in the schema)");
                }
            }
        }
        DialogForm::EditType {
            ddl,
            kind,
            labels,
            op,
            a,
            b,
            before,
            flag,
            ..
        } => {
            if !ddl.is_empty() {
                egui::ScrollArea::vertical()
                    .id_salt("schema_type_ddl")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        let mut text = ddl.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
                ui.add_space(6.0);
            }
            let previous = *op;
            ui.horizontal(|ui| {
                ui.label("Change");
                egui::ComboBox::from_id_salt("schema_type_op")
                    .selected_text(op.label())
                    .show_ui(ui, |ui| {
                        for candidate in TypeOp::for_kind(kind) {
                            ui.selectable_value(op, *candidate, candidate.label());
                        }
                    });
            });
            if *op != previous {
                a.clear();
                b.clear();
                *flag = false;
            }
            match op {
                TypeOp::AddEnumValue => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("new value"));
                    ui.horizontal(|ui| {
                        ui.radio_value(before, true, "before");
                        ui.radio_value(before, false, "after");
                        egui::ComboBox::from_id_salt("schema_type_pos")
                            .selected_text(if b.is_empty() { "(end)" } else { b.as_str() })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(b, String::new(), "(end)");
                                for label in labels.iter() {
                                    ui.selectable_value(b, label.clone(), label);
                                }
                            });
                    });
                }
                TypeOp::RenameEnumValue => {
                    egui::ComboBox::from_id_salt("schema_type_rename_from")
                        .selected_text(if b.is_empty() { "value…" } else { b.as_str() })
                        .show_ui(ui, |ui| {
                            for label in labels.iter() {
                                ui.selectable_value(b, label.clone(), label);
                            }
                        });
                    ui.add(egui::TextEdit::singleline(a).hint_text("new value"));
                }
                TypeOp::AddAttribute => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("attribute name"));
                    ui.add(egui::TextEdit::singleline(b).hint_text("data type, e.g. text"));
                }
                TypeOp::DropAttribute => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("attribute name"));
                    ui.checkbox(flag, "CASCADE");
                }
                TypeOp::RenameAttribute => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("attribute name"));
                    ui.add(egui::TextEdit::singleline(b).hint_text("new name"));
                }
                TypeOp::SetDefault => {
                    ui.add(
                        egui::TextEdit::singleline(a)
                            .hint_text("default expression (empty = DROP DEFAULT)"),
                    );
                }
                TypeOp::NotNull => {
                    ui.checkbox(flag, "NOT NULL");
                }
                TypeOp::AddCheck => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("constraint name (optional)"));
                    ui.add(egui::TextEdit::singleline(b).hint_text("VALUE > 0"));
                }
                TypeOp::DropConstraint => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("constraint name"));
                }
                TypeOp::Rename => {
                    ui.add(egui::TextEdit::singleline(a).hint_text("new name"));
                }
                TypeOp::Drop => {
                    ui.checkbox(flag, "CASCADE (also drop columns and objects using it)");
                }
            }
        }
    }
}

/// Tampilkan result set kecil (mis. keluaran OPTIMIZE / CHECK / integrity_check).
pub(crate) fn render_result_sets(ui: &mut egui::Ui, sets: &[ResultSet]) {
    egui::ScrollArea::both()
        .id_salt("schema_action_results")
        .max_height(200.0)
        .show(ui, |ui| {
            for (i, set) in sets.iter().enumerate() {
                egui::Grid::new(("schema_action_result", i))
                    .striped(true)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for h in &set.headers {
                            ui.label(egui::RichText::new(h).strong());
                        }
                        ui.end_row();
                        for row in set.rows.iter().take(200) {
                            for cell in row {
                                ui.label(cell);
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
            }
        });
}
