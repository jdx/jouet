use usage_rs::Args;

/// Trust this repository's rules, routines and plugins as they are now
///
/// Covers .jouet/rules, .jouet/routines and the plugins .jouet/plugins.toml
/// pins, together. Prints the rules, fetches the pinned plugins, and runs
/// every rule's examples; trusts nothing (exit 1) if one fails, lacks
/// either kind or does not load. Trust is kept against a hash of every file
/// in them: a change on a pull or another branch needs trusting again, and
/// going back to files trusted before does not. Trusting is the user's
/// decision: without a terminal it is refused (exit 4) unless --yes is
/// given, and the built-in `trust` rule denies it to an agent.
#[derive(Args)]
pub(crate) struct TrustArgs {
    /// Trust without a terminal, for a script or CI run by the user
    #[usage(long)]
    pub(crate) yes: bool,
    /// List every directory trusted, and whether this repository is
    #[usage(long)]
    pub(crate) list: bool,
}

/// Stop trusting this repository's rules, routines and plugins
#[derive(Args)]
pub(crate) struct UntrustArgs {}
