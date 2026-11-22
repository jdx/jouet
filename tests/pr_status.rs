//! `jouet pr status` against a local fake GitHub, replaying GraphQL responses
//! recorded from github.com. Nothing here talks to the real API.

mod common;

use common::{CUT_SHORT, Fake, RATE_LIMITED, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

const BATCH: &str = include_str!("../crates/jouet-github/src/pr/fixtures/batch.json");
const FAILING_CHECKS: &str =
    include_str!("../crates/jouet-github/src/pr/fixtures/failing_checks.json");
const SEARCH: &str = include_str!("../crates/jouet-github/src/pr/fixtures/search.json");

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
    answer(request, GOOD)
}

/// The recorded response to `request`, or a 401 unless it carries `token`.
fn answer(request: &Request, token: &str) -> (u16, String) {
    if request.token != token {
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
fn an_exhausted_rate_limit_fails_at_once_and_says_when_it_resets() {
    let fake = serve(|_, _| {
        (
            RATE_LIMITED,
            r#"{"message":"API rate limit exceeded"}"#.into(),
        )
    });
    let dir = tempfile::tempdir().unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[("JOUET_GITHUB_TOKEN", GOOD)],
        &["pr", "status", "--repo", "jdx/jouet", "3", "--json"],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let message = json(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(
        message.contains("rate limit exhausted; it resets in "),
        "{message}"
    );
    assert_eq!(fake.requests().len(), 1);
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

/// Puts a `gh` in `dir` that runs `script`, a shell script with `$asks` set
/// to how many times gh has been run, this one included. `dir` is the whole
/// `PATH`, so the script has only what the shell has built in.
#[cfg(unix)]
fn fake_gh(dir: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let gh = dir.join("gh");
    let count = "echo asked >>\"$0.asks\"\nasks=0\n\
                 while read -r line; do asks=$((asks + 1)); done <\"$0.asks\"";
    std::fs::write(&gh, format!("#!/bin/sh\n{count}\n{script}\n")).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// How many times the `gh` in `dir` was run.
#[cfg(unix)]
fn gh_asks(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("gh.asks"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// A GitHub where the token `first` works for one request and only `second`
/// after it, as when gh's stored token expires and gh replaces it.
#[cfg(unix)]
fn github_whose_token_expires(first: &'static str, second: &'static str) -> Fake {
    serve(move |request, index| answer(request, if index == 0 { first } else { second }))
}

#[cfg(unix)]
#[test]
fn gh_is_asked_for_its_own_token_after_the_environment_fails() {
    let fake = serve(github);
    let dir = tempfile::tempdir().unwrap();
    // gh prints GH_TOKEN when it is set; jouet must clear it to get gh's own.
    fake_gh(
        dir.path(),
        &format!("if [ -n \"$GH_TOKEN\" ]; then echo \"$GH_TOKEN\"; else echo {GOOD}; fi"),
    );
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

#[cfg(unix)]
#[test]
fn gh_is_asked_again_when_the_token_it_gave_stops_working() {
    let fake = github_whose_token_expires("gh-token-1", "gh-token-2");
    let dir = tempfile::tempdir().unwrap();
    fake_gh(dir.path(), "echo \"gh-token-$asks\"");
    let args = [BATCH_ARGS, &["--json"]].concat();
    let out = jouet(&fake, dir.path(), &[], &args);
    // The batch was read with gh's first token; the names of the failing
    // checks, a second request, needed the one gh had by then.
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(
        json(&out)["prs"][0]["checks"]["failing"],
        json!(["untrusted / lint", "untrusted / test-ci", "test-ci"])
    );
    assert_eq!(
        stderr(&out).trim(),
        "jouet: gh auth token was rejected (401); using the token gh has now"
    );
    let tokens: Vec<String> = fake.requests().into_iter().map(|r| r.token).collect();
    assert_eq!(tokens, ["gh-token-1", "gh-token-1", "gh-token-2"]);
    assert_eq!(gh_asks(dir.path()), 2);
}

#[cfg(unix)]
#[test]
fn a_gh_that_still_has_the_rejected_token_is_a_remote_failure() {
    let fake = github_whose_token_expires("gh-token-1", "gh-token-2");
    let dir = tempfile::tempdir().unwrap();
    fake_gh(dir.path(), "echo gh-token-1");
    let args = [BATCH_ARGS, &["--json"]].concat();
    let out = jouet(&fake, dir.path(), &[], &args);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(
        json(&out)["error"]["message"],
        "GitHub rejected every token (401): gh auth token; \
         run `gh auth login` or set JOUET_GITHUB_TOKEN"
    );
    // The token gh repeated was not sent a second time.
    let tokens: Vec<String> = fake.requests().into_iter().map(|r| r.token).collect();
    assert_eq!(tokens, ["gh-token-1", "gh-token-1"]);
    assert_eq!(gh_asks(dir.path()), 2);
}

#[cfg(unix)]
#[test]
fn gh_is_asked_again_only_once_for_a_request() {
    // gh has a new token every time, and GitHub takes none after the first.
    let fake = github_whose_token_expires("gh-token-1", "no such token");
    let dir = tempfile::tempdir().unwrap();
    fake_gh(dir.path(), "echo \"gh-token-$asks\"");
    let args = [BATCH_ARGS, &["--json"]].concat();
    let out = jouet(&fake, dir.path(), &[], &args);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(
        json(&out)["error"]["message"],
        "GitHub rejected every token (401): gh auth token; \
         run `gh auth login` or set JOUET_GITHUB_TOKEN"
    );
    let tokens: Vec<String> = fake.requests().into_iter().map(|r| r.token).collect();
    assert_eq!(tokens, ["gh-token-1", "gh-token-1", "gh-token-2"]);
    assert_eq!(gh_asks(dir.path()), 2);
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
