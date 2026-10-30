//! `jouet agent-help [COMMAND]`.

use super::Output;
use crate::AgentHelpArgs;
use crate::exit::Exit;
use crate::guide;

pub fn run(args: &AgentHelpArgs, out: &Output) -> Exit {
    let text = if args.command.is_empty() {
        guide::overview()
    } else {
        match guide::command(&args.command) {
            Some(text) => text,
            None => {
                return out.error(
                    Exit::NotFound,
                    &format!("jouet has no `{}` command", args.command.join(" ")),
                );
            }
        }
    };
    out.result(&serde_json::json!({ "text": text }), || text.clone());
    Exit::Ok
}
