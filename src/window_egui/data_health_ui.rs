//! Panel "Data Health" per project: daftar data check (query invariant yang
//! seharusnya kosong), status run terakhir, jumlah baris bermasalah, tren,
//! tombol buka hasil di grid, editor check, jadwal harian, dan usulan check
//! dari AI (disetujui user sebelum disimpan).
//!
//! State disimpan di memori egui (bukan field `Tabular`) karena panel ini
//! berdiri sendiri. Run dan AI berjalan di thread latar; render hanya membaca
//! hasil yang sudah ada (tidak ada query jaringan/DB di render path).

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_icons::icons as ic;

use super::{Tabular, style};
use crate::data_checks::{self, CheckRun, CheckStatus, DataCheck, DataCheckSet, Severity};

type Slot<T> = Arc<Mutex<Option<T>>>;

#[derive(Clone, Default)]
struct State {
    open: bool,
    project_id: Option<String>,
    set: DataCheckSet,
    env: Option<String>,
    last_run: Option<CheckRun>,
    /// Salinan kerja check yang sedang diedit.
    editing: Option<DataCheck>,
    run_job: Option<Slot<Result<CheckRun, String>>>,
    ai_rule: String,
    ai_job: Option<Slot<Result<DataCheck, String>>>,
    ai_proposal: Option<DataCheck>,
    show_ai: bool,
}

/// Status jadwal lintas project.
#[derive(Clone, Default)]
struct Scheduler {
    last_tick: Option<std::time::Instant>,
    /// (project id, hasil) run terjadwal yang sedang berjalan.
    jobs: Vec<(String, Slot<Result<CheckRun, String>>)>,
}

fn state_id() -> egui::Id {
    egui::Id::new("data_health_state")
}

fn scheduler_id() -> egui::Id {
    egui::Id::new("data_health_scheduler")
}

fn load(ctx: &egui::Context) -> State {
    ctx.data_mut(|d| d.get_temp::<State>(state_id()).unwrap_or_default())
}

fn store(ctx: &egui::Context, st: State) {
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

/// Buka panel untuk project aktif (menu bar / command palette).
pub fn open_for_active_project(t: &mut Tabular, ctx: &egui::Context) {
    super::project_ui::ensure_loaded(t);
    let Some(p) = super::project_ui::active(t).cloned() else {
        t.toasts
            .error("Select a project first: Data Health checks belong to a project.".to_string());
        return;
    };
    let mut st = load(ctx);
    if st.project_id.as_deref() != Some(p.id.as_str()) {
        st = State {
            project_id: Some(p.id.clone()),
            env: p.active_environment().map(|e| e.name.clone()),
            ..Default::default()
        };
    }
    st.open = true;
    st.set = data_checks::load_set(&p.id);
    st.last_run = data_checks::load_runs(&p.id, 1).into_iter().next();
    store(ctx, st);
}

fn take<T>(slot: &Option<Slot<T>>) -> Option<T> {
    slot.as_ref()
        .and_then(|s| s.lock().ok().and_then(|mut g| g.take()))
}

fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Slot<T> {
    let slot: Slot<T> = Arc::new(Mutex::new(None));
    let out = slot.clone();
    std::thread::spawn(move || {
        let v = f();
        if let Ok(mut g) = out.lock() {
            *g = Some(v);
        }
    });
    slot
}

/// Jalankan check project di latar dengan variabel environment `env`.
fn spawn_run(
    project: crate::project::Project,
    env: Option<String>,
    set: DataCheckSet,
    only: Vec<String>,
) -> Slot<Result<CheckRun, String>> {
    spawn(move || {
        let env_name = env.or_else(|| project.active_environment().map(|e| e.name.clone()));
        let vars = env_name
            .as_deref()
            .map(|e| project.env_vars_with(e, crate::secrets::get_secret))
            .unwrap_or_default();
        let req = data_checks::RunRequest {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            environment: env_name,
            vars,
            only,
        };
        data_checks::run_and_save(&set, &req, None, &mut |_| {})
    })
}

fn status_color(ctx: &egui::Context, s: CheckStatus) -> egui::Color32 {
    match s {
        CheckStatus::Healthy => style::theme_success(ctx),
        CheckStatus::Failing => style::theme_danger(ctx),
        CheckStatus::Error => style::theme_warning(ctx),
        CheckStatus::Skipped => ctx.global_style().visuals.weak_text_color(),
    }
}

fn status_icon(s: CheckStatus) -> &'static str {
    match s {
        CheckStatus::Healthy => ic::ICON_CHECK_CIRCLE.codepoint,
        CheckStatus::Failing => ic::ICON_ERROR.codepoint,
        CheckStatus::Error => ic::ICON_WARNING.codepoint,
        CheckStatus::Skipped => ic::ICON_REMOVE.codepoint,
    }
}

/// Koneksi untuk nilai variabel koneksi (nama atau id).
fn connection_for(t: &Tabular, value: &str) -> Option<i64> {
    let v = value.trim();
    if let Ok(id) = v.parse::<i64>()
        && t.connections.iter().any(|c| c.id == Some(id))
    {
        return Some(id);
    }
    t.connections
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(v))
        .and_then(|c| c.id)
}

/// Buka query check di tab baru dan jalankan (hasil di grid).
fn open_in_grid(
    t: &mut Tabular,
    project: &crate::project::Project,
    env: Option<&str>,
    check: &DataCheck,
) {
    let env_name = env
        .map(str::to_string)
        .or_else(|| project.active_environment().map(|e| e.name.clone()));
    let vars = env_name
        .as_deref()
        .map(|e| project.env_vars_with(e, crate::secrets::get_secret))
        .unwrap_or_default();
    let Some(conn_value) = vars.get(check.connection_var.trim()) else {
        t.toasts.error(format!(
            "Variable {{{{{}}}}} is not set in environment {}",
            check.connection_var,
            env_name.unwrap_or_default()
        ));
        return;
    };
    let Some(conn_id) = connection_for(t, conn_value) else {
        t.toasts
            .error(format!("Connection '{conn_value}' not found in Tabular"));
        return;
    };
    // Variabel lain diganti teks hanya untuk ditampilkan di editor; query
    // dijalankan lewat jalur editor biasa (Safety mode koneksi berlaku).
    let sql = crate::http_tests::substitute(&check.sql, &vars);
    crate::editor::create_new_tab_with_connection_and_database(
        t,
        format!("Check: {}", check.name),
        sql.clone(),
        Some(conn_id),
        check.database.clone(),
    );
    t.pending_query = sql;
    crate::editor::execute_query(t);
}

/// Render panel bila terbuka dan proses jadwal. Dipanggil sekali per frame.
pub fn render(t: &mut Tabular, ctx: &egui::Context) {
    tick_scheduler(t, ctx);
    let mut st = load(ctx);
    if !st.open {
        return;
    }
    if let Some(res) = take(&st.run_job) {
        st.run_job = None;
        match res {
            Ok(run) => {
                let failing = run.count(CheckStatus::Failing);
                if failing > 0 {
                    t.toasts.error(format!(
                        "Data Health: {failing} check(s) found broken data ({})",
                        run.verdict()
                    ));
                } else {
                    t.toasts.info(format!("Data Health: {}", run.verdict()));
                }
                st.last_run = Some(run);
            }
            Err(e) => t.toasts.error(format!("Data Health: {e}")),
        }
    }
    if let Some(res) = take(&st.ai_job) {
        st.ai_job = None;
        match res {
            Ok(c) => st.ai_proposal = Some(c),
            Err(e) => t.toasts.error(format!("Generate with AI: {e}")),
        }
    }
    if st.run_job.is_some() || st.ai_job.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    let project = st
        .project_id
        .as_deref()
        .and_then(|id| t.projects.list.iter().find(|p| p.id == id))
        .cloned();
    let Some(project) = project else {
        st.open = false;
        store(ctx, st);
        return;
    };

    let mut open = st.open;
    let mut actions: Vec<Action> = Vec::new();
    egui::Window::new(format!("Data Health: {}", project.name))
        .id(egui::Id::new("data_health_window"))
        .open(&mut open)
        .default_size([760.0, 520.0])
        .resizable(true)
        .show(ctx, |ui| {
            header(ui, &mut st, &project, &mut actions);
            ui.separator();
            if st.set.checks.is_empty() {
                empty_state(ui, &mut actions);
            } else {
                checks_table(ui, &st, &mut actions);
            }
            ui.separator();
            if let Some(draft) = st.editing.as_mut() {
                editor(ui, draft, &mut actions);
            }
            ai_section(ui, &mut st, &mut actions);
            schedule_section(ui, &mut st, &project, &mut actions);
        });
    st.open = open;
    for a in actions {
        apply(t, &mut st, &project, a);
    }
    store(ctx, st);
}

enum Action {
    RunAll,
    RunOne(String),
    OpenGrid(String),
    Edit(String),
    New,
    Delete(String),
    Toggle(String),
    SaveDraft,
    CancelDraft,
    AddStarters,
    GenerateAi,
    AcceptAi,
    RejectAi,
    SaveSchedule,
}

fn header(
    ui: &mut egui::Ui,
    st: &mut State,
    project: &crate::project::Project,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(ic::ICON_MONITOR_HEART.codepoint).size(18.0));
        ui.label(
            egui::RichText::new("Queries that must return no rows. Every row is broken data.")
                .weak(),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Environment");
        egui::ComboBox::from_id_salt("dh_env")
            .selected_text(st.env.clone().unwrap_or_else(|| "(none)".into()))
            .show_ui(ui, |ui| {
                for e in &project.environments {
                    ui.selectable_value(&mut st.env, Some(e.name.clone()), &e.name);
                }
            });
        let running = st.run_job.is_some();
        if ui
            .add_enabled(
                !running && !st.set.checks.is_empty(),
                egui::Button::new(format!("{} Run all checks", ic::ICON_PLAY_ARROW.codepoint)),
            )
            .on_hover_text("Read-only SELECTs with a row limit and timeout; safe on Production")
            .clicked()
        {
            actions.push(Action::RunAll);
        }
        if running {
            ui.spinner();
            ui.label("Checking data...");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(format!("{} New check", ic::ICON_ADD.codepoint))
                .clicked()
            {
                actions.push(Action::New);
            }
            if ui
                .button(format!(
                    "{} Generate with AI",
                    ic::ICON_AUTO_AWESOME.codepoint
                ))
                .clicked()
            {
                st.show_ai = !st.show_ai;
            }
        });
    });
    if let Some(run) = &st.last_run {
        let color = match run.verdict() {
            "HEALTHY" => style::theme_success(ui.ctx()),
            "FAILING" => style::theme_danger(ui.ctx()),
            _ => style::theme_warning(ui.ctx()),
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(run.verdict()).strong().color(color));
            ui.label(
                egui::RichText::new(format!(
                    "last run {} on {}: {} healthy, {} failing, {} error",
                    run.started_at
                        .get(..16)
                        .unwrap_or(&run.started_at)
                        .replace('T', " "),
                    run.environment.as_deref().unwrap_or("-"),
                    run.count(CheckStatus::Healthy),
                    run.count(CheckStatus::Failing),
                    run.count(CheckStatus::Error)
                ))
                .weak(),
            );
        });
    }
}

fn empty_state(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.add_space(8.0);
    ui.label("No data checks yet.");
    ui.label(
        egui::RichText::new(
            "Add a check, let the AI propose one from a business rule, or start from the stock checks of the VOLTUNES audit.",
        )
        .weak(),
    );
    ui.horizontal(|ui| {
        if ui
            .button(format!(
                "{} Add stock checks (MySQL)",
                ic::ICON_ADD.codepoint
            ))
            .on_hover_text(
                "Duplicate receipts, balance vs transactions, orphan stock-outs, overdrawn stock-ins, negative balance",
            )
            .clicked()
        {
            actions.push(Action::AddStarters);
        }
        if ui
            .button(format!("{} New check", ic::ICON_ADD.codepoint))
            .clicked()
        {
            actions.push(Action::New);
        }
    });
}

fn checks_table(ui: &mut egui::Ui, st: &State, actions: &mut Vec<Action>) {
    egui::ScrollArea::vertical()
        .id_salt("dh_checks")
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new("dh_grid")
                .num_columns(6)
                .striped(true)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    for h in ["", "Check", "Severity", "Rows", "Trend", ""] {
                        ui.label(egui::RichText::new(h).small().strong());
                    }
                    ui.end_row();
                    for c in &st.set.checks {
                        let r = st
                            .last_run
                            .as_ref()
                            .and_then(|run| run.results.iter().find(|x| x.check_id == c.id));
                        match r {
                            Some(r) => {
                                ui.label(
                                    egui::RichText::new(status_icon(r.status))
                                        .color(status_color(ui.ctx(), r.status)),
                                )
                                .on_hover_text(
                                    r.error
                                        .clone()
                                        .unwrap_or_else(|| r.status.label().to_string()),
                                );
                            }
                            None => {
                                ui.label(egui::RichText::new("-").weak());
                            }
                        }
                        let mut name = egui::RichText::new(&c.name);
                        if !c.enabled {
                            name = name.strikethrough().weak();
                        }
                        ui.label(name).on_hover_text(if c.description.is_empty() {
                            c.sql.clone()
                        } else {
                            format!("{}\n\n{}", c.description, c.sql)
                        });
                        ui.label(c.severity.label());
                        let rows = match r.and_then(|r| r.row_count.map(|n| (n, r.truncated))) {
                            Some((n, true)) => format!("{n}+"),
                            Some((n, false)) => n.to_string(),
                            None => "-".into(),
                        };
                        ui.label(rows);
                        ui.label(r.and_then(|r| r.trend()).unwrap_or_else(|| "-".into()));
                        ui.horizontal(|ui| {
                            if ui
                                .small_button(ic::ICON_PLAY_ARROW.codepoint)
                                .on_hover_text("Run this check")
                                .clicked()
                            {
                                actions.push(Action::RunOne(c.id.clone()));
                            }
                            if ui
                                .small_button(ic::ICON_TABLE_VIEW.codepoint)
                                .on_hover_text("Open the query in a new tab and show the rows")
                                .clicked()
                            {
                                actions.push(Action::OpenGrid(c.id.clone()));
                            }
                            if ui
                                .small_button(ic::ICON_EDIT.codepoint)
                                .on_hover_text("Edit")
                                .clicked()
                            {
                                actions.push(Action::Edit(c.id.clone()));
                            }
                            if ui
                                .small_button(if c.enabled {
                                    ic::ICON_PAUSE.codepoint
                                } else {
                                    ic::ICON_PLAY_CIRCLE.codepoint
                                })
                                .on_hover_text(if c.enabled { "Disable" } else { "Enable" })
                                .clicked()
                            {
                                actions.push(Action::Toggle(c.id.clone()));
                            }
                            if ui
                                .small_button(ic::ICON_DELETE.codepoint)
                                .on_hover_text("Delete this check")
                                .clicked()
                            {
                                actions.push(Action::Delete(c.id.clone()));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
}

fn editor(ui: &mut egui::Ui, draft: &mut DataCheck, actions: &mut Vec<Action>) {
    ui.label(egui::RichText::new("Edit check").strong());
    egui::Grid::new("dh_edit")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(360.0));
            ui.end_row();
            ui.label("Rule");
            ui.add(
                egui::TextEdit::singleline(&mut draft.description)
                    .desired_width(360.0)
                    .hint_text("e.g. A delivery note may be received only once"),
            );
            ui.end_row();
            ui.label("Severity");
            egui::ComboBox::from_id_salt("dh_sev")
                .selected_text(draft.severity.label())
                .show_ui(ui, |ui| {
                    for s in Severity::ALL {
                        ui.selectable_value(&mut draft.severity, s, s.label());
                    }
                });
            ui.end_row();
            ui.label("Connection variable")
                .on_hover_text("Environment variable whose value is the connection name or id");
            ui.add(
                egui::TextEdit::singleline(&mut draft.connection_var)
                    .desired_width(160.0)
                    .font(egui::TextStyle::Monospace),
            );
            ui.end_row();
            ui.label("Tolerated rows");
            let mut tol = draft.max_rows.unwrap_or(0);
            if ui
                .add(egui::DragValue::new(&mut tol).range(0..=100_000))
                .changed()
            {
                draft.max_rows = (tol > 0).then_some(tol);
            }
            ui.end_row();
        });
    ui.add(
        egui::TextEdit::multiline(&mut draft.sql)
            .code_editor()
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .hint_text("SELECT ... rows that violate the rule ..."),
    );
    let problem = data_checks::validate(draft).err();
    if let Some(p) = &problem {
        ui.label(
            egui::RichText::new(p)
                .small()
                .color(style::theme_danger(ui.ctx())),
        );
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(problem.is_none(), egui::Button::new("Save"))
            .clicked()
        {
            actions.push(Action::SaveDraft);
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::CancelDraft);
        }
    });
    ui.separator();
}

fn ai_section(ui: &mut egui::Ui, st: &mut State, actions: &mut Vec<Action>) {
    if let Some(p) = &st.ai_proposal {
        ui.label(egui::RichText::new("Proposed by AI (not saved yet)").strong());
        ui.label(format!("{} ({})", p.name, p.severity.label()));
        if !p.description.is_empty() {
            ui.label(egui::RichText::new(&p.description).weak());
        }
        let mut sql = p.sql.clone();
        ui.add(
            egui::TextEdit::multiline(&mut sql)
                .code_editor()
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .interactive(false),
        );
        ui.horizontal(|ui| {
            if ui.button("Review and save").clicked() {
                actions.push(Action::AcceptAi);
            }
            if ui.button("Discard").clicked() {
                actions.push(Action::RejectAi);
            }
        });
        ui.separator();
        return;
    }
    if !st.show_ai {
        return;
    }
    ui.label(egui::RichText::new("Generate with AI").strong());
    ui.label(
        egui::RichText::new(
            "Describe the business rule; the AI reads the schema of the connection and proposes a SELECT that returns violating rows. You review it before it is saved.",
        )
        .weak()
        .small(),
    );
    ui.add(
        egui::TextEdit::multiline(&mut st.ai_rule)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("e.g. One delivery note may be received only once"),
    );
    ui.horizontal(|ui| {
        let busy = st.ai_job.is_some();
        if ui
            .add_enabled(
                !busy && !st.ai_rule.trim().is_empty(),
                egui::Button::new("Propose check"),
            )
            .clicked()
        {
            actions.push(Action::GenerateAi);
        }
        if busy {
            ui.spinner();
        }
    });
    ui.separator();
}

fn schedule_section(
    ui: &mut egui::Ui,
    st: &mut State,
    project: &crate::project::Project,
    actions: &mut Vec<Action>,
) {
    let sched = st.set.schedule.get_or_insert_with(Default::default);
    let before = sched.clone();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(ic::ICON_SCHEDULE.codepoint));
        ui.checkbox(&mut sched.enabled, "Run daily at")
            .on_hover_text(
                "Runs while Tabular is open; you get a notice when a healthy check starts failing",
            );
        ui.add(
            egui::TextEdit::singleline(&mut sched.daily_at)
                .desired_width(50.0)
                .hint_text("06:00"),
        );
        ui.label("on");
        egui::ComboBox::from_id_salt("dh_sched_env")
            .selected_text(
                sched
                    .environment
                    .clone()
                    .unwrap_or_else(|| "active environment".into()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut sched.environment, None, "active environment");
                for e in &project.environments {
                    ui.selectable_value(&mut sched.environment, Some(e.name.clone()), &e.name);
                }
            });
        if data_checks::parse_hhmm(&sched.daily_at).is_none() {
            ui.label(
                egui::RichText::new("use HH:MM")
                    .small()
                    .color(style::theme_danger(ui.ctx())),
            );
        }
    });
    if *sched != before && data_checks::parse_hhmm(&sched.daily_at).is_some() {
        actions.push(Action::SaveSchedule);
    }
}

fn save_set(t: &mut Tabular, st: &mut State) -> bool {
    match data_checks::save_set(&st.set) {
        Ok(()) => true,
        Err(e) => {
            t.toasts.error(e);
            // Jangan pertahankan perubahan yang gagal disimpan.
            if let Some(pid) = st.project_id.clone() {
                st.set = data_checks::load_set(&pid);
            }
            false
        }
    }
}

fn apply(t: &mut Tabular, st: &mut State, project: &crate::project::Project, a: Action) {
    match a {
        Action::RunAll => {
            st.run_job = Some(spawn_run(
                project.clone(),
                st.env.clone(),
                st.set.clone(),
                Vec::new(),
            ));
        }
        Action::RunOne(id) => {
            st.run_job = Some(spawn_run(
                project.clone(),
                st.env.clone(),
                st.set.clone(),
                vec![id],
            ));
        }
        Action::OpenGrid(id) => {
            if let Some(c) = st.set.check(&id).cloned() {
                open_in_grid(t, project, st.env.as_deref(), &c);
            }
        }
        Action::Edit(id) => st.editing = st.set.check(&id).cloned(),
        Action::New => st.editing = Some(DataCheck::new("New check")),
        Action::Delete(id) => {
            st.set.checks.retain(|c| c.id != id);
            save_set(t, st);
        }
        Action::Toggle(id) => {
            if let Some(c) = st.set.checks.iter_mut().find(|c| c.id == id) {
                c.enabled = !c.enabled;
            }
            save_set(t, st);
        }
        Action::SaveDraft => {
            if let Some(draft) = st.editing.take() {
                st.set.upsert(draft);
                if save_set(t, st) {
                    t.toasts.info("Data check saved".to_string());
                }
            }
        }
        Action::CancelDraft => st.editing = None,
        Action::AddStarters => {
            for c in data_checks::voltunes_stock_checks() {
                if st.set.check(&c.id).is_none() {
                    st.set.checks.push(c);
                }
            }
            if save_set(t, st) {
                t.toasts.info(
                    "Stock checks added. Set db_main in each environment to the connection name."
                        .to_string(),
                );
            }
        }
        Action::GenerateAi => start_ai(t, st, project),
        Action::AcceptAi => st.editing = st.ai_proposal.take(),
        Action::RejectAi => st.ai_proposal = None,
        Action::SaveSchedule => {
            save_set(t, st);
        }
    }
}

/// Usulan check dari AI: aturan bisnis + skema koneksi `db_main` (atau
/// variabel koneksi check pertama) di environment terpilih.
fn start_ai(t: &mut Tabular, st: &mut State, project: &crate::project::Project) {
    let target = t.effective_chat_target();
    if let Err(e) = crate::ai_assistant::backend_ready_for(t, target) {
        t.toasts.error(e);
        return;
    }
    let backend = crate::ai_assistant::chat_backend_for(t, target);
    let var = st
        .set
        .checks
        .first()
        .map(|c| c.connection_var.clone())
        .unwrap_or_else(|| "db_main".into());
    let env = st
        .env
        .clone()
        .or_else(|| project.active_environment().map(|e| e.name.clone()));
    let vars = env
        .as_deref()
        .map(|e| project.env_vars_with(e, crate::secrets::get_secret))
        .unwrap_or_default();
    let Some(conn_id) = vars.get(&var).and_then(|v| connection_for(t, v)) else {
        t.toasts.error(format!(
            "Set {{{{{var}}}}} in environment {} to a connection first; the AI needs its schema.",
            env.unwrap_or_default()
        ));
        return;
    };
    let dialect = t
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .map(|c| format!("{:?}", c.connection_type))
        .unwrap_or_else(|| "SQL".into());
    let rule = st.ai_rule.trim().to_string();
    st.ai_job = Some(spawn(move || {
        let schema = crate::runtime::block_on(async {
            let pool = crate::agent::core::open_cache_pool()
                .await
                .map_err(|e| e.to_string())?;
            let session = crate::agent::core::HeadlessSession::new(pool);
            session
                .describe_schema(conn_id, None, Some(&rule), Some(25))
                .await
                .map(|d| d.ddl)
                .map_err(|e| e.to_string())
        })
        .unwrap_or_else(|| Err("async runtime unavailable".into()))?;
        let (system, user) = data_checks::build_generate_prompts(&rule, &schema, &dialect);
        let (tx, _rx) = std::sync::mpsc::channel::<crate::repo_scan::RepoJobEvent<()>>();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let text = crate::repo_scan::ask_ai(
            &backend,
            system,
            user,
            crate::ai_assistant::ChatWorkspace::default(),
            &tx,
            &cancel,
        )
        .map_err(|e| e.to_string())?;
        data_checks::parse_generated(&text, &var)
    }));
}

/// Jadwal harian: diperiksa paling sering sekali per menit. Toast bila check
/// yang sebelumnya sehat menjadi gagal.
fn tick_scheduler(t: &mut Tabular, ctx: &egui::Context) {
    let mut sched: Scheduler = ctx.data_mut(|d| d.get_temp(scheduler_id()).unwrap_or_default());
    let mut finished: Vec<(String, Result<CheckRun, String>)> = Vec::new();
    sched.jobs.retain(
        |(pid, slot)| match slot.lock().ok().and_then(|mut g| g.take()) {
            Some(r) => {
                finished.push((pid.clone(), r));
                false
            }
            None => true,
        },
    );
    for (pid, res) in finished {
        let name = t
            .projects
            .list
            .iter()
            .find(|p| p.id == pid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| pid.clone());
        match res {
            Ok(run) => {
                let previous = data_checks::load_runs(&pid, 2).into_iter().nth(1);
                let reg = run.regressions(previous.as_ref());
                if !reg.is_empty() {
                    let names: Vec<&str> = reg.iter().map(|r| r.name.as_str()).collect();
                    t.toasts.error(format!(
                        "Data Health {name}: started failing: {}",
                        names.join(", ")
                    ));
                }
                log::info!("[DATA_CHECKS] scheduled run {name}: {}", run.verdict());
            }
            Err(e) => log::warn!("[DATA_CHECKS] scheduled run {name} failed: {e}"),
        }
    }
    let due_check = sched
        .last_tick
        .is_none_or(|t0| t0.elapsed() >= std::time::Duration::from_secs(60));
    if due_check {
        sched.last_tick = Some(std::time::Instant::now());
        let now = chrono::Local::now().naive_local();
        for p in &t.projects.list {
            if sched.jobs.iter().any(|(pid, _)| pid == &p.id) {
                continue;
            }
            let set = data_checks::load_set(&p.id);
            let Some(schedule) = set.schedule.clone().filter(|s| s.enabled) else {
                continue;
            };
            if set.checks.is_empty() {
                continue;
            }
            let last = data_checks::load_runs(&p.id, 1)
                .first()
                .and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.started_at).ok())
                .map(|d| d.with_timezone(&chrono::Local).naive_local());
            if data_checks::schedule_due(&schedule, now, last) {
                log::info!("[DATA_CHECKS] scheduled run for {}", p.name);
                let slot = spawn_run(p.clone(), schedule.environment.clone(), set, Vec::new());
                sched.jobs.push((p.id.clone(), slot));
            }
        }
    }
    if !sched.jobs.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    ctx.data_mut(|d| d.insert_temp(scheduler_id(), sched));
}
