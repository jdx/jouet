//! What jouet saved and what it cost: counted from the hook ledger, and
//! estimated against the user's own history from before `jouet setup`.
//!
//! The two are kept apart. [`measure`] counts what the ledger recorded: jouet
//! command runs, rule decisions and what the agent did after a denial,
//! checks, and failure excerpts. [`baseline`] reads an audit of the history
//! before the ledger began, [`estimate`] prices each jouet run by it, and
//! [`costs`] prices what jouet added the same way, each showing its basis or
//! refusing when the history is too thin. [`net`] is the one less the other.

mod costs;
mod ledger;
mod summary;

pub use costs::{AfterSetup, Cost, CostKind, costs, net};
pub use ledger::{Ledger, LedgerDecision, LedgerEntry};
pub use summary::{SummaryStyle, summary};

use crate::analyze::HarnessReport;
use crate::category::Category;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The fewest runs of a kind of work, before setup, to estimate from.
pub const MIN_BASELINE_RUNS: u64 = 20;

/// Which ledger entries count.
#[derive(Debug, Clone, Default)]
pub struct StatsFilter {
    /// Seconds since the Unix epoch.
    pub since: Option<u64>,
    pub harness: Option<String>,
    pub session: Option<String>,
}

impl StatsFilter {
    fn keeps(&self, e: &LedgerEntry) -> bool {
        self.since.is_none_or(|s| e.ts >= s)
            && self.harness.as_ref().is_none_or(|h| *h == e.harness)
            && self
                .session
                .as_ref()
                .is_none_or(|s| e.session.as_ref() == Some(s))
    }
}

/// What the ledger recorded.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Measured {
    pub entries: u64,
    pub sessions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ts: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ts: Option<u64>,
    /// jouet command runs, most first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandRuns>,
    /// Rule decisions, most first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleCounts>,
    /// Test, lint and build runs the `checks` rule recorded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckRuns>,
    /// Sessions stopped from finishing after a failed check.
    #[serde(skip_serializing_if = "is_zero")]
    pub verify_blocks: u64,
    /// `jouet wait ci` and `wait run` runs that failed, each returning the
    /// failed step's excerpt instead of the whole log.
    #[serde(skip_serializing_if = "is_zero")]
    pub failure_excerpts: u64,
    /// What jouet itself made the agent do again, per harness: the costs
    /// [`costs`] prices.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overhead: Vec<Overhead>,
}

/// One harness's requests jouet added, as the ledger recorded them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Overhead {
    pub harness: String,
    /// Sessions stopped from finishing after a failed check.
    #[serde(skip_serializing_if = "is_zero")]
    pub stop_blocks: u64,
    /// Calls a rule denied.
    #[serde(skip_serializing_if = "is_zero")]
    pub denials: u64,
    /// Calls a rule warned about.
    #[serde(skip_serializing_if = "is_zero")]
    pub warnings: u64,
    /// jouet commands that failed by their own exit code, other than `jouet
    /// edit` (see [`Kind::Edit`]) and a check's answer (see [`is_answer`]).
    /// Before jouet recorded its own exit codes, calls that ran only jouet (a
    /// `cd` aside) and failed: a call that also ran something else could have
    /// failed on that.
    #[serde(skip_serializing_if = "is_zero")]
    pub failed_runs: u64,
}

/// The commands whose exit code 1 (or `pr next`'s 10 and 20, or a timeout's
/// 124) is the answer to what they checked, not a failure of jouet: a test
/// that fails fails without jouet too.
const ANSWERS: &[&str] = &[
    "jouet test",
    "jouet wait",
    "jouet ci",
    "jouet secrets",
    "jouet pr status",
    "jouet pr next",
    "jouet http",
];

/// Whether a command line that ran `commands` and exited with `code` only
/// reported a check's answer.
fn is_answer(commands: &[String], code: i32) -> bool {
    matches!(code, 1 | 10 | 20 | 124)
        && commands.iter().any(|c| {
            ANSWERS
                .iter()
                .any(|a| c == a || c.starts_with(&format!("{a} ")))
        })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CommandRuns {
    /// `jouet wait ci`, `jouet pr status`, ...
    pub command: String,
    pub harness: String,
    pub runs: u64,
    /// Calls that ran it and failed. The ledger has the exit code of the
    /// whole call, so this mixes in failures of whatever else the call ran,
    /// except a check's answer (see [`is_answer`]).
    #[serde(skip_serializing_if = "is_zero")]
    pub failed: u64,
    /// Runs that recorded their own exit code (`jouet-run` entries), from
    /// this jouet on.
    #[serde(skip_serializing_if = "is_zero")]
    pub own_runs: u64,
    /// Of those, the ones that failed: its own failures.
    #[serde(skip_serializing_if = "is_zero")]
    pub own_failed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
}

/// The program each stage of a shape runs, a `cd` left out.
fn programs(shape: &str) -> Vec<&str> {
    shape
        .split(['|', ';', '&'])
        .filter_map(|stage| stage.split_whitespace().next())
        .filter(|program| *program != "cd")
        .collect()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuleCounts {
    pub rule: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub denies: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub warns: u64,
    /// The same command again after a deny or warning.
    #[serde(skip_serializing_if = "is_zero")]
    pub overrides: u64,
    /// After a deny, the agent's next command was a jouet command.
    #[serde(skip_serializing_if = "is_zero")]
    pub then_jouet: u64,
    /// After a deny, it ran something else.
    #[serde(skip_serializing_if = "is_zero")]
    pub then_other: u64,
    /// After a deny, the session ran nothing more.
    #[serde(skip_serializing_if = "is_zero")]
    pub then_nothing: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRuns {
    /// `test`, `lint` or `build`.
    pub kind: String,
    pub runs: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub failed: u64,
}

/// The jouet command a shape runs, as its program and subcommands:
/// `jouet wait ci <arg> --all` is `jouet wait ci`. One per pipeline stage.
pub fn jouet_commands(shape: &str) -> Vec<String> {
    shape
        .split(['|', ';', '&'])
        .filter_map(|stage| {
            let mut words = stage.split_whitespace();
            (words.next()? == "jouet").then(|| {
                let subs: Vec<&str> = words
                    .take_while(|w| w.starts_with(|c: char| c.is_ascii_lowercase()))
                    .take(3)
                    .collect();
                std::iter::once("jouet")
                    .chain(subs)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .collect()
}

/// Counts what the ledger recorded, in the entries `filter` keeps.
pub fn measure(entries: &[LedgerEntry], filter: &StatsFilter) -> Measured {
    // A jouet command records its own run without knowing the harness: it
    // is its session's, from the hooks' entries. One whose session the hooks
    // never saw is left out.
    let harness_of: HashMap<&str, &str> = entries
        .iter()
        .filter(|e| !matches!(e.harness.as_str(), "jouet" | "cli"))
        .filter_map(|e| Some((e.session.as_deref()?, e.harness.as_str())))
        .collect();
    let resolved: Vec<LedgerEntry> = entries
        .iter()
        .filter_map(|e| {
            if e.event != "jouet-run" {
                return Some(e.clone());
            }
            let harness = harness_of.get(e.session.as_deref()?)?;
            Some(LedgerEntry {
                harness: (*harness).to_string(),
                ..e.clone()
            })
        })
        .collect();
    let kept: Vec<&LedgerEntry> = resolved.iter().filter(|e| filter.keeps(e)).collect();
    // Sessions whose jouet commands recorded their own exit codes: there,
    // those decide what jouet failed, not the exit code of the whole call.
    let own: HashSet<Option<&str>> = kept
        .iter()
        .filter(|e| e.event == "jouet-run")
        .map(|e| e.session.as_deref())
        .collect();
    let mut m = Measured {
        entries: kept.len() as u64,
        first_ts: kept.iter().map(|e| e.ts).min(),
        last_ts: kept.iter().map(|e| e.ts).max(),
        ..Measured::default()
    };
    let mut sessions = HashSet::new();
    let mut commands: BTreeMap<(String, String), CommandRuns> = BTreeMap::new();
    let mut rules: BTreeMap<String, RuleCounts> = BTreeMap::new();
    let mut checks: BTreeMap<String, CheckRuns> = BTreeMap::new();
    let mut overhead: BTreeMap<String, Overhead> = BTreeMap::new();
    // The next pre-tool-use entry of each session, for "what came next".
    let mut by_session: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
    for (i, e) in kept.iter().enumerate() {
        sessions.insert(e.session.as_deref());
        if e.event == "pre-tool-use" {
            by_session.entry(e.session.as_deref()).or_default().push(i);
        }
        let added = overhead
            .entry(e.harness.clone())
            .or_insert_with(|| Overhead {
                harness: e.harness.clone(),
                ..Overhead::default()
            });
        for d in &e.decisions {
            match d.action.as_str() {
                "deny" => {
                    rule_counts(&mut rules, &d.rule).denies += 1;
                    added.denials += 1;
                }
                "warn" => {
                    rule_counts(&mut rules, &d.rule).warns += 1;
                    added.warnings += 1;
                }
                "override" => rule_counts(&mut rules, &d.rule).overrides += 1,
                "block" if d.rule == "verify" => {
                    m.verify_blocks += 1;
                    added.stop_blocks += 1;
                }
                action => {
                    if let Some(kind) = action.strip_prefix("record:") {
                        let c = checks.entry(kind.to_string()).or_insert_with(|| CheckRuns {
                            kind: kind.to_string(),
                            runs: 0,
                            failed: 0,
                        });
                        c.runs += 1;
                        c.failed += u64::from(e.exit_code.is_some_and(|x| x != 0));
                    }
                }
            }
        }
        let ran = e.shape.as_deref().map(jouet_commands).unwrap_or_default();
        let failed_run = |code: i32| {
            code != 0
                && ran.iter().any(|c| Kind::of(c) != Some(Kind::Edit))
                && !is_answer(&ran, code)
        };
        if e.event == "jouet-run" {
            let code = e.exit_code.unwrap_or(0);
            added.failed_runs += u64::from(failed_run(code));
            for command in &ran {
                let c = command_runs(&mut commands, &e.harness, command);
                c.own_runs += 1;
                c.own_failed += u64::from(code != 0);
            }
            continue;
        }
        if e.event != "post-tool-use" {
            continue;
        }
        // Before jouet recorded its own exit codes, a failed call counts
        // only when it ran nothing but jouet, so it cannot have failed on
        // something else.
        let only_jouet = e
            .shape
            .as_deref()
            .is_some_and(|s| !ran.is_empty() && programs(s).iter().all(|p| *p == "jouet"));
        if !own.contains(&e.session.as_deref())
            && only_jouet
            && failed_run(e.exit_code.unwrap_or(0))
        {
            added.failed_runs += 1;
        }
        // A line that ran a check and exited with its answer failed on the
        // check, not on the jouet commands around it: `jouet edit ... &&
        // jouet test` exiting 1 is a failing test, not a failed edit.
        let code = e.exit_code.unwrap_or(0);
        let answered = code != 0 && is_answer(&ran, code);
        for command in ran {
            let failed =
                code != 0 && (!answered || is_answer(std::slice::from_ref(&command), code));
            if failed && (command == "jouet wait ci" || command == "jouet wait run") {
                m.failure_excerpts += 1;
            }
            let c = command_runs(&mut commands, &e.harness, &command);
            c.runs += 1;
            c.failed += u64::from(failed);
            c.duration_ms += e.duration_ms.unwrap_or(0);
        }
    }
    // What the agent did after each denial: its next command in the session.
    for indexes in by_session.values() {
        for (n, &i) in indexes.iter().enumerate() {
            let next = indexes.get(n + 1).map(|&j| kept[j]);
            for d in kept[i].decisions.iter().filter(|d| d.action == "deny") {
                let r = rule_counts(&mut rules, &d.rule);
                match next.and_then(|e| e.shape.as_deref()) {
                    None if next.is_none() => r.then_nothing += 1,
                    Some(shape) if !jouet_commands(shape).is_empty() => r.then_jouet += 1,
                    _ => r.then_other += 1,
                }
            }
        }
    }
    m.sessions = sessions.len() as u64;
    m.commands = commands.into_values().collect();
    m.commands.sort_by(|a, b| {
        b.runs
            .cmp(&a.runs)
            .then_with(|| a.harness.cmp(&b.harness))
            .then_with(|| a.command.cmp(&b.command))
    });
    m.rules = rules.into_values().collect();
    m.rules.sort_by(|a, b| {
        (b.denies + b.warns)
            .cmp(&(a.denies + a.warns))
            .then(a.rule.cmp(&b.rule))
    });
    m.checks = checks.into_values().collect();
    m.overhead = overhead
        .into_values()
        .filter(|o| {
            *o != Overhead {
                harness: o.harness.clone(),
                ..Overhead::default()
            }
        })
        .collect();
    m
}

fn command_runs<'a>(
    commands: &'a mut BTreeMap<(String, String), CommandRuns>,
    harness: &str,
    command: &str,
) -> &'a mut CommandRuns {
    commands
        .entry((harness.to_string(), command.to_string()))
        .or_insert_with(|| CommandRuns {
            command: command.to_string(),
            harness: harness.to_string(),
            ..CommandRuns::default()
        })
}

fn rule_counts<'a>(rules: &'a mut BTreeMap<String, RuleCounts>, rule: &str) -> &'a mut RuleCounts {
    rules.entry(rule.to_string()).or_insert_with(|| RuleCounts {
        rule: rule.to_string(),
        ..RuleCounts::default()
    })
}

/// A kind of work jouet does in one call that agents otherwise spread over
/// many requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    CiWait,
    ProcessWait,
    PrStatus,
    PrThreads,
    Edit,
}

impl Kind {
    pub const ALL: [Self; 5] = [
        Self::CiWait,
        Self::ProcessWait,
        Self::PrStatus,
        Self::PrThreads,
        Self::Edit,
    ];

    /// The jouet commands that do this work.
    pub fn commands(self) -> &'static [&'static str] {
        match self {
            Self::CiWait => &["jouet wait ci", "jouet wait run", "jouet wait pr"],
            Self::ProcessWait => &[
                "jouet wait file",
                "jouet wait log",
                "jouet wait port",
                "jouet wait http",
                "jouet wait pid",
                "jouet wait cmd",
            ],
            Self::PrStatus => &["jouet pr status"],
            Self::PrThreads => &[
                "jouet pr threads",
                "jouet pr threads reply",
                "jouet pr threads resolve",
                "jouet pr threads apply",
            ],
            Self::Edit => &["jouet edit", "jouet edit replace"],
        }
    }

    /// The jouet command, for text.
    pub fn command(self) -> &'static str {
        match self {
            Self::CiWait => "jouet wait ci/run/pr",
            Self::ProcessWait => "jouet wait (local)",
            Self::PrStatus => "jouet pr status",
            Self::PrThreads => "jouet pr threads",
            Self::Edit => "jouet edit",
        }
    }

    /// `n` runs of the work, as they were before setup: `1 CI wait`.
    fn count(self, n: u64) -> String {
        let one = match self {
            Self::CiWait => "CI wait",
            Self::ProcessWait => "process wait",
            Self::PrStatus => "PR status check",
            Self::PrThreads => "review-thread round",
            Self::Edit => "edit",
        };
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{} {one}s", count_text(n))
        }
    }

    fn category(self) -> Category {
        match self {
            Self::CiWait => Category::CiPolling,
            Self::ProcessWait => Category::ProcessPolling,
            Self::PrStatus => Category::PrStatus,
            Self::PrThreads => Category::PrThreads,
            Self::Edit => Category::Edits,
        }
    }

    fn of(command: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.commands().contains(&command))
    }
}

/// One harness's history before setup, as the estimates use it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub harness: String,
    /// The history ends here, in seconds since the Unix epoch: the harness's
    /// first ledger entry. Absent when there is no ledger yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    pub sessions: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub requests_with_tokens: u64,
    /// Their input tokens: with `requests_with_tokens`, the price of the
    /// average request, which [`costs`] charges for each one jouet added.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input_tokens: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<KindBaseline>,
}

impl Baseline {
    /// The average request's input tokens before setup.
    pub fn tokens_per_request(&self) -> f64 {
        ratio(self.input_tokens, self.requests_with_tokens)
    }
}

/// How one kind of work went before setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindBaseline {
    pub kind: Kind,
    /// Runs of consecutive requests doing it: one CI wait polled eight times
    /// is one run.
    pub runs: u64,
    pub requests: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub calls: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed: u64,
    /// Process-wait runs were built from linked, eligible checks, not the
    /// old category-wide grouping. Absent on saved older baselines.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linked_waits: bool,
}

impl KindBaseline {
    pub fn requests_per_run(&self) -> f64 {
        ratio(self.requests, self.runs)
    }

    pub fn tokens_per_request(&self) -> f64 {
        ratio(self.input_tokens, self.requests)
    }

    pub fn failure_rate(&self) -> f64 {
        ratio(self.failed, self.calls)
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

/// The baseline in an audit report of a harness's history before setup.
pub fn baseline(report: &HarnessReport, until: Option<u64>) -> Baseline {
    Baseline {
        harness: report.harness.clone(),
        until,
        sessions: report.totals.sessions,
        requests_with_tokens: report.totals.requests_with_tokens,
        input_tokens: report.totals.input_tokens,
        kinds: Kind::ALL
            .into_iter()
            .filter_map(|kind| {
                let c = report
                    .categories
                    .iter()
                    .find(|c| c.category == kind.category())?;
                Some(KindBaseline {
                    kind,
                    runs: if kind == Kind::ProcessWait {
                        report
                            .process_waits
                            .as_ref()
                            .map_or(0, |w| w.opportunity_runs)
                    } else {
                        c.runs
                    },
                    requests: if kind == Kind::ProcessWait {
                        report
                            .process_waits
                            .as_ref()
                            .map_or(0, |w| w.opportunity_requests)
                    } else {
                        c.requests
                    },
                    input_tokens: if kind == Kind::ProcessWait {
                        report
                            .process_waits
                            .as_ref()
                            .map_or(0, |w| w.opportunity_input_tokens)
                    } else {
                        c.input_tokens
                    },
                    calls: c.calls,
                    failed: c.failed,
                    linked_waits: kind == Kind::ProcessWait,
                })
            })
            .collect(),
    }
}

/// An estimate of what one kind of jouet run saved in one harness.
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub harness: String,
    pub kind: Kind,
    /// jouet runs of this kind the ledger recorded.
    pub runs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Requests not made, for the waits, PR reads and thread rounds, to one
    /// decimal place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests_avoided: Option<f64>,
    /// What the estimate rests on.
    pub basis: String,
    /// Why there is no estimate, when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// Prices the measured jouet runs by each harness's own history: what they
/// would have cost at the rates from before setup, which is an assumption,
/// not a measurement. A run that replaces a run of requests saves the
/// requests after the first, at that harness's average context. An edit
/// saves the retries by which the old failure rate exceeds `jouet edit`'s
/// own; when `jouet edit` failed as often or more, it saves nothing and
/// [`costs`] charges the difference.
pub fn estimate(measured: &Measured, baselines: &[Baseline]) -> Vec<Estimate> {
    let mut runs: BTreeMap<(String, Kind), (u64, u64, u64)> = BTreeMap::new();
    for c in &measured.commands {
        if let Some(kind) = Kind::of(&c.command) {
            let r = runs.entry((c.harness.clone(), kind)).or_default();
            r.0 += c.runs;
            r.1 += c.own_runs;
            r.2 += c.own_failed;
        }
    }
    runs.into_iter()
        .map(|((harness, kind), (runs, own_runs, failed))| {
            let refuse = |reason: String| Estimate {
                harness: harness.clone(),
                kind,
                runs,
                input_tokens: None,
                requests_avoided: None,
                basis: String::new(),
                refused: Some(reason),
            };
            let Some(b) = baselines.iter().find(|b| b.harness == harness) else {
                return refuse(format!("no {harness} history from before setup"));
            };
            if b.requests_with_tokens == 0 {
                return refuse(format!("the {harness} history records no token counts"));
            }
            let found = b.kinds.iter().find(|k| k.kind == kind);
            if kind == Kind::ProcessWait && found.is_some_and(|k| !k.linked_waits) {
                return refuse("the saved process-wait baseline has no linked job evidence; run `jouet stats` to refresh it".to_string());
            }
            let before = found.map_or(0, |k| k.runs);
            if before < MIN_BASELINE_RUNS {
                if kind == Kind::ProcessWait {
                    return refuse(format!("only {before} linked process-wait opportunities before setup; an estimate needs {MIN_BASELINE_RUNS}"));
                }
                return refuse(format!(
                    "only {} before setup, and an estimate needs {MIN_BASELINE_RUNS}",
                    kind.count(before)
                ));
            }
            let k = found.expect("runs were counted from it");
            let per_request = k.tokens_per_request();
            if kind == Kind::Edit && own_runs == 0 {
                return refuse(
                    "no jouet edit run recorded its own exit code (from this release on, each does), and the exit code of the whole call mixes in failures of whatever else it ran".to_string(),
                );
            }
            let (tokens, avoided, basis) = if kind == Kind::Edit {
                // Its own rate, from the runs that recorded their exit code.
                let before = k.failure_rate();
                let after = ratio(failed, own_runs);
                let jouet = format!(
                    "jouet edit failed {:.1}% of the {} runs that recorded their own exit code ({failed})",
                    after * 100.0,
                    count_text(own_runs)
                );
                let credit = if after < before {
                    format!("at {} tokens per request", tokens_text(per_request))
                } else {
                    "so nothing is credited".to_string()
                };
                (
                    runs as f64 * (before - after).max(0.0) * per_request,
                    None,
                    format!(
                        "assuming each failed edit was redone: {} before setup, {:.1}% of which failed; {jouet}, {credit}",
                        kind.count(k.calls),
                        before * 100.0,
                    ),
                )
            } else {
                let extra = (k.requests_per_run() - 1.0).max(0.0);
                let avoided = runs as f64 * extra;
                (
                    avoided * per_request,
                    Some((avoided * 10.0).round() / 10.0),
                    format!(
                        "{} before setup, averaging {:.1} requests each at {} tokens per request",
                        kind.count(k.runs),
                        k.requests_per_run(),
                        tokens_text(per_request)
                    ),
                )
            };
            Estimate {
                harness,
                kind,
                runs,
                input_tokens: Some(tokens.round() as u64),
                requests_avoided: avoided,
                basis,
                refused: None,
            }
        })
        .collect()
}

/// What one jouet run of each kind would save, by each harness's history:
/// the view when there is no ledger to measure from yet. Kinds without
/// enough history are refused, as in [`estimate`].
pub fn per_run(baselines: &[Baseline]) -> Vec<Estimate> {
    let commands = baselines
        .iter()
        .flat_map(|b| {
            Kind::ALL.into_iter().map(|kind| CommandRuns {
                command: kind.commands()[0].to_string(),
                harness: b.harness.clone(),
                runs: 1,
                own_runs: 1,
                ..CommandRuns::default()
            })
        })
        .collect();
    let mut estimates = estimate(
        &Measured {
            commands,
            ..Measured::default()
        },
        baselines,
    );
    // An edit saves only what `jouet edit` fails less often, and nothing
    // has measured that yet.
    for e in estimates
        .iter_mut()
        .filter(|e| e.kind == Kind::Edit && e.refused.is_none())
    {
        e.input_tokens = None;
        e.basis = String::new();
        e.refused = Some(
            "it depends on how often jouet edit fails, which no ledger has measured yet".into(),
        );
    }
    estimates
}

/// `987` or `12,345`.
pub(crate) fn count_text(n: u64) -> String {
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
pub(crate) fn tokens_text(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests;
