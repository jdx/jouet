use usage_rs::{Args, Subcommands};

/// Run one-off background work, such as a build or a server for one check
///
/// A job is a command running in the background, detached in a process
/// group of its own, with its output in one log. Jobs belong to the work
/// tree they were started in. No daemon runs: each job is a file under
/// jouet's state directory. Unix only; on Windows, start and stop exit 4.
#[derive(Args)]
pub(crate) struct JobArgs {
    #[usage(subcommand)]
    pub(crate) cmd: JobCmd,
}

#[derive(Subcommands)]
pub(crate) enum JobCmd {
    /// Start a command in the background and return once it is ready
    Start(JobStartArgs),
    /// List this work tree's jobs: running, exited, stopped or gone
    Ls(JobLsArgs),
    /// Print the end of a job's log
    Logs(JobLogsArgs),
    /// Stop jobs: SIGTERM to the whole process group, then SIGKILL
    Stop(JobStopArgs),
}

/// Start a command in the background and return once it is ready
///
/// Runs CMD (directly, not through a shell) in this directory, with stdin
/// closed and stdout and stderr in the job's log. With --port, --http or
/// --output, waits up to --timeout (default 60s) for the job to be ready and
/// fails (exit 1) with the end of its log if it exits first; a job still not
/// ready at the timeout keeps running (exit 124). Without one, it returns at
/// once, unless the command fails straight away. Refuses (exit 4) while a
/// job of the same ID runs, unless --replace stops it first.
#[derive(Args)]
pub(crate) struct JobStartArgs {
    /// The job's ID: letters, digits, `_`, `.` and `-`
    pub(crate) id: String,
    /// Ready once localhost's port N accepts connections
    #[usage(long, value_name = "N")]
    pub(crate) port: Option<u16>,
    /// Ready once URL answers with a 2xx status
    #[usage(long, value_name = "URL")]
    pub(crate) http: Option<String>,
    /// Ready once a line of the job's log matches REGEX
    #[usage(short = 'o', long, value_name = "REGEX")]
    pub(crate) output: Option<String>,
    /// How long to wait for the ready check (default 60s)
    #[usage(long, value_name = "DUR")]
    pub(crate) timeout: Option<String>,
    /// Stop a running job of the same ID first
    #[usage(long)]
    pub(crate) replace: bool,
    /// The command and its arguments, after `--`
    #[usage(double_dash = "required", value_name = "CMD")]
    pub(crate) command: Vec<String>,
}

/// List this work tree's jobs: running, exited, stopped or gone
///
/// `gone` is a job that is no longer running with nothing recorded about
/// how it ended. Jobs that ended more than a week ago are removed.
#[derive(Args)]
pub(crate) struct JobLsArgs {}

/// Print the end of a job's log
#[derive(Args)]
pub(crate) struct JobLogsArgs {
    /// The job's ID
    pub(crate) id: String,
    /// Lines to print (default 50; 0 prints all of it)
    #[usage(short = 'n', long, value_name = "N")]
    pub(crate) lines: Option<usize>,
}

/// Stop jobs: SIGTERM to the whole process group, then SIGKILL
///
/// A job that has already ended is left as it is.
#[derive(Args)]
pub(crate) struct JobStopArgs {
    /// The jobs' IDs
    pub(crate) ids: Vec<String>,
    /// How long to wait after SIGTERM before SIGKILL (default 5s)
    #[usage(long, value_name = "DUR")]
    pub(crate) grace: Option<String>,
}
