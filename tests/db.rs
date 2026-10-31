//! `jouet db` end to end: a SQLite fixture always, and Postgres when one can
//! be started on this machine (`initdb` and `pg_ctl` on PATH, in $PGBIN, or
//! in /usr/lib/postgresql/*/bin), on a loopback port. Without one, the
//! Postgres test says so and passes; it never reaches another machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(out)))
}

fn jouet(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("JOUET_DISPLAY", "agent");
    for var in [
        "DATABASE_URL",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "PGPASSWORD",
    ] {
        cmd.env_remove(var);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.output().expect("jouet should start")
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("app.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE teams (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
         CREATE TABLE users (
           id INTEGER PRIMARY KEY,
           email TEXT NOT NULL,
           team_id INTEGER REFERENCES teams(id),
           bio TEXT
         );
         CREATE INDEX users_team ON users(team_id);
         INSERT INTO teams VALUES (1, 'core'), (2, 'docs');
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 120)
           INSERT INTO users (email, team_id, bio)
           SELECT 'u' || i || '@example.com', 1 + i % 2, printf('%.200c', 'x') FROM n;",
    )
    .unwrap();
    dir
}

#[test]
fn the_schema_lists_tables_and_describes_one() {
    let dir = fixture();
    let out = jouet(dir.path(), &[], &["db", "schema", "--url", "app.db"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "sqlite app.db (--url): 2 tables\n\
         teams: id integer pk, name text not null\n  indexes: unique (name)\n\
         users: id integer pk, email text not null, team_id integer -> teams(id), bio text\n  indexes: users_team (team_id)\n"
    );

    let out = jouet(
        dir.path(),
        &[("DATABASE_URL", "sqlite:app.db")],
        &["db", "schema", "teams"],
    );
    assert_eq!(
        stdout(&out),
        "sqlite app.db (DATABASE_URL): 1 table\n\
         teams (table, 2 rows)\n  id integer pk\n  name text not null\n  index unique (name)\n  referenced by users(team_id)\n"
    );

    let out = jouet(
        dir.path(),
        &[],
        &["db", "schema", "nope", "--url", "app.db"],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));

    // The budget names the tables it left out.
    let out = jouet(
        dir.path(),
        &[],
        &[
            "db", "schema", "--url", "app.db", "--budget", "100", "--json",
        ],
    );
    let doc = json(&out);
    assert_eq!(doc["tables"].as_array().unwrap().len(), 1);
    assert_eq!(doc["omitted_tables"], serde_json::json!(["users"]));
}

#[test]
fn a_query_prints_the_limit_and_counts_the_rest() {
    let dir = fixture();
    let out = jouet(
        dir.path(),
        &[],
        &[
            "db",
            "query",
            "select id, bio from users order by id",
            "--url",
            "app.db",
            "--limit",
            "2",
            "--width",
            "10",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "id | bio\n1 | xxxxxxxxxx…[+190 chars]\n2 | xxxxxxxxxx…[+190 chars]\n\
         ... 118 more rows, 118 past --limit 2; narrow the query, or raise them\n\
         (2 values cut to --width 10: select the column alone with a larger --width to see it whole)\n"
    );

    let out = jouet(
        dir.path(),
        &[],
        &[
            "db",
            "query",
            "select id, team_id is null as orphan, email from users where id < 3",
            "--url",
            "app.db",
            "--json",
        ],
    );
    assert_eq!(
        json(&out),
        serde_json::json!({
            "columns": ["id", "orphan", "email"],
            "rows": [[1, 0, "u1@example.com"], [2, 0, "u2@example.com"]],
        })
    );
}

#[test]
fn writes_are_refused_with_exit_4() {
    let dir = fixture();
    for sql in [
        "delete from users",
        "with x as (select 1) delete from users",
        "select 1; delete from users",
        "pragma user_version = 3",
    ] {
        let out = jouet(dir.path(), &[], &["db", "query", sql, "--url", "app.db"]);
        assert_eq!(out.status.code(), Some(4), "{sql}: {}", stderr(&out));
        assert!(stderr(&out).contains("refus"), "{sql}: {}", stderr(&out));
    }
    let out = jouet(
        dir.path(),
        &[],
        &[
            "db",
            "query",
            "select count(*) as n from users",
            "--url",
            "app.db",
        ],
    );
    assert_eq!(stdout(&out), "n\n120\n");
}

#[test]
fn another_machine_needs_remote_and_the_password_is_never_shown() {
    let dir = tempfile::tempdir().unwrap();
    let url = "postgres://app:hunter2@db.example.com:6543/app";
    let out = jouet(dir.path(), &[("DATABASE_URL", url)], &["db", "schema"]);
    assert_eq!(out.status.code(), Some(4));
    let err = stderr(&out);
    assert!(
        err.contains("refusing db.example.com:6543 (not local); pass --remote if you mean it"),
        "{err}"
    );
    assert!(
        err.contains("postgres://app:***@db.example.com:6543/app"),
        "{err}"
    );
    assert!(!err.contains("hunter2"), "{err}");

    // With --json the error is the one document, and still has no password.
    let out = jouet(
        dir.path(),
        &[("DATABASE_URL", url)],
        &["db", "query", "select 1", "--json"],
    );
    assert_eq!(out.status.code(), Some(4));
    let doc = json(&out);
    assert_eq!(doc["error"]["code"], "precondition");
    assert!(!stdout(&out).contains("hunter2"));

    let out = jouet(dir.path(), &[], &["db", "schema"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(stderr(&out).contains("no database"), "{}", stderr(&out));
}

/// A directory holding `initdb` and `pg_ctl`, if this machine has them.
fn postgres_bin() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PGBIN")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Ok(entries) = std::fs::read_dir("/usr/lib/postgresql") {
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path().join("bin")).collect();
        versions.sort();
        dirs.extend(versions.into_iter().rev());
    }
    let exe = |dir: &Path, name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    dirs.into_iter().find(|dir| {
        exe(dir, "initdb").is_file()
            && exe(dir, "pg_ctl").is_file()
            // A mise shim with no version set is not a Postgres.
            && Command::new(exe(dir, "initdb"))
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
    })
}

/// A Postgres cluster in a temporary directory, stopped when dropped.
struct Cluster {
    bin: PathBuf,
    data: PathBuf,
    port: u16,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = Command::new(self.bin.join("pg_ctl"))
            .args(["stop", "-m", "immediate", "-w", "-D"])
            .arg(&self.data)
            .output();
    }
}

fn start(bin: PathBuf, dir: &Path) -> Option<Cluster> {
    let data = dir.join("data");
    let init = Command::new(bin.join("initdb"))
        .args(["-U", "jouet", "--auth=password", "-E", "UTF8", "--pwfile"])
        .arg({
            std::fs::write(dir.join("pw"), "s3cret-pw\n").unwrap();
            dir.join("pw")
        })
        .arg("-D")
        .arg(&data)
        .output()
        .ok()?;
    if !init.status.success() {
        eprintln!("initdb failed: {}", String::from_utf8_lossy(&init.stderr));
        return None;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let started = Command::new(bin.join("pg_ctl"))
        .args(["start", "-w", "-t", "60", "-D"])
        .arg(&data)
        .arg("-l")
        .arg(dir.join("log"))
        .arg("-o")
        .arg(format!(
            "-p {port} -c listen_addresses=127.0.0.1 -c unix_socket_directories=''"
        ))
        .output()
        .ok()?;
    let cluster = Cluster { bin, data, port };
    if !started.status.success() {
        eprintln!(
            "pg_ctl start failed: {}",
            std::fs::read_to_string(dir.join("log")).unwrap_or_default()
        );
        return None;
    }
    Some(cluster)
}

#[test]
fn postgres_reads_and_refuses_writes() {
    // pg_ctl's `-o` goes through a shell on Unix; Windows runs it under CI
    // without one.
    if cfg!(windows) {
        eprintln!("skipped: the Postgres test runs on Unix");
        return;
    }
    let Some(bin) = postgres_bin() else {
        eprintln!("skipped: no initdb and pg_ctl on this machine (set PGBIN to run it)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some(cluster) = start(bin.clone(), dir.path()) else {
        eprintln!("skipped: could not start Postgres from {}", bin.display());
        return;
    };
    let url = format!(
        "postgres://jouet:s3cret-pw@127.0.0.1:{}/postgres",
        cluster.port
    );
    let psql = Command::new(bin.join("psql"))
        .args(["-q", "-v", "ON_ERROR_STOP=1", "-d", &url, "-c"])
        .arg(
            "create table teams (id serial primary key, name text not null unique);
             create table users (id bigint generated always as identity primary key,
               email text not null, team_id int references teams(id), profile jsonb);
             create schema billing;
             create table billing.invoices (id int, user_id bigint references users(id),
               primary key (id, user_id));
             insert into teams (name) values ('core'), ('docs');
             insert into users (email, team_id, profile)
               select 'u' || i || '@example.com', 1 + i % 2, jsonb_build_object('n', i)
               from generate_series(1, 120) i;",
        )
        .output()
        .unwrap();
    assert!(
        psql.status.success(),
        "{}",
        String::from_utf8_lossy(&psql.stderr)
    );
    let env = [("DATABASE_URL", url.as_str())];
    let shown = format!("postgres://jouet:***@127.0.0.1:{}/postgres", cluster.port);

    let out = jouet(dir.path(), &env, &["db", "schema"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "postgres {shown} (DATABASE_URL): 3 tables\n\
             teams: id integer pk default nextval('teams_id_seq'::regclass), name text not null\n  indexes: teams_name_key unique (name)\n\
             users: id bigint pk generated always as identity, email text not null, team_id integer -> teams(id), profile jsonb\n\
             billing.invoices: id integer, user_id bigint -> users(id)\n  pk (id, user_id)\n"
        )
    );

    let out = jouet(dir.path(), &env, &["db", "schema", "users"]);
    assert!(
        stdout(&out).contains("users (table, 120 rows)\n"),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("  referenced by billing.invoices(user_id)\n"));

    let out = jouet(
        dir.path(),
        &env,
        &[
            "db",
            "query",
            "select id, email, profile from users order by id",
            "--limit",
            "2",
            "--json",
        ],
    );
    assert_eq!(
        json(&out),
        serde_json::json!({
            "columns": ["id", "email", "profile"],
            "rows": [[1, "u1@example.com", {"n": 1}], [2, "u2@example.com", {"n": 2}]],
            "more_rows": 118,
        })
    );

    for sql in [
        "delete from users",
        "with x as (delete from users returning id) select * from x",
        "select 1; delete from users",
        "select nextval('teams_id_seq')",
        "select * from users for update",
    ] {
        let out = jouet(dir.path(), &env, &["db", "query", sql]);
        assert_eq!(out.status.code(), Some(4), "{sql}: {}", stderr(&out));
    }
    let out = jouet(
        dir.path(),
        &env,
        &["db", "query", "select count(*) from users"],
    );
    assert_eq!(stdout(&out), "count\n120\n");

    let out = jouet(
        dir.path(),
        &env,
        &["db", "query", "select pg_sleep(5)", "--timeout", "500ms"],
    );
    assert_eq!(out.status.code(), Some(124), "{}", stderr(&out));

    let out = jouet(dir.path(), &env, &["db", "query", "select * from nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    let wrong = url.replace("s3cret-pw", "wrong-pw");
    let out = jouet(dir.path(), &[], &["db", "schema", "--url", &wrong]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert!(!stderr(&out).contains("wrong-pw"), "{}", stderr(&out));
}

#[test]
fn a_routine_queries_with_db_read() {
    let dir = fixture();
    let routines = dir.path().join(".config/jouet/routines");
    std::fs::create_dir_all(&routines).unwrap();
    std::fs::write(
        routines.join("count-users.luau"),
        r#"--- about: Count the users
--- capabilities: db.read
return {
    run = function()
        local ran = jouet.db.query({ "select count(*) as n from users", url = "app.db" })
        return { ok = ran.ok, n = ran.json.rows[1][1] }
    end,
}
"#,
    )
    .unwrap();
    std::fs::write(
        routines.join("no-cap.luau"),
        "--- about: Forgets its capability\nreturn { run = function() return jouet.db.schema({ url = \"app.db\" }) end }\n",
    )
    .unwrap();
    let out = jouet(dir.path(), &[], &["run", "count-users", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out), serde_json::json!({ "ok": true, "n": 120 }));
    let out = jouet(dir.path(), &[], &["run", "no-cap"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(stderr(&out).contains("db.read"), "{}", stderr(&out));
}
