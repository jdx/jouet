use usage_rs::{Args, Subcommands};

/// Run GitHub Actions jobs here, or say whether a failure on GitHub is flaky
#[derive(Args)]
pub(crate) struct CiArgs {
    #[usage(subcommand)]
    pub(crate) cmd: CiCmd,
}

#[derive(Subcommands)]
pub(crate) enum CiCmd {
    /// Run a workflow's jobs here with jactionlint, and show why any failed
    Run(CiRunArgs),
    /// Say whether each failed job is flaky, infrastructure or real, and
    /// rerun only a flake
    Explain(CiExplainArgs),
}

/// Say whether each failed job is flaky, infrastructure or real, and rerun
/// only a flake
///
/// For each failed GitHub Actions job of a PR's latest runs, a run or one
/// job: the verdict, the evidence for it, the failed step and an excerpt of
/// its log. The log is read with fixed patterns for failed tests (cargo,
/// nextest, bats, pytest, vitest, jest, go test), compile and lint errors,
/// and runner or network trouble; history comes from the workflow's recent
/// runs. flaky: it passed on the same commit elsewhere (another attempt, or
/// another job on the same OS), or the same test failed on another branch
/// that passed later. infrastructure: the runner, disk,
/// network, a registry, a rate limit, a service container or the cache.
/// real: a compile or lint error, a test that failed on every attempt on
/// this commit, one that passed on this commit only on other OSes (most
/// likely an OS difference), or one that fails the same way on the default
/// branch, which the evidence says is not the change's fault. Anything else is
/// unknown. A job that failed or was cancelled only because another did is
/// listed as following it. --rerun reruns the failed jobs when every one is
/// flaky or infrastructure, and refuses (exit 4) otherwise. A second failure
/// on the same commit turns a flake real. Exits 0 when nothing failed or
/// every failure is flaky or infrastructure, 1 when one is real or unknown,
/// 3 when the PR, run or job does not exist, 4 when nothing has failed yet
/// in a run still going. What it fetches is cached under
/// the state dir, so explaining the same run again is cheap.
#[derive(Args)]
pub(crate) struct CiExplainArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch, or an Actions run or
    /// job URL. Defaults to the current branch's PR
    pub(crate) target: Option<String>,
    /// The repository for a bare number, branch, --run or --job. Defaults
    /// to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Option<String>,
    /// Explain this Actions run instead of a PR
    #[usage(long, value_name = "ID")]
    pub(crate) run: Option<u64>,
    /// Explain this one Actions job
    #[usage(long, value_name = "ID")]
    pub(crate) job: Option<u64>,
    /// Rerun the failed jobs if every one is flaky or infrastructure
    #[usage(long)]
    pub(crate) rerun: bool,
    /// Lines of each failure log to show (default 40; 0 shows the whole
    /// failed step)
    #[usage(long, value_name = "N")]
    pub(crate) log_lines: Option<usize>,
}

/// Run a workflow's jobs here with jactionlint, and show why any failed
///
/// Runs `jactionlint run`, which needs no container engine: each job runs in
/// a copy of the checkout, in the OS sandbox, with a throwaway home and no
/// secrets unless given. Prints one line for the run, then each failed step
/// with the end of its output (cut like `wait ci`'s log excerpts) and the
/// jobs that were skipped and why; the whole transcript is kept in a log.
/// WORKFLOW is a file of .github/workflows (the extension is optional), a
/// path, or a workflow's `name:`. --check takes a check's name as `wait ci`
/// prints it and runs the job behind it. Arguments after `--` go to
/// `jactionlint run` as they are, such as `--secret NAME=VALUE`.
/// `--secrets-from fnox` resolves the `secrets.*` the jobs read through
/// fnox and passes them in a temporary file, never on the command line. It shows
/// what happens on this machine, not whether GitHub will pass: jobs for
/// another OS are skipped. Exits 1 when a job failed, 3 when no workflow or
/// job matches, 4 when jactionlint is missing or the run could not start.
#[derive(Args)]
pub(crate) struct CiRunArgs {
    /// The workflow. Needed when the repository has several and no --check
    /// is given
    pub(crate) workflow: Option<String>,
    /// Run only this job and the jobs it needs. Repeatable
    #[usage(short = 'j', long, value_name = "ID")]
    pub(crate) job: Vec<String>,
    /// Run the job behind this check name, such as `unit (ubuntu-latest)`
    #[usage(long, value_name = "NAME")]
    pub(crate) check: Option<String>,
    /// The event that triggers the workflow (default push)
    #[usage(short = 'e', long, value_name = "EVENT")]
    pub(crate) event: Option<String>,
    /// Lines of each failed step to show (default 60; 0 shows all of it)
    #[usage(long, value_name = "N")]
    pub(crate) log_lines: Option<usize>,
    /// Give the jobs the secrets they read, resolved through this tool
    #[usage(long, value_name = "SOURCE", choices("fnox"))]
    pub(crate) secrets_from: Option<String>,
    /// Arguments for `jactionlint run`, after `--`
    #[usage(double_dash = "required", value_name = "ARGS")]
    pub(crate) args: Vec<String>,
}
