//! The changed set: what differs from the merge base with the base branch,
//! committed or not, plus untracked files.

use super::Error;
use jouet_output::exit::Exit;
use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};

/// What the changes are measured against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Base {
    /// The ref given with `--since`, or the one chosen: the upstream, else
    /// the default branch, else HEAD.
    #[serde(rename = "ref")]
    pub name: String,
    /// The merge base of that ref and HEAD.
    pub merge_base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Added,
    Modified,
    Deleted,
    Untracked,
}

/// A changed file, by its path from the repository root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub path: String,
    pub status: Status,
}

/// git in `root`, without prompts; its stdout, or why it failed.
pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The ref to measure from when `--since` is not given: the branch's
/// upstream, else the remote's default branch, else a local `main` or
/// `master`; None when there is none of these.
fn default_base(root: &Path) -> Option<String> {
    let first = |args: &[&str]| {
        git(root, args)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    if let Some(upstream) = first(&[
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    ]) {
        return Some(upstream);
    }
    if let Some(head) = first(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]) {
        return Some(head);
    }
    ["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find(|name| first(&["rev-parse", "--verify", "--quiet", name]).is_some())
        .map(str::to_string)
}

/// The base to measure from: `since`, or the default.
pub(crate) fn base(root: &Path, since: Option<&str>) -> Result<Base, Error> {
    let name = match since {
        Some(since) => since.to_string(),
        None => default_base(root).unwrap_or_else(|| "HEAD".into()),
    };
    let merge_base = git(root, &["merge-base", &name, "HEAD"]).map_err(|e| {
        Error::new(
            Exit::Usage,
            format!("no merge base between `{name}` and HEAD: {e}"),
        )
        .hint("pass a branch or commit this one grew from with --since")
    })?;
    Ok(Base {
        name,
        merge_base: merge_base.trim().to_string(),
    })
}

/// Every file that differs from `merge_base` in the work tree (committed,
/// staged or not), and every untracked file git does not ignore. A rename
/// is a deletion and an addition.
pub(crate) fn changed(root: &Path, merge_base: &str) -> Result<Vec<Change>, Error> {
    let fail = |e: String| Error::new(Exit::Failed, format!("listing changed files: {e}"));
    let diff = git(
        root,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "--no-ext-diff",
            "-z",
            merge_base,
            "--",
        ],
    )
    .map_err(fail)?;
    let mut changes = parse_name_status(&diff);
    let untracked =
        git(root, &["ls-files", "--others", "--exclude-standard", "-z"]).map_err(fail)?;
    changes.extend(
        untracked
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|p| Change {
                path: p.to_string(),
                status: Status::Untracked,
            }),
    );
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes.dedup_by(|a, b| a.path == b.path);
    Ok(changes)
}

/// `git diff --name-status -z` output: a status letter, then the path.
fn parse_name_status(text: &str) -> Vec<Change> {
    let mut words = text.split('\0').filter(|w| !w.is_empty());
    let mut out = Vec::new();
    while let (Some(status), Some(path)) = (words.next(), words.next()) {
        let status = match status.chars().next() {
            Some('A') => Status::Added,
            Some('D') => Status::Deleted,
            _ => Status::Modified,
        };
        out.push(Change {
            path: path.to_string(),
            status,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_name_status_output() {
        let changes = parse_name_status("M\0src/a.ts\0D\0old.py\0A\0new file.py\0T\0link\0");
        let got: Vec<(&str, Status)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.status))
            .collect();
        assert_eq!(
            got,
            [
                ("src/a.ts", Status::Modified),
                ("old.py", Status::Deleted),
                ("new file.py", Status::Added),
                ("link", Status::Modified),
            ]
        );
    }
}
