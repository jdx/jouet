use super::*;
use crate::wind::detect::tests::ran;
use crate::wind::stats::tests::fired;

const DAY: u64 = 1_793_318_400;

const RULE: &str = "--- why: tags are cut by the release job at /home/alice/acme\n--- mode: deny\nreturn {\n\tcheck = function(call)\n\t\tfor _, c in call.commands do\n\t\t\tif c.argv[1] == \"git\" and c.argv[2] == \"tag\" then return jouet.deny(\"Leave tags to ci.acme.internal.\") end\n\t\tend\n\t\treturn nil\n\tend,\n\texamples = { deny = { \"git tag v1\" }, allow = { \"git status\" } },\n}\n";

fn fixture(
    fires: u64,
    overrides: u64,
    promoted_days_ago: Option<u64>,
) -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        ledger: dir.path().join("ledger"),
        state: dir.path().join("wind.json"),
        rules: dir.path().join("rules"),
        log: dir.path().join("rule-log.jsonl"),
    };
    std::fs::create_dir_all(&paths.rules).unwrap();
    std::fs::write(paths.rules.join("no-tags.luau"), RULE).unwrap();
    for i in 0..fires {
        let session = format!("s{i}");
        ledger::append(
            &paths.ledger,
            &fired(DAY + i, &session, "git tag v1", &["no-tags"]),
        );
        ledger::append(&paths.ledger, &ran(DAY + i, &session, "git status", 0));
    }
    for i in 0..overrides {
        ledger::append(
            &paths.ledger,
            &crate::wind::stats::tests::overridden(DAY + i, "s0", "git tag v1", "no-tags"),
        );
    }
    if let Some(days) = promoted_days_ago {
        rulefile::log(
            &paths.log,
            &rulefile::Change {
                ts: DAY + 100 - days * 86_400,
                rule: "no-tags".into(),
                change: "promoted".into(),
                from: Some("warn".into()),
                to: Some("deny".into()),
                reason: "x".into(),
                by: "user".into(),
            },
        )
        .unwrap();
    }
    (dir, paths)
}

use crate::ledger;

#[test]
fn a_proven_rule_becomes_an_anonymized_proposal() {
    let (_dir, paths) = fixture(12, 1, Some(9));
    let (evidence, why, code) = check_rule(&paths, "no-tags", DAY + 100).unwrap();
    assert_eq!((evidence.fired, evidence.deny_days), (12, 9));
    let p = rule_proposal(
        "no-tags",
        &why,
        &code,
        &evidence,
        &["acme".into()],
        "*AI-assisted*",
    );
    assert_eq!(p.title, "Rule: no-tags");
    assert!(p.body.contains("```lua\n--- why:"), "{}", p.body);
    assert!(
        p.body
            .contains("Fired 12 times in 12 sessions; overridden 1 times (8%)"),
        "{}",
        p.body
    );
    assert!(p.body.contains("- `git tag <arg>` (12x)"), "{}", p.body);
    assert!(p.body.ends_with("*AI-assisted*\n"));
    for private in ["/home/alice", "ci.acme.internal", "acme"] {
        assert!(!p.body.contains(private), "{private} leaked:\n{}", p.body);
    }
    assert!(
        p.diff.contains("-") && p.diff.contains("<path>"),
        "{}",
        p.diff
    );
    assert!(
        p.diff
            .contains("---- why: tags are cut by the release job at /home/alice/acme"),
        "{}",
        p.diff
    );
}

#[test]
fn a_rule_that_is_not_proven_is_refused_with_every_reason() {
    let (_dir, paths) = fixture(3, 2, Some(2));
    let problems = check_rule(&paths, "no-tags", DAY + 100).unwrap_err();
    let text = problems.join("\n");
    assert!(text.contains("denied for 2 days"), "{text}");
    assert!(text.contains("fired 3 times"), "{text}");
    assert!(
        text.contains("2 of its 3 firings were overridden"),
        "{text}"
    );

    let (_dir, paths) = fixture(12, 0, None);
    let problems = check_rule(&paths, "no-tags", DAY + 100).unwrap_err();
    assert!(problems[0].contains("no promotion"), "{problems:?}");

    assert!(check_rule(&paths, "git-stash", DAY).unwrap_err()[0].contains("builtin rule"));
    assert!(check_rule(&paths, "nope", DAY).unwrap_err()[0].starts_with("no rule"));
}

#[test]
fn gaps_are_counted_and_become_ideas() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gaps.json");
    let gap = |rule: &str, p: &str| Gap {
        rule: rule.into(),
        primitive: p.into(),
    };
    record(&path, &[gap("a", "pr_status"), gap("b", "ci_state")], DAY).unwrap();
    record(&path, &[gap("a", "pr_status")], DAY + 1).unwrap();
    record_note(
        &path,
        "I kept checking the last test run; there is no primitive for it",
        None,
        DAY,
    )
    .unwrap();
    let gaps = read_gaps(&path);
    let list = ideas(&gaps);
    let keys: Vec<&str> = list.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        ["pr_status", "ci_state", "note-i-kept-checking-the-last"]
    );

    let p = idea_proposal(
        "pr_status",
        &list[0].1,
        &["merging while CI is red".into()],
        &[],
        "f",
    );
    assert_eq!(p.title, "Idea: a `jouet.pr_status` primitive");
    assert!(p.body.contains("2 times (in a)"), "{}", p.body);
    assert!(
        p.body
            .contains("I kept trying to catch: merging while CI is red"),
        "{}",
        p.body
    );
    let note = idea_proposal(&list[2].0, &list[2].1, &[], &[], "f");
    assert!(note.body.contains("> I kept checking"), "{}", note.body);
}

#[test]
fn the_weekly_limit_counts_recent_posts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.jsonl");
    for ts in [DAY - 8 * 86_400, DAY - 86_400, DAY] {
        note_posted(
            &path,
            &Posted {
                ts,
                kind: "rule".into(),
                title: "t".into(),
                url: "u".into(),
            },
        )
        .unwrap();
    }
    assert_eq!(posted_this_week(&path, DAY), 2);
}
