//! `jouet commit`.

use super::Output;
use super::secrets::{self, CommitScan, Fnox};
use crate::cli::CommitArgs;
use crate::config::Config;
use crate::fnox;
use jouet_git::Git;
use jouet_git::agent::Agent;
use jouet_git::commit::{self, Report, Request, ScanStatus, Scanner};
use jouet_output::exit::Exit;
use std::fmt::Write as _;
use std::io::Read as _;

pub(crate) fn run(args: &CommitArgs, out: &Output) -> Exit {
    let (req, secret_scan) = match request(args) {
        Ok(req) => req,
        Err((message, hint)) => return out.error_with_hint(Exit::Usage, &message, hint),
    };
    let mut allow = Vec::new();
    for given in &args.allow_secret {
        match secrets::allowance(given) {
            Ok(a) => allow.push(a),
            Err(message) => return out.error(Exit::Usage, &message),
        }
    }
    // gitleaks' rules scan every commit; a fnox project's commits are also
    // scanned for its secrets' values, when fnox is here to resolve them.
    let fnox = if fnox::Project::find(std::path::Path::new(".")).is_none() {
        Fnox::No
    } else if fnox::installed() {
        Fnox::Scan(fnox::Scan { dir: ".".into() })
    } else {
        Fnox::Missing
    };
    let scan = CommitScan { allow, fnox };
    let scanner: Option<&dyn Scanner> = secret_scan.then_some(&scan as &dyn Scanner);
    let mut report = match commit::run_scanned(&Git::here(), &req, scanner) {
        Ok(report) => report,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    if let Some(reason) = report.secret_scan.as_ref().and_then(|s| s.reason.clone()) {
        report.warnings.push(reason);
    }
    if !out.json {
        for w in &report.warnings {
            eprintln!("jouet: warning: {w}");
        }
    }
    out.result(&report, || render(&report));
    if report.failure.is_some() || report.secret_found() {
        Exit::Failed
    } else {
        Exit::Ok
    }
}

type Refusal = (String, Option<&'static str>);

/// The request, and whether a fnox project's commit is to be scanned.
fn request(args: &CommitArgs) -> Result<(Request, bool), Refusal> {
    if args.all {
        return Err((
            "jouet commit does not take -A or --all".into(),
            Some("name the paths to commit, so nothing else in the checkout is swept up"),
        ));
    }
    if args.no_verify {
        return Err((
            "jouet commit does not take --no-verify: hooks always run".into(),
            Some("fix what the hook reports, then commit again"),
        ));
    }
    if args.paths.is_empty() {
        return Err((
            "name the paths to commit".into(),
            Some("jouet commit src/a.rs src/b.rs -m MSG"),
        ));
    }
    let message = match (&args.file, args.message.is_empty()) {
        (Some(_), false) => return Err(("give -m or -F, not both".into(), None)),
        (Some(file), true) => read_message(file).map_err(|e| (e, None))?,
        (None, false) => args.message.join("\n\n"),
        (None, true) => return Err(("give the message with -m MSG or -F FILE".into(), None)),
    };
    let config = Config::load().map_err(|e| (e, None))?;
    let conventional = config.commit_conventional().map_err(|e| (e, None))?;
    let agent = Agent::from_env();
    let mut trailers = if args.no_trailers {
        Vec::new()
    } else {
        config.commit_trailers(&agent)
    };
    trailers.extend(args.trailer.iter().map(|t| agent.expand(t)));
    let secret_scan = config.commit_secret_scan().map_err(|e| (e, None))?;
    let req = Request {
        paths: args.paths.clone(),
        message,
        trailers,
        conventional,
        types: config.commit_types(),
        dry_run: args.dry_run,
        log_dir: jouet_output::paths::log_dir(),
    };
    Ok((req, secret_scan))
}

fn read_message(file: &str) -> Result<String, String> {
    if file == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("reading the message from stdin: {e}"))?;
        return Ok(text);
    }
    std::fs::read_to_string(file).map_err(|e| format!("reading the message from {file}: {e}"))
}

/// How many paths left uncommitted a commit names.
const LEFT_SHOWN: usize = 8;

fn render(report: &Report) -> String {
    let mut s = String::new();
    let on = report
        .branch
        .as_deref()
        .map_or(String::new(), |b| format!(" on {b}"));
    if let Some(failure) = &report.failure {
        let code = failure
            .exit_code
            .map_or("a signal".to_string(), |c| format!("exit {c}"));
        let _ = writeln!(
            s,
            "commit failed ({code}); nothing was committed and nothing was left staged"
        );
        if failure.lines_omitted > 0 {
            let _ = writeln!(s, "  ... {} earlier lines", failure.lines_omitted);
        }
        for line in &failure.output {
            let _ = writeln!(s, "  {line}");
        }
        let _ = writeln!(s, "  full log: {}", failure.log_path);
        return s;
    }
    if let Some(scan) = report
        .secret_scan
        .as_ref()
        .filter(|_| report.secret_found())
    {
        let values = scan.findings.iter().any(|f| f.rule.is_none());
        let rules = scan.findings.iter().any(|f| f.rule.is_some());
        let what = match (rules, values) {
            (true, true) => "gitleaks' rules and fnox found secrets",
            (true, false) => "gitleaks' rules found a secret",
            _ => "fnox found the value of a secret",
        };
        let _ = writeln!(
            s,
            "commit refused: {what} in what would be committed; nothing was committed and nothing was left staged"
        );
        let name =
            |f: &jouet_git::commit::SecretFinding| f.rule.clone().unwrap_or(f.secret.clone());
        let width = scan
            .findings
            .iter()
            .map(|f| name(f).len())
            .max()
            .unwrap_or(0);
        let at = |f: &jouet_git::commit::SecretFinding| format!("{}:{}", f.path, f.line);
        let at_width = scan.findings.iter().map(|f| at(f).len()).max().unwrap_or(0);
        for f in &scan.findings {
            let at = at(f);
            let line = match &f.redacted {
                Some(r) => format!("  {:<width$}  {at:<at_width$}  {r}", name(f)),
                None => format!("  {:<width$}  {at}", name(f)),
            };
            let _ = writeln!(s, "{}", line.trim_end());
        }
        if values {
            s.push_str(
                "  remove the value from those files; read it from the environment, with `fnox exec -- CMD`, instead\n",
            );
        }
        if rules {
            s.push_str(
                "  remove the secret and read it from the environment instead; for a false positive, commit again with --allow-secret RULE:PATH:LINE for each, or add `gitleaks:allow` to its line\n",
            );
            let ignores: Vec<String> = scan
                .findings
                .iter()
                .filter_map(|f| Some(format!("{}:{}:{}", f.path, f.rule.as_ref()?, f.line)))
                .collect();
            let _ = writeln!(
                s,
                "  or, to pass over it for good, add to .gitleaksignore: {}",
                ignores.join(" ")
            );
        }
        return s;
    }
    if report.dry_run {
        let _ = writeln!(
            s,
            "dry run: would commit {} {}{on}",
            report.files.len(),
            if report.files.len() == 1 {
                "file"
            } else {
                "files"
            }
        );
    } else {
        let sha = report.sha.as_deref().unwrap_or_default();
        let _ = writeln!(
            s,
            "committed {}{on}: {}",
            sha.get(..12).unwrap_or(sha),
            report.subject
        );
    }
    let width = report.files.iter().map(|f| f.path.len()).max().unwrap_or(0);
    for f in &report.files {
        let mut line = format!("  {:<9} {:<width$}", f.status, f.path);
        if let (Some(a), Some(r)) = (f.added, f.removed) {
            let _ = write!(line, "  +{a} -{r}");
        }
        let _ = writeln!(s, "{}", line.trim_end());
    }
    if let Some(scan) = report
        .secret_scan
        .as_ref()
        .filter(|s| s.findings.is_empty() && s.status != ScanStatus::Found)
    {
        let files = scan.files_scanned.map_or(String::new(), |n| {
            format!(" in {n} {}", if n == 1 { "file" } else { "files" })
        });
        // A fnox scan that could not run checked nothing: its warning says why.
        let by = if scan.tool.contains("fnox") && scan.status == ScanStatus::Clean {
            "gitleaks' rules or fnox"
        } else {
            "gitleaks' rules"
        };
        let _ = writeln!(s, "  no secrets found by {by}{files}");
    }
    for note in report.secret_scan.iter().flat_map(|s| &s.notes) {
        let _ = writeln!(s, "  {note}");
    }
    // What `git status` would say now, so a check of it is not needed.
    match report.left.as_deref() {
        Some([]) => {
            let _ = writeln!(s, "  nothing else is uncommitted");
        }
        Some(left) => {
            let shown: Vec<&str> = left.iter().take(LEFT_SHOWN).map(String::as_str).collect();
            let more = left.len() - shown.len();
            let more = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            let _ = writeln!(s, "  still uncommitted: {}{more}", shown.join(", "));
        }
        None => {}
    }
    if report.dry_run {
        s.push_str("message:\n");
        for line in report.message.lines() {
            let _ = writeln!(s, "{}", format!("  {line}").trim_end());
        }
    }
    s
}
