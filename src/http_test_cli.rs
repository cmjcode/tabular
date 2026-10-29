//! Subcommand `tabular test`: daftar dan jalankan suite integration test dari
//! terminal atau CI, dengan cleanup, verifikasi database, dan laporan
//! (HTML/Markdown/JSON/JUnit). Headless: tidak menyentuh `window_egui`.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use crate::http_test_report::{self as report, ReportFormat, ReportInput, Verdict};
use crate::http_tests::{self, HttpTestSuite, RunScope, TestEvent, TestRun};
use crate::project::Project;

pub const USAGE: &str = "\
USAGE:
  tabular test list [--project <name|id>] [--json]
  tabular test run <suite id|name> [options]

OPTIONS for `run`:
  --group <name>         run only this group (repeatable); required steps from other groups run too
  --env <name>           project environment to use (default: the active one)
  --report <file>        write a report; format from the extension (.html, .md, .json, .xml)
  --junit <file>         write a JUnit XML report for CI
  --allow-production     allow running against a Production environment (also lets
                         database steps read connections marked Production)
  --no-db-check          skip verifying the database for leftover test data

Database steps (kind db_query) read the connection named by an environment variable
such as db_main; values from {{variables}} are bound as parameters, never spliced.

Exit code is 0 when the verdict is PASS or PASS WITH DEVIATIONS, 1 otherwise.
";

fn arg_value(rest: &[String], i: &mut usize, name: &str) -> Result<String, String> {
    *i += 1;
    rest.get(*i)
        .cloned()
        .ok_or_else(|| format!("{name} needs a value\n\n{USAGE}"))
}

/// Titik masuk `tabular test …`.
pub fn run(rest: &[String]) -> Result<(), String> {
    match rest.first().map(String::as_str) {
        Some("list") => list(&rest[1..]),
        Some("run") => run_suite(&rest[1..]),
        Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!(
            "unknown `tabular test` command: {other}\n\n{USAGE}"
        )),
    }
}

pub(crate) fn projects() -> Vec<Project> {
    crate::project::load_all(&crate::config::get_data_dir())
}

pub(crate) fn find_project<'a>(all: &'a [Project], key: &str) -> Option<&'a Project> {
    let key = key.trim();
    all.iter()
        .find(|p| p.id == key)
        .or_else(|| all.iter().find(|p| p.name.eq_ignore_ascii_case(key)))
}

fn project_name(all: &[Project], id: Option<&str>) -> Option<String> {
    id.and_then(|id| all.iter().find(|p| p.id == id))
        .map(|p| p.name.clone())
}

fn list(rest: &[String]) -> Result<(), String> {
    let mut project: Option<String> = None;
    let mut json = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--project" => project = Some(arg_value(rest, &mut i, "--project")?),
            "--json" => json = true,
            other => return Err(format!("unknown option: {other}\n\n{USAGE}")),
        }
        i += 1;
    }
    let all = projects();
    let pid = match &project {
        Some(k) => Some(
            find_project(&all, k)
                .ok_or_else(|| format!("project '{k}' not found"))?
                .id
                .clone(),
        ),
        None => None,
    };
    let suites: Vec<HttpTestSuite> = http_tests::load_suites()
        .into_iter()
        .filter(|s| pid.is_none() || s.project_id == pid)
        .collect();
    if json {
        #[derive(serde::Serialize)]
        struct Item<'a> {
            id: &'a str,
            name: &'a str,
            project: Option<String>,
            groups: Vec<String>,
            steps: usize,
        }
        let items: Vec<Item> = suites
            .iter()
            .map(|s| Item {
                id: &s.id,
                name: &s.name,
                project: project_name(&all, s.project_id.as_deref()),
                groups: s.group_names(),
                steps: s.steps.len(),
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if suites.is_empty() {
        println!("No integration test suites.");
    }
    for s in &suites {
        let groups = s.group_names();
        println!(
            "{}  {}  [{}]  {} step(s){}",
            s.id,
            s.name,
            project_name(&all, s.project_id.as_deref()).unwrap_or_else(|| "-".into()),
            s.steps.len(),
            if groups.is_empty() {
                String::new()
            } else {
                format!("  groups: {}", groups.join(", "))
            }
        );
    }
    Ok(())
}

struct RunOptions {
    suite: String,
    groups: Vec<String>,
    env: Option<String>,
    report: Option<String>,
    junit: Option<String>,
    allow_production: bool,
    db_check: bool,
}

fn parse_run(rest: &[String]) -> Result<RunOptions, String> {
    let mut o = RunOptions {
        suite: String::new(),
        groups: Vec::new(),
        env: None,
        report: None,
        junit: None,
        allow_production: false,
        db_check: true,
    };
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--group" => o.groups.push(arg_value(rest, &mut i, "--group")?),
            "--env" => o.env = Some(arg_value(rest, &mut i, "--env")?),
            "--report" => o.report = Some(arg_value(rest, &mut i, "--report")?),
            "--junit" => o.junit = Some(arg_value(rest, &mut i, "--junit")?),
            "--allow-production" => o.allow_production = true,
            "--no-db-check" => o.db_check = false,
            s if s.starts_with('-') => return Err(format!("unknown option: {s}\n\n{USAGE}")),
            s if o.suite.is_empty() => o.suite = s.to_string(),
            s => return Err(format!("unexpected argument: {s}\n\n{USAGE}")),
        }
        i += 1;
    }
    if o.suite.is_empty() {
        return Err(format!("missing suite id or name\n\n{USAGE}"));
    }
    Ok(o)
}

fn find_suite(key: &str) -> Result<HttpTestSuite, String> {
    if let Some(s) = http_tests::load_suite(key) {
        return Ok(s);
    }
    let mut matches: Vec<HttpTestSuite> = http_tests::load_suites()
        .into_iter()
        .filter(|s| s.name.eq_ignore_ascii_case(key.trim()))
        .collect();
    match matches.len() {
        0 => Err(format!("suite '{key}' not found; see `tabular test list`")),
        1 => Ok(matches.remove(0)),
        n => Err(format!("{n} suites are named '{key}'; use the suite id")),
    }
}

/// Environment terpilih: (nama, variabel dengan secret, apakah Production).
pub(crate) fn environment(
    project: Option<&Project>,
    wanted: Option<&str>,
) -> Result<(Option<String>, HashMap<String, String>, bool), String> {
    let Some(p) = project else {
        return Ok((None, HashMap::new(), false));
    };
    let env = match wanted {
        Some(name) => Some(
            p.env(name)
                .ok_or_else(|| format!("environment '{name}' not found in {}", p.name))?,
        ),
        None => p.active_environment(),
    };
    Ok(match env {
        Some(e) => (
            Some(e.name.clone()),
            p.env_vars_with(&e.name, crate::secrets::get_secret),
            e.environment() == Some(crate::connection_env::Environment::Production),
        ),
        None => (None, HashMap::new(), false),
    })
}

fn print_step(r: &http_tests::StepResult) {
    let status = if r.status > 0 {
        r.status.to_string()
    } else {
        "-".into()
    };
    println!(
        "  [{:<7}] {:<7} {:<12} {} {} ({status}, {} ms)",
        r.outcome().label(),
        r.phase.label(),
        if r.group.is_empty() {
            "-"
        } else {
            r.group.as_str()
        },
        r.method,
        r.name,
        r.time_ms
    );
    let details = report::step_details_text(r);
    if !r.passed() && !details.is_empty() {
        println!("            {details}");
    }
}

fn run_suite(rest: &[String]) -> Result<(), String> {
    let opts = parse_run(rest)?;
    let suite = find_suite(&opts.suite)?;
    let all = projects();
    let project = suite
        .project_id
        .as_deref()
        .and_then(|id| all.iter().find(|p| p.id == id));
    let (env_name, env_vars, production) = environment(project, opts.env.as_deref())?;
    if production && !opts.allow_production {
        return Err(format!(
            "environment '{}' is Production; integration tests create and delete data. \
             Pass --allow-production to run anyway.",
            env_name.unwrap_or_default()
        ));
    }
    let known = suite.group_names();
    let unknown: Vec<&str> = opts
        .groups
        .iter()
        .filter(|g| !known.contains(g))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown group(s): {}. Groups in this suite: {}",
            unknown.join(", "),
            known.join(", ")
        ));
    }
    let mut scope = RunScope::groups(&suite, opts.groups.clone());
    scope.allow_production = opts.allow_production;
    let mut suite = suite;
    http_tests::fill_test_credentials(&mut suite.variables);
    let vars = http_tests::initial_vars_with(&suite, &env_vars);
    http_tests::preflight(&suite, &scope, &vars)?;
    let run_id = vars.get("run_id").cloned().unwrap_or_default();
    println!(
        "Running '{}'{} (run {run_id}){}",
        suite.name,
        if opts.groups.is_empty() {
            String::new()
        } else {
            format!(" groups {}", opts.groups.join(", "))
        },
        env_name
            .as_deref()
            .map(|e| format!(" on {e}"))
            .unwrap_or_default()
    );

    let started = chrono::Utc::now();
    let cancel = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let mut cleanup_announced = false;
    let mut emit = |e: TestEvent| match e {
        TestEvent::StepFinished(r) => print_step(&r),
        TestEvent::CleanupStarted if !cleanup_announced => {
            cleanup_announced = true;
            println!("Cleaning up test data");
        }
        _ => {}
    };
    let summary = http_tests::run_with_session(&suite, &scope, vars, &cancel, &abort, &mut emit);
    let mut run = TestRun::from_summary(&suite, &scope, run_id.clone(), started, summary);
    run.project_name = project.map(|p| p.name.clone());
    run.environment = env_name;

    if opts.db_check
        && let Some(p) = project
    {
        let tables = http_tests::touched_tables(&suite, &run.results);
        if !tables.is_empty() {
            run.db_checks = db_check(p, &tables, &run_id)?;
        }
    }
    if let Err(e) = http_tests::save_run(&run) {
        eprintln!("warning: {e}");
    }

    let history = http_tests::load_runs(&suite.id, 20);
    let input = ReportInput {
        suite: &suite,
        run: &run,
        history: &history,
        catalog: &[],
    };
    for (path, fallback) in [
        (opts.report.as_deref(), ReportFormat::Html),
        (opts.junit.as_deref(), ReportFormat::JUnit),
    ] {
        let Some(path) = path else { continue };
        let format = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ReportFormat::parse)
            .unwrap_or(fallback);
        std::fs::write(path, format.render(&input))
            .map_err(|e| format!("cannot write {path}: {e}"))?;
        println!("{} report written to {path}", format.label());
    }

    let s = report::compute(&run);
    println!(
        "\n{}: {} passed, {} failed, {} error, {} blocked, {} skipped; pass rate {:.1}% (criterion {:.0}%)",
        s.verdict().label(),
        s.tests.passed,
        s.tests.failed,
        s.tests.error,
        s.tests.blocked,
        s.tests.skipped,
        s.tests.pass_rate(),
        s.threshold
    );
    if s.data_clean() {
        println!("Test data: cleaned up");
    } else {
        println!("Test data may be left behind:");
        for r in &run.residue {
            println!("  - {} ({} {}): {}", r.step_name, r.method, r.url, r.reason);
        }
        for c in run.db_checks.iter().filter(|c| c.rows.unwrap_or(0) > 0) {
            println!(
                "  - {}.{}: {} row(s) contain run id {run_id}",
                c.connection,
                c.table,
                c.rows.unwrap_or(0)
            );
        }
    }
    match s.verdict() {
        Verdict::Pass | Verdict::PassWithDeviations => Ok(()),
        v => Err(format!("integration tests {}", v.label())),
    }
}

/// Verifikasi sisa data di koneksi milik project.
fn db_check(
    project: &Project,
    tables: &[String],
    run_id: &str,
) -> Result<Vec<http_tests::DbResidueCheck>, String> {
    // Titik masuk CLI `tabular test`: proses terpisah dari GUI.
    #[allow(clippy::disallowed_methods)]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to start async runtime: {e}"))?;
    Ok(runtime.block_on(async {
        let pool = match crate::agent::core::open_cache_pool().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("warning: database check skipped: {e}");
                return Vec::new();
            }
        };
        let session = crate::agent::core::HeadlessSession::new(pool);
        let targets: Vec<http_tests::DbCheckTarget> = session
            .list_connections()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c| project.owns_connection_entry(&c.name, c.folder.as_deref()))
            .map(|c| http_tests::DbCheckTarget {
                connection_id: c.id,
                connection_name: c.name,
            })
            .collect();
        http_tests::check_db_residue(&session, &targets, tables, run_id).await
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_run_options() {
        let o = parse_run(&args(&[
            "ERP flow",
            "--group",
            "Purchasing",
            "--group",
            "Plant",
            "--env",
            "Staging",
            "--junit",
            "out.xml",
            "--no-db-check",
        ]))
        .expect("valid");
        assert_eq!(o.suite, "ERP flow");
        assert_eq!(o.groups, vec!["Purchasing", "Plant"]);
        assert_eq!(o.env.as_deref(), Some("Staging"));
        assert_eq!(o.junit.as_deref(), Some("out.xml"));
        assert!(!o.db_check);
        assert!(!o.allow_production);
    }

    #[test]
    fn rejects_missing_values_and_unknown_options() {
        assert!(parse_run(&args(&[])).is_err());
        assert!(parse_run(&args(&["s", "--group"])).is_err());
        assert!(parse_run(&args(&["s", "--bogus"])).is_err());
        assert!(parse_run(&args(&["a", "b"])).is_err());
    }
}
