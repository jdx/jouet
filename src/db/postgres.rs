//! Postgres, through the `postgres` crate with jouet's TLS (the system's
//! root certificates, ring). Every statement runs alone in a `READ ONLY`
//! transaction that has already taken its snapshot, so it cannot write or
//! switch the transaction back to read-write, whatever its text.

use super::{COUNT_CAP, Column, Error, ForeignKey, Index, Rows, Table, Value, place_keys};
use jouet_output::exit::Exit;
use postgres::error::SqlState;
use postgres::types::Type;
use postgres::{Client, Config, NoTls, SimpleQueryMessage, Transaction};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// A connection, and the password to keep out of its errors.
pub(crate) struct Db {
    client: Client,
    password: Option<String>,
}

pub(crate) fn open(url: &str, display: &str) -> Result<Db, Error> {
    let mut config: Config = url.parse().map_err(|e| {
        Error::new(
            Exit::Usage,
            format!("{display} is not a Postgres connection string: {e}"),
        )
    })?;
    let password = config
        .get_password()
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .filter(|p| !p.is_empty());
    if config.get_connect_timeout().is_none() {
        config.connect_timeout(Duration::from_secs(10));
    }
    if config.get_application_name().is_none() {
        config.application_name("jouet");
    }
    let connected = if config.get_ssl_mode() == postgres::config::SslMode::Disable {
        config.connect(NoTls)
    } else {
        config.connect(tls()?)
    };
    let client = connected.map_err(|e| {
        let why = e
            .as_db_error()
            .map_or_else(|| chain(&e), |db| db.message().to_string());
        Error::new(
            Exit::Remote,
            scrub(
                &format!("connecting to {display}: {why}"),
                password.as_deref(),
            ),
        )
    })?;
    Ok(Db { client, password })
}

/// TLS that trusts the system's root certificates, as jouet's HTTP does.
fn tls() -> Result<tokio_postgres_rustls::MakeRustlsConnect, Error> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::new(Exit::Remote, format!("setting up TLS: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

/// An error and its causes: `error connecting to server: Connection
/// refused (os error 111)`.
fn chain(e: &postgres::Error) -> String {
    let mut s = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !s.contains(&cause_text) {
            s.push_str(": ");
            s.push_str(&cause_text);
        }
        source = cause.source();
    }
    s
}

/// `text` with the password, if it appears, replaced by `***`.
fn scrub(text: &str, password: Option<&str>) -> String {
    match password {
        Some(p) => text.replace(p, "***"),
        None => text.to_string(),
    }
}

impl Db {
    fn error(&self, e: &postgres::Error, timeout: Duration) -> Error {
        let message = scrub(
            &e.as_db_error()
                .map_or_else(|| chain(e), |db| db.message().to_string()),
            self.password.as_deref(),
        );
        match e.code() {
            Some(&SqlState::QUERY_CANCELED) => Error::new(
                Exit::Timeout,
                format!("the query ran past {}s and was stopped", timeout.as_secs()),
            )
            .hint("narrow it with WHERE or LIMIT, or raise --timeout"),
            Some(&SqlState::READ_ONLY_SQL_TRANSACTION) => Error::new(
                Exit::Precondition,
                format!("refused: Postgres says this statement writes ({message})"),
            ),
            // `DECLARE CURSOR` refuses a `WITH` that changes data.
            Some(&SqlState::FEATURE_NOT_SUPPORTED) if message.contains("data-modifying") => {
                Error::new(
                    Exit::Precondition,
                    format!("refused: Postgres says this statement writes ({message})"),
                )
            }
            Some(_) => Error::new(Exit::Failed, message),
            // No SQLSTATE: the connection failed.
            None => Error::new(Exit::Remote, message),
        }
    }

    /// A read-only transaction that has taken its snapshot, so `SET
    /// TRANSACTION READ WRITE` can no longer change it.
    fn begin(&mut self, timeout: Duration) -> Result<Transaction<'_>, postgres::Error> {
        let mut tx = self.client.build_transaction().read_only(true).start()?;
        tx.batch_execute(&format!(
            "SET LOCAL statement_timeout = {}; SELECT 1",
            timeout.as_millis()
        ))?;
        Ok(tx)
    }

    /// Runs one statement: up to `limit` rows through a cursor, then counts
    /// the rest up to [`COUNT_CAP`] without reading them.
    pub(crate) fn query(
        &mut self,
        sql: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<Rows, Error> {
        match self.run(sql, limit, timeout) {
            Ok(rows) => Ok(rows),
            Err(Fail::Pg(e)) => Err(self.error(&e, timeout)),
            Err(Fail::Refused(e)) => Err(e),
        }
    }

    fn run(&mut self, sql: &str, limit: usize, timeout: Duration) -> Result<Rows, Fail> {
        let mut tx = self.begin(timeout)?;
        // The extended protocol parses one statement only, which is what
        // keeps a second one out of the cursor below.
        let stmt = tx.prepare(sql).map_err(|e| {
            if e.as_db_error()
                .is_some_and(|db| db.message().contains("multiple commands"))
            {
                Fail::Refused(Error::new(
                    Exit::Precondition,
                    "refusing several statements: jouet db query runs one at a time",
                ))
            } else {
                Fail::Pg(e)
            }
        })?;
        let types: Vec<Type> = stmt.columns().iter().map(|c| c.type_().clone()).collect();
        let first = super::skip_comments(sql)
            .split(|c: char| !c.is_ascii_alphabetic())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let (columns, mut rows, more) = if matches!(first.as_str(), "show" | "explain") {
            // Their output is small, and a cursor cannot hold them.
            let (columns, mut rows) = text_rows(tx.simple_query(sql)?);
            let more = rows.len().saturating_sub(limit) as u64;
            rows.truncate(limit);
            (columns, rows, more)
        } else {
            tx.batch_execute(&format!("DECLARE jouet_query NO SCROLL CURSOR FOR {sql}"))?;
            let (columns, rows) =
                text_rows(tx.simple_query(&format!("FETCH FORWARD {limit} FROM jouet_query"))?);
            let moved = tx
                .simple_query(&format!("MOVE FORWARD {COUNT_CAP} IN jouet_query"))?
                .into_iter()
                .find_map(|m| match m {
                    SimpleQueryMessage::CommandComplete(n) => Some(n),
                    _ => None,
                })
                .unwrap_or(0);
            (columns, rows, moved)
        };
        tx.rollback()?;
        let columns = if columns.is_empty() {
            stmt.columns()
                .iter()
                .map(|c| c.name().to_string())
                .collect()
        } else {
            columns
        };
        let values = rows
            .iter_mut()
            .map(|row| {
                row.drain(..)
                    .enumerate()
                    .map(|(i, cell)| typed(cell, types.get(i)))
                    .collect()
            })
            .collect();
        Ok(Rows {
            columns,
            rows: values,
            more,
            more_capped: more >= COUNT_CAP,
        })
    }

    /// The tables and views, or the one named `only` (`name` or
    /// `schema.name`), with their columns, keys and indexes; for one table,
    /// its row count when counting is cheap, else Postgres's estimate.
    pub(crate) fn schema(
        &mut self,
        only: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<Table>, Error> {
        match self.read_schema(only, timeout) {
            Ok(tables) => Ok(tables),
            Err(Fail::Pg(e)) => Err(self.error(&e, timeout)),
            Err(Fail::Refused(e)) => Err(e),
        }
    }

    fn read_schema(&mut self, only: Option<&str>, timeout: Duration) -> Result<Vec<Table>, Fail> {
        let mut tx = self.begin(timeout)?;
        let mut tables = Vec::new();
        let mut by_oid: HashMap<u32, usize> = HashMap::new();
        let mut regclass = Vec::new();
        let mut estimates = Vec::new();
        for row in tx.query(TABLES, &[&only])? {
            let schema: String = row.get(1);
            let name: String = row.get(2);
            by_oid.insert(row.get(0), tables.len());
            tables.push(Table {
                name: if schema == "public" {
                    name
                } else {
                    format!("{schema}.{name}")
                },
                kind: match row.get::<_, String>(3).as_str() {
                    "r" => "table",
                    "p" => "partitioned table",
                    "v" => "view",
                    "m" => "materialized view",
                    _ => "foreign table",
                }
                .to_string(),
                ..Table::default()
            });
            estimates.push(row.get::<_, f64>(4));
            regclass.push(row.get::<_, String>(5));
        }
        for row in tx.query(COLUMNS, &[&only])? {
            let Some(&i) = by_oid.get(&row.get::<_, u32>(0)) else {
                continue;
            };
            let default: Option<String> = row.get(4);
            let default = match (
                row.get::<_, String>(5).as_str(),
                row.get::<_, String>(6).as_str(),
            ) {
                ("a", _) => Some("generated always as identity".into()),
                ("d", _) => Some("generated by default as identity".into()),
                (_, "s") => default.map(|d| format!("generated always as {d}")),
                _ => default,
            };
            tables[i].columns.push(Column {
                name: row.get(1),
                ty: row.get(2),
                not_null: row.get(3),
                default,
                ..Column::default()
            });
        }
        for row in tx.query(CONSTRAINTS, &[&only])? {
            let Some(&i) = by_oid.get(&row.get::<_, u32>(0)) else {
                continue;
            };
            let columns: Vec<String> = row.get(2);
            match row.get::<_, String>(1).as_str() {
                "p" => tables[i].primary_key = columns,
                _ => {
                    let to: String = row.get(3);
                    let to_cols: Vec<String> = row.get(4);
                    tables[i].foreign_keys.push(ForeignKey {
                        columns,
                        references: format!("{to}({})", to_cols.join(", ")),
                    });
                }
            }
        }
        for row in tx.query(INDEXES, &[&only])? {
            let Some(&i) = by_oid.get(&row.get::<_, u32>(0)) else {
                continue;
            };
            let def: String = row.get(3);
            let on = def.split_once(" USING ").map_or(def.as_str(), |(_, on)| on);
            tables[i].indexes.push(Index {
                name: row.get(1),
                on: on.strip_prefix("btree ").unwrap_or(on).to_string(),
                unique: row.get(2),
            });
        }
        for table in &mut tables {
            // A primary key's columns are never null.
            for col in &mut table.columns {
                if table.primary_key.contains(&col.name) {
                    col.not_null = false;
                }
            }
            place_keys(table);
        }
        if only.is_some() {
            for row in tx.query(REFERENCED_BY, &[&only])? {
                let Some(&i) = by_oid.get(&row.get::<_, u32>(0)) else {
                    continue;
                };
                let from: String = row.get(1);
                let cols: Vec<String> = row.get(2);
                tables[i]
                    .referenced_by
                    .push(format!("{from}({})", cols.join(", ")));
            }
            for (i, table) in tables.iter_mut().enumerate() {
                if !matches!(table.kind.as_str(), "table" | "partitioned table") {
                    continue;
                }
                table.rows = count(&mut tx, &regclass[i]);
                // `reltuples` is -1 for a table never analyzed.
                if table.rows.is_none() && estimates[i] >= 0.0 {
                    table.rows_estimate = Some(estimates[i].round() as u64);
                }
            }
        }
        tx.rollback()?;
        Ok(tables)
    }
}

enum Fail {
    Pg(postgres::Error),
    Refused(Error),
}

impl From<postgres::Error> for Fail {
    fn from(e: postgres::Error) -> Self {
        Fail::Pg(e)
    }
}

/// `count(*)` within half a second, in a savepoint, so a large table's full
/// scan is given up and the transaction carries on.
fn count(tx: &mut Transaction<'_>, regclass: &str) -> Option<u64> {
    let mut sp = tx.transaction().ok()?;
    let n = sp
        .batch_execute("SET LOCAL statement_timeout = 500")
        .and_then(|()| sp.query_one(&format!("SELECT count(*) FROM {regclass}"), &[]))
        .ok()
        .map(|row| row.get::<_, i64>(0));
    let _ = sp.rollback();
    n.and_then(|n| u64::try_from(n).ok())
}

/// The column names and text cells of a simple query's rows.
fn text_rows(messages: Vec<SimpleQueryMessage>) -> (Vec<String>, Vec<Vec<Option<String>>>) {
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(cols) => {
                columns = cols.iter().map(|c| c.name().to_string()).collect();
            }
            SimpleQueryMessage::Row(row) => {
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                rows.push(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_string))
                        .collect(),
                );
            }
            _ => {}
        }
    }
    (columns, rows)
}

/// A text cell as its column's type: numbers, booleans and JSON as
/// themselves, everything else as Postgres writes it.
fn typed(cell: Option<String>, ty: Option<&Type>) -> Value {
    let Some(text) = cell else {
        return Value::Null;
    };
    match ty {
        Some(t) if [Type::INT2, Type::INT4, Type::INT8, Type::OID].contains(t) => {
            text.parse().map_or(Value::Text(text), Value::Int)
        }
        Some(t) if [Type::FLOAT4, Type::FLOAT8].contains(t) => {
            text.parse().map_or(Value::Text(text), Value::Float)
        }
        Some(&Type::BOOL) => Value::Bool(text == "t"),
        Some(t) if [Type::JSON, Type::JSONB].contains(t) => {
            serde_json::from_str(&text).map_or(Value::Text(text), Value::Json)
        }
        _ => Value::Text(text),
    }
}

/// The tables, views and their kin outside Postgres's own schemas,
/// partitions aside, or the one `$1` names.
macro_rules! rels {
    () => {
        r"WITH rels AS (
  SELECT c.oid, n.nspname::text AS nspname, c.relname::text AS relname, c.relkind::text AS kind,
    c.reltuples::float8 AS est, c.oid::regclass::text AS reg
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND NOT c.relispartition
    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
    AND n.nspname NOT LIKE 'pg\_toast%' AND n.nspname NOT LIKE 'pg\_temp\_%'
    AND ($1::text IS NULL OR c.relname = $1 OR n.nspname || '.' || c.relname = $1)
) "
    };
}

/// `public` first, then the other schemas by name.
const TABLES: &str = concat!(
    rels!(),
    r"SELECT oid, nspname, relname, kind, est, reg FROM rels
ORDER BY nspname <> 'public', nspname, relname"
);

const COLUMNS: &str = concat!(
    rels!(),
    r"SELECT a.attrelid, a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull,
  pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text
FROM pg_attribute a JOIN rels r ON r.oid = a.attrelid
LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
WHERE a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attrelid, a.attnum"
);

/// Primary and foreign keys, with their columns in order.
const CONSTRAINTS: &str = concat!(
    rels!(),
    r"SELECT con.conrelid, con.contype::text,
  ARRAY(SELECT a.attname::text FROM unnest(con.conkey) WITH ORDINALITY k(n, i)
    JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n ORDER BY k.i),
  CASE WHEN con.contype = 'f' THEN con.confrelid::regclass::text ELSE '' END,
  ARRAY(SELECT a.attname::text FROM unnest(con.confkey) WITH ORDINALITY k(n, i)
    JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.n ORDER BY k.i)
FROM pg_constraint con JOIN rels r ON r.oid = con.conrelid
WHERE con.contype IN ('p', 'f') ORDER BY con.conrelid, con.conname"
);

/// Indexes other than the primary key's.
const INDEXES: &str = concat!(
    rels!(),
    r"SELECT i.indrelid, ic.relname::text, i.indisunique, pg_get_indexdef(i.indexrelid)
FROM pg_index i JOIN rels r ON r.oid = i.indrelid JOIN pg_class ic ON ic.oid = i.indexrelid
WHERE NOT i.indisprimary ORDER BY i.indrelid, ic.relname"
);

/// Foreign keys elsewhere that point at the tables.
const REFERENCED_BY: &str = concat!(
    rels!(),
    r"SELECT con.confrelid, con.conrelid::regclass::text,
  ARRAY(SELECT a.attname::text FROM unnest(con.conkey) WITH ORDINALITY k(n, i)
    JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n ORDER BY k.i)
FROM pg_constraint con JOIN rels r ON r.oid = con.confrelid
WHERE con.contype = 'f' ORDER BY 2, con.conname"
);
