//! `jouet agent-help [COMMAND]`.

use super::Output;
use crate::AgentHelpArgs;
use crate::guide;
use jouet_output::exit::Exit;

pub(crate) fn run(args: &AgentHelpArgs, out: &Output) -> Exit {
    if args.command.is_empty() {
        let text = guide::overview();
        out.result(&serde_json::json!({ "text": text }), || text.clone());
        return Exit::Ok;
    }
    let (Some(short), Some(full)) = (guide::command(&args.command), guide::full(&args.command))
    else {
        return out.error(
            Exit::NotFound,
            &format!("jouet has no `{}` command", args.command.join(" ")),
        );
    };
    // The JSON document carries the whole guide, and the usage block when
    // the command has one.
    let mut doc = serde_json::json!({ "text": full });
    if short != full {
        doc["usage"] = short.clone().into();
    }
    let text = if args.full { &full } else { &short };
    out.result(&doc, || text.clone());
    Exit::Ok
}
