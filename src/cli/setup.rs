use usage_rs::Args;

/// Install jouet's hooks into a coding agent's harness
///
/// Merges hooks into Claude Code's settings.json, Codex's hooks.json,
/// Cursor's hooks.json, Gemini CLI's settings.json or Copilot's
/// hooks/jouet.json, keeping every other hook. The session-start hook primes each session with
/// the commands and rules, so the skill, which says the same, is installed
/// only with --skill or when priming is off ([hooks] prime). Running it again
/// changes nothing; --uninstall removes only what jouet added. The hooks do
/// nothing where jouet is not on PATH.
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
    /// Remove jouet's hooks and skill
    #[usage(long)]
    pub(crate) uninstall: bool,
    /// Install the skill as well, which the session-start primer otherwise
    /// stands in for
    #[usage(long)]
    pub(crate) skill: bool,
}
