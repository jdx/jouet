//! The lease a `wait pr --until needs-agent` watcher holds while an agent's
//! session waits on it, and the Auto-fix events held for it.
//!
//! A watcher started in an agent's session (one with a session ID) writes a
//! lease under `$XDG_STATE_HOME/jouet/watch/`, one file per watcher:
//! its pid, the PR it covers and a heartbeat it rewrites every
//! [`HEARTBEAT`] from a thread of its own, so a long poll interval never
//! makes it look stale. It removes the file when it exits in order.
//!
//! The hooks read the leases of their session. One that is live (its process
//! is running and its heartbeat is under [`STALE`] old) covers its PR, and an
//! Auto-fix event about that PR alone can wait for the watcher's digest
//! instead of starting a turn. When jouet holds one, the hook appends what it
//! said to the lease's held file, and the watcher accounts for each held
//! event when it wakes the agent. Anything this cannot read, or a lease that
//! is not live, lets the event through.

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// How often the watcher rewrites its heartbeat.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);
/// A heartbeat older than this, in seconds, means the watcher is gone.
pub const STALE: i64 = 90;

/// One watcher's lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub pid: u32,
    /// The session, as its ID was given.
    pub session: String,
    /// `owner/name`.
    pub repo: String,
    /// The PR, once the watcher has resolved its target; 0 before.
    #[serde(default)]
    pub pr: u64,
    /// The branch the watcher was started for, when it was given none and
    /// took the current one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// When it started (Unix seconds).
    pub started: i64,
    /// Its `--since`: comments from then on are new to it (Unix seconds).
    #[serde(default)]
    pub since: i64,
    /// Its latest heartbeat (Unix seconds).
    pub heartbeat: i64,
}

/// An Auto-fix event jouet held for a watcher.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// When it was held (Unix seconds).
    pub at: i64,
    /// The PRs it names, as `owner/name#N`.
    pub prs: Vec<String>,
    /// What it reports: `review-comment`, `ci-failure` or `conflict`.
    pub kinds: Vec<String>,
    /// How many comments it relays.
    #[serde(default)]
    pub comments: u64,
    /// The failing checks it names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
    /// The IDs of the comments it relays, when it names every one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comment_ids: Vec<u64>,
}

/// `$XDG_STATE_HOME/jouet/watch`.
pub fn dir() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("watch"))
}

/// The session ID as it may appear in a file name.
fn safe(session: &str) -> Option<String> {
    let safe: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    (!safe.is_empty()).then_some(safe)
}

impl Lease {
    /// Where the lease of `session`'s watcher `pid` lives under `dir`.
    pub fn path(dir: &Path, session: &str, pid: u32) -> Option<PathBuf> {
        Some(dir.join(format!("settle-{}-{pid}.json", safe(session)?)))
    }

    /// The file the events held for this watcher are appended to.
    pub fn held_path(lease: &Path) -> PathBuf {
        lease.with_extension("held")
    }

    /// Writes the lease whole: to a temporary file, then renamed, so a
    /// hook never reads half of one.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// Every lease of `session` under `dir`, live or not, with its path.
    pub fn of_session(dir: &Path, session: &str) -> Vec<(PathBuf, Self)> {
        let Some(prefix) = safe(session).map(|s| format!("settle-{s}-")) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<(PathBuf, Self)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
            })
            .filter_map(|p| Self::read(&p).map(|l| (p, l)))
            // Another session whose safe ID shares the prefix.
            .filter(|(_, l)| l.session == session)
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Writes the lease under `dir`, unless a live lease of its session
    /// already covers its PR: `None` then. Looking and writing are one step
    /// under a lock, so of two watchers started at once for one PR (two
    /// hooks that ran for the same push), exactly one gets the lease.
    pub fn claim(&self, dir: &Path, now: i64) -> std::io::Result<Option<PathBuf>> {
        let path = Self::path(dir, &self.session, self.pid)
            .ok_or_else(|| std::io::Error::other("the session ID names no file"))?;
        std::fs::create_dir_all(dir)?;
        let lock = std::fs::File::create(dir.join("claim.lock"))?;
        lock.lock()?;
        let covered = Self::of_session(dir, &self.session)
            .iter()
            .any(|(_, l)| l.pid != self.pid && l.live(now) && l.covers(&self.repo, self.pr));
        if covered {
            return Ok(None);
        }
        self.write(&path)?;
        Ok(Some(path))
    }

    /// Whether its watcher is still there: the process runs and the
    /// heartbeat is fresh.
    pub fn live(&self, now: i64) -> bool {
        now - self.heartbeat < STALE && crate::os::process_running(self.pid)
    }

    /// Whether it covers `repo`'s PR `pr`.
    pub fn covers(&self, repo: &str, pr: u64) -> bool {
        self.pr != 0 && self.pr == pr && self.repo.eq_ignore_ascii_case(repo)
    }

    /// Removes the lease and its held events.
    pub fn remove(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(Self::held_path(path));
    }
}

/// Appends a held event to `lease`'s held file: one line with O_APPEND, so
/// two hooks at once never interleave within a line.
pub fn hold(lease: &Path, held: &Held) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(held)?;
    line.push(b'\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Lease::held_path(lease))?
        .write_all(&line)
}

/// The events held for the watcher of `lease`, oldest first.
pub fn held(lease: &Path) -> Vec<Held> {
    std::fs::read_to_string(Lease::held_path(lease))
        .map(|text| {
            text.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(session: &str, pid: u32, heartbeat: i64) -> Lease {
        Lease {
            pid,
            session: session.into(),
            repo: "jdx/jouet".into(),
            pr: 12,
            branch: None,
            started: heartbeat,
            since: heartbeat,
            heartbeat,
        }
    }

    #[test]
    fn a_lease_is_live_while_its_process_runs_and_its_heartbeat_is_fresh() {
        let me = std::process::id();
        assert!(lease("s", me, 1_000).live(1_000 + STALE - 1));
        assert!(!lease("s", me, 1_000).live(1_000 + STALE));
        // No such process.
        assert!(!lease("s", u32::MAX - 1, 1_000).live(1_000));
        let l = lease("s", me, 0);
        assert!(l.covers("JDX/jouet", 12));
        assert!(!l.covers("jdx/jouet", 13));
        assert!(!l.covers("jdx/other", 12));
        assert!(!Lease { pr: 0, ..l }.covers("jdx/jouet", 0));
    }

    #[test]
    fn only_one_watcher_of_a_session_claims_a_pr() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        // A live watcher: this process.
        let first = lease("s", me, 1_000);
        let path = first.claim(dir.path(), 1_000).unwrap().unwrap();
        assert_eq!(Lease::read(&path), Some(first.clone()));
        // Claiming again is the same watcher, not a second one.
        assert!(first.claim(dir.path(), 1_000).unwrap().is_some());
        let second = lease("s", me + 1, 1_000);
        assert_eq!(second.claim(dir.path(), 1_000).unwrap(), None);
        assert_eq!(Lease::of_session(dir.path(), "s").len(), 1);
        // Another PR, another session, or once the first has gone stale.
        let other_pr = Lease {
            pr: 13,
            ..second.clone()
        };
        assert!(other_pr.claim(dir.path(), 1_000).unwrap().is_some());
        assert!(
            lease("t", me + 1, 1_000)
                .claim(dir.path(), 1_000)
                .unwrap()
                .is_some()
        );
        assert!(second.claim(dir.path(), 1_000 + STALE).unwrap().is_some());
        assert!(lease("../", 1, 0).claim(dir.path(), 0).is_err());
    }

    #[test]
    fn leases_and_held_events_round_trip_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let a = Lease::path(dir.path(), "sess-1", 10).unwrap();
        let b = Lease::path(dir.path(), "sess-1", 11).unwrap();
        let other = Lease::path(dir.path(), "sess-12", 10).unwrap();
        lease("sess-1", 10, 5).write(&a).unwrap();
        lease("sess-1", 11, 5).write(&b).unwrap();
        lease("sess-12", 10, 5).write(&other).unwrap();
        let found = Lease::of_session(dir.path(), "sess-1");
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].1.pid, found[1].1.pid), (10, 11));
        assert!(Lease::of_session(dir.path(), "../x").is_empty());
        let one = Held {
            at: 7,
            prs: vec!["jdx/jouet#12".into()],
            kinds: vec!["ci-failure".into()],
            comments: 0,
            checks: vec!["trusted / test".into()],
            comment_ids: Vec::new(),
        };
        hold(&a, &one).unwrap();
        hold(
            &a,
            &Held {
                at: 8,
                ..one.clone()
            },
        )
        .unwrap();
        assert_eq!(held(&a).iter().map(|h| h.at).collect::<Vec<_>>(), [7, 8]);
        assert!(held(&b).is_empty());
        Lease::remove(&a);
        assert!(!a.exists() && !Lease::held_path(&a).exists());
        assert_eq!(Lease::of_session(dir.path(), "sess-1").len(), 1);
    }
}
