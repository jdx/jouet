//! `jouet stack sync`: bring each layer up to date by merging its parent,
//! bottom first, verifying each merge by ancestry and content before it is
//! committed. Never a rebase, and never a force push.
//!
//! The lessons this encodes:
//! - A layer starts from its local branch, never `origin/…`, so fixes not
//!   yet pushed are kept; a local branch behind its remote is refused.
//! - After a prerequisite merged (often squashed), the trunk is merged in
//!   rather than rebased onto, which would conflict with the old commits.
//! - A merge whose tree lacks some of the parent's changes is not
//!   committed: sync stops there and says what to do.
//! - A branch checked out in another worktree belongs to whoever works
//!   there, so sync stops rather than change it under them.

use super::{Stack, missing_changes, trunk_tip};
use crate::{Error, Git};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Show the plan; merge, fetch and push nothing.
    pub dry_run: bool,
    /// Push the layers that are ahead of their remote branch, afterwards.
    pub push: bool,
    /// Fetch the remote first.
    pub fetch: bool,
}

/// What happened to one layer, or would in a dry run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub branch: String,
    /// `up_to_date`, `merged` (or `would_merge`), `skipped_merged`.
    pub action: &'static str,
    /// What was merged in: the parent branch, or `origin/main`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The merge commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The local branch was created from the remote's first.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub created: bool,
}

/// Where sync stopped, and what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stopped {
    pub branch: String,
    /// `conflict`, `content`, `behind_remote`, `checked_out_elsewhere`,
    /// `dirty`, `missing_branch` or `error`.
    pub reason: &'static str,
    pub message: String,
    /// What to do, as commands where it can.
    pub next: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(skip)]
    pub exit: Exit,
}

/// A push of one layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Push {
    pub branch: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<Stopped>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pushed: Vec<Push>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

impl Report {
    pub fn exit(&self) -> Exit {
        if let Some(stopped) = &self.stopped {
            return stopped.exit;
        }
        if self.pushed.iter().any(|p| !p.ok) {
            return Exit::Remote;
        }
        Exit::Ok
    }
}

/// Syncs the stack, bottom first, stopping at the first layer that cannot
/// be brought up to date.
pub fn sync(git: &Git, stack: &Stack, opts: Options) -> Report {
    let mut report = Report {
        dry_run: opts.dry_run,
        ..Report::default()
    };
    if opts.fetch
        && !opts.dry_run
        && let Err(e) = git.text(&["fetch", "--quiet", "--prune", &stack.remote])
    {
        report.stopped = Some(Stopped {
            branch: stack.trunk.clone(),
            reason: "error",
            message: e.message,
            next: format!("check that `git fetch {}` works, then rerun", stack.remote),
            files: Vec::new(),
            exit: Exit::Remote,
        });
        return report;
    }
    let here = git.toplevel().ok().and_then(|p| p.canonicalize().ok());
    let checked_out = worktrees(git);
    // In a dry run, a layer that would be merged changes for the one above.
    let mut below_changes = false;
    for (i, layer) in stack.layers.iter().enumerate() {
        if layer.merged {
            report.steps.push(step(&layer.branch, "skipped_merged"));
            below_changes = false;
            continue;
        }
        match layer_sync(git, stack, i, here.as_deref(), &checked_out, opts) {
            Ok(mut step) => {
                let on_layer_below = !layer.parent_merged && layer.parent != stack.trunk;
                if opts.dry_run && below_changes && on_layer_below && step.action == "up_to_date" {
                    step.action = "would_merge";
                    step.from = Some(layer.parent.clone());
                }
                below_changes = step.action == "would_merge";
                report.steps.push(step);
            }
            Err(stopped) => {
                report.stopped = Some(stopped);
                break;
            }
        }
    }
    if opts.push && !opts.dry_run && report.stopped.is_none() {
        report.pushed = push(git, stack);
    }
    report
}

fn step(branch: &str, action: &'static str) -> Step {
    Step {
        branch: branch.to_string(),
        action,
        from: None,
        commit: None,
        created: false,
    }
}

fn stop(branch: &str, reason: &'static str, message: String, next: String, exit: Exit) -> Stopped {
    Stopped {
        branch: branch.to_string(),
        reason,
        message,
        next,
        files: Vec::new(),
        exit,
    }
}

/// Each branch checked out in a worktree, with the worktree.
fn worktrees(git: &Git) -> Vec<(String, PathBuf)> {
    let out = git
        .text(&["worktree", "list", "--porcelain"])
        .unwrap_or_default();
    let mut found = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let (Some(b), Some(p)) = (line.strip_prefix("branch refs/heads/"), &path) {
            found.push((
                b.to_string(),
                p.canonicalize().unwrap_or_else(|_| p.clone()),
            ));
        }
    }
    found
}

fn layer_sync(
    git: &Git,
    stack: &Stack,
    i: usize,
    here: Option<&Path>,
    checked_out: &[(String, PathBuf)],
    opts: Options,
) -> Result<Step, Stopped> {
    let layer = &stack.layers[i];
    let branch = &layer.branch;
    let local_ref = format!("refs/heads/{branch}");
    let remote_name = format!("{}/{branch}", stack.remote);
    let remote = git.commit_sha(&format!("refs/remotes/{remote_name}"));
    let mut created = false;
    let local = match git.commit_sha(&local_ref) {
        Some(sha) => sha,
        None => {
            let Some(remote) = remote.clone() else {
                return Err(stop(
                    branch,
                    "missing_branch",
                    format!("`{branch}` has no local or remote branch"),
                    "fetch it or remove it from the stack, then rerun".to_string(),
                    Exit::Precondition,
                ));
            };
            // Nothing local to lose: the local branch starts as the remote's.
            if !opts.dry_run {
                git.text(&["branch", "--no-track", branch, &remote])
                    .map_err(|e| error(branch, e))?;
            }
            created = true;
            remote
        }
    };
    if let Some(remote) = &remote {
        let behind = git.count(&format!("{local}..{remote}")).unwrap_or(0);
        if behind > 0 {
            return Err(stop(
                branch,
                "behind_remote",
                format!(
                    "`{branch}` is {behind} commit(s) behind {remote_name}: someone pushed work the local branch does not have"
                ),
                format!(
                    "merge it first (`git merge {remote_name}` where `{branch}` is checked out), then rerun"
                ),
                Exit::Precondition,
            ));
        }
    }
    // What to merge in: the trunk once the layer below has merged, else
    // the layer below as it is locally (synced already in this run).
    let (from, from_sha) = if layer.parent_merged || layer.parent == stack.trunk {
        let Some(sha) = trunk_tip(git, stack) else {
            return Err(stop(
                branch,
                "missing_branch",
                format!("the trunk `{}` was not found", stack.trunk),
                format!("`git fetch {}`, then rerun", stack.remote),
                Exit::Precondition,
            ));
        };
        let name = if git
            .commit_sha(&format!("refs/remotes/{}/{}", stack.remote, stack.trunk))
            .is_some()
        {
            format!("{}/{}", stack.remote, stack.trunk)
        } else {
            stack.trunk.clone()
        };
        (name, sha)
    } else {
        let Some(sha) = git
            .commit_sha(&format!("refs/heads/{}", layer.parent))
            .or_else(|| git.commit_sha(&format!("refs/remotes/{}/{}", stack.remote, layer.parent)))
        else {
            return Err(stop(
                branch,
                "missing_branch",
                format!("its parent `{}` was not found", layer.parent),
                format!("`git fetch {}`, then rerun", stack.remote),
                Exit::Precondition,
            ));
        };
        (layer.parent.clone(), sha)
    };
    let base = git
        .line(&["merge-base", &local, &from_sha])
        .unwrap_or_default();
    if git.is_ancestor(&from_sha, &local) {
        // Up to date by ancestry; it must be by content too.
        let parent_base = super::parent_base(git, stack, i, &from_sha);
        if let Some(pb) = parent_base.filter(|_| !layer.parent_merged) {
            let problems =
                missing_changes(git, &pb, &from_sha, &local).map_err(|e| error(branch, e))?;
            if !problems.is_empty() {
                return Err(content_stop(branch, &from, &problems));
            }
        }
        let mut s = step(branch, "up_to_date");
        s.created = created;
        return Ok(s);
    }
    let mut s = step(
        branch,
        if opts.dry_run {
            "would_merge"
        } else {
            "merged"
        },
    );
    s.from = Some(from.clone());
    s.created = created;
    if opts.dry_run {
        return Ok(s);
    }
    // Where to merge: this worktree when the branch is checked out here; a
    // temporary one when it is checked out nowhere.
    let elsewhere = checked_out
        .iter()
        .find(|(b, _)| b == branch)
        .map(|(_, p)| p.clone());
    let in_place = elsewhere.as_deref().is_some_and(|p| Some(p) == here);
    if let (Some(path), false) = (&elsewhere, in_place) {
        return Err(stop(
            branch,
            "checked_out_elsewhere",
            format!(
                "`{branch}` is checked out in {}, which another session may be working in",
                path.display()
            ),
            format!(
                "run `jouet stack sync` in {} (or merge `{from}` there yourself), then rerun here",
                path.display()
            ),
            Exit::Precondition,
        ));
    }
    let temp;
    let work = if in_place {
        let dirty = git
            .text(&["status", "--porcelain", "--untracked-files=no"])
            .map_err(|e| error(branch, e))?;
        if !dirty.trim().is_empty() {
            return Err(stop(
                branch,
                "dirty",
                format!("`{branch}` is checked out here with uncommitted changes"),
                "commit them (or set them aside in a WIP commit), then rerun".into(),
                Exit::Precondition,
            ));
        }
        Git::new(here.unwrap_or(Path::new(".")))
    } else {
        temp = TempWorktree::add(git, branch).map_err(|e| error(branch, e))?;
        Git::new(&temp.path)
    };
    let merged = work
        .command(&["merge", "--no-ff", "--no-commit", "--quiet", &from_sha])
        .output()
        .map_err(|e| {
            error(
                branch,
                Error::new(Exit::Failed, format!("running git merge: {e}")),
            )
        })?;
    if !merged.status.success() {
        let files: Vec<String> = work
            .text(&["diff", "--name-only", "--diff-filter=U"])
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let next = if in_place {
            "resolve them, `git add` them and `git commit` (or `git merge --abort`), then rerun `jouet stack sync`".to_string()
        } else {
            let _ = work.text(&["merge", "--abort"]);
            format!(
                "`git switch {branch} && git merge {from}`, resolve them and `git commit`, then rerun `jouet stack sync`"
            )
        };
        let mut stopped = stop(
            branch,
            "conflict",
            format!("merging `{from}` into `{branch}` conflicts"),
            next,
            Exit::Failed,
        );
        stopped.files = files;
        return Err(stopped);
    }
    // Verify the merged tree before committing it: every change the merge
    // was to bring in must be there.
    let tree = work.text(&["write-tree"]).map_err(|e| error(branch, e))?;
    let tree = tree.trim();
    let problems = missing_changes(&work, &base, &from_sha, tree).map_err(|e| error(branch, e))?;
    if !problems.is_empty() {
        let _ = work.text(&["merge", "--abort"]);
        return Err(content_stop(branch, &from, &problems));
    }
    work.text(&["commit", "--quiet", "--no-edit"])
        .map_err(|e| error(branch, e))?;
    let new = work.commit_sha("HEAD").unwrap_or_default();
    if !in_place {
        // The temporary worktree was detached at the branch; move the
        // branch only if nobody moved it meanwhile.
        git.text(&["update-ref", &local_ref, &new, &local])
            .map_err(|e| error(branch, e))?;
    }
    s.commit = Some(new);
    Ok(s)
}

fn content_stop(branch: &str, from: &str, problems: &[super::FileProblem]) -> Stopped {
    let files: Vec<String> = problems.iter().map(|p| p.path.clone()).collect();
    let described: Vec<String> = problems
        .iter()
        .map(|p| match &p.commit {
            Some(c) => format!("{} (undoes {c})", p.path),
            None => format!("{} ({})", p.path, p.problem.describe()),
        })
        .collect();
    let mut stopped = stop(
        branch,
        "content",
        format!(
            "`{branch}` would not have all of `{from}`'s changes: {}",
            described.join(", ")
        ),
        format!(
            "in `{branch}`, restore them with `git checkout {from} -- {}` and commit, unless the difference is meant; then rerun `jouet stack sync`",
            files.join(" ")
        ),
        Exit::Failed,
    );
    stopped.files = files;
    stopped
}

fn error(branch: &str, e: Error) -> Stopped {
    stop(
        branch,
        "error",
        e.message,
        "fix that, then rerun".into(),
        e.exit,
    )
}

/// Pushes every layer that is ahead of its remote branch, with a plain
/// push: never a force push. A branch never pushed is left alone.
fn push(git: &Git, stack: &Stack) -> Vec<Push> {
    let mut pushed = Vec::new();
    for layer in stack.layers.iter().filter(|l| !l.merged) {
        let local = git.commit_sha(&format!("refs/heads/{}", layer.branch));
        let remote = git.commit_sha(&format!("refs/remotes/{}/{}", stack.remote, layer.branch));
        let (Some(local), Some(remote)) = (local, remote) else {
            continue;
        };
        if local == remote || git.is_ancestor(&local, &remote) {
            continue;
        }
        let refspec = format!("refs/heads/{0}:refs/heads/{0}", layer.branch);
        let result = git.text(&["push", "--quiet", &stack.remote, &refspec]);
        pushed.push(Push {
            branch: layer.branch.clone(),
            ok: result.is_ok(),
            error: result.err().map(|e| e.message),
        });
    }
    pushed
}

/// A detached worktree of a branch, removed when dropped.
struct TempWorktree<'a> {
    git: &'a Git,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl<'a> TempWorktree<'a> {
    fn add(git: &'a Git, branch: &str) -> Result<Self, Error> {
        let dir = tempfile::tempdir().map_err(|e| {
            Error::new(Exit::Failed, format!("creating a temporary directory: {e}"))
        })?;
        let path = dir.path().join("wt");
        git.text(&[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &path.to_string_lossy(),
            &format!("refs/heads/{branch}"),
        ])?;
        Ok(Self {
            git,
            path,
            _dir: dir,
        })
    }
}

impl Drop for TempWorktree<'_> {
    fn drop(&mut self) {
        let _ = self.git.text(&[
            "worktree",
            "remove",
            "--force",
            &self.path.to_string_lossy(),
        ]);
    }
}

#[cfg(test)]
mod tests;
