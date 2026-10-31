use usage_rs::{Args, Subcommands};

/// Block until a condition holds, then print one summary
///
/// Replaces `sleep N; check; repeat` loops. The first check is immediate;
/// later ones back off from --interval to 60s and speed up again when the
/// state changes. Exits 0 when the condition holds and 124 on timeout, where
/// running the same command again keeps waiting. In human mode a live status
/// line is shown on stderr; otherwise each change of state is one line on
/// stderr, so a backgrounded wait's log stays small.
#[derive(Args)]
pub(crate) struct WaitArgs {
    /// Give up after DUR, such as 90s, 10m or 1h30m (default 30m)
    #[usage(long, global, value_name = "DUR")]
    pub(crate) timeout: Option<String>,
    /// Time between the first checks; backs off to 60s (default 10s)
    #[usage(long, global, value_name = "DUR")]
    pub(crate) interval: Option<String>,
    /// How long a conflict GitHub reports must persist before `wait ci` and
    /// `wait pr` believe it (default 10s). For tests
    #[usage(long, global, hide, value_name = "DUR")]
    pub(crate) confirm_conflict: Option<String>,
    /// Print one JSON event per line, then the result document
    #[usage(long, global)]
    pub(crate) jsonl: bool,
    /// Print no progress and no text result, only the exit code (and JSON
    /// when asked for)
    #[usage(long, global)]
    pub(crate) quiet: bool,
    #[usage(subcommand)]
    pub(crate) cmd: WaitCmd,
}

#[derive(Subcommands)]
pub(crate) enum WaitCmd {
    /// Wait for a PR's checks, and show why any failed
    Ci(WaitCiArgs),
    /// Wait for a GitHub Actions run or a GitLab pipeline
    Run(WaitRunArgs),
    /// Wait for a PR to become mergeable, merged, approved, ready, ...
    Pr(WaitPrArgs),
    /// Wait for a file to exist, contain a match, or be gone
    File(WaitFileArgs),
    /// Wait for a line matching a regex to be appended to a log
    Log(WaitLogArgs),
    /// Wait for a TCP port to accept connections, or to stop
    Port(WaitPortArgs),
    /// Wait for a URL to answer with a status, and optionally a match
    Http(WaitHttpArgs),
    /// Wait for a process to exit
    Pid(WaitPidArgs),
    /// Re-run a command until it exits with the expected code
    Cmd(WaitCmdArgs),
    /// Wait for a `jouet job` to end, or to be ready
    Job(WaitJobArgs),
    /// Wait for a pitchfork daemon to be ready, or to stop
    Daemon(WaitDaemonArgs),
}

/// Wait for a PR's checks, and show why any failed
///
/// Follows the PR head, switching to a new head when one is pushed, unless
/// --sha pins a commit. Returns on the first failed check unless --all.
/// Skipped and neutral checks count as passing. A conflicted PR returns
/// `conflict` (exit 4), since pull_request workflows will not run, once
/// GitHub reports it again 10s later (right after a push it can still answer
/// for the old head); no checks within --appear-timeout returns `no_checks`
/// (exit 1).
/// For each
/// failed Actions job or GitLab job, the failed step's log is excerpted and
/// the full log saved. On GitLab the checks are the head's pipeline jobs and
/// commit statuses, on Forgejo its commit statuses, and on an Entire-hosted
/// trail its check runs and gates. Waits on the same PR share
/// their state, so they poll the forge once.
#[derive(Args)]
pub(crate) struct WaitCiArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the
    /// current branch's PR
    pub(crate) pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git
    /// remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Option<String>,
    /// Wait on this commit's checks instead of following the PR head
    #[usage(long, value_name = "SHA")]
    pub(crate) sha: Option<String>,
    /// Only checks that branch protection requires
    #[usage(long)]
    pub(crate) required: bool,
    /// Only checks whose name matches GLOB (`*` and `?`). Repeatable
    #[usage(long, value_name = "GLOB")]
    pub(crate) check: Vec<String>,
    /// Leave out checks whose name matches GLOB. Repeatable
    #[usage(long, value_name = "GLOB")]
    pub(crate) ignore: Vec<String>,
    /// Wait for every check to finish, even after one fails
    #[usage(long)]
    pub(crate) all: bool,
    /// Lines of each failure log to show (default 60; 0 shows the whole
    /// failed step)
    #[usage(long, value_name = "N")]
    pub(crate) log_lines: Option<usize>,
    /// Do not fetch failure logs
    #[usage(long)]
    pub(crate) no_logs: bool,
    /// How long checks may take to appear on a commit (default 3m)
    #[usage(long, value_name = "DUR")]
    pub(crate) appear_timeout: Option<String>,
}

/// Wait for a GitHub Actions run or a GitLab pipeline
///
/// Returns when the run completes, or on its first failed job, with the
/// failed step's log excerpted as for `wait ci`. Forgejo and Entire have no
/// runs jouet can read, so it exits 4 there.
#[derive(Args)]
pub(crate) struct WaitRunArgs {
    /// The run ID, or its Actions or GitLab pipeline URL
    pub(crate) run: String,
    /// The repository for a bare run ID. Defaults to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Option<String>,
    /// Lines of each failure log to show (default 60; 0 shows the whole
    /// failed step)
    #[usage(long, value_name = "N")]
    pub(crate) log_lines: Option<usize>,
    /// Do not fetch failure logs
    #[usage(long)]
    pub(crate) no_logs: bool,
}

/// Wait for a PR to become mergeable, merged, approved, ready, ...
///
/// --until is one of: mergeable (keeps waiting while GitHub reports
/// UNKNOWN; exit 1 on a conflict), merged (exit 1 if closed unmerged),
/// closed, approved (exit 1 when changes are requested), checks-registered
/// (any check on the head), ready (no `pr status` attention codes and no
/// pending checks; exit 1 on conflict, checks_failing, changes_requested or
/// unresolved_threads), or any-change (the next push, check result, review,
/// thread or state change, printed as one event). A conflict counts once
/// GitHub reports it again 10s later, since right after a push it can still
/// answer for the old head.
#[derive(Args)]
pub(crate) struct WaitPrArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the
    /// current branch's PR
    pub(crate) pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git
    /// remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Option<String>,
    /// The condition to wait for
    #[usage(long, value_name = "CONDITION")]
    pub(crate) until: String,
}

/// Wait for a file to exist, contain a match, or be gone
#[derive(Args)]
pub(crate) struct WaitFileArgs {
    /// The file (or directory) to watch
    pub(crate) path: String,
    /// Wait until a line of the file matches REGEX, and print that line
    #[usage(long, value_name = "REGEX")]
    pub(crate) contains: Option<String>,
    /// Wait until nothing exists at PATH
    #[usage(long)]
    pub(crate) gone: bool,
}

/// Wait for a line matching a regex to be appended to a log
///
/// Follows the path like `tail -F`: a file that is rotated or truncated is
/// read again from its start, and one that does not exist yet is waited for.
/// Starts at the current end of the file and prints the matching line.
#[derive(Args)]
pub(crate) struct WaitLogArgs {
    /// The log file to follow
    pub(crate) path: String,
    /// The regex a line must match
    #[usage(long = "match", value_name = "REGEX")]
    pub(crate) r#match: String,
    /// Also search what the file already holds
    #[usage(long)]
    pub(crate) from_start: bool,
}

/// Wait for a TCP port to accept connections, or to stop
#[derive(Args)]
pub(crate) struct WaitPortArgs {
    /// HOST:PORT, or just PORT for localhost
    pub(crate) addr: String,
    /// Wait until the port refuses connections instead
    #[usage(long)]
    pub(crate) closed: bool,
}

/// Wait for a URL to answer with a status, and optionally a match
///
/// Any 2xx status counts unless --status says otherwise. Redirects are
/// followed, except when --status asks for a 3xx. --expect takes the
/// assertions `jouet http` does, such as `jsonpath $.ready == true`, and
/// waits until all of them hold.
#[derive(Args)]
pub(crate) struct WaitHttpArgs {
    /// The http:// or https:// URL to GET
    pub(crate) url: String,
    /// The status code to wait for (default: any 2xx)
    #[usage(long, value_name = "CODE")]
    pub(crate) status: Option<u16>,
    /// Also wait until a line of the body matches REGEX, and print that line
    #[usage(long, value_name = "REGEX")]
    pub(crate) contains: Option<String>,
    /// Also wait until ASSERTION holds, as in `jouet http --expect`
    /// (repeatable)
    #[usage(long, value_name = "ASSERTION")]
    pub(crate) expect: Vec<String>,
}

/// Wait for a process to exit
#[derive(Args)]
pub(crate) struct WaitPidArgs {
    /// The process ID
    pub(crate) pid: u32,
}

/// Re-run a command until it exits with the expected code
///
/// Runs CMD directly, not through a shell, every --interval. A run still
/// going at the timeout is stopped, though every run gets at least one
/// --interval, so `--timeout 0` runs it once. Prints the last run's exit code and the
/// tail of its output, and saves the full output to a log.
#[derive(Args)]
pub(crate) struct WaitCmdArgs {
    /// The exit code to wait for (default 0)
    #[usage(long, value_name = "N")]
    pub(crate) expect_exit: Option<i32>,
    /// The command and its arguments, after `--`
    #[usage(double_dash = "required", value_name = "CMD")]
    pub(crate) command: Vec<String>,
}

/// Wait for a `jouet job` to end, or to be ready
///
/// --until exited (the default) is met when the job exits 0 or `jouet job
/// stop` stopped it, and fails (exit 1) when it exits with another code.
/// --until ready is met while it runs and its ready check passes: --port or
/// --http, else the one it was started with. A job that ends before it is
/// ready fails. On a failure or a timeout, the end of its log is shown.
/// Exits 3 when this work tree has no such job.
#[derive(Args)]
pub(crate) struct WaitJobArgs {
    /// The job's ID
    pub(crate) id: String,
    /// `exited` (the default) or `ready`
    #[usage(long, value_name = "CONDITION")]
    pub(crate) until: Option<String>,
    /// Ready once localhost's port N accepts connections
    #[usage(long, value_name = "N")]
    pub(crate) port: Option<u16>,
    /// Ready once URL answers with a 2xx status
    #[usage(long, value_name = "URL")]
    pub(crate) http: Option<String>,
    /// Lines of its log to show on failure or timeout (default 60; 0 shows
    /// all of it)
    #[usage(long, value_name = "N")]
    pub(crate) log_lines: Option<usize>,
    /// Do not show its log
    #[usage(long)]
    pub(crate) no_logs: bool,
}

/// Wait for a pitchfork daemon to be ready, or to stop
///
/// For a project's long-running services, the daemons pitchfork runs (one-off
/// work is `wait job`'s). Reads `pitchfork status ID --json` in this
/// directory, so a short ID resolves in this project's namespace. Ready is
/// `running` and, when there is a port to try (--port, or the port pitchfork
/// says the daemon binds) or a URL (--http), answering there: pitchfork's
/// JSON does not say whether its own ready check passed. --until stopped
/// waits until the daemon no longer runs. A daemon that errors or fails ends
/// the wait with exit 1; then, and on a timeout, the end of its log is shown
/// and the whole log is saved. Exits 3 when pitchfork has no such daemon and
/// 4 when pitchfork is not installed.
#[derive(Args)]
pub(crate) struct WaitDaemonArgs {
    /// The daemon's ID: `api`, or `namespace/api`
    pub(crate) id: String,
    /// `ready` (the default) or `stopped`
    #[usage(long, value_name = "CONDITION")]
    pub(crate) until: Option<String>,
    /// Ready once localhost's port N accepts connections
    #[usage(long, value_name = "N")]
    pub(crate) port: Option<u16>,
    /// Ready once URL answers with a 2xx status
    #[usage(long, value_name = "URL")]
    pub(crate) http: Option<String>,
    /// Lines of its log to show on failure or timeout (default 60; 0 shows
    /// all of it)
    #[usage(long, value_name = "N")]
    pub(crate) log_lines: Option<usize>,
    /// Do not read its log
    #[usage(long)]
    pub(crate) no_logs: bool,
}
