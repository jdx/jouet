//! `jouet commit`.

use super::Output;
use crate::CommitArgs;
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
    // A fnox project's commits are scanned for its secrets' values, when
    // fnox is here to resolve them.
    let fnox_project = secret_scan && fnox::Project::find(std::path::Path::new(".")).is_some();
    let fnox_installed = fnox_project && fnox::installed();
    let scan = fnox::Scan { dir: ".".into() };
    let scanner: Option<&dyn Scanner> = fnox_installed.then_some(&scan as &dyn Scanner);
    let mut report = match commit::run_scanned(&Git::here(), &req, scanner) {
        Ok(report) => report,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    if fnox_project && !fnox_installed && !req.dry_run {
        report.secret_scan = Some(fnox::skipped_scan());
    }
    if let Some(scan) = &report.secret_scan
        && scan.status == ScanStatus::Skipped
        && let Some(reason) = &scan.reason
    {
        report.warnings.push(reason.clone());
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
        let _ = writeln!(
            s,
            "commit refused: {} found the value of a secret in what would be committed; nothing was committed and nothing was left staged",
            scan.tool
        );
        let width = scan
            .findings
            .iter()
            .map(|f| f.secret.len())
            .max()
            .unwrap_or(0);
        for f in &scan.findings {
            let _ = writeln!(s, "  {:<width$}  {}:{}", f.secret, f.path, f.line);
        }
        s.push_str(
            "  remove the value from those files; read it from the environment, with `fnox exec -- CMD`, instead\n",
        );
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
        .filter(|s| s.status == ScanStatus::Clean)
    {
        let files = scan.files_scanned.map_or(String::new(), |n| {
            format!(" in {n} {}", if n == 1 { "file" } else { "files" })
        });
        let _ = writeln!(s, "  {} found no secret values{files}", scan.tool);
    }
    for note in report.secret_scan.iter().flat_map(|s| &s.notes) {
        let _ = writeln!(s, "  fnox: {note}");
    }
    if report.dry_run {
        s.push_str("message:\n");
        for line in report.message.lines() {
            let _ = writeln!(s, "{}", format!("  {line}").trim_end());
        }
    }
    s
}
