use usage_rs::{Args, Subcommands};

/// Propose mise tasks for commands agents keep repeating, and add one
///
/// `jouet task propose` reads this repository's recent session transcripts
/// (Claude Code and Codex) and drafts a mise task for each command sequence,
/// or long command, that agents ran at least 3 times in 2 sessions and no
/// task already runs: its name, `run`, description, `depends` and `sources`
/// where they can be inferred, how often it ran, and which file it should go
/// in (mise.local.toml or mise.toml) with why. The hook ledger notices such
/// sequences too, and a session start then names them. `jouet task apply ID
/// --file local|shared` adds the task through `mise tasks add`, never by
/// editing TOML; which file is the user's choice, so ask them first. Applying
/// needs mise 2026.10.25 or later.
#[derive(Args)]
pub(crate) struct TaskArgs {
    #[usage(subcommand)]
    pub(crate) cmd: TaskCmd,
}

#[derive(Subcommands)]
pub(crate) enum TaskCmd {
    /// Draft mise tasks for commands agents repeat in this repository
    Propose(TaskProposeArgs),
    /// Add a proposed task through mise, to the file the user chose
    Apply(TaskApplyArgs),
}

/// Draft mise tasks for commands agents repeat in this repository
///
/// Prints every pending proposal for the repository: the task, the evidence
/// (runs, sessions, time, output tokens), the file jouet suggests and why,
/// and the question to ask the user. Proposals are kept with winding's notes:
/// `jouet wind show ID` prints one and `jouet wind dismiss ID` drops it.
#[derive(Args)]
pub(crate) struct TaskProposeArgs {
    /// Read the sessions written since TIME (7d, 12h, a date or RFC 3339), the
    /// 8 most recent (default 14d)
    #[usage(long, value_name = "TIME")]
    pub(crate) since: Option<String>,
    /// Which transcripts to read: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    pub(crate) harness: Option<String>,
}

/// Add a proposed task through mise, to the file the user chose
///
/// Runs `mise tasks add --json` (or `mise tasks set` for a task jouet added
/// before) with `--env local` for mise.local.toml or `--path` to the
/// repository's shared config for mise.toml, and prints the JSON mise
/// returns. Without --file it refuses (exit 4) and says which file jouet
/// suggests: ask the user, then pass their choice. A mise older than
/// 2026.10.25 is refused (exit 4). A command with shell syntax (`&&`, pipes,
/// quotes) is written as `sh -c '...'`, since mise quotes each word of a
/// task's command.
#[derive(Args)]
pub(crate) struct TaskApplyArgs {
    /// The proposal, such as t2
    pub(crate) id: String,
    /// Which file the user chose: local (mise.local.toml, theirs, not
    /// committed) or shared (mise.toml, committed)
    #[usage(long, value_name = "FILE")]
    pub(crate) file: Option<String>,
    /// Name the task NAME instead of the proposed name
    #[usage(long, value_name = "NAME")]
    pub(crate) name: Option<String>,
    /// Show what mise would write, writing nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
}
