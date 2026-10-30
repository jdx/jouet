//! The exit codes every jouet command shares.
//!
//! `Failed` and `Precondition` are deliberately separate: `Failed` says the
//! work was done and the answer is no (fix the code), `Precondition` says the
//! work was refused before it started (fix the request). `remote` (5) and
//! `timeout` (124) join this list with the GitHub commands that return them.

use std::process::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok,
    Failed,
    Usage,
    NotFound,
    Precondition,
}

impl Exit {
    pub const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::Precondition => 4,
        }
    }

    /// The name used for `error.code` in JSON output.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Usage => "usage",
            Self::NotFound => "not_found",
            Self::Precondition => "precondition",
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.code())
    }
}
