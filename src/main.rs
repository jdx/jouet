//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod cmd;
mod config;
mod git;
mod guide;
mod ledger;
mod reads;
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

// `joue` is a view of `jouet run`: argv[0] `joue` parses as `jouet run`,
// with --json and --display carried over, so `joue NAME` runs a routine.
#[derive(Cli)]
#[usage(
    name = "jouet",
    bin = "jouet",
    version,
    about = "A toolkit CLI for coding agents",
    unknown_flags = "error",
    completion = true,
    view("joue", root = "run", globals)
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
    /// Commit only the named paths, with trailers
    Commit(CommitArgs),
    /// Make exact, all-or-nothing edits across files
    Edit(EditArgs),
    /// Query the code graph: symbols, callers, impact and affected tests
    Graph(GraphArgs),
    /// Record the facts about this checkout for the next session
    Handoff(HandoffArgs),
    /// Print a handoff and check that it still holds
    Pickup(PickupArgs),
    /// Show what jouet saved: measured from its ledger, estimated from your history
    Stats(StatsArgs),
    /// Inspect pull requests
    Pr(PrArgs),
    /// Read a file by its outline, a symbol or a range, or the repository's map
    Read(ReadArgs),
    /// Write one file for reviewing a range of commits
    ReviewPackage(ReviewPackageArgs),
    /// Search files like rg, within a budget, grouped by symbol
    Search(SearchArgs),
    /// Block until a condition holds, then print one summary
    Wait(WaitArgs),
    /// List, test and trust the rules jouet's hooks run
    Rule(RuleArgs),
    /// Run a routine: a Luau program on jouet's primitives (also `joue`)
    Run(RunArgs),
    /// Notice costly patterns in the hook ledger and whole sessions, and propose fixes
    Wind(WindArgs),
    /// Propose retiring rules that are noisy or always overridden
    Unwind(UnwindArgs),
    /// Propose a proven rule, a missing primitive, or a fix to jouet itself to its maintainers
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

/// Commit only the named paths, with trailers
///
/// Commits exactly the paths given, as they are in the working tree (`git
/// commit --only`): anything else already staged stays staged, and untracked
/// files are added only when a named path covers them. Refuses -A, `.`, the
/// repository root and a path with no changes. Paths are literal, never
/// globs. Appends the configured trailers; inside an agent the default is a
/// co-author line (Claude Code) and the AI disclosure, with the tool and
/// version from AI_AGENT. Warns about a subject that is not a conventional
/// commit when the repository checks them. Hooks always run; when one fails,
/// the end of its output is shown and the whole of it kept in a log.
#[derive(Args)]
pub struct CommitArgs {
    /// The files and directories to commit
    paths: Vec<String>,
    /// The message. Repeatable: each is a paragraph, as with git
    #[usage(short = 'm', long, var, value_name = "MSG")]
    message: Vec<String>,
    /// Read the message from FILE (`-` is stdin)
    #[usage(short = 'F', long, value_name = "FILE")]
    file: Option<String>,
    /// Append a trailer, `Key: value`. Repeatable
    #[usage(long, var, value_name = "TRAILER")]
    trailer: Vec<String>,
    /// Leave out the configured and default trailers
    #[usage(long)]
    no_trailers: bool,
    /// Show what would be committed, committing nothing
    #[usage(long)]
    dry_run: bool,
    /// Refused: name the paths to commit instead
    #[usage(short = 'A', long, hide)]
    all: bool,
    /// Refused: jouet commit always runs hooks
    #[usage(long, hide)]
    no_verify: bool,
}

/// Write one file for reviewing a range of commits
///
/// The file holds the commits, oldest first, a diffstat, and the full diff
/// with --context lines around each change. Refuses an empty range, and a
/// BASE that is not an ancestor of HEAD (exit 4), since the diff would then
/// show changes HEAD does not make. Written to
/// `<git dir>/jouet/review-BASE-HEAD.md` unless -o names a file (`-` is
/// stdout).
#[derive(Args)]
pub struct ReviewPackageArgs {
    /// The commit the review starts after, such as origin/main
    base: String,
    /// The last commit to review (default HEAD)
    head: Option<String>,
    /// Write the package to FILE (`-` is stdout)
    #[usage(short = 'o', long, value_name = "FILE")]
    output: Option<String>,
    /// Lines of context around each change (default 10)
    #[usage(long, value_name = "N")]
    context: Option<u32>,
}

/// Record the facts about this checkout for the next session
///
/// Gathers the branch, its upstream and ahead/behind counts, HEAD, an
/// operation in progress, uncommitted and untracked files, the branch's PR
/// and its `pr status` attention codes, the session's last test, lint and
/// build runs (recorded by `jouet setup`'s hooks) and background processes
/// running in the worktree (Linux). Writes them as markdown with the data
/// embedded, to `<git dir>/jouet/handoff.md` unless -o names a file (`.json`
/// writes JSON; `-` is stdout). Facts only: what the work is and what to do
/// next is the agent's to add.
#[derive(Args)]
pub struct HandoffArgs {
    /// Write the handoff to FILE (`-` is stdout)
    #[usage(short = 'o', long, value_name = "FILE")]
    output: Option<String>,
    /// Do not look up the branch's PR on GitHub
    #[usage(long)]
    no_pr: bool,
    /// The agent session whose check runs to include (default
    /// JOUET_SESSION_ID, else CLAUDE_CODE_SESSION_ID)
    #[usage(long, value_name = "ID")]
    session: Option<String>,
}

/// Print a handoff and check that it still holds
///
/// Reads `<git dir>/jouet/handoff.md`, or FILE, prints it, and compares it
/// with the checkout: the branch, HEAD, the upstream, an operation in
/// progress, uncommitted and untracked files, and whether its background
/// processes still run. Exits 0 when nothing has drifted and 1 when
/// something has. The PR is not fetched again: run `jouet pr status` for
/// its current state.
#[derive(Args)]
pub struct PickupArgs {
    /// The handoff to read (`-` is stdin)
    file: Option<String>,
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
///
/// A blocks section headed `=== PATH#SYMBOL` replaces a whole definition,
/// found with tree-sitter, so a formatter reflowing its text cannot break
/// the match. SYMBOL is a name, a kind and a name (`fn download`), or a
/// qualified name (`Vfox::download`), as `jouet read FILE --outline` lists
/// them; it must match exactly one definition. The new text runs to the
/// next `===` line, at any indentation: it is indented to where the
/// definition sits. Comments and attributes above the definition stay. An
/// edit that would add syntax errors is refused, and when it changes the
/// definition's first line, references to its name in other files are
/// listed as callers that may need updating.
///
///   === src/backend/vfox.rs#Vfox::download
///   pub fn download(&self, url: &str, retries: u32) -> Result<()> {
///       ...
///   }
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
///
/// While jouet's hooks keep a ledger, it also refuses (exit 4) unless they
/// recorded a full test run passing at the PR head's tree; pass
/// --skip-verify with a reason, which the ledger keeps, to land anyway.
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
    /// Land without a full test pass recorded at the PR head's tree, and
    /// say why; the reason is kept in the hook ledger
    #[usage(long, value_name = "REASON")]
    skip_verify: Option<String>,
}

/// Search files like rg, within a budget, grouped by symbol
///
/// Embeds ripgrep's walker and searcher, so it respects .gitignore, skips
/// hidden and binary files, and is about as fast as rg. Patterns are rg's
/// regular expressions (-F for a literal). Beyond rg:
///
/// - Matches are grouped by file, then by the function, method, class or
///   module they are in (for Rust, Python, JavaScript, TypeScript, Go,
///   Ruby, Java, C and C++).
/// - Output stops at a byte budget, in total and per file, and says what it
///   left out: `... 4 more in src/x.rs`, and a closing line naming the files
///   it never reached. Nothing is cut silently; --budget 0 removes a limit.
/// - Inside an agent session (JOUET_SESSION_ID, CLAUDE_CODE_SESSION_ID or
///   CODEX_THREAD_ID), a match this session was already shown, in a file
///   that has not changed since, collapses to one line (`src/x.rs: 3 already
///   shown, unchanged`). --again shows them anyway.
///
/// Exits 0 with matches, 1 with none, 2 for a bad pattern or glob, and 3
/// for a path that does not exist.
///
/// Examples:
///   jouet search 'fn download' src/
///   jouet search -F 'client.get(' -g '*.rs'
///   jouet search -i timeout --budget 2000 --file-budget 400
///   jouet search -l TODO
#[derive(Args)]
pub struct SearchArgs {
    /// The regular expression to search for (a literal with -F)
    pattern: String,
    /// Files and directories to search (default .)
    paths: Vec<String>,
    /// Treat the pattern as a literal string
    #[usage(short = 'F', long)]
    fixed_strings: bool,
    /// Match case-insensitively
    #[usage(short = 'i', long)]
    ignore_case: bool,
    /// Match whole words only
    #[usage(short = 'w', long)]
    word_regexp: bool,
    /// Include or exclude files, as rg's -g: `*.rs`, `!*_test.go`. Repeatable
    #[usage(short = 'g', long, var, value_name = "GLOB")]
    glob: Vec<String>,
    /// Search hidden files and directories too
    #[usage(long)]
    hidden: bool,
    /// List the matching files with their match counts, not the lines
    #[usage(short = 'l', long)]
    files_with_matches: bool,
    /// The most bytes to print in total (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    budget: Option<usize>,
    /// The most bytes to print from one file (default 1500; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    file_budget: Option<usize>,
    /// Show matches this session was already shown, too
    #[usage(long)]
    again: bool,
}

/// Read a file by its outline, a symbol or a range, or the repository's map
///
/// Reading a large file whole to find one thing fills the context with code
/// that is not needed. Read it in steps instead:
///
/// 1. `--outline`: every definition with its line range, as `kind name
///    (start-end)`, the name qualified by its scope (`Vfox::download`).
/// 2. `--symbol NAME`: one definition's code. NAME is a name, a kind and a
///    name (`fn download`, `class Cart`), or a qualified name
///    (`Vfox::download`, `Shop.Cart`); every definition it matches is printed.
/// 3. `--range A:B`: lines A to B (`A:` to the end, `:B` from the start).
///
/// With none of them, a file of up to 300 lines is printed whole, and a
/// longer one as its outline. Code is printed with line numbers, at most
/// --max-lines lines (default 400; 0 is no limit) with a marker for the
/// rest. Inside an agent session, lines this session was already shown, from
/// a file unchanged since, collapse to one line; --again prints them.
/// Definitions come from tree-sitter, for Rust, Python, JavaScript,
/// TypeScript, Go, Ruby, Java, C and C++.
///
/// `--map` prints, for the files that matter most, the line of each of
/// their most important definitions, sized to a token budget, so an agent
/// can find its way around a repository without reading whole files. It is
/// Aider's repo map rebuilt in Rust: tree-sitter finds every definition and
/// reference, files are ranked by personalised PageRank over who refers to
/// whom, and as many top-ranked definitions are shown as fit the budget.
/// Tags are cached by modification time, so a second map of the same tree
/// only re-reads files that changed. --focus names a file you are working
/// on, or an identifier you care about: what it uses ranks higher, and a
/// focused file's own definitions are left out, as you already have it
/// open. The budget defaults to 1024 tokens with --focus and 8192 without.
///
/// Exits 3 for a file or symbol that does not exist, and 2 for an outline or
/// symbol of a file whose language jouet cannot read.
///
/// Examples:
///   jouet read src/backend/vfox.rs --outline
///   jouet read src/backend/vfox.rs --symbol 'Vfox::download'
///   jouet read src/backend/vfox.rs --range 120:160
///   jouet read --map --focus src/backend/vfox.rs --focus download
#[derive(Args)]
pub struct ReadArgs {
    /// The file to read, or with --map the directory to map (default .)
    path: Option<String>,
    /// List the file's definitions and their line ranges
    #[usage(long)]
    outline: bool,
    /// Print the definitions this names: NAME, KIND NAME or Scope::NAME
    #[usage(long, value_name = "NAME")]
    symbol: Option<String>,
    /// Print lines A to B: `A:B`, `A:` or `:B`
    #[usage(long, value_name = "A:B")]
    range: Option<String>,
    /// The most lines of code to print (default 400; 0 is no limit)
    #[usage(long, value_name = "N")]
    max_lines: Option<usize>,
    /// Print lines this session was already shown, too
    #[usage(long)]
    again: bool,
    /// Print a map of the repository's most important definitions
    #[usage(long)]
    map: bool,
    /// The map's size in tokens, estimated at 4 bytes each
    #[usage(long, value_name = "TOKENS")]
    budget: Option<usize>,
    /// For the map: a file you are working on, or an identifier you care about. Repeatable
    #[usage(long, var, value_name = "FILE|NAME")]
    focus: Vec<String>,
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
    /// How long a conflict GitHub reports must persist before `wait ci` and
    /// `wait pr` believe it (default 10s). For tests
    #[usage(long, global, hide, value_name = "DUR")]
    confirm_conflict: Option<String>,
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

/// Query the code graph: symbols, callers, impact and affected tests
///
/// The graph is built with tree-sitter from Rust, Python, TypeScript and
/// JavaScript, Go, Ruby, Java, C and C++ files, and kept in a database in
/// $XDG_CACHE_HOME/jouet/graph, one per work tree: nothing is written into
/// the repository. There is no daemon: every query first
/// re-indexes the files that changed since the last one, and jouet's
/// post-tool-use hook re-indexes files an agent edits. The code graph
/// builds on codegraph's design and extraction.
#[derive(Args)]
pub struct GraphArgs {
    #[usage(subcommand)]
    cmd: GraphCmd,
}

#[derive(Subcommands)]
pub enum GraphCmd {
    /// Build the graph, or bring it up to date
    Index(GraphIndexArgs),
    /// Show where the graph lives and what it holds
    Status(GraphStatusArgs),
    /// Find symbols by name, qualified name or words
    Symbols(GraphSymbolsArgs),
    /// List what calls, constructs or names a symbol
    Callers(GraphTraverseArgs),
    /// List what a symbol calls, constructs or names
    Callees(GraphTraverseArgs),
    /// List everything a change to a symbol could break
    Impact(GraphTraverseArgs),
    /// Find the code that answers a question, with its source and neighbours
    Explore(GraphExploreArgs),
    /// List the test files that depend on changed files
    Affected(GraphAffectedArgs),
}

/// Build the graph, or bring it up to date
///
/// Lists the work tree's source files (honouring .gitignore), and extracts
/// only the files whose content changed since the last index. Files larger
/// than 1 MiB or not UTF-8 are recorded without symbols.
#[derive(Args)]
pub struct GraphIndexArgs {
    /// Drop the graph and index everything from scratch
    #[usage(long)]
    full: bool,
}

/// Show where the graph lives and what it holds
///
/// Prints the database's path and size, and how many files, symbols,
/// edges and references it holds. Does not index.
#[derive(Args)]
pub struct GraphStatusArgs {}

/// Find symbols by name, qualified name or words
///
/// An exact name comes first, then a qualified name (`Client::push`,
/// `Client.push`), then symbols whose name, docstring or signature has
/// every word (`retry backoff`; `processPayment` has the words `process`
/// and `payment`). With no QUERY, lists every symbol under --path. Each
/// line is `path:line kind qualified-name`, and the ID after `#` names the
/// symbol to other graph commands.
#[derive(Args)]
pub struct GraphSymbolsArgs {
    /// A name, a qualified name, or words
    query: Option<String>,
    /// Only symbols of this kind: function, method, class, struct, …
    #[usage(long, value_name = "KIND")]
    kind: Option<String>,
    /// Only symbols in this file or directory
    #[usage(long, value_name = "PATH")]
    path: Option<String>,
    /// The most bytes to print (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    budget: Option<usize>,
    /// Which page of a longer answer to print (default 1)
    #[usage(long, value_name = "N")]
    page: Option<usize>,
}

/// A symbol's callers, callees or impact
///
/// SYMBOL is a name, a qualified name (`Client::push`), `PATH#NAME`,
/// `PATH:LINE` (the innermost symbol there) or an ID from `jouet graph
/// symbols`. A name that several symbols share answers for each of them.
/// callers and callees follow calls, constructions and references; impact
/// follows every edge but containment, from the symbol and everything it
/// contains, and groups the answer by file.
#[derive(Args)]
pub struct GraphTraverseArgs {
    /// The symbol
    symbol: String,
    /// How many hops to follow (default 1, and 3 for impact)
    #[usage(long, value_name = "N")]
    depth: Option<u32>,
    /// The most bytes to print (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    budget: Option<usize>,
    /// Which page of a longer answer to print (default 1)
    #[usage(long, value_name = "N")]
    page: Option<usize>,
}

/// Find the code that answers a question, with its source and neighbours
///
/// Ranks symbols by how well their names (split into words), qualified
/// names, docstrings and signatures match the question, puts tests after
/// the code they test unless the question is about tests, and prints each
/// with its signature, docstring, callers, callees and the start of its
/// source. The answer is paged rather than printed whole: a dense answer
/// stays in the agent's context for the rest of the session.
#[derive(Args)]
pub struct GraphExploreArgs {
    /// The question, or the names you know (`retry backoff for webhooks`)
    query: Vec<String>,
    /// The most bytes to print (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    budget: Option<usize>,
    /// The most bytes of one symbol's source (default 1200; 0 for none)
    #[usage(long, value_name = "BYTES")]
    source_budget: Option<usize>,
    /// Which page of a longer answer to print (default 1)
    #[usage(long, value_name = "N")]
    page: Option<usize>,
}

/// List the test files that depend on changed files
///
/// Follows the graph backwards from each FILE, through imports, calls,
/// references and inheritance, to every test file that reaches it, and
/// shows the chain that does. A FILE the graph cannot speak for (not
/// indexed, or in a language jouet does not extract) is listed as unknown,
/// and the exit code is 1: run the whole suite. A fallback for languages
/// without a native tool; `jouet test --affected` prefers the native one.
#[derive(Args)]
pub struct GraphAffectedArgs {
    /// The changed files
    files: Vec<String>,
}

/// List, test and trust the rules jouet's hooks run
///
/// Rules are `[[rule]]` tables in TOML files: built in, in
/// ~/.config/jouet/rules, and in a repository's .jouet/rules once it is
/// trusted. `jouet rule schema` prints their JSON schema; a rule with logic
/// a matcher cannot express names a Luau `script`, written against the API
/// `jouet rule types` prints.
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
    /// Print the JSON schema of rule files
    Schema(RuleSchemaArgs),
    /// Print the Luau type definitions of the jouet module, for rule scripts
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

/// Run a routine: a Luau program on jouet's primitives (also `joue`)
///
/// Routines are Luau programs, where rules are TOML configuration.
/// `joue NAME [ARGS]` is the same as `jouet run NAME [ARGS]`. Routines are
/// built in (land, pr-next, ship-check, sweep), in ~/.config/jouet/routines,
/// and in a repository's .jouet/routines once it is trusted with --trust.
/// Each declares its arguments, so `joue NAME --help` says what it takes,
/// and the capabilities it reaches; anything else is refused (exit 4). A
/// routine that writes acts only with --yes: without it every write is
/// previewed, and joue exits 4 (0 with --dry-run).
#[derive(Args)]
pub(crate) struct RunArgs {
    /// List every routine, where it comes from and what it reaches
    #[usage(long)]
    list: bool,
    /// Trust this repository's .jouet/routines as they are now
    #[usage(long, conflicts = "--untrust")]
    trust: bool,
    /// Stop trusting this repository's .jouet/routines
    #[usage(long)]
    untrust: bool,
    #[usage(subcommand)]
    cmd: Option<RunCmd>,
}

#[derive(Subcommands)]
pub(crate) enum RunCmd {
    /// Run routines' examples against their recorded responses
    Test(RunTestArgs),
    /// Print the Luau type definitions of the primitives
    Types(RunTypesArgs),
    #[usage(external_subcommand)]
    External(Vec<std::ffi::OsString>),
}

/// Run routines' examples against their recorded responses
///
/// Each example runs the routine with its arguments against a fixture: a
/// local fake GitHub serves its recorded responses, and jouet's commands,
/// git, mise and the ledger answer from it too. Nothing real is reached,
/// and file edits are only previewed. Fails (exit 1) when an example's exit
/// code, output, JSON, previewed writes or requests differ from what it
/// expects, or when a routine has no examples.
#[derive(Args)]
pub(crate) struct RunTestArgs {
    /// The routine (default: every one)
    routine: Option<String>,
}

/// Print the Luau type definitions of the primitives
#[derive(Args)]
pub(crate) struct RunTypesArgs {}

/// List every rule, where it comes from and whether it runs
#[derive(Args)]
pub struct RuleListArgs {}

/// Run a rule against a command or hook payload, or against its examples
///
/// With neither --command nor --payload, runs the rule's `examples`: every
/// `fires` example must make it deny, warn or record, and every `quiet`
/// one must leave it silent. It fails, showing what was expected and what
/// the rule did, when one does not, or when the rule lacks either kind.
/// --all does this for every rule. --replay runs the rule over every call
/// in the hook ledger instead, and fails if it fires on none of them.
#[derive(Args)]
pub struct RuleTestArgs {
    /// A rule's name, or the path to a .toml (or .luau) rule file
    rule: Option<String>,
    /// Run the examples of every rule: built in, yours and this repository's
    #[usage(long)]
    all: bool,
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

/// Print the JSON schema of rule files
///
/// Every key a `[[rule]]` table takes, with what it means. The rule files
/// jouet writes start with a `#:schema` line, so editors that read it
/// complete and check them.
#[derive(Args)]
pub struct RuleSchemaArgs {}

/// Print the Luau type definitions of the jouet module, for rule scripts
#[derive(Args)]
pub struct RuleTypesArgs {}

/// Trust or stop trusting this repository's .jouet/rules
#[derive(Args)]
pub struct RuleTrustArgs {}

/// Write a new rule of yours, in warn mode, from a template or a winding note
///
/// Writes ~/.config/jouet/rules/NAME.toml, a rule saying why it exists, who
/// wrote it and when, with a matcher and examples to fill in. It starts in
/// warn mode: `jouet rule promote` makes its denials real. --script adds
/// NAME.luau for logic a matcher cannot express. With --from-note, the rule
/// is the one `jouet wind` drafted.
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
    /// Also write NAME.luau, a script for logic a matcher cannot express
    #[usage(long)]
    script: bool,
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

/// Propose a proven rule, a missing primitive, or a fix to jouet itself to its maintainers
///
/// Builds a proposal and prints the exact text, with paths, repository
/// names, hostnames and emails replaced, and a diff of what that changed.
/// Nothing is posted without --yes, which a person must approve: it posts
/// with your GitHub login, a bug as an Issue on jdx/jouet and anything else
/// as a Discussion in its "Rules & ideas" category. A rule is shared only
/// once it passes its tests, has denied for a week and fired often with few
/// overrides. --idea also covers what sessions show about jouet itself:
/// crashes, exit codes outside its table, malformed JSON, usage errors,
/// output agents abandon, built-in rules overridden again and again, slow
/// hooks, and scripts written in many sessions. At most three posts a week.
#[derive(Args)]
pub struct ShareArgs {
    /// The rule to share; with --idea, the idea's key (default: the most recorded)
    name: Option<String>,
    /// Share an idea: a missing primitive, or something sessions show about jouet itself
    #[usage(long)]
    idea: bool,
    /// List the ideas ready to share, with their keys
    #[usage(long)]
    list: bool,
    /// Post it; without this, only show what would be posted
    #[usage(long)]
    yes: bool,
    /// Hide WORD as well, such as an internal tool's name. Repeatable
    #[usage(long, value_name = "WORD")]
    redact: Vec<String>,
}

/// Notice costly patterns in the hook ledger and whole sessions, and propose fixes
///
/// Reads the ledger jouet's hooks keep from where it last stopped. When a
/// command keeps failing across sessions, or agents keep sleeping instead of
/// waiting, it drafts a rule in warn mode and keeps a note of it, which the
/// next session start shows (two at most). The hooks run it on their own;
/// running it by hand reads the ledger now. Off with JOUET_WIND=0.
///
/// The ledger holds only command shapes. `jouet wind digest` and `jouet wind
/// review` read whole sessions: the user's corrections, abandoned
/// approaches, waits and repeated exploration. A review's proposals (mise
/// tasks, rules, routines, AGENTS.md or memory notes) are recorded with
/// `jouet wind propose` and shown like the ledger's notes.
#[derive(Args)]
pub struct WindArgs {
    #[usage(subcommand)]
    cmd: Option<WindCmd>,
}

#[derive(Subcommands)]
pub enum WindCmd {
    /// Show a note or a proposal: what was noticed, and what was drafted
    Show(WindNoteArgs),
    /// Drop a note, a proposal or a suggested review, so it is never shown again
    Dismiss(WindNoteArgs),
    /// Mark a review's proposal as applied
    Adopt(WindNoteArgs),
    /// Digest whole sessions: corrections, failures, waits, re-reads
    Digest(WindDigestArgs),
    /// Print a prompt and session digests for an agent to review
    Review(WindDigestArgs),
    /// Record what a review proposed, to show at the next session start
    Propose(WindProposeArgs),
}

/// Show or drop a winding note
#[derive(Args)]
pub struct WindNoteArgs {
    /// The id of a note (n3), a proposal (p2) or a suggested review (r1)
    id: String,
}

/// Digest whole sessions: corrections, failures, waits, re-reads
///
/// Reads a Claude Code or Codex transcript and reduces it to a few thousand
/// tokens: the user's corrections and interruptions, failures and their
/// retries, long calls, where tokens and time went (as `jouet audit` counts
/// them), files read repeatedly, commands typed out again, and a timeline
/// with a phase per prompt. Every item names its lines in the transcript.
/// Secrets are redacted (with gitleaks' rules too, when a config is found)
/// and the home directory is written as ~. Nothing leaves the machine.
/// `jouet wind review` prints the same with a prompt for a reviewing agent.
#[derive(Args)]
pub struct WindDigestArgs {
    /// A session ID, a prefix of one, or a transcript's path
    session: Option<String>,
    /// The most recently written session (inside a session, that session)
    #[usage(long)]
    last: bool,
    /// Every session written since TIME (7d, 12h, a date or RFC 3339), the
    /// 8 most recent
    #[usage(long, value_name = "TIME")]
    since: Option<String>,
    /// Which transcripts to read: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    harness: Option<String>,
    /// Only sessions that ran in PATH or a directory below it
    #[usage(long, value_name = "PATH")]
    project: Option<String>,
    /// Tokens per session's digest, about (default 4000)
    #[usage(long, value_name = "N")]
    budget: Option<usize>,
}

/// Record what a review proposed, to show at the next session start
///
/// Reads a review's JSON reply on stdin (or from FILE): `{"session": "...",
/// "proposals": [{"kind", "title", "why", "evidence", "scope", "draft"}]}`,
/// as `jouet wind review` asks for. Each proposal is checked and recorded
/// as pending: three per review and three a day at most, never one that
/// looks like it holds a secret, never one already proposed. Recording
/// applies nothing: the next session starts show each proposal in one line,
/// and the agent there asks the user before applying it.
#[derive(Args)]
pub struct WindProposeArgs {
    /// Read the reply from FILE instead of stdin
    file: Option<String>,
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
/// nothing; --uninstall removes only what jouet added. The hooks do nothing
/// where jouet is not on PATH.
#[derive(Args)]
pub struct SetupArgs {
    /// claude or codex
    harness: String,
    /// Install for every project, under your home directory (the default)
    #[usage(long)]
    global: bool,
    /// Install into this repository for you alone: Claude Code's untracked
    /// .claude/settings.local.json
    #[usage(long)]
    project: bool,
    /// Install into the repository's tracked .claude/settings.json, or
    /// .codex/hooks.json, for everyone who clones it (implies --project)
    #[usage(long)]
    shared: bool,
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
    /// Complete `joue` instead, the name routines run under
    #[usage(long)]
    joue: bool,
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
    if let Some(exit) = routine_typo() {
        return exit;
    }
    let cli = Jouet::parse();
    // Before the output mode is chosen: a bad JOUET_DISPLAY must not make a
    // hook exit 2, which a harness reads as "block".
    if let Cmd::Hook(args) = &cli.cmd {
        return cmd::hook::run(args).into();
    }
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Audit(args) => cmd::audit::run(&args, &out),
        Cmd::Commit(args) => cmd::commit::run(&args, &out),
        Cmd::Edit(args) => cmd::edit::run(args, &out),
        Cmd::Graph(args) => cmd::graph::run(&args, &out),
        Cmd::Handoff(args) => cmd::handoff::run_handoff(&args, &out),
        Cmd::Pickup(args) => cmd::handoff::run_pickup(&args, &out),
        Cmd::ReviewPackage(args) => cmd::review::run(&args, &out),
        Cmd::Stats(args) => cmd::stats::run(&args, &out),
        Cmd::Pr(args) => return cmd::pr::run(args, &out),
        Cmd::Read(args) => cmd::read::run(&args, &out),
        Cmd::Search(args) => cmd::search::run(&args, &out),
        Cmd::Wait(args) => cmd::wait::run(args, &out),
        Cmd::Rule(args) => cmd::rule::run(&args, &out),
        Cmd::Run(args) => return cmd::run::run(args, &out),
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

/// `jouet triage`, when `triage` is a routine and not a command: says to
/// run `joue triage` instead of failing as an unknown command.
fn routine_typo() -> Option<ExitCode> {
    let argv: Vec<String> = std::env::args().collect();
    let program = argv.first()?;
    let base = std::path::Path::new(program)
        .file_stem()?
        .to_string_lossy()
        .into_owned();
    if base == "joue" {
        return None;
    }
    let mut words = argv[1..].iter();
    let word = loop {
        let word = words.next()?;
        match word.as_str() {
            "--json" => continue,
            "--display" => {
                words.next();
                continue;
            }
            w if w.starts_with("--display=") => continue,
            w if w.starts_with('-') || w.starts_with("__") => return None,
            w => break w,
        }
    };
    if spec::has_command(&[word]) || !cmd::run::is_routine(word) {
        return None;
    }
    let out = cmd::Output::new(argv.iter().any(|w| w == "--json"), None);
    out.error_with_hint(
        jouet_output::exit::Exit::Usage,
        &format!("no command `{word}`"),
        Some(&format!("`{word}` is a routine: run it with `joue {word}`")),
    );
    Some(ExitCode::from(jouet_output::exit::Exit::Usage.code()))
}
