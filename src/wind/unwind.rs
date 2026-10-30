//! Proposing rules to retire: user and repository rules that are noisy,
//! nearly always overridden, or broken. Built-in rules are never proposed;
//! they are jouet's floor.

use super::Paths;
use super::rulefile::{self, Change};
use super::stats::Stats;
use jouet_rules::{Engine, Origin, Scope, State};
use serde::Serialize;

/// Firings before a rule's override rate means anything.
pub(crate) const MIN_FIRINGS: u64 = 5;
/// An override rate at or above this proposes retirement.
pub(crate) const OVERRIDDEN: f64 = 0.5;
/// A rule firing on at least this share of all shell calls, at least
/// [`NOISY_FIRINGS`] times, is noisy.
pub(crate) const NOISY_SHARE: f64 = 0.1;
pub(crate) const NOISY_FIRINGS: u64 = 20;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Proposal {
    pub rule: String,
    pub scope: Scope,
    pub source: String,
    /// Why it should go, in a sentence.
    pub reason: String,
    pub fired: u64,
    pub overridden: u64,
    /// False for a repository rule, which changes through a PR.
    pub can_apply: bool,
}

/// The rules worth retiring, with why.
pub(crate) fn proposals(engine: &Engine, stats: &Stats) -> Vec<Proposal> {
    let mut out = Vec::new();
    for rule in engine.rules() {
        if rule.scope == Scope::Builtin || rule.state == State::Untrusted {
            continue;
        }
        let s = stats.get(&rule.name);
        let reason = if let State::Broken(e) = &rule.state {
            format!("it does not load: {e}")
        } else if s.fired >= MIN_FIRINGS && s.override_rate() >= OVERRIDDEN {
            format!(
                "{} of its {} firings were overridden: the command went ahead anyway",
                s.overridden, s.fired
            )
        } else if s.fired >= NOISY_FIRINGS
            && stats.calls > 0
            && s.fired as f64 / stats.calls as f64 >= NOISY_SHARE
        {
            format!(
                "it fired on {} of {} shell calls; a rule that speaks that often gets tuned out",
                s.fired, stats.calls
            )
        } else {
            continue;
        };
        out.push(Proposal {
            rule: rule.name.clone(),
            scope: rule.scope,
            source: rule.origin.describe(),
            reason,
            fired: s.fired,
            overridden: s.overridden,
            can_apply: matches!(rule.origin, Origin::User(_)),
        });
    }
    out
}

/// Retires the user rules among the proposals, logging each. Returns the
/// changes made and the errors met.
pub(crate) fn apply(
    paths: &Paths,
    proposals: &[Proposal],
    by: &str,
    now: u64,
) -> (Vec<Change>, Vec<String>) {
    let mut changes = Vec::new();
    let mut errors = Vec::new();
    for p in proposals.iter().filter(|p| p.can_apply) {
        let path = paths.rules.join(format!("{}.luau", p.rule));
        match rulefile::retire(&path) {
            Ok(to) => {
                let change = Change {
                    ts: now,
                    rule: p.rule.clone(),
                    change: "retired".into(),
                    from: Some(path.display().to_string()),
                    to: Some(to.display().to_string()),
                    reason: p.reason.clone(),
                    by: by.to_string(),
                };
                match rulefile::log(&paths.log, &change) {
                    Ok(()) => changes.push(change),
                    Err(e) => errors.push(format!("writing the rule log: {e}")),
                }
            }
            Err(e) => errors.push(format!("retiring {}: {e}", path.display())),
        }
    }
    (changes, errors)
}
