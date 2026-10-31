//! `jouet graph`: the code graph in `jouet_graph`.

use super::Output;
use crate::{GraphArgs, GraphCmd, GraphIndexArgs};
use jouet_graph::{Graph, Refresh, Status};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a hook or `jouet edit` may spend re-indexing what it just
/// wrote. Files left over stay stale until the next query's refresh.
const AFTER_EDIT_BUDGET: Duration = Duration::from_millis(250);

pub(crate) fn run(args: &GraphArgs, out: &Output) -> Exit {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Precondition, &format!("no current directory: {e}")),
    };
    let mut graph = match Graph::open(&cwd) {
        Ok(g) => g,
        Err(e) => return out.error(Exit::Precondition, &e.to_string()),
    };
    match &args.cmd {
        GraphCmd::Index(a) => index(&mut graph, a, out),
        GraphCmd::Status(_) => status(&graph, out),
    }
}

#[derive(Serialize)]
struct IndexReport<'a> {
    db: String,
    #[serde(flatten)]
    refresh: &'a Refresh,
    #[serde(flatten)]
    status: &'a Status,
}

fn index(graph: &mut Graph, args: &GraphIndexArgs, out: &Output) -> Exit {
    let refresh = if args.full {
        graph.rebuild()
    } else {
        graph.refresh(None)
    };
    let refresh = match refresh {
        Ok(r) => r,
        Err(e) => return out.error(Exit::Failed, &e.to_string()),
    };
    let status = match graph.status() {
        Ok(s) => s,
        Err(e) => return out.error(Exit::Failed, &e.to_string()),
    };
    let report = IndexReport {
        db: graph.db_path().display().to_string(),
        refresh: &refresh,
        status: &status,
    };
    out.result(&report, || {
        let mut s = format!(
            "indexed {} of {} files in {} ms ({} unchanged",
            refresh.indexed,
            refresh.checked,
            refresh.elapsed.as_millis(),
            refresh.checked.saturating_sub(refresh.indexed),
        );
        if refresh.removed > 0 {
            let _ = write!(s, ", {} removed", refresh.removed);
        }
        if refresh.skipped > 0 {
            let _ = write!(s, ", {} too large or not UTF-8", refresh.skipped);
        }
        s.push_str(")\n");
        for (path, why) in &refresh.failed {
            let _ = writeln!(s, "  failed: {path}: {why}");
        }
        let _ = writeln!(
            s,
            "{} symbols, {} edges, {} references; {} at {}",
            status.nodes,
            status.edges,
            status.refs,
            super::audit::bytes(status.db_bytes),
            report.db,
        );
        s
    });
    Exit::Ok
}

#[derive(Serialize)]
struct StatusReport<'a> {
    root: String,
    db: String,
    #[serde(flatten)]
    status: &'a Status,
}

fn status(graph: &Graph, out: &Output) -> Exit {
    let status = match graph.status() {
        Ok(s) => s,
        Err(e) => return out.error(Exit::Failed, &e.to_string()),
    };
    let report = StatusReport {
        root: graph.root().display().to_string(),
        db: graph.db_path().display().to_string(),
        status: &status,
    };
    out.result(&report, || {
        let mut s = format!(
            "{} files, {} symbols, {} edges, {} references\n",
            status.files, status.nodes, status.edges, status.refs
        );
        if status.files_with_errors > 0 {
            let _ = writeln!(
                s,
                "{} files had parse errors (indexed as tree-sitter recovered them)",
                status.files_with_errors
            );
        }
        if status.files_without_symbols > 0 {
            let _ = writeln!(
                s,
                "{} files recorded without symbols",
                status.files_without_symbols
            );
        }
        let _ = writeln!(
            s,
            "database: {} ({}), outside the repository",
            report.db,
            super::audit::bytes(status.db_bytes)
        );
        s
    });
    Exit::Ok
}

/// The files a finished tool call wrote: a Claude Code file tool's path, or
/// the files a Codex patch adds, updates, deletes or moves.
fn edited_paths(payload: &serde_json::Value, call: &jouet_rules::Call) -> Vec<PathBuf> {
    if matches!(
        call.tool.as_str(),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit"
    ) {
        return call.path.iter().map(PathBuf::from).collect();
    }
    let Some(input) = payload.get("tool_input") else {
        return Vec::new();
    };
    let texts: Vec<&str> = match input {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Object(o) => o.values().filter_map(|v| v.as_str()).collect(),
        _ => Vec::new(),
    };
    texts
        .into_iter()
        .filter(|t| t.contains("*** Begin Patch"))
        .flat_map(patch_paths)
        .collect()
}

/// The paths named by a Codex patch's file headers.
fn patch_paths(patch: &str) -> Vec<PathBuf> {
    patch
        .lines()
        .filter_map(|line| {
            [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(|p| PathBuf::from(p.trim()))
        .collect()
}

/// After a file tool or a patch, re-indexes what it wrote, when the work
/// tree already has a graph. Best effort, within [`AFTER_EDIT_BUDGET`]: an
/// error or a file left over only leaves the graph stale until the next
/// query refreshes it.
pub(crate) fn reindex_after_tool(payload: &serde_json::Value, call: &jouet_rules::Call) {
    if call.exit_code.is_some_and(|c| c != 0) {
        return;
    }
    let Some(cwd) = call.cwd.as_deref().map(Path::new) else {
        return;
    };
    let paths: Vec<PathBuf> = edited_paths(payload, call)
        .into_iter()
        .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
        .collect();
    reindex(cwd, &paths);
}

/// Re-indexes files jouet itself just wrote, when the work tree already has
/// a graph. Best effort, like [`reindex_after_tool`].
pub(crate) fn reindex(cwd: &Path, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    if let Ok(Some(mut graph)) = Graph::open_existing(cwd) {
        let _ = graph.reindex_paths(paths, Some(AFTER_EDIT_BUDGET));
    }
}
