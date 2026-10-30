//! `jouet commit`.

use super::Output;
use crate::CommitArgs;
use crate::config::Config;
use jouet_git::Git;
use jouet_git::agent::Agent;
use jouet_git::commit::{self, Report, Request};
use jouet_output::exit::Exit;
use std::fmt::Write as _;
use std::io::Read as _;

pub(crate) fn run(args: &CommitArgs, out: &Output) -> Exit {
    let req = match request(args) {
        Ok(req) => req,
        Err((message, hint)) => return out.error_with_hint(Exit::Usage, &message, hint),
    };
    let report = match commit::run(&Git::here(), &req) {
        Ok(report) => report,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    if !out.json {
        for w in &report.warnings {
            eprintln!("jouet: warning: {w}");
        }
    }
    out.result(&report, || render(&report));
    if report.failure.is_some() {
        Exit::Failed
    } else {
        Exit::Ok
    }
}

type Refusal = (String, Option<&'static str>);

fn request(args: &CommitArgs) -> Result<Request, Refusal> {
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
    Ok(Request {
        paths: args.paths.clone(),
        message,
        trailers,
        conventional,
        types: config.commit_types(),
        dry_run: args.dry_run,
        log_dir: jouet_output::paths::log_dir(),
    })
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
    if report.dry_run {
        s.push_str("message:\n");
        for line in report.message.lines() {
            let _ = writeln!(s, "{}", format!("  {line}").trim_end());
        }
    }
    s
}
