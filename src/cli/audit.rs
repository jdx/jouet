use usage_rs::Args;

/// Report where coding-agent sessions spend tokens and time
///
/// Reads Claude Code's transcripts (~/.claude/projects) and Codex's rollouts
/// (~/.codex/sessions and archived_sessions) on this machine, and ranks what
/// their requests spent input tokens on: CI polling, process polling, PR
/// threads, PR status, edits, builds and tests, reads and searches, git and
/// more. For each costly category it names the jouet command or rule that
/// addresses it, with an estimate of the saving, and it counts recurring
/// patterns such as sleep loops. Nothing leaves the machine. Commands are
/// shown only as shapes, with every value replaced, unless --examples asks
/// for redacted examples.
#[derive(Args)]
pub(crate) struct AuditArgs {
    /// Only requests made since TIME: a duration ago such as 7d or 12h, a
    /// date such as 2026-10-01, or RFC 3339
    #[usage(long, value_name = "TIME")]
    pub(crate) since: Option<String>,
    /// Which transcripts to read: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    pub(crate) harness: Option<String>,
    /// Only sessions that ran in PATH or a directory below it
    #[usage(long, value_name = "PATH")]
    pub(crate) project: Option<String>,
    /// Categories to list per harness in text (default 10; 0 lists all).
    /// --json lists every category
    #[usage(long, value_name = "N")]
    pub(crate) top: Option<usize>,
    /// Show one redacted example command per shape and pattern
    #[usage(long)]
    pub(crate) examples: bool,
}
