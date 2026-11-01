//! What a failed step's output says went wrong, read with fixed patterns:
//! the tests that failed (cargo and nextest, bats, pytest, vitest and jest,
//! go test), compile and lint errors, and runner or network trouble. Two
//! failures with a test or an error in common are the same failure, which
//! is how history is compared.

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid regex")
}

/// A task runner's line prefix, such as mise's `[test:cargo] `, and hk's
/// `  cargo-clippy – `.
static PREFIX: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:\[[^\]\s]+\] |\s*[\w.-]+ – )"));

/// A failed test, in the framework's own words: `(framework, name)`.
static FAILED_TESTS: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
    vec![
        ("cargo", re(r"^test (\S+) \.\.\. FAILED$")),
        ("nextest", re(r"^\s*FAIL \[\s*[\d.]+s\] (?:\S+ )?(\S+)$")),
        ("bats", re(r"^not ok \d+ (.+?)(?: # .*)?$")),
        ("pytest", re(r"^FAILED (\S+::\S+)")),
        ("pytest", re(r"^(\S+::\S+) FAILED")),
        ("go", re(r"^\s*--- FAIL: (\S+) \(")),
        ("jest", re(r"^\s*● (.+ › .+)$")),
        (
            "vitest",
            re(r"^\s*(?:FAIL|×)\s+(\S+\.(?:test|spec)\.\w+ > .+?)(?:\s+\d+ms)?$"),
        ),
    ]
});

/// Where a test failed: a Rust panic or a Go test's `file_test.go:12:`.
static LOCATION: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(r"panicked at (\S+:\d+:\d+)"),
        re(r"^\s+(\S+_test\.go:\d+): "),
    ]
});

/// Compile and lint errors, which fail the same way every time. The
/// message is kept without its location, so a moved line still matches.
static ERRORS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        re(r"(error\[E\d{4}\]: .+)"),
        re(r"\S+\.tsx?(?:\(\d+,\d+\):|:\d+:\d+ -) (error TS\d+: .+)"),
        re(r"^\S+\.go:\d+:\d+: (.+)$"),
        re(r"^(Diff in \S+) at line \d+:"),
    ]
});

/// A rustc or clippy `error: ` that is a diagnostic of its own, not cargo
/// reporting that something else failed.
static RUST_ERROR: LazyLock<Regex> = LazyLock::new(|| re(r"^(error: .+)$"));
static RUST_WRAPPER: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^error: (?:could not compile|test failed|process didn't exit|aborting due to|Recipe|build failed|\d+ (?:tests? )?failed|failed to run custom build)",
    )
});
/// The location line rustc prints under a diagnostic.
static ARROW: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*--> \S+:\d+:\d+"));

/// Runner and network trouble: `(cause, pattern, strong)`. A strong cause
/// explains the failure even when tests failed too, since a runner that
/// ran out of disk fails whatever runs on it; a weak one only when nothing
/// else in the output does, since a test can print a connection error.
static INFRA: LazyLock<Vec<(&str, Regex, bool)>> = LazyLock::new(|| {
    let i = |p: &str| re(&format!("(?i){p}"));
    vec![
        (
            "runner",
            i(
                r"the runner has received a shutdown signal|lost communication with the server|the hosted runner encountered an error|the runner has run out of memory",
            ),
            true,
        ),
        (
            "disk",
            i(r"no space left on device|\bENOSPC\b|not enough space on the disk"),
            true,
        ),
        (
            "rate_limit",
            i(
                r"api rate limit exceeded|secondary rate limit|429 too many requests|toomanyrequests",
            ),
            false,
        ),
        (
            "network",
            i(
                r"could not resolve host|temporary failure in name resolution|getaddrinfo (?:ENOTFOUND|EAI_AGAIN)|\bETIMEDOUT\b|\bECONNRESET\b|connection timed out|connection reset by peer|tls handshake timeout|i/o timeout|network is unreachable|failed to connect to \S+ port",
            ),
            false,
        ),
        (
            "registry",
            i(
                r"\b50[234] (?:bad gateway|service unavailable|gateway time-?out)|http status server error|error sending request for url|spurious network error",
            ),
            false,
        ),
        (
            "service",
            i(
                r"failed to initialize container|service container \S+ failed|is unhealthy|error response from daemon",
            ),
            false,
        ),
        (
            "cache",
            i(
                r"failed to restore cache|cache service responded with 5\d\d|unable to reserve cache",
            ),
            false,
        ),
    ]
});

/// The steps the runner itself runs, whose failure is never the code's.
const RUNNER_STEPS: &[&str] = &["Set up job", "Initialize containers", "Complete job"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Test {
    pub(crate) framework: &'static str,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Infra {
    /// `runner`, `disk`, `rate_limit`, `network`, `registry`, `service` or
    /// `cache`.
    pub(crate) cause: &'static str,
    /// The line that says so.
    pub(crate) line: String,
    #[serde(skip)]
    pub(crate) strong: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Signature {
    pub(crate) tests: Vec<Test>,
    pub(crate) location: Option<String>,
    pub(crate) errors: Vec<String>,
    pub(crate) infra: Option<Infra>,
}

impl Signature {
    /// Reads a failed step's lines (without timestamps or escapes).
    pub(crate) fn of(step: Option<&str>, lines: &[String]) -> Self {
        let mut sig = Self::default();
        let lines: Vec<&str> = lines.iter().map(|l| strip(l)).collect();
        for (i, line) in lines.iter().enumerate() {
            for (framework, pattern) in FAILED_TESTS.iter() {
                if let Some(c) = pattern.captures(line) {
                    let test = Test {
                        framework,
                        name: c[1].trim().to_string(),
                    };
                    if !sig.tests.contains(&test) {
                        sig.tests.push(test);
                    }
                }
            }
            if sig.location.is_none() {
                sig.location = LOCATION
                    .iter()
                    .find_map(|p| p.captures(line).map(|c| c[1].to_string()));
            }
            let error = ERRORS
                .iter()
                .find_map(|p| p.captures(line).map(|c| c[1].trim().to_string()))
                .or_else(|| {
                    let c = RUST_ERROR.captures(line)?;
                    let located = lines[i + 1..].iter().take(3).any(|l| ARROW.is_match(l));
                    (located && !RUST_WRAPPER.is_match(line)).then(|| c[1].to_string())
                });
            if let Some(error) = error
                && !sig.errors.contains(&error)
            {
                sig.errors.push(error);
            }
            if sig.infra.as_ref().is_none_or(|i| !i.strong) {
                let found = INFRA
                    .iter()
                    .find(|(_, p, strong)| p.is_match(line) && (*strong || sig.infra.is_none()));
                if let Some((cause, _, strong)) = found {
                    sig.infra = Some(Infra {
                        cause,
                        line: line.trim().to_string(),
                        strong: *strong,
                    });
                }
            }
        }
        if sig.infra.is_none()
            && let Some(step) = step.filter(|s| RUNNER_STEPS.contains(s))
        {
            sig.infra = Some(Infra {
                cause: "runner",
                line: format!("the runner's own step \"{step}\" failed"),
                strong: true,
            });
        }
        sig
    }

    /// Whether the output says nothing this module recognises.
    pub(crate) fn is_empty(&self) -> bool {
        self.tests.is_empty() && self.errors.is_empty() && self.infra.is_none()
    }

    /// The infrastructure cause that explains the failure: a strong one
    /// always, a weak one when no test or error does.
    pub(crate) fn infrastructure(&self) -> Option<&Infra> {
        self.infra
            .as_ref()
            .filter(|i| i.strong || (self.tests.is_empty() && self.errors.is_empty()))
    }

    /// Whether two failures have a failed test or an error in common.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.tests
            .iter()
            .any(|t| other.tests.iter().any(|o| o.name == t.name))
            || self.errors.iter().any(|e| other.errors.contains(e))
    }

    /// The key two jobs with the same failure share, so they are reported
    /// together and judged once; `None` for a failure nothing recognised.
    /// A failed test's verdict depends on the OS the job ran on (a pass on
    /// another OS is not a pass on its own), so jobs on different OSes that
    /// failed only tests are kept apart.
    pub(crate) fn group_key(&self, os: Option<&str>) -> Option<String> {
        if let Some(infra) = self.infrastructure() {
            return Some(format!("infra:{}", infra.cause));
        }
        let mut parts: Vec<&str> = self.errors.iter().map(String::as_str).collect();
        parts.extend(self.tests.iter().map(|t| t.name.as_str()));
        if self.errors.is_empty() && !self.tests.is_empty() {
            parts.push(os.unwrap_or("unknown os"));
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }
}

/// A line without a task runner's prefix.
fn strip(line: &str) -> &str {
    PREFIX.find(line).map_or(line, |m| &line[m.end()..])
}

/// Whether a log shows `test` passing, in any framework's words.
pub(crate) fn passed(lines: &[String], test: &Test) -> bool {
    let name = test.name.as_str();
    // jest prints a passing test by its own title, without its suites.
    let title = name.rsplit(" › ").next().unwrap_or(name);
    lines.iter().map(|l| strip(l).trim()).any(|line| {
        line == format!("test {name} ... ok")
            || (line.starts_with("PASS [") && line.ends_with(&format!(" {name}")))
            || line
                .strip_prefix("ok ")
                .and_then(|rest| rest.split_once(' '))
                .is_some_and(|(n, rest)| n.bytes().all(|b| b.is_ascii_digit()) && rest == name)
            || line == format!("{name} PASSED")
            || line.starts_with(&format!("PASSED {name}"))
            || line.starts_with(&format!("--- PASS: {name} ("))
            || line
                .strip_prefix("✓ ")
                .is_some_and(|rest| rest == title || rest.starts_with(&format!("{title} (")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn names(sig: &Signature) -> Vec<(&str, &str)> {
        sig.tests
            .iter()
            .map(|t| (t.framework, t.name.as_str()))
            .collect()
    }

    #[test]
    fn a_failed_test_groups_by_os_and_a_compile_error_does_not() {
        let failed = Signature::of(
            Some("Run mise run test"),
            &lines("[test:cargo] test a ... FAILED"),
        );
        assert_eq!(
            failed.group_key(Some("linux")),
            failed.group_key(Some("linux"))
        );
        assert_ne!(
            failed.group_key(Some("linux")),
            failed.group_key(Some("macos"))
        );
        let error = Signature {
            errors: vec!["error[E0425]: x".into()],
            ..failed
        };
        assert_eq!(
            error.group_key(Some("linux")),
            error.group_key(Some("macos"))
        );
    }

    #[test]
    fn cargo_tests_behind_a_task_prefix() {
        let sig = Signature::of(
            Some("Run mise run test"),
            &lines(
                "[test:cargo] test a_pid_is_met ... ok
[test:cargo] test a_command_is_rerun_until_it_exits_as_expected ... FAILED
[test:cargo] thread 'a_command_is_rerun_until_it_exits_as_expected' (47818) panicked at tests/wait.rs:436:5:
[test:cargo] error: test failed, to rerun pass `-p jouet --test wait`",
            ),
        );
        assert_eq!(
            names(&sig),
            [("cargo", "a_command_is_rerun_until_it_exits_as_expected")]
        );
        assert_eq!(sig.location.as_deref(), Some("tests/wait.rs:436:5"));
        assert!(sig.errors.is_empty(), "{:?}", sig.errors);
    }

    #[test]
    fn the_other_frameworks() {
        let sig = Signature::of(
            None,
            &lines(
                "        FAIL [   0.012s] jouet-wait checks::tests::globs
not ok 98 wait job is met when the job exits 0 and fails with its log otherwise
not ok 3 a skipped one # skip no network
FAILED tests/test_a.py::test_dates - AssertionError: 1 != 2
--- FAIL: TestWait (0.01s)
    wait_test.go:12: got 1
  ● parser › reads dates
 FAIL  src/a.test.ts > dates > parses 12ms",
            ),
        );
        assert_eq!(
            names(&sig),
            [
                ("nextest", "checks::tests::globs"),
                (
                    "bats",
                    "wait job is met when the job exits 0 and fails with its log otherwise"
                ),
                ("bats", "a skipped one"),
                ("pytest", "tests/test_a.py::test_dates"),
                ("go", "TestWait"),
                ("jest", "parser › reads dates"),
                ("vitest", "src/a.test.ts > dates > parses"),
            ]
        );
        assert_eq!(sig.location.as_deref(), Some("wait_test.go:12"));
    }

    #[test]
    fn compile_errors_are_kept_without_their_location() {
        let sig = Signature::of(
            None,
            &lines(
                "  cargo-clippy – error[E0425]: cannot find function `lowercase_first` in this scope
error[E0425]: cannot find function `lowercase_first` in this scope
   --> src/guide.rs:243:17
error: unused variable: `x`
  --> src/a.rs:1:5
error: could not compile `jouet` (bin \"jouet\") due to 1 previous error
src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.
Diff in /w/src/a.rs at line 4:",
            ),
        );
        assert_eq!(
            sig.errors,
            [
                "error[E0425]: cannot find function `lowercase_first` in this scope",
                "error: unused variable: `x`",
                "error TS2322: Type 'string' is not assignable to type 'number'.",
                "Diff in /w/src/a.rs",
            ]
        );
        assert!(sig.infra.is_none());
    }

    #[test]
    fn runner_trouble_explains_a_failure_and_network_trouble_only_alone() {
        let disk = Signature::of(
            None,
            &lines("test a ... FAILED\nwrite /tmp/x: No space left on device"),
        );
        assert_eq!(disk.infrastructure().map(|i| i.cause), Some("disk"));
        let dns = Signature::of(None, &lines("curl: (6) Could not resolve host: x.org"));
        assert_eq!(dns.infrastructure().map(|i| i.cause), Some("network"));
        let test = Signature::of(
            None,
            &lines("test refuses ... FAILED\nconnection reset by peer"),
        );
        assert_eq!(test.infrastructure(), None);
        let setup = Signature::of(Some("Set up job"), &[]);
        assert_eq!(setup.infrastructure().map(|i| i.cause), Some("runner"));
        let limit = Signature::of(
            None,
            &lines("gh: API rate limit exceeded for installation ID 1"),
        );
        assert_eq!(limit.infrastructure().map(|i| i.cause), Some("rate_limit"));
    }

    #[test]
    fn a_passing_test_is_found_in_any_framework() {
        let log = lines(
            "[test:cargo] test a_command_is_rerun_until_it_exits_as_expected ... ok
        PASS [   0.012s] jouet-wait checks::tests::globs
[test:e2e] ok 98 wait job is met
tests/test_a.py::test_dates PASSED
--- PASS: TestWait (0.01s)
    ✓ reads dates (3 ms)",
        );
        let t = |framework, name: &str| Test {
            framework,
            name: name.into(),
        };
        assert!(passed(
            &log,
            &t("cargo", "a_command_is_rerun_until_it_exits_as_expected")
        ));
        assert!(passed(&log, &t("nextest", "checks::tests::globs")));
        assert!(passed(&log, &t("bats", "wait job is met")));
        assert!(passed(&log, &t("pytest", "tests/test_a.py::test_dates")));
        assert!(passed(&log, &t("go", "TestWait")));
        assert!(passed(&log, &t("jest", "parser › reads dates")));
        assert!(!passed(&log, &t("bats", "wait job")));
        assert!(!passed(&log, &t("cargo", "a_pid_is_met")));
    }
}
