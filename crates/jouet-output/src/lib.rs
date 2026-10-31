//! The conventions every jouet command shares: exit codes, who the output is
//! for, how a result or an error is printed, how times are read and shown,
//! where files that outlive a command are kept, and how a deprecation is
//! announced.

pub mod deprecation;
pub mod display;
pub mod exit;
mod output;
pub mod paths;
pub mod time;

pub use output::Output;
