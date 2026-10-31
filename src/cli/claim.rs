use usage_rs::{Args, Subcommands};

/// Claim paths for this session, so other sessions are warned off them
///
/// A claim is advisory: it blocks nothing. It tells other agent sessions on
/// this machine that this session works on these paths, and jouet's hooks
/// warn a session that edits or commits a path another live session claims,
/// naming that session and its worktree. Every worktree of a repository
/// shares its claims. PATH is a file, a directory (covering everything
/// under it) or a glob (`*`, `**`, `?`). A claim lasts --for (default 2h),
/// and lapses early when its worktree is removed or its agent exits.
/// Claiming a path again renews it. Refuses (exit 1) when another live
/// session claims an overlapping path, naming it; exits 4 outside an agent
/// session (set JOUET_SESSION_ID to claim from elsewhere).
#[derive(Args)]
#[usage(args_conflicts_with_subcommands)]
pub(crate) struct ClaimArgs {
    #[usage(subcommand)]
    pub(crate) cmd: Option<ClaimCmd>,
    /// The files, directories or globs to claim
    pub(crate) paths: Vec<String>,
    /// How long the claim lasts (default 2h)
    #[usage(long = "for", value_name = "DUR")]
    pub(crate) duration: Option<String>,
    /// What the session is doing there, shown to the others
    #[usage(long, value_name = "TEXT")]
    pub(crate) note: Option<String>,
}

#[derive(Subcommands)]
pub(crate) enum ClaimCmd {
    /// List the live claims in this repository, every session's
    Ls(ClaimLsArgs),
    /// Release this session's claims
    Release(ClaimReleaseArgs),
}

/// List the live claims in this repository, every session's
///
/// Claims that expired, or whose worktree or agent is gone, are removed
/// first; --all lists them before they go.
#[derive(Args)]
pub(crate) struct ClaimLsArgs {
    /// Also list the claims that no longer hold
    #[usage(long)]
    pub(crate) all: bool,
}

/// Release this session's claims
///
/// Releases the claims on the paths given, or all of this session's claims
/// in this repository. Another session's claim is never released: it lapses
/// on its own.
#[derive(Args)]
pub(crate) struct ClaimReleaseArgs {
    /// The claimed paths to release; all of this session's when none
    pub(crate) paths: Vec<String>,
}
