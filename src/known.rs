//! What a command did at a merge base on this machine, kept so a failure
//! that was already there is known without running the merge base again.
//!
//! `jouet test --baseline`, and a failing full suite on its own, run the
//! failed command at the merge base and keep what came of it here, by
//! repository (its git common directory, so every worktree shares it), merge
//! base and command. A later failure of the same command at the same merge
//! base is then compared with what is kept: the failures it names that also
//! failed there are known, and pass. A merge base that could not run the
//! command, failing before it named any test, is kept too, so the failures
//! are known to be unverifiable here, and the stop gate raises them once.
//! Entries older than [`MAX_AGE`] are dropped.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How long what a merge base did is kept.
const MAX_AGE: u64 = 14 * 24 * 60 * 60;

/// What a command did at a merge base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// It passed: every failure now is new.
    Passed,
    /// It failed with these failures, after this many tests.
    Failed { names: Vec<String>, ran: u64 },
    /// It failed without naming a failing test, such as when a tool it
    /// needs is missing from that checkout: nothing can be compared.
    Unnamed { exit: i32, log: Option<String> },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    at: u64,
    #[serde(flatten)]
    outcome: Outcome,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The file for the repository at `root`: one per git common directory.
fn path(root: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let common = Path::new(&common)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&common));
    let digest = Sha256::digest(common.to_string_lossy().as_bytes());
    let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some(
        jouet_output::paths::state_dir()?
            .join("known")
            .join(format!("{name}.json")),
    )
}

fn key(merge_base: &str, command: &str) -> String {
    format!("{merge_base} {command}")
}

fn load(path: &Path) -> Store {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// What `command` did at `merge_base` in the repository at `root`, when it
/// is kept and not too old.
pub(crate) fn lookup(root: &Path, merge_base: &str, command: &str) -> Option<Outcome> {
    let store = load(&path(root)?);
    let entry = store.entries.get(&key(merge_base, command))?;
    (now().saturating_sub(entry.at) <= MAX_AGE).then(|| entry.outcome.clone())
}

/// Keeps what `command` did at `merge_base`. Best effort: a store that
/// cannot be written only means the merge base runs again next time.
pub(crate) fn keep(root: &Path, merge_base: &str, command: &str, outcome: Outcome) {
    let Some(path) = path(root) else {
        return;
    };
    let _ = update(&path, |store| {
        let at = now();
        store
            .entries
            .retain(|_, e| at.saturating_sub(e.at) <= MAX_AGE);
        store
            .entries
            .insert(key(merge_base, command), Entry { at, outcome });
    });
}

/// Loads, changes and saves the store under an exclusive lock.
fn update(path: &Path, change: impl FnOnce(&mut Store)) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut store = load(path);
    change(&mut store);
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, &store)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
