//! `jouet pr status` against a local fake GitHub, replaying GraphQL responses
//! recorded from github.com. Nothing here talks to the real API.

mod common;

use common::{CUT_SHORT, Fake, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

const BATCH: &str = include_str!("../src/pr/fixtures/batch.json");
const FAILING_CHECKS: &str = include_str!("../src/pr/fixtures/failing_checks.json");
const SEARCH: &str = include_str!("../src/pr/fixtures/search.json");

/// Runs jouet with only the given tokens and a PATH holding `path_dir`, so
/// `gh` is found only when a test puts one there.
fn jouet(fake: &Fake, path_dir: &Path, tokens: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(path_dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("PATH", path_dir);
    for var in [
        "JOUET_GITHUB_TOKEN",
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
    for (name, value) in tokens {
        cmd.env(name, value);
    }
    cmd.output().expect("jouet should start")
}

const GOOD: &str = "good-token";

/// The recorded responses: a batch of PRs, the failing checks of one, and a
/// search. Any other token gets a 401.
fn github(request: &Request, _: usize) -> (u16, String) {
    if request.token != GOOD {
        return (401, r#"{"message":"Bad credentials"}"#.into());
    }
    let q = query(request);
    let body = if q.contains("search(") {
        SEARCH
    } else if q.contains("contexts(first: 100") {
        FAILING_CHECKS
    } else {
        BATCH
    };
    (200, body.into())
}

/// The arguments that produce the recorded batch's lookups.
const BATCH_ARGS: &[&str] = &[
    "pr",
    "status",
    "jdx/mise#14955",
    "https://github.com/jdx/mise/pull/15147",
    "jdx/mise#15187",
    "99999999",
    "jdx/jouet#3",
    "claude/no-such-branch",
    "--repo",
    "jdx/mise",
    "--settle",
    "0",
];

#[test]
fn prs_are_fetched_in_one_query_and_missing_ones_exit_3() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let args = [BATCH_ARGS, &["--json"]].concat();
    let out = jouet(&fake, dir.path(), &[("JOUET_GITHUB_TOKEN", GOOD)], &args);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let doc = json(&out);
    let numbers: Vec<u64> = doc["prs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [14955, 15147, 15187, 3]);
    assert_eq!(
        doc["not_found"],
        json!(["jdx/mise#99999999", "jdx/mise:claude/no-such-branch"])
    );
    assert_eq!(
        doc["prs"][0]["checks"]["failing"],
        json!(["untrusted / lint", "untrusted / test-ci", "test-ci"])
    );
    let codes: Vec<&str> = doc["prs"][0]["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["conflict", "checks_failing", "draft"]);

    // Six PRs in one query, then one for the failing check names.
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.method == "POST" && r.path == "/graphql")
    );
    let vars = &requests[0].body["variables"];
    assert_eq!(vars["n1"], json!(15147));
    assert_eq!(vars["o4"], json!("jdx"));
    assert_eq!(vars["r4"], json!("jouet"));
    assert_eq!(vars["b5"], json!("claude/no-such-branch"));
}

#[test]
fn text_output_has_a_line_per_pr_and_a_summary() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let args = [BATCH_ARGS, &["--display", "plain"]].concat();
    let out = jouet(&fake, dir.path(), &[("JOUET_GITHUB_TOKEN", GOOD)], &args);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "{text}");
    assert!(
        lines[0].starts_with("jdx/mise#14955 draft  fix(hook-env): install the error...  "),
        "{text}"
    );
    assert!(
        lines[0]
            .contains("CONFLICT  ci 25/51 fail: untrusted / lint, untrusted / test-ci, test-ci"),
        "{text}"
    );
    assert!(lines[2].contains(" merged "), "{text}");
    assert_eq!(
        lines[4],
        "not found: jdx/mise#99999999, jdx/mise:claude/no-such-branch"
    );
    assert_eq!(
        lines[5],
        "4 PRs, 3 need attention: jdx/mise#14955 (conflict, checks_failing, draft), \
         jdx/mise#15147 (conflict, unresolved_threads, draft), jdx/jouet#3 (conflict); 2 not found"
    );
}

#[test]
fn search_results_exit_0_unless_attention_fails_the_run() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let tokens = [("JOUET_GITHUB_TOKEN", GOOD)];
    let args = [
        "pr",
        "status",
        "--search",
        "is:open",
        "--repo",
        "jdx/jouet",
        "--settle",
        "0",
    ];
    let out = jouet(
        &fake,
        dir.path(),
        &tokens,
        &[&args[..], &["--display", "plain"]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).lines().last(),
        Some("4 PRs, 2 need attention: #5 (unresolved_threads), #3 (unresolved_threads)")
    );
    assert!(
        stdout(&out).starts_with(
            "#7 open feat(wait): add jouet wait for files,...  ok  ci 3/10 pending  threads 0  review -\n"
        ),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        fake.requests()[0].body["variables"]["q0"],
        json!("is:pr is:open repo:jdx/jouet")
    );

    let out = jouet(
        &fake,
        dir.path(),
        &tokens,
        &[&args[..], &["--fail-on-attention"]].concat(),
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

#[test]
fn a_rejected_token_falls_back_to_the_next_source() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let tokens = [("GH_TOKEN", "stale"), ("GITHUB_TOKEN", GOOD)];
    let out = jouet(
        &fake,
        dir.path(),
        &tokens,
        &[
            "pr",
            "status",
            "--search",
            "is:open",
            "--repo",
            "jdx/jouet",
            "--settle",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stderr(&out).trim(),
        "jouet: GH_TOKEN was rejected (401); using GITHUB_TOKEN"
    );
    let tokens: Vec<String> = fake.requests().into_iter().map(|r| r.token).collect();
    assert_eq!(tokens, ["stale", GOOD]);
}

#[test]
fn every_token_rejected_is_a_remote_failure() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let tokens = [("GH_TOKEN", "stale"), ("GITHUB_TOKEN", "older")];
    let out = jouet(
        &fake,
        dir.path(),
        &tokens,
        &["pr", "status", "--repo", "jdx/jouet", "3", "--json"],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(json(&out)["error"]["code"], "remote");
    let message = json(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains("GH_TOKEN and GITHUB_TOKEN"), "{message}");
}

#[test]
fn no_token_at_all_is_a_remote_failure() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[],
        &["pr", "status", "--repo", "jdx/jouet", "3"],
    );
    assert_eq!(out.status.code(), Some(5));
    assert!(stderr(&out).contains("no GitHub token"), "{}", stderr(&out));
    assert!(fake.requests().is_empty());
}

#[cfg(unix)]
#[test]
fn gh_is_asked_for_its_own_token_after_the_environment_fails() {
    use std::os::unix::fs::PermissionsExt;
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    // gh prints GH_TOKEN when it is set; jouet must clear it to get gh's own.
    let gh = dir.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\nif [ -n \"$GH_TOKEN\" ]; then echo \"$GH_TOKEN\"; else echo {GOOD}; fi\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GH_TOKEN", "stale")],
        &[
            "pr",
            "status",
            "--search",
            "is:open",
            "--repo",
            "jdx/jouet",
            "--settle",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stderr(&out).trim(),
        "jouet: GH_TOKEN was rejected (401); using gh auth token"
    );
}

#[test]
fn a_server_error_is_retried() {
    let fake = serve(|request, index| {
        if index == 0 {
            (502, "bad gateway".into())
        } else {
            github(request, index)
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[("JOUET_GITHUB_TOKEN", GOOD)],
        &[
            "pr",
            "status",
            "--search",
            "is:open",
            "--repo",
            "jdx/jouet",
            "--settle",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("GitHub returned 502; retrying"));
    assert_eq!(fake.requests().len(), 2);
}

#[test]
fn a_response_cut_short_is_retried() {
    let fake = serve(|request, index| {
        let (status, body) = github(request, index);
        if index == 0 {
            (CUT_SHORT, body)
        } else {
            (status, body)
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[("JOUET_GITHUB_TOKEN", GOOD)],
        &[
            "pr",
            "status",
            "--search",
            "is:open",
            "--repo",
            "jdx/jouet",
            "--settle",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("request to GitHub failed"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fake.requests().len(), 2);
}

/// A search whose first PR GitHub has not computed mergeability for yet,
/// which a re-query by number then settles.
fn unsettled(request: &Request, _: usize) -> (u16, String) {
    let mut search: Value = serde_json::from_str(SEARCH).unwrap();
    let pr = &mut search["data"]["l0"]["nodes"][0];
    if query(request).contains("search(") {
        pr["mergeable"] = json!("UNKNOWN");
        (200, search.to_string())
    } else {
        let settled = json!({ "data": { "l0": { "pullRequest": pr.clone() } } });
        (200, settled.to_string())
    }
}

#[test]
fn settle_requeries_an_unknown_mergeable_state() {
    let dir = tempfile::tempdir().unwrap();
    let tokens = [("JOUET_GITHUB_TOKEN", GOOD)];
    let args = [
        "pr",
        "status",
        "--search",
        "is:open",
        "--repo",
        "jdx/jouet",
        "--json",
    ];

    let fake = serve(unsettled);
    let out = jouet(
        &fake,
        dir.path(),
        &tokens,
        &[&args[..], &["--settle", "5s"]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("to compute mergeability of jdx/jouet#7"),
        "{}",
        stderr(&out)
    );
    assert_eq!(json(&out)["prs"][0]["mergeable"], "mergeable");
    let requery = &fake.requests()[1].body["variables"];
    assert_eq!(requery["n0"], json!(7));

    let fake = serve(unsettled);
    let out = jouet(
        &fake,
        dir.path(),
        &tokens,
        &[&args[..], &["--settle", "0"]].concat(),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        json(&out)["prs"][0]["attention"][0]["code"],
        "mergeable_unknown"
    );
    assert_eq!(fake.requests().len(), 1);
}

#[test]
fn bad_arguments_are_usage_errors() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    let tokens = [("JOUET_GITHUB_TOKEN", GOOD)];
    for args in [
        &["pr", "status", "--repo", "jdx", "3"][..],
        &[
            "pr", "status", "--repo", "jdx/mise", "--repo", "jdx/hk", "3",
        ],
        &[
            "pr", "status", "--repo", "jdx/mise", "--settle", "soon", "3",
        ],
    ] {
        let out = jouet(&fake, dir.path(), &tokens, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    assert!(fake.requests().is_empty());
}
