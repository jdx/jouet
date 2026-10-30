//! Command implementations. The output conventions they share live in
//! `jouet_output`.

pub(crate) mod agent_help;
pub(crate) mod audit;
pub(crate) mod edit;
pub(crate) mod hook;
pub(crate) mod pr;
pub(crate) mod rule;
pub(crate) mod setup;
pub(crate) mod skill;
pub(crate) mod threads;
pub(crate) mod wait;

pub(crate) use jouet_output::Output;
