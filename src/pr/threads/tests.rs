//! Review threads read from GraphQL and REST responses recorded from
//! github.com (jdx/mise#14922, a PR from a fork), and the write rules
//! checked against them.

use super::write::{self, Entry, Options, PlanError, parse_apply, plan};
use super::*;
use crate::exit::Exit;

const PR: &str = include_str!("fixtures/pr.json");
const COMMITS: &str = include_str!("fixtures/commits.json");
const ACTIVITY: &str = include_str!("fixtures/activity.json");

const HEAD: &str = "c42414f907109526b406d2b72bef0bfe5cf07209";
/// Unresolved, by coderabbitai: src/hook_env.rs:515.
const OPEN_CODE: u64 = 4_385_981_140;
/// Unresolved, by coderabbitai: docs/ide-integration.md:139.
const OPEN_DOCS: u64 = 4_386_056_098;
/// Resolved and outdated, by greptile-apps.
const RESOLVED_OUTDATED: u64 = 4_361_752_342;

fn pr() -> PrThreads {
    let data: Value = serde_json::from_str(PR).unwrap();
    let repo = Repo::parse("jdx/mise").unwrap();
    let mut pages = Pages::default();
    let pr = read(&repo, &data["data"], &[], &mut pages)
        .unwrap()
        .unwrap();
    assert!(pages.threads.is_none() && pages.thread_comments.is_empty());
    pr
}

fn filter() -> Filter {
    Filter {
        status: Status::Unresolved,
        outdated: None,
        authors: Vec::new(),
        bots: None,
        since: None,
        paths: Vec::new(),
    }
}

/// The review threads (not reviews or comments) a filter selects.
fn selected(pr: &PrThreads, f: Filter) -> Vec<u64> {
    select(pr, &f)
        .into_iter()
        .filter(|t| t.kind == Kind::Thread)
        .map(|t| t.id)
        .collect()
}

#[test]
fn a_recorded_pr_reads_into_threads() {
    let pr = pr();
    assert_eq!((pr.number, pr.viewer.as_str()), (14922, "jdx"));
    assert_eq!(pr.head_oid, HEAD);
    assert_eq!(pr.head_ref, "feat/hook-env-json");
    assert_eq!(pr.head_repo, Repo::parse("refractalize/mise"));
    assert_eq!(pr.threads.len(), 5);
    let t = pr.threads.iter().find(|t| t.id == OPEN_CODE).unwrap();
    assert_eq!(t.node_id, "PRRT_kwDOIvux3s6wlNzt");
    assert_eq!(
        (t.path.as_deref(), t.line),
        (Some("src/hook_env.rs"), Some(515))
    );
    // GraphQL says `coderabbitai`; the [bot] suffix is what people filter by.
    assert_eq!(t.author, "coderabbitai[bot]");
    assert!(t.is_bot && !t.resolved && !t.outdated);
    assert_eq!(
        t.url,
        format!("https://github.com/jdx/mise/pull/14922#discussion_r{OPEN_CODE}")
    );
    let outdated = pr
        .threads
        .iter()
        .find(|t| t.id == RESOLVED_OUTDATED)
        .unwrap();
    assert!(outdated.outdated && outdated.resolved && outdated.line.is_none());
    // Reviews without a summary are left out.
    let kinds: Vec<Kind> = pr.others.iter().map(|t| t.kind).collect();
    assert_eq!(kinds.len(), 5);
    assert!(
        pr.others
            .iter()
            .all(|t| !t.comments[0].body.trim().is_empty())
    );
}

#[test]
fn threads_keep_their_side_of_the_diff() {
    assert!(pr().threads.iter().all(|t| t.side == Side::Right));
    // A comment on lines 4-6 of the base version, and one whose range starts
    // on the base side and ends on the head's.
    let mut data: Value = serde_json::from_str(PR).unwrap();
    let nodes = &mut data["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"];
    for (i, start_side) in [(0, "LEFT"), (1, "LEFT")] {
        let side = if i == 0 { "LEFT" } else { "RIGHT" };
        let node = &mut nodes[i];
        node["diffSide"] = side.into();
        node["startDiffSide"] = start_side.into();
        node["line"] = 6.into();
        node["startLine"] = 4.into();
    }
    let repo = Repo::parse("jdx/mise").unwrap();
    let pr = read(&repo, &data["data"], &[], &mut Pages::default())
        .unwrap()
        .unwrap();
    let (left, mixed) = (&pr.threads[0], &pr.threads[1]);
    assert_eq!(
        (left.side, left.start_line, left.line),
        (Side::Left, Some(4), Some(6))
    );
    assert_eq!(serde_json::to_value(left).unwrap()["side"], "left");
    // Line 4 is in the other file, so the range has no first line here.
    assert_eq!((mixed.side, mixed.start_line), (Side::Right, None));
    assert!(serde_json::to_value(mixed).unwrap().get("side").is_none());
}

#[test]
fn counts_cover_every_thread() {
    assert_eq!(
        counts(&pr()),
        Counts {
            unresolved: 2,
            resolved: 3,
            outdated: 0
        }
    );
}

#[test]
fn filters_select_threads() {
    let pr = pr();
    let select = |f: Filter| selected(&pr, f);
    assert_eq!(select(filter()), [OPEN_CODE, OPEN_DOCS]);
    assert_eq!(
        select(Filter {
            status: Status::All,
            ..filter()
        })
        .len(),
        5
    );
    let resolved = select(Filter {
        status: Status::Resolved,
        ..filter()
    });
    assert_eq!(resolved.len(), 3);
    assert_eq!(
        select(Filter {
            status: Status::All,
            outdated: Some(true),
            ..filter()
        }),
        [RESOLVED_OUTDATED]
    );
    for author in ["coderabbitai", "CodeRabbitAI[bot]"] {
        let f = Filter {
            status: Status::All,
            authors: vec![author.into()],
            ..filter()
        };
        assert_eq!(select(f), [OPEN_CODE, OPEN_DOCS], "{author}");
    }
    assert!(
        select(Filter {
            bots: Some(false),
            ..filter()
        })
        .is_empty()
    );
    for path in [
        "docs",
        "docs/",
        "*.md",
        "docs/**",
        "**/*.md",
        "docs/ide-*.md",
    ] {
        let f = Filter {
            paths: vec![path.into()],
            ..filter()
        };
        assert_eq!(select(f), [OPEN_DOCS], "{path}");
    }
}

#[test]
fn since_goes_by_when_comments_were_posted() {
    let pr = pr();
    let at = |s: &str| Some(time::parse_utc(s).unwrap());
    // Both open threads were posted on the same commit, an hour apart.
    let select = |since| selected(&pr, Filter { since, ..filter() });
    let code = pr
        .threads
        .iter()
        .find(|t| t.id == OPEN_CODE)
        .unwrap()
        .comments[0]
        .created;
    let docs = pr
        .threads
        .iter()
        .find(|t| t.id == OPEN_DOCS)
        .unwrap()
        .comments[0]
        .created;
    assert!(code < docs);
    assert_eq!(select(Some(code)), [OPEN_CODE, OPEN_DOCS]);
    assert_eq!(select(Some(code + 1)), [OPEN_DOCS]);
    assert!(select(at("2030-01-01T00:00:00Z")).is_empty());
    // Reviews and comments are cut by the same window.
    let others = |since| {
        super::select(&pr, &Filter { since, ..filter() })
            .iter()
            .filter(|t| t.kind != Kind::Thread)
            .count()
    };
    assert_eq!(others(None), 5);
    assert_eq!(others(Some(docs + 1)), 0);
}

#[test]
fn globs() {
    for (pattern, path, matches) in [
        ("*.rs", "src/main.rs", true),
        ("src/*.rs", "src/main.rs", true),
        ("src/*.rs", "src/cmd/pr.rs", false),
        ("src/**/*.rs", "src/main.rs", true),
        ("src/**/*.rs", "src/cmd/pr.rs", true),
        ("src/**", "src/cmd/pr.rs", true),
        ("src/c?d/*", "src/cmd/pr.rs", true),
        ("src/cmd", "src/cmd/pr.rs", true),
        ("src/cm", "src/cmd/pr.rs", false),
        ("pr.rs", "src/cmd/pr.rs", true),
        ("main.rs", "src/cmd/pr.rs", false),
    ] {
        assert_eq!(path_matches(pattern, path), matches, "{pattern} {path}");
    }
}

#[test]
fn a_push_is_found_in_the_activity_log() {
    let activity: Vec<Activity> = serde_json::from_str(ACTIVITY).unwrap();
    let commits: Value = serde_json::from_str(COMMITS).unwrap();
    let oids: Vec<&str> = commits["data"]["node"]["commits"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["commit"]["oid"].as_str().unwrap())
        .collect();
    assert_eq!(oids, [HEAD]);
    // Force-pushed at 07:18:55, a quarter of an hour after its commit time.
    assert_eq!(
        push_time(&activity, &oids, HEAD),
        time::parse_utc("2026-10-27T07:18:55Z")
    );
    assert_eq!(push_time(&activity, &oids, "0123"), None);
}

#[test]
fn a_commit_pushed_under_a_later_one_takes_that_push_time() {
    let push = |after: &str, timestamp: &str| Activity {
        after: after.into(),
        timestamp: timestamp.into(),
    };
    // a and b were pushed together (head b), then c.
    let activity = [
        push("c", "2026-10-30T12:00:00Z"),
        push("b", "2026-10-30T11:00:00Z"),
    ];
    let commits = ["a", "b", "c"];
    let at = |s| time::parse_utc(s);
    assert_eq!(
        push_time(&activity, &commits, "a"),
        at("2026-10-30T11:00:00Z")
    );
    assert_eq!(
        push_time(&activity, &commits, "c"),
        at("2026-10-30T12:00:00Z")
    );
}

#[test]
fn shas_must_name_one_commit() {
    let commits = [("abcd1234".to_string(), 1), ("abcd9999".to_string(), 2)];
    // A head from before a force-push is still a push to the PR.
    let activity = [Activity {
        after: "beef5678".into(),
        timestamp: "2026-10-30T11:00:00Z".into(),
    }];
    let find = |sha| find_commit(&commits, &activity, sha);
    assert_eq!(find("abcd12"), Ok("abcd1234".into()));
    assert_eq!(find("beef"), Ok("beef5678".into()));
    assert!(matches!(find("abcd"), Err(Lookup::Usage(_))));
    assert!(matches!(find("cafe"), Err(Lookup::NotFound(_))));
    assert_eq!(
        push_time(&activity, &["abcd1234"], "beef5678"),
        time::parse_utc("2026-10-30T11:00:00Z")
    );
}

fn options() -> Options {
    Options {
        footer: None,
        sha: Some("abc1234".into()),
        ignore_resolved: false,
        retry_safe: false,
    }
}

fn entry(id: u64, reply: Option<&str>, resolve: bool) -> Entry {
    Entry {
        id,
        reply: reply.map(str::to_string),
        resolve,
        unresolve: false,
    }
}

fn plan_error(entries: &[Entry], o: &Options) -> PlanError {
    plan(entries, &pr(), o).unwrap_err()
}

#[test]
fn unknown_ids_fail_the_whole_call() {
    let entries = [entry(OPEN_CODE, None, true), entry(1, None, true)];
    let e = plan_error(&entries, &options());
    assert_eq!(e.exit, Exit::NotFound);
    assert!(
        e.message.contains("thread 1 on jdx/mise#14922"),
        "{}",
        e.message
    );
    // A reply's ID is not a handle; say which thread it is in.
    let mut pr = pr();
    let thread = pr.threads[0].id;
    let reply = Comment {
        id: 7,
        ..pr.threads[0].comments[0].clone()
    };
    pr.threads[0].comments.push(reply);
    let e = plan(&[entry(7, None, true)], &pr, &options()).unwrap_err();
    assert!(
        e.message
            .contains(&format!("7 (a reply in thread {thread})")),
        "{}",
        e.message
    );
}

#[test]
fn resolved_threads_fail_unless_skipped() {
    let entries = [
        entry(OPEN_CODE, Some("done"), true),
        entry(RESOLVED_OUTDATED, None, true),
    ];
    let e = plan_error(&entries, &options());
    assert_eq!(e.exit, Exit::Precondition);
    assert!(
        e.message.contains(&RESOLVED_OUTDATED.to_string()),
        "{}",
        e.message
    );
    assert!(e.hint.unwrap().contains("--ignore-resolved"));
    let o = Options {
        ignore_resolved: true,
        ..options()
    };
    let out = plan(&entries, &pr(), &o).unwrap();
    assert_eq!(out[1].skipped.as_deref(), Some("already resolved"));
    assert!(out[0].skipped.is_none());
    // A reply into a thread someone resolved is refused even by apply.
    let reply = [entry(RESOLVED_OUTDATED, Some("late"), true)];
    let retry = Options {
        retry_safe: true,
        ..options()
    };
    assert_eq!(plan_error(&reply, &retry).exit, Exit::Precondition);
    // But a resolve that already happened is done.
    let out = plan(&entries[1..], &pr(), &retry).unwrap();
    assert_eq!(out[0].skipped.as_deref(), Some("already resolved"));
}

#[test]
fn replies_expand_sha_and_take_the_footer() {
    let o = Options {
        footer: Some("*AI-assisted*".into()),
        ..options()
    };
    let mut out = plan(
        &[entry(OPEN_CODE, Some("Fixed in {sha}.\n"), true)],
        &pr(),
        &o,
    )
    .unwrap();
    assert!(out[0].posts_sha());
    write::dry_run(&mut out);
    assert_eq!(
        out[0].body.as_deref(),
        Some("Fixed in abc1234.\n\n*AI-assisted*")
    );
    assert!(out[0].replied && out[0].resolved);
    let no_sha = Options {
        sha: None,
        ..options()
    };
    assert_eq!(
        plan_error(&[entry(OPEN_CODE, Some("in {sha}"), false)], &no_sha).exit,
        Exit::Precondition
    );
}

#[test]
fn a_reply_already_posted_is_not_posted_again() {
    let mut pr = pr();
    let first = pr.threads.iter().position(|t| t.id == OPEN_CODE).unwrap();
    let mut mine = pr.threads[first].comments[0].clone();
    mine.id = 9;
    mine.author = "jdx".into();
    mine.body = "Fixed in abc1234.\r\n".into();
    mine.url = "https://github.com/jdx/mise/pull/14922#discussion_r9".into();
    pr.threads[first].comments.push(mine);
    let out = plan(
        &[entry(OPEN_CODE, Some("Fixed in {sha}."), false)],
        &pr,
        &options(),
    )
    .unwrap();
    assert_eq!(out[0].skipped.as_deref(), Some("reply already posted"));
    assert!(out[0].already_replied);
    assert_eq!(
        out[0].url.as_deref(),
        Some("https://github.com/jdx/mise/pull/14922#discussion_r9")
    );
    // With a resolve, only the resolve is left to do.
    let mut out = plan(
        &[entry(OPEN_CODE, Some("Fixed in {sha}."), true)],
        &pr,
        &options(),
    )
    .unwrap();
    assert!(!out[0].posts_sha());
    write::dry_run(&mut out);
    assert!(out[0].already_replied && out[0].resolved && !out[0].replied);
    // A different text, or someone else's comment, is a new reply.
    let out = plan(
        &[entry(OPEN_CODE, Some("Fixed elsewhere."), false)],
        &pr,
        &options(),
    )
    .unwrap();
    assert!(out[0].skipped.is_none() && !out[0].already_replied);
}

#[test]
fn unresolving_an_open_thread_is_skipped() {
    let entries = [
        Entry {
            unresolve: true,
            ..entry(OPEN_CODE, None, false)
        },
        Entry {
            unresolve: true,
            ..entry(RESOLVED_OUTDATED, None, false)
        },
    ];
    let mut out = plan(&entries, &pr(), &options()).unwrap();
    assert_eq!(out[0].skipped.as_deref(), Some("not resolved"));
    write::dry_run(&mut out);
    assert!(out[1].unresolved);
}

#[test]
fn apply_files_are_checked() {
    let ok = parse_apply(r#"[{"id": 4377519031, "reply": "x", "resolve": true}, {"id": "4377522210", "resolve": true}]"#).unwrap();
    assert_eq!(ok[1], entry(4_377_522_210, None, true));
    for (text, says) in [
        ("[]", "empty"),
        ("{}", "apply file"),
        (r#"[{"id": 1}]"#, "neither replies nor resolves"),
        (r#"[{"id": 1, "reply": "  "}]"#, "is empty"),
        (
            r#"[{"id": 1, "resolve": true}, {"id": 1, "reply": "x"}]"#,
            "twice",
        ),
        (r#"[{"id": 1, "resolved": true}]"#, "unknown field"),
        (r##"[{"id": "#1", "resolve": true}]"##, "not a thread ID"),
    ] {
        let e = parse_apply(text).unwrap_err();
        assert!(e.contains(says), "{text}: {e}");
    }
}
