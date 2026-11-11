//! The drop toggle's arms: how many turns the desktop app's events start in
//! each session, and how the noise-only events jouet would drop turned out.
//!
//! Every session records, on each prompt, whether jouet would drop it. In
//! the control arm those events still reach the agent, so whether the turn
//! they started edited a file, committed or replied says how often a drop
//! would have hidden work.

use super::{LedgerEntry, StatsFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a `prompt` entry recorded about the message that started a turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerPrompt {
    /// `user`, `ci-monitor`, `side-sessions`, `session-message` or
    /// `task-notification`.
    pub class: String,
    pub kinds: Vec<String>,
    /// The noise class of an event jouet would drop, in either arm.
    pub would_drop: Option<String>,
    /// The drop arm blocked it.
    pub dropped: bool,
}

/// One arm of the drop toggle.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DropArm {
    /// `control` or `new`.
    pub arm: String,
    pub sessions: u64,
    /// Turns the app's events started: every prompt not the user's, less
    /// those dropped.
    pub app_turns: u64,
    /// `app_turns` per session.
    pub app_turns_per_session: f64,
    /// Events jouet would drop.
    pub would_drop: u64,
    /// Events it dropped (the new arm only).
    pub dropped: u64,
    /// Would-drops that reached the agent and whose turn edited a file,
    /// committed, pushed or replied: an upper bound on the work a drop
    /// would have hidden, since a turn may do work queued from before.
    pub would_drop_with_work: u64,
}

/// Commands that commit, push or reply, by the start of their shape.
const WORK: &[&str] = &[
    "git commit",
    "git push",
    "git merge",
    "jouet commit",
    "gh pr comment",
    "gh issue comment",
    "gh pr review",
];

/// Whether `e` is work a turn did: an edit, or a call any of whose
/// chained commands commits, pushes or replies.
fn is_work(e: &LedgerEntry) -> bool {
    if e.event == "edit" {
        return true;
    }
    if e.event != "pre-tool-use" {
        return false;
    }
    let Some(shape) = e.shape.as_deref() else {
        return false;
    };
    shape
        .split(['&', ';', '|', '\n'])
        .map(str::trim)
        .any(|command| {
            WORK.iter().any(|w| command.starts_with(w))
                // A `gh api` call does not count: its shape keeps neither
                // the endpoint nor the method, so a reply and a read look
                // the same, and jouet sends thread replies here instead.
                || command.starts_with("jouet pr threads") && command.contains("--body")
        })
}

/// The drop toggle's arms over the sessions `arms` assigns one, from the
/// entries `filter` keeps; none when no session has a drop arm.
pub fn drop_arms(
    entries: &[LedgerEntry],
    arms: &BTreeMap<String, BTreeMap<String, String>>,
    filter: &StatsFilter,
) -> Vec<DropArm> {
    // Per session: app turns, would-drops, drops, and would-drops with work.
    let mut sessions: BTreeMap<&str, [u64; 4]> = BTreeMap::new();
    // The session's open would-drop turn, and whether it has done work.
    let mut open: BTreeMap<&str, bool> = BTreeMap::new();
    let close = |session: &str, open: &mut BTreeMap<&str, bool>, counts: &mut [u64; 4]| {
        if open.remove(session) == Some(true) {
            counts[3] += 1;
        }
    };
    for e in entries.iter().filter(|e| filter.keeps(e)) {
        let Some(session) = e.session.as_deref() else {
            continue;
        };
        if !arms.get(session).is_some_and(|a| a.contains_key(DROP)) {
            continue;
        }
        let counts = sessions.entry(session).or_default();
        if let Some(prompt) = e.prompt.as_ref().filter(|_| e.event == "prompt") {
            close(session, &mut open, counts);
            if prompt.class != "user" && !prompt.dropped {
                counts[0] += 1;
            }
            if prompt.would_drop.is_some() {
                counts[1] += 1;
                if prompt.dropped {
                    counts[2] += 1;
                } else {
                    open.insert(session, false);
                }
            }
        } else if is_work(e)
            && let Some(work) = open.get_mut(session)
        {
            *work = true;
        }
    }
    for (session, counts) in &mut sessions {
        close(session, &mut open, counts);
    }
    ["control", "new"]
        .into_iter()
        .filter_map(|arm| {
            let mut out = DropArm {
                arm: arm.to_string(),
                ..DropArm::default()
            };
            for (session, counts) in &sessions {
                if arms[*session][DROP] != arm {
                    continue;
                }
                out.sessions += 1;
                out.app_turns += counts[0];
                out.would_drop += counts[1];
                out.dropped += counts[2];
                out.would_drop_with_work += counts[3];
            }
            if out.sessions == 0 {
                return None;
            }
            out.app_turns_per_session = out.app_turns as f64 / out.sessions as f64;
            Some(out)
        })
        .collect()
}

const DROP: &str = "drop";

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(session: &str, event: &str) -> LedgerEntry {
        LedgerEntry {
            harness: "claude".into(),
            session: Some(session.into()),
            event: event.into(),
            ..LedgerEntry::default()
        }
    }

    fn prompt(session: &str, class: &str, would_drop: bool, dropped: bool) -> LedgerEntry {
        LedgerEntry {
            prompt: Some(LedgerPrompt {
                class: class.into(),
                would_drop: would_drop.then(|| "rate_limit".into()),
                dropped,
                ..LedgerPrompt::default()
            }),
            ..entry(session, "prompt")
        }
    }

    fn command(session: &str, shape: &str) -> LedgerEntry {
        LedgerEntry {
            shape: Some(shape.into()),
            ..entry(session, "pre-tool-use")
        }
    }

    #[test]
    fn each_arm_counts_app_turns_and_would_drops_that_did_work() {
        let arms: BTreeMap<String, BTreeMap<String, String>> =
            [("c1", "control"), ("c2", "control"), ("n1", "new")]
                .into_iter()
                .map(|(s, a)| {
                    (
                        s.to_string(),
                        BTreeMap::from([(DROP.to_string(), a.to_string())]),
                    )
                })
                .chain([("other".to_string(), BTreeMap::new())])
                .collect();
        let entries = vec![
            // c1: a would-drop that replied, one that only read, then one
            // whose turn committed, left open at the end.
            prompt("c1", "user", false, false),
            prompt("c1", "ci-monitor", true, false),
            command(
                "c1",
                "cd <arg> && jouet pr threads reply <arg> <arg> --resolve --body <arg>",
            ),
            prompt("c1", "ci-monitor", true, false),
            command("c1", "git status"),
            command("c1", "jouet pr threads <arg>"),
            command("c1", "gh api <arg> -X <arg> -f <arg>"),
            prompt("c1", "ci-monitor", false, false),
            entry("c1", "edit"),
            prompt("c1", "ci-monitor", true, false),
            command(
                "c1",
                "cd <arg> && git add <arg> && git commit --message <arg>",
            ),
            // c2: a would-drop, then the user's prompt starts the work.
            prompt("c2", "ci-monitor", true, false),
            prompt("c2", "user", false, false),
            entry("c2", "edit"),
            // n1: a dropped event starts no turn; one that passed does.
            prompt("n1", "ci-monitor", true, true),
            entry("n1", "edit"),
            prompt("n1", "side-sessions", false, false),
            // Not in the experiment.
            prompt("other", "ci-monitor", true, false),
        ];
        let [control, new] = drop_arms(&entries, &arms, &StatsFilter::default())
            .try_into()
            .unwrap();
        assert_eq!(
            control,
            DropArm {
                arm: "control".into(),
                sessions: 2,
                app_turns: 5,
                app_turns_per_session: 2.5,
                would_drop: 4,
                dropped: 0,
                would_drop_with_work: 2,
            }
        );
        assert_eq!(
            (new.sessions, new.app_turns, new.would_drop, new.dropped),
            (1, 1, 1, 1)
        );
        assert_eq!(new.would_drop_with_work, 0);
        assert!(drop_arms(&entries, &BTreeMap::new(), &StatsFilter::default()).is_empty());
    }
}
