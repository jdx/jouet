use super::*;
use crate::routine::{Origin, Source};
use crate::testing::FakeGithub;
use crate::world::{Github, Host, Recorded, StepOutput};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// jouet's commands, answered with exit 0 and an empty document; the
/// command lines are kept.
#[derive(Default, Clone)]
struct Commands(Arc<Mutex<Vec<String>>>);

impl Host for Commands {
    fn jouet(&mut self, argv: &[String]) -> Result<StepOutput, String> {
        self.0.lock().unwrap().push(argv.join(" "));
        Ok(StepOutput {
            exit_code: 0,
            json: Some(json!({ "result": "ok" })),
        })
    }
}

fn routine(name: &str, header: &str, body: &str) -> Routine {
    Routine::load(Source {
        name: name.into(),
        origin: Origin::User(PathBuf::from(format!("/r/{name}.luau"))),
        code: format!("--- about: test\n{header}\n{body}"),
        trusted: true,
    })
}

fn world(github: Github, recorded: Option<Recorded>, host: Commands) -> World {
    World {
        root: std::env::current_dir().unwrap(),
        host: Box::new(host),
        github,
        ledger: None,
        repo_key: None,
        log_dir: std::env::temp_dir(),
        recorded,
    }
}

fn offline() -> Github {
    Github::Fake {
        url: "http://127.0.0.1:9".into(),
        token: "t".into(),
    }
}

/// Runs a routine with these words, offline, with every primitive's
/// recorded answers empty.
fn run_words(r: &Routine, words: &[&str], options: Options) -> Outcome {
    let catalog = Catalog::new(Vec::new());
    let words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
    let args = r.parse_args(&words).unwrap();
    run(
        &catalog,
        r,
        args,
        world(offline(), Some(Recorded::default()), Commands::default()),
        options,
    )
}

#[test]
fn print_is_the_text_and_the_returned_table_the_json() {
    let r = routine(
        "hello",
        r#"--- usage: arg "<who>""#,
        r#"return { run = function(args)
            print("hello", args.who, 3)
            return joue.object({ who = args.who, n = 3, list = joue.array({}) }, { "who" }), joue.exit.act
        end }"#,
    );
    let out = run_words(&r, &["world"], Options::default());
    assert_eq!(out.text, "hello\tworld\t3\n");
    assert_eq!(out.exit_code, 10);
    assert_eq!(
        serde_json::to_string(&out.value.unwrap()).unwrap(),
        r#"{"who":"world","list":[],"n":3}"#
    );
}

#[test]
fn joue_fail_sets_the_exit_code_and_hint() {
    let r = routine(
        "f",
        "",
        r#"return { run = function() joue.fail(joue.exit.usage, "bad", "do this") end }"#,
    );
    let out = run_words(&r, &[], Options::default());
    assert_eq!(out.exit_code, 2);
    let fail = out.error.unwrap();
    assert_eq!(
        (fail.exit, fail.message.as_str(), fail.hint.as_deref()),
        (Exit::Usage, "bad", Some("do this"))
    );
}

#[test]
fn a_script_error_fails_with_where_it_happened() {
    let r = routine(
        "oops",
        "",
        "return { run = function() local t = nil; return t.x end }",
    );
    let out = run_words(&r, &[], Options::default());
    assert_eq!(out.exit_code, 1);
    assert!(out.error.unwrap().message.contains("oops:"));
}

#[test]
fn a_primitive_outside_the_capabilities_is_refused() {
    let r = routine(
        "sneaky",
        "",
        "return { run = function() return github.repo() end }",
    );
    let out = run_words(&r, &[], Options::default());
    assert_eq!(out.exit_code, 4);
    let fail = out.error.unwrap();
    assert_eq!(
        fail.message,
        "github.repo needs the `github.read` capability, which sneaky does not declare"
    );
    assert!(fail.hint.unwrap().contains("--- capabilities: github.read"));
}

const POSTER: &str = r#"return {
    run = function()
        local land = jouet.pr_land({ pr = "7", repo = "jdx/demo" })
        print(if joue.acting then "acting" else "previewing")
        return { land = land.json }
    end,
    examples = { { name = "x" } },
}"#;

#[test]
fn a_write_without_yes_is_previewed_and_exits_4() {
    let r = routine("poster", "--- capabilities: github.write", POSTER);
    let commands = Commands::default();
    let catalog = Catalog::new(Vec::new());
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(offline(), None, commands.clone()),
        Options::default(),
    );
    assert_eq!(out.exit_code, 4);
    assert!(out.needs_yes);
    assert_eq!(out.planned, ["jouet pr land 7 --repo=jdx/demo"]);
    assert_eq!(out.text, "previewing\n");
    assert_eq!(
        *commands.0.lock().unwrap(),
        ["pr land 7 --repo=jdx/demo --dry-run"]
    );
}

#[test]
fn dry_run_previews_without_failing_and_yes_acts() {
    let r = routine("poster", "--- capabilities: github.write", POSTER);
    let catalog = Catalog::new(Vec::new());
    let dry = Options {
        dry_run: true,
        ..Options::default()
    };
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(offline(), None, Commands::default()),
        dry,
    );
    assert_eq!((out.exit_code, out.needs_yes), (0, false));
    assert_eq!(out.planned.len(), 1);
    let commands = Commands::default();
    let yes = Options {
        yes: true,
        ..Options::default()
    };
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(offline(), None, commands.clone()),
        yes,
    );
    assert_eq!(out.exit_code, 0);
    assert!(out.planned.is_empty());
    assert_eq!(out.text, "acting\n");
    assert_eq!(
        *commands.0.lock().unwrap(),
        ["pr land 7 --repo=jdx/demo --yes"]
    );
}

#[test]
fn a_routine_that_writes_needs_examples() {
    let r = routine(
        "bare",
        "--- capabilities: github.write",
        "return { run = function() return 1 end }",
    );
    let catalog = Catalog::new(Vec::new());
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(offline(), None, Commands::default()),
        Options::default(),
    );
    assert_eq!(out.exit_code, 4);
    assert!(out.error.unwrap().message.contains("needs examples"));
}

#[test]
fn a_runaway_loop_is_stopped_by_the_budget() {
    let r = routine(
        "spin",
        "",
        "return { run = function() while true do end end }",
    );
    let catalog = Catalog::new(Vec::new());
    let budget = Budget {
        time: Duration::from_millis(100),
        ..Budget::default()
    };
    let out = run_with(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(offline(), None, Commands::default()),
        Options::default(),
        budget,
    );
    assert_eq!(out.exit_code, 1);
    assert!(out.error.unwrap().message.contains("budget"));
}

#[test]
fn a_called_routine_needs_nothing_the_caller_lacks() {
    let inner = routine(
        "inner",
        "--- capabilities: git.read\n--- usage: arg \"<n>\"",
        "return { run = function(args) print('dropped'); return { n = args.n }, 20 end }",
    );
    let narrow = routine(
        "narrow",
        "",
        "return { run = function() return joue.call('inner', { '1' }) end }",
    );
    let wide = routine(
        "wide",
        "--- capabilities: git.read",
        "return { run = function() local v, c = joue.call('inner', { '5' }); return { got = v.n, code = c } end }",
    );
    let catalog = Catalog::new(vec![inner_source(&inner)]);
    let out = run(
        &catalog,
        &narrow,
        narrow.parse_args(&[]).unwrap(),
        world(offline(), None, Commands::default()),
        Options::default(),
    );
    assert_eq!(out.exit_code, 4);
    assert!(
        out.error
            .unwrap()
            .message
            .contains("narrow calls inner, which needs git.read")
    );
    let out = run(
        &catalog,
        &wide,
        wide.parse_args(&[]).unwrap(),
        world(offline(), None, Commands::default()),
        Options::default(),
    );
    assert_eq!(out.value.unwrap(), json!({ "got": "5", "code": 20 }));
    assert_eq!(out.text, "");
}

fn inner_source(r: &Routine) -> Source {
    Source {
        name: r.name.clone(),
        origin: r.origin.clone(),
        code: r.code.clone(),
        trusted: true,
    }
}

#[test]
fn require_and_the_outside_world_are_not_there() {
    let r = routine(
        "escape",
        "",
        "return { run = function() return { require = require == nil, io = io == nil, os_exec = os.execute == nil } end }",
    );
    let out = run_words(&r, &[], Options::default());
    assert_eq!(
        out.value.unwrap(),
        json!({ "require": true, "io": true, "os_exec": true })
    );
}

#[test]
fn github_reads_and_writes_go_through_the_client() {
    let server = FakeGithub::start(vec![
        json!({ "method": "POST", "path": "/graphql", "match": "viewer", "body": { "data": { "viewer": { "login": "octocat" } } } }),
        json!({ "method": "GET", "path": "/repos/jdx/demo/issues", "body": [{ "n": 1 }, { "n": 2 }],
                "headers": { "link": "</repos/jdx/demo/issues/page2>; rel=\"next\"" } }),
        json!({ "method": "GET", "path": "/repos/jdx/demo/issues/page2", "body": [{ "n": 3 }] }),
        json!({ "method": "GET", "path": "/repos/jdx/demo", "status": 304, "body": "", "headers": { "etag": "\"abc\"" } }),
        json!({ "method": "POST", "path": "/repos/jdx/demo/issues/1/comments", "status": 201, "body": { "id": 9 } }),
    ])
    .unwrap();
    let github = Github::Fake {
        url: server.url.clone(),
        token: "t".into(),
    };
    let r = routine(
        "gh",
        "--- capabilities: github.read, github.write",
        r#"return {
            run = function()
                local who = github.graphql("query { viewer { login } }").viewer.login
                local issues = github.paginate("/repos/jdx/demo/issues")
                local cached = github.get("/repos/jdx/demo", { etag = '"abc"' })
                local posted = github.request("POST", "/repos/jdx/demo/issues/1/comments", { body = "hi" })
                return { who = who, n = #issues, last = issues[3].n, status = cached.status, posted = posted.body.id }
            end,
            examples = { { name = "x" } },
        }"#,
    );
    let catalog = Catalog::new(Vec::new());
    let yes = Options {
        yes: true,
        ..Options::default()
    };
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(github, None, Commands::default()),
        yes,
    );
    assert_eq!(out.error, None);
    assert_eq!(
        out.value.unwrap(),
        json!({ "who": "octocat", "n": 3, "last": 3, "status": 304, "posted": 9 })
    );
    let seen = server.requests.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|r| r.starts_with("POST /repos/jdx/demo/issues/1/comments {\"body\":\"hi\"}")),
        "{seen:?}"
    );
}

#[test]
fn github_failures_are_remote_errors() {
    let r = routine(
        "gh",
        "--- capabilities: github.read",
        r#"return { run = function() return github.graphql("query { viewer { login } }") end }"#,
    );
    let server = FakeGithub::start(vec![json!({ "method": "POST", "path": "/graphql", "status": 422, "body": { "message": "bad" } })]).unwrap();
    let github = Github::Fake {
        url: server.url.clone(),
        token: "t".into(),
    };
    let catalog = Catalog::new(Vec::new());
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(github, None, Commands::default()),
        Options::default(),
    );
    assert_eq!(out.exit_code, 5);
    assert_eq!(out.error.unwrap().message, "GitHub returned 422: bad");
}

#[test]
fn git_reads_the_work_tree() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(&["config", k, v]);
    }
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-q", "-m", "first"]);
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
    let r = routine(
        "g",
        "--- capabilities: git.read",
        r#"return { run = function()
            local state = git.state()
            local log = git.log({ max = 5 })
            local stat = git.diff({ stat = true })
            return { branch = state.branch, dirty = state.dirty, subject = log[1].subject,
                     added = stat[1].added, head = git.rev("HEAD") == log[1].sha, missing = git.rev("nope") }
        end }"#,
    );
    let catalog = Catalog::new(Vec::new());
    let mut w = world(offline(), None, Commands::default());
    w.root = dir.path().to_path_buf();
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        w,
        Options::default(),
    );
    assert_eq!(out.error, None);
    assert_eq!(
        out.value.unwrap(),
        json!({ "branch": "main", "dirty": true, "subject": "first", "added": 1, "head": true })
    );
}

#[test]
fn the_ledger_is_read_newest_first_for_this_work_tree() {
    let dir = tempfile::tempdir().unwrap();
    let lines = [
        json!({ "ts": 1, "event": "post-tool-use", "repo": "here", "exit_code": 1, "shape": "cargo test",
                "decisions": [{ "rule": "checks", "action": "record:test" }] }),
        json!({ "ts": 2, "event": "post-tool-use", "repo": "elsewhere", "exit_code": 0, "shape": "cargo test",
                "decisions": [{ "rule": "checks", "action": "record:test" }] }),
        json!({ "ts": 3, "event": "pre-tool-use", "repo": "here", "shape": "ls" }),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.path().join("ledger.jsonl"), text).unwrap();
    let r = routine(
        "l",
        "--- capabilities: ledger.read",
        r#"return { run = function()
            local all = ledger.entries()
            local run = ledger.last_run("test")
            return { n = #all, first = all[1].ts, ok = run.ok, ts = run.ts }
        end }"#,
    );
    let catalog = Catalog::new(Vec::new());
    let mut w = world(offline(), None, Commands::default());
    w.ledger = Some(dir.path().to_path_buf());
    w.repo_key = Some("here".into());
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        w,
        Options::default(),
    );
    assert_eq!(
        out.value.unwrap(),
        json!({ "n": 2, "first": 3, "ok": false, "ts": 1 })
    );
}

#[test]
fn recorded_answers_stand_in_for_mise_and_git() {
    let recorded = Recorded::from_fixture(&json!({
        "mise": { "tasks": [{ "name": "build" }], "run": { "lint": { "ok": true, "exit_code": 0 } } },
        "git": { "state": { "branch": "feat" } },
    }));
    let r = routine(
        "m",
        "--- capabilities: mise.read, mise.run, git.read",
        r#"return { run = function()
            local ok, err = pcall(mise.run, "nope")
            return { task = mise.tasks()[1].name, lint = mise.run("lint").ok, branch = git.state().branch, missing = tostring(err) }
        end }"#,
    );
    let catalog = Catalog::new(Vec::new());
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[]).unwrap(),
        world(offline(), Some(recorded), Commands::default()),
        Options::default(),
    );
    let value = out.value.unwrap();
    assert_eq!(
        (
            value["task"].as_str(),
            value["lint"].as_bool(),
            value["branch"].as_str()
        ),
        (Some("build"), Some(true), Some("feat"))
    );
    assert!(
        value["missing"]
            .as_str()
            .unwrap()
            .contains("no answer for mise.run.nope")
    );
}

#[test]
fn edits_are_planned_with_fs_read_and_previewed_without_yes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    std::fs::write(&path, "hello\n").unwrap();
    let spec = format!(
        "=== {}\n<<<<<<< SEARCH\nhello\n=======\nbye\n>>>>>>> REPLACE\n",
        path.display()
    );
    let r = routine(
        "e",
        "--- capabilities: fs.read, fs.write\n--- usage: arg \"<spec>\"",
        r#"return {
            run = function(args)
                local plan = edit.plan(args.spec)
                local applied = edit.apply(args.spec)
                return { planned_ok = plan.ok, dry = applied.dry_run }
            end,
            examples = { { name = "x" } },
        }"#,
    );
    let catalog = Catalog::new(Vec::new());
    let out = run(
        &catalog,
        &r,
        r.parse_args(std::slice::from_ref(&spec)).unwrap(),
        world(offline(), None, Commands::default()),
        Options::default(),
    );
    assert_eq!(out.error, None);
    assert_eq!(
        out.value.unwrap(),
        json!({ "planned_ok": true, "dry": true })
    );
    assert_eq!(out.exit_code, 4);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    let yes = Options {
        yes: true,
        ..Options::default()
    };
    let out = run(
        &catalog,
        &r,
        r.parse_args(&[spec]).unwrap(),
        world(offline(), None, Commands::default()),
        yes,
    );
    assert_eq!(out.exit_code, 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "bye\n");
}
