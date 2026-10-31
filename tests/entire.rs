//! Entire in `jouet pr status`, `pr next` and `pr threads`: a local fake
//! GitHub replays jdx/mise#13958 as recorded from github.com (its Entire
//! Gates check failing on checks and on two open findings), and a fake
//! `entire` replays `--json` output recorded for its trail, #424. Nothing
//! here talks to the real API or the real `entire`. test/entire.bats covers
//! changing findings.
#![cfg(unix)]

mod common;

use common::{Fake, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const PR: &str = include_str!("../crates/jouet-github/src/pr/fixtures/entire.json");
const CONTEXTS: &str = include_str!("../crates/jouet-github/src/pr/fixtures/entire_contexts.json");
const THREADS: &str = include_str!("../crates/jouet-github/src/pr/threads/fixtures/pr.json");
const TOKEN: &str = "good-token";

/// The fake `entire`: logs its arguments to `entire.log` beside it and
/// answers from test/fixtures/entire. `FAKE_ENTIRE_AUTH=out` is logged out.
/// PATH holds only its directory, so it names `cat` by its path.
const FAKE_ENTIRE: &str = r#"#!/bin/sh
printf '%s\n' "$*" >>"${0%/*}/entire.log"
fx="$FAKE_ENTIRE_FIXTURES"
case "$*" in
"auth status --json")
	if [ "${FAKE_ENTIRE_AUTH:-}" = out ]; then /bin/cat "$fx/auth-logged-out.json"; else /bin/cat "$fx/auth-ok.json"; fi ;;
"trail show 424 --json --repo gh/jdx/mise"|"trail show --branch "*" --json --repo gh/jdx/mise")
	/bin/cat "$fx/show.json" ;;
"trail finding --trail 424 --json -n 100 --repo gh/jdx/mise")
	/bin/cat "$fx/finding-open.json" ;;
"trail finding --trail 424 --json -n 100 --repo gh/jdx/mise --status any --freshness any --include-dismissed")
	/bin/cat "$fx/finding-any.json" ;;
*)
	echo "fake entire: unexpected call: $*" >&2; exit 64 ;;
esac
"#;

/// How the fake GitHub changes the recording.
#[derive(Clone, Copy, Default)]
struct Edit {
    /// Only Entire Gates fails, ready but for its findings: not a draft,
    /// mergeable, every other check passing.
    findings_only: bool,
}

fn github(edit: Edit) -> Fake {
    serve(move |request, _| respond(edit, request))
}

fn respond(edit: Edit, request: &Request) -> (u16, String) {
    if request.token != TOKEN {
        return (401, r#"{"message":"Bad credentials"}"#.into());
    }
    let q = query(request);
    if q.contains("reviewThreads(first: 100, after:") || q.contains("$extra") {
        return (200, THREADS.into());
    }
    if q.contains("contexts(first: 100") {
        let mut contexts: Value = serde_json::from_str(CONTEXTS).unwrap();
        if edit.findings_only {
            for node in contexts["data"]["l0"]["contexts"]["nodes"]
                .as_array_mut()
                .unwrap()
            {
                if node["conclusion"] == "FAILURE" && node["name"] != "Entire Gates" {
                    node["conclusion"] = json!("SUCCESS");
                }
            }
        }
        return (200, contexts.to_string());
    }
    let mut pr: Value = serde_json::from_str(PR).unwrap();
    if edit.findings_only {
        let node = &mut pr["data"]["l0"]["pullRequest"];
        node["isDraft"] = json!(false);
        node["mergeable"] = json!("MERGEABLE");
        node["mergeStateStatus"] = json!("BLOCKED");
        node["reviewDecision"] = Value::Null;
        let counts = &mut node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["checkRunCountsByState"];
        *counts = json!([
            { "state": "SUCCESS", "count": 32 },
            { "state": "FAILURE", "count": 1 },
            { "state": "SKIPPED", "count": 22 },
        ]);
    }
    (200, pr.to_string())
}

/// Runs jouet with `entire` as the fake, unless `entire` is false.
fn jouet(fake: &Fake, dir: &Path, entire: bool, env: &[(&str, &str)], args: &[&str]) -> Output {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("entire");
    std::fs::write(&program, FAKE_ENTIRE).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", TOKEN)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env(
            "FAKE_ENTIRE_FIXTURES",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/entire"),
        )
        // Only the fake on PATH, or nothing at all.
        .env("PATH", if entire { bin } else { dir.join("empty") });
    for var in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "JOUET_DISPLAY",
        "JOUET_REPLY_FOOTER",
        "JOUET_ENTIRE",
        "JOUET_ENTIRE_TIMEOUT_MS",
        "HTTPS_PROXY",
        "https_proxy",
    ] {
        cmd.env_remove(var);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.output().expect("jouet should start")
}

fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("bin/entire.log")).unwrap_or_default()
}

const STATUS: &[&str] = &["pr", "status", "jdx/mise#13958", "--settle", "0"];

#[test]
fn pr_status_names_the_failing_gates_and_the_open_findings() {
    let fake = github(Edit::default());
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(&fake, dir.path(), true, &[], STATUS);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(
            "\n  entire: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity (trail #424)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("    [entire:01M4C997EA] e2e/tasks/test_task_monorepo_deps_tool_install:44  medium  Identical"),
        "{text}"
    );
    assert!(
        text.contains("    [entire:01M4C997DZ] e2e/cli/test_deps_tool_install:21  medium  "),
        "{text}"
    );
    assert!(
        text.contains("(conflict, checks_failing, entire_findings, draft)"),
        "{text}"
    );
    assert_eq!(
        calls(dir.path()),
        "auth status --json\n\
         trail show 424 --json --repo gh/jdx/mise\n\
         trail finding --trail 424 --json -n 100 --repo gh/jdx/mise\n"
    );
    // The gates come with the PR: GitHub is asked nothing more for them.
    let sent: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(sent, ["POST /graphql", "POST /graphql"]);

    let mut args = STATUS.to_vec();
    args.push("--json");
    let doc = json(&jouet(&fake, dir.path(), true, &[], &args));
    assert_eq!(doc["entire"], json!({ "available": true }));
    let pr = &doc["prs"][0];
    assert_eq!(pr["entire"]["trail"], json!(424));
    assert_eq!(pr["entire"]["state"], json!("failed"));
    assert_eq!(
        pr["entire"]["findings"][1],
        json!({
            "id": "01M4C997DZH5KQRP31NW93NQHR",
            "handle": "entire:01M4C997DZ",
            "severity": "medium",
            "path": "e2e/cli/test_deps_tool_install",
            "line": 21,
            "title": pr["entire"]["findings"][1]["title"],
        })
    );
    let attention: Vec<&Value> = pr["attention"].as_array().unwrap().iter().collect();
    let failing = attention
        .iter()
        .find(|a| a["code"] == "checks_failing")
        .unwrap();
    assert_eq!(
        failing["message"],
        json!("3 failing: trusted / windows-e2e, trusted / test-ci, test-ci"),
        "Entire Gates is not a failing check of its own"
    );
}

#[test]
fn without_entire_the_gates_still_show_and_json_says_why() {
    let fake = github(Edit::default());
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(&fake, dir.path(), false, &[], STATUS);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stderr(&out), "", "nothing on stderr either");
    let text = stdout(&out);
    assert!(text.contains("  entire: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity (trail #424)\n"));
    assert!(!text.contains("[entire:"), "{text}");

    let mut args = STATUS.to_vec();
    args.push("--json");
    let doc = json(&jouet(&fake, dir.path(), false, &[], &args));
    assert_eq!(
        doc["entire"],
        json!({ "available": false, "reason": "entire is not on PATH" })
    );
    let finding = doc["prs"][0]["attention"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["code"] == "entire_findings")
        .unwrap()
        .clone();
    assert_eq!(
        finding["message"],
        json!(
            "Entire Gates: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity"
        )
    );

    let doc = json(&jouet(
        &fake,
        dir.path(),
        true,
        &[("FAKE_ENTIRE_AUTH", "out")],
        &args,
    ));
    assert_eq!(
        doc["entire"],
        json!({ "available": false, "reason": "entire is not logged in (see `entire auth status`)" })
    );
    assert_eq!(calls(dir.path()), "auth status --json\n");
}

#[test]
fn pr_next_names_the_finding_to_fix() {
    let fake = github(Edit {
        findings_only: true,
    });
    let dir = tempfile::tempdir().unwrap();
    let args = ["pr", "next", "jdx/mise#13958", "--settle", "0"];
    let out = jouet(&fake, dir.path(), true, &[], &args);
    assert_eq!(out.status.code(), Some(10), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("#13958 act  entire_findings: Fix Entire finding [entire:01M4C997EA] at e2e/tasks/test_task_monorepo_deps_tool_install:44 (medium): Identical"),
        "{text}"
    );
    assert!(text.contains("; 1 more open\n"), "{text}");
    assert!(
        text.contains("       next: jouet pr threads jdx/mise#13958\n"),
        "{text}"
    );

    let mut args = args.to_vec();
    args.push("--json");
    let doc = json(&jouet(&fake, dir.path(), false, &[], &args));
    assert_eq!(doc["prs"][0]["action"], json!("entire_findings"));
    assert_eq!(
        doc["prs"][0]["reason"],
        json!(
            "Entire Gates: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity"
        )
    );
    assert_eq!(doc["entire"]["available"], json!(false));
}

#[test]
fn pr_threads_lists_findings_apart_from_threads() {
    let fake = github(Edit::default());
    let dir = tempfile::tempdir().unwrap();
    let args = ["pr", "threads", "jdx/mise#14922"];
    let out = jouet(&fake, dir.path(), true, &[], &args);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let (threads, findings) = text
        .split_once("\nEntire trail #424: 2 open findings\n")
        .unwrap();
    assert!(threads.starts_with("jdx/mise#14922  "), "{text}");
    assert!(findings.starts_with(
        "These are Entire's, not GitHub threads: resolving one leaves the other as it was.\n\n\
         [entire:01M4C997EA] e2e/tasks/test_task_monorepo_deps_tool_install:44  medium  agent:trail-review  "
    ));
    assert!(findings.ends_with(
        "\nResolve a finding with `jouet pr threads resolve entire:ID --reply REASON`, or add --dismiss to dismiss it, which needs a reason.\n"
    ));
    assert!(
        calls(dir.path())
            .contains("trail show --branch feat/hook-env-json --json --repo gh/jdx/mise\n")
    );

    // --all lists resolved and dismissed (stale) ones too; --path narrows.
    let doc = json(&jouet(
        &fake,
        dir.path(),
        true,
        &[],
        &[
            "pr",
            "threads",
            "jdx/mise#14922",
            "--all",
            "--path",
            "src/install_layout/",
            "--json",
        ],
    ));
    let findings = doc["entire"]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 9);
    assert!(findings.iter().all(|f| f["stale"] == json!(true)));
    assert_eq!(
        findings
            .iter()
            .filter(|f| f["status"] == "dismissed")
            .count(),
        3
    );
    assert!(
        doc["threads"].is_array(),
        "GitHub threads stay under threads"
    );

    let doc = json(&jouet(
        &fake,
        dir.path(),
        false,
        &[],
        &["pr", "threads", "jdx/mise#14922", "--json"],
    ));
    assert_eq!(
        doc["entire"],
        json!({ "available": false, "reason": "entire is not on PATH" })
    );
}
