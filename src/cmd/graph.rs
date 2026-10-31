//! `jouet graph`: the code graph in `jouet_graph`.

use super::Output;
use crate::{
    GraphAffectedArgs, GraphArgs, GraphCmd, GraphExploreArgs, GraphIndexArgs, GraphSymbolsArgs,
    GraphTraverseArgs,
};
use jouet_graph::{Explored, Graph, MAX_RESULTS, Reach, Refresh, Status, Symbol, SymbolQuery};
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
    if !matches!(args.cmd, GraphCmd::Index(_) | GraphCmd::Status(_)) {
        // Every query first brings the graph up to date: only files whose
        // size or mtime changed are read, so this is cheap once built.
        if let Err(e) = graph.refresh(None) {
            return out.error(Exit::Failed, &e.to_string());
        }
    }
    let result = match &args.cmd {
        GraphCmd::Index(a) => return index(&mut graph, a, out),
        GraphCmd::Status(_) => return status(&graph, out),
        GraphCmd::Symbols(a) => symbols(&graph, a, out),
        GraphCmd::Callers(a) => traverse(&graph, a, Direction::Callers, out),
        GraphCmd::Callees(a) => traverse(&graph, a, Direction::Callees, out),
        GraphCmd::Impact(a) => traverse(&graph, a, Direction::Impact, out),
        GraphCmd::Explore(a) => explore(&graph, a, out),
        GraphCmd::Affected(a) => affected(&graph, a, out),
    };
    result.unwrap_or_else(|e| out.error(Exit::Failed, &e.to_string()))
}

/// The default byte budget for an answer, as `jouet search` uses: about
/// 1,500 tokens.
const BUDGET: usize = 6000;

/// The rendered entries of one page, and how many entries the whole
/// answer has.
struct Paged {
    /// 1-based.
    page: usize,
    pages: usize,
    /// Entry indexes on this page.
    range: std::ops::Range<usize>,
    total: usize,
}

/// Splits entries, each `sizes[i]` bytes when printed, into pages of at
/// most `budget` bytes (each page holds at least one entry), and picks
/// `page`. Deterministic, so `--page 2` continues exactly where page 1 cut.
fn paginate(sizes: &[usize], budget: usize, page: usize) -> Paged {
    let budget = if budget == 0 { usize::MAX } else { budget };
    let mut starts = vec![0];
    let mut used = 0usize;
    for (i, &size) in sizes.iter().enumerate() {
        if used > 0 && used.saturating_add(size) > budget {
            starts.push(i);
            used = 0;
        }
        used = used.saturating_add(size);
    }
    let pages = starts.len();
    let page = page.clamp(1, pages);
    let start = starts[page - 1];
    let end = starts.get(page).copied().unwrap_or(sizes.len());
    Paged {
        page,
        pages,
        range: start..end,
        total: sizes.len(),
    }
}

/// The line after a page that was cut: how much is left and how to see it.
fn more_marker(paged: &Paged, rerun: &str, noun: &str) -> String {
    if paged.page >= paged.pages {
        return String::new();
    }
    let left = paged.total - paged.range.end;
    format!(
        "... {left} more {noun} on {} more {} ({rerun} --page {})\n",
        paged.pages - paged.page,
        if paged.pages - paged.page == 1 {
            "page"
        } else {
            "pages"
        },
        paged.page + 1
    )
}

/// A page of a JSON answer: the entries and where they sit in the whole.
#[derive(Serialize)]
struct Page<'a, T: Serialize> {
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<&'a Symbol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    items: &'a [T],
    total: usize,
    page: usize,
    pages: usize,
    /// The answer was longer than the most a query returns.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    capped: bool,
}

fn symbol_line(s: &Symbol) -> String {
    format!(
        "{}:{} {} {}  #{}\n",
        s.path, s.start_line, s.kind, s.qualified_name, s.id
    )
}

fn quoted(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_alphanumeric() || "_-./:#".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

fn symbols(
    graph: &Graph,
    args: &GraphSymbolsArgs,
    out: &Output,
) -> Result<Exit, jouet_graph::Error> {
    let found = graph.symbols(&SymbolQuery {
        text: args.query.clone(),
        kind: args.kind.clone(),
        path: args.path.clone(),
    })?;
    let lines: Vec<String> = found.iter().map(symbol_line).collect();
    let sizes: Vec<usize> = lines.iter().map(String::len).collect();
    let paged = paginate(
        &sizes,
        args.budget.unwrap_or(BUDGET),
        args.page.unwrap_or(1),
    );
    let page = Page {
        symbol: None,
        query: args.query.as_deref(),
        items: &found[paged.range.clone()],
        total: found.len(),
        page: paged.page,
        pages: paged.pages,
        capped: found.len() >= MAX_RESULTS,
    };
    let mut rerun = String::from("jouet graph symbols");
    if let Some(q) = &args.query {
        let _ = write!(rerun, " {}", quoted(q));
    }
    out.result(&page, || {
        if found.is_empty() {
            return "no symbols found\n".to_string();
        }
        let mut s: String = lines[paged.range.clone()].concat();
        s.push_str(&more_marker(&paged, &rerun, "symbols"));
        s
    });
    Ok(if found.is_empty() {
        Exit::NotFound
    } else {
        Exit::Ok
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Callers,
    Callees,
    Impact,
}

impl Direction {
    fn name(self) -> &'static str {
        match self {
            Direction::Callers => "callers",
            Direction::Callees => "callees",
            Direction::Impact => "impact",
        }
    }
}

/// The most symbols one name answers for before the rest are only counted.
const MAX_TARGETS: usize = 5;

#[derive(Serialize)]
struct Traversal {
    symbol: Symbol,
    #[serde(flatten)]
    page: TraversalPage,
}

#[derive(Serialize)]
struct TraversalPage {
    items: Vec<Reach>,
    total: usize,
    page: usize,
    pages: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    capped: bool,
}

fn reach_line(r: &Reach, direction: Direction) -> String {
    let indent = "  ".repeat(r.depth as usize);
    let at = match (direction, r.line) {
        // A caller's line is where the call is; a callee's is its definition.
        (Direction::Callers | Direction::Impact, Some(line)) => line,
        _ => r.symbol.start_line,
    };
    format!(
        "{indent}{}:{at} {} {} ({})\n",
        r.symbol.path, r.symbol.kind, r.symbol.qualified_name, r.via
    )
}

fn traverse(
    graph: &Graph,
    args: &GraphTraverseArgs,
    direction: Direction,
    out: &Output,
) -> Result<Exit, jouet_graph::Error> {
    let targets = graph.find(&args.symbol)?;
    if targets.is_empty() {
        return Ok(out.error_with_hint(
            Exit::NotFound,
            &format!("no symbol {}", args.symbol),
            Some("find one with `jouet graph symbols QUERY`, then pass its #ID or PATH#NAME"),
        ));
    }
    let depth = args
        .depth
        .unwrap_or(if direction == Direction::Impact { 3 } else { 1 });
    let shown = &targets[..targets.len().min(MAX_TARGETS)];
    let mut sections = Vec::new();
    for t in shown {
        let mut found = match direction {
            Direction::Callers => graph.callers(&t.id, depth)?,
            Direction::Callees => graph.callees(&t.id, depth)?,
            Direction::Impact => graph.impact(&t.id, depth)?,
        };
        if direction == Direction::Impact {
            // By file, so a reviewer sees which files a change touches.
            found.sort_by(|a, b| {
                a.symbol
                    .path
                    .cmp(&b.symbol.path)
                    .then(a.depth.cmp(&b.depth))
            });
        }
        sections.push((t.clone(), found));
    }
    // One budget for the whole answer, split evenly between the symbols.
    let budget = args.budget.unwrap_or(BUDGET);
    let per = if budget == 0 {
        0
    } else {
        (budget / sections.len()).max(1)
    };
    let mut docs = Vec::new();
    let mut text = String::new();
    for (symbol, found) in sections {
        let lines: Vec<String> = found.iter().map(|r| reach_line(r, direction)).collect();
        let sizes: Vec<usize> = lines.iter().map(String::len).collect();
        let paged = paginate(&sizes, per, args.page.unwrap_or(1));
        let _ = writeln!(
            text,
            "{} of {} {} ({}:{}): {}",
            direction.name(),
            symbol.kind,
            symbol.qualified_name,
            symbol.path,
            symbol.start_line,
            if found.is_empty() {
                "none".to_string()
            } else {
                found.len().to_string()
            }
        );
        if direction == Direction::Impact && !found.is_empty() {
            let files: std::collections::BTreeSet<&str> =
                found.iter().map(|r| r.symbol.path.as_str()).collect();
            let _ = writeln!(text, "  in {} files", files.len());
        }
        if !found.is_empty() {
            text.push_str(&lines[paged.range.clone()].concat());
            let rerun = format!("jouet graph {} {}", direction.name(), quoted(&symbol.id));
            text.push_str(&more_marker(&paged, &rerun, direction.name()));
        }
        docs.push(Traversal {
            symbol,
            page: TraversalPage {
                items: found[paged.range.clone()].to_vec(),
                total: found.len(),
                page: paged.page,
                pages: paged.pages,
                capped: found.len() >= MAX_RESULTS,
            },
        });
    }
    if targets.len() > shown.len() {
        let _ = writeln!(
            text,
            "... {} more symbols named {} (name one by PATH#NAME or #ID from `jouet graph symbols`)",
            targets.len() - shown.len(),
            args.symbol
        );
    }
    #[derive(Serialize)]
    struct Answer {
        symbols: Vec<Traversal>,
        #[serde(skip_serializing_if = "is_zero")]
        more_symbols: usize,
    }
    fn is_zero(n: &usize) -> bool {
        *n == 0
    }
    let answer = Answer {
        more_symbols: targets.len() - shown.len(),
        symbols: docs,
    };
    out.result(&answer, || text);
    Ok(Exit::Ok)
}

/// A symbol's source, at most `budget` bytes, cut at a line.
fn excerpt(graph: &Graph, s: &Symbol, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    let Ok(text) = std::fs::read_to_string(graph.root().join(&s.path)) else {
        return String::new();
    };
    let lines: Vec<&str> = text
        .lines()
        .skip(s.start_line.saturating_sub(1) as usize)
        .take((s.end_line + 1).saturating_sub(s.start_line) as usize)
        .collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let numbered = format!("  {:>5}  {line}\n", s.start_line as usize + i);
        if out.len() + numbered.len() > budget && !out.is_empty() {
            let _ = writeln!(
                out,
                "  ... {} more lines (jouet read {}#{})",
                lines.len() - i,
                s.path,
                s.name
            );
            break;
        }
        out.push_str(&numbered);
    }
    out
}

fn names_line(label: &str, names: &[String], total: usize) -> String {
    const SHOWN: usize = 5;
    if names.is_empty() {
        return String::new();
    }
    let mut s = format!("  {label}: {}", names[..names.len().min(SHOWN)].join(", "));
    if total > SHOWN {
        let _ = write!(s, ", ... {} more", total - SHOWN);
    }
    s.push('\n');
    s
}

fn explored_text(graph: &Graph, e: &Explored, source_budget: usize) -> String {
    let s = &e.symbol;
    let mut text = format!(
        "{}:{}-{} {} {}  #{}\n",
        s.path, s.start_line, s.end_line, s.kind, s.qualified_name, s.id
    );
    if let Some(doc) = s.docstring.as_deref().and_then(|d| d.lines().next()) {
        let _ = writeln!(text, "  {doc}");
    }
    text.push_str(&names_line("called by", &e.callers, e.caller_count));
    text.push_str(&names_line("calls", &e.callees, e.callee_count));
    text.push_str(&excerpt(graph, s, source_budget));
    text
}

fn explore(
    graph: &Graph,
    args: &GraphExploreArgs,
    out: &Output,
) -> Result<Exit, jouet_graph::Error> {
    let query = args.query.join(" ");
    if query.trim().is_empty() {
        return Ok(out.error(Exit::Usage, "explore needs a question or names to look for"));
    }
    let found = graph.explore(&query)?;
    let source_budget = args.source_budget.unwrap_or(1200);
    let texts: Vec<String> = found
        .iter()
        .map(|e| explored_text(graph, e, source_budget))
        .collect();
    let sizes: Vec<usize> = texts.iter().map(|t| t.len() + 1).collect();
    let paged = paginate(
        &sizes,
        args.budget.unwrap_or(BUDGET),
        args.page.unwrap_or(1),
    );
    let page = Page {
        symbol: None,
        query: Some(&query),
        items: &found[paged.range.clone()],
        total: found.len(),
        page: paged.page,
        pages: paged.pages,
        capped: found.len() >= MAX_RESULTS,
    };
    out.result(&page, || {
        if found.is_empty() {
            return format!("nothing in the graph matches {query}\n");
        }
        let mut s = texts[paged.range.clone()].join("\n");
        s.push_str(&more_marker(
            &paged,
            &format!("jouet graph explore {}", quoted(&query)),
            "symbols",
        ));
        s
    });
    Ok(if found.is_empty() {
        Exit::NotFound
    } else {
        Exit::Ok
    })
}

fn affected(
    graph: &Graph,
    args: &GraphAffectedArgs,
    out: &Output,
) -> Result<Exit, jouet_graph::Error> {
    if args.files.is_empty() {
        return Ok(out.error(Exit::Usage, "name the changed files"));
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let files: Vec<PathBuf> = args.files.iter().map(|f| cwd.join(f)).collect();
    let mut affected = graph.affected_tests(&files)?;
    // Unknown files as they were named, not as absolute paths.
    for u in &mut affected.unknown {
        if let Ok(rel) = u.strip_prefix(&cwd) {
            *u = rel.to_path_buf();
        }
    }
    out.result(&affected, || {
        let mut s = String::new();
        for t in &affected.tests {
            let chain: Vec<String> = t.via.iter().map(|p| p.display().to_string()).collect();
            if chain.len() > 1 {
                let _ = writeln!(
                    s,
                    "{}  (via {})",
                    t.path.display(),
                    chain[..chain.len() - 1].join(" -> ")
                );
            } else {
                let _ = writeln!(s, "{}  (changed)", t.path.display());
            }
        }
        if affected.tests.is_empty() {
            s.push_str("no test files depend on these files\n");
        }
        for u in &affected.unknown {
            let _ = writeln!(
                s,
                "unknown: {} (not in the graph; run the whole suite)",
                u.display()
            );
        }
        s
    });
    Ok(if affected.unknown.is_empty() {
        Exit::Ok
    } else {
        Exit::Failed
    })
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
