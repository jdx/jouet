use usage_rs::Args;

/// Watch the PR a push went to, from a harness's background hook
///
/// Reads a PostToolUse hook's JSON on stdin. For a push that worked, in a
/// session in the settle arm with no watcher on the PR yet, it waits as
/// `jouet wait pr --until needs-agent` does, then exits 2 with the digest on
/// stderr, which wakes the agent. Anything else exits 0 at once. Installed
/// by `jouet setup claude`.
#[derive(Args)]
pub(crate) struct WatchArgs {
    /// claude
    pub(crate) harness: Option<String>,
    /// How long nothing may change before the PR counts as settled (default
    /// 20m, or 5m once every check has finished)
    #[usage(long, value_name = "DUR")]
    pub(crate) quiet_for: Option<String>,
    /// Give up after DUR (default 24h)
    #[usage(long, value_name = "DUR")]
    pub(crate) timeout: Option<String>,
    /// Time between the first checks (default 10s)
    #[usage(long, value_name = "DUR")]
    pub(crate) interval: Option<String>,
}
