//! `jouet stack status` against a local fake GitHub that answers the PR
//! lookups for a chain of branches, over real git repositories. Nothing
//! here talks to the real API.

mod common;

use common::{Fake, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

const TOKEN: &str = "good-token";

/// A PR node as GitHub's GraphQL returns it.
fn pr(number: u64, head: &str, base: &str, state: &str) -> Value {
    json!({
        "number": number,
        "url": format!("https://github.com/acme/app/pull/{number}"),
        "state": state,
        "isDraft": false,
        "baseRefName": base,
        "headRefName": head,
        "headRefOid": "0000000000000000000000000000000000000000",
    })
}

/// Answers each `hN` alias with the PRs whose head is that branch and each
/// `bN` alias with the open PRs based on it, from `prs`.
fn github(prs: Vec<Value>) -> Fake {
    serve(move |request: &Request, _| {
        if request.token != TOKEN {
            return (401, r#"{"message":"Bad credentials"}"#.into());
        }
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("POST", "/graphql")
        );
        assert!(
            query(request).contains("pullRequests("),
            "{}",
            query(request)
        );
        let vars = &request.body["variables"];
        let mut repo = json!({ "defaultBranchRef": { "name": "main" } });
        for (key, value) in vars.as_object().unwrap() {
            let Some(branch) = value.as_str() else {
                continue;
            };
            let nodes: Vec<&Value> = if key.starts_with('h') {
                prs.iter().filter(|p| p["headRefName"] == branch).collect()
            } else if key.starts_with('b') {
                prs.iter()
                    .filter(|p| p["baseRefName"] == branch && p["state"] == "OPEN")
                    .collect()
            } else {
                continue;
            };
            repo[key] = json!({ "nodes": nodes });
        }
        (200, json!({ "data": { "repository": repo } }).to_string())
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-qm", message]);
}

/// main ← a ← b ← c, each pushed (its remote-tracking ref set), with a
/// GitHub remote; `a` fixes `lib.txt`.
fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    git(
        dir,
        &["remote", "add", "origin", "https://github.com/acme/app.git"],
    );
    commit(dir, "lib.txt", "one\nbug\nthree\n", "init");
    git(dir, &["checkout", "-q", "-b", "a"]);
    commit(dir, "lib.txt", "one\nfixed\nthree\n", "a: fix");
    git(dir, &["checkout", "-q", "-b", "b"]);
    commit(dir, "b.txt", "b\n", "b");
    git(dir, &["checkout", "-q", "-b", "c"]);
    commit(dir, "c.txt", "c\n", "c");
    for branch in ["main", "a", "b", "c"] {
        let sha = git(dir, &["rev-parse", branch]);
        git(
            dir,
            &["update-ref", &format!("refs/remotes/origin/{branch}"), &sha],
        );
    }
    git(dir, &["checkout", "-q", "b"]);
    tmp
}

fn jouet(dir: &Path, fake: &Fake, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", TOKEN)
        .env("JOUET_DISPLAY", "plain")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for var in ["GH_TOKEN", "GITHUB_TOKEN", "HTTPS_PROXY", "https_proxy"] {
        cmd.env_remove(var);
    }
    cmd.output().expect("jouet should start")
}

fn chain() -> Vec<Value> {
    vec![
        pr(1, "a", "main", "OPEN"),
        pr(2, "b", "a", "OPEN"),
        pr(3, "c", "b", "OPEN"),
        pr(9, "other", "main", "OPEN"),
    ]
}

#[test]
fn the_chain_of_pr_bases_is_followed_down_and_up() {
    let tmp = repo();
    let fake = github(chain());
    let out = jouet(tmp.path(), &fake, &["stack", "status"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "STACK main ← #1 ← #2 ← #3 (from PR bases on GitHub)\n  \
         #1 a  contains main\n\
         * #2 b  contains a\n  \
         #3 c  contains b\n"
    );
    let doc = json(&jouet(tmp.path(), &fake, &["stack", "status", "--json"]));
    assert_eq!(doc["source"], "github");
    assert_eq!(doc["trunk"], "main");
    let layers = doc["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 3);
    assert_eq!(layers[1]["pr"]["number"], 2);
    assert_eq!(layers[1]["parent"], "a");
    assert_eq!(layers[1]["contains_parent"], true);
    assert!(layers[1].get("needs").is_none());
    // One query per step down or up; never more than the stack is deep.
    assert!(fake.requests().len() <= 6 + 3, "{}", fake.requests().len());
}

#[test]
fn a_restack_that_reverts_the_parents_fix_is_flagged() {
    let tmp = repo();
    let dir = tmp.path();
    // A new fix lands on `a`; `b` is "restacked" by a merge that keeps its
    // own tree, so it has a's commits but not the fix.
    git(dir, &["checkout", "-q", "a"]);
    commit(dir, "lib.txt", "one\nfixed better\nthree\n", "a: fix again");
    git(dir, &["checkout", "-q", "b"]);
    git(dir, &["merge", "-q", "--no-edit", "-s", "ours", "a"]);
    let fake = github(chain());
    let out = jouet(dir, &fake, &["stack", "status"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    let fix = git(dir, &["log", "-1", "--format=%h %s", "a"]);
    assert!(
        text.contains(&format!(
            "* #2 b  has a's commits but not its changes to lib.txt (undoes {fix}); 2 unpushed\n    restore it with `git checkout a -- lib.txt` and commit, unless the revert is meant\n"
        )),
        "{text}"
    );
    // `c` was built on the old `b`, so it needs b merged in.
    assert!(text.contains("  #3 c  needs b merged in\n"), "{text}");
    let doc = json(&jouet(dir, &fake, &["stack", "status", "--json"]));
    let b = &doc["layers"][1];
    assert_eq!(b["contains_parent"], true);
    assert_eq!(
        b["content"],
        json!([{ "path": "lib.txt", "problem": "reverted", "commit": fix }])
    );
    assert_eq!(b["needs"], json!(["parent_changes_missing"]));
    assert_eq!(b["unpushed"], 2);
}

#[test]
fn a_merged_prerequisite_means_the_trunk_must_be_merged_in() {
    let tmp = repo();
    let dir = tmp.path();
    // `a` is squash-merged on GitHub; main moves on.
    git(dir, &["checkout", "-q", "main"]);
    git(dir, &["merge", "-q", "--squash", "a"]);
    git(dir, &["commit", "-qm", "a (#1)"]);
    let sha = git(dir, &["rev-parse", "main"]);
    git(dir, &["update-ref", "refs/remotes/origin/main", &sha]);
    git(dir, &["checkout", "-q", "b"]);
    let mut prs = chain();
    prs[0] = pr(1, "a", "main", "MERGED");
    let fake = github(prs);
    let out = jouet(dir, &fake, &["stack", "status", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let doc = json(&out);
    assert_eq!(doc["layers"][0]["merged"], true);
    assert_eq!(doc["layers"][1]["needs"], json!(["needs_trunk"]));
    let text = stdout(&jouet(dir, &fake, &["stack", "status"]));
    assert!(text.contains("  #1 a  merged\n"), "{text}");
    assert!(
        text.contains("* #2 b  its base merged: needs origin/main merged in\n"),
        "{text}"
    );
}

#[test]
fn a_branch_in_no_stack_exits_3_and_github_errors_exit_5() {
    let tmp = repo();
    let fake = github(vec![]);
    let out = jouet(tmp.path(), &fake, &["stack", "status"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("`b` has no PR"), "{}", stderr(&out));
    let down = serve(|_, _| (502, r#"{"message":"bad gateway"}"#.into()));
    let out = jouet(tmp.path(), &down, &["stack", "status", "--json"]);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(json(&out)["error"]["code"], "remote");
}

#[test]
fn gh_stack_metadata_names_the_layers_and_works_without_github() {
    let tmp = repo();
    let dir = tmp.path();
    std::fs::write(
        dir.join(".git/gh-stack"),
        r#"{"schemaVersion":1,"stacks":[{"trunk":{"branch":"main"},"branches":[
            {"branch":"a","pullRequest":{"number":1}},{"branch":"b"},{"branch":"c"}]}]}"#,
    )
    .unwrap();
    let fake = github(chain());
    let out = jouet(dir, &fake, &["stack", "status"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("STACK main ← #1 ← #2 ← #3 (from gh-stack)\n"));
    // One query for every layer at once.
    assert_eq!(fake.requests().len(), 1);
    let down = serve(|_, _| (502, r#"{"message":"bad gateway"}"#.into()));
    let out = jouet(dir, &down, &["stack", "status"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(
        text.starts_with("STACK main ← a ← b ← c (from gh-stack)\n"),
        "{text}"
    );
    assert!(text.contains("note: GitHub could not be reached, so merged PRs are not known"));
}
