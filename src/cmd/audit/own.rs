//! `jouet audit --jouet`: what jouet itself cost, per session. The hook
//! ledger gives the stop blocks and denials, the transcripts give the usage
//! errors, crashes, abandoned answers and test-log reads.

use super::{Output, ScanDoc, count, no_transcripts, plural};
use jouet_audit::digest::jouet::Kind;
use jouet_audit::stats::{self, Ledger, LedgerEntry, Measured, RuleCounts, StatsFilter};
use jouet_audit::{LogRead, OwnFinding, OwnSession, Scan};
use jouet_output::exit::Exit;
use jouet_output::time;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What the audit covered.
pub(super) struct Window<'a> {
    pub(super) since_ms: Option<i64>,
    pub(super) until_ms: Option<i64>,
    pub(super) project: Option<&'a Path>,
    /// Only this harness's ledger entries.
    pub(super) harness: Option<&'a str>,
    pub(super) roots: &'a [PathBuf],
    pub(super) threads: usize,
}

pub(super) fn run(out: &Output, scan: &Scan, w: &Window<'_>) -> Exit {
    let dir = crate::ledger::dir();
    let ledger = match dir.as_deref().map(Ledger::read).transpose() {
        Ok(ledger) => ledger.flatten(),
        Err(e) => {
            let dir = dir.unwrap_or_default();
            return out.error(Exit::Failed, &format!("reading {}: {e}", dir.display()));
        }
    };
    let transcripts = scan.analyzer.own();
    let filter = StatsFilter {
        since: w.since_ms.map(|ms| u64::try_from(ms / 1000).unwrap_or(0)),
        harness: w.harness.map(str::to_string),
        ..StatsFilter::default()
    };
    // With --project, the ledger counts only for the sessions it kept: the
    // ledger does not record where a session ran.
    let entries: Vec<LedgerEntry> = ledger
        .as_ref()
        .map(|l| {
            l.entries
                .iter()
                .filter(|e| {
                    let ts = i64::try_from(e.ts).unwrap_or(i64::MAX);
                    w.until_ms
                        .is_none_or(|until| ts.saturating_mul(1000) < until)
                })
                .filter(|e| {
                    w.project.is_none()
                        || e.session
                            .as_ref()
                            .is_some_and(|s| transcripts.iter().any(|t| &t.session == s))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let measured = stats::measure(&entries, &filter);
    if transcripts.is_empty() && measured.entries == 0 {
        return no_transcripts(out, w.since_ms.or(w.until_ms), w.project, w.roots);
    }
    let doc = doc(
        &transcripts,
        &entries,
        &measured,
        &filter,
        scan,
        w,
        ledger.as_ref(),
    );
    out.result(&doc, || text(&doc));
    Exit::Ok
}

#[derive(Serialize)]
struct Doc {
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    ledger: LedgerDoc,
    totals: Totals,
    /// Rules that denied or warned, most first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rules: Vec<RuleCounts>,
    /// Sessions jouet cost something in, the costliest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sessions: Vec<SessionDoc>,
    scan: ScanDoc,
}

#[derive(Serialize)]
struct LedgerDoc {
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    present: bool,
    #[serde(skip_serializing_if = "is_zero")]
    entries: u64,
    /// When its first entry in the window was written: nothing before it
    /// can be counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    first: Option<String>,
}

#[derive(Default, Serialize)]
struct Totals {
    /// Sessions with requests in the window, and of those (or of the
    /// ledger's), the ones jouet cost something in.
    #[serde(skip_serializing_if = "is_zero")]
    sessions: u64,
    #[serde(skip_serializing_if = "is_zero")]
    sessions_with_costs: u64,
    #[serde(flatten)]
    costs: Costs,
}

#[derive(Default, Clone, Serialize)]
struct Costs {
    #[serde(skip_serializing_if = "is_zero")]
    stop_blocks: u64,
    /// Blocks a passing check followed before the session next stopped.
    #[serde(skip_serializing_if = "is_zero")]
    stop_blocks_resolved: u64,
    #[serde(skip_serializing_if = "is_zero")]
    denials: u64,
    #[serde(skip_serializing_if = "is_zero")]
    warnings: u64,
    /// jouet commands that failed by their own exit code, a check's answer
    /// aside.
    #[serde(skip_serializing_if = "is_zero")]
    failed_runs: u64,
    #[serde(skip_serializing_if = "is_zero")]
    usage_errors: u64,
    #[serde(skip_serializing_if = "is_zero")]
    crashes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    exit_codes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    bad_json: u64,
    /// Answers the agent dropped for the raw command right after.
    #[serde(skip_serializing_if = "is_zero")]
    abandoned: u64,
    #[serde(skip_serializing_if = "is_zero")]
    test_runs: u64,
    /// Test logs read within three requests of a `jouet test` run.
    #[serde(skip_serializing_if = "is_zero")]
    log_reads: u64,
}

impl Costs {
    fn add_ledger(&mut self, m: &Measured) {
        for o in &m.overhead {
            self.stop_blocks += o.stop_blocks;
            self.stop_blocks_resolved += o.stop_blocks_resolved;
            self.denials += o.denials;
            self.warnings += o.warnings;
            self.failed_runs += o.failed_runs;
        }
    }

    fn add_transcript(&mut self, s: &OwnSession) {
        for f in &s.findings {
            *match f.kind {
                Kind::Usage => &mut self.usage_errors,
                Kind::Crash => &mut self.crashes,
                Kind::ExitCode => &mut self.exit_codes,
                Kind::BadJson => &mut self.bad_json,
                Kind::Abandoned => &mut self.abandoned,
            } += 1;
        }
        self.test_runs += s.test_runs;
        self.log_reads += s.log_reads.len() as u64;
    }

    /// Everything but the `jouet test` runs, which cost nothing themselves.
    fn any(&self) -> u64 {
        self.stop_blocks
            + self.denials
            + self.warnings
            + self.failed_runs
            + self.usage_errors
            + self.crashes
            + self.exit_codes
            + self.bad_json
            + self.abandoned
            + self.log_reads
    }
}

#[derive(Serialize)]
struct SessionDoc {
    session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(flatten)]
    costs: Costs,
    /// What each rule decided here, and what the agent ran after a denial.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rules: Vec<RuleCounts>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    findings: Vec<FindingDoc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    log_reads: Vec<LogReadDoc>,
}

#[derive(Serialize)]
struct FindingDoc {
    kind: &'static str,
    subject: String,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
}

#[derive(Serialize)]
struct LogReadDoc {
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
    test: String,
    read: String,
    after_requests: usize,
}

fn at(ms: Option<i64>) -> Option<String> {
    ms.map(|ms| time::format_utc(ms / 1000))
}

impl From<&OwnFinding> for FindingDoc {
    fn from(f: &OwnFinding) -> Self {
        Self {
            kind: f.kind.name(),
            subject: f.subject.clone(),
            detail: f.detail.clone(),
            at: at(f.at),
        }
    }
}

impl From<&LogRead> for LogReadDoc {
    fn from(r: &LogRead) -> Self {
        Self {
            at: at(r.at),
            test: r.test.clone(),
            read: r.read.clone(),
            after_requests: r.after_requests,
        }
    }
}

fn doc(
    transcripts: &[OwnSession],
    entries: &[LedgerEntry],
    measured: &Measured,
    filter: &StatsFilter,
    scan: &Scan,
    w: &Window<'_>,
    ledger: Option<&Ledger>,
) -> Doc {
    let mut totals = Totals {
        sessions: transcripts.len() as u64,
        ..Totals::default()
    };
    totals.costs.add_ledger(measured);
    for s in transcripts {
        totals.costs.add_transcript(s);
    }
    // Each session's ledger entries, measured on their own: a block's
    // resolution and a denial's next command never cross sessions.
    let mut by_session: BTreeMap<&str, Vec<LedgerEntry>> = BTreeMap::new();
    for e in entries {
        if let Some(s) = e.session.as_deref() {
            by_session.entry(s).or_default().push(e.clone());
        }
    }
    let by_id: HashMap<&str, &OwnSession> = transcripts
        .iter()
        .map(|s| (s.session.as_str(), s))
        .collect();
    let ids: BTreeSet<&str> = by_session
        .keys()
        .copied()
        .chain(by_id.keys().copied())
        .collect();
    let mut sessions = Vec::new();
    for id in ids {
        let mut costs = Costs::default();
        let mut rules = Vec::new();
        let mut harness = None;
        if let Some(entries) = by_session.get(id) {
            let m = stats::measure(entries, filter);
            costs.add_ledger(&m);
            rules = m
                .rules
                .into_iter()
                .filter(|r| r.denies + r.warns > 0)
                .collect();
            harness = m.overhead.first().map(|o| o.harness.clone());
        }
        let own = by_id.get(id);
        if let Some(s) = own {
            costs.add_transcript(s);
            harness = Some(s.harness.clone());
        }
        if costs.any() == 0 {
            continue;
        }
        sessions.push(SessionDoc {
            session: id.to_string(),
            harness,
            project: own.and_then(|s| s.project.clone()),
            costs,
            rules,
            findings: own
                .map(|s| s.findings.iter().map(FindingDoc::from).collect())
                .unwrap_or_default(),
            log_reads: own
                .map(|s| s.log_reads.iter().map(LogReadDoc::from).collect())
                .unwrap_or_default(),
        });
    }
    sessions.sort_by(|a, b| {
        b.costs
            .any()
            .cmp(&a.costs.any())
            .then_with(|| a.session.cmp(&b.session))
    });
    totals.sessions_with_costs = sessions.len() as u64;
    Doc {
        since: at(w.since_ms),
        until: at(w.until_ms),
        project: w.project.map(|p| p.display().to_string()),
        ledger: LedgerDoc {
            dir: ledger.map(|l| l.dir.display().to_string()),
            present: ledger.is_some(),
            entries: measured.entries,
            first: measured
                .first_ts
                .map(|ts| time::format_utc(i64::try_from(ts).unwrap_or(0))),
        },
        totals,
        rules: measured
            .rules
            .iter()
            .filter(|r| r.denies + r.warns > 0)
            .cloned()
            .collect(),
        sessions,
        scan: ScanDoc::from(&scan.stats, w.threads),
    }
}

fn text(doc: &Doc) -> String {
    let mut s = String::new();
    let t = &doc.totals;
    let c = &t.costs;
    let _ = write!(s, "jouet audit --jouet: {}", plural(t.sessions, "session"));
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
        "; jouet cost something in {}",
        count(t.sessions_with_costs)
    );
    match (&doc.ledger, doc.ledger.present) {
        (_, false) => {
            let _ = writeln!(
                s,
                "  no hook ledger, so no stop blocks or denials (jouet setup installs the hooks)"
            );
        }
        (l, true) => {
            let entries = if l.entries == 1 { "entry" } else { "entries" };
            let _ = write!(s, "  ledger: {} {entries}", count(l.entries));
            if let Some(first) = &l.first {
                let _ = write!(s, " from {first}");
            }
            s.push('\n');
        }
    }
    let _ = writeln!(
        s,
        "  stop blocks: {} ({} resolved by a passing check before the next stop)",
        count(c.stop_blocks),
        count(c.stop_blocks_resolved)
    );
    let _ = writeln!(
        s,
        "  denials: {}, warnings: {}",
        count(c.denials),
        count(c.warnings)
    );
    for r in &doc.rules {
        let _ = write!(s, "    {}: {} denied, {} warned", r.rule, r.denies, r.warns);
        if r.denies > 0 {
            let _ = write!(
                s,
                "; then a jouet command {}, something else {}, nothing {}",
                r.then_jouet, r.then_other, r.then_nothing
            );
        }
        if r.overrides > 0 {
            let _ = write!(s, "; ran again anyway {}", r.overrides);
        }
        s.push('\n');
    }
    let _ = writeln!(s, "  failed jouet runs: {}", count(c.failed_runs));
    let _ = writeln!(
        s,
        "  usage errors: {}, crashes: {}, exit codes outside the table: {}, bad --json: {}, answers dropped for the raw command: {}",
        count(c.usage_errors),
        count(c.crashes),
        count(c.exit_codes),
        count(c.bad_json),
        count(c.abandoned)
    );
    let _ = writeln!(
        s,
        "  test logs read right after `jouet test`: {} of {}",
        count(c.log_reads),
        plural(c.test_runs, "run")
    );
    if !doc.sessions.is_empty() {
        let _ = writeln!(s, "\n  by session:");
    }
    for d in &doc.sessions {
        let id: String = d.session.chars().take(8).collect();
        let _ = write!(s, "  {id}");
        if let Some(h) = &d.harness {
            let _ = write!(s, " {h}");
        }
        if let Some(p) = &d.project {
            let _ = write!(s, " {p}");
        }
        s.push(':');
        let mut parts = Vec::new();
        let dc = &d.costs;
        if dc.stop_blocks > 0 {
            parts.push(format!(
                "{} ({} resolved)",
                plural(dc.stop_blocks, "stop block"),
                dc.stop_blocks_resolved
            ));
        }
        for r in &d.rules {
            if r.denies > 0 {
                parts.push(format!("{} denied ×{}", r.rule, r.denies));
            }
            if r.warns > 0 {
                parts.push(format!("{} warned ×{}", r.rule, r.warns));
            }
        }
        if dc.failed_runs > 0 {
            parts.push(plural(dc.failed_runs, "failed jouet run"));
        }
        if parts.is_empty() {
            s.push('\n');
        } else {
            let _ = writeln!(s, " {}", parts.join(", "));
        }
        for f in &d.findings {
            let _ = writeln!(s, "    {} `{}`: {}", f.kind, f.subject, f.detail);
        }
        for r in &d.log_reads {
            let _ = writeln!(
                s,
                "    test log read {} after `{}`: `{}`",
                plural(r.after_requests as u64, "request"),
                r.test,
                r.read
            );
        }
    }
    s
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
