//! Command implementations, and the output conventions they share.

pub mod audit;
pub mod edit;
pub mod pr;
pub mod threads;
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
        self.error_with_hint(exit, message, None)
    }

    /// Like [`Output::error`], with a hint on what to change.
    pub fn error_with_hint(&self, exit: Exit, message: &str, hint: Option<&str>) -> Exit {
        if self.json {
            let mut error = serde_json::json!({ "code": exit.name(), "message": message });
            if let Some(hint) = hint {
                error["hint"] = hint.into();
            }
            println!("{}", self.mode.json(&serde_json::json!({ "error": error })));
        } else {
            eprintln!("jouet: {message}");
            if let Some(hint) = hint {
                eprintln!("hint: {hint}");
            }
        }
        exit
    }
}
