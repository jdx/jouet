//! What a command did on the files as they are now, kept so the same command
//! on the same files is answered without running it again.
//!
//! An agent, the stop gate and `--baseline` all ask for a check that already
//! ran on the files as they are: `jouet test --baseline --cmd -- CMD` after a
//! slow failing CMD runs CMD again before it can compare. A run is kept by
//! the contents of the working tree (tracked and untracked files, less the
//! ignored ones), HEAD, the command, its directory and its environment, and
//! only when the run left the tree as it found it, so a formatter, a code
//! generator or a snapshot update is never answered from what is kept.
//! Entries older than `[test] reuse_s` are not used; `--fresh` runs anyway.

use super::Job;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A kept run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Kept {
    /// Seconds since the epoch when it ran.
    pub(super) at: u64,
    pub(super) code: i32,
    pub(super) ms: u64,
    /// Its whole output, which test logs keep for a week.
    pub(super) log: PathBuf,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args).stdin(Stdio::null());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let out = command.stderr(Stdio::null()).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The working tree of the repository at `root` as one hash, with HEAD:
/// what `git add -A && git write-tree` would give, written to a copy of the
/// index so the real one is left alone. None outside a repository.
pub(super) fn state(root: &Path) -> Option<String> {
    let head = git(root, &["rev-parse", "--verify", "-q", "HEAD"], None).unwrap_or_default();
    let index = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        None,
    )?;
    let scratch = tempfile::tempdir().ok()?;
    let copy = scratch.path().join("index");
    // A repository without an index yet starts from none, which git creates.
    if Path::new(&index).exists() {
        std::fs::copy(&index, &copy).ok()?;
    }
    git(root, &["add", "-A", "--", "."], Some(&copy))?;
    let tree = git(root, &["write-tree"], Some(&copy))?;
    Some(format!("{tree} {head}"))
}

/// The key for `job` on the tree `state`.
pub(super) fn key(
    state: &str,
    job: &Job,
    env: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> String {
    let mut hash = Sha256::new();
    let mut part = |bytes: &[u8]| {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    };
    part(b"jouet test result v3");
    part(state.as_bytes());
    part(job.dir.as_os_str().as_encoded_bytes());
    part(&(job.argv.len() as u64).to_le_bytes());
    for word in &job.argv {
        part(word.as_bytes());
    }
    let mut vars: Vec<_> = env
        .chain(
            job.env
                .iter()
                .map(|(key, value)| (key.into(), value.into())),
        )
        .collect();
    vars.sort();
    for (k, v) in &vars {
        part(k.as_encoded_bytes());
        part(v.as_encoded_bytes());
    }
    hash.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn dir() -> Option<PathBuf> {
    Some(jouet_output::paths::state_dir()?.join("results"))
}

/// The run kept under `key`, when it is younger than `max_age` seconds and
/// its log is still there.
pub(super) fn lookup(key: &str, max_age: u64) -> Option<Kept> {
    lookup_in(&dir()?, key, max_age, now())
}

fn lookup_in(dir: &Path, key: &str, max_age: u64, now: u64) -> Option<Kept> {
    let bytes = std::fs::read(dir.join(format!("{key}.json"))).ok()?;
    let kept: Kept = serde_json::from_slice(&bytes).ok()?;
    (now.saturating_sub(kept.at) <= max_age && kept.log.is_file()).then_some(kept)
}

/// Keeps a run under `key`, and drops what is older than `max_age`. Best
/// effort: a run that cannot be kept only runs again next time.
pub(super) fn keep(key: &str, kept: &Kept, max_age: u64) {
    if let Some(dir) = dir() {
        keep_in(&dir, key, kept, max_age, now());
    }
}

fn keep_in(dir: &Path, key: &str, kept: &Kept, max_age: u64, now: u64) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > max_age);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let Ok(mut file) = tempfile::NamedTempFile::new_in(dir) else {
        return;
    };
    let kept = Kept {
        at: now,
        ..kept.clone()
    };
    if serde_json::to_writer(&mut file, &kept).is_ok() {
        let _ = file.persist(dir.join(format!("{key}.json")));
    }
}

/// How long ago `at` was, for the line that says a run was reused.
pub(super) fn ago(at: u64) -> String {
    let s = now().saturating_sub(at);
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m", s / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(argv: &[&str]) -> Job {
        Job {
            argv: argv.iter().map(|w| (*w).to_string()).collect(),
            env: Vec::new(),
            dir: PathBuf::from("/repo"),
            exit: Box::new(|_| jouet_output::exit::Exit::Ok),
        }
    }

    fn env(
        vars: &[(&str, &str)],
    ) -> impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)> {
        vars.iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_key_holds_the_tree_command_and_environment() {
        let base = key(
            "t h",
            &job(&["cargo", "test"]),
            env(&[("A", "1"), ("PWD", "/x")]),
        );
        assert_ne!(
            base,
            key(
                "t h",
                &job(&["cargo", "test"]),
                env(&[("PWD", "/y"), ("A", "1")])
            )
        );
        assert_ne!(
            base,
            key("u h", &job(&["cargo", "test"]), env(&[("A", "1")]))
        );
        assert_ne!(
            base,
            key("t h", &job(&["cargo", "test", "x"]), env(&[("A", "1")]))
        );
        assert_ne!(
            base,
            key("t h", &job(&["cargo", "test"]), env(&[("A", "2")]))
        );
        // Words are kept apart: `a b` is not `ab`.
        assert_ne!(
            key("t", &job(&["a", "b"]), env(&[])),
            key("t", &job(&["ab"]), env(&[]))
        );
    }

    #[test]
    fn the_key_separates_arguments_from_environment_entries() {
        assert_ne!(
            key("t", &job(&["run", "A", "1"]), env(&[])),
            key("t", &job(&["run"]), env(&[("A", "1")])),
        );
    }

    #[test]
    fn a_kept_run_is_used_until_it_is_too_old_or_its_log_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("run.log");
        std::fs::write(&log, "ok\n").unwrap();
        let kept = Kept {
            at: 0,
            code: 1,
            ms: 1500,
            log: log.clone(),
        };
        keep_in(dir.path(), "k", &kept, 3600, 1000);
        let found = lookup_in(dir.path(), "k", 3600, 1000 + 3600).unwrap();
        assert_eq!((found.at, found.code, found.ms), (1000, 1, 1500));
        assert_eq!(lookup_in(dir.path(), "k", 3600, 1000 + 3601), None);
        assert_eq!(lookup_in(dir.path(), "other", 3600, 1000), None);
        std::fs::remove_file(&log).unwrap();
        assert_eq!(lookup_in(dir.path(), "k", 3600, 1000), None);
    }

    #[test]
    fn the_state_changes_with_any_file_but_an_ignored_one_and_leaves_the_index_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "out/\n").unwrap();
        std::fs::write(root.join("a.txt"), "1").unwrap();
        let first = state(root).unwrap();
        assert_eq!(state(root).unwrap(), first);
        std::fs::create_dir(root.join("out")).unwrap();
        std::fs::write(root.join("out/b"), "x").unwrap();
        assert_eq!(state(root).unwrap(), first);
        std::fs::write(root.join("new.txt"), "2").unwrap();
        let second = state(root).unwrap();
        assert_ne!(second, first);
        std::fs::write(root.join("a.txt"), "changed").unwrap();
        assert_ne!(state(root).unwrap(), second);
        // Nothing was staged in the real index.
        let staged = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(staged.stdout.is_empty());
    }
}
