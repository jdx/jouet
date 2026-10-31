use super::*;
use crate::wind::detect::tests::ran;

const NOW: u64 = 1_793_354_400;
const ROOT: &str = "/work/app";

fn run(session: &str, command: &str) -> Run {
    Run {
        session: session.into(),
        command: command.into(),
        failed: false,
        duration_ms: 1_000,
        output_bytes: 4_000,
    }
}

fn nowhere(_: &str) -> bool {
    false
}

fn project<'a>(documented: &'a dyn Fn(&str) -> bool, tools: &[&str]) -> Project<'a> {
    Project {
        root: ROOT,
        shared_config: true,
        tools: tools.iter().map(|t| (*t).to_string()).collect(),
        documented,
    }
}

fn existing(name: &str, run: &str) -> Existing {
    Existing {
        name: name.into(),
        run: vec![run.into()],
    }
}

/// The same command run in sessions `a`, `a` and `b`.
fn three(command: &str) -> Vec<Run> {
    ["a", "a", "b"].iter().map(|s| run(s, command)).collect()
}

fn drafts_of(runs: &[Run], existing: &[Existing]) -> Vec<Draft> {
    drafts(
        runs,
        existing,
        &project(&nowhere, &["cargo"]),
        &BTreeMap::new(),
        &BTreeSet::new(),
    )
}

#[test]
fn commands_lose_what_only_trims_output_and_a_cd_to_the_root() {
    assert_eq!(
        normalize("cargo build && cargo test -p x 2>&1 | tail -20", Some(ROOT)),
        "cargo build && cargo test -p x"
    );
    assert_eq!(
        normalize(
            "cd /work/app && cargo fmt; cargo clippy | head -n 5",
            Some(ROOT)
        ),
        "cargo fmt; cargo clippy"
    );
    // Paths in the project become relative; a cd elsewhere stays.
    assert_eq!(
        normalize(
            "/work/app/target/debug/app --check /work/app/x.toml",
            Some(ROOT)
        ),
        "target/debug/app --check x.toml"
    );
    assert_eq!(normalize("cd /tmp && make", Some(ROOT)), "cd /tmp && make");
    // Quoted operators are words; a pipe into something else stays.
    assert_eq!(
        normalize("echo 'a && b' | grep a", Some(ROOT)),
        "echo 'a && b' | grep a"
    );
    // A ledger shape has no values, so any leading cd goes.
    assert_eq!(
        shape_key("cd <arg> && cargo test 2>&1 | tail -<n>"),
        "cargo test"
    );
}

#[test]
fn a_task_replaces_sequences_and_long_commands_that_do_work() {
    assert!(worth_a_task("cargo build && cargo test -p x"));
    assert!(worth_a_task("cargo fmt --check || cargo fmt"));
    assert!(worth_a_task(&format!(
        "cargo test {}",
        "--features x ".repeat(10)
    )));
    assert!(!worth_a_task("cargo test -p x"));
    assert!(!worth_a_task("git status && git diff --stat"));
    assert!(!worth_a_task("cd x && ls"));
    assert!(!worth_a_task("cargo build & cargo test"));
    assert!(!worth_a_task("cat <<EOF\nx\nEOF\ncargo test"));
    assert!(!worth_a_task(
        "GH_TOKEN=<redacted> cargo build && cargo test"
    ));
}

#[test]
fn a_command_repeated_across_sessions_is_drafted_with_its_evidence() {
    let mut runs = three("cd /work/app && cargo build && cargo test -p app 2>&1 | tail -30");
    runs.push(run("b", "cargo build"));
    runs[0].failed = true;
    let drafts = drafts_of(&runs, &[]);
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    let d = &drafts[0];
    assert_eq!(d.name, "build-test");
    assert_eq!(d.run, "cargo build && cargo test -p app");
    assert_eq!(
        d.description,
        "cargo build, then cargo test (added by jouet: agents ran it 3 times in 2 sessions)"
    );
    assert_eq!(
        d.evidence,
        Evidence {
            runs: 3,
            sessions: 2,
            failed: 1,
            duration_ms: 3_000,
            output_tokens: 3_000,
            ledger_runs: 0,
        }
    );
    assert_eq!(d.shape, "cargo build && cargo test -p <arg>");
    assert_eq!(d.suggest.file, File::Shared);
}

#[test]
fn too_few_runs_or_sessions_draft_nothing() {
    let once_each = vec![run("a", "make && make test"), run("b", "make && make test")];
    assert!(drafts_of(&once_each, &[]).is_empty());
    let one_session: Vec<Run> = (0..5).map(|_| run("a", "make && make test")).collect();
    assert!(drafts_of(&one_session, &[]).is_empty());
}

#[test]
fn a_command_an_existing_task_runs_is_not_drafted() {
    let runs = three("cargo fmt && cargo clippy");
    assert!(drafts_of(&runs, &[existing("lint", "cargo  fmt && cargo clippy")]).is_empty());
    let split = Existing {
        name: "lint".into(),
        run: vec!["cargo fmt".into(), "cargo clippy".into()],
    };
    assert!(drafts_of(&runs, &[split]).is_empty());
}

#[test]
fn a_sequence_that_starts_with_a_task_depends_on_it() {
    let d = &drafts_of(&three("mise run build && ./target/debug/app check"), &[])[0];
    assert_eq!(d.depends, ["build"]);
    assert_eq!(d.run, "./target/debug/app check");
    assert_eq!(d.name, "app");
    let d = &drafts_of(
        &three("cargo build --release && ./target/release/app bench"),
        &[existing("release", "cargo build --release")],
    )[0];
    assert_eq!(d.depends, ["release"]);
    assert_eq!(d.run, "./target/release/app bench");
    // Two tasks in a row would run at once as dependencies, so only the
    // first is one.
    let d = &drafts_of(&three("mise run a && mise run b && make"), &[])[0];
    assert_eq!(d.depends, ["a"]);
    assert_eq!(d.run, "mise run b && make");
}

#[test]
fn names_are_free_and_sources_are_files_in_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    std::fs::write(dir.path().join("rules.toml"), "").unwrap();
    let root = dir.path().to_string_lossy().into_owned();
    let p = Project {
        root: &root,
        shared_config: true,
        tools: BTreeSet::new(),
        documented: &nowhere,
    };
    let runs = three("shellcheck scripts/*.sh && taplo check rules.toml missing.toml --x=y");
    let taken: BTreeSet<String> = ["shellcheck-taplo".to_string()].into();
    let d = &drafts(&runs, &[], &p, &BTreeMap::new(), &taken)[0];
    assert_eq!(d.name, "shellcheck-taplo-2");
    assert_eq!(d.sources, ["scripts/*.sh", "rules.toml"]);
}

#[test]
fn the_ledger_adds_runs_of_the_same_shape() {
    let runs = three("cargo build && cargo test -p app");
    let ledger: BTreeMap<String, u64> =
        [("cargo build && cargo test -p <arg>".to_string(), 12)].into();
    let d = &drafts(
        &runs,
        &[],
        &project(&nowhere, &["cargo"]),
        &ledger,
        &BTreeSet::new(),
    )[0];
    assert_eq!(d.evidence.ledger_runs, 12);
}

#[test]
fn personal_commands_go_in_the_local_file_and_the_project_workflow_in_the_shared_one() {
    let docs = |text: &str| text == "cargo test -p app";
    let s = suggest("cargo build && cargo test -p app", &project(&docs, &[]));
    assert_eq!(s.file, File::Shared);
    assert_eq!(
        s.reasons,
        [
            "the repository's CI or docs run `cargo test -p app` too",
            "it uses only paths inside the repository"
        ]
    );
    let s = suggest(
        "cargo build && ./scripts/x.sh",
        &project(&nowhere, &["cargo"]),
    );
    assert_eq!(s.file, File::Local, "x.sh is not in /work/app: {s:?}");
    let s = suggest("cargo build && cargo test", &project(&nowhere, &["cargo"]));
    assert_eq!(s.file, File::Shared);
    assert_eq!(
        s.reasons[0],
        "it runs only tools the project already uses (cargo)"
    );
    let s = suggest("make && ~/bin/deploy --to me", &project(&docs, &["make"]));
    assert_eq!(s.file, File::Local);
    assert_eq!(
        s.reasons,
        ["it uses `~/bin/deploy`, a path on this machine that others won't have"]
    );
    let s = suggest("make && cp out \"$HOME/x\"", &project(&docs, &["make"]));
    assert!(s.reasons[0].contains("$HOME"), "{s:?}");
    let s = suggest("make && make test", &project(&nowhere, &[]));
    assert_eq!(s.file, File::Local);
    assert_eq!(
        s.reasons,
        ["only your sessions run it: nothing in the repository's CI, docs or tools does"]
    );
    let mut bare = project(&docs, &["cargo"]);
    bare.shared_config = false;
    let s = suggest("cargo build && cargo test -p app", &bare);
    assert_eq!(s.file, File::Local);
    assert!(s.reasons[0].contains("no mise.toml in git"), "{s:?}");
}

fn ledger_run(ts: u64, session: &str, repo: &str, script: &str) -> Entry {
    let mut entry = ran(ts, session, script, 0);
    entry.repo = Some(repo.into());
    entry
}

#[test]
fn the_ledger_notices_a_sequence_once_per_repository() {
    let mut t = Tasks::default();
    for (i, s) in ["a", "a", "b"].iter().enumerate() {
        t.observe(&ledger_run(
            i as u64,
            s,
            "r1",
            "cargo build && cargo test -p x 2>&1 | tail -5",
        ));
        t.observe(&ledger_run(
            i as u64,
            s,
            "r2",
            "cargo build && cargo test -p y",
        ));
        // Failures, lone commands and inspection are not tasks.
        let mut failed = ledger_run(i as u64, s, "r1", "make && make test");
        failed.exit_code = Some(2);
        t.observe(&failed);
        t.observe(&ledger_run(i as u64, s, "r1", "cargo test"));
        t.observe(&ledger_run(i as u64, s, "r1", "git status && git log"));
    }
    let ids = t.notice(NOW);
    assert_eq!(ids, ["t1", "t2"]);
    let p = t.proposal("t1").unwrap();
    assert_eq!(
        (
            p.repo.as_deref(),
            p.shape.as_str(),
            p.seen_runs,
            p.seen_sessions
        ),
        (Some("r1"), "cargo build && cargo test -p <arg>", 3, 2)
    );
    assert!(p.task.is_none());
    assert!(t.notice(NOW).is_empty(), "noticed once");
}

#[test]
fn notices_keep_to_the_daily_limit() {
    let mut t = Tasks::default();
    for repo in ["r1", "r2", "r3"] {
        for (i, s) in ["a", "a", "b"].iter().enumerate() {
            t.observe(&ledger_run(i as u64, s, repo, "make && make test"));
        }
    }
    assert_eq!(t.notice(NOW).len(), MAX_NEW_PER_DAY);
    assert!(t.notice(NOW + 60).is_empty());
    assert_eq!(t.notice(NOW + 24 * 60 * 60).len(), 1);
}

fn draft(run: &str, shape: &str) -> Draft {
    Draft {
        name: "build-test".into(),
        run: run.into(),
        description: String::new(),
        depends: Vec::new(),
        sources: Vec::new(),
        evidence: Evidence::default(),
        suggest: Suggestion {
            file: File::Local,
            reasons: vec!["only your sessions run it".into()],
        },
        shape: shape.into(),
    }
}

#[test]
fn a_draft_fills_in_what_the_ledger_noticed_and_the_rest_lapses() {
    let mut t = Tasks::default();
    for (i, s) in ["a", "a", "b"].iter().enumerate() {
        t.observe(&ledger_run(
            i as u64,
            s,
            "r1",
            "cargo build && cargo test -p x",
        ));
        t.observe(&ledger_run(i as u64, s, "r1", "make <arg> && make test"));
    }
    assert_eq!(t.notice(NOW), ["t1", "t2"]);
    let shape = "cargo build && cargo test -p <arg>";
    let d = draft("cargo build && cargo test -p x", shape);
    let other = draft("npm ci && npm test", "npm ci && npm test");
    let rec = t.record(Some("r1"), ROOT, vec![d.clone(), other], NOW);
    assert_eq!(rec.drafted, ["t1"]);
    assert_eq!(rec.lapsed, ["t2"]);
    assert_eq!(rec.new, ["t3"]);
    assert_eq!(t.proposal("t1").unwrap().task.as_ref().unwrap().run, d.run);
    assert_eq!(t.proposal("t1").unwrap().root.as_deref(), Some(ROOT));
    assert_eq!(t.proposal("t2").unwrap().status, Status::Lapsed);
    // Drafting again refreshes the evidence and keeps the name shown.
    let mut again = d.clone();
    again.name = "build-test-2".into();
    again.evidence.runs = 9;
    let rec = t.record(Some("r1"), ROOT, vec![again], NOW);
    assert_eq!(rec.updated, ["t1"]);
    let task = t.proposal("t1").unwrap().task.clone().unwrap();
    assert_eq!((task.name.as_str(), task.evidence.runs), ("build-test", 9));
    // A dismissed proposal is not proposed again.
    assert!(t.set_status("t3", Status::Dismissed));
    let rec = t.record(
        Some("r1"),
        ROOT,
        vec![draft("npm ci && npm test", "x")],
        NOW,
    );
    assert!(rec.new.is_empty(), "{rec:?}");
    // Another repository's proposals are its own.
    let rec = t.record(Some("r2"), "/w/b", vec![d], NOW);
    assert_eq!(rec.new, ["t4"]);
}

#[test]
fn a_draft_matches_the_shape_the_ledger_recorded_for_the_same_command() {
    let command = "cd /work/app && cargo build && cargo test -p app 2>&1 | tail -30";
    let entry = ledger_run(1, "a", "r1", command);
    let drafted = &drafts_of(&three(command), &[])[0];
    assert_eq!(shape_key(entry.shape.as_deref().unwrap()), drafted.shape);
}

#[test]
fn runs_of_jouets_tasks_are_counted_and_unused_ones_found() {
    let mut t = Tasks::default();
    for name in ["build-test", "lint"] {
        t.applied.push(Applied {
            name: name.into(),
            root: ROOT.into(),
            path: format!("{ROOT}/mise.local.toml"),
            file: File::Local,
            run: "x".into(),
            proposal: "t1".into(),
            at: NOW,
            uses: 0,
            last_used: None,
        });
    }
    assert_eq!(
        task_runs("mise run build-test -- --nocapture"),
        ["build-test"]
    );
    assert_eq!(
        task_runs("mise r a ::: b && mise run --force c"),
        ["a", "b", "c"]
    );
    assert!(task_runs("mise tasks ls").is_empty());
    assert!(t.record_use(ROOT, "cd x && mise run build-test", NOW + 5));
    assert!(!t.record_use("/elsewhere", "mise run lint", NOW + 5));
    let later = NOW + UNUSED_DAYS * 24 * 60 * 60;
    let unused: Vec<&str> = t.unused(later).iter().map(|a| a.name.as_str()).collect();
    assert_eq!(unused, ["lint"]);
    assert!(t.unused(later - 60).is_empty(), "not yet");
}

#[test]
fn a_session_start_line_says_what_to_ask_and_how_to_apply() {
    let mut p = Proposal {
        id: "t1".into(),
        created: NOW,
        repo: Some("r1".into()),
        root: None,
        shape: "cargo build && cargo test -p <arg>".into(),
        task: None,
        seen_runs: 4,
        seen_sessions: 2,
        status: Status::Pending,
        shown: Vec::new(),
    };
    assert_eq!(
        line(&p),
        "jouet wind noticed `cargo build && cargo test -p <arg>` run 4 times in 2 sessions in this repository; a mise task may replace it. At a natural pause, `jouet task propose` drafts one from the session transcripts (`jouet wind dismiss t1` drops it)."
    );
    let mut d = draft("cargo build && cargo test -p x", "");
    d.evidence.runs = 4;
    d.evidence.sessions = 2;
    p.task = Some(d);
    assert_eq!(
        line(&p),
        "jouet task t1 is ready: a mise task `build-test` for `cargo build && cargo test -p x`, which agents ran 4 times in 2 sessions here (`jouet task propose` shows it in full). At a natural pause, ask the user whether it goes in mise.local.toml (just theirs) or mise.toml (shared), suggesting mise.local.toml because only your sessions run it; then `jouet task apply t1 --file local|shared`, or `jouet wind dismiss t1`."
    );
    assert_eq!(clip("abcdef", 3), "abc…");
}
