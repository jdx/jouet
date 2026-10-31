//! `jouet pr threads` against a local fake GitHub, replaying responses
//! recorded from jdx/mise#14922. Writes are answered by the fake; nothing
//! here talks to the real API.

mod common;

use common::{CUT_SHORT, Fake, RATE_LIMITED, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output, Stdio};

const PR: &str = include_str!("../crates/jouet-github/src/pr/threads/fixtures/pr.json");
const COMMITS: &str = include_str!("../crates/jouet-github/src/pr/threads/fixtures/commits.json");
const ACTIVITY: &str = include_str!("../crates/jouet-github/src/pr/threads/fixtures/activity.json");

const HEAD: &str = "c42414f907109526b406d2b72bef0bfe5cf07209";
const BASE: &str = "118797e7b63bc6579693dfc04fa7113a80d4ae02";
/// The merge base the fake's compare of the PR's base and head names.
const MERGE_BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const OPEN_CODE: &str = "4385981140";
const OPEN_DOCS: &str = "4386056098";
const RESOLVED: &str = "4361752342";
const TOKEN: &str = "good-token";

/// How the fake answers what the recording does not cover.
#[derive(Clone, Default)]
struct Script {
    /// Replaces the PR's head SHA.
    head: Option<String>,
    /// The `status` of a compare, or a 404 when `None`.
    compare: Option<&'static str>,
    /// The index of a request to answer with a 502.
    fail_at: Option<usize>,
    /// The index of a request whose response is cut off mid-body.
    cut_at: Option<usize>,
    /// Answer the activity log with an exhausted rate limit.
    activity_rate_limited: bool,
    /// A thread to put on the base side of the diff.
    left: Option<&'static str>,
}

fn github(script: Script) -> Fake {
    serve(move |request, index| respond(&script, request, index))
}

fn respond(script: &Script, request: &Request, index: usize) -> (u16, String) {
    if request.token != TOKEN {
        return (401, r#"{"message":"Bad credentials"}"#.into());
    }
    if script.fail_at == Some(index) {
        return (502, r#"{"message":"Bad gateway"}"#.into());
    }
    if script.cut_at == Some(index) {
        return (CUT_SHORT, "{".into());
    }
    if request.method == "GET" {
        if request.path.contains("/activity?") {
            if script.activity_rate_limited {
                return (
                    RATE_LIMITED,
                    r#"{"message":"API rate limit exceeded"}"#.into(),
                );
            }
            return (200, ACTIVITY.into());
        }
        if request.path.contains(&format!("/compare/{BASE}...{HEAD}")) {
            return (
                200,
                json!({ "merge_base_commit": { "sha": MERGE_BASE } }).to_string(),
            );
        }
        if request.path.contains("/compare/") {
            return match script.compare {
                Some(status) => (200, json!({ "status": status }).to_string()),
                None => (404, r#"{"message":"Not Found"}"#.into()),
            };
        }
        return (404, r#"{"message":"Not Found"}"#.into());
    }
    let q = query(request);
    let vars = &request.body["variables"];
    let body = if q.contains("addPullRequestReviewThreadReply") {
        let n = 9_000_000_000u64 + index as u64;
        json!({ "data": { "addPullRequestReviewThreadReply": { "comment": {
            "url": format!("https://github.com/jdx/mise/pull/14922#discussion_r{n}")
        } } } })
    } else if q.contains("unresolveReviewThread") {
        json!({ "data": { "unresolveReviewThread": { "thread": { "isResolved": false } } } })
    } else if q.contains("resolveReviewThread") {
        json!({ "data": { "resolveReviewThread": { "thread": { "isResolved": true } } } })
    } else if q.contains("object(expression:") {
        // Every file read for --context is 600 numbered lines, marked
        // `base` at the merge base.
        let files: serde_json::Map<String, Value> = (0..)
            .map_while(|i| {
                vars.get(format!("e{i}"))
                    .and_then(Value::as_str)
                    .map(|e| (i, e))
            })
            .map(|(i, expression)| {
                let mark = if expression.starts_with(MERGE_BASE) {
                    "base "
                } else {
                    ""
                };
                let text: String = (1..=600).map(|n| format!("{mark}line {n}\n")).collect();
                (format!("f{i}"), json!({ "text": text }))
            })
            .collect();
        json!({ "data": { "repository": files } })
    } else if q.contains("commits(first: 100") {
        serde_json::from_str(COMMITS).unwrap()
    } else if vars["n"] != json!(14922) {
        json!({ "data": { "viewer": { "login": "jdx" }, "repository": { "pullRequest": null } } })
    } else {
        let mut pr: Value = serde_json::from_str(PR).unwrap();
        let node = &mut pr["data"]["repository"]["pullRequest"];
        // GitHub leaves out what `@include(if: false)` skips.
        if vars["extra"] == json!(false) {
            let node = node.as_object_mut().unwrap();
            node.remove("reviews");
            node.remove("comments");
        }
        if let Some(head) = &script.head {
            node["headRefOid"] = head.as_str().into();
        }
        if let Some(id) = script.left {
            for thread in node["reviewThreads"]["nodes"].as_array_mut().unwrap() {
                if thread["comments"]["nodes"][0]["fullDatabaseId"] == json!(id) {
                    thread["diffSide"] = "LEFT".into();
                }
            }
        }
        pr
    };
    (200, body.to_string())
}

/// Runs jouet in `dir` with one token, no config and a clean git setup.
fn jouet(fake: &Fake, dir: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    jouet_with(fake, dir, args, stdin, &[])
}

fn jouet_with(
    fake: &Fake,
    dir: &Path,
    args: &[&str],
    stdin: Option<&str>,
    env: &[(&str, &str)],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", TOKEN)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Never the developer's `entire`; a test that wants one names it.
        .env("JOUET_ENTIRE", "false")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "JOUET_DISPLAY",
        "JOUET_REPLY_FOOTER",
        "JOUET_BOTS",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    let mut child = cmd.spawn().expect("jouet should start");
    if let Some(text) = stdin {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

/// The mutations the fake received, as `name thread-id`.
fn writes(fake: &Fake) -> Vec<String> {
    fake.requests()
        .iter()
        .filter(|r| query(r).starts_with("mutation"))
        .map(|r| {
            let q = query(r);
            let name = [
                "addPullRequestReviewThreadReply",
                "unresolveReviewThread",
                "resolveReviewThread",
            ]
            .into_iter()
            .find(|n| q.contains(n))
            .unwrap();
            format!("{name} {}", r.body["variables"]["id"].as_str().unwrap())
        })
        .collect()
}

const CODE_NODE: &str = "PRRT_kwDOIvux3s6wlNzt";
const RESOLVED_NODE: &str = "PRRT_kwDOIvux3s6vruPA";

fn node_of(fake_pr: &str, id: &str) -> String {
    let pr: Value = serde_json::from_str(fake_pr).unwrap();
    pr["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["comments"]["nodes"][0]["fullDatabaseId"] == json!(id))
        .map(|t| t["id"].as_str().unwrap().to_string())
        .unwrap()
}

#[test]
fn listing_shows_unresolved_threads_as_minified_json() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[
            "pr",
            "threads",
            "jdx/mise#14922",
            "--json",
            "--display",
            "plain",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 1, "one minified document");
    let doc = json(&out);
    assert_eq!(doc["repo"], json!("jdx/mise"));
    assert_eq!(
        doc["counts"],
        json!({ "unresolved": 2, "resolved": 3, "outdated": 0 })
    );
    let threads = doc["threads"].as_array().unwrap();
    let ids: Vec<String> = threads.iter().map(|t| t["id"].to_string()).collect();
    assert_eq!(ids, [OPEN_CODE, OPEN_DOCS]);
    let t = &threads[0];
    assert_eq!(t["kind"], json!("thread"));
    assert_eq!(t["author"], json!("coderabbitai[bot]"));
    assert_eq!(t["is_bot"], json!(true));
    assert_eq!(t["node_id"], json!(CODE_NODE));
    // False and empty fields are left out.
    assert!(t.get("resolved").is_none() && t.get("outdated").is_none());
    // The body is condensed: CodeRabbit's state comments and prompt are gone.
    let body = t["comments"][0]["body"].as_str().unwrap();
    assert!(
        !body.contains("<!--") && body.contains("[details: Prompt for AI Agents]"),
        "{body}"
    );
    assert_eq!(t["comments"][0]["body_truncated"], json!(true));
    // Reviews were not asked for, so the query skipped them.
    assert_eq!(fake.requests()[0].body["variables"]["extra"], json!(false));
}

#[test]
fn text_listing_has_a_header_and_a_block_per_thread() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "pr",
        "threads",
        "14922",
        "--repo",
        "jdx/mise",
        "--all",
        "--display",
        "plain",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("jdx/mise#14922  2 unresolved of 5 threads  showing 5")
    );
    let heads: Vec<&str> = text.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(heads.len(), 5, "{text}");
    assert!(
        heads[0].starts_with("[4361752331] src/cli/hook_env.rs:275-279  greptile-apps[bot]  "),
        "{text}"
    );
    assert!(heads[0].ends_with("  resolved"), "{text}");
    assert!(heads[1].ends_with("  outdated  resolved"), "{text}");
    // greptile's priority badge reads as text.
    assert!(
        text.contains("  [P1] **Inherited tool paths disappear**"),
        "{text}"
    );
    // With unresolved threads shown, the listing ends with how to reply.
    assert!(
        text.trim_end().ends_with(
            "Reply with `jouet pr threads reply <ID> [PR] --body TEXT`, ID being the number in brackets; add --resolve to resolve it too, or `jouet pr threads apply` takes many at once."
        ),
        "{text}"
    );
    let resolved = [
        "pr",
        "threads",
        "14922",
        "--repo",
        "jdx/mise",
        "--resolved",
        "--display",
        "plain",
    ];
    let out = jouet(&fake, dir.path(), &resolved, None);
    assert!(!stdout(&out).contains("Reply with"), "{}", stdout(&out));
}

#[test]
fn reviews_and_comments_join_on_request() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "pr",
        "threads",
        "jdx/mise#14922",
        "--include-reviews",
        "--author",
        "github-actions",
        "--json",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    let threads = doc["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 1, "{doc}");
    assert_eq!(threads[0]["kind"], json!("comment"));
    assert_eq!(threads[0]["author"], json!("github-actions[bot]"));
    assert_eq!(fake.requests()[0].body["variables"]["extra"], json!(true));
    // Without --include-reviews they are not listed.
    let args = [
        "pr",
        "threads",
        "jdx/mise#14922",
        "--author",
        "github-actions",
        "--json",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(json(&out)["threads"], json!([]));
}

#[test]
fn since_a_sha_means_since_its_push() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "pr",
        "threads",
        "jdx/mise#14922",
        "--since",
        "@c42414f9",
        "--json",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    // Pushed at 07:18:55; CodeRabbit's first thread came before that, on the
    // same commit, and is left out.
    assert_eq!(doc["since"], json!("2026-10-27T07:18:55Z"));
    let ids: Vec<String> = doc["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].to_string())
        .collect();
    assert_eq!(ids, [OPEN_DOCS]);
    let paths: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        paths,
        [
            "POST /graphql",
            "POST /graphql",
            "GET /repos/refractalize/mise/activity?ref=refs/heads/feat/hook-env-json&per_page=100"
        ]
    );

    let out = jouet(
        &fake,
        dir.path(),
        &["pr", "threads", "jdx/mise#14922", "--since", "@beef"],
        None,
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let out = jouet(
        &fake,
        dir.path(),
        &["pr", "threads", "jdx/mise#14922", "--since", "@"],
        None,
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn listing_exit_codes() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        jouet(
            &fake,
            dir.path(),
            &[&["pr", "threads"], args].concat(),
            None,
        )
    };
    assert_eq!(
        run(&["jdx/mise#14922", "--fail-if-unresolved"])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run(&["jdx/mise#14922", "--resolved", "--fail-if-unresolved"])
            .status
            .code(),
        Some(0)
    );
    let out = run(&["jdx/mise#1", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["error"]["code"], json!("not_found"));
    assert_eq!(
        run(&["jdx/mise#14922", "--since", "yesterday"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["jdx/mise#14922", "--all", "--resolved"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn apply_validates_then_posts_in_order() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let list = format!(
        r#"[{{"id": {OPEN_CODE}, "reply": "Bounded by the timeout now.", "resolve": true}},
            {{"id": "{OPEN_DOCS}", "resolve": true}}]"#
    );
    let args = ["pr", "threads", "apply", "-", "jdx/mise#14922", "--json"];
    let footer = [("JOUET_REPLY_FOOTER", "*AI-assisted*")];
    let out = jouet_with(&fake, dir.path(), &args, Some(&list), &footer);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    let results = doc["results"].as_array().unwrap();
    assert_eq!(results[0]["replied"], json!(true));
    assert_eq!(results[0]["resolved"], json!(true));
    assert!(
        results[0]["url"]
            .as_str()
            .unwrap()
            .contains("#discussion_r")
    );
    assert_eq!(
        results[1],
        json!({ "id": OPEN_DOCS.parse::<u64>().unwrap(), "resolved": true })
    );
    let docs_node = node_of(PR, OPEN_DOCS);
    assert_eq!(
        writes(&fake),
        [
            format!("addPullRequestReviewThreadReply {CODE_NODE}"),
            format!("resolveReviewThread {CODE_NODE}"),
            format!("resolveReviewThread {docs_node}"),
        ]
    );
    let reply = fake
        .requests()
        .into_iter()
        .find(|r| query(r).contains("addPullRequestReviewThreadReply"))
        .unwrap();
    assert_eq!(
        reply.body["variables"]["body"],
        json!("Bounded by the timeout now.\n\n*AI-assisted*")
    );
}

#[test]
fn the_config_footer_and_a_dry_run() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config/jouet")).unwrap();
    std::fs::write(
        dir.path().join("config/jouet/config.toml"),
        "reply_footer = \"-- an agent\"\n",
    )
    .unwrap();
    let file = dir.path().join("list.json");
    std::fs::write(
        &file,
        format!(r#"[{{"id": {OPEN_CODE}, "reply": "Done."}}]"#),
    )
    .unwrap();
    let args = [
        "pr",
        "threads",
        "apply",
        "--file",
        file.to_str().unwrap(),
        "--dry-run",
        "jdx/mise#14922",
        "--display",
        "plain",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("[{OPEN_CODE}] would reply\n  | Done.\n  |\n  | -- an agent\n")
    );
    assert!(writes(&fake).is_empty());
}

#[test]
fn bad_ids_post_nothing_and_resolved_threads_still_take_replies() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        jouet(
            &fake,
            dir.path(),
            &[&["pr", "threads"], args, &["--repo", "jdx/mise", "--json"]].concat(),
            None,
        )
    };
    let out = run(&["resolve", OPEN_CODE, "1", "#14922"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(json(&out)["error"]["code"], json!("not_found"));
    assert!(writes(&fake).is_empty(), "{:?}", writes(&fake));

    // Resolving a thread that is already resolved does nothing.
    let out = run(&["resolve", OPEN_CODE, RESOLVED, "#14922"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        json(&out)["results"][1]["skipped"],
        json!("already resolved")
    );
    assert_eq!(writes(&fake), [format!("resolveReviewThread {CODE_NODE}")]);

    // A reply goes into a resolved thread, which stays resolved: a review
    // bot often resolves its own thread after a push.
    let out = run(&["reply", RESOLVED, "#14922", "--body", "fixed", "--resolve"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["results"][0]["already_resolved"], json!(true));
    assert_eq!(
        writes(&fake)[1..],
        [format!("addPullRequestReviewThreadReply {RESOLVED_NODE}")]
    );

    // --ignore-resolved leaves it alone.
    let out = run(&[
        "reply",
        RESOLVED,
        "#14922",
        "--body",
        "again",
        "--ignore-resolved",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        json(&out)["results"][0]["skipped"],
        json!("already resolved")
    );
    assert_eq!(writes(&fake).len(), 2, "{:?}", writes(&fake));
}

#[test]
fn empty_replies_and_bad_lists_are_usage_errors() {
    let fake = github(Script::default());
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str], stdin| {
        jouet(
            &fake,
            dir.path(),
            &[&["pr", "threads"], args].concat(),
            stdin,
        )
    };
    assert_eq!(
        run(&["reply", OPEN_CODE, "jdx/mise#14922", "--body", " "], None)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(
            &["reply", OPEN_CODE, "jdx/mise#14922", "--body-file", "-"],
            Some("\n")
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        run(&["reply", OPEN_CODE, "jdx/mise#14922"], None)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["apply", "-", "jdx/mise#14922"], Some("[]"))
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["apply", "jdx/mise#14922"], None).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&["resolve", "jdx/mise#14922"], None).status.code(),
        Some(2)
    );
    assert!(fake.requests().is_empty());
}

/// A git repository with one commit, and that commit's SHA.
fn repo(dir: &Path) -> String {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "work"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "x",
    ]);
    git(&["rev-parse", "HEAD"])
}

#[test]
fn sha_is_refused_until_head_is_pushed() {
    let dir = tempfile::tempdir().unwrap();
    let head = repo(dir.path());
    let args = [
        "pr",
        "threads",
        "reply",
        OPEN_CODE,
        "jdx/mise#14922",
        "--body",
        "Fixed in {sha}.",
        "--json",
    ];

    // The PR's head is not in this clone and GitHub has never seen HEAD.
    let fake = github(Script::default());
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let message = json(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(
        message.contains("is not on refractalize/mise:feat/hook-env-json"),
        "{message}"
    );
    assert!(writes(&fake).is_empty());
    let compare = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "GET")
        .unwrap();
    assert_eq!(
        compare.path,
        format!("/repos/refractalize/mise/compare/{head}...{HEAD}")
    );

    // GitHub says the PR's head contains HEAD.
    let fake = github(Script {
        compare: Some("ahead"),
        ..Script::default()
    });
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    // HEAD is the PR's head: no need to ask.
    let fake = github(Script {
        head: Some(head.clone()),
        ..Script::default()
    });
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(fake.requests().iter().all(|r| r.method == "POST"));
    let reply = fake
        .requests()
        .into_iter()
        .find(|r| query(r).contains("addPullRequestReviewThreadReply"))
        .unwrap();
    assert_eq!(
        reply.body["variables"]["body"],
        json!(format!("Fixed in {}.", &head[..7]))
    );
}

#[test]
fn a_failed_write_stops_the_batch_and_is_not_retried() {
    // Requests: the PR, then reply (1), resolve (2), resolve (3).
    let fake = github(Script {
        fail_at: Some(2),
        ..Script::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let list = format!(
        r#"[{{"id": {OPEN_CODE}, "reply": "Done.", "resolve": true}}, {{"id": {OPEN_DOCS}, "resolve": true}}]"#
    );
    let args = [
        "pr",
        "threads",
        "apply",
        "-",
        "jdx/mise#14922",
        "--display",
        "plain",
    ];
    let out = jouet(&fake, dir.path(), &args, Some(&list));
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with(&format!(
            "[{OPEN_CODE}] replied, failed: GitHub returned 502"
        )),
        "{text}"
    );
    assert_eq!(
        lines[1],
        format!("[{OPEN_DOCS}] failed: not attempted after an earlier failure")
    );
    assert!(lines[2].starts_with("stopped at a failure"), "{text}");
    assert_eq!(
        fake.requests().len(),
        3,
        "the failed mutation was not retried"
    );
}

#[test]
fn a_reply_cut_off_after_sending_is_not_sent_again() {
    // The reply may well have been posted; only a re-run, which checks the
    // thread's last comment, can tell.
    let fake = github(Script {
        cut_at: Some(1),
        ..Script::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "pr",
        "threads",
        "reply",
        OPEN_CODE,
        "jdx/mise#14922",
        "--body",
        "Done.",
        "--json",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert!(json(&out)["results"][0]["failed"].is_string());
    assert_eq!(writes(&fake).len(), 1);
}

#[test]
fn a_failed_request_keeps_the_token_for_the_next_one() {
    // The push log is rate limited; the --context query after it must reuse
    // the token rather than look for another, which on a runner without `gh`
    // finds none.
    let fake = github(Script {
        activity_rate_limited: true,
        ..Script::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    let args = [
        "pr",
        "threads",
        "jdx/mise#14922",
        "--since",
        "@c42414f9",
        "--context",
        "1",
        "--json",
    ];
    let out = jouet_with(
        &fake,
        dir.path(),
        &args,
        None,
        &[("PATH", empty.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("could not read the push log"), "{err}");
    assert!(!err.contains("token"), "{err}");
    let doc = json(&out);
    let t = &doc["threads"][0];
    assert_eq!(t["id"].to_string(), OPEN_CODE);
    assert_eq!(
        t["context"],
        json!({ "start": 514, "lines": ["line 514", "line 515", "line 516"] })
    );
}

#[test]
fn context_for_a_base_side_thread_comes_from_the_merge_base() {
    let fake = github(Script {
        left: Some(OPEN_CODE),
        ..Script::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "pr",
        "threads",
        "jdx/mise#14922",
        "--context",
        "1",
        "--json",
    ];
    let out = jouet(&fake, dir.path(), &args, None);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    let (left, right) = (&doc["threads"][0], &doc["threads"][1]);
    assert_eq!(left["side"], json!("left"));
    assert_eq!(
        left["context"],
        json!({ "start": 514, "lines": ["base line 514", "base line 515", "base line 516"] })
    );
    assert!(right.get("side").is_none());
    assert_eq!(
        right["context"],
        json!({ "start": 138, "lines": ["line 138", "line 139", "line 140"] })
    );
    let gets: Vec<String> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET")
        .map(|r| r.path)
        .collect();
    assert_eq!(
        gets,
        [format!(
            "/repos/jdx/mise/compare/{BASE}...{HEAD}?per_page=1"
        )]
    );

    let args = ["pr", "threads", "jdx/mise#14922", "--display", "plain"];
    let text = stdout(&jouet(&fake, dir.path(), &args, None));
    assert!(
        text.lines().any(|l| l.starts_with(&format!(
            "[{OPEN_CODE}] src/hook_env.rs:515  coderabbitai[bot]  "
        )) && l.ends_with("  base side")),
        "{text}"
    );
}
