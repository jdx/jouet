//! The `pr-next` routine against the Rust `pr next` it ports: every state,
//! made from PRs recorded on github.com, through the same fake GitHub, must
//! come out with the same action, kind, reason and command.

use crate::routine::{Catalog, builtin};
use crate::runtime::{self, Options};
use crate::testing::FakeGithub;
use crate::world::{Github, Host, Recorded, StepOutput, World};
use jouet_github::Client;
use jouet_github::pr::next;
use jouet_github::pr::select;
use jouet_github::pr::status::Lookup;
use jouet_github::pr::target::{Repo, Target};
use serde_json::{Value, json};
use std::time::Duration;

const RECORDED: &str = include_str!("../../jouet-github/src/pr/fixtures/next.json");

struct Nothing;

impl Host for Nothing {
    fn jouet(&mut self, _: &[String]) -> Result<StepOutput, String> {
        Err("no commands here".into())
    }
}

fn recorded(alias: &str) -> Value {
    let v: Value = serde_json::from_str(RECORDED).unwrap();
    v["data"][alias]["pullRequest"].clone()
}

fn green(mut pr: Value) -> Value {
    let rollup = &mut pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
    rollup["checkRunCountsByState"] = json!([{ "state": "SUCCESS", "count": 12 }]);
    rollup["statusContextCountsByState"] = json!([]);
    pr["mergeStateStatus"] = json!("CLEAN");
    pr["isDraft"] = json!(false);
    pr["mergeable"] = json!("MERGEABLE");
    pr["reviewThreads"]["nodes"] = json!([]);
    pr
}

fn edited(mut pr: Value, edit: impl FnOnce(&mut Value)) -> Value {
    edit(&mut pr);
    pr
}

/// Every state `pr next` can reach, but a failing check (whose names a
/// follow-up query fetches).
fn variants() -> Vec<(&'static str, Value)> {
    let base = || green(recorded("l3"));
    let no_checks = |pushed: &str| {
        let pushed = pushed.to_string();
        edited(base(), move |pr| {
            pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = Value::Null;
            pr["commits"]["nodes"][0]["commit"]["committedDate"] = json!(pushed);
        })
    };
    vec![
        ("merged", recorded("l0")),
        ("closed", recorded("l1")),
        ("draft", recorded("l2")),
        ("pending", recorded("l3")),
        ("pending with threads", recorded("l5")),
        ("ready", base()),
        (
            "conflict",
            edited(base(), |pr| pr["mergeable"] = json!("CONFLICTING")),
        ),
        (
            "unknown",
            edited(base(), |pr| pr["mergeable"] = json!("UNKNOWN")),
        ),
        (
            "behind",
            edited(base(), |pr| pr["mergeStateStatus"] = json!("BEHIND")),
        ),
        ("unresolved", green(recorded("l5"))),
        (
            "unresolved threads kept",
            edited(green(recorded("l5")), |pr| {
                pr["reviewThreads"] = recorded("l5")["reviewThreads"].clone();
            }),
        ),
        (
            "changes requested",
            edited(base(), |pr| {
                pr["reviewDecision"] = json!("CHANGES_REQUESTED")
            }),
        ),
        (
            "review required",
            edited(base(), |pr| {
                pr["reviewDecision"] = json!("REVIEW_REQUIRED");
                pr["mergeStateStatus"] = json!("BLOCKED");
            }),
        ),
        (
            "approved",
            edited(base(), |pr| pr["reviewDecision"] = json!("APPROVED")),
        ),
        (
            "merge state unknown",
            edited(base(), |pr| pr["mergeStateStatus"] = json!("UNKNOWN")),
        ),
        (
            "blocked",
            edited(base(), |pr| pr["mergeStateStatus"] = json!("BLOCKED")),
        ),
        (
            "unstable",
            edited(base(), |pr| pr["mergeStateStatus"] = json!("UNSTABLE")),
        ),
        (
            "auto-merge",
            edited(base(), |pr| {
                pr["autoMergeRequest"] = json!({ "enabledAt": "2026-10-30T00:00:00Z" })
            }),
        ),
        ("checks registering", no_checks("2099-01-01T00:00:00Z")),
        ("checks missing", no_checks("2020-01-01T00:00:00Z")),
    ]
}

fn serve(pr: &Value) -> FakeGithub {
    FakeGithub::start(vec![json!({
        "method": "POST", "path": "/graphql",
        "body": { "data": { "l0": { "pullRequest": pr } } },
    })])
    .unwrap()
}

#[test]
fn the_routine_decides_as_the_rust_pr_next_does() {
    let catalog = Catalog::new(builtin());
    let routine = catalog.get("pr-next").unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for (name, pr) in variants() {
        let target = format!(
            "{}#{}",
            pr["repository"]["nameWithOwner"].as_str().unwrap(),
            pr["number"]
        );
        let server = serve(&pr);
        let mut client = Client::from_env().with_api_url(&server.url).with_token("t");
        let (owner, repo) = target.split_once('#').unwrap().0.split_once('/').unwrap();
        let lookup = Lookup::Pr(Target::Number(
            Repo::parse(&format!("{owner}/{repo}")).unwrap(),
            pr["number"].as_u64().unwrap(),
        ));
        let report = select::fetch(&mut client, vec![lookup], Duration::ZERO).unwrap();
        let rust = serde_json::to_value(next::next(&report.prs[0])).unwrap();

        let server = serve(&pr);
        let args = routine
            .parse_args(&[
                target.clone(),
                "--settle".into(),
                "0".into(),
                "--json".into(),
            ])
            .unwrap();
        let world = World {
            root: std::env::current_dir().unwrap(),
            host: Box::new(Nothing),
            github: Github::Fake {
                url: server.url.clone(),
                token: "t".into(),
            },
            ledger: None,
            repo_key: None,
            log_dir: std::env::temp_dir(),
            recorded: Some(Recorded::default()),
        };
        let options = Options {
            json: true,
            ..Options::default()
        };
        let out = runtime::run(&catalog, routine, args, world, options);
        assert_eq!(out.error, None, "{name}");
        let entry = &out.value.unwrap()["prs"][0];
        for key in ["action", "kind", "reason", "command"] {
            assert_eq!(entry[key], rust[key], "{name}: {key}");
        }
        seen.insert(rust["action"].as_str().unwrap().to_string());
    }
    // Every action but checks_failing was reached.
    assert_eq!(seen.len(), 13, "{seen:?}");
}
