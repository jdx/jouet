//! `jouet edit`, `jouet edit replace` and `jouet edit hash`.

use super::Output;
use crate::{EditArgs, EditCmd, HashArgs, ReplaceArgs};
use jouet_edit::spec::{self, Count, Edit, FileOp, Format, Spec};
use jouet_edit::{self as edit, EditStatus, FileStatus, Options, Report, Span, SymbolError};
use jouet_output::exit::Exit;
use jouet_search::structural::Structural;
use jouet_search::symbols::{self, SelectError, Selector};
use std::fmt::Write as _;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

pub(crate) fn run(args: EditArgs, out: &Output) -> Exit {
    if args.pattern.is_none()
        && (!args.paths.is_empty() || args.rewrite.is_some() || args.lang.is_some())
    {
        return out.error(
            Exit::Usage,
            "paths, --rewrite and --lang go with --pattern; a spec names its own files",
        );
    }
    let spec = match &args.cmd {
        Some(EditCmd::Hash(hash)) => return run_hash(hash, out),
        Some(EditCmd::Replace(replace)) => replace_spec(replace),
        None if args.pattern.is_some() => match pattern_spec(&args) {
            Ok(spec) => Ok(spec),
            Err((exit, message)) => return out.error(exit, &message),
        },
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
    if args.revert_on_fail && args.check.is_none() {
        return out.error(Exit::Usage, "--revert-on-fail needs --check");
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

    if !plan.report.ok() {
        let exit = if plan.report.only_missing_files() {
            Exit::NotFound
        } else {
            Exit::Precondition
        };
        print_report(out, &plan.report);
        return exit;
    }
    if args.dry_run {
        plan.report.dry_run = true;
        print_report(out, &plan.report);
        return Exit::Ok;
    }
    let journal = match edit::write(&mut plan) {
        Ok(journal) => journal,
        Err(e) => {
            return out.error(
                Exit::Failed,
                &format!("writing failed, and the files already written were restored: {e}"),
            );
        }
    };
    let mut exit = Exit::Ok;
    if let Some(command) = &args.check {
        let mut check = match edit::run_check(command, &jouet_output::paths::log_dir()) {
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
        if !check.passed {
            exit = Exit::Failed;
            if args.revert_on_fail {
                if let Err(e) = edit::revert(&journal) {
                    return out.error(
                        Exit::Failed,
                        &format!("the check failed and restoring the files failed too: {e}"),
                    );
                }
                check.reverted = true;
                plan.report.applied = false;
                plan.report.files_written = 0;
                for f in &mut plan.report.files {
                    f.status = FileStatus::Reverted;
                }
            }
        }
        plan.report.check = Some(check);
    }
    print_report(out, &plan.report);
    exit
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

/// `--pattern`, `--rewrite` and `--lang`: every match of the pattern under
/// the paths, rewritten, as one edit per file.
fn pattern_spec(args: &EditArgs) -> Result<Spec, (Exit, String)> {
    let usage = |m: &str| (Exit::Usage, m.to_string());
    let pattern = args.pattern.as_deref().unwrap_or_default();
    let (Some(rewrite), Some(lang)) = (args.rewrite.as_deref(), args.lang.as_deref()) else {
        return Err(usage("--pattern needs --rewrite and --lang"));
    };
    if args.spec.is_some() || args.format.is_some() {
        return Err(usage(
            "--pattern rewrites code itself; it takes no --spec or --format",
        ));
    }
    let structural =
        Structural::new(pattern, lang, Some(rewrite)).map_err(|e| usage(&e.to_string()))?;
    let paths: Vec<PathBuf> = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.iter().map(PathBuf::from).collect()
    };
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
        return Err((
            Exit::NotFound,
            format!("{}: no such file or directory", missing.display()),
        ));
    }
    let (files, read) = structural.search_files(&paths);
    if files.is_empty() {
        return Err((
            Exit::Precondition,
            format!(
                "the pattern matched nothing in {read} {lang} {}; nothing was written (`jouet agent-help edit` has pattern examples)",
                if read == 1 { "file" } else { "files" }
            ),
        ));
    }
    let ops = files
        .into_iter()
        .map(|f| FileOp::Update {
            path: f.path.to_string_lossy().into_owned(),
            edits: vec![Edit::Spans {
                spans: f
                    .matches
                    .into_iter()
                    .map(|m| (m.start, m.end, m.replacement.unwrap_or_default()))
                    .collect(),
                original: f.source,
            }],
            move_to: None,
        })
        .collect();
    Ok(Spec { ops })
}

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

fn print_report(out: &Output, report: &Report) {
    out.result(report, || render(report));
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
        EditStatus::NoMatch => match &e.symbol {
            Some(symbol) => format!("no definition matches `{symbol}`"),
            None => "no match".into(),
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
    for change in report
        .files
        .iter()
        .flat_map(|f| &f.edits)
        .filter_map(|e| e.signature.as_ref())
    {
        let _ = writeln!(
            s,
            "note: the signature of {} changed; check its callers",
            change.name
        );
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
