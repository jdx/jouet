//! How each rule has done, from the ledger: how often it denied or warned,
//! and how often the agent went ahead anyway.
//!
//! Overrides are the ledger's own `override` entries: the hooks record one
//! when a command of the same shape as one a rule denied or warned about
//! runs within ten minutes in the same session.

use crate::ledger::Entry;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RuleStats {
    pub fired: u64,
    pub overridden: u64,
    pub sessions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<u64>,
}

impl RuleStats {
    /// Overrides as a share of firings, 0 to 1.
    pub fn override_rate(&self) -> f64 {
        if self.fired == 0 {
            0.0
        } else {
            self.overridden as f64 / self.fired as f64
        }
    }
}

#[derive(Debug, Default)]
pub struct Stats {
    pub rules: BTreeMap<String, RuleStats>,
    /// Pre-tool-use shell calls the ledger holds, for judging noise.
    pub calls: u64,
}

impl Stats {
    pub fn get(&self, rule: &str) -> RuleStats {
        self.rules.get(rule).cloned().unwrap_or_default()
    }
}

/// Every rule's stats over the entries.
pub fn compute(entries: &[Entry]) -> Stats {
    let mut stats = Stats::default();
    let mut sessions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for entry in entries {
        let session = entry.session.as_deref().unwrap_or("");
        match entry.event.as_str() {
            "pre-tool-use" => {
                stats.calls += 1;
                for d in entry
                    .decisions
                    .iter()
                    .filter(|d| d.action == "deny" || d.action == "warn")
                {
                    let s = stats.rules.entry(d.rule.clone()).or_default();
                    s.fired += 1;
                    s.first = Some(s.first.map_or(entry.ts, |f| f.min(entry.ts)));
                    s.last = Some(s.last.map_or(entry.ts, |l| l.max(entry.ts)));
                    sessions.entry(&d.rule).or_default().insert(session);
                }
            }
            "override" => {
                for d in &entry.decisions {
                    stats.rules.entry(d.rule.clone()).or_default().overridden += 1;
                }
            }
            _ => {}
        }
    }
    for (rule, set) in sessions {
        if let Some(s) = stats.rules.get_mut(rule) {
            s.sessions = set.len() as u64;
        }
    }
    stats
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ledger::Decision;

    fn entry(ts: u64, session: &str, event: &str, script: &str, rules: &[(&str, &str)]) -> Entry {
        Entry {
            ts,
            harness: "claude".into(),
            session: Some(session.into()),
            event: event.into(),
            tool: Some("Bash".into()),
            shape: Some(crate::ledger::shape(script)),
            decisions: rules
                .iter()
                .map(|(rule, action)| Decision {
                    rule: (*rule).into(),
                    action: (*action).into(),
                })
                .collect(),
            ..Entry::default()
        }
    }

    /// A pre-tool-use entry in which each rule warned.
    pub fn fired(ts: u64, session: &str, script: &str, rules: &[&str]) -> Entry {
        let warned: Vec<(&str, &str)> = rules.iter().map(|r| (*r, "warn")).collect();
        entry(ts, session, "pre-tool-use", script, &warned)
    }

    /// The ledger's record of a rule overridden.
    pub fn overridden(ts: u64, session: &str, script: &str, rule: &str) -> Entry {
        entry(ts, session, "override", script, &[(rule, "override")])
    }

    #[test]
    fn counts_firings_sessions_and_the_ledgers_overrides() {
        let entries = vec![
            fired(1, "a", "x", &["r"]),
            fired(2, "a", "y", &[]),
            overridden(3, "a", "x", "r"),
            fired(3, "a", "x", &["r"]),
            fired(4, "b", "x", &["r", "q"]),
            entry(5, "b", "post-tool-use", "x", &[("checks", "record:test")]),
        ];
        let stats = compute(&entries);
        assert_eq!(stats.calls, 4);
        let r = stats.get("r");
        assert_eq!((r.fired, r.overridden, r.sessions), (3, 1, 2));
        assert_eq!((r.first, r.last), (Some(1), Some(4)));
        assert!((r.override_rate() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(stats.get("q").overridden, 0);
        assert_eq!(stats.get("checks"), RuleStats::default());
    }
}
