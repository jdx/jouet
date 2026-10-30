//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod cmd;
mod display;
mod edit;
mod exit;
mod guide;
mod ledger;
mod paths;
mod rules;
mod settings;
mod setup;
mod spec;
mod time;
mod tls;
mod verify;
mod wait;
mod wind;

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
    /// Block until a condition holds, then print one summary
    Wait(WaitArgs),
    /// List, test and trust the rules jouet's hooks run
    Rule(RuleArgs),
    /// Notice costly patterns in the hook ledger and draft rules for them
    Wind(WindArgs),
    /// Propose retiring rules that are noisy or always overridden
    Unwind(UnwindArgs),
    /// Print a short usage guide written for agents
    AgentHelp(AgentHelpArgs),
    /// Print jouet's agent skill
    Skill(SkillArgs),
    /// Install jouet's hooks and skill into Claude Code or Codex
    Setup(SetupArgs),
    /// Answer a Claude Code or Codex hook
    #[usage(hide)]
    Hook(HookArgs),
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

fn main() -> ExitCode {
    let cli = Jouet::parse();
    // Before the output mode is chosen: a bad JOUET_DISPLAY must not make a
    // hook exit 2, which a harness reads as "block".
    if let Cmd::Hook(args) = &cli.cmd {
        return cmd::hook::run(args).into();
    }
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Edit(args) => cmd::edit::run(args, &out),
        Cmd::Wait(args) => cmd::wait::run(args, &out),
        Cmd::Rule(args) => cmd::rule::run(&args, &out),
        Cmd::Wind(args) => cmd::wind::run(&args, &out),
        Cmd::Unwind(args) => cmd::wind::unwind(&args, &out),
        Cmd::AgentHelp(args) => cmd::agent_help::run(&args, &out),
        Cmd::Skill(args) => cmd::skill::run(&args, &out),
        Cmd::Setup(args) => cmd::setup::run(&args, &out),
        Cmd::Hook(_) => unreachable!("handled above"),
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
