//! `jouet search`: budgeted search with session memory.

use super::Output;
use crate::SearchArgs;
use jouet_output::exit::Exit;
use jouet_search::memory::Memory;
use jouet_search::search::{self, Budget, Error, Options};
use std::path::PathBuf;

/// The default total budget in bytes: about 1,500 tokens.
const BUDGET: usize = 6000;
/// The default budget for one file.
const FILE_BUDGET: usize = 1500;

pub(crate) fn run(args: &SearchArgs, out: &Output) -> Exit {
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
    out.result(&report, || search::render(&report, args.files_with_matches));
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
