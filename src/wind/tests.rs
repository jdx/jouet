use super::*;
use crate::ledger::Entry;
use crate::wind::detect::tests::ran;
use crate::wind::stats::tests::{fired, overridden};

const DAY: u64 = 1_793_318_400;

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
    std::fs::create_dir_all(&paths.rules).unwrap();
    Fixture { _dir: dir, paths }
}

fn settings() -> WindSettings {
    WindSettings {
        enabled: true,
        auto_promote: false,
        promote_after: 3,
    }
}

fn write(paths: &Paths, entries: &[Entry]) {
    for entry in entries {
        ledger::append(&paths.ledger, entry);
    }
}

/// Three patterns' worth of failures, each across two sessions.
fn failures(ts: u64) -> Vec<Entry> {
    let mut out = Vec::new();
    for (i, session) in ["a", "a", "b"].iter().enumerate() {
        let t = ts + i as u64;
        out.push(ran(t, session, "baz-tool test", 127));
        out.push(ran(t, session, "foo-tool sync", 2));
        out.push(ran(t, session, "bar-cli up", 1));
    }
    out
}

fn warned(ts: u64, session: &str, command: &str, rule: &str) -> Entry {
    fired(ts, session, command, &[rule])
}

#[test]
fn winding_reads_incrementally_and_drafts_within_the_daily_limit() {
    let f = fixture();
    write(&f.paths, &failures(DAY));
    let report = run(&f.paths, &settings(), DAY + 100, false);
    assert_eq!(report.read, 9);
    // Three patterns are ready, but only two notes a day are drafted.
    assert_eq!(report.drafted, ["n1", "n2"]);

    // Too soon: nothing is read.
    assert!(run(&f.paths, &settings(), DAY + 110, false).skipped);
    // The next day, the third pattern gets its note, and nothing is read
    // twice.
    let report = run(&f.paths, &settings(), DAY + 86_400, false);
    assert_eq!(report.read, 0);
    assert_eq!(report.drafted, ["n3"]);
    let state = State::load(&f.paths.state);
    assert_eq!(state.notes.len(), 3);
    assert!(
        state
            .notes
            .iter()
            .all(|n| n.code.contains("--- mode: warn"))
    );
}

#[test]
fn at_most_two_notes_per_session_start_and_each_lapses() {
    let f = fixture();
    write(&f.paths, &failures(DAY));
    run(&f.paths, &settings(), DAY + 100, false);
    run(&f.paths, &settings(), DAY + 86_400, true);

    let first = notes_for_session(&f.paths, "s1");
    assert_eq!(first.len(), MAX_PER_START);
    let line = note_line(&first[0], "s1");
    assert!(line.contains("--from-note n1 --author s1"), "{line}");

    notes_for_session(&f.paths, "s2");
    notes_for_session(&f.paths, "s3");
    // n1 and n2 were shown to three sessions and lapsed; n3 is next.
    let later: Vec<String> = notes_for_session(&f.paths, "s4")
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(later, ["n3"]);
    assert!(set_status(&f.paths, "n3", Status::Dismissed));
    assert!(notes_for_session(&f.paths, "s5").is_empty());
    assert!(!set_status(&f.paths, "nope", Status::Dismissed));
}

const USER_RULE: &str = "--- why: no tags\n--- author: s1\nreturn { check = function(call)\n\tfor _, c in call.commands do\n\t\tif c.argv[1] == \"git\" and c.argv[2] == \"tag\" then return jouet.deny(\"no\") end\n\tend\n\treturn nil\nend }\n";

#[test]
fn promote_switches_a_user_rule_to_deny_and_logs_it() {
    let f = fixture();
    let path = f.paths.rules.join("no-tags.luau");
    std::fs::write(&path, USER_RULE).unwrap();
    let change = promote(&f.paths, "no-tags", "user", "asked", DAY).unwrap();
    assert_eq!(change.to.as_deref(), Some("deny"));
    let engine = user_engine(&f.paths.rules).unwrap();
    assert_eq!(engine.rule("no-tags").unwrap().mode, Mode::Deny);
    assert_eq!(rulefile::read_log(&f.paths.log), vec![change]);

    let again = promote(&f.paths, "no-tags", "user", "asked", DAY).unwrap_err();
    assert!(again.contains("already in deny mode"), "{again}");
}

#[test]
fn promote_refuses_built_in_rules_and_missing_ones() {
    let f = fixture();
    let err = promote(&f.paths, "git-stash", "user", "x", DAY).unwrap_err();
    assert!(err.contains("builtin rule"), "{err}");
    assert!(promote(&f.paths, "nope", "user", "x", DAY).is_err());
    assert!(rulefile::read_log(&f.paths.log).is_empty());
}

#[test]
fn auto_promotion_is_off_by_default_and_needs_clean_firings() {
    let f = fixture();
    std::fs::write(f.paths.rules.join("no-tags.luau"), USER_RULE).unwrap();
    // Fired four times, once per session, never overridden.
    let entries: Vec<Entry> = ["a", "b", "c", "d"]
        .iter()
        .enumerate()
        .map(|(i, s)| warned(DAY + i as u64, s, "git tag v1", "no-tags"))
        .collect();
    write(&f.paths, &entries);

    assert!(
        run(&f.paths, &settings(), DAY + 100, true)
            .promoted
            .is_empty()
    );
    let on = WindSettings {
        auto_promote: true,
        ..settings()
    };
    // An override holds it back.
    write(
        &f.paths,
        &[overridden(DAY + 5, "d", "git tag v2", "no-tags")],
    );
    assert!(run(&f.paths, &on, DAY + 200, true).promoted.is_empty());

    let f = fixture();
    std::fs::write(f.paths.rules.join("no-tags.luau"), USER_RULE).unwrap();
    write(&f.paths, &entries);
    assert_eq!(run(&f.paths, &on, DAY + 100, true).promoted, ["no-tags"]);
    let log = rulefile::read_log(&f.paths.log);
    assert_eq!(log[0].by, "jouet wind");
    assert!(log[0].reason.contains("no overrides"), "{}", log[0].reason);
}

#[test]
fn unwind_proposes_overridden_user_rules_never_built_ins() {
    let f = fixture();
    std::fs::write(f.paths.rules.join("no-tags.luau"), USER_RULE).unwrap();
    std::fs::write(f.paths.rules.join("broken.luau"), "return {}").unwrap();
    let mut entries = Vec::new();
    for i in 0..6 {
        entries.push(warned(DAY + i, "a", "git tag v1", "no-tags"));
        entries.push(overridden(DAY + i, "a", "git tag v1", "no-tags"));
        // A built-in rule overridden just as often stays.
        entries.push(warned(DAY + i, "a", "git stash", "git-stash"));
        entries.push(overridden(DAY + i, "a", "git stash", "git-stash"));
    }
    let stats = stats::compute(&entries);
    let engine = user_engine(&f.paths.rules).unwrap();
    let found = unwind::proposals(&engine, &stats);
    let names: Vec<&str> = found.iter().map(|p| p.rule.as_str()).collect();
    assert_eq!(names, ["broken", "no-tags"]);

    let (changes, errors) = unwind::apply(&f.paths, &found, "user", DAY);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(changes.len(), 2);
    assert!(jouet_rules::rule_files(&f.paths.rules).unwrap().is_empty());
    let engine = user_engine(&f.paths.rules).unwrap();
    assert!(engine.rule("git-stash").is_some());
    assert_eq!(rulefile::read_log(&f.paths.log).len(), 2);
}

#[test]
fn drafted_names_avoid_existing_rules() {
    let f = fixture();
    std::fs::write(
        f.paths.rules.join("wind-baz-tool-test-fails.luau"),
        USER_RULE.replace("no tags", "taken"),
    )
    .unwrap();
    write(&f.paths, &failures(DAY));
    run(&f.paths, &settings(), DAY + 100, false);
    let state = State::load(&f.paths.state);
    let names: Vec<&str> = state.notes.iter().map(|n| n.rule.as_str()).collect();
    assert!(names.contains(&"wind-baz-tool-test-fails-2"), "{names:?}");
}
