use usage_rs::{Args, Subcommands};

/// Add, remove, list and fetch plugins: rules and routines pinned by commit
///
/// A plugin is a git repository, or a directory, with a jouet-plugin.toml
/// (its name, description and the oldest jouet it works with) and rules/,
/// routines/ or both. Its rules and routines are named after it:
/// `acme/no-prod-db`, `joue acme/deploy`. `add` pins a git plugin at a full
/// commit and the hash of every file it ships, in ~/.config/jouet/plugins.toml,
/// or with --repo in this repository's .jouet/plugins.toml (which then
/// needs trusting again). Every hook checks the kept files against the
/// hash, and never fetches; `sync` fetches the pins missing or changed.
/// /etc/jouet/plugins.toml and /etc/jouet/plugins/ hold enforced plugins.
#[derive(Args)]
pub(crate) struct PluginArgs {
    #[usage(subcommand)]
    pub(crate) cmd: PluginCmd,
}

#[derive(Subcommands)]
pub(crate) enum PluginCmd {
    /// Fetch a plugin, run its rules' examples and pin it
    Add(PluginAddArgs),
    /// Unpin a plugin
    Rm(PluginRmArgs),
    /// List every pinned plugin and whether it is kept as pinned
    Ls(PluginLsArgs),
    /// Fetch every pinned plugin that is missing or does not match its pin
    Sync(PluginSyncArgs),
}

/// Fetch a plugin, run its rules' examples and pin it
///
/// SOURCE is a git URL, or a directory. A git plugin is pinned at the full
/// commit --rev names (default: the remote's HEAD), and `git rev-parse`
/// checks the commit fetched is that one. A directory is copied and pinned
/// by the hash of its files; give --rev to fetch it as a git repository.
/// Its name is the one its jouet-plugin.toml gives. Every rule must pass
/// its examples of both kinds, or nothing is pinned (exit 1). Like
/// trusting, it needs a terminal or --yes, and an agent is denied it.
#[derive(Args)]
pub(crate) struct PluginAddArgs {
    /// A git URL, or a directory
    pub(crate) source: String,
    /// The commit, tag or branch to pin, resolved to a full commit
    #[usage(long, value_name = "REV")]
    pub(crate) rev: Option<String>,
    /// The plugin's directory within the source, when it is not the root
    #[usage(long, value_name = "DIR")]
    pub(crate) dir: Option<String>,
    /// Pin it in this repository's .jouet/plugins.toml rather than yours
    #[usage(long)]
    pub(crate) repo: bool,
    /// Pin it without a terminal, for a script or CI run by the user
    #[usage(long)]
    pub(crate) yes: bool,
}

/// Unpin a plugin
#[derive(Args)]
pub(crate) struct PluginRmArgs {
    /// The plugin's name
    pub(crate) name: String,
    /// Unpin it from this repository's .jouet/plugins.toml rather than yours
    #[usage(long)]
    pub(crate) repo: bool,
}

/// List every pinned plugin and whether it is kept as pinned
#[derive(Args)]
pub(crate) struct PluginLsArgs {}

/// Fetch every pinned plugin that is missing or does not match its pin
///
/// Only the pins of sets that run: yours, the system's, and this
/// repository's once trusted. Each fetch must give the pinned commit and
/// hash, or it fails (exit 5).
#[derive(Args)]
pub(crate) struct PluginSyncArgs {}
