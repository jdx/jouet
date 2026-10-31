//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod affected;
mod ci;
mod cmd;
mod config;
mod deps;
mod diag;
mod docs;
mod entire;
mod fnox;
mod git;
mod guide;
mod harness;
mod ledger;
mod mise;
mod reads;
mod redact;
mod rules;
mod settings;
mod setup;
mod share;
mod spec;
mod testlog;
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
    /// Claim paths for this session, so other sessions are warned off them
    Claim(ClaimArgs),
    /// Commit only the named paths, with trailers
    Commit(CommitArgs),
    /// Find secrets in files, in what is staged, or in commits, with gitleaks' rules
    Secrets(SecretsArgs),
    /// Report the errors and warnings in files you changed, through hk
    Diag(DiagArgs),
    /// Make exact, all-or-nothing edits across files
    Edit(EditArgs),
    /// Query the code graph: symbols, callers, impact and affected tests
    Graph(GraphArgs),
    /// Record the facts about this checkout for the next session
    Handoff(HandoffArgs),
    /// Print a handoff and check that it still holds
    Pickup(PickupArgs),
    /// Show which agent sessions are working where on this machine
    Sessions(SessionsArgs),
    /// Show what jouet saved: measured from its ledger, estimated from your history
    Stats(StatsArgs),
    /// Inspect pull requests
    Pr(PrArgs),
    /// Read a file by its outline, a symbol or a range, or the repository's map
    Read(ReadArgs),
    /// Look up a dependency's API at the version the project locked
    Docs(DocsArgs),
    /// Check a package before adding it, or the locked dependencies, against its registry and OSV
    Deps(DepsArgs),
    /// Write one file for reviewing a range of commits
    ReviewPackage(ReviewPackageArgs),
    /// Check a stack of PRs: does each layer really contain the one below
    Stack(StackArgs),
    /// Search files like rg, within a budget, grouped by symbol
    Search(SearchArgs),
    /// Run the test suite, or only the tests a change affects
    Test(TestArgs),
    /// Block until a condition holds, then print one summary
    Wait(WaitArgs),
    /// Send an HTTP request and print the status and only the JSON paths asked for, within a budget
    Http(HttpArgs),
    /// Run one-off background work, such as a build or a server for one check
    Job(JobArgs),
    /// Run GitHub Actions jobs here, or say whether a failure on GitHub is flaky
    Ci(CiArgs),
    /// List, test and trust the rules jouet's hooks run
    Rule(RuleArgs),
    /// Run a routine: a Luau program on jouet's primitives (also `joue`)
    Run(RunArgs),
    /// Notice costly patterns in the hook ledger and whole sessions, and propose fixes
    Wind(WindArgs),
    /// Propose retiring rules that are noisy or always overridden, and unused tasks
    Unwind(UnwindArgs),
    /// Propose mise tasks for commands agents keep repeating, and add one
    Task(TaskArgs),
    /// Propose a proven rule, a missing primitive, or a fix to jouet itself to its maintainers
    Share(ShareArgs),
    /// Print a short usage guide written for agents
    AgentHelp(AgentHelpArgs),
    /// Print jouet's agent skill
    Skill(SkillArgs),
    /// Install jouet's hooks into a coding agent's harness
    Setup(SetupArgs),
    /// Print a shell completion script
    Completion(CompletionArgs),
    /// Answer a harness's hook
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
///
/// Before committing, the lines the commit adds are scanned with gitleaks'
/// rules (as `jouet secrets scan --staged` does), and in a project that uses
/// fnox, `fnox scan --values` checks them for its secrets' values. A secret
/// found refuses the commit (exit 1), naming the rule or secret and the
/// file and line, never the value. A false positive is let through with
/// --allow-secret RULE:PATH:LINE, `gitleaks:allow` on its line, or its
/// fingerprint in .gitleaksignore.
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
    /// Commit this finding of the secret scan anyway, a false positive:
    /// RULE:PATH:LINE as the refusal lists it. Repeatable
    #[usage(long, var, value_name = "RULE:PATH:LINE")]
    allow_secret: Vec<String>,
    /// Refused: name the paths to commit instead
    #[usage(short = 'A', long, hide)]
    all: bool,
    /// Refused: jouet commit always runs hooks
    #[usage(long, hide)]
    no_verify: bool,
}

/// Find secrets in files, in what is staged, or in commits, with gitleaks' rules
#[derive(Args)]
pub struct SecretsArgs {
    #[usage(subcommand)]
    cmd: SecretsCmd,
}

#[derive(Subcommands)]
pub enum SecretsCmd {
    /// Find API keys, tokens and private keys, with gitleaks' rules
    Scan(SecretsScanArgs),
}

/// Find API keys, tokens and private keys, with gitleaks' rules
///
/// Uses gitleaks' detection rules (github.com/gitleaks/gitleaks, MIT), built
/// in: each rule's keywords, regex, entropy and allowlists, and gitleaks'
/// own allowlists, which leave out lockfiles, images and vendored code. A
/// repository's .gitleaks.toml adds rules and allowlists ([extend]
/// useDefault = true, or allowlists alone) or replaces the rules. A line
/// with `gitleaks:allow` on it, and a finding whose fingerprint
/// (PATH:RULE:LINE) is in .gitleaksignore, are passed over.
///
/// Without --staged or --diff, scans the files and directories given (the
/// current directory by default) as they are on disk; directories are
/// walked as git sees them, so ignored files are left out unless named.
/// --staged scans only the lines the index adds to HEAD, which is what `git
/// commit` would record. --diff REF scans the lines each commit in REF..HEAD
/// adds, commit by commit, so a secret added and removed again is still
/// found; REF may also be a range, A..B. With either, the paths limit the
/// diff.
///
/// Each finding names its rule, file and line, with the match redacted to at
/// most the secret's first four characters; the secret itself is never
/// printed. Exits 1 when anything is found, 3 for a path or REF that does not
/// exist, and 4 outside a git repository with --staged or --diff, or for a
/// .gitleaks.toml that does not parse.
///
/// Examples:
///   jouet secrets scan
///   jouet secrets scan --staged
///   jouet secrets scan --diff origin/main
///   jouet secrets scan config/ .env.example
#[derive(Args)]
pub struct SecretsScanArgs {
    /// Files and directories to scan; with --staged or --diff, the paths
    /// the diff is limited to
    paths: Vec<String>,
    /// Scan the lines the index adds to HEAD: what `git commit` would record
    #[usage(long)]
    staged: bool,
    /// Scan the lines each commit in REF..HEAD adds, or in a range A..B
    #[usage(long, value_name = "REF")]
    diff: Option<String>,
}

/// Run the test suite, or only the tests a change affects
///
/// Without --affected, runs the repository's test task as the full suite: a
/// mise `test` task, a package.json `test` script, Cargo.toml (`cargo test
/// --workspace`), a justfile or Makefile `test` target or go.mod, else the
/// whole suite of each test runner found. With --affected, runs only the
/// tests that the changes since the merge base with --since (default: the
/// upstream, else the default branch), uncommitted and untracked files
/// included, affect. Adapters delegate to each runner's own selection:
/// vitest's and jest's module graphs, pytest-testmon when it is installed,
/// else a static import graph. The whole suite runs instead when a
/// lockfile, tool versions, CI, the test configuration, a conftest or setup
/// file, or a file no adapter can trace changed; nothing runs for docs
/// only. Each test is listed with the chain of imports that picked it, and
/// the command. Affected runs count as partial: the full suite still has to
/// pass before you finish or push. Words after `--` go to the runner.
///
/// With --cmd, the words after `--` are the whole command instead, run
/// directly rather than through a shell: one test, an e2e suite or a build,
/// such as `jouet test --cmd -- cargo build`.
///
/// Every run's output, stdout and stderr together, goes to a log under
/// jouet's log directory, kept for a week. For an agent (and in plain display), that is all
/// it goes to: a passing run prints one line with the counts and the log's
/// path, and a failing one each failure's name and the lines that explain it
/// (the assertion, the panic, the compiler error), up to --budget bytes,
/// then the names of the failures that did not fit. Read more from the log
/// rather than running it again. For a person at a terminal, the output
/// streams as it runs. Failures are recognised in the output of cargo test
/// and nextest, rustc, tsc, go test and go build, pytest, unittest, jest,
/// vitest and TAP (bats); for any other output, a failing run shows the end
/// of it.
///
/// Exits 1 when tests fail, 2 for a usage error (the runner's too), 4 when
/// no runner is found or installed.
#[derive(Args)]
pub struct TestArgs {
    /// Run only the tests the changes affect
    #[usage(long)]
    affected: bool,
    /// Measure changes from the merge base with REF (default: the upstream,
    /// else the default branch)
    #[usage(long, value_name = "REF")]
    since: Option<String>,
    /// Use only this adapter: vitest, jest, pytest or graph (Go, Ruby and Java, from the code graph)
    #[usage(long, value_name = "NAME")]
    adapter: Option<String>,
    /// Print what would run and why, running nothing
    #[usage(long)]
    dry_run: bool,
    /// Run the words after `--` as the command, instead of the test task
    #[usage(long)]
    cmd: bool,
    /// Bytes of failure text to show (default 4000; 0 shows every failure
    /// whole)
    #[usage(long, value_name = "BYTES")]
    budget: Option<usize>,
    /// Arguments for the test runner, after `--`; with --cmd, the command
    extra: Vec<String>,
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
/// build runs (recorded by `jouet setup`'s hooks), background processes
/// running in the worktree (Linux), and its `jouet job`s. Writes them as
/// markdown with the data embedded, to `<git dir>/jouet/handoff.md` unless
/// -o names a file (`.json` writes JSON; `-` is stdout). Facts only: what
/// the work is and what to do next is the agent's to add.
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
/// its current state. Where the repository has Entire (`entire` on PATH,
/// the repository enabled, a working login), it also lists the earlier
/// sessions with checkpoints on this branch: the last checkpoint of each
/// and the files it touched, and the `entire` commands that show more.
#[derive(Args)]
pub struct PickupArgs {
    /// The handoff to read (`-` is stdin)
    file: Option<String>,
}

/// Show which agent sessions are working where on this machine
///
/// Reads Claude Code's transcripts (~/.claude/projects, or
/// $CLAUDE_CONFIG_DIR) and Codex's rollouts (~/.codex/sessions, or
/// $CODEX_HOME) written within --since (default 24h). For each session it
/// shows the harness, the directory it last worked in, that directory's
/// work tree and checked-out branch, and when its transcript was last
/// written. A session is live when its transcript was written in the last
/// 15 minutes. Only the end of each transcript is read, never what the
/// session said, and nothing is written. Warns about two live sessions in
/// one work tree, which commit each other's uncommitted work, and about a
/// live session whose directory is gone. Lists live sessions; --all also
/// lists idle ones, marking those whose directory is gone. Where the
/// repository in the current directory has Entire, the sessions Entire
/// tracks there are added: a session jouet found gets its checkpoint
/// count, and one only Entire knows (another agent, or a transcript kept
/// elsewhere) is listed with it. Exits 0 whether or not there are
/// warnings.
#[derive(Args)]
pub struct SessionsArgs {
    /// Only sessions active since TIME: a duration ago such as 24h or 7d, a
    /// date such as 2026-10-01, or RFC 3339 (default 24h)
    #[usage(long, value_name = "TIME")]
    since: Option<String>,
    /// List idle sessions too, not only live ones
    #[usage(long)]
    all: bool,
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
/// A SEARCH block holds whole lines, indentation included: one that holds
/// part of a line matches nothing, and the failure says which line the text
/// is inside. `jouet edit replace` matches text anywhere, inside a line too.
///
/// A blocks section headed `=== PATH#SYMBOL` replaces a whole definition,
/// found with tree-sitter, so a formatter reflowing its text cannot break
/// the match. SYMBOL is a name, a kind and a name (`fn download`), or a
/// qualified name (`Vfox::download`), as `jouet read FILE --outline` lists
/// them; it must match exactly one definition. The new text runs to the
/// next `===` line, at any indentation: it is indented to where the
/// definition sits. Comments and attributes above the definition stay. An
/// edit that would add syntax errors is refused. When an edit changes the
/// definition's first line, its callers in other files are listed from the
/// code graph (`jouet graph`), or the change is noted when there is none.
///
///   === src/backend/vfox.rs#Vfox::download
///   pub fn download(&self, url: &str, retries: u32) -> Result<()> {
///       ...
///   }
///
/// `--pattern PAT --rewrite REPL --lang LANG [PATH...]` rewrites every
/// match of a structural pattern (ast-grep's) under the paths, instead of
/// reading a spec. The rewrites go through the same all-or-nothing writes,
/// --dry-run, --diff, --check and --revert-on-fail, a rewrite that adds
/// syntax errors is refused, and a pattern that matches nothing writes
/// nothing (exit 4). A pattern is code in LANG:
///
/// - Quote it in single quotes. In double quotes the shell expands `$X`,
///   and `"$X.unwrap()"` becomes `.unwrap()`, which matches nothing.
/// - `$X` (uppercase) matches exactly one node; `$$$X` matches any number,
///   such as all of a call's arguments. `foo($A)` matches only one-argument
///   calls, `foo($$$ARGS)` every call. `$_` matches without capturing.
/// - It must be whole code: `client.get($URL)`, not `get($URL`. A Rust
///   call pattern matches the call, not a `?` or `;` after it.
/// - A name used twice must match the same text twice: `$A == $A`.
/// - REPL is text with the pattern's `$X` filled in; the rest is copied as
///   written, and indentation follows the match.
///
/// `--callers-of PATH#SYMBOL` rewrites only the calls the code graph
/// attributes to that definition, so `load($$$A)` leaves other functions
/// named `load` alone; write the pattern for the whole call (`$F($P)`,
/// `$R.load($$$A)`). `--impact PATH#SYMBOL` rewrites only inside it and the
/// code depending on it. Matches the graph cannot attribute (dynamic
/// dispatch, macros, files it has not read) are listed as unresolved and
/// left alone, and so are its calls the pattern missed. The report lists
/// each rewrite and the test files that depend on them; --dry-run first.
///
/// Examples:
///   jouet edit --pattern '$C.get($URL).send()?' --rewrite '$C.get($URL).timeout(TIMEOUT).send()?' --lang rust src/
///   jouet edit --pattern 'print($$$ARGS)' --rewrite 'log.info($$$ARGS)' --lang python app/ --check 'python -m pytest -q' --revert-on-fail
///   jouet edit --dry-run --pattern '$F($P)' --rewrite '$F($P, None)' --lang rust --callers-of src/config.rs#load
///
/// Try a pattern with `jouet search --structural --lang LANG 'PAT'` first.
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
    /// Fail if the edit introduced diagnostics: the edited files are checked
    /// through hk (`jouet diag`) before and after writing, and only new
    /// findings count
    #[usage(long, global)]
    diag: bool,
    /// Restore every edited file if --check or --diag fails
    #[usage(long, global)]
    revert_on_fail: bool,
    /// Warn when the edit changes lines an earlier agent session wrote, by
    /// `entire why`: the session, checkpoint and prompt behind them, so
    /// `--dry-run --why` shows it before anything is written. It warns and
    /// does not refuse. Needs Entire enabled in the repository; off by
    /// default because `entire why` can take a minute where the history is
    /// long. Looks at the first 10 files
    #[usage(long, global)]
    why: bool,
    /// Rewrite code matching this ast-grep pattern, instead of reading a spec
    #[usage(long, value_name = "PAT")]
    pattern: Option<String>,
    /// What --pattern's matches become; its `$X` are the pattern's
    #[usage(long, value_name = "REPL")]
    rewrite: Option<String>,
    /// The language of --pattern: rust, python, javascript, typescript, tsx,
    /// go, ruby, java, c or cpp
    #[usage(long, value_name = "LANG")]
    lang: Option<String>,
    /// With --pattern, rewrite only the calls the code graph attributes to
    /// this symbol, given as PATH#NAME (or #ID from `jouet graph symbols`)
    #[usage(long, value_name = "PATH#SYMBOL")]
    callers_of: Option<String>,
    /// With --pattern, rewrite only inside this symbol and the code the
    /// graph says depends on it, three hops back
    #[usage(long, value_name = "PATH#SYMBOL")]
    impact: Option<String>,
    /// With --pattern, the files and directories to rewrite (default .)
    paths: Vec<String>,
    /// Refuse unless PATH's contents have this SHA-256 hash. Repeatable
    #[usage(long, global, value_name = "PATH=SHA256")]
    expect_hash: Vec<String>,
    #[usage(subcommand)]
    cmd: Option<EditCmd>,
}

#[derive(Subcommands)]
pub enum EditCmd {
    /// Replace text in files, given on the command line
    Replace(ReplaceArgs),
    /// Print the SHA-256 hash of files, for --expect-hash
    Hash(HashArgs),
}

/// Report the errors and warnings in files you changed, through hk
///
/// With an hk config (hk.pkl), runs `hk run check --safe --format json` on
/// exactly the files given, or with --changed (the default) those that
/// differ from HEAD or are untracked, and prints hk's normalised findings:
/// file, line, column, severity, rule, message and the step that found it.
/// A step that sets no `diagnostic_format` is reported with its status and
/// the end of its output. Without an hk config, falls back to cargo clippy,
/// ruff or shellcheck in their JSON modes, or says diagnostics are
/// unavailable. Exits 0 when clean, 1 with findings, 4 when hk refused to run
/// (`--safe` rejects commands whose effect is unknown or destructive) or
/// nothing can check the files.
#[derive(Args)]
pub struct DiagArgs {
    #[usage(subcommand)]
    cmd: DiagCmd,
}

#[derive(Subcommands)]
pub enum DiagCmd {
    /// Check files and report every finding
    Check(DiagCheckArgs),
    /// Report only the findings an edit introduced
    Delta(DiagDeltaArgs),
    /// Format files with the project's fixers (`hk run fix --safe`)
    Fix(DiagFixArgs),
}

/// Check files and report every finding
///
/// Each check also remembers, per file, what it found in that version of
/// the file, which is what `jouet diag delta` compares with.
#[derive(Args)]
pub struct DiagCheckArgs {
    /// Files to check (default: --changed)
    files: Vec<String>,
    /// The files that differ from HEAD or are untracked
    #[usage(long)]
    changed: bool,
    /// Show at most N findings (default 50); the rest are counted
    #[usage(long, value_name = "N")]
    max: Option<usize>,
}

/// Report only the findings an edit introduced
///
/// Checks the files, then compares each with the last check of an earlier
/// version of it: a finding is the same one when its file, rule and step
/// match and its message matches or it is within three lines, so findings
/// that only moved are not new. A file unchanged since HEAD has nothing new.
/// A changed file no check has seen before has no baseline; its findings are
/// listed apart and not counted. Exits 1 when the edit introduced a finding
/// or made a step without diagnostics fail.
#[derive(Args)]
pub struct DiagDeltaArgs {
    /// Files to check (default: --changed)
    files: Vec<String>,
    /// The files that differ from HEAD or are untracked
    #[usage(long)]
    changed: bool,
    /// Show at most N findings (default 50); the rest are counted
    #[usage(long, value_name = "N")]
    max: Option<usize>,
}

/// Format files with the project's fixers (`hk run fix --safe`)
///
/// Runs hk's fix steps on exactly these files, refusing (exit 4) a config
/// with a fixer whose effect is unknown or destructive, and reports which
/// files changed. Needs an hk config.
#[derive(Args)]
pub struct DiagFixArgs {
    /// Files to format (default: --changed)
    files: Vec<String>,
    /// The files that differ from HEAD or are untracked
    #[usage(long)]
    changed: bool,
    /// Only this hk step. Repeatable
    #[usage(long, var, value_name = "STEP")]
    step: Vec<String>,
    /// Say what would run, changing nothing
    #[usage(long)]
    dry_run: bool,
}

/// Replace text in files, given on the command line
///
/// Matches anywhere in a file, like `str::replace`, and requires exactly one
/// match in each file unless --count or --all says otherwise. Repeat --old
/// and --new for several replacements: the Nth --new replaces the Nth
/// --old, in order. Every replacement applies to every file, and every file
/// is checked before any is written, so a rename across files is one call
/// that lands whole or not at all.
///
/// Examples:
///   jouet edit replace src/lib.rs --old 'let x = 1;' --new 'let x = 2;'
///   jouet edit replace a.py b.py --old retry_limit --new max_attempts --all
///   jouet edit replace app.py --old 'a()' --new 'b()' --old 'c()' --new 'd()'
#[derive(Args)]
pub struct ReplaceArgs {
    /// The files to edit
    paths: Vec<String>,
    /// The text to find. Repeatable
    #[usage(long, var, value_name = "TEXT")]
    old: Vec<String>,
    /// The text to put in its place, one for each --old
    #[usage(long, var, value_name = "TEXT")]
    new: Vec<String>,
    /// How many matches each --old must have in each file
    #[usage(long, value_name = "N")]
    count: Option<String>,
    /// Replace every match (at least one in each file)
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
/// Prints one line per PR, then a summary. GitHub PRs are fetched in one
/// batched GraphQL query; GitLab merge requests and Forgejo or Gitea PRs
/// (named HOST/OWNER/NAME#N, by URL, or through a remote on that forge) are
/// read through their REST APIs. Each PR lists fixed attention codes: conflict,
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
/// With --structural, PATTERN is code in --lang, matched by ast-grep: `$X`
/// is one node, `$$$X` any number, and only files in that language are
/// searched. Quote it in single quotes, or the shell expands `$X`
/// (`jouet agent-help edit` has more on patterns).
///
/// Exits 0 with matches, 1 with none, 2 for a bad pattern or glob, and 3
/// for a path that does not exist.
///
/// Examples:
///   jouet search 'fn download' src/
///   jouet search -F 'client.get(' -g '*.rs'
///   jouet search -i timeout --budget 2000 --file-budget 400
///   jouet search -l TODO
///   jouet search --structural --lang rust '$X.unwrap()'
///   jouet search --structural --lang go 'if err != nil { return $$$ }'
///   jouet search --structural --lang typescript 'useEffect($FN, [])'
///   jouet search --structural --lang rust '$F($$$A)' --callers-of src/config.rs#load

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
    /// Match PATTERN as code, with ast-grep: `$X` is one node, `$$$X` any
    /// number. Needs --lang
    #[usage(long)]
    structural: bool,
    /// The language of a --structural pattern: rust, python, javascript,
    /// typescript, tsx, go, ruby, java, c or cpp
    #[usage(long, value_name = "LANG")]
    lang: Option<String>,
    /// With --structural, only the calls the code graph attributes to this
    /// symbol, given as PATH#NAME (or #ID from `jouet graph symbols`)
    #[usage(long, value_name = "PATH#SYMBOL")]
    callers_of: Option<String>,
    /// With --structural, only matches inside this symbol and the code the
    /// graph says depends on it, three hops back
    #[usage(long, value_name = "PATH#SYMBOL")]
    impact: Option<String>,
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
/// longer one as its outline. `--outline` prints a file of up to 100 lines
/// whole too (with its outline in --json), since reading it after its
/// outline would cost another call. Code is printed with line numbers, at most
/// --max-lines lines (default 400; 0 is no limit) with a marker for the
/// rest. Inside an agent session, lines this session was already shown, from
/// a file unchanged since, collapse to one line; --again prints them.
/// Definitions come from tree-sitter, for Rust, Python, JavaScript,
/// TypeScript, Go, Ruby, Java, C and C++.
///
/// `--map` prints, for the files that matter most, the line of each of
/// their most important definitions, sized to a token budget, so an agent
/// can find its way around a repository without reading whole files. It is
/// Aider's repo map on the code graph (`jouet graph`): files are ranked by
/// personalised PageRank over which file uses which definition, as the
/// graph resolved them, and as many top-ranked definitions are shown as fit
/// the budget. --focus names a file you are working on, or an identifier you
/// care about: what it uses ranks higher, and a focused file's own
/// definitions are left out, as you already have it open. The budget
/// defaults to 1024 tokens with --focus and 8192 without. With a PATH, only
/// files under that directory are listed.
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
    /// List the file's definitions and their line ranges (a file of up to
    /// 100 lines is printed whole)
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
    /// With --symbol or --range, say where the lines came from, by `entire
    /// why`: the agent session, checkpoint and prompt that wrote them, or the
    /// commits people made them in. Needs Entire enabled in the repository.
    /// Off by default because `entire why` takes from tens of milliseconds
    /// to two minutes where the history is long ([entire] why_timeout_ms)
    #[usage(long)]
    why: bool,
    /// Print a map of the repository's most important definitions, from the
    /// code graph
    #[usage(long)]
    map: bool,
    /// The map's size in tokens, estimated at 4 bytes each
    #[usage(long, value_name = "TOKENS")]
    budget: Option<usize>,
    /// For the map: a file you are working on, or an identifier you care
    /// about. Repeatable
    #[usage(long, var, value_name = "FILE|NAME")]
    focus: Vec<String>,
}

/// Check a package before adding it, or the locked dependencies, against its registry and OSV
///
/// Asks the package's registry (crates.io, npm, PyPI or the Go module
/// proxy) and OSV (osv.dev), so a dependency that does not exist, is
/// deprecated or yanked, or has a known advisory is caught before it is
/// added. Only package names and versions are sent; a package the lockfile
/// says comes from a private registry, a path or git is never sent. Answers
/// are cached under jouet's cache directory: a registry's for ten minutes,
/// OSV's for a day.
#[derive(Args)]
pub struct DepsArgs {
    #[usage(subcommand)]
    cmd: DepsCmd,
}

#[derive(Subcommands)]
pub enum DepsCmd {
    /// Say whether a package exists, its latest version, licence, repository and advisories
    Info(DepsInfoArgs),
    /// Check the project's locked dependencies against OSV
    Audit(DepsAuditArgs),
}

/// Say whether a package exists, its latest version, licence, repository and advisories
///
/// PKG is ECOSYSTEM:NAME[@VERSION], where ECOSYSTEM is cargo, npm, pypi or
/// go. A bare NAME takes its ecosystem from the lockfile that locks it, or
/// else from the one kind of manifest here (Cargo.toml, package.json,
/// pyproject.toml or requirements.txt, go.mod). The version checked is the
/// one given, else the one the lockfile locks, else the latest.
///
/// For a package that does not exist, crates.io's and npm's search name
/// close matches; PyPI and the Go proxy have no search. Go modules have no
/// licence in the proxy.
///
/// Exits 3 when a package does not exist (or has no such version), 1 when
/// the version checked has an advisory or is yanked or retracted, or the
/// package is deprecated, 4 when the lockfile locks it from somewhere other
/// than the public registry (its name is not sent), and 5 when a registry or
/// OSV cannot be reached.
///
/// Examples:
///   jouet deps info cargo:serde_json
///   jouet deps info npm:lodash@4.17.15
///   jouet deps info pypi:requests go:github.com/spf13/cobra
///   jouet deps info zod
#[derive(Args)]
pub struct DepsInfoArgs {
    /// The packages: ECOSYSTEM:NAME[@VERSION], or a NAME the project's
    /// lockfile or manifest places
    #[usage(required)]
    packages: Vec<String>,
    /// Ask the registry and OSV again rather than reading the cache
    #[usage(long)]
    refresh: bool,
}

/// Check the project's locked dependencies against OSV
///
/// Reads the nearest Cargo.lock, npm-family lockfile (package-lock.json,
/// pnpm-lock.yaml, yarn.lock, bun.lock), uv.lock or poetry.lock, and go.mod,
/// from here up to the repository's root, and asks OSV about every package
/// version they lock from a public registry, in batches. Lists only the
/// packages an advisory affects, each with its advisory IDs, severity and
/// the first version that fixes it.
///
/// Exits 1 when any locked package is affected, 3 when there is no
/// lockfile, and 5 when OSV cannot be reached.
///
/// Examples:
///   jouet deps audit
///   jouet deps audit --ecosystem npm
#[derive(Args)]
pub struct DepsAuditArgs {
    /// Only this ecosystem's lockfile: cargo, npm, pypi or go
    #[usage(long, value_name = "ECOSYSTEM")]
    ecosystem: Option<String>,
    /// Ask OSV again rather than reading the cache
    #[usage(long)]
    refresh: bool,
}

/// Send an HTTP request and print the status and only the JSON paths asked for, within a budget
///
/// Prints the status, the content type, size and time, the headers worth
/// acting on (location, retry-after, rate limits), and the body cut to
/// --budget bytes with a marker naming what was left out and the file the
/// whole body is saved in. With --select, it prints only the values at
/// those JSONPaths (RFC 9535) instead of the body, so nothing needs piping
/// through jq or Python. JSON is printed compact for agents and indented
/// for people.
///
/// --expect checks the response in hurl's style: a query (`status`, `header
/// NAME`, `jsonpath PATH`, `body`, `duration` in milliseconds), an optional
/// `count` and `not`, then a predicate (`==`, `!=`, `>`, `>=`, `<`, `<=`,
/// `contains`, `startsWith`, `endsWith`, `matches`, `exists`, `isEmpty`) and
/// its value. Without a `status` assertion, a status of 400 or more fails.
///
/// Secrets are never printed: Authorization, cookies, and headers and JSON
/// keys named like a token, password, API key or session are redacted, and so is any value of `${NAME}`, which jouet
/// expands from its own environment in the URL, headers and an inline body.
/// Give credentials that way, from the environment or through `fnox exec --
/// jouet http ...`, not written into the command; in a project that uses
/// fnox, its secrets' values are redacted too.
///
/// Exits 1 when an assertion fails, 3 when a --data or --json-body file is
/// missing, 4 when a `${NAME}` is not set, 5 when the server cannot be
/// reached, and 124 when it does not answer within --timeout.
///
/// Examples:
///   jouet http GET https://api.github.com/repos/jdx/mise --select '$.stargazers_count'
///   jouet http GET localhost:3000/health --expect 'jsonpath $.ok == true'
///   jouet http POST https://api.example.com/items --json-body '{"name":"x"}' --expect status=201
///   fnox exec -- jouet http GET https://api.example.com/me -H 'Authorization: Bearer ${API_TOKEN}'
#[derive(Args)]
pub struct HttpArgs {
    /// GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS
    method: String,
    /// The URL; one without a scheme is taken as http:// for localhost and
    /// https:// otherwise
    url: String,
    /// A request header, NAME:VALUE (repeatable)
    #[usage(short = 'H', long, value_name = "NAME:VALUE")]
    header: Vec<String>,
    /// Send JSON (checked to be JSON) with Content-Type: application/json;
    /// @FILE reads it from a file
    #[usage(long, value_name = "JSON")]
    json_body: Option<String>,
    /// Send this text as the body; @FILE reads a file and @- stdin
    #[usage(short = 'd', long, value_name = "DATA")]
    data: Option<String>,
    /// Print only the values at this JSONPath, such as `$.items[*].name`
    /// (repeatable)
    #[usage(short = 's', long, value_name = "JSONPATH")]
    select: Vec<String>,
    /// Check the response, as in `status == 200` or `jsonpath $.ok == true`,
    /// and exit 1 if it fails (repeatable)
    #[usage(short = 'e', long, value_name = "ASSERTION")]
    expect: Vec<String>,
    /// The most bytes of body or selected values to print (default 4000; 0
    /// is no limit)
    #[usage(long, value_name = "BYTES")]
    budget: Option<usize>,
    /// Print every response header, not only those worth acting on
    #[usage(short = 'i', long)]
    headers: bool,
    /// Do not follow redirects, so a 3xx is the answer
    #[usage(long)]
    no_follow: bool,
    /// Give up after DUR, such as 10s or 2m (default 30s)
    #[usage(long, value_name = "DUR")]
    timeout: Option<String>,
}

/// Look up a dependency's API at the version the project locked
///
/// Answers from the sources already installed for the project, at the
/// version its lockfile names, rather than from memory or a hosted index of
/// some other version. PKG's version comes from the nearest lockfile, from
/// the current directory up to the repository's root: Cargo.lock;
/// package-lock.json, pnpm-lock.yaml, yarn.lock or bun.lock; uv.lock,
/// poetry.lock or the virtualenv's metadata; go.mod. Its sources are read
/// where the package manager installed them: the cargo registry
/// ($CARGO_HOME) or a vendor directory, node_modules (its .d.ts files, else
/// @types/PKG, else its JavaScript), the virtualenv's site-packages
/// ($VIRTUAL_ENV, .venv or venv; a .pyi stub before its .py), the Go module
/// cache ($GOMODCACHE) or vendor/. Nothing is downloaded: a package that is
/// locked but not installed exits 4 with the command that installs it.
///
/// Without SYMBOL, an outline of the public API: each definition's path,
/// kind and signature with the first sentence of its doc, the package's
/// root first, cut to --budget with a marker naming what was left out. With
/// a module's path as SYMBOL, that module's outline.
///
/// With SYMBOL, each definition it names: the version and the file:line it
/// came from, its signature, its doc comment, the start of its source, and
/// for a type its methods, fields or variants (and for a Rust type the
/// traits it derives and implements). SYMBOL is a path from the
/// package's root, with or without its name (`Agent::get`,
/// `ureq::Agent::get`, `requests.Session.get`), else the end of one
/// (`get`). Several matches are each shown, shortest path first, up to
/// three, then listed.
///
/// Public means: for Rust, plain `pub` items reachable from the crate root
/// through `pub mod`s and `pub use`s (followed through the crate; one into
/// another crate names it, at the version the lockfile has), without
/// #[doc(hidden)]; for TypeScript, exported
/// declarations; for Python, names without a leading `_`, or those in
/// `__all__`, with re-exports followed; for Go, capitalised names.
/// --private includes the rest.
///
/// The index of each package version is built once, from the code graph's
/// parsers, and cached under jouet's cache directory.
///
/// Exits 3 for a package no lockfile names or a symbol it does not define,
/// and 4 for one that is locked but not installed.
///
/// Examples:
///   jouet docs ureq
///   jouet docs ureq Agent::get
///   jouet docs reqwest blocking
///   jouet docs zod ZodType
///   jouet docs click Context.invoke
///   jouet docs github.com/spf13/cobra Command.Execute
#[derive(Args)]
pub struct DocsArgs {
    /// The package: a crate, an npm package, a Python distribution or
    /// import name, or a Go package path; PKG@VERSION picks one of several
    /// locked versions
    package: String,
    /// The definition or module to show (default: the package's outline)
    symbol: Option<String>,
    /// The ecosystem, when PKG is in more than one lockfile here: rust, ts,
    /// python or go
    #[usage(long, value_name = "LANG")]
    lang: Option<String>,
    /// The outline's size in tokens, estimated at 4 bytes each (default
    /// 2048)
    #[usage(long, value_name = "TOKENS")]
    budget: Option<usize>,
    /// Include private definitions too
    #[usage(long)]
    private: bool,
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
    /// Wait for a GitHub Actions run or a GitLab pipeline
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
    /// Wait for a `jouet job` to end, or to be ready
    Job(WaitJobArgs),
    /// Wait for a pitchfork daemon to be ready, or to stop
    Daemon(WaitDaemonArgs),
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
/// failed Actions job or GitLab job, the failed step's log is excerpted and
/// the full log saved. On GitLab the checks are the head's pipeline jobs and
/// commit statuses, on Forgejo its commit statuses. Waits on the same PR share
/// their state, so they poll the forge once.
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

/// Wait for a GitHub Actions run or a GitLab pipeline
///
/// Returns when the run completes, or on its first failed job, with the
/// failed step's log excerpted as for `wait ci`. Forgejo has no runs jouet
/// can read, so it exits 4 there.
#[derive(Args)]
pub struct WaitRunArgs {
    /// The run ID, or its Actions or GitLab pipeline URL
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
/// followed, except when --status asks for a 3xx. --expect takes the
/// assertions `jouet http` does, such as `jsonpath $.ready == true`, and
/// waits until all of them hold.
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
    /// Also wait until ASSERTION holds, as in `jouet http --expect`
    /// (repeatable)
    #[usage(long, value_name = "ASSERTION")]
    expect: Vec<String>,
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

/// Wait for a `jouet job` to end, or to be ready
///
/// --until exited (the default) is met when the job exits 0 or `jouet job
/// stop` stopped it, and fails (exit 1) when it exits with another code.
/// --until ready is met while it runs and its ready check passes: --port or
/// --http, else the one it was started with. A job that ends before it is
/// ready fails. On a failure or a timeout, the end of its log is shown.
/// Exits 3 when this work tree has no such job.
#[derive(Args)]
pub struct WaitJobArgs {
    /// The job's ID
    id: String,
    /// `exited` (the default) or `ready`
    #[usage(long, value_name = "CONDITION")]
    until: Option<String>,
    /// Ready once localhost's port N accepts connections
    #[usage(long, value_name = "N")]
    port: Option<u16>,
    /// Ready once URL answers with a 2xx status
    #[usage(long, value_name = "URL")]
    http: Option<String>,
    /// Lines of its log to show on failure or timeout (default 60; 0 shows
    /// all of it)
    #[usage(long, value_name = "N")]
    log_lines: Option<usize>,
    /// Do not show its log
    #[usage(long)]
    no_logs: bool,
}

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
pub struct ClaimArgs {
    #[usage(subcommand)]
    cmd: Option<ClaimCmd>,
    /// The files, directories or globs to claim
    paths: Vec<String>,
    /// How long the claim lasts (default 2h)
    #[usage(long = "for", value_name = "DUR")]
    duration: Option<String>,
    /// What the session is doing there, shown to the others
    #[usage(long, value_name = "TEXT")]
    note: Option<String>,
}

#[derive(Subcommands)]
pub enum ClaimCmd {
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
pub struct ClaimLsArgs {
    /// Also list the claims that no longer hold
    #[usage(long)]
    all: bool,
}

/// Release this session's claims
///
/// Releases the claims on the paths given, or all of this session's claims
/// in this repository. Another session's claim is never released: it lapses
/// on its own.
#[derive(Args)]
pub struct ClaimReleaseArgs {
    /// The claimed paths to release; all of this session's when none
    paths: Vec<String>,
}

/// Check a stack of PRs: does each layer really contain the one below
///
/// A stack is a chain of PRs, each based on the branch of the one below.
/// The chain comes from gh-stack's metadata when it tracks the branch, else
/// from the PRs' bases on GitHub. Branches are read as they are locally and
/// as of the last fetch; nothing is fetched or changed.
#[derive(Args)]
pub struct StackArgs {
    #[usage(subcommand)]
    cmd: StackCmd,
}

#[derive(Subcommands)]
pub enum StackCmd {
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
pub struct StackStatusArgs {
    /// A branch in the stack (default: the current branch)
    branch: Option<String>,
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
pub struct StackSyncArgs {
    /// A branch in the stack (default: the current branch)
    branch: Option<String>,
    /// Show what would be merged; merge, fetch and push nothing
    #[usage(long)]
    dry_run: bool,
    /// Push the layers ahead of their remote branch afterwards (never forced)
    #[usage(long)]
    push: bool,
    /// Use the remote branches as last fetched
    #[usage(long)]
    no_fetch: bool,
}

/// Wait for a pitchfork daemon to be ready, or to stop
///
/// For a project's long-running services, the daemons pitchfork runs (one-off
/// work is `wait job`'s). Reads `pitchfork status ID --json` in this
/// directory, so a short ID resolves in this project's namespace. Ready is
/// `running` and, when there is a port to try (--port, or the port pitchfork
/// says the daemon binds) or a URL (--http), answering there: pitchfork's
/// JSON does not say whether its own ready check passed. --until stopped
/// waits until the daemon no longer runs. A daemon that errors or fails ends
/// the wait with exit 1; then, and on a timeout, the end of its log is shown
/// and the whole log is saved. Exits 3 when pitchfork has no such daemon and
/// 4 when pitchfork is not installed.
#[derive(Args)]
pub struct WaitDaemonArgs {
    /// The daemon's ID: `api`, or `namespace/api`
    id: String,
    /// `ready` (the default) or `stopped`
    #[usage(long, value_name = "CONDITION")]
    until: Option<String>,
    /// Ready once localhost's port N accepts connections
    #[usage(long, value_name = "N")]
    port: Option<u16>,
    /// Ready once URL answers with a 2xx status
    #[usage(long, value_name = "URL")]
    http: Option<String>,
    /// Lines of its log to show on failure or timeout (default 60; 0 shows
    /// all of it)
    #[usage(long, value_name = "N")]
    log_lines: Option<usize>,
    /// Do not read its log
    #[usage(long)]
    no_logs: bool,
}

/// Run one-off background work, such as a build or a server for one check
///
/// A job is a command running in the background, detached in a process
/// group of its own, with its output in one log. Jobs belong to the work
/// tree they were started in. No daemon runs: each job is a file under
/// jouet's state directory. Unix only; on Windows, start and stop exit 4.
#[derive(Args)]
pub struct JobArgs {
    #[usage(subcommand)]
    cmd: JobCmd,
}

#[derive(Subcommands)]
pub enum JobCmd {
    /// Start a command in the background and return once it is ready
    Start(JobStartArgs),
    /// List this work tree's jobs: running, exited, stopped or gone
    Ls(JobLsArgs),
    /// Print the end of a job's log
    Logs(JobLogsArgs),
    /// Stop jobs: SIGTERM to the whole process group, then SIGKILL
    Stop(JobStopArgs),
}

/// Start a command in the background and return once it is ready
///
/// Runs CMD (directly, not through a shell) in this directory, with stdin
/// closed and stdout and stderr in the job's log. With --port, --http or
/// --output, waits up to --timeout (default 60s) for the job to be ready and
/// fails (exit 1) with the end of its log if it exits first; a job still not
/// ready at the timeout keeps running (exit 124). Without one, it returns at
/// once, unless the command fails straight away. Refuses (exit 4) while a
/// job of the same ID runs, unless --replace stops it first.
#[derive(Args)]
pub struct JobStartArgs {
    /// The job's ID: letters, digits, `_`, `.` and `-`
    id: String,
    /// Ready once localhost's port N accepts connections
    #[usage(long, value_name = "N")]
    port: Option<u16>,
    /// Ready once URL answers with a 2xx status
    #[usage(long, value_name = "URL")]
    http: Option<String>,
    /// Ready once a line of the job's log matches REGEX
    #[usage(short = 'o', long, value_name = "REGEX")]
    output: Option<String>,
    /// How long to wait for the ready check (default 60s)
    #[usage(long, value_name = "DUR")]
    timeout: Option<String>,
    /// Stop a running job of the same ID first
    #[usage(long)]
    replace: bool,
    /// The command and its arguments, after `--`
    #[usage(double_dash = "required", value_name = "CMD")]
    command: Vec<String>,
}

/// List this work tree's jobs: running, exited, stopped or gone
///
/// `gone` is a job that is no longer running with nothing recorded about
/// how it ended. Jobs that ended more than a week ago are removed.
#[derive(Args)]
pub struct JobLsArgs {}

/// Print the end of a job's log
#[derive(Args)]
pub struct JobLogsArgs {
    /// The job's ID
    id: String,
    /// Lines to print (default 50; 0 prints all of it)
    #[usage(short = 'n', long, value_name = "N")]
    lines: Option<usize>,
}

/// Stop jobs: SIGTERM to the whole process group, then SIGKILL
///
/// A job that has already ended is left as it is.
#[derive(Args)]
pub struct JobStopArgs {
    /// The jobs' IDs
    ids: Vec<String>,
    /// How long to wait after SIGTERM before SIGKILL (default 5s)
    #[usage(long, value_name = "DUR")]
    grace: Option<String>,
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
    /// Delete the graphs of worktrees that no longer exist
    Prune(GraphPruneArgs),
}

/// Delete the graphs of worktrees that no longer exist
///
/// Each worktree's graph records its root. This deletes every graph in
/// $XDG_CACHE_HOME/jouet/graph whose root is gone, and keeps one whose root
/// exists or cannot be checked. Querying a graph also does this, at most
/// once a day.
#[derive(Args)]
pub struct GraphPruneArgs {}

/// Build the graph, or bring it up to date
///
/// Lists the work tree's source files (honouring .gitignore), and extracts
/// only the files whose content changed since the last index. Files larger
/// than 1 MiB or not UTF-8 are recorded without symbols. A worktree with no
/// graph yet starts from a copy of the graph of a sibling worktree of the
/// same repository, the one whose HEAD is nearest, so only the files that
/// differ are extracted.
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

/// Run GitHub Actions jobs here, or say whether a failure on GitHub is flaky
#[derive(Args)]
pub struct CiArgs {
    #[usage(subcommand)]
    cmd: CiCmd,
}

#[derive(Subcommands)]
pub enum CiCmd {
    /// Run a workflow's jobs here with jactionlint, and show why any failed
    Run(CiRunArgs),
    /// Say whether each failed job is flaky, infrastructure or real, and
    /// rerun only a flake
    Explain(CiExplainArgs),
}

/// Say whether each failed job is flaky, infrastructure or real, and rerun
/// only a flake
///
/// For each failed GitHub Actions job of a PR's latest runs, a run or one
/// job: the verdict, the evidence for it, the failed step and an excerpt of
/// its log. The log is read with fixed patterns for failed tests (cargo,
/// nextest, bats, pytest, vitest, jest, go test), compile and lint errors,
/// and runner or network trouble; history comes from the workflow's recent
/// runs. flaky: it passed on the same commit elsewhere (another attempt, or
/// another OS's job, which the evidence names), or the same test failed on
/// another branch that passed later. infrastructure: the runner, disk,
/// network, a registry, a rate limit, a service container or the cache.
/// real: a compile or lint error, a test that failed on every attempt on
/// this commit, or one that fails the same way on the default branch,
/// which the evidence says is not the change's fault. Anything else is
/// unknown. A job that failed or was cancelled only because another did is
/// listed as following it. --rerun reruns the failed jobs when every one is
/// flaky or infrastructure, and refuses (exit 4) otherwise. A second failure
/// on the same commit turns a flake real. Exits 0 when nothing failed or
/// every failure is flaky or infrastructure, 1 when one is real or unknown,
/// 3 when the PR, run or job does not exist, 4 when nothing has failed yet
/// in a run still going. What it fetches is cached under
/// the state dir, so explaining the same run again is cheap.
#[derive(Args)]
pub struct CiExplainArgs {
    /// The PR: a number, URL, OWNER/REPO#N or branch, or an Actions run or
    /// job URL. Defaults to the current branch's PR
    target: Option<String>,
    /// The repository for a bare number, branch, --run or --job. Defaults
    /// to the git remote
    #[usage(long, value_name = "OWNER/NAME")]
    repo: Option<String>,
    /// Explain this Actions run instead of a PR
    #[usage(long, value_name = "ID")]
    run: Option<u64>,
    /// Explain this one Actions job
    #[usage(long, value_name = "ID")]
    job: Option<u64>,
    /// Rerun the failed jobs if every one is flaky or infrastructure
    #[usage(long)]
    rerun: bool,
    /// Lines of each failure log to show (default 40; 0 shows the whole
    /// failed step)
    #[usage(long, value_name = "N")]
    log_lines: Option<usize>,
}

/// Run a workflow's jobs here with jactionlint, and show why any failed
///
/// Runs `jactionlint run`, which needs no container engine: each job runs in
/// a copy of the checkout, in the OS sandbox, with a throwaway home and no
/// secrets unless given. Prints one line for the run, then each failed step
/// with the end of its output (cut like `wait ci`'s log excerpts) and the
/// jobs that were skipped and why; the whole transcript is kept in a log.
/// WORKFLOW is a file of .github/workflows (the extension is optional), a
/// path, or a workflow's `name:`. --check takes a check's name as `wait ci`
/// prints it and runs the job behind it. Arguments after `--` go to
/// `jactionlint run` as they are, such as `--secret NAME=VALUE`.
/// `--secrets-from fnox` resolves the `secrets.*` the jobs read through
/// fnox and passes them in a temporary file, never on the command line. It shows
/// what happens on this machine, not whether GitHub will pass: jobs for
/// another OS are skipped. Exits 1 when a job failed, 3 when no workflow or
/// job matches, 4 when jactionlint is missing or the run could not start.
#[derive(Args)]
pub struct CiRunArgs {
    /// The workflow. Needed when the repository has several and no --check
    /// is given
    workflow: Option<String>,
    /// Run only this job and the jobs it needs. Repeatable
    #[usage(short = 'j', long, value_name = "ID")]
    job: Vec<String>,
    /// Run the job behind this check name, such as `unit (ubuntu-latest)`
    #[usage(long, value_name = "NAME")]
    check: Option<String>,
    /// The event that triggers the workflow (default push)
    #[usage(short = 'e', long, value_name = "EVENT")]
    event: Option<String>,
    /// Lines of each failed step to show (default 60; 0 shows all of it)
    #[usage(long, value_name = "N")]
    log_lines: Option<usize>,
    /// Give the jobs the secrets they read, resolved through this tool
    #[usage(long, value_name = "SOURCE", choices("fnox"))]
    secrets_from: Option<String>,
    /// Arguments for `jactionlint run`, after `--`
    #[usage(double_dash = "required", value_name = "ARGS")]
    args: Vec<String>,
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
    /// The id of a note (n3), a proposal (p2), a suggested review (r1) or a
    /// mise task (t1)
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

/// Propose retiring rules that are noisy or always overridden, and unused tasks
///
/// Lists your rules and a trusted repository's that the hook ledger shows
/// are nearly always overridden, fire on a large share of calls, or no
/// longer load. Built-in rules are never proposed. --apply moves your own
/// rules to ~/.config/jouet/rules/retired/ and logs it; a repository's rules
/// change through a PR. It also lists the mise tasks `jouet task apply` added
/// that no agent session has run in the 21 days since, and still run what
/// jouet wrote; --apply removes them with `mise tasks rm`.
#[derive(Args)]
pub struct UnwindArgs {
    /// Retire the proposed rules of yours, and remove the proposed tasks
    #[usage(long)]
    apply: bool,
}

/// Propose mise tasks for commands agents keep repeating, and add one
///
/// `jouet task propose` reads this repository's recent session transcripts
/// (Claude Code and Codex) and drafts a mise task for each command sequence,
/// or long command, that agents ran at least 3 times in 2 sessions and no
/// task already runs: its name, `run`, description, `depends` and `sources`
/// where they can be inferred, how often it ran, and which file it should go
/// in (mise.local.toml or mise.toml) with why. The hook ledger notices such
/// sequences too, and a session start then names them. `jouet task apply ID
/// --file local|shared` adds the task through `mise tasks add`, never by
/// editing TOML; which file is the user's choice, so ask them first. Applying
/// needs mise 2026.10.25 or later.
#[derive(Args)]
pub struct TaskArgs {
    #[usage(subcommand)]
    cmd: TaskCmd,
}

#[derive(Subcommands)]
pub enum TaskCmd {
    /// Draft mise tasks for commands agents repeat in this repository
    Propose(TaskProposeArgs),
    /// Add a proposed task through mise, to the file the user chose
    Apply(TaskApplyArgs),
}

/// Draft mise tasks for commands agents repeat in this repository
///
/// Prints every pending proposal for the repository: the task, the evidence
/// (runs, sessions, time, output tokens), the file jouet suggests and why,
/// and the question to ask the user. Proposals are kept with winding's notes:
/// `jouet wind show ID` prints one and `jouet wind dismiss ID` drops it.
#[derive(Args)]
pub struct TaskProposeArgs {
    /// Read the sessions written since TIME (7d, 12h, a date or RFC 3339), the
    /// 8 most recent (default 14d)
    #[usage(long, value_name = "TIME")]
    since: Option<String>,
    /// Which transcripts to read: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    harness: Option<String>,
}

/// Add a proposed task through mise, to the file the user chose
///
/// Runs `mise tasks add --json` (or `mise tasks set` for a task jouet added
/// before) with `--env local` for mise.local.toml or `--path` to the
/// repository's shared config for mise.toml, and prints the JSON mise
/// returns. Without --file it refuses (exit 4) and says which file jouet
/// suggests: ask the user, then pass their choice. A mise older than
/// 2026.10.25 is refused (exit 4). A command with shell syntax (`&&`, pipes,
/// quotes) is written as `sh -c '...'`, since mise quotes each word of a
/// task's command.
#[derive(Args)]
pub struct TaskApplyArgs {
    /// The proposal, such as t2
    id: String,
    /// Which file the user chose: local (mise.local.toml, theirs, not
    /// committed) or shared (mise.toml, committed)
    #[usage(long, value_name = "FILE")]
    file: Option<String>,
    /// Name the task NAME instead of the proposed name
    #[usage(long, value_name = "NAME")]
    name: Option<String>,
    /// Show what mise would write, writing nothing
    #[usage(long)]
    dry_run: bool,
}

/// Print a short usage guide written for agents
///
/// When to reach for each command, its key flags, the exit codes and the
/// output modes, generated from this binary's own CLI definition. With a
/// command, prints the invocations agents use, each with what it does, and
/// what --full adds: every subcommand and flag, and the details. With --json,
/// `text` is always the whole guide and `usage` the short one.
#[derive(Args)]
pub struct AgentHelpArgs {
    /// A command to describe, such as `edit`
    command: Vec<String>,
    /// Print the command's whole guide: every subcommand and flag, and the details
    #[usage(long)]
    full: bool,
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

/// Install jouet's hooks into a coding agent's harness
///
/// Merges hooks into Claude Code's settings.json, Codex's hooks.json,
/// Cursor's hooks.json, Gemini CLI's settings.json or Copilot's
/// hooks/jouet.json, keeping every other hook. The session-start hook primes each session with
/// the commands and rules, so the skill, which says the same, is installed
/// only with --skill or when priming is off ([hooks] prime). Running it again
/// changes nothing; --uninstall removes only what jouet added. The hooks do
/// nothing where jouet is not on PATH.
#[derive(Args)]
pub struct SetupArgs {
    /// claude, codex, cursor, gemini or copilot
    harness: String,
    /// Install for every project, under your home directory (the default)
    #[usage(long)]
    global: bool,
    /// Install into this repository for you alone: Claude Code's untracked
    /// .claude/settings.local.json
    #[usage(long)]
    project: bool,
    /// Install into the repository's tracked hooks file (.claude/settings.json,
    /// .codex/hooks.json, .cursor/hooks.json, .gemini/settings.json or
    /// .github/hooks/jouet.json), for everyone who clones it (implies
    /// --project)
    #[usage(long)]
    shared: bool,
    /// Show what would change without writing anything
    #[usage(long)]
    dry_run: bool,
    /// Remove jouet's hooks and skill
    #[usage(long)]
    uninstall: bool,
    /// Install the skill as well, which the session-start primer otherwise
    /// stands in for
    #[usage(long)]
    skill: bool,
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

/// Answer a harness's hook
///
/// Reads the hook's JSON on stdin and runs jouet's rules. Installed by
/// `jouet setup`; never blocks the agent on an error of its own.
#[derive(Args)]
pub struct HookArgs {
    /// claude, codex, cursor, gemini or copilot
    harness: Option<String>,
    /// session-start, pre-tool-use, post-tool-use, post-tool-use-failure,
    /// post-edit or stop
    event: Option<String>,
    /// For `stop`: the hk stop hook (`hk agent stop-hook ...`) jouet runs in
    /// its place, so one hook gives one verdict
    #[usage(long, value_name = "CMD")]
    hk: Option<String>,
}

/// List, reply to and resolve review threads
///
/// Lists unresolved threads by default, outdated ones included and marked.
/// Each thread is addressed by its first comment's ID, the number in the web
/// UI's `#discussion_r` links (`#note_` on GitLab, `#issuecomment-` on
/// Forgejo). Forgejo cannot resolve threads through its API, so resolving
/// there exits 4 before anything is posted. Bodies are condensed: HTML comments and badges
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
    /// Leave a thread that is already resolved alone; without this, a reply still goes into it
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
    /// Leave threads that are already resolved alone; without this, replies still go into them
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
    /// Leave threads that are already resolved alone; without this, replies still go into them
    #[usage(long)]
    ignore_resolved: bool,
    /// Check everything and show what would be posted, posting nothing
    #[usage(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    jouet_output::deprecation::set_recorder(ledger::record_deprecation);
    if let Some(exit) = routine_typo() {
        return exit;
    }
    let cli = Jouet::parse();
    // Hooks too, so a rule's PR probe knows the configured hosts. The config
    // is read only when an unknown host comes up, and an unreadable one names
    // no hosts, so a bad config cannot make a hook fail.
    jouet_github::forge::configure(config::forges);
    // Before the output mode is chosen: a bad JOUET_DISPLAY must not make a
    // hook exit 2, which a harness reads as "block".
    if let Cmd::Hook(args) = &cli.cmd {
        return cmd::hook::run(args).into();
    }
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Audit(args) => cmd::audit::run(&args, &out),
        Cmd::Claim(args) => cmd::claim::run(args, &out),
        Cmd::Commit(args) => cmd::commit::run(&args, &out),
        Cmd::Secrets(args) => cmd::secrets::run(&args, &out),
        Cmd::Diag(args) => cmd::diag::run(&args, &out),
        Cmd::Edit(args) => cmd::edit::run(args, &out),
        Cmd::Graph(args) => cmd::graph::run(&args, &out),
        Cmd::Handoff(args) => cmd::handoff::run_handoff(&args, &out),
        Cmd::Pickup(args) => cmd::handoff::run_pickup(&args, &out),
        Cmd::Sessions(args) => cmd::sessions::run(&args, &out),
        Cmd::ReviewPackage(args) => cmd::review::run(&args, &out),
        Cmd::Stack(args) => cmd::stack::run(args, &out),
        Cmd::Stats(args) => cmd::stats::run(&args, &out),
        Cmd::Pr(args) => return cmd::pr::run(args, &out),
        Cmd::Read(args) => cmd::read::run(&args, &out),
        Cmd::Docs(args) => cmd::docs::run(&args, &out),
        Cmd::Deps(args) => cmd::deps::run(&args, &out),
        Cmd::Search(args) => cmd::search::run(&args, &out),
        Cmd::Test(args) => cmd::test::run(&args, &out),
        Cmd::Wait(args) => cmd::wait::run(args, &out),
        Cmd::Http(args) => cmd::http::run(&args, &out),
        Cmd::Job(args) => cmd::job::run(args, &out),
        Cmd::Ci(args) => cmd::ci::run(args, &out),
        Cmd::Rule(args) => cmd::rule::run(&args, &out),
        Cmd::Run(args) => return cmd::run::run(args, &out),
        Cmd::Wind(args) => cmd::wind::run(&args, &out),
        Cmd::Unwind(args) => cmd::wind::unwind(&args, &out),
        Cmd::Task(args) => cmd::task::run(&args, &out),
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
