//! Which agent is running, as far as the environment says, and the commit
//! trailers that follow from it.
//!
//! Nothing here is guessed. The tool and its version come from `AI_AGENT`
//! (`claude-code_2-1-309_agent` is Claude Code 2.1.309), the tool alone from
//! `mise-agent-env`'s detection when `AI_AGENT` is not set, and the model only
//! from `JOUET_AI_MODEL`, since no harness exports the model it runs. A value
//! the environment does not give is `unavailable`.

use std::ffi::OsString;

const UNAVAILABLE: &str = "unavailable";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agent {
    /// The agent's stable identifier, such as `claude-code` or `codex`.
    pub id: Option<String>,
    pub version: Option<String>,
    /// `provider/model`, from `JOUET_AI_MODEL`.
    pub model: Option<String>,
}

impl Agent {
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    pub(crate) fn from_lookup(env: impl Fn(&str) -> Option<OsString>) -> Self {
        let text = |name| {
            env(name)
                .map(|v| v.to_string_lossy().trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let raw = text("AI_AGENT");
        let (named, version) = raw.as_deref().map_or((None, None), parse_ai_agent);
        // mise-agent-env's names, so jouet and mise agree on who is running;
        // an agent it does not know keeps the name `AI_AGENT` gives it.
        let id = match mise_agent_env::detect_with_env(&env).map(|d| d.agent) {
            Some(mise_agent_env::Agent::Other) | None => named,
            Some(agent) => Some(agent.as_str().to_string()),
        };
        Self {
            id,
            version,
            model: text("JOUET_AI_MODEL"),
        }
    }

    /// The tool's name as people write it: `Claude Code`, `Codex`, or the
    /// identifier itself for agents without a known spelling.
    pub fn tool(&self) -> Option<String> {
        let id = self.id.as_deref()?;
        Some(
            match id {
                "claude-code" => "Claude Code",
                "claude-cowork" => "Claude Cowork",
                "codex" => "Codex",
                "cursor" => "Cursor",
                "gemini-cli" => "Gemini CLI",
                "github-copilot" => "GitHub Copilot",
                "opencode" => "OpenCode",
                other => other,
            }
            .to_string(),
        )
    }

    /// The trailers added when none are configured: none outside an agent;
    /// inside one, the co-author line its vendor publishes (Claude Code
    /// only, so far) and the AI disclosure.
    pub fn default_trailers(&self) -> Vec<String> {
        let Some(id) = self.id.as_deref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if matches!(id, "claude-code" | "claude-cowork") {
            out.push("Co-Authored-By: Claude <noreply@anthropic.com>".to_string());
        }
        out.push("AI-Assisted: Tool: {tool}; model: {model}; version: {version}".to_string());
        out
    }

    /// Fills `{tool}`, `{model}` and `{version}` in a configured trailer.
    pub fn expand(&self, template: &str) -> String {
        let or = |v: Option<String>| v.unwrap_or_else(|| UNAVAILABLE.into());
        template
            .replace("{tool}", &or(self.tool()))
            .replace("{model}", &or(self.model.clone()))
            .replace("{version}", &or(self.version.clone()))
    }
}

/// `claude-code_2-1-309_agent` is `claude-code`, version `2.1.309`, and
/// `name@1.2.3` is `name`, version `1.2.3`. The value is free-form across
/// vendors, so a word counts as the version only when it is digits separated
/// by `-` or `.`.
fn parse_ai_agent(raw: &str) -> (Option<String>, Option<String>) {
    let (name, at_version) = match raw.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (raw, None),
    };
    let mut words = name.split('_');
    let id = words
        .next()
        .map(|w| w.trim().to_ascii_lowercase())
        .filter(|w| !w.is_empty() && !matches!(w.as_str(), "1" | "true" | "yes"));
    let is_version = |w: &&str| {
        w.starts_with(|c: char| c.is_ascii_digit())
            && w.chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == '.')
    };
    let version = at_version
        .into_iter()
        .chain(words)
        .find(is_version)
        .map(|w| w.replace('-', "."));
    (id, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(vars: &[(&str, &str)]) -> Agent {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        Agent::from_lookup(|name| {
            vars.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| OsString::from(v))
        })
    }

    #[test]
    fn ai_agent_names_the_tool_and_version() {
        let a = agent(&[("AI_AGENT", "claude-code_2-1-309_agent")]);
        assert_eq!(a.id.as_deref(), Some("claude-code"));
        assert_eq!(a.version.as_deref(), Some("2.1.309"));
        assert_eq!(a.tool().as_deref(), Some("Claude Code"));
        assert_eq!(a.model, None);
        assert_eq!(
            a.expand("Tool: {tool}; model: {model}; version: {version}"),
            "Tool: Claude Code; model: unavailable; version: 2.1.309"
        );
    }

    #[test]
    fn a_vendor_marker_names_the_tool_without_a_version() {
        let a = agent(&[("CODEX_SANDBOX", "seatbelt")]);
        assert_eq!(a.id.as_deref(), Some("codex"));
        assert_eq!(a.version, None);
        let a = agent(&[("AI_AGENT", "codex"), ("JOUET_AI_MODEL", "openai/gpt-5")]);
        assert_eq!(a.tool().as_deref(), Some("Codex"));
        assert_eq!(a.model.as_deref(), Some("openai/gpt-5"));
    }

    #[test]
    fn defaults_follow_the_agent() {
        assert!(agent(&[]).default_trailers().is_empty());
        let claude = agent(&[("AI_AGENT", "claude-code_2-1-309_agent")]);
        assert_eq!(
            claude.default_trailers(),
            [
                "Co-Authored-By: Claude <noreply@anthropic.com>",
                "AI-Assisted: Tool: {tool}; model: {model}; version: {version}",
            ]
        );
        let codex = agent(&[("AI_AGENT", "codex")]);
        assert_eq!(codex.default_trailers().len(), 1);
    }

    #[test]
    fn flag_like_values_name_no_tool() {
        assert_eq!(parse_ai_agent("1"), (None, None));
        assert_eq!(parse_ai_agent("opencode"), (Some("opencode".into()), None));
        assert_eq!(
            parse_ai_agent("some-agent_v2_x"),
            (Some("some-agent".into()), None)
        );
        assert_eq!(
            parse_ai_agent("pi@0.4.1"),
            (Some("pi".into()), Some("0.4.1".into()))
        );
        let copilot = agent(&[("AI_AGENT", "github_copilot_vscode_agent")]);
        assert_eq!(copilot.tool().as_deref(), Some("GitHub Copilot"));
    }
}
