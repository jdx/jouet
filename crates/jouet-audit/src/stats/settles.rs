//! The settle toggle's arms: the Auto-fix events a `jouet wait pr --until
//! needs-agent` watcher covered, the ones jouet held for it, and what its
//! digest did with them.
//!
//! Every session records an event a live watcher covers as `would_drop =
//! "covered"`, held or not. A watcher records a `settle` entry when it ends
//! and a hook a `lease-lost` one when it finds a watcher gone without its
//! digest. A held event that neither ever accounts for is an orphaned hold;
//! one a digest left out is a silent drop.

use super::{LedgerEntry, StatsFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a `settle` or `lease-lost` entry recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerSettle {
    /// `needs_agent`, `closed`, `timeout`, `parked`, `error` or
    /// `lease-lost`.
    pub result: String,
    pub held: u64,
    pub accounted: u64,
    pub wake_after_first_held_s: Option<u64>,
}

/// A hold with no settle after it for this long is orphaned: a watcher
/// waits a day at most.
pub const ORPHANED_AFTER_S: u64 = 25 * 60 * 60;

/// One arm of the settle toggle, or `off` for the sessions it does not
/// assign one.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SettleArm {
    /// `control`, `new` or `off`.
    pub arm: String,
    /// Sessions with an event a watcher covered, or a watcher's entry.
    pub sessions: u64,
    /// Events a live watcher covered.
    pub covered: u64,
    /// Of those, the ones jouet held.
    pub held: u64,
    /// Watchers that ended, by result.
    pub settles: BTreeMap<String, u64>,
    /// Held events a watcher's digest showed or said were over.
    pub accounted: u64,
    /// Held events a digest left out.
    pub silent_drops: u64,
    /// Watchers a hook found gone without their digest, and the events
    /// held for them, which that hook reported.
    pub lease_lost: u64,
    pub lease_lost_held: u64,
    /// Holds no watcher or hook accounted for in [`ORPHANED_AFTER_S`].
    pub orphaned_holds: u64,
    /// The median seconds from a watcher's first held event to its wake.
    pub median_wake_after_first_held_s: Option<u64>,
}

#[derive(Default)]
struct Session {
    covered: u64,
    held: u64,
    settles: BTreeMap<String, u64>,
    accounted: u64,
    silent: u64,
    lost: u64,
    lost_held: u64,
    /// When each hold not yet accounted for was made.
    pending: Vec<u64>,
    wakes: Vec<u64>,
}

/// The settle toggle's arms from the entries `filter` keeps, as of `now`
/// (Unix seconds); none when no session saw a watcher.
pub fn settle_arms(
    entries: &[LedgerEntry],
    arms: &BTreeMap<String, BTreeMap<String, String>>,
    filter: &StatsFilter,
    now: u64,
) -> Vec<SettleArm> {
    // A watcher records its entry as jouet, not as the agent's harness.
    let filter = StatsFilter {
        harness: None,
        ..filter.clone()
    };
    let mut sessions: BTreeMap<&str, Session> = BTreeMap::new();
    for e in entries.iter().filter(|e| filter.keeps(e)) {
        let Some(session) = e.session.as_deref() else {
            continue;
        };
        if let Some(prompt) = e.prompt.as_ref().filter(|_| e.event == "prompt") {
            if prompt.would_drop.as_deref() != Some(COVERED) {
                continue;
            }
            let s = sessions.entry(session).or_default();
            s.covered += 1;
            if prompt.dropped {
                s.held += 1;
                s.pending.push(e.ts);
            }
        } else if let Some(settle) = e.settle.as_ref() {
            let s = sessions.entry(session).or_default();
            if settle.result == "lease-lost" {
                s.lost += 1;
                s.lost_held += settle.held;
            } else {
                *s.settles.entry(settle.result.clone()).or_default() += 1;
                s.accounted += settle.accounted;
                s.silent += settle.held.saturating_sub(settle.accounted);
                s.wakes.extend(settle.wake_after_first_held_s);
            }
            // Each accounts for as many of the oldest holds before it as
            // were held for it.
            let mut left = settle.held;
            s.pending.retain(|at| {
                let mine = left > 0 && *at <= e.ts;
                left -= u64::from(mine);
                !mine
            });
        }
    }
    ["control", "new", "off"]
        .into_iter()
        .filter_map(|arm| {
            let mut out = SettleArm {
                arm: arm.to_string(),
                ..SettleArm::default()
            };
            let mut wakes = Vec::new();
            for (session, s) in &sessions {
                let of = arms
                    .get(*session)
                    .and_then(|a| a.get(SETTLE))
                    .map_or("off", String::as_str);
                if of != arm {
                    continue;
                }
                out.sessions += 1;
                out.covered += s.covered;
                out.held += s.held;
                for (result, n) in &s.settles {
                    *out.settles.entry(result.clone()).or_default() += n;
                }
                out.accounted += s.accounted;
                out.silent_drops += s.silent;
                out.lease_lost += s.lost;
                out.lease_lost_held += s.lost_held;
                out.orphaned_holds += s
                    .pending
                    .iter()
                    .filter(|at| now.saturating_sub(**at) >= ORPHANED_AFTER_S)
                    .count() as u64;
                wakes.extend(&s.wakes);
            }
            if out.sessions == 0 {
                return None;
            }
            wakes.sort_unstable();
            out.median_wake_after_first_held_s = wakes.get(wakes.len() / 2).copied();
            Some(out)
        })
        .collect()
}

const SETTLE: &str = "settle";
const COVERED: &str = "covered";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::LedgerPrompt;

    fn entry(session: &str, ts: u64, event: &str) -> LedgerEntry {
        LedgerEntry {
            ts,
            harness: "claude".into(),
            session: Some(session.into()),
            event: event.into(),
            ..LedgerEntry::default()
        }
    }

    fn covered(session: &str, ts: u64, held: bool) -> LedgerEntry {
        LedgerEntry {
            prompt: Some(LedgerPrompt {
                class: "ci-monitor".into(),
                would_drop: Some(COVERED.into()),
                dropped: held,
                ..LedgerPrompt::default()
            }),
            ..entry(session, ts, "prompt")
        }
    }

    fn settle(session: &str, ts: u64, result: &str, held: u64, accounted: u64) -> LedgerEntry {
        LedgerEntry {
            harness: "jouet".into(),
            settle: Some(LedgerSettle {
                result: result.into(),
                held,
                accounted,
                wake_after_first_held_s: (held > 0).then_some(ts - 100),
            }),
            ..entry(
                session,
                ts,
                if result == "lease-lost" {
                    "lease-lost"
                } else {
                    "settle"
                },
            )
        }
    }

    #[test]
    fn each_arm_counts_holds_and_what_became_of_them() {
        let arms: BTreeMap<String, BTreeMap<String, String>> = [("n1", "new"), ("c1", "control")]
            .into_iter()
            .map(|(s, a)| {
                (
                    s.to_string(),
                    BTreeMap::from([(SETTLE.to_string(), a.to_string())]),
                )
            })
            .collect();
        let day = ORPHANED_AFTER_S;
        let entries = vec![
            // n1: two holds a digest covered one of, one a lost lease
            // reported, and two no watcher accounted for (a parked one held
            // nothing).
            covered("n1", 100, true),
            covered("n1", 150, true),
            settle("n1", 300, "needs_agent", 2, 1),
            covered("n1", 400, true),
            settle("n1", 500, "lease-lost", 1, 0),
            covered("n1", 600, true),
            settle("n1", 700, "parked", 0, 0),
            covered("n1", 800, true),
            // c1: covered, never held.
            covered("c1", 100, false),
            settle("c1", 300, "closed", 0, 0),
            // No arm: the default config still records what a watcher covered.
            covered("s1", 100, false),
            entry("s1", 120, "edit"),
            // Not about a watcher.
            entry("s2", 100, "edit"),
        ];
        let got = settle_arms(&entries, &arms, &StatsFilter::default(), 800 + day);
        let [control, new, off] = got.try_into().unwrap();
        assert_eq!(
            new,
            SettleArm {
                arm: "new".into(),
                sessions: 1,
                covered: 5,
                held: 5,
                settles: BTreeMap::from([("needs_agent".into(), 1), ("parked".into(), 1)]),
                accounted: 1,
                silent_drops: 1,
                lease_lost: 1,
                lease_lost_held: 1,
                orphaned_holds: 2,
                median_wake_after_first_held_s: Some(200),
            }
        );
        assert_eq!(
            (control.covered, control.held, control.settles.len()),
            (1, 0, 1)
        );
        assert_eq!((off.sessions, off.covered, off.held), (1, 1, 0));
        // Under a day old, a hold may still be in a watcher's wait.
        let recent = settle_arms(&entries, &arms, &StatsFilter::default(), 800 + day - 1);
        assert_eq!(recent[1].orphaned_holds, 1);
        assert!(settle_arms(&[], &arms, &StatsFilter::default(), 0).is_empty());
    }
}
