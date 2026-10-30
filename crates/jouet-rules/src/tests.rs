use super::*;

fn engine(sources: Vec<Source>) -> Engine {
    engine_with(sources, Budget::default(), Vec::new())
}

fn engine_with(sources: Vec<Source>, budget: Budget, disabled: Vec<String>) -> Engine {
    Engine::new(
        sources,
        Options {
            has_command: Box::new(|_| true),
            disabled,
            budget,
            cache_dir: None,
        },
    )
    .unwrap()
}

fn user(name: &str, code: &str) -> Source {
    Source {
        name: name.into(),
        origin: Origin::User(PathBuf::from(format!("{name}.luau"))),
        code: code.into(),
        trusted: true,
    }
}

fn bash(command: &str) -> Call {
    Call {
        event: Some(Event::PreToolUse),
        harness: "test".into(),
        tool: "Bash".into(),
        command: Some(command.into()),
        ..Call::default()
    }
}

#[test]
fn every_builtin_rule_loads_and_passes_its_examples() {
    let engine = engine(builtin());
    for rule in engine.rules() {
        assert_eq!(rule.state, State::Enabled, "{}", rule.name);
        assert!(!rule.description.is_empty(), "{}", rule.name);
        let examples = engine.examples(rule).unwrap();
        assert!(examples.len() >= 4, "{} has too few examples", rule.name);
        for example in examples {
            assert!(example.ok, "{}: {example:?}", rule.name);
        }
    }
}

#[test]
fn the_first_denial_wins_and_names_its_rule() {
    let outcome = engine(builtin()).evaluate(&bash("git add -A && git stash"));
    let deny = outcome.deny.unwrap();
    assert_eq!(deny.rule, "git-add-all");
    assert!(deny.message.unwrap().starts_with("Stage explicit paths"));
    assert!(outcome.errors.is_empty());
}

#[test]
fn post_tool_use_rules_record_checks() {
    let call = Call {
        event: Some(Event::PostToolUse),
        exit_code: Some(101),
        ..bash("cd crate && cargo test -p jouet")
    };
    let outcome = engine(builtin()).evaluate(&call);
    assert_eq!(
        outcome.records,
        [Record {
            rule: "checks".into(),
            kind: Kind::Test,
            command: "cargo test -p jouet".into(),
            exit_code: 101,
        }]
    );
}

#[test]
fn disabled_rules_and_missing_commands_do_not_run() {
    let engine = Engine::new(
        builtin(),
        Options {
            has_command: Box::new(|path| path != ["wait", "ci"]),
            disabled: vec!["git-stash".into()],
            budget: Budget::default(),
            cache_dir: None,
        },
    )
    .unwrap();
    assert_eq!(engine.rule("git-stash").unwrap().state, State::Disabled);
    assert_eq!(
        engine.rule("sleep-poll").unwrap().state,
        State::Needs("jouet wait ci".into())
    );
    assert!(engine.evaluate(&bash("git stash")).deny.is_none());
    assert!(
        engine
            .evaluate(&bash("sleep 9; gh pr checks"))
            .deny
            .is_none()
    );
}

#[test]
fn a_user_rule_can_warn_and_use_the_whole_api() {
    let rule = user(
        "no-main",
        r#"
        return {
            description = "Warn before touching main",
            check = function(call)
                for _, c in call.commands do
                    local sub, args = jouet.git(c)
                    if sub == "checkout" and jouet.contains(args, "main") then
                        return jouet.warn("you are on " .. jouet.basename("/x/main"))
                    end
                end
                assert(jouet.has_command("edit"))
                assert(#jouet.parse("a | b") == 2)
                return jouet.allow()
            end,
        }
        "#,
    );
    let engine = engine(vec![rule]);
    let outcome = engine.evaluate(&bash("git checkout main"));
    assert_eq!(
        outcome.warnings[0].message.as_deref(),
        Some("you are on main")
    );
    assert!(engine.evaluate(&bash("ls")).warnings.is_empty());
}

#[test]
fn broken_rules_are_kept_and_explained() {
    let engine = engine(vec![
        user("syntax", "return {"),
        user("no-check", "return { description = 'x' }"),
        user("not-a-table", "return 1"),
        user("bad-on", "return { on = 'later', check = function() end }"),
    ]);
    for rule in engine.rules() {
        assert!(
            matches!(rule.state, State::Broken(_)),
            "{}: {:?}",
            rule.name,
            rule.state
        );
    }
}

#[test]
fn a_failing_rule_fails_open() {
    let engine = engine(vec![
        user("throws", "return { check = function() error('boom') end }"),
        user("bad-return", "return { check = function() return 42 end }"),
        user(
            "bad-kind",
            "return { on = 'post-tool-use', check = function(c) jouet.record('docs', c.commands[1]) end }",
        ),
    ]);
    let outcome = engine.evaluate(&bash("ls"));
    assert!(outcome.deny.is_none());
    assert_eq!(outcome.errors.len(), 2, "{:?}", outcome.errors);
    assert!(outcome.errors[0].contains("boom"), "{:?}", outcome.errors);
}

#[test]
fn budgets_stop_runaway_rules() {
    let budget = Budget {
        time: Duration::from_millis(20),
        ..Budget::default()
    };
    let engine = engine_with(
        vec![
            user(
                "spin",
                "return { check = function() while true do end end }",
            ),
            user(
                "deny",
                "return { check = function() return jouet.deny('still runs') end }",
            ),
        ],
        budget,
        Vec::new(),
    );
    let start = Instant::now();
    let outcome = engine.evaluate(&bash("ls"));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(outcome.errors[0].contains("budget"), "{:?}", outcome.errors);
    // The next rule still runs.
    assert_eq!(outcome.deny.unwrap().rule, "deny");

    let engine = engine_with(
        vec![user(
            "spin",
            "return { check = function() while true do end end }",
        )],
        Budget {
            interrupts: 1_000,
            time: Duration::from_secs(10),
            ..Budget::default()
        },
        Vec::new(),
    );
    let outcome = engine.evaluate(&bash("ls"));
    assert!(
        outcome.errors[0].contains("instruction budget"),
        "{:?}",
        outcome.errors
    );
}

#[test]
fn a_spinning_file_is_broken_not_hung() {
    let engine = engine_with(
        vec![user("spin", "while true do end")],
        Budget {
            time: Duration::from_millis(20),
            ..Budget::default()
        },
        Vec::new(),
    );
    assert!(matches!(&engine.rules()[0].state, State::Broken(e) if e.contains("budget")));
}

#[test]
fn the_sandbox_holds() {
    let engine = engine(vec![
        user(
            "module",
            "jouet.deny = nil; return { check = function() end }",
        ),
        user("require", "require('x'); return { check = function() end }"),
        user(
            "io",
            "io.open('/etc/passwd'); return { check = function() end }",
        ),
        user(
            "os",
            "os.execute('true'); return { check = function() end }",
        ),
    ]);
    for rule in engine.rules() {
        assert!(
            matches!(rule.state, State::Broken(_)),
            "{}: {:?}",
            rule.name,
            rule.state
        );
    }
}

#[test]
fn rules_do_not_share_globals() {
    let engine = engine(vec![
        user("a", "leak = 1; return { check = function() end }"),
        user(
            "b",
            "return { check = function() if leak then return jouet.deny('leaked') end end }",
        ),
    ]);
    assert!(engine.evaluate(&bash("ls")).deny.is_none());
}

#[test]
fn discovery_layers_user_and_trusted_repository_rules() {
    let dir = tempfile::tempdir().unwrap();
    let user_dir = dir.path().join("user");
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(&repo_dir).unwrap();
    std::fs::write(
        user_dir.join("git-stash.luau"),
        "return { check = function() end }",
    )
    .unwrap();
    std::fs::write(
        repo_dir.join("team.luau"),
        "return { check = function() end }",
    )
    .unwrap();
    let mut trust = TrustStore::load(dir.path().join("trust.json"));

    let (sources, errors) = discover(Some(&user_dir), Some(&repo_dir), &trust);
    assert!(errors.is_empty());
    let stash = sources.iter().find(|s| s.name == "git-stash").unwrap();
    assert!(
        matches!(stash.origin, Origin::User(_)),
        "the user's rule replaces the builtin"
    );
    let team = sources.iter().find(|s| s.name == "team").unwrap();
    assert!(!team.trusted);

    trust.trust(&repo_dir).unwrap();
    let (sources, _) = discover(Some(&user_dir), Some(&repo_dir), &trust);
    assert!(sources.iter().find(|s| s.name == "team").unwrap().trusted);
}

#[test]
fn an_untrusted_repository_rule_never_replaces_or_runs() {
    let mut untrusted = user("git-add-all", "return { check = function() end }");
    untrusted.origin = Origin::Repo(PathBuf::from(".jouet/rules/git-add-all.luau"));
    untrusted.trusted = false;
    let mut sources = builtin();
    sources.push(untrusted);
    let engine = engine(sources);
    let states: Vec<_> = engine
        .rules()
        .iter()
        .filter(|r| r.name == "git-add-all")
        .map(|r| r.state.clone())
        .collect();
    assert_eq!(states, [State::Enabled, State::Untrusted]);
    assert!(engine.evaluate(&bash("git add -A")).deny.is_some());
}

#[test]
fn compiled_rules_are_cached() {
    let dir = tempfile::tempdir().unwrap();
    let code = "return { check = function() return jouet.deny('x') end }";
    let first = cache::compile(code, Some(dir.path())).unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(cache::compile(code, Some(dir.path())).unwrap(), first);
    assert!(cache::compile("return {", Some(dir.path())).is_err());
}

#[test]
fn calls_are_read_from_hook_payloads() {
    let call = Call::from_hook(
        &serde_json::json!({"hook_event_name": "PostToolUseFailure", "tool_name": "Bash",
            "tool_input": {"command": "cargo test"}, "cwd": "/w", "error": "Exit code 3\nboom"}),
        "claude",
    );
    assert_eq!(call.event, Some(Event::PostToolUse));
    assert_eq!(call.exit_code, Some(3));
    assert_eq!(call.command.as_deref(), Some("cargo test"));
    assert_eq!(call.cwd.as_deref(), Some("/w"));

    let call = Call::from_hook(
        &serde_json::json!({"hook_event_name": "PostToolUse"}),
        "claude",
    );
    assert_eq!(call.exit_code, Some(0));
    let call = Call::from_hook(
        &serde_json::json!({"tool_name": "Bash", "exit_code": 7}),
        "codex",
    );
    assert_eq!((call.event, call.exit_code), (None, Some(7)));
}
