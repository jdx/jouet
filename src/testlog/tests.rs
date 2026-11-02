use super::*;

/// A runner's real output, from `fixtures/`, with paths made `/work`.
fn fixture(name: &str) -> String {
    let path = format!(
        "{}/src/testlog/fixtures/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

fn names(d: &Digest) -> Vec<&str> {
    d.failures.iter().map(|f| f.name.as_str()).collect()
}

fn counts(d: &Digest) -> [u64; 3] {
    [d.passed, d.failed, d.skipped]
}

#[test]
fn cargo_test_failures_keep_the_panic_and_drop_libtest_noise() {
    let d = digest(&fixture("cargo-test"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [40, 3, 0]);
    assert_eq!(
        names(&d),
        [
            "tests::adds_negative_numbers",
            "tests::parses_padded_input",
            "tests::unwraps_none"
        ]
    );
    assert_eq!(
        d.failures[0].lines,
        [
            "checking negatives",
            "",
            "thread 'tests::adds_negative_numbers' (1234) panicked at src/lib.rs:9:74:",
            "assertion `left == right` failed: sum of negatives",
            "  left: -5",
            " right: -6",
        ]
    );
    assert!(d.more_failures.is_empty());
}

#[test]
fn nextest_failures_are_named_once_with_their_streams() {
    let d = digest(&fixture("nextest"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [37, 2, 0]);
    assert_eq!(
        names(&d),
        [
            "demo tests::adds_negative_numbers",
            "demo tests::parses_padded_input"
        ]
    );
    // The empty stdout section is gone, and so is libtest's own summary.
    assert_eq!(
        d.failures[1].lines,
        [
            "  stderr ───",
            "",
            "    thread 'tests::parses_padded_input' (1234) panicked at src/lib.rs:10:64:",
            "    parse failed: Err(\"invalid digit found in string\")",
        ]
    );
}

#[test]
fn rustc_errors_are_named_by_location_and_warnings_counted() {
    let d = digest(&fixture("cargo-build"), true, DEFAULT_BUDGET);
    assert_eq!(
        names(&d),
        [
            "src/lib.rs:1:61: error[E0425]: cannot find value `undefined_var` in this scope",
            "src/lib.rs:2:23: error[E0308]: mismatched types",
        ]
    );
    assert!(
        d.failures[1]
            .lines
            .contains(&"help: try using a conversion method".to_string())
    );

    let warned = "   Compiling demo v0.1.0 (/work)
warning: unused variable: `unused`
 --> src/lib.rs:1:41
  |
1 | pub fn add(a: i32, b: i32) -> i32 { let unused = 1; a + b }
  |                                         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_unused`

warning: `demo` (lib) generated 1 warning (run `cargo fix --lib -p demo` to apply 1 suggestion)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
";
    let d = digest(warned, false, DEFAULT_BUDGET);
    assert_eq!(d.warnings, 1);
    assert!(d.failures.is_empty());
}

#[test]
fn go_subtests_replace_their_parent_and_build_errors_are_grouped() {
    let d = digest(&fixture("go"), true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["TestAdd", "TestTable/zero", "TestTable/one"]);
    assert_eq!(
        d.failures[0].lines,
        ["calc_test.go:7: Add(2, 2) = 5, want 4"]
    );

    // Two top-level tests failed.
    assert_eq!(counts(&d), [0, 2, 0]);

    // With -v, a test logs before its result line.
    let d = digest(&fixture("go-v"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [29, 1, 0]);
    assert_eq!(names(&d), ["TestAdd6"]);
    assert_eq!(
        d.failures[0].lines,
        [
            "calc_test.go:48: checking 6",
            "calc_test.go:50: Add(6, 1) = 0, want 7"
        ]
    );

    let d = digest(&fixture("go-build"), true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["build example.com/calc"]);
    assert_eq!(d.failures[0].lines.len(), 2);
}

#[test]
fn pytest_sections_merge_with_the_short_summary() {
    let d = digest(&fixture("pytest-q"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [2, 3, 1]);
    assert_eq!(
        names(&d),
        [
            "tests/test_calc.py::test_add",
            "tests/test_calc.py::TestStrings::test_upper",
            "tests/test_calc.py::test_param[2]",
        ]
    );
    assert_eq!(
        d.failures[0].lines.last().unwrap(),
        "tests/test_calc.py:7: AssertionError"
    );

    let d = digest(&fixture("pytest"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [0, 1, 0]);
    assert_eq!(names(&d), ["ERROR collecting tests/test_broken.py"]);
    assert!(
        d.failures[0]
            .lines
            .contains(&"E   ModuleNotFoundError: No module named 'not_a_module'".to_string())
    );
}

#[test]
fn unittest_failures_and_errors_count_as_failed() {
    let d = digest(&fixture("unittest"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [1, 2, 0]);
    assert_eq!(
        names(&d),
        [
            "test_err (test_u.T.test_err)",
            "test_add (test_u.T.test_add)"
        ]
    );
    assert_eq!(
        d.failures[1].lines.last().unwrap(),
        "AssertionError: 2 != 3"
    );
}

#[test]
fn jest_and_vitest_keep_the_assertion_and_the_diff() {
    let d = digest(&fixture("jest"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [1, 2, 0]);
    assert_eq!(names(&d), ["add › adds", "add › objects"]);
    assert_eq!(
        d.failures[0].lines[0],
        "    expect(received).toBe(expected) // Object.is equality"
    );

    let d = digest(&fixture("vitest"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [1, 2, 0]);
    assert_eq!(
        names(&d),
        [
            "test/broken.test.js [ test/broken.test.js ]",
            "test/calc.test.js > add > adds",
            "test/calc.test.js > add > objects",
        ]
    );
    let objects = &d.failures[2].lines;
    assert_eq!(
        objects[0],
        "AssertionError: expected { a: 1, b: [ 1, 2 ] } to deeply equal { a: 1, b: [ 1, 3 ] }"
    );
    // The middle of a long failure keeps its diff.
    assert!(objects.contains(&"-     3,".to_string()), "{objects:#?}");
    assert!(objects.contains(&"+     2,".to_string()), "{objects:#?}");
    assert!(objects.iter().any(|l| l.ends_with("lines skipped")));
}

#[test]
fn bats_failures_carry_their_comment_lines() {
    let d = digest(&fixture("bats"), true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [1, 2, 1]);
    assert_eq!(names(&d), ["greets", "fails with output"]);
    assert_eq!(
        d.failures[1].lines,
        [
            "(in test file t.bats, line 10)",
            "  `false' failed",
            "some diagnostic"
        ]
    );
}

#[test]
fn a_passing_run_has_counts_and_no_failures() {
    let text = "running 2 tests\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 2 passed; 0 failed; 1 ignored; 0 measured\n\nrunning 1 test\ntest c ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured\n";
    let d = digest(text, false, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [3, 0, 1]);
    assert!(d.failures.is_empty());
}

#[test]
fn an_unrecognised_failure_shows_the_end_of_the_output() {
    let text: String = (1..=50).map(|i| format!("step {i}\n")).collect::<String>() + "boom\n";
    let d = digest(&text, true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["no failure recognised; the end of the output"]);
    let lines = &d.failures[0].lines;
    assert_eq!(lines.first().unwrap(), "... 31 lines skipped");
    assert_eq!(lines.last().unwrap(), "boom");
    // A passing run that says nothing recognisable shows nothing.
    assert!(digest(&text, false, DEFAULT_BUDGET).failures.is_empty());
}

#[test]
fn colour_carriage_returns_and_task_prefixes_are_cleaned() {
    let text = "[test:e2e] 1..2\n[test:e2e] ok 1 a\n[test:e2e] progress 10%\rprogress 100%\n[test:e2e] \x1b[31mnot ok 2 b\x1b[0m\n[test:e2e] # it broke\n";
    let d = digest(text, true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [1, 1, 0]);
    assert_eq!(names(&d), ["b"]);
    assert_eq!(d.failures[0].lines, ["it broke"]);
}

#[test]
fn the_budget_shows_failures_in_order_then_names_then_a_count() {
    let mut text = String::new();
    for i in 0..300 {
        text.push_str(&format!("test t{i:03} ... FAILED\n"));
    }
    text.push_str("\nfailures:\n\n");
    for i in 0..300 {
        text.push_str(&format!(
            "---- t{i:03} stdout ----\nthread 't{i:03}' panicked at src/lib.rs:{i}:1:\n{}\n\n",
            "x".repeat(200)
        ));
    }
    let d = digest(&text, true, 1000);
    // Each failure costs about 300 bytes, so three fit.
    assert_eq!(names(&d), ["t000", "t001", "t002"]);
    assert_eq!(d.more_failures.first().unwrap(), "t003");
    let named = 3 + d.more_failures.len() as u64;
    assert_eq!(named + d.failures_omitted, 300);
    assert!(d.failures_omitted > 0);

    // A first failure bigger than the budget is cut to it.
    let d = digest(&text, true, 100);
    assert_eq!(names(&d), ["t000"]);
    assert_eq!(d.failures[0].lines.last().unwrap(), "... 1 line skipped");

    // 0 removes the budget.
    assert_eq!(digest(&text, true, 0).failures.len(), 300);
}

#[test]
fn a_long_line_is_cut_and_says_by_how_much() {
    let text = format!(
        "test t ... FAILED\n\nfailures:\n\n---- t stdout ----\nthread 't' panicked at src/lib.rs:1:1:\n  left: \"{}\"\n right: \"b\"\n\n",
        "é".repeat(300)
    );
    let d = digest(&text, true, DEFAULT_BUDGET);
    let lines = &d.failures[0].lines;
    assert_eq!(lines.len(), 3);
    assert!(lines[1].ends_with(" ... 211 more bytes"), "{}", lines[1]);
    assert_eq!(lines[2], " right: \"b\"");
}

/// An e2e script's format, as a project would declare it.
const E2E_FORMAT: &str = r#"
description = "A shell e2e runner"
failed = [
  '^(?:ERROR: )?E2E test (?<name>\S+) failed: exited with status code \d+$',
  '^::error .*title=E2E test (?<name>\S+) failed::',
]
start = ['^(?:>>> |::group::)E2E (\S+)$']
excerpt = ['^(?:ERROR: )?E2E assertion failed: ', '^::error .*title=E2E assertion failed::']
drop = ['^\S+: \d+s$']

[[examples]]
output = """
>>> E2E cli/test_a
DEBUG: $ mise ls
ERROR: E2E assertion failed: expected 'x'
cli/test_a: 1s
ERROR: E2E test cli/test_a failed: exited with status code 1
"""
failures = ["cli/test_a"]
"#;

#[test]
fn a_declared_format_names_failures_with_their_excerpt() {
    // Without it, nothing in the output is recognised.
    let d = digest(&fixture("e2e-script"), true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["no failure recognised; the end of the output"]);

    let format = Format::parse("e2e", E2E_FORMAT).unwrap();
    let d = digest_with(&fixture("e2e-script"), true, DEFAULT_BUDGET, &[format]);
    assert_eq!(counts(&d), [0, 1, 0]);
    assert_eq!(names(&d), ["backend/test_vfox_exec_env_cache"]);
    let lines = &d.failures[0].lines;
    assert!(
        lines[0].starts_with("ERROR: E2E assertion failed: [mise env -s bash]"),
        "{lines:?}"
    );
    assert_eq!(lines.len(), 2, "{lines:?}");

    // The same runner's GitHub Actions annotations.
    let text = "::group::E2E cli/test_a\nDEBUG: $ mise ls\n::error file=e2e/cli/test_a,title=E2E assertion failed::expected 'x'\n::endgroup::\n::error file=e2e/cli/test_a,title=E2E test cli/test_a failed::exited with status code 1\n";
    let format = Format::parse("e2e", E2E_FORMAT).unwrap();
    let d = digest_with(text, true, DEFAULT_BUDGET, &[format]);
    assert_eq!(names(&d), ["cli/test_a"]);
    assert_eq!(
        d.failures[0].lines[0],
        "::error file=e2e/cli/test_a,title=E2E assertion failed::expected 'x'"
    );
}

#[test]
fn a_format_that_cannot_work_does_not_load() {
    for (text, why) in [
        ("failed = []", "needs at least one pattern"),
        ("failed = ['^FAIL']", "needs a group that names the test"),
        ("failed = ['^FAIL (']", "`failed` pattern"),
        ("failed = ['^FAIL (\\S+)']\ncolour = true", "unknown field"),
        (
            "failed = ['^FAIL (\\S+)']\n[[examples]]\noutput = 'FAIL a'\nfailures = ['b']",
            "example 1 names [\"a\"], not [\"b\"]",
        ),
    ] {
        let err = Format::parse("x", text).unwrap_err();
        assert!(err.contains(why), "{text}: {err}");
    }
}

#[test]
fn a_one_line_failure_keeps_its_line() {
    let format = Format::parse("one", "failed = ['^FAIL (?<name>\\w+):']").unwrap();
    let d = digest_with(
        "ok case_b\nFAIL case_a: expected 1, got 2\n",
        true,
        DEFAULT_BUDGET,
        &[format],
    );
    assert_eq!(names(&d), ["case_a"]);
    assert_eq!(d.failures[0].lines, ["FAIL case_a: expected 1, got 2"]);
}

#[test]
fn nested_mise_tasks_lose_every_prefix() {
    let out = "[test:bats] [test:bats:libgit2] 1..2\n[test:bats] [test:bats:libgit2] ok 1 a\n[test:bats] [test:bats:libgit2] not ok 2 b\n[test:bats] [test:bats:libgit2] # it broke\n";
    let d = digest(out, true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["b"]);
    assert_eq!(d.failures[0].lines, ["it broke"]);
}

#[test]
fn a_go_package_that_fails_without_a_test_of_its_own_is_named() {
    // TestMain gave up on a service it needed: no `--- FAIL` line.
    let d = digest(&fixture("go-package"), true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["example.com/app/blob", "TestBuild"]);
    assert_eq!(
        d.failures[0].lines,
        [
            "2026/11/01 00:40:58 reached retry deadline: Get \"http://localhost:32768/minio/health/ready\": dial tcp [::1]:32768: connect: connection refused"
        ]
    );
    assert_eq!(d.failed, 2);
}

#[test]
fn a_failed_build_script_shows_how_it_exited_and_its_stderr() {
    let d = digest(&fixture("cargo-build-script"), true, DEFAULT_BUDGET);
    assert_eq!(names(&d), ["build script zstd-sys v2.1.2+zstd.1.5.7"]);
    let lines = &d.failures[0].lines;
    assert!(
        lines[0].starts_with("process didn't exit successfully:"),
        "{lines:?}"
    );
    assert_eq!(
        lines[1..],
        [
            "error occurred in cc-rs: failed to find tool \"x86_64-w64-mingw32-gcc\": No such file or directory (os error 2)"
        ]
    );
}

#[test]
fn rustfmt_check_names_each_place_with_its_diff() {
    let out = "Diff in /work/src/a.rs:12:\n     let x = 1;\n-    f(a,b);\n+    f(a, b);\n\nDiff in /work/src/b.rs at line 3:\n-use b;\n+use a;\n";
    let d = digest(out, true, DEFAULT_BUDGET);
    assert_eq!(
        names(&d),
        ["rustfmt /work/src/a.rs:12", "rustfmt /work/src/b.rs:3"]
    );
    assert_eq!(
        d.failures[0].lines,
        ["     let x = 1;", "-    f(a,b);", "+    f(a, b);"]
    );
}

#[test]
fn a_panics_backtrace_folds_so_its_message_is_shown() {
    // A mise e2e run whose command failed with color-eyre's report, and no
    // runner jouet knows: the message sits above some 40 frames.
    let d = digest_with(&fixture("mise-e2e-eyre"), true, DEFAULT_BUDGET, &[]);
    let lines = &d.failures[0].lines;
    assert!(
        lines
            .iter()
            .any(|l| l.trim() == "0: dummy@latest is not installed"),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with("backtrace lines skipped")),
        "{lines:#?}"
    );
    // A frame of a trait object folds too, and mbx's build lines are noise.
    assert!(!lines.iter().any(|l| l.contains("<&dyn")), "{lines:#?}");
    assert!(
        !lines.iter().any(|l| l.contains("mbx[cache]")),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("[mise uninstall dummy@latest] command failed")),
        "{lines:#?}"
    );
}

#[test]
fn a_failure_that_blames_the_machine_is_told_apart() {
    let dbus = "1..2\nok 1 edit command accepts an editor command\nnot ok 2 edit command: create, edit, and remove keychain secrets\n# WARN fnox_core::keyring_store: Failed to initialize OS keyring store: Platform failure: DBus error: Using X11 for dbus-daemon autolaunch\n# (in test file test/edit.bats, line 120)\n";
    assert!(environment_cause(dbus, &[]).unwrap().contains("DBus error"));
    let lua = "error: failed to run custom build command for `mlua-sys v0.6.8`\n\nCaused by:\n  process didn't exit successfully: `/w/target/debug/build/mlua-sys/out/build_script_build` (exit status: 101)\n  --- stdout\n  cargo:rerun-if-env-changed=LUA_INC\n  --- stderr\n\n  thread 'main' panicked at build/find_normal.rs:\n  could not find system library 'lua5.1' required by the 'mlua-sys' crate\n";
    assert!(environment_cause(lua, &[]).is_some());
    // One failure the change caused, beside the machine's: not the machine's.
    let mixed = "1..2\nnot ok 1 a\n# DBus error: no bus\nnot ok 2 b\n# expected 1, got 2\n";
    assert_eq!(environment_cause(mixed, &[]), None);
    // A missing command is most often a binary the change did not build.
    assert_eq!(
        environment_cause("1..1\nnot ok 1 a\n# fnox: command not found\n", &[]),
        None
    );
}
