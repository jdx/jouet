//! Parsing and attention derivation, against GraphQL responses recorded from
//! github.com (`fixtures/`).

use super::*;

/// Responses to the queries `query` built for these lookups.
const BATCH: &str = include_str!("fixtures/batch.json");
const FAILING_CHECKS: &str = include_str!("fixtures/failing_checks.json");
const SEARCH: &str = include_str!("fixtures/search.json");

fn repo(s: &str) -> Repo {
    Repo::parse(s).unwrap()
}

fn batch_lookups() -> Vec<Lookup> {
    vec![
        Lookup::Pr(Target::Number(repo("jdx/mise"), 14955)),
        Lookup::Pr(Target::Number(repo("jdx/mise"), 15147)),
        Lookup::Pr(Target::Number(repo("jdx/mise"), 15187)),
        Lookup::Pr(Target::Number(repo("jdx/mise"), 99_999_999)),
        Lookup::Pr(Target::Number(repo("jdx/jouet"), 3)),
        Lookup::Pr(Target::Branch(
            repo("jdx/mise"),
            "claude/no-such-branch".into(),
        )),
    ]
}

fn data(fixture: &str) -> Value {
    let v: Value = serde_json::from_str(fixture).unwrap();
    v["data"].clone()
}

fn now() -> i64 {
    time::parse_utc("2026-10-30T15:00:00Z").unwrap()
}

fn fetch_batch() -> FetchResult {
    let mut result = FetchResult::default();
    let more = read_prs(&batch_lookups(), &data(BATCH), &mut result).unwrap();
    assert!(more.is_empty());
    result
}

fn codes(pr: &PrStatus) -> Vec<&'static str> {
    pr.attention.iter().map(|a| a.code).collect()
}

#[test]
fn a_batch_reads_every_pr_and_reports_the_missing_ones() {
    let result = fetch_batch();
    let numbers: Vec<_> = result
        .prs
        .iter()
        .map(|p| (p.status.repo.as_str(), p.status.number))
        .collect();
    assert_eq!(
        numbers,
        [
            ("jdx/mise", 14955),
            ("jdx/mise", 15147),
            ("jdx/mise", 15187),
            ("jdx/jouet", 3)
        ]
    );
    assert_eq!(
        result.not_found,
        ["jdx/mise#99999999", "jdx/mise:claude/no-such-branch"]
    );
}

#[test]
fn fields_are_read_from_the_response() {
    let pr = &fetch_batch().prs[0].status;
    assert_eq!(
        pr.title,
        "fix(hook-env): install the error reporter before the fast path"
    );
    assert_eq!(pr.url, "https://github.com/jdx/mise/pull/14955");
    assert_eq!(pr.state, State::Draft);
    assert_eq!(pr.base, "main");
    assert_eq!(pr.head, "fix/hook-env-error-hook-order");
    assert_eq!(pr.head_sha, "a03b3ef655");
    assert_eq!(pr.pushed_at.as_deref(), Some("2026-10-25T13:25:21Z"));
    assert_eq!(pr.mergeable, Mergeability::Conflicting);
    assert_eq!(pr.merge_state, "dirty");
    assert_eq!(pr.review, None);
    assert!(!pr.auto_merge);
    // Check runs and commit statuses are counted together; NEUTRAL counts
    // as skipped, as in `gh pr checks`.
    assert_eq!(
        (
            pr.checks.pass,
            pr.checks.fail,
            pr.checks.pending,
            pr.checks.skipped
        ),
        (25, 3, 0, 23)
    );
}

#[test]
fn failing_check_names_are_fetched_only_for_prs_with_failures() {
    let mut result = fetch_batch();
    let pages: Vec<Lookup> = result.prs.iter().flat_map(Fetched::pages).collect();
    let rollup = "SCR_kwDOIvux3toAKGEwM2IzZWY2NTU2OThjY2RiYTVkY2I4ZjhlNTA4ZGFlMDFjMmZiYjE";
    assert_eq!(
        pages,
        [Lookup::Contexts {
            id: rollup.into(),
            after: None
        }]
    );
    // The recorded follow-up query was built from that one lookup.
    let (q, vars) = query(&pages);
    assert!(q.contains("contexts(first: 100, after: $c0)"), "{q}");
    assert!(!q.contains("fragment"), "{q}");
    assert_eq!(vars, json!({ "id0": rollup, "c0": null }));

    read_page(&mut result.prs[0], &pages[0], &data(FAILING_CHECKS)["l0"]).unwrap();
    assert!(result.prs[0].pages().is_empty());
    assert_eq!(
        result.prs[0].status.checks.failing,
        ["untrusted / lint", "untrusted / test-ci", "test-ci"]
    );
}

#[test]
fn attention_is_derived_from_the_fields() {
    let mut result = fetch_batch();
    let page = result.prs[0].pages().remove(0);
    read_page(&mut result.prs[0], &page, &data(FAILING_CHECKS)["l0"]).unwrap();
    let prs: Vec<PrStatus> = result.prs.into_iter().map(|p| p.finish(now())).collect();
    assert_eq!(codes(&prs[0]), ["conflict", "checks_failing", "draft"]);
    assert_eq!(
        prs[0].attention[0].message,
        "Conflicts with main; pull_request workflows will not run until it is rebased"
    );
    assert_eq!(
        prs[0].attention[1].message,
        "3 failing: untrusted / lint, untrusted / test-ci, test-ci"
    );
    assert_eq!(codes(&prs[1]), ["conflict", "unresolved_threads", "draft"]);
    assert_eq!(prs[1].attention[1].message, "1 unresolved review thread");
    // Merged and closed PRs need nothing.
    assert_eq!(prs[2].state, State::Merged);
    assert!(prs[2].attention.is_empty());
}

#[test]
fn each_attention_code() {
    let base = fetch_batch().prs.remove(3).finish(now());
    assert_eq!(base.state, State::Open);
    let derive = |change: &dyn Fn(&mut PrStatus)| {
        let mut pr = base.clone();
        pr.mergeable = Mergeability::Mergeable;
        change(&mut pr);
        attention(&pr, now())
    };
    assert!(derive(&|_| {}).is_empty());

    // `now()` is 15:00.
    let no_checks = |pushed_at: &'static str| {
        derive(&move |pr: &mut PrStatus| {
            pr.checks = Checks::default();
            pr.pushed_at = Some(pushed_at.into());
        })
    };
    assert!(no_checks("2026-10-30T14:56:00Z").is_empty());
    let missing = no_checks("2026-10-30T14:48:00Z");
    assert_eq!(missing[0].code, "checks_missing");
    assert_eq!(
        missing[0].message,
        "No checks registered 12m after the last push"
    );

    let review = derive(&|pr| pr.review = Some("changes_requested".into()));
    assert_eq!(review[0].code, "changes_requested");
    assert!(derive(&|pr| pr.review = Some("approved".into())).is_empty());

    let threads = derive(&|pr| pr.unresolved_threads = 3);
    assert_eq!(threads[0].message, "3 unresolved review threads");

    let behind = derive(&|pr| pr.merge_state = "behind".into());
    assert_eq!(behind[0].code, "behind_base");
    assert_eq!(
        behind[0].message,
        "Behind main; update the branch before it can merge"
    );

    let unknown = derive(&|pr| pr.mergeable = Mergeability::Unknown);
    assert_eq!(unknown[0].code, "mergeable_unknown");

    let closed = derive(&|pr| {
        pr.state = State::Closed;
        pr.mergeable = Mergeability::Conflicting;
    });
    assert!(closed.is_empty());
}

#[test]
fn json_leaves_out_empty_fields() {
    let pr = fetch_batch().prs.remove(2).finish(now());
    assert_eq!(
        serde_json::to_string(&pr).unwrap(),
        concat!(
            r#"{"repo":"jdx/mise","number":15187,"#,
            r#""title":"fix(dotfiles): keep saving a directory tracked in place of narrower paths","#,
            r#""url":"https://github.com/jdx/mise/pull/15187","state":"merged","base":"main","#,
            r#""head":"claude/github-issue-discussion-e01ea6","head_sha":"e1d9305884","#,
            r#""pushed_at":"2026-10-30T03:40:39Z","mergeable":"unknown","merge_state":"unknown","#,
            r#""checks":{"pass":26,"skipped":22}}"#
        )
    );
}

#[test]
fn search_pages_are_followed() {
    let lookups = [Lookup::Search {
        query: "is:pr is:open repo:jdx/jouet".into(),
        after: None,
        listed: 0,
    }];
    let mut result = FetchResult::default();
    let more = read_prs(&lookups, &data(SEARCH), &mut result).unwrap();
    assert!(more.is_empty());
    assert_eq!(result.prs.len(), 4);
    assert!(result.incomplete.is_empty());

    let mut page = data(SEARCH);
    page["l0"]["pageInfo"] = json!({ "hasNextPage": true, "endCursor": "Y3Vyc29yOjI=" });
    let mut result = FetchResult::default();
    let more = read_prs(&lookups, &page, &mut result).unwrap();
    assert_eq!(
        more,
        [Lookup::Search {
            query: "is:pr is:open repo:jdx/jouet".into(),
            after: Some("Y3Vyc29yOjI=".into()),
            listed: 4,
        }]
    );
}

#[test]
fn a_search_past_githubs_listing_limit_is_reported() {
    // GitHub lists at most 1,000 results; issueCount says how many matched.
    let lookups = [Lookup::Search {
        query: "is:pr is:merged repo:jdx/mise".into(),
        after: Some("Y3Vyc29yOjk5Ng==".into()),
        listed: 996,
    }];
    let mut page = data(SEARCH);
    page["l0"]["issueCount"] = json!(1234);
    let mut result = FetchResult::default();
    let more = read_prs(&lookups, &page, &mut result).unwrap();
    assert!(more.is_empty());
    assert_eq!(
        result.incomplete,
        [Incomplete {
            query: "is:pr is:merged repo:jdx/mise".into(),
            matched: 1234,
            listed: 1000,
        }]
    );
}

#[test]
fn review_threads_past_the_first_page_are_counted() {
    let mut pr = data(BATCH)["l1"]["pullRequest"].clone();
    pr["reviewThreads"]["pageInfo"] = json!({ "hasNextPage": true, "endCursor": "Y3Vyc29yOjE=" });
    let mut fetched = read_pr(&pr).unwrap();
    assert_eq!(fetched.status.unresolved_threads, 1);
    let pages = fetched.pages();
    assert_eq!(
        pages,
        [Lookup::Threads {
            id: "PR_kwDOIvux3s8AAAABLeaPDA".into(),
            after: "Y3Vyc29yOjE=".into()
        }]
    );
    let page = json!({ "reviewThreads": {
        "pageInfo": { "hasNextPage": false, "endCursor": "Y3Vyc29yOjM=" },
        "nodes": [{ "isResolved": false }, { "isResolved": true }, { "isResolved": false }]
    }});
    read_page(&mut fetched, &pages[0], &page).unwrap();
    assert_eq!(fetched.status.unresolved_threads, 3);
    assert!(fetched.pages().is_empty());
}

#[test]
fn a_branch_prefers_its_open_pr() {
    let open = data(BATCH)["l0"]["pullRequest"].clone();
    let merged = data(BATCH)["l2"]["pullRequest"].clone();
    let lookups = [Lookup::Pr(Target::Branch(repo("jdx/mise"), "b".into()))];
    let read = |open: Vec<Value>, newest: Value| {
        let entry = json!({ "l0": {
            "open": { "nodes": open },
            "newest": { "nodes": [newest] },
        }});
        let mut result = FetchResult::default();
        read_prs(&lookups, &entry, &mut result).unwrap();
        result.prs[0].status.number
    };
    assert_eq!(read(vec![open], merged.clone()), 14955);
    assert_eq!(read(vec![], merged), 15187);
}

#[test]
fn queries_declare_only_the_fragments_they_use() {
    let (q, vars) = query(&batch_lookups());
    assert!(
        q.starts_with("query($o0: String!, $r0: String!, $n0: Int!,"),
        "{q}"
    );
    assert!(q.contains("fragment pr on PullRequest"));
    assert!(q.contains("fragment threads on"));
    assert_eq!(vars["n3"], json!(99_999_999));
    assert_eq!(vars["b5"], json!("claude/no-such-branch"));

    let (q, _) = query(&[Lookup::Threads {
        id: "PR_x".into(),
        after: "c".into(),
    }]);
    assert!(!q.contains("fragment pr "), "{q}");
    assert!(q.contains("fragment threads on"), "{q}");
}
