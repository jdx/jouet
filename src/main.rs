//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod cmd;
mod display;
mod edit;
mod exit;
mod paths;

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

fn main() -> ExitCode {
    let cli = Jouet::parse();
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Edit(args) => cmd::edit::run(args, &out),
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
