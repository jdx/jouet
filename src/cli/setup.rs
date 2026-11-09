use usage_rs::Args;

/// Install jouet's hooks into a coding agent's harness
///
/// Merges hooks into Claude Code's settings.json, Codex's hooks.json,
/// Cursor's hooks.json, Gemini CLI's settings.json or Copilot's
/// hooks/jouet.json, keeping every other hook. The session-start hook primes each session with
/// the commands and rules, so the skill, which says the same, is installed
/// only with --skill or when priming is off ([hooks] prime). Running it again
/// changes nothing. The jouet-init project onboarding skill is always installed;
/// --uninstall removes only what jouet added. The hooks do
/// nothing where jouet is not on PATH.
///
/// For Claude Code, setup also offers a context budget from your own recent
/// sessions: a compaction window when they run on a 1M-token model, and
/// denying what none of them used. Asked at a terminal, otherwise only
/// noted: nothing is written without --compact-window or --deny-unused, and
/// --undo takes each back.
#[derive(Args)]
pub(crate) struct SetupArgs {
    /// claude, codex, cursor, gemini or copilot
    pub(crate) harness: String,
    /// Install for every project, under your home directory (the default)
    #[usage(long)]
    pub(crate) global: bool,
    /// Install into this repository for you alone: Claude Code's untracked
    /// .claude/settings.local.json
    #[usage(long)]
    pub(crate) project: bool,
    /// Install into the repository's tracked hooks file (.claude/settings.json,
    /// .codex/hooks.json, .cursor/hooks.json, .gemini/settings.json or
    /// .github/hooks/jouet.json), for everyone who clones it (implies
    /// --project)
    #[usage(long)]
    pub(crate) shared: bool,
    /// Show what would change without writing anything
    #[usage(long)]
    pub(crate) dry_run: bool,
    /// Remove jouet's hooks and skills
    #[usage(long)]
    pub(crate) uninstall: bool,
    /// Install the skill as well, which the session-start primer otherwise
    /// stands in for
    #[usage(long)]
    pub(crate) skill: bool,
    /// Print a prompt for an agent to benchmark jouet on this project
    #[usage(long)]
    pub(crate) benchmark: bool,
    /// Claude Code: compact a conversation once its context reaches TOKENS
    /// (100k to 1m), by setting autoCompactWindow. It pays on a 1M-token
    /// model; a 200k one already compacts there
    #[usage(long, value_name = "TOKENS")]
    pub(crate) compact_window: Option<String>,
    /// Claude Code: deny the tools and MCP servers that none of your last 50
    /// sessions called, and hide from the model the skills none used (still
    /// there as /name)
    #[usage(long)]
    pub(crate) deny_unused: bool,
    /// Claude Code: take back what --compact-window (window) or
    /// --deny-unused (unused) added, and nothing else
    #[usage(long, value_name = "CHOICE", choices("window", "unused"))]
    pub(crate) undo: Option<String>,
}
