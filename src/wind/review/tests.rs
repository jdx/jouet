use super::*;
use crate::wind::Paths;

const NOW: u64 = 1_793_354_400;

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        ledger: dir.path().join("ledger"),
        state: dir.path().join("wind.json"),
        rules: dir.path().join("rules"),
        log: dir.path().join("rule-log.jsonl"),
    };
    Fixture { _dir: dir, paths }
}

fn settings() -> WindSettings {
    WindSettings {
        enabled: true,
        auto_promote: false,
        promote_after: 20,
        review: true,
        review_every: 3,
        review_corrections: 2,
        memories: true,
        gitleaks: None,
    }
}

const NOISY: Signals = Signals {
    corrections: 3,
    failed_retries: 0,
    polling_ms: 0,
};
const QUIET: Signals = Signals {
    corrections: 1,
    failed_retries: 2,
    polling_ms: 60_000,
};

fn reply(json: &str) -> Reply {
    serde_json::from_str(json).unwrap()
}

#[test]
fn a_session_is_digested_at_most_every_ten_minutes() {
    let f = fixture();
    assert!(due(&f.paths, "s1", NOW));
    assert!(!due(&f.paths, "s1", NOW + 60));
    assert!(due(&f.paths, "s1", NOW + CHECK_INTERVAL));
    assert!(due(&f.paths, "s2", NOW + 60));
}

#[test]
fn corrections_or_waste_queue_a_review_once_every_n_sessions() {
    let f = fixture();
    let s = settings();
    let check = |session: &str, signals: Signals| {
        assert!(due(&f.paths, session, NOW));
        consider(
            &f.paths,
            &s,
            "claude",
            session,
            Some("repo".into()),
            signals,
            NOW,
        )
    };
    assert_eq!(check("s1", QUIET), Check::Quiet);
    assert_eq!(check("s2", NOISY), Check::Queued("r1".into()));
    // Two more sessions since the last queued are not yet three.
    assert_eq!(check("s3", NOISY), Check::Quiet);
    assert_eq!(check("s4", NOISY), Check::Quiet);
    let wasteful = Signals {
        corrections: 0,
        failed_retries: FAILED_RETRIES,
        polling_ms: 0,
    };
    assert_eq!(check("s5", wasteful), Check::Queued("r2".into()));
    // Queued once, never again.
    assert_eq!(
        consider(&f.paths, &s, "claude", "s5", None, NOISY, NOW),
        Check::Skipped
    );
    assert!(!due(&f.paths, "s5", NOW + CHECK_INTERVAL));
}

#[test]
fn a_queued_review_is_shown_to_other_sessions_in_its_repository() {
    let f = fixture();
    let s = settings();
    assert!(due(&f.paths, "aaaaaaaa-1111", NOW));
    consider(
        &f.paths,
        &s,
        "claude",
        "aaaaaaaa-1111",
        Some("repo".into()),
        NOISY,
        NOW,
    );
    // Not to itself, not in another repository.
    assert!(lines_for_session(&f.paths, "aaaaaaaa-1111", Some("repo"), 2, NOW).is_empty());
    assert!(lines_for_session(&f.paths, "b", Some("other"), 2, NOW).is_empty());
    let lines = lines_for_session(&f.paths, "b", Some("repo"), 2, NOW + 7200);
    assert_eq!(
        lines,
        [
            "jouet wind: session aaaaaaaa (2h ago, this repository) had 3 user corrections. At a natural pause, review it in a subagent: give it the output of `jouet wind review aaaaaaaa-1111`, and it records what it proposes with `jouet wind propose`. `jouet wind dismiss r1` to skip it."
        ]
    );
    // No room, no line.
    assert!(lines_for_session(&f.paths, "c", Some("repo"), 0, NOW).is_empty());
    // Shown to three sessions, it lapses.
    lines_for_session(&f.paths, "c", Some("repo"), 2, NOW);
    lines_for_session(&f.paths, "d", Some("repo"), 2, NOW);
    assert!(lines_for_session(&f.paths, "e", Some("repo"), 2, NOW).is_empty());
}

#[test]
fn proposals_are_checked_limited_and_shown() {
    let f = fixture();
    let out = record(
        &f.paths,
        reply(
            r#"{"session": "aaaaaaaa-1111", "proposals": [
            {"kind": "task", "title": "Add a release:check task", "why": "Typed out three times.", "evidence": ["L8-14"], "draft": "[tasks.\"release:check\"]\nrun = \"cargo xtask release-check\""},
            {"kind": "agents_md", "title": "Say that tests run through mise", "why": "The user corrected it (L11).", "evidence": ["L11"], "scope": "project", "draft": "Run tests with `mise run test`."},
            {"kind": "rule", "title": "Warn on a token", "why": "x", "draft": "GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789"},
            {"kind": "task", "title": "Bad scope", "why": "x", "scope": "repo", "draft": "x"},
            {"kind": "memory", "title": "Bad span", "why": "x", "evidence": ["line 3"], "draft": "x"},
            {"kind": "memory", "title": "Never open draft PRs", "why": "Said twice.", "draft": "The user wants ready PRs."},
            {"kind": "memory", "title": "A fourth", "why": "x", "draft": "x"},
            {"kind": "nothing"}
        ]}"#,
        ),
        NOW,
        &[],
    )
    .unwrap();
    assert_eq!(out.recorded, ["p1", "p2", "p3"]);
    assert!(out.nothing);
    let reasons: Vec<(&str, &str)> = out
        .skipped
        .iter()
        .map(|s| (s.title.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            (
                "Warn on a token",
                "it holds something that looks like a secret"
            ),
            (
                "Bad scope",
                "a mise task takes the scope local, project, global, not `repo`"
            ),
            (
                "Bad span",
                "evidence `line 3` is not a span such as L120-188"
            ),
            ("A fourth", "a review records 3 proposals at most"),
        ]
    );
    let state = State::load(&f.paths.state);
    let task = state.reviews.proposal("p1").unwrap();
    assert_eq!(task.scope, "local");
    assert_eq!(task.status, Status::Pending);

    // The same title again is a duplicate; the day's limit is reached too.
    let again = record(
        &f.paths,
        reply(r#"{"proposals": [{"kind": "task", "title": "add a RELEASE:CHECK task", "why": "x", "draft": "x"}, {"kind": "memory", "title": "New", "why": "x", "draft": "x"}]}"#),
        NOW,
        &[],
    )
    .unwrap();
    let reasons: Vec<&str> = again.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(
        reasons,
        [
            "already proposed",
            "3 proposals a day at most; propose it again tomorrow"
        ]
    );

    let lines = lines_for_session(&f.paths, "s9", None, 2, NOW);
    assert_eq!(
        lines[0],
        "jouet wind review proposed a mise task (p1): Add a release:check task. `jouet wind show p1` for why and the draft; ask the user whether it goes in mise.local.toml (the default) or mise.toml, and add it with `mise tasks add`; then `jouet wind adopt p1`, or `jouet wind dismiss p1` to drop it."
    );
    assert!(lines[1].contains("an AGENTS.md note (p2)"));
}

#[test]
fn a_reviewed_session_leaves_the_queue() {
    let f = fixture();
    assert!(due(&f.paths, "aaaaaaaa-1111", NOW));
    consider(
        &f.paths,
        &settings(),
        "claude",
        "aaaaaaaa-1111",
        None,
        NOISY,
        NOW,
    );
    record(
        &f.paths,
        reply(r#"{"session": "aaaaaaaa", "proposals": [{"kind": "nothing"}]}"#),
        NOW,
        &[],
    )
    .unwrap();
    let state = State::load(&f.paths.state);
    assert_eq!(state.reviews.queued("r1").unwrap().status, Status::Adopted);
}

#[test]
fn the_prompt_names_every_kind_and_the_reply_it_reads() {
    for kind in [
        "\"task\"",
        "\"routine\"",
        "\"rule\"",
        "\"agents_md\"",
        "\"memory\"",
        "\"memory_update\"",
        "\"memory_delete\"",
        "\"nothing\"",
    ] {
        assert!(PROMPT.contains(kind), "{kind}");
    }
    // The example in the prompt is a reply `jouet wind propose` accepts.
    let start = PROMPT.find("{\"session\"").unwrap();
    let end = PROMPT[start..].find("\n]}").unwrap() + start + 3;
    let example: Reply = serde_json::from_str(&PROMPT[start..end]).unwrap();
    assert!(problem(&example.proposals[0], &[]).is_none());
}

#[test]
fn memory_changes_name_a_listed_memory_once() {
    let f = fixture();
    let targets = ["~/.claude/projects/-work-app/memory/mbx-gc.md".to_string()];
    let out = record(
        &f.paths,
        reply(
            r#"{"session": "s1", "proposals": [
            {"kind": "memory_delete", "title": "Drop the mbx GC workaround", "why": "cargo worked plainly (L40).", "evidence": ["L40"], "target": "~/.claude/projects/-work-app/memory/mbx-gc.md", "draft": "mbx 1.30 keeps the target."},
            {"kind": "memory_update", "title": "No target", "why": "x", "draft": "x"},
            {"kind": "memory_update", "title": "Unknown target", "why": "x", "target": "~/elsewhere.md", "draft": "x"}
        ]}"#,
        ),
        NOW,
        &targets,
    )
    .unwrap();
    assert_eq!(out.recorded, ["p1"]);
    let reasons: Vec<&str> = out.skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(
        reasons,
        [
            "an update to a memory needs a target: the memory's path",
            "`~/elsewhere.md` is not one of the memories the review listed"
        ]
    );
    // Another title for the same memory is the same proposal.
    let again = record(
        &f.paths,
        reply(
            r#"{"proposals": [{"kind": "memory_delete", "title": "Delete it", "why": "x", "target": "~/.claude/projects/-work-app/memory/mbx-gc.md", "draft": "x"}]}"#,
        ),
        NOW,
        &targets,
    )
    .unwrap();
    assert_eq!(again.skipped[0].reason, "already proposed");
    let lines = lines_for_session(&f.paths, "s9", None, 2, NOW);
    assert_eq!(
        lines[0],
        "jouet wind review proposed deleting a memory `~/.claude/projects/-work-app/memory/mbx-gc.md` (p1): Drop the mbx GC workaround. `jouet wind show p1` for why and the draft; ask the user, then delete the memory and its line in the index (MEMORY.md); then `jouet wind adopt p1`, or `jouet wind dismiss p1` to drop it."
    );
}
