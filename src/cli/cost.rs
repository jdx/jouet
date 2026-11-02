use usage_rs::{Args, Subcommands};

/// Show what builds, tests and lints cost here, from timings kept in git notes
///
/// `jouet test` and the hooks time each build, test and lint run and keep
/// it as a git note on HEAD under refs/notes/jouet-cost (`[cost] record` in
/// the config file or JOUET_COST_RECORD turns it off). This lists each
/// command, or each family of commands such as one e2e test, costliest
/// first, read from up to 200 first-parent commits back. Timings from other
/// machines, such as CI's, are scaled to this one: by the median ratio of
/// this machine's time to theirs over the commands timed in both places, or
/// shown as they are when there are none. CI keeps its timings with `jouet
/// cost push` after its runs; `jouet cost fetch` brings them here.
///
/// With [cost] sequences = true (JOUET_COST_SEQUENCES=1), `jouet test`
/// records local start and finish observations for each recognized check in
/// an identified agent session. `--sequences` shows the second check's
/// median duration after the first for at least three adjacent recorded
/// pairs in the same session and worktree. Other work and cache changes may
/// intervene; these observations do not measure savings or prove cache reuse.
#[derive(Args)]
#[usage(args_conflicts_with_subcommands)]
pub(crate) struct CostArgs {
    #[usage(subcommand)]
    pub(crate) cmd: Option<CostCmd>,
    /// List only the commands estimated to take at least DUR, such as 30s
    #[usage(long, value_name = "DUR")]
    pub(crate) min: Option<String>,
    /// Show opt-in local observations of successive recorded checks
    #[usage(long)]
    pub(crate) sequences: bool,
}

#[derive(Subcommands)]
pub(crate) enum CostCmd {
    /// Fetch the remote's timings and merge them with the ones here
    Fetch(CostRemoteArgs),
    /// Push the timings here to the remote, merging in what it has; never forced
    Push(CostRemoteArgs),
}

/// The remote to fetch timings from or push them to
#[derive(Args)]
pub(crate) struct CostRemoteArgs {
    /// The remote (default origin)
    pub(crate) remote: Option<String>,
}
