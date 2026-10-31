//! The state machine against GraphQL responses recorded from github.com
//! (`fixtures/next.json`). States no recorded PR was in are made by editing
//! one recorded PR's fields, so every state is read through the same parser
//! `pr status` uses.

use super::*;
use crate::pr::status::read_pr;
use jouet_output::time;
use serde_json::{Value, json};

/// The recorded PRs, by alias: a merged and a closed PR, a draft, one with
/// checks running, one with cancelled checks and auto-merge on, and one with
/// unresolved threads (and checks running).
const NEXT: &str = include_str!("fixtures/next.json");

fn recorded(alias: &str) -> Value {
    let v: Value = serde_json::from_str(NEXT).unwrap();
    v["data"][alias]["pullRequest"].clone()
}

/// A while after the last recorded push.
fn now() -> i64 {
    time::parse_utc("2026-10-30T19:00:00Z").unwrap()
}

fn status(raw: &Value) -> PrStatus {
    read_pr(raw).unwrap().finish(now())
}

fn next_of(alias: &str, edit: impl FnOnce(&mut Value)) -> Next {
    let mut raw = recorded(alias);
    edit(&mut raw);
    next(&status(&raw))
}

/// Every check succeeded and GitHub would let it merge: the starting point
/// for the states that come after the checks.
fn green(pr: &mut Value) {
    let rollup = &mut pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
    rollup["checkRunCountsByState"] = json!([{ "state": "SUCCESS", "count": 12 }]);
    rollup["statusContextCountsByState"] = json!([]);
    pr["mergeStateStatus"] = json!("CLEAN");
    pr["isDraft"] = json!(false);
    pr["mergeable"] = json!("MERGEABLE");
}

/// What `jdx/jouet#19` (`l3`) looks like once its checks pass.
fn ready(edit: impl FnOnce(&mut Value)) -> Next {
    next_of("l3", |pr| {
        green(pr);
        edit(pr);
    })
}

#[test]
fn a_merged_pr_is_done() {
    let n = next_of("l0", |_| {});
    assert_eq!((n.action, n.kind), (Action::Merged, Kind::Done));
    assert_eq!(n.command, None);
}

#[test]
fn a_closed_pr_is_done() {
    let n = next_of("l1", |_| {});
    assert_eq!((n.action, n.kind), (Action::Closed, Kind::Done));
    assert_eq!(n.reason, "Closed without merging; reopen it to continue");
}

#[test]
fn a_draft_must_be_marked_ready() {
    let n = next_of("l2", |_| {});
    assert_eq!((n.action, n.kind), (Action::Draft, Kind::Act));
    assert_eq!(n.reason, "The PR is a draft");
    assert_eq!(
        n.command.as_deref(),
        Some("gh pr ready 15172 --repo jdx/mise")
    );
}

#[test]
fn running_checks_are_waited_for() {
    let n = next_of("l3", |_| {});
    assert_eq!((n.action, n.kind), (Action::ChecksPending, Kind::Wait));
    assert_eq!(n.reason, "5 of 15 checks pending");
    assert_eq!(n.command.as_deref(), Some("jouet wait ci jdx/jouet#19"));
}

#[test]
fn a_failed_check_comes_before_running_ones_and_auto_merge() {
    // Two cancelled checks, seven still running, and auto-merge on.
    let n = next_of("l4", |_| {});
    assert_eq!((n.action, n.kind), (Action::ChecksFailing, Kind::Act));
    assert!(n.reason.starts_with("2 failing"), "{}", n.reason);
    assert_eq!(n.command.as_deref(), Some("jouet wait ci jdx/mise#15205"));
}

#[test]
fn threads_wait_for_the_checks_then_need_answers() {
    let n = next_of("l5", |_| {});
    assert_eq!(n.action, Action::ChecksPending);
    let n = next_of("l5", green);
    assert_eq!((n.action, n.kind), (Action::UnresolvedThreads, Kind::Act));
    assert_eq!(n.reason, "1 unresolved review thread");
    assert_eq!(n.command.as_deref(), Some("jouet pr threads jdx/jouet#15"));
}

#[test]
fn a_conflict_is_merged_away() {
    let n = ready(|pr| {
        pr["mergeable"] = json!("CONFLICTING");
        pr["mergeStateStatus"] = json!("DIRTY");
    });
    assert_eq!((n.action, n.kind), (Action::Conflict, Kind::Act));
    assert_eq!(
        n.command.as_deref(),
        Some("git fetch origin main && git merge origin/main")
    );
}

#[test]
fn unknown_mergeability_is_waited_out() {
    let n = ready(|pr| pr["mergeable"] = json!("UNKNOWN"));
    assert_eq!((n.action, n.kind), (Action::MergeableUnknown, Kind::Wait));
    assert_eq!(
        n.command.as_deref(),
        Some("jouet wait pr jdx/jouet#19 --until mergeable")
    );
    // GitHub can know there is no conflict before it knows the merge state.
    let n = ready(|pr| pr["mergeStateStatus"] = json!("UNKNOWN"));
    assert_eq!(n.action, Action::MergeableUnknown);
}

#[test]
fn a_branch_behind_its_base_is_updated() {
    let n = ready(|pr| pr["mergeStateStatus"] = json!("BEHIND"));
    assert_eq!((n.action, n.kind), (Action::BehindBase, Kind::Act));
    assert_eq!(
        n.reason,
        "Behind main; update the branch before it can merge"
    );
    assert_eq!(
        n.command.as_deref(),
        Some("git fetch origin main && git merge origin/main")
    );
}

#[test]
fn requested_changes_are_read_with_the_reviews() {
    let n = ready(|pr| pr["reviewDecision"] = json!("CHANGES_REQUESTED"));
    assert_eq!((n.action, n.kind), (Action::ChangesRequested, Kind::Act));
    assert_eq!(
        n.command.as_deref(),
        Some("jouet pr threads jdx/jouet#19 --include-reviews")
    );
}

#[test]
fn a_required_review_is_waited_for() {
    let n = ready(|pr| {
        pr["reviewDecision"] = json!("REVIEW_REQUIRED");
        pr["mergeStateStatus"] = json!("BLOCKED");
    });
    assert_eq!((n.action, n.kind), (Action::AwaitingApproval, Kind::Wait));
    assert_eq!(
        n.command.as_deref(),
        Some("jouet wait pr jdx/jouet#19 --until approved")
    );
}

#[test]
fn another_protection_rule_blocks_without_bypass() {
    let n = ready(|pr| pr["mergeStateStatus"] = json!("BLOCKED"));
    assert_eq!((n.action, n.kind), (Action::Blocked, Kind::Wait));
    assert_eq!(
        n.reason,
        "GitHub reports the merge as blocked by branch protection or a ruleset"
    );
}

#[test]
fn auto_merge_is_left_to_github() {
    let n = ready(|pr| pr["autoMergeRequest"] = json!({ "enabledAt": "2026-10-30T18:00:00Z" }));
    assert_eq!((n.action, n.kind), (Action::AutoMerge, Kind::Wait));
    assert_eq!(
        n.command.as_deref(),
        Some("jouet wait pr jdx/jouet#19 --until merged")
    );
}

#[test]
fn a_green_pr_is_ready_to_merge() {
    let n = ready(|_| {});
    assert_eq!((n.action, n.kind), (Action::ReadyToMerge, Kind::Done));
    assert_eq!(
        n.reason,
        "Checks pass (12/12), no review required, no unresolved threads"
    );
    assert_eq!(n.command.as_deref(), Some("jouet pr land jdx/jouet#19"));
    let n = ready(|pr| pr["reviewDecision"] = json!("APPROVED"));
    assert_eq!(
        n.reason,
        "Checks pass (12/12), approved, no unresolved threads"
    );
    // A check branch protection does not require may fail.
    let n = ready(|pr| pr["mergeStateStatus"] = json!("UNSTABLE"));
    assert_eq!(n.action, Action::ReadyToMerge);
}

#[test]
fn checks_that_have_not_registered_yet_are_waited_for() {
    let no_checks = |pushed: &'static str| {
        ready(move |pr| {
            let commit = &mut pr["commits"]["nodes"][0]["commit"];
            commit["statusCheckRollup"] = Value::Null;
            commit["committedDate"] = json!(pushed);
            pr["timelineItems"]["nodes"] = json!([]);
        })
    };
    let n = no_checks("2026-10-30T18:58:00Z");
    assert_eq!(n.action, Action::ChecksPending);
    assert_eq!(n.reason, "Waiting for checks to register on the new head");
    // Long after the push, a repository with no CI is not held up forever:
    // branch protection, which knows which checks it requires, decides.
    let n = no_checks("2026-10-30T12:00:00Z");
    assert_eq!(n.action, Action::ReadyToMerge);
    assert!(n.reason.starts_with("No checks ran"), "{}", n.reason);
}

#[test]
fn the_order_is_fixed() {
    // A draft that conflicts is a draft first.
    let n = next_of("l2", |pr| pr["mergeable"] = json!("CONFLICTING"));
    assert_eq!(n.action, Action::Draft);
    // A conflict comes before failing checks, which cannot run until it is
    // resolved.
    let n = next_of("l4", |pr| pr["mergeable"] = json!("CONFLICTING"));
    assert_eq!(n.action, Action::Conflict);
    // Behind comes before failing checks, which may pass once updated.
    let n = next_of("l4", |pr| pr["mergeStateStatus"] = json!("BEHIND"));
    assert_eq!(n.action, Action::BehindBase);
    // Requested changes come before threads.
    let n = next_of("l5", |pr| {
        green(pr);
        pr["reviewDecision"] = json!("CHANGES_REQUESTED");
    });
    assert_eq!(n.action, Action::ChangesRequested);
}

#[test]
fn each_action_has_one_kind_and_a_snake_case_name() {
    let n = ready(|_| {});
    assert_eq!(
        serde_json::to_value(&n).unwrap(),
        json!({
            "action": "ready_to_merge",
            "kind": "done",
            "reason": "Checks pass (12/12), no review required, no unresolved threads",
            "command": "jouet pr land jdx/jouet#19",
        })
    );
    assert_eq!(Action::AwaitingApproval.as_str(), "awaiting_approval");
}

/// A failed Entire Gates check on the head with this summary, recorded
/// from jdx/mise#13958.
fn gates(pr: &mut Value, summary: &str) {
    pr["commits"]["nodes"][0]["commit"]["checkSuites"] = json!({ "nodes": [{ "checkRuns": { "nodes": [{
        "status": "COMPLETED", "conclusion": "FAILURE",
        "detailsUrl": "https://entire.io/gh/jdx/mise/trails/424",
        "title": "2 blocking gates not passing", "summary": summary,
    }] } }] });
}

const CHECKS_AND_FINDINGS: &str = "- ❌ **Checks**: failed — CI checks have not passed\n- ❌ **Findings**: failed — Unresolved findings: 2 medium severity\n\n[Trail #424 on Entire](https://entire.io/gh/jdx/mise/trails/424)";

/// A PR whose only failing check is Entire Gates, failing as `summary` says.
fn entire_only(summary: &str) -> PrStatus {
    let mut raw = recorded("l3");
    green(&mut raw);
    raw["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["checkRunCountsByState"] =
        json!([{ "state": "SUCCESS", "count": 12 }, { "state": "FAILURE", "count": 1 }]);
    gates(&mut raw, summary);
    let mut fetched = read_pr(&raw).unwrap();
    fetched.status_mut().checks.failing = vec![crate::entire::CHECK.into()];
    fetched.finish(now())
}

#[test]
fn entire_gates_is_not_a_failing_check_of_its_own() {
    let pr = entire_only(
        "- ❌ **Checks**: failed — CI checks have not passed\n\n[Trail #424 on Entire](https://entire.io/gh/jdx/mise/trails/424)",
    );
    assert!(
        pr.attention.iter().all(|a| a.code != "checks_failing"),
        "{:?}",
        pr.attention
    );
    assert_eq!(next(&pr).action, Action::ReadyToMerge);
}

#[test]
fn open_entire_findings_name_the_first_to_fix() {
    let mut pr = entire_only(CHECKS_AND_FINDINGS);
    let n = next(&pr);
    assert_eq!((n.action, n.kind), (Action::EntireFindings, Kind::Act));
    assert_eq!(
        n.reason,
        "Entire Gates: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity"
    );
    assert_eq!(n.command.as_deref(), Some("jouet pr threads jdx/jouet#19"));
    // With the findings `entire` listed.
    let e = pr.entire.as_mut().unwrap();
    e.findings = crate::entire::Brief::of(&[crate::entire::Finding {
        id: "01M4C997DZH5KQRP31NW93NQHR".into(),
        status: "open".into(),
        severity: Some("medium".into()),
        stale: false,
        path: Some("e2e/cli/test_deps_tool_install".into()),
        line: Some(21),
        end_line: None,
        title: "Only removes the compat symlink.".into(),
        body: String::new(),
        author: "agent:trail-review".into(),
        created_at: String::new(),
        status_reason: None,
    }]);
    assert_eq!(
        e.findings_message(),
        "Fix Entire finding [entire:01M4C997DZ] at e2e/cli/test_deps_tool_install:21 (medium): Only removes the compat symlink."
    );
}

#[test]
fn an_out_of_date_branch_by_entire_is_behind_base() {
    let pr = entire_only(
        "- ✅ **Checks**: passed — CI checks have passed\n- ❌ **Up to date**: failed — 3 commits behind · Merge conflicts\n\n[Trail #88 on Entire](https://entire.com/gh/jdx/-/trails/3284/changes/jouet/88)",
    );
    let n = next(&pr);
    assert_eq!(n.action, Action::BehindBase);
    assert_eq!(
        n.reason,
        "Entire Gates: 3 commits behind · Merge conflicts; update the branch from main before it can merge"
    );
}
