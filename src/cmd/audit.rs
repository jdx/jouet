//! `jouet audit`.

use super::Output;
use crate::AuditArgs;
use crate::exit::Exit;
use crate::time;
use jouet_audit::{
    Analyzer, CategoryReport, ClaudeCode, Codex, Filter, HarnessReport, Reader, Report, ScanStats,
};
use serde::Serialize;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Categories listed per harness unless --top says otherwise.
const DEFAULT_TOP: usize = 10;
/// Shapes kept per category in JSON, and shown per suggestion in text.
const JSON_SHAPES: usize = 5;
const TEXT_SHAPES: usize = 3;

pub fn run(args: &AuditArgs, out: &Output) -> Exit {
    let now = time::now();
    let since_ms = match args.since.as_deref().map(|s| parse_since(s, now)) {
        None => None,
        Some(Ok(secs)) => Some(secs * 1000),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--since: {e}")),
    };
    let (claude, codex) = match args.harness.as_deref().unwrap_or("all") {
        "all" => (true, true),
        "claude" => (true, false),
        "codex" => (false, true),
        other => {
            return out.error(
                Exit::Usage,
                &format!("--harness must be claude, codex or all, not `{other}`"),
            );
        }
    };
    let project = match args.project.as_deref().map(std::path::absolute) {
        None => None,
        Some(Ok(path)) => Some(path),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--project: {e}")),
    };
    let top = match args.top {
        Some(0) => usize::MAX,
        Some(n) => n,
        None => DEFAULT_TOP,
    };

    let claude = claude.then(ClaudeCode::from_env).flatten();
    let codex = codex.then(Codex::from_env).flatten();
    let mut readers: Vec<&dyn Reader> = Vec::new();
    if let Some(r) = &claude {
        readers.push(r);
    }
    if let Some(r) = &codex {
        readers.push(r);
    }
    let roots: Vec<PathBuf> = claude
        .iter()
        .map(|c| c.root.join("projects"))
        .chain(codex.iter().map(|c| c.root.clone()))
        .collect();
    let filter = Filter {
        since_ms,
        project: project.clone(),
    };
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let scan = jouet_audit::scan(
        &readers,
        &filter,
        Analyzer::new(args.examples, home),
        threads,
    );
    let report = scan.analyzer.report(JSON_SHAPES);

    if report.harnesses.is_empty() {
        let mut message = String::from("no transcripts");
        if since_ms.is_some() || project.is_some() {
            message.push_str(" matched");
        } else {
            message.push_str(" found");
        }
        let places: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
        if !places.is_empty() {
            let _ = write!(message, " under {}", places.join(" and "));
        }
        return out.error_with_hint(
            Exit::NotFound,
            &message,
            Some("Claude Code's directory is $CLAUDE_CONFIG_DIR or ~/.claude, Codex's is $CODEX_HOME or ~/.codex"),
        );
    }
    let doc = Doc {
        since: since_ms.map(|ms| time::format_utc(ms / 1000)),
        project: project.as_ref().map(|p| p.display().to_string()),
        scan: ScanDoc::from(&scan.stats, threads),
        harnesses: &report.harnesses,
    };
    out.result(&doc, || text(&report, &scan.stats, &doc, top));
    Exit::Ok
}

/// `--since`: a duration ago (`7d`, `12h`, `1h30m`), a date, or RFC 3339.
fn parse_since(s: &str, now: i64) -> Result<i64, String> {
    let bad = || format!("`{s}` is not a time; write it as 7d, 12h, 2026-10-01 or RFC 3339");
    if let Some(n) = s.strip_suffix('d').or_else(|| s.strip_suffix('w')) {
        let days: i64 = n.parse().map_err(|_| bad())?;
        let days = if s.ends_with('w') { days * 7 } else { days };
        return Ok(now - days * 86_400);
    }
    if s.len() == 10 && s.as_bytes()[4] == b'-' {
        return time::parse_utc(&format!("{s}T00:00:00Z")).ok_or_else(bad);
    }
    time::parse_time(s, now).map_err(|_| bad())
}

#[derive(Serialize)]
struct Doc<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    scan: ScanDoc,
    harnesses: &'a [HarnessReport],
}

#[derive(Serialize)]
struct ScanDoc {
    files: u64,
    bytes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    skipped_old: u64,
    #[serde(skip_serializing_if = "is_zero")]
    other_projects: u64,
    #[serde(skip_serializing_if = "is_zero")]
    bad_lines: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unreadable: Vec<UnreadableDoc>,
    elapsed_ms: u64,
    threads: usize,
}

#[derive(Serialize)]
struct UnreadableDoc {
    path: String,
    error: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    partial: bool,
}

impl ScanDoc {
    fn from(stats: &ScanStats, threads: usize) -> Self {
        Self {
            files: stats.files,
            bytes: stats.bytes,
            skipped_old: stats.skipped_old,
            other_projects: stats.other_projects,
            bad_lines: stats.bad_lines,
            unreadable: stats
                .unreadable
                .iter()
                .map(|u| UnreadableDoc {
                    path: u.path.display().to_string(),
                    error: u.error.clone(),
                    partial: u.partial,
                })
                .collect(),
            elapsed_ms: stats.elapsed_ms,
            threads,
        }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn text(report: &Report, stats: &ScanStats, doc: &Doc<'_>, top: usize) -> String {
    let mut s = String::new();
    let sessions: u64 = report.harnesses.iter().map(|h| h.totals.sessions).sum();
    let _ = write!(s, "jouet audit: {}", plural(sessions, "session"));
    if let Some(since) = &doc.since {
        let _ = write!(s, " since {since}");
    }
    if let Some(project) = &doc.project {
        let _ = write!(s, " in {project}");
    }
    let _ = writeln!(
        s,
        " (read {}, {}, in {:.1}s)",
        plural(stats.files, "file"),
        bytes(stats.bytes),
        stats.elapsed_ms as f64 / 1000.0
    );
    for h in &report.harnesses {
        s.push('\n');
        harness_text(&mut s, h, top);
    }
    if stats.bad_lines > 0 {
        let _ = writeln!(
            s,
            "\nskipped {} that were not JSON",
            plural(stats.bad_lines, "line")
        );
    }
    if !stats.unreadable.is_empty() {
        let n = stats.unreadable.len() as u64;
        let _ = writeln!(s, "\n{} could not be read in full:", plural(n, "file"));
        for u in &stats.unreadable {
            let partial = if u.partial {
                " (counted up to the error)"
            } else {
                ""
            };
            let _ = writeln!(s, "  {}: {}{partial}", u.path.display(), u.error);
        }
    }
    let _ = writeln!(
        s,
        "\nSavings are estimates: requests that would not have been made, at the context they re-read."
    );
    s
}

fn harness_text(s: &mut String, h: &HarnessReport, top: usize) {
    let t = &h.totals;
    let name = match h.harness.as_str() {
        "claude" => "Claude Code",
        "codex" => "Codex",
        other => other,
    };
    let _ = write!(s, "{name}: {}", plural(t.sessions, "session"));
    if t.transcripts > t.sessions {
        let _ = write!(
            s,
            " ({} with subagents)",
            plural(t.transcripts, "transcript")
        );
    }
    let _ = writeln!(
        s,
        ", {}, {} ({} failed)",
        plural(t.requests, "request"),
        plural(t.calls, "tool call"),
        count(t.failed_calls)
    );
    let _ = write!(s, "  ");
    if t.input_tokens > 0 {
        let _ = write!(
            s,
            "input {} tokens, output {}; ",
            tokens(t.input_tokens),
            tokens(t.output_tokens)
        );
    }
    let _ = writeln!(
        s,
        "{} ran {}; tools returned {}",
        plural(t.commands, "command"),
        duration(t.command_ms),
        bytes(t.output_bytes)
    );
    let by = if h.ranked_by == "input_tokens" {
        "share of input tokens"
    } else {
        "share of tool time (no token counts)"
    };
    let _ = writeln!(s, "\n  {by}:");
    let rows: Vec<&CategoryReport> = h.categories.iter().take(top).collect();
    let mut table = vec![[
        "category".to_string(),
        "share".into(),
        "tokens".into(),
        "requests".into(),
        "calls".into(),
        "time".into(),
        "output".into(),
    ]];
    for c in &rows {
        table.push([
            c.category.label().to_string(),
            format!("{:.1}%", c.share * 100.0),
            tokens(c.input_tokens),
            count(c.requests),
            count(c.calls),
            duration(c.duration_ms),
            bytes(c.output_bytes),
        ]);
    }
    let widths: Vec<usize> = (0..7)
        .map(|i| {
            table
                .iter()
                .map(|r| r[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in &table {
        let _ = write!(s, "  {:<w$}", row[0], w = widths[0]);
        for (i, cell) in row.iter().enumerate().skip(1) {
            let _ = write!(s, "  {cell:>w$}", w = widths[i]);
        }
        s.push('\n');
    }
    let hidden = h.categories.len() - rows.len();
    if hidden > 0 {
        let _ = writeln!(s, "  … {hidden} more categories (--top 0 lists all)");
    }

    let shown: Vec<_> = h
        .suggestions
        .iter()
        .filter(|sg| rows.iter().any(|c| c.category == sg.category))
        .collect();
    if !shown.is_empty() {
        let _ = writeln!(s, "\n  what would cut it:");
    }
    for sg in shown {
        let row = rows.iter().find(|c| c.category == sg.category);
        let share = row.map_or(0.0, |c| c.share * 100.0);
        let _ = writeln!(
            s,
            "  - {} ({share:.1}%): {} — {}",
            sg.category.label(),
            sg.fix,
            sg.why
        );
        if let Some(saving) = &sg.saving {
            let _ = writeln!(
                s,
                "    estimate: {} tokens ({:.1}%), counting {}",
                tokens(saving.input_tokens),
                saving.share * 100.0,
                saving.basis
            );
        }
        if let Some(row) = row {
            shapes_text(s, row);
        }
    }
    if !h.patterns.is_empty() {
        let _ = writeln!(s, "\n  patterns:");
    }
    for p in &h.patterns {
        let _ = write!(
            s,
            "  - {}: {} in {}",
            p.pattern.label(),
            plural(p.calls, "call"),
            plural(p.sessions, "session")
        );
        if p.input_tokens > 0 {
            let _ = write!(s, ", {} tokens", tokens(p.input_tokens));
        }
        let _ = writeln!(s, " → {}", p.advice);
        if let Some(example) = &p.example {
            let _ = writeln!(s, "      e.g. {}", indent(example));
        }
    }
}

fn shapes_text(s: &mut String, row: &CategoryReport) {
    let shapes: Vec<_> = row.top_shapes.iter().take(TEXT_SHAPES).collect();
    if shapes.is_empty() {
        return;
    }
    let _ = writeln!(s, "    top shapes:");
    for shape in &shapes {
        let _ = write!(s, "      {} ({}", shape.shape, plural(shape.calls, "call"));
        if shape.input_tokens > 0 {
            let _ = write!(s, ", {} tokens", tokens(shape.input_tokens));
        }
        let _ = writeln!(s, ")");
        if let Some(example) = &shape.example {
            let _ = writeln!(s, "        e.g. {}", indent(example));
        }
    }
    let more = row.shape_count.saturating_sub(shapes.len() as u64);
    if more > 0 {
        let _ = writeln!(
            s,
            "      … {} more (--json lists {JSON_SHAPES})",
            plural(more, "shape")
        );
    }
}

/// Indents an example's later lines under its first.
fn indent(example: &str) -> String {
    example.replace('\n', "\n          ")
}

/// `1 call`, `12,345 calls`.
fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{} {word}s", count(n))
    }
}

/// `987`, `12,345`, `1.2M`.
fn count(n: u64) -> String {
    if n >= 1_000_000 {
        return format!("{:.1}M", n as f64 / 1e6);
    }
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `950`, `12k`, `3.4M`, `1.25B`.
fn tokens(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{n}")
    }
}

/// `45s`, `12m`, `3.5h`, `491h`.
fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..36_000 => format!("{:.1}h", secs as f64 / 3600.0),
        _ => format!("{}h", secs / 3600),
    }
}

/// `512 B`, `14 KB`, `3.2 MB`, `1.4 GB`.
fn bytes(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.1} GB", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1} MB", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.0} KB", f / 1e3)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_takes_days_dates_and_rfc_3339() {
        let now = 1_000_000_000;
        assert_eq!(parse_since("7d", now), Ok(now - 7 * 86_400));
        assert_eq!(parse_since("2w", now), Ok(now - 14 * 86_400));
        assert_eq!(parse_since("12h", now), Ok(now - 12 * 3600));
        assert_eq!(parse_since("1970-01-02", now), Ok(86_400));
        assert_eq!(parse_since("1970-01-01T00:01:00Z", now), Ok(60));
        assert!(parse_since("last tuesday", now).is_err());
    }

    #[test]
    fn numbers_read_at_a_glance() {
        assert_eq!(count(12_345), "12,345");
        assert_eq!(count(999), "999");
        assert_eq!(count(2_500_000), "2.5M");
        assert_eq!(tokens(1_250_000_000), "1.25B");
        assert_eq!(tokens(12_400), "12k");
        assert_eq!(duration(45_000), "45s");
        assert_eq!(duration(7_200_000), "2.0h");
        assert_eq!(duration(1_767_600_000), "491h");
        assert_eq!(bytes(3_200_000), "3.2 MB");
    }
}
