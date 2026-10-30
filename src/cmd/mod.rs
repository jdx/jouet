//! Command implementations. The output conventions they share live in
//! `jouet_output`.

pub(crate) mod agent_help;
pub(crate) mod audit;
pub(crate) mod commit;
pub(crate) mod completion;
pub(crate) mod edit;
pub(crate) mod handoff;
pub(crate) mod hook;
pub(crate) mod land;
pub(crate) mod next;
pub(crate) mod pr;
pub(crate) mod review;
pub(crate) mod rule;
pub(crate) mod run;
pub(crate) mod setup;
pub(crate) mod share;
pub(crate) mod skill;
pub(crate) mod stats;
pub(crate) mod test;
pub(crate) mod threads;
pub(crate) mod wait;
pub(crate) mod wind;

pub(crate) use jouet_output::Output;
