//! Git worktrees: a new worktree's graph starts as a copy of a sibling's,
//! and graphs whose worktree is gone are deleted.
//!
//! Each worktree has its own database, keyed by a hash of its root. A new
//! worktree of a repository that already has a graph somewhere would
//! otherwise index everything again; instead it copies the graph of the
//! sibling nearest its HEAD (`seed`). Paths in the graph are relative and
//! files are tracked by content hash, so the first refresh after the copy
//! only hashes files whose timestamps differ and re-extracts the ones whose
//! content does.
//!
//! Short-lived agent worktrees would leave their databases behind; `prune`
//! deletes the databases whose recorded root no longer exists.

use crate::{Error, store};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// How often [`maybe_prune`] looks for graphs whose worktree is gone.
const PRUNE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The file whose modification time says when the graphs were last pruned.
const PRUNE_STAMP: &str = ".pruned";

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A worktree of the same repository, and the commit it has checked out.
struct Sibling {
    root: PathBuf,
    head: Option<String>,
}

/// The other worktrees of `root`'s repository, from `git worktree list`.
fn siblings(root: &Path) -> Vec<Sibling> {
    let Some(list) = git(root, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let here = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut out = Vec::new();
    for block in list.split("\n\n") {
        let mut path = None;
        let mut head = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                head = Some(h.to_string());
            }
        }
        let Some(path) = path else { continue };
        let path = path.canonicalize().unwrap_or(path);
        if path != here {
            out.push(Sibling { root: path, head });
        }
    }
    out
}

/// How far `theirs` is from `ours`: 0 for the same commit, else the number
/// of commits on either side of their merge base, `u64::MAX` when git
/// cannot say.
fn distance(root: &Path, ours: &str, theirs: &str) -> u64 {
    if ours == theirs {
        return 0;
    }
    git(
        root,
        &["rev-list", "--count", &format!("{ours}...{theirs}")],
    )
    .and_then(|n| n.parse().ok())
    .unwrap_or(u64::MAX)
}

fn modified(path: &Path) -> SystemTime {
    ["", "-wal"]
        .iter()
        .filter_map(|suffix| {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            std::fs::metadata(p).and_then(|m| m.modified()).ok()
        })
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The sibling's graph to copy into a new worktree's: the one whose HEAD
/// is nearest ours (the same commit first), then the most recently
/// written.
fn pick_source(root: &Path, db_for: impl Fn(&Path) -> Option<PathBuf>) -> Option<PathBuf> {
    let ours = git(root, &["rev-parse", "HEAD"]);
    let mut candidates: Vec<(u64, std::cmp::Reverse<SystemTime>, PathBuf)> = siblings(root)
        .into_iter()
        .filter_map(|s| {
            let db = db_for(&s.root)?;
            if !db.is_file() {
                return None;
            }
            let d = match (&ours, &s.head) {
                (Some(o), Some(t)) => distance(root, o, t),
                _ => u64::MAX,
            };
            Some((d, std::cmp::Reverse(modified(&db)), db))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next().map(|(_, _, db)| db)
}

/// Fills `db`, which does not exist yet, with a copy of a sibling
/// worktree's graph, re-rooted at `root`. `false` when there is no sibling
/// graph to copy, or it is from another schema version: the caller builds
/// from scratch as before.
///
/// The copy is `VACUUM INTO` from a read-only connection: one read
/// transaction, so a sibling writing at the same moment cannot tear it,
/// and it is renamed into place only once complete.
pub(crate) fn seed(root: &Path, db: &Path, db_for: impl Fn(&Path) -> Option<PathBuf>) -> bool {
    let Some(source) = pick_source(root, db_for) else {
        return false;
    };
    copy(&source, root, db).is_ok()
}

fn copy(source: &Path, root: &Path, db: &Path) -> Result<(), Error> {
    let from = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    from.busy_timeout(Duration::from_secs(5))?;
    if store::get_meta(&from, "schema_version")?.as_deref() != Some(store::SCHEMA_VERSION) {
        return Err(Error(
            "the sibling's graph is from another schema version".into(),
        ));
    }
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut partial = db.as_os_str().to_owned();
    partial.push(format!(".seed-{}", std::process::id()));
    let partial = PathBuf::from(partial);
    let _ = std::fs::remove_file(&partial);
    let result = (|| -> Result<(), Error> {
        from.execute("VACUUM INTO ?1", [partial.to_string_lossy()])?;
        drop(from);
        let to = Connection::open(&partial)?;
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        store::set_meta(&to, "root", &root.to_string_lossy())?;
        drop(to);
        // Another process may have built or seeded this graph meanwhile;
        // theirs stands.
        if db.exists() {
            return Err(Error("the graph appeared while it was being seeded".into()));
        }
        std::fs::rename(&partial, db)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// What pruning did.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Pruned {
    /// The worktree roots whose graphs were deleted.
    pub removed: Vec<String>,
    /// Graphs kept: their worktree exists, or it could not be told.
    pub kept: usize,
    /// Bytes freed.
    pub bytes: u64,
}

/// The work tree a graph was built for, from its `meta` table; `None` when
/// the file cannot be read as a graph.
fn recorded_root(db: &Path) -> Option<PathBuf> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    store::get_meta(&conn, "root")
        .ok()
        .flatten()
        .map(PathBuf::from)
}

/// Deletes every graph in `dir` whose recorded worktree root no longer
/// exists. A root that exists, or that cannot be checked (a permission
/// error, an unmounted volume that reports one), keeps its graph, and so
/// does a file that cannot be read as a graph.
pub(crate) fn prune(dir: &Path) -> Result<Pruned, Error> {
    let mut pruned = Pruned::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(pruned);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "db") {
            continue;
        }
        let gone = recorded_root(&path).is_some_and(|root| matches!(root.try_exists(), Ok(false)));
        if !gone {
            pruned.kept += 1;
            continue;
        }
        let root = recorded_root(&path).unwrap_or_default();
        let bytes: u64 = ["", "-wal", "-shm"]
            .iter()
            .filter_map(|s| {
                let mut p = path.as_os_str().to_owned();
                p.push(s);
                std::fs::metadata(p).ok().map(|m| m.len())
            })
            .sum();
        if store::remove_db(&path).is_ok() {
            pruned.removed.push(root.to_string_lossy().into_owned());
            pruned.bytes += bytes;
        }
    }
    Ok(pruned)
}

/// [`prune`], at most once a day: when the stamp in `dir` is older than
/// [`PRUNE_EVERY`], or missing. Best effort.
pub(crate) fn maybe_prune(dir: &Path) {
    let stamp = dir.join(PRUNE_STAMP);
    let due = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|age| age >= PRUNE_EVERY);
    if !due {
        return;
    }
    // Stamp first, so parallel sessions don't all prune at once.
    if std::fs::write(&stamp, b"").is_err() {
        return;
    }
    let _ = prune(dir);
}
