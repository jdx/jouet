use super::*;

#[test]
fn a_command_key_names_what_runs_and_drops_the_plumbing() {
    for (command, key) in [
        (
            "cd crates/x && cargo test -p y 2>&1 | tail -5",
            "cargo test",
        ),
        ("grep -rn 'fn main' src | head -20", "grep"),
        (
            "env -u GH_TOKEN -u GITHUB_TOKEN gh pr view 12 --json state",
            "gh pr view",
        ),
        ("mise run test:e2e cli/use", "mise run test:e2e"),
        ("mise x hk -- hk check --all", "hk check"),
        (
            "git status --short; git log --oneline -3; echo ---; ls src",
            "git status + git log + ls",
        ),
        (
            "for n in 1 2; do gh pr checks $n; done",
            "for + gh pr checks",
        ),
        ("FOO=1 timeout 60 ./scripts/run.sh --fast", "run.sh"),
        ("a; b; c; d", "a + b + c + …"),
        ("echo hi", "echo <v>"),
    ] {
        assert_eq!(command_key(command), key, "{command}");
    }
}

#[test]
fn shell_reads_count_as_reads_of_their_file() {
    let read = |command: &str| read_target(&Call::shell(command));
    assert_eq!(
        read("sed -n '1,80p' src/lib.rs").as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(
        read("cat docs/a.md | head -5").as_deref(),
        Some("docs/a.md")
    );
    assert_eq!(
        read("cd x && nl -ba src/main.rs").as_deref(),
        Some("src/main.rs")
    );
    assert_eq!(read("sed -i 's/a/b/' src/lib.rs"), None);
    assert_eq!(read("cargo test"), None);
}

#[test]
fn an_error_line_says_what_went_wrong() {
    let keep = |s: &str| s.to_string();
    assert_eq!(
        first_line(
            "Exit code 101\n   Compiling x v0.1.0\nerror[E0425]: cannot find value",
            &keep
        )
        .as_deref(),
        Some("error[E0425]: cannot find value")
    );
    assert_eq!(
        first_line(
            "Process exited with code 2\nWall time: 1 seconds\nOutput:\nusage: x",
            &keep
        )
        .as_deref(),
        Some("usage: x")
    );
    assert_eq!(first_line("Exit code 1", &keep), None);
}

#[test]
fn the_session_directory_survives_redaction() {
    let mut t = Transcript {
        session: "s".into(),
        cwd: Some("/home/me/src/app/.claude/worktrees/fix-thing-7ea322".into()),
        ..Default::default()
    };
    let mut call = Call::shell(
        "cat /home/me/src/app/.claude/worktrees/fix-thing-7ea322/src/a.rs ghp_abcdefghijklmnopqrstuvwxyz0123456789",
    );
    call.line = 3;
    t.calls.push(call);
    let d = digest(
        "claude",
        &t,
        "/home/me/.claude/projects/x/s.jsonl",
        &Options {
            home: Some("/home/me"),
            redact: &crate::redact,
        },
    );
    assert_eq!(
        d.project.as_deref(),
        Some("~/src/app/.claude/worktrees/fix-thing-7ea322")
    );
    assert_eq!(d.transcript, "~/.claude/projects/x/s.jsonl");
    let text = render(&d, 4000);
    assert!(text.contains("src/a.rs"), "{text}");
    assert!(!text.contains("ghp_"), "{text}");
}

#[test]
fn a_single_failed_search_is_not_a_failure_worth_listing() {
    let mut t = Transcript::default();
    for (command, failed) in [
        ("grep -rn nothing src", true),
        ("cargo build", true),
        ("cargo build", false),
    ] {
        let mut call = Call::shell(command);
        call.failed = failed;
        t.calls.push(call);
    }
    let d = digest(
        "claude",
        &t,
        "x",
        &Options {
            home: None,
            redact: &crate::redact,
        },
    );
    let listed: Vec<(&str, bool)> = d
        .failures
        .iter()
        .map(|f| (f.what.as_str(), f.recovered))
        .collect();
    assert_eq!(listed, [("cargo build", true)]);
}
