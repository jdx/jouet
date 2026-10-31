//! `jouet.stack.layer()`: where the current branch sits in a gh-stack
//! stack, and whether the layer below it has merged; and
//! `jouet.git.ahead()`: how many of a branch's commits another commit
//! lacks. Both read git and gh-stack's file, never GitHub.

use super::git::git;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// The current branch's layer, or None when HEAD is detached or gh-stack
/// does not track it. The layer below counts as merged when it was
/// submitted as a PR and its remote branch is gone, as GitHub deletes a
/// merged PR's branch and `git fetch --prune` follows.
pub(crate) fn layer(dir: Option<&Path>, timeout: Duration) -> Result<Option<Value>, String> {
    let started = Instant::now();
    let left = || timeout.saturating_sub(started.elapsed());
    let Ok(branch) = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], left()) else {
        return Ok(None);
    };
    let branch = branch.trim().to_string();
    let dirs = git(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
        left(),
    )?;
    let Some(stack) = dirs.lines().find_map(|d| {
        let text = std::fs::read_to_string(Path::new(d).join("gh-stack")).ok()?;
        jouet_git::stack::parse_gh_stack(&text, &branch)
    }) else {
        return Ok(None);
    };
    let Some(i) = stack.branches.iter().position(|(b, _)| *b == branch) else {
        return Ok(None);
    };
    let (parent, parent_pr) = match i.checked_sub(1) {
        Some(j) => stack.branches[j].clone(),
        None => (stack.trunk.clone(), None),
    };
    let remotes = git(dir, &["remote"], left()).unwrap_or_default();
    let remote = if remotes.lines().any(|r| r == "origin") {
        "origin"
    } else {
        remotes.lines().next().unwrap_or("origin")
    };
    let gone = git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{parent}"),
        ],
        left(),
    )
    .is_err();
    let parent_merged = parent != stack.trunk && parent_pr.is_some() && gone;
    Ok(Some(json!({
        "branch": branch,
        "parent": parent,
        "trunk": stack.trunk,
        "parent_merged": parent_merged,
    })))
}

/// How many commits on the local branch `branch` are not in `of`; None
/// when there is no such local branch.
pub(crate) fn ahead(
    dir: Option<&Path>,
    branch: &str,
    of: &str,
    timeout: Duration,
) -> Result<Option<Value>, String> {
    let started = Instant::now();
    let local = format!("refs/heads/{branch}");
    if git(dir, &["rev-parse", "--verify", "--quiet", &local], timeout).is_err() {
        return Ok(None);
    }
    let left = timeout.saturating_sub(started.elapsed());
    let out = git(
        dir,
        &["rev-list", "--count", &format!("{of}..{local}"), "--"],
        left,
    )?;
    let n: u64 = out
        .trim()
        .parse()
        .map_err(|_| format!("`git rev-list` printed `{}`", out.trim()))?;
    Ok(Some(n.into()))
}
