//! The exit codes every jouet command shares.
//!
//! `Failed` and `Precondition` are deliberately separate: `Failed` says the
//! work was done and the answer is no (fix the code), `Precondition` says the
//! work was refused before it started (fix the request).

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
    /// Still pending when `--timeout` expired; running the same command
    /// again keeps waiting.
    Timeout,
    /// `pr next` only: the agent has something to do. Outside the shared
    /// table on purpose, as in Kent C. Dodds' `ship-pr`, so a script can
    /// tell "act" and "wait" apart from every error.
    Act,
    /// `pr next` only: waiting on someone or something else, such as CI
    /// or a reviewer.
    Waiting,
}

impl Exit {
    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::Precondition => 4,
            Self::Remote => 5,
            Self::Timeout => 124,
            Self::Act => 10,
            Self::Waiting => 20,
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
            Self::Timeout => "timeout",
            Self::Act => "act",
            Self::Waiting => "waiting",
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.code())
    }
}
