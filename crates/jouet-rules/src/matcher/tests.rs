use super::*;
use crate::Call;

/// Whether the matcher in `toml` matches `command`, run as a shell call.
fn fires(toml: &str, command: &str) -> bool {
    let m: Matcher = toml::from_str(toml).unwrap_or_else(|e| panic!("{toml}: {e}"));
    assert_eq!(m.problem(false), None, "{toml}");
    let call = Call {
        tool: "Bash".into(),
        command: Some(command.into()),
        ..Call::default()
    };
    any(&[m], &call, &crate::call_commands(&call))
}

#[test]
fn a_command_is_its_program_and_subcommands() {
    let m = r#"command = "git stash pop""#;
    assert!(fires(m, "git stash pop"));
    assert!(fires(m, "/usr/bin/git stash pop stash@{1}"));
    assert!(fires(m, "git -C repo -c a=b stash pop"));
    assert!(!fires(m, "git stash"));
    assert!(!fires(m, "git stash list"));
    assert!(!fires(m, "echo git stash pop"));
    assert!(!fires(m, "git commit -m 'git stash pop'"));
    // Options between subcommands are not skipped: `-u` is not `pop`.
    assert!(!fires(m, "git stash -u pop"));
    assert!(fires(r#"command = "cargo test""#, "cargo +1.91 test"));
    assert!(fires(r#"command = ["npm test", "pnpm test"]"#, "pnpm test"));
    assert!(fires(r#"command = "python3.*""#, "python3.12 x.py"));
    assert!(!fires(r#"command = "python3.*""#, "python3 x.py"));
}

#[test]
fn wrappers_are_seen_through_and_recorded_as_via() {
    let m = r#"command = "git add""#;
    for wrapped in [
        "rtk git add .",
        "sudo -u me git add .",
        "env FOO=1 timeout 60 rtk git add .",
        "mise x git@latest -- git add .",
    ] {
        assert!(fires(m, wrapped), "{wrapped}");
    }
    let via = "command = \"git add\"\nvia = \"rtk\"";
    assert!(fires(via, "rtk git add ."));
    assert!(fires(via, "env A=1 rtk git add ."));
    assert!(!fires(via, "git add ."));
    assert!(!fires(via, "echo rtk git add ."));
}

#[test]
fn nested_commands_match_too() {
    let m = r#"command = "git push""#;
    assert!(fires(m, "bash -lc 'git push'"));
    assert!(fires(m, "echo $(git push)"));
    assert!(fires(m, "cd x && git push"));
}

#[test]
fn flags_read_clusters_long_values_and_the_end_of_options() {
    let m = "command = \"git push\"\nflags = [\"-f\", \"--force\"]";
    assert!(fires(m, "git push -f"));
    assert!(fires(m, "git push -uf origin x"));
    assert!(fires(m, "git push --force=yes"));
    assert!(!fires(m, "git push --force-with-lease"));
    assert!(!fires(m, "git push origin -- -f"));
    assert!(!fires(m, "git push"));
    let unless = "command = \"git push\"\nflags = \"-f\"\nunless_flags = \"--force-with-lease\"";
    assert!(!fires(unless, "git push -f --force-with-lease=main"));
}

#[test]
fn value_flags_take_the_next_word_or_the_rest_of_the_cluster() {
    let m = "command = \"git commit\"\nflags = \"-n\"\nvalue_flags = [\"-m\", \"--message\"]";
    assert!(fires(m, "git commit -n -m x"));
    assert!(fires(m, "git commit -nm x"));
    assert!(!fires(m, "git commit -m -n"));
    assert!(!fires(m, "git commit -mn"));
    assert!(!fires(m, "git commit --message -n"));
    assert!(fires(m, "git commit --message=x -n"));
}

#[test]
fn exact_flags_ignore_values_and_order() {
    let m = "command = \"mytool sync\"\nexact_flags = [\"--all\", \"-q\"]";
    assert!(fires(m, "mytool sync -q --all=yes x"));
    assert!(fires(m, "mytool sync --all -q -q"));
    assert!(!fires(m, "mytool sync --all"));
    assert!(!fires(m, "mytool sync --all -q --offline"));
    let none = "command = \"mytool sync\"\nexact_flags = []";
    assert!(fires(none, "mytool sync x"));
    assert!(!fires(none, "mytool sync -v x"));
}

#[test]
fn args_are_the_words_that_are_not_options() {
    let m = "command = \"git add\"\nargs = [\".\", \":/\"]";
    assert!(fires(m, "git add ."));
    assert!(fires(m, "git add -- ."));
    assert!(fires(m, "git add -v :/"));
    assert!(!fires(m, "git add ./src"));
    let glob = "command = \"git push\"\nargs = \"+?*\"";
    assert!(fires(glob, "git push origin +main"));
    assert!(!fires(glob, "git push origin main"));
    let unless = "command = \"rm\"\nunless_args = \"/tmp/*\"";
    assert!(fires(unless, "rm -r build"));
    assert!(!fires(unless, "rm -r /tmp/x"));
    let max = "command = \"git stash drop\"\nmax_args = 0";
    assert!(fires(max, "git stash drop -q"));
    assert!(!fires(max, "git stash drop stash@{1}"));
    let some = "command = \"sleep\"\nargs = \"*\"";
    assert!(fires(some, "sleep 5"));
    assert!(!fires(some, "sleep"));
}

#[test]
fn text_patterns_must_each_match_a_word_or_heredoc() {
    let m = "command = \"python3\"\ntext = ['\\.replace\\(', 'write(_text)?\\(']";
    assert!(fires(
        m,
        "python3 -c \"open('f','w').write(s.replace('a','b'))\""
    ));
    assert!(fires(
        m,
        "python3 - <<'EOF'\np.write_text(p.read_text().replace('a', 'b'))\nEOF"
    ));
    assert!(!fires(m, "python3 -c \"print(s.replace('a','b'))\""));
    // The program itself is not text.
    assert!(!fires("command = \"*\"\ntext = \"^rg$\"", "rg x"));
}

#[test]
fn with_needs_another_command_in_the_same_call() {
    let m = "command = \"gh pr checks\"\nwith = { command = \"sleep\", args = \"*\" }";
    assert!(fires(m, "sleep 60 && gh pr checks"));
    assert!(fires(m, "while true; do gh pr checks; sleep 30; done"));
    assert!(fires(m, "bash -c 'sleep 30; gh pr checks'"));
    assert!(!fires(m, "gh pr checks"));
    assert!(!fires(m, "sleep 60"));
    assert!(!fires(m, "sleep && gh pr checks"));
    assert!(!fires(m, "echo 'sleep 30; gh pr checks'"));
    // `with` names another command, not the one that matched.
    assert!(!fires(
        "command = \"sleep\"\nwith = { command = \"sleep\" }",
        "sleep 5"
    ));
    assert!(fires(
        "command = \"sleep\"\nwith = { command = \"sleep\" }",
        "sleep 5; sleep 6"
    ));
}

#[test]
fn tools_and_paths_match_file_tools() {
    let m: Matcher =
        toml::from_str("tool = [\"Edit\", \"Write\"]\npaths = [\"*.lock\", \"vendor/**\"]")
            .unwrap();
    let call = |tool: &str, path: &str| Call {
        tool: tool.into(),
        path: Some(path.into()),
        cwd: Some("/repo".into()),
        ..Call::default()
    };
    let fires = |c: Call| any(std::slice::from_ref(&m), &c, &[]);
    assert!(fires(call("Edit", "/repo/Cargo.lock")));
    assert!(fires(call("Write", "/repo/vendor/a/b.rs")));
    assert!(!fires(call("Write", "/elsewhere/vendor/a/b.rs")));
    assert!(!fires(call("Read", "/repo/Cargo.lock")));
    assert!(!fires(call("Edit", "/repo/src/main.rs")));
    assert!(!fires(Call {
        tool: "Edit".into(),
        ..Call::default()
    }));
}

#[test]
fn a_matcher_that_says_too_little_is_refused() {
    let problem = |toml: &str| toml::from_str::<Matcher>(toml).unwrap().problem(false);
    assert!(problem("flags = \"-f\"").unwrap().contains("add `command`"));
    assert!(problem("").unwrap().contains("every call"));
    assert!(
        problem("command = \"a\"\nwith = { tool = \"Bash\", command = \"b\" }")
            .unwrap()
            .contains("`with`")
    );
    assert!(
        problem("command = \"a\"\nwith = { flags = \"-x\" }")
            .unwrap()
            .contains("add `command`")
    );
}

#[test]
fn unless_with_refuses_a_call_with_the_other_command() {
    let m = "command = \"rg\"\nunless_with = { command = [\"head\", \"wc\"] }";
    assert!(fires(m, "rg TODO"));
    assert!(fires(m, "rg TODO && echo done"));
    assert!(!fires(m, "rg TODO | head -20"));
    assert!(!fires(m, "rg -n TODO | wc -l"));
    assert!(
        toml::from_str::<Matcher>(
            "command = \"a\"\nunless_with = { tool = \"Bash\", command = \"b\" }"
        )
        .unwrap()
        .problem(false)
        .unwrap()
        .contains("`unless_with`")
    );
}

#[test]
fn input_fields_match_a_tool_s_input() {
    let m: Matcher = toml::from_str(
        "tool = \"Grep\"\ninput = { output_mode = \"content\", multiline = \"true\" }\nunless_input = \"head_limit\"",
    )
    .unwrap();
    assert_eq!(m.problem(false), None);
    let grep = |input: serde_json::Value| Call {
        tool: "Grep".into(),
        input: Some(input),
        ..Call::default()
    };
    let fires = |c: Call| any(std::slice::from_ref(&m), &c, &[]);
    assert!(fires(grep(
        serde_json::json!({"pattern": "x", "output_mode": "content", "multiline": true})
    )));
    // A value that is not a string matches as its JSON text.
    assert!(!fires(grep(
        serde_json::json!({"pattern": "x", "output_mode": "content", "multiline": false})
    )));
    assert!(!fires(grep(
        serde_json::json!({"pattern": "x", "multiline": true})
    )));
    assert!(!fires(grep(serde_json::json!({
        "output_mode": "content", "multiline": true, "head_limit": 20
    }))));
    // A null field is an absent one.
    assert!(fires(grep(serde_json::json!({
        "output_mode": "content", "multiline": true, "head_limit": null
    }))));
    assert!(!fires(Call {
        tool: "Grep".into(),
        ..Call::default()
    }));
}

#[test]
fn seen_matches_what_the_caller_worked_out() {
    let m: Matcher = toml::from_str("tool = \"Read\"\nseen = true").unwrap();
    let read = |seen: Option<bool>| Call {
        tool: "Read".into(),
        path: Some("a.rs".into()),
        seen,
        ..Call::default()
    };
    let fires = |c: Call| any(std::slice::from_ref(&m), &c, &[]);
    assert!(fires(read(Some(true))));
    assert!(!fires(read(Some(false))));
    assert!(!fires(read(None)));
}
