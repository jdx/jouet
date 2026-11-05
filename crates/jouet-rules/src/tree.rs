//! The tree a check ran against: a hash of the content of every tracked
//! file in the work tree, staged or not, and of untracked files that are
//! not ignored.
//!
//! It is built from git's own object ids, read without writing anything to
//! the repository: `git ls-files -s` for the index, then `git hash-object`
//! (without `-w`) for each file `git status` reports changed in the work
//! tree, and for each untracked file that is not ignored. Each `mode oid
//! path` line is hashed with SHA-256. A commit's tree hashes the same way
//! from `git ls-tree -r`, so committing the changes a test ran against,
//! new files included, keeps the hash, and so does moving to another branch
//! with the same content. Ignored files are left out, and so are untracked
//! ones past [`MAX_UNTRACKED`] and core dumps, and a submodule counts by the commit the
//! index records for it.
//!
//! `git stash create` would give a real tree id, but it writes objects into
//! the repository on every check, and hooks and probes only read.

use crate::probe::git::{git, git_with_input};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// Past this many untracked files that are not ignored, such as a
/// dependency tree nobody ignored, they are left out rather than hashed at
/// every hook.
const MAX_UNTRACKED: usize = 1000;

/// How many files the work tree containing `dir` tracks, from `git ls-files`,
/// or why it cannot tell: not a repository, or `timeout` passing.
pub fn tracked_files(dir: Option<&Path>, timeout: Duration) -> Result<u32, String> {
    let listed = git(dir, &["ls-files", "-z"], timeout)?;
    Ok(u32::try_from(listed.matches('\0').count()).unwrap_or(u32::MAX))
}

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
    let untracked = git(
        Some(top),
        &["ls-files", "--others", "--exclude-standard", "-z"],
        left(),
    )?;
    let untracked: Vec<&str> = untracked
        .split('\0')
        // A nested repository is listed as its directory.
        .filter(|path| !path.is_empty() && !path.ends_with('/'))
        .collect();
    if untracked.len() <= MAX_UNTRACKED {
        for path in untracked {
            let file = top.join(path);
            if let Some(mode) = file_mode(&file)
                && !core_dump(&file)
            {
                changed.push((path.to_string(), mode.to_string()));
            }
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

/// The mode git would commit a regular file at `path` with, or None for
/// anything else (a symbolic link, a file gone since it was listed).
/// Whether `path` is an ELF core dump, which a crashing test leaves in its
/// directory (`core`, `core.1234`): it is output, not source, so creating or
/// deleting one changes nothing a check ran against.
fn core_dump(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 18];
    let read = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head));
    // `e_type` is ET_CORE (4), in either byte order (`EI_DATA`: 1 little, 2 big).
    read.is_ok()
        && head[..4] == *b"\x7fELF"
        && match head[5] {
            1 => head[16..18] == [4, 0],
            2 => head[16..18] == [0, 4],
            _ => false,
        }
}

fn file_mode(path: &Path) -> Option<&'static str> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Some("100755");
        }
    }
    Some("100644")
}

/// Whether the work tree at `dir` has anything uncommitted: a staged or
/// unstaged change, an untracked file that is not ignored, or a submodule
/// with either or with a commit the index does not record.
pub fn uncommitted(dir: &Path, timeout: Duration) -> Result<bool, String> {
    let status = git(
        Some(dir),
        &[
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ],
        timeout,
    )?;
    Ok(!status.is_empty())
}

/// Every file in the work tree at `dir` that differs from HEAD, staged or
/// not, and every untracked file that is not ignored, relative to the work
/// tree's root: what hk checks by default. Deleted files, which no check
/// can read, are left out.
pub fn changed_paths(dir: &Path, timeout: Duration) -> Result<Vec<String>, String> {
    let started = Instant::now();
    let top = git(Some(dir), &["rev-parse", "--show-toplevel"], timeout)?;
    let top = Path::new(top.trim_end_matches('\n'));
    let status = git(
        Some(top),
        &[
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=all",
        ],
        timeout.saturating_sub(started.elapsed()),
    )?;
    Ok(status
        .split('\0')
        // `XY PATH`
        // A nested repository is listed as its directory, which no check
        // reads, and a deleted file is gone; a path still on disk counts
        // whatever its status says.
        .filter_map(|entry| {
            let path = entry.get(3..)?;
            let file = top.join(path);
            let meta = std::fs::symlink_metadata(&file).ok()?;
            (!meta.is_dir() && !(entry.starts_with("??") && core_dump(&file)))
                .then(|| path.to_string())
        })
        .collect())
}

/// The tree of commit `rev` in the repository containing `dir`, hashed as
/// [`work_tree`] hashes a work tree.
#[cfg(test)]
fn commit_tree(dir: Option<&Path>, rev: &str, timeout: Duration) -> Result<String, String> {
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
/// path order, as hex; then a `.` and the same over the files that are not
/// [`prose`], which [`code_part`] reads, so a check can tell a change to the
/// docs alone from one a test could see.
fn digest(files: &BTreeMap<String, (String, String)>) -> String {
    let hex = |hash: Sha256| -> String {
        hash.finalize()
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let mut all = Sha256::new();
    let mut code = Sha256::new();
    for (path, (mode, oid)) in files {
        let line = format!("{mode} {oid} {path}\0");
        all.update(line.as_bytes());
        if !prose(path) {
            code.update(line.as_bytes());
        }
    }
    format!("{}.{}", hex(all), hex(code))
}

/// The part of a tree from [`work_tree`] or [`commit_tree`] that leaves out
/// prose files; a tree from before it had one is its own.
pub fn code_part(tree: &str) -> &str {
    tree.split_once('.').map_or(tree, |(_, code)| code)
}

/// Whether `path` is prose that a test rarely reads: Markdown,
/// reStructuredText, AsciiDoc or plain text, or a licence, changelog,
/// notice or authors file with no other extension (`LICENSE`,
/// `LICENSE-MIT`, `NOTICE`), never source that only starts with one of
/// those names (`license.rs`).
fn prose(path: &str) -> bool {
    const TEXT: [&str; 6] = ["md", "mdx", "markdown", "rst", "adoc", "txt"];
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let (stem, extension) = match lower.rsplit_once('.') {
        Some((stem, extension)) => (stem, Some(extension)),
        None => (lower.as_str(), None),
    };
    if extension.is_some_and(|e| TEXT.contains(&e)) {
        return true;
    }
    extension.is_none()
        && [
            "license",
            "licence",
            "changelog",
            "notice",
            "authors",
            "copying",
        ]
        .iter()
        .any(|n| {
            stem == *n
                || stem
                    .strip_prefix(n)
                    .is_some_and(|rest| rest.starts_with('-'))
        })
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
    fn a_change_to_prose_alone_keeps_the_code_part() {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        run(dir, &["init", "-q"]);
        std::fs::write(dir.join("a.rs"), "one").unwrap();
        std::fs::create_dir(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/guide.md"), "one").unwrap();
        run(dir, &["add", "."]);
        let before = work_tree(Some(dir), T).unwrap();
        std::fs::write(dir.join("docs/guide.md"), "two").unwrap();
        std::fs::write(dir.join("CHANGELOG.md"), "new").unwrap();
        let docs = work_tree(Some(dir), T).unwrap();
        assert_ne!(before, docs);
        assert_eq!(code_part(&before), code_part(&docs));
        std::fs::write(dir.join("a.rs"), "two").unwrap();
        let code = work_tree(Some(dir), T).unwrap();
        assert_ne!(code_part(&docs), code_part(&code));
        // A tree from before the code part had one is its own.
        assert_eq!(code_part("abc"), "abc");
        for path in [
            "LICENSE",
            "LICENSE-MIT",
            "docs/a.md",
            "NOTICE",
            "notes.txt",
            "CHANGELOG.md",
        ] {
            assert!(prose(path), "{path}");
        }
        for path in [
            "src/license.rs",
            "licenses.py",
            "notice_handler.go",
            "authors/mod.rs",
            "Makefile",
        ] {
            assert!(!prose(path), "{path}");
        }
    }

    #[test]
    fn an_untracked_core_dump_is_not_a_change() {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        run(dir, &["init", "-q"]);
        std::fs::write(dir.join("a.rs"), "one").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-q", "-m", "a"]);
        let before = work_tree(Some(dir), T).unwrap();
        let mut core = b"\x7fELF\x02\x01\x01".to_vec();
        core.resize(16, 0);
        core.extend([4, 0, 0x3e, 0]);
        std::fs::write(dir.join("core.2460150"), &core).unwrap();
        assert_eq!(work_tree(Some(dir), T).unwrap(), before);
        assert_eq!(changed_paths(dir, T).unwrap(), Vec::<String>::new());
        // An executable (ET_EXEC) of the same name is a file like any other.
        core[16] = 2;
        std::fs::write(dir.join("core.2460150"), &core).unwrap();
        assert_ne!(work_tree(Some(dir), T).unwrap(), before);
        assert_eq!(changed_paths(dir, T).unwrap(), vec!["core.2460150"]);
    }

    #[test]
    fn uncommitted_counts_changes_and_untracked_files_not_ignored_ones() {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        run(dir, &["init", "-q"]);
        std::fs::write(dir.join("a.rs"), "one").unwrap();
        run(dir, &["add", "a.rs"]);
        run(dir, &["commit", "-qm", "first"]);
        assert_eq!(uncommitted(dir, T), Ok(false));
        std::fs::write(dir.join(".git/info/exclude"), "*.log\n").unwrap();
        std::fs::write(dir.join("run.log"), "output").unwrap();
        assert_eq!(uncommitted(dir, T), Ok(false));
        std::fs::write(dir.join("new.rs"), "two").unwrap();
        assert_eq!(uncommitted(dir, T), Ok(true));
        std::fs::remove_file(dir.join("new.rs")).unwrap();
        std::fs::write(dir.join("a.rs"), "changed").unwrap();
        assert_eq!(uncommitted(dir, T), Ok(true));
        run(dir, &["checkout", "-q", "a.rs"]);
        assert!(uncommitted(&dir.join("missing"), T).is_err());

        // A change inside a submodule counts too.
        let source = tempfile::tempdir().unwrap();
        run(source.path(), &["init", "-q"]);
        std::fs::write(source.path().join("lib.rs"), "one").unwrap();
        run(source.path(), &["add", "lib.rs"]);
        run(source.path(), &["commit", "-qm", "lib"]);
        let url = source.path().to_str().unwrap();
        run(
            dir,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                url,
                "sub",
            ],
        );
        run(dir, &["commit", "-qm", "sub"]);
        assert_eq!(uncommitted(dir, T), Ok(false));
        std::fs::write(dir.join("sub/lib.rs"), "changed").unwrap();
        assert_eq!(uncommitted(dir, T), Ok(true));
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

        // An ignored file is left out; an untracked one counts, and
        // committing it keeps the hash.
        std::fs::write(dir.join(".git/info/exclude"), "*.log\n").unwrap();
        std::fs::write(dir.join("run.log"), "output").unwrap();
        assert_eq!(work_tree(Some(dir), T).unwrap(), clean);
        std::fs::write(dir.join("new.rs"), "three").unwrap();
        let added = work_tree(Some(dir), T).unwrap();
        assert_ne!(added, clean);
        run(dir, &["add", "new.rs"]);
        run(dir, &["commit", "-qm", "new"]);
        assert_eq!(work_tree(Some(dir), T).unwrap(), added);
        assert_eq!(commit_tree(Some(dir), "HEAD", T).unwrap(), added);
        // An executable one is committed as one.
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let script = work_tree(Some(dir), T).unwrap();
        run(dir, &["add", "run.sh"]);
        run(dir, &["commit", "-qm", "script"]);
        assert_eq!(commit_tree(Some(dir), "HEAD", T).unwrap(), script);
        run(dir, &["rm", "-q", "new.rs", "run.sh"]);
        run(dir, &["commit", "-qm", "back"]);
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

        // A deleted file and a new one count; staging the new one keeps it.
        std::fs::remove_file(dir.join("sub/b.rs")).unwrap();
        let deleted = work_tree(Some(dir), T).unwrap();
        assert_ne!(deleted, edited);
        std::fs::write(dir.join("notes.md"), "scratch").unwrap();
        let noted = work_tree(Some(dir), T).unwrap();
        assert_ne!(noted, deleted);
        run(dir, &["add", "notes.md"]);
        assert_eq!(work_tree(Some(dir), T).unwrap(), noted);

        assert!(commit_tree(Some(dir), "nope", T).is_err());
        let outside = tempfile::tempdir().unwrap();
        assert!(work_tree(Some(outside.path()), T).is_err());
    }
}
