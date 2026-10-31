//! `jouet search`: budgeted search with session memory.

use super::Output;
use super::scope::{How, Scope};
use crate::cli::SearchArgs;
use jouet_output::exit::Exit;
use jouet_search::memory::Memory;
use jouet_search::search::{self, Budget, Error, Options};
use std::path::PathBuf;
use std::sync::Arc;

/// The default total budget in bytes: about 1,500 tokens.
const BUDGET: usize = 6000;
/// The default budget for one file.
const FILE_BUDGET: usize = 1500;

pub(crate) fn run(args: &SearchArgs, out: &Output) -> Exit {
    if args.structural != args.lang.is_some() {
        return out.error(Exit::Usage, "--structural and --lang go together");
    }
    if args.structural && (args.fixed_strings || args.ignore_case || args.word_regexp) {
        return out.error(
            Exit::Usage,
            "-F, -i and -w are for text patterns, not --structural ones",
        );
    }
    let how = match How::from_flags(args.callers_of.as_deref(), args.impact.as_deref()) {
        Ok(how) => how,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    if how.is_some() && !args.structural {
        return out.error(
            Exit::Usage,
            "--callers-of and --impact go with --structural",
        );
    }
    // The graph is consulted before the walk, and dropped before it: the
    // filter holds only what it read.
    let scope = match how.map(|(how, symbol)| Scope::open(how, symbol)) {
        Some(Ok((_, scope))) => Some(Arc::new(scope)),
        Some(Err((exit, message))) => return out.error(exit, &message),
        None => None,
    };
    let options = Options {
        pattern: args.pattern.clone(),
        paths: args.paths.iter().map(PathBuf::from).collect(),
        fixed_strings: args.fixed_strings,
        ignore_case: args.ignore_case,
        word: args.word_regexp,
        hidden: args.hidden,
        globs: args.glob.clone(),
        files_only: args.files_with_matches,
        budget: Budget {
            total: args.budget.unwrap_or(BUDGET),
            per_file: args.file_budget.unwrap_or(FILE_BUDGET),
        },
        cache: jouet_output::paths::cache_dir().map(|d| d.join("defs")),
        structural: args.lang.clone(),
        filter: scope
            .clone()
            .map(|s| s as Arc<dyn jouet_search::structural::Filter>),
    };
    let mut memory = if args.again || args.files_with_matches {
        Memory::none()
    } else {
        session_memory()
    };
    let report = match search::run(&options, &mut memory) {
        Ok(report) => report,
        Err(e @ Error::Usage(_)) => return out.error(Exit::Usage, &e.to_string()),
        Err(e @ Error::NotFound(_)) => return out.error(Exit::NotFound, &e.to_string()),
    };
    memory.save();
    drop(options);
    let scope = scope.and_then(Arc::into_inner).map(Scope::finish);
    #[derive(serde::Serialize)]
    struct Scoped<'a> {
        #[serde(flatten)]
        report: &'a search::Report,
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<&'a super::scope::Report>,
    }
    let doc = Scoped {
        report: &report,
        scope: scope.as_ref(),
    };
    out.result(&doc, || {
        let mut text = search::render(&report, args.files_with_matches);
        if let Some(scope) = &scope {
            text.push_str(&super::scope::render(scope));
        }
        text
    });
    if report.matches == 0 {
        Exit::Failed
    } else {
        Exit::Ok
    }
}

/// The agent session's id: `JOUET_SESSION_ID`, else the one Claude Code or
/// Codex sets for the commands it runs.
pub(crate) fn session_id() -> Option<String> {
    [
        "JOUET_SESSION_ID",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
    ]
    .iter()
    .filter_map(|name| std::env::var(name).ok())
    .find(|v| !v.trim().is_empty())
}

/// What this session has been shown, kept where the hooks keep it too.
pub(crate) fn session_memory() -> Memory {
    match (seen_dir(), session_id()) {
        (Some(dir), Some(id)) => Memory::open(&dir, &id),
        _ => Memory::none(),
    }
}

/// `<state dir>/seen`.
pub(crate) fn seen_dir() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("seen"))
}
