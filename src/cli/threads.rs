use usage_rs::{Args, Subcommands};

/// List, reply to and resolve review threads
///
/// Lists unresolved threads by default, outdated ones included and marked.
/// Each thread is addressed by its first comment's ID, the number in the web
/// UI's `#discussion_r` links (`#note_` on GitLab, `#issuecomment-` on
/// Forgejo). On an Entire-hosted trail the threads are its discussions. Forgejo cannot resolve threads through its API, so resolving
/// there exits 4 before anything is posted. Bodies are condensed: HTML comments and badges
/// are removed, `<details>` collapse to their summary and each comment is cut
/// at 30 lines with a marker; suggestion blocks are always kept whole.
///
/// When the `entire` CLI is on PATH and logged in, the open findings on the
/// PR's Entire trail follow the threads, each addressed as `entire:ID`.
/// They are Entire's, not GitHub's: resolving a finding leaves the threads
/// as they were. A stale finding counts as outdated and an agent's as a
/// bot's. Without the CLI, `--json` says why under `entire`.
#[derive(Args)]
pub(crate) struct ThreadsArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the current
    /// branch's PR
    pub(crate) pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git remote
    #[usage(long, global, value_name = "OWNER/NAME")]
    pub(crate) repo: Option<String>,
    /// List resolved threads too
    #[usage(long, conflicts = "--resolved")]
    pub(crate) all: bool,
    /// List only resolved threads
    #[usage(long)]
    pub(crate) resolved: bool,
    /// List only outdated threads
    #[usage(long, conflicts = "--no-outdated")]
    pub(crate) outdated: bool,
    /// Leave out outdated threads
    #[usage(long)]
    pub(crate) no_outdated: bool,
    /// Only threads started by LOGIN. Repeatable
    #[usage(long, var, value_name = "LOGIN")]
    pub(crate) author: Vec<String>,
    /// Only threads started by bots: logins ending in `[bot]`, and the config's
    /// `bots`
    #[usage(long, conflicts = "--no-bots")]
    pub(crate) bots: bool,
    /// Leave out threads started by bots
    #[usage(long)]
    pub(crate) no_bots: bool,
    /// Only threads with a comment posted at or after TIME: RFC 3339, a
    /// duration ago such as 2h, or @SHA for when that commit was pushed. Goes
    /// by when comments were posted, never by the commit they are on
    #[usage(long, value_name = "TIME")]
    pub(crate) since: Option<String>,
    /// Only threads on paths matching GLOB, a directory, or (without a /) a
    /// file name. Repeatable
    #[usage(long, var, value_name = "GLOB")]
    pub(crate) path: Vec<String>,
    /// Also list review summaries and PR conversation comments
    #[usage(long)]
    pub(crate) include_reviews: bool,
    /// Show N lines of the file around each thread's line
    #[usage(long, value_name = "N")]
    pub(crate) context: Option<u32>,
    /// Show comment bodies unaltered
    #[usage(long)]
    pub(crate) full: bool,
    /// Exit 1 when any listed thread is unresolved
    #[usage(long)]
    pub(crate) fail_if_unresolved: bool,
    #[usage(subcommand)]
    pub(crate) cmd: Option<ThreadsCmd>,
}

#[derive(Subcommands)]
pub(crate) enum ThreadsCmd {
    /// Reply to a thread
    Reply(ReplyArgs),
    /// Resolve threads
    Resolve(ResolveArgs),
    /// Unresolve threads
    Unresolve(UnresolveArgs),
    /// Reply to and resolve many threads from a JSON list
    Apply(ApplyArgs),
}

/// Reply to a thread
///
/// `{sha}` in the reply becomes the short HEAD SHA, and is refused unless
/// HEAD is on the PR's branch on GitHub. The configured reply footer is
/// appended. A reply that is already the thread's last comment from you is
/// not posted again.
///
/// For an Entire finding (`entire:ID`), the reply is the reason recorded on
/// Entire, so it needs --resolve or --dismiss.
#[derive(Args)]
pub(crate) struct ReplyArgs {
    /// The thread's ID, or an Entire finding's handle (`entire:ID`)
    pub(crate) id: String,
    /// The PR. Defaults to the current branch's PR
    pub(crate) pr: Option<String>,
    /// The reply
    #[usage(long, value_name = "TEXT", conflicts = "--body-file")]
    pub(crate) body: Option<String>,
    /// Read the reply from FILE (`-` is stdin)
    #[usage(long, value_name = "FILE")]
    pub(crate) body_file: Option<String>,
    /// Resolve the thread after replying
    #[usage(long)]
    pub(crate) resolve: bool,
    /// Dismiss the Entire finding instead of resolving it, with the reply
    /// as the reason
    #[usage(long)]
    pub(crate) dismiss: bool,
    /// Leave a thread that is already resolved alone; without this, a reply still goes into it
    #[usage(long)]
    pub(crate) ignore_resolved: bool,
    /// Check everything and show what would be posted, posting nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
}

/// Resolve threads
///
/// Every ID is checked before any thread is resolved. A PR given after the
/// IDs must not be a bare number: write it as #N.
///
/// Entire findings (`entire:ID`, as `jouet pr threads` lists them) are
/// resolved on Entire through the `entire` CLI, with --reply as the reason;
/// --dismiss dismisses them instead, and needs a reason. A finding and a
/// GitHub thread are separate: resolving one never resolves the other.
#[derive(Args)]
pub(crate) struct ResolveArgs {
    /// Thread IDs or Entire finding handles, then optionally the PR
    pub(crate) args: Vec<String>,
    /// Reply with TEXT before resolving each thread; for an Entire finding,
    /// the reason recorded
    #[usage(long, value_name = "TEXT")]
    pub(crate) reply: Option<String>,
    /// Dismiss the Entire findings instead of resolving them; needs --reply
    #[usage(long)]
    pub(crate) dismiss: bool,
    /// Leave threads that are already resolved alone; without this, replies still go into them
    #[usage(long)]
    pub(crate) ignore_resolved: bool,
    /// Check everything and show what would happen, changing nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
}

/// Unresolve threads
///
/// A PR given after the IDs must not be a bare number: write it as #N. An
/// Entire finding (`entire:ID`) is reopened on Entire.
#[derive(Args)]
pub(crate) struct UnresolveArgs {
    /// Thread IDs or Entire finding handles, then optionally the PR
    pub(crate) args: Vec<String>,
    /// Check everything and show what would happen, changing nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
}

/// Reply to and resolve many threads from a JSON list
///
/// The list holds {"id": N, "reply": "...", "resolve": true} entries. Every
/// entry is checked before anything is posted. Re-running the same list after
/// a failure is safe: resolved threads and replies already posted are
/// skipped.
///
/// An entry for an Entire finding has its handle as the ID, {"id":
/// "entire:ID", "reply": "...", "resolve": true}, with "dismiss": true to
/// dismiss it (which needs a reply) or "unresolve": true to reopen it. Its
/// reply is the reason recorded on Entire.
#[derive(Args)]
pub(crate) struct ApplyArgs {
    /// `-` to read the list from stdin, and optionally the PR
    pub(crate) args: Vec<String>,
    /// Read the list from FILE (`-` is stdin)
    #[usage(long, value_name = "FILE")]
    pub(crate) file: Option<String>,
    /// Leave threads that are already resolved alone; without this, replies still go into them
    #[usage(long)]
    pub(crate) ignore_resolved: bool,
    /// Check everything and show what would be posted, posting nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
}
