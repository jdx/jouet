//! `jouet cost`: what builds, tests and lints cost here, from the timings
//! [`crate::cost`] keeps in git notes.

use super::Output;
use crate::cli::{CostArgs, CostCmd};
use crate::cost::{Costs, Estimate, NOTES};
use jouet_audit::digest::duration;
use jouet_git::Git;
use jouet_git::notes::Notes;
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write;

#[derive(Serialize)]
struct Report {
    commands: Vec<Estimate>,
    /// This machine's time over the others', over `scaled_by` commands
    /// timed in both places; 1 when there are none.
    scale: f64,
    scaled_by: usize,
}

pub(crate) fn run(args: &CostArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let notes = Notes::new(Git::new(&cwd), NOTES);
    match &args.cmd {
        Some(CostCmd::Fetch(a)) => {
            let remote = a.remote.as_deref().unwrap_or("origin");
            return match notes.fetch(remote) {
                Ok(true) => {
                    out.result(&serde_json::json!({ "fetched": true }), || {
                        format!("fetched refs/notes/{NOTES} from {remote}\n")
                    });
                    Exit::Ok
                }
                Ok(false) => out.error(
                    Exit::NotFound,
                    &format!("{remote} has no refs/notes/{NOTES}"),
                ),
                Err(e) => out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
            };
        }
        Some(CostCmd::Push(a)) => {
            let remote = a.remote.as_deref().unwrap_or("origin");
            return match notes.push(remote) {
                Ok(()) => {
                    out.result(&serde_json::json!({ "pushed": true }), || {
                        format!("pushed refs/notes/{NOTES} to {remote}\n")
                    });
                    Exit::Ok
                }
                Err(e) => out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
            };
        }
        None => {}
    }
    let min = match args.min.as_deref().map(jouet_output::time::parse_duration) {
        None => 0,
        Some(Ok(d)) => u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--min: {e}")),
    };
    let costs = Costs::load(&cwd);
    let report = Report {
        commands: costs.table().into_iter().filter(|e| e.ms >= min).collect(),
        scale: costs.scale(),
        scaled_by: costs.scaled_by,
    };
    out.result(&report, || text(&report, costs.is_empty()));
    Exit::Ok
}

fn text(report: &Report, empty: bool) -> String {
    if empty {
        return format!(
            "no timings yet: `jouet test` and the hooks keep one for each build, test and lint run, and `jouet cost fetch` brings CI's (refs/notes/{NOTES})\n"
        );
    }
    let width = report
        .commands
        .iter()
        .map(|e| e.key.chars().count())
        .max()
        .unwrap_or(0);
    let mut s = String::new();
    for e in &report.commands {
        let basis = match e.scale {
            None => format!("{} {} here", e.here, runs(e.here)),
            Some(scale) if report.scaled_by > 0 => {
                format!(
                    "{} {} elsewhere, × {scale:.1}",
                    e.elsewhere,
                    runs(e.elsewhere)
                )
            }
            Some(_) => format!("{} {} elsewhere, unscaled", e.elsewhere, runs(e.elsewhere)),
        };
        let _ = writeln!(s, "{:<width$}  {:>7}  {basis}", e.key, duration(e.ms));
    }
    if report.scaled_by > 0 {
        let _ = writeln!(
            s,
            "this machine takes {:.1}× as long as the others, over {} {} timed in both",
            report.scale,
            report.scaled_by,
            if report.scaled_by == 1 {
                "command"
            } else {
                "commands"
            }
        );
    }
    s
}

fn runs(n: usize) -> &'static str {
    if n == 1 { "run" } else { "runs" }
}
