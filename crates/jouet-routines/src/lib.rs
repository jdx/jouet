//! Routines: small Luau programs written on jouet's primitives, run by
//! `joue`.
//!
//! A routine composes primitives the way a shell script composes commands,
//! but over structured values: the authenticated GitHub client, git state,
//! the wait engine, all-or-nothing edits, the hooks' ledger and mise. Each
//! one declares what it reaches in its header ([`routine`]), and the
//! runtime refuses anything else ([`Capability`]).
//!
//! The guardrails:
//! - A primitive outside the routine's capabilities fails with exit 4.
//! - A routine with a capability that writes acts only with `--yes`.
//!   Without it every write is previewed instead of made, and the run ends
//!   with exit 4 (or 0 under `--dry-run`), listing what it would have done.
//!   Such a routine must carry examples, run by `joue test` against
//!   recorded responses, or it refuses to run.
//! - The Luau between two primitive calls runs under a budget of time,
//!   instructions and memory; a primitive's own time is not counted.
//! - A repository's routines run only once it is trusted.

pub mod capability;
mod help;
mod json;
pub mod prims;
pub mod routine;
pub mod runtime;
pub mod testing;
pub mod world;

#[cfg(test)]
mod parity;

pub use capability::Capability;
pub use routine::{ArgError, Args, Catalog, Origin, Routine, Source, State};
pub use runtime::{Options, Outcome};
pub use world::{Host, StepOutput, World};

use jouet_output::exit::Exit;
use std::fmt;

/// The Luau type definitions of the primitives, for type-checking routines.
pub const TYPES: &str = include_str!("../routines.d.luau");

/// Why a routine stopped: the exit code, what went wrong, and what to
/// change. Primitives raise it, and so does `joue.fail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

impl Fail {
    pub fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// As an error a Luau callback can raise.
    pub(crate) fn raise(self) -> mlua::Error {
        mlua::Error::external(self)
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Fail {}
