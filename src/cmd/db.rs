//! `jouet db schema [TABLE]` and `jouet db query SQL`: a database's schema,
//! and read-only queries, cut to a budget.

use super::Output;
use crate::cli::{DbArgs, DbCmd, DbQueryArgs, DbSchemaArgs};
use crate::db::{self, Connection, Error, Rows, Source, Table, Target, Value};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;

/// The default output budget, in bytes, for both commands.
const BUDGET: usize = 8000;
/// The default number of rows a query prints.
const LIMIT: usize = 50;
/// The default number of characters a value prints.
const WIDTH: usize = 80;

pub(crate) fn run(args: &DbArgs, out: &Output) -> Exit {
    let result = match &args.cmd {
        DbCmd::Schema(args) => schema(args, out),
        DbCmd::Query(args) => query(args, out),
    };
    match result {
        Ok(exit) => exit,
        Err(e) => out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    }
}

enum Open {
    Sqlite(rusqlite::Connection),
    Postgres(Box<db::postgres::Db>),
}

fn open(url: Option<&str>, remote: bool) -> Result<(Connection, Open), Error> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let conn = db::connect(&cwd, &db::Options { url, remote })?;
    let open = match &conn.target {
        Target::Sqlite(path) => Open::Sqlite(db::sqlite::open(path)?),
        Target::Postgres(url) => Open::Postgres(Box::new(db::postgres::open(url, &conn.display)?)),
    };
    Ok((conn, open))
}

// --- schema ------------------------------------------------------------------

#[derive(Serialize)]
struct SchemaReport {
    database: String,
    kind: &'static str,
    source: Source,
    tables: Vec<Table>,
    /// Tables left out to stay within the budget, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    omitted_tables: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget: Option<usize>,
}

fn schema(args: &DbSchemaArgs, out: &Output) -> Result<Exit, Error> {
    let (conn, mut open) = open(args.url.as_deref(), args.remote)?;
    let only = args.table.as_deref();
    let tables = match &mut open {
        Open::Sqlite(c) => db::sqlite::schema(c, only)?,
        Open::Postgres(c) => c.schema(only, db::TIMEOUT)?,
    };
    if let Some(name) = only
        && tables.is_empty()
    {
        return Err(Error::new(
            Exit::NotFound,
            format!("no table or view named {name} in {}", conn.describe()),
        )
        .hint("jouet db schema lists them"));
    }
    let budget = Some(args.budget.unwrap_or(BUDGET)).filter(|b| *b > 0);
    let header = header(&conn, &tables);
    let mut report = SchemaReport {
        database: conn.display.clone(),
        kind: conn.kind(),
        source: conn.source,
        tables: Vec::new(),
        omitted_tables: Vec::new(),
        budget: None,
    };
    let mut text = header;
    let single = only.is_some();
    for table in tables {
        let block = if single {
            one_table(&table)
        } else {
            table_line(&table)
        };
        let fits = budget.is_none_or(|b| text.len() + block.len() <= b);
        if fits && report.omitted_tables.is_empty() {
            text.push_str(&block);
            report.tables.push(table);
        } else {
            report.omitted_tables.push(table.name);
        }
    }
    if !report.omitted_tables.is_empty() {
        report.budget = budget;
        let _ = writeln!(
            text,
            "... {} not shown, over the {}-byte --budget: {} (jouet db schema TABLE shows one)",
            plural(report.omitted_tables.len() as u64, "more table"),
            budget.unwrap_or_default(),
            names(&report.omitted_tables, 600)
        );
    }
    out.result(&report, || text);
    Ok(Exit::Ok)
}

/// `sqlite app.db (--url): 3 tables, 1 view`.
fn header(conn: &Connection, tables: &[Table]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for table in tables {
        match counts.iter_mut().find(|(k, _)| *k == table.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((&table.kind, 1)),
        }
    }
    // Tables first, then views and the rest, as they come.
    counts.sort_by_key(|(kind, _)| *kind != "table");
    let counts: Vec<String> = counts
        .iter()
        .map(|(kind, n)| plural(*n as u64, kind))
        .collect();
    format!(
        "{} {}: {}\n",
        conn.kind(),
        conn.describe(),
        if counts.is_empty() {
            "no tables".to_string()
        } else {
            counts.join(", ")
        }
    )
}

/// `id integer pk`, `email text not null`, `team_id integer -> teams(id)`.
fn column(col: &db::Column) -> String {
    let mut s = format!("{} {}", col.name, col.ty);
    if col.primary_key {
        s.push_str(" pk");
    }
    if col.not_null {
        s.push_str(" not null");
    }
    match &col.default {
        // Identity and generated columns, from Postgres.
        Some(generated) if generated.starts_with("generated ") => {
            let _ = write!(s, " {generated}");
        }
        Some(default) => {
            let _ = write!(s, " default {default}");
        }
        None => {}
    }
    if let Some(to) = &col.references {
        let _ = write!(s, " -> {to}");
    }
    s
}

fn index(index: &db::Index) -> String {
    // SQLite names a UNIQUE constraint's index itself; the name says nothing.
    let name = if index.name.starts_with("sqlite_autoindex_") {
        ""
    } else {
        &index.name
    };
    let unique = if index.unique { "unique" } else { "" };
    [name, unique, &index.on]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One table on a line, its keys and indexes indented under it.
fn table_line(table: &Table) -> String {
    let kind = if table.kind == "table" {
        String::new()
    } else {
        format!("{} ", table.kind)
    };
    let cols: Vec<String> = table.columns.iter().map(column).collect();
    let mut s = format!("{kind}{}: {}\n", table.name, cols.join(", "));
    if !table.primary_key.is_empty() {
        let _ = writeln!(s, "  pk ({})", table.primary_key.join(", "));
    }
    for fk in &table.foreign_keys {
        let _ = writeln!(s, "  fk ({}) -> {}", fk.columns.join(", "), fk.references);
    }
    if !table.indexes.is_empty() {
        let indexes: Vec<String> = table.indexes.iter().map(index).collect();
        let _ = writeln!(s, "  indexes: {}", indexes.join("; "));
    }
    s
}

/// One table in full: a column a line, then its keys, indexes and what
/// points at it.
fn one_table(table: &Table) -> String {
    let rows = match (table.rows, table.rows_estimate) {
        (Some(n), _) => format!(", {}", plural(n, "row")),
        (None, Some(n)) => format!(", ~{} (estimate)", plural(n, "row")),
        _ => String::new(),
    };
    let mut s = format!("{} ({}{rows})\n", table.name, table.kind);
    for col in &table.columns {
        let _ = writeln!(s, "  {}", column(col));
    }
    if !table.primary_key.is_empty() {
        let _ = writeln!(s, "  pk ({})", table.primary_key.join(", "));
    }
    for fk in &table.foreign_keys {
        let _ = writeln!(s, "  fk ({}) -> {}", fk.columns.join(", "), fk.references);
    }
    for i in &table.indexes {
        let _ = writeln!(s, "  index {}", index(i));
    }
    if !table.referenced_by.is_empty() {
        let _ = writeln!(s, "  referenced by {}", table.referenced_by.join(", "));
    }
    s
}

/// Names joined with `, `, up to about `max` bytes, then `and N more`.
fn names(list: &[String], max: usize) -> String {
    let mut s = String::new();
    for (i, name) in list.iter().enumerate() {
        if !s.is_empty() && s.len() + name.len() > max {
            let _ = write!(s, ", and {} more", list.len() - i);
            break;
        }
        if !s.is_empty() {
            s.push_str(", ");
        }
        s.push_str(name);
    }
    s
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

// --- query -------------------------------------------------------------------

#[derive(Serialize)]
struct QueryReport {
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
    /// Rows not shown: past --limit (counted, not read) and over the
    /// budget.
    #[serde(skip_serializing_if = "is_zero")]
    more_rows: u64,
    /// Counting stopped at 100,000, so there are at least `more_rows`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    more_rows_at_least: bool,
    /// Of `more_rows`, those read but left out for the budget.
    #[serde(skip_serializing_if = "is_zero")]
    over_budget: u64,
    /// Values cut to --width.
    #[serde(skip_serializing_if = "is_zero")]
    cut_values: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn query(args: &DbQueryArgs, out: &Output) -> Result<Exit, Error> {
    let timeout = match args.timeout.as_deref() {
        Some(t) => jouet_output::time::parse_duration(t)
            .map_err(|e| Error::new(Exit::Usage, format!("--timeout: {e}")))?,
        None => db::TIMEOUT,
    };
    let (conn, mut open) = open(args.url.as_deref(), args.remote)?;
    db::check_read(&args.sql, matches!(conn.target, Target::Sqlite(_)))?;
    let limit = args.limit.unwrap_or(LIMIT);
    let rows = match &mut open {
        Open::Sqlite(c) => db::sqlite::query(c, &args.sql, limit, timeout)?,
        Open::Postgres(c) => c.query(&args.sql, limit, timeout)?,
    };
    let width = Some(args.width.unwrap_or(WIDTH)).filter(|w| *w > 0);
    let budget = Some(args.budget.unwrap_or(BUDGET)).filter(|b| *b > 0);
    let (report, text) = render(&rows, limit, width, budget, out.mode);
    out.result(&report, || text);
    Ok(Exit::Ok)
}

/// The rows as text and as a report, each value cut to `width`, the text
/// to `budget`; both hold the same rows.
fn render(
    rows: &Rows,
    limit: usize,
    width: Option<usize>,
    budget: Option<usize>,
    mode: Mode,
) -> (QueryReport, String) {
    // Each row's cells, and how many of them were cut.
    let clipped: Vec<(Vec<String>, u64)> = rows
        .rows
        .iter()
        .map(|row| {
            let mut cut = 0;
            let cells = row
                .iter()
                .map(|v| {
                    let (s, was_cut) = clip(&text(v), width);
                    cut += u64::from(was_cut);
                    s
                })
                .collect();
            (cells, cut)
        })
        .collect();
    let cells: Vec<&Vec<String>> = clipped.iter().map(|(c, _)| c).collect();
    let human = mode == Mode::Human;
    let widths: Vec<usize> = (0..rows.columns.len())
        .map(|i| {
            cells
                .iter()
                .map(|r| r[i].chars().count())
                .chain(std::iter::once(rows.columns[i].chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |row: &[String]| -> String {
        let mut s = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                s.push_str(" | ");
            }
            s.push_str(cell);
            if human && i + 1 < row.len() {
                let pad = widths[i].saturating_sub(cell.chars().count());
                s.extend(std::iter::repeat_n(' ', pad));
            }
        }
        s.push('\n');
        s
    };
    let mut text = line(&rows.columns);
    let mut shown = 0;
    for row in &cells {
        let l = line(row);
        if budget.is_some_and(|b| text.len() + l.len() > b) {
            break;
        }
        text.push_str(&l);
        shown += 1;
    }
    if cells.is_empty() {
        text.push_str("(no rows)\n");
    }
    let cut: u64 = clipped[..shown].iter().map(|(_, n)| n).sum();
    let over_budget = (cells.len() - shown) as u64;
    let more = over_budget + rows.more;
    if more > 0 {
        let past = if rows.more_capped {
            format!("at least {} past --limit {limit}", rows.more)
        } else {
            format!("{} past --limit {limit}", rows.more)
        };
        let over = format!(
            "{over_budget} over the {}-byte --budget",
            budget.unwrap_or_default()
        );
        let why = match (over_budget > 0, rows.more > 0) {
            (true, true) => format!(": {over}, {past}"),
            (true, false) => format!(", {over}"),
            _ => format!(", {past}"),
        };
        let at_least = if rows.more_capped { "at least " } else { "" };
        let _ = writeln!(
            text,
            "... {at_least}{}{why}; narrow the query, or raise them",
            plural(more, "more row")
        );
    }
    if cut > 0 {
        let _ = writeln!(
            text,
            "({} cut to --width {}: select the column alone with a larger --width to see it whole)",
            plural(cut, "value"),
            width.unwrap_or_default()
        );
    }
    let report = QueryReport {
        columns: rows.columns.clone(),
        rows: rows.rows[..shown]
            .iter()
            .map(|row| row.iter().map(|v| json(v, width)).collect())
            .collect(),
        more_rows: more,
        more_rows_at_least: rows.more_capped,
        over_budget,
        cut_values: cut,
    };
    (report, text)
}

/// A value as text: `NULL`, a number, `true`, text with its line breaks
/// and tabs escaped, JSON, or `x'..'` bytes.
fn text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Text(s) => s
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t"),
        Value::Json(j) => j.to_string(),
        Value::Blob(b) => {
            let mut s = String::with_capacity(b.len() * 2 + 3);
            s.push_str("x'");
            for byte in b {
                let _ = write!(s, "{byte:02x}");
            }
            s.push('\'');
            s
        }
    }
}

/// `s` cut to `width` characters, with a marker naming how many were left
/// out, and whether it was cut.
fn clip(s: &str, width: Option<usize>) -> (String, bool) {
    let Some(width) = width else {
        return (s.to_string(), false);
    };
    let total = s.chars().count();
    if total <= width {
        return (s.to_string(), false);
    }
    let kept: String = s.chars().take(width).collect();
    (format!("{kept}…[+{} chars]", total - width), true)
}

/// A value in `--json`: numbers, booleans and JSON as themselves (a value
/// longer than `width` is cut text), the rest as text.
fn json(v: &Value, width: Option<usize>) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Int(i) => J::from(*i),
        Value::Float(f) => {
            serde_json::Number::from_f64(*f).map_or_else(|| J::String(f.to_string()), J::Number)
        }
        Value::Bool(b) => J::Bool(*b),
        Value::Json(j) => {
            let s = j.to_string();
            match clip(&s, width) {
                (_, false) => j.clone(),
                (cut, true) => J::String(cut),
            }
        }
        Value::Text(s) => J::String(clip(s, width).0),
        Value::Blob(_) => J::String(clip(&text(v), width).0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize, more: u64) -> Rows {
        Rows {
            columns: vec!["id".into(), "note".into()],
            rows: (0..n)
                .map(|i| vec![Value::Int(i as i64), Value::Text(format!("line\n{i}"))])
                .collect(),
            more,
            more_capped: false,
        }
    }

    #[test]
    fn rows_print_with_a_marker_for_the_rest() {
        let (report, text) = render(&rows(3, 47), 3, Some(80), Some(8000), Mode::Agent);
        assert_eq!(
            text,
            "id | note\n0 | line\\n0\n1 | line\\n1\n2 | line\\n2\n\
             ... 47 more rows, 47 past --limit 3; narrow the query, or raise them\n"
        );
        assert_eq!(report.more_rows, 47);
        assert_eq!(report.rows.len(), 3);
    }

    #[test]
    fn the_budget_and_width_cut_with_markers() {
        let (report, text) = render(&rows(50, 0), 50, Some(4), Some(60), Mode::Agent);
        assert!(text.contains("0 | line…[+3 chars]\n"), "{text}");
        assert!(
            text.contains("over the 60-byte --budget; narrow the query"),
            "{text}"
        );
        assert_eq!(report.over_budget, 50 - report.rows.len() as u64);
        assert_eq!(report.more_rows, report.over_budget);
        assert!(report.cut_values > 0);
    }

    #[test]
    fn no_rows_says_so() {
        let (_, text) = render(&rows(0, 0), 50, Some(80), Some(8000), Mode::Agent);
        assert_eq!(text, "id | note\n(no rows)\n");
    }

    #[test]
    fn names_are_cut_with_a_count() {
        let list: Vec<String> = (0..100).map(|i| format!("table_{i}")).collect();
        let s = names(&list, 40);
        assert!(s.ends_with("more"), "{s}");
        assert!(s.starts_with("table_0, table_1"), "{s}");
    }
}
