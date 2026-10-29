//! Subcommand `tabular check`: daftar, isi awal, dan jalankan data check
//! project (lihat [`crate::data_checks`]) dari terminal, cron, atau CI.
//! Headless: tidak menyentuh `window_egui`.

use crate::data_checks::{self, CheckStatus, DataCheckSet, RunRequest};
use crate::http_test_cli::{environment, find_project, projects};

pub const USAGE: &str = "\
USAGE:
  tabular check list --project <name|id> [--json]
  tabular check run --project <name|id> [options]
  tabular check seed --project <name|id> --preset voltunes-stock [--connection-var <name>]

OPTIONS for `run`:
  --env <name>           project environment to check (default: the active one)
  --check <id|name>      run only this check (repeatable)
  --report <file>        write a report; format from the extension (.html, .md, .json, .xml)
  --junit <file>         write a JUnit XML report for CI
  --json                 print the run as JSON instead of text

Data checks are read-only SELECTs that should return no rows; every row is evidence of
broken data. They are safe for Production (no writes, row limit and timeout), so no
--allow-production is needed. Exit code is 1 when a Critical check fails or cannot run.
";

fn arg_value(rest: &[String], i: &mut usize, name: &str) -> Result<String, String> {
    *i += 1;
    rest.get(*i)
        .cloned()
        .ok_or_else(|| format!("{name} needs a value\n\n{USAGE}"))
}

/// Titik masuk `tabular check …`.
pub fn run(rest: &[String]) -> Result<(), String> {
    match rest.first().map(String::as_str) {
        Some("list") => list(&rest[1..]),
        Some("run") => run_checks(&rest[1..]),
        Some("seed") => seed(&rest[1..]),
        Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!(
            "unknown `tabular check` command: {other}\n\n{USAGE}"
        )),
    }
}

#[derive(Debug, Default)]
struct Options {
    project: Option<String>,
    env: Option<String>,
    checks: Vec<String>,
    report: Option<String>,
    junit: Option<String>,
    json: bool,
    preset: Option<String>,
    connection_var: Option<String>,
}

fn parse(rest: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--project" => o.project = Some(arg_value(rest, &mut i, "--project")?),
            "--env" => o.env = Some(arg_value(rest, &mut i, "--env")?),
            "--check" => o.checks.push(arg_value(rest, &mut i, "--check")?),
            "--report" => o.report = Some(arg_value(rest, &mut i, "--report")?),
            "--junit" => o.junit = Some(arg_value(rest, &mut i, "--junit")?),
            "--preset" => o.preset = Some(arg_value(rest, &mut i, "--preset")?),
            "--connection-var" => {
                o.connection_var = Some(arg_value(rest, &mut i, "--connection-var")?)
            }
            "--json" => o.json = true,
            other => return Err(format!("unknown option: {other}\n\n{USAGE}")),
        }
        i += 1;
    }
    if o.project.is_none() {
        return Err(format!("--project is required\n\n{USAGE}"));
    }
    Ok(o)
}

fn project(key: &str) -> Result<crate::project::Project, String> {
    let all = projects();
    find_project(&all, key)
        .cloned()
        .ok_or_else(|| format!("project '{key}' not found"))
}

fn list(rest: &[String]) -> Result<(), String> {
    let o = parse(rest)?;
    let p = project(o.project.as_deref().unwrap_or_default())?;
    let set = data_checks::load_set(&p.id);
    let last = data_checks::load_runs(&p.id, 1).into_iter().next();
    if o.json {
        #[derive(serde::Serialize)]
        struct Item<'a> {
            id: &'a str,
            name: &'a str,
            severity: &'static str,
            enabled: bool,
            connection_var: &'a str,
            last_status: Option<&'static str>,
            last_rows: Option<usize>,
        }
        let items: Vec<Item> = set
            .checks
            .iter()
            .map(|c| {
                let last = last
                    .as_ref()
                    .and_then(|r| r.results.iter().find(|x| x.check_id == c.id));
                Item {
                    id: &c.id,
                    name: &c.name,
                    severity: c.severity.label(),
                    enabled: c.enabled,
                    connection_var: &c.connection_var,
                    last_status: last.map(|x| x.status.label()),
                    last_rows: last.and_then(|x| x.row_count),
                }
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if set.checks.is_empty() {
        println!(
            "No data checks in {}. Add them in Tabular (Data Health) or with `tabular check seed`.",
            p.name
        );
    }
    for c in &set.checks {
        let last = last
            .as_ref()
            .and_then(|r| r.results.iter().find(|x| x.check_id == c.id));
        println!(
            "{}  {:<8} {}{}  [{{{{{}}}}}]{}",
            c.id,
            c.severity.label(),
            c.name,
            if c.enabled { "" } else { " (disabled)" },
            c.connection_var,
            last.map(|x| format!(
                "  last: {} {}",
                x.status.label(),
                x.row_count
                    .map(|n| format!("({n} rows)"))
                    .unwrap_or_default()
            ))
            .unwrap_or_default()
        );
    }
    Ok(())
}

fn seed(rest: &[String]) -> Result<(), String> {
    let o = parse(rest)?;
    let p = project(o.project.as_deref().unwrap_or_default())?;
    let preset = o.preset.as_deref().unwrap_or("");
    let mut starters = match preset {
        "voltunes-stock" | "voltunes" => data_checks::voltunes_stock_checks(),
        other => {
            return Err(format!(
                "unknown preset '{other}'; available: voltunes-stock\n\n{USAGE}"
            ));
        }
    };
    if let Some(var) = &o.connection_var {
        for c in &mut starters {
            c.connection_var = var.trim().to_string();
        }
    }
    let mut set = data_checks::load_set(&p.id);
    let mut added = 0;
    for c in starters {
        if set.check(&c.id).is_none() {
            set.checks.push(c);
            added += 1;
        }
    }
    data_checks::save_set(&set)?;
    println!(
        "Added {added} check(s) to {} ({} total). Set the connection variable in each environment.",
        p.name,
        set.checks.len()
    );
    Ok(())
}

fn run_checks(rest: &[String]) -> Result<(), String> {
    let o = parse(rest)?;
    let p = project(o.project.as_deref().unwrap_or_default())?;
    let set: DataCheckSet = data_checks::load_set(&p.id);
    if set.checks.is_empty() {
        return Err(format!("project {} has no data checks", p.name));
    }
    for k in &o.checks {
        if set.check(k).is_none() {
            return Err(format!("check '{k}' not found in {}", p.name));
        }
    }
    let (env_name, vars, _production) = environment(Some(&p), o.env.as_deref())?;
    let req = RunRequest {
        project_id: p.id.clone(),
        project_name: p.name.clone(),
        environment: env_name.clone(),
        vars,
        only: o.checks.clone(),
    };
    if !o.json {
        println!(
            "Checking data of '{}'{} ({} check(s))",
            p.name,
            env_name
                .as_deref()
                .map(|e| format!(" on {e}"))
                .unwrap_or_default(),
            if o.checks.is_empty() {
                set.checks.len()
            } else {
                o.checks.len()
            }
        );
    }
    let json = o.json;
    let run = data_checks::run_and_save(&set, &req, None, &mut |r| {
        if json {
            return;
        }
        println!(
            "  [{:<7}] {:<8} {}{}{}",
            r.status.label(),
            r.severity.label(),
            r.name,
            r.row_count
                .map(|n| format!("  {n}{} row(s)", if r.truncated { "+" } else { "" }))
                .unwrap_or_default(),
            r.trend()
                .map(|t| format!(" ({t} since last run)"))
                .unwrap_or_default()
        );
        if let Some(e) = r.error.as_ref().filter(|_| r.status == CheckStatus::Error) {
            println!("            {e}");
        }
    })?;
    for (path, junit) in [(o.report.as_deref(), false), (o.junit.as_deref(), true)] {
        let Some(path) = path else { continue };
        let text = if junit {
            data_checks::to_junit(&run)
        } else {
            data_checks::render_for_path(path, &run, &set)
        };
        std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))?;
        if !json {
            println!("Report written to {path}");
        }
    }
    if json {
        println!("{}", data_checks::to_json(&run));
    } else {
        println!(
            "\n{}: {} healthy, {} failing, {} error, {} skipped",
            run.verdict(),
            run.count(CheckStatus::Healthy),
            run.count(CheckStatus::Failing),
            run.count(CheckStatus::Error),
            run.count(CheckStatus::Skipped)
        );
    }
    if run.critical_failed() {
        Err("a Critical data check failed".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_check_options() {
        let o = parse(&args(&[
            "--project",
            "VOLTUNES",
            "--env",
            "Production",
            "--check",
            "chk_dup_receipt",
            "--junit",
            "out.xml",
        ]))
        .expect("valid");
        assert_eq!(o.project.as_deref(), Some("VOLTUNES"));
        assert_eq!(o.env.as_deref(), Some("Production"));
        assert_eq!(o.checks, vec!["chk_dup_receipt"]);
        assert_eq!(o.junit.as_deref(), Some("out.xml"));
        assert!(parse(&args(&["--env", "x"])).is_err());
        assert!(parse(&args(&["--project"])).is_err());
        assert!(parse(&args(&["--project", "p", "--bogus"])).is_err());
    }
}
