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
