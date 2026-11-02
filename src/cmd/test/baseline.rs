//! `jouet test --baseline`: a failing run run again at the merge base, so a
//! failure that was there before the change, from the environment or an
//! earlier commit, does not count against it.

use super::{Job, execute, log_path};
use crate::testlog::{self, Format};
use crate::{affected, known};
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
    /// The failures the merge base's run may not have reached: it ran far
    /// fewer tests, so a test missing from its failures may not have run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) unknown: Vec<String>,
}

impl Baseline {
    /// Whether every failure was there at the merge base too.
    pub(super) fn none_new(&self) -> bool {
        self.new.is_empty() && self.unknown.is_empty() && !self.pre_existing.is_empty()
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
            let total = self.new.len() + self.pre_existing.len() + self.unknown.len();
            let mut text = if self.new.is_empty() {
                let which = match (self.unknown.len(), total) {
                    (1, 1) => "the failure".to_string(),
                    (n, total) if n == total => format!("all {n} failures"),
                    (n, total) => format!("{n} of {total} failures"),
                };
                format!(
                    "baseline: the run at {at} ran far fewer tests, so {which} could not be compared: {}\n",
                    list(&self.unknown)
                )
            } else {
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
                if !self.unknown.is_empty() {
                    text.push_str(&format!(
                        "  not reached at {at}, which ran far fewer tests: {}\n",
                        list(&self.unknown)
                    ));
                }
                text
            };
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
/// failures cannot be compared. A path into `checkout` in a name (rustfmt's
/// `rustfmt /src/app/a.rs:3`, pytest's) is written from its root, so the same
/// failure in the merge base's worktree has the same name.
pub(super) fn failure_names(
    output: &str,
    formats: &[Format],
    checkout: &Path,
) -> Option<BTreeSet<String>> {
    // Not as a failed run, which names an output it recognises nothing in by
    // a stand-in that would match the merge base's.
    let digest = testlog::digest_with(output, false, 0, formats);
    let names: BTreeSet<String> = digest
        .failures
        .into_iter()
        .map(|f| f.name)
        .chain(digest.more_failures)
        .map(|name| from_root(&name, checkout))
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The merge base of HEAD with `since` (the upstream or default branch when
/// None), checked before the run so a comparison that cannot tell anything
/// costs nothing: when the work tree is that commit exactly, as after
/// committing on the default branch itself, the run there would be the
/// same code, and every failure would look as if it were there before.
pub(super) fn base(root: &Path, since: Option<&str>) -> Result<affected::Base, affected::Error> {
    let base = affected::base(root, since)?;
    if affected::unchanged_since(root, &base.merge_base)? {
        return Err(affected::Error::new(
            Exit::Usage,
            format!(
                "--baseline compares with the merge base of HEAD and `{}`, which is HEAD itself with nothing changed since, so it would run this same code: name the commit your change started from with --since",
                base.name
            ),
        ));
    }
    Ok(base)
}

/// Whether [`compare`] may run a failed job at the merge base when nothing
/// is kept for it ([`known`]), retry an inconclusive run, or only use what is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Run,
    /// An explicit retry may replace an inconclusive cached run.
    Retry,
    Kept,
}

/// Why the failures could not be compared. `unverifiable` when the merge
/// base could not run the command either, so nothing ever will be here:
/// the stop gate then asks about the failures once, not at every turn.
#[derive(Debug)]
pub(super) struct NotCompared {
    pub(super) message: String,
    pub(super) unverifiable: bool,
}

impl From<String> for NotCompared {
    fn from(message: String) -> Self {
        Self {
            message,
            unverifiable: false,
        }
    }
}

/// A command as [`known`] keeps it: its words and directory, with paths
/// into the checkout written from its root, so every worktree of the
/// repository shares it.
fn command_key(job: &Job, root: &Path) -> String {
    let dot = Path::new(".");
    let words: Vec<String> = job.argv.iter().map(|w| rebase(w, root, dot)).collect();
    let dir = job.dir.strip_prefix(root).unwrap_or(Path::new(""));
    format!("{} @{}", words.join(" "), dir.display())
}

/// A worktree of the merge base, checked out when a job first needs it.
struct Checkout<'a> {
    root: &'a Path,
    base: &'a affected::Base,
    tmp: Option<(tempfile::TempDir, PathBuf)>,
}

impl Checkout<'_> {
    fn tree(&mut self) -> Result<&Path, String> {
        if self.tmp.is_none() {
            let tmp = tempfile::Builder::new()
                .prefix("jouet-baseline-")
                .tempdir()
                .map_err(|e| format!("making a directory for the merge base: {e}"))?;
            let tree = tmp.path().join("tree");
            let tree_arg = tree.to_string_lossy().into_owned();
            git(
                self.root,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    "--quiet",
                    &tree_arg,
                    &self.base.merge_base,
                ],
            )
            .map_err(|e| format!("checking out the merge base {}: {e}", self.base.merge_base))?;
            if tree.join(".gitmodules").is_file() {
                let _ = git(&tree, &["submodule", "update", "--init", "--quiet"]);
            }
            self.tmp = Some((tmp, tree));
        }
        Ok(&self.tmp.as_ref().expect("checked out above").1)
    }
}

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        if let Some((_, tree)) = &self.tmp {
            let tree = tree.to_string_lossy().into_owned();
            let _ = git(self.root, &["worktree", "remove", "--force", &tree]);
        }
    }
}

/// Sorts each failed job's failures into those the merge base has too and
/// the new ones, from what [`known`] keeps for the job at `base`, or by
/// running it again in a worktree of `base` and keeping what came of it.
/// [`Mode::Retry`] also retries an inconclusive cached run; [`Mode::Kept`]
/// returns None when a job has nothing kept.
pub(super) fn compare(
    root: &Path,
    base: affected::Base,
    formats: &[Format],
    failed: &[(&Job, BTreeSet<String>, u64)],
    mode: Mode,
) -> Result<Option<Baseline>, NotCompared> {
    let mut checkout = Checkout {
        root,
        base: &base,
        tmp: None,
    };
    let mut pre_existing = BTreeSet::new();
    let mut new = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for (i, (job, now, ran)) in failed.iter().enumerate() {
        let key = command_key(job, root);
        let kept = known::lookup(root, &base.merge_base, &key);
        let outcome = match (kept, mode) {
            (Some(known::Outcome::Unnamed { .. }), Mode::Retry)
            | (None, Mode::Run | Mode::Retry) => {
                let tree = checkout.tree()?.to_path_buf();
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
                let output = String::from_utf8_lossy(&output);
                let outcome = if code == 0 {
                    known::Outcome::Passed
                } else {
                    match failure_names(&output, formats, &tree) {
                        Some(names) => {
                            let d = testlog::digest_with(&output, false, 0, formats);
                            known::Outcome::Failed {
                                names: names.into_iter().collect(),
                                ran: d.passed + d.failed + d.skipped,
                            }
                        }
                        None => known::Outcome::Unnamed {
                            exit: code,
                            log: Some(log.display().to_string()),
                        },
                    }
                };
                known::keep(root, &base.merge_base, &key, outcome.clone());
                outcome
            }
            (Some(outcome), _) => outcome,
            (None, Mode::Kept) => return Ok(None),
        };
        let (at_base, reached): (BTreeSet<String>, u64) = match outcome {
            known::Outcome::Passed => (BTreeSet::new(), u64::MAX),
            known::Outcome::Failed { names, ran } => (names.into_iter().collect(), ran),
            known::Outcome::Unnamed { exit, log } => {
                let log = log.map_or(String::new(), |l| format!(" (full log: {l})"));
                return Err(NotCompared {
                    message: format!(
                        "the run at {} failed (exit {exit}) without naming a failing test, so the failures could not be compared{log}",
                        base.name
                    ),
                    unverifiable: true,
                });
            }
        };
        // Far fewer tests at the merge base: one that is not among its
        // failures may never have run there.
        let short = reached.saturating_mul(2) < *ran;
        for name in now {
            if at_base.contains(name) {
                pre_existing.insert(name.clone());
            } else if short {
                unknown.insert(name.clone());
            } else {
                new.insert(name.clone());
            }
        }
    }
    drop(checkout);
    Ok(Some(Baseline {
        base,
        pre_existing: pre_existing.into_iter().collect(),
        new: new.into_iter().collect(),
        unknown: unknown.into_iter().collect(),
    }))
}

/// `name` with each path into `checkout` written from its root: a worktree
/// under `/tmp` can also be named by its canonical path (`/private/tmp` on
/// macOS).
fn from_root(name: &str, checkout: &Path) -> String {
    let mut name = name.to_string();
    let canonical = checkout.canonicalize().ok();
    for dir in std::iter::once(checkout).chain(canonical.as_deref()) {
        let prefix = format!("{}/", dir.display());
        name = name.replace(&prefix, "");
    }
    name
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
    fn a_command_is_kept_the_same_from_every_worktree() {
        let job = |root: &str| Job {
            argv: vec![
                "cargo".into(),
                "test".into(),
                format!("--manifest-path={root}/crates/a/Cargo.toml"),
            ],
            env: Vec::new(),
            dir: Path::new(root).join("crates/a"),
            exit: Box::new(|_| Exit::Ok),
        };
        let here = command_key(&job("/src/app"), Path::new("/src/app"));
        assert_eq!(
            here,
            "cargo test --manifest-path=./crates/a/Cargo.toml @crates/a"
        );
        assert_eq!(command_key(&job("/wt/app-2"), Path::new("/wt/app-2")), here);
    }

    #[test]
    fn an_unrecognised_failure_names_nothing() {
        assert_eq!(
            failure_names("something broke\n", &[], Path::new("/x")),
            None
        );
    }

    #[test]
    fn names_are_written_from_the_checkout_root() {
        let names = |out: &str, root: &str| failure_names(out, &[], Path::new(root));
        let here = names("Diff in /src/app/src/a.rs:3:\n-x\n+y\n", "/src/app");
        let there = names("Diff in /tmp/b/tree/src/a.rs:3:\n-x\n+y\n", "/tmp/b/tree");
        assert_eq!(here, there);
        assert_eq!(
            here.unwrap().into_iter().next().unwrap(),
            "rustfmt src/a.rs:3"
        );
    }
}
