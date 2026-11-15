//! The hooks' side of a `jouet wait pr --until needs-agent` watcher's lease
//! (see `jouet_wait::lease`).
//!
//! In the `settle` arm (`crate::experiment`), an Auto-fix event about the PR
//! one live watcher of the session covers is held for that watcher's digest
//! instead of starting a turn. Every session records, as `would_drop =
//! "covered"`, the events it would hold. A watcher that ends without its
//! digest leaves its lease behind; the next hook of its session reports the
//! events held for it, records a `lease-lost` entry, and removes it.

use crate::prompt::{Coverable, Shape};
use jouet_wait::lease::{self, Held, Lease};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Why a covered event did not start a turn.
pub(crate) const HOLD_REASON: &str = "jouet held this Auto-fix event for the `jouet wait pr --until needs-agent` watcher running in this session, which reports it in its digest when the PR settles; set `[experiment] settle_share = 0` in jouet's config to stop.";

/// What tells an agent that pushed with nothing watching its PR to start a
/// watcher.
pub(crate) const PUSH_NOTE: &str = "jouet: nothing in this session watches the PR yet. Start `jouet wait pr --until needs-agent` as a background Bash command (run in the background, its output not piped), then end the turn: it wakes you once the PR settles, with the Auto-fix events jouet held for it.";

fn now() -> i64 {
    jouet_output::time::now()
}

/// The live lease of `session` that covers every PR the event names.
pub(crate) fn covering(session: &str, event: &Coverable) -> Option<PathBuf> {
    let dir = lease::dir()?;
    covering_in(&dir, session, event, now())
}

fn covering_in(dir: &Path, session: &str, event: &Coverable, now: i64) -> Option<PathBuf> {
    Lease::of_session(dir, session)
        .into_iter()
        .find(|(_, l)| l.live(now) && event.prs.iter().all(|(r, n)| l.covers(r, *n)))
        .map(|(path, _)| path)
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
    };
    // The watcher removes its lease before it reads the held file a last
    // time, so a lease still there means the watcher will see this one.
    lease::hold(lease_path, &held).is_ok() && lease_path.exists()
}

/// A watcher that ended without its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lost {
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
        .filter(|(_, l)| !l.live(now))
        .map(|(path, l)| {
            let held = lease::held(&path);
            Lease::remove(&path);
            Lost {
                held: held.len(),
                note: lost_note(&l, &held),
            }
        })
        .collect()
}

fn lost_note(lease: &Lease, held: &[Held]) -> Option<String> {
    let first = held.iter().map(|h| h.at).min()?;
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
        jouet_output::time::format_utc(first)
    );
    Some(note)
}

/// Whether a session has a live watcher.
pub(crate) fn watching(session: &str) -> bool {
    let now = now();
    lease::dir().is_some_and(|dir| {
        Lease::of_session(&dir, session)
            .iter()
            .any(|(_, l)| l.live(now))
    })
}

/// Whether a shell command pushes a branch or opens a PR.
pub(crate) fn pushes(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    let has = |pair: [&str; 2]| words.windows(2).any(|w| w == pair);
    has(["git", "push"])
        || has(["pr", "create"])
        || has(["stack", "submit"])
        || (has(["jouet", "commit"]) && words.contains(&"--push"))
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
            heartbeat,
        }
    }

    fn event(prs: &[(&str, u64)]) -> Coverable {
        Coverable {
            prs: prs.iter().map(|(r, n)| ((*r).to_string(), *n)).collect(),
            checks: vec!["trusted / test".into()],
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
            Some(path.clone())
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
            note.contains("jouet wait pr 12 --repo jdx/jouet --until needs-agent --since "),
            "{note}"
        );
        assert!(lost.iter().any(|l| l.held == 0 && l.note.is_none()));
        // Removed, so the next hook says nothing; the live one stays.
        assert!(lost_in(dir.path(), "s1", 1_010).is_empty());
        assert!(live.exists());
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
        assert!(pushes("jouet commit a.rs --message m --push"));
        assert!(pushes("gh pr create --fill"));
        assert!(pushes("gh stack submit --open"));
        assert!(!pushes("git status"));
        assert!(!pushes("jouet commit a.rs --message m"));
        assert!(!pushes("echo pushing"));
    }
}
