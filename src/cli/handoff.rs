use usage_rs::Args;

/// Record the facts about this checkout for the next session
///
/// Gathers the branch, its upstream and ahead/behind counts, HEAD, an
/// operation in progress, uncommitted and untracked files, the branch's PR
/// and its `pr status` attention codes, the session's last test, lint and
/// build runs (recorded by `jouet setup`'s hooks), background processes
/// running in the worktree (Linux), and its `jouet job`s. Writes them as
/// markdown with the data embedded, to `<git dir>/jouet/handoff.md` unless
/// -o names a file (`.json` writes JSON; `-` is stdout). Facts only: what
/// the work is and what to do next is the agent's to add.
#[derive(Args)]
pub(crate) struct HandoffArgs {
    /// Write the handoff to FILE (`-` is stdout)
    #[usage(short = 'o', long, value_name = "FILE")]
    pub(crate) output: Option<String>,
    /// Do not look up the branch's PR on GitHub
    #[usage(long)]
    pub(crate) no_pr: bool,
    /// The agent session whose check runs to include (default
    /// JOUET_SESSION_ID, else CLAUDE_CODE_SESSION_ID)
    #[usage(long, value_name = "ID")]
    pub(crate) session: Option<String>,
}

/// Print a handoff and check that it still holds
///
/// Reads `<git dir>/jouet/handoff.md`, or FILE, prints it, and compares it
/// with the checkout: the branch, HEAD, the upstream, an operation in
/// progress, uncommitted and untracked files, and whether its background
/// processes still run. Exits 0 when nothing has drifted and 1 when
/// something has. The PR is not fetched again: run `jouet pr status` for
/// its current state. Where the repository has Entire (`entire` on PATH,
/// the repository enabled, a working login), it also lists the earlier
/// sessions with checkpoints on this branch: the last checkpoint of each
/// and the files it touched, and the `entire` commands that show more.
#[derive(Args)]
pub(crate) struct PickupArgs {
    /// The handoff to read (`-` is stdin)
    pub(crate) file: Option<String>,
}
