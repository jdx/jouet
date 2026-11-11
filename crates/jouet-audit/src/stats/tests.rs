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
        output_bytes: 0,
        linked_waits: false,
    }
}

#[test]
fn jouet_test_saves_the_output_it_cut_counted_once() {
    let with = |shape: &str, bytes: u64| LedgerEntry {
        output_bytes: Some(bytes),
        ..post(shape, 0)
    };
    let entries = [
        with("jouet test --affected", 1_000),
        with("jouet test --cmd -- <arg>", 3_000),
        // Output a call shares with another command is not jouet test's.
        with(
            "jouet test --affected && jouet commit <arg> --message <arg>",
            90_000,
        ),
        with("cargo test && jouet test --affected", 90_000),
        // No size recorded.
        post("jouet test --affected", 0),
    ];
    let m = measure(&entries, &StatsFilter::default());
    let test = m
        .commands
        .iter()
        .find(|c| c.command == "jouet test")
        .unwrap();
    assert_eq!(
        (test.runs, test.output_calls, test.output_bytes),
        (5, 2, 4_000)
    );
    let baseline = |output_bytes: u64| Baseline {
        harness: "claude".into(),
        requests_with_tokens: 1000,
        input_tokens: 100_000_000,
        kinds: vec![KindBaseline {
            output_bytes,
            ..kind(Kind::Test, 100, 100, 10_000_000)
        }],
        ..Baseline::default()
    };
    // 100 calls printed 2 MB before: 20,000 bytes each, against 2,000.
    let e = estimate(&m, &[baseline(2_000_000)]);
    let e = e.iter().find(|e| e.kind == Kind::Test).unwrap();
    assert_eq!(e.input_tokens, Some(9_000));
    assert!(
        e.basis
            .contains("20,000 bytes of output on average; jouet test gave 2,000 over the 2 calls"),
        "{}",
        e.basis
    );
    // A baseline saved before output sizes were kept asks for a rescan.
    let e = estimate(&m, &[baseline(0)]);
    let e = e.iter().find(|e| e.kind == Kind::Test).unwrap();
    assert!(e.refused.as_deref().unwrap().contains("--rescan"));
    // So does one saved before test runs were a kind, which has no entry
    // for them: a baseline made now always has one.
    let stale = Baseline {
        kinds: Vec::new(),
        ..baseline(0)
    };
    let e = estimate(&m, &[stale]);
    let e = e.iter().find(|e| e.kind == Kind::Test).unwrap();
    assert!(e.refused.as_deref().unwrap().contains("--rescan"));
    // Printing more than before saves nothing.
    let e = estimate(&m, &[baseline(100_000)]);
    assert_eq!(
        e.iter()
            .find(|e| e.kind == Kind::Test)
            .unwrap()
            .input_tokens,
        Some(0)
    );
    let after = [AfterSetup {
        harness: "claude".into(),
        requests: 0,
        primer_tokens: 0,
    }];
    let measured_costs = costs(&m, &[baseline(100_000)], &after);
    let extra = measured_costs
        .iter()
        .find(|cost| cost.kind.label() == "extra jouet test output" && cost.input_tokens.is_some())
        .expect("larger output must be priced as a cost");
    assert_eq!((extra.count, extra.input_tokens), (2, Some(500)));
    assert_eq!(
        net(&e, &measured_costs),
        None,
        "unrecorded output remains unknown"
    );
    let complete = measure(&entries[..2], &StatsFilter::default());
    let complete_estimates = estimate(&complete, &[baseline(100_000)]);
    let complete_costs = costs(&complete, &[baseline(100_000)], &after);
    assert_eq!(net(&complete_estimates, &complete_costs), Some(-500));
    assert!(
        costs(&complete, &[baseline(2_000_000)], &after)
            .iter()
            .all(|cost| cost.kind.label() != "extra jouet test output")
    );
    let missing = costs(&complete, &[baseline(0)], &after);
    assert!(
        missing
            .iter()
            .any(|cost| cost.kind.label() == "extra jouet test output" && cost.refused.is_some())
    );
    assert_eq!(net(&estimate(&complete, &[baseline(0)]), &missing), None);
    let no_output = measure(&[post("jouet test --affected", 0)], &StatsFilter::default());
    let no_output_costs = costs(&no_output, &[baseline(100_000)], &after);
    assert_eq!(
        net(
            &estimate(&no_output, &[baseline(100_000)]),
            &no_output_costs
        ),
        None
    );
    let mut sparse = baseline(100_000);
    sparse.kinds[0].runs = MIN_BASELINE_RUNS - 1;
    let sparse_costs = costs(&complete, &[sparse.clone()], &after);
    assert_eq!(net(&estimate(&complete, &[sparse]), &sparse_costs), None);
    let no_baseline = costs(&complete, &[], &after);
    assert!(no_baseline.iter().all(|cost| cost.outside_net));
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
    assert_eq!(ci.requests_avoided, None);
    assert_eq!(ci.input_tokens, None);
    assert_eq!(
        ci.refused.as_deref(),
        Some("CI polling history does not establish linked, avoidable checks")
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

fn warned(session: &str, rule: &str) -> LedgerEntry {
    warned_by(session, rule, None)
}

fn warned_by(session: &str, rule: &str, revision: Option<&str>) -> LedgerEntry {
    LedgerEntry {
        harness: "claude".into(),
        session: Some(session.into()),
        event: "pre-tool-use".into(),
        shape: Some("cargo test | tail <arg>".into()),
        decisions: vec![LedgerDecision {
            rule: rule.into(),
            action: "warn".into(),
            revision: revision.map(str::to_string),
            scope: None,
        }],
        ..LedgerEntry::default()
    }
}

#[test]
fn counts_the_sessions_a_rule_fired_in_more_than_once() {
    let entries = [
        warned("a", "piped-test-output"),
        warned("a", "piped-test-output"),
        warned("a", "piped-test-output"),
        warned("b", "piped-test-output"),
        warned("b", "large-read"),
        warned("c", "large-read"),
    ];
    let m = measure(&entries, &StatsFilter::default());
    let counts: Vec<(&str, u64, u64, u64)> = m
        .rules
        .iter()
        .map(|r| (r.rule.as_str(), r.warns, r.sessions, r.sessions_again))
        .collect();
    assert_eq!(
        counts,
        [("piped-test-output", 4, 2, 1), ("large-read", 2, 2, 0)]
    );
}

#[test]
fn with_the_rules_loaded_now_only_their_current_revision_counts() {
    let entries = [
        // Before the ledger recorded revisions, then an older revision.
        warned_by("a", "large-read", None),
        warned_by("a", "large-read", Some("old")),
        warned_by("a", "large-read", Some("new")),
        warned_by("b", "large-read", Some("new")),
        // A rule that is no longer loaded.
        warned_by("a", "push-untested", Some("x")),
    ];
    let filter = StatsFilter {
        revisions: Some([("large-read".to_string(), "new".to_string())].into()),
        ..StatsFilter::default()
    };
    let m = measure(&entries, &filter);
    let counts: Vec<(&str, u64, u64, u64)> = m
        .rules
        .iter()
        .map(|r| (r.rule.as_str(), r.warns, r.sessions, r.sessions_again))
        .collect();
    assert_eq!(counts, [("large-read", 2, 2, 0)]);
    assert_eq!(m.older_decisions, 2);
    assert_eq!(m.rules_not_loaded, ["push-untested"]);
    // What the warnings cost counts whatever the revision.
    assert_eq!(m.overhead[0].warnings, 5);
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
fn a_stop_block_a_passing_check_followed_is_held_not_wasted() {
    let stop = |block: bool| LedgerEntry {
        harness: "claude".into(),
        session: Some("s".into()),
        event: "stop".into(),
        decisions: if block {
            vec![LedgerDecision {
                rule: "verify".into(),
                action: "block".into(),
                revision: None,
                scope: None,
            }]
        } else {
            Vec::new()
        },
        ..LedgerEntry::default()
    };
    let recorded = |exit_code: i32| LedgerEntry {
        decisions: vec![LedgerDecision {
            rule: "checks".into(),
            action: "record:test".into(),
            revision: None,
            scope: None,
        }],
        ..post("cargo test", exit_code)
    };
    let own = |exit_code: i32| LedgerEntry {
        event: "jouet-run".into(),
        ..post("jouet test --affected", exit_code)
    };
    let entries = [
        // Held: the test passed after it.
        stop(true),
        recorded(1),
        recorded(0),
        stop(false),
        // Held: jouet test passed, recording its own exit code.
        stop(true),
        own(0),
        stop(false),
        // Wasted: the session stopped again with nothing passing.
        stop(true),
        recorded(1),
        stop(false),
        // Wasted: a pass after the next stop does not count.
        stop(true),
        stop(false),
        recorded(0),
    ];
    let m = measure(&entries, &StatsFilter::default());
    assert_eq!(
        (
            m.overhead[0].stop_blocks,
            m.overhead[0].stop_blocks_resolved
        ),
        (4, 2)
    );
    let baseline = Baseline {
        harness: "claude".into(),
        requests_with_tokens: 1000,
        input_tokens: 100_000_000,
        ..Baseline::default()
    };
    let costs = costs(&m, &[baseline], &[]);
    let wasted = costs
        .iter()
        .find(|c| c.kind == CostKind::StopBlocks)
        .unwrap();
    assert_eq!((wasted.count, wasted.input_tokens), (2, Some(200_000)));
    let held = costs
        .iter()
        .find(|c| c.kind == CostKind::HeldBlocks)
        .unwrap();
    assert_eq!((held.count, held.input_tokens), (2, None));
}

#[test]
fn the_gates_reasons_are_judged_by_what_came_next() {
    let gate = |session: &str, action: &str, reasons: &[&str]| LedgerEntry {
        harness: "claude".into(),
        session: Some(session.into()),
        event: "stop".into(),
        decisions: reasons
            .iter()
            .map(|reason| LedgerDecision {
                rule: "verify".into(),
                action: action.into(),
                revision: None,
                scope: Some((*reason).into()),
            })
            .collect(),
        ..LedgerEntry::default()
    };
    let in_session = |session: &str, e: LedgerEntry| LedgerEntry {
        session: Some(session.into()),
        ..e
    };
    let edit = |session: &str| {
        in_session(
            session,
            LedgerEntry {
                event: "edit".into(),
                tool: Some("Edit".into()),
                shape: None,
                exit_code: None,
                ..post("", 0)
            },
        )
    };
    let recorded = |session: &str, exit_code: i32| {
        in_session(
            session,
            LedgerEntry {
                decisions: vec![LedgerDecision {
                    rule: "checks".into(),
                    action: "record:test".into(),
                    revision: None,
                    scope: Some("partial".into()),
                }],
                ..post("cargo test", exit_code)
            },
        )
    };
    let entries = [
        // Right: an edit came next, across the next turn's stop.
        gate("a", "would-block", &["failed-test"]),
        gate("a", "", &[]),
        edit("a"),
        recorded("a", 0),
        // Right: a check failed before any passed.
        gate("b", "would-block", &["failed-test", "changed-after-pass"]),
        recorded("b", 1),
        // Passed: a check passed with no edit before it.
        gate("c", "would-block", &["changed-after-pass"]),
        in_session("c", post("jouet edit replace <arg>", 1)),
        recorded("c", 0),
        edit("c"),
        // Open: nothing since.
        gate("d", "would-block", &["hk-partial"]),
        // A block, judged the same way and counted once per stop.
        gate("e", "block", &["failed-lint", "hk-changed"]),
        in_session("e", post("jouet edit replace <arg>", 0)),
    ];
    let m = measure(&entries, &StatsFilter::default());
    let got: Vec<(&str, &str, u64, u64, u64, u64)> = m
        .gate
        .iter()
        .map(|g| {
            (
                g.action.as_str(),
                g.reason.as_str(),
                g.stops,
                g.right,
                g.passed,
                g.open,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("would-block", "changed-after-pass", 2, 1, 1, 0),
            ("would-block", "failed-test", 2, 2, 0, 0),
            ("block", "failed-lint", 1, 1, 0, 0),
            ("block", "hk-changed", 1, 1, 0, 0),
            ("would-block", "hk-partial", 1, 0, 0, 1),
        ]
    );
    assert_eq!((m.verify_blocks, m.overhead[0].stop_blocks), (1, 1));
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
        vec![kind(Kind::PrStatus, 25, 225, 22_500_000)],
    )];
    let mut saved = estimate(&m, &baselines);
    assert_eq!(
        net(&saved, &[]),
        None,
        "unpriced CI waits make the net unknown"
    );
    saved.retain(|e| e.kind != Kind::CiWait);
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
        "assuming one more request per block; no test, lint or build passed after it before the session next stopped, at 100k tokens per request (the average before setup)"
    );
    assert_eq!(
        c[2].basis,
        "50 requests in the history after setup, subagents' included, each reading the primer's ~500 tokens"
    );
    // The PR status estimate saves 800k; the costs come to 325k.
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
fn ci_wait_without_a_baseline_does_not_hide_another_harness_net() {
    let measured = measure(&ledger().entries, &StatsFilter::default());
    let without = estimate(&measured, &[]);
    let ci = without.iter().find(|e| e.kind == Kind::CiWait).unwrap();
    assert!(ci.outside_net);
    assert!(ci.refused.as_deref().unwrap().contains("no claude history"));
    assert_eq!(net(&without, &[]), Some(0));

    let with = estimate(&measured, &[priced("claude", Vec::new())]);
    let ci = with.iter().find(|e| e.kind == Kind::CiWait).unwrap();
    assert!(!ci.outside_net);
    assert!(ci.refused.as_deref().unwrap().contains("linked, avoidable"));
    assert_eq!(net(&with, &[]), None);
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
fn search_without_matches_is_an_answer_but_errors_are_overhead() {
    let run = |code| LedgerEntry {
        harness: "jouet".into(),
        event: "jouet-run".into(),
        ..post("jouet search", code)
    };
    let entries = [
        post("jouet search <arg>", 1),
        run(1),
        post("jouet search <arg>", 2),
        run(2),
        post("jouet search <arg>", 124),
        run(124),
    ];
    let measured = measure(&entries, &StatsFilter::default());
    assert_eq!(measured.overhead[0].failed_runs, 2);

    let legacy = measure(
        &[post("jouet search <arg>", 1), post("jouet search <arg>", 2)],
        &StatsFilter::default(),
    );
    assert_eq!(legacy.overhead[0].failed_runs, 1);

    // The search may have succeeded and the later command failed.
    let chained = measure(
        &[post("jouet search <arg> && jouet commit <arg>", 1)],
        &StatsFilter::default(),
    );
    assert_eq!(chained.overhead[0].failed_runs, 1);
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

#[test]
fn each_deny_warning_and_rewrite_runs_until_the_next_check_or_edit() {
    let at = |ts: u64, e: LedgerEntry| LedgerEntry { ts, ..e };
    let decided = |ts: u64, session: &str, action: &str, revision: Option<&str>| {
        let mut e = warned_by(session, "piped-test-output", revision);
        e.decisions[0].action = action.into();
        at(ts, e)
    };
    let edit = |ts: u64, session: &str| LedgerEntry {
        ts,
        session: Some(session.into()),
        event: "edit".into(),
        ..LedgerEntry::default()
    };
    let check = |ts: u64, session: &str, exit_code: i32| LedgerEntry {
        ts,
        duration_ms: Some(3_000),
        session: Some(session.into()),
        decisions: vec![LedgerDecision {
            rule: "checks".into(),
            action: "record:test".into(),
            revision: None,
            scope: None,
        }],
        ..post("cargo test", exit_code)
    };
    let entries = [
        decided(10, "a", "deny", Some("r1")),
        // Neither a check nor an edit.
        at(
            12,
            LedgerEntry {
                session: Some("a".into()),
                ..post("git status", 0)
            },
        ),
        // A failing check counts as much as a passing one.
        check(15, "a", 1),
        decided(20, "a", "warn", Some("r1")),
        // Another session's edit is not this one's.
        edit(21, "b"),
        edit(25, "a"),
        decided(30, "a", "rewrite", None),
        // An older revision's deny is left out.
        decided(31, "a", "deny", Some("r0")),
        check(32, "a", 0),
        // Nothing follows this one.
        decided(40, "b", "deny", Some("r1")),
        decided(50, "c", "deny", Some("r1")),
        // Issued at 49s, before the deny: it does not end it.
        check(51, "c", 0),
        // A jouet test run is dated by the shell call that ran it, which
        // the ledger writes after it: issued at 62s less 4.
        LedgerEntry {
            ts: 60,
            harness: "jouet".into(),
            session: Some("c".into()),
            event: "jouet-run".into(),
            shape: Some("jouet test".into()),
            exit_code: Some(0),
            ..LedgerEntry::default()
        },
        // A call of something else in between is not the run's.
        at(
            60,
            LedgerEntry {
                session: Some("c".into()),
                duration_ms: Some(9_000),
                ..post("git status", 0)
            },
        ),
        // Nor is a jouet call begun before the deny, or one that does not
        // name the run when another does.
        at(
            61,
            LedgerEntry {
                session: Some("c".into()),
                duration_ms: Some(30_000),
                ..post("jouet wait ci", 0)
            },
        ),
        at(
            61,
            LedgerEntry {
                session: Some("c".into()),
                duration_ms: Some(500),
                ..post("jouet read src/main.rs", 0)
            },
        ),
        at(
            61,
            LedgerEntry {
                session: Some("c".into()),
                duration_ms: Some(4_000),
                ..post("jouet test --cmd -- cargo test", 0)
            },
        ),
        // A run whose call was not found within two seconds is dated by
        // its own line, not by a later jouet call.
        decided(70, "d", "deny", Some("r1")),
        LedgerEntry {
            ts: 72,
            harness: "jouet".into(),
            session: Some("d".into()),
            event: "jouet-run".into(),
            shape: Some("jouet test".into()),
            exit_code: Some(0),
            ..LedgerEntry::default()
        },
        at(
            80,
            LedgerEntry {
                session: Some("d".into()),
                duration_ms: Some(1_000),
                ..post("jouet read src/main.rs", 0)
            },
        ),
        // An older ledger wrote the subcommand as <arg>: then the first
        // jouet call that began after the deny ran it.
        decided(90, "e", "deny", Some("r1")),
        LedgerEntry {
            ts: 92,
            harness: "jouet".into(),
            session: Some("e".into()),
            event: "jouet-run".into(),
            shape: Some("jouet test".into()),
            exit_code: Some(0),
            ..LedgerEntry::default()
        },
        at(
            92,
            LedgerEntry {
                session: Some("e".into()),
                duration_ms: Some(500),
                ..post("git status", 0)
            },
        ),
        at(
            92,
            LedgerEntry {
                session: Some("e".into()),
                duration_ms: Some(20_000),
                ..post("jouet <arg> ci", 0)
            },
        ),
        at(
            93,
            LedgerEntry {
                session: Some("e".into()),
                duration_ms: Some(3_000),
                ..post("jouet <arg> --cmd -- cargo test", 0)
            },
        ),
        // A jouet test already running when the deny came does not end it:
        // the edit after it does.
        decided(110, "f", "deny", Some("r1")),
        LedgerEntry {
            ts: 112,
            harness: "jouet".into(),
            session: Some("f".into()),
            event: "jouet-run".into(),
            shape: Some("jouet test".into()),
            exit_code: Some(0),
            ..LedgerEntry::default()
        },
        at(
            112,
            LedgerEntry {
                session: Some("f".into()),
                duration_ms: Some(5_000),
                ..post("jouet test --cmd -- cargo test", 0)
            },
        ),
        edit(115, "f"),
        // The record a jouet test writes is dated by its shell call too.
        decided(130, "g", "deny", Some("r1")),
        LedgerEntry {
            ts: 132,
            harness: "jouet".into(),
            session: Some("g".into()),
            event: "test-run".into(),
            duration_ms: None,
            ..check(132, "g", 0)
        },
        at(
            132,
            LedgerEntry {
                session: Some("g".into()),
                duration_ms: Some(2_000),
                ..post("jouet test --cmd -- cargo test", 0)
            },
        ),
    ];
    let filter = StatsFilter {
        revisions: Some(BTreeMap::from([(
            "piped-test-output".to_string(),
            "r1".to_string(),
        )])),
        ..StatsFilter::default()
    };
    let m = measure(&entries, &filter);
    let spans: Vec<_> = m
        .decision_spans
        .iter()
        .map(|s| (s.action.as_str(), s.session.as_str(), s.at, s.until_ms))
        .collect();
    // A check is dated when it ended, 3 seconds after it was issued.
    assert_eq!(
        spans,
        [
            ("deny", "a", 10, Some(13_000)),
            ("warn", "a", 20, Some(26_000)),
            ("rewrite", "a", 30, Some(30_000)),
            ("deny", "b", 40, None),
            ("deny", "c", 50, Some(58_000)),
            ("deny", "d", 70, Some(73_000)),
            ("deny", "e", 90, Some(91_000)),
            ("deny", "f", 110, Some(116_000)),
            ("deny", "g", 130, Some(131_000)),
        ]
    );
}

#[test]
fn a_checks_answer_is_not_an_own_failure() {
    let run = |shape: &str, code| LedgerEntry {
        harness: "jouet".into(),
        event: "jouet-run".into(),
        ..post(shape, code)
    };
    let entries = [
        post("jouet wait pr <arg>", 124),
        // Still waiting at the timeout, and the PR closed: both answers.
        run("jouet wait pr", 124),
        run("jouet wait pr", 1),
        // A real failure is the answer; nothing failed yet (4) is not.
        run("jouet ci explain", 1),
        run("jouet ci explain", 4),
        // Findings are diag's answer.
        run("jouet diag check", 1),
        run("jouet test", 1),
        run("jouet test", 2),
        run("jouet commit", 1),
    ];
    let m = measure(&entries, &StatsFilter::default());
    let own = |command: &str| {
        let c = m.commands.iter().find(|c| c.command == command).unwrap();
        (c.own_runs, c.own_failed)
    };
    assert_eq!(own("jouet wait pr"), (2, 0));
    assert_eq!(own("jouet ci explain"), (2, 1));
    assert_eq!(own("jouet diag check"), (1, 0));
    assert_eq!(own("jouet test"), (2, 1));
    assert_eq!(own("jouet commit"), (1, 1));
    // The cost counts the same failures as the table.
    assert_eq!(m.overhead[0].failed_runs, 3);
}
