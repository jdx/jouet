//! `jouet audit`.

mod own;

use super::Output;
use crate::cli::AuditArgs;
use jouet_audit::{
    Analyzer, CategoryReport, ClaudeCode, Codex, Filter, FixedContext, FixedItem, HarnessReport,
    Reader, Report, ScanStats, WhatIf, WhatIfReport,
};
use jouet_output::exit::Exit;
use jouet_output::time;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

/// Categories listed per harness unless --top says otherwise.
const DEFAULT_TOP: usize = 10;
/// Shapes kept per category in JSON, and shown per suggestion in text.
const JSON_SHAPES: usize = 5;
const TEXT_SHAPES: usize = 3;

pub(crate) fn run(args: &AuditArgs, out: &Output) -> Exit {
    let now = time::now();
    let since_ms = match args.since.as_deref().map(|s| parse_since(s, now)) {
        None => None,
        Some(Ok(secs)) => match secs.checked_mul(1000) {
            Some(ms) => Some(ms),
            None => return out.error(Exit::Usage, "--since: too far back"),
        },
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--since: {e}")),
    };
    let until_ms = match args.until.as_deref().map(|s| parse_since(s, now)) {
        None => None,
        Some(Ok(secs)) => match secs.checked_mul(1000) {
            Some(ms) => Some(ms),
            None => return out.error(Exit::Usage, "--until: too far back"),
        },
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--until: {e}")),
    };
    let only = args.harness.as_deref().filter(|h| *h != "all");
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
        Some(Ok(path)) => Some(lexical(&path)),
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
        until_ms,
        project: project.clone(),
        detail: args.jouet,
        ..Filter::default()
    };
    let home = jouet_audit::home().map(|h| h.to_string_lossy().into_owned());
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let what_if = match args.what_if.as_deref().map(WhatIf::parse) {
        None => None,
        Some(Ok(spec)) => Some(spec),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--what-if: {e}")),
    };
    let mut analyzer = Analyzer::new(args.examples, home);
    if let Some(spec) = what_if {
        analyzer = analyzer.with_what_if(spec);
    }
    if args.jouet {
        analyzer = analyzer.with_own();
    }
    let scan = jouet_audit::scan(&readers, &filter, analyzer, threads);
    if args.jouet {
        let window = own::Window {
            since_ms,
            until_ms,
            project: project.as_deref(),
            harness: only,
            roots: &roots,
            threads,
        };
        return own::run(out, &scan, &window);
    }
    let report = scan.analyzer.report(JSON_SHAPES);

    if report.harnesses.is_empty() {
        return no_transcripts(out, since_ms.or(until_ms), project.as_deref(), &roots);
    }
    let doc = Doc {
        since: since_ms.map(|ms| time::format_utc(ms / 1000)),
        until: until_ms.map(|ms| time::format_utc(ms / 1000)),
        project: project.as_ref().map(|p| p.display().to_string()),
        scan: ScanDoc::from(&scan.stats, threads),
        harnesses: &report.harnesses,
    };
    out.result(&doc, || text(&report, &scan.stats, &doc, top));
    Exit::Ok
}

/// Exit 3: no transcripts, or none that matched.
fn no_transcripts(
    out: &Output,
    since_ms: Option<i64>,
    project: Option<&Path>,
    roots: &[PathBuf],
) -> Exit {
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
    out.error_with_hint(
        Exit::NotFound,
        &message,
        Some("Claude Code's directory is $CLAUDE_CONFIG_DIR or ~/.claude, Codex's is $CODEX_HOME or ~/.codex"),
    )
}

/// `--since`: a duration ago (`7d`, `12h`, `1h30m`), a date, or RFC 3339.
pub(crate) fn parse_since(s: &str, now: i64) -> Result<i64, String> {
    let bad = || format!("`{s}` is not a time; write it as 7d, 12h, 2026-10-01 or RFC 3339");
    if let Some(n) = s.strip_suffix('d').or_else(|| s.strip_suffix('w')) {
        let days: i64 = n.parse().map_err(|_| bad())?;
        let days = if s.ends_with('w') {
            days.checked_mul(7)
        } else {
            Some(days)
        };
        return days
            .and_then(|d| d.checked_mul(86_400))
            .and_then(|secs| now.checked_sub(secs))
            .ok_or_else(bad);
    }
    if s.len() == 10 && s.as_bytes()[4] == b'-' {
        return time::parse_utc(&format!("{s}T00:00:00Z")).ok_or_else(bad);
    }
    time::parse_time(s, now).map_err(|_| bad())
}

/// `path` with `.` and `..` resolved by name, the way a session records
/// the directory it ran in. Symlinks are not followed: `--project` matches
/// the path as written, not where a link leads.
pub(crate) fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[derive(Serialize)]
struct Doc<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<String>,
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
    #[serde(skip_serializing_if = "is_zero")]
    copied_requests: u64,
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
            copied_requests: stats.copied_requests,
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
    if let Some(until) = &doc.until {
        let _ = write!(s, " until {until}");
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
        let n = stats.bad_lines;
        let what = if n == 1 {
            "line that was"
        } else {
            "lines that were"
        };
        let _ = writeln!(s, "\nskipped {} {what} not JSON", count(n));
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
        "{}; recorded shell-call time {}; tools returned {}",
        plural(t.commands, "command"),
        duration(t.command_ms),
        bytes(t.output_bytes)
    );
    if let (Some(cached), Some(other), Some(requests)) = (
        t.cache_read_input_tokens,
        t.other_input_tokens,
        t.cache_split_requests,
    ) {
        let _ = writeln!(
            s,
            "  cache split for {} of {} requests: {} cached input tokens, {} other input tokens (including cache writes)",
            count(requests),
            count(t.requests),
            tokens(cached),
            tokens(other)
        );
    }
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
    columns(s, &table);
    let hidden = h.categories.len() - rows.len();
    if hidden > 0 {
        let _ = writeln!(s, "  … {hidden} more categories (--top 0 lists all)");
    }
    if let Some(waits) = &h.process_waits {
        let _ = writeln!(
            s,
            "\n  process waits: {} linked checks in {} sequences; {} sequences may benefit from a longer wait",
            count(waits.linked_checks),
            count(waits.linked_sequences),
            count(waits.opportunity_runs)
        );
        if let Some(elapsed) = waits.longest_sequence_ms {
            let _ = writeln!(
                s,
                "  longest linked sequence: {} checks over {}",
                count(waits.longest_sequence_checks),
                duration(elapsed)
            );
        }
        if let Some(cached) = waits.avoidable_cached_input_tokens {
            let _ = writeln!(
                s,
                "  potentially avoidable input: {} tokens ({} cached, {} not cached)",
                tokens(waits.avoidable_input_tokens),
                tokens(cached),
                tokens(waits.avoidable_input_tokens.saturating_sub(cached))
            );
        }
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
    if let Some(f) = &h.fixed_context {
        fixed_text(s, f, top);
    }
    if let Some(c) = &h.carried {
        carried_text(s, c, top);
    }
    if let Some(w) = &h.what_if {
        what_if_text(s, w);
    }
}

/// What each request re-read, by what put it in the context.
fn carried_text(s: &mut String, c: &jouet_audit::Carried, top: usize) {
    let _ = writeln!(
        s,
        "\n  carried: each request's input, charged to what put it in the context and how many requests re-read it before a compaction (estimated from context growth):"
    );
    let _ = write!(
        s,
        "  start of the session or after a compaction {:.1}%",
        c.start_share * 100.0
    );
    if c.compactions > 0 {
        let _ = write!(s, " ({})", plural(c.compactions, "compaction"));
    }
    s.push('\n');
    let mut table = vec![["category".to_string(), "share".into(), "tokens".into()]];
    for row in c.categories.iter().take(top) {
        table.push([
            row.category.label().to_string(),
            format!("{:.1}%", row.share * 100.0),
            tokens(row.input_tokens),
        ]);
    }
    columns(s, &table);
    if !c.files.is_empty() {
        let _ = writeln!(s, "  files that kept costing most:");
        for f in c.files.iter().take(top) {
            let _ = writeln!(
                s,
                "    {} ({:.1}%, {} tokens)",
                f.file,
                f.share * 100.0,
                tokens(f.input_tokens)
            );
        }
    }
}

/// The replay: what the same requests would have read.
fn what_if_text(s: &mut String, w: &WhatIfReport) {
    let mut spec = Vec::new();
    if let Some(window) = w.window {
        spec.push(format!("window {}", tokens(window)));
    }
    if !w.deny.is_empty() {
        spec.push(format!("deny {}", w.deny.join(", ")));
    }
    let _ = writeln!(
        s,
        "\n  what if ({}): replaying {} that read {} tokens ({} price-weighted: uncached 1, cache write 1.25, cache read 0.1)",
        spec.join("; "),
        plural(w.requests, "request"),
        tokens(w.input_tokens),
        tokens(w.weighted_input_tokens)
    );
    for d in &w.denied {
        let _ = writeln!(
            s,
            "  - deny {}: {} tokens fewer ({} price-weighted), on every request that carried it",
            d.name,
            tokens(d.input_tokens),
            tokens(d.weighted_input_tokens)
        );
    }
    if !w.not_found.is_empty() {
        let _ = writeln!(
            s,
            "  - not in any transcript's definitions: {}",
            w.not_found.join(", ")
        );
    }
    let pct = |x: f64| format!("{:+.1}%", -x * 100.0);
    match w.scenarios.as_slice() {
        [late, early] => {
            let _ = writeln!(
                s,
                "  input {} to {} ({} to {}), price-weighted {} to {}; {} to {} compactions added",
                tokens(late.input_tokens),
                tokens(early.input_tokens),
                pct(late.saved),
                pct(early.saved),
                pct(late.weighted_saved),
                pct(early.weighted_saved),
                count(late.compactions),
                count(early.compactions)
            );
            let _ = writeln!(
                s,
                "  the range is compaction firing at the window, then {} below it; it assumes the same requests after a compaction",
                tokens(early.reserve)
            );
        }
        [only] => {
            let _ = writeln!(
                s,
                "  input {} ({}), price-weighted {}",
                tokens(only.input_tokens),
                pct(only.saved),
                pct(only.weighted_saved)
            );
        }
        _ => {}
    }
}

/// The definitions sent with every request: the costliest, the ones
/// nothing called with the setting that removes each, and the weeks.
fn fixed_text(s: &mut String, f: &FixedContext, top: usize) {
    let _ = writeln!(
        s,
        "\n  fixed context: tool, skill and MCP definitions sent with every request, from each transcript's first snapshot (estimated tokens):"
    );
    let _ = writeln!(
        s,
        "  {} per request on average, over {} in {}: {} tokens, {:.1}% of input",
        per_request(f.mean_tokens),
        plural(f.requests, "request"),
        plural(f.transcripts, "transcript"),
        tokens(f.input_tokens),
        f.share * 100.0
    );
    let rows: Vec<&FixedItem> = f.items.iter().take(top).collect();
    let mut table = vec![[
        "definition".to_string(),
        "kind".into(),
        "per request".into(),
        "transcripts".into(),
        "tokens".into(),
        "calls".into(),
    ]];
    for i in &rows {
        table.push([
            i.name.clone(),
            i.kind.label().into(),
            per_request(i.tokens_per_request),
            count(i.transcripts),
            tokens(i.input_tokens),
            count(i.calls),
        ]);
    }
    columns(s, &table);
    let hidden = f.items.len() - rows.len();
    if hidden > 0 {
        let _ = writeln!(
            s,
            "  … {} (--top 0 lists all)",
            plural(hidden as u64, "more definition")
        );
    }
    let unused: Vec<&FixedItem> = f.items.iter().filter(|i| i.remove.is_some()).collect();
    if !unused.is_empty() {
        let per: u64 = unused.iter().map(|i| i.tokens_per_request).sum();
        let _ = writeln!(
            s,
            "\n  never called: {} of up to {} per request; what removes each:",
            plural(unused.len() as u64, "definition"),
            per_request(per)
        );
        for i in unused.iter().take(top) {
            let _ = writeln!(
                s,
                "  - {} ({}, {} per request, {} tokens): {}",
                i.name,
                i.kind.label(),
                per_request(i.tokens_per_request),
                tokens(i.input_tokens),
                i.remove.as_deref().unwrap_or_default()
            );
        }
        if unused.len() > top {
            let _ = writeln!(
                s,
                "  … {} (--top 0 lists all)",
                plural((unused.len() - top) as u64, "more unused definition")
            );
        }
    }
    if f.periods.len() > 1 {
        let _ = writeln!(s, "\n  by week (from Monday, UTC):");
        for p in &f.periods {
            let _ = write!(
                s,
                "  {}  {} per request, {}",
                p.start,
                per_request(p.mean_tokens),
                plural(p.transcripts, "transcript")
            );
            for (what, list, sign) in [("grew", &p.grew, '+'), ("shrank", &p.shrank, '−')] {
                if list.is_empty() {
                    continue;
                }
                let names: Vec<String> = list
                    .iter()
                    .map(|g| {
                        format!(
                            "{} {sign}{}",
                            g.name,
                            per_request(g.after.abs_diff(g.before))
                        )
                    })
                    .collect();
                let _ = write!(s, "; {what}: {}", names.join(", "));
            }
            s.push('\n');
        }
    }
}

/// A table, its first column left-aligned and the rest right-aligned.
fn columns<const N: usize>(s: &mut String, table: &[[String; N]]) {
    let widths: Vec<usize> = (0..N)
        .map(|i| {
            table
                .iter()
                .map(|r| r[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in table {
        let _ = write!(s, "  {:<w$}", row[0], w = widths[0]);
        for (i, cell) in row.iter().enumerate().skip(1) {
            let _ = write!(s, "  {cell:>w$}", w = widths[i]);
        }
        s.push('\n');
    }
}

/// `950`, `1.8k`, `85k`: a definition's tokens, one decimal under 100k.
fn per_request(n: u64) -> String {
    if (1000..100_000).contains(&n) {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        tokens(n)
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
            "      … {} (--json lists {JSON_SHAPES})",
            plural(more, "more shape")
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
pub(crate) fn bytes(n: u64) -> String {
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
    fn cache_split_names_its_request_coverage() {
        let transcript = jouet_audit::Transcript {
            session: "one".into(),
            requests: vec![
                jouet_audit::Request {
                    input_tokens: Some(100),
                    cached_input_tokens: Some(70),
                    input_tokens_complete: true,
                    ..Default::default()
                },
                jouet_audit::Request {
                    input_tokens: Some(50),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut analyzer = Analyzer::default();
        analyzer.add("codex", &transcript);
        let report = analyzer.report(0);
        let mut rendered = String::new();
        harness_text(&mut rendered, &report.harnesses[0], 0);
        assert!(
            rendered.contains(
                "cache split for 1 of 2 requests: 70 cached input tokens, 30 other input tokens"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn since_takes_days_dates_and_rfc_3339() {
        let now = 1_000_000_000;
        assert_eq!(parse_since("7d", now), Ok(now - 7 * 86_400));
        assert_eq!(parse_since("2w", now), Ok(now - 14 * 86_400));
        assert_eq!(parse_since("12h", now), Ok(now - 12 * 3600));
        assert_eq!(parse_since("1970-01-02", now), Ok(86_400));
        assert_eq!(parse_since("1970-01-01T00:01:00Z", now), Ok(60));
        assert!(parse_since("last tuesday", now).is_err());
        // Too far back to count is a usage error, not an overflow.
        assert!(parse_since("999999999999999d", now).is_err());
        assert!(parse_since("9999999999999999w", now).is_err());
    }

    #[test]
    fn project_paths_are_resolved_by_name() {
        assert_eq!(
            lexical(Path::new("/home/me/src/../other/./app")),
            Path::new("/home/me/other/app")
        );
        assert_eq!(lexical(Path::new("/../x")), Path::new("/x"));
    }

    #[test]
    fn secret_option_values_stay_out_of_both_formats() {
        let mut analyzer = Analyzer::new(true, None);
        let records = [jouet_audit::CommandRecord {
            session: "s".into(),
            command:
                r#"gh api user --token "correct horse" --password   'battery staple' -pS3cret"#
                    .into(),
            ..Default::default()
        }];
        for t in jouet_audit::transcripts(&records) {
            analyzer.add("claude", &t);
        }
        let report = analyzer.report(JSON_SHAPES);
        let stats = ScanStats::default();
        let doc = Doc {
            since: None,
            until: None,
            project: None,
            scan: ScanDoc::from(&stats, 1),
            harnesses: &report.harnesses,
        };
        let json = serde_json::to_string(&doc).unwrap();
        let text = text(&report, &stats, &doc, DEFAULT_TOP);
        assert!(
            text.contains("e.g. gh api user --token <redacted>"),
            "{text}"
        );
        for out in [&json, &text] {
            for word in ["correct", "horse", "battery", "staple"] {
                assert!(!out.contains(word), "{word} in {out}");
            }
        }
        // An attached value is not a known secret, so an example keeps it,
        // but no shape does.
        let shape = &report.harnesses[0].categories[0].top_shapes[0].shape;
        assert_eq!(shape, "gh api <path> --token <s> --password <s> -p<v>");
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
