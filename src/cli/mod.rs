//! The command line, declared with usage-rs: the root, the subcommand enum, and
//! one module of argument types per command. Help text lives in the doc
//! comments here; `docs/cli` and the shell completions are generated from them.

use usage_rs::{Cli, Subcommands};

mod agent_help;
mod audit;
mod ci;
mod claim;
mod commit;
mod completion;
mod db;
mod deps;
mod diag;
mod docs;
mod edit;
mod graph;
mod handoff;
mod hook;
mod http;
mod job;
mod plugin;
mod pr;
mod read;
mod review_package;
mod rule;
mod run;
mod search;
mod secrets;
mod sessions;
mod setup;
mod share;
mod skill;
mod stack;
mod stats;
mod task;
mod test;
mod threads;
mod trust;
mod wait;
mod wind;

pub(crate) use agent_help::*;
pub(crate) use audit::*;
pub(crate) use ci::*;
pub(crate) use claim::*;
pub(crate) use commit::*;
pub(crate) use completion::*;
pub(crate) use db::*;
pub(crate) use deps::*;
pub(crate) use diag::*;
pub(crate) use docs::*;
pub(crate) use edit::*;
pub(crate) use graph::*;
pub(crate) use handoff::*;
pub(crate) use hook::*;
pub(crate) use http::*;
pub(crate) use job::*;
pub(crate) use plugin::*;
pub(crate) use pr::*;
pub(crate) use read::*;
pub(crate) use review_package::*;
pub(crate) use rule::*;
pub(crate) use run::*;
pub(crate) use search::*;
pub(crate) use secrets::*;
pub(crate) use sessions::*;
pub(crate) use setup::*;
pub(crate) use share::*;
pub(crate) use skill::*;
pub(crate) use stack::*;
pub(crate) use stats::*;
pub(crate) use task::*;
pub(crate) use test::*;
pub(crate) use threads::*;
pub(crate) use trust::*;
pub(crate) use wait::*;
pub(crate) use wind::*;

// `joue` is a view of `jouet run`: argv[0] `joue` parses as `jouet run`,
// with --json and --display carried over, so `joue NAME` runs a routine.
#[derive(Cli)]
#[usage(
    name = "jouet",
    bin = "jouet",
    version,
    about = "A toolkit CLI for coding agents",
    unknown_flags = "error",
    completion = true,
    view("joue", root = "run", globals)
)]
pub(crate) struct Jouet {
    /// Print one JSON document instead of text
    #[usage(long, global)]
    pub(crate) json: bool,
    /// Who the output is for: agent, human or plain. Detected when not given
    #[usage(long, global, value_name = "MODE")]
    pub(crate) display: Option<String>,
    #[usage(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommands)]
pub(crate) enum Cmd {
    /// Report where coding-agent sessions spend tokens and time
    Audit(AuditArgs),
    /// Claim paths for this session, so other sessions are warned off them
    Claim(ClaimArgs),
    /// Commit only the named paths, with trailers
    Commit(CommitArgs),
    /// Find secrets in files, in what is staged, or in commits, with gitleaks' rules
    Secrets(SecretsArgs),
    /// Report the errors and warnings in files you changed, through hk
    Diag(DiagArgs),
    /// Make exact, all-or-nothing edits across files
    Edit(EditArgs),
    /// Query the code graph: symbols, callers, impact and affected tests
    Graph(GraphArgs),
    /// Record the facts about this checkout for the next session
    Handoff(HandoffArgs),
    /// Print a handoff and check that it still holds
    Pickup(PickupArgs),
    /// Show which agent sessions are working where on this machine
    Sessions(SessionsArgs),
    /// Show what jouet saved: measured from its ledger, estimated from your history
    Stats(StatsArgs),
    /// Inspect pull requests
    Pr(PrArgs),
    /// Read a file by its outline, a symbol or a range, or the repository's map
    Read(ReadArgs),
    /// Look up a dependency's API at the version the project locked
    Docs(DocsArgs),
    /// Check a package before adding it, or the locked dependencies, against its registry and OSV
    Deps(DepsArgs),
    /// Read a database's schema, or run a read-only query, in SQLite or Postgres
    Db(DbArgs),
    /// Write one file for reviewing a range of commits
    ReviewPackage(ReviewPackageArgs),
    /// Check a stack of PRs: does each layer really contain the one below
    Stack(StackArgs),
    /// Search files like rg, within a budget, grouped by symbol
    Search(SearchArgs),
    /// Run the test suite, or only the tests a change affects
    Test(TestArgs),
    /// Block until a condition holds, then print one summary
    Wait(WaitArgs),
    /// Send an HTTP request and print the status and only the JSON paths asked for, within a budget
    Http(HttpArgs),
    /// Run one-off background work, such as a build or a server for one check
    Job(JobArgs),
    /// Run GitHub Actions jobs here, or say whether a failure on GitHub is flaky
    Ci(CiArgs),
    /// List and test the rules jouet's hooks run
    Rule(RuleArgs),
    /// Trust this repository's rules, routines and plugins as they are now
    Trust(TrustArgs),
    /// Stop trusting this repository's rules, routines and plugins
    Untrust(UntrustArgs),
    /// Add, remove, list and fetch plugins: rules and routines pinned by commit
    Plugin(PluginArgs),
    /// Run a routine: a Luau program on jouet's primitives (also `joue`)
    Run(RunArgs),
    /// Notice costly patterns in the hook ledger and whole sessions, and propose fixes
    Wind(WindArgs),
    /// Propose retiring rules that are noisy or always overridden, and unused tasks
    Unwind(UnwindArgs),
    /// Propose mise tasks for commands agents keep repeating, and add one
    Task(TaskArgs),
    /// Propose a proven rule, a missing primitive, or a fix to jouet itself to its maintainers
    Share(ShareArgs),
    /// Print a short usage guide written for agents
    AgentHelp(AgentHelpArgs),
    /// Print jouet's agent skill
    Skill(SkillArgs),
    /// Install jouet's hooks into a coding agent's harness
    Setup(SetupArgs),
    /// Print a shell completion script
    Completion(CompletionArgs),
    /// Answer a harness's hook
    #[usage(hide)]
    Hook(HookArgs),
}
