use usage_rs::{Args, Subcommands};

/// Check a stack of PRs: does each layer really contain the one below
///
/// A stack is a chain of PRs, each based on the branch of the one below.
/// The chain comes from gh-stack's metadata when it tracks the branch, else
/// from the PRs' bases on GitHub. Branches are read as they are locally and
/// as of the last fetch; nothing is fetched or changed.
#[derive(Args)]
pub(crate) struct StackArgs {
    #[usage(subcommand)]
    pub(crate) cmd: StackCmd,
}

#[derive(Subcommands)]
pub(crate) enum StackCmd {
    /// Show each layer's PR and whether it contains its parent, by ancestry and content
    Status(StackStatusArgs),
    /// Bring each layer up to date by merging its parent, verified, never rebasing
    Sync(StackSyncArgs),
}

/// Show each layer's PR and whether it contains its parent, by ancestry and content
///
/// For each layer: its PR, base and head, unpushed local commits, and
/// whether it contains the layer below. That is checked by ancestry (the
/// parent's tip is in its history) and by content: every file the parent
/// layer changed must have that change in the layer's tree, so a restack
/// that kept the commits but reverted their changes is caught. A layer
/// whose prerequisite merged needs the trunk merged in. A layer's local
/// branch is used when there is one, since it may hold unpushed fixes.
/// Exits 1 when a layer needs attention, 3 when the branch is in no stack
/// (no PR, and gh-stack does not track it), 5 when GitHub cannot be
/// reached.
#[derive(Args)]
pub(crate) struct StackStatusArgs {
    /// A branch in the stack (default: the current branch)
    pub(crate) branch: Option<String>,
}

/// Bring each layer up to date by merging its parent, verified, never rebasing
///
/// Fetches, then goes up the stack from the bottom. Each layer starts from
/// its local branch, never the remote's, so unpushed fixes are kept; a
/// local branch behind its remote is refused. A layer that lacks its parent
/// gets the parent merged in, or the trunk once its prerequisite has merged
/// (a squash merge included). Each merge is checked before it is committed:
/// every change it brings in must be in the merged tree. Sync stops at the
/// first layer that conflicts, fails that check, is checked out in another
/// worktree, or has uncommitted changes, and says what to do. A branch
/// checked out nowhere is merged in a temporary worktree; the one checked
/// out here is merged in place. Never rebases or force-pushes; pushes only
/// with --push, plainly. Exits 1 on a conflict or a failed check, 4 when
/// refused before merging, 5 when a fetch or push fails.
#[derive(Args)]
pub(crate) struct StackSyncArgs {
    /// A branch in the stack (default: the current branch)
    pub(crate) branch: Option<String>,
    /// Show what would be merged; merge, fetch and push nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
    /// Push the layers ahead of their remote branch afterwards (never forced)
    #[usage(long)]
    pub(crate) push: bool,
    /// Use the remote branches as last fetched
    #[usage(long)]
    pub(crate) no_fetch: bool,
}
