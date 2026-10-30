//! Command implementations, and the output conventions they share.

pub mod agent_help;
pub mod edit;
pub mod hook;
pub mod rule;
pub mod setup;
pub mod skill;
pub mod wait;

use crate::display::Mode;
use crate::exit::Exit;

pub struct Output {
    pub json: bool,
    pub mode: Mode,
}

impl Output {
    /// Prints a command's result to stdout.
    pub fn result<T: serde::Serialize>(&self, value: &T, text: impl FnOnce() -> String) {
        if self.json {
            println!("{}", self.mode.json(value));
        } else {
            print!("{}", text());
        }
    }

    /// Reports an error that stopped a command: as a JSON document on stdout
    /// with --json, so `jouet ... --json | jq` always gets one document, and
    /// as text on stderr otherwise.
    pub fn error(&self, exit: Exit, message: &str) -> Exit {
        if self.json {
            let doc = serde_json::json!({ "error": { "code": exit.name(), "message": message } });
            println!("{}", self.mode.json(&doc));
        } else {
            eprintln!("jouet: {message}");
        }
        exit
    }
}
