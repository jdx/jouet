//! Scans the fixture transcripts in `tests/fixtures/home`: a Claude Code
//! session with a subagent and a cut-off last line, a Codex session with
//! bare-key JavaScript cells, a zstd-compressed archived rollout, and one
//! whose second zstd frame is cut off.

use jouet_audit::{
    Analyzer, Category, ClaudeCode, Codex, Filter, HarnessReport, Pattern, Reader, Report, Source,
    scan,
};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/home")
}

fn readers() -> (ClaudeCode, Codex) {
    (
        ClaudeCode {
            root: fixtures().join("claude"),
        },
        Codex {
            root: fixtures().join("codex"),
        },
    )
}

fn run(filter: &Filter, examples: bool) -> (Report, jouet_audit::ScanStats) {
    let (claude, codex) = readers();
    let readers: [&dyn Reader; 2] = [&claude, &codex];
    let s = scan(&readers, filter, Analyzer::new(examples, None), 4);
    (s.analyzer.report(5), s.stats)
}

fn harness<'a>(report: &'a Report, name: &str) -> &'a HarnessReport {
    report
        .harnesses
        .iter()
        .find(|h| h.harness == name)
        .unwrap_or_else(|| panic!("no {name} in the report"))
}

fn tokens(h: &HarnessReport, category: Category) -> u64 {
    h.categories
        .iter()
        .find(|c| c.category == category)
        .map_or(0, |c| c.input_tokens)
}

#[test]
fn reads_both_harnesses_and_reports_what_it_could_not_read() {
    let (report, stats) = run(&Filter::default(), false);
    // Two Claude transcripts (the workflow journal is not one) and three
    // Codex rollouts.
    assert_eq!(stats.files, 5);
    assert_eq!(stats.bad_lines, 1);
    assert_eq!(stats.unreadable.len(), 1);
    assert!(stats.unreadable[0].partial);
    assert!(
        stats.unreadable[0].path.ends_with(
            "rollout-2026-10-28T09-00-00-01a00000-0000-7000-8000-000000000003.jsonl.zst"
        )
    );

    let claude = harness(&report, "claude");
    assert_eq!(claude.totals.sessions, 1);
    assert_eq!(claude.totals.transcripts, 2);
    // Seven requests in the session (m1 is two lines) and one in the subagent.
    assert_eq!(claude.totals.requests, 8);
    assert_eq!(
        claude.totals.input_tokens,
        20_000 + 21_000 + 22_000 + 23_000 + 24_000 + 25_000 + 26_000 + 8_000
    );
    assert_eq!(tokens(claude, Category::CiPolling), 41_000);
    assert_eq!(tokens(claude, Category::Reply), 26_000);
    let ci = claude
        .suggestions
        .iter()
        .find(|s| s.category == Category::CiPolling)
        .expect("a CI suggestion");
    assert_eq!(ci.saving.as_ref().map(|s| s.input_tokens), Some(21_000));

    let codex = harness(&report, "codex");
    // The live session, the archived one, and the one cut off after its
    // first request.
    assert_eq!(codex.totals.sessions, 3);
    // The repeated token count is counted once.
    assert_eq!(codex.totals.requests, 9);
    assert_eq!(codex.categories[0].category, Category::ProcessPolling);
    assert_eq!(
        tokens(codex, Category::ProcessPolling),
        31_000 + 32_000 + 40_000 + 41_000
    );
    assert_eq!(tokens(codex, Category::CiPolling), 30_000);
    assert_eq!(tokens(codex, Category::Edits), 34_000);
    // The cell with two commands splits its request between git and cargo.
    assert_eq!(tokens(codex, Category::Git), 16_500);
    // The cut-off rollout's first request: the rest of it was never read.
    assert_eq!(tokens(codex, Category::ReadSearch), 5_000);
    assert_eq!(codex.totals.failed_calls, 1);
}

#[test]
fn finds_the_patterns() {
    let (report, _) = run(&Filter::default(), false);
    let claude = harness(&report, "claude");
    let found: Vec<Pattern> = claude.patterns.iter().map(|p| p.pattern).collect();
    for p in [
        Pattern::SleepPoll,
        Pattern::HeadOnTestOutput,
        Pattern::StaleTokenPrefix,
        Pattern::HeredocEdit,
        Pattern::RateLimited,
    ] {
        assert!(found.contains(&p), "{p:?} not in {found:?}");
    }
}

#[test]
fn shows_shapes_and_only_redacted_examples() {
    let (report, _) = run(&Filter::default(), false);
    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains("gh pr checks <n> --repo <path>"), "{json}");
    for private in ["acme/app", "src/lib.rs", "a - b", "secret", "FAKE"] {
        assert!(!json.contains(private), "{private} leaked: {json}");
    }

    let (report, _) = run(&Filter::default(), true);
    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains("gh pr checks 12 --repo acme/app"), "{json}");
    assert!(json.contains("Bearer <redacted>"), "{json}");
    assert!(json.contains("<heredoc: 3 lines>"), "{json}");
    for private in ["FAKE", "a - b", "write_text"] {
        assert!(!json.contains(private), "{private} leaked: {json}");
    }
}

#[test]
fn filters_by_project_and_time() {
    let (report, stats) = run(
        &Filter {
            project: Some(PathBuf::from("/work/app")),
            ..Default::default()
        },
        false,
    );
    assert_eq!(stats.other_projects, 2);
    assert_eq!(harness(&report, "codex").totals.sessions, 1);
    assert_eq!(harness(&report, "claude").totals.sessions, 1);

    // 10:01:42 leaves the Claude session's last two requests and its subagent.
    let since = jouet_audit::parse_timestamp("2026-10-30T10:01:42Z");
    let (report, _) = run(
        &Filter {
            since_ms: since,
            ..Default::default()
        },
        false,
    );
    assert_eq!(harness(&report, "claude").totals.requests, 3);
}

#[test]
fn a_file_that_vanished_is_reported_not_fatal() {
    struct Gone;
    impl Reader for Gone {
        fn harness(&self) -> &'static str {
            "gone"
        }
        fn discover(&self, _: &Filter) -> std::io::Result<Vec<Source>> {
            Ok(vec![Source {
                path: fixtures().join("missing.jsonl"),
                bytes: 0,
                modified_ms: None,
            }])
        }
        fn read(
            &self,
            _: &Source,
            _: &mut dyn std::io::BufRead,
            _: &Filter,
        ) -> std::io::Result<Option<jouet_audit::Transcript>> {
            unreachable!("the file cannot be opened")
        }
    }
    let s = scan(&[&Gone], &Filter::default(), Analyzer::default(), 2);
    assert_eq!(s.stats.files, 0);
    assert_eq!(s.stats.unreadable.len(), 1);
    assert!(!s.stats.unreadable[0].partial);
    assert!(s.analyzer.report(1).harnesses.is_empty());
}

#[test]
fn missing_harness_directories_have_no_transcripts() {
    let none = ClaudeCode {
        root: fixtures().join("nowhere"),
    };
    assert_eq!(none.discover(&Filter::default()).unwrap(), vec![]);
    let none = Codex {
        root: fixtures().join("nowhere"),
    };
    assert_eq!(none.discover(&Filter::default()).unwrap(), vec![]);
}
