//! What jouet saved and what it cost: counted from the hook ledger, and
//! estimated against the user's own history from before `jouet setup`.
//!
//! The two are kept apart. [`measure`] counts what the ledger recorded: jouet
//! command runs, rule decisions and what the agent did after a denial,
//! checks, and failure excerpts. [`baseline`] reads an audit of the history
//! before the ledger began, [`estimate`] prices each jouet run by it, and
//! [`costs`] prices what jouet added the same way, each showing its basis or
//! refusing when the history is too thin. [`net`] is the one less the other.

mod arms;
mod costs;
mod decisions;
mod ledger;
mod summary;

use arms::quantile;
pub use arms::{
    ArmComparison, ArmSpread, Difference, NEEDED_FOR, Needed, SessionMetrics, Spread, compare_arms,
    session_arms,
};
pub use costs::{AfterSetup, Cost, CostKind, costs, net};
/// When a check or edit was issued, in milliseconds: the end of the ledger
/// second it ended in, less how long it ran. A `jouet-run` line has no
/// duration, so the shell call that ran it gives both: one of the session's
/// `post-tool-use` lines of a `jouet` command in the next two seconds,
/// the one whose shape names the run. Older ledgers wrote every subcommand
/// as `<arg>`, so there it is the first `jouet` call that began at or after
/// `not_before`, the decision (an earlier one was another, already
/// running); with none, the run is dated by its own line.
fn issued_ms<'a>(
    closer: &'a LedgerEntry,
    after: impl Iterator<Item = &'a LedgerEntry>,
    not_before: u64,
) -> u64 {
    let issued = |e: &LedgerEntry| {
        e.ts.saturating_add(1)
            .saturating_mul(1000)
            .saturating_sub(e.duration_ms.unwrap_or(0))
    };
    if closer.duration_ms.is_none() && closer.event == "jouet-run" {
        let within = closer.ts.saturating_add(2);
        let calls: Vec<&LedgerEntry> = after
            .take_while(|e| e.ts <= within)
            .filter(|e| {
                e.event == "post-tool-use"
                    && e.duration_ms.is_some()
                    && e.shape.as_deref().is_some_and(|s| s.contains("jouet"))
            })
            .collect();
        // The call that names the run is its own, whenever it began: one
        // begun before the decision is skipped by the caller.
        let named = closer.shape.as_deref().and_then(|run| {
            calls
                .iter()
                .find(|e| e.shape.as_deref().is_some_and(|s| s.contains(run)))
        });
        let unnamed = || calls.iter().find(|e| issued(e) >= not_before);
        if let Some(call) = named.or_else(unnamed) {
            return issued(call);
        }
    }
    issued(closer)
}

pub use decisions::{DecisionSpan, DecisionsAfter, after_decisions, request_times};
pub use ledger::{Ledger, LedgerDecision, LedgerEntry};
pub use summary::{SummaryStyle, summary};

use crate::analyze::HarnessReport;
use crate::category::Category;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The fewest runs of a kind of work, before setup, to estimate from.
pub const MIN_BASELINE_RUNS: u64 = 20;

/// Which ledger entries count.
#[derive(Debug, Clone, Default)]
pub struct StatsFilter {
    /// Seconds since the Unix epoch.
    pub since: Option<u64>,
    pub harness: Option<String>,
    pub session: Option<String>,
    /// The rules loaded now, by name, with their revisions. When set, the
    /// rules table counts each rule only from decisions of that revision,
    /// and leaves out rules that are not loaded.
    pub revisions: Option<BTreeMap<String, String>>,
}

impl StatsFilter {
    /// Whether the rules table counts a rule's decision: without
    /// `revisions`, every one; with them, one of the revision loaded now.
    fn current(&self, d: &LedgerDecision) -> bool {
        self.revisions.as_ref().is_none_or(|r| {
            r.get(&d.rule)
                .is_some_and(|rev| d.revision.as_ref() == Some(rev))
        })
    }

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
    /// Decisions of a loaded rule left out of `rules` as an older
    /// revision's, or from before the ledger recorded revisions.
    #[serde(skip_serializing_if = "is_zero")]
    pub older_decisions: u64,
    /// Rules the ledger names that are not loaded now, left out of `rules`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules_not_loaded: Vec<String>,
    /// Test, lint and build runs the `checks` rule recorded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckRuns>,
    /// Sessions stopped from finishing after a failed check.
    #[serde(skip_serializing_if = "is_zero")]
    pub verify_blocks: u64,
    /// The stop gate's reasons, each as it blocked for it or only
    /// observed it (`[hooks] gate`), and what came after: most first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gate: Vec<GateReason>,
    /// `jouet wait ci` and `wait run` runs that failed, each returning the
    /// failed step's excerpt instead of the whole log.
    #[serde(skip_serializing_if = "is_zero")]
    pub failure_excerpts: u64,
    /// What jouet itself made the agent do again, per harness: the costs
    /// [`costs`] prices.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overhead: Vec<Overhead>,
    /// Each deny, warning and rewrite, and when its session next ran a
    /// check or edited a file: what [`after_decisions`] counts the requests
    /// over.
    #[serde(skip)]
    pub decision_spans: Vec<DecisionSpan>,
    /// Each rule's decisions by action, with the requests until the next
    /// check or edit, once the caller has read the transcripts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub after_decisions: Vec<DecisionsAfter>,
}

/// One harness's requests jouet added, as the ledger recorded them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Overhead {
    pub harness: String,
    /// Sessions stopped from finishing after a failed check.
    #[serde(skip_serializing_if = "is_zero")]
    pub stop_blocks: u64,
    /// Of those, the ones a passing check followed before the session next
    /// stopped: the block held the agent until its check passed, which a
    /// correct change needs anyway. The rest finished with no check passing:
    /// the agent spent a request on the block and changed nothing it tested.
    #[serde(skip_serializing_if = "is_zero")]
    pub stop_blocks_resolved: u64,
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
/// that fails fails without jouet too. Search exit 1 means no matches and is
/// handled separately because its other nonzero exits are not search results.
const ANSWERS: &[&str] = &[
    "jouet test",
    "jouet wait",
    "jouet ci",
    "jouet secrets",
    "jouet pr status",
    "jouet pr next",
    "jouet http",
    "jouet diag",
];

/// Whether a command line that ran `commands` and exited with `code` only
/// reported a check's answer.
fn is_answer(commands: &[String], code: i32) -> bool {
    if code == 1 && !commands.is_empty() && commands.iter().all(|command| command == "jouet search")
    {
        return true;
    }
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
    /// Of those, the ones that failed: its own failures, a check's answer
    /// left out (see [`is_answer`]).
    #[serde(skip_serializing_if = "is_zero")]
    pub own_failed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
    /// Calls that ran only it and recorded how much output they gave the
    /// agent, and that output's bytes.
    #[serde(skip_serializing_if = "is_zero")]
    pub output_calls: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub output_bytes: u64,
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
    /// The sessions it denied or warned in.
    #[serde(skip_serializing_if = "is_zero")]
    pub sessions: u64,
    /// Of those, the sessions it denied or warned in more than once: its
    /// message did not keep the agent from doing it again.
    #[serde(skip_serializing_if = "is_zero")]
    pub sessions_again: u64,
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

/// One reason the stop gate found, as it acted on it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GateReason {
    /// What the reason was about: `failed-test`, `failed-lint`,
    /// `failed-build`, `changed-after-pass`, `hk-partial`, `hk-changed`, or
    /// `hk-stop` for hk's own stop hook.
    pub reason: String,
    /// `block` where the gate stopped the agent, `would-block` where it
    /// only observed.
    pub action: String,
    /// The stops it was found at.
    pub stops: u64,
    /// Of those, the ones that proved right: the session's next edit or
    /// failed check came before any check passed. An edit means the agent
    /// changed something after all; a failed check, that something was
    /// still broken.
    #[serde(skip_serializing_if = "is_zero")]
    pub right: u64,
    /// The ones where a check passed first, with no edit before it: the
    /// change was fine as it stood.
    #[serde(skip_serializing_if = "is_zero")]
    pub passed: u64,
    /// The rest: the session has done neither since, so far as the ledger
    /// knows.
    #[serde(skip_serializing_if = "is_zero")]
    pub open: u64,
}

/// What first followed a stop the gate found a reason at, in its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    /// An edit, or a check that failed.
    Right,
    /// A check that passed.
    Passed,
}

/// The first edit or check among `after`, the session's entries after a
/// stop. An edit is the harness's edit tool (`edit` entries) or a `jouet
/// edit` that succeeded; an edit made some other way, such as by `sed -i`,
/// is not seen. A call that edits and checks counts as the edit.
fn judge<'a>(after: impl IntoIterator<Item = &'a LedgerEntry>) -> Option<After> {
    after.into_iter().find_map(|e| {
        let ran = e.shape.as_deref().map(jouet_commands).unwrap_or_default();
        let edited = e.event == "edit"
            || (matches!(e.event.as_str(), "jouet-run" | "post-tool-use")
                && e.exit_code == Some(0)
                && ran.iter().any(|c| Kind::of(c) == Some(Kind::Edit)));
        if edited {
            Some(After::Right)
        } else if passing_check(e) {
            Some(After::Passed)
        } else if failed_check(e) {
            Some(After::Right)
        } else {
            None
        }
    })
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
    // How often each rule denied or warned in each session.
    let mut fired: HashMap<(&str, &str), u64> = HashMap::new();
    let mut not_loaded: BTreeSet<String> = BTreeSet::new();
    // The next pre-tool-use entry of each session, for "what came next".
    let mut by_session: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
    // Every entry of each session, for what followed a stop block.
    let mut whole_session: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut blocks: Vec<usize> = Vec::new();
    // Each reason the gate found at a stop: the entry, its reason and action.
    let mut gate: Vec<(usize, &str, &str)> = Vec::new();
    for (i, e) in kept.iter().enumerate() {
        sessions.insert(e.session.as_deref());
        if let Some(session) = e.session.as_deref() {
            whole_session.entry(session).or_default().push(i);
        }
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
            // What a rule decided goes in the rules table only from its
            // current revision; what it cost counts whatever the revision.
            let ruled = matches!(d.action.as_str(), "deny" | "warn" | "override");
            let counted = !ruled || filter.current(d);
            if !counted {
                match &filter.revisions {
                    Some(r) if r.contains_key(&d.rule) => m.older_decisions += 1,
                    _ => {
                        not_loaded.insert(d.rule.clone());
                    }
                }
            }
            if let ("deny" | "warn", Some(session), true) =
                (d.action.as_str(), e.session.as_deref(), counted)
            {
                *fired.entry((d.rule.as_str(), session)).or_default() += 1;
            }
            match d.action.as_str() {
                "deny" => {
                    if counted {
                        rule_counts(&mut rules, &d.rule).denies += 1;
                    }
                    added.denials += 1;
                }
                "warn" => {
                    if counted {
                        rule_counts(&mut rules, &d.rule).warns += 1;
                    }
                    added.warnings += 1;
                }
                "override" if counted => rule_counts(&mut rules, &d.rule).overrides += 1,
                "block" | "would-block" if d.rule == "verify" => {
                    // A stop blocked for several reasons is one block.
                    if d.action == "block" && blocks.last() != Some(&i) {
                        m.verify_blocks += 1;
                        added.stop_blocks += 1;
                        blocks.push(i);
                    }
                    if let Some(reason) = d.scope.as_deref() {
                        gate.push((i, reason, d.action.as_str()));
                    }
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
                // Exit 1 from `jouet test` is the tests failing, not jouet.
                c.own_failed +=
                    u64::from(code != 0 && !is_answer(std::slice::from_ref(command), code));
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
        // Output is the call's, so it is the command's only when the call
        // ran nothing else.
        let alone = e
            .shape
            .as_deref()
            .is_some_and(|s| ran.len() == 1 && programs(s).len() == 1);
        if let (true, Some(bytes), [command]) = (alone, e.output_bytes, ran.as_slice()) {
            let c = command_runs(&mut commands, &e.harness, command);
            c.output_calls += 1;
            c.output_bytes += bytes;
        }
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
            for d in kept[i]
                .decisions
                .iter()
                .filter(|d| d.action == "deny" && filter.current(d))
            {
                let r = rule_counts(&mut rules, &d.rule);
                match next.and_then(|e| e.shape.as_deref()) {
                    None if next.is_none() => r.then_nothing += 1,
                    Some(shape) if !jouet_commands(shape).is_empty() => r.then_jouet += 1,
                    _ => r.then_other += 1,
                }
            }
        }
    }
    // Each deny, warning and rewrite, until the next check or edit.
    for (i, e) in kept.iter().enumerate() {
        let Some(session) = e.session.as_deref() else {
            continue;
        };
        for d in &e.decisions {
            let ruled = matches!(d.action.as_str(), "deny" | "warn" | "override");
            if !decisions::JUDGED.contains(&d.action.as_str()) || ruled && !filter.current(d) {
                continue;
            }
            let until_ms = whole_session.get(session).and_then(|entries| {
                let later = &entries[entries.iter().position(|&j| j == i)? + 1..];
                later
                    .iter()
                    .enumerate()
                    .filter(|&(_, &j)| judge(std::iter::once(kept[j])).is_some())
                    .map(|(k, &j)| {
                        let after = later[k + 1..].iter().map(|&j| kept[j]);
                        issued_ms(kept[j], after, e.ts.saturating_mul(1000))
                    })
                    // A check or edit begun before the decision, and still
                    // running when it came, does not end it.
                    .find(|&ms| ms >= e.ts.saturating_mul(1000))
            });
            m.decision_spans.push(DecisionSpan {
                rule: d.rule.clone(),
                action: d.action.clone(),
                session: session.to_string(),
                at: e.ts,
                until_ms,
            });
        }
    }
    for &i in &blocks {
        let Some(entries) = kept[i]
            .session
            .as_deref()
            .and_then(|s| whole_session.get(s))
        else {
            continue;
        };
        let after = entries.iter().skip_while(|&&j| j != i).skip(1);
        let resolved = after
            .map(|&j| kept[j])
            .take_while(|e| e.event != "stop")
            .any(passing_check);
        if resolved && let Some(o) = overhead.get_mut(&kept[i].harness) {
            o.stop_blocks_resolved += 1;
        }
    }
    let mut reasons: BTreeMap<(&str, &str), GateReason> = BTreeMap::new();
    for (i, reason, action) in gate {
        let r = reasons
            .entry((reason, action))
            .or_insert_with(|| GateReason {
                reason: reason.to_string(),
                action: action.to_string(),
                ..GateReason::default()
            });
        r.stops += 1;
        let after = kept[i]
            .session
            .as_deref()
            .and_then(|s| whole_session.get(s))
            .map(|entries| {
                judge(
                    entries
                        .iter()
                        .skip_while(|&&j| j != i)
                        .skip(1)
                        .map(|&j| kept[j]),
                )
            });
        match after.flatten() {
            Some(After::Right) => r.right += 1,
            Some(After::Passed) => r.passed += 1,
            None => r.open += 1,
        }
    }
    m.gate = reasons.into_values().collect();
    m.gate.sort_by(|a, b| {
        b.stops
            .cmp(&a.stops)
            .then_with(|| a.action.cmp(&b.action))
            .then_with(|| a.reason.cmp(&b.reason))
    });
    for ((rule, _), n) in fired {
        let r = rule_counts(&mut rules, rule);
        r.sessions += 1;
        r.sessions_again += u64::from(n > 1);
    }
    m.sessions = sessions.len() as u64;
    m.rules_not_loaded = not_loaded.into_iter().collect();
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

/// Whether `e` is a test, lint or build run that passed: one the `checks`
/// rule recorded, or a `jouet test` that recorded its own exit code.
fn passing_check(e: &LedgerEntry) -> bool {
    e.exit_code == Some(0)
        && (e.decisions.iter().any(|d| d.action.starts_with("record:"))
            || (e.event == "jouet-run"
                && e.shape
                    .as_deref()
                    .is_some_and(|s| jouet_commands(s).iter().any(|c| c == "jouet test"))))
}

/// Whether `e` is a test, lint or build run that failed, as
/// [`passing_check`] tells one that passed.
fn failed_check(e: &LedgerEntry) -> bool {
    e.exit_code.is_some_and(|code| code != 0)
        && (e.decisions.iter().any(|d| d.action.starts_with("record:"))
            || (e.event == "jouet-run"
                && e.shape
                    .as_deref()
                    .is_some_and(|s| jouet_commands(s).iter().any(|c| c == "jouet test"))))
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
    /// A test, lint or build run whose output jouet cut to its failures.
    Test,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::CiWait,
        Self::ProcessWait,
        Self::PrStatus,
        Self::PrThreads,
        Self::Edit,
        Self::Test,
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
            Self::Test => &["jouet test"],
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
            Self::Test => "jouet test",
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
            Self::Test => "test, lint or build call",
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
            Self::Test => Category::BuildTestLint,
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
    /// The bytes of output its calls gave the agent. Absent on saved older
    /// baselines.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output_bytes: u64,
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
                let Some(c) = report
                    .categories
                    .iter()
                    .find(|c| c.category == kind.category())
                else {
                    // Kept with nothing in it, so a baseline without one is
                    // known to predate test runs (see `estimate`).
                    return (kind == Kind::Test).then_some(KindBaseline {
                        kind,
                        runs: 0,
                        requests: 0,
                        input_tokens: 0,
                        calls: 0,
                        failed: 0,
                        output_bytes: 0,
                        linked_waits: false,
                    });
                };
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
                    output_bytes: c.output_bytes,
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
    /// The harness has no baseline and is excluded from the combined net.
    #[serde(skip)]
    pub(crate) outside_net: bool,
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
    let mut output: BTreeMap<(String, Kind), (u64, u64)> = BTreeMap::new();
    for c in &measured.commands {
        if let Some(kind) = Kind::of(&c.command) {
            let r = runs.entry((c.harness.clone(), kind)).or_default();
            r.0 += c.runs;
            r.1 += c.own_runs;
            r.2 += c.own_failed;
            let o = output.entry((c.harness.clone(), kind)).or_default();
            o.0 += c.output_calls;
            o.1 += c.output_bytes;
        }
    }
    runs.into_iter()
        .map(|((harness, kind), (runs, own_runs, failed))| {
            let (output_calls, output_bytes) =
                output.get(&(harness.clone(), kind)).copied().unwrap_or_default();
            let refuse = |reason: String| Estimate {
                harness: harness.clone(),
                kind,
                runs,
                input_tokens: None,
                requests_avoided: None,
                basis: String::new(),
                refused: Some(reason),
                outside_net: !baselines.iter().any(|b| b.harness == harness),
            };
            let Some(b) = baselines.iter().find(|b| b.harness == harness) else {
                return refuse(format!("no {harness} history from before setup"));
            };
            if kind == Kind::CiWait {
                return refuse("CI polling history does not establish linked, avoidable checks".to_string());
            }
            if b.requests_with_tokens == 0 {
                return refuse(format!("the {harness} history records no token counts"));
            }
            let found = b.kinds.iter().find(|k| k.kind == kind);
            if kind == Kind::ProcessWait && found.is_some_and(|k| !k.linked_waits) {
                return refuse("the saved process-wait baseline has no linked job evidence; run `jouet stats` to refresh it".to_string());
            }
            // A baseline has an entry for every kind (`Kind::ALL`), so one
            // with none for test runs was saved before they were counted.
            if kind == Kind::Test && found.is_none() {
                return refuse("the saved baseline predates test runs; run `jouet stats --rescan`".to_string());
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
            if kind == Kind::Test && output_calls == 0 {
                return refuse("no jouet test call recorded how much output it gave the agent (from this release on, each does in Claude Code)".to_string());
            }
            if kind == Kind::Test && k.calls > 0 && k.output_bytes == 0 {
                return refuse("the saved baseline has no output sizes; run `jouet stats --rescan`".to_string());
            }
            let (tokens, avoided, basis) = if kind == Kind::Test {
                // What a check printed before setup, against what jouet test
                // printed, counted once: the output stays in the context of
                // every later request, so this is a lower bound.
                let before = ratio(k.output_bytes, k.calls);
                let after = ratio(output_bytes, output_calls);
                (
                    output_calls as f64 * (before - after).max(0.0) / BYTES_PER_TOKEN,
                    None,
                    format!(
                        "{} before setup gave the agent {} bytes of output on average; jouet test gave {} over the {} calls that recorded it, at {BYTES_PER_TOKEN} bytes a token, counted once though output stays in the context of every later request",
                        kind.count(k.calls),
                        count_text(before.round() as u64),
                        count_text(after.round() as u64),
                        count_text(output_calls),
                    ),
                )
            } else if kind == Kind::Edit {
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
            outside_net: false,
            }
        })
        .collect()
}

/// About how many bytes of tool output make a token.
const BYTES_PER_TOKEN: f64 = 4.0;

/// What one jouet run of each kind would save, by each harness's history:
/// the view when there is no ledger to measure from yet. Kinds without
/// enough history are refused, as in [`estimate`].
pub fn per_run(baselines: &[Baseline]) -> Vec<Estimate> {
    let commands = baselines
        .iter()
        .flat_map(|b| {
            Kind::ALL
                .into_iter()
                .map(|kind| CommandRuns {
                    command: kind.commands()[0].to_string(),
                    harness: b.harness.clone(),
                    runs: 1,
                    own_runs: 1,
                    ..CommandRuns::default()
                })
                // What jouet test prints depends on the run, which no ledger has
                // measured yet.
                .filter(|c| c.command != "jouet test")
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
/// A count with thousands separators.
pub fn count_text(n: u64) -> String {
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
