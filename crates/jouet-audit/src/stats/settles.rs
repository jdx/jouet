//! The settle toggle's arms: the pushes, the watchers jouet started for
//! them, the Auto-fix events a watcher covered, the ones jouet held for it,
//! and what its digest did with them.
//!
//! Every session records a push with its `post-tool-use` entry, and an event
//! a live watcher covers as `would_drop = "covered"`, held or not. In the
//! `new` arm, a `watch` entry says what became of each push: a watcher
//! started, or why none did. A watcher records a `settle` entry when it
//! ends, and a hook a `lease-lost` one when it finds a watcher gone without
//! its digest. A held event that neither ever accounts for is an orphaned
//! hold; one a digest left out is a silent drop.

use super::{LedgerEntry, StatsFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a `settle`, `lease-lost` or `watch` entry recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerSettle {
    /// `needs_agent`, `closed`, `timeout`, `parked`, `error`, `stopped` or
    /// `lease-lost`. For a `watch`: `started`, `watching`, `no-pr`,
    /// `foreground` or `error`.
    pub result: String,
    pub held: u64,
    pub accounted: u64,
    pub wake_after_first_held_s: Option<u64>,
    /// The watcher's pid; absent from entries jouet wrote before it was
    /// recorded.
    pub watcher: Option<u32>,
    /// Whether the watcher's end gave the agent a digest to act on.
    pub woke: bool,
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
    /// Sessions with an event a watcher covered or a watcher's entry and,
    /// in `control` and `new`, those with a push.
    pub sessions: u64,
    /// Calls that pushed a branch or opened a PR.
    pub pushes: u64,
    /// What became of the pushes jouet considered a watcher for, by
    /// outcome: `started`, or why none did.
    pub watchers: BTreeMap<String, u64>,
    /// Watchers whose end gave the agent a digest to act on.
    pub wakes: u64,
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
    /// Whether it has anything about a watcher: a session outside the
    /// toggle counts only then, not for its pushes.
    watched: bool,
    pushes: u64,
    watchers: BTreeMap<String, u64>,
    woke: u64,
    covered: u64,
    held: u64,
    settles: BTreeMap<String, u64>,
    accounted: u64,
    silent: u64,
    lost: u64,
    lost_held: u64,
    /// When each hold not yet accounted for was made, and for which
    /// watcher.
    pending: Vec<(u64, Option<u32>)>,
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
    // A watcher records its entry as jouet, not as the agent's harness, so
    // a harness filter keeps the sessions of that harness with their
    // watchers' entries.
    let of_harness: Option<std::collections::BTreeSet<&str>> = filter.harness.as_ref().map(|_| {
        entries
            .iter()
            .filter(|e| filter.keeps(e))
            .filter_map(|e| e.session.as_deref())
            .collect()
    });
    let any_harness = StatsFilter {
        harness: None,
        ..filter.clone()
    };
    let keeps = |e: &LedgerEntry| {
        any_harness.keeps(e)
            && of_harness.as_ref().is_none_or(|set| {
                filter.keeps(e)
                    || (e.harness == "jouet"
                        && e.session.as_deref().is_some_and(|s| set.contains(s)))
            })
    };
    let mut sessions: BTreeMap<&str, Session> = BTreeMap::new();
    for e in entries.iter().filter(|e| keeps(e)) {
        let Some(session) = e.session.as_deref() else {
            continue;
        };
        if let Some(prompt) = e.prompt.as_ref().filter(|_| e.event == "prompt") {
            if prompt.would_drop.as_deref() != Some(COVERED) {
                continue;
            }
            let s = sessions.entry(session).or_default();
            s.watched = true;
            s.covered += 1;
            if prompt.dropped {
                s.held += 1;
                s.pending.push((e.ts, prompt.watcher));
            }
        } else if e.push && e.event == "post-tool-use" {
            sessions.entry(session).or_default().pushes += 1;
        } else if let Some(watch) = e.settle.as_ref().filter(|_| e.event == "watch") {
            let s = sessions.entry(session).or_default();
            s.watched = true;
            *s.watchers.entry(watch.result.clone()).or_default() += 1;
        } else if let Some(settle) = e.settle.as_ref() {
            let s = sessions.entry(session).or_default();
            s.watched = true;
            s.woke += u64::from(settle.woke);
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
            // were held for it, among those held for its watcher when both
            // name one, so overlapping watchers keep their own.
            let mut left = settle.held;
            s.pending.retain(|(at, watcher)| {
                let ours = match (watcher, settle.watcher) {
                    (Some(a), Some(b)) => *a == b,
                    _ => true,
                };
                let mine = left > 0 && *at <= e.ts && ours;
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
                if of != arm || (arm == "off" && !s.watched) {
                    continue;
                }
                out.sessions += 1;
                out.pushes += s.pushes;
                for (outcome, n) in &s.watchers {
                    *out.watchers.entry(outcome.clone()).or_default() += n;
                }
                out.wakes += s.woke;
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
                    .filter(|(at, _)| now.saturating_sub(*at) >= ORPHANED_AFTER_S)
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
                watcher: None,
                woke: result == "needs_agent",
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
            // Three pushes in n1: a watcher for the first, one already
            // there for the second, and a branch with no PR.
            push("n1", 50),
            watch("n1", 51, "started"),
            push("n1", 60),
            watch("n1", 61, "watching"),
            push("n1", 70),
            watch("n1", 71, "no-pr"),
            // A push in the control arm counts; one outside the toggle
            // does not make its session one of these.
            push("c1", 50),
            push("s2", 50),
        ];
        let got = settle_arms(&entries, &arms, &StatsFilter::default(), 800 + day);
        let [control, new, off] = got.try_into().unwrap();
        assert_eq!(
            new,
            SettleArm {
                arm: "new".into(),
                sessions: 1,
                pushes: 3,
                watchers: BTreeMap::from([
                    ("no-pr".into(), 1),
                    ("started".into(), 1),
                    ("watching".into(), 1),
                ]),
                wakes: 1,
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
        assert_eq!(
            (control.pushes, control.watchers.len(), control.wakes),
            (1, 0, 0)
        );
        assert_eq!(
            (off.sessions, off.covered, off.held, off.pushes),
            (1, 1, 0, 0)
        );
        // A session in an arm counts for its pushes alone.
        let only_pushes = settle_arms(
            &[push("c1", 50), push("s2", 50)],
            &arms,
            &StatsFilter::default(),
            100,
        );
        assert_eq!(only_pushes.len(), 1);
        assert_eq!(
            (only_pushes[0].arm.as_str(), only_pushes[0].pushes),
            ("control", 1)
        );
        // Under a day old, a hold may still be in a watcher's wait.
        let recent = settle_arms(&entries, &arms, &StatsFilter::default(), 800 + day - 1);
        assert_eq!(recent[1].orphaned_holds, 1);
        assert!(settle_arms(&[], &arms, &StatsFilter::default(), 0).is_empty());
    }

    fn push(session: &str, ts: u64) -> LedgerEntry {
        LedgerEntry {
            push: true,
            ..entry(session, ts, "post-tool-use")
        }
    }

    fn watch(session: &str, ts: u64, outcome: &str) -> LedgerEntry {
        LedgerEntry {
            settle: Some(LedgerSettle {
                result: outcome.into(),
                watcher: Some(9),
                ..LedgerSettle::default()
            }),
            ..entry(session, ts, "watch")
        }
    }

    fn of_watcher(mut e: LedgerEntry, pid: u32) -> LedgerEntry {
        if let Some(p) = e.prompt.as_mut() {
            p.watcher = Some(pid);
        }
        if let Some(s) = e.settle.as_mut() {
            s.watcher = Some(pid);
        }
        e
    }

    #[test]
    fn overlapping_watchers_account_for_their_own_holds() {
        let day = ORPHANED_AFTER_S;
        let entries = vec![
            of_watcher(covered("n1", 100, true), 7),
            of_watcher(covered("n1", 200, true), 8),
            // Watcher 8 ends first; the older hold is 7's, still waiting.
            of_watcher(settle("n1", 300, "needs_agent", 1, 1), 8),
        ];
        // A day after the hold at 100 only: matched by age alone, 8's
        // settle would have taken it and left none orphaned.
        let got = settle_arms(
            &entries,
            &BTreeMap::new(),
            &StatsFilter::default(),
            200 + day - 1,
        );
        assert_eq!(got[0].orphaned_holds, 1, "the hold at 100 is 7's");
    }

    #[test]
    fn a_harness_filter_keeps_its_sessions_watchers() {
        let codex = |e: LedgerEntry| LedgerEntry {
            harness: "codex".into(),
            ..e
        };
        let entries = vec![
            covered("c1", 100, false),
            settle("c1", 300, "closed", 0, 0),
            codex(covered("x1", 100, false)),
            settle("x1", 300, "closed", 0, 0),
        ];
        let filter = StatsFilter {
            harness: Some("claude".into()),
            ..StatsFilter::default()
        };
        let [off] = settle_arms(&entries, &BTreeMap::new(), &filter, 400)
            .try_into()
            .unwrap();
        assert_eq!(
            (
                off.sessions,
                off.covered,
                off.settles.get("closed").copied()
            ),
            (1, 1, Some(1))
        );
    }
}
