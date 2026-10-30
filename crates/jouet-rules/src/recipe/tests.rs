use super::*;
use serde_json::json;
use std::cell::RefCell;
use std::collections::VecDeque;

/// What the stub host saw, shared with the test.
type Seen = Rc<RefCell<Vec<String>>>;

/// A host that answers each step from a queue of canned outputs, keyed by
/// command name, and records the command lines it was asked to run.
struct Stub {
    answers: BTreeMap<&'static str, VecDeque<StepOutput>>,
    seen: Seen,
}

impl Host for Stub {
    fn describe(&self, step: &Step) -> String {
        match step.argv() {
            Some(argv) => format!("jouet {}", argv.join(" ")),
            None => "cargo test".into(),
        }
    }

    fn run(&mut self, step: &Step) -> Result<StepOutput, String> {
        self.seen.borrow_mut().push(self.describe(step));
        self.answers
            .get_mut(step.command.name)
            .and_then(VecDeque::pop_front)
            .ok_or_else(|| format!("no answer for {}", step.command.name))
    }
}

fn out(exit_code: i32, json: serde_json::Value) -> StepOutput {
    StepOutput {
        exit_code,
        json: Some(json),
    }
}

fn stub(answers: Vec<(&'static str, StepOutput)>) -> (Box<dyn Host>, Seen) {
    let seen = Seen::default();
    let mut queues: BTreeMap<&'static str, VecDeque<StepOutput>> = BTreeMap::new();
    for (name, output) in answers {
        queues.entry(name).or_default().push_back(output);
    }
    let host = Stub {
        answers: queues,
        seen: seen.clone(),
    };
    (Box::new(host), seen)
}

fn load(extra: Vec<Source>) -> Recipes {
    let mut sources = builtin();
    sources.extend(extra);
    Recipes::new(
        sources,
        Options {
            has_command: Box::new(|_| true),
            cache_dir: None,
        },
    )
    .unwrap()
}

fn user(name: &str, body: &str) -> Source {
    Source {
        name: name.into(),
        origin: Origin::User(PathBuf::from(format!("{name}.luau"))),
        code: format!("--- why: a test recipe\nreturn {{ run = function(args)\n{body}\nend }}\n"),
        trusted: true,
    }
}

fn run(
    recipes: &Recipes,
    name: &str,
    words: &[&str],
    host: Box<dyn Host>,
    dry_run: bool,
) -> Report {
    let words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
    let recipe = recipes.get(name).unwrap();
    recipes.run(
        recipe,
        &words,
        Probes::new(None, None, false),
        host,
        dry_run,
    )
}

fn next(action: &str) -> StepOutput {
    let exit = if action == "ready_to_merge" { 0 } else { 20 };
    out(
        exit,
        json!({ "prs": [{ "repo": "jdx/jouet", "number": 23, "action": action, "kind": "wait", "reason": "r" }] }),
    )
}

#[test]
fn every_builtin_recipe_loads() {
    let recipes = load(Vec::new());
    for recipe in recipes.recipes() {
        assert_eq!(recipe.state, State::Ready, "{}", recipe.name);
        assert!(!recipe.why.is_empty(), "{}", recipe.name);
        assert!(recipe.usage.is_some(), "{}", recipe.name);
    }
}

#[test]
fn land_shows_the_plan_without_yes() {
    let recipes = load(Vec::new());
    let (host, seen) = stub(vec![
        ("pr_next", next("ready_to_merge")),
        (
            "pr_land",
            out(
                0,
                json!({ "result": "dry_run", "repo": "jdx/jouet", "pr": 23 }),
            ),
        ),
    ]);
    let report = run(&recipes, "land", &[], host, false);
    assert!(report.ok, "{report:?}");
    assert_eq!(
        *seen.borrow(),
        ["jouet --json pr next", "jouet --json pr land 23 --dry-run"]
    );
    assert!(report.message.unwrap().contains("yes=true"));
}

#[test]
fn land_waits_for_ci_then_merges_with_yes() {
    let recipes = load(Vec::new());
    let (host, seen) = stub(vec![
        ("pr_next", next("checks_pending")),
        ("wait_ci", out(0, json!({ "result": "passed" }))),
        ("pr_next", next("ready_to_merge")),
        (
            "pr_land",
            out(
                0,
                json!({ "result": "merged", "repo": "jdx/jouet", "pr": 23 }),
            ),
        ),
    ]);
    let report = run(&recipes, "land", &["23", "yes=true"], host, false);
    assert_eq!(report.exit_code, 0, "{report:?}");
    assert_eq!(report.message.as_deref(), Some("merged jdx/jouet#23"));
    assert_eq!(
        *seen.borrow(),
        [
            "jouet --json pr next 23",
            "jouet --json wait ci 23",
            "jouet --json pr next 23",
            "jouet --json pr land 23 --yes",
        ]
    );
}

#[test]
fn land_stops_with_the_exit_code_of_what_stopped_it() {
    let recipes = load(Vec::new());
    let (host, seen) = stub(vec![
        ("pr_next", next("checks_pending")),
        ("wait_ci", out(1, json!({ "result": "failed" }))),
    ]);
    let report = run(&recipes, "land", &["yes=true"], host, false);
    assert!(!report.ok);
    assert_eq!(report.exit_code, 1);
    assert_eq!(seen.borrow().len(), 2);

    let (host, _) = stub(vec![("pr_next", next("unresolved_threads"))]);
    let report = run(&recipes, "land", &[], host, false);
    assert_eq!(report.exit_code, 20);
    assert_eq!(report.message.as_deref(), Some("unresolved_threads: r"));
}

#[test]
fn a_dry_run_lists_every_step_and_runs_none() {
    let recipes = load(Vec::new());
    let (host, seen) = stub(Vec::new());
    let report = run(&recipes, "land", &["23", "yes=true"], host, true);
    assert!(seen.borrow().is_empty());
    let steps: Vec<&str> = report.steps.iter().map(|s| s.command.as_str()).collect();
    assert_eq!(
        steps,
        [
            "jouet --json pr next 23",
            "jouet --json wait ci 23",
            "jouet --json pr next 23",
            "jouet --json pr land 23 --yes",
        ]
    );
    assert!(report.dry_run && report.ok);
}

#[test]
fn a_write_needs_yes_and_nothing_runs_without_it() {
    let recipes = load(vec![user("merge", "return jouet.cmd.pr_land({ pr = 1 })")]);
    let (host, seen) = stub(Vec::new());
    let report = run(&recipes, "merge", &[], host, false);
    assert_eq!(report.exit_code, 4);
    assert!(report.error.unwrap().contains("yes = true"));
    assert!(seen.borrow().is_empty());

    // `yes` reaches a command that has no --yes of its own only as the
    // runner's permission.
    let recipes = load(vec![user(
        "reply",
        r#"return jouet.cmd.pr_reply({ id = 5, body = "done", resolve = true, yes = true })"#,
    )]);
    let (host, seen) = stub(vec![("pr_reply", out(0, json!({})))]);
    run(&recipes, "reply", &[], host, false);
    assert_eq!(
        *seen.borrow(),
        ["jouet --json pr threads reply 5 --body=done --resolve"]
    );
}

#[test]
fn arguments_are_checked_against_the_command() {
    let cases = [
        ("jouet.cmd.wait_ci({ pr = '-x' })", "cannot start with `-`"),
        (
            "jouet.cmd.pr_threads({ pr = 'reply' })",
            "reads as a subcommand",
        ),
        (
            "jouet.cmd.wait_ci({ shell = 'rm -rf /' })",
            "no argument `shell`",
        ),
        ("jouet.cmd.wait_ci({ '1', '2' })", "too many positional"),
        (
            "jouet.cmd.wait_ci({ all = 'yes' })",
            "must be true or false",
        ),
        ("jouet.cmd.test({ 'x' })", "it takes no arguments"),
        ("jouet.cmd.nope()", "attempt to call a nil value"),
    ];
    for (call, expected) in cases {
        let recipes = load(vec![user("bad", &format!("return {call}"))]);
        let (host, seen) = stub(Vec::new());
        let report = run(&recipes, "bad", &[], host, false);
        let error = report.error.unwrap_or_default();
        assert!(error.contains(expected), "{call}: {error}");
        assert!(seen.borrow().is_empty());
    }

    let recipes = load(vec![user(
        "flags",
        "return jouet.cmd.wait_ci({ '7', log_lines = 20, check = { 'unit', 'lint' }, no_logs = true, quiet = false, timeout = '10m' })",
    )]);
    let (host, seen) = stub(vec![("wait_ci", out(0, json!({})))]);
    run(&recipes, "flags", &[], host, false);
    assert_eq!(
        *seen.borrow(),
        ["jouet --json wait ci 7 --timeout=10m --check=unit --check=lint --log-lines=20 --no-logs"]
    );
}

#[test]
fn a_step_returns_the_commands_json_and_exit_code() {
    let recipes = load(vec![user(
        "status",
        r#"local r = jouet.cmd.pr_status({ prs = { "1", "2" }, repo = "jdx/jouet" })
        return { ok = r.ok, exit_code = r.exit_code, n = #r.json.prs, err = r.error, command = r.command }"#,
    )]);
    let (host, _) = stub(vec![(
        "pr_status",
        out(
            3,
            json!({ "prs": [{ "number": 1 }], "error": { "message": "gone" } }),
        ),
    )]);
    let report = run(&recipes, "status", &[], host, false);
    assert_eq!(report.exit_code, 3);
    assert_eq!(
        report.result.unwrap(),
        json!({ "ok": false, "exit_code": 3, "n": 1, "err": "gone", "command": "jouet --json pr status 1 2 --repo=jdx/jouet" })
    );
    assert_eq!(report.steps[0].exit_code, Some(3));
}

#[test]
fn sweep_lists_threads_compactly_and_posts_nothing() {
    let recipes = load(Vec::new());
    let threads = json!({
        "repo": "jdx/jouet", "pr": 23,
        "threads": [
            { "id": 11, "path": "src/a.rs", "line": 4, "author": "bot[bot]",
              "comments": [{ "body": "Use a constant here.\nMore detail" }] },
            { "id": 12, "path": "README.md", "author": "jdx", "comments": [{ "body": "typo" }] },
        ]
    });
    let (host, seen) = stub(vec![("pr_threads", out(0, threads))]);
    let report = run(&recipes, "sweep", &[], host, false);
    assert_eq!(
        report.message.as_deref(),
        Some(
            "2 unresolved threads on jdx/jouet#23\n11  src/a.rs:4  bot[bot]: Use a constant here.\n12  README.md:?  jdx: typo"
        )
    );
    assert_eq!(*seen.borrow(), ["jouet --json pr threads"]);
}

#[test]
fn ship_check_reports_tests_and_the_pr() {
    let recipes = load(Vec::new());
    let (host, seen) = stub(vec![
        ("test", out(101, json!({ "log": "/tmp/t.log" }))),
        (
            "pr_status",
            out(
                0,
                json!({ "prs": [{ "repo": "jdx/jouet", "number": 23, "state": "open",
                "checks": { "pass": 4, "fail": 1 }, "attention": [{ "code": "ci_failed", "message": "lint failed" }] }] }),
            ),
        ),
    ]);
    let report = run(&recipes, "ship-check", &[], host, false);
    assert_eq!(report.exit_code, 101);
    assert_eq!(
        report.message.as_deref(),
        Some(
            "tests failed (exit 101): cargo test\n  log: /tmp/t.log\njdx/jouet#23 open: ci 4/5, threads 0, review -\n  ci_failed: lint failed"
        )
    );
    assert_eq!(*seen.borrow(), ["cargo test", "jouet --json pr status"]);
}

#[test]
fn recipes_cannot_take_a_builtin_name_and_untrusted_ones_do_not_run() {
    let mut repo = user("deploy", "return 'ran'");
    repo.origin = Origin::Repo("deploy.luau".into());
    repo.trusted = false;
    let recipes = load(vec![user("land", "return 'mine'"), repo]);
    let land: Vec<&State> = recipes
        .recipes()
        .iter()
        .filter(|r| r.name == "land")
        .map(|r| &r.state)
        .collect();
    assert_eq!(land[0], &State::Ready);
    assert!(matches!(land[1], State::Broken(e) if e.contains("built-in recipe")));
    assert_eq!(recipes.get("land").unwrap().origin, Origin::Builtin);

    let (host, _) = stub(Vec::new());
    let report = run(&recipes, "deploy", &[], host, false);
    assert_eq!(report.exit_code, 4);
    assert!(report.error.unwrap().contains("--trust"));
}

#[test]
fn rules_cannot_run_commands() {
    let engine = crate::Engine::new(
        vec![Source {
            name: "sneaky".into(),
            origin: Origin::User("sneaky.luau".into()),
            code: "--- why: t\nreturn { check = function() jouet.cmd.pr_land({ yes = true }) end }"
                .into(),
            trusted: true,
        }],
        crate::Options {
            has_command: Box::new(|_| true),
            disabled: Vec::new(),
            budget: Budget::default(),
            cache_dir: None,
        },
    )
    .unwrap();
    let outcome = engine.evaluate(&crate::Call {
        command: Some("true".into()),
        ..crate::Call::default()
    });
    assert!(
        outcome.errors[0].contains("from recipes"),
        "{:?}",
        outcome.errors
    );
}

#[test]
fn a_recipe_runs_at_most_max_steps_and_within_its_budget() {
    let recipes = load(vec![
        user("loop", "while true do jouet.cmd.wait_pid({ pid = 1 }) end"),
        user("spin", "while true do end"),
    ]);
    let answers = (0..MAX_STEPS + 1)
        .map(|_| ("wait_pid", out(0, json!({}))))
        .collect();
    let (host, seen) = stub(answers);
    let report = run(&recipes, "loop", &[], host, false);
    assert!(report.error.unwrap().contains("at most"));
    assert_eq!(seen.borrow().len(), MAX_STEPS);

    let (host, _) = stub(Vec::new());
    let report = run(&recipes, "spin", &[], host, false);
    assert!(report.error.unwrap().contains("budget"));
}

#[test]
fn recipe_args_come_from_the_words_after_its_name() {
    let recipes = load(vec![user(
        "echo",
        "return { first = args[1], second = args[2], yes = args.yes, no = args.no, repo = args.repo }",
    )]);
    let (host, _) = stub(Vec::new());
    let report = run(
        &recipes,
        "echo",
        &["a", "yes=true", "no=false", "repo=jdx/jouet", "b=c=d", "-x"],
        host,
        false,
    );
    assert_eq!(
        report.result.unwrap(),
        json!({ "first": "a", "second": "-x", "yes": true, "no": false, "repo": "jdx/jouet" })
    );
}
