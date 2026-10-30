use super::*;
use crate::ledger::Decision;

fn post(session: &str, shape: &str, exit_code: i32) -> Entry {
    Entry {
        ts: 1,
        harness: "claude".into(),
        session: Some(session.into()),
        event: "post-tool-use".into(),
        tool: Some("Bash".into()),
        shape: Some(shape.into()),
        exit_code: Some(exit_code),
        ..Entry::default()
    }
}

fn pre(session: &str, shape: &str) -> Entry {
    Entry {
        event: "pre-tool-use".into(),
        exit_code: None,
        ..post(session, shape, 0)
    }
}

fn signal<'a>(signals: &'a [Signal], kind: Kind, subject: &str) -> &'a Signal {
    signals
        .iter()
        .find(|s| s.kind() == kind && s.subject == subject)
        .unwrap_or_else(|| panic!("no {kind:?} for {subject}: {signals:#?}"))
}

#[test]
fn the_ledger_shows_crashes_codes_usage_errors_and_abandoned_answers() {
    let entries = [
        post("a", "jouet pr threads <arg>", 101),
        post("a", "jouet wait ci <arg>", 7),
        post("a", "jouet wind digest --session <arg>", 2),
        post("a", "jouet wind digest <arg>", 0),
        post("a", "jouet pr status <arg>", 0),
        post("a", "gh pr view <arg> --json <arg>", 0),
        post("b", "jouet pr status <arg>", 0),
        post("b", "gh api <arg>", 0),
        post("c", "jouet pr status <arg>", 0),
        post("c", "gh pr view <arg>", 0),
    ];
    let found = from_ledger(&entries, None);
    let crash = signal(&found, Kind::Crash, "jouet pr threads");
    assert_eq!(crash.destination(), Destination::Issue);
    assert!(crash.ready());
    assert_eq!(crash.key(), "crash-pr-threads");
    assert_eq!(
        signal(&found, Kind::ExitCode, "jouet wait ci").details,
        BTreeMap::from([("exit 7".to_string(), 1)])
    );
    let usage = signal(&found, Kind::Usage, "jouet wind digest");
    assert_eq!(usage.destination(), Destination::Discussion);
    // Once, in one session: not yet worth a maintainer's time.
    assert!(!usage.ready());
    let dropped = signal(&found, Kind::Abandoned, "jouet pr status");
    assert_eq!((dropped.count, dropped.sessions.len()), (3, 3));
    assert!(dropped.ready());
    assert_eq!(
        dropped.summary(),
        "agents dropped `jouet pr status` for raw commands right after it answered, 3 times in 3 sessions (then `gh pr view`)"
    );
}

#[test]
fn slow_hooks_are_bugs_with_their_budget_and_slowest_run() {
    let slow = |ms| Entry {
        ts: 1,
        harness: "claude".into(),
        session: Some("a".into()),
        event: "slow-hook".into(),
        tool: Some("pre-tool-use".into()),
        duration_ms: Some(ms),
        budget_ms: Some(500),
        ..Entry::default()
    };
    let found = from_ledger(&[slow(700), slow(1900), slow(650)], None);
    let s = signal(&found, Kind::SlowHook, "pre-tool-use");
    assert_eq!((s.count, s.numbers.clone()), (3, vec![500, 1900]));
    assert!(s.ready());
    assert_eq!(s.destination(), Destination::Issue);
    assert_eq!(hook_budget("stop", 500), 5000);
    assert_eq!(hook_budget("pre-tool-use", 500), 500);
}

#[test]
fn a_script_written_in_five_sessions_is_a_candidate_built_in() {
    let script = "cd <arg> && for <arg> in <arg>…; do gh pr checks <arg>; done";
    let mut entries: Vec<Entry> = ["a", "b", "c", "d"]
        .iter()
        .map(|s| pre(s, script))
        .collect();
    entries.push(pre("a", "cargo test"));
    assert!(from_ledger(&entries, None).is_empty());
    entries.push(pre("e", script));
    let found = from_ledger(&entries, None);
    let routine = signal(&found, Kind::Routine, script);
    assert_eq!(routine.count, 5);
    assert!(routine.key().starts_with("routine-") && routine.key().len() == "routine-".len() + 8);
    assert!(!script_like("git status") && !script_like("cd <arg> && cargo test"));
}

#[test]
fn an_overridden_built_in_rule_is_an_idea_and_mostly_overridden_a_bug() {
    let engine = crate::wind::user_engine(std::path::Path::new("/nonexistent"));
    let rule = engine
        .rules()
        .iter()
        .find(|r| r.scope == Scope::Builtin && r.event == jouet_rules::Event::PreToolUse)
        .unwrap()
        .name
        .clone();
    let denied = |session: &str| Entry {
        decisions: vec![Decision {
            rule: rule.clone(),
            action: "deny".into(),
            scope: None,
        }],
        ..pre(session, "git stash")
    };
    let overridden = |session: &str| Entry {
        event: "override".into(),
        decisions: vec![Decision {
            rule: rule.clone(),
            action: "override".into(),
            scope: None,
        }],
        ..pre(session, "git stash")
    };
    let mut entries: Vec<Entry> = (0..10).map(|i| denied(&format!("s{i}"))).collect();
    entries.extend((0..3).map(|i| overridden(&format!("s{i}"))));
    let found = from_ledger(&entries, Some(&engine));
    let s = signal(&found, Kind::Overridden, &rule);
    assert_eq!(s.numbers, [10, 3]);
    assert_eq!(s.destination(), Destination::Discussion);
    assert_eq!(s.examples, ["git stash"]);

    entries.extend((3..6).map(|i| overridden(&format!("s{i}"))));
    let found = from_ledger(&entries, Some(&engine));
    assert_eq!(
        signal(&found, Kind::Overridden, &rule).destination(),
        Destination::Issue
    );
    // Too few overrides is not a signal.
    let few: Vec<Entry> = (0..10).map(|i| denied(&format!("s{i}"))).collect();
    assert!(from_ledger(&few, Some(&engine)).is_empty());
}

#[test]
fn a_session_digested_twice_counts_once_and_sharing_survives_a_refresh() {
    let found = jouet_audit::digest::jouet::Found {
        kind: jouet_audit::digest::jouet::Kind::Crash,
        subject: "jouet pr threads".into(),
        index: 0,
        detail: "panicked at src/cmd/threads.rs:41:9".into(),
    };
    let one = [(
        found,
        "jouet pr threads 7 --repo acme/secret".to_string(),
        12,
    )];
    let mut gaps = Gaps::default();
    add_digest(&mut gaps, "s1", &one);
    add_digest(&mut gaps, "s1", &one);
    let s = gaps.gaps[0].signal.as_ref().unwrap();
    assert_eq!(s.count, 1);
    // Only the shape is kept.
    assert_eq!(s.examples, ["jouet pr threads <arg> --repo <arg>"]);

    gaps.gaps[0].shared = true;
    gaps.gaps[0].shown.push("s9".into());
    let fresh = from_ledger(&[post("s2", "jouet pr threads <arg>", 101)], None);
    replace_ledger(&mut gaps, fresh);
    assert_eq!(gaps.gaps.len(), 1);
    let entry = &gaps.gaps[0];
    assert!(entry.shared && entry.shown == ["s9"]);
    let s = entry.signal.as_ref().unwrap();
    assert_eq!(s.sessions.len(), 2);
    assert_eq!(s.count, 1);
}
