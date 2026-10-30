//! The optional config file, `$XDG_CONFIG_HOME/jouet/config.toml` (by
//! default `~/.config/jouet/config.toml`), and the environment variables that
//! override it. Keys this build does not read are ignored, so one file serves
//! every jouet version.

use jouet_git::agent::Agent;
use jouet_git::conventional::{self, Policy};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    reply_footer: Option<String>,
    #[serde(default)]
    bots: Vec<String>,
    #[serde(default)]
    commit: CommitConfig,
}

/// The `[commit]` table, for `jouet commit`.
#[derive(Debug, Default, Deserialize)]
struct CommitConfig {
    /// Trailers to append; `{tool}`, `{model}` and `{version}` are filled in
    /// from the environment. An empty list turns the defaults off.
    trailers: Option<Vec<String>>,
    /// `warn`, `refuse` or `off`.
    conventional: Option<String>,
    /// The conventional-commit types to accept.
    types: Option<Vec<String>>,
}

impl Config {
    /// Reads the config file; a missing file is an empty config.
    pub(crate) fn load() -> Result<Self, String> {
        let Some(path) = path() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// The text appended to every reply: `JOUET_REPLY_FOOTER` when it is set
    /// (set but empty turns the footer off), else `reply_footer`.
    pub(crate) fn reply_footer(&self) -> Option<String> {
        let footer = match std::env::var("JOUET_REPLY_FOOTER") {
            Ok(footer) => footer,
            Err(_) => self.reply_footer.clone()?,
        };
        let footer = footer.trim();
        (!footer.is_empty()).then(|| footer.to_string())
    }

    /// Logins counted as bots besides those ending in `[bot]`: `bots`, plus
    /// any in `JOUET_BOTS`, separated by commas or spaces.
    pub(crate) fn bots(&self) -> Vec<String> {
        let env = std::env::var("JOUET_BOTS").unwrap_or_default();
        self.bots
            .iter()
            .map(String::as_str)
            .chain(env.split([',', ' ']))
            .filter(|b| !b.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    }

    /// The trailers `jouet commit` appends: `JOUET_COMMIT_TRAILERS` (one per
    /// line) when it is set, set but empty for none; else `[commit]
    /// trailers`; else the agent's defaults. Templates are filled in.
    pub(crate) fn commit_trailers(&self, agent: &Agent) -> Vec<String> {
        let templates = match std::env::var("JOUET_COMMIT_TRAILERS") {
            Ok(lines) => lines
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            Err(_) => self
                .commit
                .trailers
                .clone()
                .unwrap_or_else(|| agent.default_trailers()),
        };
        templates.iter().map(|t| agent.expand(t)).collect()
    }

    /// What to do with a subject that is not a conventional commit:
    /// `JOUET_COMMIT_CONVENTIONAL`, else `[commit] conventional`, else warn.
    pub(crate) fn commit_conventional(&self) -> Result<Policy, String> {
        let (name, source) = match std::env::var("JOUET_COMMIT_CONVENTIONAL") {
            Ok(name) if !name.is_empty() => (name, "JOUET_COMMIT_CONVENTIONAL"),
            _ => match &self.commit.conventional {
                Some(name) => (name.clone(), "[commit] conventional"),
                None => return Ok(Policy::default()),
            },
        };
        Policy::parse(&name)
            .ok_or_else(|| format!("{source} must be warn, refuse or off, not `{name}`"))
    }

    /// The conventional-commit types to accept.
    pub(crate) fn commit_types(&self) -> Vec<String> {
        self.commit.types.clone().unwrap_or_else(|| {
            conventional::DEFAULT_TYPES
                .iter()
                .map(|t| (*t).to_string())
                .collect()
        })
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))?;
    Some(base.join("jouet").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_ignored() {
        let config: Config =
            toml::from_str("reply_footer = \"-- sent by an agent\"\ndefault_timeout = \"10m\"\n")
                .unwrap();
        assert_eq!(config.reply_footer.as_deref(), Some("-- sent by an agent"));
        assert!(config.bots.is_empty());
    }

    #[test]
    fn the_commit_table() {
        let config: Config = toml::from_str(
            "[commit]\ntrailers = [\"AI-Assisted: Tool: {tool}; model: anthropic/claude-opus-5-5\"]\n\
             conventional = \"refuse\"\ntypes = [\"feat\", \"registry\"]\n",
        )
        .unwrap();
        assert_eq!(config.commit_types(), ["feat", "registry"]);
        assert_eq!(
            config.commit.trailers.as_deref(),
            Some(&["AI-Assisted: Tool: {tool}; model: anthropic/claude-opus-5-5".to_string()][..])
        );
        assert_eq!(config.commit.conventional.as_deref(), Some("refuse"));
        let empty = Config::default();
        assert!(empty.commit_types().contains(&"fix".to_string()));
    }
}
