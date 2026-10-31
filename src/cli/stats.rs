use usage_rs::Args;

/// Show what jouet saved: measured from its ledger, estimated from your history
///
/// Counts what the hook ledger recorded (jouet command runs, rule decisions
/// and what the agent did after a denial, checks, and failing CI logs
/// returned as excerpts), then estimates the input tokens those runs saved
/// against your own Claude Code and Codex history from before `jouet setup`.
/// Every estimate shows its basis, and a kind of work with fewer than 20 runs
/// before setup is not estimated. Without a ledger, it shows what one run of
/// each command would save.
#[derive(Args)]
pub(crate) struct StatsArgs {
    /// Only ledger entries since TIME: a duration ago such as 7d or 12h, a
    /// date such as 2026-10-01, or RFC 3339
    #[usage(long, value_name = "TIME")]
    pub(crate) since: Option<String>,
    /// Which harness: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    pub(crate) harness: Option<String>,
    /// Only one session's ledger entries
    #[usage(long, value_name = "ID")]
    pub(crate) session: Option<String>,
    /// Print one line about what jouet saved, or nothing, for a stop hook.
    /// `[stats] summary` in the config file or JOUET_STATS_SUMMARY sets its
    /// style: quips (default), plain or off
    #[usage(long)]
    pub(crate) summary: bool,
}
