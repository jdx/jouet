//! `jouet db`: a database's schema, and read-only queries, for SQLite and
//! Postgres.
//!
//! Reads are enforced by the database, not only by looking at the SQL:
//! SQLite is opened read-only and asked whether the statement writes, and
//! Postgres runs the statement alone in a read-only transaction. The check
//! of the statement's first word comes first only to give a clear reason.

pub(crate) mod postgres;
pub(crate) mod sqlite;

use jouet_output::exit::Exit;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Why a command stopped: the exit, the message and a hint.
#[derive(Debug)]
pub(crate) struct Error {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

impl Error {
    pub(crate) fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Where the connection came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    /// `--url`.
    Flag,
    /// `DATABASE_URL` in the environment.
    Env,
    /// `fnox get DATABASE_URL`, in a project whose fnox config defines it.
    Fnox,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Flag => "--url",
            Source::Env => "DATABASE_URL",
            Source::Fnox => "fnox",
        }
    }
}

/// The database to open.
pub(crate) enum Target {
    Sqlite(PathBuf),
    /// A Postgres URL or `key=value` connection string. It holds the
    /// password, so it is never printed: [`Connection::display`] is.
    Postgres(String),
}

/// A database found from the flags, the environment or fnox.
pub(crate) struct Connection {
    pub target: Target,
    pub source: Source,
    /// The URL or path to show: a password is `***`.
    pub display: String,
}

impl Connection {
    pub(crate) fn kind(&self) -> &'static str {
        match self.target {
            Target::Sqlite(_) => "sqlite",
            Target::Postgres(_) => "postgres",
        }
    }

    /// `postgres://app:***@localhost/app (DATABASE_URL)`.
    pub(crate) fn describe(&self) -> String {
        format!("{} ({})", self.display, self.source.label())
    }
}

/// The options every `jouet db` command takes.
pub(crate) struct Options<'a> {
    pub url: Option<&'a str>,
    pub remote: bool,
}

/// Finds the database: `--url`, then `DATABASE_URL`, then, in a project
/// whose fnox config defines `DATABASE_URL`, `fnox get DATABASE_URL`. A
/// plain `.env` is never read. Refuses a Postgres host that is not on this
/// machine without `--remote`.
pub(crate) fn connect(cwd: &Path, opts: &Options) -> Result<Connection, Error> {
    let (url, source) = match opts.url.filter(|u| !u.is_empty()) {
        Some(url) => (url.to_string(), Source::Flag),
        None => match std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()) {
            Some(url) => (url, Source::Env),
            None => match from_fnox(cwd)? {
                Some(url) => (url, Source::Fnox),
                None => {
                    return Err(Error::new(
                        Exit::Precondition,
                        "no database: pass --url, or set DATABASE_URL",
                    )
                    .hint(
                        "a SQLite file is --url path/to/app.db; a .env file is not read, but DATABASE_URL in the project's fnox config is",
                    ));
                }
            },
        },
    };
    let target = parse(cwd, &url)?;
    let display = match &target {
        Target::Sqlite(path) => path.strip_prefix(cwd).unwrap_or(path).display().to_string(),
        Target::Postgres(url) => redact(url),
    };
    let conn = Connection {
        target,
        source,
        display,
    };
    if let Target::Postgres(url) = &conn.target
        && !opts.remote
        && let Some(host) = remote_host(url)
    {
        return Err(Error::new(
            Exit::Precondition,
            format!("refusing {host} (not local); pass --remote if you mean it"),
        )
        .hint(format!(
            "the database is {}; --remote reads a database on another machine, which may be production",
            conn.describe()
        )));
    }
    Ok(conn)
}

/// `DATABASE_URL` from fnox, when the project's fnox config defines it. The
/// value is read from fnox's stdout and never printed or passed on to
/// another command line.
fn from_fnox(cwd: &Path) -> Result<Option<String>, Error> {
    let Some(project) = crate::fnox::Project::find(cwd) else {
        return Ok(None);
    };
    if !project.secrets.contains("DATABASE_URL") {
        return Ok(None);
    }
    if !crate::fnox::installed() {
        return Err(Error::new(
            Exit::Precondition,
            "this project keeps DATABASE_URL in fnox, but fnox is not installed",
        )
        .hint("install fnox (https://fnox.jdx.dev), or pass --url"));
    }
    let mut command = Command::new("fnox");
    command
        .args(["get", "DATABASE_URL"])
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !jouet_output::display::person_at_terminal() {
        command
            .env("FNOX_NON_INTERACTIVE", "1")
            .stdin(Stdio::null());
    }
    let out = command.output().map_err(|e| {
        Error::new(
            Exit::Precondition,
            format!("running fnox get DATABASE_URL: {e}"),
        )
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "fnox could not give DATABASE_URL: {}",
                crate::fnox::failure(&stderr, out.status.code())
            ),
        )
        .hint("pass --url, or fix the fnox config"));
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(Some(url).filter(|u| !u.is_empty()))
}

/// What a URL names: a SQLite file (`sqlite:`, `file:`, or a path) or a
/// Postgres database (`postgres://`, `postgresql://`, or `key=value`).
fn parse(cwd: &Path, url: &str) -> Result<Target, Error> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        return Ok(Target::Postgres(url.to_string()));
    }
    if let Some(rest) = ["sqlite://", "sqlite:", "file://", "file:"]
        .iter()
        .find_map(|p| lower.starts_with(p).then(|| &url[p.len()..]))
    {
        // `sqlite:///abs/path` is an absolute path; `sqlite://rel` is not.
        let path = rest.split(['?', '#']).next().unwrap_or(rest);
        return sqlite_path(cwd, path);
    }
    if let Some((scheme, _)) = url.split_once("://") {
        return Err(Error::new(
            Exit::Usage,
            format!("`{scheme}://` is not a database jouet reads: SQLite or Postgres"),
        ));
    }
    if url.contains('=') && !Path::new(url).exists() {
        return Ok(Target::Postgres(url.to_string()));
    }
    sqlite_path(cwd, url)
}

fn sqlite_path(cwd: &Path, path: &str) -> Result<Target, Error> {
    if path.is_empty() || path == ":memory:" {
        return Err(Error::new(Exit::Usage, "a SQLite database needs a file"));
    }
    let path = cwd.join(path);
    if !path.is_file() {
        return Err(Error::new(
            Exit::NotFound,
            format!("no SQLite database at {}", path.display()),
        ));
    }
    Ok(Target::Sqlite(path))
}

/// The URL with its password replaced by `***`: in `user:pass@`, and in a
/// `password=` parameter of a URL or a `key=value` string.
pub(crate) fn redact(url: &str) -> String {
    let mut out = url.to_string();
    if let Some(scheme_end) = out.find("://") {
        let rest_start = scheme_end + 3;
        let authority_end = out[rest_start..]
            .find(['/', '?', '#'])
            .map_or(out.len(), |i| rest_start + i);
        if let Some(at) = out[rest_start..authority_end].rfind('@') {
            let userinfo = &out[rest_start..rest_start + at];
            if let Some(colon) = userinfo.find(':') {
                out.replace_range(rest_start + colon + 1..rest_start + at, "***");
            }
        }
    }
    redact_param(&out)
}

/// `password=...` in a query string or a `key=value` string.
fn redact_param(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find("password=") {
        let start = at + found;
        let boundary = start == 0
            || matches!(
                text.as_bytes()[start - 1],
                b'?' | b'&' | b' ' | b'\t' | b'\n'
            );
        let value_start = start + "password=".len();
        out.push_str(&text[at..value_start]);
        if !boundary {
            at = value_start;
            continue;
        }
        let bytes = text.as_bytes();
        let value_end = if bytes.get(value_start) == Some(&b'\'') {
            // A quoted value in a `key=value` string, with `\'` escapes.
            let mut i = value_start + 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            (i + 1).min(bytes.len())
        } else {
            text[value_start..]
                .find(['&', ' ', '\t', '\n', '#'])
                .map_or(text.len(), |i| value_start + i)
        };
        out.push_str("***");
        at = value_end;
    }
    out.push_str(&text[at..]);
    out
}

/// The first host in a Postgres connection string that is not this
/// machine, as `host:port`, or `None` when every host is local: localhost,
/// a loopback address or a Unix socket (also the default with no host).
fn remote_host(url: &str) -> Option<String> {
    let config: ::postgres::Config = url.parse().ok()?;
    let ports = config.get_ports();
    let port = |i: usize| {
        ports
            .get(i)
            .or_else(|| ports.first())
            .copied()
            .unwrap_or(5432)
    };
    let addrs = config.get_hostaddrs();
    for (i, host) in config.get_hosts().iter().enumerate() {
        // `hostaddr` is the address connected to; `host` only names it.
        let name = match (addrs.get(i), host) {
            (Some(addr), _) => addr.to_string(),
            (None, ::postgres::config::Host::Tcp(name)) => name.clone(),
            // A Unix socket is this machine; Windows has none.
            #[cfg(unix)]
            (None, ::postgres::config::Host::Unix(_)) => continue,
        };
        if !local(&name) {
            return Some(if name.contains(':') {
                format!("[{name}]:{}", port(i))
            } else {
                format!("{name}:{}", port(i))
            });
        }
    }
    if config.get_hosts().is_empty() {
        for addr in addrs {
            if !addr.is_loopback() {
                return Some(format!("{addr}:{}", port(0)));
            }
        }
    }
    None
}

fn local(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let lower = host.to_ascii_lowercase();
    lower == "localhost"
        || lower.ends_with(".localhost")
        || host.starts_with('/')
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The statements `jouet db query` runs: reads only, one at a time.
/// `pragma` is allowed for SQLite, which asks it whether it writes.
const READS: &[&str] = &["select", "with", "values", "table", "show", "explain"];

/// Refuses a statement that is not a read, by its first word, before the
/// database is asked. The database's own read-only mode is what enforces
/// it; this gives the reason.
pub(crate) fn check_read(sql: &str, sqlite: bool) -> Result<(), Error> {
    let body = skip_comments(sql);
    if body.is_empty() {
        return Err(Error::new(Exit::Usage, "the query is empty"));
    }
    let word: String = body
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    if READS.contains(&word.as_str()) || (sqlite && word == "pragma") {
        return Ok(());
    }
    let shown = if word.is_empty() {
        body.chars().take(20).collect()
    } else {
        word.to_ascii_uppercase()
    };
    Err(Error::new(
        Exit::Precondition,
        format!("refusing `{shown}`: jouet db query only reads"),
    )
    .hint("run SELECT, WITH, VALUES, TABLE, SHOW or EXPLAIN; changes to the database are not jouet's to make"))
}

/// The SQL after leading whitespace and comments.
fn skip_comments(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else {
            return rest;
        }
    }
}

/// How long a query or a schema read may take.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(30);
/// Rows past `--limit` are counted, without being read, up to this many.
pub(crate) const COUNT_CAP: u64 = 100_000;

/// A cell's value: SQL `NULL`, a number, a boolean, text, JSON or bytes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Text, and Postgres types read as text (numeric, dates, arrays).
    Text(String),
    Json(serde_json::Value),
    Blob(Vec<u8>),
}

/// A query's result, before it is cut to fit.
pub(crate) struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// Rows past the limit: counted, not read.
    pub more: u64,
    /// Whether counting stopped at [`COUNT_CAP`].
    pub more_capped: bool,
}

/// A table or view, as `jouet db schema` describes it.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Table {
    pub name: String,
    /// `table`, `view`, `materialized view`, `partitioned table` or
    /// `foreign table`.
    pub kind: String,
    pub columns: Vec<Column>,
    /// A primary key of more than one column; one column's is on the
    /// column.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub primary_key: Vec<String>,
    /// Foreign keys of more than one column; one column's is on the column.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<Index>,
    /// Foreign keys elsewhere that point here, for one table.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub referenced_by: Vec<String>,
    /// The exact row count, for one table, when counting was cheap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    /// Postgres's estimate, when counting was not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_estimate: Option<u64>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub not_null: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub primary_key: bool,
    /// `table(column)`, for a one-column foreign key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ForeignKey {
    pub columns: Vec<String>,
    /// `table(a, b)`.
    pub references: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Index {
    pub name: String,
    /// The indexed columns or expressions, and a partial index's `WHERE`.
    pub on: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unique: bool,
}

/// Moves one-column primary and foreign keys onto their columns.
pub(crate) fn place_keys(table: &mut Table) {
    if let [only] = table.primary_key.as_slice() {
        if let Some(col) = table.columns.iter_mut().find(|c| &c.name == only) {
            col.primary_key = true;
        }
        table.primary_key.clear();
    }
    table.foreign_keys.retain(|fk| {
        let [only] = fk.columns.as_slice() else {
            return true;
        };
        match table.columns.iter_mut().find(|c| &c.name == only) {
            Some(col) if col.references.is_none() => {
                col.references = Some(fk.references.clone());
                false
            }
            _ => true,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_redacted() {
        assert_eq!(
            redact("postgres://app:s3cr%40t@db.internal:5432/app"),
            "postgres://app:***@db.internal:5432/app"
        );
        assert_eq!(
            redact("postgresql://app@localhost/app?sslmode=require&password=hunter2"),
            "postgresql://app@localhost/app?sslmode=require&password=***"
        );
        assert_eq!(
            redact("host=localhost password='a b\\'c' dbname=app"),
            "host=localhost password=*** dbname=app"
        );
        assert_eq!(
            redact("host=localhost user=app password=x"),
            "host=localhost user=app password=***"
        );
        assert_eq!(
            redact("postgres://localhost/app"),
            "postgres://localhost/app"
        );
        // A `@` in the path is not userinfo.
        assert_eq!(
            redact("postgres://localhost/a@b"),
            "postgres://localhost/a@b"
        );
    }

    #[test]
    fn only_local_hosts_pass_without_remote() {
        for url in [
            "postgres://app:pw@localhost/app",
            "postgres://127.0.0.1:5433/app",
            "postgres://[::1]/app",
            "postgresql:///app?host=/run/postgresql",
            "postgres:///app",
            "host=/tmp dbname=app",
            "host=api.localhost dbname=app",
        ] {
            assert_eq!(remote_host(url), None, "{url}");
        }
        assert_eq!(
            remote_host("postgres://app:pw@db:5432/app").as_deref(),
            Some("db:5432")
        );
        assert_eq!(
            remote_host("postgres://app@prod.example.com/app").as_deref(),
            Some("prod.example.com:5432")
        );
        assert_eq!(
            remote_host("host=localhost,10.0.0.5 port=5432,6432 dbname=app").as_deref(),
            Some("10.0.0.5:6432")
        );
        // `hostaddr` is where it connects, whatever `host` says.
        assert_eq!(
            remote_host("host=localhost hostaddr=10.1.2.3 dbname=app").as_deref(),
            Some("10.1.2.3:5432")
        );
    }

    #[test]
    fn the_first_word_decides_what_is_a_read() {
        for sql in [
            "select 1",
            "  -- a comment\n  SELECT * FROM t",
            "/* x */ with a as (select 1) select * from a",
            "values (1)",
            "table users",
            "explain select 1",
            "show search_path",
        ] {
            assert!(check_read(sql, false).is_ok(), "{sql}");
        }
        assert!(check_read("pragma table_info(t)", true).is_ok());
        let err = check_read("pragma table_info(t)", false).unwrap_err();
        assert_eq!(err.exit, Exit::Precondition);
        let err = check_read("delete from users", false).unwrap_err();
        assert_eq!(err.exit, Exit::Precondition);
        assert!(err.message.contains("`DELETE`"), "{}", err.message);
        assert_eq!(
            check_read(" -- only\n", false).unwrap_err().exit,
            Exit::Usage
        );
    }

    #[test]
    fn urls_name_sqlite_or_postgres() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.db"), b"").unwrap();
        for url in [
            "app.db",
            "sqlite:app.db",
            "sqlite://app.db",
            "file:app.db?mode=ro",
        ] {
            assert!(
                matches!(parse(dir.path(), url), Ok(Target::Sqlite(p)) if p.ends_with("app.db")),
                "{url}"
            );
        }
        assert!(matches!(
            parse(dir.path(), "postgres://localhost/app"),
            Ok(Target::Postgres(_))
        ));
        assert!(matches!(
            parse(dir.path(), "host=localhost dbname=app"),
            Ok(Target::Postgres(_))
        ));
        assert_eq!(
            parse(dir.path(), "missing.db").err().unwrap().exit,
            Exit::NotFound
        );
        assert_eq!(
            parse(dir.path(), "mysql://localhost/app")
                .err()
                .unwrap()
                .exit,
            Exit::Usage
        );
    }
}
