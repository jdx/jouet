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
const OTHER: &str = "/repos/jdx/other";

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
    advance_head_after_pr_read: bool,
    advance_head_after_graphql_read: bool,
    mergeable: Option<bool>,
    mergeable_state: &'static str,
    checks: BTreeMap<&'static str, Vec<CheckRun>>,
    /// Complete the check runs after the fake returns an unchanged check poll.
    complete_after_304: bool,
    base_sha: &'static str,
    /// For the base-change test, advance the branch after its first read and
    /// complete CI only once the compare has actually been requested.
    advance_base_after_read: bool,
    complete_after_compare: bool,
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
    /// The PR's review threads and conversation comments, as GraphQL nodes.
    review_threads: Vec<Value>,
    conversation: Vec<Value>,
    /// The branch has no pull request.
    no_pr: bool,
}

impl Default for World {
    fn default() -> Self {
        Self {
            head: SHA_A,
            advance_head_after_pr_read: false,
            advance_head_after_graphql_read: false,
            mergeable: Some(true),
            mergeable_state: "clean",
            checks: BTreeMap::new(),
            complete_after_304: false,
            base_sha: "base000",
            advance_base_after_read: false,
            complete_after_compare: false,
            base_files: Vec::new(),
            pr_files: vec!["src/a.rs"],
            required: Vec::new(),
            rate_limit_for: None,
            run_status: "in_progress",
            run_conclusion: None,
            graphql_mergeable: "UNKNOWN",
            review_threads: Vec::new(),
            conversation: Vec::new(),
            no_pr: false,
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
    let mut run = json!({
        "id": id, "node_id": format!("check-{id}"), "name": name, "status": status, "conclusion": conclusion,
        "html_url": format!("https://github.com/jdx/demo/actions/runs/900/job/{id}"),
        "app": { "slug": "github-actions" },
    });
    if name == "Entire Gates" {
        run["output"] = json!({"summary": "- ❌ **Checks**: failed — CI checks have not passed"});
    }
    run
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
        let response = graphql(world, body);
        if std::mem::take(&mut world.advance_head_after_graphql_read) {
            world.head = SHA_B;
        }
        return (200, Vec::new(), response);
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
    // jdx/demo has PR 7, and jdx/other PR 9.
    let (rest, pr) = [(REPO, "7"), (OTHER, "9")]
        .iter()
        .find_map(|(repo, pr)| Some((path.strip_prefix(repo)?, *pr)))
        .unwrap_or(("", "7"));
    let parts: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["pulls", number] if *number == pr => {
            let response = ok(json!({
                "number": pr.parse::<u64>().unwrap(), "state": "open", "merged": false, "merged_at": null,
                "head": { "sha": world.head }, "base": { "ref": "main" },
                "mergeable": world.mergeable, "mergeable_state": world.mergeable_state,
            }));
            if std::mem::take(&mut world.advance_head_after_pr_read) {
                world.head = SHA_B;
            }
            response
        }
        ["pulls"] if world.no_pr => ok(json!([])),
        ["pulls"] => ok(json!([{ "number": pr.parse::<u64>().unwrap() }])),
        ["pulls", number, "files"] if *number == pr => ok(Value::Array(
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
        ["branches", "main"] => {
            let sha = world.base_sha;
            if std::mem::take(&mut world.advance_base_after_read) {
                world.base_sha = "base111";
            }
            ok(json!({ "commit": { "sha": sha } }))
        }
        ["compare", _] => {
            if std::mem::take(&mut world.complete_after_compare) {
                world.checks.insert(
                    world.head,
                    vec![check(1, "unit", "completed", Some("success"))],
                );
            }
            ok(json!({
                "files": world.base_files.iter().map(|f| json!({ "filename": f })).collect::<Vec<_>>()
            }))
        }
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
    // The PR the request names (`$r` and `$n`, or the first of a batch):
    // jdx/demo's PR 7 unless it says another.
    let named = |key: &str| {
        let vars = &request["variables"];
        vars.get(key)
            .or_else(|| vars.get(format!("{key}0")))
            .cloned()
    };
    let number = named("n").and_then(|n| n.as_u64()).unwrap_or(7);
    let repo = format!(
        "jdx/{}",
        named("r")
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or("demo")
    );
    let url = format!("https://github.com/{repo}/pull/{number}");
    if request["variables"]["extra"].is_boolean() {
        let page = json!({ "hasNextPage": false, "endCursor": null });
        return json!({ "data": {
            "viewer": { "login": "agent" },
            "repository": { "pullRequest": {
                "id": "PR_1", "number": number, "url": url,
                "headRefName": "feat", "headRefOid": world.head, "baseRefOid": world.base_sha,
                "headRepository": { "nameWithOwner": repo },
                "reviewThreads": { "pageInfo": page, "nodes": world.review_threads },
                "reviews": { "pageInfo": page, "nodes": [] },
                "comments": { "pageInfo": page, "nodes": world.conversation }
            } }
        } })
        .to_string();
    }
    if let Some(ids) = request["variables"]["ids"].as_array() {
        let nodes: Vec<Value> = ids
            .iter()
            .map(|id| {
                let number = id
                    .as_str()
                    .and_then(|s| s.strip_prefix("check-"))
                    .and_then(|s| s.parse::<u64>().ok());
                let check = world
                    .checks
                    .values()
                    .flatten()
                    .find(|c| Some(c.id) == number);
                check.map_or(Value::Null, |c| {
                    json!({
                        "id": id, "name": c.name, "checkSuite": {
                            "app": { "slug": "github-actions" }, "branch": { "name": "feat" },
                            "workflowRun": { "event": "pull_request", "runNumber": c.id,
                                "runAttempt": 1, "workflow": { "id": "workflow-1" } }
                        }
                    })
                })
            })
            .collect();
        return json!({ "data": { "nodes": nodes } }).to_string();
    }
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
            "id": "PR_1", "number": number, "title": "a change", "url": url,
            "state": "OPEN", "isDraft": false, "baseRefName": "main", "headRefName": "feat",
            "headRefOid": world.head, "repository": { "nameWithOwner": repo },
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
            if status == 304 && target.contains("/check-runs") {
                let mut world = w.lock().unwrap();
                if std::mem::take(&mut world.complete_after_304) {
                    world.checks.insert(
                        SHA_A,
                        vec![
                            check(1, "lint", "completed", Some("success")),
                            check(2, "unit", "completed", Some("skipped")),
                        ],
                    );
                }
            }
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
fn ci_does_not_pass_while_only_the_checks_gate_reports_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![
            check(1, "lint", "completed", Some("success")),
            check(2, "Entire Gates", "completed", Some("failure")),
        ],
    );
    let fake = serve(world);
    let out = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--timeout", "0s", "--all", "--json"]].concat(),
    );
    assert_eq!(
        out.status.code(),
        Some(124),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(json_doc(&out)["result"], "timeout");
    assert!(
        json_doc(&out)["message"]
            .as_str()
            .unwrap()
            .contains("no failing or pending check is visible")
    );

    fake.set(|world| {
        world.checks.insert(
            SHA_A,
            vec![
                check(1, "lint", "completed", Some("success")),
                check(2, "Entire Gates", "completed", Some("success")),
            ],
        );
    });
    let fresh = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        fresh.path(),
        &[&CI[..], &["100ms", "--timeout", "0s", "--all", "--json"]].concat(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(json_doc(&out)["result"], "success");
}

#[test]
fn ci_ignores_a_superseded_cancelled_workflow_check() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![
            check(2, "title", "completed", Some("success")),
            check(1, "title", "completed", Some("cancelled")),
        ],
    );
    let fake = serve(world);
    let out = jouet(&fake, dir.path(), &[&CI[..], &["100ms", "--json"]].concat());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["counts"]["pass"], 1);
    assert_eq!(doc["counts"]["cancelled"], 0);
}

#[test]
fn ci_passes_and_unchanged_polls_are_304s() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World {
        complete_after_304: true,
        ..World::default()
    };
    world.checks.insert(
        SHA_A,
        vec![
            check(1, "lint", "in_progress", None),
            check(2, "unit", "queued", None),
        ],
    );
    let fake = serve(world);
    let child = spawn(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--timeout", "5s", "--json"]].concat(),
    );
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
    let out = jouet(&fake, dir.path(), &[&CI[..], &["100ms", "--json"]].concat());
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
        &[&CI[..], &["100ms", "--display", "agent"]].concat(),
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
    assert!(!text.contains("locally:"), "no jactionlint here: {text}");

    // With jactionlint installed, it says how to run the job here.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["jactionlint", "jactionlint.exe"] {
        std::fs::write(bin.join(name), "").unwrap();
    }
    let out = command(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--display", "agent"]].concat(),
    )
    .env("PATH", &bin)
    .output()
    .unwrap();
    let text = stdout(&out);
    assert!(
        text.contains("  locally: jouet ci run test --check unit\n"),
        "{text}"
    );
}

#[test]
fn ci_follows_a_moved_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World {
        advance_head_after_pr_read: true,
        ..World::default()
    };
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "in_progress", None)]);
    world
        .checks
        .insert(SHA_B, vec![check(2, "unit", "completed", Some("success"))]);
    let fake = serve(world);
    let child = spawn(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--timeout", "15s", "--jsonl"]].concat(),
    );
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
        &[&CI[..], &["100ms", "--sha", SHA_A, "--json"]].concat(),
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
                "100ms",
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
fn a_stale_conflict_right_after_a_push_is_not_believed() {
    let dir = tempfile::tempdir().unwrap();
    // GitHub still says "conflict" for a moment after the push that fixed it.
    let mut world = World {
        mergeable: Some(false),
        mergeable_state: "dirty",
        ..World::default()
    };
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "completed", Some("success"))]);
    let fake = serve(world);
    let child = spawn(&fake, dir.path(), &[&CI[..], &["100ms", "--json"]].concat());
    let w = handle(&fake);
    after(500, move || {
        let mut w = w.lock().unwrap();
        w.mergeable = Some(true);
        w.mergeable_state = "clean";
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
    assert_eq!(json_doc(&out)["result"], "success");
}

/// A conflict that GitHub keeps reporting is believed once it has lasted
/// the confirmation window (10s; shortened here).
#[test]
fn a_conflicted_pr_returns_once_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World {
        mergeable: Some(false),
        mergeable_state: "dirty",
        ..World::default()
    });
    let out = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--confirm-conflict", "100ms", "--json"]].concat(),
    );
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
            &["100ms", "--appear-timeout", "200ms", "--display", "plain"],
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
    let out = jouet(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--jsonl"]].concat(),
    );
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
        &[&CI[..], &["100ms", "--timeout", "5s", "--json"]].concat(),
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
    let mut world = World {
        advance_base_after_read: true,
        complete_after_compare: true,
        ..World::default()
    };
    world
        .checks
        .insert(SHA_A, vec![check(1, "unit", "in_progress", None)]);
    world.base_files = vec!["src/a.rs", "README.md"];
    let fake = serve(world);
    let child = spawn(
        &fake,
        dir.path(),
        &[&CI[..], &["100ms", "--timeout", "5s", "--jsonl"]].concat(),
    );
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
        &[&CI[..], &["100ms", "--timeout", "0", "--json"]].concat(),
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
            &[
                "100ms",
                "--check",
                "unit*",
                "--ignore",
                "*windows*",
                "--json",
            ],
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
            "100ms",
            "--json",
        ],
    );
    let w = handle(&fake);
    after(500, move || {
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
            "100ms",
            "--json",
        ],
    );
    let w = handle(&fake);
    after(500, move || {
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
            "--interval",
            "100ms",
            "--confirm-conflict",
            "100ms",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert_eq!(json_doc(&out)["result"], "failed");
}

#[test]
fn pr_any_change_returns_the_first_change() {
    let dir = tempfile::tempdir().unwrap();
    let fake = serve(World {
        advance_head_after_graphql_read: true,
        ..World::default()
    });
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
            "100ms",
            "--timeout",
            "15s",
            "--json",
        ],
    );
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
        &[
            "wait",
            "pr",
            "7",
            "--repo",
            "jdx/demo",
            "--until",
            "ready",
            "--quiet-for",
            "1m",
        ],
        &[
            "wait",
            "pr",
            "7",
            "--repo",
            "jdx/demo",
            "--until",
            "needs-agent",
            "--acks",
            "some",
        ],
    ] {
        let out = jouet(&fake, dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    assert_eq!(fake.total(), 0);
}

/// A GraphQL comment node posted at `at`.
fn comment_node(id: u64, author: &str, at: &str, body: &str) -> Value {
    json!({
        "fullDatabaseId": id.to_string(), "url": format!("https://github.com/jdx/demo/pull/7#discussion_r{id}"),
        "createdAt": at, "body": body, "author": { "__typename": "Bot", "login": author }
    })
}

#[test]
fn needs_agent_waits_for_quiet_then_digests_failures_and_findings_but_not_noise() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world.checks.insert(
        SHA_A,
        vec![check(101, "unit", "completed", Some("failure"))],
    );
    let page = json!({ "hasNextPage": false, "endCursor": null });
    // Far in the future, so after the wait's start.
    let later = "2099-01-01T00:00:00Z";
    world.review_threads = vec![
        json!({ "id": "T_1", "isResolved": false, "isOutdated": false, "path": "src/a.rs", "line": 3,
            "startLine": null, "originalLine": 3, "originalStartLine": null, "diffSide": "RIGHT",
            "startDiffSide": null, "comments": { "pageInfo": page, "nodes": [
                comment_node(501, "cursor", later, "### Off by one\n**Medium Severity** the loop skips the last item")
            ] } }),
        json!({ "id": "T_2", "isResolved": false, "isOutdated": false, "path": "src/a.rs", "line": 9,
            "startLine": null, "originalLine": 9, "originalStartLine": null, "diffSide": "RIGHT",
            "startDiffSide": null, "comments": { "pageInfo": page, "nodes": [
                comment_node(601, "coderabbitai", "2020-01-01T00:00:00Z", "_⚠️ Potential issue_ old"),
                comment_node(602, "coderabbitai", later, "Thanks for the fix! Looks right now.")
            ] } }),
    ];
    world.conversation = vec![comment_node(
        701,
        "coderabbitai",
        later,
        "> [!WARNING]\n> ## Review limit reached",
    )];
    let fake = serve(world);
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
            "needs-agent",
            "--quiet-for",
            "300ms",
            "--interval",
            "100ms",
            "--timeout",
            "20s",
            "--json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["result"], "needs_agent");
    assert_eq!(doc["checks"][0]["name"], "unit");
    assert_eq!(doc["checks"][0]["failed_step"], "cargo nextest");
    let threads = doc["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 1, "{threads:?}");
    assert_eq!(threads[0]["id"], 501);
    assert_eq!(threads[0]["path"], "src/a.rs");
    assert_eq!(doc["dropped"]["ack"], 1);
    assert_eq!(doc["dropped"]["rate_limit"], 1);
}

#[test]
fn needs_agent_timing_out_unsettled_keeps_since_for_the_next_wait() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World::default();
    world
        .checks
        .insert(SHA_A, vec![check(101, "unit", "in_progress", None)]);
    let page = json!({ "hasNextPage": false, "endCursor": null });
    world.review_threads = vec![
        json!({ "id": "T_1", "isResolved": false, "isOutdated": false,
        "path": "src/a.rs", "line": 3, "startLine": null, "originalLine": 3,
        "originalStartLine": null, "diffSide": "RIGHT", "startDiffSide": null,
        "comments": { "pageInfo": page, "nodes": [
            comment_node(501, "cursor", "2099-01-01T00:00:00Z", "**Medium Severity** a finding")
        ] } }),
    ];
    let fake = serve(world);
    let since = "2026-01-01T00:00:00Z";
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
            "needs-agent",
            "--since",
            since,
            "--interval",
            "100ms",
            "--timeout",
            "1s",
            "--json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(124),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let doc = json_doc(&out);
    assert_eq!(doc["state"], "the PR had not settled");
    assert_eq!(doc["next_since"], since);
}

/// `jouet watch`: the watcher a background hook starts after a push. It
/// answers with its exit code alone: 2 with the digest on stderr wakes the
/// agent, 0 says nothing.
#[cfg(unix)]
mod watch {
    use super::*;
    use std::path::PathBuf;

    /// A stand-in for the Claude Code process `CLAUDE_PID` names, killed
    /// when the test ends.
    struct Claude(Child);

    impl Claude {
        /// An interactive session: nothing on its command line says it
        /// prints and exits.
        fn interactive() -> Self {
            Self(Command::new("sleep").arg("300").spawn().unwrap())
        }

        /// `claude -p hello`: a one-shot run, which runs the hook in the
        /// foreground. The shell waits on its stdin, so it has no child.
        fn one_shot() -> Self {
            Self(
                Command::new("sh")
                    .args(["-c", "read line", "claude", "-p", "hello"])
                    .stdin(Stdio::piped())
                    .spawn()
                    .unwrap(),
            )
        }

        fn pid(&self) -> String {
            self.0.id().to_string()
        }
    }

    impl Drop for Claude {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Makes `dir` a checkout of jdx/`repo` on the branch `feat`.
    fn init(dir: &Path, repo: &str) {
        let remote = format!("https://github.com/jdx/{repo}.git");
        for args in [
            &["init", "-q", "-b", "feat"][..],
            &["remote", "add", "origin", &remote],
        ] {
            let done = Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap();
            assert!(done.success(), "git {args:?}");
        }
    }

    /// A checkout of jdx/demo on the branch `feat`, and a directory holding
    /// only the programs the watcher runs, so no `gh` is on its PATH.
    fn checkout(dir: &Path) -> PathBuf {
        init(dir, "demo");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = std::env::var_os("PATH").unwrap();
        for tool in ["git", "ps"] {
            let Some(found) = std::env::split_paths(&path)
                .map(|p| p.join(tool))
                .find(|p| p.is_file())
            else {
                continue;
            };
            let script = bin.join(tool);
            std::fs::write(
                &script,
                format!("#!/bin/sh\nexec '{}' \"$@\"\n", found.display()),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    /// What Claude Code gives a PostToolUse hook for a Bash call in session
    /// `s1`.
    fn payload(dir: &Path, id: &str, command: &str) -> Value {
        json!({
            "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_use_id": id, "cwd": dir, "tool_input": { "command": command },
            "tool_response": { "stdout": "", "stderr": "", "interrupted": false },
        })
    }

    /// `jouet watch` as the hook entry runs it, in the settle arm of a
    /// Claude Code new enough, with `payload` on stdin.
    fn start(fake: &Fake, dir: &Path, claude: &Claude, payload: &Value, args: &[&str]) -> Child {
        let mut cmd = command(fake, dir, &[&["watch", "claude"], args].concat());
        cmd.env("PATH", dir.join("bin"))
            .env("HOME", dir.join("home"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("JOUET_ENTIRE", "false")
            .env("JOUET_EXPERIMENT_SETTLE_SHARE", "100")
            .env("AI_AGENT", "claude-code_2-1-318_harness")
            .env("CLAUDE_PID", claude.pid())
            .stdin(Stdio::piped());
        for var in ["JOUET_HOOKS", "JOUET_HOOKS_LEDGER", "JOUET_SESSION_ID"] {
            cmd.env_remove(var);
        }
        let mut child = cmd.spawn().expect("jouet should start");
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.to_string().as_bytes()).unwrap();
        child
    }

    fn run(fake: &Fake, dir: &Path, claude: &Claude, payload: &Value, args: &[&str]) -> Output {
        start(fake, dir, claude, payload, args)
            .wait_with_output()
            .unwrap()
    }

    /// The leases under `dir`'s state, without their held files.
    fn leases(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir.join("state/jouet/watch"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect()
    }

    /// The lease of the watcher that just started.
    fn lease(dir: &Path) -> PathBuf {
        for _ in 0..500 {
            if let [lease] = leases(dir).as_slice() {
                return lease.clone();
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("no watcher took a lease");
    }

    /// The lease of each of the session's `count` watchers, by the PR it is
    /// for, once every one has found its PR.
    fn leases_by_pr(dir: &Path, count: usize) -> BTreeMap<String, PathBuf> {
        for _ in 0..500 {
            let found: BTreeMap<String, PathBuf> = leases(dir)
                .into_iter()
                .filter_map(|path| {
                    let lease: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
                    let pr = lease["pr"].as_u64().filter(|pr| *pr != 0)?;
                    assert_eq!(lease["session"], "s1");
                    Some((format!("{}#{pr}", lease["repo"].as_str()?), path))
                })
                .collect();
            if found.len() == count {
                return found;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the session has no {count} watchers: {:?}", leases(dir));
    }

    /// Waits for the ledger's `count`th entry of `event`: a watcher has its
    /// lease before it records that it started.
    fn await_recorded(dir: &Path, event: &str, count: usize) {
        for _ in 0..500 {
            if recorded(dir, event).len() >= count {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the ledger has no {count} entries of {event}");
    }

    /// The work tree the ledger says each entry of `event` was for.
    fn work_trees(dir: &Path, event: &str) -> Vec<String> {
        std::fs::read_to_string(dir.join("state/jouet/ledger/ledger.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|e| e["event"] == event)
            .map(|e| e["repo"].as_str().expect("a work tree").to_string())
            .collect()
    }

    /// Holds an Auto-fix event for the watcher of `lease`, as the prompt
    /// hook does.
    fn hold(lease: &Path) {
        let line = json!({ "at": 1_700_000_000, "prs": ["jdx/demo#7"], "kinds": ["ci-failure"], "checks": ["unit"] });
        std::fs::write(lease.with_extension("held"), format!("{line}\n")).unwrap();
    }

    /// What the ledger's entries of `event` recorded, in order.
    fn recorded(dir: &Path, event: &str) -> Vec<Value> {
        std::fs::read_to_string(dir.join("state/jouet/ledger/ledger.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|e| e["event"] == event)
            .map(|e| e["settle"].clone())
            .collect()
    }

    fn results(dir: &Path, event: &str) -> Vec<String> {
        recorded(dir, event)
            .iter()
            .map(|s| s["result"].as_str().unwrap().to_string())
            .collect()
    }

    const FAST: [&str; 4] = ["--interval", "100ms", "--quiet-for", "2s"];

    #[test]
    fn a_push_starts_a_watcher_that_wakes_the_agent_with_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let mut world = World::default();
        world.checks.insert(
            SHA_A,
            vec![check(101, "unit", "completed", Some("failure"))],
        );
        let fake = serve(world);
        let push = payload(dir, "t1", "git push -u origin feat");
        let child = start(&fake, dir, &claude, &push, &FAST);
        let lease = lease(dir);
        // An Auto-fix event about the PR arrives while the watcher waits.
        hold(&lease);
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        assert_eq!(stdout(&out), "");
        let digest = stderr(&out);
        assert!(digest.contains("unit"), "{digest}");
        assert!(digest.contains("cargo nextest"), "{digest}");
        assert!(
            digest.contains("held for this wait (jouet kept these Auto-fix events"),
            "{digest}"
        );
        assert!(
            digest.contains("next: jouet watches this PR again after your next push"),
            "{digest}"
        );
        assert!(leases(dir).is_empty() && !lease.with_extension("held").exists());
        assert_eq!(results(dir, "watch"), ["started"]);
        let [settle] = recorded(dir, "settle").try_into().unwrap();
        assert_eq!(
            (&settle["result"], &settle["held"], &settle["woke"]),
            (&json!("needs_agent"), &json!(1), &json!(true)),
            "{settle}"
        );
    }

    #[test]
    fn anything_but_a_push_that_worked_in_the_settle_arm_exits_0_and_says_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let fake = serve(World::default());
        let quiet = |out: &Output, what: &str| {
            assert_eq!(out.status.code(), Some(0), "{what}: {}", stderr(out));
            assert_eq!(
                (stdout(out), stderr(out)),
                (String::new(), String::new()),
                "{what}"
            );
            assert!(leases(dir).is_empty(), "{what}");
        };
        // Calls that are no push, or one that did not work or is not over.
        let mut failed = payload(dir, "t2", "git push");
        failed["hook_event_name"] = json!("PostToolUseFailure");
        let mut background = payload(dir, "t3", "git push");
        background["tool_input"]["run_in_background"] = json!(true);
        let mut no_session = payload(dir, "t4", "git push");
        no_session.as_object_mut().unwrap().remove("session_id");
        for (what, payload) in [
            ("not a push", payload(dir, "t1", "git status")),
            ("a failed push", failed),
            ("a background push", background),
            ("no session", no_session),
        ] {
            quiet(&run(&fake, dir, &claude, &payload, &FAST), what);
        }
        let push = |id: &str| payload(dir, id, "gh pr create --fill");
        // A harness that cannot be woken, and a flag that does not parse.
        let mut codex = command(&fake, dir, &["watch", "codex"]);
        codex.stdin(Stdio::piped());
        let mut child = codex.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(push("t5").to_string().as_bytes()).unwrap();
        drop(stdin);
        quiet(&child.wait_with_output().unwrap(), "codex");
        let out = run(&fake, dir, &claude, &push("t6"), &["--quiet-for", "soon"]);
        quiet(&out, "a bad flag");
        // Outside the settle arm, which is where every session is by default.
        let mut off = command(&fake, dir, &["watch", "claude"]);
        off.env("PATH", dir.join("bin"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("AI_AGENT", "claude-code_2-1-318_harness")
            .env("CLAUDE_PID", claude.pid())
            .env_remove("JOUET_EXPERIMENT_SETTLE_SHARE")
            .stdin(Stdio::piped());
        let mut child = off.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(push("t7").to_string().as_bytes()).unwrap();
        drop(stdin);
        quiet(&child.wait_with_output().unwrap(), "outside the arm");
        assert_eq!(fake.total(), 0, "none of these asked GitHub anything");
        assert_eq!(results(dir, "watch"), [""; 0]);

        // A push in the arm that gets no watcher says why in the ledger. A
        // one-shot run would hold the hook in the foreground.
        let one_shot = Claude::one_shot();
        quiet(&run(&fake, dir, &one_shot, &push("t8"), &FAST), "one-shot");
        // The same call's second hook entry does nothing at all.
        quiet(&run(&fake, dir, &one_shot, &push("t8"), &FAST), "again");
        fake.set(|w| w.no_pr = true);
        quiet(&run(&fake, dir, &claude, &push("t9"), &FAST), "no PR");
        assert_eq!(results(dir, "watch"), ["foreground", "no-pr"]);
    }

    #[test]
    fn a_pr_has_one_watcher_and_one_told_to_stop_gives_its_lease_back() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let mut world = World::default();
        world
            .checks
            .insert(SHA_A, vec![check(101, "unit", "in_progress", None)]);
        let fake = serve(world);
        let stop = |child: &Child| {
            // SAFETY: a signal to a child this test started and still owns.
            let sent = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            assert_eq!(sent, 0);
        };
        let first = start(&fake, dir, &claude, &payload(dir, "t1", "git push"), &FAST);
        let lease_path = lease(dir);
        // A second push while the first's watcher is there.
        let out = run(&fake, dir, &claude, &payload(dir, "t2", "git push"), &FAST);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(stderr(&out), "");
        assert_eq!(results(dir, "watch"), ["started", "watching"]);
        assert_eq!(leases(dir), std::slice::from_ref(&lease_path));
        // The session closes: Claude Code stops its hooks with SIGTERM.
        stop(&first);
        let out = first.wait_with_output().unwrap();
        assert_eq!(out.status.code(), None, "it dies of the signal");
        assert_eq!(stderr(&out), "");
        assert!(leases(dir).is_empty());
        assert_eq!(results(dir, "settle"), ["stopped"]);

        // One stopped with an event held leaves it for the next hook to
        // report (`lease-lost`).
        let third = start(&fake, dir, &claude, &payload(dir, "t3", "git push"), &FAST);
        let lease_path = lease(dir);
        hold(&lease_path);
        stop(&third);
        third.wait_with_output().unwrap();
        assert!(lease_path.exists() && lease_path.with_extension("held").exists());
    }

    #[test]
    fn a_push_in_another_checkout_gets_a_watcher_for_that_checkout_s_pr() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // The session's own checkout, of jdx/demo, and one of jdx/other
        // under the home directory.
        checkout(dir);
        let other = dir.join("home/src/other");
        std::fs::create_dir_all(&other).unwrap();
        init(&other, "other");
        let claude = Claude::interactive();
        let mut world = World::default();
        world
            .checks
            .insert(SHA_A, vec![check(101, "unit", "in_progress", None)]);
        let fake = serve(world);
        let push = |id: &str, command: &str| payload(dir, id, command);
        let first = start(&fake, dir, &claude, &push("t1", "git push"), &FAST);
        lease(dir);
        await_recorded(dir, "watch", 1);
        // A push the session makes from its own directory into the other
        // checkout: the PR there has no watcher, so it gets one.
        let elsewhere = "cd ~/src/other && git add tasks.md && git commit -q -m \"docs(tasks): say what lint runs\" && git push 2>&1 | tail -2 && jouet pr threads reply PRRT_1 --body done";
        let second = start(&fake, dir, &claude, &push("t2", elsewhere), &FAST);
        let by_pr = leases_by_pr(dir, 2);
        await_recorded(dir, "watch", 2);
        assert_eq!(
            by_pr.keys().collect::<Vec<_>>(),
            ["jdx/demo#7", "jdx/other#9"]
        );
        let pushed = |pr: &str| by_pr[pr].with_extension("pushed");
        assert!(!pushed("jdx/demo#7").exists() && !pushed("jdx/other#9").exists());

        // Each further push is left for the watcher of its own PR.
        let out = run(
            &fake,
            dir,
            &claude,
            &push("t3", "git -C ~/src/other push"),
            &FAST,
        );
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert!(!pushed("jdx/demo#7").exists() && pushed("jdx/other#9").exists());
        // A directory the command does not settle is the session's.
        let unknown = push("t4", "cd \"$DIR\" && git push");
        let out = run(&fake, dir, &claude, &unknown, &FAST);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert!(pushed("jdx/demo#7").exists());
        assert_eq!(
            results(dir, "watch"),
            ["started", "started", "watching", "watching"]
        );
        assert_eq!(leases(dir).len(), 2);
        // The ledger has each push under the work tree it went to.
        let [here, there, third, fourth] = work_trees(dir, "watch").try_into().unwrap();
        assert_ne!(here, there);
        assert_eq!((&third, &fourth), (&there, &here));

        for watcher in [first, second] {
            // SAFETY: a signal to a child this test started and still owns.
            let sent = unsafe { libc::kill(watcher.id() as libc::pid_t, libc::SIGTERM) };
            assert_eq!(sent, 0);
            watcher.wait_with_output().unwrap();
        }
        assert!(leases(dir).is_empty());
        assert_eq!(results(dir, "settle"), ["stopped", "stopped"]);
        assert_eq!(work_trees(dir, "settle"), [here, there]);
    }

    #[test]
    fn a_push_that_finds_the_pr_watched_leaves_its_commit_for_that_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let mut world = World::default();
        world
            .checks
            .insert(SHA_A, vec![check(101, "unit", "in_progress", None)]);
        let fake = serve(world);
        let first = start(&fake, dir, &claude, &payload(dir, "t1", "git push"), &FAST);
        let lease_path = lease(dir);
        let pushed = lease_path.with_extension("pushed");
        assert!(!pushed.exists());
        let out = run(&fake, dir, &claude, &payload(dir, "t2", "git push"), &FAST);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(results(dir, "watch"), ["started", "watching"]);
        // The checkout has no commit, so what was pushed is not the commit
        // the first watcher's digest is about: it waits once more, then
        // wakes the agent as it would have.
        assert_eq!(std::fs::read_to_string(&pushed).unwrap(), "");
        fake.set(|w| {
            w.checks.insert(
                SHA_A,
                vec![check(101, "unit", "completed", Some("failure"))],
            );
        });
        let out = first.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        let digest = stderr(&out);
        assert_eq!(digest.matches("NEEDS_AGENT").count(), 1, "{digest}");
        assert!(leases(dir).is_empty() && !pushed.exists());
        assert_eq!(results(dir, "settle"), ["needs_agent"]);
    }

    #[test]
    fn a_wait_the_agent_started_waits_on_the_commit_a_later_push_left_it() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let mut world = World::default();
        world
            .checks
            .insert(SHA_A, vec![check(101, "unit", "in_progress", None)]);
        let fake = serve(world);
        let wait = [
            &["wait", "pr", "7", "--repo", "jdx/demo"][..],
            &["--until", "needs-agent"],
            &FAST,
        ]
        .concat();
        let mut own = command(&fake, dir, &wait);
        own.env("PATH", dir.join("bin"))
            .env("HOME", dir.join("home"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("JOUET_ENTIRE", "false")
            .env("JOUET_SESSION_ID", "s1");
        for var in ["JOUET_HOOKS", "JOUET_HOOKS_LEDGER"] {
            own.env_remove(var);
        }
        let own = own.spawn().expect("jouet should start");
        let pushed = lease(dir).with_extension("pushed");
        // The push finds the agent's own wait watching the PR.
        let out = run(&fake, dir, &claude, &payload(dir, "t1", "git push"), &FAST);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(results(dir, "watch"), ["watching"]);
        assert!(pushed.exists());
        // The wait comes to a digest about a commit that is not the one
        // pushed (the checkout has none), so it reads what the push left
        // and waits again instead of printing it.
        let fail = |name: &'static str| {
            fake.set(move |w| {
                w.checks
                    .insert(SHA_A, vec![check(101, name, "completed", Some("failure"))]);
            });
        };
        fail("unit");
        for _ in 0..1_000 {
            if !pushed.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!pushed.exists(), "the wait never read what the push left");
        fail("lint");
        let out = own.wait_with_output().unwrap();
        let digest = stdout(&out);
        assert_eq!(digest.matches("NEEDS_AGENT").count(), 1, "{digest}");
        assert!(
            digest.contains("FAIL lint") && !digest.contains("FAIL unit"),
            "{digest}"
        );
        // Each round waited the quiet window, and the digest counts both.
        let took = digest
            .lines()
            .next()
            .and_then(|line| line.rsplit_once('('))
            .map_or("", |(_, took)| took.trim_end_matches(')'));
        let seconds = took.strip_suffix('s').and_then(|s| s.parse::<u64>().ok());
        assert!(
            took.contains('m') || seconds.is_some_and(|s| s >= 4),
            "{digest}"
        );
        assert!(leases(dir).is_empty() && !pushed.exists());
        assert_eq!(results(dir, "settle"), ["needs_agent"]);
    }

    #[test]
    fn a_watcher_at_its_time_limit_wakes_the_agent_only_with_what_it_held() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let mut world = World::default();
        world
            .checks
            .insert(SHA_A, vec![check(101, "unit", "in_progress", None)]);
        let fake = serve(world);
        let limit = ["--interval", "100ms", "--timeout", "2s"];
        let out = run(&fake, dir, &claude, &payload(dir, "t1", "git push"), &limit);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(stderr(&out), "");
        assert!(leases(dir).is_empty());

        let child = start(&fake, dir, &claude, &payload(dir, "t2", "git push"), &limit);
        let lease_path = lease(dir);
        hold(&lease_path);
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        let digest = stderr(&out);
        assert!(digest.contains("held for this wait"), "{digest}");
        assert!(digest.contains("ci-failure on jdx/demo#7"), "{digest}");
        assert!(leases(dir).is_empty() && !lease_path.with_extension("held").exists());
        let settles = recorded(dir, "settle");
        assert_eq!(results(dir, "settle"), ["timeout", "timeout"]);
        assert_eq!(
            (
                &settles[0]["woke"],
                &settles[1]["woke"],
                &settles[1]["held"]
            ),
            (&Value::Null, &json!(true), &json!(1))
        );
    }

    #[test]
    fn a_watcher_that_cannot_reach_github_exits_0() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        checkout(dir);
        let claude = Claude::interactive();
        let fake = serve(World::default());
        fake.set(|w| w.rate_limit_for = Some(3600));
        let out = run(&fake, dir, &claude, &payload(dir, "t1", "git push"), &FAST);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!((stdout(&out), stderr(&out)), (String::new(), String::new()));
        assert!(leases(dir).is_empty());
        assert_eq!(results(dir, "watch"), ["error"]);
    }
}
