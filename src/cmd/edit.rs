//! `jouet edit`, `jouet edit replace` and `jouet edit hash`.

use super::Output;
use super::diag as diag_cmd;
use crate::diag::{self, Status, delta};
use crate::{EditArgs, EditCmd, HashArgs, ReplaceArgs};
use jouet_edit::spec::{self, Count, Edit, FileOp, Format, Spec};
use jouet_edit::{
    self as edit, Callers, EditStatus, FileStatus, NearMiss, Options, Reference, Report, Span,
    SymbolError,
};
use jouet_output::exit::Exit;
use jouet_search::symbols::{self, SelectError, Selector};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

pub(crate) fn run(args: EditArgs, out: &Output) -> Exit {
    let spec = match &args.cmd {
        Some(EditCmd::Hash(hash)) => return run_hash(hash, out),
        Some(EditCmd::Replace(replace)) => replace_spec(replace),
        None => read_spec(&args),
    };
    let spec = match spec {
        Ok(spec) => spec,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    let mut expect_hashes = Vec::new();
    for pair in &args.expect_hash {
        match pair.rsplit_once('=') {
            Some((path, hash)) if !path.is_empty() && !hash.is_empty() => {
                expect_hashes.push((path.to_string(), hash.to_string()));
            }
            _ => {
                return out.error(
                    Exit::Usage,
                    &format!("--expect-hash takes PATH=SHA256, not `{pair}`"),
                );
            }
        }
    }
    if args.revert_on_fail && args.check.is_none() && !args.diag {
        return out.error(Exit::Usage, "--revert-on-fail needs --check or --diag");
    }
    let options = Options {
        expect_hashes,
        diff: args.diff,
        symbols: Some(&TreeSitter),
    };
    let mut plan = match edit::plan(&spec, &options) {
        Ok(plan) => plan,
        Err(e) => return out.error(Exit::Failed, &format!("reading files: {e}")),
    };
    if plan.report.ok() {
        plan.report.callers = callers(&plan.report);
    }
    if !plan.report.ok() {
        let exit = if plan.report.only_missing_files() {
            Exit::NotFound
        } else {
            Exit::Precondition
        };
        print_report(out, &plan.report, None, None);
        return exit;
    }
    if args.dry_run {
        plan.report.dry_run = true;
        print_report(out, &plan.report, None, None);
        return Exit::Ok;
    }
    let settings = crate::settings::Settings::load();
    let after_edit = if args.diag || settings.edit.fix_after {
        let named: Vec<String> = plan
            .report
            .files
            .iter()
            .flat_map(|f| [Some(f.path.clone()), f.moved_to.clone()])
            .flatten()
            .collect();
        match diag_cmd::target(&named, false) {
            Ok(target) => Some(target),
            // Without --diag, a path the fixers cannot take (outside the
            // repository) only means nothing is formatted.
            Err(e) if args.diag => return out.error(Exit::Usage, &e),
            Err(_) => None,
        }
    } else {
        None
    };
    // --diag compares with the files as they are now, so that is checked
    // first; a check that cannot run refuses the edit before anything is
    // written.
    let before = match (&after_edit, args.diag) {
        (Some((root, files)), true) => {
            let existing: Vec<String> = files
                .iter()
                .filter(|f| root.join(f).is_file())
                .cloned()
                .collect();
            let report = baseline(root, existing);
            if matches!(report.status, Status::Refused | Status::Unavailable) {
                return out.error(
                    Exit::Precondition,
                    &format!(
                        "--diag cannot check these files, so nothing was written: {}",
                        report.reason.as_deref().unwrap_or("no checker")
                    ),
                );
            }
            Some(report)
        }
        _ => None,
    };
    let journal = match edit::write(&mut plan) {
        Ok(journal) => journal,
        Err(e) => {
            return out.error(
                Exit::Failed,
                &format!("writing failed, and the files already written were restored: {e}"),
            );
        }
    };
    let written = |root: &std::path::Path, files: &[String]| -> Vec<String> {
        files
            .iter()
            .filter(|f| root.join(f).is_file())
            .cloned()
            .collect()
    };
    let fixed = match &after_edit {
        Some((root, files)) if settings.edit.fix_after => {
            let steps = settings.edit.fix_steps.clone().unwrap_or_default();
            let timeout = std::time::Duration::from_millis(settings.edit.fix_timeout_ms);
            let fixed = diag::fix::run(root, &written(root, files), &steps, timeout);
            crate::ledger::record_fix(root, None, "cli", None, &fixed);
            Some(fixed)
        }
        _ => None,
    };
    let mut exit = Exit::Ok;
    let mut failed = false;
    if let Some(command) = &args.check {
        let check = match edit::run_check(command, &jouet_output::paths::log_dir()) {
            Ok(check) => check,
            Err(e) => {
                // The files are already written, so a check that cannot run
                // must still honour --revert-on-fail.
                let message = format!("running --check: {e}");
                if !args.revert_on_fail {
                    return out.error(Exit::Failed, &format!("{message}; the edits were kept"));
                }
                return match edit::revert(&journal) {
                    Ok(()) => out.error(
                        Exit::Failed,
                        &format!("{message}; every edited file was restored"),
                    ),
                    Err(r) => out.error(
                        Exit::Failed,
                        &format!("{message}, and restoring the files failed too: {r}"),
                    ),
                };
            }
        };
        failed |= !check.passed;
        plan.report.check = Some(check);
    }
    // A check after writing that cannot run leaves the edit unverified: exit
    // 4, and with --revert-on-fail the files are restored, as when --check
    // cannot start.
    let mut unverified = false;
    let diagnosed = match (&after_edit, &before) {
        (Some((root, files)), Some(before)) => {
            let after = diag_cmd::checked(root, written(root, files));
            let outcome = DiagOutcome::of(before, &after, &renames(root, &plan.report));
            match outcome.status {
                "introduced" => failed = true,
                "refused" | "unavailable" => {
                    exit = Exit::Precondition;
                    unverified = true;
                }
                _ => {}
            }
            Some(outcome)
        }
        _ => None,
    };
    if failed {
        exit = Exit::Failed;
    }
    if (failed || unverified) && args.revert_on_fail {
        if let Err(e) = edit::revert(&journal) {
            return out.error(
                Exit::Failed,
                &format!("the check failed and restoring the files failed too: {e}"),
            );
        }
        if let Some(check) = &mut plan.report.check {
            check.reverted = true;
        }
        plan.report.applied = false;
        plan.report.files_written = 0;
        for f in &mut plan.report.files {
            f.status = FileStatus::Reverted;
        }
    }
    print_report(out, &plan.report, fixed.as_ref(), diagnosed.as_ref());
    // Keep the code graph in step with what was written (or restored), when
    // the repository has one, as the post-tool-use hook does for file tools.
    if let Ok(cwd) = std::env::current_dir() {
        let touched: Vec<_> = plan
            .report
            .files
            .iter()
            .flat_map(|f| std::iter::once(&f.path).chain(&f.moved_to))
            .map(|p| cwd.join(p))
            .collect();
        super::graph::reindex(&cwd, &touched);
    }
    exit
}

/// What `--diag` found: the delta between the files before and after.
#[derive(Serialize)]
struct DiagOutcome {
    /// clean, introduced, refused or unavailable.
    status: &'static str,
    #[serde(flatten)]
    delta: delta::Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_path: Option<String>,
}

impl DiagOutcome {
    fn of(before: &diag::Report, after: &diag::Report, renamed: &[(String, String)]) -> Self {
        let (status, delta) = match after.status {
            Status::Refused => ("refused", delta::Delta::default()),
            Status::Unavailable => ("unavailable", delta::Delta::default()),
            Status::Clean | Status::Findings => {
                let d = delta::between(before, after, renamed);
                (
                    if d.introduced() {
                        "introduced"
                    } else {
                        "clean"
                    },
                    d,
                )
            }
        };
        Self {
            status,
            delta,
            reason: after.reason.clone(),
            log_path: after.log_path.clone(),
        }
    }
}

/// Each moved file's path before and after, relative to `root`.
fn renames(root: &std::path::Path, report: &Report) -> Vec<(String, String)> {
    let Ok(cwd) = std::env::current_dir().and_then(|d| d.canonicalize()) else {
        return Vec::new();
    };
    report
        .files
        .iter()
        .filter_map(|f| {
            let to = f.moved_to.as_ref()?;
            let pair = diag::resolve(root, &cwd, &[f.path.clone(), to.clone()]).ok()?;
            match pair.as_slice() {
                [from, to] => Some((from.clone(), to.clone())),
                _ => None,
            }
        })
        .collect()
}

/// The files' findings as they are now: from the cache when every file was
/// checked at exactly this content, else from a check.
fn baseline(root: &std::path::Path, files: Vec<String>) -> diag::Report {
    if let Some(path) = delta::cache_path(root) {
        let cache = delta::Cache::load(&path);
        let seen: Option<Vec<&delta::Seen>> = files
            .iter()
            .map(|f| cache.at(f, &delta::hash(root, f)?))
            .collect();
        if let Some(seen) = seen.filter(|s| !s.is_empty()) {
            return diag::Report::from_cache(files, &seen);
        }
    }
    diag_cmd::checked(root, files)
}

/// Finds definitions for symbol edits with tree-sitter.
#[derive(Debug)]
struct TreeSitter;

impl edit::Symbols for TreeSitter {
    fn locate(&self, path: &Path, text: &str, selector: &str) -> Result<Vec<Span>, SymbolError> {
        let selector = Selector::parse(selector).map_err(|e| match e {
            SelectError::Invalid(message) => SymbolError::Invalid(message),
            SelectError::Unsupported | SelectError::NotFound => SymbolError::Unsupported,
        })?;
        let describe = |l: &symbols::Located| {
            format!(
                "{} {} ({}-{})",
                l.def.kind,
                l.qualified(),
                l.def.start,
                l.def.end
            )
        };
        match symbols::select(path, text.as_bytes(), &selector) {
            Ok(found) => Ok(found
                .iter()
                .map(|l| Span {
                    start: l.start_byte,
                    end: l.end_byte,
                    line: l.def.line as usize,
                    describe: describe(l),
                })
                .collect()),
            Err(SelectError::NotFound) => Err(SymbolError::NotFound(
                symbols::outline(path, text.as_bytes())
                    .unwrap_or_default()
                    .iter()
                    .map(describe)
                    .collect(),
            )),
            Err(SelectError::Invalid(message)) => Err(SymbolError::Invalid(message)),
            Err(SelectError::Unsupported) => Err(SymbolError::Unsupported),
        }
    }

    fn syntax_errors(&self, path: &Path, text: &str) -> Option<usize> {
        symbols::syntax_errors(path, text.as_bytes())
    }
}

/// How many references to a changed signature are listed.
const CALLERS_SHOWN: usize = 10;

/// For each definition whose signature a symbol edit changed, its callers
/// in files the edit does not change: from the code graph when the work
/// tree has one, else references to its name matched by text, so some may
/// be another definition's.
fn callers(report: &Report) -> Vec<Callers> {
    let changed: Vec<(&str, &edit::SignatureChange)> = report
        .files
        .iter()
        .flat_map(|f| f.edits.iter().map(move |e| (f.path.as_str(), e)))
        .filter_map(|(path, e)| e.signature.as_ref().map(|s| (path, s)))
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = crate::git::toplevel().unwrap_or_else(|| cwd.clone());
    let touched: Vec<PathBuf> = report
        .files
        .iter()
        .filter_map(|f| std::fs::canonicalize(&f.path).ok())
        .collect();
    let cache = jouet_output::paths::cache_dir().map(|d| d.join("tags"));
    let shown = |full: PathBuf| {
        full.strip_prefix(&cwd)
            .map_or(full.clone(), Path::to_path_buf)
            .to_string_lossy()
            .into_owned()
    };
    // The code graph, when the work tree has one, knows the definition's
    // callers, not just lines that mention its name.
    let mut graph = jouet_graph::Graph::open_existing(&cwd).ok().flatten();
    let mut out = Vec::new();
    for (path, change) in changed {
        let callers = graph
            .as_mut()
            .and_then(|g| graph_callers(g, &cwd.join(path), change, &touched));
        if let Some(mut references) = callers {
            if references.is_empty() {
                continue;
            }
            for r in &mut references {
                r.path = shown(root.join(&r.path));
            }
            let more = references.len().saturating_sub(CALLERS_SHOWN);
            references.truncate(CALLERS_SHOWN);
            out.push(Callers {
                symbol: change.name.clone(),
                references,
                resolved: true,
                more,
            });
            continue;
        }
        let mut references: Vec<Reference> =
            jouet_search::map::references(&root, &change.name, cache.as_deref())
                .into_iter()
                .filter(|(rel, _)| {
                    std::fs::canonicalize(root.join(rel)).is_ok_and(|p| !touched.contains(&p))
                })
                .map(|(rel, line)| Reference {
                    path: shown(root.join(&rel)),
                    line: line as usize,
                })
                .collect();
        if references.is_empty() {
            continue;
        }
        let more = references.len().saturating_sub(CALLERS_SHOWN);
        references.truncate(CALLERS_SHOWN);
        out.push(Callers {
            symbol: change.name.clone(),
            references,
            resolved: false,
            more,
        });
    }
    out
}

/// The callers the code graph has for the definition a symbol edit
/// changes, outside the files the edit touches, as paths relative to the
/// work tree; `None` when the graph has no such definition, and the text
/// match stands in.
fn graph_callers(
    graph: &mut jouet_graph::Graph,
    path: &Path,
    change: &edit::SignatureChange,
    touched: &[PathBuf],
) -> Option<Vec<Reference>> {
    // The edit is not written yet, so this is the definition as it was.
    graph
        .reindex_paths(&[path.to_path_buf()], Some(CALLER_BUDGET))
        .ok()?;
    let symbol = graph.enclosing(path, change.line as u32).ok()??;
    if symbol.name != change.name {
        return None;
    }
    let root = graph.root().to_path_buf();
    let mut seen = std::collections::BTreeSet::new();
    Some(
        graph
            .callers(&symbol.id, 1)
            .ok()?
            .into_iter()
            .filter(|r| !touched.contains(&root.join(&r.symbol.path)))
            .map(|r| {
                (
                    r.symbol.path,
                    r.line.unwrap_or(r.symbol.start_line) as usize,
                )
            })
            .filter(|key| seen.insert(key.clone()))
            .map(|(path, line)| Reference { path, line })
            .collect(),
    )
}

/// How long finding a changed definition's callers may spend bringing the
/// graph up to date for its file.
const CALLER_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

fn read_spec(args: &EditArgs) -> Result<Spec, String> {
    let text = match args.spec.as_deref() {
        Some("-") | None => {
            if args.spec.is_none() && std::io::stdin().is_terminal() {
                return Err("no spec: pass --spec FILE or pipe one on stdin".into());
            }
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| format!("reading the spec from stdin: {e}"))?;
            text
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("reading the spec from {path}: {e}"))?,
    };
    let format = match args.format.as_deref() {
        Some(name) => Format::parse(name).ok_or_else(|| format!("--format must be blocks, patch or json, not `{name}`"))?,
        None => Format::detect(&text).ok_or(
            "could not tell the spec's format from its first line; start with `=== PATH`, `*** Begin Patch` or `[`, or pass --format",
        )?,
    };
    spec::parse(&text, format).map_err(|e| e.to_string())
}

fn replace_spec(args: &ReplaceArgs) -> Result<Spec, String> {
    if args.old.is_empty() {
        return Err("--old cannot be empty".into());
    }
    let count = match (&args.count, args.all) {
        (Some(_), true) => return Err("give --count or --all, not both".into()),
        (None, true) => Count::All,
        (None, false) => Count::Exactly(1),
        (Some(n), false) => match n.parse::<usize>() {
            Ok(n) if n > 0 => Count::Exactly(n),
            _ => return Err(format!("--count must be a positive number, not `{n}`")),
        },
    };
    Ok(Spec {
        ops: vec![FileOp::Update {
            path: args.path.clone(),
            edits: vec![Edit::Replace {
                old: args.old.clone(),
                new: args.new.clone(),
                count,
                line_aligned: false,
            }],
            move_to: None,
        }],
    })
}

fn run_hash(args: &HashArgs, out: &Output) -> Exit {
    if args.paths.is_empty() {
        return out.error(Exit::Usage, "give at least one file to hash");
    }
    let mut files = Vec::new();
    for path in &args.paths {
        match std::fs::read(path) {
            Ok(bytes) => {
                files.push(serde_json::json!({ "path": path, "sha256": edit::sha256_hex(&bytes) }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return out.error(Exit::NotFound, &format!("{path}: no such file"));
            }
            Err(e) => return out.error(Exit::Failed, &format!("{path}: {e}")),
        }
    }
    let doc = serde_json::json!({ "files": files });
    out.result(&doc, || {
        files
            .iter()
            .map(|f| {
                format!(
                    "{}  {}\n",
                    f["sha256"].as_str().unwrap_or(""),
                    f["path"].as_str().unwrap_or("")
                )
            })
            .collect()
    });
    Exit::Ok
}

#[derive(Serialize)]
struct EditOutput<'a> {
    #[serde(flatten)]
    report: &'a Report,
    /// What `[edit] fix_after` formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<&'a diag::fix::Fixed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diag: Option<&'a DiagOutcome>,
}

fn print_report(
    out: &Output,
    report: &Report,
    fix: Option<&diag::fix::Fixed>,
    diag: Option<&DiagOutcome>,
) {
    let doc = EditOutput { report, fix, diag };
    out.result(&doc, || {
        let mut text = render(report);
        if let Some(line) = fix.and_then(diag::fix::Fixed::summary) {
            let _ = writeln!(text, "{line}");
        }
        if let Some(d) = diag {
            text.push_str(&render_diag(d));
        }
        text
    });
}

fn render_diag(d: &DiagOutcome) -> String {
    let mut s = String::new();
    match d.status {
        "refused" | "unavailable" => {
            let _ = writeln!(
                s,
                "diag: could not check the edited files: {}",
                d.reason.as_deref().unwrap_or("")
            );
        }
        _ => {
            s.push_str(&diag_cmd::delta_lines(&d.delta, 0, d.log_path.as_deref()));
            let _ = writeln!(
                s,
                "diag: {} new, {} kept, {} fixed",
                d.delta.new.len(),
                d.delta.kept,
                d.delta.fixed
            );
        }
    }
    s
}

fn status_word(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Written => "ok",
        FileStatus::Created => "created",
        FileStatus::Deleted => "deleted",
        FileStatus::Moved => "moved",
        FileStatus::NotWritten => "not written",
        FileStatus::Failed => "failed",
        FileStatus::Reverted => "reverted",
    }
}

fn edit_problem(e: &edit::EditReport) -> String {
    let expected = match e.expected {
        Count::Exactly(n) => n.to_string(),
        Count::All => "at least 1".into(),
    };
    let lines = || {
        e.lines
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    match e.status {
        EditStatus::Ok => "ok".into(),
        EditStatus::NoMatch => match (&e.symbol, &e.near_miss) {
            (Some(symbol), _) => format!("no definition matches `{symbol}`"),
            (None, Some(NearMiss::InsideLine { lines, multiline })) => {
                let at = if *multiline {
                    format!("at {}, but starts or ends inside a line", line_list(lines))
                } else {
                    format!("inside {}", line_list(lines))
                };
                format!(
                    "no match as whole lines; the text occurs {at} (SEARCH blocks hold whole lines, or use `jouet edit replace` for text inside a line)"
                )
            }
            (None, Some(NearMiss::Whitespace { lines })) => format!(
                "no match; the text matches at {} once indentation and trailing whitespace are ignored (copy the file's whitespace exactly)",
                line_list(lines)
            ),
            (None, None) => "no match".into(),
        },
        EditStatus::TooManyMatches if e.symbol.is_some() => format!(
            "{} definitions match `{}`, at lines {}; add a kind or a scope, such as `fn download` or `Vfox::download`",
            e.found,
            e.symbol.as_deref().unwrap_or(""),
            lines()
        ),
        EditStatus::TooManyMatches => format!(
            "{} matches, expected {expected}, at lines {}",
            e.found,
            lines()
        ),
        EditStatus::TooFewMatches => format!(
            "{} matches, expected {expected}, at lines {}",
            e.found,
            lines()
        ),
        EditStatus::AnchorNotFound => {
            format!("anchor not found: {}", e.anchor.as_deref().unwrap_or(""))
        }
        EditStatus::Unsupported => format!(
            "cannot edit `{}` by symbol",
            e.symbol.as_deref().unwrap_or("")
        ),
        EditStatus::DoesNotParse => format!(
            "the new `{}` does not parse",
            e.symbol.as_deref().unwrap_or("")
        ),
    }
}

/// `line 13`, `lines 13 and 40`, or the first few and how many more.
fn line_list(lines: &[usize]) -> String {
    const SHOWN: usize = 5;
    let mut shown: Vec<String> = lines.iter().take(SHOWN).map(usize::to_string).collect();
    let more = lines.len().saturating_sub(SHOWN);
    if more > 0 {
        shown.push(format!("{more} more"));
    }
    match shown.as_slice() {
        [one] => format!("line {one}"),
        [rest @ .., last] => format!("lines {} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// The text form of a report: one line per file, then the problems under it,
/// then a summary line.
fn render(report: &Report) -> String {
    let mut s = String::new();
    let width = report.files.iter().map(|f| f.path.len()).max().unwrap_or(0);
    for f in &report.files {
        let edits = match f.edits.len() {
            0 => String::new(),
            1 => "1 edit".into(),
            n => format!("{n} edits"),
        };
        let mut line = format!("{:<11} {:<width$}", status_word(f.status), f.path);
        if let Some(to) = &f.moved_to {
            let _ = write!(line, " -> {to}");
        }
        if !edits.is_empty() {
            let _ = write!(line, "  {edits}");
        }
        if f.added + f.removed > 0 {
            let _ = write!(line, "  +{} -{}", f.added, f.removed);
        }
        s.push_str(line.trim_end());
        s.push('\n');
        if let Some(err) = &f.error {
            let _ = writeln!(s, "  {}", err.message);
        }
        for e in f.edits.iter().filter(|e| e.status != EditStatus::Ok) {
            let _ = writeln!(s, "  edit {}: {}", e.index + 1, edit_problem(e));
            if let Some(message) = &e.message {
                let _ = writeln!(s, "    {message}");
            }
            if !e.candidates.is_empty() {
                let what = if e.status == EditStatus::NoMatch {
                    "the file defines"
                } else {
                    "it matches"
                };
                let _ = writeln!(s, "    {what}:");
                for c in &e.candidates {
                    let _ = writeln!(s, "      {c}");
                }
            }
            if let Some(c) = &e.closest {
                let _ = writeln!(
                    s,
                    "    closest at line {} ({:.0}% similar), - spec / + file:",
                    c.line,
                    c.similarity * 100.0
                );
                for l in c.diff.lines() {
                    let _ = writeln!(s, "    {l}");
                }
            }
        }
        if let Some(diff) = &f.diff {
            s.push_str(diff);
        }
    }
    if let Some(check) = &report.check {
        let code = check
            .exit_code
            .map_or("a signal".to_string(), |c| format!("exit {c}"));
        if check.passed {
            let _ = writeln!(s, "check passed: {}", check.command);
        } else {
            let _ = writeln!(s, "check failed ({code}): {}", check.command);
            if check.output_truncated {
                s.push_str("  ...\n");
            }
            for l in &check.output {
                let _ = writeln!(s, "  {l}");
            }
        }
        let _ = writeln!(s, "  full log: {}", check.log_path);
        if check.reverted {
            s.push_str("  every edited file was restored\n");
        }
    }
    for c in &report.callers {
        let total = c.references.len() + c.more;
        let found = match (c.resolved, total == 1) {
            (true, true) => "caller",
            (true, false) => "callers",
            (false, true) => "reference to it",
            (false, false) => "references to it",
        };
        let _ = writeln!(
            s,
            "warning: the signature of {} changed; {total} {found} in files this edit does not change may need updating:",
            c.symbol,
        );
        for r in &c.references {
            let _ = writeln!(s, "  {}:{}", r.path, r.line);
        }
        if c.more > 0 {
            let _ = writeln!(s, "  ... {} more", c.more);
        }
    }
    let total = report.files.len();
    if report.dry_run {
        let _ = writeln!(s, "dry run: {total} {} would be written", plural(total));
    } else {
        let _ = writeln!(
            s,
            "{} of {total} {} written",
            report.files_written,
            plural(total)
        );
    }
    s
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "file" } else { "files" }
}
