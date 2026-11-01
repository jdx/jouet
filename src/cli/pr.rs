use super::ThreadsArgs;
use usage_rs::{Args, Subcommands};

/// Inspect pull requests
#[derive(Args)]
pub(crate) struct PrArgs {
    #[usage(subcommand)]
    pub(crate) cmd: PrCmd,
}

#[derive(Subcommands)]
pub(crate) enum PrCmd {
    /// Show where PRs stand and why any need attention
    Status(StatusArgs),
    /// Say the one thing each PR needs next, and the command for it
    Next(NextArgs),
    /// Merge a PR that is ready, honouring branch protection
    Land(LandArgs),
    /// List, reply to and resolve review threads
    Threads(ThreadsArgs),
}

/// Show where PRs stand and why any need attention
///
/// Prints one line per PR, then a summary. GitHub PRs are fetched in one
/// batched GraphQL query; GitLab merge requests and Forgejo or Gitea PRs
/// (named HOST/OWNER/NAME#N, by URL, or through a remote on that forge) are
/// read through their REST APIs, and trails on Entire-hosted repositories
/// (entire.com/PROJECT/NAME#N) through the `entire` CLI. Each PR lists fixed attention codes: conflict,
/// checks_failing, checks_missing, changes_requested, unresolved_threads,
/// entire_findings, entire_gates, behind_base, draft and mergeable_unknown.
///
/// A PR with an Entire Gates check gets a line saying which gate fails and
/// why. The check is not counted as a failing check of its own: a failed
/// checks gate repeats the checks above, an up-to-date gate is behind_base,
/// and a findings gate is entire_findings, naming the open findings when
/// the `entire` CLI is on PATH and logged in. Without it, `--json` says
/// why under `entire`.
#[derive(Args)]
pub(crate) struct StatusArgs {
    /// PRs as numbers, URLs, OWNER/REPO#N or branch names. Defaults to the
    /// current branch's PR
    pub(crate) prs: Vec<String>,
    /// Add the open PRs authored by the token's user
    #[usage(long)]
    pub(crate) mine: bool,
    /// Add the PRs matching a GitHub search query
    #[usage(long, value_name = "QUERY")]
    pub(crate) search: Option<String>,
    /// The repository for bare numbers, branches, --mine and --search.
    /// Repeatable. Defaults to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Vec<String>,
    /// How long to wait for GitHub to compute an unknown mergeable state
    /// (default 20s; 0 disables)
    #[usage(long, value_name = "DUR")]
    pub(crate) settle: Option<String>,
    /// Exit 1 when any PR needs attention
    #[usage(long)]
    pub(crate) fail_on_attention: bool,
}

/// Say the one thing each PR needs next, and the command for it
///
/// Works through a fixed order and stops at the first state that applies:
/// merged, closed, draft, conflict, mergeable_unknown, behind_base,
/// checks_failing, checks_pending, changes_requested, unresolved_threads,
/// entire_findings, entire_gates, awaiting_approval, blocked, auto_merge,
/// ready_to_merge. The states come
/// from the same data and attention codes as `pr status`. Exits 0 when
/// nothing is left to do, 10 when the agent must act and 20 when waiting on
/// someone or something else; with several PRs, 10 wins over 20. These two
/// codes are outside the shared table on purpose.
#[derive(Args)]
pub(crate) struct NextArgs {
    /// PRs as numbers, URLs, OWNER/REPO#N or branch names. Defaults to the
    /// current branch's PR
    pub(crate) prs: Vec<String>,
    /// Add the open PRs authored by the token's user
    #[usage(long)]
    pub(crate) mine: bool,
    /// Add the PRs matching a GitHub search query
    #[usage(long, value_name = "QUERY")]
    pub(crate) search: Option<String>,
    /// The repository for bare numbers, branches, --mine and --search.
    /// Repeatable. Defaults to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Vec<String>,
    /// How long to wait for GitHub to compute an unknown mergeable state
    /// (default 20s; 0 disables)
    #[usage(long, value_name = "DUR")]
    pub(crate) settle: Option<String>,
}

/// Merge a PR that is ready, honouring branch protection
///
/// Refuses (exit 4) unless `pr next` says ready_to_merge, and says what is
/// missing. With --update, a branch behind its base is first updated on
/// GitHub by merging the base in (never a force-push), and its new checks
/// are waited for. The merge goes through the API with the head SHA as a
/// guard, the way the merge button does, so branch protection applies and
/// is never bypassed. Then the PR is confirmed merged and the merge commit
/// printed. It asks first only when a person is at a terminal (stdin and
/// stderr are terminals and no coding agent is detected); anyone else must
/// pass --yes or --dry-run, or it exits 2 before contacting GitHub.
///
/// The PR's checks on GitHub, which `pr next` reads, are its tests: the
/// full suite runs on CI, not here.
#[derive(Args)]
pub(crate) struct LandArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the
    /// current branch's PR
    pub(crate) pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git
    /// remote
    #[usage(long, value_name = "OWNER/NAME")]
    pub(crate) repo: Option<String>,
    /// How to merge: squash, merge or rebase (default squash)
    #[usage(long, value_name = "METHOD")]
    pub(crate) method: Option<String>,
    /// Update a branch that is behind its base by merging the base in on
    /// GitHub, then wait for its checks
    #[usage(long)]
    pub(crate) update: bool,
    /// Merge without asking
    #[usage(long)]
    pub(crate) yes: bool,
    /// Print the plan and change nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
    /// How long to wait for checks after --update (default 30m)
    #[usage(long, value_name = "DUR")]
    pub(crate) timeout: Option<String>,
    /// Time between the first checks of that wait; backs off to 60s
    /// (default 10s)
    #[usage(long, value_name = "DUR")]
    pub(crate) interval: Option<String>,
    /// How long to wait for GitHub to compute an unknown mergeable state
    /// (default 20s; 0 disables)
    #[usage(long, value_name = "DUR")]
    pub(crate) settle: Option<String>,
}
