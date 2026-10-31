//! `jouet edit`: exact, checked, all-or-nothing text edits across files.
//!
//! An edit runs in two phases. [`plan`] applies every operation to an
//! in-memory view of the files it touches and reports each one; nothing is
//! written. Only when every operation succeeded does [`write`] put the results
//! on disk, and if any write fails part way, the files already written are
//! restored before the error is returned. A failed edit therefore never leaves
//! some files changed and others not.

pub mod spec;

use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use spec::{Count, Edit, FileOp, Spec};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Written,
    Created,
    Deleted,
    Moved,
    /// Valid, but not written because another file failed.
    NotWritten,
    Failed,
    /// Written, then restored because `--check` failed.
    Reverted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditStatus {
    Ok,
    NoMatch,
    TooManyMatches,
    TooFewMatches,
    AnchorNotFound,
    /// A symbol edit on a file jouet cannot read definitions from, or a
    /// malformed selector.
    Unsupported,
    /// A symbol edit whose result has more syntax errors than the file had.
    DoesNotParse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileErrorCode {
    NotFound,
    AlreadyExists,
    HashMismatch,
    NotText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileError {
    pub(crate) code: FileErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Closest {
    /// 1-based line where the closest region starts.
    pub line: usize,
    pub similarity: f64,
    /// Line diff from the spec's text (`-`) to the file's text (`+`).
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditReport {
    pub index: usize,
    pub status: EditStatus,
    #[serde(serialize_with = "serialize_count")]
    pub expected: Count,
    pub found: usize,
    /// 1-based start line of each match.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closest: Option<Closest>,
    /// For a symbol edit: the selector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// For a symbol edit that matched nothing, the file's definitions; for
    /// one that matched several, each of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
    /// Why a symbol edit was refused, when the status alone does not say,
    /// or for `jouet edit replace`, the flags that accept the matches found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// For a symbol edit that changed the definition's first line: what it
    /// was and what it is now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureChange>,
    /// For a whole-line edit that matched nothing: where its text comes
    /// close, but not as whole lines or not with the same whitespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_miss: Option<NearMiss>,
}

/// Where the text of a whole-line edit that matched nothing does occur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NearMiss {
    /// The text occurs, but starts or ends inside a line: the search held
    /// part of a line.
    InsideLine {
        /// 1-based line where each occurrence starts.
        lines: Vec<usize>,
        /// The text spans more than one line.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        multiline: bool,
    },
    /// The lines match once leading and trailing whitespace is ignored:
    /// indentation, tabs for spaces, or trailing spaces differ.
    Whitespace {
        /// 1-based line where each such match starts.
        lines: Vec<usize>,
    },
}

/// A symbol edit that changed a definition's first line, its signature in
/// most languages, so callers may need changing too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignatureChange {
    /// The definition's name, without its scope.
    pub name: String,
    /// The 1-based line of the definition's name, before the edit.
    pub line: usize,
    pub before: String,
    pub after: String,
}

/// Finds definitions for symbol edits. jouet's binary implements it with
/// tree-sitter, so this crate needs no grammars.
pub trait Symbols: std::fmt::Debug + Sync {
    /// The definitions `selector` names in `text`, the contents of `path`.
    fn locate(&self, path: &Path, text: &str, selector: &str) -> Result<Vec<Span>, SymbolError>;
    /// How many syntax errors `text` has, or `None` if that cannot be told.
    fn syntax_errors(&self, path: &Path, text: &str) -> Option<usize>;
}

/// Where a definition is: bytes `start..end` of the text, its first line,
/// and a short description such as `method Vfox::download (40-88)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub describe: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolError {
    /// jouet cannot read definitions from this kind of file.
    Unsupported,
    /// The selector is malformed.
    Invalid(String),
    /// Nothing matched; the file's definitions, described.
    NotFound(Vec<String>),
}

/// A definition whose signature changed, and its callers in the code graph
/// in files the edit did not touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Callers {
    pub symbol: String,
    pub references: Vec<Reference>,
    /// References left out of the list.
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reference {
    pub path: String,
    pub line: usize,
}

fn serialize_count<S: Serializer>(count: &Count, s: S) -> Result<S::Ok, S::Error> {
    match count {
        Count::Exactly(n) => s.serialize_u64(*n as u64),
        Count::All => s.serialize_str("all"),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileReport {
    pub path: String,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<EditReport>,
    #[serde(skip_serializing_if = "is_zero")]
    pub added: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<FileError>,
    /// Unified diff of the change, filled in only when asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub applied: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    pub files_written: usize,
    pub files: Vec<FileReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckReport>,
    /// Definitions whose signature a symbol edit changed, with references
    /// to them elsewhere: callers that may need changing too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub callers: Vec<Callers>,
}

/// The result of `--check`: the command run after writing, and its output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckReport {
    pub command: String,
    /// The command's exit code, absent when a signal ended it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub passed: bool,
    /// The last lines of its combined stdout and stderr.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub output_truncated: bool,
    pub log_path: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reverted: bool,
}

/// How many trailing lines of a check's output the report keeps.
const CHECK_TAIL: usize = 30;

/// Runs `command` through the shell with stdout and stderr captured to one
/// log file under `log_dir`, and reports its exit code and the end of its
/// output.
pub fn run_check(command: &str, log_dir: &Path) -> io::Result<CheckReport> {
    fs::create_dir_all(log_dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let log_path = log_dir.join(format!("edit-check-{stamp}-{}.log", std::process::id()));
    let log = fs::File::create(&log_path)?;
    let status = shell(command)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()?;
    let text = String::from_utf8_lossy(&fs::read(&log_path)?).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines.len().saturating_sub(CHECK_TAIL);
    Ok(CheckReport {
        command: command.to_string(),
        exit_code: status.code(),
        passed: status.success(),
        output: lines[tail..].iter().map(|l| (*l).to_string()).collect(),
        output_truncated: tail > 0,
        log_path: log_path.display().to_string(),
        reverted: false,
    })
}

/// `sh -c COMMAND`.
#[cfg(not(windows))]
fn shell(command: &str) -> std::process::Command {
    let mut c = std::process::Command::new("sh");
    c.args(["-c", command]);
    c
}

/// `cmd /S /C "COMMAND"`, with COMMAND exactly as given. An argument passed
/// with `arg` is escaped for the MSVC runtime, and cmd.exe does not undo that
/// escaping: `findstr /C:"a b" f` would fail with `Cannot open b"`.
#[cfg(windows)]
fn shell(command: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("cmd");
    c.raw_arg(format!("/S /C \"{command}\""));
    c
}

impl Report {
    pub fn ok(&self) -> bool {
        self.files.iter().all(|f| f.status != FileStatus::Failed)
    }

    /// True when every failure is a missing file, so the exit code is
    /// `not_found` rather than `precondition`.
    pub fn only_missing_files(&self) -> bool {
        let mut any = false;
        for f in self.files.iter().filter(|f| f.status == FileStatus::Failed) {
            match &f.error {
                Some(e) if e.code == FileErrorCode::NotFound => any = true,
                _ => return false,
            }
        }
        any
    }
}

#[derive(Debug, Default)]
pub struct Options {
    /// Paths and SHA-256 hashes their current contents must have.
    pub expect_hashes: Vec<(String, String)>,
    pub diff: bool,
    /// What finds definitions for symbol edits; without it, they are
    /// refused.
    pub symbols: Option<&'static dyn Symbols>,
}

/// What is on disk for one path, and what the plan has turned it into.
#[derive(Debug)]
struct Entry {
    original: Option<Vec<u8>>,
    /// The mode on disk before the edit, which a revert restores.
    permissions: Option<fs::Permissions>,
    /// The mode the written file gets. Usually the same as `permissions`, but
    /// a file moved here takes its source's mode.
    write_permissions: Option<fs::Permissions>,
    current: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Absent,
    Text(String),
    Binary,
}

impl Entry {
    fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let permissions = fs::metadata(path).ok().map(|m| m.permissions());
                let current = match String::from_utf8(bytes.clone()) {
                    Ok(s) => State::Text(s),
                    Err(_) => State::Binary,
                };
                Ok(Self {
                    original: Some(bytes),
                    write_permissions: permissions.clone(),
                    permissions,
                    current,
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self {
                original: None,
                permissions: None,
                write_permissions: None,
                current: State::Absent,
            }),
            Err(e) => Err(e),
        }
    }

    fn changed(&self) -> bool {
        match (&self.current, &self.original) {
            (State::Absent, None) | (State::Binary, Some(_)) => false,
            (State::Absent, Some(_)) => true,
            (State::Text(s), Some(bytes)) => s.as_bytes() != bytes.as_slice(),
            (State::Text(_), None) => true,
            (State::Binary, None) => unreachable!("binary state always has original bytes"),
        }
    }
}

/// The in-memory result of applying a spec, ready to be written.
#[derive(Debug)]
pub struct Plan {
    pub report: Report,
    entries: BTreeMap<PathBuf, Entry>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn plan(spec: &Spec, options: &Options) -> io::Result<Plan> {
    let mut entries: BTreeMap<PathBuf, Entry> = BTreeMap::new();
    let mut files = Vec::new();
    let mut hash_failures: BTreeMap<PathBuf, (String, FileError)> = BTreeMap::new();
    for (path, want) in &options.expect_hashes {
        let entry = load(&mut entries, path)?;
        let have = entry.original.as_deref().map(sha256_hex);
        if have.as_deref() != Some(want.to_ascii_lowercase().as_str()) {
            let message = match have {
                Some(have) => format!(
                    "expected sha256 {want}, found {have}; the file changed since it was read"
                ),
                None => format!("expected sha256 {want}, but the file does not exist"),
            };
            hash_failures.insert(
                identity(Path::new(path)),
                (
                    path.clone(),
                    FileError {
                        code: FileErrorCode::HashMismatch,
                        message,
                    },
                ),
            );
        }
    }
    for op in &spec.ops {
        let mut report = apply_op(&mut entries, op, options.symbols)?;
        if let Some((_, error)) = hash_failures.remove(&identity(Path::new(op.path())))
            && report.error.is_none()
        {
            report.status = FileStatus::Failed;
            report.error = Some(error);
        }
        files.push(report);
    }
    for (path, error) in hash_failures.into_values() {
        files.push(failed(&path, error));
    }
    let ok = files.iter().all(|f| f.status != FileStatus::Failed);
    for f in &mut files {
        if !ok && f.status != FileStatus::Failed {
            f.status = FileStatus::NotWritten;
        }
        if options.diff && f.status != FileStatus::Failed {
            f.diff = file_diff(&entries, f);
        }
    }
    Ok(Plan {
        report: Report {
            applied: false,
            dry_run: false,
            files_written: 0,
            files,
            check: None,
            callers: Vec::new(),
        },
        entries,
    })
}

/// One key per file however a spec spells its path: `a.rs`, `./a.rs` and
/// its absolute path must share one in-memory entry, or edits made through
/// each spelling would be planned separately and one would overwrite the
/// other on write. Symlinks resolve to their target too, so writing replaces
/// the target rather than the link.
fn identity(path: &Path) -> PathBuf {
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    // A file that does not exist yet: resolve its directory instead.
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(real) = fs::canonicalize(parent) {
            return real.join(name);
        }
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn load<'a>(entries: &'a mut BTreeMap<PathBuf, Entry>, path: &str) -> io::Result<&'a mut Entry> {
    let key = identity(Path::new(path));
    if !entries.contains_key(&key) {
        let entry = Entry::load(&key)?;
        entries.insert(key.clone(), entry);
    }
    Ok(entries.get_mut(&key).expect("just inserted"))
}

fn failed(path: &str, error: FileError) -> FileReport {
    FileReport {
        path: path.to_string(),
        status: FileStatus::Failed,
        moved_to: None,
        edits: Vec::new(),
        added: 0,
        removed: 0,
        error: Some(error),
        diff: None,
    }
}

fn not_found(path: &str) -> FileReport {
    failed(
        path,
        FileError {
            code: FileErrorCode::NotFound,
            message: "no such file".into(),
        },
    )
}

fn apply_op(
    entries: &mut BTreeMap<PathBuf, Entry>,
    op: &FileOp,
    symbols: Option<&dyn Symbols>,
) -> io::Result<FileReport> {
    match op {
        FileOp::Add { path, content } => {
            let entry = load(entries, path)?;
            if entry.current != State::Absent {
                return Ok(failed(
                    path,
                    FileError {
                        code: FileErrorCode::AlreadyExists,
                        message: "the file already exists; use a search block to change it".into(),
                    },
                ));
            }
            entry.current = State::Text(content.clone());
            Ok(FileReport {
                added: content.lines().count(),
                ..ok_report(path, FileStatus::Created)
            })
        }
        FileOp::Delete { path } => {
            let entry = load(entries, path)?;
            let removed = match &entry.current {
                State::Absent => return Ok(not_found(path)),
                State::Text(s) => s.lines().count(),
                State::Binary => 0,
            };
            entry.current = State::Absent;
            Ok(FileReport {
                removed,
                ..ok_report(path, FileStatus::Deleted)
            })
        }
        FileOp::Update {
            path,
            edits,
            move_to,
        } => {
            let entry = load(entries, path)?;
            let before = match &entry.current {
                State::Absent => return Ok(not_found(path)),
                State::Binary => {
                    return Ok(failed(
                        path,
                        FileError {
                            code: FileErrorCode::NotText,
                            message: "the file is not UTF-8 text".into(),
                        },
                    ));
                }
                State::Text(s) => s.clone(),
            };
            let mut text = before.clone();
            let mut cursor = 0;
            let mut reports = Vec::new();
            for (index, edit) in edits.iter().enumerate() {
                let (next, report) =
                    apply_edit(&text, edit, index, &mut cursor, Path::new(path), symbols);
                if report.status == EditStatus::Ok {
                    text = next;
                }
                reports.push(report);
            }
            let ok = reports.iter().all(|r| r.status == EditStatus::Ok);
            let (added, removed) = line_counts(&before, &text);
            let mut report = FileReport {
                edits: reports,
                added,
                removed,
                ..ok_report(
                    path,
                    if ok {
                        FileStatus::Written
                    } else {
                        FileStatus::Failed
                    },
                )
            };
            if !ok {
                return Ok(report);
            }
            match move_to {
                None => entry.current = State::Text(text),
                Some(to) => {
                    entry.current = State::Absent;
                    let mode = entry.write_permissions.clone();
                    let target = load(entries, to)?;
                    if target.current != State::Absent {
                        report.status = FileStatus::Failed;
                        report.error = Some(FileError {
                            code: FileErrorCode::AlreadyExists,
                            message: format!("cannot move to {to}: it already exists"),
                        });
                        return Ok(report);
                    }
                    target.current = State::Text(text);
                    target.write_permissions = mode;
                    report.status = FileStatus::Moved;
                    report.moved_to = Some(to.clone());
                }
            }
            Ok(report)
        }
    }
}

fn ok_report(path: &str, status: FileStatus) -> FileReport {
    FileReport {
        path: path.to_string(),
        status,
        moved_to: None,
        edits: Vec::new(),
        added: 0,
        removed: 0,
        error: None,
        diff: None,
    }
}

fn line_counts(before: &str, after: &str) -> (usize, usize) {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

fn file_diff(entries: &BTreeMap<PathBuf, Entry>, f: &FileReport) -> Option<String> {
    let text_of = |path: &str, current: bool| -> String {
        let Some(entry) = entries.get(&identity(Path::new(path))) else {
            return String::new();
        };
        if current {
            match &entry.current {
                State::Text(s) => s.clone(),
                _ => String::new(),
            }
        } else {
            entry
                .original
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or_default()
                .to_string()
        }
    };
    let before = text_of(&f.path, false);
    let after = text_of(f.moved_to.as_deref().unwrap_or(&f.path), true);
    if before == after {
        return None;
    }
    let to = f.moved_to.as_deref().unwrap_or(&f.path);
    Some(
        similar::TextDiff::from_lines(&before, &after)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{}", f.path), &format!("b/{to}"))
            .to_string(),
    )
}

/// Applies one edit to `text`. On failure the text is returned unchanged and
/// the report says why, so later edits can still be checked and every
/// problem in a spec is reported at once.
fn edit_report(index: usize) -> EditReport {
    EditReport {
        index,
        status: EditStatus::Ok,
        expected: Count::Exactly(1),
        found: 0,
        lines: Vec::new(),
        anchor: None,
        closest: None,
        symbol: None,
        candidates: Vec::new(),
        message: None,
        signature: None,
        near_miss: None,
    }
}

/// Replaces the one definition `selector` names with `new`, indented to
/// where the definition sits.
fn apply_symbol(
    path: &Path,
    text: &str,
    selector: &str,
    new: &str,
    index: usize,
    symbols: Option<&dyn Symbols>,
) -> (String, EditReport) {
    let mut report = edit_report(index);
    report.symbol = Some(selector.to_string());
    let refuse = |mut report: EditReport, status, message: &str| {
        report.status = status;
        report.message = Some(message.to_string());
        (text.to_string(), report)
    };
    let Some(symbols) = symbols else {
        return refuse(
            report,
            EditStatus::Unsupported,
            "symbol edits need tree-sitter grammars, which this jouet or this caller lacks; use a search block",
        );
    };
    let spans = match symbols.locate(path, text, selector) {
        Ok(spans) => spans,
        Err(SymbolError::Unsupported) => {
            return refuse(
                report,
                EditStatus::Unsupported,
                "jouet cannot read definitions from this kind of file; use a search block",
            );
        }
        Err(SymbolError::Invalid(message)) => {
            return refuse(report, EditStatus::Unsupported, &message);
        }
        Err(SymbolError::NotFound(available)) => {
            report.status = EditStatus::NoMatch;
            report.candidates = available;
            return (text.to_string(), report);
        }
    };
    report.found = spans.len();
    report.lines = spans.iter().map(|s| s.line).collect();
    let [span] = spans.as_slice() else {
        report.status = EditStatus::TooManyMatches;
        report.candidates = spans.iter().map(|s| s.describe.clone()).collect();
        return (text.to_string(), report);
    };
    let newline = newline_of(text);
    let line_start = text[..span.start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..span.start];
    let indent = if indent.chars().all(char::is_whitespace) {
        indent
    } else {
        ""
    };
    let body = with_newline(&reindent(new, indent), newline);
    let out = format!("{}{}{}", &text[..span.start], body, &text[span.end..]);
    let before = symbols.syntax_errors(path, text);
    let after = symbols.syntax_errors(path, &out);
    if let (Some(before), Some(after)) = (before, after)
        && after > before
    {
        let errors = |n: usize| format!("{n} syntax {}", if n == 1 { "error" } else { "errors" });
        let message = format!(
            "the file would have {} where it has {}: check that the new text is a whole definition",
            errors(after),
            errors(before)
        );
        return refuse(report, EditStatus::DoesNotParse, &message);
    }
    let first = |s: &str| s.lines().next().unwrap_or("").trim().to_string();
    let (was, is) = (first(&text[span.start..span.end]), first(&body));
    if was != is {
        let name = selector
            .rsplit(|c: char| c == ':' || c == '.' || c.is_whitespace())
            .next()
            .unwrap_or(selector);
        report.signature = Some(SignatureChange {
            name: name.to_string(),
            line: span.line,
            before: was,
            after: is,
        });
    }
    (out, report)
}

/// Replaces the byte ranges `spans` of a file that must still be
/// `original`. The ranges must not overlap.
fn apply_spans(
    path: &Path,
    text: &str,
    original: &str,
    spans: &[(usize, usize, String)],
    index: usize,
    symbols: Option<&dyn Symbols>,
) -> (String, EditReport) {
    let mut report = edit_report(index);
    report.expected = Count::All;
    report.found = spans.len();
    if text != original {
        report.status = EditStatus::NoMatch;
        report.message =
            Some("the file changed after the pattern was matched; run the edit again".into());
        return (text.to_string(), report);
    }
    let mut sorted: Vec<&(usize, usize, String)> = spans.iter().collect();
    sorted.sort_by_key(|(start, _, _)| *start);
    let in_bounds = sorted.iter().all(|(s, e, _)| {
        s <= e && *e <= text.len() && text.is_char_boundary(*s) && text.is_char_boundary(*e)
    });
    if !in_bounds || sorted.windows(2).any(|w| w[0].1 > w[1].0) {
        report.status = EditStatus::Unsupported;
        report.message = Some("the matches overlap or fall outside the file".into());
        return (text.to_string(), report);
    }
    report.lines = sorted.iter().map(|(s, _, _)| line_of(text, *s)).collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, new) in &sorted {
        out.push_str(&text[at..*start]);
        out.push_str(new);
        at = *end;
    }
    out.push_str(&text[at..]);
    if let Some(symbols) = symbols
        && let (Some(before), Some(after)) = (
            symbols.syntax_errors(path, text),
            symbols.syntax_errors(path, &out),
        )
        && after > before
    {
        report.status = EditStatus::DoesNotParse;
        report.message = Some(format!(
            "the rewrite would leave {after} syntax errors where there are {before}: check that it is a whole expression or statement, like the pattern"
        ));
        return (text.to_string(), report);
    }
    (out, report)
}

/// `new` moved to `indent`: its common leading whitespace removed, and every
/// line after the first indented, as the first takes the definition's place
/// after its indentation. Trailing newlines are dropped, so what followed the
/// definition stays as it was.
fn reindent(new: &str, indent: &str) -> String {
    let lines: Vec<&str> = new.trim_end_matches(['\n', '\r']).lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let l = l.get(common..).unwrap_or_else(|| l.trim_start());
            if i == 0 || l.is_empty() {
                l.to_string()
            } else {
                format!("{indent}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_edit(
    text: &str,
    edit: &Edit,
    index: usize,
    cursor: &mut usize,
    path: &Path,
    symbols: Option<&dyn Symbols>,
) -> (String, EditReport) {
    let newline = newline_of(text);
    let mut report = edit_report(index);
    match edit {
        Edit::Symbol { selector, new } => apply_symbol(path, text, selector, new, index, symbols),
        Edit::Spans { original, spans } => apply_spans(path, text, original, spans, index, symbols),
        Edit::Replace {
            old,
            new,
            count,
            line_aligned,
        } => {
            report.expected = *count;
            let old = with_newline(old, newline);
            let new = with_newline(new, newline);
            let (work, padded) = if *line_aligned {
                pad_final_newline(text, newline)
            } else {
                (text.to_string(), false)
            };
            let found = find_all(&work, &old, *line_aligned, 0);
            report.found = found.len();
            report.lines = found.iter().map(|&p| line_of(&work, p)).collect();
            let wanted = match count {
                Count::Exactly(n) => found.len() == *n,
                Count::All => !found.is_empty(),
            };
            if !wanted {
                report.status = match (found.len(), count) {
                    (0, _) => EditStatus::NoMatch,
                    (n, Count::Exactly(want)) if n > *want => EditStatus::TooManyMatches,
                    _ => EditStatus::TooFewMatches,
                };
                if found.is_empty() {
                    report.closest = closest(&work, &old);
                    if *line_aligned {
                        report.near_miss = whitespace_only(&work, &old)
                            .or_else(|| inside_line(&work, &old, newline));
                    }
                }
                return (text.to_string(), report);
            }
            let out = replace_at(&work, &found, old.len(), &new);
            (unpad(out, padded, newline), report)
        }
        Edit::Hunk {
            anchors,
            old,
            new,
            at_eof,
        } => {
            let (work, padded) = pad_final_newline(text, newline);
            let mut start = (*cursor).min(work.len());
            for anchor in anchors {
                match find_anchor(&work, anchor, start) {
                    Some(end) => start = end,
                    None => {
                        report.status = EditStatus::AnchorNotFound;
                        report.anchor = Some(anchor.clone());
                        return (text.to_string(), report);
                    }
                }
            }
            let old = join_with(old, newline);
            let new = join_with(new, newline);
            let found: Vec<usize> = if old.is_empty() {
                // A pure insertion goes at the anchor, or at the end of the file.
                if *at_eof || anchors.is_empty() {
                    vec![work.len()]
                } else {
                    vec![start]
                }
            } else {
                find_all(&work, &old, true, start)
                    .into_iter()
                    .filter(|&p| !*at_eof || p + old.len() == work.len())
                    .collect()
            };
            report.found = found.len();
            report.lines = found.iter().map(|&p| line_of(&work, p)).collect();
            if found.len() != 1 {
                report.status = if found.is_empty() {
                    EditStatus::NoMatch
                } else {
                    EditStatus::TooManyMatches
                };
                if found.is_empty() {
                    report.closest = closest(&work, &old);
                    report.near_miss = whitespace_only(&work, &old);
                }
                return (text.to_string(), report);
            }
            *cursor = found[0] + new.len();
            let out = replace_at(&work, &found, old.len(), &new);
            (unpad(out, padded, newline), report)
        }
    }
}

/// The file's line ending: CRLF when most of its lines end that way.
fn newline_of(text: &str) -> &'static str {
    let lf = text.matches('\n').count();
    let crlf = text.matches("\r\n").count();
    if crlf > 0 && crlf * 2 >= lf {
        "\r\n"
    } else {
        "\n"
    }
}

fn with_newline(s: &str, newline: &str) -> String {
    if newline == "\n" {
        s.to_string()
    } else {
        s.replace("\r\n", "\n").replace('\n', newline)
    }
}

fn join_with(lines: &[String], newline: &str) -> String {
    let mut s = String::new();
    for line in lines {
        s.push_str(line);
        s.push_str(newline);
    }
    s
}

/// Line-based edits end every line with a newline, so a file whose last line
/// has none is matched as if it had one, and the newline is taken off again.
fn pad_final_newline(text: &str, newline: &str) -> (String, bool) {
    if text.is_empty() || text.ends_with('\n') {
        (text.to_string(), false)
    } else {
        (format!("{text}{newline}"), true)
    }
}

fn unpad(mut text: String, padded: bool, newline: &str) -> String {
    if padded && text.ends_with(newline) {
        text.truncate(text.len() - newline.len());
    }
    text
}

/// Byte offsets of non-overlapping matches of `needle` at or after `from`.
/// A line-aligned match must start at the beginning of a line.
fn find_all(hay: &str, needle: &str, line_aligned: bool, from: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let mut start = from;
    while start <= hay.len() {
        let Some(i) = hay[start..].find(needle) else {
            break;
        };
        let p = start + i;
        if !line_aligned || p == 0 || hay.as_bytes()[p - 1] == b'\n' {
            out.push(p);
            start = p + needle.len();
        } else {
            start = p + hay[p..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

fn replace_at(hay: &str, positions: &[usize], len: usize, with: &str) -> String {
    let mut out = String::with_capacity(hay.len());
    let mut last = 0;
    for &p in positions {
        out.push_str(&hay[last..p]);
        out.push_str(with);
        last = p + len;
    }
    out.push_str(&hay[last..]);
    out
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// Finds the first line at or after `from` equal to `anchor`, or failing
/// that, equal once surrounding whitespace is ignored. Returns the offset
/// just past that line.
fn find_anchor(text: &str, anchor: &str, from: usize) -> Option<usize> {
    let lines = || {
        let mut offset = from;
        text[from..].split_inclusive('\n').map(move |line| {
            offset += line.len();
            (line.trim_end_matches(['\n', '\r']), offset)
        })
    };
    lines()
        .find(|(line, _)| *line == anchor)
        .or_else(|| lines().find(|(line, _)| line.trim() == anchor.trim()))
        .map(|(_, end)| end)
}

/// Where a whole-line search's text occurs in `text` without its final
/// newline, which can only be inside a line, since as whole lines it would
/// have matched.
fn inside_line(text: &str, needle: &str, newline: &str) -> Option<NearMiss> {
    let core = needle.strip_suffix(newline).unwrap_or(needle);
    if core.trim().is_empty() {
        return None;
    }
    let mut lines: Vec<usize> = find_all(text, core, false, 0)
        .into_iter()
        .map(|p| line_of(text, p))
        .collect();
    lines.dedup();
    (!lines.is_empty()).then(|| NearMiss::InsideLine {
        lines,
        multiline: core.contains('\n'),
    })
}

/// Where `needle`'s lines all match `text`'s once each line's leading and
/// trailing whitespace is ignored.
fn whitespace_only(text: &str, needle: &str) -> Option<NearMiss> {
    let want: Vec<&str> = needle.lines().map(str::trim).collect();
    if want.iter().all(|l| l.is_empty()) {
        return None;
    }
    let have: Vec<&str> = text.lines().map(str::trim).collect();
    let lines: Vec<usize> = have
        .windows(want.len())
        .enumerate()
        .filter(|(_, window)| *window == want.as_slice())
        .map(|(i, _)| i + 1)
        .collect();
    (!lines.is_empty()).then_some(NearMiss::Whitespace { lines })
}

/// The region of `text` most like `needle`, compared line by line. Windows
/// are first ranked by how many lines match once whitespace is trimmed, and
/// only the best few are compared character by character.
fn closest(text: &str, needle: &str) -> Option<Closest> {
    let want: Vec<&str> = needle.lines().collect();
    let have: Vec<&str> = text.lines().collect();
    if want.is_empty() || have.is_empty() {
        return None;
    }
    let width = want.len().min(have.len());
    let mut ranked: Vec<(usize, usize)> = (0..=have.len() - width)
        .map(|start| {
            let same = want
                .iter()
                .zip(&have[start..start + width])
                .filter(|(a, b)| a.trim() == b.trim())
                .count();
            (same, start)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let want_text = want.join("\n");
    let mut best: Option<(f32, usize)> = None;
    for &(_, start) in ranked.iter().take(8) {
        let window = have[start..start + width].join("\n");
        let ratio = similar::TextDiff::from_chars(want_text.as_str(), window.as_str()).ratio();
        if best.is_none_or(|(r, _)| ratio > r) {
            best = Some((ratio, start));
        }
    }
    let (ratio, start) = best?;
    if ratio < 0.5 {
        return None;
    }
    let window = have[start..start + width].join("\n") + "\n";
    let want_text = want_text + "\n";
    let mut diff = String::new();
    for change in
        similar::TextDiff::from_lines(want_text.as_str(), window.as_str()).iter_all_changes()
    {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Insert => '+',
            similar::ChangeTag::Equal => ' ',
        };
        diff.push(sign);
        diff.push_str(change.value().trim_end_matches(['\n', '\r']));
        diff.push('\n');
    }
    Some(Closest {
        line: start + 1,
        similarity: (f64::from(ratio) * 100.0).round() / 100.0,
        diff,
    })
}

/// What was on disk before [`write`], so it can be put back.
#[derive(Debug)]
pub struct Journal {
    saved: Vec<(PathBuf, Option<Vec<u8>>, Option<fs::Permissions>)>,
}

/// Writes a plan whose report is ok. Each file is written to a temporary
/// file beside it and renamed into place. If any write fails, the files
/// already written are restored and the error is returned.
pub fn write(plan: &mut Plan) -> io::Result<Journal> {
    assert!(plan.report.ok(), "only a plan without failures is written");
    let mut journal = Journal { saved: Vec::new() };
    for (path, entry) in &plan.entries {
        if !entry.changed() {
            continue;
        }
        let result = match &entry.current {
            State::Text(s) => put(path, s.as_bytes(), entry.write_permissions.as_ref()),
            State::Absent => fs::remove_file(path),
            State::Binary => Ok(()),
        };
        if let Err(e) = result {
            // Best effort: the original error is the one worth reporting.
            let _ = revert(&journal);
            return Err(io::Error::new(e.kind(), format!("{}: {e}", path.display())));
        }
        journal.saved.push((
            path.clone(),
            entry.original.clone(),
            entry.permissions.clone(),
        ));
    }
    plan.report.applied = true;
    plan.report.files_written = journal.saved.len();
    Ok(journal)
}

fn put(path: &Path, bytes: &[u8], permissions: Option<&fs::Permissions>) -> io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    if let Some(permissions) = permissions {
        fs::set_permissions(tmp.path(), permissions.clone())?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Restores every file in the journal to its contents before [`write`]:
/// rewritten files get their old bytes and mode back, and created files are
/// removed.
pub fn revert(journal: &Journal) -> io::Result<()> {
    let mut first_error = None;
    for (path, original, permissions) in journal.saved.iter().rev() {
        let result = match original {
            Some(bytes) => put(path, bytes, permissions.as_ref()),
            None => match fs::remove_file(path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        if let Err(e) = result {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests;
