use super::*;

fn source(name: &str, code: &str) -> Source {
    Source {
        name: name.into(),
        origin: Origin::User(PathBuf::from(format!("/r/{name}.luau"))),
        code: code.into(),
        trusted: true,
    }
}

fn broken(name: &str, code: &str) -> String {
    let mut routine = Routine::load(source(name, code));
    routine.check();
    match routine.state {
        State::Broken(e) => e,
        other => panic!("expected broken, got {other:?}"),
    }
}

const TRIAGE: &str = r#"--- name: triage
--- about: List my PRs that need me
--- help: Most urgent first.
--- capabilities: github.read, git.read
--- usage: arg "[prs]..." help="PRs"
--- usage: flag "--repo <OWNER/NAME>" var=#true help="Repository"
--- usage: flag "--mine" help="Mine"
--- usage: flag "--max-count <N>" help="At most N"
return { run = function(args) end }
"#;

#[test]
fn the_header_names_what_it_is_and_reaches() {
    let r = Routine::load(source("triage", TRIAGE));
    assert_eq!(r.state, State::Ready);
    assert_eq!(r.about, "List my PRs that need me");
    assert_eq!(r.help.as_deref(), Some("Most urgent first."));
    assert_eq!(
        r.capabilities,
        [Capability::GithubRead, Capability::GitRead]
    );
    assert!(!r.writes());
    assert!(!r.spec_kdl().contains("--yes"));
}

#[test]
fn a_header_that_is_wrong_says_how() {
    assert!(broken("x", "return {}").contains("--- about:"));
    assert!(
        broken("x", "--- about: a\n--- capabilities: github\n").contains("no capability `github`")
    );
    assert!(
        broken("x", "--- about: a\n--- capabilites: github.read\n")
            .contains("unknown header key `capabilites`")
    );
    assert!(broken("x", "--- name: y\n--- about: a\n").contains("the header names `y`"));
    assert!(broken("test", "--- about: a\n").contains("joue's own commands"));
    assert!(broken("Bad_Name", "--- about: a\n").contains("lowercase words"));
    assert!(
        broken("x", "--- about: a\n--- usage: flag \"--x\" help=\n")
            .contains("usage spec does not parse")
    );
}

#[test]
fn arguments_parse_against_the_usage_spec() {
    let r = Routine::load(source("triage", TRIAGE));
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    let args = r
        .parse_args(&words(&[
            "7",
            "8",
            "--repo",
            "jdx/a",
            "--repo=jdx/b",
            "--mine",
            "--max-count",
            "3",
            "--json",
        ]))
        .unwrap();
    assert_eq!(
        serde_json::Value::Object(args.values),
        serde_json::json!({ "prs": ["7", "8"], "repo": ["jdx/a", "jdx/b"], "mine": true, "max_count": "3" })
    );
    assert!(args.json);
    assert!(!args.yes);
    match r.parse_args(&words(&["--nope"])) {
        Err(ArgError::Usage(e)) => assert!(e.contains("--nope"), "{e}"),
        other => panic!("{other:?}"),
    }
    match r.parse_args(&words(&["--help"])) {
        Err(ArgError::Help(page)) => {
            assert!(page.contains("List my PRs that need me"), "{page}");
            assert!(page.contains("--max-count"), "{page}");
        }
        other => panic!("{other:?}"),
    }
    // --yes is only for a routine that writes.
    assert!(matches!(
        r.parse_args(&words(&["--yes"])),
        Err(ArgError::Usage(_))
    ));
}

#[test]
fn a_routine_that_writes_takes_yes_and_dry_run() {
    let r = Routine::load(source(
        "post",
        "--- about: Post\n--- capabilities: github.write\nreturn { run = function() end }",
    ));
    assert!(r.writes());
    let args = r.parse_args(&["--yes".into()]).unwrap();
    assert!(args.yes && !args.dry_run);
    let args = r.parse_args(&["--dry-run".into()]).unwrap();
    assert!(args.dry_run && !args.yes);
    assert!(args.values.is_empty());
}

#[test]
fn a_users_routine_never_replaces_a_built_in_one() {
    let mut sources = builtin();
    sources.push(source("land", "--- about: mine\nreturn {}"));
    sources.push(source("mine", "--- about: mine\nreturn {}"));
    sources.push(source("mine", "--- about: again\nreturn {}"));
    let catalog = Catalog::new(sources);
    assert_eq!(catalog.get("land").unwrap().origin, Origin::Builtin);
    let names: Vec<&State> = catalog
        .routines()
        .iter()
        .filter(|r| r.name == "land")
        .map(|r| &r.state)
        .collect();
    assert!(matches!(names[1], State::Broken(e) if e.contains("built-in")));
    assert_eq!(catalog.get("mine").unwrap().about, "mine");
    assert!(
        matches!(&catalog.routines().last().unwrap().state, State::Broken(e) if e.contains("already defines"))
    );
}

#[test]
fn every_built_in_routine_is_ready() {
    for r in Catalog::new(builtin()).routines() {
        assert_eq!(r.state, State::Ready, "{}", r.name);
    }
}

#[test]
fn an_untrusted_repository_routine_is_listed_but_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.luau"), "--- about: hi\nreturn {}").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a routine").unwrap();
    let repo = |trusted| {
        [RoutineDir {
            dir: dir.path().to_path_buf(),
            origin: Origin::Repo,
            trusted,
            namespace: None,
        }]
    };
    let (sources, errors) = discover(&repo(false));
    assert!(errors.is_empty());
    let catalog = Catalog::new(sources);
    let hello = catalog.get("hello").unwrap();
    assert_eq!(hello.state, State::Untrusted);
    assert!(matches!(hello.origin, Origin::Repo(_)));
    let (sources, _) = discover(&repo(true));
    assert_eq!(
        Catalog::new(sources).get("hello").unwrap().state,
        State::Ready
    );
}

#[test]
fn a_plugins_routine_is_named_after_it_and_takes_no_built_in_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("land.luau"), "--- about: hi\nreturn {}").unwrap();
    let (sources, errors) = discover(&[RoutineDir {
        dir: dir.path().to_path_buf(),
        origin: Origin::Plugin,
        trusted: true,
        namespace: Some("acme".into()),
    }]);
    assert!(errors.is_empty());
    let catalog = Catalog::new(sources);
    assert_eq!(catalog.get("acme/land").unwrap().state, State::Ready);
    assert!(matches!(
        catalog.get("land").unwrap().origin,
        Origin::Builtin
    ));
}

#[test]
fn near_misses_are_suggested() {
    let catalog = Catalog::new(builtin());
    assert_eq!(catalog.similar("lnad"), ["land"]);
    assert_eq!(catalog.similar("pr-nxt"), ["pr-next"]);
    assert_eq!(catalog.similar("sweeep"), ["sweep"]);
    assert!(catalog.similar("zzzzzzzz").is_empty());
}
