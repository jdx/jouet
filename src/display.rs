//! Who is reading jouet's output, and so how it is written.
//!
//! An agent comes before the TTY check: a terminal does not prove a person is
//! reading (Codex can run commands under a PTY, and so can a terminal pane an
//! agent drives). Detection is mise's `mise-agent-env`, so mise, mbx and jouet
//! agree on who counts as an agent.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Compact text, no colour, minified JSON, never truncated silently.
    Agent,
    /// Colour, aligned tables and pretty-printed JSON.
    Human,
    /// Like `Agent`, for pipes, redirects and CI.
    Plain,
}

impl Mode {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "agent" => Some(Self::Agent),
            "human" => Some(Self::Human),
            "plain" => Some(Self::Plain),
            _ => None,
        }
    }

    /// Picks the mode: `--display`, then `JOUET_DISPLAY`, then agent
    /// detection, then whether stdout is a terminal.
    pub fn detect(flag: Option<&str>) -> Result<Self, String> {
        let chosen = flag.map(|f| (f.to_string(), "--display")).or_else(|| {
            std::env::var("JOUET_DISPLAY")
                .ok()
                .map(|v| (v, "JOUET_DISPLAY"))
        });
        if let Some((name, source)) = chosen {
            return Self::parse(&name)
                .ok_or_else(|| format!("{source} must be agent, human or plain, not `{name}`"));
        }
        Ok(if mise_agent_env::is_agent() {
            Self::Agent
        } else if std::io::stdout().is_terminal() {
            Self::Human
        } else {
            Self::Plain
        })
    }

    pub fn pretty_json(self) -> bool {
        self == Self::Human
    }

    /// Serializes a JSON document the way this mode prints it.
    pub fn json<T: serde::Serialize>(self, value: &T) -> String {
        let out = if self.pretty_json() {
            serde_json::to_string_pretty(value)
        } else {
            serde_json::to_string(value)
        };
        out.expect("jouet's output types always serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_wins_and_names_must_be_known() {
        assert_eq!(Mode::detect(Some("human")), Ok(Mode::Human));
        assert_eq!(Mode::detect(Some("agent")), Ok(Mode::Agent));
        let err = Mode::detect(Some("fancy")).unwrap_err();
        assert!(err.contains("--display"), "{err}");
    }

    #[test]
    fn only_human_mode_pretty_prints() {
        let value = serde_json::json!({ "a": [1, 2] });
        assert_eq!(Mode::Agent.json(&value), r#"{"a":[1,2]}"#);
        assert_eq!(Mode::Plain.json(&value), r#"{"a":[1,2]}"#);
        assert!(Mode::Human.json(&value).contains('\n'));
    }
}
