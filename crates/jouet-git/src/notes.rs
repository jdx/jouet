//! Lines of text kept as git notes on commits, under one notes ref, the way
//! tak keeps its measurements:
//!
//! - The data is about a commit, which is what notes are for, and it never
//!   touches the commits or the work tree.
//! - Concurrent writers merge with git's `cat_sort_uniq` strategy, so each
//!   note is a set of lines, kept sorted and without duplicates locally too,
//!   and no merge code is needed.
//! - A push is never forced. tak learned why: a forced push from a checkout
//!   that had not fetched the ref replaced all of its history at once. A
//!   rejected push fetches, merges and tries again instead.

use crate::{Error, Git};
use jouet_output::exit::Exit;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Stdio;

/// How many times a rejected push fetches, merges and tries again.
const PUSH_ATTEMPTS: u32 = 5;

/// The notes under `refs/notes/NAME` of one repository.
pub struct Notes {
    git: Git,
    name: String,
}

impl Notes {
    pub fn new(git: Git, name: &str) -> Self {
        Self {
            git,
            name: name.to_string(),
        }
    }

    fn reference(&self) -> String {
        format!("refs/notes/{}", self.name)
    }

    /// Where a fetch puts the remote's notes before they are merged, so it
    /// never overwrites lines not yet pushed.
    fn remote_reference(&self) -> String {
        format!("refs/notes/{}-remote", self.name)
    }

    /// Adds `lines` to the note on `commit`, keeping what it holds.
    pub fn append(&self, commit: &str, lines: &[String]) -> Result<(), Error> {
        if lines.is_empty() {
            return Ok(());
        }
        let reference = self.reference();
        let mut all: Vec<String> = self
            .git
            .line(&["notes", "--ref", &reference, "show", commit])
            .map(|note| note.lines().map(str::to_string).collect())
            .unwrap_or_default();
        all.extend(lines.iter().cloned());
        // As cat_sort_uniq would leave them, so a local write and a merge
        // of the same lines agree.
        all.sort();
        all.dedup();
        let note = all.join("\n");
        self.committing(&[
            "notes", "--ref", &reference, "add", "-f", "-m", &note, commit,
        ])
    }

    /// The notes on `rev` and its first-parent ancestors, newest first, up
    /// to `depth` commits back: each commit that has a note, with its lines.
    pub fn along(&self, rev: &str, depth: usize) -> Vec<(String, Vec<String>)> {
        let reference = self.reference();
        let Some(listed) = self.git.line(&["notes", "--ref", &reference, "list"]) else {
            return Vec::new();
        };
        let blobs: HashMap<&str, &str> = listed
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(blob, commit)| (commit, blob))
            .collect();
        let depth = depth.to_string();
        let Some(commits) = self
            .git
            .line(&["rev-list", "--first-parent", "-n", &depth, rev, "--"])
        else {
            return Vec::new();
        };
        let noted: Vec<(&str, &str)> = commits
            .lines()
            .filter_map(|c| blobs.get(c).map(|b| (c, *b)))
            .collect();
        let texts = self.blobs(noted.iter().map(|(_, b)| *b));
        noted
            .into_iter()
            .zip(texts)
            .map(|((commit, _), text)| {
                (
                    commit.to_string(),
                    text.lines().map(str::to_string).collect(),
                )
            })
            .collect()
    }

    /// The text of each blob, in order, through one `git cat-file --batch`.
    fn blobs<'a>(&self, ids: impl Iterator<Item = &'a str>) -> Vec<String> {
        let ids: Vec<&str> = ids.collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let mut cmd = self.git.command(&["cat-file", "--batch"]);
        let Ok(mut child) = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Vec::new();
        };
        let input: String = ids.iter().map(|id| format!("{id}\n")).collect();
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let mut out = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut texts = Vec::with_capacity(ids.len());
        for _ in &ids {
            let mut header = String::new();
            if out.read_line(&mut header).unwrap_or(0) == 0 {
                break;
            }
            // `<id> blob <size>`, or `<id> missing`.
            let Some(size) = header
                .split_whitespace()
                .nth(2)
                .and_then(|s| s.parse::<usize>().ok())
            else {
                texts.push(String::new());
                continue;
            };
            let mut body = vec![0; size + 1];
            if out.read_exact(&mut body).is_err() {
                break;
            }
            body.pop();
            texts.push(String::from_utf8_lossy(&body).into_owned());
        }
        let _ = writer.join();
        let _ = child.wait();
        texts.resize(ids.len(), String::new());
        texts
    }

    /// Fetches `remote`'s notes and merges them into the local ones.
    /// `Ok(false)` when the remote has none.
    pub fn fetch(&self, remote: &str) -> Result<bool, Error> {
        let spec = format!("+{}:{}", self.reference(), self.remote_reference());
        if !self.git.succeeds(&["fetch", "--quiet", remote, &spec]) {
            return Ok(false);
        }
        self.absorb()?;
        Ok(true)
    }

    /// Merges the fetched notes into the local ones, keeping both sides.
    fn absorb(&self) -> Result<(), Error> {
        let (local, remote) = (self.reference(), self.remote_reference());
        if self.git.commit_sha(&local).is_none() {
            // Nothing local yet: `notes merge` needs a ref to merge into.
            return self.git.text(&["update-ref", &local, &remote]).map(drop);
        }
        self.committing(&[
            "notes",
            "--ref",
            &local,
            "merge",
            "-q",
            "-s",
            "cat_sort_uniq",
            &remote,
        ])
    }

    /// Pushes the notes to `remote`, merging in what another writer pushed
    /// first, and never forcing.
    pub fn push(&self, remote: &str) -> Result<(), Error> {
        let reference = self.reference();
        if self.git.commit_sha(&reference).is_none() {
            return Err(Error::new(
                Exit::Precondition,
                format!("there are no notes under {reference} to push"),
            ));
        }
        let spec = format!("{reference}:{reference}");
        let mut last = String::new();
        for _ in 0..PUSH_ATTEMPTS {
            let out = self
                .git
                .command(&["push", "--quiet", remote, &spec])
                .output()
                .map_err(|e| Error::new(Exit::Failed, format!("running git: {e}")))?;
            if out.status.success() {
                return Ok(());
            }
            last = String::from_utf8_lossy(&out.stderr).trim().to_string();
            self.fetch(remote)?;
        }
        Err(Error::new(
            Exit::Failed,
            format!("could not push {reference} after {PUSH_ATTEMPTS} attempts: {last}"),
        ))
    }

    /// A git command that writes a notes commit, which needs an author: CI
    /// images often have none, so jouet's own stands in only then.
    fn committing(&self, args: &[&str]) -> Result<(), Error> {
        let mut all: Vec<&str> = Vec::new();
        if self.git.line(&["config", "user.name"]).is_none() {
            all.extend(["-c", "user.name=jouet"]);
        }
        if self.git.line(&["config", "user.email"]).is_none() {
            all.extend(["-c", "user.email=jouet@localhost"]);
        }
        all.extend(args);
        self.git.text(&all).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo(dir: &Path, commits: usize) -> Vec<String> {
        git(dir, &["init", "-q", "-b", "main"]);
        (0..commits)
            .map(|i| {
                git(
                    dir,
                    &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")],
                );
                git(dir, &["rev-parse", "HEAD"])
            })
            .collect()
    }

    #[test]
    fn appended_lines_are_a_sorted_set_read_back_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let shas = repo(dir.path(), 3);
        let notes = Notes::new(Git::new(dir.path()), "t");
        notes.append(&shas[0], &["b".into(), "a".into()]).unwrap();
        notes.append(&shas[0], &["a".into(), "c".into()]).unwrap();
        notes.append(&shas[2], &["z".into()]).unwrap();
        let along = notes.along("HEAD", 10);
        assert_eq!(
            along,
            [
                (shas[2].clone(), vec!["z".to_string()]),
                (shas[0].clone(), vec!["a".into(), "b".into(), "c".into()]),
            ]
        );
        // Only `depth` commits back.
        assert_eq!(notes.along("HEAD", 2).len(), 1);
        // Another notes ref is someone else's.
        assert!(
            Notes::new(Git::new(dir.path()), "other")
                .along("HEAD", 10)
                .is_empty()
        );
    }

    #[test]
    fn a_push_merges_what_another_writer_pushed_first() {
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        let a = tempfile::tempdir().unwrap();
        let shas = repo(a.path(), 1);
        let url = remote.path().to_str().unwrap();
        git(a.path(), &["push", "-q", url, "main"]);
        let b = tempfile::tempdir().unwrap();
        git(b.path(), &["clone", "-q", url, "."]);

        let on_a = Notes::new(Git::new(a.path()), "t");
        on_a.append(&shas[0], &["from a".into()]).unwrap();
        on_a.push(url).unwrap();
        // b never fetched a's notes: its push is rejected, merges, retries.
        let on_b = Notes::new(Git::new(b.path()), "t");
        on_b.append(&shas[0], &["from b".into()]).unwrap();
        on_b.push(url).unwrap();

        assert!(on_a.fetch(url).unwrap());
        assert_eq!(
            on_a.along("HEAD", 1),
            [(shas[0].clone(), vec!["from a".to_string(), "from b".into()])]
        );
    }
}
