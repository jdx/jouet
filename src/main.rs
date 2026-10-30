//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod cmd;
mod config;
mod display;
mod edit;
mod exit;
mod git;
mod github;
mod paths;
mod pr;
mod time;

use display::Mode;
use exit::Exit;
use std::process::ExitCode;
use usage_rs::{Args, Cli, Subcommands};

#[derive(Cli)]
#[usage(
    name = "jouet",
    bin = "jouet",
    version,
    about = "A toolkit CLI for coding agents",
    unknown_flags = "error"
)]
struct Jouet {
    /// Print one JSON document instead of text
    #[usage(long, global)]
    json: bool,
    /// Who the output is for: agent, human or plain. Detected when not given
    #[usage(long, global, value_name = "MODE")]
    display: Option<String>,
    #[usage(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommands)]
enum Cmd {
    /// Make exact, all-or-nothing edits across files
    Edit(EditArgs),
    /// Inspect pull requests
    Pr(PrArgs),
}

/// Make exact, all-or-nothing edits across files
///
/// Reads a spec from --spec or stdin: search/replace blocks, a Codex-style
/// patch, or JSON. Every edit is checked before any file is written, so a
/// failed edit leaves every file as it was.
#[derive(Args)]
pub struct EditArgs {
    /// Read the spec from FILE instead of stdin (`-` is stdin)
    #[usage(long, value_name = "FILE")]
    spec: Option<String>,
    /// Spec format: blocks, patch or json. Detected from the first line
    #[usage(long, value_name = "FORMAT")]
    format: Option<String>,
    /// Report what would change without writing anything
    #[usage(long, global)]
    dry_run: bool,
    /// Include a unified diff of each changed file
    #[usage(long, global)]
    diff: bool,
    /// Run CMD through the shell after writing
    #[usage(long, global, value_name = "CMD")]
    check: Option<String>,
    /// Restore every edited file if --check fails
    #[usage(long, global)]
    revert_on_fail: bool,
    /// Refuse unless PATH's contents have this SHA-256 hash. Repeatable
    #[usage(long, global, value_name = "PATH=SHA256")]
    expect_hash: Vec<String>,
    #[usage(subcommand)]
    cmd: Option<EditCmd>,
}

#[derive(Subcommands)]
pub enum EditCmd {
    /// Replace text in one file, given on the command line
    Replace(ReplaceArgs),
    /// Print the SHA-256 hash of files, for --expect-hash
    Hash(HashArgs),
}

/// Replace text in one file, given on the command line
///
/// Matches anywhere in the file, like `str::replace`, and requires exactly one
/// match unless --count or --all says otherwise.
#[derive(Args)]
pub struct ReplaceArgs {
    /// The file to edit
    path: String,
    /// The text to find
    #[usage(long, value_name = "TEXT")]
    old: String,
    /// The text to put in its place
    #[usage(long, value_name = "TEXT")]
    new: String,
    /// How many matches there must be
    #[usage(long, value_name = "N")]
    count: Option<String>,
    /// Replace every match (at least one)
    #[usage(long)]
    all: bool,
}

/// Print the SHA-256 hash of files, for --expect-hash
#[derive(Args)]
pub struct HashArgs {
    /// Files to hash
    paths: Vec<String>,
}

/// Inspect pull requests
#[derive(Args)]
pub struct PrArgs {
    #[usage(subcommand)]
    cmd: PrCmd,
}

#[derive(Subcommands)]
pub enum PrCmd {
    /// Show where PRs stand and why any need attention
    Status(StatusArgs),
    /// List, reply to and resolve review threads
    Threads(ThreadsArgs),
}

/// Show where PRs stand and why any need attention
///
/// Prints one line per PR, then a summary. All PRs are fetched in one batched
/// GraphQL query. Each PR lists fixed attention codes: conflict,
/// checks_failing, checks_missing, changes_requested, unresolved_threads,
/// behind_base, draft and mergeable_unknown.
#[derive(Args)]
pub struct StatusArgs {
    /// PRs as numbers, URLs, OWNER/REPO#N or branch names. Defaults to the
    /// current branch's PR
    prs: Vec<String>,
    /// Add the open PRs authored by the token's user
    #[usage(long)]
    mine: bool,
    /// Add the PRs matching a GitHub search query
    #[usage(long, value_name = "QUERY")]
    search: Option<String>,
    /// The repository for bare numbers, branches, --mine and --search.
    /// Repeatable. Defaults to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    repo: Vec<String>,
    /// How long to wait for GitHub to compute an unknown mergeable state
    /// (default 20s; 0 disables)
    #[usage(long, value_name = "DUR")]
    settle: Option<String>,
    /// Exit 1 when any PR needs attention
    #[usage(long)]
    fail_on_attention: bool,
}

/// List, reply to and resolve review threads
///
/// Lists unresolved threads by default, outdated ones included and marked.
/// Each thread is addressed by its first comment's ID, the number in the web
/// UI's #discussion_r links. Bodies are condensed: HTML comments and badges
/// are removed, <details> collapse to their summary and each comment is cut
/// at 30 lines with a marker; suggestion blocks are always kept whole.
#[derive(Args)]
pub struct ThreadsArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the current
    /// branch's PR
    pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git remote
    #[usage(long, global, value_name = "OWNER/NAME")]
    repo: Option<String>,
    /// List resolved threads too
    #[usage(long, conflicts = "--resolved")]
    all: bool,
    /// List only resolved threads
    #[usage(long)]
    resolved: bool,
    /// List only outdated threads
    #[usage(long, conflicts = "--no-outdated")]
    outdated: bool,
    /// Leave out outdated threads
    #[usage(long)]
    no_outdated: bool,
    /// Only threads started by LOGIN. Repeatable
    #[usage(long, var, value_name = "LOGIN")]
    author: Vec<String>,
    /// Only threads started by bots: logins ending in [bot], and the config's
    /// `bots`
    #[usage(long, conflicts = "--no-bots")]
    bots: bool,
    /// Leave out threads started by bots
    #[usage(long)]
    no_bots: bool,
    /// Only threads with a comment posted at or after TIME: RFC 3339, a
    /// duration ago such as 2h, or @SHA for when that commit was pushed. Goes
    /// by when comments were posted, never by the commit they are on
    #[usage(long, value_name = "TIME")]
    since: Option<String>,
    /// Only threads on paths matching GLOB, a directory, or (without a /) a
    /// file name. Repeatable
    #[usage(long, var, value_name = "GLOB")]
    path: Vec<String>,
    /// Also list review summaries and PR conversation comments
    #[usage(long)]
    include_reviews: bool,
    /// Show N lines of the file around each thread's line
    #[usage(long, value_name = "N")]
    context: Option<u32>,
    /// Show comment bodies unaltered
    #[usage(long)]
    full: bool,
    /// Exit 1 when any listed thread is unresolved
    #[usage(long)]
    fail_if_unresolved: bool,
    #[usage(subcommand)]
    cmd: Option<ThreadsCmd>,
}

#[derive(Subcommands)]
pub enum ThreadsCmd {
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
#[derive(Args)]
pub struct ReplyArgs {
    /// The thread's ID
    id: String,
    /// The PR. Defaults to the current branch's PR
    pr: Option<String>,
    /// The reply
    #[usage(long, value_name = "TEXT", conflicts = "--body-file")]
    body: Option<String>,
    /// Read the reply from FILE (`-` is stdin)
    #[usage(long, value_name = "FILE")]
    body_file: Option<String>,
    /// Resolve the thread after replying
    #[usage(long)]
    resolve: bool,
    /// Skip a thread that is already resolved instead of failing
    #[usage(long)]
    ignore_resolved: bool,
    /// Check everything and show what would be posted, posting nothing
    #[usage(long)]
    dry_run: bool,
}

/// Resolve threads
///
/// Every ID is checked before any thread is resolved. A PR given after the
/// IDs must not be a bare number: write it as #N.
#[derive(Args)]
pub struct ResolveArgs {
    /// Thread IDs, then optionally the PR
    args: Vec<String>,
    /// Reply with TEXT before resolving each thread
    #[usage(long, value_name = "TEXT")]
    reply: Option<String>,
    /// Skip threads that are already resolved instead of failing
    #[usage(long)]
    ignore_resolved: bool,
    /// Check everything and show what would happen, changing nothing
    #[usage(long)]
    dry_run: bool,
}

/// Unresolve threads
///
/// A PR given after the IDs must not be a bare number: write it as #N.
#[derive(Args)]
pub struct UnresolveArgs {
    /// Thread IDs, then optionally the PR
    args: Vec<String>,
    /// Check everything and show what would happen, changing nothing
    #[usage(long)]
    dry_run: bool,
}

/// Reply to and resolve many threads from a JSON list
///
/// The list holds {"id": N, "reply": "...", "resolve": true} entries. Every
/// entry is checked before anything is posted. Re-running the same list after
/// a failure is safe: resolved threads and replies already posted are
/// skipped.
#[derive(Args)]
pub struct ApplyArgs {
    /// `-` to read the list from stdin, and optionally the PR
    args: Vec<String>,
    /// Read the list from FILE (`-` is stdin)
    #[usage(long, value_name = "FILE")]
    file: Option<String>,
    /// Skip threads that are already resolved instead of failing
    #[usage(long)]
    ignore_resolved: bool,
    /// Check everything and show what would be posted, posting nothing
    #[usage(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    let cli = Jouet::parse();
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Edit(args) => cmd::edit::run(args, &out),
        Cmd::Pr(args) => cmd::pr::run(args, &out),
    };
    exit.into()
}

impl cmd::Output {
    fn new(json: bool, display: Option<&str>) -> Self {
        match Mode::detect(display) {
            Ok(mode) => Self { json, mode },
            Err(message) => {
                let out = Self {
                    json,
                    mode: Mode::Plain,
                };
                out.error(Exit::Usage, &message);
                std::process::exit(Exit::Usage.code().into());
            }
        }
    }
}
