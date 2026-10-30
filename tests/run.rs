//! The GitHub probes end to end, against a local fake GitHub replaying a
//! recorded PR: from a recipe, which always has them, and from a rule under
//! `jouet rule test`, which has them only with `rules.network`.

mod common;

use common::{Fake, json, query, serve, stderr, stdout};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

const NEXT: &str = include_str!("../crates/jouet-github/src/pr/fixtures/next.json");

/// Answers every query with the recorded draft PR jdx/mise#15172 as `l0`.
fn fake() -> Fake {
    let recorded: Value = serde_json::from_str(NEXT).unwrap();
    let body = serde_json::json!({ "data": { "l0": recorded["data"]["l2"] } }).to_string();
    serve(move |_, _| (200, body.clone()))
}

fn jouet(fake: &Fake, home: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", "t");
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "JOUET_DISPLAY",
        "JOUET_RULES_NETWORK",
        "JOUET_RULES_DISABLE",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.output().expect("jouet should start")
}

fn write(home: &Path, dir: &str, name: &str, code: &str) {
    let dir = home.join(".config/jouet").join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), code).unwrap();
}

#[test]
fn a_recipe_reads_a_pr_through_the_probes() {
    let fake = fake();
    let home = tempfile::tempdir().unwrap();
    write(
        home.path(),
        "recipes",
        "look.luau",
        r#"--- why: read a PR
return { run = function()
    local s = jouet.pr.status("jdx/mise#15172")
    local n = jouet.pr.next("jdx/mise#15172")
    return { number = s.number, state = s.state, repo = n.repo, action = n.action, kind = n.kind }
end }
"#,
    );
    let out = jouet(&fake, home.path(), &[], &["--json", "run", "look"]);
    let report = json(&out);
    assert_eq!(report["exit_code"], 0, "{}{}", stdout(&out), stderr(&out));
    let result = &report["result"];
    assert_eq!(result["number"], 15172);
    assert_eq!(result["state"], "draft");
    assert_eq!(result["repo"], "jdx/mise");
    assert!(result["action"].is_string() && result["kind"].is_string());
    // Status and next come from one fetch, cached for the step, made the
    // way `jouet pr status` makes it.
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        (
            request.method.as_str(),
            request.path.as_str(),
            request.token.as_str()
        ),
        ("POST", "/graphql", "t")
    );
    assert!(
        query(request).contains("pullRequest(number:"),
        "{}",
        query(request)
    );
}

#[test]
fn a_rule_reaches_github_only_with_rules_network() {
    let fake = fake();
    let home = tempfile::tempdir().unwrap();
    write(
        home.path(),
        "rules",
        "pr-open.luau",
        r#"--- why: say what state the PR is in
--- mode: deny
return { check = function()
    local s, err = jouet.pr.status("jdx/mise#15172")
    return jouet.deny(if s then s.state else err)
end }
"#,
    );
    let args = ["rule", "test", "pr-open", "--command", "true"];
    let out = jouet(&fake, home.path(), &[], &args);
    assert_eq!(
        stdout(&out).trim(),
        "deny: network probes are off in hooks; set `rules.network = true` to turn them on"
    );
    assert!(fake.requests().is_empty());

    // With the setting on, the probe goes to GitHub. Whether the answer
    // arrives within the rule's 50 ms depends on the machine, so either
    // outcome is right: the PR's state, or the rule failing open over its
    // budget. What must not happen is the probe being refused. The fetch
    // itself is checked, without a deadline that matters, by the recipe test
    // above.
    let out = jouet(
        &fake,
        home.path(),
        &[("JOUET_RULES_NETWORK", "true")],
        &args,
    );
    let (said, err) = (stdout(&out), stderr(&out));
    let answered = said.trim() == "deny: draft";
    let failed_open = (said.trim().is_empty() && err.contains("budget"))
        || said.contains("did not answer within the budget");
    assert!(answered || failed_open, "stdout: {said}\nstderr: {err}");
    assert!(!said.contains("network probes are off"));
}
