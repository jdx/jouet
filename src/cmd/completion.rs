//! `jouet completion`: the shell completion script, generated from the same
//! CLI definition as the parser, so it cannot drift from the commands.

use super::Output;
use crate::{CompletionArgs, Jouet};
use jouet_output::exit::Exit;
use usage_rs::complete::Shell;

pub(crate) fn run(args: &CompletionArgs, out: &Output) -> Exit {
    // The parser's `choices` reject any name `Shell::from_name` does not
    // know, so this is unreachable from the command line.
    let Some(shell) = Shell::from_name(&args.shell) else {
        return out.error(Exit::Usage, &format!("unsupported shell `{}`", args.shell));
    };
    let script = Jouet::completion_script(shell);
    out.result(
        &serde_json::json!({ "shell": shell.as_str(), "script": script }),
        || script.clone(),
    );
    Exit::Ok
}
