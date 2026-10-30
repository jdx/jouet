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
    let rule = "--- why: test\nreturn { check = function() end }";
    std::fs::write(user_dir.join("mine.luau"), rule).unwrap();
    std::fs::write(repo_dir.join("team.luau"), rule).unwrap();
    let mut trust = TrustStore::load(Some(dir.path().join("trust.json")));

    let (sources, errors) = discover(Some(&user_dir), Some(&repo_dir), &trust);
    assert!(errors.is_empty());
    let mine = sources.iter().find(|s| s.name == "mine").unwrap();
    assert!(matches!(mine.origin, Origin::User(_)) && mine.trusted);
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
    )
    .unwrap();
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
            harness: "codex".into(),
            exit_code: None,
            expect: Expect::Fires {
                action: Some(Action::Deny),
                kind: None,
            },
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
