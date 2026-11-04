//! Selection on real git repositories made from the fixture projects in
//! `test/fixtures/affected`, without running a test runner: the pytest
//! fixture uses the static import graph, and the JavaScript ones only reach
//! decisions that need no runner (the whole suite, or nothing).

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test/fixtures/affected")
        .join(name)
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A script that exits 1, standing in for a runner jouet only needs to find.
fn stub(path: &Path) {
    write(path, "#!/bin/sh\nexit 1\n");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

/// A repository on `main` with `fixture` at `at`, and a branch `work` off it
/// to change. A stub pytest (with a Python that cannot import testmon) and
/// stub vitest and jest are installed, ignored as they would be.
fn repo(fixture_name: &str, at: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    copy(&fixture(fixture_name), &root.join(at));
    write(&root.join(".gitignore"), ".venv/\nnode_modules/\n");
    let project = root.join(at);
    stub(&project.join(".venv/bin/pytest"));
    stub(&project.join(".venv/bin/python"));
    stub(&project.join("node_modules/.bin/vitest"));
    stub(&project.join("node_modules/.bin/jest"));
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["checkout", "-q", "-b", "work"]);
    dir
}

fn go_available() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn plan_in(dir: &Path) -> Result<Plan, Error> {
    plan(dir, Some("main"), None, &[])
}

#[test]
fn root_graph_refuses_to_miss_changed_nested_vitest_project() {
    let dir = repo("vitest", "web");
    let root = dir.path();
    write(
        &root.join("go.mod"),
        "module example.test/project\n\ngo 1.20\n",
    );
    write(
        &root.join("web/src/sum.js"),
        "export const sum = () => 42;\n",
    );
    let error = match plan_in(root) {
        Ok(_) => panic!("the root suite cannot cover web tests"),
        Err(error) => error,
    };
    assert_eq!(error.exit, Exit::Precondition);
    assert!(error.message.contains("web (vitest)"), "{}", error.message);
    assert!(error.hint.unwrap().contains("each relevant project"));
}

#[test]
fn nested_documentation_does_not_refuse_root_selection() {
    let dir = repo("vitest", "web");
    let root = dir.path();
    let project = Project {
        root: root.into(),
        dir: root.into(),
        rel: String::new(),
    };
    let changes = vec![Change {
        path: "web/README.md".into(),
        status: Status::Modified,
    }];
    assert!(check_nested_projects(&project, &changes).is_ok());
}

#[test]
fn root_graph_does_not_refuse_a_nested_maven_module() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("pom.xml"), "<project/>\n");
    write(&root.join("service/pom.xml"), "<project/>\n");
    let project = Project {
        root: root.into(),
        dir: root.into(),
        rel: String::new(),
    };
    let changes = vec![Change {
        path: "service/src/Main.java".into(),
        status: Status::Modified,
    }];
    assert!(detect(&root.join("service")).contains(&"graph"));
    assert!(check_nested_projects(&project, &changes).is_ok());
}

fn tests_of(suite: &Suite) -> Vec<(&str, Vec<&str>)> {
    suite
        .tests
        .iter()
        .map(|t| (t.path.as_str(), t.via.iter().map(String::as_str).collect()))
        .collect()
}

fn full_reason(suite: &Suite) -> &str {
    match &suite.decision {
        Decision::Full(why) => why,
        other => panic!("expected the whole suite, got {other:?}"),
    }
}

fn nothing_reason(suite: &Suite) -> &str {
    match &suite.decision {
        Decision::Nothing(why) => why,
        other => panic!("expected nothing, got {other:?}"),
    }
}

#[test]
fn go_selection_stops_for_inactive_packages_and_accepts_their_build_tag() {
    if !go_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("go.mod"),
        "module example.test/project\n\ngo 1.20\n",
    );
    write(
        &root.join("lib/lib.go"),
        "package lib\nfunc Value() int { return 1 }\n",
    );
    write(
        &root.join("lib/lib_test.go"),
        "package lib\nimport \"testing\"\nfunc TestValue(t *testing.T) { _ = Value() }\n",
    );
    write(
        &root.join("tagged/tagged_test.go"),
        "//go:build optional\n\npackage tagged\nimport (\"testing\"; \"example.test/project/lib\")\nfunc TestValue(t *testing.T) { _ = lib.Value() }\n",
    );
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["checkout", "-q", "-b", "work"]);
    write(
        &root.join("lib/lib.go"),
        "package lib\nfunc Value() int { return 2 }\n",
    );
    let error = match plan_in(root) {
        Ok(_) => panic!("the inactive package must stop the plan"),
        Err(error) => error,
    };
    assert_eq!(error.exit, Exit::Precondition);
    assert!(error.message.contains("./tagged:"), "{}", error.message);
    let enabled = plan(root, Some("main"), None, &["-tags=optional".into()]).unwrap();
    let command = enabled.suites[0].command.as_ref().unwrap();
    assert!(command.contains(&"./tagged".into()), "{command:?}");
    assert!(command.contains(&"-tags=optional".into()), "{command:?}");
}
#[test]
fn go_selection_ignores_fixture_sources_but_keeps_actual_tests_in_testdata() {
    if !go_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("go.mod"),
        "module example.test/project\n\ngo 1.22\n",
    );
    write(
        &root.join("lib/lib.go"),
        "package lib\nfunc Value() int { return 1 }\n",
    );
    write(
        &root.join("lib/lib_test.go"),
        "package lib\nimport \"testing\"\nfunc TestValue(t *testing.T) { _ = Value() }\n",
    );
    write(
        &root.join("testdata/probe/main.go"),
        "package main\nimport \"example.test/project/lib\"\nfunc main() { _ = lib.Value() }\n",
    );
    write(
        &root.join("testdata/real/real_test.go"),
        "package real\nimport (\"testing\"; \"example.test/project/lib\")\nfunc TestValue(t *testing.T) { _ = lib.Value() }\n",
    );
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["checkout", "-q", "-b", "work"]);
    write(
        &root.join("lib/lib.go"),
        "package lib\nfunc Value() int { return 2 }\n",
    );

    let plan = plan_in(root).unwrap();
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    assert_eq!(suite.decision, Decision::Affected);
    let paths: Vec<_> = suite.tests.iter().map(|test| test.path.as_str()).collect();
    assert!(paths.contains(&"lib/lib_test.go"), "{paths:?}");
    assert!(paths.contains(&"testdata/real/real_test.go"), "{paths:?}");
    assert!(!paths.contains(&"testdata/probe/main.go"), "{paths:?}");
}

#[test]
fn go_selection_includes_tests_in_the_changed_files_package() {
    if !go_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join(".gitignore"), "lib/local_test.go\n");
    write(
        &root.join("go.mod"),
        "module example.test/project\n\ngo 1.22\n",
    );
    write(&root.join("lib/type.go"), "package lib\ntype T struct{}\n");
    write(
        &root.join("lib/method.go"),
        "package lib\nfunc (T) Work() int { return 7 }\n",
    );
    write(
        &root.join("lib/method_test.go"),
        "package lib\nimport \"testing\"\nfunc TestDirect(t *testing.T) { if (T{}).Work() != 7 { t.Fatal(\"bad\") } }\n",
    );
    write(
        &root.join("client/client_test.go"),
        "package client\nimport (\"testing\"; \"example.test/project/lib\")\nfunc TestClient(t *testing.T) { if (lib.T{}).Work() == 0 { t.Fatal(\"bad\") } }\n",
    );
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["checkout", "-q", "-b", "work"]);
    // Go runs local ignored tests too, though the graph deliberately does not index them.
    write(
        &root.join("lib/local_test.go"),
        "package lib\nimport \"testing\"\nfunc TestLocal(t *testing.T) { _ = T{} }\n",
    );
    write(
        &root.join("lib/method.go"),
        "package lib\nfunc (T) Work() int { return 8 }\n",
    );

    let plan = plan_in(root).unwrap();
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    let paths: Vec<_> = suite.tests.iter().map(|test| test.path.as_str()).collect();
    assert!(paths.contains(&"lib/method_test.go"), "{paths:?}");
    assert!(paths.contains(&"lib/local_test.go"), "{paths:?}");
    assert!(suite.command.as_ref().unwrap().contains(&"./lib".into()));
}

#[test]
fn go_selection_finds_a_packages_only_test_even_when_git_ignores_it() {
    if !go_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("go.mod"),
        "module example.test/project\n\ngo 1.22\n",
    );
    write(
        &root.join(".gitignore"),
        "lib/local_test.go\nlib/_ignored_test.go\nlib/.hidden_test.go\n",
    );
    write(
        &root.join("lib/value.go"),
        "package lib\nfunc Value() int { return 7 }\n",
    );
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["checkout", "-q", "-b", "work"]);
    write(
        &root.join("lib/local_test.go"),
        "package lib\nimport \"testing\"\nfunc TestLocal(t *testing.T) { if Value() != 7 { t.Fatal(\"bad\") } }\n",
    );
    write(&root.join("lib/_ignored_test.go"), "package lib\n");
    write(&root.join("lib/.hidden_test.go"), "package lib\n");
    write(
        &root.join("lib/value.go"),
        "package lib\nfunc Value() int { return 8 }\n",
    );
    let plan = plan_in(root).unwrap();
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    assert_eq!(suite.decision, Decision::Affected);
    assert_eq!(suite.tests.len(), 1);
    assert_eq!(suite.tests[0].path, "lib/local_test.go");
    assert_eq!(
        suite.command.as_deref(),
        Some(["go".into(), "test".into(), "./lib".into()].as_slice())
    );
}

#[test]
fn go_selection_falls_back_when_an_unchanged_test_cannot_be_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("go.mod"),
        "module example.test/project\n\ngo 1.26\n",
    );
    write(
        &root.join("lib/lib.go"),
        "package lib\nfunc Value() int { return 1 }\n",
    );
    // Go 1.26 accepts new(expression), but the bundled tree-sitter-go grammar
    // does not yet parse it. An unchanged, valid test must not be omitted.
    write(
        &root.join("lib/lib_test.go"),
        "package lib\nimport \"testing\"\nfunc TestValue(t *testing.T) { _ = *new(Value()) }\n",
    );
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["checkout", "-q", "-b", "work"]);
    write(
        &root.join("lib/lib.go"),
        "package lib\nfunc Value() int { return 2 }\n",
    );

    let plan = plan_in(root).unwrap();
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    assert_eq!(
        suite.command.as_deref(),
        Some(["go".into(), "test".into(), "./...".into()].as_slice())
    );
    let Decision::Full(reason) = &suite.decision else {
        panic!("unparseable Go test should run the whole suite")
    };
    assert!(reason.contains("lib/lib_test.go"));
    assert!(suite.tests.is_empty());

    // The second half asks Go to load a module requiring Go 1.26.
    let version = Command::new("go").args(["env", "GOVERSION"]).output();
    let supports_module = version
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|version| {
            version
                .trim()
                .strip_prefix("go1.")
                .and_then(|v| v.split('.').next())
                .and_then(|v| v.parse::<u32>().ok())
        })
        .is_some_and(|minor| minor >= 26);
    if !supports_module {
        return;
    }
    write(
        &root.join("lib/lib_test.go"),
        "package lib\nimport \"testing\"\nfunc TestValue(t *testing.T) { _ = Value() }\n",
    );
    let plan = plan_in(root).unwrap();
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    assert_eq!(suite.note, None);
    assert_eq!(
        suite.command.as_deref(),
        Some(["go".into(), "test".into(), "./lib".into()].as_slice())
    );
}

#[test]
fn a_changed_module_selects_the_tests_that_import_it_directly_or_not() {
    let dir = repo("pytest", "");
    let root = dir.path();
    write(
        &root.join("src/calc/core/arith.py"),
        "def add(a, b):\n    return b + a\n",
    );
    let plan = plan_in(root).unwrap();
    assert_eq!(plan.base.name, "main");
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    assert_eq!(suite.adapter.name(), "pytest");
    assert!(suite.adapter.precision().contains("best-effort"));
    assert_eq!(suite.decision, Decision::Affected);
    assert_eq!(
        tests_of(suite),
        [
            (
                "tests/test_arith.py",
                vec!["src/calc/core/arith.py", "tests/test_arith.py"]
            ),
            (
                "tests/test_ops.py",
                vec![
                    "src/calc/core/arith.py",
                    "src/calc/ops.py",
                    "tests/test_ops.py"
                ]
            ),
        ]
    );
    let command = suite.command.as_ref().unwrap();
    assert!(command[0].ends_with(".venv/bin/pytest"), "{command:?}");
    assert_eq!(command[1..], ["tests/test_arith.py", "tests/test_ops.py"]);
}

#[test]
fn a_changed_test_selects_itself_and_extra_arguments_pass_through() {
    let dir = repo("pytest", "");
    let root = dir.path();
    write(
        &root.join("tests/test_new.py"),
        "def test_new():\n    pass\n",
    );
    let plan = plan(root, Some("main"), Some("pytest"), &["-x".into()]).unwrap();
    let suite = &plan.suites[0];
    assert_eq!(
        tests_of(suite),
        [("tests/test_new.py", vec!["tests/test_new.py"])]
    );
    assert_eq!(
        suite.command.as_ref().unwrap()[1..],
        ["-x", "tests/test_new.py"]
    );
}

#[test]
fn a_deleted_module_selects_the_tests_that_imported_it() {
    let dir = repo("pytest", "");
    let root = dir.path();
    git(root, &["rm", "-q", "src/calc/fmt.py"]);
    git(root, &["commit", "-q", "-m", "drop fmt"]);
    let plan = plan_in(root).unwrap();
    assert_eq!(
        tests_of(&plan.suites[0]),
        [(
            "tests/test_fmt.py",
            vec!["src/calc/fmt.py", "tests/test_fmt.py"]
        )]
    );
}

#[test]
fn a_module_no_test_imports_runs_nothing() {
    let dir = repo("pytest", "");
    let root = dir.path();
    write(&root.join("src/calc/unused.py"), "X = 1\n");
    let plan = plan_in(root).unwrap();
    assert_eq!(
        nothing_reason(&plan.suites[0]),
        "no tests selected for the changed files"
    );
    assert_eq!(plan.suites[0].command, None);
}

#[test]
fn a_changed_js_test_omitted_by_the_runner_is_not_silently_skipped() {
    let dir = repo("vitest", "");
    let root = dir.path();
    write(
        &root.join("test/new.browser.test.js"),
        "test('new', () => {})\n",
    );
    let runner = root.join("node_modules/.bin/vitest");
    let listing = |path: &str| {
        write(
            &runner,
            &format!(
                r##"#!/bin/sh
for arg in "$@"; do case "$arg" in --json=*) printf '["%s/{path}"]' "$PWD" > "${{arg#--json=}}";; esac; done
"##
            ),
        );
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    listing("test/fmt.test.js");
    let error = plan_in(root).err().unwrap();
    assert_eq!(error.exit, Exit::Precondition);
    assert!(error.message.contains("test/new.browser.test.js"));
    listing("test/new.browser.test.js");
    let plan = plan_in(root).unwrap();
    assert!(
        plan.suites[0]
            .tests
            .iter()
            .any(|test| test.path == "test/new.browser.test.js")
    );
}

#[test]
fn a_changed_js_test_selected_by_another_runner_is_not_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project {
        root: dir.path().into(),
        dir: dir.path().into(),
        rel: String::new(),
    };
    let change = Change {
        path: "src/new.test.ts".into(),
        status: Status::Modified,
    };
    let suite = |adapter: Box<dyn Adapter>, tests: Vec<Test>| Suite {
        adapter,
        project: project.clone(),
        files: vec![FileEffect {
            path: change.path.clone(),
            status: change.status,
            effect: Effect::Source,
        }],
        decision: Decision::Affected,
        tests,
        command: None,
        note: None,
    };
    let suites = vec![
        suite(Box::new(js::Js::vitest(&project)), Vec::new()),
        suite(
            Box::new(js::Js::jest(&project)),
            vec![Test {
                path: change.path.clone(),
                via: Vec::new(),
            }],
        ),
    ];
    assert!(check_changed_js_tests(&suites, &[change]).is_ok());
}

#[test]
fn docs_only_runs_nothing_and_no_change_runs_nothing() {
    let dir = repo("pytest", "");
    let root = dir.path();
    let plan = plan_in(root).unwrap();
    assert_eq!(
        nothing_reason(&plan.suites[0]),
        "nothing changed since main"
    );
    write(&root.join("README.md"), "# changed\n");
    write(&root.join("docs/guide.md"), "# new\n");
    let plan = plan_in(root).unwrap();
    assert_eq!(
        nothing_reason(&plan.suites[0]),
        "only documentation changed"
    );
    assert_eq!(plan.changes.len(), 2);
}

#[test]
fn shared_configuration_runs_the_whole_suite() {
    for (path, text, why) in [
        (
            "tests/conftest.py",
            "# changed\n",
            "a conftest changed (tests/conftest.py)",
        ),
        (
            "pyproject.toml",
            "# changed\n",
            "the package configuration changed (pyproject.toml)",
        ),
        ("uv.lock", "version = 1\n", "a lockfile changed (uv.lock)"),
        (
            "requirements-dev.txt",
            "pytest\n",
            "a lockfile changed (requirements-dev.txt)",
        ),
        (
            "mise.toml",
            "[tools]\n",
            "tool versions or settings changed (mise.toml)",
        ),
        (
            ".github/workflows/ci.yml",
            "on: push\n",
            "CI configuration changed (.github/workflows/ci.yml)",
        ),
        (
            "tests/data/cases.json",
            "[]\n",
            "pytest cannot trace tests/data/cases.json to the tests that use it",
        ),
    ] {
        let dir = repo("pytest", "");
        let root = dir.path();
        // An ordinary change beside it does not narrow the run.
        write(
            &root.join("src/calc/fmt.py"),
            "def shout(s):\n    return s\n",
        );
        let path_buf = root.join(path);
        let text = match std::fs::read_to_string(&path_buf) {
            Ok(old) => format!("{old}{text}"),
            Err(_) => text.to_string(),
        };
        write(&path_buf, &text);
        let plan = plan_in(root).unwrap();
        let suite = &plan.suites[0];
        assert_eq!(full_reason(suite), why, "{path}");
        assert!(suite.tests.is_empty());
        let command = suite.command.as_ref().unwrap();
        assert_eq!(command.len(), 1, "{command:?}");
    }
}

#[test]
fn files_outside_every_package_run_everything_and_other_packages_are_ignored() {
    let dir = repo("pytest", "py");
    let root = dir.path();
    // Another ecosystem's package: its code cannot reach these tests.
    write(&root.join("api/go.mod"), "module api\n");
    write(&root.join("api/main.go"), "package main\n");
    let plan = plan_in(&root.join("py")).unwrap();
    let suite = &plan.suites[0];
    assert_eq!(suite.project.rel, "py");
    assert_eq!(
        nothing_reason(suite),
        "no change pytest tests can see: only docs and other packages' files"
    );
    assert_eq!(
        suite.files[1].effect,
        Effect::Ignored("in another package (api), which pytest tests cannot import".into())
    );
    // A script in no package at all could be anything.
    write(&root.join("scripts/gen.sh"), "echo\n");
    let plan = plan_in(&root.join("py/src")).unwrap();
    assert_eq!(
        full_reason(&plan.suites[0]),
        "scripts/gen.sh is outside every package, so jouet cannot tell what it affects"
    );
}

#[test]
fn javascript_configuration_and_deletions_run_the_whole_suite() {
    let dir = repo("vitest", "");
    let root = dir.path();
    write(&root.join("vitest.config.js"), "export default {}\n");
    let plan = plan_in(root).unwrap();
    let [suite] = plan.suites.as_slice() else {
        panic!("one suite")
    };
    assert_eq!(suite.adapter.name(), "vitest");
    assert_eq!(
        full_reason(suite),
        "the test configuration changed (vitest.config.js)"
    );
    let command = suite.command.as_ref().unwrap();
    assert!(command[0].ends_with("node_modules/.bin/vitest"));
    assert_eq!(command[1..], ["run"]);

    let dir = repo("jest", "");
    let root = dir.path();
    git(root, &["rm", "-q", "src/fmt.js"]);
    let plan = plan_in(root).unwrap();
    assert_eq!(plan.suites[0].adapter.name(), "jest");
    assert_eq!(
        full_reason(&plan.suites[0]),
        "src/fmt.js was deleted, and jest cannot trace a file that is gone"
    );
}

#[test]
fn each_suite_ignores_the_other_suites_files() {
    let dir = repo("pytest", "");
    let root = dir.path();
    copy(&fixture("vitest"), &root.join("web"));
    // One directory with both: the JavaScript project's files beside the
    // Python one's.
    for name in ["package.json", "vitest.config.js"] {
        std::fs::rename(root.join("web").join(name), root.join(name)).unwrap();
    }
    git(root, &["checkout", "-q", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "add web"]);
    git(root, &["checkout", "-q", "-B", "work"]);
    write(
        &root.join("src/calc/fmt.py"),
        "def shout(s):\n    return s\n",
    );
    let plan = plan_in(root).unwrap();
    let names: Vec<&str> = plan.suites.iter().map(|s| s.adapter.name()).collect();
    assert_eq!(names, ["vitest", "pytest"]);
    assert_eq!(
        nothing_reason(&plan.suites[0]),
        "no change vitest tests can see: only docs and other packages' files"
    );
    assert_eq!(
        plan.suites[0].files[0].effect,
        Effect::Ignored("pytest's, not vitest's".into())
    );
    assert_eq!(
        tests_of(&plan.suites[1]),
        [(
            "tests/test_fmt.py",
            vec!["src/calc/fmt.py", "tests/test_fmt.py"]
        )]
    );
}

#[test]
fn adapters_and_repositories_that_cannot_be_used_are_refused() {
    let dir = repo("pytest", "");
    let root = dir.path();
    let e = plan(root, None, Some("rspec"), &[]).err().unwrap();
    assert_eq!(e.exit, Exit::Usage);
    assert!(e.message.contains("vitest, jest, pytest"), "{}", e.message);
    let e = plan(root, None, Some("jest"), &[]).err().unwrap();
    assert_eq!(e.exit, Exit::Precondition);
    assert!(e.message.contains("no jest project"), "{}", e.message);
    let e = plan(root, Some("no-such-ref"), None, &[]).err().unwrap();
    assert_eq!(e.exit, Exit::Usage);

    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q", "-b", "main"]);
    let e = plan(empty.path(), None, None, &[]).err().unwrap();
    assert_eq!(e.exit, Exit::Precondition);
    assert!(e.message.starts_with("no test runner jouet can select for"));
}

#[test]
fn the_base_defaults_to_the_upstream_then_the_default_branch() {
    let dir = repo("pytest", "");
    let root = dir.path();
    assert_eq!(changes::base(root, None).unwrap().name, "main");
    git(root, &["branch", "-q", "--set-upstream-to=main"]);
    assert_eq!(changes::base(root, None).unwrap().name, "main");
    git(root, &["checkout", "-q", "-b", "next"]);
    git(root, &["branch", "-q", "--set-upstream-to=work"]);
    assert_eq!(changes::base(root, None).unwrap().name, "work");
}

#[test]
fn a_branchs_own_pushed_copy_is_not_its_base() {
    // After `git push -u origin feat`, the upstream is origin/feat, which
    // holds the change itself.
    let dir = repo("pytest", "");
    let root = dir.path();
    let remote = tempfile::tempdir().unwrap();
    let url = remote.path().to_string_lossy().into_owned();
    git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(root, &["remote", "add", "origin", &url]);
    git(root, &["push", "-q", "origin", "main"]);
    git(root, &["remote", "set-head", "origin", "main"]);
    git(root, &["checkout", "-q", "-b", "feat"]);
    write(&root.join("new.txt"), "x\n");
    git(root, &["add", "new.txt"]);
    git(root, &["commit", "-q", "-m", "feat"]);
    git(root, &["push", "-q", "-u", "origin", "feat"]);
    let base = changes::base(root, None).unwrap();
    assert_eq!(base.name, "origin/main");
    assert!(!changes::unchanged_since(root, &base.merge_base).unwrap());
}
