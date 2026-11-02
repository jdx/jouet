//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: commands land one at a time.

mod affected;
mod ci;
mod cli;
mod cmd;
mod config;
mod cost;
mod db;
mod deps;
mod diag;
mod docs;
mod entire;
mod fnox;
mod git;
mod guide;
mod harness;
mod known;
mod ledger;
mod mise;
mod plugins;
mod reads;
mod redact;
mod rules;
mod settings;
mod setup;
mod share;
mod sources;
mod spec;
mod testlog;
mod verify;
mod wind;

use cli::{Cmd, Jouet};
use std::process::ExitCode;

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
    let entire = settings::Settings::load().entire;
    jouet_github::entire::configure(
        entire.enabled,
        std::time::Duration::from_millis(entire.timeout_ms),
    );
    let out = cmd::Output::new(cli.json, cli.display.as_deref());
    let exit = match cli.cmd {
        Cmd::Init(args) => cmd::init::run(&args, &out),
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
        Cmd::Cost(args) => cmd::cost::run(&args, &out),
        Cmd::Pr(args) => return cmd::pr::run(args, &out),
        Cmd::Read(args) => cmd::read::run(&args, &out),
        Cmd::Docs(args) => cmd::docs::run(&args, &out),
        Cmd::Deps(args) => cmd::deps::run(&args, &out),
        Cmd::Db(args) => cmd::db::run(&args, &out),
        Cmd::Search(args) => cmd::search::run(&args, &out),
        Cmd::Test(args) => cmd::test::run(&args, &out),
        Cmd::Wait(args) => cmd::wait::run(args, &out),
        Cmd::Http(args) => cmd::http::run(&args, &out),
        Cmd::Job(args) => cmd::job::run(args, &out),
        Cmd::Ci(args) => cmd::ci::run(args, &out),
        Cmd::Rule(args) => cmd::rule::run(&args, &out),
        Cmd::Trust(args) => cmd::trust::trust(&args, &out),
        Cmd::Untrust(_) => cmd::trust::untrust(&out),
        Cmd::Plugin(args) => cmd::plugin::run(&args, &out),
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
