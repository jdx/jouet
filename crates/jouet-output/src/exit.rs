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
    /// Authentication, network or rate-limit failure talking to GitHub, a
    /// package registry or the server `jouet http` asked.
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
    /// The shared codes, in the order `jouet agent-help` lists them. `Act`
    /// and `Waiting` are left out: only `pr next` uses them.
    pub const ALL: [Self; 7] = [
        Self::Ok,
        Self::Failed,
        Self::Usage,
        Self::NotFound,
        Self::Precondition,
        Self::Remote,
        Self::Timeout,
    ];

    pub const fn code(self) -> u8 {
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

    /// The exit with this code, for a code another jouet command (or a
    /// routine) exited with.
    pub fn from_code(code: i32) -> Option<Self> {
        [
            Self::Ok,
            Self::Failed,
            Self::Usage,
            Self::NotFound,
            Self::Precondition,
            Self::Remote,
            Self::Timeout,
            Self::Act,
            Self::Waiting,
        ]
        .into_iter()
        .find(|e| i32::from(e.code()) == code)
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

    /// What the code tells an agent to do next.
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Ok => "done",
            Self::Failed => "the answer is no: fix the code",
            Self::Usage => "bad arguments",
            Self::NotFound => "no such file, PR, thread or package",
            Self::Precondition => "refused before doing anything: fix the request",
            Self::Remote => {
                "GitHub, a registry or a server could not be reached or refused: check auth, then retry"
            }
            Self::Timeout => "still pending: run the same command again to keep waiting",
            Self::Act => "the agent has something to do: run the suggested command",
            Self::Waiting => "waiting on CI, a reviewer or GitHub: wait, then ask again",
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.code())
    }
}
