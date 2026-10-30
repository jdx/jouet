//! The exit codes every jouet command shares.
//!
//! `Failed` and `Precondition` are deliberately separate: `Failed` says the
//! work was done and the answer is no (fix the code), `Precondition` says the
//! work was refused before it started (fix the request). `timeout` (124)
//! joins this list with the command that returns it.

use std::process::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok,
    Failed,
    Usage,
    NotFound,
    Precondition,
    /// Authentication, network or rate-limit failure talking to the host.
    Remote,
}

impl Exit {
    pub const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::Precondition => 4,
            Self::Remote => 5,
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
            Self::Remote => "remote",
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.code())
    }
}
