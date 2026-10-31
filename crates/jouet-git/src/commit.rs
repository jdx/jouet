//! `jouet commit PATH... -m MSG`: commit only the named paths.
//!
//! In a checkout shared by several sessions, `git add -A` or `git commit -a`
//! sweeps up whatever else is there: untracked caches, build output, another
//! session's half-done work. This commits exactly the paths given, as they
//! are in the working tree, with `git commit --only`, so anything else
//! already staged stays staged and out of the commit. Untracked files are
//! added only when a named path covers them, and are unstaged again if the
//! commit fails.
//!
//! Hooks run as they would for `git commit`; `--no-verify` is never passed.
//! git's output, hooks included, goes to a log, and a failed commit reports
//! the end of it.
//!
//! A [`Scanner`] may look at the content first: [`run_scanned`] builds a
//! temporary index holding HEAD plus the named paths, exactly what `git
//! commit --only` will record, and refuses the commit when the scanner
//! finds a secret there. The real index is never touched.

use crate::conventional::{self, Policy};
use crate::{Error, Git, nul_fields};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Lines of git's output shown when a commit fails.
const HOOK_TAIL: usize = 40;

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub paths: Vec<String>,
    pub message: String,
    /// `Key: value` lines to append, already expanded.
    pub trailers: Vec<String>,
    pub conventional: Policy,
    /// The allowed conventional-commit types.
    pub types: Vec<String>,
    pub dry_run: bool,
    /// Where a failed commit's full output is kept.
    pub log_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct File {
    pub path: String,
    /// `added`, `modified`, `deleted` or `type_changed`.
    pub status: &'static str,
    /// Lines added and removed; absent for binary files and, in a dry run,
    /// for files not yet tracked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
}

/// The output of a commit that git refused, usually because a hook failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The last lines of git's output, hooks included.
    pub output: Vec<String>,
    /// How many lines came before `output`.
    #[serde(skip_serializing_if = "is_zero")]
    pub lines_omitted: usize,
    pub log_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub committed: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub subject: String,
    /// The whole message, trailers included.
    pub message: String,
    pub files: Vec<File>,
    /// Untracked files a named path covered, added by this commit.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub newly_tracked: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    /// What the secret scan found, when one ran or was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_scan: Option<SecretScan>,
}

impl Report {
    /// Whether the secret scan found a secret, so nothing was committed.
    pub fn secret_found(&self) -> bool {
        self.secret_scan
            .as_ref()
            .is_some_and(|s| s.status == ScanStatus::Found)
    }
}

/// A secret scan of the content a commit would record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretScan {
    /// What scanned: `jouet` (gitleaks' rules), `fnox`, or `jouet+fnox`.
    pub tool: String,
    pub status: ScanStatus,
    /// How many files it read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_scanned: Option<u64>,
    /// Where a secret was found. Never the secret itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<SecretFinding>,
    /// Why the scan, or a part of it, was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the scanner warned about, such as secrets it could not check.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanStatus {
    Clean,
    Found,
    Skipped,
}

/// One secret, found in one file: a fnox secret's value, named by
/// `secret`, or what a gitleaks rule matched, named by `rule`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SecretFinding {
    /// The fnox secret's name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub secret: String,
    /// The gitleaks rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub path: String,
    pub line: u64,
    /// The rule's match, with the secret cut to its first characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<String>,
}

/// Scans the content a commit would record.
pub trait Scanner {
    /// `index` is a git index file holding exactly what the commit will
    /// record: HEAD plus the named paths. Point `GIT_INDEX_FILE` at it and
    /// read the staged content.
    fn scan(&self, top: &Path, index: &Path) -> Result<SecretScan, Error>;
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Checks the request, then commits (or, for a dry run, reports what would
/// be committed). A commit git refuses is `Ok` with `failure` set.
pub fn run(git: &Git, req: &Request) -> Result<Report, Error> {
    run_scanned(git, req, None)
}

/// [`run`], with `scanner` reading the content before it is committed. A
/// secret it finds is `Ok` with `secret_scan` saying so, and nothing is
/// committed or left staged. A dry run does not scan.
pub fn run_scanned(
    git: &Git,
    req: &Request,
    scanner: Option<&dyn Scanner>,
) -> Result<Report, Error> {
    let top = git.toplevel()?;
    if req.paths.is_empty() {
        return Err(no_paths());
    }
    let message = req.message.trim_end();
    if message.trim().is_empty() {
        return Err(Error::new(Exit::Usage, "the commit message is empty"));
    }
    for path in &req.paths {
        check_path(&top, &git.resolve(path), path)?;
    }
    // Untracked files are listed relative to the top, so they are added
    // from there.
    let at_top = Git::new(&top);
    let head = git.head();
    let mut files = Vec::new();
    let mut untracked = Vec::new();
    for path in &req.paths {
        let (changed, new) = changes(git, head.as_deref(), path)?;
        if changed.is_empty() && new.is_empty() {
            return Err(unchanged(git, path));
        }
        files.extend(changed);
        untracked.extend(new);
    }
    dedupe(&mut files, |f| f.path.clone());
    dedupe(&mut untracked, Clone::clone);
    for path in &untracked {
        if !files.iter().any(|f| &f.path == path) {
            files.push(File {
                path: path.clone(),
                status: "added",
                added: None,
                removed: None,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut warnings = Vec::new();
    if req.conventional != Policy::Off
        && let Some(source) = conventional::detect(&top)
    {
        let problems = conventional::lint(message, &req.types);
        if !problems.is_empty() {
            let why = format!(
                "the subject is not a conventional commit, which {source} asks for: {}",
                problems.join("; ")
            );
            if req.conventional == Policy::Refuse {
                return Err(Error::new(Exit::Precondition, why)
                    .hint("fix the subject, or set [commit] conventional = \"warn\""));
            }
            warnings.push(why);
        }
    }
    let message = with_trailers(git, message, &req.trailers)?;
    let subject = message.lines().next().unwrap_or_default().to_string();
    let mut report = Report {
        dry_run: req.dry_run,
        branch: git.branch(),
        subject,
        message,
        files,
        newly_tracked: untracked.clone(),
        warnings,
        ..Report::default()
    };
    if req.dry_run {
        return Ok(report);
    }
    if let Some(scanner) = scanner {
        let scan = scan(git, &top, head.as_deref(), &req.paths, scanner)?;
        report.secret_scan = Some(scan);
        if report.secret_found() {
            return Ok(report);
        }
    }

    if !untracked.is_empty() {
        let mut args = vec!["add".to_string(), "--".to_string()];
        args.extend(untracked.iter().cloned());
        at_top.text(&args)?;
    }
    match commit(git, &req.paths, &report.message, &req.log_dir) {
        Ok(None) => {}
        Ok(Some(failure)) => {
            unstage(&at_top, head.as_deref(), &untracked);
            report.failure = Some(failure);
            return Ok(report);
        }
        Err(e) => {
            unstage(&at_top, head.as_deref(), &untracked);
            return Err(e);
        }
    }
    report.committed = true;
    report.sha = git.head();
    report.files = committed_files(git)?;
    // A commit-msg hook may have changed the message.
    if let Some(message) = git.line(&["log", "-1", "--format=%B", "HEAD"]) {
        report.subject = message.lines().next().unwrap_or_default().to_string();
        report.message = message;
    }
    Ok(report)
}

fn no_paths() -> Error {
    Error::new(Exit::Usage, "name the paths to commit")
        .hint("jouet commit stages only the paths it is given, never everything: jouet commit src/a.rs src/b.rs -m MSG")
}

/// Refuses a path that names the whole tree, or lies outside it. `full` is
/// `path` made absolute.
fn check_path(top: &Path, full: &Path, path: &str) -> Result<(), Error> {
    if path.is_empty() {
        return Err(Error::new(Exit::Usage, "an empty path names nothing"));
    }
    let full = normalize(full);
    // Canonicalize what exists, so symlinked checkouts (macOS's /tmp) still
    // compare equal to git's top.
    let top = top.canonicalize().unwrap_or_else(|_| top.to_path_buf());
    let full = existing_prefix_canonical(&full);
    if full == top {
        return Err(
            Error::new(Exit::Usage, format!("`{path}` is the whole repository")).hint(
                "name the files or directories to commit; jouet commit never sweeps up everything",
            ),
        );
    }
    if !full.starts_with(&top) {
        return Err(Error::new(
            Exit::Usage,
            format!("`{path}` is outside the repository at {}", top.display()),
        ));
    }
    Ok(())
}

/// Removes `.` and folds `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Canonicalizes the longest existing prefix of `path` and appends the rest,
/// so a deleted file still resolves through a symlinked parent.
fn existing_prefix_canonical(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(real) = cur.canonicalize() {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cur.file_name().map(ToOwned::to_owned), cur.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The tracked files under `path` that differ from HEAD (the working tree
/// against HEAD, staged or not), and the untracked, not ignored files under
/// it.
fn changes(git: &Git, head: Option<&str>, path: &str) -> Result<(Vec<File>, Vec<String>), Error> {
    let mut files = Vec::new();
    match head {
        Some(head) => {
            let status = git.text(&[
                "diff",
                "--no-renames",
                "--name-status",
                "-z",
                head,
                "--",
                path,
            ])?;
            let numstat =
                git.text(&["diff", "--no-renames", "--numstat", "-z", head, "--", path])?;
            let counts = parse_numstat(&numstat);
            let mut fields = nul_fields(&status);
            while let (Some(code), Some(name)) = (fields.next(), fields.next()) {
                let (added, removed) = counts.get(name).copied().unwrap_or((None, None));
                files.push(File {
                    path: name.to_string(),
                    status: status_name(code),
                    added,
                    removed,
                });
            }
        }
        None => {
            // An unborn branch: every tracked file is new.
            let cached = git.text(&["ls-files", "-z", "--cached", "--full-name", "--", path])?;
            for name in nul_fields(&cached) {
                files.push(File {
                    path: name.to_string(),
                    status: "added",
                    added: None,
                    removed: None,
                });
            }
        }
    }
    let others = git.text(&[
        "ls-files",
        "-z",
        "--others",
        "--exclude-standard",
        "--full-name",
        "--",
        path,
    ])?;
    let untracked = nul_fields(&others).map(str::to_string).collect();
    Ok((files, untracked))
}

fn parse_numstat(text: &str) -> std::collections::HashMap<&str, (Option<u64>, Option<u64>)> {
    let mut out = std::collections::HashMap::new();
    for field in nul_fields(text) {
        let mut parts = field.splitn(3, '\t');
        if let (Some(a), Some(r), Some(name)) = (parts.next(), parts.next(), parts.next()) {
            out.insert(name, (a.parse().ok(), r.parse().ok()));
        }
    }
    out
}

fn status_name(code: &str) -> &'static str {
    match code.chars().next() {
        Some('A') => "added",
        Some('D') => "deleted",
        Some('T') => "type_changed",
        _ => "modified",
    }
}

/// Why a named path has nothing to commit.
fn unchanged(git: &Git, path: &str) -> Error {
    if git.ignores(path) {
        return Error::new(
            Exit::Precondition,
            format!("`{path}` is ignored by git, so it cannot be committed"),
        );
    }
    let tracked = git.line(&["ls-files", "--", path]).is_some();
    if !tracked && std::fs::symlink_metadata(git.resolve(path)).is_err() {
        return Error::new(
            Exit::NotFound,
            format!("`{path}`: no such file or directory"),
        );
    }
    Error::new(
        Exit::Precondition,
        format!("`{path}` has no changes to commit"),
    )
    .hint("name only the paths you changed")
}

fn dedupe<T, K: PartialEq>(items: &mut Vec<T>, key: impl Fn(&T) -> K) {
    let mut seen: Vec<K> = Vec::new();
    items.retain(|item| {
        let k = key(item);
        if seen.contains(&k) {
            false
        } else {
            seen.push(k);
            true
        }
    });
}

/// The message with `trailers` appended by `git interpret-trailers`, which
/// joins an existing trailer block and skips a trailer already present.
fn with_trailers(git: &Git, message: &str, trailers: &[String]) -> Result<String, Error> {
    if trailers.is_empty() {
        return Ok(format!("{message}\n"));
    }
    let mut args = vec![
        "-c".to_string(),
        "trailer.ifexists=addIfDifferent".to_string(),
        "-c".to_string(),
        "trailer.where=end".to_string(),
        "interpret-trailers".to_string(),
    ];
    for t in trailers {
        match t.split_once(':') {
            Some((key, value))
                if !key.trim().is_empty()
                    && !key.contains(char::is_whitespace)
                    && !value.trim().is_empty() => {}
            _ => {
                return Err(Error::new(
                    Exit::Usage,
                    format!("a trailer is `Key: value`, not `{t}`"),
                ));
            }
        }
        args.push("--trailer".into());
        args.push(t.clone());
    }
    let mut child = git
        .command(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::new(Exit::Failed, format!("running git: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("{message}\n").as_bytes())
            .map_err(|e| Error::new(Exit::Failed, format!("writing to git: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::new(Exit::Failed, format!("running git: {e}")))?;
    if !out.status.success() {
        return Err(Error::new(
            Exit::Failed,
            format!(
                "git interpret-trailers failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs `scanner` over a temporary index of HEAD plus `paths` as they are in
/// the working tree, which is what `git commit --only` records.
fn scan(
    git: &Git,
    top: &Path,
    head: Option<&str>,
    paths: &[String],
    scanner: &dyn Scanner,
) -> Result<SecretScan, Error> {
    let io =
        |e: std::io::Error| Error::new(Exit::Failed, format!("preparing the secret scan: {e}"));
    let dir = tempfile::tempdir().map_err(io)?;
    let index = dir.path().join("index");
    let run = |args: &[String]| -> Result<(), Error> {
        let out = git
            .command(args)
            .env("GIT_INDEX_FILE", &index)
            .output()
            .map_err(|e| Error::new(Exit::Failed, format!("running git: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        Err(Error::new(
            Exit::Failed,
            format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ))
    };
    match head {
        Some(head) => run(&["read-tree".into(), head.into()])?,
        None => run(&["read-tree".into(), "--empty".into()])?,
    }
    let mut add = vec!["add".to_string(), "--".to_string()];
    add.extend(paths.iter().cloned());
    run(&add)?;
    scanner.scan(top, &index)
}

/// `git commit --only -F MESSAGE -- PATHS`, its output to a log. `None`
/// when it succeeded.
fn commit(
    git: &Git,
    paths: &[String],
    message: &str,
    log_dir: &Path,
) -> Result<Option<Failure>, Error> {
    let io = |e: std::io::Error| Error::new(Exit::Failed, format!("writing the commit log: {e}"));
    let mut file = tempfile::NamedTempFile::new().map_err(io)?;
    file.write_all(message.as_bytes()).map_err(io)?;
    let (log_path, log) = crate::new_log(log_dir, "commit").map_err(io)?;
    let mut args: Vec<std::ffi::OsString> = vec![
        "commit".into(),
        "--only".into(),
        "--cleanup=whitespace".into(),
        "-F".into(),
        file.path().as_os_str().to_owned(),
        "--".into(),
    ];
    args.extend(paths.iter().map(Into::into));
    let status = git
        .command(&args)
        .stdout(log.try_clone().map_err(io)?)
        .stderr(log)
        .status()
        .map_err(|e| Error::new(Exit::Failed, format!("running git commit: {e}")))?;
    if status.success() {
        let _ = std::fs::remove_file(&log_path);
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&std::fs::read(&log_path).unwrap_or_default()).into_owned();
    let (output, lines_omitted) = crate::tail(&text, HOOK_TAIL);
    Ok(Some(Failure {
        exit_code: status.code(),
        output,
        lines_omitted,
        log_path: log_path.display().to_string(),
    }))
}

/// Unstages the untracked files this commit added, after it failed.
fn unstage(git: &Git, head: Option<&str>, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let mut args: Vec<String> = match head {
        Some(_) => vec!["reset".into(), "-q".into()],
        None => vec!["rm".into(), "-q".into(), "--cached".into()],
    };
    args.push("--".into());
    args.extend(paths.iter().cloned());
    let _ = git.text(&args);
}

/// The files in HEAD's commit, with line counts.
fn committed_files(git: &Git) -> Result<Vec<File>, Error> {
    let base = [
        "diff-tree",
        "-r",
        "--root",
        "--no-commit-id",
        "--no-renames",
        "-z",
    ];
    let status = git.text(&[&base[..], &["--name-status", "HEAD"]].concat())?;
    let numstat = git.text(&[&base[..], &["--numstat", "HEAD"]].concat())?;
    let counts = parse_numstat(&numstat);
    let mut files = Vec::new();
    let mut fields = nul_fields(&status);
    while let (Some(code), Some(name)) = (fields.next(), fields.next()) {
        let (added, removed) = counts.get(name).copied().unwrap_or((None, None));
        files.push(File {
            path: name.to_string(),
            status: status_name(code),
            added,
            removed,
        });
    }
    Ok(files)
}

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;
