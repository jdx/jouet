//! `jouet test --baseline`: a failing run run again at the merge base, so a
//! failure that was there before the change, from the environment or an
//! earlier commit, does not count against it.

use super::{Job, execute, log_path};
use crate::affected;
use crate::testlog::{self, Format};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What a failing run's failures were at the merge base.
#[derive(Debug, Serialize)]
pub(super) struct Baseline {
    /// The ref the merge base was taken with, and the commit.
    pub(super) base: affected::Base,
    /// The failures that also fail at the merge base.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) pre_existing: Vec<String>,
    /// The failures the merge base does not have.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) new: Vec<String>,
}

impl Baseline {
    /// Whether every failure was there at the merge base too.
    pub(super) fn none_new(&self) -> bool {
        self.new.is_empty() && !self.pre_existing.is_empty()
    }

    /// What it found, in a line or two.
    pub(super) fn text(&self) -> String {
        let at = format!(
            "{} ({})",
            self.base.name,
            &self.base.merge_base[..self.base.merge_base.len().min(12)]
        );
        let list = |names: &[String]| {
            let shown: Vec<&str> = names.iter().take(10).map(String::as_str).collect();
            let more = names.len().saturating_sub(shown.len());
            let more = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            format!("{}{more}", shown.join(", "))
        };
        if self.none_new() {
            let all = match self.pre_existing.len() {
                1 => "the failure also fails".to_string(),
                n => format!("all {n} failures also fail"),
            };
            format!(
                "baseline: {all} at {at}, so none is from this change; counted as passing: {}\n",
                list(&self.pre_existing)
            )
        } else {
            let total = self.new.len() + self.pre_existing.len();
            let new = match (self.new.len(), total) {
                (1, 1) => "the failure is new".to_string(),
                (1, total) => format!("1 of {total} failures is new"),
                (n, total) if n == total => format!("all {n} failures are new"),
                (n, total) => format!("{n} of {total} failures are new"),
            };
            let mut text = format!(
                "baseline: {new}, not failing at {at}: {}\n",
                list(&self.new)
            );
            if !self.pre_existing.is_empty() {
                text.push_str(&format!(
                    "  also failing at {at}: {}\n",
                    list(&self.pre_existing)
                ));
            }
            text
        }
    }
}

/// Every failure a run's output names, or None when it names none, so the
/// failures cannot be compared.
pub(super) fn failure_names(output: &str, formats: &[Format]) -> Option<BTreeSet<String>> {
    // Not as a failed run, which names an output it recognises nothing in by
    // a stand-in that would match the merge base's.
    let digest = testlog::digest_with(output, false, 0, formats);
    let names: BTreeSet<String> = digest
        .failures
        .into_iter()
        .map(|f| f.name)
        .chain(digest.more_failures)
        .collect();
    (!names.is_empty()).then_some(names)
}

/// Runs each failed job again in a worktree of the merge base of HEAD with
/// `since` (the upstream or default branch when None), and sorts the
/// failures it had into those the merge base has too and the new ones.
pub(super) fn compare(
    root: &Path,
    since: Option<&str>,
    formats: &[Format],
    failed: &[(&Job, BTreeSet<String>)],
) -> Result<Baseline, String> {
    let base = affected::base(root, since).map_err(|e| e.message)?;
    let tmp = tempfile::Builder::new()
        .prefix("jouet-baseline-")
        .tempdir()
        .map_err(|e| format!("making a directory for the merge base: {e}"))?;
    let tree = tmp.path().join("tree");
    let tree_arg = tree.to_string_lossy().into_owned();
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            "--quiet",
            &tree_arg,
            &base.merge_base,
        ],
    )
    .map_err(|e| format!("checking out the merge base {}: {e}", base.merge_base))?;
    let _remove = Worktree { root, tree: &tree };
    if tree.join(".gitmodules").is_file() {
        let _ = git(&tree, &["submodule", "update", "--init", "--quiet"]);
    }
    let mut pre_existing = BTreeSet::new();
    let mut new = BTreeSet::new();
    for (i, (job, now)) in failed.iter().enumerate() {
        let dir = job.dir.strip_prefix(root).unwrap_or(Path::new(""));
        let at_base = Job {
            argv: job.argv.iter().map(|w| rebase(w, root, &tree)).collect(),
            env: job.env.clone(),
            dir: tree.join(dir),
            exit: Box::new(|_| Exit::Ok),
        };
        // After this run's own logs, so pruning never takes one of them.
        let log = log_path(failed.len() + i + 1);
        let (code, _) = execute(&at_base, &log, false)?;
        let output = std::fs::read(&log).unwrap_or_default();
        let at_base: BTreeSet<String> = if code == 0 {
            BTreeSet::new()
        } else {
            failure_names(&String::from_utf8_lossy(&output), formats).unwrap_or_default()
        };
        for name in now {
            if at_base.contains(name) {
                pre_existing.insert(name.clone());
            } else {
                new.insert(name.clone());
            }
        }
    }
    Ok(Baseline {
        base,
        pre_existing: pre_existing.into_iter().collect(),
        new: new.into_iter().collect(),
    })
}

/// `word`, a command word, with a path into `root` (alone, or an option's
/// value after `=`) pointed at the same path in `tree`, so the run at the
/// merge base does not read the checkout.
fn rebase(word: &str, root: &Path, tree: &Path) -> String {
    let root = root.to_string_lossy();
    let tree = tree.to_string_lossy();
    let at = |path: &str| -> Option<String> {
        let rest = path.strip_prefix(root.as_ref())?;
        (rest.is_empty() || rest.starts_with(['/', '\\'])).then(|| format!("{tree}{rest}"))
    };
    if let Some(path) = at(word) {
        return path;
    }
    match word.split_once('=') {
        Some((option, value)) => {
            at(value).map_or_else(|| word.to_string(), |v| format!("{option}={v}"))
        }
        None => word.to_string(),
    }
}

/// A worktree of the merge base, removed when dropped.
struct Worktree<'a> {
    root: &'a Path,
    tree: &'a PathBuf,
}

impl Drop for Worktree<'_> {
    fn drop(&mut self) {
        let tree = self.tree.to_string_lossy().into_owned();
        let _ = git(self.root, &["worktree", "remove", "--force", &tree]);
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebases_paths_into_the_checkout() {
        let (root, tree) = (Path::new("/src/app"), Path::new("/tmp/base/tree"));
        for (word, want) in [
            ("/src/app", "/tmp/base/tree"),
            ("/src/app/Cargo.toml", "/tmp/base/tree/Cargo.toml"),
            (
                "--manifest-path=/src/app/Cargo.toml",
                "--manifest-path=/tmp/base/tree/Cargo.toml",
            ),
            ("/src/apple/x", "/src/apple/x"),
            ("cargo", "cargo"),
            ("--jobs=4", "--jobs=4"),
        ] {
            assert_eq!(rebase(word, root, tree), want, "{word}");
        }
    }

    #[test]
    fn an_unrecognised_failure_names_nothing() {
        assert_eq!(failure_names("something broke\n", &[]), None);
    }
}
