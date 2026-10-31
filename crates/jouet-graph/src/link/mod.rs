//! Linking: resolving each reference extraction found (a call, an import,
//! a base class) to the symbol it names, as an edge.
//!
//! Derived from codegraph's `ReferenceResolver` (`src/resolution/`), whose
//! cascade it keeps in a much smaller form:
//! 1. **imports** resolve to the files their module names (`modules`), and
//!    an imported name to its definition in that file;
//! 2. **every other reference** resolves by name, preferring, in order: a
//!    name the file imports, a member of the type a `self`/`this` or a type
//!    qualifier names, the same file, the files the file imports, the same
//!    directory (a Go or Java package), and finally the one definition of
//!    that name in the whole project;
//! 3. a **visibility gate** drops a target its language hides from the
//!    referring file (an unexported Go name in another package, a private
//!    member in another file).
//!
//! Not ported: codegraph's framework routing (Express, Rails, Spring, …),
//! its synthesizers for callbacks and function pointers, receiver type
//! inference, and the hundreds of language-specific matchers in
//! `name-matcher.ts`. A reference nothing resolves stays unresolved rather
//! than guessed.
//!
//! References are kept after they resolve, so linking is incremental: a
//! re-indexed file's own references are new, an edge whose target was
//! deleted puts its reference back to pending (a trigger), and an
//! unresolved reference is retried when a re-indexed file defines its name.

mod modules;

use crate::Error;
use modules::Files;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(crate) const PENDING: i64 = 0;
pub(crate) const LINKED: i64 = 1;
pub(crate) const FAILED: i64 = 2;

/// A node, as linking needs it.
struct N {
    nid: i64,
    kind: String,
    name: String,
    qn: String,
    file: String,
    language: String,
    start: u32,
    end: u32,
    exported: bool,
    visibility: Option<String>,
}

struct PendingRef {
    id: i64,
    from: i64,
    name: String,
    kind: String,
    line: u32,
    col: u32,
}

fn family(language: &str) -> &str {
    match language {
        "typescript" | "tsx" | "javascript" | "jsx" => "js",
        "c" | "cpp" => "c",
        other => other,
    }
}

/// The last segment of a reference's name: what a re-indexed file must
/// define for an unresolved reference to be worth retrying.
pub(crate) fn tail(name: &str) -> &str {
    let name = name.trim_end_matches("()");
    let name = name.rsplit_once('/').map_or(name, |(_, f)| f);
    // A path's file extension (`job_util.rb`, `server.h`) is not a name.
    let name = match name.rsplit_once('.') {
        Some((stem, ext))
            if crate::extract::language_for_path(name).is_some() && !ext.is_empty() =>
        {
            stem
        }
        _ => name,
    };
    name.rfind(['.', ':', '>']).map_or(name, |i| &name[i + 1..])
}

/// Splits `self.store().put` into `["self", "store", "put"]`, and
/// `crate::a::B` into `["crate", "a", "B"]`.
fn segments(name: &str) -> Vec<&str> {
    name.split(['.', ':'])
        .flat_map(|s| s.split("->"))
        .map(|s| s.trim_end_matches("()").trim_start_matches('@'))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether `qn` is `owner`'s `member`: `Client::push`, `a::Client::push`
/// or `Client.push`, but never `HttpClient::push`.
fn names_member(qn: &str, owner: &str, member: &str) -> bool {
    [format!("{owner}::{member}"), format!("{owner}.{member}")]
        .iter()
        .any(|tail| {
            qn.strip_suffix(tail.as_str())
                .is_some_and(|head| head.is_empty() || head.ends_with("::") || head.ends_with('.'))
        })
}

/// Kinds a reference of `kind` can name.
fn accepts(kind: &str, target: &str) -> bool {
    match kind {
        "calls" => matches!(
            target,
            "function" | "method" | "class" | "struct" | "constant" | "variable"
        ),
        "function_ref" => matches!(
            target,
            "function"
                | "method"
                | "class"
                | "struct"
                | "constant"
                | "variable"
                | "field"
                | "property"
        ),
        "instantiates" => matches!(target, "class" | "struct" | "method" | "function"),
        "extends" | "implements" => matches!(
            target,
            "class" | "struct" | "interface" | "trait" | "protocol" | "module"
        ),
        "decorates" => matches!(target, "function" | "class" | "interface"),
        _ => matches!(
            target,
            "class"
                | "struct"
                | "interface"
                | "trait"
                | "protocol"
                | "enum"
                | "type_alias"
                | "union"
                | "module"
                | "namespace"
                | "constant"
                | "variable"
                | "function"
                | "enum_member"
                | "field"
                | "property"
        ),
    }
}

/// Method names so common that a call through a receiver jouet knows
/// nothing about is never linked by name alone.
const COMMON: &[&str] = &[
    "get",
    "set",
    "add",
    "push",
    "pop",
    "len",
    "size",
    "update",
    "items",
    "keys",
    "values",
    "map",
    "filter",
    "append",
    "remove",
    "clear",
    "close",
    "open",
    "read",
    "write",
    "run",
    "call",
    "send",
    "new",
    "init",
    "start",
    "stop",
    "next",
    "copy",
    "clone",
    "format",
    "join",
    "split",
    "trim",
    "parse",
    "load",
    "save",
    "find",
    "contains",
    "insert",
    "delete",
    "lock",
    "unlock",
    "print",
    "log",
    "to_string",
    "toString",
    "equals",
    "hashCode",
    "dup",
    "each",
    "is_empty",
    "unwrap",
    "iter",
    "collect",
    "as_str",
    "as_ref",
    "into",
    "from",
    "default",
    "apply",
    "then",
    "catch",
    "resolve",
    "reject",
    "emit",
    "on",
    "off",
    "has",
    "replace",
    "reset",
    "flush",
    "error",
    "warn",
    "info",
    "debug",
    "Error",
    "String",
    "length",
    "count",
    "first",
    "last",
    "build",
    "create",
    "handle",
    "process",
    "execute",
    "render",
    "fmt",
    "drop",
    "eq",
    "cmp",
    "extend",
    "path",
    "name",
    "id",
    "value",
    "kind",
    "type",
    "text",
    "data",
    "body",
    "url",
    "key",
    "version",
    "sort",
    "reverse",
    "wait",
    "exists",
    "is_some",
    "is_none",
    "ok",
    "err",
    "map_err",
    "and_then",
    "unwrap_or",
    "unwrap_or_default",
    "expect",
    "borrow",
    "borrow_mut",
    "to_owned",
    "to_vec",
    "to_f",
    "to_i",
    "to_s",
    "to_a",
    "to_h",
    "to_sym",
    "empty?",
    "include?",
    "any?",
    "nil?",
    "size?",
    "merge",
    "dig",
    "fetch",
    "slice",
    "entries",
    "forEach",
    "some",
    "every",
    "reduce",
    "includes",
    "indexOf",
    "toLowerCase",
    "toUpperCase",
    "test",
    "match",
    "exec",
    "query",
    "list",
    "lines",
    "chars",
    "bytes",
    "as_bytes",
    "display",
    "status",
    "result",
    "as_deref",
    "as_mut",
    "get_mut",
    "iter_mut",
    "into_iter",
    "filter_map",
    "flat_map",
    "find_map",
    "and",
    "or",
    "or_else",
    "ok_or",
    "ok_or_else",
    "take",
    "skip",
    "chain",
    "zip",
    "rev",
    "max",
    "min",
    "sum",
    "any",
    "all",
    "position",
    "retain",
    "truncate",
    "extend_from_slice",
    "starts_with",
    "ends_with",
    "strip_prefix",
    "strip_suffix",
    "to_lowercase",
    "to_uppercase",
    "push_str",
    "with_capacity",
    "is_file",
    "is_dir",
    "parent",
    "file_name",
    "canonicalize",
    "exists?",
];

/// What an imports reference resolved to.
enum Import {
    Targets(Vec<usize>),
    /// A name its module does not define but may re-export (`pub use`,
    /// `export { x } from`): looked up through the module's own imports
    /// once every import has bound, else linked to the module (`fallback`).
    ReExported {
        module: String,
        name: String,
        fallback: Vec<usize>,
    },
}

/// What linking did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Linked {
    pub(crate) resolved: usize,
    pub(crate) unresolved: usize,
}

struct Linker<'c> {
    conn: &'c Connection,
    /// Every node is loaded; nothing needs fetching.
    complete: bool,
    /// Files whose linked imports are loaded into `bindings`.
    bindings_loaded: HashSet<String>,
    nodes: Vec<N>,
    by_nid: HashMap<i64, usize>,
    by_name: HashMap<String, Vec<usize>>,
    by_file: HashMap<String, Vec<usize>>,
    files: Files,
    /// Per file: the files its imports resolved to.
    imported_files: HashMap<String, HashSet<String>>,
    /// Per file: an imported local name and what it resolved to.
    bindings: HashMap<String, HashMap<String, Vec<usize>>>,
    /// Per file: every name its imports bind, resolved or not.
    import_names: HashMap<String, HashSet<String>>,
}

/// What re-indexed files now offer that they did not before: names a
/// failed reference might have been waiting for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Offered {
    /// Names of symbols a file defines now but did not before.
    pub(crate) names: std::collections::BTreeSet<String>,
    /// Files that were not indexed before, which an import may name.
    pub(crate) files: Vec<String>,
}

/// The names `path` defines, before it is rewritten.
pub(crate) fn names_in(conn: &Connection, path: &str) -> Result<HashSet<String>, Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT name FROM nodes WHERE file_path = ?1 AND kind NOT IN ('file', 'import')",
    )?;
    let names = stmt.query_map([path], |r| r.get(0))?;
    Ok(names.collect::<Result<_, _>>()?)
}

/// Retries the unresolved references that what re-indexed files offer
/// could now satisfy: by the new names, and by a new file's stem or
/// directory (the last segment of an import that names it). Retrying by
/// every name a file defines would retry thousands of `run`s and `new`s
/// on each edit, to fail again.
pub(crate) fn reopen(conn: &Connection, offered: &Offered) -> Result<(), Error> {
    let mut by_tail =
        conn.prepare_cached("UPDATE refs SET status = 0 WHERE status = 2 AND tail = ?1")?;
    for name in &offered.names {
        by_tail.execute([name])?;
    }
    for path in &offered.files {
        by_tail.execute([modules::stem(path)])?;
        by_tail.execute([modules::file_name(modules::dir_of(path))])?;
    }
    Ok(())
}

/// Resolves every pending reference.
pub(crate) fn link(conn: &mut Connection, root: &Path) -> Result<Linked, Error> {
    let pending_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM refs WHERE status = ?1",
        [PENDING],
        |r| r.get(0),
    )?;
    if pending_count == 0 {
        return Ok(Linked::default());
    }
    let pending = {
        let mut stmt = conn.prepare(
            "SELECT id, from_node, name, kind, line, col FROM refs WHERE status = ?1
             ORDER BY kind != 'imports', id",
        )?;
        let rows = stmt.query_map([PENDING], |r| {
            Ok(PendingRef {
                id: r.get(0)?,
                from: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                line: r.get(4)?,
                col: r.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let tx = conn.transaction()?;
    let mut linker = Linker::load(&tx, root, &pending)?;
    let mut report = Linked::default();
    {
        let mut edge = tx.prepare_cached(
            "INSERT OR IGNORE INTO edges(source, target, kind, line, col, provenance, ref_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 'link', ?6)",
        )?;
        let mut mark = tx.prepare_cached("UPDATE refs SET status = ?2, tail = ?3 WHERE id = ?1")?;
        let mut record =
            |r: &PendingRef, targets: &[usize], linker: &Linker<'_>| -> Result<(), Error> {
                let kind = match r.kind.as_str() {
                    "function_ref" => "references",
                    k => k,
                };
                let mut linked = false;
                for &t in targets {
                    let target = linker.nodes[t].nid;
                    if target == r.from {
                        continue;
                    }
                    edge.execute(params![r.from, target, kind, r.line, r.col, r.id])?;
                    linked = true;
                }
                mark.execute(params![
                    r.id,
                    if linked { LINKED } else { FAILED },
                    tail(&r.name)
                ])?;
                if linked {
                    report.resolved += 1;
                } else {
                    report.unresolved += 1;
                }
                Ok(())
            };
        let (imports, names): (Vec<&PendingRef>, Vec<&PendingRef>) =
            pending.iter().partition(|r| r.kind == "imports");
        // Imports first, so names see what their file imports; a name a
        // module only re-exports waits until every import has bound.
        let mut deferred = Vec::new();
        for r in imports {
            let Some(&from) = linker.by_nid.get(&r.from) else {
                continue;
            };
            match linker.resolve_import(from, r) {
                Import::Targets(targets) => record(r, &targets, &linker)?,
                Import::ReExported {
                    module,
                    name,
                    fallback,
                } => deferred.push((r, from, module, name, fallback)),
            }
        }
        for (r, from, module, name, fallback) in deferred {
            let target = linker.reexported(&module, &name)?;
            match target {
                Some(t) => {
                    linker.record_import(from, &r.name, t);
                    record(r, &[t], &linker)?;
                }
                None => record(r, &fallback, &linker)?,
            }
        }
        // A name through an imported module may be one that module only
        // re-exports: its own imports must be loaded first.
        for r in &names {
            let Some(&from) = linker.by_nid.get(&r.from) else {
                continue;
            };
            let file = linker.nodes[from].file.clone();
            let root_name = segments(&r.name).first().map(|s| s.to_string());
            let modules: Vec<String> = root_name
                .and_then(|n| linker.bindings.get(&file).and_then(|b| b.get(&n)).cloned())
                .unwrap_or_default()
                .into_iter()
                .filter(|&t| linker.nodes[t].kind == "file")
                .map(|t| linker.nodes[t].file.clone())
                .collect();
            for m in modules {
                linker.ensure_bindings(&m)?;
            }
        }
        for r in names {
            let Some(&from) = linker.by_nid.get(&r.from) else {
                continue;
            };
            let targets: Vec<usize> = linker.resolve_name(from, r).into_iter().collect();
            record(r, &targets, &linker)?;
        }
    }
    drop(linker);
    tx.commit()?;
    if report.resolved + report.unresolved > CHECKPOINT_AFTER {
        // A large link leaves a write-ahead log as large as the database;
        // fold it back. Best effort: a reader holding it only delays this.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    Ok(report)
}

/// References linked in one go before the write-ahead log is folded back.
const CHECKPOINT_AFTER: usize = 10_000;

/// The node columns linking reads, for a `SELECT` with `n` as the alias.
const NODE_COLUMNS: &str = "n.nid, n.kind, n.name, n.qualified_name, n.file_path, n.language, n.start_line, n.end_line, n.is_exported, n.visibility";

fn node_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<N> {
    Ok(N {
        nid: r.get(at)?,
        kind: r.get(at + 1)?,
        name: r.get(at + 2)?,
        qn: r.get(at + 3)?,
        file: r.get(at + 4)?,
        language: r.get(at + 5)?,
        start: r.get(at + 6)?,
        end: r.get(at + 7)?,
        exported: r.get(at + 8)?,
        visibility: r.get(at + 9)?,
    })
}

/// Fewer pending references than this load only the nodes they can name
/// (an edit's file), rather than the whole graph (a full index).
const SCOPED_UNDER: usize = 2000;

impl<'c> Linker<'c> {
    fn load(conn: &'c Connection, root: &Path, pending: &[PendingRef]) -> Result<Self, Error> {
        let paths: Vec<String> = {
            let mut s = conn.prepare("SELECT path FROM files")?;
            s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let mut linker = Linker {
            conn,
            complete: pending.len() >= SCOPED_UNDER,
            bindings_loaded: HashSet::new(),
            nodes: Vec::new(),
            by_nid: HashMap::new(),
            by_name: HashMap::new(),
            by_file: HashMap::new(),
            files: Files::new(root, paths),
            imported_files: HashMap::new(),
            bindings: HashMap::new(),
            import_names: HashMap::new(),
        };
        if linker.complete {
            let mut stmt = conn.prepare(&format!("SELECT {NODE_COLUMNS} FROM nodes n"))?;
            for n in stmt.query_map([], |r| node_at(r, 0))? {
                linker.add(n?);
            }
            let mut s = conn.prepare(
                "SELECT n.file_path, r.name FROM refs r JOIN nodes n ON n.nid = r.from_node
                 WHERE r.kind = 'imports'",
            )?;
            for row in s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (file, name) = row?;
                linker
                    .import_names
                    .entry(file)
                    .or_default()
                    .insert(tail(&name).to_string());
            }
            // Imports linked earlier, so a name in a file whose imports are
            // not pending still sees what the file imports.
            let mut s = conn.prepare(
                "SELECT r.from_node, r.name, e.target FROM refs r JOIN edges e ON e.ref_id = r.id
                 WHERE r.kind = 'imports' AND r.status = 1",
            )?;
            let rows = s.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (from, name, target) = row?;
                if let (Some(&f), Some(&t)) = (linker.by_nid.get(&from), linker.by_nid.get(&target))
                {
                    linker.record_import(f, &name, t);
                }
            }
            return Ok(linker);
        }
        // Scoped: the files the references are in, every node a segment of
        // a reference names, and every file node (an import names a file).
        conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS link_names(name TEXT PRIMARY KEY) WITHOUT ROWID;
             CREATE TEMP TABLE IF NOT EXISTS link_from(nid INTEGER PRIMARY KEY);
             DELETE FROM link_names; DELETE FROM link_from;",
        )?;
        {
            let mut name =
                conn.prepare_cached("INSERT OR IGNORE INTO link_names(name) VALUES (?1)")?;
            let mut from =
                conn.prepare_cached("INSERT OR IGNORE INTO link_from(nid) VALUES (?1)")?;
            for r in pending {
                from.execute([r.from])?;
                for seg in segments(&r.name) {
                    name.execute([seg])?;
                }
                name.execute([tail(&r.name)])?;
            }
        }
        let mut stmt = conn.prepare(&format!(
            "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.kind = 'file'
               OR n.name IN (SELECT name FROM link_names)
               OR n.file_path IN (SELECT f.file_path FROM nodes f JOIN link_from l ON l.nid = f.nid)"
        ))?;
        for n in stmt.query_map([], |r| node_at(r, 0))? {
            linker.add(n?);
        }
        let from_files: HashSet<String> = pending
            .iter()
            .filter_map(|r| {
                linker
                    .by_nid
                    .get(&r.from)
                    .map(|&i| linker.nodes[i].file.clone())
            })
            .collect();
        for file in &from_files {
            linker.ensure_bindings(file)?;
        }
        Ok(linker)
    }

    fn add(&mut self, n: N) -> usize {
        if let Some(&i) = self.by_nid.get(&n.nid) {
            return i;
        }
        let i = self.nodes.len();
        self.by_nid.insert(n.nid, i);
        if n.kind != "import" && n.kind != "file" {
            self.by_name.entry(n.name.clone()).or_default().push(i);
        }
        self.by_file.entry(n.file.clone()).or_default().push(i);
        self.nodes.push(n);
        i
    }

    /// Loads what `file`'s imports bound when they were linked, and the
    /// names they import, unless every node is loaded already.
    fn ensure_bindings(&mut self, file: &str) -> Result<(), Error> {
        if self.complete || !self.bindings_loaded.insert(file.to_string()) {
            return Ok(());
        }
        let mut names = self.conn.prepare_cached(
            "SELECT r.name FROM refs r JOIN nodes n ON n.nid = r.from_node
             WHERE n.file_path = ?1 AND r.kind = 'imports'",
        )?;
        let list: Vec<String> = names
            .query_map([file], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let set = self.import_names.entry(file.to_string()).or_default();
        set.extend(list.iter().map(|n| tail(n).to_string()));
        let target_columns = NODE_COLUMNS.replace("n.", "t.");
        let mut linked = self.conn.prepare_cached(&format!(
            "SELECT r.name, {NODE_COLUMNS}, {target_columns} FROM refs r
             JOIN nodes n ON n.nid = r.from_node JOIN edges e ON e.ref_id = r.id
             JOIN nodes t ON t.nid = e.target
             WHERE n.file_path = ?1 AND r.kind = 'imports' AND r.status = 1"
        ))?;
        let rows: Vec<(String, N, N)> = linked
            .query_map([file], |r| Ok((r.get(0)?, node_at(r, 1)?, node_at(r, 11)?)))?
            .collect::<Result<_, _>>()?;
        for (name, from, target) in rows {
            let (f, t) = (self.add(from), self.add(target));
            self.record_import(f, &name, t);
        }
        Ok(())
    }

    /// The file node of `path`.
    fn file_node(&self, path: &str) -> Option<usize> {
        self.by_file
            .get(path)?
            .iter()
            .copied()
            .find(|&i| self.nodes[i].kind == "file")
    }

    /// The import node whose lines hold `line` in `from`'s file, if any.
    fn import_node_at(&self, file: &str, line: u32) -> Option<usize> {
        self.by_file.get(file)?.iter().copied().find(|&i| {
            let n = &self.nodes[i];
            n.kind == "import" && n.start <= line && line <= n.end
        })
    }

    fn record_import(&mut self, from: usize, name: &str, target: usize) {
        let file = self.nodes[from].file.clone();
        let target_node = &self.nodes[target];
        let local = if target_node.kind == "file" {
            // `import x` / Go's `"a/b/render"` binds the module's last name.
            tail(name).to_string()
        } else {
            target_node.name.clone()
        };
        self.imported_files
            .entry(file.clone())
            .or_default()
            .insert(target_node.file.clone());
        let list = self
            .bindings
            .entry(file)
            .or_default()
            .entry(local)
            .or_default();
        if !list.contains(&target) {
            list.push(target);
        }
    }

    /// The files (as file nodes), or the symbol, an imports reference names.
    fn resolve_import(&mut self, from: usize, r: &PendingRef) -> Import {
        let file = self.nodes[from].file.clone();
        let language = self.nodes[from].language.clone();
        let module = self
            .import_node_at(&file, r.line)
            .map(|i| self.nodes[i].name.clone());
        let mut out = Vec::new();
        // The module itself, or a reference that is a path on its own
        // (rust, java, go, ruby, c).
        let is_binding = module.as_deref().is_some_and(|m| m != r.name)
            && matches!(family(&language), "js" | "python");
        if !is_binding {
            let mut spec = r.name.clone();
            if language == "rust" {
                if let Some((path, rest)) = self.files.rust_module(&file, &spec) {
                    let in_file = rest
                        .last()
                        .and_then(|name| self.symbol_in(std::slice::from_ref(&path), name));
                    match (in_file, rest.as_slice()) {
                        (Some(t), _) => out.push(t),
                        (None, []) => out.extend(self.file_node(&path)),
                        (None, [name]) if path != file => {
                            return Import::ReExported {
                                module: path.clone(),
                                name: name.clone(),
                                fallback: self.file_node(&path).into_iter().collect(),
                            };
                        }
                        _ => {}
                    }
                }
            } else {
                if language == "python" {
                    spec = spec.trim_end_matches(".*").to_string();
                }
                for path in self.files.resolve(&language, &file, &spec) {
                    if path != file {
                        out.extend(self.file_node(&path));
                    }
                }
                if out.is_empty() && language == "java" {
                    // A static import names a member of its class.
                    if let Some((class_path, member)) = spec.rsplit_once('.') {
                        let files = self.files.resolve("java", &file, class_path);
                        out.extend(self.symbol_in(&files, member));
                    }
                    if spec.ends_with(".*")
                        || (out.is_empty()
                            && spec
                                .split('.')
                                .next_back()
                                .is_some_and(|s| s.starts_with(char::is_lowercase)))
                    {
                        let package = spec.trim_end_matches(".*");
                        for p in self.files.java_package_files(package) {
                            out.extend(self.file_node(&p));
                        }
                    }
                }
            }
        } else if let Some(module) = module {
            // A name imported from a module (`import { a } from './m'`,
            // `from .m import a`): its definition there, or a submodule.
            let files = self.files.resolve(&language, &file, &module);
            if let Some(t) = self.symbol_in(&files, &r.name) {
                out.push(t);
            } else if let (Some(module_file), "js") = (files.first(), family(&language)) {
                return Import::ReExported {
                    module: module_file.clone(),
                    name: r.name.clone(),
                    fallback: self.file_node(module_file).into_iter().collect(),
                };
            } else if language == "python" {
                let sub = if module.ends_with('.') {
                    format!("{module}{}", r.name)
                } else {
                    format!("{module}.{}", r.name)
                };
                for path in self.files.resolve("python", &file, &sub) {
                    out.extend(self.file_node(&path));
                }
            }
        }
        for &t in &out {
            self.record_import(from, &r.name, t);
        }
        Import::Targets(out)
    }

    /// The definition `module` makes available as `name` through its own
    /// imports, following up to three re-exports.
    fn reexported(&mut self, module: &str, name: &str) -> Result<Option<usize>, Error> {
        let mut file = module.to_string();
        for _ in 0..3 {
            self.ensure_bindings(&file)?;
            let Some(hit) = self
                .bindings
                .get(&file)
                .and_then(|b| b.get(name))
                .and_then(|v| v.first().copied())
            else {
                return Ok(None);
            };
            if self.nodes[hit].kind != "file" {
                return Ok(Some(hit));
            }
            file = self.nodes[hit].file.clone();
            if let Some(t) = self.symbol_in(std::slice::from_ref(&file), name) {
                return Ok(Some(t));
            }
        }
        Ok(None)
    }

    /// The definition named `name` in one of `files`, preferring exported
    /// top-level declarations.
    fn symbol_in(&self, files: &[String], name: &str) -> Option<usize> {
        files
            .iter()
            .filter_map(|f| self.by_file.get(f))
            .flatten()
            .copied()
            .filter(|&i| {
                let n = &self.nodes[i];
                n.name == name && n.kind != "import" && n.kind != "file"
            })
            .min_by_key(|&i| {
                let n = &self.nodes[i];
                (
                    !n.exported,
                    n.qn.contains("::") || n.qn.contains('.'),
                    n.start,
                )
            })
    }

    /// The enclosing type of a node, by its qualified name
    /// (`Sidekiq::Client::push` → `Sidekiq::Client`).
    fn owner_of(&self, from: usize) -> Option<String> {
        let n = &self.nodes[from];
        let cut = n.qn.rfind("::").or_else(|| n.qn.rfind('.'))?;
        Some(n.qn[..cut].to_string())
    }

    fn visible(&self, from: usize, target: usize) -> bool {
        let (f, t) = (&self.nodes[from], &self.nodes[target]);
        if f.file == t.file {
            return true;
        }
        if t.visibility.as_deref() == Some("private") {
            return false;
        }
        if t.language == "go" {
            let exported = t.name.starts_with(|c: char| c.is_uppercase());
            if !exported && modules::dir_of(&f.file) != modules::dir_of(&t.file) {
                return false;
            }
        }
        true
    }

    fn resolve_name(&self, from: usize, r: &PendingRef) -> Option<usize> {
        let segs = segments(&r.name);
        let (member, qualifier) = segs.split_last()?;
        let fnode = &self.nodes[from];
        let fam = family(&fnode.language);
        let candidates: Vec<usize> = self
            .by_name
            .get(*member)?
            .iter()
            .copied()
            .filter(|&i| {
                let n = &self.nodes[i];
                i != from
                    && accepts(&r.kind, &n.kind)
                    && family(&n.language) == fam
                    && self.visible(from, i)
            })
            .collect();
        let empty = HashMap::new();
        let bindings = self.bindings.get(&fnode.file).unwrap_or(&empty);

        // 1. A name the file imports, or a member of what an imported
        //    name stands for (a module, a Go package, a class).
        if qualifier.is_empty() {
            if let Some(ts) = bindings.get(*member)
                && let Some(&t) = ts.iter().find(|&&t| candidates.contains(&t))
            {
                return Some(t);
            }
        } else if let Some(ts) = bindings.get(qualifier[0]) {
            // Through a module that re-exports the name (`requests.post`,
            // which `requests/__init__.py` imports from `.api`).
            for &t in ts {
                let tn = &self.nodes[t];
                if tn.kind != "file" || qualifier.len() != 1 {
                    continue;
                }
                let reexported = self
                    .bindings
                    .get(&tn.file)
                    .and_then(|b| b.get(*member))
                    .and_then(|rs| rs.iter().find(|&&x| candidates.contains(&x)));
                let defined_here = candidates.iter().any(|&c| self.nodes[c].file == tn.file);
                if let (Some(&x), false) = (reexported, defined_here) {
                    return Some(x);
                }
            }
            for &t in ts {
                let tn = &self.nodes[t];
                let hit = candidates.iter().copied().find(|&c| {
                    let cn = &self.nodes[c];
                    if tn.kind == "file" {
                        cn.file == tn.file
                            || (fam == "go"
                                && modules::dir_of(&cn.file) == modules::dir_of(&tn.file))
                    } else {
                        cn.qn
                            .strip_prefix(tn.qn.as_str())
                            .is_some_and(|rest| rest.starts_with("::") || rest.starts_with('.'))
                    }
                });
                if hit.is_some() {
                    return hit;
                }
            }
        }

        // 2. `self`/`this`/`cls`/`Self`: a member of the enclosing type.
        // A bare name the file imports from outside the project (`Path`
        // after `use std::path::Path`) is that library's.
        if qualifier.is_empty()
            && !bindings.contains_key(*member)
            && self
                .import_names
                .get(&fnode.file)
                .is_some_and(|names| names.contains(*member))
        {
            return None;
        }

        // A receiver the file imports from outside the project (`mock.patch`
        // after `from unittest import mock`): nothing here defines it.
        if let Some(root) = qualifier.first()
            && !bindings.contains_key(*root)
            && self
                .import_names
                .get(&fnode.file)
                .is_some_and(|names| names.contains(*root))
        {
            return None;
        }

        // `self.run()`, but not `self.store.run()`, whose receiver is a
        // field of unknown type.
        let is_self = matches!(qualifier, ["self" | "this" | "cls" | "Self" | "super"]);
        if (is_self || qualifier.is_empty())
            && let Some(owner) = self.owner_of(from)
        {
            let mut scope = owner.as_str();
            loop {
                let hit = candidates.iter().copied().find(|&c| {
                    let qn = &self.nodes[c].qn;
                    qn.strip_prefix(scope).is_some_and(|rest| {
                        rest.strip_prefix("::")
                            .or_else(|| rest.strip_prefix('.'))
                            .is_some_and(|m| m == *member)
                    })
                });
                if hit.is_some() {
                    return hit;
                }
                match scope.rfind("::").or_else(|| scope.rfind('.')) {
                    Some(cut) if !qualifier.is_empty() => scope = &scope[..cut],
                    _ => break,
                }
            }
        }

        // A Rust path through modules: `flags::complete::bash::generate()`,
        // `crate::util::f()`, or through a `use`d module.
        if fnode.language == "rust" && !qualifier.is_empty() && !is_self {
            if let Some(t) = self.rust_path(from, qualifier, member, &candidates) {
                return Some(t);
            }
            // A path through modules (`std::env::var`, `file::write`) that
            // names nothing in the project is a library's.
            let path = r.name.contains("::") && !r.name.contains('.');
            if path && qualifier[0].starts_with(|c: char| c.is_lowercase()) {
                return None;
            }
        }

        // 3. A type qualifier: `Client::new`, `Foo.bar`, `ns::f`.
        let type_like =
            |q: &str| q.starts_with(|c: char| c.is_uppercase()) || r.name.contains("::");
        if let Some(q) = qualifier.last().filter(|q| !is_self && type_like(q)) {
            let typed: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&c| names_member(&self.nodes[c].qn, q, member))
                .collect();
            if let Some(t) = self.pick(&typed) {
                return Some(t);
            }
            // A qualifier that is a project type, but no member of it by
            // that name: an inherited or library member. Don't guess.
            let qualifier_is_type = self.by_name.get(*q).is_some_and(|ts| {
                ts.iter().any(|&t| {
                    matches!(
                        self.nodes[t].kind.as_str(),
                        "class"
                            | "struct"
                            | "interface"
                            | "trait"
                            | "enum"
                            | "module"
                            | "namespace"
                    )
                })
            });
            if qualifier_is_type {
                return None;
            }
        }

        // Past this point nothing knows the receiver's type, so a common
        // method name (`get`, `push`, `parse`) is never guessed.
        let unknown_receiver = !qualifier.is_empty() && !is_self;
        if unknown_receiver && COMMON.contains(member) {
            return None;
        }
        // A field, property or enum member is reached through its owner
        // (above), never by its name alone; nor is a method by a bare name,
        // which can only call one of its own type's (above).
        let candidates: Vec<usize> = candidates
            .into_iter()
            .filter(|&c| {
                let kind = self.nodes[c].kind.as_str();
                !matches!(kind, "field" | "property" | "enum_member")
                    && (unknown_receiver || is_self || kind != "method")
            })
            .collect();

        // 4–6. The same file, the files it imports, the same directory.
        let same_file: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&c| self.nodes[c].file == fnode.file)
            .collect();
        let unqualified_ok = qualifier.is_empty() || is_self;
        if unqualified_ok && let Some(t) = self.pick(&same_file) {
            return Some(t);
        }
        if let Some(imported) = self.imported_files.get(&fnode.file) {
            let hits: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&c| imported.contains(&self.nodes[c].file))
                .collect();
            if let Some(t) = self.pick(&hits) {
                return Some(t);
            }
        }
        let dir = modules::dir_of(&fnode.file);
        let same_dir: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&c| modules::dir_of(&self.nodes[c].file) == dir)
            .collect();
        if unqualified_ok && let Some(t) = self.pick(&same_dir) {
            return Some(t);
        }

        // 7. The project's only definition of the name. Through a receiver
        //    jouet knows nothing about, only an uncommon method name.
        // A one-letter name (minified code, loop variables) or a function
        // named as a value (often a same-named local) is never a guess, nor
        // a call through a chain of fields (`call.cwd.as_deref`), whose
        // types nothing here knows.
        if member.chars().count() < 2 || r.kind == "function_ref" || qualifier.len() > 1 {
            return None;
        }
        // The rust walker writes a method chain it cannot follow as the bare
        // method name (`iter.collect()` → `collect`), so a bare rust name may
        // be a method: never a common one, and only within its own crate,
        // where a free function needs no `use`.
        let rust_bare = fnode.language == "rust" && qualifier.is_empty();
        if rust_bare && COMMON.contains(member) {
            return None;
        }
        let top: Vec<usize> = if unknown_receiver {
            candidates
                .iter()
                .copied()
                .filter(|&c| self.nodes[c].kind == "method")
                .collect()
        } else {
            candidates
        };
        // A rust name from another crate needs a `use` or a path, which
        // the tiers above follow; a guess stays within the crate.
        let top: Vec<usize> = if fnode.language == "rust" {
            let krate = self.files.rust_crate_src(&fnode.file);
            top.into_iter()
                .filter(|&c| self.files.rust_crate_src(&self.nodes[c].file) == krate)
                .collect()
        } else {
            top
        };
        (top.len() == 1).then(|| top[0])
    }

    /// The candidate a Rust module path names: the qualifier resolved as
    /// modules from a `use`d module, from the file (`self::`), from the
    /// crate root, or as a path of its own (`crate::`, a workspace crate).
    fn rust_path(
        &self,
        from: usize,
        qualifier: &[&str],
        member: &str,
        candidates: &[usize],
    ) -> Option<usize> {
        let file = &self.nodes[from].file;
        let joined = qualifier.join("::");
        let mut tries: Vec<(String, Vec<String>)> = Vec::new();
        if let Some(ts) = self.bindings.get(file).and_then(|b| b.get(qualifier[0])) {
            for &t in ts {
                let tn = &self.nodes[t];
                if tn.kind == "file" {
                    let rest = qualifier[1..].join("::");
                    let path = if rest.is_empty() {
                        "self".to_string()
                    } else {
                        format!("self::{rest}")
                    };
                    tries.extend(
                        self.files
                            .rust_module(&tn.file, &format!("{path}::{member}")),
                    );
                }
            }
        }
        if matches!(qualifier[0], "crate" | "self" | "super") || qualifier.len() > 1 {
            tries.extend(self.files.rust_module(file, &format!("{joined}::{member}")));
        }
        if !matches!(qualifier[0], "crate" | "self" | "super") {
            tries.extend(
                self.files
                    .rust_module(file, &format!("self::{joined}::{member}")),
            );
            tries.extend(
                self.files
                    .rust_module(file, &format!("crate::{joined}::{member}")),
            );
        }
        for (module_file, rest) in tries {
            let hit = candidates.iter().copied().find(|&c| {
                let n = &self.nodes[c];
                n.file == module_file
                    && match rest.as_slice() {
                        [m] => m == member && !n.qn.contains("::"),
                        [ty, m] => m == member && names_member(&n.qn, ty, member),
                        _ => false,
                    }
            });
            if hit.is_some() {
                return hit;
            }
        }
        None
    }

    /// One of several equally good candidates, or `None` when they are not
    /// one definition: a declaration and its definition (a C header and its
    /// source) count as one, and the earliest wins.
    fn pick(&self, hits: &[usize]) -> Option<usize> {
        match hits {
            [] => None,
            [only] => Some(*only),
            many => {
                let first = &self.nodes[many[0]];
                let same_symbol = many.iter().all(|&h| self.nodes[h].qn == first.qn);
                same_symbol.then(|| {
                    *many
                        .iter()
                        .min_by_key(|&&h| {
                            (
                                self.nodes[h].end == self.nodes[h].start,
                                &self.nodes[h].file,
                                self.nodes[h].start,
                            )
                        })
                        .unwrap_or(&many[0])
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_are_the_last_name_written() {
        assert_eq!(tail("self.store().put"), "put");
        assert_eq!(tail("crate::cli::args::ToolArg"), "ToolArg");
        assert_eq!(tail("sidekiq/job_util.rb"), "job_util");
        assert_eq!(tail("./sqlite-adapter"), "sqlite-adapter");
        assert_eq!(tail("._internal_utils"), "_internal_utils");
        assert_eq!(tail("c->funcs->read"), "read");
        assert_eq!(tail("foo"), "foo");
    }

    #[test]
    fn members_match_whole_owner_names() {
        assert!(names_member("Context::new", "Context", "new"));
        assert!(names_member("tera::Context::new", "Context", "new"));
        assert!(names_member("pkg.Context.new", "Context", "new"));
        assert!(!names_member(
            "WorkspaceDiscoveryContext::new",
            "Context",
            "new"
        ));
    }

    #[test]
    fn segments_split_every_separator() {
        assert_eq!(segments("self.store().put"), ["self", "store", "put"]);
        assert_eq!(segments("Foo::new().bar"), ["Foo", "new", "bar"]);
        assert_eq!(segments("::memset"), ["memset"]);
        assert_eq!(segments("@config.redis_pool"), ["config", "redis_pool"]);
        assert_eq!(segments("c->funcs->read"), ["c", "funcs", "read"]);
    }
}
