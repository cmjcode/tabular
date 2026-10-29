//! Tab Integration Testing: editor suite langkah HTTP berurutan milik project.
//!
//! - Kolom kiri: group (mis. Purchasing, Plant), variabel.
//! - Tengah: tabel langkah dalam urutan eksekusi (Setup → Test per group →
//!   Cleanup) beserta aliran data antar langkah, hasil run, dan lint.
//! - Kanan: detail langkah (extract, assertion, hasil terakhir).
//!
//! Klik dua kali (atau tombol buka) membuka langkah di tab HTTP client; simpan
//! di sana menulis balik ke suite. Run berjalan di
//! [`super::integration_test_jobs`]; cleanup selalu dijalankan setelahnya.

use std::collections::HashMap;

use eframe::egui;
use egui_icons::icons as ic;

use super::integration_test_jobs as jobs;
use super::{Tabular, style};
use crate::http_collection::SavedRequest;
use crate::http_test_report::{self as report, ReportFormat, ReportInput, Verdict};
use crate::http_tests::{
    self, AssertOp, Assertion, Extract, HttpTestStep, HttpTestSuite, LintLevel, RunScope,
    StepGenMode, StepOutcome, StepPhase, StepResult, TestRun,
};
use crate::models::structs::HttpMethod;

/// Jumlah run riwayat yang dimuat di tab.
const HISTORY_LIMIT: usize = 20;
const LEFT_WIDTH: f32 = 210.0;
const RIGHT_WIDTH: f32 = 380.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Steps,
    Report,
    History,
}

/// Filter group di kolom kiri.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum GroupFilter {
    #[default]
    All,
    /// `""` = langkah tanpa group.
    Group(String),
}

/// State tab (disimpan di `QueryTab::integration_test_state`).
#[derive(Debug, Clone, Default)]
pub struct IntegrationTestTabState {
    suite: HttpTestSuite,
    /// `http_tests::suites_revision()` saat suite terakhir dimuat/disimpan.
    rev: u64,
    dirty: bool,
    selected: Option<String>,
    filter: GroupFilter,
    view: View,
    last_run: Option<TestRun>,
    history: Vec<TestRun>,
    runs_rev: Option<u64>,
    ai_open: bool,
    ai_prompt: String,
    new_group: String,
    renaming: Option<(String, String)>,
    /// Group yang akan dijalankan, menunggu konfirmasi (kosong = semua).
    confirm_run: Option<Vec<String>>,
    allow_production: bool,
    confirm_delete_suite: bool,
    picker: Option<String>,
    /// Hasil validasi terakhir yang masih perlu perhatian (langkah gagal).
    validation_note: Option<Vec<String>>,
    /// Dialog "Fix API code with AI": langkah terpilih + catatan.
    codefix_dialog: Option<(Vec<String>, String)>,
    /// Daftar hasil pemeriksaan (lint) di bawah tabel sedang dibuka.
    checks_open: bool,
}

impl IntegrationTestTabState {
    fn new(suite: HttpTestSuite) -> Self {
        Self {
            rev: http_tests::suites_revision(),
            suite,
            ..Default::default()
        }
    }

    pub fn suite_id(&self) -> &str {
        &self.suite.id
    }

    fn title(&self) -> String {
        format!("Tests: {}", self.suite.name.trim())
    }

    fn mark(&mut self) {
        self.dirty = true;
        self.suite.touch();
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected
            .as_deref()
            .and_then(|id| self.suite.step_index(id))
    }
}

// ─── Sidebar ────────────────────────────────────────────────────────────────

/// Sembunyikan sidebar saat tab Integration Testing menjadi aktif, supaya
/// tabel langkah mendapat ruang penuh. User tetap bisa memunculkannya lagi
/// lewat tombol sidebar; pilihan itu dihormati selama tab yang sama aktif.
/// Saat pindah ke tab lain, visibilitas sebelumnya dikembalikan (kecuali user
/// sudah memunculkan sidebar sendiri). Dipanggil setiap frame sebelum sidebar
/// dirender.
pub fn sync_sidebar(t: &mut Tabular) {
    let active = t
        .query_tabs
        .get(t.active_tab_index)
        .filter(|tab| tab.integration_test_state.is_some())
        .map(|tab| tab.id);
    match (active, t.integration_tests.sidebar_auto) {
        // Masih tab yang sama: jangan ganggu toggle user.
        (Some(id), Some((auto_id, _))) if id == auto_id => {}
        // Masuk ke tab Integration Testing (dari tab lain atau tab IT lain).
        (Some(id), prev) => {
            let restore = prev.map(|(_, r)| r).unwrap_or(t.sidebar_visible);
            t.sidebar_visible = false;
            t.integration_tests.sidebar_auto = Some((id, restore));
        }
        // Keluar dari tab Integration Testing.
        (None, Some((_, restore))) => {
            if !t.sidebar_visible {
                t.sidebar_visible = restore;
            }
            t.integration_tests.sidebar_auto = None;
        }
        (None, None) => {}
    }
}

// ─── Membuka tab ────────────────────────────────────────────────────────────

fn open_tab(t: &mut Tabular, suite: HttpTestSuite) {
    let st = IntegrationTestTabState::new(suite);
    crate::editor::create_new_tab(t, st.title(), String::new());
    if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
        tab.integration_test_state = Some(st);
    }
}

/// Buka suite; tab yang sudah membukanya diaktifkan saja.
pub fn open_suite(t: &mut Tabular, suite_id: &str) {
    if let Some(idx) = t.query_tabs.iter().position(|tab| {
        tab.integration_test_state
            .as_ref()
            .is_some_and(|s| s.suite.id == suite_id)
    }) {
        crate::editor::switch_to_tab(t, idx);
        return;
    }
    match http_tests::load_suite(suite_id) {
        Some(s) => open_tab(t, s),
        None => t
            .toasts
            .warning("The integration test suite no longer exists"),
    }
}

/// Tombol header: buka suite project aktif, atau buat suite kosong bila belum ada.
pub fn open_for_active_project(t: &mut Tabular) {
    super::project_ui::ensure_loaded(t);
    let project_id = super::project_ui::active(t).map(|p| p.id.clone());
    if let Some(idx) = t.query_tabs.iter().position(|tab| {
        tab.integration_test_state
            .as_ref()
            .is_some_and(|s| s.suite.project_id == project_id)
    }) {
        crate::editor::switch_to_tab(t, idx);
        return;
    }
    let mut suites = match &project_id {
        Some(id) => http_tests::load_project_suites(id),
        None => http_tests::load_suites()
            .into_iter()
            .filter(|s| s.project_id.is_none())
            .collect(),
    };
    suites.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    if let Some(first) = suites.into_iter().next() {
        open_tab(t, first);
        return;
    }
    let suite = HttpTestSuite::new("Integration tests", project_id);
    if let Err(e) = http_tests::save_suite(&suite) {
        t.toasts.error(e);
    }
    open_tab(t, suite);
}

// ─── Snapshot keadaan aplikasi untuk satu frame ─────────────────────────────

/// Data dari `Tabular` yang dibutuhkan render (tanpa memegang pinjaman).
struct Live {
    running: bool,
    cancelling: bool,
    current: Option<usize>,
    progress: String,
    /// Hasil run berjalan (bila ada) atau run terakhir, per id langkah.
    results: HashMap<String, StepResult>,
    run_scope: Option<Vec<bool>>,
    gen_status: Option<String>,
    suite_gen_status: Option<String>,
    ai_ready: Result<String, String>,
    project_name: Option<String>,
    env_name: Option<String>,
    env_production: bool,
    known_vars: std::collections::HashSet<String>,
    suites: Vec<(String, String)>,
    catalogs: Vec<http_tests::FolderCatalog>,
    /// Status validasi yang sedang berjalan.
    validating: Option<String>,
    auto_validate: bool,
    codefix_status: Option<String>,
    proposal: Option<crate::http_test_codefix::CodeFixProposal>,
    /// Cek build: (masih berjalan, hasil).
    check: Option<(bool, Option<Result<String, String>>)>,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            running: false,
            cancelling: false,
            current: None,
            progress: String::new(),
            results: HashMap::new(),
            run_scope: None,
            gen_status: None,
            suite_gen_status: None,
            ai_ready: Err(String::new()),
            project_name: None,
            env_name: None,
            env_production: false,
            known_vars: Default::default(),
            suites: Vec::new(),
            catalogs: Vec::new(),
            validating: None,
            auto_validate: true,
            codefix_status: None,
            proposal: None,
            check: None,
        }
    }
}

fn snapshot(t: &mut Tabular, st: &IntegrationTestTabState, need_catalog: bool) -> Live {
    let mut live = Live::default();
    if let Some(job) = t.integration_tests.runs.get(&st.suite.id)
        && job.is_running()
    {
        live.running = true;
        live.cancelling = job.is_cancelling();
        live.current = job.current;
        live.progress = job.progress_text();
        live.results = job.results.clone();
        live.run_scope = job.scope.include.clone();
    } else if let Some(run) = &st.last_run {
        live.results = run
            .results
            .iter()
            .map(|r| (r.step_id.clone(), r.clone()))
            .collect();
    }
    live.gen_status = t
        .integration_tests
        .gens
        .get(&st.suite.id)
        .map(|g| format!("{} ({}s)", g.status, g.started.elapsed().as_secs()));
    live.suite_gen_status = t
        .integration_tests
        .suite_gen
        .as_ref()
        .map(|g| format!("{} ({}s)", g.status, g.started.elapsed().as_secs()));
    let target = t.effective_chat_target();
    live.ai_ready = crate::ai_assistant::backend_ready_for(t, target)
        .map(|()| crate::ai_assistant::backend_label_for(t, target));
    if let Some(p) = jobs::suite_project(t, &st.suite) {
        live.project_name = Some(p.name.clone());
        if let Some(env) = p.active_environment() {
            live.env_name = Some(env.name.clone());
            live.env_production =
                env.environment() == Some(crate::connection_env::Environment::Production);
        }
    }
    live.known_vars = jobs::project_var_names(t, &st.suite);
    live.validating = t
        .integration_tests
        .validations
        .get(&st.suite.id)
        .filter(|j| j.is_running())
        .map(|j| format!("{} ({}s)", j.status, j.started.elapsed().as_secs()));
    live.auto_validate = !t.integration_tests.skip_auto_validate;
    live.codefix_status = t
        .integration_tests
        .codefixes
        .get(&st.suite.id)
        .filter(|j| j.is_running())
        .map(|j| format!("{} ({}s)", j.status, j.started.elapsed().as_secs()));
    live.proposal = t.integration_tests.proposals.get(&st.suite.id).cloned();
    live.check = t
        .integration_tests
        .checks
        .get(&st.suite.id)
        .map(|c| (c.result.is_none(), c.result.clone()));
    let mut suites: Vec<HttpTestSuite> = match &st.suite.project_id {
        Some(id) => http_tests::load_project_suites(id),
        None => http_tests::load_suites()
            .into_iter()
            .filter(|s| s.project_id.is_none())
            .collect(),
    };
    suites.sort_by_key(|s| s.name.to_lowercase());
    live.suites = suites.into_iter().map(|s| (s.id, s.name)).collect();
    if need_catalog {
        live.catalogs = jobs::project_catalogs(t, &st.suite);
    }
    live
}

/// Aksi yang butuh `&mut Tabular`, dijalankan setelah render.
enum Act {
    /// Group terpilih, dan apakah user mengizinkan Production.
    Run(Vec<String>, bool),
    Stop,
    Abort,
    OpenStep(String),
    GenSteps(StepGenMode),
    GenSuite,
    CancelGen,
    NewSuite,
    SwitchSuite(String),
    DeleteSuite,
    DuplicateSuite,
    OpenHtmlReport,
    Export(ReportFormat),
    CopyMarkdown,
    Validate,
    CancelValidate,
    SetAutoValidate(bool),
    CodeFix(Vec<String>, String),
    CancelCodeFix,
    Proposal(jobs::ProposalAction),
}

// ─── Render ─────────────────────────────────────────────────────────────────

pub fn render_active_tab(t: &mut Tabular, ui: &mut egui::Ui) {
    let idx = t.active_tab_index;
    let Some(mut st) = t
        .query_tabs
        .get_mut(idx)
        .and_then(|tab| tab.integration_test_state.take())
    else {
        return;
    };
    sync_from_app(t, &mut st);
    // Katalog dibaca setiap frame hanya bila dibutuhkan (picker / laporan).
    let need_catalog = st.picker.is_some() || st.view == View::Report;
    let live = snapshot(t, &st, need_catalog);
    let mut acts: Vec<Act> = Vec::new();

    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 8,
            bottom: 6,
        })
        .show(ui, |ui| {
            toolbar(ui, &mut st, &live, &mut acts);
            if st.ai_open {
                ui.add_space(6.0);
                ai_bar(ui, &mut st, &live, &mut acts);
            }
            status_banners(ui, &mut st, &live, &mut acts);
        });
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        egui::Stroke::new(1.0, style::nav_border(ui.ctx())),
    );
    ui.add_space(4.0);

    match st.view {
        // Suite dikunci selama validasi: hasilnya akan menggantikan isi suite.
        View::Steps => {
            ui.add_enabled_ui(live.validating.is_none(), |ui| {
                body(ui, &mut st, &live, &mut acts)
            });
        }
        View::Report => report_view(ui, &mut st, &live, &mut acts),
        View::History => history_view(ui, &mut st),
    }

    confirm_run_modal(ui.ctx(), &mut st, &live, &mut acts);
    confirm_delete_modal(ui.ctx(), &mut st, &mut acts);
    picker_window(ui.ctx(), &mut st, &live);
    codefix_dialog(ui.ctx(), &mut st, &mut acts);
    proposal_window(ui.ctx(), &live, &mut acts);

    // Simpan otomatis saat user tidak sedang mengetik.
    if st.dirty && !ui.ctx().egui_wants_keyboard_input() {
        save(t, &mut st);
    }

    let title = st.title();
    let suite_snapshot = st.suite.clone();
    let last_run = st.last_run.clone();
    if let Some(tab) = t.query_tabs.get_mut(idx) {
        tab.title = title;
        tab.integration_test_state = Some(st);
    }
    let ctx = ui.ctx().clone();
    for act in acts {
        apply(t, &ctx, idx, &suite_snapshot, last_run.as_ref(), act);
    }
}

/// Muat ulang suite/riwayat dan ambil hasil AI yang tertunda.
fn sync_from_app(t: &mut Tabular, st: &mut IntegrationTestTabState) {
    let rev = http_tests::suites_revision();
    if rev != st.rev && !st.dirty {
        if let Some(s) = http_tests::load_suite(&st.suite.id) {
            st.suite = s;
        }
        st.rev = rev;
    }
    if let Some(result) = t.integration_tests.gen_results.remove(&st.suite.id) {
        match result {
            Ok((steps, vars)) => {
                let n = http_tests::merge_generated_steps(&mut st.suite, steps, vars);
                st.dirty = true;
                t.toasts.success(format!("Added {n} step(s) from AI"));
                // Pastikan langkah baru benar-benar jalan di server.
                st.suite.ensure_ids();
                st.suite.normalize_order();
                jobs::auto_validate(t, &st.suite);
            }
            Err(e) => t.toasts.error(format!("AI step generation failed: {e}")),
        }
    }
    if let Some(result) = t.integration_tests.validate_results.remove(&st.suite.id) {
        match result {
            Ok(o) => {
                st.suite = o.suite;
                st.dirty = true;
                st.validation_note = (!o.remaining.is_empty()).then_some(o.remaining);
            }
            Err(e) => st.validation_note = Some(vec![e]),
        }
    }
    if st.runs_rev != Some(t.integration_tests.runs_rev) {
        st.runs_rev = Some(t.integration_tests.runs_rev);
        st.history = http_tests::load_runs(&st.suite.id, HISTORY_LIMIT);
        st.last_run = st.history.first().cloned();
    }
}

fn save(t: &mut Tabular, st: &mut IntegrationTestTabState) {
    st.suite.ensure_ids();
    st.suite.normalize_order();
    if let Err(e) = http_tests::save_suite(&st.suite) {
        t.toasts.error(e);
    }
    st.rev = http_tests::suites_revision();
    st.dirty = false;
}

fn weak(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(
        egui::RichText::new(text.into())
            .small()
            .color(ui.visuals().weak_text_color()),
    );
}

fn icon_button(ui: &mut egui::Ui, glyph: &str, tip: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(glyph).size(14.0)).frame(false))
        .on_hover_text(tip)
}

fn outcome_color(ctx: &egui::Context, o: StepOutcome) -> egui::Color32 {
    match o {
        StepOutcome::Passed => style::theme_success(ctx),
        StepOutcome::Failed | StepOutcome::Error => style::theme_danger(ctx),
        StepOutcome::Blocked => style::theme_warning(ctx),
        StepOutcome::Skipped => style::theme_muted_text(ctx),
    }
}

fn outcome_glyph(o: StepOutcome) -> &'static str {
    match o {
        StepOutcome::Passed => ic::ICON_CHECK_CIRCLE.codepoint,
        StepOutcome::Failed => ic::ICON_CANCEL.codepoint,
        StepOutcome::Error => ic::ICON_ERROR.codepoint,
        StepOutcome::Blocked => ic::ICON_BLOCK.codepoint,
        StepOutcome::Skipped => ic::ICON_SKIP_NEXT.codepoint,
    }
}

fn verdict_color(ctx: &egui::Context, v: Verdict) -> egui::Color32 {
    match v {
        Verdict::Pass => style::theme_success(ctx),
        Verdict::Fail => style::theme_danger(ctx),
        _ => style::theme_warning(ctx),
    }
}

fn group_label(g: &str) -> &str {
    if g.trim().is_empty() { "No group" } else { g }
}

/// Judul bagian: ikon + judul, keterangan singkat opsional, lalu garis tipis.
fn section_header(ui: &mut egui::Ui, icon: &str, title: &str, hint: Option<&str>) {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(icon)
                .size(15.0)
                .color(style::theme_muted_text(ui.ctx())),
        );
        ui.label(egui::RichText::new(title).strong().size(13.5));
    });
    if let Some(h) = hint {
        weak(ui, h);
    }
    ui.add_space(3.0);
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        egui::Stroke::new(1.0, style::nav_border(ui.ctx())),
    );
    ui.add_space(6.0);
}

/// Ikon bantuan kecil dengan penjelasan saat di-hover.
fn help_icon(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(ic::ICON_HELP_OUTLINE.codepoint)
            .size(13.0)
            .color(ui.visuals().weak_text_color()),
    )
    .on_hover_text(text);
}

/// Label status langkah: ikon + nama status dalam warnanya.
fn outcome_badge(ui: &mut egui::Ui, o: StepOutcome, size: f32) -> egui::Response {
    ui.label(
        egui::RichText::new(format!("{} {}", outcome_glyph(o), o.label()))
            .size(size)
            .strong()
            .color(outcome_color(ui.ctx(), o)),
    )
}

fn phase_color(ctx: &egui::Context, p: StepPhase) -> egui::Color32 {
    match p {
        StepPhase::Setup => style::theme_info(ctx),
        StepPhase::Test => style::theme_muted_text(ctx),
        StepPhase::Cleanup => style::theme_warning(ctx),
    }
}

/// Judul blok langkah di tabel dan laporan, mis. "Tests · Purchasing".
fn block_title(phase: StepPhase, group: &str) -> String {
    let g = group.trim();
    match phase {
        StepPhase::Setup if g.is_empty() => "Setup".into(),
        StepPhase::Setup => format!("Setup · {g}"),
        StepPhase::Test if g.is_empty() => "Tests".into(),
        StepPhase::Test => format!("Tests · {g}"),
        StepPhase::Cleanup => "Cleanup".into(),
    }
}

fn block_hint(phase: StepPhase) -> &'static str {
    match phase {
        StepPhase::Setup => "Runs first: login and the data the tests need",
        StepPhase::Test => "The checks themselves, in this order",
        StepPhase::Cleanup => "Runs last and removes the test data, even when tests fail",
    }
}

/// Kunci blok: fase + group (cleanup selalu satu blok).
fn block_key(phase: StepPhase, group: &str) -> (StepPhase, String) {
    let g = if phase == StepPhase::Cleanup {
        String::new()
    } else {
        group.trim().to_string()
    };
    (phase, g)
}

/// Baris kunci-nilai di dalam `egui::Grid` dua kolom.
fn kv_row(ui: &mut egui::Ui, key: &str, value: impl Into<String>) {
    ui.label(
        egui::RichText::new(key)
            .small()
            .color(ui.visuals().weak_text_color()),
    );
    ui.label(egui::RichText::new(value.into()).small());
    ui.end_row();
}

/// Kalimat satu baris yang menjelaskan hasil run untuk manusia.
fn run_sentence(run: &TestRun, s: &report::RunStats) -> String {
    let t = &s.tests;
    let mut out = if run.preflight_error.is_some() {
        "The run did not start because the server could not be reached.".to_string()
    } else {
        format!(
            "{} of {} test step(s) passed ({:.1}%). The pass criterion is {:.0}%.",
            t.passed,
            t.planned - t.skipped,
            t.pass_rate(),
            s.threshold
        )
    };
    if t.blocked > 0 {
        out.push_str(&format!(
            " {} step(s) were not sent because an earlier step failed.",
            t.blocked
        ));
    }
    if run.cancelled || run.aborted {
        out.push_str(" The run was stopped before it finished.");
    }
    out.push_str(if s.data_clean() {
        " Test data was cleaned up."
    } else {
        " Some test data was left behind."
    });
    out
}

// ─── Toolbar ────────────────────────────────────────────────────────────────

fn toolbar(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, live: &Live, acts: &mut Vec<Act>) {
    // Baris 1: identitas suite + pilihan tampilan.
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(ic::ICON_FACT_CHECK.codepoint)
                .size(18.0)
                .color(style::theme_accent(ui.ctx())),
        );
        egui::ComboBox::from_id_salt(("it_suite_pick", st.suite.id.as_str()))
            .selected_text(egui::RichText::new(st.suite.name.clone()).strong())
            .width(220.0)
            .show_ui(ui, |ui| {
                for (id, name) in &live.suites {
                    if ui
                        .selectable_label(*id == st.suite.id, name.as_str())
                        .clicked()
                        && *id != st.suite.id
                    {
                        acts.push(Act::SwitchSuite(id.clone()));
                    }
                }
                ui.separator();
                if ui
                    .button(format!("{} New empty suite", ic::ICON_ADD.codepoint))
                    .clicked()
                {
                    acts.push(Act::NewSuite);
                }
                if ui
                    .button(format!(
                        "{} Duplicate this suite",
                        ic::ICON_CONTENT_COPY.codepoint
                    ))
                    .clicked()
                {
                    acts.push(Act::DuplicateSuite);
                }
            })
            .response
            .on_hover_text("Switch to another suite of this project");
        if ui
            .add(
                egui::TextEdit::singleline(&mut st.suite.name)
                    .desired_width(180.0)
                    .hint_text("Suite name"),
            )
            .on_hover_text("Rename this suite")
            .changed()
        {
            st.mark();
        }
        if let Some(p) = &live.project_name {
            let env = live
                .env_name
                .clone()
                .unwrap_or_else(|| "no environment".into());
            let (color, tip) = if live.env_production {
                (
                    style::theme_danger(ui.ctx()),
                    "Runs against PRODUCTION. Test data is created and removed there.",
                )
            } else {
                (
                    style::theme_muted_text(ui.ctx()),
                    "Project and the environment the steps run against",
                )
            };
            ui.label(
                egui::RichText::new(format!("{p} · {env}"))
                    .small()
                    .color(color),
            )
            .on_hover_text(tip);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let failed = st
                .last_run
                .as_ref()
                .map(|r| {
                    r.results
                        .iter()
                        .filter(|x| {
                            matches!(
                                x.outcome(),
                                StepOutcome::Failed | StepOutcome::Error | StepOutcome::Blocked
                            )
                        })
                        .count()
                })
                .unwrap_or(0);
            let report_label = if failed > 0 {
                format!("Report ({failed})")
            } else {
                "Report".to_string()
            };
            let segments = [
                style::NavSegment {
                    key: "steps",
                    icon: ic::ICON_LIST.codepoint,
                    label: "Steps",
                },
                style::NavSegment {
                    key: "report",
                    icon: ic::ICON_ASSESSMENT.codepoint,
                    label: report_label.as_str(),
                },
                style::NavSegment {
                    key: "history",
                    icon: ic::ICON_HISTORY.codepoint,
                    label: "History",
                },
            ];
            let current = match st.view {
                View::Steps => "steps",
                View::Report => "report",
                View::History => "history",
            };
            let picked = ui
                .allocate_ui_with_layout(
                    egui::vec2(340.0, 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| style::render_segmented_nav(ui, "it_view", &segments, current, 30.0),
                )
                .inner;
            match picked {
                Some("steps") => st.view = View::Steps,
                Some("report") => st.view = View::Report,
                Some("history") => st.view = View::History,
                _ => {}
            }
        });
    });

    // Baris 2: aksi utama di kiri, alat bantu di kanan.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        run_buttons(ui, st, live, acts);
        ui.add_space(8.0);
        validation_badge(ui, st, live, acts);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            more_menu(ui, st, acts);
            if ui
                .selectable_label(
                    st.ai_open,
                    format!("{} Generate with AI", ic::ICON_AUTO_AWESOME.codepoint),
                )
                .on_hover_text(
                    "Describe a flow and let AI create the steps, cleanup or whole suites",
                )
                .clicked()
            {
                st.ai_open = !st.ai_open;
            }
        });
    });
}

/// Penanda validasi + tombol "Validate & fix".
fn validation_badge(
    ui: &mut egui::Ui,
    st: &IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    if live.validating.is_some() || live.running {
        return;
    }
    let ready = live.ai_ready.is_ok() && st.suite.steps.iter().any(|s| s.enabled);
    if ui
        .add_enabled(
            ready,
            egui::Button::new(format!("{} Validate & fix", ic::ICON_AUTO_AWESOME.codepoint)),
        )
        .on_hover_text(
            "Run the suite against the server, then let AI fix the steps that fail and run again (up to 3 rounds). Test data is cleaned up.",
        )
        .on_disabled_hover_text("Needs an AI backend and at least one enabled step")
        .clicked()
    {
        acts.push(Act::Validate);
    }
    let (text, color, tip) = match (st.suite.validation_current(), &st.suite.validation) {
        (Some(v), _) if v.all_passed() => (
            format!("{} Validated", ic::ICON_CHECK_CIRCLE.codepoint),
            style::theme_success(ui.ctx()),
            format!(
                "{}/{} steps passed on {} ({})",
                v.passed,
                v.total,
                v.environment.clone().unwrap_or_default(),
                v.at
            ),
        ),
        (Some(v), _) => (
            format!(
                "{} {}/{} pass",
                ic::ICON_WARNING.codepoint,
                v.passed,
                v.total
            ),
            style::theme_warning(ui.ctx()),
            format!(
                "Validated {} on {}",
                v.at,
                v.environment.clone().unwrap_or_default()
            ),
        ),
        (None, Some(_)) => (
            format!("{} Changed since validation", ic::ICON_INFO.codepoint),
            style::theme_muted_text(ui.ctx()),
            "Edit made after the last validation; validate again to be sure it runs".to_string(),
        ),
        (None, None) => (
            "Not validated".to_string(),
            style::theme_muted_text(ui.ctx()),
            "This suite has not been run against the server yet".to_string(),
        ),
    };
    ui.label(egui::RichText::new(text).small().color(color))
        .on_hover_text(tip);
}

/// Banner: validasi berjalan, hasil validasi yang perlu perhatian, server mati.
fn status_banners(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    if let Some(status) = &live.validating {
        ui.add_space(6.0);
        style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(egui::RichText::new("Validating with AI").strong());
                ui.label(egui::RichText::new(status).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Cancel").clicked() {
                        acts.push(Act::CancelValidate);
                    }
                });
            });
            weak(
                ui,
                "Steps are sent to the server, failing steps are fixed from the server's response and the handler code, then run again. The suite is locked until it finishes.",
            );
        });
    }
    if let Some(status) = &live.codefix_status {
        ui.add_space(6.0);
        style::theme_alert_frame(ui.ctx(), false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(egui::RichText::new("AI is preparing a code fix").strong());
                ui.label(egui::RichText::new(status).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Cancel").clicked() {
                        acts.push(Act::CancelCodeFix);
                    }
                });
            });
            weak(
                ui,
                "The change is made in a separate git branch. Your files are not touched until you apply it.",
            );
        });
    }
    if let Some(e) = st
        .last_run
        .as_ref()
        .and_then(|r| r.preflight_error.clone())
        .filter(|_| live.validating.is_none() && !live.running)
    {
        ui.add_space(6.0);
        style::theme_alert_frame(ui.ctx(), true).show(ui, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} Last run did not start",
                    ic::ICON_ERROR.codepoint
                ))
                .strong()
                .color(style::theme_danger(ui.ctx())),
            );
            ui.label(egui::RichText::new(e).small());
        });
    }
    let mut dismiss = false;
    let mut open_fix: Option<Vec<String>> = None;
    if let Some(lines) = &st.validation_note
        && live.validating.is_none()
    {
        ui.add_space(6.0);
        style::theme_alert_frame(ui.ctx(), true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} Validation: {} item(s) still need attention",
                        ic::ICON_WARNING.codepoint,
                        lines.len()
                    ))
                    .strong()
                    .color(style::theme_warning(ui.ctx())),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Dismiss").clicked() {
                        dismiss = true;
                    }
                });
            });
            for l in lines.iter().take(8) {
                weak(ui, crate::repo_scan::truncate_chars(l, 220));
            }
            weak(
                ui,
                "These may be real API bugs, or need data or variables the AI could not create. Open the step to check it, then Validate & fix again.",
            );
            let ids = failed_step_ids(st);
            if !ids.is_empty()
                && live.codefix_status.is_none()
                && live.proposal.is_none()
                && ui
                    .button(format!(
                        "{} Fix API code for {} failed step(s)…",
                        ic::ICON_AUTO_AWESOME.codepoint,
                        ids.len()
                    ))
                    .clicked()
            {
                open_fix = Some(ids);
            }
        });
    }
    if dismiss {
        st.validation_note = None;
    }
    if let Some(ids) = open_fix {
        st.codefix_dialog = Some((ids, String::new()));
    }
}

fn run_buttons(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    if live.validating.is_some() {
        return;
    }
    if live.running {
        ui.add(egui::Spinner::new().size(14.0));
        ui.label(egui::RichText::new(&live.progress).small());
        if !live.cancelling
            && ui
                .button(format!("{} Stop", ic::ICON_STOP.codepoint))
                .on_hover_text("Stop the remaining tests. Cleanup still runs.")
                .clicked()
        {
            acts.push(Act::Stop);
        }
        if ui
            .button(format!("{} Abort", ic::ICON_CLOSE.codepoint))
            .on_hover_text("Stop immediately, including cleanup. Test data may be left behind.")
            .clicked()
        {
            acts.push(Act::Abort);
        }
        return;
    }
    let enabled = st.suite.steps.iter().any(|s| s.enabled);
    if let GroupFilter::Group(g) = &st.filter {
        let g = g.clone();
        let label = format!("{} Run {}", ic::ICON_PLAY_ARROW.codepoint, group_label(&g));
        if ui
            .add_enabled(enabled, crate::diagram_repo::accent_button(ui, label))
            .on_hover_text("Run this group plus the steps it depends on, then clean up")
            .on_disabled_hover_text("Add or enable at least one step first")
            .clicked()
        {
            st.confirm_run = Some(vec![g]);
        }
        if ui
            .add_enabled(enabled, egui::Button::new("Run all groups"))
            .on_hover_text("Run every step of the suite in order, then clean up")
            .clicked()
        {
            st.confirm_run = Some(Vec::new());
        }
    } else if ui
        .add_enabled(
            enabled,
            crate::diagram_repo::accent_button(
                ui,
                format!("{} Run all", ic::ICON_PLAY_ARROW.codepoint),
            ),
        )
        .on_hover_text("Run every step in order, then clean up the test data")
        .on_disabled_hover_text("Add or enable at least one step first")
        .clicked()
    {
        st.confirm_run = Some(Vec::new());
    }
}

fn more_menu(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, acts: &mut Vec<Act>) {
    ui.menu_button(format!("{} Settings", ic::ICON_SETTINGS.codepoint), |ui| {
        if ui
            .button(format!(
                "{} Auto-order cleanup steps",
                ic::ICON_SORT.codepoint
            ))
            .on_hover_text("Remove data in reverse order of creation")
            .clicked()
        {
            http_tests::auto_order_cleanup(&mut st.suite);
            st.mark();
            ui.close();
        }
        if ui
            .checkbox(&mut st.suite.stop_on_failure, "Stop after first failure")
            .on_hover_text("Remaining tests are blocked after a failure. Cleanup still runs.")
            .changed()
        {
            st.mark();
        }
        ui.horizontal(|ui| {
            ui.label("Pass criterion");
            let mut v = st.suite.pass_threshold();
            if ui
                .add(
                    egui::DragValue::new(&mut v)
                        .range(0.0..=100.0)
                        .speed(1.0)
                        .suffix(" %"),
                )
                .changed()
            {
                st.suite.min_pass_rate = (v < 100.0).then_some(v);
                st.mark();
            }
        });
        ui.separator();
        if ui
            .button(format!(
                "{} Copy report as Markdown",
                ic::ICON_CONTENT_COPY.codepoint
            ))
            .clicked()
        {
            acts.push(Act::CopyMarkdown);
            ui.close();
        }
        if ui
            .button(format!("{} Delete suite…", ic::ICON_DELETE.codepoint))
            .clicked()
        {
            st.confirm_delete_suite = true;
            ui.close();
        }
    });
}

fn ai_bar(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, live: &Live, acts: &mut Vec<Act>) {
    style::theme_card_frame(ui.ctx()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ic::ICON_AUTO_AWESOME.codepoint)
                    .color(style::theme_accent(ui.ctx())),
            );
            match &live.ai_ready {
                Ok(label) => weak(ui, format!("AI: {label}")),
                Err(e) => {
                    ui.label(
                        egui::RichText::new(format!("AI not ready: {e}"))
                            .small()
                            .color(style::theme_warning(ui.ctx())),
                    );
                }
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut st.ai_prompt)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text(
                    "Describe the flow or a group, e.g. \"Purchasing: create vendor, create PO, approve, receive goods\". \
                     The AI uses the project's API endpoints, chains ids between steps and adds cleanup.",
                ),
        );
        ui.horizontal(|ui| {
            let ready = live.ai_ready.is_ok();
            if let Some(status) = live.gen_status.as_ref().or(live.suite_gen_status.as_ref()) {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(egui::RichText::new(status).small());
                if ui.small_button("Cancel").clicked() {
                    acts.push(Act::CancelGen);
                }
                return;
            }
            if ui
                .add_enabled(ready, crate::diagram_repo::accent_button(ui, "Add steps"))
                .on_hover_text(
                    "Append AI-designed steps to this suite (into the selected group when one is selected)",
                )
                .clicked()
            {
                acts.push(Act::GenSteps(StepGenMode::Append));
            }
            if ui
                .add_enabled(
                    ready && !st.suite.steps.is_empty(),
                    egui::Button::new("Add missing cleanup"),
                )
                .on_hover_text("Ask the AI for cleanup steps for data that is not removed yet")
                .clicked()
            {
                acts.push(Act::GenSteps(StepGenMode::Cleanup));
            }
            if ui
                .add_enabled(ready, egui::Button::new("Design new suites"))
                .on_hover_text("Let the AI design complete suites from all of the project's API endpoints")
                .clicked()
            {
                acts.push(Act::GenSuite);
            }
        });
        let mut auto = live.auto_validate;
        let env = live.env_name.clone().unwrap_or_else(|| "the active environment".into());
        if ui
            .checkbox(
                &mut auto,
                format!(
                    "After generating, run the steps against {env} and let AI fix what fails (test data is cleaned up)"
                ),
            )
            .changed()
        {
            acts.push(Act::SetAutoValidate(auto));
        }
    });
}

// ─── Body: group, tabel, detail ─────────────────────────────────────────────

fn body(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, live: &Live, acts: &mut Vec<Act>) {
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LEFT_WIDTH, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(LEFT_WIDTH, height));
                egui::ScrollArea::vertical()
                    .id_salt("it_left")
                    .show(ui, |ui| left_panel(ui, st, live));
            },
        );
        ui.separator();
        let center_w = (ui.available_width() - RIGHT_WIDTH - 12.0).max(360.0);
        ui.allocate_ui_with_layout(
            egui::vec2(center_w, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(center_w, height));
                center_panel(ui, st, live, acts);
            },
        );
        ui.separator();
        let right_w = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(right_w, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(egui::vec2(right_w, height));
                egui::ScrollArea::vertical()
                    .id_salt("it_right")
                    .show(ui, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin {
                                left: 6,
                                right: 12,
                                top: 0,
                                bottom: 0,
                            })
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width());
                                right_panel(ui, st, live, acts)
                            });
                    });
            },
        );
    });
}

fn left_panel(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, live: &Live) {
    ui.label(egui::RichText::new("Groups").strong());
    weak(
        ui,
        "Run one group at a time; steps it needs from other groups run too.",
    );
    ui.add_space(4.0);
    let total = st.suite.steps.len();
    if ui
        .selectable_label(
            st.filter == GroupFilter::All,
            format!("All steps ({total})"),
        )
        .clicked()
    {
        st.filter = GroupFilter::All;
    }
    let mut groups = st.suite.group_names();
    if st.suite.steps.iter().any(|s| s.group.trim().is_empty()) {
        groups.insert(0, String::new());
    }
    let mut rename_done: Option<(String, String)> = None;
    let mut remove: Option<String> = None;
    let mut move_group: Option<(String, isize)> = None;
    for g in &groups {
        let steps: Vec<&HttpTestStep> = st.suite.steps.iter().filter(|s| s.group == *g).collect();
        let passed = steps
            .iter()
            .filter(|s| live.results.get(&s.id).is_some_and(|r| r.passed()))
            .count();
        let failed = steps
            .iter()
            .filter(|s| {
                live.results.get(&s.id).is_some_and(|r| {
                    matches!(
                        r.outcome(),
                        StepOutcome::Failed | StepOutcome::Error | StepOutcome::Blocked
                    )
                })
            })
            .count();
        if let Some((from, to)) = st.renaming.as_mut()
            && from == g
        {
            let resp = ui.add(egui::TextEdit::singleline(to).desired_width(LEFT_WIDTH - 20.0));
            if resp.lost_focus() {
                rename_done = Some((from.clone(), to.clone()));
            } else {
                resp.request_focus();
            }
            continue;
        }
        let selected = st.filter == GroupFilter::Group(g.clone());
        let resp = ui
            .horizontal(|ui| {
                let r = ui.selectable_label(
                    selected,
                    format!(
                        "{} {} ({})",
                        ic::ICON_FOLDER.codepoint,
                        group_label(g),
                        steps.len()
                    ),
                );
                if passed > 0 {
                    ui.label(
                        egui::RichText::new(format!("{passed}"))
                            .small()
                            .color(style::theme_success(ui.ctx())),
                    )
                    .on_hover_text("Passed in the last run");
                }
                if failed > 0 {
                    ui.label(
                        egui::RichText::new(format!("{failed}"))
                            .small()
                            .color(style::theme_danger(ui.ctx())),
                    )
                    .on_hover_text("Failed or blocked in the last run");
                }
                r
            })
            .inner;
        if resp.clicked() {
            st.filter = GroupFilter::Group(g.clone());
        }
        if !g.is_empty() {
            resp.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    st.renaming = Some((g.clone(), g.clone()));
                    ui.close();
                }
                if ui.button("Move up").clicked() {
                    move_group = Some((g.clone(), -1));
                    ui.close();
                }
                if ui.button("Move down").clicked() {
                    move_group = Some((g.clone(), 1));
                    ui.close();
                }
                if ui.button("Remove group (keep steps)").clicked() {
                    remove = Some(g.clone());
                    ui.close();
                }
            });
        }
    }
    if let Some((from, to)) = rename_done {
        st.renaming = None;
        if !to.trim().is_empty() && to.trim() != from {
            st.suite.rename_group(&from, &to);
            if st.filter == GroupFilter::Group(from) {
                st.filter = GroupFilter::Group(to.trim().to_string());
            }
            st.mark();
        }
    }
    if let Some(g) = remove {
        st.suite.remove_group(&g);
        st.filter = GroupFilter::All;
        st.mark();
    }
    if let Some((g, d)) = move_group {
        st.suite.move_group(&g, d);
        st.mark();
    }
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.new_group)
                .desired_width(LEFT_WIDTH - 46.0)
                .hint_text("New group, e.g. Purchasing"),
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (icon_button(ui, ic::ICON_ADD.codepoint, "Add group").clicked() || enter)
            && !st.new_group.trim().is_empty()
        {
            let name = st.new_group.trim().to_string();
            st.suite.groups = st.suite.group_names();
            if !st.suite.groups.contains(&name) {
                st.suite.groups.push(name.clone());
            }
            st.filter = GroupFilter::Group(name);
            st.new_group.clear();
            st.mark();
        }
    });

    ui.add_space(10.0);
    ui.separator();
    variables_section(ui, st, live);
}

fn variables_section(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, live: &Live) {
    ui.label(egui::RichText::new("Variables").strong());
    weak(
        ui,
        "Suite values override the project environment; empty values use the environment. {{run_id}} is unique per run.",
    );
    let mut remove: Option<usize> = None;
    let mut changed = false;
    for (i, (k, v)) in st.suite.variables.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(k)
                        .desired_width(80.0)
                        .hint_text("name"),
                )
                .changed();
            let lower = k.to_ascii_lowercase();
            let secret = crate::http_client_widgets::is_sensitive_key(k)
                || lower.contains("token")
                || lower.contains("password");
            let hint = if live.known_vars.contains(k.trim()) {
                "from environment"
            } else {
                "value"
            };
            changed |= ui
                .add(
                    egui::TextEdit::singleline(v)
                        .desired_width(LEFT_WIDTH - 120.0)
                        .password(secret)
                        .hint_text(hint),
                )
                .changed();
            if icon_button(ui, ic::ICON_CLOSE.codepoint, "Remove variable").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        st.suite.variables.remove(i);
        changed = true;
    }
    if ui
        .small_button(format!("{} Variable", ic::ICON_ADD.codepoint))
        .clicked()
    {
        st.suite.variables.push((String::new(), String::new()));
        changed = true;
    }
    if changed {
        st.mark();
    }
    if !live.known_vars.is_empty() {
        let mut names: Vec<&str> = live.known_vars.iter().map(String::as_str).collect();
        names.sort_unstable();
        ui.add_space(4.0);
        weak(
            ui,
            format!(
                "Environment {}: {}",
                live.env_name.clone().unwrap_or_default(),
                names.join(", ")
            ),
        );
    }
}

/// Langkah yang tampil di tabel untuk filter saat ini. `true` menandai langkah
/// group lain yang ikut dijalankan karena dibutuhkan group terpilih.
fn visible_rows(st: &IntegrationTestTabState) -> Vec<(usize, bool)> {
    let order = st.suite.execution_order();
    match &st.filter {
        GroupFilter::All => order.into_iter().map(|i| (i, false)).collect(),
        GroupFilter::Group(g) => {
            let include = http_tests::select_for_groups(&st.suite, std::slice::from_ref(g));
            order
                .into_iter()
                .filter(|&i| include[i] || st.suite.steps[i].group == *g)
                .map(|i| (i, st.suite.steps[i].group != *g))
                .collect()
        }
    }
}

#[derive(Default)]
struct RowActions {
    select: Option<String>,
    open: Option<String>,
    swap: Option<(usize, usize)>,
    delete: Option<usize>,
    toggle: Option<usize>,
    /// Buat langkah cleanup untuk langkah dengan id ini.
    add_cleanup: Option<String>,
}

/// Lebar kolom tabel langkah; header dan semua blok memakai lebar yang sama
/// supaya kolom tetap sejajar.
struct Cols {
    check: f32,
    num: f32,
    method: f32,
    name: f32,
    result: f32,
    time: f32,
    menu: f32,
}

const COL_GAP: f32 = 8.0;
const ROW_H: f32 = 38.0;

impl Cols {
    fn new(total: f32) -> Self {
        let (check, num, method, result, time, menu) = (22.0, 28.0, 62.0, 112.0, 64.0, 28.0);
        let fixed = check + num + method + result + time + menu + COL_GAP * 7.0 + 12.0;
        Self {
            check,
            num,
            method,
            name: (total - fixed).max(160.0),
            result,
            time,
            menu,
        }
    }
}

/// Satu sel tabel dengan lebar tetap.
fn cell<R>(
    ui: &mut egui::Ui,
    width: f32,
    layout: egui::Layout,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(egui::vec2(width, ROW_H), layout, |ui| {
        ui.set_min_size(egui::vec2(width, ROW_H));
        ui.set_max_width(width);
        add(ui)
    })
    .inner
}

fn cell_left() -> egui::Layout {
    egui::Layout::left_to_right(egui::Align::Center)
}

/// Blok berurutan (fase + group) dari baris yang terlihat.
fn row_blocks(st: &IntegrationTestTabState, rows: &[(usize, bool)]) -> Vec<Vec<(usize, bool)>> {
    let mut out: Vec<Vec<(usize, bool)>> = Vec::new();
    let mut last: Option<(bool, (StepPhase, String))> = None;
    for &(i, required) in rows {
        let s = &st.suite.steps[i];
        let key = (required, block_key(s.phase, &s.group));
        if last.as_ref() != Some(&key) {
            out.push(Vec::new());
            last = Some(key);
        }
        if let Some(b) = out.last_mut() {
            b.push((i, required));
        }
    }
    out
}

/// Kalimat aliran data satu langkah untuk tooltip di tabel.
fn flow_sentence(
    st: &IntegrationTestTabState,
    deps: &[http_tests::StepDeps],
    pos: &HashMap<usize, usize>,
    i: usize,
) -> String {
    let d = &deps[i];
    let mut lines = Vec::new();
    for v in &d.uses {
        match d.depends_on.iter().find(|&&p| deps[p].provides.contains(v)) {
            Some(&p) => lines.push(format!("Needs {{{{{v}}}}} from step {}", pos[&p] + 1)),
            None => lines.push(format!("Needs {{{{{v}}}}}")),
        }
    }
    for v in &d.provides {
        let users: Vec<String> = d
            .used_by
            .iter()
            .filter(|&&u| deps[u].uses.contains(v))
            .map(|&u| (pos[&u] + 1).to_string())
            .collect();
        if users.is_empty() {
            lines.push(format!("Gives {{{{{v}}}}} (not used yet)"));
        } else {
            lines.push(format!("Gives {{{{{v}}}}} to step {}", users.join(", ")));
        }
    }
    if let Some(t) = http_tests::cleanup_target(&st.suite, deps, i) {
        lines.push(format!("Removes the data of step {}", pos[&t] + 1));
    }
    lines.join("\n")
}

fn center_panel(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    let deps = http_tests::dependencies(&st.suite);
    let order = st.suite.execution_order();
    let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(p, &i)| (i, p)).collect();
    let rows = visible_rows(st);
    let issues = http_tests::lint(&st.suite, &live.known_vars);

    // Judul + tombol tambah langkah.
    ui.horizontal(|ui| {
        let title = match &st.filter {
            GroupFilter::All => "All steps".to_string(),
            GroupFilter::Group(g) => group_label(g).to_string(),
        };
        ui.label(egui::RichText::new(title).strong().size(14.5));
        weak(
            ui,
            format!("{} step(s), run from top to bottom", rows.len()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(format!("{} Add step", ic::ICON_ADD.codepoint), |ui| {
                if ui
                    .button(format!(
                        "{} From the project's API collection…",
                        ic::ICON_HTTP.codepoint
                    ))
                    .clicked()
                {
                    st.picker = Some(String::new());
                    ui.close();
                }
                if ui
                    .button(format!("{} Empty request", ic::ICON_ADD.codepoint))
                    .clicked()
                {
                    add_step(st, empty_request(), None);
                    ui.close();
                }
                if ui
                    .button(format!("{} Database check…", ic::ICON_DATABASE.codepoint))
                    .on_hover_text(
                        "A read-only SELECT with checks on its result, e.g. the balance after a receipt",
                    )
                    .clicked()
                {
                    add_db_step(st);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        st.selected_index().is_some(),
                        egui::Button::new(format!(
                            "{} Cleanup step for the selected step",
                            ic::ICON_CLEANING_SERVICES.codepoint
                        )),
                    )
                    .clicked()
                {
                    add_cleanup_for_selected(st);
                    ui.close();
                }
            });
        });
    });
    ui.add_space(6.0);

    let mut ra = RowActions::default();
    let cols = Cols::new(ui.available_width());
    if !rows.is_empty() {
        table_header(ui, &cols);
    }
    let checks_h = match (issues.is_empty(), st.checks_open) {
        (true, _) => 0.0,
        (false, false) => 34.0,
        (false, true) => 170.0,
    };
    let table_h = (ui.available_height() - checks_h - 6.0).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("it_steps")
        .max_height(table_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if rows.is_empty() {
                empty_state(ui, st);
                return;
            }
            for block in row_blocks(st, &rows) {
                let first = &st.suite.steps[block[0].0];
                block_header(ui, first.phase, &first.group, block[0].1, block.len());
                for (n, &(i, req)) in block.iter().enumerate() {
                    step_row(
                        ui,
                        st,
                        live,
                        &deps,
                        &order,
                        &pos,
                        i,
                        req,
                        n % 2 == 1,
                        &cols,
                        &mut ra,
                    );
                }
            }
            ui.add_space(8.0);
        });

    if !issues.is_empty() {
        checks_bar(ui, st, &issues, &pos, &mut ra);
    }

    if let Some(id) = ra.select {
        st.selected = Some(id);
    }
    if let Some(id) = ra.open {
        st.selected = Some(id.clone());
        acts.push(Act::OpenStep(id));
    }
    if let Some(i) = ra.toggle {
        st.suite.steps[i].enabled = !st.suite.steps[i].enabled;
        st.mark();
    }
    if let Some((a, b)) = ra.swap {
        st.suite.steps.swap(a, b);
        st.mark();
    }
    if let Some(id) = ra.add_cleanup {
        st.selected = Some(id);
        add_cleanup_for_selected(st);
    }
    if let Some(i) = ra.delete {
        let removed = st.suite.steps.remove(i);
        for s in &mut st.suite.steps {
            if s.cleanup_for.as_deref() == Some(removed.id.as_str()) {
                s.cleanup_for = None;
            }
        }
        if st.selected.as_deref() == Some(removed.id.as_str()) {
            st.selected = None;
        }
        st.mark();
    }
}

fn table_header(ui: &mut egui::Ui, c: &Cols) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = COL_GAP;
        ui.add_space(6.0);
        let head = |ui: &mut egui::Ui, w: f32, text: &str| {
            ui.allocate_ui_with_layout(egui::vec2(w, 18.0), cell_left(), |ui| {
                ui.set_min_width(w);
                ui.label(
                    egui::RichText::new(text)
                        .small()
                        .strong()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        };
        head(ui, c.check, "");
        head(ui, c.num, "#");
        head(ui, c.method, "METHOD");
        head(ui, c.name, "STEP");
        head(ui, c.result, "LAST RESULT");
        head(ui, c.time, "TIME");
        head(ui, c.menu, "");
    });
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        egui::Stroke::new(1.0, style::nav_border(ui.ctx())),
    );
}

fn block_header(ui: &mut egui::Ui, phase: StepPhase, group: &str, required: bool, count: usize) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let color = phase_color(ui.ctx(), phase);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 1.0, color);
        ui.label(egui::RichText::new(block_title(phase, group)).strong());
        weak(ui, format!("{count} step(s)"));
        if required {
            weak(ui, "· runs too, because the selected group needs its data");
        } else {
            weak(ui, format!("· {}", block_hint(phase)));
        }
    });
    ui.add_space(2.0);
}

#[allow(clippy::too_many_arguments)]
fn step_row(
    ui: &mut egui::Ui,
    st: &IntegrationTestTabState,
    live: &Live,
    deps: &[http_tests::StepDeps],
    order: &[usize],
    pos: &HashMap<usize, usize>,
    i: usize,
    required: bool,
    stripe: bool,
    cols: &Cols,
    ra: &mut RowActions,
) {
    let step = &st.suite.steps[i];
    let same_block = |j: usize| {
        let n = &st.suite.steps[j];
        n.phase == step.phase && n.group == step.group
    };
    let order_pos = order.iter().position(|&x| x == i).unwrap_or(0);
    let up = (order_pos > 0 && same_block(order[order_pos - 1])).then(|| order[order_pos - 1]);
    let down = (order_pos + 1 < order.len() && same_block(order[order_pos + 1]))
        .then(|| order[order_pos + 1]);
    let selected = st.selected.as_deref() == Some(step.id.as_str());
    let dim = required || !step.enabled;
    let text_color = if dim {
        ui.visuals().weak_text_color()
    } else {
        ui.visuals().strong_text_color()
    };

    // Seluruh baris bisa diklik; widget di dalamnya tetap menerima kliknya sendiri.
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if selected {
        ui.visuals().widgets.active.weak_bg_fill
    } else if hovered {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else if stripe {
        ui.visuals().faint_bg_color
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, ROW_H)),
            1.5,
            style::theme_accent(ui.ctx()),
        );
    }
    if resp.clicked() {
        ra.select = Some(step.id.clone());
    }
    if resp.double_clicked() {
        ra.open = Some(step.id.clone());
    }
    let can_cleanup = step.phase != StepPhase::Cleanup && step.mutates();
    resp.context_menu(|ui| row_menu(ui, step, i, up, down, can_cleanup, ra));

    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(6.0, 0.0)))
            .layout(cell_left()),
    );
    row.spacing_mut().item_spacing.x = COL_GAP;
    let ui = &mut row;

    cell(ui, cols.check, cell_left(), |ui| {
        let mut on = step.enabled;
        if ui
            .checkbox(&mut on, "")
            .on_hover_text(if on {
                "Included in runs. Untick to skip this step."
            } else {
                "Skipped in runs. Tick to include it again."
            })
            .changed()
        {
            ra.toggle = Some(i);
        }
    });
    cell(ui, cols.num, cell_left(), |ui| {
        ui.label(
            egui::RichText::new(format!("{}", pos[&i] + 1)).color(ui.visuals().weak_text_color()),
        );
    });
    cell(ui, cols.method, cell_left(), |ui| {
        crate::http_repo::method_chip(ui, step.method_label());
        if let Some(c) = step.concurrent() {
            ui.label(
                egui::RichText::new(ic::ICON_BOLT.codepoint)
                    .size(12.0)
                    .color(style::theme_warning(ui.ctx())),
            )
            .on_hover_text(format!(
                "Sent {} times at once, {} round(s)",
                c.count(),
                c.rounds()
            ));
        }
    });
    let subtitle = match (&step.db, step.is_db()) {
        (Some(db), true) => format!(
            "{{{{{}}}}}  {}",
            db.connection_var,
            db.sql.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
        _ => step.request.url.clone(),
    };
    cell(
        ui,
        cols.name,
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let flow = flow_sentence(st, deps, pos, i);
                let reserve = if flow.is_empty() { 0.0 } else { 20.0 };
                let mut name = egui::RichText::new(&step.name).color(text_color);
                if required {
                    name = name.italics();
                }
                if !step.enabled {
                    name = name.strikethrough();
                }
                ui.allocate_ui_with_layout(
                    egui::vec2((cols.name - reserve).max(40.0), 18.0),
                    cell_left(),
                    |ui| {
                        ui.add(egui::Label::new(name).truncate().selectable(false));
                    },
                );
                if !flow.is_empty() {
                    ui.label(
                        egui::RichText::new(ic::ICON_LINK.codepoint)
                            .size(13.0)
                            .color(style::theme_info(ui.ctx())),
                    )
                    .on_hover_text(flow);
                }
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&subtitle)
                        .family(egui::FontFamily::Monospace)
                        .size(10.5)
                        .color(ui.visuals().weak_text_color()),
                )
                .truncate()
                .selectable(false),
            );
        },
    );
    let running_here = live.running && live.current == Some(i);
    let result = live.results.get(&step.id);
    cell(ui, cols.result, cell_left(), |ui| {
        if running_here {
            ui.add(egui::Spinner::new().size(12.0));
            weak(ui, "Running");
        } else if let Some(r) = result {
            let o = r.outcome();
            let tip = match &r.note {
                Some(n) => format!("{}\n{n}", outcome_help(o)),
                None => outcome_help(o).to_string(),
            };
            outcome_badge(ui, o, 12.0).on_hover_text(tip);
        } else if live.running
            && live
                .run_scope
                .as_ref()
                .is_none_or(|m| m.get(i).copied().unwrap_or(false))
        {
            weak(ui, "Waiting");
        } else {
            weak(ui, "Not run");
        }
    });
    cell(ui, cols.time, cell_left(), |ui| {
        if let Some(r) = result.filter(|r| r.executed() || r.error.is_some()) {
            weak(ui, report::fmt_ms(r.time_ms));
        }
    });
    cell(ui, cols.menu, cell_left(), |ui| {
        ui.menu_button(ic::ICON_MORE_VERT.codepoint, |ui| {
            row_menu(ui, step, i, up, down, can_cleanup, ra)
        })
        .response
        .on_hover_text("Step actions");
    });
}

/// Aksi satu langkah; dipakai menu titik tiga dan klik kanan.
fn row_menu(
    ui: &mut egui::Ui,
    step: &HttpTestStep,
    i: usize,
    up: Option<usize>,
    down: Option<usize>,
    can_cleanup: bool,
    ra: &mut RowActions,
) {
    if ui
        .button(format!("{} Edit details", ic::ICON_TUNE.codepoint))
        .clicked()
    {
        ra.select = Some(step.id.clone());
        ui.close();
    }
    if ui
        .button(format!(
            "{} Open request in HTTP client",
            ic::ICON_OPEN_IN_NEW.codepoint
        ))
        .clicked()
    {
        ra.open = Some(step.id.clone());
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(
            up.is_some(),
            egui::Button::new(format!("{} Move up", ic::ICON_ARROW_UPWARD.codepoint)),
        )
        .on_disabled_hover_text("Steps move only within their own block")
        .clicked()
        && let Some(j) = up
    {
        ra.swap = Some((i, j));
        ui.close();
    }
    if ui
        .add_enabled(
            down.is_some(),
            egui::Button::new(format!("{} Move down", ic::ICON_ARROW_DOWNWARD.codepoint)),
        )
        .on_disabled_hover_text("Steps move only within their own block")
        .clicked()
        && let Some(j) = down
    {
        ra.swap = Some((i, j));
        ui.close();
    }
    if can_cleanup
        && ui
            .button(format!(
                "{} Add cleanup step for this",
                ic::ICON_CLEANING_SERVICES.codepoint
            ))
            .on_hover_text("Adds a DELETE step that removes the data this step creates")
            .clicked()
    {
        ra.add_cleanup = Some(step.id.clone());
        ui.close();
    }
    ui.separator();
    if ui
        .button(
            egui::RichText::new(format!("{} Delete step", ic::ICON_DELETE.codepoint))
                .color(style::theme_danger(ui.ctx())),
        )
        .clicked()
    {
        ra.delete = Some(i);
        ui.close();
    }
}

/// Panduan saat suite belum punya langkah.
fn empty_state(ui: &mut egui::Ui, st: &mut IntegrationTestTabState) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(520.0);
        style::theme_card_frame(ui.ctx()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.label(
                    egui::RichText::new("Build your first test flow")
                        .strong()
                        .size(16.0),
                );
                weak(
                    ui,
                    "A suite is a list of API requests that run in order and pass values to each other.",
                );
                ui.add_space(10.0);
                let item = |ui: &mut egui::Ui, n: &str, title: &str, text: &str| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(n)
                                .strong()
                                .size(15.0)
                                .color(style::theme_accent(ui.ctx())),
                        );
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(title).strong());
                            weak(ui, text);
                        });
                    });
                    ui.add_space(6.0);
                };
                item(
                    ui,
                    "1",
                    "Add the requests to test",
                    "Pick them from the project's API collection, or let AI build the flow from a description.",
                );
                item(
                    ui,
                    "2",
                    "Pass values between steps",
                    "Save a value from a response, e.g. the new id, and use it later as {{id}}.",
                );
                item(
                    ui,
                    "3",
                    "Say what a good answer looks like",
                    "Add checks such as status equals 200, then press Run all.",
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(crate::diagram_repo::accent_button(
                            ui,
                            format!("{} Add from API collection", ic::ICON_HTTP.codepoint),
                        ))
                        .clicked()
                    {
                        st.picker = Some(String::new());
                    }
                    if ui
                        .button(format!(
                            "{} Generate with AI",
                            ic::ICON_AUTO_AWESOME.codepoint
                        ))
                        .clicked()
                    {
                        st.ai_open = true;
                    }
                    if ui.button("Empty request").clicked() {
                        add_step(st, empty_request(), None);
                    }
                });
            });
        });
    });
}

/// Baris ringkas hasil pemeriksaan suite; bisa dibuka menjadi daftar.
fn checks_bar(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    issues: &[http_tests::LintIssue],
    pos: &HashMap<usize, usize>,
    ra: &mut RowActions,
) {
    let count = |l: LintLevel| issues.iter().filter(|i| i.level == l).count();
    let (errors, warnings, infos) = (
        count(LintLevel::Error),
        count(LintLevel::Warning),
        count(LintLevel::Info),
    );
    let (icon, color) = if errors > 0 {
        (ic::ICON_ERROR.codepoint, style::theme_danger(ui.ctx()))
    } else if warnings > 0 {
        (ic::ICON_WARNING.codepoint, style::theme_warning(ui.ctx()))
    } else {
        (ic::ICON_INFO.codepoint, style::theme_info(ui.ctx()))
    };
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(format!("{errors} error(s)"));
    }
    if warnings > 0 {
        parts.push(format!("{warnings} warning(s)"));
    }
    if infos > 0 {
        parts.push(format!("{infos} tip(s)"));
    }
    ui.add_space(2.0);
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        egui::Stroke::new(1.0, style::nav_border(ui.ctx())),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let chevron = if st.checks_open {
            ic::ICON_EXPAND_MORE.codepoint
        } else {
            ic::ICON_CHEVRON_RIGHT.codepoint
        };
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(format!(
                        "{chevron} {icon} Suite checks: {}",
                        parts.join(", ")
                    ))
                    .color(color),
                )
                .frame(false),
            )
            .on_hover_text("Problems found in the suite before running it")
            .clicked()
        {
            st.checks_open = !st.checks_open;
        }
        if st.checks_open {
            weak(ui, "Click a step name to select it.");
        }
    });
    if !st.checks_open {
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("it_lint")
        .max_height(130.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for issue in issues {
                let (glyph, color) = match issue.level {
                    LintLevel::Error => (ic::ICON_ERROR.codepoint, style::theme_danger(ui.ctx())),
                    LintLevel::Warning => {
                        (ic::ICON_WARNING.codepoint, style::theme_warning(ui.ctx()))
                    }
                    LintLevel::Info => (ic::ICON_INFO.codepoint, style::theme_info(ui.ctx())),
                };
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.label(egui::RichText::new(glyph).color(color));
                    if let Some(s) = issue.step {
                        let label = format!(
                            "Step {} '{}':",
                            pos.get(&s).map(|p| p + 1).unwrap_or(0),
                            st.suite.steps[s].name
                        );
                        if ui.link(egui::RichText::new(label).small()).clicked() {
                            ra.select = Some(st.suite.steps[s].id.clone());
                        }
                    }
                    ui.label(egui::RichText::new(&issue.message).small());
                });
            }
        });
}

fn empty_request() -> SavedRequest {
    SavedRequest {
        name: "New step".into(),
        url: "{{base_url}}/".into(),
        method: HttpMethod::GET,
        form_data: vec![(String::new(), String::new(), true)],
        ..Default::default()
    }
}

/// Tambah langkah DB (query baca) ke group yang sedang dipilih.
fn add_db_step(st: &mut IntegrationTestTabState) {
    let group = match &st.filter {
        GroupFilter::Group(g) => g.clone(),
        GroupFilter::All => String::new(),
    };
    let step = HttpTestStep {
        id: st.suite.next_step_id(),
        name: "Check database".into(),
        phase: StepPhase::Test,
        group,
        kind: http_tests::StepKind::DbQuery,
        db: Some(http_tests::DbQueryStep::default()),
        assertions: vec![Assertion {
            target: "row_count".into(),
            op: AssertOp::Equals,
            value: "0".into(),
        }],
        enabled: true,
        ..Default::default()
    };
    st.selected = Some(step.id.clone());
    st.suite.steps.push(step);
    st.suite.normalize_order();
    st.mark();
}

/// Tambah langkah ke group yang sedang dipilih.
fn add_step(st: &mut IntegrationTestTabState, request: SavedRequest, request_id: Option<String>) {
    let group = match &st.filter {
        GroupFilter::Group(g) => g.clone(),
        GroupFilter::All => String::new(),
    };
    let mutating = http_tests::is_mutating(&request.method);
    let step = HttpTestStep {
        id: st.suite.next_step_id(),
        name: request.display_name(),
        phase: StepPhase::Test,
        group,
        assertions: vec![Assertion {
            target: "status".into(),
            op: AssertOp::LessThan,
            value: if mutating { "300".into() } else { "400".into() },
        }],
        enabled: true,
        request_id,
        request,
        ..Default::default()
    };
    st.selected = Some(step.id.clone());
    st.suite.steps.push(step);
    st.suite.normalize_order();
    st.mark();
}

/// Buat langkah DELETE untuk langkah terpilih yang membuat data.
fn add_cleanup_for_selected(st: &mut IntegrationTestTabState) {
    let Some(i) = st.selected_index() else {
        return;
    };
    let src = st.suite.steps[i].clone();
    let id_var = src
        .extract
        .iter()
        .map(|e| e.var.clone())
        .find(|v| v.to_ascii_lowercase().contains("id"))
        .or_else(|| src.extract.first().map(|e| e.var.clone()));
    let base = src
        .request
        .url
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let url = match &id_var {
        Some(v) => format!("{base}/{{{{{v}}}}}"),
        None => base.to_string(),
    };
    let request = SavedRequest {
        name: format!("Delete: {}", src.name),
        url,
        method: HttpMethod::DELETE,
        headers: src.request.headers.clone(),
        auth_type: src.request.auth_type.clone(),
        bearer_token: src.request.bearer_token.clone(),
        basic_user: src.request.basic_user.clone(),
        basic_pass: src.request.basic_pass.clone(),
        api_key_name: src.request.api_key_name.clone(),
        api_key_value: src.request.api_key_value.clone(),
        api_key_in_header: src.request.api_key_in_header,
        tables: src.request.tables.clone(),
        form_data: vec![(String::new(), String::new(), true)],
        ..Default::default()
    };
    let step = HttpTestStep {
        id: st.suite.next_step_id(),
        name: format!("Delete: {}", src.name),
        phase: StepPhase::Cleanup,
        group: src.group.clone(),
        cleanup_for: Some(src.id.clone()),
        assertions: vec![Assertion {
            target: "status".into(),
            op: AssertOp::LessThan,
            value: "300".into(),
        }],
        enabled: true,
        request,
        ..Default::default()
    };
    st.selected = Some(step.id.clone());
    st.suite.steps.push(step);
    http_tests::auto_order_cleanup(&mut st.suite);
    st.mark();
}

fn right_panel(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    let Some(i) = st.selected_index() else {
        suite_overview(ui, st, live);
        return;
    };
    let deps = http_tests::dependencies(&st.suite);
    let order = st.suite.execution_order();
    let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(p, &x)| (x, p)).collect();
    let groups = st.suite.group_names();
    let creators: Vec<(String, String)> = st
        .suite
        .steps
        .iter()
        .filter(|s| s.phase != StepPhase::Cleanup && s.mutates())
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    let result = live.results.get(&st.suite.steps[i].id).cloned();
    let mut changed = false;
    let mut close = false;

    {
        let step = &mut st.suite.steps[i];
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Step {} of {}", pos[&i] + 1, order.len()))
                    .strong()
                    .size(14.0),
            );
            ui.label(
                egui::RichText::new(block_title(step.phase, &step.group))
                    .small()
                    .color(phase_color(ui.ctx(), step.phase)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(
                    ui,
                    ic::ICON_CLOSE.codepoint,
                    "Close and show the suite overview",
                )
                .clicked()
                {
                    close = true;
                }
            });
        });
        ui.add_space(4.0);
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut step.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("Step name, e.g. Create purchase order"),
            )
            .changed();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Type").on_hover_text(
                "HTTP request, or a read-only database query that checks the data the API wrote",
            );
            let before = step.kind;
            egui::ComboBox::from_id_salt("it_kind")
                .selected_text(step.kind.label())
                .width(160.0)
                .show_ui(ui, |ui| {
                    for k in [http_tests::StepKind::Http, http_tests::StepKind::DbQuery] {
                        ui.selectable_value(&mut step.kind, k, k.label());
                    }
                });
            if step.kind != before {
                if step.is_db() {
                    step.db.get_or_insert_with(Default::default);
                    step.concurrency = None;
                    if step.assertions.iter().all(|a| a.target == "status") {
                        step.assertions = vec![Assertion {
                            target: "row_count".into(),
                            op: AssertOp::Equals,
                            value: "0".into(),
                        }];
                    }
                }
                changed = true;
            }
        });
        ui.add_space(4.0);
        if step.is_db() {
            changed |= db_query_section(ui, step, live);
        } else {
            style::theme_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    crate::http_repo::method_chip(ui, step.request.method.label());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&step.request.url)
                                .family(egui::FontFamily::Monospace)
                                .size(11.5),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&step.request.url);
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!(
                            "{} Edit request in HTTP client",
                            ic::ICON_OPEN_IN_NEW.codepoint
                        ))
                        .on_hover_text(
                            "Opens a new tab. Save there writes the request back to this step.",
                        )
                        .clicked()
                    {
                        acts.push(Act::OpenStep(step.id.clone()));
                    }
                });
            });
        }
    }

    if let Some(r) = &result {
        if last_result_section(ui, r, live) {
            st.codefix_dialog = Some((vec![r.step_id.clone()], String::new()));
        }
        result_evidence(ui, r);
    }
    changed |= assertions_section(ui, &mut st.suite.steps[i], result.as_ref());
    changed |= extract_section(ui, &mut st.suite.steps[i]);
    if st.suite.steps[i].kind.is_http() {
        changed |= concurrency_section(ui, &mut st.suite.steps[i]);
    }
    data_flow_section(ui, st, live, &deps, &pos, i);
    changed |= settings_section(ui, &mut st.suite.steps[i], &groups, &creators);
    ui.add_space(12.0);

    if changed {
        st.mark();
    }
    if close {
        st.selected = None;
    }
}

/// Fase, group, target cleanup, aktif/nonaktif dan catatan requirement.
fn settings_section(
    ui: &mut egui::Ui,
    step: &mut HttpTestStep,
    groups: &[String],
    creators: &[(String, String)],
) -> bool {
    let mut changed = false;
    section_header(ui, ic::ICON_TUNE.codepoint, "Settings", None);
    egui::Grid::new("it_step_meta")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Phase")
                .on_hover_text("Setup runs first, Test next, Cleanup always runs last");
            egui::ComboBox::from_id_salt("it_phase")
                .selected_text(step.phase.label())
                .width(200.0)
                .show_ui(ui, |ui| {
                    for p in StepPhase::ALL {
                        changed |= ui
                            .selectable_value(&mut step.phase, p, p.label())
                            .on_hover_text(block_hint(p))
                            .changed();
                    }
                });
            ui.end_row();
            ui.label("Group")
                .on_hover_text("Business area this step belongs to, e.g. Purchasing");
            egui::ComboBox::from_id_salt("it_group")
                .selected_text(group_label(&step.group).to_string())
                .width(200.0)
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut step.group, String::new(), "No group")
                        .changed();
                    for g in groups {
                        changed |= ui
                            .selectable_value(&mut step.group, g.clone(), g.as_str())
                            .changed();
                    }
                });
            ui.end_row();
            if step.phase == StepPhase::Cleanup {
                ui.label("Removes data of")
                    .on_hover_text("The step whose test data this cleanup step deletes");
                let current = step
                    .cleanup_for
                    .as_deref()
                    .and_then(|id| creators.iter().find(|(c, _)| c == id))
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| "Detect from variables".into());
                egui::ComboBox::from_id_salt("it_cleans")
                    .selected_text(current)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(&mut step.cleanup_for, None, "Detect from variables")
                            .changed();
                        for (id, name) in creators {
                            changed |= ui
                                .selectable_value(
                                    &mut step.cleanup_for,
                                    Some(id.clone()),
                                    name.as_str(),
                                )
                                .changed();
                        }
                    });
                ui.end_row();
            }
            ui.label("Include in runs");
            changed |= ui.checkbox(&mut step.enabled, "").changed();
            ui.end_row();
        });
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Requirement / note").small())
        .on_hover_text("What this step verifies. Shown in the report.");
    changed |= ui
        .add(
            egui::TextEdit::multiline(&mut step.notes)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("e.g. REQ-PUR-012: an approved PO can be received"),
        )
        .changed();
    changed
}

/// Nama operator yang mudah dibaca untuk daftar pilihan.
fn op_text(op: AssertOp) -> &'static str {
    match op {
        AssertOp::LessThan => "is less than",
        AssertOp::GreaterThan => "is greater than",
        AssertOp::Equals => "equals",
        AssertOp::NotEquals => "does not equal",
        AssertOp::Contains => "contains",
        AssertOp::NotContains => "does not contain",
        AssertOp::Exists => "exists",
        AssertOp::NotExists => "does not exist",
        AssertOp::Matches => "matches regex",
        AssertOp::Approx => "is about (number)",
    }
}

const FIELD_HELP: &str = "What to check in the response:\n\
status: the HTTP status code\n\
time_ms: the response time in ms\n\
body: the whole response body\n\
header:Content-Type: a response header\n\
json:$.data.id: a value in the JSON body";

const DB_FIELD_HELP: &str = "What to check in the query result:\n\
row_count: number of rows\n\
scalar: first column of the first row\n\
cell:0.qty_balance: row 0, column qty_balance\n\
column:id: every value of a column, comma separated\n\
Values may use variables and arithmetic, e.g. {{before}} + {{qty}}.\n\
Use the condition 'is about (number)' for decimals; add +- 0.5 for a tolerance.";

const BURST_FIELD_HELP: &str = "Checks over all responses of the burst:\n\
count:status=2xx: how many responses were 2xx (also 409, 4xx)\n\
count:json:$.status=success: how many have this JSON value\n\
any:json:$.message: at least one response matches\n\
all:status: every response matches (same as a plain field)";

fn field_help(step: &HttpTestStep) -> &'static str {
    if step.is_db() {
        DB_FIELD_HELP
    } else if step.concurrent().is_some() {
        BURST_FIELD_HELP
    } else {
        FIELD_HELP
    }
}

/// Editor langkah DB: variabel koneksi, database, SQL, dan polling.
fn db_query_section(ui: &mut egui::Ui, step: &mut HttpTestStep, live: &Live) -> bool {
    let mut changed = false;
    let db = step.db.get_or_insert_with(Default::default);
    style::theme_card_frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(ic::ICON_DATABASE.codepoint).strong());
            ui.label(egui::RichText::new("Database query (read-only)").strong());
            help_icon(
                ui,
                "One SELECT runs against the connection named by the environment variable. \
                 {{variables}} become bound parameters, never pasted into the SQL. Writes are \
                 refused on every access level. Connections marked Production need the \
                 Production confirmation when the run starts.",
            );
        });
        ui.add_space(4.0);
        egui::Grid::new("it_db_meta")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Connection variable")
                    .on_hover_text("Environment variable whose value is the connection name or id, per environment");
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut db.connection_var)
                                .desired_width(140.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("db_main"),
                        )
                        .changed();
                    let var = db.connection_var.trim();
                    if !var.is_empty() && !live.known_vars.contains(var) {
                        ui.label(
                            egui::RichText::new(ic::ICON_WARNING.codepoint)
                                .color(style::theme_warning(ui.ctx())),
                        )
                        .on_hover_text(
                            "Not set in the active project environment. Add it there with the connection name as value.",
                        );
                    }
                });
                ui.end_row();
                ui.label("Database")
                    .on_hover_text("Optional; defaults to the connection's database");
                let mut d = db.database.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut d)
                            .desired_width(140.0)
                            .hint_text("default"),
                    )
                    .changed()
                {
                    db.database = Some(d).filter(|x| !x.trim().is_empty());
                    changed = true;
                }
                ui.end_row();
                ui.label("Wait until checks pass")
                    .on_hover_text("Re-run the query until every check holds, for data written asynchronously after the response");
                ui.horizontal(|ui| {
                    let mut wait = db.wait_until_ms.is_some();
                    if ui.checkbox(&mut wait, "").changed() {
                        db.wait_until_ms = wait.then_some(5_000);
                        changed = true;
                    }
                    if let Some(ms) = db.wait_until_ms.as_mut() {
                        changed |= ui
                            .add(
                                egui::DragValue::new(ms)
                                    .range(100..=crate::http_test_db::MAX_WAIT_MS)
                                    .speed(100)
                                    .suffix(" ms"),
                            )
                            .changed();
                        ui.label("every");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut db.poll_ms)
                                    .range(crate::http_test_db::MIN_POLL_MS..=10_000)
                                    .speed(50)
                                    .suffix(" ms"),
                            )
                            .changed();
                    }
                });
                ui.end_row();
            });
        ui.add_space(4.0);
        changed |= ui
            .add(
                egui::TextEdit::multiline(&mut db.sql)
                    .code_editor()
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .hint_text(
                        "SELECT qty_balance FROM stocks WHERE id = {{stock_id}} AND deleted_at IS NULL",
                    ),
            )
            .changed();
    });
    changed
}

/// Burst serentak untuk langkah HTTP (double submit, race condition).
fn concurrency_section(ui: &mut egui::Ui, step: &mut HttpTestStep) -> bool {
    let mut changed = false;
    section_header(
        ui,
        ic::ICON_BOLT.codepoint,
        "Concurrent requests",
        Some(
            "Send this request several times at once to catch double submits and race conditions.",
        ),
    );
    let mut on = step.concurrency.is_some();
    if ui
        .checkbox(&mut on, "Send this request in parallel")
        .changed()
    {
        step.concurrency = on.then(http_tests::Concurrency::default);
        if on
            && !step
                .assertions
                .iter()
                .any(|a| http_tests::is_aggregate_target(&a.target))
        {
            step.assertions.push(Assertion {
                target: "count:status=2xx".into(),
                op: AssertOp::Equals,
                value: "1".into(),
            });
        }
        changed = true;
    }
    if let Some(c) = step.concurrency.as_mut() {
        egui::Grid::new("it_conc")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Requests at once");
                changed |= ui
                    .add(egui::DragValue::new(&mut c.count).range(2..=http_tests::MAX_CONCURRENCY))
                    .changed();
                ui.end_row();
                ui.label("Rounds")
                    .on_hover_text("Repeat the whole burst to catch races that happen rarely");
                changed |= ui
                    .add(egui::DragValue::new(&mut c.rounds).range(1..=http_tests::MAX_ROUNDS))
                    .changed();
                ui.end_row();
                ui.label("Release together").on_hover_text(
                    "All requests wait at a barrier and are sent at the same moment",
                );
                changed |= ui.checkbox(&mut c.barrier, "").changed();
                ui.end_row();
            });
        weak(
            ui,
            "Values extracted from the n-th successful response are also available as {{name#n}}; a linked cleanup step runs once per created record.",
        );
    }
    changed
}

/// Hasil langkah DB / concurrent di bagian "Last run".
fn result_evidence(ui: &mut egui::Ui, r: &StepResult) {
    if let Some(db) = &r.db {
        ui.add_space(4.0);
        weak(
            ui,
            format!(
                "{} row(s) from {}{}",
                db.row_count,
                if db.connection.is_empty() {
                    "-"
                } else {
                    db.connection.as_str()
                },
                if db.attempts > 1 {
                    format!(
                        ", {} queries over {}{}",
                        db.attempts,
                        report::fmt_ms(db.waited_ms),
                        if db.timed_out { " (timed out)" } else { "" }
                    )
                } else {
                    String::new()
                }
            ),
        );
        if !db.columns.is_empty() && !db.sample.is_empty() {
            egui::ScrollArea::horizontal()
                .id_salt("it_db_sample")
                .max_height(160.0)
                .show(ui, |ui| {
                    egui::Grid::new("it_db_rows").striped(true).show(ui, |ui| {
                        for c in &db.columns {
                            ui.label(egui::RichText::new(c).small().strong());
                        }
                        ui.end_row();
                        for row in db.sample.iter().take(10) {
                            for v in row {
                                ui.label(
                                    egui::RichText::new(crate::repo_scan::truncate_chars(v, 40))
                                        .small()
                                        .family(egui::FontFamily::Monospace),
                                );
                            }
                            ui.end_row();
                        }
                    });
                });
        }
    }
    if !r.instances.is_empty() {
        let ok = r
            .instances
            .iter()
            .filter(|i| i.error.is_none() && (200..300).contains(&i.status))
            .count();
        ui.add_space(4.0);
        weak(
            ui,
            format!(
                "{} parallel request(s), {ok} accepted (2xx)",
                r.instances.len()
            ),
        );
        ui.horizontal_wrapped(|ui| {
            for i in &r.instances {
                let color = if i.error.is_some() || i.status >= 500 {
                    style::theme_danger(ui.ctx())
                } else if (200..300).contains(&i.status) {
                    style::theme_success(ui.ctx())
                } else {
                    style::theme_warning(ui.ctx())
                };
                ui.label(
                    egui::RichText::new(if i.error.is_some() {
                        "ERR".to_string()
                    } else {
                        i.status.to_string()
                    })
                    .small()
                    .family(egui::FontFamily::Monospace)
                    .color(color),
                )
                .on_hover_text(format!(
                    "Round {} request {}: {}",
                    i.round,
                    i.index,
                    i.error.as_deref().unwrap_or(&i.body_preview)
                ));
            }
        });
    }
}

fn data_flow_section(
    ui: &mut egui::Ui,
    st: &IntegrationTestTabState,
    live: &Live,
    deps: &[http_tests::StepDeps],
    pos: &HashMap<usize, usize>,
    i: usize,
) {
    section_header(
        ui,
        ic::ICON_LINK.codepoint,
        "Data flow",
        Some("Values this step needs from earlier steps and passes on to later ones."),
    );
    let d = &deps[i];
    let target = http_tests::cleanup_target(&st.suite, deps, i);
    if d.uses.is_empty() && d.provides.is_empty() && target.is_none() {
        weak(ui, "This step does not use or pass on any values.");
        return;
    }
    let line = |ui: &mut egui::Ui, icon: &str, text: String, color: Option<egui::Color32>| {
        ui.horizontal(|ui| {
            let c = color.unwrap_or_else(|| ui.visuals().text_color());
            ui.label(egui::RichText::new(icon).size(13.0).color(c));
            ui.label(egui::RichText::new(text).small().color(c));
        });
    };
    for v in &d.uses {
        let from = d
            .depends_on
            .iter()
            .find(|&&p| deps[p].provides.contains(v))
            .map(|&p| format!("step {} '{}'", pos[&p] + 1, st.suite.steps[p].name));
        let text = match from {
            Some(f) => Some(format!("Needs {{{{{v}}}}} from {f}")),
            None if v == "run_id" => Some(format!("Needs {{{{{v}}}}}, unique for every run")),
            None if st.suite.variables.iter().any(|(k, _)| k == v) => {
                Some(format!("Needs {{{{{v}}}}} from the suite variables"))
            }
            None if live.known_vars.contains(v) => {
                Some(format!("Needs {{{{{v}}}}} from the environment"))
            }
            None => None,
        };
        match text {
            Some(t) => line(ui, ic::ICON_INPUT.codepoint, t, None),
            None => line(
                ui,
                ic::ICON_ERROR.codepoint,
                format!("Needs {{{{{v}}}}}, but no earlier step or variable provides it"),
                Some(style::theme_danger(ui.ctx())),
            ),
        }
    }
    for v in &d.provides {
        let users: Vec<String> = d
            .used_by
            .iter()
            .filter(|&&u| deps[u].uses.contains(v))
            .map(|&u| (pos[&u] + 1).to_string())
            .collect();
        let text = if users.is_empty() {
            format!("Gives {{{{{v}}}}}, not used by any step yet")
        } else {
            format!("Gives {{{{{v}}}}} to step {}", users.join(", "))
        };
        line(ui, ic::ICON_OUTPUT.codepoint, text, None);
    }
    if let Some(t) = target {
        line(
            ui,
            ic::ICON_CLEANING_SERVICES.codepoint,
            format!(
                "Removes the data created by step {} '{}'",
                pos[&t] + 1,
                st.suite.steps[t].name
            ),
            None,
        );
    }
}

fn extract_section(ui: &mut egui::Ui, step: &mut HttpTestStep) -> bool {
    let mut changed = false;
    section_header(
        ui,
        ic::ICON_OUTPUT.codepoint,
        "Save values for later steps",
        Some("Later steps use a saved value as {{name}} in the URL, headers or body."),
    );
    let mut remove: Option<usize> = None;
    if step.extract.is_empty() {
        weak(
            ui,
            "Nothing saved. Add a value when a later step needs, for example, the id this step creates.",
        );
    } else {
        egui::Grid::new("it_extract")
            .num_columns(4)
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                weak(ui, "Name");
                ui.label("");
                ui.horizontal(|ui| {
                    weak(ui, "Taken from");
                    help_icon(ui, FIELD_HELP);
                });
                ui.label("");
                ui.end_row();
                for (n, ex) in step.extract.iter_mut().enumerate() {
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut ex.var)
                                .desired_width(110.0)
                                .hint_text("po_id"),
                        )
                        .changed();
                    ui.label(
                        egui::RichText::new(ic::ICON_ARROW_BACK.codepoint)
                            .size(13.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut ex.from)
                                .desired_width(170.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("json:$.id"),
                        )
                        .changed();
                    if icon_button(ui, ic::ICON_CLOSE.codepoint, "Remove this value").clicked() {
                        remove = Some(n);
                    }
                    ui.end_row();
                }
            });
    }
    if let Some(n) = remove {
        step.extract.remove(n);
        changed = true;
    }
    ui.add_space(2.0);
    if ui
        .small_button(format!("{} Add value", ic::ICON_ADD.codepoint))
        .clicked()
    {
        step.extract.push(Extract {
            var: String::new(),
            from: "json:$.id".into(),
        });
        changed = true;
    }
    changed
}

fn assertions_section(
    ui: &mut egui::Ui,
    step: &mut HttpTestStep,
    result: Option<&StepResult>,
) -> bool {
    let mut changed = false;
    section_header(
        ui,
        ic::ICON_RULE.codepoint,
        "Expected result",
        Some("The step passes only when every check below holds."),
    );
    let help = field_help(step);
    let (target_hint, value_hint, new_check) = if step.is_db() {
        ("row_count", "0", ("row_count", "0"))
    } else if step.concurrent().is_some() {
        ("count:status=2xx", "1", ("count:status=2xx", "1"))
    } else {
        ("status", "200", ("status", "200"))
    };
    let mut remove: Option<usize> = None;
    if step.assertions.is_empty() {
        ui.label(
            egui::RichText::new("No checks yet. Without checks, any answer counts as passed.")
                .small()
                .color(style::theme_warning(ui.ctx())),
        );
    } else {
        egui::Grid::new("it_asserts")
            .num_columns(5)
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                ui.label("");
                ui.horizontal(|ui| {
                    weak(ui, "Field");
                    help_icon(ui, help);
                });
                weak(ui, "Condition");
                weak(ui, "Value");
                ui.label("");
                ui.end_row();
                for (n, a) in step.assertions.iter_mut().enumerate() {
                    match result.and_then(|r| r.assertions.get(n)) {
                        Some(r) => {
                            let (g, c, tip) = if r.passed {
                                (
                                    ic::ICON_CHECK_CIRCLE.codepoint,
                                    style::theme_success(ui.ctx()),
                                    "Held in the last run",
                                )
                            } else {
                                (
                                    ic::ICON_CANCEL.codepoint,
                                    style::theme_danger(ui.ctx()),
                                    "Did not hold in the last run",
                                )
                            };
                            ui.label(egui::RichText::new(g).color(c))
                                .on_hover_text(format!(
                                    "{tip}. Actual: {}",
                                    r.actual.clone().unwrap_or_else(|| "(missing)".into())
                                ));
                        }
                        None => {
                            ui.label("");
                        }
                    }
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut a.target)
                                .desired_width(110.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text(target_hint),
                        )
                        .changed();
                    egui::ComboBox::from_id_salt(("it_op", n))
                        .selected_text(op_text(a.op))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for op in AssertOp::ALL {
                                changed |=
                                    ui.selectable_value(&mut a.op, op, op_text(op)).changed();
                            }
                        });
                    if matches!(a.op, AssertOp::Exists | AssertOp::NotExists) {
                        ui.label("");
                    } else {
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut a.value)
                                    .desired_width(80.0)
                                    .hint_text(value_hint),
                            )
                            .changed();
                    }
                    if icon_button(ui, ic::ICON_CLOSE.codepoint, "Remove this check").clicked() {
                        remove = Some(n);
                    }
                    ui.end_row();
                }
            });
    }
    if let Some(n) = remove {
        step.assertions.remove(n);
        changed = true;
    }
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if ui
            .small_button(format!("{} Add check", ic::ICON_ADD.codepoint))
            .clicked()
        {
            step.assertions.push(Assertion {
                target: new_check.0.into(),
                op: AssertOp::Equals,
                value: new_check.1.into(),
            });
            changed = true;
        }
        if let Some(r) = result.filter(|r| r.executed() && r.db.is_none() && r.instances.is_empty())
            && ui
                .small_button("Use last response")
                .on_hover_text(
                    "Replace the checks with the last status and the top-level JSON fields",
                )
                .clicked()
        {
            step.assertions = assertions_from_response(r);
            changed = true;
        }
    });
    changed
}

/// `true` = user meminta perbaikan kode untuk langkah ini.
fn last_result_section(ui: &mut egui::Ui, r: &StepResult, live: &Live) -> bool {
    section_header(ui, ic::ICON_ASSESSMENT.codepoint, "Last run", None);
    let o = r.outcome();
    let failed = matches!(o, StepOutcome::Failed | StepOutcome::Error);
    let mut fix = false;
    style::theme_card_frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            outcome_badge(ui, o, 15.0);
            if r.status > 0 {
                weak(
                    ui,
                    format!("HTTP {} · {}", r.status, report::fmt_ms(r.time_ms)),
                );
            }
        });
        weak(ui, outcome_help(o));
        if let Some(n) = &r.note {
            ui.label(egui::RichText::new(n).small());
        }
        if let Some(e) = &r.error {
            ui.label(
                egui::RichText::new(e)
                    .small()
                    .color(style::theme_danger(ui.ctx())),
            );
        }
        mismatch_grid(ui, r);
        if !r.unresolved.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "Missing values: {}",
                    r.unresolved.join(", ")
                ))
                .small()
                .color(style::theme_warning(ui.ctx())),
            );
        }
        if failed && live.codefix_status.is_none() && live.proposal.is_none() {
            ui.add_space(4.0);
            if ui
                .button(format!(
                    "{} Fix API code with AI…",
                    ic::ICON_AUTO_AWESOME.codepoint
                ))
                .on_hover_text(
                    "When the test is right and the API is wrong: AI changes the API code in a separate git branch for you to review",
                )
                .clicked()
            {
                fix = true;
            }
        }
    });
    if !r.extracted.is_empty() {
        egui::CollapsingHeader::new(format!("Saved values ({})", r.extracted.len()))
            .id_salt(("it_saved", r.step_id.as_str()))
            .show(ui, |ui| {
                egui::Grid::new(("it_saved_grid", r.step_id.as_str()))
                    .num_columns(2)
                    .spacing([10.0, 3.0])
                    .show(ui, |ui| {
                        for (k, v) in &r.extracted {
                            kv_row(ui, k, crate::repo_scan::truncate_chars(v, 60));
                        }
                    });
            });
    }
    if !r.url.is_empty() {
        egui::CollapsingHeader::new("Request sent")
            .id_salt(("it_sent", r.step_id.as_str()))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("{} {}", r.method, r.url))
                        .family(egui::FontFamily::Monospace)
                        .size(11.0),
                );
            });
    }
    if !r.body_preview.is_empty() {
        egui::CollapsingHeader::new("Response body")
            .id_salt(("it_body_h", r.step_id.as_str()))
            .default_open(failed)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("it_body")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        let mut body = r.body_preview.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut body)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
            });
    }
    fix
}

/// Arti setiap status, untuk tooltip dan detail langkah.
fn outcome_help(o: StepOutcome) -> &'static str {
    match o {
        StepOutcome::Passed => "Passed: the request was sent and every assertion held.",
        StepOutcome::Failed => {
            "Failed: the request was sent and the server answered, but the answer was not what the step expects (status, body or a value to extract)."
        }
        StepOutcome::Error => {
            "Error: the request was sent but no answer came back (connection or timeout)."
        }
        StepOutcome::Blocked => {
            "Blocked: not sent, because something it needs is missing, usually a value an earlier step should have extracted. Fix the earlier failed step and this one runs."
        }
        StepOutcome::Skipped => "Skipped: disabled, or not needed in this run.",
    }
}

/// Langkah yang Failed/Error di run terakhir.
fn failed_step_ids(st: &IntegrationTestTabState) -> Vec<String> {
    st.last_run
        .as_ref()
        .map(|r| {
            r.results
                .iter()
                .filter(|x| matches!(x.outcome(), StepOutcome::Failed | StepOutcome::Error))
                .map(|x| x.step_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Dialog sebelum meminta AI memperbaiki kode API.
fn codefix_dialog(ctx: &egui::Context, st: &mut IntegrationTestTabState, acts: &mut Vec<Act>) {
    let Some((ids, mut note)) = st.codefix_dialog.take() else {
        return;
    };
    let names: Vec<String> = ids
        .iter()
        .filter_map(|id| st.suite.steps.iter().find(|s| &s.id == id))
        .map(|s| s.name.clone())
        .collect();
    let mut decided: Option<bool> = None;
    style::render_modal_backdrop(ctx, "it_codefix_dialog", true);
    egui::Window::new("Fix API code with AI")
        .id(egui::Id::new("it_codefix_dialog_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(format!(
                "The AI reads the failing request{}, the server's answer and the handler code, then changes the API code so the step works.",
                if names.len() == 1 { "" } else { "s" }
            ));
            for n in names.iter().take(8) {
                weak(ui, format!("• {n}"));
            }
            if names.len() > 8 {
                weak(ui, format!("and {} more", names.len() - 8));
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Your branch and files are not touched: the change is prepared in a separate git branch (tabular/fix-…) with your uncommitted changes, and you review the diff before applying it.",
                )
                .small(),
            );
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::multiline(&mut note)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("Optional note for the AI, e.g. \"slug must be unique per commodity\""),
            );
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                if ui
                    .add(crate::diagram_repo::accent_button(ui, "Propose fix"))
                    .clicked()
                {
                    decided = Some(true);
                }
            });
        });
    match decided {
        Some(true) => acts.push(Act::CodeFix(ids, note)),
        Some(false) => {}
        None => st.codefix_dialog = Some((ids, note)),
    }
}

/// Jendela tinjau usulan perbaikan kode: ringkasan, diff, cek build, keputusan.
fn proposal_window(ctx: &egui::Context, live: &Live, acts: &mut Vec<Act>) {
    let Some(p) = &live.proposal else {
        return;
    };
    // Tanpa tombol tutup: usulan hanya selesai lewat Apply, Keep, atau Discard.
    egui::Window::new("Review code fix")
        .id(egui::Id::new(("it_proposal", p.branch.as_str())))
        .collapsible(true)
        .resizable(true)
        .default_size(egui::vec2(860.0, 620.0))
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(p.summary.trim()).strong());
            weak(
                ui,
                format!(
                    "{} · branch {} · {} file(s): {}",
                    p.repo.display(),
                    p.branch,
                    p.files.len(),
                    p.files.join(", ")
                ),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(crate::diagram_repo::accent_button(ui, "Apply to working copy"))
                    .on_hover_text("Write the change into your repository without committing. Restart the API server afterwards.")
                    .clicked()
                {
                    acts.push(Act::Proposal(jobs::ProposalAction::Apply));
                }
                if ui
                    .button("Keep as branch")
                    .on_hover_text("Commit the change on the tabular/fix-… branch only, to review or merge later")
                    .clicked()
                {
                    acts.push(Act::Proposal(jobs::ProposalAction::KeepBranch));
                }
                if let Some(cmd) = &p.check_command {
                    let running = live.check.as_ref().is_some_and(|(r, _)| *r);
                    if ui
                        .add_enabled(!running, egui::Button::new(format!("Run {}", cmd.join(" "))))
                        .on_hover_text("Build-check the change in the separate branch")
                        .clicked()
                    {
                        acts.push(Act::Proposal(jobs::ProposalAction::Check));
                    }
                }
                if ui.add(style::btn_danger_ctx(ui.ctx(), "Discard")).clicked() {
                    acts.push(Act::Proposal(jobs::ProposalAction::Discard));
                }
            });
            if let Some((running, result)) = &live.check {
                ui.add_space(4.0);
                if *running {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        weak(ui, "Checking the build…");
                    });
                } else if let Some(r) = result {
                    let (ok, text) = match r {
                        Ok(t) => (true, t),
                        Err(t) => (false, t),
                    };
                    ui.label(
                        egui::RichText::new(if ok { "Build check passed" } else { "Build check failed" })
                            .strong()
                            .color(if ok {
                                style::theme_success(ui.ctx())
                            } else {
                                style::theme_danger(ui.ctx())
                            }),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("it_check_out")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(crate::repo_scan::truncate_chars(text, 4000))
                                    .family(egui::FontFamily::Monospace)
                                    .size(11.0),
                            );
                        });
                }
            }
            ui.separator();
            egui::ScrollArea::both()
                .id_salt("it_diff")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let add = style::theme_success(ui.ctx());
                    let del = style::theme_danger(ui.ctx());
                    let meta = style::theme_info(ui.ctx());
                    for line in p.diff.lines().take(4000) {
                        let color = if line.starts_with("+++") || line.starts_with("---") {
                            meta
                        } else if line.starts_with('+') {
                            add
                        } else if line.starts_with('-') {
                            del
                        } else if line.starts_with("@@") || line.starts_with("diff ") {
                            meta
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.label(
                            egui::RichText::new(line)
                                .family(egui::FontFamily::Monospace)
                                .size(11.5)
                                .color(color),
                        );
                    }
                });
        });
}

/// Assertion dari response terakhir: status + field JSON tingkat atas.
fn assertions_from_response(r: &StepResult) -> Vec<Assertion> {
    let mut out = vec![Assertion {
        target: "status".into(),
        op: AssertOp::Equals,
        value: r.status.to_string(),
    }];
    if let Ok(serde_json::Value::Object(map)) =
        serde_json::from_str::<serde_json::Value>(&r.body_preview)
    {
        for k in map.keys().take(5) {
            out.push(Assertion {
                target: format!("json:$.{k}"),
                op: AssertOp::Exists,
                value: String::new(),
            });
        }
    }
    out
}

fn suite_overview(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, live: &Live) {
    section_header(ui, ic::ICON_DESCRIPTION.codepoint, "About this suite", None);
    if ui
        .add(
            egui::TextEdit::multiline(&mut st.suite.description)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Purpose and scope of this suite"),
        )
        .changed()
    {
        st.mark();
    }
    let steps = &st.suite.steps;
    let count = |p: StepPhase| steps.iter().filter(|s| s.phase == p).count();
    ui.add_space(4.0);
    egui::Grid::new("it_suite_counts")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            kv_row(ui, "Setup steps", count(StepPhase::Setup).to_string());
            kv_row(ui, "Test steps", count(StepPhase::Test).to_string());
            kv_row(ui, "Cleanup steps", count(StepPhase::Cleanup).to_string());
            kv_row(ui, "Groups", st.suite.group_names().len().to_string());
            kv_row(
                ui,
                "Steps that change data",
                st.suite.mutating_steps().to_string(),
            );
        });

    section_header(ui, ic::ICON_ASSESSMENT.codepoint, "Last run", None);
    if live.running {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(13.0));
            ui.label(egui::RichText::new(&live.progress).small());
        });
    }
    match &st.last_run {
        Some(run) => {
            if run_summary_card(ui, run) {
                st.view = View::Report;
            }
        }
        None => weak(ui, "Not run yet."),
    }

    section_header(ui, ic::ICON_HELP_OUTLINE.codepoint, "How to use", None);
    for (n, text) in [
        "Add steps from the API collection, or describe the flow to the AI.",
        "Click a step to say what a good answer looks like and which values it passes on.",
        "Press Run all. Cleanup steps always run at the end and remove the test data.",
        "Open Report to see what failed and why.",
    ]
    .iter()
    .enumerate()
    {
        ui.horizontal_top(|ui| {
            ui.label(
                egui::RichText::new(format!("{}.", n + 1))
                    .small()
                    .strong()
                    .color(style::theme_muted_text(ui.ctx())),
            );
            ui.add(egui::Label::new(egui::RichText::new(*text).small()).wrap());
        });
    }
    ui.add_space(4.0);
    weak(
        ui,
        "Double-click a step to edit its request in the HTTP client.",
    );
}

/// Kartu ringkas run terakhir; `true` = user ingin membuka laporan.
fn run_summary_card(ui: &mut egui::Ui, run: &TestRun) -> bool {
    let s = report::compute(run);
    let mut open = false;
    style::theme_card_frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(s.verdict().label())
                    .size(17.0)
                    .strong()
                    .color(verdict_color(ui.ctx(), s.verdict())),
            );
            weak(
                ui,
                format!(
                    "{} · {}",
                    run.started_at,
                    run.environment.clone().unwrap_or_default()
                ),
            );
        });
        ui.add(egui::Label::new(egui::RichText::new(run_sentence(run, &s)).small()).wrap());
        ui.add_space(4.0);
        if ui
            .button(format!("{} Open report", ic::ICON_ASSESSMENT.codepoint))
            .clicked()
        {
            open = true;
        }
    });
    open
}

// ─── Report & riwayat ───────────────────────────────────────────────────────

fn metric_card(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    caption: String,
    color: Option<egui::Color32>,
) {
    style::theme_card_frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        weak(ui, label);
        let mut rt = egui::RichText::new(value).size(22.0).strong();
        if let Some(c) = color {
            rt = rt.color(c);
        }
        ui.label(rt);
        weak(ui, caption);
    });
}

/// Baris "Expected / Got" untuk setiap pemeriksaan yang gagal.
fn mismatch_grid(ui: &mut egui::Ui, r: &StepResult) {
    let misses: Vec<_> = r.assertions.iter().filter(|a| !a.passed).collect();
    if misses.is_empty() {
        return;
    }
    ui.add_space(4.0);
    egui::Grid::new(("it_misses", r.step_id.as_str(), r.index))
        .num_columns(2)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            for a in misses {
                kv_row(ui, "Expected", a.assertion.describe());
                ui.label(
                    egui::RichText::new("Got")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.label(
                    egui::RichText::new(crate::repo_scan::truncate_chars(
                        a.actual.as_deref().unwrap_or("(missing)"),
                        120,
                    ))
                    .small()
                    .color(style::theme_danger(ui.ctx())),
                );
                ui.end_row();
            }
        });
}

/// Kartu judul laporan: verdict, kalimat ringkas, metadata, ekspor.
fn report_summary(ui: &mut egui::Ui, run: &TestRun, s: &report::RunStats, acts: &mut Vec<Act>) {
    style::theme_card_frame(ui.ctx()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(s.verdict().label())
                    .size(22.0)
                    .strong()
                    .color(verdict_color(ui.ctx(), s.verdict())),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(
                    format!("{} Export", ic::ICON_FILE_DOWNLOAD.codepoint),
                    |ui| {
                        for f in ReportFormat::ALL {
                            if ui.button(f.label()).clicked() {
                                acts.push(Act::Export(f));
                                ui.close();
                            }
                        }
                    },
                )
                .response
                .on_hover_text("Save the report as a file, e.g. for CI or sign-off");
                if ui
                    .button(format!(
                        "{} Open full report",
                        ic::ICON_DESCRIPTION.codepoint
                    ))
                    .on_hover_text(
                        "HTML report in your browser; print it to PDF for sign-off. \
                         Statistics follow ISTQB terms and the ISO/IEC/IEEE 29119-3 completion report; \
                         evidence follows GAMP 5 practice.",
                    )
                    .clicked()
                {
                    acts.push(Act::OpenHtmlReport);
                }
            });
        });
        ui.add(egui::Label::new(run_sentence(run, s)).wrap());
        ui.add_space(4.0);
        let scope = if run.groups.is_empty() {
            "all groups".to_string()
        } else {
            format!("groups: {}", run.groups.join(", "))
        };
        weak(
            ui,
            format!(
                "Started {} · {} · {} · by {} · took {}",
                run.started_at,
                run.environment
                    .clone()
                    .unwrap_or_else(|| "no environment".into()),
                scope,
                run.operator,
                report::fmt_ms(run.duration_ms)
            ),
        );
    });
}

fn report_view(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    let Some(run) = st.last_run.clone() else {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(ic::ICON_ASSESSMENT.codepoint)
                    .size(34.0)
                    .color(style::theme_muted_text(ui.ctx())),
            );
            ui.label(egui::RichText::new("No report yet").strong().size(16.0));
            weak(
                ui,
                "Run the suite to see which steps pass, which fail and why.",
            );
            ui.add_space(8.0);
            if !live.running
                && st.suite.steps.iter().any(|s| s.enabled)
                && ui
                    .add(crate::diagram_repo::accent_button(
                        ui,
                        format!("{} Run all", ic::ICON_PLAY_ARROW.codepoint),
                    ))
                    .clicked()
            {
                st.confirm_run = Some(Vec::new());
            }
        });
        return;
    };
    let s = report::compute(&run);
    let catalog: Vec<SavedRequest> = live
        .catalogs
        .iter()
        .flat_map(|c| c.requests.clone())
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("it_report")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 8,
                    bottom: 20,
                })
                .show(ui, |ui| {
                    report_summary(ui, &run, &s, acts);
                    ui.add_space(10.0);
                    report_metrics(ui, &run, &s);
                    report_failures(ui, st, live, &run);
                    report_steps(ui, st, &run);
                    report_groups(ui, &s);
                    report_cleanup(ui, &run, &s);
                    if !catalog.is_empty() {
                        let cov = report::coverage(&st.suite, &catalog);
                        section_header(
                            ui,
                            ic::ICON_INSIGHTS.codepoint,
                            "Endpoint coverage",
                            Some("How many of the project's API endpoints this suite tests."),
                        );
                        ui.label(format!(
                            "{} of {} endpoints are tested ({:.1}%).",
                            cov.covered,
                            cov.catalog,
                            cov.percent()
                        ));
                        if !cov.untested.is_empty() {
                            egui::CollapsingHeader::new(format!(
                                "{} endpoint(s) without a test",
                                cov.untested.len()
                            ))
                            .id_salt("it_untested")
                            .show(ui, |ui| {
                                for u in cov.untested.iter().take(300) {
                                    ui.label(
                                        egui::RichText::new(u)
                                            .family(egui::FontFamily::Monospace)
                                            .size(11.0),
                                    );
                                }
                            });
                        }
                    }
                    let flaky = report::flakiness(&st.history);
                    if !flaky.is_empty() {
                        section_header(
                            ui,
                            ic::ICON_BOLT.codepoint,
                            "Unstable steps",
                            Some("These passed in some recent runs and failed in others."),
                        );
                        egui::Grid::new("it_flaky")
                            .num_columns(2)
                            .spacing([16.0, 4.0])
                            .show(ui, |ui| {
                                for f in flaky {
                                    ui.label(egui::RichText::new(&f.name).small());
                                    weak(ui, format!("passed {} of {} runs", f.passes, f.runs));
                                    ui.end_row();
                                }
                            });
                    }
                });
        });
}

/// Empat angka utama + statistik lain yang bisa dibuka.
fn report_metrics(ui: &mut egui::Ui, run: &TestRun, s: &report::RunStats) {
    let ctx = ui.ctx().clone();
    let t = &s.tests;
    let failed = t.failed + t.error;
    let rate_ok = t.pass_rate() >= s.threshold;
    ui.columns(4, |c| {
        metric_card(
            &mut c[0],
            "Passed",
            t.passed.to_string(),
            format!("of {} test step(s)", t.planned - t.skipped),
            Some(style::theme_success(&ctx)),
        );
        metric_card(
            &mut c[1],
            "Failed",
            failed.to_string(),
            if t.error > 0 {
                format!("{} got no response", t.error)
            } else {
                "answer was not as expected".to_string()
            },
            (failed > 0).then(|| style::theme_danger(&ctx)),
        );
        metric_card(
            &mut c[2],
            "Not sent",
            t.blocked.to_string(),
            "an earlier step failed".to_string(),
            (t.blocked > 0).then(|| style::theme_warning(&ctx)),
        );
        metric_card(
            &mut c[3],
            "Pass rate",
            format!("{:.1}%", t.pass_rate()),
            format!("criterion {:.0}%", s.threshold),
            Some(if rate_ok {
                style::theme_success(&ctx)
            } else {
                style::theme_danger(&ctx)
            }),
        );
    });
    ui.add_space(6.0);
    egui::CollapsingHeader::new("More statistics")
        .id_salt("it_more_stats")
        .show(ui, |ui| {
            egui::Grid::new("it_stats")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    kv_row(ui, "Test steps planned", t.planned.to_string());
                    kv_row(
                        ui,
                        "Executed",
                        format!("{} ({:.1}%)", t.executed + t.error, t.execution_rate()),
                    );
                    kv_row(ui, "Skipped (disabled)", t.skipped.to_string());
                    kv_row(
                        ui,
                        "Checks that held",
                        format!("{} of {}", s.assertions_passed, s.assertions_total),
                    );
                    kv_row(
                        ui,
                        "Response time p50 / p95",
                        format!(
                            "{} / {}",
                            report::fmt_ms(s.latency.p50),
                            report::fmt_ms(s.latency.p95)
                        ),
                    );
                    kv_row(
                        ui,
                        "Cleanup steps passed",
                        format!(
                            "{} of {}",
                            s.cleanup.passed,
                            s.cleanup.planned - s.cleanup.skipped
                        ),
                    );
                    kv_row(ui, "Total duration", report::fmt_ms(run.duration_ms));
                    kv_row(ui, "Run id", run.run_id.clone());
                    if !run.targets.is_empty() {
                        kv_row(ui, "Servers", run.targets.join(", "));
                    }
                    if !run.app_version.is_empty() {
                        kv_row(ui, "App version", run.app_version.clone());
                    }
                });
            weak(
                ui,
                "Pass rate = passed / executed. Statistics follow ISTQB terms and ISO/IEC/IEEE 29119-3.",
            );
        });
}

/// Langkah yang gagal, error atau diblokir, satu kartu per langkah.
fn report_failures(
    ui: &mut egui::Ui,
    st: &mut IntegrationTestTabState,
    live: &Live,
    run: &TestRun,
) {
    let failures: Vec<&StepResult> = run
        .results
        .iter()
        .filter(|r| {
            matches!(
                r.outcome(),
                StepOutcome::Failed | StepOutcome::Error | StepOutcome::Blocked
            )
        })
        .collect();
    if failures.is_empty() {
        return;
    }
    section_header(
        ui,
        ic::ICON_ERROR.codepoint,
        &format!("Needs attention ({})", failures.len()),
        Some(
            "Fix the failed steps first. Steps marked Not sent were blocked by an earlier failure and run again once it passes.",
        ),
    );
    let ids = failed_step_ids(st);
    if !ids.is_empty()
        && live.codefix_status.is_none()
        && live.proposal.is_none()
        && ui
            .button(format!(
                "{} Fix API code with AI for {} step(s)…",
                ic::ICON_AUTO_AWESOME.codepoint,
                ids.len()
            ))
            .on_hover_text(
                "When the tests are right and the API is wrong: AI changes the API code in a separate git branch for you to review",
            )
            .clicked()
    {
        st.codefix_dialog = Some((ids, String::new()));
    }
    ui.add_space(4.0);
    for r in failures {
        let o = r.outcome();
        style::theme_card_frame(ui.ctx()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                outcome_badge(ui, o, 12.5);
                if ui
                    .link(egui::RichText::new(&r.name).strong())
                    .on_hover_text("Show this step")
                    .clicked()
                {
                    st.selected = Some(r.step_id.clone());
                    st.view = View::Steps;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if r.status > 0 {
                        weak(
                            ui,
                            format!("HTTP {} · {}", r.status, report::fmt_ms(r.time_ms)),
                        );
                    }
                });
            });
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    r.method,
                    crate::repo_scan::truncate_chars(&r.url, 110)
                ))
                .family(egui::FontFamily::Monospace)
                .size(10.5)
                .color(ui.visuals().weak_text_color()),
            );
            if let Some(n) = &r.note {
                ui.add(egui::Label::new(egui::RichText::new(n).small()).wrap());
            }
            if let Some(e) = &r.error {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(e)
                            .small()
                            .color(style::theme_danger(ui.ctx())),
                    )
                    .wrap(),
                );
            }
            mismatch_grid(ui, r);
            if !r.unresolved.is_empty() {
                ui.label(
                    egui::RichText::new(format!("Missing values: {}", r.unresolved.join(", ")))
                        .small()
                        .color(style::theme_warning(ui.ctx())),
                );
            }
        });
        ui.add_space(6.0);
    }
}

/// Tabel hasil semua langkah, dikelompokkan per blok fase + group.
fn report_steps(ui: &mut egui::Ui, st: &mut IntegrationTestTabState, run: &TestRun) {
    if run.results.is_empty() {
        return;
    }
    section_header(
        ui,
        ic::ICON_LIST.codepoint,
        "All steps",
        Some("Every step of this run in execution order. Click a name to open the step."),
    );
    egui::Grid::new("it_rep_steps")
        .striped(true)
        .num_columns(6)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for h in ["#", "Step", "Method", "Result", "HTTP", "Time"] {
                ui.label(
                    egui::RichText::new(h)
                        .small()
                        .strong()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.end_row();
            let mut last: Option<(StepPhase, String)> = None;
            for (n, r) in run.results.iter().enumerate() {
                let key = block_key(r.phase, &r.group);
                if last.as_ref() != Some(&key) {
                    ui.label("");
                    ui.label(
                        egui::RichText::new(block_title(r.phase, &r.group))
                            .small()
                            .strong()
                            .color(phase_color(ui.ctx(), r.phase)),
                    );
                    for _ in 0..4 {
                        ui.label("");
                    }
                    ui.end_row();
                    last = Some(key);
                }
                weak(ui, (n + 1).to_string());
                if ui.link(&r.name).clicked() {
                    st.selected = Some(r.step_id.clone());
                    st.view = View::Steps;
                }
                crate::http_repo::method_chip(ui, &r.method);
                let o = r.outcome();
                let details = report::step_details_text(r);
                outcome_badge(ui, o, 12.0).on_hover_text(if details.is_empty() {
                    outcome_help(o).to_string()
                } else {
                    details
                });
                weak(
                    ui,
                    if r.status > 0 {
                        r.status.to_string()
                    } else {
                        "-".to_string()
                    },
                );
                weak(
                    ui,
                    if r.executed() || r.error.is_some() {
                        report::fmt_ms(r.time_ms)
                    } else {
                        String::new()
                    },
                );
                ui.end_row();
            }
        });
}

fn report_groups(ui: &mut egui::Ui, s: &report::RunStats) {
    if s.groups.is_empty() {
        return;
    }
    section_header(ui, ic::ICON_FOLDER.codepoint, "By group", None);
    egui::Grid::new("it_rep_groups")
        .striped(true)
        .num_columns(7)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for h in [
                "Group",
                "Steps",
                "Passed",
                "Failed",
                "Not sent",
                "Pass rate",
                "Time",
            ] {
                ui.label(
                    egui::RichText::new(h)
                        .small()
                        .strong()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.end_row();
            for g in &s.groups {
                let c = &g.counts;
                let failed = c.failed + c.error;
                ui.label(group_label(&g.group));
                ui.label(c.planned.to_string());
                ui.label(
                    egui::RichText::new(c.passed.to_string()).color(style::theme_success(ui.ctx())),
                );
                let fc = if failed > 0 {
                    style::theme_danger(ui.ctx())
                } else {
                    ui.visuals().text_color()
                };
                ui.label(egui::RichText::new(failed.to_string()).color(fc));
                ui.label(c.blocked.to_string());
                ui.label(format!("{:.1}%", c.pass_rate()));
                weak(ui, report::fmt_ms(g.time_ms));
                ui.end_row();
            }
        });
}

fn report_cleanup(ui: &mut egui::Ui, run: &TestRun, s: &report::RunStats) {
    section_header(
        ui,
        ic::ICON_CLEANING_SERVICES.codepoint,
        "Test data cleanup",
        Some("Data the run created must be removed again."),
    );
    if s.data_clean() {
        let db = if run.db_checks.is_empty() {
            String::new()
        } else {
            format!(" Database verified in {} table(s).", run.db_checks.len())
        };
        ui.label(
            egui::RichText::new(format!(
                "{} Clean. {} of {} cleanup step(s) passed.{db}",
                ic::ICON_CHECK_CIRCLE.codepoint,
                s.cleanup.passed,
                s.cleanup.planned - s.cleanup.skipped,
            ))
            .color(style::theme_success(ui.ctx())),
        );
        return;
    }
    ui.label(
        egui::RichText::new(format!(
            "{} Some test data was left behind. Remove it by hand or fix the cleanup steps.",
            ic::ICON_WARNING.codepoint
        ))
        .color(style::theme_warning(ui.ctx())),
    );
    if !run.residue.is_empty() {
        ui.add_space(4.0);
        egui::Grid::new("it_residue")
            .striped(true)
            .num_columns(4)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for h in ["Created by", "Request", "Ids", "Why it is left"] {
                    ui.label(
                        egui::RichText::new(h)
                            .small()
                            .strong()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.end_row();
                for r in &run.residue {
                    let ids: Vec<String> = r.ids.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    ui.label(egui::RichText::new(&r.step_name).small());
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}",
                            r.method,
                            crate::repo_scan::truncate_chars(&r.url, 60)
                        ))
                        .family(egui::FontFamily::Monospace)
                        .size(10.5),
                    );
                    ui.label(egui::RichText::new(ids.join(", ")).small());
                    ui.label(egui::RichText::new(&r.reason).small());
                    ui.end_row();
                }
            });
    }
    let leftovers: Vec<_> = run
        .db_checks
        .iter()
        .filter(|c| c.rows.unwrap_or(0) > 0)
        .collect();
    if !leftovers.is_empty() {
        ui.add_space(6.0);
        egui::Grid::new("it_db_left")
            .striped(true)
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for h in ["Table", "Rows still containing the run id"] {
                    ui.label(
                        egui::RichText::new(h)
                            .small()
                            .strong()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.end_row();
                for c in leftovers {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}.{}",
                            ic::ICON_STORAGE.codepoint,
                            c.connection,
                            c.table
                        ))
                        .small(),
                    );
                    ui.label(
                        egui::RichText::new(c.rows.unwrap_or(0).to_string())
                            .small()
                            .color(style::theme_warning(ui.ctx())),
                    );
                    ui.end_row();
                }
            });
        weak(ui, format!("Run id: {}", run.run_id));
    }
}

fn history_view(ui: &mut egui::Ui, st: &mut IntegrationTestTabState) {
    if st.history.is_empty() {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(ic::ICON_HISTORY.codepoint)
                    .size(34.0)
                    .color(style::theme_muted_text(ui.ctx())),
            );
            ui.label(egui::RichText::new("No runs yet").strong().size(16.0));
            weak(ui, "Every run of this suite is kept here with its report.");
        });
        return;
    }
    let mut pick: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("it_history")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 8,
                    bottom: 20,
                })
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Run history").strong().size(14.5));
                    weak(
                        ui,
                        format!(
                            "The last {} run(s) of this suite. Open a run to see its report.",
                            st.history.len()
                        ),
                    );
                    ui.add_space(8.0);
                    egui::Grid::new("it_hist")
                        .striped(true)
                        .num_columns(9)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            for h in [
                                "Started",
                                "Result",
                                "Passed",
                                "Pass rate",
                                "Scope",
                                "Environment",
                                "Test data",
                                "Duration",
                                "",
                            ] {
                                ui.label(
                                    egui::RichText::new(h)
                                        .small()
                                        .strong()
                                        .color(ui.visuals().weak_text_color()),
                                );
                            }
                            ui.end_row();
                            for (n, run) in st.history.iter().enumerate() {
                                let s = report::compute(run);
                                let t = &s.tests;
                                ui.label(&run.started_at);
                                ui.label(
                                    egui::RichText::new(s.verdict().label())
                                        .small()
                                        .strong()
                                        .color(verdict_color(ui.ctx(), s.verdict())),
                                );
                                ui.label(format!("{} of {}", t.passed, t.planned - t.skipped));
                                ui.label(format!("{:.1}%", t.pass_rate()));
                                weak(
                                    ui,
                                    if run.groups.is_empty() {
                                        "All".to_string()
                                    } else {
                                        run.groups.join(", ")
                                    },
                                );
                                weak(ui, run.environment.clone().unwrap_or_default());
                                if s.data_clean() {
                                    ui.label(
                                        egui::RichText::new("Clean")
                                            .small()
                                            .color(style::theme_success(ui.ctx())),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} item(s) left",
                                            s.residue_items + s.db_residue_rows as usize
                                        ))
                                        .small()
                                        .color(style::theme_warning(ui.ctx())),
                                    );
                                }
                                weak(ui, report::fmt_ms(run.duration_ms));
                                if ui
                                    .small_button(format!(
                                        "{} Open report",
                                        ic::ICON_ASSESSMENT.codepoint
                                    ))
                                    .clicked()
                                {
                                    pick = Some(n);
                                }
                                ui.end_row();
                            }
                        });
                });
        });
    if let Some(n) = pick {
        st.last_run = st.history.get(n).cloned();
        st.view = View::Report;
    }
}

// ─── Dialog ─────────────────────────────────────────────────────────────────

fn confirm_run_modal(
    ctx: &egui::Context,
    st: &mut IntegrationTestTabState,
    live: &Live,
    acts: &mut Vec<Act>,
) {
    let Some(groups) = st.confirm_run.clone() else {
        return;
    };
    let scope = RunScope::groups(&st.suite, groups.clone());
    let included: Vec<usize> = (0..st.suite.steps.len())
        .filter(|&i| scope.includes(i) && st.suite.steps[i].enabled)
        .collect();
    let mutating = included
        .iter()
        .filter(|&&i| st.suite.steps[i].phase != StepPhase::Cleanup && st.suite.steps[i].mutates())
        .count();
    let cleanups = included
        .iter()
        .filter(|&&i| st.suite.steps[i].phase == StepPhase::Cleanup)
        .count();
    let outside = if groups.is_empty() {
        0
    } else {
        included
            .iter()
            .filter(|&&i| !groups.contains(&st.suite.steps[i].group))
            .count()
    };
    let issues = http_tests::lint(&st.suite, &live.known_vars);
    let in_scope = |s: Option<usize>| s.is_none_or(|s| scope.includes(s));
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.level == LintLevel::Error && in_scope(i.step))
        .collect();
    let leaks = issues
        .iter()
        .filter(|i| i.level == LintLevel::Warning && i.message.starts_with("Creates data"))
        .filter(|i| in_scope(i.step))
        .count();
    let targets: Vec<String> = st
        .suite
        .variables
        .iter()
        .filter(|(k, v)| k.starts_with("base_url") && !v.is_empty())
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    let title = if groups.is_empty() {
        "Run integration tests".to_string()
    } else {
        format!(
            "Run group {}",
            groups
                .iter()
                .map(|g| group_label(g))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut decided: Option<bool> = None;
    style::render_modal_backdrop(ctx, "it_confirm_run", true);
    egui::Window::new(title)
        .id(egui::Id::new("it_confirm_run_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            let extra = if outside > 0 {
                format!(", including {outside} required step(s) from other groups")
            } else {
                String::new()
            };
            ui.label(format!("{} step(s) will run in order{extra}.", included.len()));
            if mutating > 0 {
                ui.label(format!(
                    "{mutating} step(s) create, change or remove data. {cleanups} cleanup step(s) remove the test data afterwards, even when a test fails."
                ));
            }
            if leaks > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "{leaks} step(s) create data without a cleanup step. Add cleanup first to avoid leaving test data behind."
                    ))
                    .color(style::theme_warning(ui.ctx())),
                );
            }
            ui.label(format!(
                "Environment: {}",
                live.env_name.clone().unwrap_or_else(|| "none".into())
            ));
            for t in &targets {
                ui.label(
                    egui::RichText::new(t)
                        .family(egui::FontFamily::Monospace)
                        .size(11.5),
                );
            }
            if !errors.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} problem(s) will block steps:",
                        errors.len()
                    ))
                    .color(style::theme_danger(ui.ctx())),
                );
                for e in errors.iter().take(5) {
                    weak(ui, e.message.clone());
                }
            }
            if live.env_production {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "The active environment is Production. Integration tests create and delete real data.",
                    )
                    .strong()
                    .color(style::theme_danger(ui.ctx())),
                );
                ui.checkbox(&mut st.allow_production, "I understand, run against Production");
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                let allowed = !live.env_production || st.allow_production;
                if ui
                    .add_enabled(allowed, crate::diagram_repo::accent_button(ui, "Run"))
                    .clicked()
                {
                    decided = Some(true);
                }
            });
        });
    if let Some(yes) = decided {
        st.confirm_run = None;
        // Persetujuan Production juga berlaku untuk langkah DB yang membaca
        // koneksi bertanda Production.
        let allow_production = st.allow_production;
        st.allow_production = false;
        if yes {
            acts.push(Act::Run(groups, allow_production));
        }
    }
}

fn confirm_delete_modal(
    ctx: &egui::Context,
    st: &mut IntegrationTestTabState,
    acts: &mut Vec<Act>,
) {
    if !st.confirm_delete_suite {
        return;
    }
    style::render_modal_backdrop(ctx, "it_confirm_delete", true);
    egui::Window::new("Delete suite?")
        .id(egui::Id::new("it_confirm_delete_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!(
                "Delete '{}' with its {} step(s) and run history? This cannot be undone.",
                st.suite.name,
                st.suite.steps.len()
            ));
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    st.confirm_delete_suite = false;
                }
                if ui.add(style::btn_danger_ctx(ui.ctx(), "Delete")).clicked() {
                    st.confirm_delete_suite = false;
                    acts.push(Act::DeleteSuite);
                }
            });
        });
}

/// Pemilih request dari collection API project.
fn picker_window(ctx: &egui::Context, st: &mut IntegrationTestTabState, live: &Live) {
    let Some(mut query) = st.picker.take() else {
        return;
    };
    let mut open = true;
    let mut chosen: Option<(SavedRequest, String, String)> = None;
    egui::Window::new("Add step from API collection")
        .id(egui::Id::new(("it_picker", st.suite.id.as_str())))
        .open(&mut open)
        .collapsible(false)
        .default_size(egui::vec2(560.0, 460.0))
        .show(ctx, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text("Search method, path or name")
                    .desired_width(f32::INFINITY),
            );
            if live.catalogs.is_empty() {
                weak(ui, "The project's API workspace has no requests yet.");
                return;
            }
            let q = query.to_ascii_lowercase();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for c in &live.catalogs {
                        ui.label(egui::RichText::new(&c.folder_name).strong());
                        for r in &c.requests {
                            let path =
                                r.route
                                    .clone()
                                    .filter(|p| !p.is_empty())
                                    .unwrap_or_else(|| {
                                        crate::http_collection::extract_endpoint_url(&r.url)
                                    });
                            let hay = format!("{} {} {}", r.method.label(), path, r.display_name())
                                .to_ascii_lowercase();
                            if !q.is_empty() && !hay.contains(&q) {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                crate::http_repo::method_chip(ui, r.method.label());
                                if ui
                                    .selectable_label(
                                        false,
                                        format!("{path}  {}", r.display_name()),
                                    )
                                    .clicked()
                                {
                                    chosen =
                                        Some((r.clone(), c.base_var.clone(), c.base_url.clone()));
                                }
                            });
                        }
                    }
                });
        });
    if let Some((mut req, base_var, base_url)) = chosen {
        let req_id = req.id.clone();
        // Host diganti variabel base URL folder, seperti suite dari AI.
        if !base_url.is_empty() && req.url.starts_with(&base_url) {
            req.url = format!("{{{{{base_var}}}}}{}", &req.url[base_url.len()..]);
            if !st.suite.variables.iter().any(|(k, _)| *k == base_var) {
                st.suite.variables.insert(0, (base_var, base_url));
            }
        }
        // Nilai auth tidak ikut disalin ke suite; pakai variabel.
        let mut needed: Vec<&str> = Vec::new();
        if !req.bearer_token.is_empty() {
            req.bearer_token = "{{token}}".into();
            needed.push("token");
        }
        if !req.basic_pass.is_empty() {
            req.basic_user = "{{username}}".into();
            req.basic_pass = "{{password}}".into();
            needed.extend(["username", "password"]);
        }
        if !req.api_key_value.is_empty() {
            req.api_key_value = "{{api_key}}".into();
            needed.push("api_key");
        }
        for k in needed {
            if !st.suite.variables.iter().any(|(x, _)| x == k) {
                st.suite.variables.push((k.to_string(), String::new()));
            }
        }
        add_step(st, req, Some(req_id));
        return;
    }
    if open {
        st.picker = Some(query);
    }
}

// ─── Aksi ───────────────────────────────────────────────────────────────────

fn apply(
    t: &mut Tabular,
    ctx: &egui::Context,
    idx: usize,
    suite: &HttpTestSuite,
    last_run: Option<&TestRun>,
    act: Act,
) {
    match act {
        Act::Run(groups, allow_production) => {
            // Simpan dulu agar run memakai definisi terbaru.
            if let Err(e) = http_tests::save_suite(suite) {
                t.toasts.error(e);
            }
            let mut scope = RunScope::groups(suite, groups);
            scope.allow_production = allow_production;
            if let Err(e) = jobs::start_run(t, suite.clone(), scope) {
                t.toasts.error(e);
            }
        }
        Act::Stop => jobs::stop_run(t, &suite.id),
        Act::Abort => jobs::abort_run(t, &suite.id),
        Act::OpenStep(step_id) => open_step_in_http(t, suite, &step_id, last_run),
        Act::GenSteps(mode) => {
            let instructions = with_tab_state(t, idx, |st| {
                let mut text = st.ai_prompt.trim().to_string();
                if let GroupFilter::Group(g) = &st.filter
                    && !g.is_empty()
                {
                    text = format!("Group \"{g}\": set \"group\":\"{g}\" on new steps. {text}");
                }
                text
            })
            .unwrap_or_default();
            if let Err(e) = jobs::start_step_gen(t, suite, instructions, mode) {
                t.toasts.error(e);
            }
        }
        Act::GenSuite => {
            let instructions =
                with_tab_state(t, idx, |st| st.ai_prompt.trim().to_string()).unwrap_or_default();
            if let Err(e) = jobs::start_suite_gen(t, instructions) {
                t.toasts.error(e);
            }
        }
        Act::CancelGen => {
            jobs::cancel_gen(t, &suite.id);
            jobs::cancel_suite_gen(t);
        }
        Act::NewSuite => {
            let s = HttpTestSuite::new("New suite", suite.project_id.clone());
            match http_tests::save_suite(&s) {
                Ok(()) => replace_suite(t, idx, s),
                Err(e) => t.toasts.error(e),
            }
        }
        Act::DuplicateSuite => {
            let mut s = suite.clone();
            s.id = crate::http_collection::unique_id("suite");
            s.name = format!("{} (copy)", suite.name);
            s.created_at = chrono::Utc::now().to_rfc3339();
            s.touch();
            match http_tests::save_suite(&s) {
                Ok(()) => replace_suite(t, idx, s),
                Err(e) => t.toasts.error(e),
            }
        }
        Act::SwitchSuite(id) => match http_tests::load_suite(&id) {
            Some(s) => replace_suite(t, idx, s),
            None => t.toasts.warning("The suite no longer exists"),
        },
        Act::DeleteSuite => {
            if let Err(e) = http_tests::remove_suite(&suite.id) {
                t.toasts.error(e);
                return;
            }
            t.toasts.success(format!("Deleted suite '{}'", suite.name));
            let next = suite
                .project_id
                .as_deref()
                .and_then(|pid| http_tests::load_project_suites(pid).into_iter().next());
            let next = next.unwrap_or_else(|| {
                let s = HttpTestSuite::new("Integration tests", suite.project_id.clone());
                if let Err(e) = http_tests::save_suite(&s) {
                    log::warn!("[INTEGRATION_TEST] cannot create replacement suite: {e}");
                }
                s
            });
            replace_suite(t, idx, next);
        }
        Act::OpenHtmlReport => {
            let Some(run) = last_run else { return };
            let html = render_report(t, suite, run, ReportFormat::Html);
            let path = std::env::temp_dir().join(report::report_file_name(run, ReportFormat::Html));
            match std::fs::write(&path, html) {
                Ok(()) => crate::self_update::open_url(&format!("file://{}", path.display())),
                Err(e) => t.toasts.error(format!("Could not write the report: {e}")),
            }
        }
        Act::Export(format) => {
            let Some(run) = last_run else { return };
            let text = render_report(t, suite, run, format);
            export_file(t, run, format, text);
        }
        Act::Validate => {
            // Simpan dulu agar validasi memakai definisi terbaru.
            if let Err(e) = http_tests::save_suite(suite) {
                t.toasts.error(e);
            }
            match jobs::start_validation(t, suite) {
                Ok(()) => {
                    with_tab_state(t, idx, |st| st.validation_note = None);
                }
                Err(e) => t.toasts.error(e),
            }
        }
        Act::CancelValidate => jobs::cancel_validation(t, &suite.id),
        Act::SetAutoValidate(on) => t.integration_tests.skip_auto_validate = !on,
        Act::CodeFix(ids, note) => {
            let Some(run) = last_run else {
                t.toasts
                    .info("Run the suite first so the AI can see what fails");
                return;
            };
            if let Err(e) = jobs::start_code_fix(t, suite, run, &ids, note) {
                t.toasts.error(e);
            }
        }
        Act::CancelCodeFix => jobs::cancel_code_fix(t, &suite.id),
        Act::Proposal(action) => jobs::resolve_proposal(t, &suite.id, action),
        Act::CopyMarkdown => {
            let Some(run) = last_run else {
                t.toasts.info("Run the suite first");
                return;
            };
            let text = render_report(t, suite, run, ReportFormat::Markdown);
            ctx.copy_text(text);
            t.toasts.success("Report copied as Markdown");
        }
    }
}

fn render_report(
    t: &Tabular,
    suite: &HttpTestSuite,
    run: &TestRun,
    format: ReportFormat,
) -> String {
    let history = http_tests::load_runs(&suite.id, HISTORY_LIMIT);
    let catalog = jobs::project_requests(t, suite);
    format.render(&ReportInput {
        suite,
        run,
        history: &history,
        catalog: &catalog,
    })
}

#[cfg(not(target_os = "ios"))]
fn export_file(t: &mut Tabular, run: &TestRun, format: ReportFormat, text: String) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(format.label(), &[format.extension()])
        .set_file_name(report::report_file_name(run, format))
        .save_file()
    else {
        return;
    };
    match std::fs::write(&path, text) {
        Ok(()) => t
            .toasts
            .success(format!("Report saved to {}", path.display())),
        Err(e) => t.toasts.error(format!("Could not save the report: {e}")),
    }
}

#[cfg(target_os = "ios")]
fn export_file(t: &mut Tabular, _run: &TestRun, _format: ReportFormat, _text: String) {
    t.toasts
        .info("Exporting reports is not available on this device");
}

fn with_tab_state<R>(
    t: &mut Tabular,
    idx: usize,
    f: impl FnOnce(&mut IntegrationTestTabState) -> R,
) -> Option<R> {
    t.query_tabs
        .get_mut(idx)
        .and_then(|tab| tab.integration_test_state.as_mut())
        .map(f)
}

/// Ganti suite yang dibuka tab `idx`.
fn replace_suite(t: &mut Tabular, idx: usize, suite: HttpTestSuite) {
    if let Some(tab) = t.query_tabs.get_mut(idx) {
        let st = IntegrationTestTabState::new(suite);
        tab.title = st.title();
        tab.integration_test_state = Some(st);
    }
}

/// Buka langkah di tab HTTP client baru (atau tab yang sudah mengeditnya).
fn open_step_in_http(
    t: &mut Tabular,
    suite: &HttpTestSuite,
    step_id: &str,
    last_run: Option<&TestRun>,
) {
    // Langkah DB diedit di panel kanan; tidak ada request untuk dibuka.
    if suite.steps.iter().any(|s| s.id == step_id && s.is_db()) {
        return;
    }
    if let Some(idx) = t.query_tabs.iter().position(|tab| {
        tab.http_client_state
            .as_ref()
            .and_then(|s| s.test_step.as_ref())
            .is_some_and(|r| r.suite_id == suite.id && r.step_id == step_id)
    }) {
        crate::editor::switch_to_tab(t, idx);
        return;
    }
    let Some(step) = suite.steps.iter().find(|s| s.id == step_id).cloned() else {
        return;
    };
    let env = jobs::project_vars(t, suite);
    let mut vars = http_tests::initial_vars_with(suite, &env);
    // Nilai hasil run terakhir (id, dsb.) agar request bisa langsung dikirim.
    if let Some(run) = last_run {
        for r in &run.results {
            for (k, v) in &r.extracted {
                if v != crate::http_ai::REDACTED {
                    vars.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let mut state = crate::models::structs::HttpClientState::default();
    crate::http_collection::apply_saved_request(&step.request, &mut state);
    // Bukan request collection: simpan menulis ke langkah suite.
    state.saved_request_id = None;
    state.saved_workspace_id = None;
    state.saved_folder_id = None;
    state.save_dialog_name = step.name.clone();
    state.test_step = Some(http_tests::TestStepRef {
        suite_id: suite.id.clone(),
        step_id: step.id.clone(),
        label: format!("{} / {}", suite.name, step.name),
        vars,
    });
    crate::editor::create_new_tab(t, format!("Step: {}", step.name), String::new());
    if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
        tab.http_client_state = Some(state);
    }
}
