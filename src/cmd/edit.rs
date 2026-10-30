//! `jouet edit`, `jouet edit replace` and `jouet edit hash`.

use super::Output;
use crate::edit::spec::{self, Count, Edit, FileOp, Format, Spec};
use crate::edit::{self, EditStatus, FileStatus, Options, Report};
use crate::exit::Exit;
use crate::{EditArgs, EditCmd, HashArgs, ReplaceArgs};
use std::fmt::Write as _;
use std::io::{IsTerminal, Read};

pub fn run(args: EditArgs, out: &Output) -> Exit {
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
    if args.revert_on_fail && args.check.is_none() {
        return out.error(Exit::Usage, "--revert-on-fail needs --check");
    }
    let options = Options {
        expect_hashes,
        diff: args.diff,
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
        match edit::run_check(command, &crate::paths::log_dir()) {
            Ok(mut check) => {
                if !check.passed {
                    exit = Exit::Failed;
                    if args.revert_on_fail {
                        if let Err(e) = edit::revert(&journal) {
                            return out.error(
                                Exit::Failed,
                                &format!(
                                    "the check failed and restoring the files failed too: {e}"
                                ),
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
            Err(e) => return out.error(Exit::Failed, &format!("running --check: {e}")),
        }
    }
    print_report(out, &plan.report);
    exit
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
        EditStatus::NoMatch => "no match".into(),
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
