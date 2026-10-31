//! Queries over the graph: symbols, callers, callees, impact, explore and
//! affected tests.
//!
//! Derived from codegraph's queries (`src/graph/traversal.ts`,
//! `src/context/`, `src/search/query-utils.ts`). Every query returns all it
//! found, ranked, up to a hard cap; the command line budgets what it prints.

use crate::{Error, Graph, SYMBOL_COLUMNS, Symbol};
use regex::Regex;
use rusqlite::params;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The most results any query returns; the command line says when it cut.
pub const MAX_RESULTS: usize = 2000;

/// What `symbols` looks for. Every field narrows; none means everything.
#[derive(Debug, Default, Clone)]
pub struct SymbolQuery {
    /// A name, a qualified name, or words (`payment retry`).
    pub text: Option<String>,
    /// `function`, `class`, `method`, …
    pub kind: Option<String>,
    /// A file, or a directory prefix, relative to the work tree.
    pub path: Option<String>,
}

/// A symbol reached from another through edges.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Reach {
    #[serde(flatten)]
    pub symbol: Symbol,
    /// Edges from the starting symbol: 1 for a direct caller.
    pub depth: u32,
    /// The kind of the edge that reached it (`calls`, `imports`, …).
    pub via: String,
    /// Where that edge is written, in the source file of the edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A symbol `explore` found, with its neighbours.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Explored {
    #[serde(flatten)]
    pub symbol: Symbol,
    pub score: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub callers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub callees: Vec<String>,
    /// How many callers and callees there are in all.
    pub caller_count: usize,
    pub callee_count: usize,
}

/// The test files a change reaches.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Affected {
    pub tests: Vec<AffectedTest>,
    /// Changed files the graph cannot speak for: not indexed, or in a
    /// language jouet does not extract. Run everything when any are here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AffectedTest {
    pub path: PathBuf,
    /// The chain of files from a changed file to this test, the changed
    /// file first: each depends on the one before.
    pub via: Vec<PathBuf>,
}

/// A place the graph records a use of a name, a call, a construction or a
/// reference, and the definitions it was linked to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Site {
    /// Relative to the work tree, with `/` separators.
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 0-based, in UTF-16 code units from the start of the line: where the
    /// call or construction expression starts. `None` when not recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    /// `calls`, `instantiates` or `references`.
    pub kind: String,
    /// The IDs of the definitions linking attributed it to: none when it
    /// could not be attributed, more than one when it stayed ambiguous.
    pub targets: Vec<String>,
}

/// Edge kinds that are a use of their target, as callers count them.
const CALL_KINDS: &str = "('calls', 'instantiates', 'references')";

/// Edge kinds a change propagates along: everything but containment.
const DEPENDENCY_KINDS: &str = "('calls', 'instantiates', 'references', 'imports', 'extends', 'implements', 'type_of', 'returns', 'decorates', 'overrides')";

/// codegraph's `isTestPath`: the file and directory names every ecosystem
/// uses for its test suites.
pub fn is_test_path(path: &str) -> bool {
    static NAME: OnceLock<Regex> = OnceLock::new();
    static CAMEL: OnceLock<Regex> = OnceLock::new();
    static DIR: OnceLock<Regex> = OnceLock::new();
    let lower = path.to_lowercase();
    let file = path.rsplit('/').next().unwrap_or(path);
    let lower_file = file.to_lowercase();
    if lower_file.starts_with("test_")
        || lower_file.starts_with("test.")
        || NAME
            .get_or_init(|| {
                Regex::new(r"[._-](test|tests|unittest|unittests|spec|specs)\.[a-z0-9]+$").unwrap()
            })
            .is_match(&lower_file)
        || CAMEL
            .get_or_init(|| {
                Regex::new(r"(?:Test|Tests|TestCase|Tester|Spec|Specs)\.[A-Za-z0-9]+$").unwrap()
            })
            .is_match(file)
    {
        return true;
    }
    let dirs = format!("/{lower}");
    [
        "/tests/",
        "/test/",
        "/__tests__/",
        "/spec/",
        "/specs/",
        "/testlib/",
        "/testing/",
        "/e2e/",
    ]
    .iter()
    .any(|d| dirs.contains(d))
        || DIR
            .get_or_init(|| {
                Regex::new(
                    r"(?:^|/)[A-Za-z0-9]*(?:Test|Tests|Spec)/|(?i:(?:^|/)(?:[\w.]+[-_]test(?:s|ing)?|testdata|testutils?|test[-_]utils?|fakes?|mocks?|__mocks__|stubs|unittests|[\w.]+[-_]unittests?)/)",
                )
                .unwrap()
            })
            .is_match(path)
}

/// The words of identifiers, for full-text search: `processPayment` and
/// `process_payment` are both `process payment`.
pub(crate) fn words(text: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if c.is_alphanumeric() {
            let boundary = prev.is_some_and(|p| {
                (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic())
            });
            if boundary {
                out.push(' ');
            }
            out.extend(c.to_lowercase());
            prev = Some(c);
        } else {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            prev = None;
        }
    }
    out.trim_end().to_string()
}

/// Words too common in questions to rank by.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "in", "on", "for", "and", "or", "is", "are", "how", "what",
    "where", "which", "who", "does", "do", "with", "by", "from", "it", "this", "that", "when",
    "why", "be", "as", "at", "get", "set", "code", "function", "method", "class", "file", "work",
    "works", "handle", "handles", "use", "used", "uses",
];

fn fts_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    words(query)
        .split(' ')
        .filter(|w| w.len() > 1 && !STOPWORDS.contains(w))
        .filter(|w| seen.insert(w.to_string()))
        .map(|w| format!("\"{w}\"*"))
        .collect()
}

impl Graph {
    fn symbols_where(
        &self,
        clause: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Symbol>, Error> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SYMBOL_COLUMNS} FROM nodes WHERE {clause}"
        ))?;
        let rows = stmt.query_map(args, Symbol::from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Symbols matching `q`, best first: an exact name, then an exact
    /// qualified name or one ending in it, then full-text matches.
    pub fn symbols(&self, q: &SymbolQuery) -> Result<Vec<Symbol>, Error> {
        let mut out: Vec<Symbol> = Vec::new();
        let mut seen = HashSet::new();
        let filter = |s: &Symbol| {
            s.kind != "import"
                && s.kind != "file"
                && q.kind.as_deref().is_none_or(|k| s.kind == k)
                && q.path.as_deref().is_none_or(|p| {
                    s.path == p || s.path.starts_with(&format!("{}/", p.trim_end_matches('/')))
                })
        };
        let mut push = |list: Vec<Symbol>, out: &mut Vec<Symbol>| {
            for s in list {
                if filter(&s) && seen.insert(s.id.clone()) && out.len() < MAX_RESULTS {
                    out.push(s);
                }
            }
        };
        let Some(text) = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
            let mut all = self.symbols_where(
                "kind NOT IN ('file', 'import') ORDER BY file_path, start_line LIMIT ?1",
                &[&((MAX_RESULTS * 4) as i64)],
            )?;
            all.retain(&filter);
            all.truncate(MAX_RESULTS);
            return Ok(all);
        };
        push(
            self.symbols_where(
                "name = ?1 ORDER BY is_exported DESC, file_path, start_line",
                &[&text],
            )?,
            &mut out,
        );
        let dotted = text.replace('.', "::");
        push(
            self.symbols_where(
                "qualified_name = ?1 OR qualified_name LIKE ?2 ESCAPE '\\' ORDER BY file_path, start_line",
                &[&dotted, &format!("%::{}", dotted.replace('%', "\\%").replace('_', "\\_"))],
            )?,
            &mut out,
        );
        let terms = fts_terms(text);
        if !terms.is_empty() {
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT {} FROM nodes_fts f JOIN nodes n ON n.nid = f.rowid
                 WHERE nodes_fts MATCH ?1 ORDER BY bm25(nodes_fts, 0.0, 10.0, 5.0, 8.0, 1.0, 2.0) LIMIT ?2",
                SYMBOL_COLUMNS
                    .split(", ")
                    .map(|c| format!("n.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))?;
            // Every word first; then any word, when every word finds little.
            for joiner in [" AND ", " OR "] {
                let rows = stmt.query_map(
                    params![terms.join(joiner), (MAX_RESULTS * 2) as i64],
                    Symbol::from_row,
                )?;
                push(rows.collect::<Result<_, _>>()?, &mut out);
                if !out.is_empty() || terms.len() == 1 {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The symbols a spec names: a node ID, `path#name` or `path:line`,
    /// a qualified name, or a name.
    pub fn find(&self, spec: &str) -> Result<Vec<Symbol>, Error> {
        let by_id = self.symbols_where("id = ?1", &[&spec])?;
        if !by_id.is_empty() {
            return Ok(by_id);
        }
        if let Some((path, name)) = spec.split_once('#') {
            let path = self.rel(path);
            let name = name.split_whitespace().last().unwrap_or(name);
            return self.symbols_where(
                "file_path = ?1 AND (name = ?2 OR qualified_name = ?2 OR qualified_name LIKE '%::' || ?2)
                 AND kind NOT IN ('file', 'import') ORDER BY start_line",
                &[&path, &name],
            );
        }
        if let Some((path, line)) = spec.rsplit_once(':')
            && let Ok(line) = line.parse::<u32>()
        {
            return Ok(self.enclosing(Path::new(path), line)?.into_iter().collect());
        }
        let found = self.symbols(&SymbolQuery {
            text: Some(spec.to_string()),
            ..SymbolQuery::default()
        })?;
        let exact: Vec<Symbol> = found
            .iter()
            .filter(|s| {
                s.name == spec
                    || s.qualified_name == spec
                    || s.qualified_name.ends_with(&format!("::{spec}"))
            })
            .cloned()
            .collect();
        if !exact.is_empty() {
            return Ok(exact);
        }
        // A module path to a free function (`store::open`), whose qualified
        // name is only its name: the module is its file.
        let dotted = spec.replace('.', "::");
        let Some((module, name)) = dotted.rsplit_once("::") else {
            return Ok(exact);
        };
        let module = module.rsplit("::").next().unwrap_or(module);
        Ok(self
            .symbols_where(
                "name = ?1 AND kind NOT IN ('file', 'import') ORDER BY file_path, start_line",
                &[&name],
            )?
            .into_iter()
            .filter(|s| {
                let stem = s.path.rsplit('/').next().unwrap_or(&s.path);
                let stem = stem.split('.').next().unwrap_or(stem);
                stem == module
                    || s.path.contains(&format!("{module}/mod."))
                    || s.path.contains(&format!("/{module}/__init__."))
            })
            .collect())
    }

    fn rel(&self, path: &str) -> String {
        let p = Path::new(path);
        if p.is_absolute() {
            crate::index::relative_resolved(&self.root, p).unwrap_or_else(|| path.to_string())
        } else {
            path.trim_start_matches("./").to_string()
        }
    }

    /// The innermost symbol whose lines hold `line` in `path`.
    pub fn enclosing(&self, path: &Path, line: u32) -> Result<Option<Symbol>, Error> {
        let rel = self.rel(&path.to_string_lossy());
        Ok(self
            .symbols_where(
                "file_path = ?1 AND start_line <= ?2 AND end_line >= ?2 AND kind NOT IN ('file', 'import')
                 ORDER BY end_line - start_line, start_line DESC LIMIT 1",
                &[&rel, &line],
            )?
            .into_iter()
            .next())
    }

    fn nid(&self, id: &str) -> Result<Option<i64>, Error> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row("SELECT nid FROM nodes WHERE id = ?1", [id], |r| r.get(0))
            .optional()?)
    }

    /// Breadth-first over edges of `kinds`, from the symbols `start`:
    /// incoming (`reverse`) or outgoing.
    fn reach(
        &self,
        start: &[i64],
        kinds: &str,
        reverse: bool,
        depth: u32,
    ) -> Result<Vec<Reach>, Error> {
        let (near, far) = if reverse {
            ("target", "source")
        } else {
            ("source", "target")
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT e.{far}, e.kind, e.line, {} FROM edges e JOIN nodes n ON n.nid = e.{far}
             WHERE e.{near} = ?1 AND e.kind IN {kinds} AND n.kind != 'import'
             ORDER BY n.file_path, n.start_line",
            SYMBOL_COLUMNS
                .split(", ")
                .map(|c| format!("n.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let mut seen: HashSet<i64> = start.iter().copied().collect();
        let mut queue: VecDeque<(i64, u32)> = start.iter().map(|&n| (n, 0)).collect();
        let mut out = Vec::new();
        while let Some((nid, d)) = queue.pop_front() {
            if d >= depth || out.len() >= MAX_RESULTS {
                continue;
            }
            let rows = stmt.query_map([nid], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<u32>>(2)?,
                    Symbol::from_row_at(r, 3)?,
                ))
            })?;
            for row in rows {
                let (next, via, line, symbol) = row?;
                if !seen.insert(next) {
                    continue;
                }
                out.push(Reach {
                    symbol,
                    depth: d + 1,
                    via,
                    line,
                });
                queue.push_back((next, d + 1));
            }
        }
        Ok(out)
    }

    fn start(&self, id: &str) -> Result<Vec<i64>, Error> {
        self.nid(id)?
            .map(|n| vec![n])
            .ok_or_else(|| Error(format!("no symbol {id}")))
    }

    /// What calls, constructs or names the symbol, up to `depth` hops back.
    pub fn callers(&self, id: &str, depth: u32) -> Result<Vec<Reach>, Error> {
        self.reach(&self.start(id)?, CALL_KINDS, true, depth)
    }

    /// What the symbol calls, constructs or names, up to `depth` hops on.
    pub fn callees(&self, id: &str, depth: u32) -> Result<Vec<Reach>, Error> {
        self.reach(&self.start(id)?, CALL_KINDS, false, depth)
    }

    /// Everything that depends on the symbol or on anything it contains
    /// (a class's methods), through any edge but containment, up to
    /// `depth` hops back: what a change to it could break.
    pub fn impact(&self, id: &str, depth: u32) -> Result<Vec<Reach>, Error> {
        let mut start = self.start(id)?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT target FROM edges WHERE source = ?1 AND kind = 'contains'")?;
        let mut i = 0;
        while i < start.len() && start.len() < MAX_RESULTS {
            let children: Vec<i64> = stmt
                .query_map([start[i]], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for c in children {
                if !start.contains(&c) {
                    start.push(c);
                }
            }
            i += 1;
        }
        self.reach(&start, DEPENDENCY_KINDS, true, depth)
    }

    /// Every use of `name` the graph recorded, in file and line order: the
    /// calls, constructions and references linked to any definition of
    /// that name, and those written with it as their last segment (`foo`,
    /// `a.foo`, `m::foo`) that linking could not attribute. One entry per
    /// place, with every definition linked there.
    pub fn uses_of(&self, name: &str) -> Result<Vec<Site>, Error> {
        let mut by_place: std::collections::BTreeMap<
            (String, u32, Option<u32>, String),
            Vec<String>,
        > = std::collections::BTreeMap::new();
        let mut linked = self.conn.prepare_cached(&format!(
            "SELECT s.file_path, e.line, e.col, e.kind, t.id FROM edges e
             JOIN nodes t ON t.nid = e.target JOIN nodes s ON s.nid = e.source
             WHERE t.name = ?1 AND e.kind IN {CALL_KINDS} AND e.line IS NOT NULL"
        ))?;
        for row in linked.query_map([name], |r| {
            Ok((
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?),
                r.get::<_, String>(4)?,
            ))
        })? {
            let (place, target) = row?;
            by_place.entry(place).or_default().push(target);
        }
        let mut unlinked = self.conn.prepare_cached(
            "SELECT n.file_path, r.line, r.col, r.kind FROM refs r JOIN nodes n ON n.nid = r.from_node
             WHERE r.status = ?1 AND r.tail = ?2 AND r.kind IN ('calls', 'instantiates', 'function_ref')",
        )?;
        for row in unlinked.query_map(params![crate::link::FAILED, name], |r| {
            let kind: String = r.get(3)?;
            let kind = if kind == "function_ref" {
                "references".to_string()
            } else {
                kind
            };
            Ok((r.get(0)?, r.get(1)?, Some(r.get(2)?), kind))
        })? {
            by_place.entry(row?).or_default();
        }
        Ok(by_place
            .into_iter()
            .map(|((path, line, col, kind), mut targets)| {
                targets.sort();
                targets.dedup();
                Site {
                    path,
                    line,
                    col,
                    kind,
                    targets,
                }
            })
            .collect())
    }

    /// The files the graph read and found the symbols of, relative to the
    /// work tree: not those it could not walk.
    pub fn files(&self) -> Result<Vec<String>, Error> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT path FROM files WHERE error IS NULL ORDER BY path")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The symbols most relevant to a question, best first, with their
    /// callers and callees: full-text ranking over names (split into
    /// words), qualified names, docstrings and signatures, with exact
    /// names first and tests after the code they test.
    pub fn explore(&self, query: &str) -> Result<Vec<Explored>, Error> {
        let mut found = self.symbols(&SymbolQuery {
            text: Some(query.to_string()),
            ..SymbolQuery::default()
        })?;
        found.retain(|s| {
            !matches!(
                s.kind.as_str(),
                "parameter" | "variable" | "field" | "property"
            ) || s.exported
        });
        let asks_for_tests = words(query)
            .split(' ')
            .any(|w| w.starts_with("test") || w == "spec");
        let n = found.len().max(1) as f64;
        let mut scored: Vec<(f64, Symbol)> = found
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let mut score = 1.0 - i as f64 / n;
                if !asks_for_tests && is_test_path(&s.path) {
                    score *= 0.3;
                }
                if matches!(
                    s.kind.as_str(),
                    "class" | "struct" | "interface" | "trait" | "function"
                ) {
                    score *= 1.2;
                }
                (score, s)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut names = self
            .conn
            .prepare_cached("SELECT n.qualified_name FROM edges e JOIN nodes n ON n.nid = e.source WHERE e.target = ?1 AND e.kind IN ('calls', 'instantiates', 'references') ORDER BY n.file_path, n.start_line")?;
        let mut callee_names = self
            .conn
            .prepare_cached("SELECT n.qualified_name FROM edges e JOIN nodes n ON n.nid = e.target WHERE e.source = ?1 AND e.kind IN ('calls', 'instantiates') ORDER BY e.line")?;
        let mut out = Vec::new();
        for (score, symbol) in scored.into_iter().take(MAX_RESULTS) {
            let Some(nid) = self.nid(&symbol.id)? else {
                continue;
            };
            let dedup = |v: Vec<String>| {
                let mut seen = HashSet::new();
                v.into_iter()
                    .filter(|x| seen.insert(x.clone()))
                    .collect::<Vec<_>>()
            };
            let callers = dedup(
                names
                    .query_map([nid], |r| r.get(0))?
                    .collect::<Result<_, _>>()?,
            );
            let callees = dedup(
                callee_names
                    .query_map([nid], |r| r.get(0))?
                    .collect::<Result<_, _>>()?,
            );
            out.push(Explored {
                symbol,
                score,
                caller_count: callers.len(),
                callee_count: callees.len(),
                callers,
                callees,
            });
        }
        Ok(out)
    }

    /// The test files that depend, directly or through other files, on any
    /// of `files` (absolute or relative to the work tree).
    pub fn affected_tests(&self, files: &[PathBuf]) -> Result<Affected, Error> {
        let mut affected = Affected::default();
        let mut parent: HashMap<String, Option<String>> = HashMap::new();
        let mut queue = VecDeque::new();
        let mut known = self
            .conn
            .prepare_cached("SELECT 1 FROM files WHERE path = ?1 AND error IS NULL")?;
        for f in files {
            let rel = self.rel(&f.to_string_lossy());
            if !known.exists([&rel])? {
                affected.unknown.push(f.clone());
                continue;
            }
            if parent.insert(rel.clone(), None).is_none() {
                queue.push_back(rel);
            }
        }
        let mut dependents = self.conn.prepare_cached(&format!(
            "SELECT DISTINCT src.file_path FROM nodes tgt
             JOIN edges e ON e.target = tgt.nid AND e.kind IN {DEPENDENCY_KINDS}
             JOIN nodes src ON src.nid = e.source
             WHERE tgt.file_path = ?1 AND src.file_path != ?1 ORDER BY src.file_path"
        ))?;
        while let Some(file) = queue.pop_front() {
            if is_test_path(&file) {
                let mut via = vec![PathBuf::from(&file)];
                let mut at = parent.get(&file).cloned().flatten();
                while let Some(p) = at {
                    via.push(PathBuf::from(&p));
                    at = parent.get(&p).cloned().flatten();
                }
                via.reverse();
                affected.tests.push(AffectedTest {
                    path: PathBuf::from(&file),
                    via,
                });
            }
            let next: Vec<String> = dependents
                .query_map([&file], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for n in next {
                if !parent.contains_key(&n) {
                    parent.insert(n.clone(), Some(file.clone()));
                    queue.push_back(n);
                }
            }
        }
        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_words_split_case_and_separators() {
        assert_eq!(words("processPayment"), "process payment");
        assert_eq!(words("HTTPServer_v2"), "httpserver v 2");
        assert_eq!(
            words("Sidekiq::Client::push_bulk"),
            "sidekiq client push bulk"
        );
    }

    #[test]
    fn questions_lose_their_stopwords() {
        assert_eq!(
            fts_terms("how does the retry backoff work?"),
            ["\"retry\"*", "\"backoff\"*"]
        );
    }

    #[test]
    fn test_paths_follow_codegraph_conventions() {
        for t in [
            "src/foo_test.go",
            "tests/cli.rs",
            "pkg/test_x.py",
            "a/b.spec.ts",
            "src/__tests__/a.ts",
            "lib/FooTest.java",
            "spec/a_spec.rb",
            "x/testdata/y.go",
            "db/db_unittest.cc",
        ] {
            assert!(is_test_path(t), "{t}");
        }
        for t in [
            "src/latest.rs",
            "src/manifest.ts",
            "lib/contest.rb",
            "src/testing_utils_impl.rs",
        ] {
            assert!(!is_test_path(t), "{t}");
        }
    }
}
