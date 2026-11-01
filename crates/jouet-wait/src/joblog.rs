//! Failure logs for GitHub Actions jobs.
//!
//! `GET /repos/{o}/{r}/actions/jobs/{id}/logs` serves a job's log once the
//! job has finished, even while the rest of its run is still going (where
//! `gh run view --log-failed` refuses). The log is one file for the whole
//! job, every line stamped with a time, so the failed step is cut out by its
//! start and end times from the job's step list: from the first `##[group]`
//! at or after the step started, to the next step's `##[group]Run` line or
//! the runner's `Process completed with exit code` line. Composite actions
//! print `##[group]Run` lines of their own, but inside the step's time
//! window, so they do not end the cut. The step's opening group, which lists
//! its script and environment, is cut to its title.

use jouet_output::time::parse_utc;
use regex::Regex;
use std::sync::LazyLock;

/// Lines worth keeping even when they come before the tail.
static INTERESTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("error|FAILED|panicked|assert").expect("a valid regex"));
/// A line of a Rust backtrace, std's or color-eyre's: a frame (`  12:
/// mise::cli::run::{{closure}}`), its location (`at src/main.rs:4:5`), or
/// the lines around them. A numbered line that is a sentence, as eyre's
/// `   0: dummy@latest is not installed`, is not a frame.
static FRAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^\s*(\d+: \S*(::|<unknown>)\S*.*",
        r"|at .+:(\d+|<unknown line>)(:\d+)?",
        r"|⋮ \d+ frames? hidden ⋮",
        r"|━+ BACKTRACE ━+",
        r"|stack backtrace:",
        r"|Run with (COLORBT_SHOW_HIDDEN|RUST_BACKTRACE)=.*)$",
    ))
    .expect("a valid regex")
});
/// Of [`FRAME`]'s lines: a numbered frame, a location, and a line only a
/// backtrace prints.
static NUMBERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\d+: \S").expect("a valid regex"));
static LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*at .+:(\d+|<unknown line>)(:\d+)?$").expect("a valid regex"));
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(⋮ \d+ frames? hidden ⋮|━+ BACKTRACE ━+|stack backtrace:|Run with (COLORBT_SHOW_HIDDEN|RUST_BACKTRACE)=.*)$")
        .expect("a valid regex")
});
/// A numbered line that is a frame when a location follows it: std's
/// `   0: rust_begin_unwind`.
static BARE_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\d+: \S+$").expect("a valid regex"));
/// How many backtrace lines in a row are folded into one.
const FRAMES_FOLDED: usize = 2;
static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\x1b\\[[0-9;?]*[ -/]*[@-~]").expect("a valid regex"));
/// Lines of context kept around an interesting line.
const CONTEXT: usize = 2;

/// A step's window, from the job's `steps`.
#[derive(Debug, Clone)]
pub struct Step {
    pub name: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

impl Step {
    /// The first failed step in a job document.
    pub fn failed(job: &serde_json::Value) -> Option<Self> {
        let steps = job["steps"].as_array()?;
        let step = steps
            .iter()
            .find(|s| s["conclusion"].as_str() == Some("failure"))?;
        Some(Self {
            name: step["name"].as_str()?.to_string(),
            started_at: step["started_at"].as_str().and_then(parse_utc),
            completed_at: step["completed_at"].as_str().and_then(parse_utc),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub lines: Vec<String>,
    /// Whether anything of the step was left out.
    pub truncated: bool,
}

/// Splits a raw log line into its timestamp and the rest.
fn split_stamp(line: &str) -> (Option<i64>, &str) {
    let line = line.trim_start_matches('\u{feff}');
    match line.split_once(' ') {
        Some((stamp, rest)) if stamp.len() >= 20 && stamp.ends_with('Z') => {
            match parse_utc(stamp) {
                Some(t) => (Some(t), rest),
                None => (None, line),
            }
        }
        _ => (None, line),
    }
}

/// The text of a log line without its timestamp, ANSI escapes or a
/// trailing carriage return.
fn clean(text: &str) -> String {
    ANSI.replace_all(text.trim_end_matches('\r'), "")
        .into_owned()
}

/// The lines of `step`, or the whole log when the step cannot be found.
fn step_lines(log: &str, step: Option<&Step>) -> Vec<String> {
    let raw: Vec<(Option<i64>, &str)> = log.lines().map(split_stamp).collect();
    let window = step.and_then(|s| Some((s.started_at?, s.completed_at?)));
    let Some((start, end)) = window else {
        return raw.iter().map(|(_, t)| clean(t)).collect();
    };
    let Some(from) = raw
        .iter()
        .position(|(t, text)| t.is_some_and(|t| t >= start) && text.starts_with("##[group]"))
    else {
        return raw.iter().map(|(_, t)| clean(t)).collect();
    };
    // The step ends where the next one starts, or with the runner's own
    // last word on a failed step, whichever comes first.
    let to = raw[from + 1..]
        .iter()
        .position(|(t, text)| {
            t.is_some_and(|t| t > end || (t == end && text.starts_with("##[group]Run ")))
        })
        .map_or(raw.len(), |i| from + 1 + i);
    let to = raw[from..to]
        .iter()
        .position(|(_, text)| text.starts_with("##[error]Process completed with exit code"))
        .map_or(to, |i| from + i + 1);
    // The step's opening group holds its script, shell and environment;
    // only its `Run ...` title is worth reading.
    let body = raw[from + 1..to]
        .iter()
        .position(|(_, text)| text.starts_with("##[endgroup]"))
        .map_or(from + 1, |i| from + i + 2);
    std::iter::once(&raw[from])
        .chain(&raw[body..to])
        .map(|(_, t)| clean(t))
        .collect()
}

/// The failed step's last `tail` lines, plus earlier lines that look like
/// errors with a little context, with a marker wherever lines were skipped.
/// `tail` 0 keeps the whole step.
pub fn excerpt(log: &str, step: Option<&Step>, tail: usize) -> Excerpt {
    let lines: Vec<String> = step_lines(log, step)
        .into_iter()
        .filter(|l| l != "##[endgroup]")
        .map(|l| {
            l.strip_prefix("##[group]")
                .map_or(l.clone(), str::to_string)
        })
        .collect();
    trim(lines, tail)
}

/// Lines of a step's output cut to their last `tail` lines, plus earlier
/// lines that look like errors with a little context, as [`excerpt`] does
/// for a GitHub log. `tail` 0 keeps them all.
pub fn trim(lines: Vec<String>, tail: usize) -> Excerpt {
    let before = lines.len();
    let lines = fold_backtraces(lines);
    // Folded lines are left out too.
    let folded = lines.len() < before;
    if tail == 0 || lines.len() <= tail {
        return Excerpt {
            lines,
            truncated: folded,
        };
    }
    let tail_start = lines.len() - tail;
    let mut keep = vec![false; lines.len()];
    for k in &mut keep[tail_start..] {
        *k = true;
    }
    // At most `tail` interesting lines before the tail, so a log full of
    // warnings stays readable.
    for i in (0..tail_start)
        .filter(|&i| INTERESTING.is_match(&lines[i]))
        .take(tail)
    {
        for k in &mut keep[i.saturating_sub(CONTEXT)..(i + CONTEXT + 1).min(lines.len())] {
            *k = true;
        }
    }
    let mut out = Vec::new();
    let mut skipped = 0;
    for (line, kept) in lines.iter().zip(&keep) {
        if *kept {
            if skipped > 0 {
                out.push(skipped_marker(skipped));
                skipped = 0;
            }
            out.push(line.clone());
        } else {
            skipped += 1;
        }
    }
    Excerpt {
        lines: out,
        truncated: true,
    }
}

/// `lines` with each run of [`FRAMES_FOLDED`] or more backtrace lines as
/// one: a panic's 40 frames would otherwise fill the tail, and push out the
/// message above them that says what failed.
pub fn fold_backtraces(lines: Vec<String>) -> Vec<String> {
    let frame = |i: usize| {
        FRAME.is_match(&lines[i])
            || (BARE_FRAME.is_match(&lines[i])
                && lines.get(i + 1).is_some_and(|next| FRAME.is_match(next)))
    };
    let flags: Vec<bool> = (0..lines.len()).map(frame).collect();
    // A run folds only when it is surely a backtrace: a numbered frame with
    // a location under it, or a line only backtraces print. An error chain
    // (`0: foo::Config failed`) has no locations, and a JavaScript stack
    // (`at /work/a.js:10:3`) no numbered frames.
    let backtrace = |run: &[String]| {
        run.iter().any(|l| MARKER.is_match(l))
            || (run.iter().any(|l| NUMBERED.is_match(l))
                && run.iter().any(|l| LOCATION.is_match(l)))
    };
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let run = flags[i..].iter().take_while(|f| **f).count();
        if run >= FRAMES_FOLDED && backtrace(&lines[i..i + run]) {
            out.push(format!("... {run} backtrace lines skipped"));
            i += run;
        } else {
            out.push(lines[i].clone());
            i += 1;
        }
    }
    out
}

fn skipped_marker(n: usize) -> String {
    if n == 1 {
        "... 1 line skipped".into()
    } else {
        format!("... {n} lines skipped")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_backtrace_folds_so_the_error_above_it_stays_in_the_tail() {
        let mut text = String::from(
            "DEBUG: $ mise uninstall dummy@latest\nError:\n   0: dummy@latest is not installed\n\nLocation:\n   src/cli/uninstall.rs:120\n\n  ━━━━━━━━━━━━━━━━ BACKTRACE ━━━━━━━━━━━━━━━━\n                                ⋮ 5 frames hidden ⋮\n",
        );
        for n in 6..45 {
            text.push_str(&format!(
                "  {n:2}: std[dcc5f7e57a96086e]::panicking::catch_unwind::<i32><unknown>\n      at /rustc/fd8e211/library/std/src/panicking.rs:{n}\n"
            ));
        }
        text.push_str("  45: _start<unknown>\n      at <unknown source file>:<unknown line>\n\nRun with COLORBT_SHOW_HIDDEN=1 environment variable to disable frame filtering.\nRun with RUST_BACKTRACE=full to include source snippets.\nERROR: E2E assertion failed: [mise uninstall dummy@latest] command failed with status 1\n");
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let cut = super::trim(lines, 20);
        // Folding left lines out.
        assert!(cut.truncated, "{:#?}", cut.lines);
        assert!(
            cut.lines
                .contains(&"   0: dummy@latest is not installed".to_string()),
            "{:#?}",
            cut.lines
        );
        assert!(
            cut.lines
                .iter()
                .any(|l| l.ends_with("backtrace lines skipped")),
            "{:#?}",
            cut.lines
        );
        assert!(
            cut.lines
                .last()
                .unwrap()
                .starts_with("ERROR: E2E assertion failed")
        );
        // std's frames without a path, and a sentence that is numbered.
        let std: Vec<String> = [
            "   0: rust_begin_unwind",
            "             at /rustc/abc/library/std/src/panicking.rs:665:5",
            "   1: core::panicking::panic_fmt",
            "             at /rustc/abc/library/core/src/panicking.rs:74:14",
            "   0: a sentence with words",
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(
            super::fold_backtraces(std),
            [
                "... 4 backtrace lines skipped",
                "   0: a sentence with words"
            ]
        );
        // An error chain, and a JavaScript stack, are not backtraces.
        let chain: Vec<String> = ["   0: foo::Config failed", "   1: foo::Loader failed"]
            .map(str::to_string)
            .to_vec();
        assert_eq!(super::fold_backtraces(chain.clone()), chain);
        let js: Vec<String> = ["    at /work/test.js:10:3", "    at /work/setup.js:20:4"]
            .map(str::to_string)
            .to_vec();
        assert_eq!(super::fold_backtraces(js.clone()), js);
        assert!(!super::trim(js, 20).truncated);
    }

    use super::*;

    const LOG: &str = "\u{feff}2026-10-30T10:00:00.1000000Z ##[group]Run actions/checkout@v4
2026-10-30T10:00:00.2000000Z with: x
2026-10-30T10:00:00.3000000Z ##[endgroup]
2026-10-30T10:00:01.0000000Z checked out
2026-10-30T10:00:02.0000000Z ##[group]Run cargo nextest run
2026-10-30T10:00:02.1000000Z \x1b[36;1mcargo nextest run\x1b[0m
2026-10-30T10:00:02.2000000Z ##[endgroup]
2026-10-30T10:00:03.0000000Z running 3 tests
2026-10-30T10:00:03.5000000Z thread 'a' panicked at src/a.rs:1:1
2026-10-30T10:00:04.0000000Z ok b
2026-10-30T10:00:04.1000000Z ok c
2026-10-30T10:00:04.2000000Z ok d
2026-10-30T10:00:05.0000000Z ##[error]Process completed with exit code 100.
2026-10-30T10:00:05.0000000Z ##[group]Run actions/cache/save
2026-10-30T10:00:06.0000000Z saved
";

    fn step() -> Step {
        Step {
            name: "cargo nextest".into(),
            started_at: parse_utc("2026-10-30T10:00:02Z"),
            completed_at: parse_utc("2026-10-30T10:00:05Z"),
        }
    }

    #[test]
    fn the_failed_step_is_cut_at_its_group_markers() {
        let e = excerpt(LOG, Some(&step()), 0);
        assert_eq!(
            e.lines,
            [
                "Run cargo nextest run",
                "running 3 tests",
                "thread 'a' panicked at src/a.rs:1:1",
                "ok b",
                "ok c",
                "ok d",
                "##[error]Process completed with exit code 100.",
            ]
        );
        assert!(!e.truncated);
    }

    #[test]
    fn the_tail_keeps_earlier_errors_with_context_and_marks_gaps() {
        let e = excerpt(LOG, Some(&step()), 1);
        assert_eq!(
            e.lines,
            [
                "Run cargo nextest run",
                "running 3 tests",
                "thread 'a' panicked at src/a.rs:1:1",
                "ok b",
                "ok c",
                "... 1 line skipped",
                "##[error]Process completed with exit code 100.",
            ]
        );
        assert!(e.truncated);
    }

    #[test]
    fn without_a_step_the_whole_log_is_used() {
        let e = excerpt(LOG, None, 0);
        assert_eq!(e.lines.first().unwrap(), "Run actions/checkout@v4");
        assert_eq!(e.lines.last().unwrap(), "saved");
    }
}
