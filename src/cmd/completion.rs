//! `jouet completion`: the shell completion script, generated from the same
//! CLI definition as the parser, so it cannot drift from the commands.

use super::Output;
use crate::Jouet;
use crate::cli::CompletionArgs;
use jouet_output::exit::Exit;
use usage_rs::complete::Shell;

pub(crate) fn run(args: &CompletionArgs, out: &Output) -> Exit {
    // The parser's `choices` reject any name `Shell::from_name` does not
    // know, so this is unreachable from the command line.
    let Some(shell) = Shell::from_name(&args.shell) else {
        return out.error(Exit::Usage, &format!("unsupported shell `{}`", args.shell));
    };
    // `joue` is a view of `jouet run`, and jouet answers for a command line
    // that starts with it, so its script still asks jouet.
    let script = if args.joue {
        Jouet::completion_script_for_alias("joue", shell)
    } else {
        Jouet::completion_script(shell)
    };
    out.result(
        &serde_json::json!({ "shell": shell.as_str(), "script": script }),
        || script.clone(),
    );
    Exit::Ok
}
