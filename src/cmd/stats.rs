//! `jouet stats`.

use super::Output;
use super::audit::parse_since;
use crate::cli::StatsArgs;
use crate::settings::Settings;
use jouet_audit::stats::{
    self, AfterSetup, Baseline, Cost, Estimate, Ledger, Measured, StatsFilter,
};
use jouet_audit::{Analyzer, ClaudeCode, Codex, Filter, Reader};
use jouet_output::exit::Exit;
use jouet_output::{paths, time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// The rules loaded here, with their revisions: the rules table counts
/// each only from the revision loaded now. Where two share a name, the one
/// that decides is the first that is neither broken nor untrusted, as the
/// engine has it; the others count only when no such rule exists.
fn revisions() -> BTreeMap<String, String> {
    use jouet_rules::State;
    let cwd = std::env::current_dir().unwrap_or_default();
    let loaded = crate::rules::load(&cwd);
    let (runs, rest): (Vec<_>, Vec<_>) = loaded
        .engine
        .rules()
        .iter()
        .partition(|r| !matches!(r.state, State::Untrusted | State::Broken(_)));
    let mut revisions = BTreeMap::new();
    for rule in runs.into_iter().chain(rest) {
        revisions
            .entry(rule.name.clone())
            .or_insert_with(|| rule.revision.clone());
    }
    revisions
}

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
        revisions: Some(revisions()),
    };
    let dir = crate::ledger::dir();
    let ledger = match dir.as_deref().map(Ledger::read).transpose() {
        Ok(ledger) => ledger.flatten(),
        Err(e) => {
            let dir = dir.unwrap_or_default();
            return out.error(Exit::Failed, &format!("reading {}: {e}", dir.display()));
        }
    };
    let measured = ledger.as_ref().map(|l| stats::measure(&l.entries, &filter));
    let cache = paths::state_dir().map(|d| d.join("stats").join("baselines.json"));
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
    let settings = Settings::load();

    if args.summary {
        // Settings::load falls back to the default for a value it cannot
        // read, so a typo never breaks the stop hook.
        let style = stats::SummaryStyle::parse(&settings.stats.summary).unwrap_or_default();
        // The stop hook runs this, so it reads the baselines the last full
        // `jouet stats` saved instead of reading the history again.
        let baselines = cache.as_deref().map(read_cache).unwrap_or_default();
        let line = measured.as_ref().and_then(|m| {
            let (after, _) = after_setup(
                &readers,
                &first,
                since,
                args.session.as_deref(),
                &settings,
                threads,
            );
            let costs = stats::costs(m, &baselines, &after);
            stats::summary(m, &stats::estimate(m, &baselines), &costs, style)
        });
        out.result(
            &SummaryDoc {
                summary: line.clone(),
            },
            || line.map(|l| format!("{l}\n")).unwrap_or_default(),
        );
        return Exit::Ok;
    }

    // The history before a harness's first ledger entry no longer changes, so
    // a saved baseline for the same cutoff is reused instead of reading
    // gigabytes of transcripts again. A harness with no ledger entries has no
    // cutoff and is always read.
    let saved = if args.rescan {
        Vec::new()
    } else {
        cache.as_deref().map(read_cache).unwrap_or_default()
    };
    let mut baselines = Vec::new();
    let mut scanned = ScanDoc::default();
    for &reader in &readers {
        let until = first.get(reader.harness()).copied();
        if let Some(b) = saved
            .iter()
            .find(|b| until.is_some() && b.harness == reader.harness() && b.until == until)
            // One saved without its input tokens cannot price a request.
            .filter(|b| b.requests_with_tokens == 0 || b.input_tokens > 0)
        {
            baselines.push(b.clone());
            continue;
        }
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
    if let (None, Some(cache)) = (&harness, &cache) {
        write_cache(cache, &baselines);
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
    // The history after setup changes with every session, so it is read
    // each time; only the requests are kept.
    let (after, after_scan) = match &measured {
        Some(_) => after_setup(
            &readers,
            &first,
            since,
            args.session.as_deref(),
            &settings,
            threads,
        ),
        None => (Vec::new(), ScanDoc::default()),
    };
    let costs = measured
        .as_ref()
        .map(|m| stats::costs(m, &baselines, &after))
        .unwrap_or_default();
    let cost_tokens: u64 = costs.iter().filter_map(|c| c.input_tokens).sum();
    let net = measured
        .as_ref()
        .and_then(|_| stats::net(&estimates, &costs));
    let doc = Doc {
        ledger: LedgerDoc {
            dir: dir.map(|d| d.display().to_string()),
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
        after_setup: &after,
        costs: &costs,
        estimated_cost_tokens: cost_tokens,
        net_input_tokens: net,
        scan: scanned,
        after_setup_scan: after_scan,
    };
    out.result(&doc, || text(&doc));
    Exit::Ok
}

/// Each harness's requests after its first ledger entry (or `since`, when
/// later), and the size of its primer: what the primer cost is priced from.
/// A harness with no ledger entries has no history after setup.
fn after_setup(
    readers: &[&dyn Reader],
    first: &BTreeMap<&str, u64>,
    since: Option<u64>,
    session: Option<&str>,
    settings: &Settings,
    threads: usize,
) -> (Vec<AfterSetup>, ScanDoc) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut after = Vec::new();
    let mut scanned = ScanDoc::default();
    for &reader in readers {
        let Some(&from) = first.get(reader.harness()) else {
            continue;
        };
        let from = since.map_or(from, |s| s.max(from));
        let filter = Filter {
            since_ms: Some(i64::try_from(from).unwrap_or(i64::MAX).saturating_mul(1000)),
            session: session.map(str::to_string),
            ..Filter::default()
        };
        let scan = jouet_audit::scan(&[reader], &filter, Analyzer::default(), threads);
        scanned.files += scan.stats.files;
        scanned.bytes += scan.stats.bytes;
        scanned.elapsed_ms += scan.stats.elapsed_ms;
        // No transcript of the session was found: its requests are unknown,
        // not none, so its primer goes unpriced rather than free.
        if session.is_some() && scan.stats.files == 0 {
            continue;
        }
        let report = scan.analyzer.report(0);
        after.push(AfterSetup {
            harness: reader.harness().to_string(),
            requests: report.harnesses.iter().map(|h| h.totals.requests).sum(),
            primer_tokens: super::hook::primer_tokens(settings, &cwd, reader.harness()),
        });
    }
    (after, scanned)
}

/// What `baselines.json` holds. A cache from another jouet version is
/// ignored, since a release may read the history differently.
#[derive(Serialize, Deserialize)]
struct Cache {
    version: String,
    baselines: Vec<Baseline>,
}

fn read_cache(path: &Path) -> Vec<Baseline> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|c| c.version == env!("CARGO_PKG_VERSION"))
        .map(|c| c.baselines)
        .unwrap_or_default()
}

/// Best effort: a summary without the cache only leaves out the estimates.
fn write_cache(path: &Path, baselines: &[Baseline]) {
    let Some(dir) = path.parent() else {
        return;
    };
    let cache = Cache {
        version: env!("CARGO_PKG_VERSION").to_string(),
        baselines: baselines.to_vec(),
    };
    let Ok(json) = serde_json::to_vec(&cache) else {
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
    /// Each harness's requests after setup, and its primer's size.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    after_setup: &'a [AfterSetup],
    /// What jouet itself cost, estimated the same way.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    costs: &'a [Cost],
    #[serde(skip_serializing_if = "is_zero")]
    estimated_cost_tokens: u64,
    /// Savings less costs, which may be negative; absent when a cost could
    /// not be priced.
    #[serde(skip_serializing_if = "Option::is_none")]
    net_input_tokens: Option<i64>,
    /// Reading the history before setup.
    scan: ScanDoc,
    /// Reading the history after setup.
    after_setup_scan: ScanDoc,
}

#[derive(Serialize)]
struct LedgerDoc {
    /// Absent without a home directory, where there is no ledger.
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
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
            let at = doc
                .ledger
                .dir
                .as_deref()
                .map_or(String::new(), |d| format!(" at {d}"));
            let _ = writeln!(
                s,
                "jouet stats: no ledger{at}, so nothing is measured yet. `jouet setup` starts one."
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
        let _ = writeln!(
            s,
            "\nestimated savings: what these runs would have cost at your rates before setup (assumed, not measured):"
        );
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
    if doc.measured.is_some() {
        costs_text(&mut s, doc.costs);
        let totals = format!(
            "estimated savings {}, costs {}",
            tokens(doc.estimated_input_tokens),
            tokens(doc.estimated_cost_tokens)
        );
        let _ = match doc.net_input_tokens {
            Some(net) if net < 0 => writeln!(
                s,
                "\n{totals}: net, jouet cost {} input tokens more than it saved.",
                tokens(net.unsigned_abs())
            ),
            Some(net) => writeln!(
                s,
                "\n{totals}: net, {} input tokens saved.",
                tokens(net.unsigned_abs())
            ),
            None => writeln!(
                s,
                "\n{totals}: no net, since a cost above could not be priced."
            ),
        };
        let mut outside: Vec<&str> = doc
            .costs
            .iter()
            .filter(|c| c.outside_net)
            .map(|c| name(&c.harness))
            .collect();
        outside.dedup();
        if !outside.is_empty() {
            let _ = writeln!(
                s,
                "  left out of the net, with no history before setup to price it: {}",
                outside.join(", ")
            );
        }
    }
    let _ = writeln!(
        s,
        "\nMeasured figures come from the ledger; estimates price them by your own history and are not measurements."
    );
    s
}

fn costs_text(s: &mut String, costs: &[Cost]) {
    let _ = writeln!(s, "\nestimated costs, priced the same way:");
    if costs.is_empty() {
        let _ = writeln!(s, "  none recorded");
    }
    let mut harness = None;
    for c in costs {
        if harness != Some(&c.harness) {
            harness = Some(&c.harness);
            let _ = writeln!(s, "  {}:", name(&c.harness));
        }
        let what = format!("{}: {}", c.kind.label(), plural(c.count, "", "").trim());
        match (&c.refused, c.input_tokens) {
            (Some(reason), _) => {
                let _ = writeln!(s, "  - {what}, not estimated: {reason}");
            }
            (None, tok) => {
                let _ = writeln!(
                    s,
                    "  - {what}, about {} input tokens",
                    tokens(tok.unwrap_or(0))
                );
                let _ = writeln!(s, "      basis: {}", c.basis);
            }
        }
    }
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
            "calls failed".into(),
            "own runs".into(),
            "own failed".into(),
            "time".into(),
        ]];
        for c in &m.commands {
            rows.push([
                c.command.clone(),
                c.harness.clone(),
                c.runs.to_string(),
                c.failed.to_string(),
                c.own_runs.to_string(),
                c.own_failed.to_string(),
                duration(c.duration_ms),
            ]);
        }
        table(s, &rows, 2);
        let _ = writeln!(
            s,
            "  (calls failed: calls that ran it and failed, on it or on whatever else they ran; own: runs that recorded their own exit code)"
        );
    }
    if !m.rules.is_empty() {
        s.push('\n');
        let mut rows = vec![[
            "rule".to_string(),
            "deny".into(),
            "warn".into(),
            "sessions".into(),
            "again".into(),
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
                r.sessions.to_string(),
                r.sessions_again.to_string(),
                r.overrides.to_string(),
                r.then_jouet.to_string(),
                r.then_other.to_string(),
                r.then_nothing.to_string(),
            ]);
        }
        table(s, &rows, 1);
        let _ = writeln!(
            s,
            "  (sessions: the sessions it denied or warned in; again: those in which it did so more than once)"
        );
    }
    if m.older_decisions > 0 || !m.rules_not_loaded.is_empty() {
        if m.rules.is_empty() {
            s.push('\n');
        }
        let mut left = Vec::new();
        if m.older_decisions > 0 {
            left.push(format!(
                "{} {} of an older revision of a rule, or from before the ledger recorded revisions",
                m.older_decisions,
                if m.older_decisions == 1 {
                    "decision"
                } else {
                    "decisions"
                }
            ));
        }
        if !m.rules_not_loaded.is_empty() {
            left.push(format!(
                "the rules not loaded here: {}",
                m.rules_not_loaded.join(", ")
            ));
        }
        let _ = writeln!(
            s,
            "  rules are counted from the revision loaded now; left out: {}",
            left.join("; and ")
        );
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

/// Columns before `left` are aligned left, the rest right.
fn table<const N: usize>(s: &mut String, rows: &[[String; N]], left: usize) {
    let widths: Vec<usize> = (0..N)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    for row in rows {
        let _ = write!(s, "  {:<w$}", row[0], w = widths[0]);
        for (i, cell) in row.iter().enumerate().skip(1) {
            if i < left {
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
