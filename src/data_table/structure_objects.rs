//! Tab tambahan di Structure view: Foreign Keys, Checks, Triggers, Generated
//! columns, dan DDL. Data diambil lewat query katalog `schema_objects`;
//! perubahan (tambah/hapus) selalu lewat dialog pratinjau SQL `RunSql`.

use super::infer_current_table_name;
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::{self, ConstraintKind, FK_ACTIONS, ForeignKeyDraft};
use crate::schema_objects::{ResultSet, catalog};
use crate::window_egui::{
    self,
    schema_actions::{SchemaAction, TaskSlot, queue_action},
};
use eframe::egui;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectsView {
    ForeignKeys,
    Checks,
    Triggers,
    Generated,
    Ddl,
}

impl ObjectsView {
    pub(crate) const ALL: [ObjectsView; 5] = [
        ObjectsView::ForeignKeys,
        ObjectsView::Checks,
        ObjectsView::Triggers,
        ObjectsView::Generated,
        ObjectsView::Ddl,
    ];

    pub(crate) fn tab_label(&self) -> String {
        let (icon, text) = match self {
            ObjectsView::ForeignKeys => (egui_icons::icons::ICON_LINK.codepoint, "Foreign Keys"),
            ObjectsView::Checks => (egui_icons::icons::ICON_CHECK.codepoint, "Checks"),
            ObjectsView::Triggers => (egui_icons::icons::ICON_BOLT.codepoint, "Triggers"),
            ObjectsView::Generated => (egui_icons::icons::MDI_FUNCTION.codepoint, "Generated"),
            ObjectsView::Ddl => (egui_icons::icons::ICON_DESCRIPTION.codepoint, "DDL"),
        };
        format!("{} {}", icon, text)
    }

    fn headers(&self) -> &'static [&'static str] {
        match self {
            ObjectsView::ForeignKeys => &[
                "Name",
                "Columns",
                "References",
                "Referenced Columns",
                "On Update",
                "On Delete",
            ],
            ObjectsView::Checks => &["Name", "Expression"],
            ObjectsView::Triggers => &["Name", "Timing", "Event", "Definition"],
            ObjectsView::Generated => &["Column", "Expression", "Kind"],
            ObjectsView::Ddl => &[],
        }
    }

    fn query(&self, db: &DatabaseType, database: &str, table: &str) -> Option<String> {
        match self {
            ObjectsView::ForeignKeys => catalog::foreign_keys_query(db, database, table),
            ObjectsView::Checks => catalog::check_constraints_query(db, database, table),
            ObjectsView::Triggers => catalog::table_triggers_query(db, database, table),
            ObjectsView::Generated => catalog::generated_columns_query(db, database, table),
            ObjectsView::Ddl => None,
        }
    }
}

type DdlSlot = Arc<Mutex<Option<Option<String>>>>;

/// State tab objek structure; disimpan di `Tabular::schema_ui`.
#[derive(Default)]
pub(crate) struct StructureObjectsState {
    /// `Some` bila salah satu tab tambahan sedang aktif (menggantikan
    /// Columns/Indexes).
    pub(crate) view: Option<ObjectsView>,
    key: Option<(i64, String, String, ObjectsView)>,
    slot: Option<TaskSlot>,
    ddl_slot: Option<DdlSlot>,
    result: Option<Result<ResultSet, String>>,
    ddl: Option<Result<String, String>>,
    selected: Option<usize>,
    fk: ForeignKeyDraft,
    check_name: String,
    check_expr: String,
    gen_name: String,
    gen_type: String,
    gen_expr: String,
    gen_stored: bool,
    form_error: Option<String>,
}

impl StructureObjectsState {
    /// Paksa ambil ulang data tab aktif (dipanggil setelah perubahan sukses).
    pub(crate) fn invalidate(&mut self) {
        self.key = None;
        self.slot = None;
        self.ddl_slot = None;
        self.result = None;
        self.ddl = None;
        self.selected = None;
    }
}

struct Target {
    conn_id: i64,
    db_type: DatabaseType,
    database: String,
    table: String,
}

fn current_target(tabular: &mut window_egui::Tabular) -> Option<Target> {
    let table = infer_current_table_name(tabular);
    if table.trim().is_empty() {
        return None;
    }
    let conn_id = tabular.current_connection_id?;
    let conn = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()?;
    let database = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| conn.database.clone());
    Some(Target {
        conn_id,
        db_type: conn.connection_type,
        database,
        table,
    })
}

fn start_fetch(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
    st: &mut StructureObjectsState,
    target: &Target,
    view: ObjectsView,
) {
    if view == ObjectsView::Ddl {
        let Some(conn) = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(target.conn_id))
            .cloned()
        else {
            return;
        };
        let slot: DdlSlot = Arc::default();
        let task_slot = slot.clone();
        let database = target.database.clone();
        let table = target.table.clone();
        let ctx = ctx.clone();
        // `fetch_table_definition` membuat runtime sendiri, jadi dijalankan di
        // thread biasa (bukan di dalam runtime tokio).
        std::thread::spawn(move || {
            let ddl = crate::connection::fetch_table_definition(
                &conn,
                (!database.is_empty()).then_some(database.as_str()),
                &table,
            );
            if let Ok(mut guard) = task_slot.lock() {
                *guard = Some(ddl);
            }
            ctx.request_repaint();
        });
        st.ddl_slot = Some(slot);
        return;
    }
    match view.query(&target.db_type, &target.database, &target.table) {
        Some(query) => {
            st.slot = Some(tabular.spawn_schema_sql(
                ctx,
                target.conn_id,
                Some(target.database.clone()),
                query,
            ));
        }
        None => {
            st.result = Some(Err(format!(
                "{:?} keeps these definitions inside CREATE TABLE; see the DDL tab.",
                target.db_type
            )));
        }
    }
}

fn poll(st: &mut StructureObjectsState) {
    if let Some(slot) = &st.slot
        && let Some(outcome) = slot.take()
    {
        st.slot = None;
        st.result = Some(outcome.map(|sets| sets.into_iter().next().unwrap_or_default()));
    }
    if let Some(slot) = &st.ddl_slot {
        let done = slot.lock().ok().and_then(|mut g| g.take());
        if let Some(ddl) = done {
            st.ddl_slot = None;
            st.ddl =
                Some(ddl.ok_or_else(|| "Could not generate the DDL for this table".to_string()));
        }
    }
}

/// Gambar tab objek yang aktif. Dipanggil dari `render_structure_view`.
pub(crate) fn render_structure_objects(
    tabular: &mut window_egui::Tabular,
    ui: &mut egui::Ui,
    view: ObjectsView,
) {
    let Some(target) = current_target(tabular) else {
        ui.label("Open a table to see its structure.");
        return;
    };
    let mut st = std::mem::take(&mut tabular.schema_ui.structure);
    let key = (
        target.conn_id,
        target.database.clone(),
        target.table.clone(),
        view,
    );
    if st.key.as_ref() != Some(&key) {
        st.invalidate();
        st.form_error = None;
        st.key = Some(key);
        start_fetch(tabular, ui.ctx(), &mut st, &target, view);
    }
    poll(&mut st);

    ui.horizontal(|ui| {
        if ui
            .add(window_egui::style::btn_secondary(format!(
                "{} Reload",
                egui_icons::icons::ICON_REFRESH.codepoint
            )))
            .clicked()
        {
            st.key = None;
        }
        if st.slot.is_some() || st.ddl_slot.is_some() {
            ui.add(egui::Spinner::new());
        }
    });
    ui.add_space(4.0);

    if view == ObjectsView::Ddl {
        render_ddl(tabular, ui, &st, &target);
    } else {
        render_rows(tabular, ui, &mut st, &target, view);
        ui.add_space(8.0);
        ui.separator();
        render_add_form(ui, &mut st, &target, view);
    }

    tabular.schema_ui.structure = st;
}

fn render_ddl(
    tabular: &mut window_egui::Tabular,
    ui: &mut egui::Ui,
    st: &StructureObjectsState,
    target: &Target,
) {
    match &st.ddl {
        None => {
            ui.label("Loading DDL…");
        }
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        Some(Ok(ddl)) => {
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(ddl.clone());
                }
                if ui.button("Open in Editor").clicked() {
                    crate::editor::create_new_tab_with_connection_and_database(
                        tabular,
                        format!("DDL: {}", target.table),
                        ddl.clone(),
                        Some(target.conn_id),
                        Some(target.database.clone()),
                    );
                }
            });
            egui::ScrollArea::both()
                .id_salt("structure_ddl_scroll")
                .show(ui, |ui| {
                    let mut text = ddl.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                });
        }
    }
}

fn short(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > max {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    } else {
        one_line
    }
}

fn render_rows(
    tabular: &mut window_egui::Tabular,
    ui: &mut egui::Ui,
    st: &mut StructureObjectsState,
    target: &Target,
    view: ObjectsView,
) {
    let rows = match &st.result {
        None => {
            if st.slot.is_none() {
                ui.label("No data.");
            }
            return;
        }
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        Some(Ok(set)) => set.rows.clone(),
    };
    if rows.is_empty() {
        ui.label(egui::RichText::new("None defined for this table.").weak());
    } else {
        egui::ScrollArea::both()
            .id_salt(("structure_objects_rows", view as u8))
            .max_height(260.0)
            .show(ui, |ui| {
                egui::Grid::new(("structure_objects_grid", view as u8))
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for h in view.headers() {
                            ui.label(egui::RichText::new(*h).strong());
                        }
                        ui.end_row();
                        for (i, row) in rows.iter().enumerate() {
                            let name = row.first().cloned().unwrap_or_default();
                            if ui.selectable_label(st.selected == Some(i), &name).clicked() {
                                st.selected = Some(i);
                            }
                            for cell in row.iter().skip(1).take(view.headers().len() - 1) {
                                ui.label(short(cell, 80)).on_hover_text(cell);
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    let selected = st.selected.and_then(|i| rows.get(i)).cloned();
    let name = selected
        .as_ref()
        .and_then(|r| r.first().cloned())
        .unwrap_or_default();
    ui.add_space(6.0);
    ui.horizontal(|ui| match view {
        ObjectsView::ForeignKeys | ObjectsView::Checks => {
            let kind = if view == ObjectsView::ForeignKeys {
                ConstraintKind::ForeignKey
            } else {
                ConstraintKind::Check
            };
            if ui
                .add_enabled(
                    selected.is_some(),
                    window_egui::style::btn_danger_ctx(ui.ctx(), "Drop Constraint"),
                )
                .clicked()
            {
                match sql::drop_constraint_sql(&target.db_type, &target.table, kind, &name) {
                    Ok(stmt) => queue_run(ui.ctx(), target, format!("Drop {}", name), stmt, true),
                    Err(e) => st.form_error = Some(e),
                }
            }
        }
        ObjectsView::Triggers => {
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Open Definition"))
                .clicked()
                && let Some(row) = &selected
            {
                let definition = row.get(3).cloned().unwrap_or_default();
                crate::editor::create_new_tab_with_connection_and_database(
                    tabular,
                    format!("Trigger: {}", name),
                    definition,
                    Some(target.conn_id),
                    Some(target.database.clone()),
                );
            }
            if ui.button("New Trigger (template)").clicked() {
                crate::editor::create_new_tab_with_connection_and_database(
                    tabular,
                    format!("New trigger on {}", target.table),
                    sql::trigger_template(&target.db_type, &target.table),
                    Some(target.conn_id),
                    Some(target.database.clone()),
                );
            }
            if ui
                .add_enabled(
                    selected.is_some(),
                    window_egui::style::btn_danger_ctx(ui.ctx(), "Drop Trigger"),
                )
                .clicked()
            {
                let stmt = sql::drop_trigger_sql(&target.db_type, &target.table, &name);
                queue_run(
                    ui.ctx(),
                    target,
                    format!("Drop trigger {}", name),
                    stmt,
                    true,
                );
            }
        }
        ObjectsView::Generated => {
            if ui
                .add_enabled(
                    selected.is_some(),
                    window_egui::style::btn_danger_ctx(ui.ctx(), "Drop Column"),
                )
                .clicked()
            {
                super::trigger_drop_column(tabular, &name);
            }
        }
        ObjectsView::Ddl => {}
    });
}

fn queue_run(ctx: &egui::Context, target: &Target, title: String, sql: String, destructive: bool) {
    queue_action(
        ctx,
        SchemaAction::RunSql {
            conn_id: target.conn_id,
            database: Some(target.database.clone()),
            title,
            sql,
            destructive,
        },
    );
}

fn action_combo(ui: &mut egui::Ui, id: &str, value: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if value.is_empty() {
            "NO ACTION"
        } else {
            value.as_str()
        })
        .show_ui(ui, |ui| {
            for action in FK_ACTIONS {
                ui.selectable_value(value, action.to_string(), action);
            }
        });
}

fn render_add_form(
    ui: &mut egui::Ui,
    st: &mut StructureObjectsState,
    target: &Target,
    view: ObjectsView,
) {
    let mut submit: Option<Result<(String, String), String>> = None;
    match view {
        ObjectsView::ForeignKeys => {
            ui.label(egui::RichText::new("Add foreign key").strong());
            egui::Grid::new("structure_add_fk")
                .num_columns(4)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut st.fk.name).hint_text("optional"));
                    ui.label("Columns");
                    ui.add(egui::TextEdit::singleline(&mut st.fk.columns).hint_text("col1, col2"));
                    ui.end_row();
                    ui.label("References");
                    ui.add(egui::TextEdit::singleline(&mut st.fk.ref_table).hint_text("table"));
                    ui.label("Ref. columns");
                    ui.add(egui::TextEdit::singleline(&mut st.fk.ref_columns).hint_text("id"));
                    ui.end_row();
                    ui.label("On delete");
                    action_combo(ui, "structure_fk_on_delete", &mut st.fk.on_delete);
                    ui.label("On update");
                    action_combo(ui, "structure_fk_on_update", &mut st.fk.on_update);
                    ui.end_row();
                });
            if ui.button("Preview & Add…").clicked() {
                submit = Some(
                    sql::add_foreign_key_sql(&target.db_type, &target.table, &st.fk)
                        .map(|s| ("Add foreign key".to_string(), s)),
                );
            }
        }
        ObjectsView::Checks => {
            ui.label(egui::RichText::new("Add check constraint").strong());
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.check_name)
                        .hint_text("name (optional)")
                        .desired_width(160.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut st.check_expr)
                        .hint_text("price >= 0")
                        .desired_width(280.0),
                );
                if ui.button("Preview & Add…").clicked() {
                    submit = Some(
                        sql::add_check_sql(
                            &target.db_type,
                            &target.table,
                            &st.check_name,
                            &st.check_expr,
                        )
                        .map(|s| ("Add check constraint".to_string(), s)),
                    );
                }
            });
        }
        ObjectsView::Generated => {
            ui.label(egui::RichText::new("Add generated column").strong());
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.gen_name)
                        .hint_text("column")
                        .desired_width(120.0),
                );
                if target.db_type != DatabaseType::MsSQL {
                    ui.add(
                        egui::TextEdit::singleline(&mut st.gen_type)
                            .hint_text("type")
                            .desired_width(110.0),
                    );
                }
                ui.add(
                    egui::TextEdit::singleline(&mut st.gen_expr)
                        .hint_text("qty * price")
                        .desired_width(200.0),
                );
                let stored_label = if target.db_type == DatabaseType::MsSQL {
                    "PERSISTED"
                } else {
                    "STORED"
                };
                ui.add_enabled(
                    target.db_type != DatabaseType::PostgreSQL,
                    egui::Checkbox::new(&mut st.gen_stored, stored_label),
                )
                .on_disabled_hover_text("PostgreSQL generated columns are always STORED");
                if ui.button("Preview & Add…").clicked() {
                    submit = Some(
                        sql::add_generated_column_sql(
                            &target.db_type,
                            &target.table,
                            &st.gen_name,
                            &st.gen_type,
                            &st.gen_expr,
                            st.gen_stored,
                        )
                        .map(|s| ("Add generated column".to_string(), s)),
                    );
                }
            });
        }
        ObjectsView::Triggers => {
            ui.label(
                egui::RichText::new(
                    "Triggers are created from SQL: use “New Trigger (template)” to open an editable script.",
                )
                .weak(),
            );
        }
        ObjectsView::Ddl => {}
    }
    match submit {
        Some(Ok((title, stmt))) => {
            st.form_error = None;
            queue_run(ui.ctx(), target, title, stmt, false);
        }
        Some(Err(e)) => st.form_error = Some(e),
        None => {}
    }
    if let Some(err) = &st.form_error {
        ui.colored_label(window_egui::style::theme_warning(ui.ctx()), err);
    }
}
