//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod cmd;
mod config;
mod git;
mod guide;
mod ledger;
mod redact;
mod rules;
mod settings;
mod setup;
mod share;
mod spec;
mod verify;
mod wind;

use std::process::ExitCode;
use usage_rs::{Args, Cli, Subcommands};

#[derive(Cli)]
#[usage(
    name = "jouet",
    bin = "jouet",
    version,
    about = "A toolkit CLI for coding agents",
    unknown_flags = "error",
    completion = true
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
    /// Report where coding-agent sessions spend tokens and time
    Audit(AuditArgs),
    /// Make exact, all-or-nothing edits across files
    Edit(EditArgs),
    /// Show what jouet saved: measured from its ledger, estimated from your history
    Stats(StatsArgs),
    /// Inspect pull requests
    Pr(PrArgs),
    /// Block until a condition holds, then print one summary
    Wait(WaitArgs),
    /// List, test and trust the rules jouet's hooks run
    Rule(RuleArgs),
    /// Run a recipe: a Luau script that sequences jouet's commands
    Run(RunArgs),
    /// Notice costly patterns in the hook ledger and draft rules for them
    Wind(WindArgs),
    /// Propose retiring rules that are noisy or always overridden
    Unwind(UnwindArgs),
    /// Propose a proven rule, or a missing primitive, to jouet's maintainers
    Share(ShareArgs),
    /// Print a short usage guide written for agents
    AgentHelp(AgentHelpArgs),
    /// Print jouet's agent skill
    Skill(SkillArgs),
    /// Install jouet's hooks and skill into Claude Code or Codex
    Setup(SetupArgs),
    /// Print a shell completion script
    Completion(CompletionArgs),
    /// Answer a Claude Code or Codex hook
    #[usage(hide)]
    Hook(HookArgs),
}

/// Report where coding-agent sessions spend tokens and time
///
/// Reads Claude Code's transcripts (~/.claude/projects) and Codex's rollouts
/// (~/.codex/sessions and archived_sessions) on this machine, and ranks what
/// their requests spent input tokens on: CI polling, process polling, PR
/// threads, PR status, edits, builds and tests, reads and searches, git and
/// more. For each costly category it names the jouet command or rule that
/// addresses it, with an estimate of the saving, and it counts recurring
/// patterns such as sleep loops. Nothing leaves the machine. Commands are
/// shown only as shapes, with every value replaced, unless --examples asks
/// for redacted examples.
#[derive(Args)]
pub struct AuditArgs {
    /// Only requests made since TIME: a duration ago such as 7d or 12h, a
    /// date such as 2026-10-01, or RFC 3339
    #[usage(long, value_name = "TIME")]
    since: Option<String>,
    /// Which transcripts to read: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    harness: Option<String>,
    /// Only sessions that ran in PATH or a directory below it
    #[usage(long, value_name = "PATH")]
    project: Option<String>,
    /// Categories to list per harness in text (default 10; 0 lists all).
    /// --json lists every category
    #[usage(long, value_name = "N")]
    top: Option<usize>,
    /// Show one redacted example command per shape and pattern
    #[usage(long)]
    examples: bool,
}

/// Show what jouet saved: measured from its ledger, estimated from your history
///
/// Counts what the hook ledger recorded (jouet command runs, rule decisions
/// and what the agent did after a denial, checks, and failing CI logs
/// returned as excerpts), then estimates the input tokens those runs saved
/// against your own Claude Code and Codex history from before `jouet setup`.
/// Every estimate shows its basis, and a kind of work with fewer than 20 runs
/// before setup is not estimated. Without a ledger, it shows what one run of
/// each command would save.
#[derive(Args)]
pub struct StatsArgs {
    /// Only ledger entries since TIME: a duration ago such as 7d or 12h, a
    /// date such as 2026-10-01, or RFC 3339
    #[usage(long, value_name = "TIME")]
    since: Option<String>,
    /// Which harness: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    harness: Option<String>,
    /// Only one session's ledger entries
    #[usage(long, value_name = "ID")]
    session: Option<String>,
    /// Print one line about what jouet saved, or nothing, for a stop hook.
    /// `[stats] summary` in the config file or JOUET_STATS_SUMMARY sets its
    /// style: quips (default), plain or off
    #[usage(long)]
    summary: bool,
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
    /// Say the one thing each PR needs next, and the command for it
    Next(NextArgs),
    /// Merge a PR that is ready, honouring branch protection
    Land(LandArgs),
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

/// Say the one thing each PR needs next, and the command for it
///
/// Works through a fixed order and stops at the first state that applies:
/// merged, closed, draft, conflict, mergeable_unknown, behind_base,
/// checks_failing, checks_pending, changes_requested, unresolved_threads,
/// awaiting_approval, blocked, auto_merge, ready_to_merge. The states come
/// from the same data and attention codes as `pr status`. Exits 0 when
/// nothing is left to do, 10 when the agent must act and 20 when waiting on
/// someone or something else; with several PRs, 10 wins over 20. These two
/// codes are outside the shared table on purpose.
#[derive(Args)]
pub struct NextArgs {
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
#[derive(Args)]
pub struct LandArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the
    /// current branch's PR
    pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git
    /// remote
    #[usage(long, value_name = "OWNER/NAME")]
    repo: Option<String>,
    /// How to merge: squash, merge or rebase (default squash)
    #[usage(long, value_name = "METHOD")]
    method: Option<String>,
    /// Update a branch that is behind its base by merging the base in on
    /// GitHub, then wait for its checks
    #[usage(long)]
    update: bool,
    /// Merge without asking
    #[usage(long)]
    yes: bool,
    /// Print the plan and change nothing
    #[usage(long)]
    dry_run: bool,
    /// How long to wait for checks after --update (default 30m)
    #[usage(long, value_name = "DUR")]
    timeout: Option<String>,
    /// Time between the first checks of that wait; backs off to 60s
    /// (default 10s)
    #[usage(long, value_name = "DUR")]
    interval: Option<String>,
    /// How long to wait for GitHub to compute an unknown mergeable state
    /// (default 20s; 0 disables)
    #[usage(long, value_name = "DUR")]
    settle: Option<String>,
}

/// Block until a condition holds, then print one summary
///
/// Replaces `sleep N; check; repeat` loops. The first check is immediate;
/// later ones back off from --interval to 60s and speed up again when the
/// state changes. Exits 0 when the condition holds and 124 on timeout, where
/// running the same command again keeps waiting. In human mode a live status
/// line is shown on stderr; otherwise each change of state is one line on
/// stderr, so a backgrounded wait's log stays small.
#[derive(Args)]
pub struct WaitArgs {
    /// Give up after DUR, such as 90s, 10m or 1h30m (default 30m)
    #[usage(long, global, value_name = "DUR")]
    timeout: Option<String>,
    /// Time between the first checks; backs off to 60s (default 10s)
    #[usage(long, global, value_name = "DUR")]
    interval: Option<String>,
    /// Print one JSON event per line, then the result document
    #[usage(long, global)]
    jsonl: bool,
    /// Print no progress and no text result, only the exit code (and JSON
    /// when asked for)
    #[usage(long, global)]
    quiet: bool,
    #[usage(subcommand)]
    cmd: WaitCmd,
}

#[derive(Subcommands)]
pub enum WaitCmd {
    /// Wait for a PR's checks, and show why any failed
    Ci(WaitCiArgs),
    /// Wait for a GitHub Actions run
    Run(WaitRunArgs),
    /// Wait for a PR to become mergeable, merged, approved, ready, ...
    Pr(WaitPrArgs),
    /// Wait for a file to exist, contain a match, or be gone
    File(WaitFileArgs),
    /// Wait for a line matching a regex to be appended to a log
    Log(WaitLogArgs),
    /// Wait for a TCP port to accept connections, or to stop
    Port(WaitPortArgs),
    /// Wait for a URL to answer with a status, and optionally a match
    Http(WaitHttpArgs),
    /// Wait for a process to exit
    Pid(WaitPidArgs),
    /// Re-run a command until it exits with the expected code
    Cmd(WaitCmdArgs),
}

/// Wait for a PR's checks, and show why any failed
///
/// Follows the PR head, switching to a new head when one is pushed, unless
/// --sha pins a commit. Returns on the first failed check unless --all.
/// Skipped and neutral checks count as passing. A conflicted PR returns
/// `conflict` (exit 4), since pull_request workflows will not run, once
/// GitHub reports it again 10s later (right after a push it can still answer
/// for the old head); no checks within --appear-timeout returns `no_checks`
/// (exit 1).
/// For each
/// failed Actions job, the failed step's log is excerpted and the full log
/// saved. Waits on the same PR share their state, so they poll GitHub once.
#[derive(Args)]
pub struct WaitCiArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the
    /// current branch's PR
    pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git
    /// remote
    #[usage(long, value_name = "OWNER/NAME")]
    repo: Option<String>,
    /// Wait on this commit's checks instead of following the PR head
    #[usage(long, value_name = "SHA")]
    sha: Option<String>,
    /// Only checks that branch protection requires
    #[usage(long)]
    required: bool,
    /// Only checks whose name matches GLOB (`*` and `?`). Repeatable
    #[usage(long, value_name = "GLOB")]
    check: Vec<String>,
    /// Leave out checks whose name matches GLOB. Repeatable
    #[usage(long, value_name = "GLOB")]
    ignore: Vec<String>,
    /// Wait for every check to finish, even after one fails
    #[usage(long)]
    all: bool,
    /// Lines of each failure log to show (default 60; 0 shows the whole
    /// failed step)
    #[usage(long, value_name = "N")]
    log_lines: Option<usize>,
    /// Do not fetch failure logs
    #[usage(long)]
    no_logs: bool,
    /// How long checks may take to appear on a commit (default 3m)
    #[usage(long, value_name = "DUR")]
    appear_timeout: Option<String>,
}

/// Wait for a GitHub Actions run
///
/// Returns when the run completes, or on its first failed job, with the
/// failed step's log excerpted as for `wait ci`.
#[derive(Args)]
pub struct WaitRunArgs {
    /// The run ID, or its Actions URL
    run: String,
    /// The repository for a bare run ID. Defaults to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    repo: Option<String>,
    /// Lines of each failure log to show (default 60; 0 shows the whole
    /// failed step)
    #[usage(long, value_name = "N")]
    log_lines: Option<usize>,
    /// Do not fetch failure logs
    #[usage(long)]
    no_logs: bool,
}

/// Wait for a PR to become mergeable, merged, approved, ready, ...
///
/// --until is one of: mergeable (keeps waiting while GitHub reports
/// UNKNOWN; exit 1 on a conflict), merged (exit 1 if closed unmerged),
/// closed, approved (exit 1 when changes are requested), checks-registered
/// (any check on the head), ready (no `pr status` attention codes and no
/// pending checks; exit 1 on conflict, checks_failing, changes_requested or
/// unresolved_threads), or any-change (the next push, check result, review,
/// thread or state change, printed as one event). A conflict counts once
/// GitHub reports it again 10s later, since right after a push it can still
/// answer for the old head.
#[derive(Args)]
pub struct WaitPrArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the
    /// current branch's PR
    pr: Option<String>,
    /// The repository for a bare number or branch. Defaults to the git
    /// remote
    #[usage(long, value_name = "OWNER/NAME")]
    repo: Option<String>,
    /// The condition to wait for
    #[usage(long, value_name = "CONDITION")]
    until: String,
}

/// Wait for a file to exist, contain a match, or be gone
#[derive(Args)]
pub struct WaitFileArgs {
    /// The file (or directory) to watch
    path: String,
    /// Wait until a line of the file matches REGEX, and print that line
    #[usage(long, value_name = "REGEX")]
    contains: Option<String>,
    /// Wait until nothing exists at PATH
    #[usage(long)]
    gone: bool,
}

/// Wait for a line matching a regex to be appended to a log
///
/// Follows the path like `tail -F`: a file that is rotated or truncated is
/// read again from its start, and one that does not exist yet is waited for.
/// Starts at the current end of the file and prints the matching line.
#[derive(Args)]
pub struct WaitLogArgs {
    /// The log file to follow
    path: String,
    /// The regex a line must match
    #[usage(long = "match", value_name = "REGEX")]
    r#match: String,
    /// Also search what the file already holds
    #[usage(long)]
    from_start: bool,
}

/// Wait for a TCP port to accept connections, or to stop
#[derive(Args)]
pub struct WaitPortArgs {
    /// HOST:PORT, or just PORT for localhost
    addr: String,
    /// Wait until the port refuses connections instead
    #[usage(long)]
    closed: bool,
}

/// Wait for a URL to answer with a status, and optionally a match
///
/// Any 2xx status counts unless --status says otherwise. Redirects are
/// followed, except when --status asks for a 3xx.
#[derive(Args)]
pub struct WaitHttpArgs {
    /// The http:// or https:// URL to GET
    url: String,
    /// The status code to wait for (default: any 2xx)
    #[usage(long, value_name = "CODE")]
    status: Option<u16>,
    /// Also wait until a line of the body matches REGEX, and print that line
    #[usage(long, value_name = "REGEX")]
    contains: Option<String>,
}

/// Wait for a process to exit
#[derive(Args)]
pub struct WaitPidArgs {
    /// The process ID
    pid: u32,
}

/// Re-run a command until it exits with the expected code
///
/// Runs CMD directly, not through a shell, every --interval. A run still
/// going at the timeout is stopped, though every run gets at least one
/// --interval, so `--timeout 0` runs it once. Prints the last run's exit code and the
/// tail of its output, and saves the full output to a log.
#[derive(Args)]
pub struct WaitCmdArgs {
    /// The exit code to wait for (default 0)
    #[usage(long, value_name = "N")]
    expect_exit: Option<i32>,
    /// The command and its arguments, after `--`
    #[usage(double_dash = "required", value_name = "CMD")]
    command: Vec<String>,
}

/// List, test and trust the rules jouet's hooks run
///
/// Rules are Luau scripts: built in, in ~/.config/jouet/rules, and in a
/// repository's .jouet/rules once it is trusted. `jouet rule types` prints
/// the API they are written against.
#[derive(Args)]
pub struct RuleArgs {
    #[usage(subcommand)]
    cmd: RuleCmd,
}

#[derive(Subcommands)]
pub enum RuleCmd {
    /// List every rule, where it comes from and whether it runs
    List(RuleListArgs),
    /// Run a rule against a command or hook payload, or against its examples
    Test(RuleTestArgs),
    /// Print the Luau type definitions of the jouet module
    Types(RuleTypesArgs),
    /// Trust this repository's .jouet/rules as they are now
    Trust(RuleTrustArgs),
    /// Stop trusting this repository's .jouet/rules
    Untrust(RuleTrustArgs),
    /// Write a new rule of yours, in warn mode, from a template or a winding note
    New(RuleNewArgs),
    /// Switch a rule of yours from warn to deny
    Promote(RulePromoteArgs),
    /// Show every change made to your rules, and why
    Log(RuleLogArgs),
    /// Record something a rule could not express, for `jouet share --idea`
    Gap(RuleGapArgs),
}

/// Run a recipe: a Luau script that sequences jouet's commands
///
/// Recipes are built in (land, ship-check, sweep), in ~/.config/jouet/recipes,
/// and in a repository's .jouet/recipes once it is trusted with --trust. They
/// run only jouet's own commands, through `jouet.cmd`, and a command that
/// merges or posts needs `yes=true`. Words after the recipe are its
/// arguments: KEY=VALUE, or plain words in order, as in `jouet run land 123
/// yes=true`.
#[derive(Args)]
pub(crate) struct RunArgs {
    /// The recipe to run
    recipe: Option<String>,
    /// The recipe's arguments: KEY=VALUE, or plain words in order
    args: Vec<String>,
    /// List every recipe, where it comes from and what it takes
    #[usage(long)]
    list: bool,
    /// Print each step the recipe would run, running none
    #[usage(long)]
    dry_run: bool,
    /// Trust this repository's .jouet/recipes as they are now
    #[usage(long, conflicts = "--untrust")]
    trust: bool,
    /// Stop trusting this repository's .jouet/recipes
    #[usage(long)]
    untrust: bool,
}

/// List every rule, where it comes from and whether it runs
#[derive(Args)]
pub struct RuleListArgs {}

/// Run a rule against a command or hook payload, or against its examples
///
/// With neither --command nor --payload, runs the rule's `examples` and
/// fails if any does not do what it expects, so a rule can be checked
/// before it is enabled. --replay runs it over every call in the hook
/// ledger instead, and fails if it fires on none of them.
#[derive(Args)]
pub struct RuleTestArgs {
    /// A rule's name, or the path to a .luau file
    rule: String,
    /// A shell command to test the rule against
    #[usage(long, value_name = "CMD")]
    command: Option<String>,
    /// A Claude Code or Codex hook payload (JSON) to test against; `-` is stdin
    #[usage(long, value_name = "FILE")]
    payload: Option<String>,
    /// The command's exit code, for a post-tool-use rule (default 0)
    #[usage(long, value_name = "N")]
    exit_code: Option<i32>,
    /// Run the rule over the hook ledger's calls: what it fires on, and the near misses
    #[usage(long)]
    replay: bool,
}

/// Print the Luau type definitions of the jouet module
#[derive(Args)]
pub struct RuleTypesArgs {}

/// Trust or stop trusting this repository's .jouet/rules
#[derive(Args)]
pub struct RuleTrustArgs {}

/// Write a new rule of yours, in warn mode, from a template or a winding note
///
/// Writes ~/.config/jouet/rules/NAME.luau with a header saying why it exists,
/// who wrote it and when. It starts in warn mode: `jouet rule promote` makes
/// its denials real. With --from-note, the rule is the one `jouet wind`
/// drafted.
#[derive(Args)]
pub struct RuleNewArgs {
    /// The rule's name: lowercase words joined by dashes
    name: String,
    /// Start from the rule a winding note drafted, such as n3
    #[usage(long, value_name = "ID")]
    from_note: Option<String>,
    /// What the rule is for, in one line (the template's `why`)
    #[usage(long, value_name = "TEXT")]
    why: Option<String>,
    /// Who is writing it: your session's id (default: $JOUET_SESSION_ID, or the harness's)
    #[usage(long, value_name = "WHO")]
    author: Option<String>,
}

/// Switch a rule of yours from warn to deny
///
/// Only rules in ~/.config/jouet/rules: built-in rules are part of jouet,
/// and a repository's change through a reviewed PR. Logged in `jouet rule
/// log`.
#[derive(Args)]
pub struct RulePromoteArgs {
    /// The rule's name
    name: String,
    /// Why, for the log
    #[usage(long, value_name = "TEXT")]
    reason: Option<String>,
}

/// Show every change made to your rules, and why
#[derive(Args)]
pub struct RuleLogArgs {
    /// Only this rule's changes
    name: Option<String>,
}

/// Record something a rule could not express, for `jouet share --idea`
///
/// jouet records on its own when a rule reads a `jouet` primitive that does
/// not exist. This records a wish in your words: "I kept doing X; there is
/// no primitive for Y".
#[derive(Args)]
pub struct RuleGapArgs {
    /// What you wanted a rule to do, and what was missing
    text: String,
    /// The rule you were writing, if any
    #[usage(long, value_name = "NAME")]
    rule: Option<String>,
}

/// Propose a proven rule, or a missing primitive, to jouet's maintainers
///
/// Builds a proposal and prints the exact text, with paths, repository
/// names, hostnames and emails replaced, and a diff of what that changed.
/// Nothing is posted without --yes, which a person must approve: it posts a
/// Discussion in the "Rules & ideas" category of jdx/jouet with your GitHub
/// login. A rule is shared only once it passes its tests, has denied for a
/// week and fired often with few overrides. At most three posts a week.
#[derive(Args)]
pub struct ShareArgs {
    /// The rule to share; with --idea, the idea's key (default: the most recorded)
    name: Option<String>,
    /// Share an idea from recorded gaps instead of a rule
    #[usage(long)]
    idea: bool,
    /// Post it; without this, only show what would be posted
    #[usage(long)]
    yes: bool,
    /// Hide WORD as well, such as an internal tool's name. Repeatable
    #[usage(long, value_name = "WORD")]
    redact: Vec<String>,
}

/// Notice costly patterns in the hook ledger and draft rules for them
///
/// Reads the ledger jouet's hooks keep from where it last stopped. When a
/// command keeps failing across sessions, or agents keep sleeping instead of
/// waiting, it drafts a rule in warn mode and keeps a note of it, which the
/// next session start shows (two at most). The hooks run it on their own;
/// running it by hand reads the ledger now. Off with JOUET_WIND=0.
#[derive(Args)]
pub struct WindArgs {
    #[usage(subcommand)]
    cmd: Option<WindCmd>,
}

#[derive(Subcommands)]
pub enum WindCmd {
    /// Show a note: what was noticed, and the rule drafted for it
    Show(WindNoteArgs),
    /// Drop a note, so it is never shown again
    Dismiss(WindNoteArgs),
}

/// Show or drop a winding note
#[derive(Args)]
pub struct WindNoteArgs {
    /// The note's id, such as n3
    id: String,
}

/// Propose retiring rules that are noisy or always overridden
///
/// Lists your rules and a trusted repository's that the hook ledger shows
/// are nearly always overridden, fire on a large share of calls, or no
/// longer load. Built-in rules are never proposed. --apply moves your own
/// rules to ~/.config/jouet/rules/retired/ and logs it; a repository's rules
/// change through a PR.
#[derive(Args)]
pub struct UnwindArgs {
    /// Retire the proposed rules of yours
    #[usage(long)]
    apply: bool,
}

/// Print a short usage guide written for agents
///
/// When to reach for each command, its key flags, the exit codes and the
/// output modes, generated from this binary's own CLI definition.
#[derive(Args)]
pub struct AgentHelpArgs {
    /// A command to describe in full, such as `edit`
    command: Vec<String>,
}

/// Print jouet's agent skill
///
/// A short skill that says when to reach for jouet and points at `jouet
/// agent-help` for the flags. `jouet setup` installs it.
#[derive(Args)]
pub struct SkillArgs {
    /// claude or codex (a SKILL.md), or agents-md (a snippet for AGENTS.md)
    #[usage(long, value_name = "FORMAT")]
    format: Option<String>,
    /// Print the optional skill for proposing rules and ideas to jouet instead
    #[usage(long)]
    contribute: bool,
}

/// Install jouet's hooks and skill into Claude Code or Codex
///
/// Merges hooks into Claude Code's settings.json or Codex's hooks.json,
/// keeping every other hook, and installs the skill. Running it again changes
/// nothing; --uninstall removes only what jouet added.
#[derive(Args)]
pub struct SetupArgs {
    /// claude or codex
    harness: String,
    /// Install for every project, under your home directory (the default)
    #[usage(long)]
    global: bool,
    /// Install into this repository's .claude/, or .codex/ and .agents/
    #[usage(long)]
    project: bool,
    /// Show what would change without writing anything
    #[usage(long)]
    dry_run: bool,
    /// Remove jouet's hooks and skill
    #[usage(long)]
    uninstall: bool,
}

/// Print a shell completion script
///
/// The script asks this binary for each completion, so it needs nothing else
/// installed and follows whichever jouet is on PATH. Write it where your shell
/// loads completions, or source it from your shell's startup file.
#[derive(Args)]
pub struct CompletionArgs {
    /// The shell to complete for
    #[usage(
        arg,
        choices("bash", "zsh", "fish", "powershell", "pwsh", "elvish", "nu", "nushell")
    )]
    shell: String,
}

/// Answer a Claude Code or Codex hook
///
/// Reads the hook's JSON on stdin and runs jouet's rules. Installed by
/// `jouet setup`; never blocks the agent on an error of its own.
#[derive(Args)]
pub struct HookArgs {
    /// claude or codex
    harness: Option<String>,
    /// session-start, pre-tool-use, post-tool-use, post-tool-use-failure or stop
    event: Option<String>,
}

/// List, reply to and resolve review threads
///
/// Lists unresolved threads by default, outdated ones included and marked.
/// Each thread is addressed by its first comment's ID, the number in the web
/// UI's `#discussion_r` links. Bodies are condensed: HTML comments and badges
/// are removed, `<details>` collapse to their summary and each comment is cut
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
    /// Only threads started by bots: logins ending in `[bot]`, and the config's
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
    // Before the output mode is chosen: a bad JOUET_DISPLAY must not make a
    // hook exit 2, which a harness reads as "block".
    if let Cmd::Hook(args) = &cli.cmd {
        return cmd::hook::run(args).into();
    }
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Audit(args) => cmd::audit::run(&args, &out),
        Cmd::Edit(args) => cmd::edit::run(args, &out),
        Cmd::Stats(args) => cmd::stats::run(&args, &out),
        Cmd::Pr(args) => cmd::pr::run(args, &out),
        Cmd::Wait(args) => cmd::wait::run(args, &out),
        Cmd::Rule(args) => cmd::rule::run(&args, &out),
        Cmd::Run(args) => cmd::run::run(&args, &out),
        Cmd::Wind(args) => cmd::wind::run(&args, &out),
        Cmd::Unwind(args) => cmd::wind::unwind(&args, &out),
        Cmd::Share(args) => cmd::share::run(&args, &out),
        Cmd::AgentHelp(args) => cmd::agent_help::run(&args, &out),
        Cmd::Skill(args) => cmd::skill::run(&args, &out),
        Cmd::Setup(args) => cmd::setup::run(&args, &out),
        Cmd::Completion(args) => cmd::completion::run(&args, &out),
        Cmd::Hook(_) => unreachable!("handled above"),
    };
    exit.into()
}
