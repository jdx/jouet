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
    let text = if args.contribute {
        contribute(format)
    } else {
        render(format)
    };
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
         - Run `jouet agent-help` for the installed version's commands, flags and exit codes, and `jouet agent-help COMMAND` for one command. Work from that, not from memory. Exit 1 means the answer is no (fix the code); exit 4 means jouet refused before doing anything (fix the request).\n\
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

/// The optional skill for proposing rules and ideas to jouet's maintainers.
/// Nothing installs it; a user who wants their agent to contribute saves it
/// beside the jouet skill.
pub(crate) fn contribute(format: Format) -> String {
    let body = "Contributing to jouet is optional, and a person approves every post. Use this skill only when the user has asked for it.\n\n\
When a contribution is worth making:\n\n\
- A rule of yours has proven itself: it has denied for a week, fires often and is rarely overridden, and it would help other people, not only this machine or project. `jouet share NAME` checks all of this and says what is missing.\n\
- jouet's rule API could not express a rule you wanted. jouet records it when a rule reads a `jouet` primitive that does not exist; record anything else with `jouet rule gap \"I kept doing X; there is no primitive for Y\"`. `jouet share --idea` turns recorded gaps into an idea.\n\
- jouet itself misbehaved or fell short: it crashed, exited with a code outside its table, printed malformed JSON, its flags kept causing usage errors, agents dropped its output for raw `gh` or `git`, a built-in rule kept being overridden, a hook ran over its time budget, or the same script was written in many sessions. jouet records these from the ledger and from session digests; `jouet share --list` shows the ones ready to share, and `jouet share --idea KEY` drafts a bug as an Issue and anything else as a Discussion.\n\n\
How:\n\n\
1. Run `jouet share NAME` (or `jouet share --idea`). It posts nothing: it prints the exact text, with paths, repository names, hostnames and emails replaced, and a diff of what that changed.\n\
2. Show the user that text and the diff. Look for internal tool or project names that are still there, and add `--redact WORD` for each.\n\
3. Post with `--yes` only after the user says yes to that exact text in this conversation. It posts with the user's own GitHub login, a bug as an Issue on jdx/jouet and anything else to its \"Rules & ideas\" Discussions category, and refuses duplicates, secrets and more than three posts a week.\n\n\
Never post on your own initiative, never post several at once, and never reply in other Discussions or Issues on the user's behalf.\n";
    match format {
        Format::Claude | Format::Codex => format!(
            "---\nname: jouet-contribute\ndescription: Propose a proven jouet rule, a primitive jouet's rule API lacks, or a bug or gap in jouet itself to jouet's maintainers with `jouet share`, only when the user asks and approves the exact post.\n---\n\n# Contributing to jouet\n\n{body}"
        ),
        Format::AgentsMd => format!("## Contributing to jouet\n\n{body}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contributor_skill_puts_a_person_before_every_post() {
        let skill = contribute(Format::Claude);
        assert!(
            skill.starts_with("---\nname: jouet-contribute\n"),
            "{skill}"
        );
        assert!(skill.contains("only after the user says yes"), "{skill}");
        assert!(skill.contains("`jouet share --idea`"), "{skill}");
        assert!(contribute(Format::AgentsMd).starts_with("## Contributing to jouet\n"));
    }

    #[test]
    fn the_skill_is_thin_and_points_at_agent_help() {
        let skill = render(Format::Claude);
        assert!(
            skill.starts_with("---\nname: jouet\ndescription: "),
            "{skill}"
        );
        assert!(skill.contains("`jouet agent-help`"), "{skill}");
        assert!(skill.contains("- `jouet edit`: "), "{skill}");
        // The cap is a budget to defend, not a ceiling to fill.
        assert!(skill.lines().count() <= 30, "{skill}");
        // Flags belong to agent-help.
        assert!(!skill.contains("--expect-hash"), "{skill}");
    }

    #[test]
    fn the_agents_md_snippet_has_no_frontmatter() {
        let snippet = render(Format::AgentsMd);
        assert!(snippet.starts_with("## jouet\n"), "{snippet}");
    }
}
