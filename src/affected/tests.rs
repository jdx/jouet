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

fn plan_in(dir: &Path) -> Result<Plan, Error> {
    plan(dir, Some("main"), None, &[])
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
        "no test depends on the changed files"
    );
    assert_eq!(plan.suites[0].command, None);
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
