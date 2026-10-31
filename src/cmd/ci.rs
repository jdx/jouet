//! `jouet ci`.

use super::Output;
use crate::ci::{self, Report};
use crate::{CiArgs, CiCmd, CiRunArgs};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use jouet_output::time::elapsed;
use std::fmt::Write as _;

const DEFAULT_LOG_LINES: usize = 60;
/// Failed steps shown in full; the rest are named.
const SHOWN_STEPS: usize = 3;

pub(crate) fn run(args: CiArgs, out: &Output) -> Exit {
    match args.cmd {
        CiCmd::Run(a) => run_local(&a, out),
    }
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
