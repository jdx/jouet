//! `jouet ci`.

use super::Output;
use crate::ci::explain::{self, short_list};
use crate::ci::{self, Report};
use crate::{CiArgs, CiCmd, CiExplainArgs, CiRunArgs};
use jouet_github::Client;
use jouet_github::pr::target::{self, Repo};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use jouet_output::time::elapsed;
use std::fmt::Write as _;

const DEFAULT_LOG_LINES: usize = 60;
const EXPLAIN_LOG_LINES: usize = 40;
/// Failed steps shown in full; the rest are named.
const SHOWN_STEPS: usize = 3;

pub(crate) fn run(args: CiArgs, out: &Output) -> Exit {
    match args.cmd {
        CiCmd::Run(a) => run_local(&a, out),
        CiCmd::Explain(a) => run_explain(&a, out),
    }
}

fn run_explain(a: &CiExplainArgs, out: &Output) -> Exit {
    let target = match explain_target(a) {
        Ok(t) => t,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    let req = explain::Request {
        target,
        rerun: a.rerun,
        log_lines: a.log_lines.unwrap_or(EXPLAIN_LOG_LINES),
    };
    match explain::explain(&mut Client::from_env(), &req) {
        Ok(report) => {
            out.result(&report, || render_explain(&report));
            report.exit()
        }
        Err(r) => out.error_with_hint(r.exit, &r.message, r.hint.as_deref()),
    }
}

/// What to explain: `--run`, `--job`, an Actions URL, or a PR.
fn explain_target(a: &CiExplainArgs) -> Result<explain::Target, String> {
    let repo = || match &a.repo {
        Some(r) => Repo::parse(r).ok_or_else(|| format!("--repo takes OWNER/NAME, not `{r}`")),
        None => target::repo_from_git()
            .ok_or_else(|| "no --repo given and no GitHub remote here".into()),
    };
    match (&a.target, a.run, a.job) {
        (None, Some(id), None) => Ok(explain::Target::Run(repo()?, id)),
        (None, None, Some(id)) => Ok(explain::Target::Job(repo()?, id)),
        (Some(t), None, None) if t.contains("/actions/runs/") => {
            let (url_repo, run) = jouet_wait::run::parse_run(t)
                .ok_or_else(|| format!("not an Actions URL: `{t}`"))?;
            let repo = match url_repo {
                Some(r) => r,
                None => repo()?,
            };
            let job = t
                .split_once("/job/")
                .and_then(|(_, rest)| rest.split(['/', '?', '#']).next()?.parse().ok());
            Ok(match job {
                Some(job) => explain::Target::Job(repo, job),
                None => explain::Target::Run(repo, run),
            })
        }
        (t, None, None) => Ok(explain::Target::Pr(super::wait::target(
            t.as_deref(),
            a.repo.as_deref(),
        )?)),
        _ => Err("give a PR, --run or --job, not more than one".into()),
    }
}

fn render_explain(r: &explain::Report) -> String {
    let mut s = String::new();
    let word = |v: &str| match v {
        "real" => "REAL",
        "flaky" => "FLAKY",
        "infrastructure" => "INFRA",
        "unknown" => "UNKNOWN",
        "passed" => "PASS",
        _ => "PENDING",
    };
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for job in r.runs.iter().flat_map(|run| &run.jobs) {
        let v = job.verdict.as_str();
        match counts.iter_mut().find(|(w, _)| *w == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    let following: usize = r.runs.iter().map(|run| run.following.len()).sum();
    let mut summary: Vec<String> = counts.iter().map(|(w, n)| format!("{n} {w}")).collect();
    if following > 0 {
        summary.push(format!("{following} following"));
    }
    let what = match r.pr {
        Some(n) => format!("{}#{n}", r.repo),
        None => r.repo.clone(),
    };
    let _ = write!(s, "{} {what}", word(r.result));
    if !summary.is_empty() {
        let _ = write!(s, "  {}", summary.join(", "));
    }
    s.push('\n');
    if r.runs.is_empty() {
        s.push_str("  no failed or running workflow runs on the PR's head\n");
    }
    for run in &r.runs {
        let _ = write!(
            s,
            "  run {} {} @ {} on {}, attempt {}",
            run.run_id,
            run.workflow,
            &run.head_sha[..run.head_sha.len().min(8)],
            run.head_branch,
            run.attempt
        );
        for pr in &run.prs {
            let _ = write!(s, ", PR #{pr}");
        }
        if run.in_progress {
            s.push_str(", in progress");
        }
        s.push('\n');
        for (i, job) in run.jobs.iter().enumerate().filter(|(i, j)| j.group == *i) {
            let members: Vec<String> = run
                .jobs
                .iter()
                .filter(|j| j.group == i)
                .map(|j| j.name.clone())
                .collect();
            let _ = write!(
                s,
                "\n{} {}",
                word(job.verdict.as_str()),
                short_list(&members)
            );
            if let (Some(os), 1) = (job.os, members.len()) {
                let _ = write!(s, " ({os})");
            }
            if let Some(step) = &job.failed_step {
                let _ = write!(s, "  step \"{step}\"");
            }
            s.push('\n');
            if !job.tests.is_empty() {
                let _ = write!(s, "  failed: {}", short_list(&job.tests));
                if let Some(at) = &job.location {
                    let _ = write!(s, "  at {at}");
                }
                s.push('\n');
            }
            for error in job.errors.iter().take(SHOWN_STEPS) {
                let _ = writeln!(s, "  {error}");
            }
            for line in &job.evidence {
                let _ = writeln!(s, "  - {line}");
            }
            for line in &job.log_excerpt {
                let _ = writeln!(s, "    {line}");
            }
            if let Some(path) = &job.log_path {
                let _ = writeln!(s, "  full log: {path}");
            }
        }
        let (cancelled, failed): (Vec<_>, Vec<_>) = run
            .following
            .iter()
            .partition(|f| f.conclusion == "cancelled");
        for (list, how) in [(failed, "failed after"), (cancelled, "cancelled after")] {
            if let Some(first) = list.first() {
                let names: Vec<String> = list.iter().map(|f| f.name.clone()).collect();
                let _ = writeln!(
                    s,
                    "\n{how} {} failed, with nothing of their own: {}",
                    first.follows,
                    short_list(&names)
                );
            }
        }
    }
    s.push('\n');
    let rerun = &r.rerun;
    if rerun.started {
        let _ = writeln!(s, "rerun: started for {}", short_list(&rerun.jobs));
        for run in r.runs.iter().filter(|run| !run.jobs.is_empty()) {
            let _ = writeln!(s, "  jouet wait run {} --repo {}", run.run_id, r.repo);
        }
    } else if let Some(command) = rerun.command.as_deref().filter(|_| rerun.allowed) {
        let _ = writeln!(s, "rerun: allowed, {}: {command}", rerun.reason);
    } else if r.result != "passed" {
        let _ = writeln!(s, "rerun: refused, {}", rerun.reason);
    }
    let _ = writeln!(
        s,
        "GitHub: {} requests, {} from the cache",
        r.github.requests, r.github.cached
    );
    s
}

fn run_local(a: &CiRunArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let req = ci::Request {
        workflow: a.workflow.clone(),
        jobs: a.job.clone(),
        check: a.check.clone(),
        event: a.event.clone(),
        extra: a.args.clone(),
        log_lines: a.log_lines.unwrap_or(DEFAULT_LOG_LINES),
        live: out.mode == Mode::Human && !out.json,
        secrets_from_fnox: a.secrets_from.as_deref() == Some("fnox"),
        log_dir: jouet_output::paths::log_dir(),
    };
    match ci::run(&req, &cwd) {
        Ok(report) => {
            out.result(&report, || render(&report));
            report.exit()
        }
        Err(r) => out.error_with_hint(r.exit, &r.message, r.hint.as_deref()),
    }
}

fn render(r: &Report) -> String {
    let mut s = String::new();
    let word = match r.result {
        "success" => "PASS",
        "failure" => "FAIL",
        _ => "ERROR",
    };
    let mut what = String::from("jactionlint run");
    if let Some(w) = &r.workflow {
        let _ = write!(what, " {w}");
    }
    for job in &r.selected {
        let _ = write!(what, " -j {job}");
    }
    let _ = writeln!(
        s,
        "{word} {what}  {}  ({})",
        counts(r),
        elapsed(r.elapsed_s)
    );
    for w in &r.warnings {
        let _ = writeln!(s, "  warning: {w}");
    }
    if let Some(given) = &r.secrets {
        let mut parts = Vec::new();
        for (word, names) in [
            ("given", &given.given),
            ("missing", &given.missing),
            ("multi-line, not given", &given.multiline),
        ] {
            if !names.is_empty() {
                parts.push(format!("{word} {}", names.join(", ")));
            }
        }
        if parts.is_empty() {
            parts.push("the jobs read none".into());
        }
        let _ = writeln!(s, "  secrets from {}: {}", given.from, parts.join("; "));
    }
    for job in r
        .jobs
        .iter()
        .filter(|j| matches!(j.result.as_str(), "failure" | "cancelled"))
    {
        let word = if job.result == "cancelled" {
            "CANCELLED"
        } else {
            "FAIL"
        };
        let _ = write!(s, "\n{word} {}", job.name);
        if let Some(note) = &job.note {
            let _ = write!(s, "  ({note})");
        }
        s.push('\n');
        for step in job.failed_steps.iter().take(SHOWN_STEPS) {
            let _ = write!(s, "  [{}] \"{}\"", step.label, step.name);
            if let Some(note) = &step.note {
                let _ = write!(s, "  {note}");
            }
            s.push('\n');
            for line in &step.excerpt {
                let _ = writeln!(s, "    {line}");
            }
            if let Some(hint) = &step.hint {
                let _ = writeln!(s, "    hint: {hint}");
            }
        }
        let more = job.failed_steps.len().saturating_sub(SHOWN_STEPS);
        if more > 0 {
            let _ = writeln!(s, "  ... {more} more failed steps in the log");
        }
    }
    // What did not run here is what this run cannot vouch for.
    let skipped: Vec<_> = r.jobs.iter().filter(|j| j.result == "skipped").collect();
    if !skipped.is_empty() {
        s.push('\n');
        for job in skipped {
            let _ = write!(s, "skipped {}", job.name);
            if let Some(note) = &job.note {
                let _ = write!(s, "  ({note})");
            }
            s.push('\n');
        }
    }
    if !r.output.is_empty() {
        s.push('\n');
        for line in &r.output {
            let _ = writeln!(s, "  {line}");
        }
    }
    let _ = writeln!(s, "full log: {}", r.log_path);
    s
}

fn counts(r: &Report) -> String {
    let n = |result: &str| r.jobs.iter().filter(|j| j.result == result).count();
    let parts: Vec<String> = [
        ("pass", n("success")),
        ("fail", n("failure") + n("cancelled")),
        ("skipped", n("skipped")),
    ]
    .iter()
    .filter(|(_, c)| *c > 0)
    .map(|(w, c)| format!("{c} {w}"))
    .collect();
    if parts.is_empty() {
        "no jobs ran".into()
    } else {
        parts.join(", ")
    }
}
