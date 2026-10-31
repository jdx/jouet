use usage_rs::Args;

/// Print jouet's agent skill
///
/// A short skill that says when to reach for jouet and points at `jouet
/// agent-help` for the flags. `jouet setup` installs it.
#[derive(Args)]
pub(crate) struct SkillArgs {
    /// claude or codex (a SKILL.md), or agents-md (a snippet for AGENTS.md)
    #[usage(long, value_name = "FORMAT")]
    pub(crate) format: Option<String>,
    /// Print the optional skill for proposing rules and ideas to jouet instead
    #[usage(long)]
    pub(crate) contribute: bool,
}
