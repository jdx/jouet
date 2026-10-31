//! The SQLite store: codegraph's schema (`src/db/schema.sql`), trimmed to
//! what jouet reads, with FTS5 over symbol names, docstrings and signatures.
//!
//! One file's rows are replaced together: deleting a file deletes its nodes,
//! and its edges and references follow through `ON DELETE CASCADE`.

use crate::Error;
use crate::extract::Extraction;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;
use std::time::Duration;

/// Bumped whenever the schema or extraction changes what is stored: a
/// database written by another version is rebuilt from scratch.
pub(crate) const SCHEMA_VERSION: &str = "3";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    language TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_ns INTEGER NOT NULL,
    node_count INTEGER NOT NULL DEFAULT 0,
    has_errors INTEGER NOT NULL DEFAULT 0,
    error TEXT
);

-- `nid` keys the edges and references: an integer, where codegraph uses
-- the text `id`, which made those tables (and their indexes) several
-- times larger. Both change when a file is re-indexed.
CREATE TABLE IF NOT EXISTS nodes (
    nid INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    -- The name's words (`processPayment` → `process payment`), so full-text
    -- search finds a symbol by any word of it.
    words TEXT NOT NULL DEFAULT '',
    file_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
    language TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    docstring TEXT,
    signature TEXT,
    visibility TEXT,
    is_exported INTEGER NOT NULL DEFAULT 0,
    is_async INTEGER NOT NULL DEFAULT 0,
    is_static INTEGER NOT NULL DEFAULT 0,
    is_abstract INTEGER NOT NULL DEFAULT 0,
    decorators TEXT,
    type_parameters TEXT,
    return_type TEXT
);

CREATE TABLE IF NOT EXISTS edges (
    source INTEGER NOT NULL REFERENCES nodes(nid) ON DELETE CASCADE,
    target INTEGER NOT NULL REFERENCES nodes(nid) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    metadata TEXT,
    line INTEGER,
    col INTEGER,
    provenance TEXT,
    -- The reference linking resolved into this edge; NULL for an edge
    -- extraction found within one file.
    ref_id INTEGER REFERENCES refs(id) ON DELETE CASCADE
);

-- Every reference extraction found, kept after linking resolves it, so a
-- re-indexed file's incoming links can be re-resolved without re-reading
-- the files they come from. Its file and language are its node's.
CREATE TABLE IF NOT EXISTS refs (
    id INTEGER PRIMARY KEY,
    from_node INTEGER NOT NULL REFERENCES nodes(nid) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    line INTEGER NOT NULL,
    col INTEGER NOT NULL,
    candidates TEXT,
    -- 0 pending, 1 linked, 2 failed (see link.rs).
    status INTEGER NOT NULL DEFAULT 0,
    -- The name's last segment, written when linking fails, so a re-indexed
    -- file that defines it can retry the reference.
    tail TEXT
);

CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_qualified_name ON nodes(qualified_name);
CREATE INDEX IF NOT EXISTS idx_nodes_file_line ON nodes(file_path, start_line);
CREATE INDEX IF NOT EXISTS idx_edges_source_kind ON edges(source, kind);
CREATE INDEX IF NOT EXISTS idx_edges_target_kind ON edges(target, kind);
CREATE UNIQUE INDEX IF NOT EXISTS idx_edges_identity
  ON edges(source, target, kind, IFNULL(line, -1), IFNULL(col, -1));
CREATE INDEX IF NOT EXISTS idx_refs_from_node ON refs(from_node);
CREATE INDEX IF NOT EXISTS idx_refs_pending ON refs(status) WHERE status = 0;
CREATE INDEX IF NOT EXISTS idx_refs_failed_tail ON refs(tail) WHERE status = 2;
CREATE INDEX IF NOT EXISTS idx_edges_ref ON edges(ref_id) WHERE ref_id IS NOT NULL;

-- A linked edge whose target went away (its file was re-indexed or
-- deleted) puts its reference back to be linked again.
CREATE TRIGGER IF NOT EXISTS edges_ref_ad AFTER DELETE ON edges
WHEN OLD.ref_id IS NOT NULL BEGIN
    UPDATE refs SET status = 0 WHERE id = OLD.ref_id;
END;

CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
    id,
    name,
    qualified_name,
    words,
    docstring,
    signature,
    content='nodes',
    content_rowid='nid'
);

CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, id, name, qualified_name, words, docstring, signature)
    VALUES (NEW.nid, NEW.id, NEW.name, NEW.qualified_name, NEW.words, NEW.docstring, NEW.signature);
END;

CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, id, name, qualified_name, words, docstring, signature)
    VALUES ('delete', OLD.nid, OLD.id, OLD.name, OLD.qualified_name, OLD.words, OLD.docstring, OLD.signature);
END;
"#;

/// The indexes and triggers [`begin_bulk`] drops and [`end_bulk`] puts
/// back: everything but the tables and the indexes their keys need.
const BULK_DROPPED: &str = "
DROP TRIGGER IF EXISTS nodes_ai;
DROP TRIGGER IF EXISTS nodes_ad;
DROP INDEX IF EXISTS idx_nodes_name;
DROP INDEX IF EXISTS idx_nodes_qualified_name;
DROP INDEX IF EXISTS idx_edges_source_kind;
DROP INDEX IF EXISTS idx_edges_target_kind;
DROP INDEX IF EXISTS idx_edges_identity;
";

/// Before filling an empty database: inserting with every index and the
/// full-text triggers in place took three times as long as building them
/// once afterwards.
pub(crate) fn begin_bulk(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(BULK_DROPPED)?;
    Ok(())
}

/// After [`begin_bulk`]: recreates what it dropped and fills the full-text
/// index in one pass.
pub(crate) fn end_bulk(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(SCHEMA)?;
    conn.execute("INSERT INTO nodes_fts(nodes_fts) VALUES ('rebuild')", [])?;
    // Fold the write-ahead log, as large as the database by now, back into
    // it. Best effort: a reader holding it open only delays this.
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    Ok(())
}

/// Opens (creating if needed) the database at `path`, rebuilding it when it
/// was written by another schema version or for another work tree.
pub(crate) fn open(path: &Path, root: &str) -> Result<Connection, Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match open_or_reset(path, root) {
        Err(Error(e)) if is_corrupt(&e) => {
            remove_db(path)?;
            open_or_reset(path, root)
        }
        other => other,
    }
}

/// SQLite's words for a file that is not a sound database.
fn is_corrupt(message: &str) -> bool {
    message.contains("malformed") || message.contains("not a database")
}

fn open_or_reset(path: &Path, root: &str) -> Result<Connection, Error> {
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    let stale = {
        let has_meta: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
            [],
            |r| r.get(0),
        )?;
        !has_meta
            || get_meta(&conn, "schema_version")?.as_deref() != Some(SCHEMA_VERSION)
            || get_meta(&conn, "root")?.as_deref() != Some(root)
    };
    if stale {
        drop(conn);
        remove_db(path)?;
        conn = Connection::open(path)?;
        configure(&conn)?;
        conn.execute_batch(SCHEMA)?;
        set_meta(&conn, "schema_version", SCHEMA_VERSION)?;
        set_meta(&conn, "root", root)?;
    }
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<(), Error> {
    // Parallel sessions run hooks against one database: WAL lets readers
    // go on while one writer writes, and the busy timeout queues writers.
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // The graph is a cache: rebuilt from the work tree whenever it is lost
    // or found corrupt, so it skips the sync a short-lived process would
    // otherwise pay on every first commit (56 ms on an overlay filesystem,
    // more than the rest of a one-file re-index). An application crash
    // still cannot corrupt it in WAL mode; only an OS crash could.
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // A larger page cache than SQLite's 2 MiB default keeps the indexes'
    // pages in memory while a large index writes them.
    conn.pragma_update(None, "cache_size", -64 * 1024)?;
    Ok(())
}

/// Removes a database and its WAL files.
pub(crate) fn remove_db(path: &Path) -> Result<(), Error> {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>, Error> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub(crate) fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<(), Error> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// What the store remembers about one file.
pub(crate) struct FileRow {
    pub(crate) content_hash: String,
    pub(crate) size: u64,
    pub(crate) modified_ns: i64,
}

pub(crate) fn file_rows(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, FileRow>, Error> {
    let mut stmt = conn.prepare("SELECT path, content_hash, size, modified_ns FROM files")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            FileRow {
                content_hash: r.get(1)?,
                size: r.get::<_, i64>(2)? as u64,
                modified_ns: r.get(3)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn file_row(conn: &Connection, path: &str) -> Result<Option<FileRow>, Error> {
    Ok(conn
        .query_row(
            "SELECT content_hash, size, modified_ns FROM files WHERE path = ?1",
            [path],
            |r| {
                Ok(FileRow {
                    content_hash: r.get(0)?,
                    size: r.get::<_, i64>(1)? as u64,
                    modified_ns: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// A file's new state, ready to replace its rows.
pub(crate) struct FileUpdate {
    pub(crate) path: String,
    pub(crate) language: &'static str,
    pub(crate) content_hash: String,
    pub(crate) size: u64,
    pub(crate) modified_ns: i64,
    /// `Err` when extraction failed: the file is still recorded, with no
    /// symbols, so it is not retried until it changes.
    pub(crate) extraction: Result<Extraction, String>,
}

pub(crate) fn delete_file(tx: &Transaction, path: &str) -> Result<(), Error> {
    tx.execute("DELETE FROM files WHERE path = ?1", [path])?;
    Ok(())
}

/// Only the file's timestamp changed, not its content.
pub(crate) fn touch_file(tx: &Transaction, path: &str, modified_ns: i64) -> Result<(), Error> {
    tx.execute(
        "UPDATE files SET modified_ns = ?2 WHERE path = ?1",
        params![path, modified_ns],
    )?;
    Ok(())
}

fn json_list(items: &[String]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    serde_json::to_string(items).ok()
}

/// Replaces one file's nodes, edges and references.
pub(crate) fn write_file(tx: &Transaction, update: &FileUpdate) -> Result<(), Error> {
    delete_file(tx, &update.path)?;
    let (node_count, has_errors, error) = match &update.extraction {
        Ok(x) => (x.nodes.len(), x.has_errors, None),
        Err(e) => (0, false, Some(e.as_str())),
    };
    tx.execute(
        "INSERT INTO files(path, content_hash, language, size, modified_ns, node_count, has_errors, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            update.path,
            update.content_hash,
            update.language,
            update.size as i64,
            update.modified_ns,
            node_count as i64,
            has_errors,
            error,
        ],
    )?;
    let Ok(x) = &update.extraction else {
        return Ok(());
    };
    let mut node = tx.prepare_cached(
        "INSERT INTO nodes(id, kind, name, qualified_name, file_path, language,
             start_line, end_line, start_column, end_column, docstring, signature, visibility,
             is_exported, is_async, is_static, is_abstract, decorators, type_parameters, return_type, words)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
         ON CONFLICT(id) DO NOTHING",
    )?;
    let mut existing = tx.prepare_cached("SELECT nid FROM nodes WHERE id = ?1")?;
    // Each row's `nid`; a row whose ID another row of the file already took
    // (codegraph's allocator makes that rare) shares that row's node.
    let mut nids = Vec::with_capacity(x.nodes.len());
    for n in &x.nodes {
        let inserted = node.execute(params![
            n.id,
            n.kind,
            n.name,
            n.qualified_name,
            update.path,
            update.language,
            n.start_line,
            n.end_line,
            n.start_column,
            n.end_column,
            n.docstring,
            n.signature,
            n.visibility,
            n.is_exported,
            n.is_async,
            n.is_static,
            n.is_abstract,
            json_list(&n.decorators),
            json_list(&n.type_parameters),
            n.return_type,
            crate::query::words(&n.name),
        ])?;
        nids.push(if inserted == 1 {
            tx.last_insert_rowid()
        } else {
            existing.query_row([&n.id], |r| r.get(0))?
        });
    }
    let mut edge = tx.prepare_cached(
        "INSERT OR IGNORE INTO edges(source, target, kind, metadata, line, col, provenance)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for e in &x.edges {
        edge.execute(params![
            nids[e.source],
            nids[e.target],
            e.kind,
            e.metadata,
            e.line,
            e.column,
            e.provenance,
        ])?;
    }
    let mut reference = tx.prepare_cached(
        "INSERT INTO refs(from_node, name, kind, line, col, candidates)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for r in &x.refs {
        reference.execute(params![
            nids[r.from],
            r.name,
            r.kind,
            r.line,
            r.column,
            json_list(&r.candidates),
        ])?;
    }
    Ok(())
}
