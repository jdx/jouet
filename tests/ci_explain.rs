//! `jouet ci explain` against a local fake GitHub that replays responses
//! recorded from jdx/jouet on 2026-10-31 (trimmed to the fields jouet reads,
//! and each log to its failed step). Nothing here talks to the real API.
//!
//! - Run 41193844808: `trusted / test-macos` failed in
//!   `a_command_is_rerun_until_it_exits_as_expected` (tests/wait.rs:436, a
//!   timing assertion), which `trusted / test` passed on Linux on the same
//!   commit. Flaky.
//! - Run 41193332714 (PR #73): every build failed with E0425, the error
//!   #71 and #74 left on main until #79, which main's ci and docs runs
//!   failed with too. Real, and not the PR's fault.

mod common;

use common::{Fake, json, query, serve, stderr, stdout};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const FLAKY_RUN: &str = "41193844808";
const REAL_RUN: &str = "41193332714";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ci-explain")
}

/// Undoes the percent-escapes a client may send.
fn decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = path
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The fixture a request is answered from, named as jouet's cache names it.
fn fixture_name(path: &str) -> String {
    let path = decode(path);
    let rest = path.strip_prefix("/repos/jdx/jouet").unwrap_or(&path);
    if let Some(id) = rest
        .strip_prefix("/actions/jobs/")
        .and_then(|r| r.strip_suffix("/logs"))
    {
        return format!("job-{id}.log");
    }
    let rest = rest.trim_start_matches('/');
    let rest = if rest.is_empty() { "repository" } else { rest };
    let name: String = rest
        .chars()
        .map(|c| match c {
            '/' => '.',
            '?' | '&' => '@',
            c if c.is_ascii_alphanumeric() || "-_.=,".contains(c) => c,
            _ => '_',
        })
        .collect();
    format!("{name}.json")
}

/// Serves the recorded responses; reruns are accepted, anything else is a
/// 404.
fn github() -> Fake {
    serve(|req, _| {
        if req.method == "POST" && req.path.contains("/rerun") {
            return (201, "{}".into());
        }
        match std::fs::read_to_string(fixtures().join(fixture_name(&req.path))) {
            Ok(body) => (200, body),
            Err(_) => (404, r#"{"message":"Not Found"}"#.into()),
        }
    })
}

fn jouet(fake: &Fake, dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", "test-token")
        .env("HOME", dir)
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
    cmd.output().expect("jouet should start")
}

fn explain(fake: &Fake, dir: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["--json", "ci", "explain", "--repo", "jdx/jouet"];
    args.extend(extra);
    jouet(fake, dir, &args)
}

fn jobs(doc: &Value) -> &Vec<Value> {
    doc["runs"][0]["jobs"].as_array().unwrap()
}

#[test]
fn a_macos_only_timing_failure_is_flaky_and_names_the_oses() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["--run", FLAKY_RUN]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["result"], "flaky");
    let [job] = jobs(&doc).as_slice() else {
        panic!("one failed job: {doc}");
    };
    assert_eq!(job["name"], "trusted / test-macos");
    assert_eq!(job["os"], "macos");
    assert_eq!(job["verdict"], "flaky");
    assert_eq!(job["failed_step"], "Run mise run test");
    assert_eq!(
        job["tests"][0],
        "a_command_is_rerun_until_it_exits_as_expected"
    );
    assert_eq!(job["location"], "tests/wait.rs:436:5");
    let evidence: Vec<&str> = job["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        evidence,
        [
            "passed on linux on this commit (trusted / test); failed on macos",
            "a failure on one OS can be a real macos bug: if a rerun fails the same way on this commit, it is real",
        ]
    );
    let excerpt = job["log_excerpt"].to_string();
    assert!(
        excerpt.contains("panicked at tests/wait.rs:436:5"),
        "{excerpt}"
    );
    // The release builds fail-fast cancelled are not failures of their own.
    let following = doc["runs"][0]["following"].as_array().unwrap();
    assert_eq!(following.len(), 9);
    assert!(
        following
            .iter()
            .all(|f| f["conclusion"] == "cancelled" && f["follows"] == "trusted / test-macos")
    );
    assert_eq!(doc["rerun"]["allowed"], true);
    assert_eq!(
        doc["rerun"]["command"],
        "jouet ci explain --run 41193844808 --repo jdx/jouet --rerun"
    );
    assert!(
        !fake.requests().iter().any(|r| r.method == "POST"),
        "nothing is rerun without --rerun"
    );
}

#[test]
fn a_second_explain_is_answered_from_the_cache() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let first = explain(&fake, dir.path(), &["--run", FLAKY_RUN]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let sent = fake.requests().len();
    assert!(sent > 0);
    let again = explain(&fake, dir.path(), &["--run", FLAKY_RUN]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert_eq!(
        fake.requests().len(),
        sent,
        "{:?}",
        &fake.requests()[sent..]
    );
    let doc = json(&again);
    assert_eq!(doc["github"]["requests"], 0);
    assert_eq!(doc["result"], "flaky");
}

#[test]
fn rerun_reruns_the_failed_jobs_of_a_flaky_run() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["--run", FLAKY_RUN, "--rerun"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["rerun"]["started"], true);
    let posts: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST")
        .collect();
    let [post] = posts.as_slice() else {
        panic!("one rerun: {posts:?}");
    };
    assert_eq!(
        post.path,
        "/repos/jdx/jouet/actions/runs/41193844808/rerun-failed-jobs"
    );
    // A REST write with the token, and the empty body GitHub expects.
    assert_eq!(post.token, "test-token");
    assert_eq!(post.body, serde_json::json!({}));
    assert_eq!(query(post), "");
}

#[test]
fn one_job_is_explained_and_rerun_alone() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["--job", "124515918002", "--rerun"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(jobs(&doc).len(), 1);
    assert_eq!(jobs(&doc)[0]["verdict"], "flaky");
    let posts: Vec<String> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST")
        .map(|r| r.path)
        .collect();
    assert_eq!(posts, ["/repos/jdx/jouet/actions/jobs/124515918002/rerun"]);
}

#[test]
fn a_compile_error_main_also_has_is_real_and_not_the_prs_fault() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["--run", REAL_RUN]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["result"], "real");
    assert_eq!(doc["runs"][0]["prs"][0], 73);
    let jobs = jobs(&doc);
    assert_eq!(jobs.len(), 11, "every build failed: {doc}");
    for job in jobs {
        assert_eq!(job["verdict"], "real", "{job}");
        assert_eq!(
            job["errors"][0],
            "error[E0425]: cannot find function `lowercase_first` in this scope"
        );
        assert_eq!(job["main_broken"], true);
    }
    let evidence = jobs[0]["evidence"].to_string();
    assert!(
        evidence.contains("main is broken, not this change: the same error failed ci at b768e8fd (run 41193938886), docs at b768e8fd (run 41193938852)"),
        "{evidence}"
    );
    assert!(
        evidence.contains("main has passed since (ci at eadbb37d, run 41196232058): merge main in"),
        "{evidence}"
    );
    // `final` and `trusted / final` only report the others' result.
    let following: Vec<&str> = doc["runs"][0]["following"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(following, ["trusted / final", "final"]);
    assert_eq!(doc["rerun"]["allowed"], false);
}

#[test]
fn a_pr_is_explained_by_the_latest_failed_runs_on_its_head() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["73"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["pr"], 73);
    assert_eq!(doc["result"], "real");
    assert_eq!(doc["runs"][0]["run_id"], 41_193_332_714_u64);
    assert_eq!(doc["rerun"]["allowed"], false);
}

#[test]
fn rerun_refuses_a_real_failure_before_any_write() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["--run", REAL_RUN, "--rerun"]);
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["error"]["code"], "precondition");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("not rerunning: a rerun repeats a real failure: trusted / test (real)"),
        "{message}"
    );
    assert!(!fake.requests().iter().any(|r| r.method == "POST"));
}

#[test]
fn a_missing_run_is_not_found_and_targets_are_checked() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = explain(&fake, dir.path(), &["--run", "1"]);
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
    let out = explain(&fake, dir.path(), &["--run", "1", "--job", "2"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(fake.requests().len() <= 2);
}

#[test]
fn the_text_report_groups_jobs_that_failed_the_same_way() {
    let fake = github();
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[
            "ci",
            "explain",
            "--repo",
            "jdx/jouet",
            "--run",
            REAL_RUN,
            "--display",
            "plain",
            "--log-lines",
            "6",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "REAL jdx/jouet  11 real, 2 following");
    assert_eq!(
        lines[1],
        "  run 41193332714 ci @ 95858185 on claude/graph-repo-map, attempt 1, PR #73"
    );
    assert!(
        text.contains("\nREAL trusted / test, trusted / release-build (x86_64-unknown-linux-musl, ubuntu-latest), trusted / msrv and 8 more  step \"Run mise x hk -- hk check --all\"\n"),
        "{text}"
    );
    assert!(text.contains("\nfailed after trusted / test failed, with nothing of their own: trusted / final and final\n"), "{text}");
    assert!(
        text.contains("\nrerun: refused, a rerun repeats a real failure: "),
        "{text}"
    );
    // The excerpt keeps to its budget, and says what it left out.
    assert!(text.contains("lines skipped"), "{text}");
}
