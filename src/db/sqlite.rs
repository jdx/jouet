//! SQLite, opened read-only (`SQLITE_OPEN_READ_ONLY` and `query_only`), so
//! no statement can write whatever its text; a statement SQLite says would
//! write is refused before it runs.

use super::{COUNT_CAP, Column, Error, ForeignKey, Index, Rows, Table, Value, place_keys};
use jouet_output::exit::Exit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

pub(crate) fn open(path: &Path) -> Result<Connection, Error> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| {
        Error::new(
            Exit::Precondition,
            format!("opening {} read-only: {e}", path.display()),
        )
    })?;
    conn.busy_timeout(Duration::from_secs(5))
        .and_then(|()| conn.execute_batch("PRAGMA query_only = ON"))
        .and_then(|()| conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(())))
        .map_err(|e| {
            Error::new(
                Exit::Precondition,
                format!(
                    "{} is not a SQLite database jouet can read: {e}",
                    path.display()
                ),
            )
        })?;
    Ok(conn)
}

/// Interrupts whatever `conn` runs once `limit` passes, until dropped.
struct Deadline {
    done: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<bool>>,
}

impl Deadline {
    fn new(conn: &Connection, limit: Duration) -> Self {
        let handle = conn.get_interrupt_handle();
        let (done, wait) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let fired = matches!(
                wait.recv_timeout(limit),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
            if fired {
                handle.interrupt();
            }
            fired
        });
        Self {
            done: Some(done),
            thread: Some(thread),
        }
    }

    /// Stops the timer; whether it had fired.
    fn finish(mut self) -> bool {
        self.stop()
    }

    fn stop(&mut self) -> bool {
        drop(self.done.take());
        self.thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or(false)
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        self.stop();
    }
}

fn failed(e: &rusqlite::Error, timed_out: bool, timeout: Duration) -> Error {
    if timed_out {
        return Error::new(
            Exit::Timeout,
            format!("the query ran past {}s and was stopped", timeout.as_secs()),
        )
        .hint("narrow it with WHERE or LIMIT, or raise --timeout");
    }
    let message = e.to_string();
    if message.contains("readonly") || message.contains("read-only") {
        return Error::new(
            Exit::Precondition,
            format!("refused: SQLite says this statement writes ({message})"),
        );
    }
    Error::new(Exit::Failed, message)
}

/// Runs one read-only statement: up to `limit` rows, then counts the rest
/// up to [`COUNT_CAP`].
pub(crate) fn query(
    conn: &Connection,
    sql: &str,
    limit: usize,
    timeout: Duration,
) -> Result<Rows, Error> {
    let mut stmt = conn.prepare(sql).map_err(|e| match e {
        rusqlite::Error::MultipleStatement => Error::new(
            Exit::Precondition,
            "refusing several statements: jouet db query runs one at a time",
        ),
        e => failed(&e, false, timeout),
    })?;
    if !stmt.readonly() {
        return Err(Error::new(
            Exit::Precondition,
            "refused: SQLite says this statement writes; jouet db query only reads",
        ));
    }
    let columns: Vec<String> = stmt.column_names().map(str::to_string).collect();
    let deadline = Deadline::new(conn, timeout);
    let mut rows = Vec::new();
    let mut more = 0u64;
    let mut cursor = stmt.query([]).map_err(|e| failed(&e, false, timeout))?;
    let step = loop {
        match cursor.next() {
            Ok(Some(row)) if rows.len() < limit => {
                let values = (0..columns.len())
                    .map(|i| row.get_ref(i).map(value))
                    .collect::<Result<Vec<_>, _>>();
                match values {
                    Ok(values) => rows.push(values),
                    Err(e) => break Err(e),
                }
            }
            Ok(Some(_)) => {
                more += 1;
                if more >= COUNT_CAP {
                    break Ok(true);
                }
            }
            Ok(None) => break Ok(false),
            Err(e) => break Err(e),
        }
    };
    drop(cursor);
    let timed_out = deadline.finish();
    let more_capped = step.map_err(|e| failed(&e, timed_out, timeout))?;
    Ok(Rows {
        columns,
        rows,
        more,
        more_capped,
    })
}

fn value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::Blob(b.to_vec()),
    }
}

/// The tables and views, or the one named `only`, with their columns,
/// keys and indexes; for one table, its row count when counting is cheap.
pub(crate) fn schema(conn: &Connection, only: Option<&str>) -> Result<Vec<Table>, Error> {
    let err = |e: rusqlite::Error| Error::new(Exit::Failed, format!("reading the schema: {e}"));
    let mut stmt = conn
        .prepare(
            "SELECT name, type FROM sqlite_schema \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             AND (?1 IS NULL OR name = ?1 COLLATE NOCASE) ORDER BY name",
        )
        .map_err(err)?;
    let names: Vec<(String, String)> = stmt
        .query_map([only], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    let mut tables = Vec::new();
    for (name, kind) in names {
        let mut table = describe(conn, &name, kind).map_err(err)?;
        if only.is_some() {
            if table.kind == "table" {
                table.rows = count(conn, &name);
            }
            table.referenced_by = referenced_by(conn, &name).map_err(err)?;
        }
        place_keys(&mut table);
        tables.push(table);
    }
    Ok(tables)
}

fn describe(conn: &Connection, name: &str, kind: String) -> rusqlite::Result<Table> {
    let mut table = Table {
        name: name.to_string(),
        kind,
        ..Table::default()
    };
    let mut pk: Vec<(i64, String)> = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo(?1)",
    )?;
    let mut rows = stmt.query([name])?;
    while let Some(r) = rows.next()? {
        // Hidden columns of a virtual table are not the table's.
        if r.get::<_, i64>(5)? == 1 {
            continue;
        }
        let col: String = r.get(0)?;
        let key: i64 = r.get(4)?;
        if key > 0 {
            pk.push((key, col.clone()));
        }
        table.columns.push(Column {
            name: col,
            ty: r.get::<_, String>(1)?.to_lowercase(),
            not_null: r.get::<_, i64>(2)? == 1 && key == 0,
            default: r.get(3)?,
            ..Column::default()
        });
    }
    pk.sort();
    table.primary_key = pk.into_iter().map(|(_, c)| c).collect();

    let mut stmt = conn.prepare(
        "SELECT id, \"table\", \"from\", \"to\" FROM pragma_foreign_key_list(?1) ORDER BY id, seq",
    )?;
    let mut rows = stmt.query([name])?;
    /// One foreign key's rows, gathered.
    struct Key {
        id: i64,
        table: String,
        from: Vec<String>,
        to: Vec<String>,
    }
    let mut fks: Vec<Key> = Vec::new();
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let (from, to): (String, Option<String>) = (r.get(2)?, r.get(3)?);
        if fks.last().is_none_or(|fk| fk.id != id) {
            fks.push(Key {
                id,
                table: r.get(1)?,
                from: Vec::new(),
                to: Vec::new(),
            });
        }
        if let Some(fk) = fks.last_mut() {
            fk.from.push(from);
            fk.to.extend(to);
        }
    }
    for fk in fks {
        // A foreign key with no columns named references the primary key.
        let references = if fk.to.is_empty() {
            fk.table
        } else {
            format!("{}({})", fk.table, fk.to.join(", "))
        };
        table.foreign_keys.push(ForeignKey {
            columns: fk.from,
            references,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT l.name, l.\"unique\", l.partial, s.sql FROM pragma_index_list(?1) l \
         LEFT JOIN sqlite_schema s ON s.type = 'index' AND s.name = l.name \
         WHERE l.origin != 'pk' ORDER BY l.name",
    )?;
    let indexes: Vec<(String, bool, bool, Option<String>)> = stmt
        .query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    for (index, unique, partial, sql) in indexes {
        let mut cols = conn.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")?;
        let cols: Vec<String> = cols
            .query_map([&index], |r| r.get::<_, Option<String>>(0))?
            .map(|c| c.map(|c| c.unwrap_or_else(|| "<expr>".into())))
            .collect::<Result<_, _>>()?;
        let mut on = format!("({})", cols.join(", "));
        // An expression or a partial index is clearer as its own SQL.
        if let Some(sql) = sql.filter(|_| partial || cols.iter().any(|c| c == "<expr>"))
            && let Some(at) = sql.find('(')
        {
            on = sql[at..].split_whitespace().collect::<Vec<_>>().join(" ");
        }
        table.indexes.push(Index {
            name: index,
            on,
            unique,
        });
    }
    Ok(table)
}

/// The tables whose foreign keys point at `name`, as `table(column)`.
fn referenced_by(conn: &Connection, name: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT m.name, group_concat(f.\"from\", ', ') FROM sqlite_schema m, \
         pragma_foreign_key_list(m.name) f WHERE m.type = 'table' AND f.\"table\" = ?1 COLLATE NOCASE \
         GROUP BY m.name, f.id ORDER BY m.name, f.id",
    )?;
    stmt.query_map([name], |r| {
        Ok(format!(
            "{}({})",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })?
    .collect()
}

/// `count(*)`, given up after half a second: a large table's count is a
/// full scan.
fn count(conn: &Connection, name: &str) -> Option<u64> {
    let deadline = Deadline::new(conn, Duration::from_millis(500));
    let n = conn.query_row(
        &format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")),
        [],
        |r| r.get::<_, i64>(0),
    );
    let _ = deadline.finish();
    n.ok().and_then(|n| u64::try_from(n).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE TABLE users (
               id INTEGER PRIMARY KEY,
               email TEXT NOT NULL,
               team_id INTEGER REFERENCES teams(id),
               created_at TEXT DEFAULT CURRENT_TIMESTAMP
             );
             CREATE INDEX users_team ON users(team_id) WHERE team_id IS NOT NULL;
             CREATE TABLE memberships (user_id INTEGER, team_id INTEGER, PRIMARY KEY (user_id, team_id));
             CREATE VIEW named AS SELECT id, email FROM users;
             INSERT INTO teams VALUES (1, 'core'), (2, 'docs');
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 120)
               INSERT INTO users (email, team_id) SELECT 'u' || i || '@example.com', 1 + i % 2 FROM n;",
        )
        .unwrap();
        (dir, path)
    }

    #[test]
    fn a_query_reads_up_to_the_limit_and_counts_the_rest() {
        let (_dir, path) = fixture();
        let conn = open(&path).unwrap();
        let rows = query(
            &conn,
            "select id, email, team_id from users order by id",
            50,
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(rows.columns, ["id", "email", "team_id"]);
        assert_eq!(rows.rows.len(), 50);
        assert_eq!(rows.more, 70);
        assert!(!rows.more_capped);
        assert_eq!(rows.rows[0][1], Value::Text("u1@example.com".into()));
    }

    #[test]
    fn writes_are_refused_by_sqlite() {
        let (_dir, path) = fixture();
        let conn = open(&path).unwrap();
        // The first word is a read; SQLite still says it writes.
        let err = query(
            &conn,
            "with x as (select 1) delete from users",
            50,
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        assert_eq!(err.exit, Exit::Precondition, "{}", err.message);
        let err = query(
            &conn,
            "select 1; delete from users",
            50,
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        assert_eq!(err.exit, Exit::Precondition, "{}", err.message);
        let err = query(&conn, "pragma user_version = 3", 50, Duration::from_secs(5))
            .err()
            .unwrap();
        assert_eq!(err.exit, Exit::Precondition, "{}", err.message);
        let n: i64 = conn
            .query_row("select count(*) from users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 120);
    }

    #[test]
    fn a_slow_query_times_out() {
        let (_dir, path) = fixture();
        let conn = open(&path).unwrap();
        let err = query(
            &conn,
            "with recursive n(i) as (select 1 union all select i + 1 from n) select count(*) from n",
            50,
            Duration::from_millis(200),
        )
        .err()
        .unwrap();
        assert_eq!(err.exit, Exit::Timeout, "{}", err.message);
    }

    #[test]
    fn the_schema_has_keys_indexes_and_counts() {
        let (_dir, path) = fixture();
        let conn = open(&path).unwrap();
        let all = schema(&conn, None).unwrap();
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["memberships", "named", "teams", "users"]);
        assert_eq!(all[0].primary_key, ["user_id", "team_id"]);
        assert_eq!(all[1].kind, "view");
        assert!(all[3].rows.is_none());

        let users = &schema(&conn, Some("users")).unwrap()[0];
        assert_eq!(users.rows, Some(120));
        assert!(users.columns[0].primary_key);
        assert!(users.columns[1].not_null);
        assert_eq!(users.columns[2].references.as_deref(), Some("teams(id)"));
        assert_eq!(
            users.columns[3].default.as_deref(),
            Some("CURRENT_TIMESTAMP")
        );
        assert_eq!(users.indexes[0].on, "(team_id) WHERE team_id IS NOT NULL");

        let teams = &schema(&conn, Some("teams")).unwrap()[0];
        assert_eq!(teams.referenced_by, ["users(team_id)"]);
        assert!(teams.indexes[0].unique);
        assert!(schema(&conn, Some("nope")).unwrap().is_empty());
    }
}
