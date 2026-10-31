---
description: A SQLite or Postgres database's schema, and read-only queries cut to a row limit and a byte budget, refused when they would write or reach a database on another machine.
---

# jouet db

An agent that needs to know what is in a database usually runs `sqlite3 app.db "select * from users"` or `psql -c "..."`. Every row then lands in its context, long values included. Nothing stops the statement from writing, and nothing stops the connection string from pointing at production. `jouet db` reads the schema compactly and runs one read-only query, cut to a limit, with a marker for the rest. The database refuses writes, not only jouet, and hosts other than this machine are refused unless asked for.

```text
jouet db schema [TABLE] [--url URL] [--remote] [--budget BYTES]
jouet db query SQL [--limit N] [--width CHARS] [--budget BYTES] [--timeout DUR] [--url URL] [--remote]
```

Every flag is listed in the [CLI reference](/cli/db).

## The schema

```output
$ jouet db schema
postgres postgres://jouet:***@127.0.0.1:55432/app (DATABASE_URL): 3 tables, 1 view
view active: id bigint, email text
teams: id integer pk default nextval('teams_id_seq'::regclass), name text not null
  indexes: teams_name_key unique (name)
users: id bigint pk generated always as identity, email text not null, team_id integer -> teams(id), profile jsonb, created_at timestamp with time zone default now()
  indexes: users_lower_email (lower(email)) WHERE (email IS NOT NULL)
billing.invoices: id integer, user_id bigint -> users(id), total numeric(10,2)
  pk (id, user_id)
```

Each table or view takes one line: its columns with their types, `pk`, `not null`, defaults, and `-> table(column)` for a foreign key. Keys of more than one column and the table's indexes follow, indented. Postgres's own schemas and the partitions of a partitioned table are left out, and `public` needs no prefix. The output stops at `--budget` bytes (8000 by default), and the tables left out are named in a last line:

```output
... 41 more tables not shown, over the 8000-byte --budget: audit_log, events, ... (jouet db schema TABLE shows one)
```

With a table's name (`NAME` or `SCHEMA.NAME`), that table alone, a column a line, with the foreign keys that point at it and its row count. The count is exact when `count(*)` finishes within half a second. Otherwise Postgres gives its estimate, marked `~`:

```output
$ jouet db schema users
postgres postgres://jouet:***@127.0.0.1:55432/app (DATABASE_URL): 1 table
users (table, 120 rows)
  id bigint pk generated always as identity
  email text not null
  team_id integer -> teams(id)
  profile jsonb
  created_at timestamp with time zone default now()
  index users_lower_email (lower(email)) WHERE (email IS NOT NULL)
  referenced by billing.invoices(user_id)
```

A table that does not exist exits `3`.

## A query

```output
$ jouet db query 'select id, email, bio, avatar from users' --url app.db --limit 2
id | email | bio | avatar
1 | u1@example.com | line one\nline two line one\nline two line one\nline two line one\nline two line…[+300 chars] | x'01ff'
2 | u2@example.com | line one\nline two line one\nline two line one\nline two line one\nline two line…[+300 chars] | x'02ff'
... 118 more rows, 118 past --limit 2; narrow the query, or raise them
(2 values cut to --width 80: select the column alone with a larger --width to see it whole)
```

- **Rows:** the first `--limit` rows (50 by default), then how many more the query has. They are counted, not read, up to 100,000.
- **Values:** each is cut to `--width` characters (80 by default), with the number of characters left out. Line breaks and tabs are escaped, `NULL` is `NULL`, and bytes print as `x'..'`.
- **Budget:** the output stops at `--budget` bytes (8000 by default). The last line says how many rows the budget left out and how many the limit did.
- **Timeout:** a query still running after `--timeout` (30s by default) is stopped and exits `124`.
- **`--json`:** rows are arrays of values. Numbers, booleans and JSON columns are themselves, and the rest is text. `more_rows`, `over_budget` and `cut_values` say what was left out.

A query the database rejects, such as a syntax error or an unknown table, exits `1` with the database's message.

## Only reads

`jouet db query` runs one statement that reads: `SELECT`, `WITH`, `VALUES`, `TABLE`, `SHOW` or `EXPLAIN`, and in SQLite `PRAGMA`. Anything else exits `4` with the reason, even when the connection has write rights:

```output
$ jouet db query 'delete from users'
jouet: refusing `DELETE`: jouet db query only reads
hint: run SELECT, WITH, VALUES, TABLE, SHOW or EXPLAIN; changes to the database are not jouet's to make
(exit 4)

$ jouet db query "select nextval('teams_id_seq')"
jouet: refused: Postgres says this statement writes (cannot execute nextval() in a read-only transaction)
(exit 4)
```

The first word only gives the reason early. The database is what enforces it:

- **SQLite** is opened read-only (`SQLITE_OPEN_READ_ONLY`, with `PRAGMA query_only`). A statement SQLite says would write (`sqlite3_stmt_readonly`) is refused before it runs, such as a `WITH` that ends in `DELETE` or a `PRAGMA` that sets a value.
- **Postgres** runs the statement in a `READ ONLY` transaction that has already taken its snapshot, so the statement cannot turn it back into a read-write one. Postgres parses the statement alone through the extended protocol, so a second statement after a `;` is refused. Rows are read through a cursor, so only the rows printed cross the network. The transaction is rolled back afterwards.

Read-only is about this connection. A function that reaches elsewhere, such as `dblink` to another database, is that database's to refuse.

## Which database

- **The connection:** `--url` first, then `DATABASE_URL` in the environment. Last comes `DATABASE_URL` from the project's [fnox](https://fnox.jdx.dev) config, if it defines one. jouet reads that with `fnox get DATABASE_URL`, takes the value from fnox's output, and never prints it. A `.env` file is never read.
- **SQLite:** a file path, `sqlite:PATH` or `file:PATH`.
- **Postgres:** a `postgres://` or `postgresql://` URL, or a libpq `key=value` string.
- **Passwords:** shown as `***` wherever the database is named, errors included.
- **Local only:** a Postgres host that is not this machine is refused unless you pass `--remote`. Local means localhost, a loopback address or a Unix socket, so an agent does not read production by accident. A Docker Compose service name such as `db` counts as another machine:

```output
$ jouet db schema --url postgres://app:pw@db.internal:5432/app
jouet: refusing db.internal:5432 (not local); pass --remote if you mean it
hint: the database is postgres://app:***@db.internal:5432/app (--url); --remote reads a database on another machine, which may be production
(exit 4)
```

- **TLS:** Postgres connections use jouet's TLS, with the system's root certificates and ring. `sslmode` works as in the URL, and the certificate is always verified.
- **Errors:** a database that cannot be reached, or a failed login, exits `5`.

## Steering

| Rule | Fires on |
|---|---|
| `unbounded-query` | `sqlite3 DB "select …"`, `sqlite3 DB .dump` or `psql -c "select …"` printing to the transcript, with no `LIMIT` and not a single aggregate row. It warns, and names the `jouet db` command to run instead. |

## In routines

`jouet.db.schema{table, url, remote, budget}` and `jouet.db.query{sql, url, remote, limit, width, budget, timeout}` need the `db.read` [capability](/routines#capabilities).
