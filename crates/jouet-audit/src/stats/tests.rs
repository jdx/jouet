use super::*;

/// The fixture ledger the bats tests also read.
fn ledger() -> Ledger {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/home/state/jouet/ledger");
    Ledger::read(&dir).unwrap().expect("the fixture ledger")
}

fn kind(kind: Kind, runs: u64, requests: u64, input_tokens: u64) -> KindBaseline {
    KindBaseline {
        kind,
        runs,
        requests,
        input_tokens,
        calls: requests,
        failed: 0,
        linked_waits: false,
    }
}

#[test]
fn counts_commands_rules_checks_and_what_came_after_a_deny() {
    let l = ledger();
    assert_eq!((l.files, l.bad_lines, l.entries.len()), (2, 1, 14));
    let m = measure(&l.entries, &StatsFilter::default());
    assert_eq!(m.sessions, 2);
    let runs: Vec<(&str, &str, u64, u64)> = m
        .commands
        .iter()
        .map(|c| (c.harness.as_str(), c.command.as_str(), c.runs, c.failed))
        .collect();
    assert_eq!(
        runs,
        [
            ("claude", "jouet pr status", 1, 0),
            ("claude", "jouet wait ci", 1, 1),
            ("codex", "jouet edit", 1, 0),
            ("codex", "jouet pr threads", 1, 0),
        ]
    );
    let rule = |name: &str| m.rules.iter().find(|r| r.rule == name).unwrap();
    let sleep = rule("sleep-poll");
    assert_eq!(
        (sleep.denies, sleep.then_jouet, sleep.then_other),
        (1, 1, 0)
    );
    let edit = rule("python-edit");
    assert_eq!((edit.denies, edit.overrides, edit.then_other), (1, 1, 1));
    assert_eq!(rule("review-threads").warns, 1);
    assert_eq!(m.checks.len(), 1);
    assert_eq!(
        (
            m.checks[0].kind.as_str(),
            m.checks[0].runs,
            m.checks[0].failed
        ),
        ("test", 1, 1)
    );
    assert_eq!((m.verify_blocks, m.failure_excerpts), (1, 1));
}

#[test]
fn filters_by_time_harness_and_session() {
    let l = ledger();
    let codex = measure(
        &l.entries,
        &StatsFilter {
            harness: Some("codex".into()),
            ..StatsFilter::default()
        },
    );
    assert_eq!((codex.entries, codex.commands.len()), (3, 2));
    let late = measure(
        &l.entries,
        &StatsFilter {
            since: Some(1_793_404_800 + 330),
            session: Some("s1".into()),
            ..StatsFilter::default()
        },
    );
    assert_eq!(late.entries, 5);
    assert_eq!(late.commands[0].command, "jouet pr status");
}

#[test]
fn finds_jouet_in_any_pipeline_stage() {
    assert_eq!(
        jouet_commands("jouet wait ci <arg> --all"),
        ["jouet wait ci"]
    );
    assert_eq!(
        jouet_commands("cd <arg> && jouet pr threads reply <arg> | jq <arg>"),
        ["jouet pr threads reply"]
    );
    assert!(jouet_commands("gh pr checks <arg>").is_empty());
}

#[test]
fn estimates_against_the_harness_baseline_and_shows_the_basis() {
    let m = measure(&ledger().entries, &StatsFilter::default());
    let baselines = [Baseline {
        harness: "claude".into(),
        until: Some(1_793_404_800),
        sessions: 40,
        requests_with_tokens: 900,
        input_tokens: 90_000_000,
        kinds: vec![
            // 25 CI waits of 9 polls each at 100k tokens per request.
            kind(Kind::CiWait, 25, 225, 22_500_000),
            kind(Kind::PrStatus, 4, 8, 800_000),
        ],
    }];
    let est = estimate(&m, &baselines);
    let get = |harness: &str, k: Kind| {
        est.iter()
            .find(|e| e.harness == harness && e.kind == k)
            .unwrap()
    };
    let ci = get("claude", Kind::CiWait);
    assert_eq!(ci.runs, 1);
    assert_eq!(ci.requests_avoided, Some(8.0));
    assert_eq!(ci.input_tokens, Some(800_000));
    assert_eq!(
        ci.basis,
        "25 CI waits before setup, averaging 9.0 requests each at 100k tokens per request"
    );
    let status = get("claude", Kind::PrStatus);
    assert_eq!(status.input_tokens, None);
    assert_eq!(
        status.refused.as_deref(),
        Some("only 4 PR status checks before setup, and an estimate needs 20")
    );
    // Codex ran jouet but has no baseline at all: no guess.
    assert_eq!(
        get("codex", Kind::Edit).refused.as_deref(),
        Some("no codex history from before setup")
    );
}

#[test]
fn baselines_round_trip_through_json_for_the_stop_hook() {
    let b = Baseline {
        harness: "codex".into(),
        until: None,
        sessions: 3,
        requests_with_tokens: 9,
        input_tokens: 900,
        kinds: vec![kind(Kind::ProcessWait, 2, 4, 1000)],
    };
    let json = serde_json::to_string(&[&b]).unwrap();
    let back: Vec<Baseline> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, [b]);
}

#[test]
fn legacy_process_wait_baselines_cannot_claim_savings() {
    let old: Baseline = serde_json::from_str(r#"{"harness":"codex","sessions":30,"requests_with_tokens":100,"kinds":[{"kind":"process_wait","runs":25,"requests":100,"input_tokens":100000}]}"#).unwrap();
    let estimate = per_run(&[old]);
    let wait = estimate
        .iter()
        .find(|e| e.kind == Kind::ProcessWait)
        .unwrap();
    assert_eq!(wait.input_tokens, None);
    assert!(
        wait.refused
            .as_deref()
            .unwrap()
            .contains("no linked job evidence")
    );
}

#[test]
fn linked_process_wait_baselines_can_be_used_after_enough_opportunities() {
    let mut k = kind(Kind::ProcessWait, 25, 75, 7_500);
    k.linked_waits = true;
    let baseline = Baseline {
        harness: "codex".into(),
        sessions: 25,
        requests_with_tokens: 75,
        kinds: vec![k],
        ..Default::default()
    };
    let estimates = per_run(&[baseline]);
    let wait = estimates
        .iter()
        .find(|e| e.kind == Kind::ProcessWait)
        .unwrap();
    assert_eq!(wait.requests_avoided, Some(2.0));
    assert_eq!(wait.input_tokens, Some(200));
}

#[test]
fn baseline_uses_linked_opportunities_not_all_process_waits() {
    use crate::analyze::Analyzer;
    use crate::event::{Call, Request, Tool, Transcript, Wait};
    let wait = || {
        let mut call = Call::new(Tool::Poll);
        call.wait = Some(Wait {
            id: "job".into(),
            short: true,
            running: true,
            quiet: true,
        });
        call.duration_ms = Some(100);
        call
    };
    let transcript = Transcript {
        calls: vec![wait(), wait(), Call::new(Tool::Poll)],
        requests: vec![
            Request {
                input_tokens: Some(100),
                calls: 0..1,
                ..Default::default()
            },
            Request {
                input_tokens: Some(200),
                calls: 1..2,
                ..Default::default()
            },
            Request {
                input_tokens: Some(900),
                calls: 2..3,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut analyzer = Analyzer::default();
    analyzer.add("codex", &transcript);
    let report = analyzer.report(0);
    let b = baseline(&report.harnesses[0], None);
    let kind = b
        .kinds
        .iter()
        .find(|k| k.kind == Kind::ProcessWait)
        .unwrap();
    assert!(kind.linked_waits);
    assert_eq!((kind.runs, kind.requests, kind.input_tokens), (1, 2, 300));
    assert_eq!(kind.calls, 3);
}

/// One harness's baseline at 100k tokens per request, with `kinds`.
fn priced(harness: &str, kinds: Vec<KindBaseline>) -> Baseline {
    Baseline {
        harness: harness.into(),
        until: Some(1_793_404_800),
        sessions: 40,
        requests_with_tokens: 900,
        input_tokens: 90_000_000,
        kinds,
    }
}

fn post(shape: &str, exit_code: i32) -> LedgerEntry {
    LedgerEntry {
        harness: "claude".into(),
        session: Some("s".into()),
        event: "post-tool-use".into(),
        shape: Some(shape.into()),
        exit_code: Some(exit_code),
        ..LedgerEntry::default()
    }
}

#[test]
fn counts_what_jouet_added_per_harness() {
    let m = measure(&ledger().entries, &StatsFilter::default());
    assert_eq!(
        m.overhead,
        [
            Overhead {
                harness: "claude".into(),
                stop_blocks: 1,
                denials: 2,
                ..Overhead::default()
            },
            Overhead {
                harness: "codex".into(),
                warnings: 1,
                ..Overhead::default()
            },
        ]
    );
}

#[test]
fn a_failed_jouet_run_costs_unless_it_was_a_checks_answer() {
    let entries = [
        // A failing test or CI run fails without jouet too.
        post(
            "jouet test --affected && jouet commit <arg> --message <arg>",
            1,
        ),
        post("jouet wait ci <arg>", 1),
        post("jouet pr next", 20),
        // `jouet edit` is compared with edits before setup instead.
        post("jouet edit replace <arg> --old <arg> --new <arg>", 1),
        // A usage error, a missing thread, a refused commit.
        post("jouet commit <arg> --message <arg>", 2),
        post("jouet pr threads reply <arg> --body <arg>", 3),
        post("jouet test --affected", 4),
        post("cargo test", 101),
        // It may have failed on cargo, not jouet.
        post("cargo build && jouet commit <arg> --message <arg>", 101),
    ];
    let m = measure(&entries, &StatsFilter::default());
    assert_eq!(m.overhead[0].failed_runs, 3);
    let failed = |name: &str| {
        m.commands
            .iter()
            .filter(|c| c.command == name)
            .map(|c| c.failed)
            .sum::<u64>()
    };
    // Calls that failed: the test failed, not the commit around it, but
    // the cargo build and the commit cannot be told apart.
    assert_eq!(failed("jouet test"), 2);
    assert_eq!(failed("jouet commit"), 2);
    assert_eq!(failed("jouet edit replace"), 1);
}

#[test]
fn costs_are_priced_like_savings_and_the_net_may_be_negative() {
    let m = measure(
        &ledger().entries,
        &StatsFilter {
            harness: Some("claude".into()),
            ..StatsFilter::default()
        },
    );
    let baselines = [priced(
        "claude",
        vec![kind(Kind::CiWait, 25, 225, 22_500_000)],
    )];
    let saved = estimate(&m, &baselines);
    let after = |requests, primer_tokens| {
        [AfterSetup {
            harness: "claude".into(),
            requests,
            primer_tokens,
        }]
    };
    let c = costs(&m, &baselines, &after(50, 500));
    let got: Vec<(CostKind, u64, Option<u64>)> = c
        .iter()
        .map(|c| (c.kind, c.count, c.input_tokens))
        .collect();
    assert_eq!(
        got,
        [
            (CostKind::StopBlocks, 1, Some(100_000)),
            (CostKind::Denials, 2, Some(200_000)),
            (CostKind::Primer, 50, Some(25_000)),
        ]
    );
    assert_eq!(
        c[0].basis,
        "each is at least one more request, at 100k tokens per request (the average before setup)"
    );
    assert_eq!(
        c[2].basis,
        "50 requests in the history after setup, subagents' included, each reading the primer's ~500 tokens"
    );
    // The CI wait saved 800k; the costs come to 325k.
    assert_eq!(net(&saved, &c), Some(475_000));
    // A long history after setup: the primer alone outweighs the saving.
    let c = costs(&m, &baselines, &after(10_000, 500));
    assert_eq!(net(&saved, &c), Some(800_000 - 300_000 - 5_000_000));
    // Without the primer's requests, the net is unknown rather than flattering.
    let c = costs(&m, &baselines, &[]);
    assert_eq!(
        c.last().unwrap().refused.as_deref(),
        Some("no claude history after setup was read")
    );
    assert_eq!(net(&saved, &c), None);
    // With the primer off, it is no cost.
    assert!(
        costs(&m, &baselines, &after(50, 0))
            .iter()
            .all(|c| c.kind != CostKind::Primer)
    );
}

#[test]
fn an_unpriced_harness_leaves_the_net_unknown_and_warnings_are_counted_not_priced() {
    let m = measure(&ledger().entries, &StatsFilter::default());
    let baselines = [priced("claude", Vec::new())];
    let c = costs(&m, &baselines, &[]);
    let codex: Vec<&Cost> = c.iter().filter(|c| c.harness == "codex").collect();
    assert_eq!(codex.len(), 2);
    assert_eq!((codex[0].kind, codex[0].count), (CostKind::Warnings, 1));
    assert_eq!(codex[1].kind, CostKind::Primer);
    // Codex has no history before setup: its costs are left out of the net
    // rather than blanking it.
    assert!(codex.iter().all(|c| c.outside_net));
    let claude_read = [AfterSetup {
        harness: "claude".into(),
        requests: 0,
        primer_tokens: 0,
    }];
    assert_eq!(
        net(&[], &costs(&m, &baselines, &claude_read)),
        Some(-300_000)
    );
    assert!(codex[0].refused.as_deref().unwrap().contains("not priced"));
    // Warnings alone do not hide the net, but the primer's unread
    // requests do; with them read, the claude costs are what is left.
    assert_eq!(net(&[], &c), None);
    let read: Vec<AfterSetup> = ["claude", "codex"]
        .map(|harness| AfterSetup {
            harness: harness.into(),
            requests: 0,
            primer_tokens: 0,
        })
        .into();
    assert_eq!(net(&[], &costs(&m, &baselines, &read)), Some(-300_000));
}

#[test]
fn jouet_edit_is_credited_only_where_it_fails_less_than_edits_did() {
    let edits = |failed| Measured {
        commands: vec![CommandRuns {
            command: "jouet edit replace".into(),
            harness: "claude".into(),
            runs: 100,
            // The calls' exit codes, which mix in other failures, are not used.
            failed: 50,
            own_runs: 100,
            own_failed: failed,
            ..CommandRuns::default()
        }],
        ..Measured::default()
    };
    // Before setup, 34 of 1,000 edits failed: 3.4%.
    let mut k = kind(Kind::Edit, 1000, 1000, 100_000_000);
    k.failed = 34;
    let baselines = [priced("claude", vec![k])];

    // jouet edit failed 9%: no saving, and the 5.6 extra failures cost.
    let m = edits(9);
    let saved = estimate(&m, &baselines);
    assert_eq!(saved[0].input_tokens, Some(0));
    assert_eq!(
        saved[0].basis,
        "assuming each failed edit was redone: 1,000 edits before setup, 3.4% of which failed; jouet edit failed 9.0% of the 100 runs that recorded their own exit code (9), so nothing is credited"
    );
    let read = [AfterSetup {
        harness: "claude".into(),
        requests: 0,
        primer_tokens: 0,
    }];
    let c = costs(&m, &baselines, &read);
    assert_eq!(c.len(), 1);
    assert_eq!(
        (c[0].kind, c[0].count, c[0].input_tokens),
        (CostKind::EditFailures, 6, Some(560_000))
    );
    assert_eq!(net(&saved, &c), Some(-560_000));

    // jouet edit failed 1%: credited the 2.4 points it fails less.
    let m = edits(1);
    let saved = estimate(&m, &baselines);
    assert_eq!(saved[0].input_tokens, Some(240_000));
    assert!(costs(&m, &baselines, &read).is_empty());

    // Without a ledger, nothing has measured jouet edit's failures.
    let per = per_run(&baselines);
    let edit = per.iter().find(|e| e.kind == Kind::Edit).unwrap();
    assert_eq!(edit.input_tokens, None);
    assert!(edit.refused.is_some());
}

#[test]
fn jouet_runs_own_exit_codes_decide_what_jouet_failed() {
    let run = |shape: &str, code| LedgerEntry {
        harness: "jouet".into(),
        event: "jouet-run".into(),
        ..post(shape, code)
    };
    let entries = [
        // The call failed on its tests; jouet edit and jouet commit did not.
        post(
            "jouet edit replace <arg> && jouet commit <arg> && cargo test",
            101,
        ),
        run("jouet edit replace", 0),
        run("jouet commit", 0),
        // Here jouet failed on its own.
        post("jouet edit replace <arg> && cargo fmt", 3),
        run("jouet edit replace", 3),
        post("jouet pr threads <arg> | jq <arg>", 5),
        run("jouet pr threads", 5),
        // Waiting is pr next's answer, not a failure.
        post("jouet pr next <arg>", 20),
        run("jouet pr next", 20),
        // A session the hooks never saw has no harness, so it is left out.
        LedgerEntry {
            session: Some("elsewhere".into()),
            ..run("jouet commit", 2)
        },
    ];
    let m = measure(&entries, &StatsFilter::default());
    let edit = m
        .commands
        .iter()
        .find(|c| c.command == "jouet edit replace")
        .unwrap();
    assert_eq!(
        (
            edit.harness.as_str(),
            edit.runs,
            edit.failed,
            edit.own_runs,
            edit.own_failed
        ),
        ("claude", 2, 2, 2, 1)
    );
    assert_eq!(m.overhead.len(), 1);
    assert_eq!(m.overhead[0].failed_runs, 1);
}
