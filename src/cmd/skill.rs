//! `jouet skill`: a thin skill that says when to reach for jouet and leaves
//! the flags to `jouet agent-help`, so it never has to be updated by hand.

use super::Output;
use crate::SkillArgs;
use crate::guide;
use jouet_output::exit::Exit;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Claude,
    Codex,
    AgentsMd,
}

impl Format {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agents-md" => Some(Self::AgentsMd),
            _ => None,
        }
    }
}

pub(crate) fn run(args: &SkillArgs, out: &Output) -> Exit {
    let name = args.format.as_deref().unwrap_or("claude");
    let Some(format) = Format::parse(name) else {
        return out.error(
            Exit::Usage,
            &format!("--format must be claude, codex or agents-md, not `{name}`"),
        );
    };
    let text = render(format);
    out.result(&serde_json::json!({ "text": text }), || text.clone());
    Exit::Ok
}

/// The skill file. Claude Code and Codex read the same `SKILL.md` shape; the
/// AGENTS.md snippet is the body under a heading, for harnesses without
/// skills.
pub(crate) fn render(format: Format) -> String {
    let commands = guide::agent_commands();
    let mut body = String::new();
    body.push_str(
        "`jouet` is a CLI toolkit for coding agents, installed on this machine. Its commands replace scripts agents keep rebuilding by hand, with compact output, `--json` everywhere and one exit-code table.\n\n",
    );
    body.push_str("When to reach for it:\n\n");
    for (meta, when) in &commands {
        let _ = writeln!(body, "- `jouet {}`: {when}.", meta.cmd.name);
    }
    body.push_str(
        "\nHow:\n\n\
         - Run `jouet agent-help` for the installed version's commands, flags and exit codes, and `jouet agent-help COMMAND` for one command. Work from that, not from memory.\n\
         - Exit 1 means the answer is no (fix the code); exit 4 means jouet refused before doing anything (fix the request).\n\
         - When a jouet hook denies a command, use the command it suggests instead of rewording yours to get past it. If the denial is wrong, tell the user and name the rule.\n",
    );
    match format {
        Format::Claude | Format::Codex => {
            let abouts: Vec<String> = commands
                .iter()
                .map(|(meta, _)| meta.about.unwrap_or("").to_lowercase())
                .collect();
            format!(
                "---\nname: jouet\ndescription: Use the jouet CLI to {} instead of hand-rolled scripts. Run `jouet agent-help` for its current commands and flags.\n---\n\n# jouet\n\n{body}",
                abouts.join("; ")
            )
        }
        Format::AgentsMd => format!("## jouet\n\n{body}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_is_thin_and_points_at_agent_help() {
        let skill = render(Format::Claude);
        assert!(
            skill.starts_with("---\nname: jouet\ndescription: "),
            "{skill}"
        );
        assert!(skill.contains("`jouet agent-help`"), "{skill}");
        assert!(skill.contains("- `jouet edit`: "), "{skill}");
        assert!(skill.lines().count() <= 25, "{skill}");
        // Flags belong to agent-help.
        assert!(!skill.contains("--expect-hash"), "{skill}");
    }

    #[test]
    fn the_agents_md_snippet_has_no_frontmatter() {
        let snippet = render(Format::AgentsMd);
        assert!(snippet.starts_with("## jouet\n"), "{snippet}");
    }
}
