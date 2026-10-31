use usage_rs::{Args, Subcommands};

/// Run a routine: a Luau program on jouet's primitives (also `joue`)
///
/// Routines are Luau programs, where rules are TOML configuration.
/// `joue NAME [ARGS]` is the same as `jouet run NAME [ARGS]`. Routines are
/// built in (land, pr-next, ship-check, sweep), in /etc/jouet/routines, in
/// ~/.config/jouet/routines, in a repository's .jouet/routines once it is
/// trusted (`jouet trust`), and in plugins, named after the plugin:
/// `joue acme/deploy`.
/// Each declares its arguments, so `joue NAME --help` says what it takes,
/// and the capabilities it reaches; anything else is refused (exit 4). A
/// routine that writes acts only with --yes: without it every write is
/// previewed, and joue exits 4 (0 with --dry-run).
#[derive(Args)]
pub(crate) struct RunArgs {
    /// List every routine, where it comes from and what it reaches
    #[usage(long)]
    pub(crate) list: bool,
    #[usage(subcommand)]
    pub(crate) cmd: Option<RunCmd>,
}

#[derive(Subcommands)]
pub(crate) enum RunCmd {
    /// Run routines' examples against their recorded responses
    Test(RunTestArgs),
    /// Print the Luau type definitions of the primitives
    Types(RunTypesArgs),
    #[usage(external_subcommand)]
    External(Vec<std::ffi::OsString>),
}

/// Run routines' examples against their recorded responses
///
/// Each example runs the routine with its arguments against a fixture: a
/// local fake GitHub serves its recorded responses, and jouet's commands,
/// git, mise and the ledger answer from it too. Nothing real is reached,
/// and file edits are only previewed. Fails (exit 1) when an example's exit
/// code, output, JSON, previewed writes or requests differ from what it
/// expects, or when a routine has no examples.
#[derive(Args)]
pub(crate) struct RunTestArgs {
    /// The routine (default: every one)
    pub(crate) routine: Option<String>,
}

/// Print the Luau type definitions of the primitives
#[derive(Args)]
pub(crate) struct RunTypesArgs {}
