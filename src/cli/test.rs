use usage_rs::Args;

/// Run the test suite, or only the tests a change affects
///
/// Without --affected, runs the repository's test task as the full suite: a
/// mise `test` task, a package.json `test` script, Cargo.toml (`cargo test
/// --workspace`), a justfile or Makefile `test` target or go.mod, else the
/// whole suite of each test runner found. With --affected, runs only the
/// tests that the changes since the merge base with --since (default: the
/// upstream, else the default branch), uncommitted and untracked files
/// included, affect. Adapters delegate to each runner's own selection:
/// vitest's and jest's module graphs, pytest-testmon when it is installed,
/// else a static import graph. The whole suite runs instead when a
/// lockfile, tool versions, CI, the test configuration, a conftest or setup
/// file, or a file no adapter can trace changed; nothing runs for docs
/// only. Each test is listed with the chain of imports that picked it, and
/// the command. Affected runs count as partial; they do not verify the full
/// suite. Words after `--` go to the runner.
///
/// With --cmd, the words after `--` are the whole command instead, run
/// directly rather than through a shell: one test, an e2e suite or a build,
/// such as `jouet test --cmd -- cargo build`.
///
/// Every run's output, stdout and stderr together, goes to a log under
/// jouet's log directory, kept for a week. For an agent (and in plain display), that is all
/// it goes to: a passing run prints one line with the counts and the log's
/// path, and a failing one each failure's name and the lines that explain it
/// (the assertion, the panic, the compiler error), up to --budget bytes,
/// then the names of the failures that did not fit. Read more from the log
/// rather than running it again. For a person at a terminal, the output
/// streams as it runs. Failures are recognised in the output of cargo test
/// and nextest, rustc, tsc, go test and go build, pytest, unittest, jest,
/// vitest and TAP (bats), and in the formats declared in a formats/
/// directory (your config's, a trusted repository's .jouet/, a plugin's)
/// for other runners; for any other output, a failing run shows the end of
/// it.
///
/// With --baseline, a failing run is run again in a temporary worktree of
/// the merge base with the upstream or default branch (or --since REF). If
/// every test that failed also fails there, the failures were there before
/// the change, from the environment or an earlier commit, and the run passes,
/// naming them; any new failure still fails it. It needs the output to name
/// its failures, and costs one more run of the failed suites. What the merge
/// base did is kept, so a later failure of the same command compares with it
/// without running it again. Without --baseline, a failing full suite is
/// compared on its own, and so is a failing --cmd run that took under a
/// minute; a slower one uses what is kept for it, or names the --baseline
/// command that compares it. `[test] baseline = "off"` (JOUET_TEST_BASELINE=off) turns that off.
///
/// Exits 1 when tests fail, 2 for a usage error (the runner's too), 4 when
/// no runner is found or installed.
#[derive(Args)]
pub(crate) struct TestArgs {
    /// Run only the tests the changes affect
    #[usage(long)]
    pub(crate) affected: bool,
    /// Measure changes from the merge base with REF (default: the upstream,
    /// else the default branch)
    #[usage(long, value_name = "REF")]
    pub(crate) since: Option<String>,
    /// Use only this adapter: vitest, jest, pytest or graph (Go, Ruby and Java, from the code graph)
    #[usage(long, value_name = "NAME")]
    pub(crate) adapter: Option<String>,
    /// Print what would run and why, running nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
    /// Run the words after `--` as the command, instead of the test task
    #[usage(long)]
    pub(crate) cmd: bool,
    /// When the run fails, run it again at the merge base (with --since,
    /// of REF) and pass it if every failure also fails there
    #[usage(long)]
    pub(crate) baseline: bool,
    /// Bytes of failure text to show (default 4000; 0 shows every failure
    /// whole)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// Arguments for the test runner, after `--`; with --cmd, the command
    pub(crate) extra: Vec<String>,
}
