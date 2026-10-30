//! The tree a check ran against: a hash of the content of every tracked
//! file in the work tree, staged or not.
//!
//! It is built from git's own object ids, read without writing anything to
//! the repository: `git ls-files -s` for the index, then `git hash-object`
//! (without `-w`) for each file `git status` reports changed in the work
//! tree. Each `mode oid path` line is hashed with SHA-256. A commit's tree
//! hashes the same way from `git ls-tree -r`, so committing the changes a
//! test ran against keeps the hash, and so does moving to another branch
//! with the same content. Untracked and ignored files are left out, and a
//! submodule counts by the commit the index records for it.
//!
//! `git stash create` would give a real tree id, but it writes objects into
//! the repository on every check, and hooks and probes only read.

use crate::probe::git::{git, git_with_input};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// The tree of the work tree containing `dir` (this process's directory
/// when None), or why there is none: not a repository, a merge in
/// progress, or `timeout` passing.
pub fn work_tree(dir: Option<&Path>, timeout: Duration) -> Result<String, String> {
    let started = Instant::now();
    let left = || timeout.saturating_sub(started.elapsed());
    let top = git(dir, &["rev-parse", "--show-toplevel"], left())?;
    let top = Path::new(top.trim_end_matches('\n'));
    let mut files = BTreeMap::new();
    for entry in git(Some(top), &["ls-files", "-s", "-z"], left())?.split('\0') {
        // `MODE OID STAGE\tPATH`
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split(' ');
        let (Some(mode), Some(oid), Some(stage)) = (meta.next(), meta.next(), meta.next()) else {
            continue;
        };
        if stage != "0" {
            return Err("the index has unmerged paths".into());
        }
        files.insert(path.to_string(), (mode.to_string(), oid.to_string()));
    }
    let status = git(
        Some(top),
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=no",
            "--no-renames",
            "--ignore-submodules=all",
        ],
        left(),
    )?;
    let mut changed = Vec::new();
    for entry in status.split('\0') {
        let fields: Vec<&str> = entry.splitn(9, ' ').collect();
        match fields.as_slice() {
            ["u", ..] => return Err("the index has unmerged paths".into()),
            // `1 XY SUB mH mI mW hH hI PATH`: Y is the work tree's change.
            ["1", xy, _, _, _, worktree_mode, _, _, path] => match xy.chars().nth(1) {
                Some('D') => {
                    files.remove(*path);
                }
                Some('M' | 'T') => changed.push((path.to_string(), worktree_mode.to_string())),
                _ => {}
            },
            _ => {}
        }
    }
    if !changed.is_empty() {
        let input: String = changed
            .iter()
            .map(|(path, _)| format!("{path}\n"))
            .collect();
        let oids = git_with_input(
            Some(top),
            &["hash-object", "--stdin-paths"],
            Some(input.into_bytes()),
            left(),
        )?;
        let oids: Vec<&str> = oids.lines().collect();
        if oids.len() != changed.len() {
            return Err("`git hash-object` skipped a file".into());
        }
        for ((path, mode), oid) in changed.into_iter().zip(oids) {
            files.insert(path, (mode, oid.to_string()));
        }
    }
    Ok(digest(&files))
}

/// The tree of commit `rev` in the repository containing `dir`, hashed as
/// [`work_tree`] hashes a work tree.
pub fn commit_tree(dir: Option<&Path>, rev: &str, timeout: Duration) -> Result<String, String> {
    let rev = format!("{rev}^{{commit}}");
    let out = git(dir, &["ls-tree", "-r", "-z", "--full-tree", &rev], timeout).map_err(|_| {
        format!(
            "`{}` is not a commit in this repository",
            &rev[..rev.len() - 9]
        )
    })?;
    let mut files = BTreeMap::new();
    for entry in out.split('\0') {
        // `MODE TYPE OID\tPATH`
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split(' ');
        if let (Some(mode), Some(_), Some(oid)) = (meta.next(), meta.next(), meta.next()) {
            files.insert(path.to_string(), (mode.to_string(), oid.to_string()));
        }
    }
    Ok(digest(&files))
}

/// The first 16 bytes of the SHA-256 of `MODE OID PATH\0` for each file, in
/// path order, as hex.
fn digest(files: &BTreeMap<String, (String, String)>) -> String {
    let mut hash = Sha256::new();
    for (path, (mode, oid)) in files {
        hash.update(format!("{mode} {oid} {path}\0").as_bytes());
    }
    hash.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const T: Duration = Duration::from_secs(10);

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn the_tree_follows_content_not_commits() {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        run(dir, &["init", "-q"]);
        std::fs::write(dir.join("a.rs"), "one").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/b.rs"), "two").unwrap();
        run(dir, &["add", "a.rs", "sub/b.rs"]);
        run(dir, &["commit", "-qm", "first"]);
        let clean = work_tree(Some(dir), T).unwrap();
        assert_eq!(commit_tree(Some(dir), "HEAD", T).unwrap(), clean);
        // From a subdirectory, the same tree.
        assert_eq!(work_tree(Some(&dir.join("sub")), T).unwrap(), clean);

        // An untracked file is left out; an unstaged edit counts.
        std::fs::write(dir.join("notes.md"), "scratch").unwrap();
        assert_eq!(work_tree(Some(dir), T).unwrap(), clean);
        std::fs::write(dir.join("a.rs"), "changed").unwrap();
        let edited = work_tree(Some(dir), T).unwrap();
        assert_ne!(edited, clean);
        // Staging and committing the edit keeps the hash.
        run(dir, &["add", "a.rs"]);
        assert_eq!(work_tree(Some(dir), T).unwrap(), edited);
        run(dir, &["commit", "-qm", "second"]);
        assert_eq!(work_tree(Some(dir), T).unwrap(), edited);
        assert_eq!(commit_tree(Some(dir), "HEAD", T).unwrap(), edited);
        assert_eq!(commit_tree(Some(dir), "HEAD~1", T).unwrap(), clean);

        // A deleted file and a newly staged one count.
        std::fs::remove_file(dir.join("sub/b.rs")).unwrap();
        let deleted = work_tree(Some(dir), T).unwrap();
        assert_ne!(deleted, edited);
        run(dir, &["add", "notes.md"]);
        assert_ne!(work_tree(Some(dir), T).unwrap(), deleted);

        assert!(commit_tree(Some(dir), "nope", T).is_err());
        let outside = tempfile::tempdir().unwrap();
        assert!(work_tree(Some(outside.path()), T).is_err());
    }
}
