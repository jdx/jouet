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
/// `master`; None when there is none of these. An upstream of the same name
/// is the branch's own copy, as `git push -u` sets it, and holds the change
/// itself once pushed, so the default branch is used instead.
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
        let branch = first(&["rev-parse", "--abbrev-ref", "HEAD"]);
        let own = branch.is_some_and(|b| upstream.split_once('/').is_some_and(|(_, u)| u == b));
        if !own {
            return Some(upstream);
        }
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

/// Whether the work tree is `merge_base` exactly: HEAD is that commit, and
/// nothing differs from it, staged, unstaged or untracked.
pub(crate) fn unchanged_since(root: &Path, merge_base: &str) -> Result<bool, Error> {
    let head = git(root, &["rev-parse", "HEAD"])
        .map_err(|e| Error::new(Exit::Failed, format!("reading HEAD: {e}")))?;
    Ok(head.trim() == merge_base && changed(root, merge_base)?.is_empty())
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

/// How many lines of code in the work tree at `root` differ from the merge
/// base with its default [`base`], added plus removed, committed or not,
/// untracked files counted whole; prose ([`jouet_rules::tree::prose`]) is
/// left out. None when it cannot tell: no base branch, or the branch is its
/// own base (work committed on `main` with no `origin/main`), so what it
/// committed cannot be seen; a merge since the base; a changed file that is
/// binary, or an untracked one that is not a regular text file; or git
/// failing.
pub(crate) fn changed_code_lines(root: &Path) -> Option<u64> {
    use jouet_rules::tree::prose;
    let base = base(root, None).ok()?;
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    if base.name == "HEAD" || base.name == branch.trim() {
        return None;
    }
    let merge_base = base.merge_base.as_str();
    // A merge brings in code the change did not write, which can interact
    // with it however few lines the change itself has.
    let range = format!("{merge_base}..HEAD");
    if !git(root, &["rev-list", "--merges", "--max-count=1", &range])
        .ok()?
        .trim()
        .is_empty()
    {
        return None;
    }
    let numstat = git(
        root,
        &[
            "diff",
            "--numstat",
            "--no-renames",
            "--no-ext-diff",
            "-z",
            merge_base,
            "--",
        ],
    )
    .ok()?;
    let mut lines = 0;
    // `ADDED\tREMOVED\tPATH`, with `-` for both in a binary file.
    for entry in numstat.split('\0').filter(|e| !e.is_empty()) {
        let mut fields = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if prose(path) {
            continue;
        }
        lines += added.parse::<u64>().ok()? + removed.parse::<u64>().ok()?;
    }
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"]).ok()?;
    for path in untracked.split('\0').filter(|p| !p.is_empty() && !prose(p)) {
        // Only a regular file is opened: a FIFO would wait for a writer.
        let file = root.join(path);
        if !std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_file()) {
            return None;
        }
        let text = std::fs::read_to_string(file).ok()?;
        lines += text.lines().count() as u64;
    }
    Some(lines)
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
    fn counts_the_lines_of_code_a_branch_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let mut all = vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ];
            all.extend_from_slice(args);
            git(root, &all).unwrap();
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.py"), "one\ntwo\n").unwrap();
        run(&["add", "a.py"]);
        run(&["commit", "-q", "-m", "base"]);
        // Committed on `main` with no other base to see it from.
        assert_eq!(changed_code_lines(root), None);
        run(&["switch", "-q", "-c", "fix"]);
        std::fs::write(root.join("a.py"), "one\n2\nthree\n").unwrap();
        run(&["commit", "-q", "-am", "fix"]);
        // Uncommitted and untracked changes count too, prose does not.
        std::fs::write(root.join("a.py"), "one\n2\nthree\nfour\n").unwrap();
        std::fs::write(root.join("test_a.py"), "x\ny\n").unwrap();
        std::fs::write(root.join("NOTES.md"), "a\nb\nc\n").unwrap();
        // two -> 2 (2), three (1), four (1), test_a.py (2)
        assert_eq!(changed_code_lines(root), Some(6));
        std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        assert_eq!(changed_code_lines(root), None);
        std::fs::remove_file(root.join("blob.bin")).unwrap();
        // A merge brings in code the change did not write.
        run(&["commit", "-q", "-am", "more"]);
        run(&["switch", "-q", "main"]);
        std::fs::write(root.join("b.py"), "b\n").unwrap();
        run(&["add", "b.py"]);
        run(&["commit", "-q", "-m", "theirs"]);
        run(&["switch", "-q", "fix"]);
        assert!(changed_code_lines(root).is_some());
        run(&["merge", "-q", "--no-edit", "main"]);
        assert_eq!(changed_code_lines(root), None);
    }

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
