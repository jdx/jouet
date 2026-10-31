//! `--callers-of` and `--impact` on structural search and rewrites: the code
//! graph says which of a pattern's matches are about one symbol.
//!
//! A pattern such as `foo($$$A)` matches every function named `foo`; the
//! graph knows which `foo` each call means. With `--callers-of`, a match is
//! kept when it starts where the graph records a call attributed to the
//! symbol and names it. With `--impact`, a match is kept inside the symbol
//! or anything that depends on it. What the graph cannot attribute is listed
//! as unresolved and left alone, never rewritten or dropped unsaid.

use jouet_graph::{Affected, Graph, Site, Symbol, SymbolQuery};
use jouet_output::exit::Exit;
use jouet_search::structural::{self, Filter, Match, Structural};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How far back `--impact` follows dependents, as `jouet graph impact`.
const IMPACT_DEPTH: u32 = 3;

/// How many entries of each list the text output shows.
const SHOWN: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum How {
    CallersOf,
    Impact,
}

impl How {
    /// The flags as the command line takes them, for the one that is set.
    pub(crate) fn from_flags<'a>(
        callers_of: Option<&'a str>,
        impact: Option<&'a str>,
    ) -> Result<Option<(How, &'a str)>, String> {
        match (callers_of, impact) {
            (Some(_), Some(_)) => Err("--callers-of and --impact are alternatives".into()),
            (Some(s), None) => Ok(Some((How::CallersOf, s))),
            (None, Some(s)) => Ok(Some((How::Impact, s))),
            (None, None) => Ok(None),
        }
    }

    fn name(self) -> &'static str {
        match self {
            How::CallersOf => "callers_of",
            How::Impact => "impact",
        }
    }
}

/// What the graph made of a pattern's matches.
#[derive(Debug, Serialize)]
pub(crate) struct Report {
    /// `callers_of` or `impact`.
    how: &'static str,
    symbol: Symbol,
    /// Matches in scope: rewritten, or shown by a search.
    pub(crate) matched: usize,
    /// Matches outside the scope, left alone: calls of another definition
    /// of the name, or code that does not depend on the symbol.
    #[serde(skip_serializing_if = "is_zero")]
    skipped: usize,
    /// Matches, and mentions in macros, the graph cannot attribute: left
    /// alone, to be checked by hand.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unresolved: Vec<Unresolved>,
    /// The symbol's call sites no kept match covers (`--callers-of`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unmatched: Vec<Unmatched>,
    /// The rewrites in scope, in file order (`jouet edit` only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) changes: Vec<Change>,
    /// The test files that depend on the files a rewrite changes, from the
    /// graph's `affected`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tests: Option<Affected>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize)]
struct Unresolved {
    path: String,
    line: u32,
    text: String,
    /// `unlinked`, `ambiguous`, `not_indexed`, `no_site` or `in_macro`.
    reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Change {
    path: String,
    line: u32,
    /// The first line of the match.
    text: String,
    replacement: String,
}

#[derive(Debug, Clone, Serialize)]
struct Unmatched {
    path: String,
    line: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    text: String,
    /// `not_matched` or `not_searched`.
    reason: &'static str,
}

fn unresolved_reason(reason: &str) -> &'static str {
    match reason {
        "unlinked" => "the graph could not tell which definition these call",
        "ambiguous" => "the graph links these to more than one definition",
        "not_indexed" => "in files the graph has not read",
        "no_site" => {
            "the graph records no call starting here (a pattern for the whole call, such as `$R.foo($$$A)`, lines up with one)"
        }
        _ => "inside a macro, which neither the pattern nor the graph parses",
    }
}

fn unmatched_reason(reason: &str) -> &'static str {
    match reason {
        "not_matched" => "the pattern does not match these calls",
        _ => "outside the paths searched",
    }
}

/// The graph's view of one symbol, as a [`Filter`] for structural matches.
#[derive(Debug)]
pub(crate) struct Scope {
    how: How,
    symbol: Symbol,
    root: PathBuf,
    cwd: PathBuf,
    /// `--callers-of`: the uses of the symbol's name, by file.
    sites: HashMap<String, Vec<Site>>,
    /// Where definitions of the name start, by file, so a pattern that
    /// matches one is not taken for an unresolved call.
    definitions: HashSet<(String, u32)>,
    /// `--impact`: the line spans of the symbol and its dependents, by file.
    spans: HashMap<String, Vec<(u32, u32)>>,
    /// The files the graph read.
    known: HashSet<String>,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    matched: usize,
    skipped: usize,
    unresolved: Vec<Unresolved>,
    changes: Vec<Change>,
    /// The call sites of the symbol a kept match covers: path, line, column.
    claimed: HashSet<(String, u32, Option<u32>)>,
    /// The text of the lines with the symbol's call sites, in files read.
    lines: HashMap<(String, u32), String>,
    searched: HashSet<String>,
}

impl Scope {
    /// Opens and refreshes the work tree's graph, and resolves `spec` to the
    /// one symbol it names.
    pub(crate) fn open(how: How, spec: &str) -> Result<(Graph, Scope), (Exit, String)> {
        let cwd = std::env::current_dir()
            .map_err(|e| (Exit::Precondition, format!("no current directory: {e}")))?;
        let mut graph = Graph::open(&cwd).map_err(|e| (Exit::Precondition, e.to_string()))?;
        graph
            .refresh(None)
            .map_err(|e| (Exit::Failed, e.to_string()))?;
        let found = graph
            .find(spec)
            .map_err(|e| (Exit::Failed, e.to_string()))?;
        let symbol = match found.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err((
                    Exit::NotFound,
                    format!(
                        "no symbol {spec} in the code graph; find one with `jouet graph symbols QUERY` and pass it as PATH#NAME"
                    ),
                ));
            }
            many => {
                let mut message = format!(
                    "{spec} names {} symbols; pass one by PATH#NAME or #ID:",
                    many.len()
                );
                for s in many.iter().take(5) {
                    let _ = write!(message, "\n  {}#{}  #{}", s.path, s.qualified_name, s.id);
                }
                if many.len() > 5 {
                    let _ = write!(message, "\n  ... {} more", many.len() - 5);
                }
                return Err((Exit::Usage, message));
            }
        };
        let failed = |e: jouet_graph::Error| (Exit::Failed, e.to_string());
        let mut sites: HashMap<String, Vec<Site>> = HashMap::new();
        let mut definitions = HashSet::new();
        let mut spans: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        match how {
            How::CallersOf => {
                for site in graph.uses_of(&symbol.name).map_err(failed)? {
                    sites.entry(site.path.clone()).or_default().push(site);
                }
                let named = graph
                    .symbols(&SymbolQuery {
                        text: Some(symbol.name.clone()),
                        ..SymbolQuery::default()
                    })
                    .map_err(failed)?;
                definitions = named
                    .into_iter()
                    .filter(|s| s.name == symbol.name)
                    .map(|s| (s.path, s.start_line))
                    .collect();
            }
            How::Impact => {
                spans
                    .entry(symbol.path.clone())
                    .or_default()
                    .push((symbol.start_line, symbol.end_line));
                for r in graph.impact(&symbol.id, IMPACT_DEPTH).map_err(failed)? {
                    // A file depends on the symbol through its imports; the
                    // code that uses what it imports is reached on its own.
                    if r.symbol.kind == "file" {
                        continue;
                    }
                    spans
                        .entry(r.symbol.path.clone())
                        .or_default()
                        .push((r.symbol.start_line, r.symbol.end_line));
                }
            }
        }
        let known = graph.files().map_err(failed)?.into_iter().collect();
        let scope = Scope {
            how,
            symbol,
            root: graph.root().to_path_buf(),
            cwd: cwd.canonicalize().unwrap_or(cwd),
            sites,
            definitions,
            spans,
            known,
            state: Mutex::new(State::default()),
        };
        Ok((graph, scope))
    }

    /// `path`, as walked, relative to the work tree.
    fn relative(&self, path: &Path) -> Option<String> {
        let full = self.cwd.join(path).canonicalize().ok()?;
        let rel = full.strip_prefix(&self.root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }

    /// A work-tree path as the rest of the output shows paths: relative to
    /// the current directory.
    fn shown(&self, rel: &str) -> String {
        let full = self.root.join(rel);
        full.strip_prefix(&self.cwd)
            .map_or(full.clone(), Path::to_path_buf)
            .to_string_lossy()
            .into_owned()
    }

    fn callers_of(
        &self,
        structural: &Structural,
        rel: Option<&str>,
        shown: &str,
        source: &str,
        matches: Vec<Match>,
        state: &mut State,
    ) -> Vec<Match> {
        let name = self.symbol.name.as_str();
        let no_sites = Vec::new();
        let sites = rel.and_then(|r| self.sites.get(r)).unwrap_or(&no_sites);
        let known = rel.is_some_and(|r| self.known.contains(r));
        let mut unresolved = |m: &Match, reason| {
            state.unresolved.push(Unresolved {
                path: shown.to_string(),
                line: m.line,
                text: m.text.clone(),
                reason,
            });
        };
        let mut kept = Vec::new();
        let mut claimed = Vec::new();
        let mut skipped = 0;
        for m in matches {
            if !structural::mentions(&source[m.start..m.end], name) {
                skipped += 1;
                continue;
            }
            let col = col16(source, m.start);
            let at: Vec<&Site> = sites
                .iter()
                .filter(|s| s.line == m.line && s.col.is_none_or(|c| c == col))
                .collect();
            let ours = at.iter().any(|s| s.targets.contains(&self.symbol.id));
            let others = at
                .iter()
                .any(|s| s.targets.iter().any(|t| *t != self.symbol.id));
            if at.is_empty() {
                // No use of the name starts here: a match that names it
                // only inside its arguments (`start({ load })`) is a call
                // of something else.
                let head = source[m.start..m.end].split(['(', '{', '\n']).next();
                if !head.is_some_and(|h| structural::mentions(h, name)) {
                    skipped += 1;
                } else if !known {
                    unresolved(&m, "not_indexed");
                } else if rel.is_some_and(|r| self.definitions.contains(&(r.to_string(), m.line))) {
                    skipped += 1;
                } else {
                    unresolved(&m, "no_site");
                }
            } else if ours && others {
                unresolved(&m, "ambiguous");
            } else if ours {
                claimed.extend(at.iter().map(|s| (s.line, s.col)));
                kept.push(m);
            } else if others {
                skipped += 1;
            } else {
                unresolved(&m, "unlinked");
            }
        }
        if source.contains(name) {
            for m in structural.in_macros(source, name) {
                unresolved(&m, "in_macro");
            }
        }
        if let Some(rel) = rel {
            for s in sites.iter().filter(|s| s.targets.contains(&self.symbol.id)) {
                if let Some(text) = source.lines().nth(s.line.saturating_sub(1) as usize) {
                    state
                        .lines
                        .insert((rel.to_string(), s.line), text.trim().to_string());
                }
            }
            state.claimed.extend(
                claimed
                    .into_iter()
                    .map(|(line, col)| (rel.to_string(), line, col)),
            );
        }
        state.skipped += skipped;
        kept
    }

    fn impact(
        &self,
        rel: Option<&str>,
        shown: &str,
        matches: Vec<Match>,
        state: &mut State,
    ) -> Vec<Match> {
        let spans = rel.and_then(|r| self.spans.get(r));
        let known = rel.is_some_and(|r| self.known.contains(r));
        let mut kept = Vec::new();
        for m in matches {
            if spans.is_some_and(|s| s.iter().any(|&(a, b)| a <= m.line && m.line <= b)) {
                kept.push(m);
            } else if !known {
                state.unresolved.push(Unresolved {
                    path: shown.to_string(),
                    line: m.line,
                    text: m.text.clone(),
                    reason: "not_indexed",
                });
            } else {
                state.skipped += 1;
            }
        }
        kept
    }

    /// What the walk found, once every file was seen.
    pub(crate) fn finish(self) -> Report {
        let mut state = match self.state.lock() {
            Ok(mut state) => std::mem::take(&mut *state),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };
        state
            .unresolved
            .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        state
            .changes
            .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        state.unresolved.dedup_by(|a, b| {
            (&a.path, a.line, &a.text, a.reason) == (&b.path, b.line, &b.text, b.reason)
        });
        let mut unmatched = Vec::new();
        let mut files: BTreeMap<&str, Vec<&Site>> = BTreeMap::new();
        for (path, sites) in &self.sites {
            for s in sites.iter().filter(|s| s.targets.contains(&self.symbol.id)) {
                if !state.claimed.contains(&(path.clone(), s.line, s.col)) {
                    files.entry(path).or_default().push(s);
                }
            }
        }
        for (path, mut sites) in files {
            sites.sort_by_key(|s| (s.line, s.col));
            sites.dedup_by_key(|s| s.line);
            let searched = state.searched.contains(path);
            let text = if searched {
                None
            } else {
                std::fs::read_to_string(self.root.join(path)).ok()
            };
            for s in sites {
                let line = match &text {
                    Some(t) => t
                        .lines()
                        .nth(s.line.saturating_sub(1) as usize)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    None => state
                        .lines
                        .get(&(path.to_string(), s.line))
                        .cloned()
                        .unwrap_or_default(),
                };
                unmatched.push(Unmatched {
                    path: self.shown(path),
                    line: s.line,
                    text: line,
                    reason: if searched {
                        "not_matched"
                    } else {
                        "not_searched"
                    },
                });
            }
        }
        Report {
            how: self.how.name(),
            symbol: self.symbol,
            matched: state.matched,
            skipped: state.skipped,
            unresolved: state.unresolved,
            unmatched,
            changes: state.changes,
            tests: None,
        }
    }
}

impl Filter for Scope {
    fn keep(
        &self,
        structural: &Structural,
        path: &Path,
        source: &str,
        matches: Vec<Match>,
    ) -> Vec<Match> {
        let rel = self.relative(path);
        let shown = path
            .strip_prefix(".")
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        if let Some(rel) = &rel {
            state.searched.insert(rel.clone());
        }
        let kept = match self.how {
            How::CallersOf => self.callers_of(
                structural,
                rel.as_deref(),
                &shown,
                source,
                matches,
                &mut state,
            ),
            How::Impact => self.impact(rel.as_deref(), &shown, matches, &mut state),
        };
        state.matched += kept.len();
        for m in &kept {
            if let Some(replacement) = &m.replacement {
                state.changes.push(Change {
                    path: shown.clone(),
                    line: m.line,
                    text: m.text.clone(),
                    replacement: replacement.clone(),
                });
            }
        }
        kept
    }
}

/// The 0-based column of byte `at`, in UTF-16 code units as the graph's
/// extractors count them.
fn col16(source: &str, at: usize) -> u32 {
    let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    u32::try_from(source[start..at].encode_utf16().count()).unwrap_or(u32::MAX)
}

/// The report as text: the scope's counts, then what was left alone and
/// why, then the tests to run.
pub(crate) fn render(report: &Report) -> String {
    let s = &report.symbol;
    let mut text = String::new();
    let what = match report.how {
        "callers_of" => "calls of",
        _ => "code depending on",
    };
    let _ = write!(
        text,
        "scope: {} {} in {what} {} {} ({}:{})",
        report.matched,
        if report.matched == 1 {
            "match"
        } else {
            "matches"
        },
        s.kind,
        s.qualified_name,
        s.path,
        s.start_line
    );
    if report.skipped > 0 {
        let _ = write!(text, "; {} other matches left alone", report.skipped);
    }
    text.push('\n');
    let changes: Vec<String> = report
        .changes
        .iter()
        .map(|c| {
            let mut to = c
                .replacement
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if c.replacement.trim().lines().nth(1).is_some() {
                to.push_str(" …");
            }
            format!("{}:{}  {}  ->  {to}", c.path, c.line, c.text)
        })
        .collect();
    list(&mut text, &changes);
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for u in &report.unresolved {
        groups
            .entry(u.reason)
            .or_default()
            .push(format!("{}:{}  {}", u.path, u.line, u.text));
    }
    if !report.unresolved.is_empty() {
        let _ = writeln!(
            text,
            "unresolved, left alone: {} (check them by hand)",
            report.unresolved.len()
        );
        for (reason, lines) in &groups {
            let _ = writeln!(text, "  {reason}: {}", unresolved_reason(reason));
            list(&mut text, lines);
        }
    }
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for u in &report.unmatched {
        groups
            .entry(u.reason)
            .or_default()
            .push(format!("{}:{}  {}", u.path, u.line, u.text));
    }
    if !report.unmatched.is_empty() {
        let _ = writeln!(
            text,
            "calls of {} not covered: {}",
            s.name,
            report.unmatched.len()
        );
        for (reason, lines) in &groups {
            let _ = writeln!(text, "  {reason}: {}", unmatched_reason(reason));
            list(&mut text, lines);
        }
    }
    if let Some(tests) = &report.tests {
        let lines: Vec<String> = tests
            .tests
            .iter()
            .map(|t| t.path.to_string_lossy().into_owned())
            .collect();
        let _ = writeln!(
            text,
            "tests that depend on the changed files: {}",
            if lines.is_empty() {
                "none found".to_string()
            } else {
                lines.len().to_string()
            }
        );
        list(&mut text, &lines);
        if !tests.unknown.is_empty() {
            let _ = writeln!(
                text,
                "  the graph cannot speak for {} of the changed files; run every test",
                tests.unknown.len()
            );
        }
    }
    text
}

/// Up to [`SHOWN`] lines, indented, then a marker for the rest.
fn list(text: &mut String, lines: &[String]) {
    for line in lines.iter().take(SHOWN) {
        let _ = writeln!(text, "    {line}");
    }
    if lines.len() > SHOWN {
        let _ = writeln!(
            text,
            "    ... {} more (--json lists every one)",
            lines.len() - SHOWN
        );
    }
}
