//! What the output of a test or build run says, for an agent: how many
//! tests passed, failed and were skipped, and each failure with the lines
//! that explain it, cut to a byte budget. Formats are told apart by their
//! lines, not by the command, since one task (`mise run test`) can run
//! cargo's tests and then bats. The whole output stays in the run's log.

pub(crate) mod format;

pub(crate) use format::Format;
use jouet_wait::trim_excerpt;
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// The bytes of failure text shown when --budget does not say.
pub(crate) const DEFAULT_BUDGET: usize = 4000;
/// The first and last lines of a long failure that are kept: the first
/// usually say what was expected (jest, vitest, pytest's source), the last
/// where it failed (a panic after the test's output, pytest's `E` lines).
const FAILURE_HEAD: usize = 8;
const FAILURE_TAIL: usize = 8;
/// Lines between those kept because they look like an error or a diff.
const FAILURE_MIDDLE: usize = 8;
/// Bytes of one line of a failure that are shown.
const LINE_MAX: usize = 400;
/// Lines shown from the end of an output in which no failure was recognised.
const OUTPUT_TAIL: usize = 20;
/// Bytes past the budget that the names of failures not shown may take.
const NAMES_SLACK: usize = 200;

/// Counts are summed over every summary line the output has, such as one
/// per test binary for cargo; 0 when none says.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Digest {
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) passed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) failed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) skipped: u64,
    /// Compiler warnings (rustc's), which are counted but not shown.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) warnings: u64,
    /// The failures that fit the budget, in the order the output has them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) failures: Vec<Failure>,
    /// The names of the failures after those, which did not fit.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) more_failures: Vec<String>,
    /// Failures past the budget even by name.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) failures_omitted: u64,
    /// The tests that passed, by name, when there are at most
    /// [`PASSES_NAMED`] and the runner names each: a small run says which
    /// tests it ran, so the agent need not read the log to see its own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) passed_names: Vec<String>,
    /// A runner said it ran no test at all: a filter that matched nothing,
    /// so a pass says nothing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) no_tests: bool,
}

/// Whether the output says a run selected no test: TAP's empty plan,
/// libtest running none in every binary, pytest, jest or go saying so.
fn no_tests(lines: &[String]) -> bool {
    // A Go package that ran a test shows it only in its plain `ok` line,
    // which the counts do not see: that is a run.
    let go = |empty: bool| {
        lines
            .iter()
            .any(|l| GO_PACKAGE_OK.is_match(l) && l.contains("[no tests to run]") == empty)
    };
    if go(false) {
        return false;
    }
    let said = go(true) || lines.iter().any(|l| NO_TESTS.is_match(l));
    let mut libtest = lines
        .iter()
        .filter_map(|l| LIBTEST_RUNNING.captures(l))
        .peekable();
    let none_in_libtest = libtest.peek().is_some() && libtest.all(|c| &c[1] == "0");
    said || none_in_libtest
}

/// The most passing tests a digest names.
const PASSES_NAMED: usize = 8;

/// The longest name a digest lists, in characters.
const NAME_MAX: usize = 120;

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Failure {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) lines: Vec<String>,
}

/// A failure where the output has it: the line it starts on.
#[derive(Debug)]
struct Found {
    at: usize,
    name: String,
    lines: Vec<String>,
}

#[derive(Debug, Default)]
struct Parsed {
    passed: u64,
    failed: u64,
    skipped: u64,
    warnings: u64,
    found: Vec<Found>,
    /// The names of the passing tests, from runners that print each.
    passed_names: Vec<String>,
}

impl Parsed {
    fn count(&mut self, passed: u64, failed: u64, skipped: u64) {
        self.passed += passed;
        self.failed += failed;
        self.skipped += skipped;
    }

    /// Adds the counts in a summary line such as `3 failed, 2 passed, 1
    /// skipped`.
    fn count_words(&mut self, text: &str) {
        let (mut passed, mut failed, mut skipped) = (0, 0, 0);
        for c in COUNT.captures_iter(text) {
            let n: u64 = c[1].parse().unwrap_or(0);
            match &c[2] {
                "passed" => passed += n,
                "failed" | "error" | "errors" => failed += n,
                _ => skipped += n,
            }
        }
        self.count(passed, failed, skipped);
    }

    fn push(&mut self, at: usize, name: impl Into<String>, lines: Vec<String>) {
        self.found.push(Found {
            at,
            name: name.into(),
            lines,
        });
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid regex")
}

macro_rules! regexes {
    ($($name:ident = $pattern:expr;)*) => {
        $(static $name: LazyLock<Regex> = LazyLock::new(|| re($pattern));)*
    };
}

regexes! {
    ANSI = "\x1b\\[[0-9;?]*[ -/]*[@-~]";
    TASK_PREFIX = r"^\[[^\]\s]+\] ";
    COUNT = r"\b(\d+) (passed|failed|errors?|skipped|ignored|todo|pending)\b";
    // A line that blames the machine, not the code: no D-Bus or keyring, a
    // system library missing, no network, an API's rate limit. Not a
    // missing command or file, which is most often the change's own (a
    // binary it did not build).
    ENVIRONMENT = r"(?i)(dbus error|failed to connect to (the )?(session |system )?bus|org\.freedesktop\.(dbus|secret)|secret service|error while loading shared libraries|cannot open shared object file|could not find system library|was not found in the pkg-config search path|unable to find library -l|could not resolve host|temporary failure in name resolution|network is unreachable|api rate limit exceeded)";
    // cargo test's libtest.
    LIBTEST_FAILED = r"^test (.+) \.\.\. FAILED$";
    LIBTEST_OK = r"^test (.+) \.\.\. ok$";
    // The description, without the optional `-` before it or a `#`
    // directive after: a directive alone names no test.
    TAP_OK_NAME = r"^ok \d+(?: -)?\s+([^#]*[^#\s])\s*(?:#.*)?$";
    LIBTEST_BLOCK = r"^---- (.+) stdout ----$";
    LIBTEST_END = r"^(---- .+ ----|failures:|test result: .*)$";
    LIBTEST_RESULT = r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored";
    LIBTEST_RUNNING = r"^running (\d+) tests?$";
    // A Go package's result line: `ok  pkg  0.01s` or `(cached)`, with
    // `[no tests to run]` after it when the package ran none.
    GO_PACKAGE_OK = r"^ok\s+\S+\s+(\(cached\)|[\d.]+s)";
    // A runner saying it ran nothing: TAP's `1..0`, pytest, jest, go.
    NO_TESTS = r"^(1\.\.0\b|collected 0 items|(=+ )?no tests ran\b|(=+ )?\d+ deselected in [\d.]+s( =+)?$|collected \d+ items? / \d+ deselected / 0 selected|No tests found|testing: warning: no tests to run)";
    // Lines of libtest's that say nothing about a failure.
    // mbx's build cache and savings lines too, which a cargo build through
    // it prints last.
    RUST_NOISE = r"^\s*(running \d+ tests?|failures:|test result: .*|test \S+ \.\.\. FAILED|note: run with `RUST_BACKTRACE=1`.*|mbx\[\w+\]: .*)$";
    // A line in the middle of a long failure worth keeping: an error, an
    // assertion, or a line of a diff.
    // A numbered line is an error chain's (eyre's `0: x is not installed`):
    // backtrace frames are folded before this is asked.
    TELLING = r"(?i)error|panicked|assert|expected|received|^\s*[-+]\s|^E\s|^\s*\d+: \S";
    // A captured stream's header in nextest's output, new and old.
    SECTION = r"^\s*((stdout|stderr) ─+|--- (STDOUT|STDERR): .* ---)$";
    // cargo nextest.
    NEXTEST_FAIL = r"^\s*(?:FAIL|SIGSEGV|SIGABRT|SIGKILL|SIGBUS|TIMEOUT|ABORT|LEAK-FAIL)\s+\[\s*[\d.]+s\]\s+(?:\(\s*\d+/\d+\)\s+)?(.+)$";
    NEXTEST_NEXT = r"^(\s*(PASS|FAIL|SKIP|SIGSEGV|SIGABRT|SIGKILL|SIGBUS|TIMEOUT|ABORT|SLOW|START|LEAK|LEAK-FAIL|RETRY|FLAKY|TRY \d+ \S+)\s+\[|\s*(Summary|Cancelling|Canceling|Starting) |─{3,}|warning: |error: )";
    NEXTEST_SUMMARY = r"^\s*Summary \[[^\]]*\]\s+(.*)$";
    // rustc's diagnostics.
    RUSTC_ERROR = r"^(error(?:\[E\d{4}\])?): (.+)$";
    RUSTC_WARNING = r"^warning(?:\[[^\]]+\])?: ";
    RUSTC_AT = r"^\s*--> (\S+)$";
    // rustfmt --check, `cargo fmt --check`: each place it would change.
    RUSTFMT_DIFF = r"^Diff in (\S+?)(?::(\d+):| at line (\d+):)$";
    // cargo's build scripts, whose own output is indented under it.
    CARGO_BUILD_SCRIPT = r"^error: failed to run custom build command for `([^`]+)`$";
    // tsc, compact and pretty.
    TSC = r"^(\S+?)(?:\((\d+),(\d+)\): | ?:(\d+):(\d+) - )error (TS\d+): (.+)$";
    // go test and go build.
    GO_RESULT = r"^(\s*)--- (PASS|FAIL|SKIP): (\S+)";
    GO_RUN = r"^\s*=== (RUN|CONT|PAUSE|NAME)\s+(\S+)$";
    GO_BUILD = r"^# (\S+)(?: \[\S+\])?$";
    GO_FILE = r"^\S+\.go:\d+(:\d+)?: ";
    GO_PANIC = r"^panic: ";
    GO_END = r"^(FAIL|ok)\s|^exit status ";
    // A package's result line, `FAIL<tab>pkg<tab>1.2s` or `ok  <tab>pkg<tab>0.1s`:
    // the tab tells it from jest's and vitest's `FAIL path`.
    GO_PACKAGE = r"^(ok|FAIL|\?) *\t(\S+)";
    // pytest.
    PYTEST_SECTION = r"^=+ (FAILURES|ERRORS) =+$";
    PYTEST_HEAD = r"^_{3,} (.+?) _{3,}$";
    PYTEST_RULE = r"^(=+|!+)( .*)?$";
    PYTEST_SHORT = r"^(?:FAILED|ERROR) ([^\s(]\S*)(?: - (.+))?$";
    PYTEST_SUMMARY = r"^(?:=+ )?((?:\d+ [a-z]+(?:, )?)+) in [\d.]+s\b.*$";
    // unittest.
    UNITTEST_RULE = r"^={20,}$";
    UNITTEST_HEAD = r"^(?:FAIL|ERROR): (.+)$";
    UNITTEST_DASH = r"^-{20,}$";
    UNITTEST_RAN = r"^Ran (\d+) tests? in ";
    UNITTEST_DONE = r"^(?:FAILED|OK)(?: \((.*)\))?$";
    UNITTEST_KIND = r"(failures|errors|skipped|expected failures|unexpected successes)=(\d+)";
    // jest.
    JEST_HEAD = r"^\s*● (.+)$";
    JEST_END = r"^\s*● |^(Test Suites|Tests|Snapshots|Time):|^\s*(PASS|FAIL) ";
    JEST_TESTS = r"^Tests:\s+(.+)$";
    // vitest.
    VITEST_HEAD = r"^\s*FAIL\s+(.+ > .+|.+ \[ .+ \])$";
    VITEST_END = r"^\s*(⎯|FAIL\s|Test Files\s|Tests\s)";
    VITEST_TESTS = r"^\s*Tests\s+(\d+ .*)$";
    // TAP, as bats prints it.
    TAP_PLAN = r"^1\.\.(\d+)$";
    TAP_OK = r"^ok \d+ ";
    TAP_SKIP = r"(?i)^ok \d+ .*# skip\b";
    TAP_NOT_OK = r"^not ok \d+ (.+?)(?: # .*)?$";
}

/// What a failed run's `text` blames on the machine, when each failure
/// it names, or its end when it names none, has a line that does (see
/// `ENVIRONMENT`): the first such line, cut short. None when any failure
/// says nothing of the kind, as one the change caused.
pub(crate) fn environment_cause(text: &str, formats: &[Format]) -> Option<String> {
    let digest = digest_with(text, true, 0, formats);
    let mut first = None;
    for failure in &digest.failures {
        let line = failure.lines.iter().find(|l| ENVIRONMENT.is_match(l))?;
        first.get_or_insert_with(|| line.trim().chars().take(120).collect::<String>());
    }
    first
}

/// [`digest_with`] jouet's own formats alone.
#[cfg(test)]
pub(crate) fn digest(text: &str, failed: bool, budget: usize) -> Digest {
    digest_with(text, failed, budget, &[])
}

/// What `text`, a run's whole output, says, by jouet's own formats and
/// `formats`, those declared in `formats/` (see [`format`]). `failed` is
/// whether the run failed: without a failure it recognises, the end of the
/// output is shown instead. `budget` caps the bytes of failure text, 0 for
/// no cap.
pub(crate) fn digest_with(text: &str, failed: bool, budget: usize, formats: &[Format]) -> Digest {
    let lines = clean(text);
    let mut parsed = Parsed::default();
    libtest(&lines, &mut parsed);
    nextest(&lines, &mut parsed);
    rustc(&lines, &mut parsed);
    rustfmt(&lines, &mut parsed);
    tsc(&lines, &mut parsed);
    go(&lines, &mut parsed);
    pytest(&lines, &mut parsed);
    unittest(&lines, &mut parsed);
    jest(&lines, &mut parsed);
    vitest(&lines, &mut parsed);
    tap(&lines, &mut parsed);
    for format in formats {
        format.apply(&lines, &mut parsed);
    }
    let mut found = merge(parsed.found);
    if found.is_empty() && failed {
        let tail: Vec<String> = lines.iter().filter(|l| !l.is_empty()).cloned().collect();
        found.push(Found {
            at: 0,
            name: "no failure recognised; the end of the output".into(),
            lines: trim_excerpt(tail, OUTPUT_TAIL).lines,
        });
    }
    let (failures, more_failures, failures_omitted) = fit(found, budget);
    let no_tests = parsed.passed + parsed.failed == 0 && no_tests(&lines);
    Digest {
        passed: parsed.passed,
        failed: parsed.failed,
        skipped: parsed.skipped,
        warnings: parsed.warnings,
        failures,
        more_failures,
        failures_omitted,
        // Only when every pass is named, whole, so a list is never a part
        // of them and no name is cut to look like another.
        passed_names: if parsed.passed as usize == parsed.passed_names.len()
            && parsed.passed_names.len() <= PASSES_NAMED
            && parsed
                .passed_names
                .iter()
                .all(|n| n.chars().count() <= NAME_MAX)
        {
            parsed.passed_names.clone()
        } else {
            Vec::new()
        },
        no_tests,
    }
}

/// The output's lines without colour, with only what a carriage return left
/// on screen, and without the `[task] ` prefix mise puts on each line of a
/// task when most lines have one, as many deep as tasks were nested: a task
/// that runs `mise run` prefixes its children's lines again
/// (`[test:bats] [test:bats:libgit2] not ok 1`).
fn clean(text: &str) -> Vec<String> {
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let line = ANSI.replace_all(line, "");
            let shown = line.rsplit('\r').find(|s| !s.is_empty()).unwrap_or("");
            shown.trim_end().to_string()
        })
        .collect();
    let nonblank = lines.iter().filter(|l| !l.is_empty()).count();
    let prefixed = lines.iter().filter(|l| TASK_PREFIX.is_match(l)).count();
    if prefixed * 2 <= nonblank {
        return lines;
    }
    lines
        .into_iter()
        .map(|mut l| {
            while let Some(m) = TASK_PREFIX.find(&l) {
                l.replace_range(..m.end(), "");
            }
            l
        })
        .collect()
}

/// The lines from `from` up to the first one `end` matches.
fn until(lines: &[String], from: usize, end: &Regex) -> Vec<String> {
    lines[from.min(lines.len())..]
        .iter()
        .take_while(|l| !end.is_match(l))
        .cloned()
        .collect()
}

fn libtest(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = LIBTEST_OK.captures(line) {
            out.passed_names.push(c[1].to_string());
        } else if let Some(c) = LIBTEST_FAILED.captures(line) {
            out.push(i, &c[1], Vec::new());
        } else if let Some(c) = LIBTEST_BLOCK.captures(line) {
            out.push(i, &c[1], until(lines, i + 1, &LIBTEST_END));
        } else if let Some(c) = LIBTEST_RESULT.captures(line) {
            let n = |k: usize| c[k].parse().unwrap_or(0);
            out.count(n(1), n(2), n(3));
        }
    }
}

fn nextest(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = NEXTEST_FAIL.captures(line) {
            out.push(i, c[1].trim(), until(lines, i + 1, &NEXTEST_NEXT));
        } else if let Some(c) = NEXTEST_SUMMARY.captures(line) {
            out.count_words(&c[1]);
        }
    }
}

/// What a failed build script said: how it exited, then its stderr, which
/// holds the error; its stdout is `cargo:` directives and the environment
/// it read, shown only when it wrote nothing to stderr.
fn build_script(after: &[String]) -> Vec<String> {
    let block: Vec<&String> = after
        .iter()
        .take_while(|l| l.is_empty() || l.starts_with(char::is_whitespace) || *l == "Caused by:")
        .filter(|l| *l != "Caused by:")
        .collect();
    let section = |name: &str| block.iter().position(|l| l.trim() == name);
    let exited = block
        .iter()
        .take_while(|l| l.trim() != "--- stdout")
        .filter(|l| !l.trim().is_empty());
    let stderr: Vec<&&String> = match section("--- stderr") {
        Some(at) => block[at + 1..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .collect(),
        None => Vec::new(),
    };
    let rest: Vec<&&String> = if stderr.is_empty() {
        block
            .iter()
            .filter(|l| !l.trim_start().starts_with("cargo:"))
            .collect()
    } else {
        exited.chain(stderr).collect()
    };
    rest.into_iter()
        .map(|l| l.trim_start().to_string())
        .collect()
}

/// rustc's errors with a location or a code, and its warnings, counted.
fn rustc(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        let located = |from: usize| {
            lines
                .iter()
                .skip(from)
                .take(3)
                .find_map(|l| RUSTC_AT.captures(l).map(|c| c[1].to_string()))
        };
        if RUSTC_WARNING.is_match(line) && located(i + 1).is_some() {
            out.warnings += 1;
            continue;
        }
        if let Some(c) = CARGO_BUILD_SCRIPT.captures(line) {
            out.push(
                i,
                format!("build script {}", &c[1]),
                build_script(&lines[i + 1..]),
            );
            continue;
        }
        let Some(c) = RUSTC_ERROR.captures(line) else {
            continue;
        };
        let at = located(i + 1);
        if at.is_none() && c[1].len() == "error".len() {
            continue;
        }
        let block: Vec<String> = lines[i + 1..]
            .iter()
            .take_while(|l| !l.is_empty())
            .filter(|l| !RUSTC_AT.is_match(l))
            .cloned()
            .collect();
        let name = match at {
            Some(at) => format!("{at}: {line}"),
            None => line.clone(),
        };
        out.push(i, name, block);
    }
}

/// Where rustfmt would change a file, with the diff it prints.
fn rustfmt(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        let Some(c) = RUSTFMT_DIFF.captures(line) else {
            continue;
        };
        let at = c.get(2).or(c.get(3)).map_or("", |m| m.as_str());
        let block = lines[i + 1..]
            .iter()
            .take_while(|l| l.starts_with([' ', '+', '-']) || l.is_empty())
            .cloned()
            .collect();
        out.push(i, format!("rustfmt {}:{at}", &c[1]), block);
    }
}

fn tsc(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        let Some(c) = TSC.captures(line) else {
            continue;
        };
        let (row, col) = match (c.get(2), c.get(4)) {
            (Some(row), _) => (row.as_str(), &c[3]),
            (_, Some(row)) => (row.as_str(), &c[5]),
            _ => continue,
        };
        // The pretty form follows with a blank line and a code frame.
        let frame = if lines.get(i + 1).is_some_and(String::is_empty) {
            lines[i + 2..]
                .iter()
                .take_while(|l| !l.is_empty())
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        out.push(
            i,
            format!("{}:{row}:{col}: error {}: {}", &c[1], &c[6], &c[7]),
            frame,
        );
    }
}

fn go(lines: &[String], out: &mut Parsed) {
    // Where the current package's output starts, and how many failures had
    // been found then: a package that fails with none of its own (TestMain
    // giving up, a test binary that panicked before any test) is named itself.
    let mut package = (0, out.found.len());
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if let Some(c) = GO_PACKAGE.captures(line) {
            if &c[1] == "FAIL" && out.found.len() == package.1 {
                let block: Vec<String> = lines[package.0..i]
                    .iter()
                    .filter(|l| !l.is_empty() && *l != "FAIL")
                    .cloned()
                    .collect();
                out.count(0, 1, 0);
                out.push(i, &c[2], block);
            }
            package = (i + 1, out.found.len());
        } else if let Some(c) = GO_RESULT.captures(line) {
            // Top-level tests only: a subtest is part of its parent.
            if c[1].is_empty() {
                match &c[2] {
                    "PASS" => out.count(1, 0, 0),
                    "FAIL" => out.count(0, 1, 0),
                    _ => out.count(0, 0, 1),
                }
            }
            if &c[2] == "FAIL" {
                out.push(i, &c[3], go_output(lines, i, &c[3], c[1].len()));
            }
        } else if let Some(c) = GO_BUILD.captures(line)
            && lines.get(i + 1).is_some_and(|l| GO_FILE.is_match(l))
        {
            let block: Vec<String> = lines[i + 1..]
                .iter()
                .take_while(|l| GO_FILE.is_match(l) || l.starts_with(char::is_whitespace))
                .cloned()
                .collect();
            let n = block.len();
            out.push(i, format!("build {}", &c[1]), block);
            i += n;
        } else if GO_PANIC.is_match(line) {
            let block = until(lines, i + 1, &GO_END);
            let n = block.len();
            out.push(i, line.clone(), block);
            i += n;
        }
        i += 1;
    }
}

/// What a failed Go test logged: the lines indented under its `--- FAIL`,
/// or with `-v`, which prints them as they come, those after its `=== RUN`.
fn go_output(lines: &[String], at: usize, name: &str, indent: usize) -> Vec<String> {
    let under: Vec<String> = lines[at + 1..]
        .iter()
        .take_while(|l| l.len() - l.trim_start().len() > indent)
        .map(|l| l.trim_start().to_string())
        .collect();
    if !under.is_empty() {
        return under;
    }
    let started = lines[..at]
        .iter()
        .rposition(|l| GO_RUN.captures(l).is_some_and(|c| &c[2] == name));
    let Some(started) = started else {
        return under;
    };
    lines[started + 1..at]
        .iter()
        .take_while(|l| !GO_RUN.is_match(l) && !GO_RESULT.is_match(l))
        .map(|l| l.trim_start().to_string())
        .collect()
}

fn pytest(lines: &[String], out: &mut Parsed) {
    let mut in_section = false;
    for (i, line) in lines.iter().enumerate() {
        if PYTEST_SECTION.is_match(line) {
            in_section = true;
        } else if let Some(c) = PYTEST_SUMMARY.captures(line) {
            out.count_words(&c[1]);
        } else if PYTEST_RULE.is_match(line) {
            in_section = false;
        } else if in_section && let Some(c) = PYTEST_HEAD.captures(line) {
            let block = lines[i + 1..]
                .iter()
                .take_while(|l| !PYTEST_HEAD.is_match(l) && !PYTEST_RULE.is_match(l))
                .cloned()
                .collect();
            out.push(i, &c[1], block);
        } else if let Some(c) = PYTEST_SHORT.captures(line) {
            let why = c.get(2).map(|m| vec![m.as_str().to_string()]);
            out.push(i, &c[1], why.unwrap_or_default());
        }
    }
}

fn unittest(lines: &[String], out: &mut Parsed) {
    let mut ran = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = UNITTEST_HEAD.captures(line)
            && i > 0
            && UNITTEST_RULE.is_match(&lines[i - 1])
            && lines.get(i + 1).is_some_and(|l| UNITTEST_DASH.is_match(l))
        {
            let block = lines[i + 2..]
                .iter()
                .take_while(|l| !UNITTEST_RULE.is_match(l) && !UNITTEST_DASH.is_match(l))
                .cloned()
                .collect();
            out.push(i, &c[1], block);
        } else if let Some(c) = UNITTEST_RAN.captures(line) {
            ran = c[1].parse::<u64>().ok();
        } else if let Some(total) = ran
            && let Some(c) = UNITTEST_DONE.captures(line)
        {
            let (mut failed, mut skipped) = (0, 0);
            for kind in UNITTEST_KIND.captures_iter(c.get(1).map_or("", |m| m.as_str())) {
                let n: u64 = kind[2].parse().unwrap_or(0);
                match &kind[1] {
                    "failures" | "errors" | "unexpected successes" => failed += n,
                    _ => skipped += n,
                }
            }
            out.count(total.saturating_sub(failed + skipped), failed, skipped);
            ran = None;
        }
    }
}

fn jest(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = JEST_HEAD.captures(line) {
            out.push(i, &c[1], until(lines, i + 1, &JEST_END));
        } else if let Some(c) = JEST_TESTS.captures(line) {
            out.count_words(&c[1]);
        }
    }
}

fn vitest(lines: &[String], out: &mut Parsed) {
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = VITEST_HEAD.captures(line) {
            out.push(i, &c[1], until(lines, i + 1, &VITEST_END));
        } else if let Some(c) = VITEST_TESTS.captures(line) {
            out.count_words(&c[1]);
        }
    }
}

fn tap(lines: &[String], out: &mut Parsed) {
    if !lines.iter().any(|l| TAP_PLAN.is_match(l)) {
        return;
    }
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for (i, line) in lines.iter().enumerate() {
        if TAP_SKIP.is_match(line) {
            skipped += 1;
        } else if TAP_OK.is_match(line) {
            passed += 1;
            if let Some(c) = TAP_OK_NAME.captures(line) {
                out.passed_names.push(c[1].to_string());
            }
        } else if let Some(c) = TAP_NOT_OK.captures(line) {
            failed += 1;
            let block = lines[i + 1..]
                .iter()
                .take_while(|l| l.starts_with('#'))
                .map(|l| l.strip_prefix("# ").unwrap_or(&l[1..]).to_string())
                .collect();
            out.push(i, &c[1], block);
        }
    }
    out.count(passed, failed, skipped);
}

/// The failures in the order the output first names them, each once: a
/// name repeated (libtest's `test x ... FAILED` and its `---- x stdout`
/// block; pytest's section and its short summary) keeps the longer name and
/// the longer text. A Go test whose subtests failed gives way to them.
fn merge(mut found: Vec<Found>) -> Vec<Found> {
    found.sort_by_key(|f| f.at);
    let mut out: Vec<Found> = Vec::new();
    for f in found {
        match out.iter_mut().find(|o| same(&o.name, &f.name)) {
            Some(o) => {
                if f.name.len() > o.name.len() {
                    o.name = f.name;
                }
                if f.lines.len() > o.lines.len() {
                    o.lines = f.lines;
                }
            }
            None => out.push(f),
        }
    }
    let names: Vec<String> = out.iter().map(|f| f.name.clone()).collect();
    out.retain(|f| {
        let parent = format!("{}/", f.name);
        !names.iter().any(|n| n.starts_with(&parent))
    });
    out
}

/// Whether two names are one test: equal, or one is the other's last part,
/// as pytest's `TestA.test_b` is `tests/t.py::TestA::test_b` and its
/// `ERROR collecting t.py` is `t.py`.
fn same(a: &str, b: &str) -> bool {
    let tail = |long: &str, short: &str| {
        long.ends_with(&format!("::{}", short.replace('.', "::")))
            || long.ends_with(&format!(" {short}"))
    };
    a == b || tail(a, b) || tail(b, a)
}

/// A failure's lines as shown: without lines that say nothing (libtest's
/// own inside nextest's output, a section header with nothing under it),
/// with runs of blank lines made one, and a long one cut to its first and
/// last lines.
fn shape(name: &str, lines: Vec<String>) -> Vec<String> {
    let own = name.rsplit(' ').next().unwrap_or(name);
    let lines: Vec<String> = jouet_wait::fold_backtraces(lines)
        .into_iter()
        .filter(|l| !RUST_NOISE.is_match(l) && l.trim() != own)
        .map(cap)
        .collect();
    let blank = |l: &String| l.trim().is_empty();
    let mut out: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if SECTION.is_match(line)
            && lines[i + 1..]
                .iter()
                .find(|l| !blank(l))
                .is_none_or(|next| SECTION.is_match(next))
        {
            continue;
        }
        if blank(line) && out.last().is_none_or(blank) {
            continue;
        }
        out.push(line.clone());
    }
    if out.last().is_some_and(blank) {
        out.pop();
    }
    if out.len() <= FAILURE_HEAD + FAILURE_TAIL + 1 {
        return out;
    }
    let tail = out.len() - FAILURE_TAIL;
    let mut middle = FAILURE_MIDDLE;
    let mut shown = Vec::new();
    let mut skipped = 0;
    for (i, line) in out.into_iter().enumerate() {
        let keep = i < FAILURE_HEAD
            || i >= tail
            || (middle > 0 && TELLING.is_match(&line) && {
                middle -= 1;
                true
            });
        if !keep {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            shown.push(skipped_marker(skipped));
            skipped = 0;
        }
        shown.push(line);
    }
    shown
}

/// A line cut to [`LINE_MAX`] bytes, saying how much it left out, so one
/// long assertion message cannot take the whole budget.
fn cap(line: String) -> String {
    if line.len() <= LINE_MAX {
        return line;
    }
    let mut end = LINE_MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} ... {} more bytes", &line[..end], line.len() - end)
}

fn skipped_marker(n: usize) -> String {
    if n == 1 {
        "... 1 line skipped".into()
    } else {
        format!("... {n} lines skipped")
    }
}

/// The failures shown in full within `budget` bytes, then the names of the
/// rest that fit, then how many are left. The first failure is always
/// shown, cut to the budget if it must be. Once one does not fit, the rest
/// are names only, so the order holds.
fn fit(found: Vec<Found>, budget: usize) -> (Vec<Failure>, Vec<String>, u64) {
    let mut shown: Vec<Failure> = Vec::new();
    let mut more = Vec::new();
    let mut omitted = 0;
    let mut used = 0;
    for f in found {
        let lines = shape(&f.name, f.lines);
        // As printed: `FAIL name`, then each line indented by two.
        let cost = f.name.len() + 6 + lines.iter().map(|l| l.len() + 3).sum::<usize>();
        if more.is_empty() && omitted == 0 && (budget == 0 || used + cost <= budget) {
            used += cost;
            shown.push(Failure {
                name: f.name,
                lines,
            });
        } else if shown.is_empty() {
            let mut room = budget.saturating_sub(f.name.len() + 6);
            let kept: Vec<String> = lines
                .iter()
                .take_while(|l| {
                    let fits = l.len() + 3 <= room;
                    room = room.saturating_sub(l.len() + 3);
                    fits
                })
                .cloned()
                .collect();
            let mut kept = kept;
            let cut = lines.len() - kept.len();
            if cut > 0 {
                kept.push(skipped_marker(cut));
            }
            used = budget;
            shown.push(Failure {
                name: f.name,
                lines: kept,
            });
        } else if omitted == 0 && used + f.name.len() + 2 <= budget.saturating_add(NAMES_SLACK) {
            // Names cost little and say which tests to look up, so they may
            // go a little past the budget.
            used += f.name.len() + 2;
            more.push(f.name);
        } else {
            omitted += 1;
        }
    }
    (shown, more, omitted)
}

#[cfg(test)]
mod tests;
