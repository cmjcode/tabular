//! Laporan hasil integration test HTTP.
//!
//! Statistik disusun mengikuti istilah ISTQB dan struktur *Test Completion
//! Report* ISO/IEC/IEEE 29119-3: rencana vs eksekusi, pass/fail/blocked,
//! kriteria lulus (exit criteria), cakupan endpoint, traceability, defect per
//! endpoint, dan latensi. Bagian bukti validasi mengikuti praktik Computer
//! System Validation (GAMP 5, ALCOA+): siapa, kapan, apa yang diuji (sidik
//! jari SHA-256 definisi suite), hasil yang diharapkan vs aktual, deviasi
//! (sisa data uji), dan kolom tanda tangan reviewer.
//!
//! Modul ini headless: hanya menghitung dan memformat dari [`TestRun`] dan
//! [`HttpTestSuite`]. Semua URL dan body sudah disamarkan oleh runner.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::http_collection::SavedRequest;
use crate::http_tests::{HttpTestSuite, StepOutcome, StepPhase, StepResult, TestRun};

// ─── Statistik ──────────────────────────────────────────────────────────────

/// Jumlah per status ISTQB.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OutcomeCounts {
    pub planned: usize,
    pub executed: usize,
    pub passed: usize,
    pub failed: usize,
    pub error: usize,
    pub blocked: usize,
    pub skipped: usize,
}

impl OutcomeCounts {
    fn add(&mut self, r: &StepResult) {
        self.planned += 1;
        match r.outcome() {
            StepOutcome::Passed => self.passed += 1,
            StepOutcome::Failed => self.failed += 1,
            StepOutcome::Error => self.error += 1,
            StepOutcome::Blocked => self.blocked += 1,
            StepOutcome::Skipped => self.skipped += 1,
        }
        if matches!(r.outcome(), StepOutcome::Passed | StepOutcome::Failed) {
            self.executed += 1;
        }
    }

    /// Lulus / dieksekusi (persen). Error transport dihitung sebagai eksekusi gagal.
    pub fn pass_rate(&self) -> f64 {
        let attempted = self.executed + self.error;
        if attempted == 0 {
            0.0
        } else {
            self.passed as f64 * 100.0 / attempted as f64
        }
    }

    /// Dieksekusi / direncanakan (persen), tanpa langkah yang sengaja dinonaktifkan.
    pub fn execution_rate(&self) -> f64 {
        let planned = self.planned - self.skipped;
        if planned == 0 {
            0.0
        } else {
            (self.executed + self.error) as f64 * 100.0 / planned as f64
        }
    }
}

/// Statistik latensi (ms) langkah yang mendapat response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Latency {
    pub samples: usize,
    pub min: u128,
    pub avg: f64,
    pub p50: u128,
    pub p90: u128,
    pub p95: u128,
    pub max: u128,
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    // Metode nearest-rank.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn latency(results: &[StepResult]) -> Latency {
    let mut t: Vec<u128> = results
        .iter()
        .filter(|r| r.executed())
        .map(|r| r.time_ms)
        .collect();
    if t.is_empty() {
        return Latency::default();
    }
    t.sort_unstable();
    Latency {
        samples: t.len(),
        min: t[0],
        avg: t.iter().sum::<u128>() as f64 / t.len() as f64,
        p50: percentile(&t, 50.0),
        p90: percentile(&t, 90.0),
        p95: percentile(&t, 95.0),
        max: t[t.len() - 1],
    }
}

/// Keputusan akhir run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    /// Lulus kriteria, tetapi ada deviasi (langkah diblokir, sisa data uji).
    PassWithDeviations,
    Fail,
    /// Run dihentikan sebelum selesai.
    Incomplete,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::PassWithDeviations => "PASS WITH DEVIATIONS",
            Verdict::Fail => "FAIL",
            Verdict::Incomplete => "INCOMPLETE",
        }
    }
}

/// Statistik per group.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GroupStats {
    pub group: String,
    pub counts: OutcomeCounts,
    pub time_ms: u128,
}

/// Kegagalan per endpoint (defect clustering).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EndpointFailures {
    pub endpoint: String,
    pub executions: usize,
    pub failures: usize,
}

/// Cakupan endpoint katalog project oleh suite.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Coverage {
    pub catalog: usize,
    pub covered: usize,
    pub untested: Vec<String>,
}

impl Coverage {
    pub fn percent(&self) -> f64 {
        if self.catalog == 0 {
            0.0
        } else {
            self.covered as f64 * 100.0 / self.catalog as f64
        }
    }
}

/// Kestabilan langkah dari riwayat run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Flakiness {
    pub step_id: String,
    pub name: String,
    pub runs: usize,
    pub passes: usize,
    /// Jumlah pergantian lulus↔gagal antar run berurutan.
    pub flips: usize,
}

impl Flakiness {
    pub fn is_flaky(&self) -> bool {
        self.flips > 0 && self.passes > 0 && self.passes < self.runs
    }
}

/// Seluruh statistik satu run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunStats {
    /// Langkah Setup + Test (test case).
    pub tests: OutcomeCounts,
    pub cleanup: OutcomeCounts,
    pub assertions_total: usize,
    pub assertions_passed: usize,
    pub latency: Latency,
    pub groups: Vec<GroupStats>,
    pub endpoint_failures: Vec<EndpointFailures>,
    pub residue_items: usize,
    pub db_residue_rows: u64,
    pub db_checks_failed: usize,
    pub threshold: f64,
    pub verdict: Option<Verdict>,
}

impl RunStats {
    pub fn verdict(&self) -> Verdict {
        self.verdict.unwrap_or(Verdict::Incomplete)
    }

    /// Data uji dinyatakan bersih: tidak ada residue dan DB check tidak menemukan baris.
    pub fn data_clean(&self) -> bool {
        self.residue_items == 0 && self.db_residue_rows == 0
    }
}

/// Endpoint (`METHOD /path`) dari URL hasil run.
fn result_endpoint(r: &StepResult) -> String {
    let path = crate::http_collection::extract_endpoint_url(&r.url);
    format!("{} {}", r.method, path)
}

pub fn compute(run: &TestRun) -> RunStats {
    let mut s = RunStats {
        threshold: if run.pass_threshold > 0.0 {
            run.pass_threshold
        } else {
            100.0
        },
        ..Default::default()
    };
    let mut groups: Vec<GroupStats> = Vec::new();
    let mut endpoints: Vec<EndpointFailures> = Vec::new();
    let mut setup_failed = false;
    for r in &run.results {
        if r.phase == StepPhase::Cleanup {
            s.cleanup.add(r);
            continue;
        }
        s.tests.add(r);
        if r.phase == StepPhase::Setup
            && matches!(r.outcome(), StepOutcome::Failed | StepOutcome::Error)
        {
            setup_failed = true;
        }
        s.assertions_total += r.assertions.len();
        s.assertions_passed += r.assertions.iter().filter(|a| a.passed).count();
        let g = if r.group.trim().is_empty() {
            "(no group)".to_string()
        } else {
            r.group.clone()
        };
        let gi = match groups.iter().position(|x| x.group == g) {
            Some(i) => i,
            None => {
                groups.push(GroupStats {
                    group: g,
                    ..Default::default()
                });
                groups.len() - 1
            }
        };
        groups[gi].counts.add(r);
        groups[gi].time_ms += r.time_ms;
        if matches!(
            r.outcome(),
            StepOutcome::Passed | StepOutcome::Failed | StepOutcome::Error
        ) {
            let key = result_endpoint(r);
            let ei = match endpoints.iter().position(|x| x.endpoint == key) {
                Some(i) => i,
                None => {
                    endpoints.push(EndpointFailures {
                        endpoint: key,
                        ..Default::default()
                    });
                    endpoints.len() - 1
                }
            };
            endpoints[ei].executions += 1;
            if !r.passed() {
                endpoints[ei].failures += 1;
            }
        }
    }
    endpoints.retain(|e| e.failures > 0);
    endpoints.sort_by(|a, b| {
        b.failures
            .cmp(&a.failures)
            .then(a.endpoint.cmp(&b.endpoint))
    });
    s.groups = groups;
    s.endpoint_failures = endpoints;
    s.latency = latency(
        &run.results
            .iter()
            .filter(|r| r.phase != StepPhase::Cleanup)
            .cloned()
            .collect::<Vec<_>>(),
    );
    s.residue_items = run.residue.len();
    s.db_residue_rows = run.db_checks.iter().filter_map(|c| c.rows).sum();
    s.db_checks_failed = run.db_checks.iter().filter(|c| c.error.is_some()).count();

    let attempted = s.tests.executed + s.tests.error;
    let verdict = if run.cancelled || run.aborted || run.preflight_error.is_some() {
        Verdict::Incomplete
    } else if setup_failed || attempted == 0 || s.tests.pass_rate() + 1e-9 < s.threshold {
        Verdict::Fail
    } else if s.tests.blocked > 0
        || !s.data_clean()
        || s.cleanup.failed + s.cleanup.error + s.cleanup.blocked > 0
    {
        Verdict::PassWithDeviations
    } else {
        Verdict::Pass
    };
    s.verdict = Some(verdict);
    s
}

/// Kunci endpoint `METHOD /path/{}` untuk request (katalog maupun langkah).
pub fn endpoint_key(req: &SavedRequest) -> String {
    // `{{id}}` → `{id}` agar dinormalisasi sama dengan parameter route.
    let fix = |s: &str| s.replace("{{", "{").replace("}}", "}");
    let mut r = req.clone();
    if r.route.as_deref().is_none_or(|x| x.trim().is_empty()) {
        // Buang variabel base URL di depan (`{{base_url_x}}/a` → `/a`).
        let url = r.url.trim().to_string();
        let path = if url.starts_with("{{") {
            url.find("}}")
                .map(|e| url[e + 2..].to_string())
                .unwrap_or(url)
        } else {
            url
        };
        r.route = Some(fix(&crate::http_collection::extract_endpoint_url(&path)));
    } else {
        r.route = r.route.map(|x| fix(&x));
    }
    crate::repo_endpoints::saved_request_key(&r)
}

/// Cakupan endpoint katalog oleh langkah suite yang aktif.
pub fn coverage(suite: &HttpTestSuite, catalog: &[SavedRequest]) -> Coverage {
    let tested: std::collections::HashSet<String> = suite
        .steps
        .iter()
        .filter(|s| s.enabled)
        .map(|s| endpoint_key(&s.request))
        .collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cov = Coverage::default();
    for req in catalog {
        let key = endpoint_key(req);
        if !seen.insert(key.clone()) {
            continue;
        }
        cov.catalog += 1;
        if tested.contains(&key) {
            cov.covered += 1;
        } else {
            cov.untested.push(key);
        }
    }
    cov.untested.sort();
    cov
}

/// Kestabilan per langkah dari riwayat (terbaru lebih dulu).
pub fn flakiness(history: &[TestRun]) -> Vec<Flakiness> {
    let mut map: HashMap<String, Flakiness> = HashMap::new();
    let mut last: HashMap<String, bool> = HashMap::new();
    for run in history.iter().rev() {
        for r in &run.results {
            if !matches!(
                r.outcome(),
                StepOutcome::Passed | StepOutcome::Failed | StepOutcome::Error
            ) || r.step_id.is_empty()
            {
                continue;
            }
            let f = map.entry(r.step_id.clone()).or_insert_with(|| Flakiness {
                step_id: r.step_id.clone(),
                name: r.name.clone(),
                ..Default::default()
            });
            f.runs += 1;
            let ok = r.passed();
            if ok {
                f.passes += 1;
            }
            if let Some(prev) = last.insert(r.step_id.clone(), ok)
                && prev != ok
            {
                f.flips += 1;
            }
        }
    }
    let mut out: Vec<Flakiness> = map.into_values().filter(|f| f.is_flaky()).collect();
    out.sort_by(|a, b| b.flips.cmp(&a.flips).then(a.name.cmp(&b.name)));
    out
}

/// Baris traceability: langkah ↔ endpoint ↔ tabel ↔ hasil.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TraceRow {
    pub no: usize,
    pub step_id: String,
    pub group: String,
    pub phase: String,
    pub name: String,
    pub endpoint: String,
    pub tables: Vec<String>,
    pub requirement: String,
    pub assertions: usize,
    pub outcome: String,
}

pub fn traceability(suite: &HttpTestSuite, run: Option<&TestRun>) -> Vec<TraceRow> {
    let mut rows = Vec::new();
    for (no, i) in suite.execution_order().into_iter().enumerate() {
        let s = &suite.steps[i];
        let outcome = run
            .and_then(|r| r.results.iter().find(|x| x.step_id == s.id))
            .map(|r| r.outcome().label().to_string())
            .unwrap_or_else(|| "Not run".into());
        rows.push(TraceRow {
            no: no + 1,
            step_id: s.id.clone(),
            group: s.group.clone(),
            phase: s.phase.label().to_string(),
            name: s.name.clone(),
            endpoint: endpoint_key(&s.request),
            tables: s.request.tables.clone(),
            requirement: s.notes.clone(),
            assertions: s.assertions.len(),
            outcome,
        });
    }
    rows
}

// ─── Konteks laporan ────────────────────────────────────────────────────────

/// Semua bahan satu laporan.
pub struct ReportInput<'a> {
    pub suite: &'a HttpTestSuite,
    pub run: &'a TestRun,
    /// Run sebelumnya (terbaru lebih dulu), boleh memuat `run` itu sendiri.
    pub history: &'a [TestRun],
    /// Request katalog project untuk cakupan; kosong = cakupan tidak dihitung.
    pub catalog: &'a [SavedRequest],
}

fn pct(v: f64) -> String {
    format!("{v:.1}%")
}

fn esc_md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn fmt_ms(ms: u128) -> String {
    if ms >= 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

// ─── Markdown ───────────────────────────────────────────────────────────────

pub fn to_markdown(input: &ReportInput) -> String {
    let ReportInput {
        suite,
        run,
        history,
        catalog,
    } = input;
    let st = compute(run);
    let mut o = String::new();
    let _ = writeln!(
        o,
        "# Integration Test Report: {}\n",
        esc_md(&run.suite_name)
    );
    let _ = writeln!(
        o,
        "Structured after ISO/IEC/IEEE 29119-3 (Test Completion Report) with ISTQB metrics, \
         and GAMP 5 style validation evidence.\n"
    );
    let _ = writeln!(o, "## 1. Verdict\n");
    let _ = writeln!(o, "**{}**\n", st.verdict().label());
    let _ = writeln!(
        o,
        "- Exit criterion: pass rate of executed tests at least {}",
        pct(st.threshold)
    );
    let _ = writeln!(
        o,
        "- Test data cleanup: {}",
        if st.data_clean() {
            "clean, no test data left behind".to_string()
        } else {
            format!(
                "{} item(s) may be left behind, {} database row(s) still contain the run id",
                st.residue_items, st.db_residue_rows
            )
        }
    );
    if let Some(e) = &run.preflight_error {
        let _ = writeln!(o, "- Not executed: {}", esc_md(e));
    }
    if run.cancelled || run.aborted {
        let _ = writeln!(
            o,
            "- The run was {} before it finished",
            if run.aborted { "aborted" } else { "stopped" }
        );
    }

    let _ = writeln!(o, "\n## 2. Test identification\n");
    let _ = writeln!(o, "| Item | Value |\n|---|---|");
    let rows: Vec<(&str, String)> = vec![
        ("Suite", run.suite_name.clone()),
        ("Project", run.project_name.clone().unwrap_or_default()),
        ("Environment", run.environment.clone().unwrap_or_default()),
        (
            "Scope",
            if run.groups.is_empty() {
                "All groups".into()
            } else {
                format!("Groups: {}", run.groups.join(", "))
            },
        ),
        ("Run id", run.run_id.clone()),
        ("Started", run.started_at.clone()),
        ("Finished", run.finished_at.clone()),
        ("Duration", fmt_ms(run.duration_ms)),
        ("Executed by", format!("{} @ {}", run.operator, run.host)),
        ("Tool", format!("Tabular {}", run.app_version)),
        ("Suite fingerprint (SHA-256)", run.suite_fingerprint.clone()),
        ("Target servers", run.targets.join(", ")),
    ];
    for (k, v) in rows {
        let _ = writeln!(o, "| {k} | {} |", esc_md(&v));
    }

    let t = &st.tests;
    let _ = writeln!(o, "\n## 3. Summary (ISTQB)\n");
    let _ = writeln!(o, "| Metric | Value |\n|---|---|");
    for (k, v) in [
        ("Planned test steps", t.planned.to_string()),
        ("Executed", (t.executed + t.error).to_string()),
        ("Passed", t.passed.to_string()),
        ("Failed (assertion)", t.failed.to_string()),
        ("Error (no response)", t.error.to_string()),
        ("Blocked", t.blocked.to_string()),
        ("Skipped / disabled", t.skipped.to_string()),
        ("Pass rate", pct(t.pass_rate())),
        ("Execution rate", pct(t.execution_rate())),
        (
            "Assertions passed",
            format!("{} / {}", st.assertions_passed, st.assertions_total),
        ),
        (
            "Cleanup steps passed",
            format!(
                "{} / {}",
                st.cleanup.passed,
                st.cleanup.planned - st.cleanup.skipped
            ),
        ),
    ] {
        let _ = writeln!(o, "| {k} | {v} |");
    }
    let l = &st.latency;
    if l.samples > 0 {
        let _ = writeln!(
            o,
            "\nResponse time: min {}, avg {:.0} ms, p50 {}, p90 {}, p95 {}, max {}",
            fmt_ms(l.min),
            l.avg,
            fmt_ms(l.p50),
            fmt_ms(l.p90),
            fmt_ms(l.p95),
            fmt_ms(l.max)
        );
    }

    if !st.groups.is_empty() {
        let _ = writeln!(o, "\n## 4. Results per group\n");
        let _ = writeln!(
            o,
            "| Group | Planned | Passed | Failed | Error | Blocked | Pass rate | Time |\n|---|---|---|---|---|---|---|---|"
        );
        for g in &st.groups {
            let c = &g.counts;
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                esc_md(&g.group),
                c.planned,
                c.passed,
                c.failed,
                c.error,
                c.blocked,
                pct(c.pass_rate()),
                fmt_ms(g.time_ms)
            );
        }
    }

    let _ = writeln!(o, "\n## 5. Step results (expected vs actual)\n");
    let _ = writeln!(
        o,
        "| # | Phase | Group | Step | Request | Status | Time | Result | Details |\n|---|---|---|---|---|---|---|---|---|"
    );
    for (n, r) in run.results.iter().enumerate() {
        let details = step_details_text(r);
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} {} | {} | {} | {} | {} |",
            n + 1,
            r.phase.label(),
            esc_md(&r.group),
            esc_md(&r.name),
            r.method,
            esc_md(&r.url),
            if r.status > 0 {
                r.status.to_string()
            } else {
                "-".into()
            },
            fmt_ms(r.time_ms),
            r.outcome().label(),
            esc_md(&details)
        );
    }

    write_db_and_burst_sections(&mut o, run);

    let _ = writeln!(o, "\n## 6. Deviations and test data cleanup\n");
    if run.residue.is_empty() {
        let _ = writeln!(o, "No test data was left behind by the API steps.");
    } else {
        let _ = writeln!(
            o,
            "| Created by | Request | Ids | Reason |\n|---|---|---|---|"
        );
        for r in &run.residue {
            let ids: Vec<String> = r.ids.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let _ = writeln!(
                o,
                "| {} | {} {} | {} | {} |",
                esc_md(&r.step_name),
                r.method,
                esc_md(&r.url),
                esc_md(&ids.join(", ")),
                esc_md(&r.reason)
            );
        }
    }
    if !run.db_checks.is_empty() {
        let _ = writeln!(
            o,
            "\nDatabase verification (rows still containing run id `{}`):\n",
            run.run_id
        );
        let _ = writeln!(o, "| Connection | Table | Rows | Note |\n|---|---|---|---|");
        for c in &run.db_checks {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} |",
                esc_md(&c.connection),
                esc_md(&c.table),
                c.rows.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
                esc_md(c.error.as_deref().unwrap_or("")),
            );
        }
    }

    if !st.endpoint_failures.is_empty() {
        let _ = writeln!(o, "\n## 7. Failures per endpoint\n");
        let _ = writeln!(o, "| Endpoint | Executions | Failures |\n|---|---|---|");
        for e in &st.endpoint_failures {
            let _ = writeln!(
                o,
                "| {} | {} | {} |",
                esc_md(&e.endpoint),
                e.executions,
                e.failures
            );
        }
    }

    if !catalog.is_empty() {
        let cov = coverage(suite, catalog);
        let _ = writeln!(o, "\n## 8. Endpoint coverage\n");
        let _ = writeln!(
            o,
            "{} of {} catalog endpoints are exercised ({}).",
            cov.covered,
            cov.catalog,
            pct(cov.percent())
        );
        if !cov.untested.is_empty() {
            let _ = writeln!(o, "\nNot covered:\n");
            for u in cov.untested.iter().take(200) {
                let _ = writeln!(o, "- `{u}`");
            }
        }
    }

    let _ = writeln!(o, "\n## 9. Traceability matrix\n");
    let _ = writeln!(
        o,
        "| # | Id | Group | Phase | Step | Endpoint | Tables | Requirement / note | Assertions | Result |\n|---|---|---|---|---|---|---|---|---|---|"
    );
    for t in traceability(suite, Some(run)) {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            t.no,
            t.step_id,
            esc_md(&t.group),
            t.phase,
            esc_md(&t.name),
            esc_md(&t.endpoint),
            esc_md(&t.tables.join(", ")),
            esc_md(&t.requirement),
            t.assertions,
            t.outcome
        );
    }

    if history.len() > 1 {
        let _ = writeln!(o, "\n## 10. Trend and stability\n");
        let _ = writeln!(
            o,
            "| Run | Started | Verdict | Pass rate |\n|---|---|---|---|"
        );
        for h in history.iter().take(10) {
            let s = compute(h);
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} |",
                h.run_id,
                h.started_at,
                s.verdict().label(),
                pct(s.tests.pass_rate())
            );
        }
        let flaky = flakiness(history);
        if flaky.is_empty() {
            let _ = writeln!(o, "\nNo flaky steps in the last {} runs.", history.len());
        } else {
            let _ = writeln!(o, "\nFlaky steps (result changes between runs):\n");
            for f in flaky {
                let _ = writeln!(
                    o,
                    "- {}: passed {} of {} runs, {} change(s)",
                    esc_md(&f.name),
                    f.passes,
                    f.runs,
                    f.flips
                );
            }
        }
    }

    let _ = writeln!(o, "\n## Review and approval\n");
    let _ = writeln!(o, "| Role | Name | Signature | Date |\n|---|---|---|---|");
    let _ = writeln!(
        o,
        "| Executed by | {} |  | {} |",
        esc_md(&run.operator),
        run.finished_at
    );
    let _ = writeln!(o, "| Reviewed by |  |  |  |");
    let _ = writeln!(o, "| Approved by |  |  |  |");
    o
}

/// Bukti langkah DB (SQL + parameter, jumlah baris, contoh, polling) dan
/// langkah concurrent (tabel respons per instance).
fn write_db_and_burst_sections(o: &mut String, run: &TestRun) {
    let db_steps: Vec<&StepResult> = run.results.iter().filter(|r| r.db.is_some()).collect();
    if !db_steps.is_empty() {
        let _ = writeln!(o, "\n## 5a. Database steps\n");
        for r in db_steps {
            let Some(db) = &r.db else { continue };
            let _ = writeln!(o, "### {} ({})\n", r.name, r.outcome().label());
            let _ = writeln!(
                o,
                "- Connection: {}; rows: {}{}; queries run: {}{}",
                if db.connection.is_empty() {
                    "-"
                } else {
                    db.connection.as_str()
                },
                db.row_count,
                if db.truncated { "+" } else { "" },
                db.attempts,
                if db.waited_ms > 0 {
                    format!(
                        " over {}{}",
                        fmt_ms(db.waited_ms),
                        if db.timed_out { " (timed out)" } else { "" }
                    )
                } else {
                    String::new()
                }
            );
            if !db.sql.is_empty() {
                let one_line = db.sql.split_whitespace().collect::<Vec<_>>().join(" ");
                let _ = writeln!(o, "- SQL: `{}`", one_line.replace('`', "'"));
            }
            if let Some(e) = &r.error {
                let _ = writeln!(o, "- Error: {}", esc_md(e));
            }
            if !db.columns.is_empty() && !db.sample.is_empty() {
                let _ = writeln!(o);
                let head: Vec<String> = db.columns.iter().map(|c| esc_md(c)).collect();
                let _ = writeln!(o, "| {} |", head.join(" | "));
                let _ = writeln!(o, "|{}", "---|".repeat(db.columns.len()));
                for row in &db.sample {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|c| esc_md(&crate::repo_scan::truncate_chars(c, 60)))
                        .collect();
                    let _ = writeln!(o, "| {} |", cells.join(" | "));
                }
            }
            let _ = writeln!(o);
        }
    }
    let bursts: Vec<&StepResult> = run
        .results
        .iter()
        .filter(|r| !r.instances.is_empty())
        .collect();
    if !bursts.is_empty() {
        let _ = writeln!(o, "\n## 5b. Concurrent requests\n");
        for r in bursts {
            let ok = r
                .instances
                .iter()
                .filter(|i| i.error.is_none() && (200..300).contains(&i.status))
                .count();
            let _ = writeln!(o, "### {} ({})\n", r.name, r.outcome().label());
            let _ = writeln!(
                o,
                "- {} request(s), {} accepted (2xx)",
                r.instances.len(),
                ok
            );
            for a in &r.assertions {
                let _ = writeln!(
                    o,
                    "- {} {}: actual {}",
                    if a.passed { "Passed" } else { "Failed" },
                    esc_md(&a.assertion.describe()),
                    esc_md(a.actual.as_deref().unwrap_or("(missing)"))
                );
            }
            let _ = writeln!(
                o,
                "\n| Round | # | Status | Time | Response |\n|---|---|---|---|---|"
            );
            for i in &r.instances {
                let _ = writeln!(
                    o,
                    "| {} | {} | {} | {} | {} |",
                    i.round,
                    i.index,
                    match &i.error {
                        Some(_) => "Error".to_string(),
                        None => i.status.to_string(),
                    },
                    fmt_ms(i.time_ms),
                    esc_md(&crate::repo_scan::truncate_chars(
                        i.error.as_deref().unwrap_or(&i.body_preview),
                        80
                    ))
                );
            }
            let _ = writeln!(o);
        }
    }
}

/// Ringkasan detail satu langkah untuk tabel laporan.
pub fn step_details_text(r: &StepResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = &r.note {
        parts.push(n.clone());
    }
    if let Some(e) = &r.error {
        parts.push(e.clone());
    }
    for a in r.assertions.iter().filter(|a| !a.passed) {
        parts.push(format!(
            "expected {}, actual {}",
            a.assertion.describe(),
            a.actual
                .as_deref()
                .map(|v| crate::repo_scan::truncate_chars(v, 80))
                .unwrap_or_else(|| "(missing)".into())
        ));
    }
    if parts.is_empty() && !r.assertions.is_empty() {
        parts.push(format!("{} assertion(s) passed", r.assertions.len()));
    }
    if r.retries > 0 {
        parts.push(format!("retried {}x", r.retries));
    }
    parts.join("; ")
}

// ─── HTML ───────────────────────────────────────────────────────────────────

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Laporan HTML mandiri (tanpa aset luar) yang bisa dibuka atau dicetak ke PDF.
pub fn to_html(input: &ReportInput) -> String {
    let md = to_markdown(input);
    let st = compute(input.run);
    let mut body = String::new();
    let mut in_table = false;
    let mut header_done = false;
    let mut in_list = false;
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            let cells: Vec<String> = split_md_row(trimmed);
            if cells
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-'))
            {
                continue;
            }
            if !in_table {
                body.push_str("<table>");
                in_table = true;
                header_done = false;
            }
            let tag = if header_done { "td" } else { "th" };
            body.push_str("<tr>");
            for c in cells {
                let class = match c.as_str() {
                    "Passed" | "PASS" => " class=\"ok\"",
                    "Failed" | "Error" | "FAIL" => " class=\"bad\"",
                    "Blocked" | "INCOMPLETE" | "PASS WITH DEVIATIONS" => " class=\"warn\"",
                    _ => "",
                };
                let _ = write!(body, "<{tag}{class}>{}</{tag}>", inline_html(&c));
            }
            body.push_str("</tr>");
            header_done = true;
            continue;
        }
        if in_table {
            body.push_str("</table>");
            in_table = false;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if !in_list {
                body.push_str("<ul>");
                in_list = true;
            }
            let _ = write!(body, "<li>{}</li>", inline_html(item));
            continue;
        }
        if in_list {
            body.push_str("</ul>");
            in_list = false;
        }
        if let Some(h) = trimmed.strip_prefix("### ") {
            let _ = write!(body, "<h3>{}</h3>", esc_html(h));
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            let _ = write!(body, "<h2>{}</h2>", esc_html(h));
        } else if let Some(h) = trimmed.strip_prefix("# ") {
            let _ = write!(body, "<h1>{}</h1>", esc_html(h));
        } else if trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 4 {
            let v = trimmed.trim_matches('*');
            let class = match st.verdict() {
                Verdict::Pass => "ok",
                Verdict::Fail => "bad",
                _ => "warn",
            };
            let _ = write!(body, "<p class=\"verdict {class}\">{}</p>", esc_html(v));
        } else if !trimmed.is_empty() {
            let _ = write!(body, "<p>{}</p>", inline_html(trimmed));
        }
    }
    if in_table {
        body.push_str("</table>");
    }
    if in_list {
        body.push_str("</ul>");
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title}</title><style>\
:root{{--bg:#fff;--fg:#1d2330;--line:#d9dee7;--ok:#1e7d3a;--bad:#b3261e;--warn:#9a6700;--head:#f3f5f9}}\
@media (prefers-color-scheme: dark){{:root{{--bg:#15181e;--fg:#e6e9ef;--line:#323844;--ok:#5cc87a;--bad:#ff8a80;--warn:#e3b341;--head:#1e232b}}}}\
body{{background:var(--bg);color:var(--fg);font:14px/1.5 -apple-system,Segoe UI,Roboto,sans-serif;margin:0 auto;max-width:1200px;padding:24px 16px}}\
h1{{font-size:22px}}h2{{font-size:17px;margin-top:28px;border-bottom:1px solid var(--line);padding-bottom:4px}}\
table{{border-collapse:collapse;width:100%;margin:8px 0;display:block;overflow-x:auto}}\
th,td{{border:1px solid var(--line);padding:4px 8px;text-align:left;vertical-align:top}}th{{background:var(--head)}}\
code{{font-family:ui-monospace,Menlo,monospace;font-size:12px}}\
.ok{{color:var(--ok);font-weight:600}}.bad{{color:var(--bad);font-weight:600}}.warn{{color:var(--warn);font-weight:600}}\
.verdict{{font-size:20px}}\
@media print{{body{{max-width:none}}table{{display:table}}}}\
</style></head><body>{body}</body></html>",
        title = esc_html(&format!("Test report {}", input.run.suite_name)),
    )
}

/// Pecah baris tabel Markdown menjadi sel (`\|` tidak dianggap pemisah).
fn split_md_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '|' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

/// Markdown inline minimal: `code` dan **bold**.
fn inline_html(s: &str) -> String {
    let e = esc_html(s);
    let mut out = String::new();
    let mut code = false;
    for (i, part) in e.split('`').enumerate() {
        if i > 0 {
            out.push_str(if code { "</code>" } else { "<code>" });
            code = !code;
        }
        out.push_str(part);
    }
    if code {
        out.push_str("</code>");
    }
    let mut res = String::new();
    let mut bold = false;
    for (i, part) in out.split("**").enumerate() {
        if i > 0 {
            res.push_str(if bold { "</strong>" } else { "<strong>" });
            bold = !bold;
        }
        res.push_str(part);
    }
    if bold {
        res.push_str("</strong>");
    }
    res
}

// ─── JUnit XML ──────────────────────────────────────────────────────────────

fn esc_xml(s: &str) -> String {
    s.chars()
        .filter(|c| matches!(c, '\t' | '\n' | '\r') || !c.is_control())
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// JUnit XML untuk CI (GitHub Actions, GitLab, Jenkins).
pub fn to_junit(run: &TestRun) -> String {
    let st = compute(run);
    let t = &st.tests;
    let c = &st.cleanup;
    let tests = t.planned + c.planned;
    let failures = t.failed + c.failed;
    let errors = t.error + c.error;
    let skipped = t.skipped + t.blocked + c.skipped + c.blocked;
    let secs = run.duration_ms as f64 / 1000.0;
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        o,
        "<testsuites name=\"Tabular integration tests\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{secs:.3}\">"
    );
    let _ = writeln!(
        o,
        "  <testsuite name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{secs:.3}\" timestamp=\"{}\" hostname=\"{}\">",
        esc_xml(&run.suite_name),
        esc_xml(&run.started_at),
        esc_xml(&run.host)
    );
    o.push_str("    <properties>\n");
    for (k, v) in [
        ("run_id", run.run_id.clone()),
        ("environment", run.environment.clone().unwrap_or_default()),
        ("verdict", st.verdict().label().to_string()),
        ("suite_fingerprint", run.suite_fingerprint.clone()),
        ("residue_items", st.residue_items.to_string()),
        ("db_residue_rows", st.db_residue_rows.to_string()),
        ("groups", run.groups.join(",")),
    ] {
        let _ = writeln!(
            o,
            "      <property name=\"{k}\" value=\"{}\"/>",
            esc_xml(&v)
        );
    }
    o.push_str("    </properties>\n");
    for r in &run.results {
        let class = if r.group.is_empty() {
            format!("{}.{}", run.suite_name, r.phase.label())
        } else {
            format!("{}.{}", run.suite_name, r.group)
        };
        let _ = write!(
            o,
            "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
            esc_xml(&class),
            esc_xml(&format!("{} [{} {}]", r.name, r.method, r.url)),
            r.time_ms as f64 / 1000.0
        );
        let details = esc_xml(&step_details_text(r));
        match r.outcome() {
            StepOutcome::Passed => o.push_str("/>\n"),
            StepOutcome::Failed => {
                let _ = writeln!(
                    o,
                    ">\n      <failure message=\"{details}\" type=\"AssertionFailed\">{}</failure>\n    </testcase>",
                    esc_xml(&r.body_preview)
                );
            }
            StepOutcome::Error => {
                let _ = writeln!(
                    o,
                    ">\n      <error message=\"{details}\" type=\"RequestError\"/>\n    </testcase>"
                );
            }
            StepOutcome::Blocked | StepOutcome::Skipped => {
                let _ = writeln!(
                    o,
                    ">\n      <skipped message=\"{}: {details}\"/>\n    </testcase>",
                    r.outcome().label()
                );
            }
        }
    }
    o.push_str("  </testsuite>\n</testsuites>\n");
    o
}

// ─── JSON ───────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct JsonReport<'a> {
    run: &'a TestRun,
    stats: RunStats,
    coverage: Option<Coverage>,
    traceability: Vec<TraceRow>,
    flaky_steps: Vec<Flakiness>,
}

pub fn to_json(input: &ReportInput) -> String {
    let report = JsonReport {
        run: input.run,
        stats: compute(input.run),
        coverage: (!input.catalog.is_empty()).then(|| coverage(input.suite, input.catalog)),
        traceability: traceability(input.suite, Some(input.run)),
        flaky_steps: flakiness(input.history),
    };
    serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// Format export yang didukung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Markdown,
    Html,
    JUnit,
    Json,
}

impl ReportFormat {
    pub const ALL: [ReportFormat; 4] = [
        ReportFormat::Html,
        ReportFormat::Markdown,
        ReportFormat::JUnit,
        ReportFormat::Json,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ReportFormat::Markdown => "Markdown",
            ReportFormat::Html => "HTML",
            ReportFormat::JUnit => "JUnit XML",
            ReportFormat::Json => "JSON",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ReportFormat::Markdown => "md",
            ReportFormat::Html => "html",
            ReportFormat::JUnit => "xml",
            ReportFormat::Json => "json",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(ReportFormat::Markdown),
            "html" | "htm" => Some(ReportFormat::Html),
            "junit" | "xml" => Some(ReportFormat::JUnit),
            "json" => Some(ReportFormat::Json),
            _ => None,
        }
    }

    pub fn render(self, input: &ReportInput) -> String {
        match self {
            ReportFormat::Markdown => to_markdown(input),
            ReportFormat::Html => to_html(input),
            ReportFormat::JUnit => to_junit(input.run),
            ReportFormat::Json => to_json(input),
        }
    }
}

/// Nama file laporan yang aman.
pub fn report_file_name(run: &TestRun, format: ReportFormat) -> String {
    let slug: String = run
        .suite_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "test-report-{}-{}.{}",
        if slug.is_empty() { "suite" } else { &slug },
        run.run_id,
        format.extension()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_tests::{AssertResult, Assertion, Residue};

    fn res(
        name: &str,
        group: &str,
        phase: StepPhase,
        status: u16,
        pass: bool,
        ms: u128,
    ) -> StepResult {
        StepResult {
            step_id: name.to_lowercase(),
            name: name.into(),
            group: group.into(),
            phase,
            method: "POST".into(),
            url: "http://h/api/x".into(),
            status,
            time_ms: ms,
            assertions: vec![AssertResult {
                assertion: Assertion {
                    target: "status".into(),
                    value: "201".into(),
                    ..Default::default()
                },
                passed: pass,
                actual: Some(status.to_string()),
            }],
            ..Default::default()
        }
    }

    fn run(results: Vec<StepResult>) -> TestRun {
        TestRun {
            run_id: "1".into(),
            suite_name: "ERP <flow> & co".into(),
            results,
            pass_threshold: 100.0,
            ..Default::default()
        }
    }

    #[test]
    fn counts_istqb_outcomes_and_verdict() {
        let mut blocked = res("C", "Plant", StepPhase::Test, 0, true, 0);
        blocked.blocked = true;
        let r = run(vec![
            res("A", "Purchasing", StepPhase::Test, 201, true, 10),
            res("B", "Purchasing", StepPhase::Test, 400, false, 30),
            blocked,
            res("Del", "Purchasing", StepPhase::Cleanup, 204, true, 5),
        ]);
        let s = compute(&r);
        assert_eq!(s.tests.planned, 3);
        assert_eq!(s.tests.passed, 1);
        assert_eq!(s.tests.failed, 1);
        assert_eq!(s.tests.blocked, 1);
        assert_eq!(s.cleanup.passed, 1);
        assert!((s.tests.pass_rate() - 50.0).abs() < 1e-9);
        assert_eq!(s.verdict(), Verdict::Fail);
        assert_eq!(s.groups.len(), 2);
        assert_eq!(s.endpoint_failures.len(), 1);
    }

    #[test]
    fn residue_turns_pass_into_deviation() {
        let mut r = run(vec![res("A", "", StepPhase::Test, 201, true, 10)]);
        assert_eq!(compute(&r).verdict(), Verdict::Pass);
        r.residue.push(Residue {
            step_name: "A".into(),
            reason: "No cleanup".into(),
            ..Default::default()
        });
        assert_eq!(compute(&r).verdict(), Verdict::PassWithDeviations);
        r.cancelled = true;
        assert_eq!(compute(&r).verdict(), Verdict::Incomplete);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let rs: Vec<StepResult> = (1..=10)
            .map(|i| res("x", "", StepPhase::Test, 200, true, i * 10))
            .collect();
        let l = latency(&rs);
        assert_eq!((l.min, l.p50, l.p90, l.max), (10, 50, 90, 100));
    }

    #[test]
    fn junit_is_escaped_and_counts_failures() {
        let r = run(vec![
            res("A", "", StepPhase::Test, 201, true, 10),
            res("B", "", StepPhase::Test, 500, false, 10),
        ]);
        let x = to_junit(&r);
        assert!(x.contains("ERP &lt;flow&gt; &amp; co"));
        assert!(x.contains("failures=\"1\""));
        assert!(x.contains("<failure"));
        assert!(!x.contains("<flow>"));
    }

    #[test]
    fn html_report_escapes_and_marks_verdict() {
        let r = run(vec![res("A", "", StepPhase::Test, 201, true, 10)]);
        let suite = HttpTestSuite::default();
        let html = to_html(&ReportInput {
            suite: &suite,
            run: &r,
            history: &[],
            catalog: &[],
        });
        assert!(html.contains("<p class=\"verdict ok\">PASS</p>"));
        assert!(html.contains("ERP &lt;flow&gt; &amp; co"));
        assert!(!html.contains("<flow>"));
    }

    #[test]
    fn endpoint_keys_match_catalog_routes() {
        let catalog = SavedRequest {
            route: Some("/api/orders/{id}".into()),
            method: crate::models::structs::HttpMethod::GET,
            ..Default::default()
        };
        let step = SavedRequest {
            url: "{{base_url_erp}}/api/orders/{{order_id}}".into(),
            method: crate::models::structs::HttpMethod::GET,
            ..Default::default()
        };
        assert_eq!(endpoint_key(&catalog), endpoint_key(&step));
    }

    #[test]
    fn flaky_steps_are_detected() {
        let h = vec![
            run(vec![res("A", "", StepPhase::Test, 201, true, 1)]),
            run(vec![res("A", "", StepPhase::Test, 500, false, 1)]),
            run(vec![res("A", "", StepPhase::Test, 201, true, 1)]),
        ];
        let f = flakiness(&h);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].flips, 2);
    }
}
