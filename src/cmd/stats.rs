//! `jouet stats`.

use super::Output;
use super::audit::parse_since;
use crate::StatsArgs;
use crate::config::Config;
use jouet_audit::stats::{self, Baseline, Estimate, Ledger, Measured, StatsFilter};
use jouet_audit::{Analyzer, ClaudeCode, Codex, Filter, Reader};
use jouet_output::exit::Exit;
use jouet_output::{paths, time};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

pub(crate) fn run(args: &StatsArgs, out: &Output) -> Exit {
    let since = match args.since.as_deref().map(|s| parse_since(s, time::now())) {
        None => None,
        Some(Ok(secs)) => Some(u64::try_from(secs).unwrap_or(0)),
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--since: {e}")),
    };
    let harness = match args.harness.as_deref().unwrap_or("all") {
        "all" => None,
        h @ ("claude" | "codex") => Some(h.to_string()),
        other => {
            return out.error(
                Exit::Usage,
                &format!("--harness must be claude, codex or all, not `{other}`"),
            );
        }
    };
    let filter = StatsFilter {
        since,
        harness: harness.clone(),
        session: args.session.clone(),
    };
    let dir = paths::ledger_dir();
    let ledger = match Ledger::read(&dir) {
        Ok(ledger) => ledger,
        Err(e) => return out.error(Exit::Failed, &format!("reading {}: {e}", dir.display())),
    };
    let measured = ledger.as_ref().map(|l| stats::measure(&l.entries, &filter));
    let cache = paths::stats_dir().join("baselines.json");

    if args.summary {
        let style = match Config::load().and_then(|c| c.stats_summary()) {
            Ok(style) => style,
            Err(e) => return out.error(Exit::Usage, &e),
        };
        // The stop hook runs this, so it reads the baselines the last full
        // `jouet stats` saved instead of reading the history again.
        let baselines = read_cache(&cache);
        let line = measured
            .as_ref()
            .and_then(|m| stats::summary(m, &stats::estimate(m, &baselines), style));
        out.result(
            &SummaryDoc {
                summary: line.clone(),
            },
            || line.map(|l| format!("{l}\n")).unwrap_or_default(),
        );
        return Exit::Ok;
    }

    // The history before setup is each harness's history before its first
    // ledger entry, whatever --since says.
    let mut first: BTreeMap<&str, u64> = BTreeMap::new();
    for e in ledger.iter().flat_map(|l| &l.entries) {
        let ts = first.entry(e.harness.as_str()).or_insert(e.ts);
        *ts = (*ts).min(e.ts);
    }
    let claude = ClaudeCode::from_env().filter(|_| harness.as_deref() != Some("codex"));
    let codex = Codex::from_env().filter(|_| harness.as_deref() != Some("claude"));
    let readers: Vec<&dyn Reader> = claude
        .iter()
        .map(|r| r as &dyn Reader)
        .chain(codex.iter().map(|r| r as &dyn Reader))
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let mut baselines = Vec::new();
    let mut scanned = ScanDoc::default();
    for reader in readers {
        let until = first.get(reader.harness()).copied();
        let filter = Filter {
            until_ms: until.map(|t| i64::try_from(t).unwrap_or(i64::MAX).saturating_mul(1000)),
            ..Filter::default()
        };
        let scan = jouet_audit::scan(&[reader], &filter, Analyzer::default(), threads);
        scanned.files += scan.stats.files;
        scanned.bytes += scan.stats.bytes;
        scanned.elapsed_ms += scan.stats.elapsed_ms;
        let report = scan.analyzer.report(0);
        baselines.extend(report.harnesses.iter().map(|h| stats::baseline(h, until)));
    }
    if harness.is_none() {
        write_cache(&cache, &baselines);
    }
    let estimates = match &measured {
        Some(m) => stats::estimate(m, &baselines),
        None => stats::per_run(&baselines),
    };
    let estimated: u64 = if measured.is_some() {
        estimates.iter().filter_map(|e| e.input_tokens).sum()
    } else {
        0
    };
    let doc = Doc {
        ledger: LedgerDoc {
            dir: dir.display().to_string(),
            present: ledger.is_some(),
            files: ledger.as_ref().map_or(0, |l| l.files),
            bad_lines: ledger.as_ref().map_or(0, |l| l.bad_lines),
        },
        since: since.map(|s| time::format_utc(i64::try_from(s).unwrap_or(0))),
        measured: measured.as_ref(),
        baselines: &baselines,
        estimates: if measured.is_some() { &estimates } else { &[] },
        per_run: if measured.is_none() { &estimates } else { &[] },
        estimated_input_tokens: estimated,
        scan: scanned,
    };
    out.result(&doc, || text(&doc));
    Exit::Ok
}

fn read_cache(path: &Path) -> Vec<Baseline> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Best effort: a summary without the cache only leaves out the estimates.
fn write_cache(path: &Path, baselines: &[Baseline]) {
    let Some(dir) = path.parent() else {
        return;
    };
    let Ok(json) = serde_json::to_vec(baselines) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_ok() {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[derive(Serialize)]
struct SummaryDoc {
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Serialize)]
struct Doc<'a> {
    ledger: LedgerDoc,
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    measured: Option<&'a Measured>,
    baselines: &'a [Baseline],
    /// What the measured jouet runs saved, estimated.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    estimates: &'a [Estimate],
    /// Without a ledger: what one run of each would save.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    per_run: &'a [Estimate],
    #[serde(skip_serializing_if = "is_zero")]
    estimated_input_tokens: u64,
    scan: ScanDoc,
}

#[derive(Serialize)]
struct LedgerDoc {
    dir: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    present: bool,
    #[serde(skip_serializing_if = "is_zero")]
    files: u64,
    #[serde(skip_serializing_if = "is_zero")]
    bad_lines: u64,
}

#[derive(Serialize, Default)]
struct ScanDoc {
    files: u64,
    bytes: u64,
    elapsed_ms: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn text(doc: &Doc<'_>) -> String {
    let mut s = String::new();
    match doc.measured {
        None => {
            let _ = writeln!(
                s,
                "jouet stats: no ledger at {}, so nothing is measured yet. `jouet setup` starts one.",
                doc.ledger.dir
            );
        }
        Some(m) => {
            let _ = write!(
                s,
                "jouet stats: {} in {}",
                plural(m.entries, "ledger entry", "ledger entries"),
                plural(m.sessions, "session", "sessions")
            );
            if let (Some(a), Some(b)) = (m.first_ts, m.last_ts) {
                let _ = write!(s, ", {} to {}", utc(a), utc(b));
            }
            s.push('\n');
            measured_text(&mut s, m);
        }
    }
    if doc.measured.is_some() {
        let _ = writeln!(s, "\nestimated, against your own history before setup:");
    } else {
        let _ = writeln!(s, "\nwhat one jouet run would save, by your history:");
    }
    for b in doc.baselines {
        let _ = write!(
            s,
            "  {} ({}",
            name(&b.harness),
            plural(b.sessions, "session", "sessions")
        );
        if let Some(until) = b.until {
            let _ = write!(s, " before {}", utc(until));
        }
        let _ = writeln!(s, "):");
        let rows: Vec<&Estimate> = doc
            .estimates
            .iter()
            .chain(doc.per_run)
            .filter(|e| e.harness == b.harness)
            .collect();
        if rows.is_empty() {
            let _ = writeln!(s, "  - no jouet runs to price");
        }
        for e in rows {
            estimate_text(&mut s, e, doc.measured.is_some());
        }
    }
    if doc.baselines.is_empty() {
        let _ = writeln!(
            s,
            "  no Claude Code or Codex history found, so nothing is estimated"
        );
    }
    if doc.estimated_input_tokens > 0 {
        let _ = writeln!(
            s,
            "\nestimated total: {} input tokens not re-read.",
            tokens(doc.estimated_input_tokens)
        );
    }
    let _ = writeln!(
        s,
        "\nMeasured figures come from the ledger; estimates price them by your own history and are not measurements."
    );
    s
}

fn measured_text(s: &mut String, m: &Measured) {
    let _ = writeln!(s, "\nmeasured, from the ledger:");
    if m.commands.is_empty() {
        let _ = writeln!(s, "  no jouet commands ran");
    } else {
        let mut rows = vec![[
            "command".to_string(),
            "harness".into(),
            "runs".into(),
            "failed".into(),
            "time".into(),
        ]];
        for c in &m.commands {
            rows.push([
                c.command.clone(),
                c.harness.clone(),
                c.runs.to_string(),
                c.failed.to_string(),
                duration(c.duration_ms),
            ]);
        }
        table(s, &rows);
    }
    if !m.rules.is_empty() {
        s.push('\n');
        let mut rows = vec![[
            "rule".to_string(),
            "deny".into(),
            "warn".into(),
            "overridden".into(),
            "then jouet".into(),
            "then other".into(),
            "then nothing".into(),
        ]];
        for r in &m.rules {
            rows.push([
                r.rule.clone(),
                r.denies.to_string(),
                r.warns.to_string(),
                r.overrides.to_string(),
                r.then_jouet.to_string(),
                r.then_other.to_string(),
                r.then_nothing.to_string(),
            ]);
        }
        table(s, &rows);
    }
    if !m.checks.is_empty() {
        let checks: Vec<String> = m
            .checks
            .iter()
            .map(|c| format!("{} {} ({} failed)", c.kind, c.runs, c.failed))
            .collect();
        let _ = writeln!(s, "\n  checks recorded: {}", checks.join(", "));
    }
    if m.verify_blocks > 0 {
        let _ = writeln!(
            s,
            "  finishing blocked after a failed check: {}",
            m.verify_blocks
        );
    }
    if m.failure_excerpts > 0 {
        let _ = writeln!(
            s,
            "  failing CI logs returned as the failing step's excerpt: {}",
            m.failure_excerpts
        );
    }
}

fn estimate_text(s: &mut String, e: &Estimate, measured: bool) {
    let command = e.kind.command();
    if let Some(reason) = &e.refused {
        if measured {
            let _ = writeln!(
                s,
                "  - {command}: {}, not estimated: {reason}",
                plural(e.runs, "run", "runs")
            );
        } else {
            let _ = writeln!(s, "  - {command}: not estimated: {reason}");
        }
        return;
    }
    let tok = tokens(e.input_tokens.unwrap_or(0));
    let what = match e.requests_avoided {
        // A total is whole requests; one run's share of them is not.
        Some(n) if measured => format!(
            "about {} and {tok} input tokens",
            plural(n.round() as u64, "request", "requests")
        ),
        Some(n) => format!("about {n:.1} requests and {tok} input tokens"),
        None => format!("about {tok} input tokens"),
    };
    if measured {
        let _ = writeln!(
            s,
            "  - {command}: {}, {what} avoided",
            plural(e.runs, "run", "runs")
        );
    } else {
        let _ = writeln!(s, "  - {command}: {what} per run");
    }
    let _ = writeln!(s, "      basis: {}", e.basis);
}

fn table<const N: usize>(s: &mut String, rows: &[[String; N]]) {
    let widths: Vec<usize> = (0..N)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    for row in rows {
        let _ = write!(s, "  {:<w$}", row[0], w = widths[0]);
        for (i, cell) in row.iter().enumerate().skip(1) {
            if i == 1 && N == 5 {
                let _ = write!(s, "  {cell:<w$}", w = widths[i]);
            } else {
                let _ = write!(s, "  {cell:>w$}", w = widths[i]);
            }
        }
        s.truncate(s.trim_end().len());
        s.push('\n');
    }
}

fn name(harness: &str) -> &str {
    match harness {
        "claude" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}

fn utc(ts: u64) -> String {
    time::format_utc(i64::try_from(ts).unwrap_or(0))
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        return format!("1 {one}");
    }
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped} {many}")
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

/// `45s`, `12m`, `3.5h`.
fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{:.1}h", secs as f64 / 3600.0),
    }
}
