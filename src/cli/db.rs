use usage_rs::{Args, Subcommands};

/// Read a database's schema, or run a read-only query, in SQLite or Postgres
///
/// The database is --url, else DATABASE_URL, else DATABASE_URL from the
/// project's fnox config (`fnox get`, read inside jouet and never printed).
/// A .env file is never read. A SQLite database is a file path, `sqlite:`
/// or `file:`; Postgres is a postgres:// URL or a `key=value` string. A
/// password is shown as `***`.
///
/// Only local databases are opened: a Postgres host other than localhost,
/// a loopback address or a Unix socket is refused (exit 4) without
/// --remote, so an agent does not read production by accident.
#[derive(Args)]
pub(crate) struct DbArgs {
    #[usage(subcommand)]
    pub(crate) cmd: DbCmd,
}

#[derive(Subcommands)]
pub(crate) enum DbCmd {
    /// List the tables with their columns, keys and indexes, or one table in full
    Schema(DbSchemaArgs),
    /// Run one read-only query and print its rows, cut to a limit
    Query(DbQueryArgs),
}

/// List the tables with their columns, keys and indexes, or one table in full
///
/// Every table and view, one line each: columns with their types, `pk`,
/// `not null`, defaults and `-> table(column)` foreign keys, then the
/// table's indexes. Cut to --budget, with the names of the tables left out.
/// Postgres's own schemas and partitions are left out.
///
/// With TABLE, that table alone, a column a line, with the foreign keys
/// that point at it and its row count: exact when counting takes under
/// half a second, else Postgres's estimate (`~`).
///
/// Exits 3 when TABLE does not exist.
///
/// Examples:
///   jouet db schema
///   jouet db schema users --url app.db
///   jouet db schema billing.invoices
#[derive(Args)]
pub(crate) struct DbSchemaArgs {
    /// One table or view to describe: NAME or SCHEMA.NAME
    pub(crate) table: Option<String>,
    /// The database: a SQLite file, or a Postgres URL (default:
    /// DATABASE_URL)
    #[usage(long, value_name = "URL")]
    pub(crate) url: Option<String>,
    /// Allow a Postgres host that is not this machine
    #[usage(long)]
    pub(crate) remote: bool,
    /// The most bytes to print (default 8000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
}

/// Run one read-only query and print its rows, cut to a limit
///
/// Runs one statement that reads: SELECT, WITH, VALUES, TABLE, SHOW or
/// EXPLAIN, and PRAGMA in SQLite. The database enforces it, not only the
/// statement's first word: SQLite is opened read-only and refuses a
/// statement it says writes, and Postgres runs it alone in a READ ONLY
/// transaction. Anything else, several statements, or a statement the
/// database says writes, exits 4 with the reason, even with write rights.
///
/// Prints the first --limit rows, then how many more there are (counted
/// without reading them, up to 100,000). Each value is cut to --width
/// characters and the output to --budget bytes, with a marker saying what
/// was left out. NULL prints as NULL. With --json, rows are arrays, with
/// numbers, booleans and JSON as themselves.
///
/// Exits 1 when the database rejects the query (a syntax error, an unknown
/// table), 5 when it cannot be reached, and 124 when the query runs past
/// --timeout.
///
/// Examples:
///   jouet db query 'select * from users order by id desc'
///   jouet db query 'select status, count(*) from jobs group by 1' --url app.db
///   jouet db query 'explain select * from orders where user_id = 7'
#[derive(Args)]
pub(crate) struct DbQueryArgs {
    /// The statement to run, one only
    pub(crate) sql: String,
    /// The most rows to print (default 50)
    #[usage(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// The most characters to print of one value (default 80; 0 is no
    /// limit)
    #[usage(long, value_name = "CHARS")]
    pub(crate) width: Option<usize>,
    /// The most bytes to print (default 8000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// Stop the query after DUR, such as 30s or 2m (default 30s)
    #[usage(long, value_name = "DUR")]
    pub(crate) timeout: Option<String>,
    /// The database: a SQLite file, or a Postgres URL (default:
    /// DATABASE_URL)
    #[usage(long, value_name = "URL")]
    pub(crate) url: Option<String>,
    /// Allow a Postgres host that is not this machine
    #[usage(long)]
    pub(crate) remote: bool,
}
