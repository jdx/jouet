//! `jouet pr status`, and the `--settle` default `pr land` shares with it.

use super::Output;
use crate::{PrArgs, PrCmd, StatusArgs};
use jouet_github::Client;
use jouet_github::pr::select::DEFAULT_SETTLE;
pub(crate) use jouet_github::pr::select::{Report, Selection, fetch, lookups};
use jouet_github::pr::status::{Mergeability, PrStatus, State};
use jouet_output::exit::Exit;
use jouet_output::time;
use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Duration;

/// Titles longer than this are cut at a word boundary in text output.
const TITLE_WIDTH: usize = 40;
/// Open Entire findings listed under a PR; `pr threads` lists them all.
const FINDINGS: usize = 3;

pub(crate) fn run(args: PrArgs, out: &Output) -> ExitCode {
    match args.cmd {
        PrCmd::Status(args) => run_status(&args, out).into(),
        PrCmd::Next(args) => super::next::run(&args, out),
        PrCmd::Land(args) => super::land::run(&args, out).into(),
        PrCmd::Threads(args) => super::threads::run(args, out).into(),
    }
}

/// `--settle`, or its default.
pub(crate) fn settle_arg(settle: Option<&str>) -> Result<Duration, String> {
    match settle.map(time::parse_duration) {
        None => Ok(DEFAULT_SETTLE),
        Some(Ok(d)) => Ok(d),
        Some(Err(e)) => Err(format!("--settle: {e}")),
    }
}

fn run_status(args: &StatusArgs, out: &Output) -> Exit {
    let settle = match settle_arg(args.settle.as_deref()) {
        Ok(settle) => settle,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let selection = Selection {
        prs: &args.prs,
        mine: args.mine,
        search: args.search.as_deref(),
        repo: &args.repo,
        checkout: &jouet_github::pr::select::Git,
    };
    let lookups = match lookups(&selection) {
        Ok(lookups) => lookups,
        Err(e) => return out.error(Exit::Usage, &e),
    };
    let report = match fetch(&mut Client::from_env(), lookups, settle) {
        Ok(report) => report,
        Err(e) => return out.error(Exit::Remote, &e),
    };
    out.result(&report, || render(&report));
    if !report.not_found.is_empty() {
        Exit::NotFound
    } else if args.fail_on_attention && report.prs.iter().any(|p| !p.attention.is_empty()) {
        Exit::Failed
    } else {
        Exit::Ok
    }
}

/// One line per PR, a line naming any that were not found, then the summary.
fn render(report: &Report) -> String {
    let one_repo = report.prs.windows(2).all(|w| w[0].repo == w[1].repo);
    let name = |p: &PrStatus| {
        if one_repo {
            format!("#{}", p.number)
        } else {
            format!("{}#{}", p.repo, p.number)
        }
    };
    let rows: Vec<[String; 5]> = report
        .prs
        .iter()
        .map(|p| {
            [
                name(p),
                p.state.as_str().to_string(),
                cut_title(&p.title),
                merge_word(p).to_string(),
                ci(p),
            ]
        })
        .collect();
    let width = |col: usize| {
        rows.iter()
            .map(|r| r[col].chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths = [width(0), width(1), width(2), width(3), width(4)];
    let mut s = String::new();
    for (p, row) in report.prs.iter().zip(&rows) {
        let review = match p.review.as_deref() {
            None => "-",
            Some("review_required") => "required",
            Some(r) => r,
        };
        let mut line = format!(
            "{:<w0$} {:<w1$} {:<w2$}  {:<w3$}  {:<w4$}  threads {}  review {review}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            p.unresolved_threads,
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
        );
        if p.auto_merge {
            line.push_str("  auto-merge");
        }
        s.push_str(&line);
        s.push('\n');
        entire_lines(&mut s, p, &format!("{}#{}", p.repo, p.number));
    }
    if !report.not_found.is_empty() {
        let _ = writeln!(s, "not found: {}", report.not_found.join(", "));
    }
    for i in &report.incomplete {
        let _ = writeln!(
            s,
            "search `{}` matched {} PRs but GitHub listed only {} (it lists at most 1000)",
            i.query, i.matched, i.listed
        );
    }
    let n = report.prs.len();
    let _ = write!(s, "{n} {}", if n == 1 { "PR" } else { "PRs" });
    let needing: Vec<String> = report
        .prs
        .iter()
        .filter(|p| !p.attention.is_empty())
        .map(|p| {
            let codes: Vec<&str> = p.attention.iter().map(|a| a.code).collect();
            format!("{} ({})", name(p), codes.join(", "))
        })
        .collect();
    match needing.len() {
        0 if n > 0 => s.push_str(", nothing needs attention"),
        0 => {}
        1 => {
            let _ = write!(s, ", 1 needs attention: {}", needing[0]);
        }
        k => {
            let _ = write!(s, ", {k} need attention: {}", needing.join(", "));
        }
    }
    if !report.not_found.is_empty() {
        let _ = write!(s, "; {} not found", report.not_found.len());
    }
    let unlisted: u64 = report.incomplete.iter().map(|i| i.matched - i.listed).sum();
    if unlisted > 0 {
        let _ = write!(
            s,
            "; {unlisted} more matched the search but were not listed"
        );
    }
    s.push('\n');
    s
}

/// Under a PR whose Entire Gates check fails: which gates fail and why,
/// then its open findings, up to `FINDINGS` of them.
fn entire_lines(s: &mut String, p: &PrStatus, name: &str) {
    let Some(e) = p.entire.as_ref().filter(|e| e.gates.failed()) else {
        return;
    };
    let _ = write!(s, "  entire: {}", e.gates.failing_text());
    if let Some(trail) = e.gates.trail {
        let _ = write!(s, " (trail #{trail})");
    }
    if e.gates.derived() {
        s.push_str("; repeats what is reported above");
    }
    s.push('\n');
    for f in e.findings.iter().take(FINDINGS) {
        let _ = write!(s, "    [{}] {}", f.handle, f.location);
        if let Some(severity) = &f.severity {
            let _ = write!(s, "  {severity}");
        }
        let _ = writeln!(s, "  {}", f.title);
    }
    let more = e.findings.len().saturating_sub(FINDINGS);
    if more > 0 || e.findings_incomplete {
        let n = if e.findings_incomplete {
            format!("{more}+")
        } else {
            more.to_string()
        };
        let _ = writeln!(s, "    ... {n} more: jouet pr threads {name}");
    }
}

/// Cuts a title to `TITLE_WIDTH` characters at a word boundary, marking the
/// cut with `...`.
fn cut_title(title: &str) -> String {
    if title.chars().count() <= TITLE_WIDTH {
        return title.to_string();
    }
    let head: String = title.chars().take(TITLE_WIDTH).collect();
    let head = match head.rfind(' ') {
        Some(i) if i >= TITLE_WIDTH / 2 => &head[..i],
        _ => &head,
    };
    format!("{}...", head.trim_end())
}

fn merge_word(p: &PrStatus) -> &'static str {
    if !matches!(p.state, State::Open | State::Draft) {
        return "-";
    }
    match p.mergeable {
        Mergeability::Mergeable => "ok",
        Mergeability::Conflicting => "CONFLICT",
        Mergeability::Unknown => "unknown",
    }
}

/// `ci 41/46 fail: lint, unit`: passing out of all checks, then what failed
/// and whether any are still running.
fn ci(p: &PrStatus) -> String {
    let c = &p.checks;
    if c.total() == 0 {
        return "ci none".into();
    }
    let mut s = format!("ci {}/{}", c.pass, c.total());
    if c.fail > 0 {
        let _ = write!(s, " fail: {}", c.failing.join(", "));
    }
    if c.pending > 0 {
        s.push_str(if c.fail > 0 { "; pending" } else { " pending" });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cut_at_a_word() {
        assert_eq!(
            cut_title("docs: describe current trust rules"),
            "docs: describe current trust rules"
        );
        assert_eq!(
            cut_title("fix(vfox): bound header lock downloads by http_download_timeout"),
            "fix(vfox): bound header lock downloads..."
        );
        let long_word = "x".repeat(50);
        assert_eq!(cut_title(&long_word), format!("{}...", "x".repeat(40)));
    }

    #[test]
    fn a_cut_short_search_is_reported() {
        let report = Report {
            prs: Vec::new(),
            not_found: Vec::new(),
            incomplete: vec![jouet_github::pr::status::Incomplete {
                query: "is:pr is:merged repo:jdx/mise".into(),
                matched: 1234,
                listed: 1000,
            }],
            entire: None,
        };
        assert_eq!(
            render(&report),
            "search `is:pr is:merged repo:jdx/mise` matched 1234 PRs but GitHub listed only 1000 \
             (it lists at most 1000)\n\
             0 PRs; 234 more matched the search but were not listed\n"
        );
    }
}
