//! The hooks' side of a PR watcher's lease (see `jouet_wait::lease`).
//!
//! In the `settle` arm (`crate::experiment`), jouet starts a watcher itself
//! after a push (`jouet watch`, see `crate::cmd::watch`), and an Auto-fix
//! event about the PR one live watcher of the session covers is held for
//! that watcher's digest instead of starting a turn. Every session records,
//! as `would_drop = "covered"`, the events it would hold. A watcher that
//! ends without its digest leaves its lease behind; the next hook of its
//! session reports the events held for it, records a `lease-lost` entry,
//! and removes it.

use crate::prompt::{Coverable, Shape};
use jouet_wait::lease::{self, Held, Lease};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Why a covered event did not start a turn.
pub(crate) const HOLD_REASON: &str = "jouet held this Auto-fix event for the PR watcher running in this session (the one jouet started after a push, or a `jouet wait pr --until needs-agent`), which reports it in its digest when the PR settles; set `[experiment] settle_share = 0` in jouet's config to stop.";

fn now() -> i64 {
    jouet_output::time::now()
}

/// The live lease of `session` that covers every PR the event names, with
/// its watcher's pid.
pub(crate) fn covering(session: &str, event: &Coverable) -> Option<(PathBuf, u32)> {
    let dir = lease::dir()?;
    covering_in(&dir, session, event, now())
}

fn covering_in(dir: &Path, session: &str, event: &Coverable, now: i64) -> Option<(PathBuf, u32)> {
    Lease::of_session(dir, session)
        .into_iter()
        .find(|(_, l)| l.live(now) && event.prs.iter().all(|(r, n)| l.covers(r, *n)))
        .map(|(path, l)| (path, l.pid))
}

/// Holds the event for the watcher of `lease`. False when it could not be
/// written, or the watcher ended meanwhile, so the event starts a turn.
pub(crate) fn hold(lease_path: &Path, event: &Coverable, shape: &Shape) -> bool {
    let held = Held {
        at: now(),
        prs: event.prs.iter().map(|(r, n)| format!("{r}#{n}")).collect(),
        kinds: shape.kinds.clone(),
        comments: shape.comments,
        checks: event.checks.clone(),
        comment_ids: event.comments.clone(),
    };
    // The watcher removes its lease before it reads the held file a last
    // time, so a lease still there means the watcher will see this one, or
    // has left the lease for the next hook to report it.
    lease::hold(lease_path, &held).is_ok() && lease_path.exists()
}

/// A watcher that ended without its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lost {
    /// The watcher's pid.
    pub pid: u32,
    pub held: usize,
    /// What to tell the agent, when events were held for it.
    pub note: Option<String>,
}

/// The leases of `session` whose watchers are gone, each removed.
pub(crate) fn lost(session: &str) -> Vec<Lost> {
    lease::dir().map_or_else(Vec::new, |dir| lost_in(&dir, session, now()))
}

fn lost_in(dir: &Path, session: &str, now: i64) -> Vec<Lost> {
    Lease::of_session(dir, session)
        .into_iter()
        .filter(|(_, l)| l.lost(now))
        .map(|(path, l)| {
            // The lease goes first: a hook that held an event while it was
            // there has written it by now, and one that writes later finds
            // no lease and lets its event through (`hold`).
            let _ = std::fs::remove_file(&path);
            let held = lease::held(&path);
            Lease::remove(&path);
            Lost {
                pid: l.pid,
                held: held.len(),
                note: lost_note(&l, &held),
            }
        })
        .collect()
}

/// What to tell the agent about the events held for a watcher that ended
/// without its digest; nothing when none were.
pub(crate) fn lost_note(lease: &Lease, held: &[Held]) -> Option<String> {
    let first = held.iter().map(|h| h.at).min()?;
    // The watcher's own --since, so a restart sees the comments it did:
    // they were posted before their events were held.
    let since = Some(lease.since)
        .filter(|s| *s > 0)
        .unwrap_or_else(|| lease.started.min(first));
    let n = held.len();
    let mut note = format!(
        "jouet: the `jouet wait pr` watcher for {}#{} stopped before its digest, with {n} Auto-fix event{} jouet held for it:",
        lease.repo,
        lease.pr,
        if n == 1 { "" } else { "s" }
    );
    for h in held {
        let checks = if h.checks.is_empty() {
            String::new()
        } else {
            format!(" ({})", h.checks.join(", "))
        };
        let _ = write!(
            note,
            " {} {}{checks};",
            jouet_output::time::format_utc(h.at),
            h.kinds.join(", ")
        );
    }
    let _ = write!(
        note,
        " look at the PR, or start the watcher again in the background: `jouet wait pr {} --repo {} --until needs-agent --since {}`",
        lease.pr,
        lease.repo,
        jouet_output::time::format_utc(since)
    );
    Some(note)
}

/// The word pairs of a command that pushes a branch or opens a PR.
pub(crate) const PUSH_WORDS: [[&str; 2]; 3] =
    [["git", "push"], ["pr", "create"], ["stack", "submit"]];

/// Whether a shell command pushes a branch or opens a PR.
pub(crate) fn pushes(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    PUSH_WORDS
        .iter()
        .any(|pair| words.windows(2).any(|w| w == pair))
}

/// Whether a hook's payload reports a push that worked: a foreground Bash
/// call that exited 0 and pushed a branch or opened a PR.
pub(crate) fn pushed(payload: &serde_json::Value, call: &jouet_rules::Call) -> bool {
    matches!(call.event, Some(jouet_rules::Event::PostToolUse))
        && call.tool == "Bash"
        && call.exit_code == Some(0)
        && !crate::cmd::hook::in_background(payload)
        && call.command.as_deref().is_some_and(pushes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease_at(session: &str, pid: u32, heartbeat: i64) -> Lease {
        Lease {
            pid,
            session: session.into(),
            repo: "jdx/jouet".into(),
            pr: 12,
            branch: None,
            started: heartbeat,
            since: heartbeat - 60,
            heartbeat,
        }
    }

    fn event(prs: &[(&str, u64)]) -> Coverable {
        Coverable {
            prs: prs.iter().map(|(r, n)| ((*r).to_string(), *n)).collect(),
            checks: vec!["trusted / test".into()],
            comments: Vec::new(),
        }
    }

    #[test]
    fn only_a_live_lease_of_the_session_covering_every_pr_covers_an_event() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let path = Lease::path(dir.path(), "s1", me).unwrap();
        lease_at("s1", me, 1_000).write(&path).unwrap();
        let now = 1_010;
        assert_eq!(
            covering_in(dir.path(), "s1", &event(&[("jdx/jouet", 12)]), now),
            Some((path.clone(), me))
        );
        assert_eq!(
            covering_in(dir.path(), "s2", &event(&[("jdx/jouet", 12)]), now),
            None
        );
        assert_eq!(
            covering_in(
                dir.path(),
                "s1",
                &event(&[("jdx/jouet", 12), ("jdx/jouet", 13)]),
                now
            ),
            None
        );
        // Its heartbeat went stale.
        assert_eq!(
            covering_in(
                dir.path(),
                "s1",
                &event(&[("jdx/jouet", 12)]),
                1_000 + lease::STALE
            ),
            None
        );
    }

    #[test]
    fn a_gone_watcher_reports_what_was_held_for_it_once() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let live = Lease::path(dir.path(), "s1", me).unwrap();
        lease_at("s1", me, 1_000).write(&live).unwrap();
        let gone = Lease::path(dir.path(), "s1", u32::MAX - 1).unwrap();
        lease_at("s1", u32::MAX - 1, 1_000).write(&gone).unwrap();
        let empty = Lease::path(dir.path(), "s1", u32::MAX - 2).unwrap();
        lease_at("s1", u32::MAX - 2, 1_000).write(&empty).unwrap();
        let shape = Shape {
            kinds: vec!["ci-failure".into()],
            ..Shape::default()
        };
        assert!(hold(&gone, &event(&[("jdx/jouet", 12)]), &shape));
        let lost = lost_in(dir.path(), "s1", 1_010);
        assert_eq!(lost.len(), 2);
        let with_held = lost.iter().find(|l| l.held == 1).unwrap();
        let note = with_held.note.as_deref().unwrap();
        assert!(
            note.contains("jdx/jouet#12 stopped before its digest"),
            "{note}"
        );
        assert!(note.contains("ci-failure (trusted / test)"), "{note}");
        assert!(
            note.contains(&format!(
                "jouet wait pr 12 --repo jdx/jouet --until needs-agent --since {}",
                jouet_output::time::format_utc(940)
            )),
            "{note}"
        );
        assert!(lost.iter().any(|l| l.held == 0 && l.note.is_none()));
        // Removed, so the next hook says nothing; the live one stays.
        assert!(lost_in(dir.path(), "s1", 1_010).is_empty());
        assert!(live.exists());
        // So does the lease of a watcher that was told to stop and is still
        // running: it covers nothing, and is not taken away under a hook
        // that is holding an event for it.
        lease_at("s1", me, 1_000)
            .leaving(1_010)
            .write(&live)
            .unwrap();
        assert!(covering_in(dir.path(), "s1", &event(&[("jdx/jouet", 12)]), 1_010).is_none());
        assert!(lost_in(dir.path(), "s1", 1_011).is_empty());
        assert!(live.exists());
        assert_eq!(lost_in(dir.path(), "s1", 1_100).len(), 1);
    }

    #[test]
    fn a_hold_after_the_watcher_removed_its_lease_lets_the_event_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = Lease::path(dir.path(), "s1", 1).unwrap();
        assert!(!hold(
            &path,
            &event(&[("jdx/jouet", 12)]),
            &Shape::default()
        ));
    }

    #[test]
    fn pushes_and_pr_creation_are_recognized() {
        assert!(pushes("git push -u origin feat/x"));
        assert!(pushes("gh pr create --fill"));
        assert!(pushes("gh stack submit --open"));
        assert!(!pushes("git status"));
        assert!(!pushes("jouet commit a.rs --message m"));
        assert!(!pushes("echo pushing"));
    }
}
