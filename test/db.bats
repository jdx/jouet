#!/usr/bin/env bats
# shellcheck disable=SC2016 # backticks in expected output are Markdown

# `jouet db schema` and `jouet db query` against a SQLite fixture, how the
# database is found (--url, DATABASE_URL, fnox, never .env), and the
# `unbounded-query` rule. Postgres is tested in tests/db.rs, when a local
# one can be started.

setup() {
	load "test_helper/common_setup"
	_common_setup
	command -v python3 >/dev/null || skip "python3 is not installed"
	unset DATABASE_URL
	python3 -I - <<'EOF'
import sqlite3
c = sqlite3.connect("app.db")
c.executescript("""
CREATE TABLE teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL,
  team_id INTEGER REFERENCES teams(id), note TEXT);
INSERT INTO teams VALUES (1, 'core'), (2, 'docs');
""")
for i in range(1, 76):
    c.execute("INSERT INTO users (email, team_id, note) VALUES (?, ?, ?)",
              (f"u{i}@example.com", 1 + i % 2, "a\nb" if i == 1 else None))
c.commit()
EOF
}

@test "the schema is a line a table, and one table in full" {
	run --separate-stderr jouet db schema --url app.db
	assert_success
	assert_output 'sqlite app.db (--url): 2 tables
teams: id integer pk, name text not null
  indexes: unique (name)
users: id integer pk, email text not null, team_id integer -> teams(id), note text'

	run --separate-stderr jouet db schema users --url sqlite:app.db
	assert_success
	assert_line 'users (table, 75 rows)'
	assert_line '  team_id integer -> teams(id)'

	run --separate-stderr jouet db schema missing --url app.db
	assert_equal "$status" 3
}

@test "a query prints 50 rows, then counts the rest" {
	run --separate-stderr env DATABASE_URL=app.db jouet db query 'select id, note from users order by id'
	assert_success
	assert_line --index 0 'id | note'
	assert_line --index 1 '1 | a\nb'
	assert_line --index 2 '2 | NULL'
	assert_line --index 51 '... 25 more rows, 25 past --limit 50; narrow the query, or raise them'

	run --separate-stderr jouet db query 'select id from users where id > 70' --url app.db --json
	assert_success
	assert_output '{"columns":["id"],"rows":[[71],[72],[73],[74],[75]]}'
}

@test "a statement that writes is refused with exit 4, and nothing changes" {
	run --separate-stderr jouet db query 'update users set email = 1' --url app.db
	assert_equal "$status" 4
	assert_equal "$stderr" 'jouet: refusing `UPDATE`: jouet db query only reads
hint: run SELECT, WITH, VALUES, TABLE, SHOW or EXPLAIN; changes to the database are not jouet'"'"'s to make'

	run --separate-stderr jouet db query 'with t as (select 1) insert into teams (name) select 1 from t' --url app.db
	assert_equal "$status" 4
	assert_equal "$stderr" 'jouet: refused: SQLite says this statement writes; jouet db query only reads'

	run --separate-stderr jouet db query 'select count(*) as n from teams' --url app.db
	assert_output $'n\n2'
}

@test "a database on another machine needs --remote, and its password is never printed" {
	run --separate-stderr jouet db schema --url 'postgres://app:hunter2@db:5432/app'
	assert_equal "$status" 4
	assert_equal "$stderr" 'jouet: refusing db:5432 (not local); pass --remote if you mean it
hint: the database is postgres://app:***@db:5432/app (--url); --remote reads a database on another machine, which may be production'
}

@test "a .env file is not read, and fnox's DATABASE_URL is, without printing it" {
	echo 'DATABASE_URL=sqlite:app.db' >.env
	run --separate-stderr jouet db schema
	assert_equal "$status" 4
	assert_equal "$stderr" 'jouet: no database: pass --url, or set DATABASE_URL
hint: a SQLite file is --url path/to/app.db; a .env file is not read, but DATABASE_URL in the project'"'"'s fnox config is'

	# A fake fnox that answers `fnox get DATABASE_URL` from a file.
	printf '[secrets]\nDATABASE_URL = { provider = "age", value = "x" }\n' >fnox.toml
	cat >"$BATS_TEST_TMPDIR/bin/fnox" <<'EOF'
#!/usr/bin/env bash
[[ "$*" == "get DATABASE_URL" ]] || exit 2
cat "$FAKE_FNOX_VALUE"
EOF
	chmod +x "$BATS_TEST_TMPDIR/bin/fnox"
	export FAKE_FNOX_VALUE="$BATS_TEST_TMPDIR/value"

	echo 'sqlite:app.db' >"$FAKE_FNOX_VALUE"
	run --separate-stderr jouet db schema teams
	assert_success
	assert_line --index 0 'sqlite app.db (fnox): 1 table'

	echo 'postgres://app:hunter2@prod.example.com/app' >"$FAKE_FNOX_VALUE"
	run --separate-stderr jouet db query 'select 1' --json
	assert_equal "$status" 4
	assert_output --partial 'refusing prod.example.com:5432 (not local)'
	assert_output --partial 'postgres://app:***@prod.example.com/app (fnox)'
	refute_output --partial hunter2
}

@test "unbounded-query points sqlite3 and psql dumps at jouet db query" {
	run jouet rule test unbounded-query
	assert_success

	run jouet rule test unbounded-query --command "sqlite3 app.db 'select * from users'"
	assert_output --partial "Run \`jouet db query 'select * from users' --url 'app.db'\` instead."

	run jouet rule test unbounded-query --command "sqlite3 app.db 'select * from users limit 5'"
	refute_output --partial 'jouet db query'
}
