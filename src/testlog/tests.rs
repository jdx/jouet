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
fn playwright_failures_keep_the_test_path_and_assertion() {
    let output = "Running 3 tests using 1 worker\n\n  1) [chromium] › e2e/balance.spec.ts:5:3 › account › updates balance ───\n\n    Error: expect(locator).toHaveText(expected) failed\n    Expected: \"42.00\"\n    Received: \"17.00\"\n\n    attachment #1: screenshot ───\n    test-results/screenshot.png\n\n  2) e2e/account.test.ts:9:2 › account › opens\n\n    TimeoutError: waiting for button\n\n  2 failed\n    e2e/balance.spec.ts:5:3 › account › updates balance\n    e2e/account.test.ts:9:2 › account › opens\n  1 passed (12s)\n";
    let d = digest(output, true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [1, 2, 0]);
    assert_eq!(
        names(&d),
        [
            "[chromium] › e2e/balance.spec.ts:5:3 › account › updates balance",
            "e2e/account.test.ts:9:2 › account › opens"
        ]
    );
    assert!(
        d.failures[0]
            .lines
            .iter()
            .any(|line| line.contains("42.00"))
    );
    assert!(
        !d.failures[0]
            .lines
            .iter()
            .any(|line| line.contains("screenshot"))
    );
    assert!(
        d.failures[1]
            .lines
            .iter()
            .any(|line| line.contains("TimeoutError"))
    );
}

#[test]
fn playwright_names_need_a_run_and_a_failure_detail() {
    let heading = "  1) e2e/a.test.ts:1:1 › a test\n";
    let without_run = format!("{heading}    Error: failure\n");
    assert!(
        digest(&without_run, false, DEFAULT_BUDGET)
            .failures
            .is_empty()
    );
    let without_detail = format!("Running 1 test using 1 worker\n{heading}  1 failed\n");
    assert!(
        digest(&without_detail, false, DEFAULT_BUDGET)
            .failures
            .is_empty()
    );
    // A pytest summary following a Playwright run is not another Playwright
    // summary, even when the surrounding task indents it.
    let mixed = "Running 1 test using 1 worker\n  1 passed (1.0s)\n  2 passed in 0.2s\n";
    assert_eq!(counts(&digest(mixed, false, DEFAULT_BUDGET)), [1, 0, 0]);
}

#[test]
fn playwright_shards_name_bare_test_timeouts() {
    let output = "Running 1 test using 1 worker, shard 1 of 2\n  1) e2e/a.spec.ts:5:1 › waits for data\n    Test timeout of 30000ms exceeded.\n  1 failed\n";
    let d = digest(output, true, DEFAULT_BUDGET);
    assert_eq!(counts(&d), [0, 1, 0]);
    assert_eq!(names(&d), ["e2e/a.spec.ts:5:1 › waits for data"]);
    assert!(d.failures[0].lines[0].contains("Test timeout of 30000ms exceeded"));
    // A generic timeout does not prove the baseline failed for the same reason.
    assert!(failure_details(output, &[]).is_empty());
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
fn an_unrecognised_failure_also_shows_lines_marked_as_failures_above_the_end() {
    let text = "FAIL swebench-pylint-4551: the check should pass when solved:\nFAIL_TO_PASS tests/a.py::t not run\n"
        .to_string()
        + &(1..=50).map(|i| format!("ok   step {i}\n")).collect::<String>();
    let d = digest(&text, true, DEFAULT_BUDGET);
    assert_eq!(
        names(&d),
        [
            "no failure recognised; lines marked as failures",
            "no failure recognised; the end of the output"
        ]
    );
    assert_eq!(
        d.failures[0].lines,
        ["FAIL swebench-pylint-4551: the check should pass when solved:"]
    );
    // A machine the end blames is still blamed, whatever the marked lines say.
    let text = "FAIL keyring: stores the token\n".to_string()
        + &(1..=30).map(|i| format!("step {i}\n")).collect::<String>()
        + "Error: DBus error: no bus\n";
    assert!(environment_cause(&text, &[]).is_some());
}

#[test]
fn rspec_keeps_named_failures_and_their_assertions() {
    let output = r#"
==> Summary (2 workers)
    [ 1] 4 examples, 1 pending, 2 failures
    [ 2] 2 examples, 0 failures
==> Failures

  1) RuboCop::Config excludes a file
     Failure/Error: expect(configuration).to be_file_to_exclude('bar.rb')
       expected true, got false
       1) first item
       2) second item
       expected the second item to match
     # ./spec/rubocop/config_spec.rb:625

  2) Another example fails
     Failure/Error: expect(1).to eq(2)
       expected: 2
            got: 1
     # ./spec/other_spec.rb:7
==> Failed Examples
rspec ./spec/rubocop/config_spec.rb:619
"#;
    let d = digest(output, true, DEFAULT_BUDGET);
    assert_eq!(
        names(&d),
        ["RuboCop::Config excludes a file", "Another example fails"]
    );
    assert_eq!((d.passed, d.failed, d.skipped), (3, 2, 1));
    assert!(
        d.failures[0]
            .lines
            .iter()
            .any(|line| line.contains("expected true, got false"))
    );
    assert!(
        d.failures[0]
            .lines
            .iter()
            .any(|line| line.contains("expected the second item"))
    );
    assert!(
        d.failures[1]
            .lines
            .iter()
            .any(|line| line.contains("expected: 2"))
    );
}

#[test]
fn numbered_text_without_an_rspec_failure_is_not_a_failure() {
    let output = "Failures:\n  1) A numbered diagnostic\n     an explanation\nFinished in 0.1s\n";
    assert!(digest(output, false, DEFAULT_BUDGET).failures.is_empty());
}

#[test]
fn rspec_reports_an_empty_selection_only_when_all_summaries_are_empty() {
    assert!(digest("0 examples, 0 failures\n", false, DEFAULT_BUDGET).no_tests);
    assert!(
        !digest(
            "0 examples, 0 failures\n1 example, 0 failures\n",
            false,
            DEFAULT_BUDGET
        )
        .no_tests
    );
}

#[test]
fn rspec_hook_errors_are_not_an_empty_test_selection() {
    let output = "An error occurred in a before(:suite) hook.\n0 examples, 0 failures, 1 error occurred outside of examples\n";
    let d = digest(output, true, DEFAULT_BUDGET);
    assert_eq!(d.failed, 1);
    assert!(!d.no_tests);
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
fn a_run_that_selected_no_test_is_told_apart_from_a_pass() {
    // bats with a filter that matched nothing: TAP's empty plan.
    assert!(digest("1..0\n", false, DEFAULT_BUDGET).no_tests);
    // cargo test with a name filter that matched nothing in any binary.
    let none = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out\n";
    assert!(digest(none, false, DEFAULT_BUDGET).no_tests);
    // One binary ran a test: that is a run.
    let one = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n\nrunning 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored\n";
    assert!(!digest(one, false, DEFAULT_BUDGET).no_tests);
    for said in [
        "collected 0 items",
        "========== no tests ran in 0.01s ==========",
        "No tests found, exiting with code 1",
        "Error: No tests found.",
        "testing: warning: no tests to run",
    ] {
        assert!(digest(said, false, DEFAULT_BUDGET).no_tests, "{said}");
    }
    // pytest with -k deselecting every test, and quiet output.
    for said in [
        "======= 5 deselected in 0.01s =======",
        "5 deselected in 0.01s",
        "no tests ran in 0.01s",
        "collected 5 items / 5 deselected / 0 selected",
    ] {
        assert!(digest(said, false, DEFAULT_BUDGET).no_tests, "{said}");
    }
    // Some passed, some deselected: a run.
    assert!(
        !digest(
            "== 2 passed, 3 deselected in 0.02s ==",
            false,
            DEFAULT_BUDGET
        )
        .no_tests
    );
    // Go package by package: empty only when every package ran none.
    let go_empty = "ok  \tgithub.com/a/b\t0.004s [no tests to run]\nok  \tgithub.com/a/c\t0.003s [no tests to run]\n";
    assert!(digest(go_empty, false, DEFAULT_BUDGET).no_tests);
    let go_one = "ok  \tgithub.com/a/b\t0.004s [no tests to run]\nok  \tgithub.com/a/c\t0.120s\n";
    assert!(!digest(go_one, false, DEFAULT_BUDGET).no_tests);
    let go_no_files =
        "?   \tgithub.com/a/b\t[no test files]\n?   \tgithub.com/a/c\t[no test files]\n";
    assert!(digest(go_no_files, false, DEFAULT_BUDGET).no_tests);
    let go_no_files_and_empty = format!("{go_no_files}{go_empty}");
    assert!(digest(&go_no_files_and_empty, false, DEFAULT_BUDGET).no_tests);
    let go_no_files_and_test = format!("{go_no_files}ok  \tgithub.com/a/d\t0.120s\n");
    assert!(!digest(&go_no_files_and_test, false, DEFAULT_BUDGET).no_tests);
    // An empty cargo suite, then Go tests that ran: Go's plain lines are
    // not counted as passes, but they show a run.
    let mixed = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\nok  \tgithub.com/a/b\t0.120s\n";
    assert!(!digest(mixed, false, DEFAULT_BUDGET).no_tests);
    // A build says nothing about tests.
    assert!(
        !digest(
            "   Compiling a v0.1.0\n    Finished `dev` profile\n",
            false,
            DEFAULT_BUDGET
        )
        .no_tests
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
    // Two suites with a test of one name: the change's failure in the
    // second is not hidden by the machine's in the first.
    let twice = "1..1\nnot ok 1 secrets\n# DBus error: no bus\n# (in test file a.bats, line 3)\n# from the keyring store\n1..1\nnot ok 1 secrets\n# expected 1, got 2\n";
    assert_eq!(environment_cause(twice, &[]), None);
    // An assertion that only quotes a machine error is the test's own.
    let quoted =
        "1..1\nnot ok 1 reports the bus\n# expected: DBus error: no bus\n# actual: success\n";
    assert_eq!(environment_cause(quoted, &[]), None);
    // libtest names a failure twice, the second time with its block.
    let libtest = "running 1 test\ntest keyring ... FAILED\n\nfailures:\n\n---- keyring stdout ----\nthread panicked: DBus error: no bus\n\nfailures:\n    keyring\n\ntest result: FAILED. 0 passed; 1 failed\n";
    assert!(environment_cause(libtest, &[]).is_some());
    // No database or service running here.
    for line in [
        "psql: error: could not connect to server: No such file or directory",
        "sqlalchemy.exc.OperationalError: connection to server at \"localhost\" (::1), port 5432 failed: Connection refused",
        "psql: error: connection to server on socket \"/tmp/.s.PGSQL.5432\" failed: No such file or directory",
        "Error: connect ECONNREFUSED 127.0.0.1:6379",
        "        dial error: dial tcp 127.0.0.1:5432: connect: connection refused",
        // Go with no network: offline, or a name that does not resolve.
        "mise go@1.27.3 go: github.com/go-delve/delve/cmd/dlv@latest: module lookup disabled by GOPROXY=off",
        "Get \"https://proxy.golang.org/x\": dial tcp: lookup proxy.golang.org on 127.0.0.53:53: no such host",
    ] {
        let text = format!("1..1\nnot ok 1 a\n# {line}\n");
        assert!(environment_cause(&text, &[]).is_some(), "{line}");
    }
    // A database that answers and refuses the login, or has no such
    // database, is the change's.
    for line in [
        "connection to server at \"localhost\" (::1), port 5432 failed: FATAL:  password authentication failed for user \"app\"",
        "failed to connect to `user=app database=app`: server error: FATAL: database \"app\" does not exist",
    ] {
        let text = format!("1..1\nnot ok 1 a\n# {line}\n");
        assert_eq!(environment_cause(&text, &[]), None, "{line}");
    }
    // A refused connection to another host may be the change's own.
    assert_eq!(
        environment_cause(
            "1..1\nnot ok 1 a\n# dial tcp 10.0.0.5:443: connect: connection refused\n",
            &[]
        ),
        None
    );
    // A runner that refused its arguments ran no test.
    assert!(
        usage_error(
            "error: unexpected argument 'guide' found\n\nUsage: cargo test [OPTIONS] [TESTNAME]\n",
            &[]
        )
        .is_some()
    );
    assert!(usage_error("flag provided but not defined: -nope\n", &[]).is_some());
    // argparse and pytest name the program first.
    for line in [
        "usage: pytest [options] [file_or_dir] [file_or_dir] [...]\npytest: error: unrecognized arguments: --nope\n",
        "usage: agents.py run [-h] --tasks TASKS\nagents.py run: error: the following arguments are required: --tasks\n",
        "usage: agents.py [-h]\nagents.py: error: argument --rounds: invalid int value: 'x'\n",
    ] {
        assert!(usage_error(line, &[]).is_some(), "{line}");
    }
    // A compiler's own argument warning is no refusal of the run.
    assert_eq!(
        usage_error(
            "clang: error: argument unused during compilation: '-pthread'\n",
            &[]
        ),
        None
    );
    // A run with tests is judged by them, whatever else it printed.
    assert_eq!(
        usage_error(
            "running 1 test\ntest a ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\nerror: unexpected argument\n",
            &[]
        ),
        None
    );
    // A missing command is most often a binary the change did not build.
    assert_eq!(
        environment_cause("1..1\nnot ok 1 a\n# fnox: command not found\n", &[]),
        None
    );
}

#[test]
fn a_small_passing_run_names_its_tests() {
    let libtest = "running 2 tests\ntest edit::tests::editor_with_args ... ok\ntest edit::tests::visual_fallback ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out\n";
    assert_eq!(
        digest(libtest, false, DEFAULT_BUDGET).passed_names,
        [
            "edit::tests::editor_with_args",
            "edit::tests::visual_fallback"
        ]
    );
    let tap =
        "1..2\nok 1 edit command accepts an editor command\nok 2 falls back to VISUAL # 12ms\n";
    assert_eq!(
        digest(tap, false, DEFAULT_BUDGET).passed_names,
        [
            "edit command accepts an editor command",
            "falls back to VISUAL"
        ]
    );
    // Past eight, or with names missing (cargo test -q), none are named.
    let many: String = (0..9).map(|i| format!("ok {} t{i}\n", i + 1)).collect();
    assert!(
        digest(&format!("1..9\n{many}"), false, DEFAULT_BUDGET)
            .passed_names
            .is_empty()
    );
    // TAP's `-` is not part of the name, and a directive alone names nothing.
    let dash = "1..1\nok 1 - example\n";
    assert_eq!(
        digest(dash, false, DEFAULT_BUDGET).passed_names,
        ["example"]
    );
    let todo = "1..2\nok 1 named\nok 2 # TODO pending\n";
    assert!(digest(todo, false, DEFAULT_BUDGET).passed_names.is_empty());
    // A name too long to show whole is not cut: none are named.
    let long = format!("1..1\nok 1 {}\n", "x".repeat(121));
    assert!(digest(&long, false, DEFAULT_BUDGET).passed_names.is_empty());
    let quiet = "running 2 tests\n..\ntest result: ok. 2 passed; 0 failed; 0 ignored\n";
    assert!(digest(quiet, false, DEFAULT_BUDGET).passed_names.is_empty());
}

#[test]
fn vitest_files_that_failed_to_load_show_their_shared_error_once() {
    let mut text = String::from("⎯⎯⎯⎯⎯⎯ Failed Suites 30 ⎯⎯⎯⎯⎯⎯\n\n");
    for i in 0..30 {
        text.push_str(&format!(
            " FAIL  |zod| src/t{i}.test.ts [ src/t{i}.test.ts ]\n"
        ));
    }
    text.push_str(
        "Error: Transform failed with 1 error:\n/w/src/checks.ts:819:4: ERROR: Expected \")\" but found \"}\"\n  Plugin: vite:esbuild\n\n⎯⎯⎯⎯⎯⎯[1/30]⎯\n\n Test Files  30 failed (30)\n      Tests  no tests\n",
    );
    let d = digest(&text, true, DEFAULT_BUDGET);
    assert_eq!(
        d.failures[0].name,
        "|zod| src/t0.test.ts [ src/t0.test.ts ]"
    );
    assert_eq!(
        d.failures[0].lines[0],
        "Error: Transform failed with 1 error:"
    );
    assert!(d.failures[0].lines[1].contains("checks.ts:819:4"));
    // The others are named, without the error again.
    assert!(d.failures[1..].iter().all(|f| f.lines.is_empty()));
}

#[test]
fn a_few_warnings_are_named_with_their_place_once_each() {
    let warning = "warning: unused variable: `x`\n --> src/a.rs:1:5\n  |\n1 | let x = 1;\n";
    // cargo prints a warning once for each target that builds it.
    let text = format!(
        "{warning}{warning}warning: `fnox` (lib) generated 1 warning\nrunning 1 test\ntest t ... ok\n\ntest result: ok. 1 passed; 0 failed\n"
    );
    let d = digest(&text, false, DEFAULT_BUDGET);
    assert_eq!(d.warning_names, ["unused variable: `x` (src/a.rs:1:5)"]);
    let many: String = (0..6)
        .map(|i| format!("warning: unused variable: `x{i}`\n --> src/a.rs:{i}:5\n"))
        .collect();
    assert!(
        digest(&many, false, DEFAULT_BUDGET)
            .warning_names
            .is_empty()
    );
    // cargo's summary is not a warning, nor does it take the next one's place.
    let summary = "warning: `fnox` (lib) generated 1 warning\nwarning: unused import: `y`\n --> src/b.rs:2:5\n";
    let d = digest(summary, false, DEFAULT_BUDGET);
    assert_eq!(d.warning_names, ["unused import: `y` (src/b.rs:2:5)"]);
    assert_eq!(d.warnings, 1);
    // A long headline is cut, not its place.
    let long = format!("warning: {}\n --> src/c.rs:3:1\n", "x".repeat(300));
    let d = digest(&long, false, DEFAULT_BUDGET);
    assert!(
        d.warning_names[0].ends_with("(src/c.rs:3:1)"),
        "{:?}",
        d.warning_names
    );
}
