use super::*;
use crate::{Budget, Call, Engine, Event, Options, Origin, Source};
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

/// A repository with one commit on `main`, pushed to a bare `origin`.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let bare = dir.path().join("origin.git");
    std::fs::create_dir(&work).unwrap();
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("a.txt"), "a\n").unwrap();
    git(&work, &["add", "a.txt"]);
    git(&work, &["commit", "-q", "-m", "one"]);
    git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    dir
}

/// A rule that denies with whatever `expr` evaluates to, so a test can read
/// a probe's answer from the decision.
fn rule(expr: &str) -> Source {
    Source {
        name: "probe".into(),
        origin: Origin::User("probe.luau".into()),
        code: format!(
            "--- why: a test rule\n--- mode: deny\nreturn {{ check = function(call)\n  return jouet.deny({expr})\nend }}\n"
        ),
        trusted: true,
    }
}

fn engine(expr: &str, budget: Budget) -> Engine {
    Engine::new(
        vec![rule(expr)],
        Options {
            has_command: Box::new(|_| true),
            disabled: Vec::new(),
            budget,
            cache_dir: None,
        },
    )
    .unwrap()
}

fn bash() -> Call {
    Call {
        event: Some(Event::PreToolUse),
        harness: "test".into(),
        tool: "Bash".into(),
        command: Some("true".into()),
        ..Call::default()
    }
}

/// What the rule said, or the error it failed with.
fn said(engine: &Engine) -> String {
    let outcome = engine.evaluate(&bash());
    match outcome.deny {
        Some(d) => d.message.unwrap_or_default(),
        None => format!("error: {}", outcome.errors.join("; ")),
    }
}

#[test]
fn git_state_reads_the_branch_its_upstream_and_the_work_tree() {
    let dir = repo();
    let work = dir.path().join("work");
    std::fs::write(work.join("b.txt"), "b\n").unwrap();
    git(&work, &["add", "b.txt"]);
    git(&work, &["commit", "-q", "-m", "two"]);
    std::fs::write(work.join("new.txt"), "new\n").unwrap();
    let e = engine(
        r#"(function()
            local s = jouet.git.state()
            return `{s.branch} {s.upstream} +{s.ahead} -{s.behind} dirty={s.dirty} untracked={s.untracked} changed={s.changed} detached={s.detached}`
        end)()"#,
        Budget::default(),
    );
    e.set_probes(Probes::new(Some(work.clone()), None, false));
    assert_eq!(
        said(&e),
        "main origin/main +1 -0 dirty=true untracked=1 changed=0 detached=false"
    );
    // `jouet.git(command)` is still the subcommand reader.
    let e = engine(
        r#"(function()
            local sub, args = jouet.git(jouet.parse("git -C x push -f")[1])
            return `{sub} {args[1]}`
        end)()"#,
        Budget::default(),
    );
    assert_eq!(said(&e), "push -f");
}

#[test]
fn status_lines_are_counted_by_kind() {
    let state = git::parse_status(
        "# branch.oid (initial)\n# branch.head (detached)\n1 .M N... 100644 100644 100644 a a f\n2 R. N... 100644 100644 100644 a a R100 g\tf\nu UU N... 1 2 3 4 a b c h\n? n\n",
    );
    assert_eq!(
        state,
        json!({ "detached": true, "dirty": true, "changed": 2, "untracked": 1, "conflicted": 1 })
    );
}

#[test]
fn repo_names_the_root_the_github_remote_and_a_linked_worktree() {
    let dir = repo();
    let work = dir.path().join("work");
    git(
        &work,
        &["remote", "add", "upstream", "git@github.com:jdx/jouet.git"],
    );
    git(&work, &["worktree", "add", "-q", "../linked", "-b", "side"]);
    let expr = r#"(function()
        local r = jouet.repo()
        return `{r.repo} {r.owner} {r.name} {r.linked_worktree} {r.root}`
    end)()"#;
    let e = engine(expr, Budget::default());
    e.set_probes(Probes::new(Some(work.clone()), None, false));
    let root = work.canonicalize().unwrap();
    assert_eq!(
        said(&e),
        format!("jdx/jouet jdx jouet false {}", root.display())
    );
    let linked = dir.path().join("linked");
    e.set_probes(Probes::new(Some(linked), None, false));
    assert!(said(&e).starts_with("jdx/jouet jdx jouet true "));

    let outside = tempfile::tempdir().unwrap();
    let e = engine(
        "(function() local r, err = jouet.repo(); return `{r} {err}` end)()",
        Budget::default(),
    );
    e.set_probes(Probes::new(Some(outside.path().into()), None, false));
    assert!(
        said(&e).starts_with("nil `git rev-parse` failed"),
        "{}",
        said(&e)
    );
}

#[test]
fn last_run_reads_the_newest_record_of_a_kind_from_the_ledger() {
    let dir = repo();
    let work = dir.path().join("work");
    let ours = git::repo_key(&work.canonicalize().unwrap().display().to_string());
    let ledger = tempfile::tempdir().unwrap();
    let lines = [
        json!({"ts": 1, "harness": "claude", "event": "post-tool-use", "shape": "cargo test", "exit_code": 0,
               "decisions": [{"rule": "checks", "action": "record:test"}]}),
        json!({"ts": 2, "harness": "claude", "event": "post-tool-use", "shape": "cargo clippy", "exit_code": 1,
               "decisions": [{"rule": "checks", "action": "record:lint"}]}),
        json!({"ts": 3, "harness": "claude", "event": "post-tool-use", "shape": "cargo test", "exit_code": 101,
               "repo": ours, "duration_ms": 900, "decisions": [{"rule": "checks", "action": "record:test"}]}),
        json!({"ts": 4, "harness": "claude", "event": "pre-tool-use", "shape": "cargo test",
               "decisions": [{"rule": "checks", "action": "warn"}]}),
        json!({"ts": 5, "harness": "claude", "event": "post-tool-use", "shape": "npm test", "exit_code": 0,
               "repo": "0000000000000000", "decisions": [{"rule": "checks", "action": "record:test"}]}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(
        ledger.path().join("ledger.jsonl"),
        format!("{text}not json\n"),
    )
    .unwrap();
    let expr = r#"(function()
        local t = jouet.ci.last_run("test")
        local any = jouet.ci.last_run()
        return `{t.kind} {t.exit_code} {t.ok} {t.ts} {t.duration_ms} {t.shape} | {any.kind} {any.ts}`
    end)()"#;
    let e = engine(expr, Budget::default());
    e.set_probes(Probes::new(
        Some(work.clone()),
        Some(ledger.path().into()),
        false,
    ));
    // The run in another repository (ts 5) is passed over; the lint run is
    // older than the newest test run.
    assert_eq!(said(&e), "test 101 false 3 900 cargo test | test 3");

    let e = engine(
        r#"(function() local r, err = jouet.ci.last_run("build"); return `{r} {err}` end)()"#,
        Budget::default(),
    );
    e.set_probes(Probes::new(
        Some(work.clone()),
        Some(ledger.path().into()),
        false,
    ));
    assert_eq!(said(&e), "nil nil");
    // No ledger at all: nothing to report, and no error.
    let none = tempfile::tempdir().unwrap();
    e.set_probes(Probes::new(
        Some(work),
        Some(none.path().join("missing")),
        false,
    ));
    assert_eq!(said(&e), "nil nil");
    let e = engine(r#"jouet.ci.last_run("deploy")"#, Budget::default());
    e.set_probes(Probes::new(None, None, false));
    assert!(said(&e).contains("kind must be"));
}

#[test]
fn env_reads_only_the_allowlist() {
    let e = engine(
        r#"(function() local ok = pcall(jouet.env, "CI"); return tostring(ok) end)()"#,
        Budget::default(),
    );
    assert_eq!(said(&e), "true");
    for name in [
        "GITHUB_TOKEN",
        "JOUET_GITHUB_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "HOME",
    ] {
        let e = engine(&format!("jouet.env({name:?})"), Budget::default());
        assert!(said(&e).contains("not on the allowlist"), "{name}");
    }
}

fn view(n: u64) -> PrView {
    PrView {
        status: json!({ "repo": "jdx/jouet", "number": n, "state": "open", "checks": { "pass": 3 } }),
        next: json!({ "repo": "jdx/jouet", "number": n, "action": "ready_to_merge", "kind": "done" }),
    }
}

#[test]
fn pr_probes_answer_from_github_once_per_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let probes = Probes::new(None, None, true).with_github(move |arg| {
        counter.fetch_add(1, Ordering::SeqCst);
        match arg {
            Some("404") => Ok(None),
            Some(n) => Ok(Some(view(n.parse().unwrap()))),
            None => Ok(Some(view(7))),
        }
    });
    let e = engine(
        r#"(function()
            local s = jouet.pr.status()
            local n = jouet.pr.next()
            local again = jouet.pr.status()
            local other = jouet.pr.status("9")
            local missing, err = jouet.pr.next("404")
            return `{s.number} {s.state} {s.checks.pass} {n.action} {again.number} {other.number} {missing} {err}`
        end)()"#,
        Budget::default(),
    );
    e.set_probes(probes);
    assert_eq!(said(&e), "7 open 3 ready_to_merge 7 9 nil nil");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    // A new hook call asks again.
    said(&e);
    assert_eq!(calls.load(Ordering::SeqCst), 6);
}

#[test]
fn pr_probes_are_off_without_the_network_and_say_so() {
    let probes = Probes::new(None, None, false).with_github(|_| panic!("no network in hooks"));
    let e = engine(
        "(function() local s, err = jouet.pr.status(); return `{s} {err}` end)()",
        Budget::default(),
    );
    e.set_probes(probes);
    assert!(said(&e).starts_with("nil network probes are off in hooks"));
}

#[test]
fn a_probe_that_runs_past_the_budget_fails_the_rule_open() {
    let probes = Probes::new(None, None, true).with_github(|_| {
        std::thread::sleep(Duration::from_millis(300));
        Ok(Some(view(1)))
    });
    let budget = Budget {
        time: Duration::from_millis(50),
        ..Budget::default()
    };
    let e = engine(
        "(function() local s, err = jouet.pr.status(); return `{s} {err}` end)()",
        budget,
    );
    e.set_probes(probes);
    let started = std::time::Instant::now();
    let outcome = e.evaluate(&bash());
    assert!(started.elapsed() < Duration::from_millis(250));
    // The probe gave up within the budget; the rule had no time left after
    // it, so it said nothing.
    assert!(outcome.deny.is_none());
    assert!(outcome.errors[0].contains("budget"), "{:?}", outcome.errors);
}

#[test]
fn without_probes_set_up_every_probe_says_why() {
    let e = engine(
        "(function() local s, err = jouet.git.state(); return `{s} {err}` end)()",
        Budget::default(),
    );
    assert_eq!(said(&e), "nil probes are not available here");
}
