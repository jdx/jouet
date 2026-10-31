//! `jouet diag check`, `jouet diag delta` and `jouet diag fix`.

use super::Output;
use crate::diag::{self, Report, Status, delta, fix};
use crate::{DiagArgs, DiagCmd};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::PathBuf;

/// How many findings are shown unless `--max` says otherwise.
const DEFAULT_MAX: usize = 50;

pub(crate) fn run(args: &DiagArgs, out: &Output) -> Exit {
    match &args.cmd {
        DiagCmd::Check(a) => check(&a.files, a.changed, a.max, out),
        DiagCmd::Delta(a) => run_delta(&a.files, a.changed, a.max, out),
        DiagCmd::Fix(a) => run_fix(&a.files, a.changed, &a.step, a.dry_run, out),
    }
}

/// The repository root (or the current directory outside one) and the
/// files to work on, relative to it.
pub(crate) fn target(files: &[String], changed: bool) -> Result<(PathBuf, Vec<String>), String> {
    let cwd = std::env::current_dir()
        .and_then(|d| d.canonicalize())
        .map_err(|e| format!("reading the current directory: {e}"))?;
    let root = crate::git::toplevel().unwrap_or_else(|| cwd.clone());
    let root = root.canonicalize().unwrap_or(root);
    if !files.is_empty() && changed {
        return Err("give files or --changed, not both".into());
    }
    let files = if files.is_empty() {
        diag::changed(&root)?
    } else {
        diag::resolve(&root, &cwd, files)?
    };
    Ok((root, files))
}

/// Checks `files` and remembers what each file had, for `delta`.
pub(crate) fn checked(root: &std::path::Path, files: Vec<String>) -> Report {
    let report = diag::check(root, files, diag::CHECK_TIMEOUT);
    if matches!(report.status, Status::Clean | Status::Findings)
        && let Some(path) = delta::cache_path(root)
    {
        let seen = delta::seen(root, &report);
        delta::update(&path, |cache| cache.record(seen));
    }
    report
}

fn exit_for(status: Status) -> Exit {
    match status {
        Status::Clean => Exit::Ok,
        Status::Findings => Exit::Failed,
        Status::Refused | Status::Unavailable => Exit::Precondition,
    }
}

fn check(files: &[String], changed: bool, max: Option<usize>, out: &Output) -> Exit {
    let (root, files) = match target(files, changed) {
        Ok(t) => t,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let mut report = checked(&root, files);
    let max = max.unwrap_or(DEFAULT_MAX);
    report.cut(max);
    out.result(&report, || diag::render(&report, max));
    exit_for(report.status)
}

#[derive(Serialize)]
struct DeltaReport {
    /// clean, introduced, refused or unavailable.
    status: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    source: String,
    files: Vec<String>,
    #[serde(flatten)]
    delta: delta::Delta,
    /// New findings left out by `--max`.
    #[serde(skip_serializing_if = "is_zero")]
    omitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_path: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn run_delta(files: &[String], changed: bool, max: Option<usize>, out: &Output) -> Exit {
    let (root, files) = match target(files, changed) {
        Ok(t) => t,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let cache_path = delta::cache_path(&root);
    let cache = cache_path
        .as_deref()
        .map(delta::Cache::load)
        .unwrap_or_default();
    let report = checked(&root, files);
    let mut d = match report.status {
        Status::Clean | Status::Findings => {
            delta::against_cache(&root, &cache, &report, |f| crate::git::unchanged(&root, f))
        }
        Status::Refused | Status::Unavailable => delta::Delta::default(),
    };
    let max = max.unwrap_or(DEFAULT_MAX);
    let omitted = d.new.len().saturating_sub(max);
    d.new.truncate(max);
    d.unknown.truncate(max);
    let status = match report.status {
        Status::Refused => "refused",
        Status::Unavailable => "unavailable",
        _ if d.introduced() || omitted > 0 => "introduced",
        _ => "clean",
    };
    let doc = DeltaReport {
        status,
        source: report.source.clone(),
        files: report.files.clone(),
        delta: d,
        omitted,
        reason: report.reason.clone(),
        duration_ms: report.duration_ms,
        log_path: report.log_path.clone(),
    };
    out.result(&doc, || render_delta(&doc));
    match status {
        "clean" => Exit::Ok,
        "introduced" => Exit::Failed,
        _ => Exit::Precondition,
    }
}

fn render_delta(doc: &DeltaReport) -> String {
    let mut s = String::new();
    match doc.status {
        "refused" => {
            let _ = writeln!(
                s,
                "hk refused to check: {}",
                doc.reason.as_deref().unwrap_or("")
            );
            return s;
        }
        "unavailable" => {
            let _ = writeln!(
                s,
                "diagnostics unavailable: {}",
                doc.reason.as_deref().unwrap_or("")
            );
            return s;
        }
        _ => {}
    }
    s.push_str(&delta_lines(
        &doc.delta,
        doc.omitted,
        doc.log_path.as_deref(),
    ));
    let _ = writeln!(
        s,
        "{} new, {} kept, {} fixed in {} {} ({}, {} ms)",
        doc.delta.new.len() + doc.omitted,
        doc.delta.kept,
        doc.delta.fixed,
        doc.files.len(),
        diag::plural(doc.files.len(), "file"),
        if doc.source.is_empty() {
            "no checker"
        } else {
            &doc.source
        },
        doc.duration_ms
    );
    s
}

/// The new findings and failed steps of a delta, then the files without a
/// baseline, one line each.
pub(crate) fn delta_lines(d: &delta::Delta, omitted: usize, log: Option<&str>) -> String {
    let mut s = String::new();
    for f in &d.new {
        let _ = writeln!(s, "new: {}", diag::finding_line(f));
    }
    if omitted > 0 {
        let whole = log.map_or(String::new(), |l| format!(" (hk's full result: {l})"));
        let _ = writeln!(s, "... {omitted} more new findings{whole}");
    }
    for step in &d.new_failed_steps {
        let _ = writeln!(s, "new failure: step {}", step.name);
        for line in step.excerpt.as_deref().unwrap_or("(no output)").lines() {
            let _ = writeln!(s, "  {line}");
        }
    }
    if !d.no_baseline.is_empty() {
        let _ = writeln!(
            s,
            "no earlier check of {} ({} {} not counted as new; check before editing next time)",
            d.no_baseline.join(", "),
            d.unknown.len(),
            diag::plural(d.unknown.len(), "finding")
        );
        for f in &d.unknown {
            let _ = writeln!(s, "  {}", diag::finding_line(f));
        }
    }
    s
}

#[derive(Serialize)]
struct FixReport<'a> {
    files: &'a [String],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    fixed: Option<&'a fix::Fixed>,
}

fn run_fix(files: &[String], changed: bool, steps: &[String], dry_run: bool, out: &Output) -> Exit {
    let (root, files) = match target(files, changed) {
        Ok(t) => t,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    if diag::hk_config(&root).is_none() {
        return out.error_with_hint(
            Exit::Precondition,
            &format!("no hk config (hk.pkl) in {}", root.display()),
            Some("jouet diag fix runs the fix steps of the project's hk config"),
        );
    }
    if dry_run {
        let doc = FixReport {
            files: &files,
            dry_run: true,
            fixed: None,
        };
        out.result(&doc, || {
            format!(
                "dry run: would run `hk run fix --safe` on {} {}{}\n",
                files.len(),
                diag::plural(files.len(), "file"),
                if steps.is_empty() {
                    String::new()
                } else {
                    format!(" (steps: {})", steps.join(", "))
                }
            )
        });
        return Exit::Ok;
    }
    let fixed = fix::run(&root, &files, steps, diag::CHECK_TIMEOUT);
    crate::ledger::record_fix(&root, None, "cli", None, &fixed);
    let doc = FixReport {
        files: &files,
        dry_run: false,
        fixed: Some(&fixed),
    };
    out.result(&doc, || {
        fixed.summary().map_or_else(
            || "hk fix changed nothing\n".to_string(),
            |line| format!("{line}\n"),
        )
    });
    match fixed.status {
        fix::FixStatus::Ran if fixed.failed_steps.is_empty() => Exit::Ok,
        fix::FixStatus::Ran | fix::FixStatus::Failed | fix::FixStatus::TimedOut => Exit::Failed,
        fix::FixStatus::Refused | fix::FixStatus::NoConfig => Exit::Precondition,
    }
}
