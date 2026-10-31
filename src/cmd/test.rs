//! `jouet test`: the full suite, or with `--affected` only the tests a
//! change affects, explained.

use super::Output;
use crate::TestArgs;
use crate::affected::{self, Decision, Plan, Suite};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// How many selected tests the text output explains before counting the
/// rest; the runner lists what it runs anyway.
const TESTS_SHOWN: usize = 20;
/// How many ignored files the text output names before counting the rest.
const IGNORED_SHOWN: usize = 8;

#[derive(Serialize)]
struct Report {
    affected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<affected::Base>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<usize>,
    suites: Vec<SuiteReport>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    ok: bool,
}

#[derive(Serialize)]
struct SuiteReport {
    adapter: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision: Option<String>,
    /// The directory it runs in, from the repository root; `.` at the root.
    dir: String,
    /// `affected`, `full` or `nothing`.
    decision: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tests: Vec<affected::Test>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<FileReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<Vec<String>>,
    /// Environment variables jouet sets for the command.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

#[derive(Serialize)]
struct FileReport {
    path: String,
    status: affected::Status,
    /// `source`, `ignored` or `full`.
    effect: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// A command to run, where, and what its exit codes mean.
struct Job {
    argv: Vec<String>,
    env: Vec<(String, String)>,
    dir: PathBuf,
    exit: Box<dyn Fn(i32) -> Exit>,
}

pub(crate) fn run(args: &TestArgs, out: &Output) -> Exit {
    if !args.affected && (args.since.is_some() || args.adapter.is_some()) {
        return out.error_with_hint(
            Exit::Usage,
            "--since and --adapter select tests, so they need --affected",
            Some("jouet test --affected --since main"),
        );
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let planned = if args.affected {
        affected(&cwd, args)
    } else {
        full(&cwd, args)
    };
    let (mut report, jobs) = match planned {
        Ok(planned) => planned,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    if !out.json {
        print!("{}", text(&report));
    }
    let mut exit = Exit::Ok;
    if !args.dry_run {
        for (suite, job) in report.suites.iter_mut().zip(&jobs) {
            let Some(job) = job else { continue };
            let code = match execute(job, out.json) {
                Ok(code) => code,
                Err(e) => return out.error(Exit::Precondition, &e),
            };
            suite.exit_code = Some(code.0);
            suite.duration_ms = Some(code.1);
            let suite_exit = (job.exit)(code.0);
            if suite_exit == Exit::Failed || exit == Exit::Ok {
                exit = suite_exit;
            }
            if !out.json {
                println!("{}", outcome(suite));
            }
        }
    }
    report.ok = exit == Exit::Ok;
    if out.json {
        out.result(&report, String::new);
    } else if args.affected && !args.dry_run && jobs.iter().any(Option::is_some) {
        println!(
            "This was a partial run. Run the full suite (`jouet test`) before you finish or push."
        );
    }
    exit
}

/// Runs a job with the terminal's output, or with its stdout sent to
/// stderr when stdout holds the JSON document. Its exit code (1 when a
/// signal ended it) and how long it took.
fn execute(job: &Job, json: bool) -> Result<(i32, u64), String> {
    let mut command = Command::new(&job.argv[0]);
    command
        .args(&job.argv[1..])
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&job.dir)
        .stdin(Stdio::null());
    if json {
        command.stdout(std::io::stderr());
    }
    let started = Instant::now();
    let status = command
        .status()
        .map_err(|e| format!("running {}: {e}", job.argv[0]))?;
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok((status.code().unwrap_or(1), ms))
}

/// `jouet test`: the repository's test task, or the whole suite of each
/// runner found.
fn full(cwd: &Path, args: &TestArgs) -> Result<(Report, Vec<Option<Job>>), affected::Error> {
    let root = cwd
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf();
    if let Some(mut argv) = jouet_routines::prims::test_task(&root) {
        // Every package, not only the one at a workspace's root.
        if argv == ["cargo", "test"] {
            argv.push("--workspace".into());
        }
        // npm only passes on what follows `--`.
        if argv[0] == "npm" && !args.extra.is_empty() {
            argv.push("--".into());
        }
        argv.extend(args.extra.iter().cloned());
        let report = SuiteReport {
            adapter: "task".into(),
            precision: None,
            dir: ".".into(),
            decision: "full",
            reason: Some("the repository's test task".into()),
            tests: Vec::new(),
            files: Vec::new(),
            note: None,
            command: Some(argv.clone()),
            env: BTreeMap::new(),
            exit_code: None,
            duration_ms: None,
        };
        let job = Job {
            argv,
            env: Vec::new(),
            dir: root,
            exit: Box::new(|code| if code == 0 { Exit::Ok } else { Exit::Failed }),
        };
        return Ok((
            report_of(false, None, None, vec![report], args.dry_run),
            vec![Some(job)],
        ));
    }
    let (_, project, names) = affected::locate(cwd, None).map_err(|e| {
        affected::Error::new(
            Exit::Precondition,
            format!(
                "no test task in {}, and no test runner jouet knows: looked for a mise `test` task, a package.json `test` script, Cargo.toml, a justfile or Makefile `test` target, go.mod, and {} projects",
                root.display(),
                affected::names()
            ),
        )
        .hint(e.message)
    })?;
    let mut reports = Vec::new();
    let mut jobs = Vec::new();
    for name in names {
        let suite = affected::full_suite(name, &project, &args.extra)?;
        let argv = suite.command.clone().unwrap_or_default();
        // How it would select says nothing about a full run.
        let mut report = suite_report(&suite);
        report.precision = None;
        reports.push(report);
        jobs.push(Some(job_of(suite, argv)));
    }
    Ok((report_of(false, None, None, reports, args.dry_run), jobs))
}

/// `jouet test --affected`.
fn affected(cwd: &Path, args: &TestArgs) -> Result<(Report, Vec<Option<Job>>), affected::Error> {
    let Plan {
        base,
        changes,
        suites,
    } = affected::plan(
        cwd,
        args.since.as_deref(),
        args.adapter.as_deref(),
        &args.extra,
    )?;
    let reports = suites.iter().map(suite_report).collect();
    let jobs = suites
        .into_iter()
        .map(|suite| {
            let argv = suite.command.clone()?;
            Some(job_of(suite, argv))
        })
        .collect();
    Ok((
        report_of(true, Some(base), Some(changes.len()), reports, args.dry_run),
        jobs,
    ))
}

fn report_of(
    affected: bool,
    base: Option<affected::Base>,
    changed: Option<usize>,
    suites: Vec<SuiteReport>,
    dry_run: bool,
) -> Report {
    Report {
        affected,
        base,
        changed,
        suites,
        dry_run,
        ok: true,
    }
}

fn job_of(suite: Suite, argv: Vec<String>) -> Job {
    let adapter = suite.adapter;
    Job {
        argv,
        env: adapter.env(),
        dir: suite.project.dir,
        exit: Box::new(move |code| adapter.exit(code)),
    }
}

fn suite_report(suite: &Suite) -> SuiteReport {
    let (decision, reason) = match &suite.decision {
        Decision::Affected => ("affected", None),
        Decision::Full(why) => ("full", Some(why.clone())),
        Decision::Nothing(why) => ("nothing", Some(why.clone())),
    };
    let files = suite
        .files
        .iter()
        .map(|f| {
            let (effect, reason) = match &f.effect {
                affected::Effect::Source => ("source", None),
                affected::Effect::Ignored(why) => ("ignored", Some(why.clone())),
                affected::Effect::Full(why) => ("full", Some(why.clone())),
            };
            FileReport {
                path: f.path.clone(),
                status: f.status,
                effect,
                reason,
            }
        })
        .collect();
    SuiteReport {
        adapter: suite.adapter.name().into(),
        precision: Some(suite.adapter.precision()),
        dir: if suite.project.rel.is_empty() {
            ".".into()
        } else {
            suite.project.rel.clone()
        },
        decision,
        reason,
        tests: suite.tests.clone(),
        files,
        note: suite.note.clone(),
        command: suite
            .command
            .as_ref()
            .map(|argv| shown(argv, &suite.project.dir)),
        env: suite.adapter.env().into_iter().collect(),
        exit_code: None,
        duration_ms: None,
    }
}

/// A command as shown: the program as a path from `dir` when it is in it.
fn shown(argv: &[String], dir: &Path) -> Vec<String> {
    let mut argv = argv.to_vec();
    if let Some(first) = argv.first_mut()
        && let Ok(rel) = Path::new(first.as_str()).strip_prefix(dir)
    {
        *first = rel.to_string_lossy().replace('\\', "/");
    }
    argv
}

fn text(report: &Report) -> String {
    let mut s = String::new();
    if let Some(base) = &report.base {
        let short = base.merge_base.get(..12).unwrap_or(&base.merge_base);
        let changed = report.changed.unwrap_or(0);
        let _ = writeln!(
            s,
            "changes since {} (merge base {short}): {changed} {}",
            base.name,
            if changed == 1 { "file" } else { "files" }
        );
    }
    for suite in &report.suites {
        let name = match &suite.precision {
            Some(precision) => format!("{} ({precision})", suite.adapter),
            None => suite.adapter.clone(),
        };
        match suite.decision {
            "affected" => {
                let n = suite.tests.len();
                let _ = writeln!(
                    s,
                    "{name}: {n} test {}",
                    if n == 1 { "file" } else { "files" }
                );
                for test in suite.tests.iter().take(TESTS_SHOWN) {
                    let why = match test.via.as_slice() {
                        [] => format!("picked by {}", suite.adapter),
                        [only] if *only == test.path => "changed".into(),
                        chain => {
                            let mut chain: Vec<&str> = chain.iter().map(String::as_str).collect();
                            chain.pop();
                            chain.reverse();
                            format!("imports {} (changed)", chain.join(" → "))
                        }
                    };
                    let _ = writeln!(s, "  {}: {why}", test.path);
                }
                if n > TESTS_SHOWN {
                    let _ = writeln!(
                        s,
                        "  ... and {} more (--dry-run --json lists every test and why)",
                        n - TESTS_SHOWN
                    );
                }
            }
            "full" if !report.affected => {
                let _ = writeln!(
                    s,
                    "{name}: the full suite ({})",
                    suite.reason.as_deref().unwrap_or("")
                );
            }
            "full" => {
                let _ = writeln!(
                    s,
                    "{name}: the whole suite, because {}",
                    suite.reason.as_deref().unwrap_or("")
                );
            }
            _ => {
                let _ = writeln!(
                    s,
                    "{name}: nothing to run: {}",
                    suite.reason.as_deref().unwrap_or("")
                );
            }
        }
        if let Some(note) = &suite.note {
            let _ = writeln!(s, "  note: {note}");
        }
        let ignored: Vec<&FileReport> = suite
            .files
            .iter()
            .filter(|f| f.effect == "ignored")
            .collect();
        if !ignored.is_empty() && suite.decision != "nothing" {
            let names: Vec<String> = ignored
                .iter()
                .take(IGNORED_SHOWN)
                .map(|f| format!("{} ({})", f.path, f.reason.as_deref().unwrap_or("")))
                .collect();
            let more = ignored.len().saturating_sub(IGNORED_SHOWN);
            let more = if more > 0 {
                format!(", and {more} more (--json lists them)")
            } else {
                String::new()
            };
            let _ = writeln!(s, "  ignored: {}{more}", names.join(", "));
        }
        if let Some(command) = &suite.command {
            let at = if suite.dir == "." {
                String::new()
            } else {
                format!(" (in {})", suite.dir)
            };
            let verb = if report.dry_run { "would run" } else { "run" };
            let env: String = suite
                .env
                .iter()
                .map(|(k, v)| format!("{k}={} ", affected::quote(v)))
                .collect();
            let _ = writeln!(s, "  {verb}: {env}{}{at}", affected::show(command));
        }
    }
    s
}

/// How a suite's run ended, in a line.
fn outcome(suite: &SuiteReport) -> String {
    let secs = suite.duration_ms.unwrap_or(0) as f64 / 1000.0;
    match suite.exit_code {
        Some(0) => format!("{}: passed in {secs:.1}s", suite.adapter),
        Some(code) => format!("{}: failed (exit {code}) in {secs:.1}s", suite.adapter),
        None => format!("{}: did not run", suite.adapter),
    }
}
