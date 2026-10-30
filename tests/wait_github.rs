//! `jouet wait ci`, `wait run` and `wait pr` against a local fake GitHub
//! whose state the tests change while a wait is running. Nothing here talks
//! to the real API.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SHA_A: &str = "aaaaaaa1111111111111111111111111111111111";
const SHA_B: &str = "bbbbbbb2222222222222222222222222222222222";
const REPO: &str = "/repos/jdx/demo";

/// A failed job's log: checkout, then the failing step, then a cleanup
/// step that must not appear in the excerpt.
const JOB_LOG: &str = "2026-10-30T10:00:00.1000000Z ##[group]Run actions/checkout@v4
2026-10-30T10:00:01.0000000Z checked out
2026-10-30T10:00:02.0000000Z ##[group]Run cargo nextest run
2026-10-30T10:00:02.1000000Z cargo nextest run
2026-10-30T10:00:02.2000000Z ##[endgroup]
2026-10-30T10:00:03.0000000Z \x1b[31mthread 'trust_root' panicked at src/config.rs:812:9:\x1b[0m
2026-10-30T10:00:03.1000000Z assertion `left == right` failed
2026-10-30T10:00:04.0000000Z error: test run failed, 1 of 2210 tests failed
2026-10-30T10:00:05.0000000Z ##[error]Process completed with exit code 100.
2026-10-30T10:00:05.0000000Z ##[group]Run Post actions/checkout@v4
2026-10-30T10:00:06.0000000Z cleaning up
";

#[derive(Clone)]
struct CheckRun {
    id: u64,
    name: &'static str,
    status: &'static str,
    conclusion: Option<&'static str>,
}

/// What the fake GitHub says. Tests change it while a wait runs.
struct World {
    head: &'static str,
    mergeable: Option<bool>,
    mergeable_state: &'static str,
    checks: BTreeMap<&'static str, Vec<CheckRun>>,
    base_sha: &'static str,
    /// Files changed on the base between two SHAs, and in the PR.
    base_files: Vec<&'static str>,
    pr_files: Vec<&'static str>,
    required: Vec<&'static str>,
    /// Answer the next REST request with an exhausted rate limit that
    /// resets this many seconds from now.
    rate_limit_for: Option<i64>,
    run_status: &'static str,
    run_conclusion: Option<&'static str>,
    graphql_mergeable: &'static str,
}

impl Default for World {
    fn default() -> Self {
        Self {
            head: SHA_A,
            mergeable: Some(true),
            mergeable_state: "clean",
            checks: BTreeMap::new(),
            base_sha: "base000",
            base_files: Vec::new(),
            pr_files: vec!["src/a.rs"],
            required: Vec::new(),
            rate_limit_for: None,
            run_status: "in_progress",
            run_conclusion: None,
            graphql_mergeable: "UNKNOWN",
        }
    }
}

impl World {
    /// The checks on a commit, which GitHub also finds by an abbreviated SHA.
    fn checks_on(&self, sha: &str) -> Option<&Vec<CheckRun>> {
        self.checks
            .iter()
            .find(|(full, _)| !sha.is_empty() && full.starts_with(sha))
            .map(|(_, checks)| checks)
    }
}

#[derive(Default)]
struct Log {
    /// Every request as `METHOD path`, with the status answered.
    requests: Vec<(String, u16)>,
}

struct Fake {
    url: String,
    world: Arc<Mutex<World>>,
    log: Arc<Mutex<Log>>,
}

impl Fake {
    fn set(&self, f: impl FnOnce(&mut World)) {
        f(&mut self.world.lock().unwrap());
    }

    fn count(&self, status: u16) -> usize {
        let log = self.log.lock().unwrap();
        log.requests.iter().filter(|(_, s)| *s == status).count()
    }

    fn total(&self) -> usize {
        self.log.lock().unwrap().requests.len()
    }
}

fn run_json(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "id": id, "name": name, "status": status, "conclusion": conclusion,
        "html_url": format!("https://github.com/jdx/demo/actions/runs/900/job/{id}"),
        "app": { "slug": "github-actions" },
    })
}

fn job_json(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "id": id, "run_id": 900, "name": name, "status": status, "conclusion": conclusion,
        "workflow_name": "test",
        "html_url": format!("https://github.com/jdx/demo/actions/runs/900/job/{id}"),
        "steps": [
            { "name": "checkout", "conclusion": "success",
              "started_at": "2026-10-30T10:00:00Z", "completed_at": "2026-10-30T10:00:01Z" },
            { "name": "cargo nextest", "conclusion": conclusion.filter(|c| *c == "failure"),
              "started_at": "2026-10-30T10:00:02Z", "completed_at": "2026-10-30T10:00:05Z" },
        ],
    })
}

/// The REST and GraphQL answers for the current world.
fn respond(
    world: &mut World,
    method: &str,
    target: &str,
    body: &[u8],
) -> (u16, Vec<(String, String)>, String) {
    let (path, _query) = target.split_once('?').unwrap_or((target, ""));
    if method == "POST" && path == "/graphql" {
        return (200, Vec::new(), graphql(world, body));
    }
    if let Some(after) = world.rate_limit_for.take() {
        let reset = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + after;
        let headers = vec![
            ("x-ratelimit-remaining".into(), "0".into()),
            ("x-ratelimit-reset".into(), reset.to_string()),
        ];
        return (
            403,
            headers,
            r#"{"message":"API rate limit exceeded"}"#.into(),
        );
    }
    let ok = |v: Value| (200, Vec::new(), v.to_string());
    let rest = path.strip_prefix(REPO).unwrap_or("");
    let parts: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["pulls", "7"] => ok(json!({
            "number": 7, "state": "open", "merged": false, "merged_at": null,
            "head": { "sha": world.head }, "base": { "ref": "main" },
            "mergeable": world.mergeable, "mergeable_state": world.mergeable_state,
        })),
        ["pulls"] => ok(json!([{ "number": 7 }])),
        ["pulls", "7", "files"] => ok(Value::Array(
            world
                .pr_files
                .iter()
                .map(|f| json!({ "filename": f }))
                .collect(),
        )),
        ["commits", sha, "check-runs"] => {
            let runs: Vec<Value> = world
                .checks_on(sha)
                .into_iter()
                .flatten()
                .map(|c| run_json(c.id, c.name, c.status, c.conclusion))
                .collect();
            ok(json!({ "total_count": runs.len(), "check_runs": runs }))
        }
        ["commits", _, "status"] => ok(json!({ "total_count": 0, "statuses": [] })),
        ["branches", "main"] => ok(json!({ "commit": { "sha": world.base_sha } })),
        ["compare", _] => ok(json!({
            "files": world.base_files.iter().map(|f| json!({ "filename": f })).collect::<Vec<_>>()
        })),
        ["actions", "jobs", id] => {
            let id: u64 = id.parse().unwrap();
            let check = world
                .checks
                .values()
                .flatten()
                .find(|c| c.id == id)
                .cloned();
            match check {
                Some(c) => ok(job_json(c.id, c.name, c.status, c.conclusion)),
                None => (404, Vec::new(), r#"{"message":"Not Found"}"#.into()),
            }
        }
        ["actions", "jobs", _, "logs"] => (200, Vec::new(), JOB_LOG.into()),
        ["actions", "runs", "900"] => ok(json!({
            "id": 900, "name": "test", "head_sha": world.head,
            "status": world.run_status, "conclusion": world.run_conclusion,
        })),
        ["actions", "runs", "900", "jobs"] => {
            let jobs: Vec<Value> = world
                .checks
                .get(world.head)
                .into_iter()
                .flatten()
                .map(|c| job_json(c.id, c.name, c.status, c.conclusion))
                .collect();
            ok(json!({ "total_count": jobs.len(), "jobs": jobs }))
        }
        _ => (404, Vec::new(), r#"{"message":"Not Found"}"#.into()),
    }
}

/// GraphQL answers: the required checks on the commit a `wait ci` query
/// names (its `$s`), and a PR for `wait pr`.
fn graphql(world: &World, body: &[u8]) -> String {
    let request: Value = serde_json::from_slice(body).unwrap_or_default();
    let sha = request["variables"]["s"].as_str().unwrap_or_default();
    let nodes: Vec<Value> = world
        .checks_on(sha)
        .into_iter()
        .flatten()
        .map(|c| json!({ "name": c.name, "isRequired": world.required.contains(&c.name) }))
        .collect();
    json!({ "data": {
        "repository": { "object": { "statusCheckRollup": { "contexts": {
            "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes
        } } } },
        "l0": { "pullRequest": {
            "id": "PR_1", "number": 7, "title": "a change", "url": "https://github.com/jdx/demo/pull/7",
            "state": "OPEN", "isDraft": false, "baseRefName": "main", "headRefName": "feat",
            "headRefOid": world.head, "repository": { "nameWithOwner": "jdx/demo" },
            "mergeable": world.graphql_mergeable, "mergeStateStatus": "CLEAN", "reviewDecision": null,
            "autoMergeRequest": null,
            "reviewThreads": { "nodes": [], "pageInfo": { "hasNextPage": false, "endCursor": null } },
            "timelineItems": { "nodes": [] },
            "commits": { "nodes": [ { "commit": { "committedDate": "2026-10-30T10:00:00Z", "statusCheckRollup": null } } ] }
        } }
    } })
    .to_string()
}

fn etag(body: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut h);
    format!("W/\"{:x}\"", h.finish())
}

/// A fake GitHub that answers from `World`, with ETags: a request whose
/// `If-None-Match` matches the current body gets a 304.
fn serve(world: World) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let world = Arc::new(Mutex::new(world));
    let log = Arc::new(Mutex::new(Log::default()));
    let (w, l) = (Arc::clone(&world), Arc::clone(&log));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).unwrap_or(0) == 0 {
                continue;
            }
            let mut words = first.split_whitespace();
            let (method, target) = (
                words.next().unwrap_or_default().to_string(),
                words.next().unwrap_or_default().to_string(),
            );
            let (mut length, mut if_none_match) = (0, None);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap_or_default();
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap(),
                    "if-none-match" => if_none_match = Some(value.trim().to_string()),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let (mut status, headers, text) =
                respond(&mut w.lock().unwrap(), &method, &target, &body);
            let tag = etag(&text);
            let text = if status == 200 && if_none_match.as_deref() == Some(tag.as_str()) {
                status = 304;
                String::new()
            } else {
                text
            };
            l.lock()
                .unwrap()
                .requests
                .push((format!("{method} {target}"), status));
            let mut head = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\nETag: {tag}\r\n",
                text.len()
            );
            for (name, value) in headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            let _ = write!(stream, "{head}\r\n{text}");
        }
    });
    Fake { url, world, log }
}

fn command(fake: &Fake, dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", "test-token")
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("JOUET_LOG_DIR", dir.join("logs"))
        // No `gh` to fall back to, as on CI runners, so a lost token fails
        // instead of quietly reading the developer's own.
        .env("PATH", dir.join("empty"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "JOUET_DISPLAY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

fn jouet(fake: &Fake, dir: &Path, args: &[&str]) -> Output {
    command(fake, dir, args)
        .output()
        .expect("jouet should start")
}

fn spawn(fake: &Fake, dir: &Path, args: &[&str]) -> Child {
    command(fake, dir, args)
        .spawn()
        .expect("jouet should start")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json_doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}{}", stdout(out), stderr(out)))
}

fn jsonl(out: &Output) -> Vec<Value> {
    stdout(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

fn events<'a>(lines: &'a [Value], name: &str) -> Vec<&'a Value> {
    lines.iter().filter(|l| l["event"] == name).collect()
}

fn check(
    id: u64,
    name: &'static str,
    status: &'static str,
    conclusion: Option<&'static str>,
) -> CheckRun {
    CheckRun {
        id,
        name,
        status,
        conclusion,
    }
}

fn after(ms: u64, f: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(ms));
        f();
    })
}

/// Lets the test change the world from another thread.
fn handle(fake: &Fake) -> Arc<Mutex<World>> {
    Arc::clone(&fake.world)
}

const CI: [&str; 6] = ["wait", "ci", "7", "--repo", "jdx/demo", "--interval"];

#[test]
fn ci_passes_and_unchanged_polls_are_304s() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![
            check(1, "lint", "in_progress", None),
            check(2, "unit", "queued", None),
        ],
    );
    let fake = serve(world);
    let child = spawn(&fake, dir.path(), &[&CI[..], &["1s", "--json"]].concat());
    // Unchanged for a few polls, then both pass.
    let w = handle(&fake);
    after(3000, move || {
        let mut w = w.lock().unwrap();
        w.checks.insert(
            SHA_A,
            vec![
                check(1, "lint", "completed", Some("success")),
                check(2, "unit", "completed", Some("skipped")),
            ],
        );
    })
    .join()
    .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "success");
    assert_eq!(doc["counts"]["pass"], 1);
    assert_eq!(doc["counts"]["skipped"], 1);
    assert!(doc.get("head_changed").is_none(), "{doc}");
    assert!(fake.count(304) > 0, "unchanged polls should be 304s");
}

#[test]
fn ci_fails_fast_with_the_failed_steps_log_while_the_run_is_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![
            check(101, "unit", "completed", Some("failure")),
            check(102, "lint", "in_progress", None),
        ],
    );
    world.required = vec!["unit"];
    let fake = serve(world);
    let out = jouet(&fake, dir.path(), &[&CI[..], &["1s", "--json"]].concat());
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "failure");
    assert_eq!(doc["counts"]["pending"], 1);
    let unit = &doc["checks"][0];
    assert_eq!(unit["name"], "unit");
    assert_eq!(unit["required"], true);
    assert_eq!(unit["failed_step"], "cargo nextest");
    assert_eq!(unit["workflow"], "test");
    assert_eq!(unit["run_id"], 900);
    let excerpt: Vec<&str> = unit["log_excerpt"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(excerpt[0], "Run cargo nextest run");
    assert!(excerpt.contains(&"thread 'trust_root' panicked at src/config.rs:812:9:"));
    assert!(
        !excerpt.iter().any(|l| l.contains("cleaning up")),
        "{excerpt:?}"
    );
    let saved = std::fs::read_to_string(unit["log_path"].as_str().unwrap()).unwrap();
    assert_eq!(saved, JOB_LOG);

    // The text form names the step and the saved log.
    let out = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["1s", "--display", "agent"]].concat(),
    );
    let text = stdout(&out);
    assert!(
        text.starts_with("FAIL jdx/demo#7 @ aaaaaaa  0 pass, 1 fail"),
        "{text}"
    );
    assert!(
        text.contains("FAIL unit  step \"cargo nextest\"  required"),
        "{text}"
    );
    assert!(text.contains("  full log: "), "{text}");
}

#[test]
fn ci_follows_a_moved_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "in_progress", None)]);
    world
        .checks
        .insert(SHA_B, vec![check(2, "unit", "in_progress", None)]);
    let fake = serve(world);
    let child = spawn(&fake, dir.path(), &[&CI[..], &["1s", "--jsonl"]].concat());
    let w = handle(&fake);
    after(1500, move || {
        let mut w = w.lock().unwrap();
        w.head = SHA_B;
        w.checks
            .insert(SHA_B, vec![check(2, "unit", "completed", Some("success"))]);
    })
    .join()
    .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let lines = jsonl(&out);
    let moved = events(&lines, "head_changed");
    assert_eq!(moved.len(), 1, "{lines:?}");
    assert_eq!(moved[0]["from"], SHA_A);
    assert_eq!(moved[0]["to"], SHA_B);
    let result = lines.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["head_sha"], SHA_B);
    assert_eq!(result["head_changed"], true);
}

#[test]
fn ci_pinned_to_a_sha_ignores_the_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World {
        head: SHA_B,
        ..World::default()
    };
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "completed", Some("success"))]);
    world
        .checks
        .insert(SHA_B, vec![check(2, "unit", "in_progress", None)]);
    let fake = serve(world);
    let out = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["1s", "--sha", SHA_A, "--json"]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(json_doc(&out)["head_sha"], SHA_A);
}

#[test]
fn required_checks_are_those_of_the_pinned_commit() {
    let dir = tempfile::tempdir().unwrap();
    // The PR has moved on to B, which has no checks yet; the pinned A has
    // finished, with `unit` required.
    let mut world = World {
        head: SHA_B,
        required: vec!["unit"],
        ..World::default()
    };
    world.checks.insert(
        SHA_A,
        vec![
            check(1, "unit", "completed", Some("success")),
            check(2, "lint", "completed", Some("success")),
        ],
    );
    let fake = serve(world);
    let out = jouet(
        &fake,
        dir.path(),
        &[
            &CI[..],
            &[
                "1s",
                "--sha",
                &SHA_A[..12],
                "--required",
                "--appear-timeout",
                "2s",
            ],
            &["--timeout", "10s", "--json"],
        ]
        .concat(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "success");
    assert_eq!(doc["checks"].as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(doc["checks"][0]["name"], "unit");
    assert_eq!(doc["checks"][0]["required"], true);
}

#[test]
fn a_conflicted_pr_returns_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World {
        mergeable: Some(false),
        mergeable_state: "dirty",
        ..World::default()
    });
    let out = jouet(&fake, dir.path(), &[&CI[..], &["1s", "--json"]].concat());
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "conflict");
    assert!(
        doc["message"]
            .as_str()
            .unwrap()
            .contains("conflicts with main")
    );
}

#[test]
fn checks_that_never_appear_are_no_checks() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World::default());
    let out = jouet(
        &fake,
        dir.path(),
        &[
            &CI[..],
            &["1s", "--appear-timeout", "1s", "--display", "plain"],
        ]
        .concat(),
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stdout(&out).starts_with("NO_CHECKS jdx/demo#7"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_rate_limit_is_slept_off_when_it_resets_in_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World {
        rate_limit_for: Some(1),
        ..World::default()
    };
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "completed", Some("success"))]);
    let fake = serve(world);
    let out = jouet(&fake, dir.path(), &[&CI[..], &["1s", "--jsonl"]].concat());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let lines = jsonl(&out);
    assert_eq!(events(&lines, "rate_limited").len(), 1, "{lines:?}");
    assert_eq!(lines.last().unwrap()["result"], "success");

    // A reset after the timeout is a remote failure.
    let dir = tempfile::tempdir().unwrap();
    fake.set(|w| w.rate_limit_for = Some(3600));
    let out = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["1s", "--timeout", "5s", "--json"]].concat(),
    );
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(json_doc(&out)["error"]["code"], "remote");
}

#[test]
fn base_commits_touching_the_prs_files_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "in_progress", None)]);
    world.base_files = vec!["src/a.rs", "README.md"];
    let fake = serve(world);
    let child = spawn(&fake, dir.path(), &[&CI[..], &["1s", "--jsonl"]].concat());
    let w = handle(&fake);
    after(1500, move || w.lock().unwrap().base_sha = "base111")
        .join()
        .unwrap();
    let w = handle(&fake);
    after(1500, move || {
        w.lock()
            .unwrap()
            .checks
            .insert(SHA_A, vec![check(1, "unit", "completed", Some("success"))]);
    })
    .join()
    .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let lines = jsonl(&out);
    let touched = events(&lines, "base_touched");
    assert_eq!(touched.len(), 1, "{lines:?}");
    assert_eq!(touched[0]["files"], json!(["src/a.rs"]));
}

#[test]
fn a_second_wait_reads_the_shared_state_and_replays_its_events() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![
            check(1, "unit", "in_progress", None),
            check(2, "lint", "completed", Some("success")),
        ],
    );
    let fake = serve(world);
    let first = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["1s", "--timeout", "0", "--json"]].concat(),
    );
    assert_eq!(first.status.code(), Some(124), "{}", stdout(&first));
    assert_eq!(json_doc(&first)["result"], "timeout");
    let requests = fake.total();
    // Within its interval of the first wait's poll, a second wait asks
    // GitHub nothing.
    let second = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["60s", "--timeout", "0", "--jsonl"]].concat(),
    );
    assert_eq!(second.status.code(), Some(124), "{}", stdout(&second));
    assert_eq!(fake.total(), requests, "the second wait polled GitHub");
    let lines = jsonl(&second);
    let registered = events(&lines, "check_registered");
    assert_eq!(registered.len(), 2, "{lines:?}");
    assert!(
        registered.iter().all(|e| e["replayed"] == true),
        "{lines:?}"
    );
    assert_eq!(events(&lines, "check_completed").len(), 1, "{lines:?}");
}

#[test]
fn check_and_ignore_select_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![
            check(1, "unit (linux)", "completed", Some("success")),
            check(2, "unit (windows)", "completed", Some("failure")),
            check(3, "lint", "in_progress", None),
        ],
    );
    let fake = serve(world);
    let out = jouet(
        &fake,
        dir.path(),
        &[
            &CI[..],
            &["1s", "--check", "unit*", "--ignore", "*windows*", "--json"],
        ]
        .concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let doc = json_doc(&out);
    assert_eq!(doc["checks"].as_array().unwrap().len(), 1);
    assert_eq!(doc["checks"][0]["name"], "unit (linux)");
}

#[test]
fn a_run_finishes_and_a_missing_run_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world
        .checks
        .insert(SHA_A, vec![check(5, "build", "in_progress", None)]);
    let fake = serve(world);
    let child = spawn(
        &fake,
        dir.path(),
        &[
            "wait",
            "run",
            "https://github.com/jdx/demo/actions/runs/900",
            "--interval",
            "1s",
            "--json",
        ],
    );
    let w = handle(&fake);
    after(1500, move || {
        let mut w = w.lock().unwrap();
        w.checks
            .insert(SHA_A, vec![check(5, "build", "completed", Some("success"))]);
        w.run_status = "completed";
        w.run_conclusion = Some("success");
    })
    .join()
    .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "success");
    assert_eq!(doc["workflow"], "test");

    let out = jouet(
        &fake,
        dir.path(),
        &["wait", "run", "901", "--repo", "jdx/demo", "--json"],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
    assert_eq!(json_doc(&out)["error"]["code"], "not_found");
}

#[test]
fn pr_mergeable_waits_out_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World::default());
    let child = spawn(
        &fake,
        dir.path(),
        &[
            "wait",
            "pr",
            "7",
            "--repo",
            "jdx/demo",
            "--until",
            "mergeable",
            "--interval",
            "1s",
            "--json",
        ],
    );
    let w = handle(&fake);
    after(1500, move || {
        w.lock().unwrap().graphql_mergeable = "MERGEABLE"
    })
    .join()
    .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "met");
    assert_eq!(doc["state"], "mergeable");

    fake.set(|w| w.graphql_mergeable = "CONFLICTING");
    let out = jouet(
        &fake,
        dir.path(),
        &[
            "wait",
            "pr",
            "7",
            "--repo",
            "jdx/demo",
            "--until",
            "mergeable",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert_eq!(json_doc(&out)["result"], "failed");
}

#[test]
fn pr_any_change_returns_the_first_change() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World::default());
    let child = spawn(
        &fake,
        dir.path(),
        &[
            "wait",
            "pr",
            "7",
            "--repo",
            "jdx/demo",
            "--until",
            "any-change",
            "--interval",
            "1s",
            "--json",
        ],
    );
    let w = handle(&fake);
    after(1500, move || w.lock().unwrap().head = SHA_B)
        .join()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(json_doc(&out)["change"]["what"], "push");
}

#[test]
fn bad_github_requests_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World::default());
    for args in [
        &["wait", "pr", "7", "--repo", "jdx/demo", "--until", "soon"][..],
        &["wait", "ci", "7", "--repo", "jdx/demo", "--sha", "main"],
        &["wait", "ci", "7", "--repo", "nope"],
        &[
            "wait",
            "ci",
            "7",
            "--repo",
            "jdx/demo",
            "--no-logs",
            "--log-lines",
            "5",
        ],
        &["wait", "run", "soon", "--repo", "jdx/demo"],
    ] {
        let out = jouet(&fake, dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    assert_eq!(fake.total(), 0);
}
