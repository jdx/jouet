//! `jouet pr next` and `jouet pr land` against a local fake GitHub that
//! answers from a small model of one PR. Nothing here talks to the real API,
//! and nothing real is ever merged.

mod common;

use common::{CUT_SHORT, Fake, RATE_LIMITED, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SHA_A: &str = "aaaaaaa1111111111111111111111111111111111";
const SHA_B: &str = "bbbbbbb2222222222222222222222222222222222";
const MERGE_SHA: &str = "ccccccc3333333333333333333333333333333333";
const PR: &str = "/repos/jdx/demo/pulls/7";

/// A check run: its name, status and conclusion.
type Run = (&'static str, &'static str, Option<&'static str>);

/// What the fake GitHub says about `jdx/demo#7`. Tests change it while a
/// command runs.
struct World {
    head: &'static str,
    /// GraphQL's `mergeStateStatus`.
    merge_state: &'static str,
    review: Option<&'static str>,
    /// Check runs by commit.
    checks: BTreeMap<&'static str, Vec<Run>>,
    merged: bool,
    /// What a merge is answered with, when the head matches.
    merge_reply: (u16, &'static str),
    /// What an update moves the head to, and the merge state after it.
    update_to: (&'static str, &'static str),
    /// Answer the next GraphQL request with an exhausted rate limit.
    rate_limited: bool,
}

impl Default for World {
    fn default() -> Self {
        Self {
            head: SHA_A,
            merge_state: "CLEAN",
            review: None,
            checks: BTreeMap::from([(
                SHA_A,
                vec![
                    ("lint", "completed", Some("success")),
                    ("unit", "completed", Some("success")),
                ],
            )]),
            merged: false,
            merge_reply: (200, ""),
            update_to: (SHA_B, "CLEAN"),
            rate_limited: false,
        }
    }
}

impl World {
    fn checks_on(&self, sha: &str) -> Vec<Run> {
        self.checks
            .iter()
            .find(|(full, _)| !sha.is_empty() && full.starts_with(sha))
            .map(|(_, c)| c.clone())
            .unwrap_or_default()
    }

    /// The PR as the `pr status` query returns it.
    fn graphql_pr(&self) -> Value {
        let (mut pass, mut fail, mut pending) = (0, 0, 0);
        for (_, status, conclusion) in self.checks_on(self.head) {
            match (status, conclusion) {
                ("completed", Some("success")) => pass += 1,
                ("completed", _) => fail += 1,
                _ => pending += 1,
            }
        }
        let rollup = if pass + fail + pending == 0 {
            Value::Null
        } else {
            json!({ "id": "ROLLUP_1", "contexts": {
                "checkRunCountsByState": [
                    { "state": "SUCCESS", "count": pass },
                    { "state": "FAILURE", "count": fail },
                    { "state": "IN_PROGRESS", "count": pending },
                ],
                "statusContextCountsByState": [],
            } })
        };
        let mergeable = match self.merge_state {
            "DIRTY" => "CONFLICTING",
            "UNKNOWN" => "UNKNOWN",
            _ => "MERGEABLE",
        };
        json!({
            "id": "PR_1", "number": 7, "title": "a change",
            "url": "https://github.com/jdx/demo/pull/7",
            "state": if self.merged { "MERGED" } else { "OPEN" }, "isDraft": false,
            "baseRefName": "main", "headRefName": "feat", "headRefOid": self.head,
            "repository": { "nameWithOwner": "jdx/demo" },
            "mergeable": mergeable, "mergeStateStatus": self.merge_state,
            "reviewDecision": self.review, "autoMergeRequest": null,
            "reviewThreads": { "nodes": [], "pageInfo": { "hasNextPage": false, "endCursor": null } },
            "timelineItems": { "nodes": [] },
            "commits": { "nodes": [ { "commit": {
                "committedDate": "2026-10-30T10:00:00Z", "statusCheckRollup": rollup
            } } ] }
        })
    }
}

fn respond(world: &mut World, request: &Request) -> (u16, String) {
    let ok = |v: Value| (200, v.to_string());
    let (path, _) = request.path.split_once('?').unwrap_or((&request.path, ""));
    if request.method == "POST" && path == "/graphql" {
        if std::mem::take(&mut world.rate_limited) {
            return (
                RATE_LIMITED,
                r#"{"message":"API rate limit exceeded"}"#.into(),
            );
        }
        let q = query(request);
        if q.contains("isRequired(pullRequestNumber") {
            let sha = request.body["variables"]["s"].as_str().unwrap_or_default();
            let nodes: Vec<Value> = world
                .checks_on(sha)
                .iter()
                .map(|(name, ..)| json!({ "name": name, "isRequired": true }))
                .collect();
            return ok(
                json!({ "data": { "repository": { "object": { "statusCheckRollup": {
                "contexts": { "pageInfo": { "hasNextPage": false }, "nodes": nodes }
            } } } } }),
            );
        }
        if q.contains("... on StatusCheckRollup") {
            let nodes: Vec<Value> = world
                .checks_on(world.head)
                .iter()
                .map(|(name, _, conclusion)| json!({ "name": name, "conclusion": conclusion.map(str::to_ascii_uppercase) }))
                .collect();
            return ok(json!({ "data": { "l0": { "contexts": {
                "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes
            } } } }));
        }
        return ok(json!({ "data": { "l0": { "pullRequest": world.graphql_pr() } } }));
    }
    match (request.method.as_str(), path.strip_prefix(PR)) {
        ("GET", Some("")) => ok(json!({
            "number": 7, "state": if world.merged { "closed" } else { "open" },
            "merged": world.merged,
            "merge_commit_sha": if world.merged { Some(MERGE_SHA) } else { None },
            "head": { "sha": world.head, "ref": "feat" }, "base": { "ref": "main" },
            "mergeable": true, "mergeable_state": world.merge_state.to_ascii_lowercase(),
        })),
        ("GET", Some("/files")) => ok(json!([{ "filename": "src/a.rs" }])),
        ("PUT", Some("/update-branch")) => {
            if request.body["expected_head_sha"] != world.head {
                return (
                    422,
                    r#"{"message":"expected head sha didn't match current head ref."}"#.into(),
                );
            }
            (world.head, world.merge_state) = world.update_to;
            (202, r#"{"message":"Updating pull request branch."}"#.into())
        }
        ("PUT", Some("/merge")) => {
            if request.body["sha"] != world.head {
                return (
                    409,
                    r#"{"message":"Head branch was modified. Review and try the merge again."}"#
                        .into(),
                );
            }
            let (status, message) = world.merge_reply;
            if status == 200 || status == CUT_SHORT {
                world.merged = true;
                let body = json!({ "sha": MERGE_SHA, "merged": true, "message": "Pull Request successfully merged" });
                return (status, body.to_string());
            }
            (status, json!({ "message": message }).to_string())
        }
        _ => {
            let checks = path
                .strip_prefix("/repos/jdx/demo/commits/")
                .and_then(|rest| rest.split_once('/'));
            match checks {
                Some((sha, "check-runs")) => {
                    let runs: Vec<Value> = world
                        .checks_on(sha)
                        .iter()
                        .enumerate()
                        .map(|(i, (name, status, conclusion))| json!({
                            "id": i + 1, "name": name, "status": status, "conclusion": conclusion,
                            "html_url": format!("https://example.com/check/{}", i + 1),
                            "app": { "slug": "other-ci" },
                        }))
                        .collect();
                    ok(json!({ "total_count": runs.len(), "check_runs": runs }))
                }
                Some((_, "status")) => ok(json!({ "total_count": 0, "statuses": [] })),
                _ => (404, r#"{"message":"Not Found"}"#.into()),
            }
        }
    }
}

fn github(world: World) -> (Fake, Arc<Mutex<World>>) {
    let world = Arc::new(Mutex::new(world));
    let shared = Arc::clone(&world);
    let fake = serve(move |request, _| respond(&mut shared.lock().unwrap(), request));
    (fake, world)
}

/// Runs jouet with a token, no agent markers and no terminal, so it
/// behaves as it would under an agent that did not say it was one.
fn jouet(fake: &Fake, dir: &Path, args: &[&str]) -> Output {
    command(fake, dir, args)
        .output()
        .expect("jouet should start")
}

fn command(fake: &Fake, dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", "test-token")
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("JOUET_LOG_DIR", dir.join("logs"))
        .env("PATH", dir.join("empty"))
        .stdin(Stdio::null());
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

const LAND: [&str; 6] = ["pr", "land", "7", "--repo", "jdx/demo", "--settle"];

fn land(fake: &Fake, dir: &Path, extra: &[&str]) -> Output {
    let mut args = LAND.to_vec();
    args.push("0");
    args.extend_from_slice(extra);
    jouet(fake, dir, &args)
}

/// The writes the fake received, as `METHOD path`.
fn writes(fake: &Fake) -> Vec<String> {
    fake.requests()
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

#[test]
fn a_ready_pr_is_merged_at_its_head_and_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World::default());
    let out = land(&fake, dir.path(), &["--yes", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["result"], "merged");
    assert_eq!(doc["method"], "squash");
    assert_eq!(doc["merge_commit"], MERGE_SHA);
    assert_eq!(doc["head_sha"], &SHA_A[..10]);
    assert_eq!(writes(&fake), [format!("PUT {PR}/merge")]);
    assert!(fake.requests().iter().all(|r| r.token == "test-token"));
    let merge = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT")
        .unwrap();
    assert_eq!(
        merge.body,
        json!({ "merge_method": "squash", "sha": SHA_A })
    );

    // Text, with another method.
    let (fake, _) = github(World::default());
    let out = land(&fake, dir.path(), &["--yes", "--method", "rebase"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "merged jdx/demo#7 (rebase) as {MERGE_SHA}\n  https://github.com/jdx/demo/pull/7\n"
        )
    );
}

#[test]
fn a_pr_that_is_not_ready_is_refused_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World {
        review: Some("REVIEW_REQUIRED"),
        merge_state: "BLOCKED",
        ..World::default()
    });
    let out = land(&fake, dir.path(), &["--yes", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["code"], "precondition");
    assert_eq!(
        error["message"],
        "jdx/demo#7 is not ready to merge: awaiting_approval (Waiting for an approving review)"
    );
    assert_eq!(
        error["hint"],
        "next: jouet wait pr jdx/demo#7 --until approved"
    );
    assert!(writes(&fake).is_empty());

    let (fake, _) = github(World {
        merged: true,
        ..World::default()
    });
    let out = land(&fake, dir.path(), &["--yes"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(stderr(&out), "jouet: jdx/demo#7 is already merged\n");
    assert!(writes(&fake).is_empty());

    // Behind its base, it is refused unless --update was given.
    let (fake, _) = github(World {
        merge_state: "BEHIND",
        ..World::default()
    });
    let out = land(&fake, dir.path(), &["--yes"]);
    assert_eq!(out.status.code(), Some(4));
    let err = stderr(&out);
    assert!(err.contains("not ready to merge: behind_base"), "{err}");
    assert!(
        err.contains("hint: pass --update to merge main into it on GitHub"),
        "{err}"
    );
    assert!(writes(&fake).is_empty());
}

#[test]
fn failing_checks_after_an_update_stop_the_landing() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World {
        merge_state: "BEHIND",
        ..World::default()
    };
    world.checks.insert(
        SHA_B,
        vec![
            ("lint", "completed", Some("success")),
            ("unit", "in_progress", None),
        ],
    );
    let (fake, world) = github(world);
    let handle = Arc::clone(&world);
    let flip = thread::spawn(move || {
        // Once the wait has seen `unit` running, it fails.
        thread::sleep(Duration::from_millis(500));
        handle.lock().unwrap().checks.insert(
            SHA_B,
            vec![
                ("lint", "completed", Some("success")),
                ("unit", "completed", Some("failure")),
            ],
        );
    });
    let out = land(
        &fake,
        dir.path(),
        &["--yes", "--update", "--interval", "100ms", "--json"],
    );
    flip.join().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["result"], "ci_failed");
    assert_eq!(doc["updated"], true);
    assert_eq!(doc["head_sha"], &SHA_B[..10]);
    assert_eq!(doc["ci"]["result"], "failure");
    assert_eq!(doc["ci"]["checks"][0]["name"], "unit");
    let err = stderr(&out);
    // The wait saw `unit` running before it failed.
    assert!(err.contains("1 pending"), "{err}");
    assert!(err.contains("ci jdx/demo#7: unit fail"), "{err}");
    // The update was asked for at the head that was checked, and nothing
    // was merged.
    assert_eq!(writes(&fake), [format!("PUT {PR}/update-branch")]);
    let update = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT")
        .unwrap();
    assert_eq!(update.body, json!({ "expected_head_sha": SHA_A }));
}

#[test]
fn an_update_whose_checks_pass_is_merged_at_the_new_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut world = World {
        merge_state: "BEHIND",
        ..World::default()
    };
    world
        .checks
        .insert(SHA_B, vec![("unit", "completed", Some("success"))]);
    let (fake, _) = github(world);
    let out = land(
        &fake,
        dir.path(),
        &["--yes", "--update", "--interval", "100ms", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["result"], "merged");
    assert_eq!(doc["updated"], true);
    assert_eq!(doc["ci"]["result"], "success");
    assert_eq!(
        writes(&fake),
        [format!("PUT {PR}/update-branch"), format!("PUT {PR}/merge")]
    );
    let merge = fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "PUT")
        .unwrap();
    assert_eq!(merge.body["sha"], SHA_B);
}

#[test]
fn branch_protection_rejecting_the_merge_is_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World {
        merge_reply: (405, "Required status check \"e2e\" is expected."),
        ..World::default()
    });
    let out = land(&fake, dir.path(), &["--yes", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["code"], "precondition");
    assert_eq!(
        error["message"],
        "GitHub refused to merge jdx/demo#7: Required status check \"e2e\" is expected."
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("never bypasses branch protection"),
        "{error}"
    );
    // The merge was asked for once, and not confirmed or retried.
    let requests = fake.requests();
    assert_eq!(requests.iter().filter(|r| r.method == "PUT").count(), 1);
    assert!(!requests.iter().any(|r| r.method == "GET" && r.path == PR));
}

#[test]
fn a_merge_whose_response_is_lost_is_confirmed_by_asking() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World {
        merge_reply: (CUT_SHORT, ""),
        ..World::default()
    });
    let out = land(&fake, dir.path(), &["--yes", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["merge_commit"], MERGE_SHA);
    assert!(stderr(&out).contains("checking whether it merged"));
    // A write is never retried.
    assert_eq!(writes(&fake).len(), 1);
}

#[test]
fn a_dry_run_prints_the_plan_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World {
        merge_state: "BEHIND",
        ..World::default()
    });
    let out = land(
        &fake,
        dir.path(),
        &["--dry-run", "--update", "--method", "merge"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "jouet pr land would:\n  \
         1. merge main into feat on GitHub (update-branch, never a force-push)\n  \
         2. wait up to 30m for the checks on the new head\n  \
         3. merge jdx/demo#7 at the new head through the API, honouring branch protection\n  \
         4. confirm jdx/demo#7 is merged\n\
         dry run: nothing was changed\n"
    );
    assert!(writes(&fake).is_empty());
}

#[test]
fn without_yes_and_without_a_person_it_is_a_usage_error_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World::default());
    let out = land(&fake, dir.path(), &["--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "usage");
    assert!(fake.requests().is_empty());
}

#[test]
fn next_exits_by_whose_move_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let next = |world: World| {
        let (fake, _) = github(world);
        let out = jouet(
            &fake,
            dir.path(),
            &["pr", "next", "7", "--repo", "jdx/demo", "--json"],
        );
        let doc = json(&out);
        (
            out.status.code(),
            doc["prs"][0]["action"].as_str().map(str::to_string),
        )
    };
    assert_eq!(
        next(World::default()),
        (Some(0), Some("ready_to_merge".into()))
    );
    assert_eq!(
        next(World {
            merged: true,
            ..World::default()
        }),
        (Some(0), Some("merged".into()))
    );
    assert_eq!(
        next(World {
            merge_state: "BEHIND",
            ..World::default()
        }),
        (Some(10), Some("behind_base".into()))
    );
    let mut running = World::default();
    running
        .checks
        .insert(SHA_A, vec![("unit", "in_progress", None)]);
    assert_eq!(next(running), (Some(20), Some("checks_pending".into())));
}

#[test]
fn next_reports_a_rate_limit_as_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, _) = github(World {
        rate_limited: true,
        ..World::default()
    });
    let out = jouet(
        &fake,
        dir.path(),
        &["pr", "next", "7", "--repo", "jdx/demo"],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

#[test]
fn the_prs_checks_not_a_local_run_decide() {
    // The full suite runs on CI: a failed or missing local full run in the
    // hook ledger does not hold a ready PR back.
    let dir = tempfile::tempdir().unwrap();
    let ledger = dir.path().join("state/jouet/ledger/ledger.jsonl");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let failed = json!({"ts": 1, "harness": "claude", "event": "post-tool-use", "shape": "cargo test",
        "exit_code": 101, "tree": "t1",
        "decisions": [{"rule": "checks", "action": "record:test", "scope": "full"}]});
    std::fs::write(&ledger, format!("{failed}\n")).unwrap();
    let (fake, _) = github(World::default());
    let mut args = LAND.to_vec();
    args.extend_from_slice(&["0", "--yes"]);
    let out = command(&fake, dir.path(), &args)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(writes(&fake), [format!("PUT {PR}/merge")]);
}
