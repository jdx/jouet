//! jouet's code graph: symbols, the calls, imports and inheritance between
//! them, and queries over them, kept in a SQLite cache outside the repo.
//!
//! Derived from [codegraph](https://github.com/colbymchenry/codegraph) by
//! Colby Mchenry (MIT; see the `NOTICE` file):
//! - [`extract`] is a port of codegraph-kernel, codegraph's Rust tree-sitter
//!   walkers, with napi removed and trimmed to eleven languages;
//! - the storage schema follows codegraph's `src/db/schema.sql`.
//!
//! jouet owns the code from here on: there is no parity goal, and no MCP
//! server, file watcher or daemon. Instead every query first runs a cheap
//! freshness check ([`Graph::refresh`]), and jouet's post-tool-use hook
//! re-indexes the files an agent edits ([`Graph::reindex_paths`]).
//!
//! The database lives in `$XDG_CACHE_HOME/jouet/graph/<repo key>.db`, one
//! per work tree, keyed like jouet's ledger. Nothing is written into the
//! repository.

pub mod extract;
mod index;
mod store;

pub use index::Refresh;

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Any failure: SQLite, the filesystem, or a work tree that is not one.
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(format!("graph database: {e}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

/// The code graph of one work tree.
pub struct Graph {
    conn: Connection,
    root: PathBuf,
    db: PathBuf,
}

/// A symbol in the graph.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Symbol {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    /// Relative to the work tree, with `/` separators.
    pub path: String,
    pub language: String,
    /// 1-based, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docstring: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exported: bool,
}

/// The columns [`Symbol::from_row`] reads, for a `SELECT` over `nodes`.
pub(crate) const SYMBOL_COLUMNS: &str = "id, kind, name, qualified_name, file_path, language, start_line, end_line, signature, docstring, is_exported";

impl Symbol {
    pub(crate) fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Symbol> {
        Ok(Symbol {
            id: r.get(0)?,
            kind: r.get(1)?,
            name: r.get(2)?,
            qualified_name: r.get(3)?,
            path: r.get(4)?,
            language: r.get(5)?,
            start_line: r.get(6)?,
            end_line: r.get(7)?,
            signature: r.get(8)?,
            docstring: r.get(9)?,
            exported: r.get(10)?,
        })
    }
}

/// Counts for `jouet graph status`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub refs: u64,
    /// Files whose parse tree had errors (walked as tree-sitter recovered them).
    pub files_with_errors: u64,
    /// Files recorded without symbols: too large, not UTF-8, or unwalkable.
    pub files_without_symbols: u64,
    /// The database and its WAL, in bytes.
    pub db_bytes: u64,
}

/// Where the graph of the work tree with this [`repo_key`] lives:
/// `$XDG_CACHE_HOME/jouet/graph/<key>.db`.
///
/// [`repo_key`]: jouet_output::paths::repo_key
pub fn db_path_for(repo_key: &str) -> Option<PathBuf> {
    jouet_output::paths::cache_dir().map(|d| d.join("graph").join(format!("{repo_key}.db")))
}

impl Graph {
    /// Like [`Graph::open`], but `None` when the work tree has no graph yet,
    /// so a hook never builds one nobody asked for.
    pub fn open_existing(cwd: &Path) -> Result<Option<Graph>, Error> {
        let Some(root) = jouet_output::paths::work_tree(cwd) else {
            return Ok(None);
        };
        let Some(db) = jouet_output::paths::repo_key(&root).and_then(|k| db_path_for(&k)) else {
            return Ok(None);
        };
        if !db.exists() {
            return Ok(None);
        }
        Graph::open_at(&root, &db).map(Some)
    }

    /// Opens the graph of the work tree containing `cwd`, in jouet's cache
    /// ([`db_path_for`]). Does not index.
    pub fn open(cwd: &Path) -> Result<Graph, Error> {
        let not_a_work_tree = || Error(format!("{} is not inside a git work tree", cwd.display()));
        let root = jouet_output::paths::work_tree(cwd).ok_or_else(not_a_work_tree)?;
        let key = jouet_output::paths::repo_key(&root).ok_or_else(not_a_work_tree)?;
        let db = db_path_for(&key)
            .ok_or_else(|| Error("no cache directory: set HOME or XDG_CACHE_HOME".into()))?;
        Graph::open_at(&root, &db)
    }

    /// Opens the graph of the work tree at `root`, at `db`, creating an
    /// empty one when there is none. Does not index.
    pub fn open_at(root: &Path, db: &Path) -> Result<Graph, Error> {
        let root = root.canonicalize()?;
        let conn = store::open(db, &root.to_string_lossy())?;
        Ok(Graph {
            conn,
            root,
            db: db.to_path_buf(),
        })
    }

    /// The work tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the database lives.
    pub fn db_path(&self) -> &Path {
        &self.db
    }

    fn reopen_empty(&mut self) -> Result<(), Error> {
        let placeholder = Connection::open_in_memory()?;
        drop(std::mem::replace(&mut self.conn, placeholder));
        store::remove_db(&self.db)?;
        self.conn = store::open(&self.db, &self.root.to_string_lossy())?;
        Ok(())
    }

    /// A file's symbols in source order (by start line), without the file
    /// node itself and without imports. `path` is relative to the work tree
    /// or absolute.
    pub fn outline(&self, path: &Path) -> Result<Vec<Symbol>, Error> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let Some(rel) = index::relative(&self.root, &abs) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SYMBOL_COLUMNS} FROM nodes
             WHERE file_path = ?1 AND kind NOT IN ('file', 'import')
             ORDER BY start_line, start_column, end_line DESC"
        ))?;
        let rows = stmt.query_map([rel], Symbol::from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn status(&self) -> Result<Status, Error> {
        let count = |sql: &str| -> Result<u64, Error> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64)
        };
        let db_bytes = ["", "-wal"]
            .iter()
            .filter_map(|suffix| {
                let mut p = self.db.as_os_str().to_owned();
                p.push(suffix);
                std::fs::metadata(p).ok().map(|m| m.len())
            })
            .sum();
        Ok(Status {
            files: count("SELECT COUNT(*) FROM files")?,
            nodes: count("SELECT COUNT(*) FROM nodes")?,
            edges: count("SELECT COUNT(*) FROM edges")?,
            refs: count("SELECT COUNT(*) FROM refs")?,
            files_with_errors: count("SELECT COUNT(*) FROM files WHERE has_errors")?,
            files_without_symbols: count("SELECT COUNT(*) FROM files WHERE error IS NOT NULL")?,
            db_bytes,
        })
    }
}
