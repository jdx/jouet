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
}

/// Examples of both kinds, which a rule of yours needs before it may deny.
/// Loading only checks they are there; `jouet rule test` runs them.
const EXAMPLES: &str = "examples = { fires = { 'x' }, quiet = { 'y' } }";

/// A user rule whose header makes it deny, with examples so that it may, as
/// most tests need.
fn user(name: &str, code: &str) -> Source {
    let code = code.replacen("return {", &format!("return {{ {EXAMPLES},"), 1);
    user_raw(
        name,
        &format!("--- why: a test rule\n--- mode: deny\n{code}"),
    )
}

/// A user rule exactly as written, header and all.
fn user_raw(name: &str, code: &str) -> Source {
    Source::luau(
        name,
        Origin::User(PathBuf::from(format!("{name}.luau"))),
        code,
    )
}

/// A user rule file in TOML.
fn toml_user(file: &str, code: &str) -> Source {
    Source::toml(
        file,
        Origin::User(PathBuf::from(format!("{file}.toml"))),
        code,
    )
}

/// A TOML rule with examples of both kinds, which match it.
const DENY_LS: &str = r#"[[rule]]
name = "no-ls"
why = "a test rule"
mode = "deny"
message = "use eza"
suggest = "eza"
[rule.match]
command = "ls"
[rule.examples]
fires = ["ls -la"]
quiet = ["eza"]
"#;

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
        assert!(!rule.why.is_empty(), "{}", rule.name);
        assert_eq!(
            (rule.scope, rule.author.as_str()),
            (Scope::Builtin, "builtin")
        );
        assert_eq!(rule.missing_examples, None, "{}", rule.name);
        let examples = engine.examples(rule).unwrap();
        let fires = examples.iter().filter(|e| e.fires).count();
        assert!(fires >= 1, "{} has no fires example", rule.name);
        assert!(
            examples.len() > fires,
            "{} has no quiet near miss",
            rule.name
        );
        for example in &examples {
            assert!(example.ok, "{}: {example:?}", rule.name);
        }
        assert_eq!(engine.proven(rule), Ok(examples.len()));
    }
}

#[test]
fn the_first_denial_wins_and_names_its_rule() {
    let outcome = engine(builtin()).evaluate(&bash("git add -A && git stash"));
    let deny = outcome.deny.unwrap();
    assert_eq!(deny.rule, "git-add-all");
    assert_eq!(deny.suggest.as_deref(), Some("git add path/to/file ..."));
    assert!(
        deny.message
            .unwrap()
            .starts_with("Run `git add path/to/file ...` instead. `git add -A`")
    );
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
            full: false,
            selector: Some("-p jouet".into()),
            dir: Some("crate".into()),
        }]
    );
}

#[test]
fn a_long_check_is_recorded_whole() {
    let long = format!(
        "cargo test -p jouet {}",
        "verify::tests::a_case ".repeat(6).trim_end()
    );
    assert!(long.len() > 80);
    let call = Call {
        event: Some(Event::PostToolUse),
        exit_code: Some(101),
        ..bash(&long)
    };
    let outcome = engine(builtin()).evaluate(&call);
    assert_eq!(outcome.records[0].command, long);
    // Through `jouet test --cmd` too.
    let wrapped = format!("jouet test --cmd -- {long}");
    let call = Call {
        event: Some(Event::PostToolUse),
        exit_code: Some(1),
        ..bash(&wrapped)
    };
    let outcome = engine(builtin()).evaluate(&call);
    assert_eq!(outcome.records[0].command, wrapped);
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
    );
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
    let rule = "--- why: test\nreturn { check = function() end }";
    std::fs::write(user_dir.join("mine.luau"), rule).unwrap();
    std::fs::write(repo_dir.join("team.luau"), rule).unwrap();
    let dirs = |trusted| {
        [
            RuleDir {
                dir: user_dir.clone(),
                origin: Origin::User,
                trusted: true,
                namespace: None,
            },
            RuleDir {
                dir: repo_dir.clone(),
                origin: Origin::Repo,
                trusted,
                namespace: None,
            },
        ]
    };

    let (sources, errors) = discover(&dirs(false));
    assert!(errors.is_empty());
    let mine = sources.iter().find(|s| s.name == "mine").unwrap();
    assert!(matches!(mine.origin, Origin::User(_)) && mine.trusted);
    let team = sources.iter().find(|s| s.name == "team").unwrap();
    assert!(matches!(team.origin, Origin::Repo(_)) && !team.trusted);

    let (sources, _) = discover(&dirs(true));
    assert!(sources.iter().find(|s| s.name == "team").unwrap().trusted);
}

#[test]
fn a_plugins_rules_are_named_after_it_so_they_take_no_other_rules_name() {
    let dir = tempfile::tempdir().unwrap();
    let (mine, plugin) = (dir.path().join("mine"), dir.path().join("acme"));
    let rule = "[[rule]]\nname = \"no-ls\"\nwhy = \"w\"\nmessage = \"no\"\nmatch = { command = \"ls\" }\nexamples = { fires = [\"ls\"], quiet = [\"pwd\"] }\n";
    for d in [&mine, &plugin] {
        std::fs::create_dir(d).unwrap();
        std::fs::write(d.join("no-ls.toml"), rule).unwrap();
    }
    let (sources, errors) = discover(&[
        RuleDir {
            dir: mine,
            origin: Origin::User,
            trusted: true,
            namespace: None,
        },
        RuleDir {
            dir: plugin,
            origin: Origin::Plugin,
            trusted: true,
            namespace: Some("acme".into()),
        },
    ]);
    assert!(errors.is_empty());
    let engine = engine(sources);
    for name in ["no-ls", "acme/no-ls"] {
        assert_eq!(engine.rule(name).unwrap().state, State::Enabled, "{name}");
    }
    assert_eq!(engine.rule("acme/no-ls").unwrap().scope, Scope::Plugin);
    // Yours warns, as a rule of yours does by default; the plugin's denies.
    let outcome = engine.evaluate(&bash("ls"));
    assert_eq!(outcome.deny.unwrap().rule, "acme/no-ls");
    assert_eq!(outcome.warnings[0].rule, "no-ls");
}

#[test]
fn the_settings_cannot_turn_off_an_enforced_rule() {
    let rule = "[[rule]]\nname = \"org\"\nwhy = \"policy\"\nmessage = \"no\"\nscope = \"repo\"\nmatch = { command = \"ls\" }\nexamples = { fires = [\"ls\"], quiet = [\"pwd\"] }\n";
    let source = |origin: Origin| Source::toml("org", origin, rule);
    for disabled in [vec!["org".to_string()], vec!["all".to_string()]] {
        let engine = engine_with(
            vec![source(Origin::Enforced(PathBuf::from(
                "/etc/jouet/rules/org.toml",
            )))],
            Budget::default(),
            disabled.clone(),
        );
        let org = engine.rule("org").unwrap();
        assert_eq!(
            (org.state.clone(), org.scope, org.mode),
            (State::Enabled, Scope::Enforced, Mode::Deny)
        );
        assert!(engine.evaluate(&bash("ls")).deny.is_some());

        // A plugin's rule, written for a repository, is turned off as usual.
        let engine = engine_with(
            vec![source(Origin::Plugin(PathBuf::from(
                "/plugins/x/rules/org.toml",
            )))],
            Budget::default(),
            disabled,
        );
        assert_eq!(engine.rule("org").unwrap().state, State::Disabled);
    }
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
    let (first, cached) = cache::compile(code, Some(dir.path())).unwrap();
    assert!(!cached);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(
        cache::compile(code, Some(dir.path())).unwrap(),
        (first, true)
    );
    assert!(cache::compile("return {", Some(dir.path())).is_err());
}

#[test]
fn a_damaged_cache_entry_is_recompiled() {
    let dir = tempfile::tempdir().unwrap();
    let code = "return { check = function() return jouet.deny('x') end }";
    cache::compile(code, Some(dir.path())).unwrap();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        std::fs::write(entry.unwrap().path(), b"not bytecode").unwrap();
    }
    let engine = Engine::new(
        vec![user("cached", code)],
        Options {
            has_command: Box::new(|_| true),
            disabled: Vec::new(),
            budget: Budget::default(),
            cache_dir: Some(dir.path().to_path_buf()),
        },
    );
    assert_eq!(engine.rules()[0].state, State::Enabled);
    assert!(engine.evaluate(&bash("ls")).deny.is_some());
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

#[test]
fn a_users_rule_warns_unless_its_header_says_deny() {
    let code = format!("return {{ {EXAMPLES}, check = function() return jouet.deny('no') end }}");
    let engine = engine(vec![
        user_raw("soft", &format!("--- why: x\n{code}")),
        user_raw("hard", &format!("--- why: x\n--- mode: deny\n{code}")),
    ]);
    assert_eq!(engine.rule("soft").unwrap().mode, Mode::Warn);
    let outcome = engine.evaluate(&bash("ls"));
    assert_eq!(outcome.warnings[0].rule, "soft");
    assert_eq!(outcome.deny.unwrap().rule, "hard");

    let mut repo = user_raw("team", &format!("--- why: x\n{code}"));
    repo.origin = Origin::Repo(PathBuf::from("team.luau"));
    let engine = engine_with(vec![repo], Budget::default(), Vec::new());
    assert_eq!(engine.rule("team").unwrap().mode, Mode::Deny);
}

#[test]
fn a_rule_without_examples_of_both_kinds_only_warns() {
    let check = "check = function() return jouet.deny('no') end";
    let rule = |name: &str, examples: &str| {
        user_raw(
            name,
            &format!("--- why: x\n--- mode: deny\nreturn {{ {check}, {examples} }}"),
        )
    };
    let mut team = user_raw("team", &format!("--- why: x\nreturn {{ {check} }}"));
    team.origin = Origin::Repo(PathBuf::from("team.luau"));
    let mut builtin_like = user_raw("core", &format!("--- why: x\nreturn {{ {check} }}"));
    builtin_like.origin = Origin::Builtin;
    let engine = engine(vec![
        rule("none", "x = 1"),
        rule("fires-only", "examples = { fires = { 'ls' } }"),
        rule("quiet-only", "examples = { fires = {}, quiet = { 'ls' } }"),
        rule(
            "old-keys",
            "examples = { deny = { 'ls' }, allow = { 'pwd' } }",
        ),
        rule(
            "proven",
            "examples = { fires = { 'ls' }, quiet = { 'pwd' } }",
        ),
        team,
        builtin_like,
    ]);
    let missing = |name: &str| {
        let rule = engine.rule(name).unwrap();
        assert_eq!(rule.state, State::Enabled, "{name}");
        (rule.mode, rule.missing_examples.clone().unwrap_or_default())
    };
    assert_eq!(missing("none"), (Mode::Warn, "it has no examples".into()));
    assert_eq!(missing("team"), (Mode::Warn, "it has no examples".into()));
    assert!(missing("fires-only").1.contains("no `quiet` examples"));
    assert!(missing("quiet-only").1.contains("no `fires` examples"));
    let (mode, old) = missing("old-keys");
    assert_eq!(mode, Mode::Warn);
    assert!(
        old.contains("listed under `deny`") && old.contains("`fires`"),
        "{old}"
    );
    // Built-in rules are held to their examples by tests, not at load.
    assert_eq!(missing("core").0, Mode::Deny);
    assert_eq!(missing("proven"), (Mode::Deny, String::new()));

    let outcome = engine.evaluate(&bash("ls"));
    assert_eq!(outcome.deny.unwrap().rule, "proven");
    let warned: Vec<&str> = outcome.warnings.iter().map(|w| w.rule.as_str()).collect();
    assert_eq!(warned, ["none", "fires-only", "quiet-only", "old-keys"]);

    // What it has still runs, and it is not proven until it has both.
    let fires_only = engine.rule("fires-only").unwrap();
    assert_eq!(engine.examples(fires_only).unwrap().len(), 1);
    assert!(engine.proven(fires_only).unwrap_err().contains("quiet"));
    // `proven`'s quiet example denies too, so it fails.
    let proven = engine.rule("proven").unwrap();
    assert_eq!(
        engine.proven(proven),
        Err("1 of its 2 examples fail".into())
    );
}

#[test]
fn examples_are_tool_calls_in_compact_form() {
    let engine = engine(vec![user_raw(
        "fetch",
        r#"--- why: x
return {
	check = function(call)
		if call.tool == "WebFetch" then return jouet.warn("use the cache") end
		if call.harness == "codex" and call.command == "ls" then return jouet.deny("codex") end
		return nil
	end,
	examples = {
		fires = {
			{ tool = "WebFetch", action = "warn" },
			{ command = "ls", harness = "codex", action = "deny" },
			{ command = "ls", harness = "codex", action = "warn" },
		},
		quiet = { "ls", { tool = "Read", harness = "claude" } },
	},
}"#,
    )]);
    let rule = engine.rule("fetch").unwrap();
    assert_eq!(rule.missing_examples, None);
    assert_eq!(
        rule.examples[1],
        Case {
            command: Some("ls".into()),
            tool: "Bash".into(),
            path: None,
            harness: "codex".into(),
            exit_code: None,
            expect: Expect::Fires {
                action: Some(Action::Deny),
                kind: None,
            },
            scope: None,
            input: None,
            seen: None,
            read_lines: None,
            code_lines: None,
            tracked_files: None,
            fnox: None,
            git_dirty: None,
            session: None,
            claims: None,
            git_branch: None,
            git_ahead: None,
            stack: None,
            secrets: None,
        }
    );
    let results: Vec<(String, String, String, bool)> = engine
        .examples(rule)
        .unwrap()
        .into_iter()
        .map(|e| (e.label(), e.expected, e.actual, e.ok))
        .collect();
    let row = |label: &str, expected: &str, actual: &str, ok: bool| {
        (
            label.to_string(),
            expected.to_string(),
            actual.to_string(),
            ok,
        )
    };
    assert_eq!(
        results,
        [
            row("(tool WebFetch)", "fires warn", "warn", true),
            row("ls (harness codex)", "fires deny", "deny", true),
            row("ls (harness codex)", "fires warn", "deny", false),
            row("ls", "quiet", "quiet", true),
            row("(tool Read, harness claude)", "quiet", "quiet", true),
        ]
    );
}

#[test]
fn malformed_examples_say_where() {
    let cases = [
        ("examples = 'ls'", "`examples` must be a table"),
        (
            "examples = { fires = 'ls', quiet = { 'x' } }",
            "`examples.fires` must be a list",
        ),
        (
            "examples = { fire = { 'ls' } }",
            "`examples.fire` is not `fires` or `quiet`",
        ),
        (
            "examples = { fires = { 1 }, quiet = { 'x' } }",
            "`examples.fires[1]`: an example is a command or a table",
        ),
        (
            "examples = { fires = { { cmd = 'ls' } }, quiet = { 'x' } }",
            "unknown key `cmd`",
        ),
        (
            "examples = { fires = { {} }, quiet = { 'x' } }",
            "needs a `command`",
        ),
        (
            "examples = { fires = { 'ls' }, quiet = { { command = 'x', action = 'deny' } } }",
            "`examples.quiet[1]`: a `quiet` example takes no",
        ),
        (
            "examples = { fires = { { command = 'ls', action = 'block' } }, quiet = { 'x' } }",
            "`action` must be",
        ),
        (
            "examples = { fires = { { command = 'ls', kind = 'bench' } }, quiet = { 'x' } }",
            "`kind` must be",
        ),
    ];
    let sources = cases
        .iter()
        .enumerate()
        .map(|(i, (examples, _))| {
            user_raw(
                &format!("r{i}"),
                &format!("--- why: x\nreturn {{ check = function() end, {examples} }}"),
            )
        })
        .collect();
    let engine = engine(sources);
    for (i, (_, want)) in cases.iter().enumerate() {
        let rule = engine.rule(&format!("r{i}")).unwrap();
        assert_eq!(rule.state, State::Enabled);
        let problem = rule.missing_examples.clone().unwrap_or_default();
        assert!(problem.contains(want), "{i}: {problem}");
        assert!(rule.examples.is_empty());
    }
}

#[test]
fn headers_are_checked_against_the_file() {
    let check = "return { check = function() end }";
    let engine = engine(vec![
        user_raw("no-why", check),
        user_raw("misnamed", &format!("--- name: other\n--- why: x\n{check}")),
        user_raw(
            "claims-builtin",
            &format!("--- why: x\n--- scope: builtin\n{check}"),
        ),
        user_raw("bad-mode", &format!("--- why: x\n--- mode: loud\n{check}")),
        user_raw(
            "good",
            &format!(
                "--- name: good\n--- why: x\n--- scope: user\n--- author: session-1\n--- created: 2026-10-30\n{check}"
            ),
        ),
    ]);
    for rule in &engine.rules()[..4] {
        assert!(
            matches!(rule.state, State::Broken(_)),
            "{}: {:?}",
            rule.name,
            rule.state
        );
    }
    let good = engine.rule("good").unwrap();
    assert_eq!(good.state, State::Enabled);
    assert_eq!(
        (good.author.as_str(), good.created.as_str()),
        ("session-1", "2026-10-30")
    );
}

#[test]
fn rules_of_yours_add_decisions_and_never_weaken_the_builtin_ones() {
    let mut sources = builtin();
    // Same name as a built-in: refused, and the built-in still runs.
    sources.push(user(
        "git-add-all",
        "return { check = function() return jouet.allow() end }",
    ));
    // An explicit allow decides nothing for the other rules.
    sources.push(user(
        "allow-all",
        "return { check = function() return jouet.allow() end }",
    ));
    // A rule that would deny everything, jouet included.
    sources.push(user(
        "deny-all",
        "return { check = function() return jouet.deny('no') end }",
    ));
    let engine = engine(sources);

    let replaced = engine
        .rules()
        .iter()
        .filter(|r| r.name == "git-add-all")
        .map(|r| (r.scope, matches!(r.state, State::Broken(_))))
        .collect::<Vec<_>>();
    assert_eq!(replaced, [(Scope::Builtin, false), (Scope::User, true)]);
    assert_eq!(
        engine.evaluate(&bash("git add -A")).deny.unwrap().rule,
        "git-add-all"
    );

    // It cannot lock you out of jouet: its denial of jouet itself only warns.
    let outcome = engine.evaluate(&bash("jouet rule list && jouet setup claude --uninstall"));
    assert!(outcome.deny.is_none());
    assert_eq!(outcome.warnings[0].rule, "deny-all");
    assert_eq!(engine.evaluate(&bash("ls")).deny.unwrap().rule, "deny-all");
}

#[test]
fn a_rules_metamethods_never_run_outside_its_budget() {
    let spin = "setmetatable({}, { __index = function() while true do end end })";
    let engine = engine_with(
        vec![
            // The rule table itself: its fields are read raw, so `check` is
            // simply missing.
            user("meta-rule", &format!("return {spin}")),
            // The decision: `action` and `message` are read raw too.
            user(
                "meta-decision",
                &format!("return {{ check = function() return {spin} end }}"),
            ),
            user(
                "deny",
                "return { check = function() return jouet.deny('still runs') end }",
            ),
        ],
        Budget {
            time: Duration::from_millis(20),
            ..Budget::default()
        },
        Vec::new(),
    );
    let start = Instant::now();
    assert!(matches!(&engine.rules()[0].state, State::Broken(e) if e.contains("check")));
    let outcome = engine.evaluate(&bash("ls"));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        outcome.errors[0].contains("unknown action"),
        "{:?}",
        outcome.errors
    );
    assert_eq!(outcome.deny.unwrap().rule, "deny");
}

#[test]
fn a_warn_mode_rule_passes_its_deny_examples_with_a_warning() {
    let examples =
        "examples = { fires = { { command = 'rm -r x', action = 'deny' } }, quiet = { 'ls' } }";
    let engine = engine(vec![
        // Denies, so its deny example passes, though enforced it only warns.
        user_raw(
            "soft",
            &format!(
                "--- why: x\nreturn {{ check = function(c) if c.commands[1].program == 'rm' then return jouet.deny('no') end end, {examples} }}"
            ),
        ),
        // Only ever warns, so a deny example still catches that.
        user_raw(
            "warns",
            &format!(
                "--- why: x\nreturn {{ check = function(c) if c.commands[1].program == 'rm' then return jouet.warn('no') end end, {examples} }}"
            ),
        ),
    ]);
    let soft = engine.rule("soft").unwrap();
    assert_eq!(soft.mode, Mode::Warn);
    assert!(engine.examples(soft).unwrap().iter().all(|e| e.ok));
    let (decision, _) = engine.check(soft, &bash("rm -r x")).unwrap();
    assert_eq!(decision.unwrap().action, Action::Warn);

    let warns = engine.examples(engine.rule("warns").unwrap()).unwrap();
    assert!(
        warns
            .iter()
            .any(|e| !e.ok && e.expected == "fires deny" && e.actual == "warn")
    );
}

#[test]
fn a_missing_primitive_is_recorded_as_a_gap() {
    let engine = engine(vec![user(
        "wants-ci",
        "return { check = function(c) if jouet.ci_state and jouet.ci_state() == 'red' then return jouet.deny('red') end return jouet.pr_status(1) end }",
    )]);
    let outcome = engine.evaluate(&bash("git push"));
    // The call to a missing function fails the rule, which fails open.
    assert!(outcome.deny.is_none());
    assert_eq!(outcome.errors.len(), 1);
    let gaps: Vec<String> = engine
        .take_gaps()
        .into_iter()
        .map(|g| format!("{}:{}", g.rule, g.primitive))
        .collect();
    assert_eq!(gaps, ["wants-ci:ci_state", "wants-ci:pr_status"]);
    assert!(engine.take_gaps().is_empty());
    // Existing primitives are not gaps.
    let fine = engine_with(
        vec![user(
            "fine",
            "return { check = function(c) local _ = jouet.git; return nil end }",
        )],
        Budget::default(),
        Vec::new(),
    );
    fine.evaluate(&bash("ls"));
    assert!(fine.take_gaps().is_empty());
}

#[test]
fn matcher_rules_never_start_a_vm() {
    let engine = engine(builtin());
    for command in [
        // Not `cargo test`: affected-tests asks its script about a whole
        // workspace run, as it does about pytest's.
        "git status && cargo build --workspace",
        "git add -A",
        "sleep 60 && gh pr checks",
        "python3 -c 'print(1)'",
    ] {
        engine.evaluate(&bash(command));
    }
    assert!(!engine.vm_started());
    // `checks` runs after a tool, behind a matcher of the programs it knows.
    let post = |command: &str| Call {
        event: Some(Event::PostToolUse),
        exit_code: Some(0),
        ..bash(command)
    };
    assert!(engine.evaluate(&post("ls -la src")).records.is_empty());
    assert!(!engine.vm_started());
    assert_eq!(engine.evaluate(&post("cargo test")).records.len(), 1);
    assert!(engine.vm_started());
}

#[test]
fn a_toml_rule_denies_with_its_suggestion_in_front() {
    let engine = engine(vec![toml_user("mine", DENY_LS)]);
    let rule = engine.rule("no-ls").unwrap();
    assert_eq!(
        (rule.format, rule.line, rule.mode),
        (Format::Toml, 1, Mode::Deny)
    );
    assert_eq!(rule.deprecation(), None);
    let deny = engine.evaluate(&bash("cd x && ls -la")).deny.unwrap();
    assert_eq!(deny.message.as_deref(), Some("Run `eza` instead. use eza"));
    assert_eq!(deny.suggest.as_deref(), Some("eza"));
    assert!(engine.evaluate(&bash("echo ls")).deny.is_none());
    assert_eq!(engine.proven(rule), Ok(2));
}

#[test]
fn a_toml_rule_of_yours_warns_until_it_has_examples_of_both_kinds() {
    let code = DENY_LS.replace("quiet = [\"eza\"]\n", "");
    let engine = engine(vec![toml_user("mine", &code)]);
    let rule = engine.rule("no-ls").unwrap();
    assert_eq!(rule.mode, Mode::Warn);
    assert!(
        rule.missing_examples
            .as_deref()
            .unwrap()
            .contains("`quiet`")
    );
    let outcome = engine.evaluate(&bash("ls"));
    assert!(outcome.deny.is_none());
    assert_eq!(outcome.warnings.len(), 1);
    // Without `mode`, a rule of yours warns.
    let code = DENY_LS.replace("mode = \"deny\"\n", "");
    let unset = self::engine(vec![toml_user("mine", &code)]);
    assert_eq!(unset.rule("no-ls").unwrap().mode, Mode::Warn);
}

#[test]
fn a_broken_toml_rule_names_its_file_and_line() {
    let engine = engine(vec![
        toml_user(
            "typo",
            "[[rule]]\nname = \"a\"\nwhy = \"w\"\n[rule.match]\ncomand = \"ls\"\n",
        ),
        toml_user(
            "two",
            &format!(
                "{DENY_LS}\n[[rule]]\nname = \"no-msg\"\nwhy = \"w\"\nmatch = {{ command = \"rm\" }}\n"
            ),
        ),
        toml_user("empty", "# nothing yet\n"),
    ]);
    let state = |name: &str| match &engine.rule(name).unwrap().state {
        State::Broken(e) => e.clone(),
        other => panic!("{name}: {other:?}"),
    };
    let typo = state("typo");
    assert!(
        typo.starts_with("typo.toml:5: unknown field `comand`"),
        "{typo}"
    );
    assert!(
        state("no-msg").starts_with("two.toml:13: give it a `message`"),
        "{}",
        state("no-msg")
    );
    assert!(state("empty").contains("no `[[rule]]` tables"));
    // The file's other rule loads.
    assert_eq!(engine.rule("no-ls").unwrap().state, State::Enabled);
}

#[test]
fn rule_keys_that_do_not_fit_together_are_refused() {
    let broken = |rule: &str| {
        let code = format!("[[rule]]\nname = \"r\"\nwhy = \"w\"\n{rule}");
        let engine = engine(vec![toml_user("r", &code)]);
        match &engine.rule("r").unwrap().state {
            State::Broken(e) => e.clone(),
            other => panic!("{rule}: {other:?}"),
        }
    };
    assert!(broken("message = \"m\"\n").contains("give it a `match`"));
    assert!(broken("match = { command = \"ls\" }\n").contains("`message`"));
    assert!(broken("message = \"m\"\nscript = \"r.luau\"\n").contains("leave out `message`"));
    assert!(
        broken("on = \"post-tool-use\"\nmessage = \"m\"\nmatch = { command = \"ls\" }\n")
            .contains("needs a `script`")
    );
    assert!(
        broken("scope = \"repo\"\nmessage = \"m\"\nmatch = { command = \"ls\" }\n")
            .contains("scope")
    );
    assert!(
        broken(
            "message = \"m\"\n[[rule.match]]\ncommand = \"ls\"\n[[rule.match]]\nflags = \"-f\"\n"
        )
        .contains("`match` 2: ")
    );
    assert!(
        broken("message = \"m\"\nmatch = { command = \"ls\" }\n[rule.examples]\nfires = [{ tool = \"Bash\" }]\n")
            .contains("needs a `command`")
    );
    assert!(broken("script = \"missing.luau\"\n").contains("reading its script missing.luau"));
}

#[test]
fn a_toml_rule_cannot_replace_a_builtin_or_another_rule() {
    let mut sources = builtin();
    sources.push(toml_user("mine", &DENY_LS.replace("no-ls", "git-stash")));
    sources.push(toml_user("a", DENY_LS));
    sources.push(toml_user("b", DENY_LS));
    let engine = engine(sources);
    let states: Vec<String> = engine
        .rules()
        .iter()
        .filter(|r| r.name == "git-stash" || r.name == "no-ls")
        .map(|r| match &r.state {
            State::Broken(e) => e.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(states[0], "Enabled");
    assert!(states[1].contains("is a built-in rule"), "{}", states[1]);
    assert_eq!(states[2], "Enabled");
    assert!(
        states[3].contains("a.toml already defines `no-ls`"),
        "{}",
        states[3]
    );
    assert!(engine.evaluate(&bash("git stash")).deny.is_some());
}

#[test]
fn a_script_runs_behind_its_matcher_and_is_not_a_rule_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(
        rules.join("big-rm.toml"),
        r#"[[rule]]
name = "big-rm"
why = "a test rule"
mode = "deny"
script = "big-rm.luau"
match = { command = "rm" }
[rule.examples]
fires = ["rm -r a b c d"]
quiet = ["rm -r a", "ls a b c d"]
"#,
    )
    .unwrap();
    std::fs::write(
        rules.join("big-rm.luau"),
        "return { check = function(call)\n  for _, c in call.commands do\n    if #c.argv > 4 then return jouet.deny('one at a time') end\n  end\n  return nil\nend }\n",
    )
    .unwrap();
    let user_rules = [RuleDir {
        dir: rules.clone(),
        origin: Origin::User,
        trusted: true,
        namespace: None,
    }];
    let (sources, errors) = discover(&user_rules);
    assert!(errors.is_empty());
    // A script beside a TOML file that does not load is still a script.
    std::fs::write(rules.join("broken.toml"), "[[rule]]\nname = 1\n").unwrap();
    std::fs::write(rules.join("broken.luau"), "return {}\n").unwrap();
    let broken = self::engine(discover(&user_rules).0);
    let formats: Vec<(&str, Format)> = broken
        .rules()
        .iter()
        .filter(|r| r.scope == Scope::User)
        .map(|r| (r.name.as_str(), r.format))
        .collect();
    assert_eq!(
        formats,
        [("big-rm", Format::Toml), ("broken", Format::Toml)]
    );
    let mut engine = engine(sources);
    let mine: Vec<&str> = engine
        .rules()
        .iter()
        .filter(|r| r.scope == Scope::User)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(mine, ["big-rm"]);
    engine.load_scripts();
    let rule = engine.rule("big-rm").unwrap();
    assert_eq!(rule.state, State::Enabled);
    assert_eq!(rule.script.as_deref(), Some("big-rm.luau"));
    assert_eq!(engine.proven(rule), Ok(3));
    // `ls a b c d` never reached the script: no VM for it.
    let fresh = self::engine(discover(&user_rules).0);
    assert!(fresh.evaluate(&bash("ls a b c d e")).deny.is_none());
    assert!(!fresh.vm_started());
    assert_eq!(
        fresh
            .evaluate(&bash("rm a b c d e"))
            .deny
            .unwrap()
            .message
            .as_deref(),
        Some("one at a time")
    );
}

#[test]
fn a_script_that_does_not_load_fails_open_and_lists_as_broken() {
    let code = "[[rule]]\nname = \"bad\"\nwhy = \"w\"\nscript = \"bad.luau\"\n";
    let sources = || {
        vec![
            toml_user("bad", code).with_script("bad.luau", "return { check = "),
            toml_user("examples", &code.replace("bad", "ex")).with_script(
                "ex.luau",
                "return { check = function() end, examples = { fires = {'x'} } }",
            ),
        ]
    };
    let engine = engine(sources());
    assert_eq!(engine.rule("bad").unwrap().state, State::Enabled);
    let outcome = engine.evaluate(&bash("ls"));
    assert!(outcome.deny.is_none());
    assert_eq!(outcome.errors.len(), 2, "{:?}", outcome.errors);
    let mut engine = self::engine(sources());
    engine.load_scripts();
    let state = |name: &str| match &engine.rule(name).unwrap().state {
        State::Broken(e) => e.clone(),
        other => panic!("{other:?}"),
    };
    assert!(
        state("bad").starts_with("bad.toml:1: bad.luau: "),
        "{}",
        state("bad")
    );
    assert!(
        state("ex").contains("belongs in the rule's TOML"),
        "{}",
        state("ex")
    );
}

#[test]
fn an_untrusted_toml_rule_is_listed_and_never_runs() {
    let source = Source::toml(
        "team",
        Origin::Repo(PathBuf::from(".jouet/rules/team.toml")),
        DENY_LS,
    )
    .untrusted();
    let engine = engine(vec![source]);
    let rule = engine.rule("no-ls").unwrap();
    assert_eq!(rule.state, State::Untrusted);
    assert_eq!(rule.why, "a test rule");
    assert!(engine.evaluate(&bash("ls")).deny.is_none());
}

#[test]
fn a_file_tool_example_carries_its_path() {
    let code = r#"[[rule]]
name = "no-lock-edits"
why = "lockfiles are generated"
mode = "deny"
message = "regenerate it instead"
[rule.match]
tool = ["Edit", "Write"]
paths = "*.lock"
[rule.examples]
fires = [{ tool = "Edit", path = "/repo/Cargo.lock" }]
quiet = [{ tool = "Edit", path = "/repo/src/main.rs" }, { tool = "Read", path = "Cargo.lock" }]
"#;
    let engine = engine(vec![toml_user("locks", code)]);
    let rule = engine.rule("no-lock-edits").unwrap();
    let examples = engine.examples(rule).unwrap();
    assert!(examples.iter().all(|e| e.ok), "{examples:?}");
    assert_eq!(examples[0].label(), "(tool Edit, path /repo/Cargo.lock)");
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": { "file_path": "/repo/mise.lock", "content": "x" },
        "cwd": "/repo",
    });
    let call = Call::from_hook(&payload, "claude");
    assert_eq!(call.path.as_deref(), Some("/repo/mise.lock"));
    assert!(engine.evaluate(&call).deny.is_some());
}

#[test]
fn a_luau_rule_file_still_loads_and_says_how_to_convert() {
    let engine = engine(vec![user(
        "old",
        "return { check = function() return jouet.deny('x') end }",
    )]);
    let rule = engine.rule("old").unwrap();
    assert_eq!((rule.format, &rule.state), (Format::Luau, &State::Enabled));
    let note = rule.deprecation().unwrap();
    assert!(note.contains("old.luau is a Luau rule file"), "{note}");
    assert!(note.contains("old.toml"), "{note}");
    assert!(engine.evaluate(&bash("ls")).deny.is_some());
    assert!(builtin().iter().all(|s| s.format == Format::Toml));
}

/// Like mise's `deprecated_at!`: once jouet reaches the version that stops
/// loading Luau rule files, this fails until their support is removed.
#[test]
fn luau_rule_files_are_removed_on_time() {
    let version = |v: &str| -> Vec<u64> { v.split('.').map(|n| n.parse().unwrap_or(0)).collect() };
    assert!(
        version(env!("CARGO_PKG_VERSION")) < version(LUAU_RULES_REMOVED_IN),
        "jouet {} has reached {LUAU_RULES_REMOVED_IN}: remove Luau rule files (Format::Luau, header.rs, examples::read)",
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn a_plain_cargo_run_is_full_only_when_it_covers_every_package() {
    let tmp = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = tmp.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("solo/Cargo.toml", "[package]\nname = \"solo\"\n");
    write(
        "ws/Cargo.toml",
        "[package]\nname = \"app\"\n[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write("ws/crates/lib/Cargo.toml", "[package]\nname = \"lib\"\n");
    write("virt/Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n");
    write("virt/a/Cargo.toml", "[package]\nname = \"a\"\n");
    write("virt/b/Cargo.toml", "[package]\nname = \"b\"\n");
    let engine = engine(builtin());
    let scope = |dir: &str, command: &str| {
        engine.set_probes(Probes::new(Some(tmp.path().join(dir)), None, false));
        let call = Call {
            event: Some(Event::PostToolUse),
            exit_code: Some(0),
            ..bash(command)
        };
        let records = engine.evaluate(&call).records;
        assert_eq!(records.len(), 1, "{command}");
        records[0].scope()
    };
    assert_eq!(scope("solo", "cargo test"), "full");
    assert_eq!(scope("virt", "cargo test"), "full");
    assert_eq!(scope("virt", "cd a && cargo test"), "partial -p a");
    assert_eq!(scope("ws", "cargo test"), "partial -p app");
    assert_eq!(scope("ws", "cargo test -p app -p lib"), "full");
    assert_eq!(scope("ws", "cargo clippy -p lib"), "partial -p lib");
    assert_eq!(
        scope("ws", "cd crates/lib && cargo test --workspace"),
        "full"
    );
    assert_eq!(scope(".", "cargo test"), "partial");
}

#[test]
fn a_fires_example_can_pin_the_scope_it_records() {
    let engine = engine(vec![user(
        "scoped",
        r#"return {
	on = "post-tool-use",
	check = function(call)
		jouet.record("test", call.commands[1], "partial", "-p a")
		return nil
	end,
	examples = {
		fires = {
			{ command = "cargo test -p a", kind = "test", scope = "partial -p a" },
			{ command = "cargo test -p a", kind = "test", scope = "full" },
		},
		quiet = { { tool = "Read" } },
	},
}"#,
    )]);
    let results: Vec<(String, String, bool)> = engine
        .examples(engine.rule("scoped").unwrap())
        .unwrap()
        .into_iter()
        .map(|e| (e.expected, e.actual, e.ok))
        .collect();
    assert_eq!(
        results[..2],
        [
            (
                "fires record test partial -p a".into(),
                "record test partial -p a".into(),
                true
            ),
            (
                "fires record test full".into(),
                "record test partial -p a".into(),
                false
            ),
        ]
    );
}

#[test]
fn background_matches_an_ampersand_or_a_backgrounded_call() {
    let code = "[[rule]]\nname = \"bg\"\nwhy = \"w\"\nmode = \"deny\"\nmessage = \"m\"\nmatch = { command = \"serve\", background = true }\nexamples = { fires = [\"serve &\"], quiet = [\"serve\"] }\n";
    let engine = engine(vec![toml_user("bg", code)]);
    let fires = |call: &Call| engine.evaluate(call).deny.is_some();
    assert!(fires(&bash("serve &")));
    assert!(fires(&bash("cd x && nohup serve > log 2>&1 &")));
    assert!(!fires(&bash("serve")));
    assert!(!fires(&bash("serve && sleep 1 &")));
    let harness = Call {
        input: Some(serde_json::json!({ "command": "serve", "run_in_background": true })),
        ..bash("serve")
    };
    assert!(fires(&harness));
}

#[test]
fn job_start_names_the_job_and_its_ready_check() {
    let engine = engine(builtin());
    let advice = |command: &str| {
        let outcome = engine.evaluate(&bash(command));
        let d = outcome.deny.or(outcome.warnings.into_iter().next());
        let d = d.unwrap_or_else(|| panic!("nothing for {command}"));
        assert_eq!(d.rule, "job-start", "{command}");
        let message = d.message.unwrap();
        let start = message.find('`').unwrap() + 1;
        let end = start + message[start..].find('`').unwrap();
        message[start..end].to_string()
    };
    assert_eq!(
        advice("npm run dev &"),
        "jouet job start dev -- npm run dev"
    );
    assert_eq!(
        advice("nohup python3 -m http.server 9000 > s.log 2>&1 &"),
        "jouet job start http-server --port 9000 -- python3 -m http.server 9000"
    );
    assert_eq!(
        advice("npx vite & sleep 2 && curl -sf localhost:5173/health"),
        "jouet job start vite --http http://localhost:5173/health -- npx vite"
    );
    assert_eq!(
        advice("uvicorn app:app &"),
        "jouet job start uvicorn --port 8000 -- uvicorn app:app"
    );
    assert_eq!(
        advice("bundle exec rails s -p 3001 &"),
        "jouet job start rails --port 3001 -- bundle exec rails s -p 3001"
    );
    assert_eq!(
        advice("pnpm run dev:web -- --port=4000 &"),
        "jouet job start dev-web --port 4000 -- pnpm run dev:web -- --port=4000"
    );
    assert_eq!(
        advice("sleep 100 & node 'my server.js' &"),
        "jouet job start my-server -- node 'my server.js'"
    );
    assert_eq!(
        advice("cargo test --workspace > t.log 2>&1 &"),
        "jouet job start cargo-test -- cargo test --workspace"
    );
    assert_eq!(
        advice("npm run build &"),
        "jouet job start build -- npm run build"
    );
    // Run in the harness's background, a pipe's reader is not the server.
    let piped = Call {
        input: Some(serde_json::json!({ "run_in_background": true })),
        ..bash("npm run dev 2>&1 | tee dev.log")
    };
    let message = engine.evaluate(&piped).warnings.remove(0).message.unwrap();
    assert!(
        message.starts_with("Run `jouet job start dev -- npm run dev`"),
        "{message}"
    );
    // Nor is a command the matchers did not find, run after it.
    let then = Call {
        input: Some(serde_json::json!({ "run_in_background": true })),
        ..bash("cd web && npm run dev; echo started; ls -la")
    };
    let message = engine.evaluate(&then).warnings.remove(0).message.unwrap();
    assert!(
        message.starts_with("Run `jouet job start dev -- npm run dev`"),
        "{message}"
    );
}

#[test]
fn a_dir_resolves_from_the_base_or_the_home_directory() {
    let base = Path::new("/work");
    assert_eq!(resolve_dir(base, "crate"), Path::new("/work/crate"));
    assert_eq!(resolve_dir(base, "/src/app"), Path::new("/src/app"));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        assert_eq!(resolve_dir(base, "~"), home);
        assert_eq!(resolve_dir(base, "~/src/app"), home.join("src/app"));
        assert_eq!(resolve_dir(base, "$HOME/src/app"), home.join("src/app"));
        assert_eq!(resolve_dir(base, "${HOME}/src"), home.join("src"));
    }
    // `~user` and `$HOMEDIR` are not the home directory.
    assert_eq!(resolve_dir(base, "~bob/x"), Path::new("/work/~bob/x"));
    assert_eq!(
        resolve_dir(base, "$HOMEDIR/x"),
        Path::new("/work/$HOMEDIR/x")
    );
}

#[test]
fn a_run_after_a_cd_records_where_it_ran() {
    let engine = engine(builtin());
    let dirs = |command: &str| {
        let call = Call {
            event: Some(Event::PostToolUse),
            exit_code: Some(0),
            ..bash(command)
        };
        let outcome = engine.evaluate(&call);
        outcome
            .records
            .into_iter()
            .map(|r| r.dir)
            .collect::<Vec<_>>()
    };
    assert_eq!(dirs("cargo test --workspace"), [None]);
    assert_eq!(
        dirs("cd a && cd b && cargo test --workspace"),
        [Some("a/b".into())]
    );
    assert_eq!(
        dirs("cd ~/src/app && cargo test --workspace"),
        [Some("~/src/app".into())]
    );
    assert_eq!(
        dirs("cd \"$HOME/src/app\" && cargo test --workspace"),
        [Some("$HOME/src/app".into())]
    );
    // Where `cd` cannot be followed, nothing is recorded.
    let none: [Option<String>; 0] = [];
    assert_eq!(dirs("cd \"$DIR\" && cargo test --workspace"), none);
}

#[test]
fn a_script_sees_which_commands_its_matchers_matched() {
    let code =
        "[[rule]]\nname = \"m\"\nwhy = \"w\"\nscript = \"m.luau\"\nmatch = { command = \"b\" }\n";
    let script = "return { check = function(call)\n  local out = {}\n  for _, c in call.commands do\n    if c.matched then table.insert(out, c.text) end\n  end\n  return jouet.deny(table.concat(out, \",\"))\nend }";
    let engine = engine(vec![toml_user("m", code).with_script("m.luau", script)]);
    let matched = |command: &str| {
        let outcome = engine.evaluate(&bash(command));
        let d = outcome.deny.or(outcome.warnings.into_iter().next());
        d.and_then(|d| d.message)
    };
    assert_eq!(matched("a; b 1; c && b 2").as_deref(), Some("b 1,b 2"));
    assert_eq!(matched("echo $(b) | c").as_deref(), Some("b"));
    assert_eq!(matched("a; c"), None);
}

#[test]
fn a_cut_jouet_test_is_told_to_drop_the_cut() {
    let engine = engine(builtin());
    let warning = |command: &str| {
        let outcome = engine.evaluate(&bash(command));
        let w = outcome
            .warnings
            .into_iter()
            .find(|w| w.rule == "piped-test-output");
        w.and_then(|w| w.message).unwrap_or_default()
    };
    let piped = warning("jouet test --cmd -- mise run test 2>&1 | tail -10");
    assert!(
        piped.starts_with("Drop the tail: `jouet test` already prints"),
        "{piped}"
    );
    assert!(piped.contains("does not count as passing"), "{piped}");
    // Read back from a file, the run's exit status is its own.
    let read_back = warning("jouet test > t.log 2>&1; tail -5 t.log");
    assert!(read_back.starts_with("Drop the tail"), "{read_back}");
    assert!(!read_back.contains("exit"), "{read_back}");
    // Other runners are still pointed at `jouet test`.
    assert!(warning("cargo test 2>&1 | tail -3").starts_with("Run `jouet test`"));
}

/// A user rule that denies `serve`, only where `marker.toml` is.
fn requiring() -> Source {
    toml_user(
        "needs-file",
        r#"[[rule]]
name = "needs-file"
why = "a test rule"
mode = "deny"
message = "use the tool"
requires = { files = ["marker.toml", ".config/marker.toml"] }
[rule.match]
command = "serve"
[rule.examples]
fires = ["serve"]
quiet = ["ls"]
"#,
    )
}

#[test]
fn a_rule_that_requires_a_file_runs_only_where_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let sub = project.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let engine = engine(vec![requiring()]);
    engine.set_probes(Probes::new(Some(sub.clone()), None, false));
    assert!(engine.evaluate(&bash("serve")).deny.is_none());
    assert!(!engine.in_use_here(engine.rule("needs-file").unwrap()));
    std::fs::create_dir_all(project.join(".config")).unwrap();
    std::fs::write(project.join(".config/marker.toml"), "").unwrap();
    assert!(engine.evaluate(&bash("serve")).deny.is_some());
    assert!(engine.in_use_here(engine.rule("needs-file").unwrap()));
    // The call's own directory wins over the probes'.
    let elsewhere = Call {
        cwd: Some(tmp.path().display().to_string()),
        ..bash("serve")
    };
    assert!(engine.evaluate(&elsewhere).deny.is_none());
    // Its examples still run as if the file were there.
    let rule = engine.rule("needs-file").unwrap();
    assert!(engine.examples(rule).unwrap().iter().all(|e| e.ok));
}

#[test]
fn primer_false_keeps_a_rule_out_of_the_primer_but_it_still_denies() {
    let code = "[[rule]]\nname = \"on-content\"\nwhy = \"w\"\nmode = \"deny\"\nmessage = \"m\"\nprimer = false\nmatch = { command = \"serve\" }\n[rule.examples]\nfires = [\"serve\"]\nquiet = [\"ls\"]\n";
    let user = engine(vec![toml_user("on-content", code)]);
    assert!(!user.rule("on-content").unwrap().primer);
    assert!(user.evaluate(&bash("serve")).deny.is_some());
    // Omitted, a rule is in the primer; the built-in one about content is not.
    let builtin = engine(builtin());
    assert!(builtin.rule("git-stash").unwrap().primer);
    assert!(!builtin.rule("committed-secrets").unwrap().primer);
}

#[test]
fn requires_names_files_within_a_directory() {
    for (requires, want) in [
        ("{}", "names the `files`"),
        (r#"{ files = ["../x.toml"] }"#, "paths within a directory"),
        (r#"{ files = ["/etc/x.toml"] }"#, "paths within a directory"),
    ] {
        let code = format!(
            "[[rule]]\nname = \"r\"\nwhy = \"w\"\nmessage = \"m\"\nrequires = {requires}\nmatch = {{ command = \"x\" }}\n"
        );
        let engine = engine(vec![toml_user("r", &code)]);
        match &engine.rule("r").unwrap().state {
            State::Broken(e) => assert!(e.contains(want), "{e}"),
            other => panic!("{requires}: {other:?}"),
        }
    }
}

#[test]
fn a_daemon_the_project_declares_goes_to_pitchfork_and_anything_else_to_jouet_job() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("pitchfork.toml"),
        "[daemons.web]\nrun = \"npm run dev\"\nport = 5173\n\n[daemons.api]\nrun = \"cd api && exec uvicorn app:app --port 8000\"\n",
    )
    .unwrap();
    let engine = engine(builtin());
    engine.set_probes(Probes::new(Some(tmp.path().to_path_buf()), None, false));
    let denied = |command: &str| {
        let d = engine.evaluate(&bash(command)).deny;
        d.map(|d| (d.rule, d.message.unwrap_or_default()))
    };
    let (rule, message) = denied("npm run dev -- --host &").unwrap();
    assert_eq!(rule, "pitchfork-start");
    assert!(message.starts_with("`npm run dev -- --host` is the `web` daemon in pitchfork.toml: run `pitchfork start web` instead."), "{message}");
    let (rule, message) = denied("vite --port 5173").unwrap();
    assert_eq!(rule, "pitchfork-start");
    assert!(
        message.contains("listens on port 5173, the `web` daemon's"),
        "{message}"
    );
    assert_eq!(
        denied("uvicorn app:app --port 8000").unwrap().0,
        "pitchfork-start"
    );
    // Not a declared daemon: a job.
    assert_eq!(
        denied("python3 -m http.server 9000 &").unwrap().0,
        "job-start"
    );
    assert_eq!(denied("npm run build"), None);

    let (rule, message) = denied("kill $(lsof -ti :8000)").unwrap();
    assert_eq!(rule, "pitchfork-stop");
    assert!(
        message.starts_with("Port 8000 is the `api` daemon's: run `pitchfork stop api` instead"),
        "{message}"
    );
    assert_eq!(
        denied("pkill -f 'npm run dev'").unwrap().0,
        "pitchfork-stop"
    );
    // Not a declared daemon's: job-stop's warning, no denial.
    let outcome = engine.evaluate(&bash("pkill -f vite"));
    assert!(outcome.deny.is_none());
    assert_eq!(outcome.warnings[0].rule, "job-stop");
}
