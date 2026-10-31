use usage_rs::Args;

/// Print a short usage guide written for agents
///
/// When to reach for each command, its key flags, the exit codes and the
/// output modes, generated from this binary's own CLI definition. With a
/// command, prints the invocations agents use, each with what it does, and
/// what --full adds: every subcommand and flag, and the details. With --json,
/// `text` is always the whole guide and `usage` the short one.
#[derive(Args)]
pub(crate) struct AgentHelpArgs {
    /// A command to describe, such as `edit`
    pub(crate) command: Vec<String>,
    /// Print the command's whole guide: every subcommand and flag, and the details
    #[usage(long)]
    pub(crate) full: bool,
}
