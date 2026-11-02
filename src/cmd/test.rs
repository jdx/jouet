//! `jouet test`: the full suite, or with `--affected` only the tests a
//! change affects, explained, or with `--cmd` any test or build command.
//! Each run's output goes to a log, and what it says is digested
//! (`crate::testlog`) for an agent.

use super::Output;
use crate::affected::{self, Decision, Plan, Suite};
use crate::cli::TestArgs;
use crate::testlog::{self, Digest};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

mod baseline;

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
    /// With `--baseline`, a failing run's failures at the merge base.
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline: Option<baseline::Baseline>,
    /// The failure formats declared in `formats/` that loaded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    formats: Vec<String>,
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
    /// The run's whole output.
    #[serde(skip_serializing_if = "Option::is_none")]
    log: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    digest: Option<Digest>,
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
    if args.baseline && args.affected {
        return out.error(
            Exit::Usage,
            "--baseline reruns a whole run's failures at the merge base; the tests --affected picks depend on the changes, so it cannot",
        );
    }
    if !args.affected && (args.adapter.is_some() || (args.since.is_some() && !args.baseline)) {
        return out.error_with_hint(
            Exit::Usage,
            "--since and --adapter select tests, so they need --affected",
            Some("jouet test --affected --since main"),
        );
    }
    if args.cmd && args.affected {
        return out.error(
            Exit::Usage,
            "--cmd runs the command given, so it cannot also select the affected tests",
        );
    }
    if args.cmd && args.extra.is_empty() {
        return out.error_with_hint(
            Exit::Usage,
            "--cmd needs the command after `--`",
            Some("jouet test --cmd -- cargo test -p mycrate"),
        );
    }
    // `--budget` is jouet's, and no runner's: after `--` it would go to the
    // command, which then fails on it before running a test.
    if let Some(word) = args
        .extra
        .iter()
        .find(|w| *w == "--budget" || w.starts_with("--budget="))
    {
        let mut budget = "BYTES";
        let mut rest: Vec<&str> = Vec::new();
        let mut words = args.extra.iter();
        while let Some(w) = words.next() {
            if w == "--budget" {
                budget = words.next().map_or(budget, String::as_str);
            } else if let Some(value) = w.strip_prefix("--budget=") {
                budget = value;
            } else {
                rest.push(w);
            }
        }
        let mut fixed = vec!["jouet", "test", "--budget", budget];
        if args.cmd {
            fixed.push("--cmd");
        }
        fixed.push("--");
        fixed.extend(rest);
        let fixed: Vec<String> = fixed.into_iter().map(quote).collect();
        return out.error_with_hint(
            Exit::Usage,
            &format!(
                "`{word}` after `--` would go to the command, not to jouet: put it before `--`"
            ),
            Some(&fixed.join(" ")),
        );
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let root = cwd
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(&cwd)
        .to_path_buf();
    // Before the run, which may take many minutes, not after it.
    let base = if args.baseline {
        match baseline::base(&root, args.since.as_deref()) {
            Ok(base) => Some(base),
            Err(e) => {
                let mut fixed = vec!["jouet", "test", "--baseline", "--since", "HEAD~1"];
                if args.cmd {
                    fixed.push("--cmd");
                }
                if !args.extra.is_empty() {
                    fixed.push("--");
                    fixed.extend(args.extra.iter().map(String::as_str));
                }
                let fixed: Vec<String> = fixed.into_iter().map(quote).collect();
                let hint = e.hint.unwrap_or_else(|| fixed.join(" "));
                return out.error_with_hint(e.exit, &e.message, Some(&hint));
            }
        }
    } else if auto_baseline(args) {
        // Without --baseline, a failing run is still compared with the merge
        // base when there is one to compare with; nothing is refused.
        baseline::base(&root, args.since.as_deref()).ok()
    } else {
        None
    };
    // A failing full suite is run at the merge base, once per merge base; any
    // other failing run uses only what an earlier one kept.
    let mode = if args.baseline {
        baseline::Mode::Retry
    } else if !args.cmd {
        baseline::Mode::Run
    } else {
        baseline::Mode::Kept
    };
    let planned = if args.cmd {
        Ok(given(&cwd, args))
    } else if args.affected {
        affected(&cwd, args)
    } else {
        full(&cwd, args)
    };
    let (mut report, jobs) = match planned {
        Ok(planned) => planned,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    // Formats declared for runners jouet does not know; one that does not
    // load is said, and the run goes on without it.
    let (formats, problems) = testlog::format::load(&cwd);
    for problem in &problems {
        eprintln!("jouet: warning: {problem}");
    }
    report.formats = formats.iter().map(|f| f.name.clone()).collect();
    // The agent wrote the command it gave, so only a dry run repeats it.
    if !out.json && (!args.cmd || args.dry_run) {
        emit(&text(&report));
    }
    // A person watches the run; an agent reads what it said, once it ends.
    let stream = !out.json && out.mode == Mode::Human;
    let budget = args.budget.unwrap_or(testlog::DEFAULT_BUDGET);
    let mut exit = Exit::Ok;
    // What each failed suite's output names, for --baseline.
    let mut failed: Vec<(&Job, Option<std::collections::BTreeSet<String>>, u64)> = Vec::new();
    // What each failed suite blames on the machine; the run is the
    // machine's only when every one of them does.
    let mut environment: Vec<Option<String>> = Vec::new();
    let runs = !args.dry_run && jobs.iter().any(Option::is_some);
    let mut timed = Vec::new();
    // Marked for the stop gate while it runs; a run that dies early leaves a
    // mark whose process is gone, which holds nothing up.
    if runs {
        super::hook::mark_own_run(&cwd, &own_command(), true);
    }
    if !args.dry_run {
        for (i, (suite, job)) in report.suites.iter_mut().zip(&jobs).enumerate() {
            let Some(job) = job else { continue };
            let log = log_path(i);
            let (code, ms) = match execute(job, &log, stream) {
                Ok(ran) => ran,
                Err(e) => return out.error(Exit::Precondition, &e),
            };
            suite.exit_code = Some(code);
            suite.duration_ms = Some(ms);
            // A job is one command; one that runs several checks is not
            // split between them.
            if let [record] = super::hook::checks(&job.dir, &command_line(&job.argv)).as_slice() {
                timed.push(crate::cost::Sample::of(record, ms, code));
            }
            suite.log = Some(log.display().to_string());
            let output = std::fs::read(&log).unwrap_or_default();
            let output = String::from_utf8_lossy(&output);
            let digest = testlog::digest_with(&output, code != 0, budget, &formats);
            if code != 0 {
                environment.push(testlog::environment_cause(&output, &formats));
            }
            if base.is_some() && code != 0 {
                let ran = digest.passed + digest.failed + digest.skipped;
                failed.push((job, baseline::failure_names(&output, &formats, &root), ran));
            }
            suite.digest = Some(digest);
            let suite_exit = (job.exit)(code);
            if suite_exit == Exit::Failed || exit == Exit::Ok {
                exit = suite_exit;
            }
            if !out.json {
                emit(&outcome(suite, stream));
            }
        }
    }
    if let (Some(base), Exit::Failed) = (base, exit) {
        let named: Option<Vec<_>> = failed
            .into_iter()
            .map(|(job, names, ran)| names.map(|names| (job, names, ran)))
            .collect();
        let found = match named {
            // Said only when asked for: a failing build names no test.
            None if !args.baseline => Ok(None),
            None => Err(baseline::NotCompared::from(
                "the output names no failing test, so there is nothing to compare at the merge base"
                    .to_string(),
            )),
            Some(named) => baseline::compare(&root, base, &formats, &named, mode),
        };
        match found {
            Ok(None) => {}
            Ok(Some(found)) => {
                if !out.json {
                    emit(&found.text());
                }
                if found.none_new() {
                    exit = Exit::Ok;
                }
                report.baseline = Some(found);
            }
            Err(e) => {
                if e.unverifiable && runs {
                    super::hook::mark_unverifiable(&cwd, &own_command());
                }
                if out.json {
                    eprintln!("jouet: baseline: {}", e.message);
                } else {
                    emit(&format!("baseline: {}\n", e.message));
                }
            }
        }
    }
    report.ok = exit == Exit::Ok;
    // Kept by the run itself, since a pipe hides its exit status from the
    // hooks.
    if runs {
        crate::cost::record(&cwd, &timed);
        let blamed = environment
            .iter()
            .all(Option::is_some)
            .then(|| environment.iter().flatten().next().cloned())
            .flatten();
        super::hook::record_own_run(
            &cwd,
            &own_command(),
            i32::from(exit.code()),
            blamed.as_deref(),
        );
        super::hook::mark_own_run(&cwd, &own_command(), false);
    }
    if out.json {
        out.result(&report, String::new);
    } else if args.affected && !args.dry_run && jobs.iter().any(Option::is_some) {
        emit("This was a partial run; the full suite was not verified.\n");
    }
    exit
}

/// Whether a failing run is compared with the merge base without
/// --baseline: not an affected run, whose tests depend on the changes, nor
/// a dry run, and not with `[test] baseline = "off"`.
fn auto_baseline(args: &TestArgs) -> bool {
    !args.affected && !args.dry_run && crate::settings::Settings::load().test.baseline != "off"
}

/// Writes `text` to stdout, ignoring a closed pipe: in `jouet test | head
/// -1`, head may exit after the first line, and the run still has to finish
/// and record its result. `print!` would panic there instead.
fn emit(text: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
}

/// How this `jouet test` was called, as a shell command: what the rules
/// classify, as they would the hook's.
fn own_command() -> String {
    let words: Vec<String> = std::env::args().skip(1).collect();
    format!("jouet {}", command_line(&words))
}

/// `argv` as a shell command line.
fn command_line(argv: &[String]) -> String {
    argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
}

/// `word` as a shell reads it back: alone when it is plain, else in single
/// quotes. A `~` is plain past the first character (`HEAD~1`), where no
/// shell expands it.
fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && !word.starts_with('~')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=+,@%~".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// A new log for the `i`th suite of this run. The first also removes the
/// test logs older than [`LOG_KEPT`].
fn log_path(i: usize) -> PathBuf {
    let dir = jouet_output::paths::log_dir();
    if i == 0 {
        prune_logs(&dir);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    dir.join(format!("test-{stamp}-{}-{i}.log", std::process::id()))
}

/// How long a test run's log is kept: runs come often, and a log can be
/// large.
const LOG_KEPT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn prune_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > LOG_KEPT);
        if old && name.starts_with("test-") && name.ends_with(".log") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Runs a job with its stdout and stderr in `log`, copied to stdout as it
/// runs when `stream`. Its exit code (1 when a signal ended it) and how long
/// it took.
fn execute(job: &Job, log: &Path, stream: bool) -> Result<(i32, u64), String> {
    let saving = |e: std::io::Error| format!("saving the output to {}: {e}", log.display());
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(saving)?;
    }
    let file = File::create(log).map_err(saving)?;
    let mut command = Command::new(&job.argv[0]);
    command
        .args(&job.argv[1..])
        .envs(job.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&job.dir)
        .stdin(Stdio::null())
        .stdout(file.try_clone().map_err(saving)?)
        .stderr(file);
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| format!("running {}: {e}", job.argv[0]))?;
    let status = if stream {
        follow(&mut child, log)
    } else {
        child.wait()
    }
    .map_err(|e| format!("running {}: {e}", job.argv[0]))?;
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok((status.code().unwrap_or(1), ms))
}

/// Copies what the child writes to its log onto stdout until it exits.
fn follow(child: &mut Child, log: &Path) -> std::io::Result<ExitStatus> {
    let mut reader = File::open(log)?;
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 8192];
    loop {
        let status = child.try_wait()?;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buf[..n])?;
        }
        stdout.flush()?;
        if let Some(status) = status {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `jouet test --cmd -- CMD`: the command given, here.
fn given(cwd: &Path, args: &TestArgs) -> (Report, Vec<Option<Job>>) {
    let root = cwd
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd);
    let dir = match cwd.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => ".".into(),
    };
    let report = SuiteReport {
        adapter: "cmd".into(),
        precision: None,
        dir,
        decision: "full",
        reason: Some("the command given".into()),
        tests: Vec::new(),
        files: Vec::new(),
        note: None,
        command: Some(args.extra.clone()),
        env: BTreeMap::new(),
        exit_code: None,
        duration_ms: None,
        log: None,
        digest: None,
    };
    let job = Job {
        argv: args.extra.clone(),
        env: Vec::new(),
        dir: cwd.to_path_buf(),
        exit: Box::new(|code| if code == 0 { Exit::Ok } else { Exit::Failed }),
    };
    (
        report_of(false, None, None, vec![report], args.dry_run),
        vec![Some(job)],
    )
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
            log: None,
            digest: None,
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
        baseline: None,
        formats: Vec::new(),
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
        log: None,
        digest: None,
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
                // cargo selects crates, which depend on each other; the
                // other adapters select test files, which import.
                let crates = suite.adapter == "cargo";
                let (unit, link) = match (crates, n == 1) {
                    (true, true) => ("crate", "depends on"),
                    (true, false) => ("crates", "depends on"),
                    (false, true) => ("test file", "imports"),
                    (false, false) => ("test files", "imports"),
                };
                let _ = writeln!(s, "{name}: {n} {unit}");
                for test in suite.tests.iter().take(TESTS_SHOWN) {
                    let why = match test.via.as_slice() {
                        [] => format!("picked by {}", suite.adapter),
                        [only] if *only == test.path => "changed".into(),
                        chain => {
                            let mut chain: Vec<&str> = chain.iter().map(String::as_str).collect();
                            chain.pop();
                            chain.reverse();
                            format!("{link} {} (changed)", chain.join(" → "))
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
    if !report.formats.is_empty() {
        let _ = writeln!(s, "failure formats: {}", report.formats.join(", "));
    }
    s
}

/// How a suite's run ended: a line with its counts and its log, and for an
/// agent (`!streamed`) the failures it found, cut to the budget.
fn outcome(suite: &SuiteReport, streamed: bool) -> String {
    let secs = suite.duration_ms.unwrap_or(0) as f64 / 1000.0;
    let mut s = match suite.exit_code {
        Some(0) => format!("{}: passed in {secs:.1}s", suite.adapter),
        Some(code) => format!("{}: failed (exit {code}) in {secs:.1}s", suite.adapter),
        None => return format!("{}: did not run\n", suite.adapter),
    };
    let log = suite.log.as_deref().unwrap_or("");
    let Some(digest) = &suite.digest else {
        return s + "\n";
    };
    let counts = counts(digest);
    if !counts.is_empty() {
        let _ = write!(s, ": {counts}");
    }
    if streamed || suite.exit_code == Some(0) {
        let _ = writeln!(s, " (full log: {log})");
        return s;
    }
    s.push('\n');
    for failure in &digest.failures {
        let _ = writeln!(s, "FAIL {}", failure.name);
        for line in &failure.lines {
            if line.is_empty() {
                s.push('\n');
            } else {
                let _ = writeln!(s, "  {line}");
            }
        }
    }
    let more = digest.more_failures.len() as u64 + digest.failures_omitted;
    if more > 0 {
        let what = if more == 1 { "failure" } else { "failures" };
        let _ = write!(s, "... {more} more {what}");
        if !digest.more_failures.is_empty() {
            let _ = write!(s, ": {}", digest.more_failures.join(", "));
            if digest.failures_omitted > 0 {
                let _ = write!(s, ", and {} more", digest.failures_omitted);
            }
        }
        s.push('\n');
    }
    let _ = writeln!(s, "full log: {log}");
    s
}

/// `3 failed, 40 passed, 1 skipped, 2 warnings`, leaving out what is 0.
fn counts(digest: &Digest) -> String {
    let warnings = if digest.warnings == 1 {
        "warning"
    } else {
        "warnings"
    };
    [
        (digest.failed, "failed"),
        (digest.passed, "passed"),
        (digest.skipped, "skipped"),
        (digest.warnings, warnings),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect::<Vec<_>>()
    .join(", ")
}
