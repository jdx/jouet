//! Routines and the GitHub probes end to end, against a local fake GitHub
//! replaying a recorded PR: `joue` run by its launcher, by a link named
//! `joue` and as `jouet run`, and a rule under `jouet rule test`, which has
//! the probes only with `rules.network`.

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

/// `joue` as the launcher runs it, as a symlink named `joue` does, and as
/// `jouet run`: the same routine, the same document.
fn joue_three_ways(fake: &Fake, home: &Path, args: &[&str]) -> Vec<Output> {
    let link = home.join(format!("joue{}", std::env::consts::EXE_SUFFIX));
    if !link.exists() {
        link_binary(Path::new(env!("CARGO_BIN_EXE_jouet")), &link);
    }
    let mut outputs = Vec::new();
    for program in [Path::new(env!("CARGO_BIN_EXE_joue")), &link] {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("JOUET_GITHUB_API_URL", &fake.url)
            .env("JOUET_GITHUB_TOKEN", "t")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("JOUET_DISPLAY");
        outputs.push(cmd.output().expect("joue should start"));
    }
    let mut run = vec!["run"];
    run.extend(args);
    outputs.push(jouet(fake, home, &[], &run));
    outputs
}

#[cfg(unix)]
fn link_binary(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn link_binary(target: &Path, link: &Path) {
    std::fs::copy(target, link).unwrap();
}

#[test]
fn the_pr_next_routine_says_what_pr_next_says() {
    let fake = fake();
    let home = tempfile::tempdir().unwrap();
    let rust = jouet(
        &fake,
        home.path(),
        &[],
        &["--json", "pr", "next", "jdx/mise#15172"],
    );
    assert_eq!(rust.status.code(), Some(10), "{}", stderr(&rust));
    for out in joue_three_ways(&fake, home.path(), &["--json", "pr-next", "jdx/mise#15172"]) {
        assert_eq!(out.status.code(), Some(10), "{}", stderr(&out));
        assert_eq!(stdout(&out), stdout(&rust));
    }
    let request = &fake.requests()[0];
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
fn a_users_routine_composes_primitives() {
    let fake = fake();
    let home = tempfile::tempdir().unwrap();
    write(
        home.path(),
        "routines",
        "draft-check.luau",
        r#"--- about: Say whether a PR is still a draft
--- capabilities: github.read
--- usage: arg "<pr>" help="The PR"
return {
    run = function(args)
        local found = github.prs({ prs = { args.pr }, settle = 0 })
        local pr = found.prs[1]
        print(`{pr.repo}#{pr.number} is {pr.state}`)
        return { draft = pr.state == "draft" }, if pr.state == "draft" then joue.exit.act else 0
    end,
}
"#,
    );
    let outs = joue_three_ways(&fake, home.path(), &["draft-check", "jdx/mise#15172"]);
    for out in &outs {
        assert_eq!(out.status.code(), Some(10), "{}", stderr(out));
        assert_eq!(stdout(out), "jdx/mise#15172 is draft\n");
    }
    let out = jouet(
        &fake,
        home.path(),
        &[],
        &["run", "draft-check", "jdx/mise#15172", "--json"],
    );
    assert_eq!(json(&out), serde_json::json!({ "draft": true }));
    let out = jouet(&fake, home.path(), &[], &["run", "draft-check", "--help"]);
    assert!(
        stdout(&out).contains("Usage: joue draft-check [FLAGS] <pr>"),
        "{}",
        stdout(&out)
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
return {
    check = function()
        local s, err = jouet.pr.status("jdx/mise#15172")
        return jouet.deny(if s then s.state else err)
    end,
    -- Only so that it may deny; this test runs it on one command.
    examples = { fires = { "true" }, quiet = { "false" } },
}
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
    // itself is the one `pr status` makes, which the routine tests above
    // check without a deadline that matters.
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
