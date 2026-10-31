//! How a command prints its result, and the error that stopped it.

use crate::display::Mode;
use crate::exit::Exit;

pub struct Output {
    pub json: bool,
    pub mode: Mode,
}

impl Output {
    pub fn new(json: bool, display: Option<&str>) -> Self {
        match Mode::detect(display) {
            Ok(mode) => {
                crate::deprecation::set_mode(mode);
                Self { json, mode }
            }
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
