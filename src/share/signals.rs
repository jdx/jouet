//! What sessions say about jouet itself, gathered for `jouet share`.
//!
//! Two sources feed it. A [digest](jouet_audit::digest) finds jouet
//! crashing, exiting outside its exit-code table, printing `--json` that is
//! not JSON, meeting usage errors, or being dropped for raw `gh` or `git`
//! right after it answered. The hook ledger shows the same from command
//! shapes, and three things only it can: built-in rules overridden again
//! and again, hooks over their time budget, and the same script written in
//! many sessions.
//!
//! Each becomes a [`Signal`] in the gap file beside the missing primitives,
//! so `jouet share --idea KEY` drafts it like any idea: bugs as an Issue,
//! the rest as a Discussion. Only shapes and counts are kept, never values.

use super::{GapEntry, Gaps};
use crate::ledger::Entry;
use crate::wind::stats;
use jouet_audit::digest::jouet::{self, Run};
use jouet_rules::{Engine, Scope};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Crash,
    ExitCode,
    BadJson,
    Usage,
    Abandoned,
    /// A built-in rule overridden again and again.
    Overridden,
    /// A hook over its time budget.
    SlowHook,
    /// The same script, written in many sessions: a candidate built-in.
    Routine,
}

/// Where a signal goes on jdx/jouet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Destination {
    Issue,
    Discussion,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::ExitCode => "exit-code",
            Self::BadJson => "bad-json",
            Self::Usage => "usage",
            Self::Abandoned => "abandoned",
            Self::Overridden => "overridden",
            Self::SlowHook => "slow-hook",
            Self::Routine => "routine",
        }
    }

    fn from_digest(kind: jouet::Kind) -> Self {
        match kind {
            jouet::Kind::Crash => Self::Crash,
            jouet::Kind::ExitCode => Self::ExitCode,
            jouet::Kind::BadJson => Self::BadJson,
            jouet::Kind::Usage => Self::Usage,
            jouet::Kind::Abandoned => Self::Abandoned,
        }
    }
}

/// Firings, overrides and the share of them overridden that make a
/// built-in rule a likely false positive; from [`ISSUE_RATE`] on, a bug.
const OVERRIDE_FIRINGS: u64 = 10;
const OVERRIDES: u64 = 3;
const OVERRIDE_RATE: f64 = 0.3;
const ISSUE_RATE: f64 = 0.5;
/// Sessions a script must appear in to be a candidate built-in.
const ROUTINE_SESSIONS: usize = 5;
/// Sessions kept per signal, to count them.
const SESSIONS: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Signal {
    pub kind: Option<Kind>,
    /// The jouet command, the rule, the hook event or the script's shape.
    pub subject: String,
    pub count: u64,
    /// Sessions it was seen in, as the ledger names them.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sessions: BTreeSet<String>,
    /// What was seen, without values, and how often: `exit 7`, `then `gh
    /// pr view``.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, u64>,
    /// Example shapes, values replaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// The digest findings counted, as `session:line`, so a session
    /// digested twice counts once.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub seen: BTreeSet<String>,
    /// For an overridden rule: fired, overridden; for a slow hook: the
    /// budget and the slowest run, in milliseconds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numbers: Vec<u64>,
}

impl Signal {
    pub(crate) fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Routine)
    }

    /// What `jouet share --idea` names it by.
    pub(crate) fn key(&self) -> String {
        let subject = match self.kind() {
            // A shape is long and full of placeholders: a hash names it.
            Kind::Routine => short_hash(&self.subject),
            _ => self
                .subject
                .split_whitespace()
                .filter(|w| *w != "jouet")
                .collect::<Vec<_>>()
                .join("-"),
        };
        let slug: String = subject
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        format!("{}-{}", self.kind().name(), slug.to_lowercase())
    }

    pub(crate) fn destination(&self) -> Destination {
        match self.kind() {
            Kind::Crash | Kind::ExitCode | Kind::BadJson | Kind::SlowHook => Destination::Issue,
            Kind::Overridden if self.override_rate() >= ISSUE_RATE => Destination::Issue,
            _ => Destination::Discussion,
        }
    }

    fn override_rate(&self) -> f64 {
        match self.numbers[..] {
            [fired, overridden, ..] if fired > 0 => overridden as f64 / fired as f64,
            _ => 0.0,
        }
    }

    /// Whether it has been seen enough to be worth a maintainer's time. A
    /// crash or a malformed document is worth it at once; a usage error or
    /// an abandoned answer only once it repeats across sessions.
    pub(crate) fn ready(&self) -> bool {
        match self.kind() {
            Kind::Crash | Kind::ExitCode | Kind::BadJson => self.count >= 1,
            Kind::Usage | Kind::Abandoned => self.count >= 3 && self.sessions.len() >= 2,
            Kind::SlowHook => self.count >= 3,
            Kind::Overridden | Kind::Routine => true,
        }
    }

    /// One line, for a session start or a listing.
    pub(crate) fn summary(&self) -> String {
        let n = self.count;
        let s = self.sessions.len();
        let top = self
            .details
            .iter()
            .max_by_key(|(_, c)| **c)
            .map_or("", |(d, _)| d.as_str());
        match self.kind() {
            Kind::Crash => format!("`{}` crashed {n} times ({top})", self.subject),
            Kind::ExitCode => format!(
                "`{}` exited outside the exit-code table {n} times ({top})",
                self.subject
            ),
            Kind::BadJson => format!(
                "`{} --json` printed something that is not JSON {n} times",
                self.subject
            ),
            Kind::Usage => format!(
                "`{}` hit usage errors {n} times in {s} sessions",
                self.subject
            ),
            Kind::Abandoned => format!(
                "agents dropped `{}` for raw commands right after it answered, {n} times in {s} sessions ({top})",
                self.subject
            ),
            Kind::Overridden => format!(
                "built-in rule `{}` was overridden {} of {} times",
                self.subject,
                self.numbers.get(1).copied().unwrap_or(0),
                self.numbers.first().copied().unwrap_or(0)
            ),
            Kind::SlowHook => format!(
                "the {} hook ran over its {} ms budget {n} times (slowest {} ms)",
                self.subject,
                self.numbers.first().copied().unwrap_or(0),
                self.numbers.get(1).copied().unwrap_or(0)
            ),
            Kind::Routine => format!(
                "the same script was written in {s} sessions: `{}`",
                self.subject
            ),
        }
    }
}

fn short_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A signal's subject and examples as ledger shapes: no values.
fn shape_of(command: &str) -> String {
    crate::ledger::shape(command)
}

/// Adds a digest's findings for one session, each with its command and its
/// line in the transcript. Digesting a session again adds nothing new: a
/// finding counts once per session and line.
pub(crate) fn add_digest(gaps: &mut Gaps, session: &str, found: &[(jouet::Found, String, u64)]) {
    for (f, command, line) in found {
        let kind = Kind::from_digest(f.kind);
        let entry = entry_for(gaps, kind, &f.subject);
        let signal = entry.signal.as_mut().expect("entry_for makes a signal");
        if !signal.seen.insert(format!("{session}:{line}")) {
            continue;
        }
        signal.count += 1;
        signal.sessions.insert(session.to_string());
        *signal.details.entry(f.detail.clone()).or_default() += 1;
        let shape = shape_of(command);
        if signal.examples.len() < 3 && !signal.examples.contains(&shape) {
            signal.examples.push(shape);
        }
        trim(signal);
    }
}

fn trim(signal: &mut Signal) {
    while signal.sessions.len() > SESSIONS {
        signal.sessions.pop_first();
    }
    while signal.seen.len() > SESSIONS * 4 {
        signal.seen.pop_first();
    }
}

/// The gap entry for a signal, made if it is new.
fn entry_for<'a>(gaps: &'a mut Gaps, kind: Kind, subject: &str) -> &'a mut GapEntry {
    let i = gaps.gaps.iter().position(|g| {
        g.signal
            .as_ref()
            .is_some_and(|s| s.kind() == kind && s.subject == subject)
    });
    match i {
        Some(i) => &mut gaps.gaps[i],
        None => {
            gaps.gaps.push(GapEntry {
                signal: Some(Signal {
                    kind: Some(kind),
                    subject: subject.to_string(),
                    ..Signal::default()
                }),
                ..GapEntry::default()
            });
            gaps.gaps.last_mut().expect("just pushed")
        }
    }
}

/// The time budgets a hook is held to, in milliseconds: `budget` for the
/// tool hooks, which run on every call, and ten times that for session
/// start and stop, which wind.
pub(crate) fn hook_budget(event: &str, budget: u64) -> u64 {
    match event {
        "session-start" | "stop" => budget.saturating_mul(10),
        _ => budget,
    }
}

/// Every signal the ledger shows, counted afresh from all of it.
pub(crate) fn from_ledger(entries: &[Entry], engine: Option<&Engine>) -> Vec<Signal> {
    let mut out: BTreeMap<(Kind, String), Signal> = BTreeMap::new();
    let mut add = |kind: Kind,
                   subject: &str,
                   session: Option<&str>,
                   detail: &str,
                   example: &str| {
        let s = out
            .entry((kind, subject.to_string()))
            .or_insert_with(|| Signal {
                kind: Some(kind),
                subject: subject.to_string(),
                ..Signal::default()
            });
        s.count += 1;
        if let Some(session) = session {
            s.sessions.insert(session.to_string());
        }
        if !detail.is_empty() {
            *s.details.entry(detail.to_string()).or_default() += 1;
        }
        if !example.is_empty() && s.examples.len() < 3 && !s.examples.iter().any(|e| e == example) {
            s.examples.push(example.to_string());
        }
    };

    // jouet's own runs, session by session, in order.
    let mut by_session: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in entries
        .iter()
        .filter(|e| e.event.starts_with("post-tool-use") && e.shape.is_some())
    {
        by_session
            .entry(e.session.as_deref().unwrap_or_default())
            .or_default()
            .push(e);
    }
    for (session, list) in &by_session {
        let runs: Vec<Run<'_>> = list
            .iter()
            .map(|e| Run {
                command: e.shape.as_deref().unwrap_or_default(),
                exit_code: e.exit_code,
                output: None,
            })
            .collect();
        for f in jouet::scan(&runs) {
            let example = runs[f.index].command;
            add(
                Kind::from_digest(f.kind),
                &f.subject,
                Some(session),
                &f.detail,
                example,
            );
        }
    }

    // Hooks over budget, as the hook itself recorded them.
    let mut slowest: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.event == "slow-hook") {
        let event = e.tool.clone().unwrap_or_default();
        let ms = e.duration_ms.unwrap_or(0);
        let budget = e.budget_ms.unwrap_or(0);
        let slot = slowest.entry(event.clone()).or_default();
        *slot = (budget.max(slot.0), ms.max(slot.1));
        add(Kind::SlowHook, &event, e.session.as_deref(), "", "");
    }
    for (event, (budget, max)) in slowest {
        if let Some(s) = out.get_mut(&(Kind::SlowHook, event)) {
            s.numbers = vec![budget, max];
        }
    }

    // Built-in rules overridden again and again.
    if let Some(engine) = engine {
        let stats = stats::compute(entries);
        for rule in engine.rules().iter().filter(|r| r.scope == Scope::Builtin) {
            let s = stats.get(&rule.name);
            if s.fired >= OVERRIDE_FIRINGS
                && s.overridden >= OVERRIDES
                && s.override_rate() >= OVERRIDE_RATE
            {
                let fired_on: Vec<String> = entries
                    .iter()
                    .filter(|e| e.event == "override")
                    .filter(|e| e.decisions.iter().any(|d| d.rule == rule.name))
                    .filter_map(|e| e.shape.clone())
                    .collect();
                out.insert(
                    (Kind::Overridden, rule.name.clone()),
                    Signal {
                        kind: Some(Kind::Overridden),
                        subject: rule.name.clone(),
                        count: s.overridden,
                        numbers: vec![s.fired, s.overridden],
                        examples: dedup(fired_on, 5),
                        ..Signal::default()
                    },
                );
            }
        }
    }

    // The same script, written in many sessions.
    let mut scripts: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.event == "pre-tool-use") {
        let (Some(shape), Some(session)) = (e.shape.as_deref(), e.session.as_deref()) else {
            continue;
        };
        if script_like(shape) && jouet::subject(shape).is_none() {
            scripts.entry(shape).or_default().insert(session);
        }
    }
    for (shape, sessions) in scripts {
        if sessions.len() >= ROUTINE_SESSIONS {
            out.insert(
                (Kind::Routine, shape.to_string()),
                Signal {
                    kind: Some(Kind::Routine),
                    subject: shape.to_string(),
                    count: sessions.len() as u64,
                    sessions: sessions.iter().map(|s| s.to_string()).collect(),
                    ..Signal::default()
                },
            );
        }
    }
    out.into_values()
        .map(|mut s| {
            trim(&mut s);
            s
        })
        .collect()
}

fn dedup(list: Vec<String>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        if out.len() == max {
            break;
        }
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// A script, not a command: a loop or conditional, or three commands or
/// more chained together.
fn script_like(shape: &str) -> bool {
    let compound = ["for ", "while ", "until ", "if ", "case "]
        .iter()
        .any(|k| {
            shape.starts_with(k)
                || shape.contains(&format!("&& {k}"))
                || shape.contains(&format!("; {k}"))
        });
    let joins =
        shape.matches(" && ").count() + shape.matches("; ").count() + shape.matches(" | ").count();
    compound || joins >= 2
}

/// Replaces the ledger's signals in the gap file with `fresh`, keeping
/// whether each was shared and to whom it was shown. The digest's
/// signals, kept per session, are left alone.
pub(crate) fn replace_ledger(gaps: &mut Gaps, fresh: Vec<Signal>) {
    for signal in fresh {
        let kind = signal.kind();
        let entry = entry_for(gaps, kind, &signal.subject);
        let old = entry.signal.take().unwrap_or_default();
        // A digest may have seen more of the same: keep the larger count.
        let mut merged = signal;
        if old.count > merged.count {
            merged.count = old.count;
        }
        merged.sessions.extend(old.sessions);
        merged.seen = old.seen;
        for (detail, n) in old.details {
            let slot = merged.details.entry(detail).or_default();
            *slot = (*slot).max(n);
        }
        for example in old.examples {
            if merged.examples.len() < 3 && !merged.examples.contains(&example) {
                merged.examples.push(example);
            }
        }
        trim(&mut merged);
        entry.signal = Some(merged);
    }
}

#[cfg(test)]
mod tests;
