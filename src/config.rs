//! The optional config file, `$XDG_CONFIG_HOME/jouet/config.toml` (by
//! default `~/.config/jouet/config.toml`), and the environment variables that
//! override it. Keys this build does not read are ignored, so one file serves
//! every jouet version.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    reply_footer: Option<String>,
    #[serde(default)]
    bots: Vec<String>,
    stats_summary: Option<String>,
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
}

impl Config {
    /// How `jouet stats --summary` tells its line: `JOUET_STATS_SUMMARY` when
    /// it is set, else `stats_summary`, else quips. `off` prints no line.
    pub(crate) fn stats_summary(&self) -> Result<jouet_audit::stats::SummaryStyle, String> {
        let (name, source) = match std::env::var("JOUET_STATS_SUMMARY") {
            Ok(name) => (name, "JOUET_STATS_SUMMARY"),
            Err(_) => match &self.stats_summary {
                Some(name) => (name.clone(), "stats_summary"),
                None => return Ok(jouet_audit::stats::SummaryStyle::default()),
            },
        };
        jouet_audit::stats::SummaryStyle::parse(&name)
            .ok_or_else(|| format!("{source} must be quips, plain or off, not `{name}`"))
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
}
