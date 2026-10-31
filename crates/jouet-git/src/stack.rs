//! Stacked branches, for `jouet stack`: whether each layer really contains
//! the layer below it, by ancestry and by content.
//!
//! Ancestry alone is not enough. A restack that resolves a conflict the
//! wrong way, or a rebase that drops a commit, leaves a branch that has its
//! parent's commits in its history but not their changes in its tree. So
//! every file the parent layer changed is checked in the child's tree too:
//! the parent's change, merged into the child's version of the file, must
//! leave it as it is.
//!
//! Which branches form the stack, and which PRs have merged, is the
//! caller's to say (gh-stack's metadata, or the PRs' bases on GitHub); this
//! module only reads git. Refs are read as they are: nothing is fetched.

use crate::{Error, Git};
use jouet_output::exit::Exit;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One layer of a stack, bottom first, as the caller found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub branch: String,
    /// The branch it is built on: the layer below, or the trunk.
    pub parent: String,
    /// The layer's own PR has merged, so it has left the stack.
    pub merged: bool,
    /// The layer below has merged into the trunk, so this layer needs the
    /// trunk merged in rather than its old parent.
    pub parent_merged: bool,
}

/// A stack: its layers, bottom first, and where they land.
#[derive(Debug, Clone)]
pub struct Stack {
    /// The branch the bottom layer targets, such as `main`.
    pub trunk: String,
    /// The remote the branches are pushed to.
    pub remote: String,
    pub layers: Vec<Layer>,
}

/// What became of one of the parent's file changes in the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The child has the file as it was before the parent changed it.
    Reverted,
    /// Some or all of the parent's change is not in the child's version.
    Missing,
    /// The child changed the same lines differently, so jouet cannot tell.
    Conflicting,
}

impl Problem {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Reverted => "reverted",
            Self::Missing => "missing",
            Self::Conflicting => "changed differently",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileProblem {
    pub path: String,
    pub problem: Problem,
    /// The parent's commit whose change is undone, as `abc1234 subject`,
    /// when the file is as it was just before that commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// One layer as git sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayerState {
    pub branch: String,
    pub parent: String,
    /// The local branch's commit; absent when there is no local branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<String>,
    /// The remote-tracking branch's commit, as of the last fetch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Local commits the remote does not have.
    #[serde(skip_serializing_if = "is_zero")]
    pub unpushed: u32,
    /// Remote commits the local branch does not have.
    #[serde(skip_serializing_if = "is_zero")]
    pub behind: u32,
    /// Whether the parent's tip is in this layer's history; absent when
    /// either branch is missing, or the layer has merged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contains_parent: Option<bool>,
    /// The parent's file changes this layer's tree does not have.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<FileProblem>,
    /// The parent merged into the trunk, and the trunk is not merged in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_trunk: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub merged: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl LayerState {
    /// The commit the layer is at: the local branch, else the remote's.
    pub fn tip(&self) -> Option<&str> {
        self.local.as_deref().or(self.remote.as_deref())
    }

    /// What needs doing, as codes, worst first; empty when the layer is in
    /// order. Unpushed commits are not a problem, so not listed.
    pub fn needs(&self) -> Vec<&'static str> {
        let mut needs = Vec::new();
        if self.merged {
            return needs;
        }
        if self.tip().is_none() {
            needs.push("missing_branch");
        }
        if !self.content.is_empty() {
            needs.push("parent_changes_missing");
        }
        if self.behind > 0 {
            needs.push("behind_remote");
        }
        if self.needs_trunk {
            needs.push("needs_trunk");
        } else if self.contains_parent == Some(false) {
            needs.push("needs_parent");
        }
        needs
    }
}

/// The remote the branches are pushed to: `origin`, else the first one.
pub fn remote(git: &Git) -> String {
    let remotes = git.line(&["remote"]).unwrap_or_default();
    let mut names = remotes.lines();
    let first = names.next();
    match first {
        Some(first) if first != "origin" && !remotes.lines().any(|r| r == "origin") => {
            first.to_string()
        }
        _ => "origin".into(),
    }
}

/// The commits of a branch: the local branch, and the remote's.
fn refs(git: &Git, remote: &str, branch: &str) -> (Option<String>, Option<String>) {
    (
        git.commit_sha(&format!("refs/heads/{branch}")),
        git.commit_sha(&format!("refs/remotes/{remote}/{branch}")),
    )
}

/// The trunk's commit: the remote's, which is what merged PRs land on, else
/// the local branch.
pub fn trunk_tip(git: &Git, stack: &Stack) -> Option<String> {
    let (local, remote) = refs(git, &stack.remote, &stack.trunk);
    remote.or(local)
}

/// A branch's commit: the local branch, which may hold fixes not yet
/// pushed, else the remote's.
pub fn branch_tip(git: &Git, remote: &str, branch: &str) -> Option<String> {
    let (local, remote) = refs(git, remote, branch);
    local.or(remote)
}

/// The tip of a layer's parent: the trunk, or the layer below.
pub fn parent_tip(git: &Git, stack: &Stack, layer: &Layer) -> Option<String> {
    if layer.parent == stack.trunk {
        trunk_tip(git, stack)
    } else {
        branch_tip(git, &stack.remote, &layer.parent)
    }
}

/// Where the parent layer's own changes start: where it branched off its
/// own parent. None for the trunk, whose changes are not checked.
pub fn parent_base(git: &Git, stack: &Stack, index: usize, parent: &str) -> Option<String> {
    let below = index.checked_sub(1).map(|i| &stack.layers[i])?;
    if below.branch != stack.layers[index].parent {
        return None;
    }
    let grandparent = parent_tip(git, stack, below)?;
    git.line(&["merge-base", parent, &grandparent])
}

/// Every layer of the stack, as git sees it now.
pub fn inspect(git: &Git, stack: &Stack) -> Result<Vec<LayerState>, Error> {
    let mut out = Vec::new();
    for (i, layer) in stack.layers.iter().enumerate() {
        let (local, remote) = refs(git, &stack.remote, &layer.branch);
        let mut state = LayerState {
            branch: layer.branch.clone(),
            parent: layer.parent.clone(),
            local,
            remote,
            unpushed: 0,
            behind: 0,
            contains_parent: None,
            content: Vec::new(),
            needs_trunk: false,
            merged: layer.merged,
        };
        if let (Some(local), Some(remote)) = (&state.local, &state.remote) {
            state.unpushed = git.count(&format!("{remote}..{local}")).unwrap_or(0);
            state.behind = git.count(&format!("{local}..{remote}")).unwrap_or(0);
        } else if let (Some(local), false) = (&state.local, layer.merged) {
            // Never pushed: every commit past its parent is unpushed.
            if let Some(parent) = parent_tip(git, stack, layer) {
                state.unpushed = git.count(&format!("{parent}..{local}")).unwrap_or(0);
            }
        }
        let tip = state.tip().map(str::to_string);
        if let (false, Some(tip)) = (layer.merged, tip) {
            if layer.parent_merged
                && let Some(trunk) = trunk_tip(git, stack)
            {
                state.needs_trunk = !git.is_ancestor(&trunk, &tip);
            }
            if let Some(parent) = parent_tip(git, stack, layer) {
                let contains = git.is_ancestor(&parent, &tip);
                state.contains_parent = Some(contains);
                // A layer without its parent's commits plainly lacks their
                // changes; one with them should have them all.
                if contains && let Some(base) = parent_base(git, stack, i, &parent) {
                    state.content = missing_changes(git, &base, &parent, &tip)?;
                }
            }
        }
        out.push(state);
    }
    Ok(out)
}

/// The file changes from `from` to `to` that are not in `tip`'s tree. A
/// change is in `tip` when merging it into `tip`'s version of the file
/// leaves that version as it is; a version equal to the one before the
/// change has it reverted.
pub fn missing_changes(
    git: &Git,
    from: &str,
    to: &str,
    tip: &str,
) -> Result<Vec<FileProblem>, Error> {
    let changed = names(git, from, to)?;
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    // Only a file that differs between the parent and the child can lack
    // the parent's change.
    let differs = names(git, to, tip)?;
    let mut problems = Vec::new();
    for path in changed.iter().filter(|p| differs.contains(p)) {
        let base = blob(git, from, path);
        let theirs = blob(git, to, path);
        let mine = blob(git, tip, path);
        let problem = if mine == base {
            Some(Problem::Reverted)
        } else {
            match (&theirs, &mine) {
                // The child deleted a file the parent changed, or kept one
                // the parent deleted.
                (Some(_), None) | (None, Some(_)) => Some(Problem::Missing),
                (Some(theirs), Some(mine)) => merge_check(git, mine, base.as_deref(), theirs)?,
                (None, None) => None,
            }
        };
        if let Some(problem) = problem {
            let undone = undone_commit(git, from, to, path, mine.as_deref());
            problems.push(FileProblem {
                path: path.clone(),
                problem: if undone.is_some() {
                    Problem::Reverted
                } else {
                    problem
                },
                commit: undone,
            });
        }
    }
    Ok(problems)
}

/// The newest of the parent's commits in `from..to` touching `path` that
/// `mine` (the child's blob, None when it has no such file) undoes: the
/// file is as it was just before that commit.
fn undone_commit(
    git: &Git,
    from: &str,
    to: &str,
    path: &str,
    mine: Option<&str>,
) -> Option<String> {
    let log = git
        .text(&[
            "log",
            "--format=%H %h %s",
            "--no-merges",
            &format!("{from}..{to}"),
            "--",
            path,
        ])
        .ok()?;
    log.lines().find_map(|line| {
        let (sha, short) = line.split_once(' ')?;
        (blob(git, &format!("{sha}^"), path).as_deref() == mine).then(|| short.to_string())
    })
}

/// The paths that differ between two commits.
fn names(git: &Git, a: &str, b: &str) -> Result<Vec<String>, Error> {
    let out = git.text(&["diff", "--name-only", "-z", "--no-renames", a, b])?;
    Ok(crate::nul_fields(&out).map(str::to_string).collect())
}

/// The blob of `path` at `commit`, or None when it has no such file.
fn blob(git: &Git, commit: &str, path: &str) -> Option<String> {
    git.line(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{commit}:{path}"),
    ])
}

/// Whether merging `base`→`theirs` into `mine` would change it: None when
/// it would not (the change is there), else what is wrong. A missing base
/// is an empty file.
fn merge_check(
    git: &Git,
    mine: &str,
    base: Option<&str>,
    theirs: &str,
) -> Result<Option<Problem>, Error> {
    let dir = tempfile::tempdir()
        .map_err(|e| Error::new(Exit::Failed, format!("creating a temporary directory: {e}")))?;
    let write = |name: &str, oid: Option<&str>| -> Result<(PathBuf, Vec<u8>), Error> {
        let path = dir.path().join(name);
        let bytes = match oid {
            Some(oid) => cat(git, oid)?,
            None => Vec::new(),
        };
        std::fs::write(&path, &bytes)
            .map_err(|e| Error::new(Exit::Failed, format!("writing {}: {e}", path.display())))?;
        Ok((path, bytes))
    };
    let (mine_path, mine_bytes) = write("mine", Some(mine))?;
    let (base_path, _) = write("base", base)?;
    let (theirs_path, _) = write("theirs", Some(theirs))?;
    let out = git
        .command(&[
            Path::new("merge-file"),
            Path::new("-p"),
            Path::new("--quiet"),
            &mine_path,
            &base_path,
            &theirs_path,
        ])
        .output()
        .map_err(|e| Error::new(Exit::Failed, format!("running git merge-file: {e}")))?;
    Ok(match out.status.code() {
        Some(0) if out.stdout == mine_bytes => None,
        Some(0) => Some(Problem::Missing),
        // A count of conflicts, or an error such as a binary file.
        _ => Some(Problem::Conflicting),
    })
}

fn cat(git: &Git, oid: &str) -> Result<Vec<u8>, Error> {
    let out = git
        .command(&["cat-file", "blob", oid])
        .output()
        .map_err(|e| Error::new(Exit::Failed, format!("running git cat-file: {e}")))?;
    if !out.status.success() {
        return Err(Error::new(Exit::Failed, format!("reading blob {oid}")));
    }
    Ok(out.stdout)
}

/// A stack as gh-stack keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhStack {
    pub trunk: String,
    /// The branches, bottom first, with their PR numbers once submitted.
    pub branches: Vec<(String, Option<u64>)>,
}

#[derive(Deserialize)]
struct GhStackFile {
    #[serde(default)]
    stacks: Vec<GhStackEntry>,
}

#[derive(Deserialize)]
struct GhStackEntry {
    trunk: GhBranch,
    #[serde(default)]
    branches: Vec<GhBranch>,
}

#[derive(Deserialize)]
struct GhBranch {
    branch: String,
    #[serde(default, rename = "pullRequest")]
    pull_request: Option<GhPr>,
}

#[derive(Deserialize)]
struct GhPr {
    number: u64,
}

/// The gh-stack stack holding `branch`, from the `gh-stack` file in the
/// worktree's git directory or the repository's common one. None when
/// gh-stack does not track it.
pub fn gh_stack(git: &Git, branch: &str) -> Option<GhStack> {
    let dirs = [
        git.git_dir().ok(),
        git.line(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .map(PathBuf::from),
    ];
    dirs.into_iter().flatten().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join("gh-stack")).ok()?;
        parse_gh_stack(&text, branch)
    })
}

/// The stack holding `branch` in the text of a `gh-stack` file.
pub fn parse_gh_stack(text: &str, branch: &str) -> Option<GhStack> {
    let file: GhStackFile = serde_json::from_str(text).ok()?;
    let entry = file
        .stacks
        .into_iter()
        .find(|s| s.branches.iter().any(|b| b.branch == branch))?;
    Some(GhStack {
        trunk: entry.trunk.branch,
        branches: entry
            .branches
            .into_iter()
            .map(|b| (b.branch, b.pull_request.map(|p| p.number)))
            .collect(),
    })
}

pub mod sync;

#[cfg(test)]
mod tests;
