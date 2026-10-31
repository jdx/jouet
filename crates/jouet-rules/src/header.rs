//! The header every Luau rule file starts with: a few `--- key: value` lines that
//! say what the rule is for and who wrote it, readable without running it.
//!
//! ```lua
//! --- name: git-add-all
//! --- why: `git add -A` sweeps in caches and other sessions' work.
//! --- mode: deny
//! --- scope: builtin
//! --- author: builtin
//! --- created: 2026-10-30
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a rule's denials do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// A denial is shown as a warning, and the call goes ahead.
    Warn,
    Deny,
}

/// Where a rule lives, which decides what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Builtin,
    User,
    Repo,
    /// From a rule pack, pinned by its content and, from git, its commit.
    Pack,
    /// From the system directory, `/etc/jouet`: the settings cannot turn it
    /// off.
    Enforced,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Repo => "repo",
            Self::Pack => "pack",
            Self::Enforced => "enforced",
        }
    }

    /// The mode a rule gets when its header names none: a user's rules,
    /// often written by an agent, start out only warning.
    pub fn default_mode(self) -> Mode {
        match self {
            Self::User => Mode::Warn,
            Self::Builtin | Self::Repo | Self::Pack | Self::Enforced => Mode::Deny,
        }
    }

    /// Whether a rule that says it has scope `declared` may live here. A
    /// pack or the system directory may hold rules written for a
    /// repository.
    pub(crate) fn accepts(self, declared: Self) -> bool {
        declared == self || (declared == Self::Repo && matches!(self, Self::Pack | Self::Enforced))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub name: Option<String>,
    pub why: Option<String>,
    pub mode: Option<String>,
    pub scope: Option<String>,
    pub author: Option<String>,
    pub created: Option<String>,
}

/// Reads the `--- key: value` lines at the top of a rule file, skipping blank
/// lines and `--!` directives. The header ends at the first other line.
pub(crate) fn parse(code: &str) -> Header {
    let mut header = Header::default();
    for line in code.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--!") {
            continue;
        }
        let Some(field) = line.strip_prefix("---") else {
            break;
        };
        let Some((key, value)) = field.split_once(':') else {
            continue;
        };
        let value = Some(value.trim().to_string()).filter(|v| !v.is_empty());
        match key.trim() {
            "name" => header.name = value,
            "why" => header.why = value,
            "mode" => header.mode = value,
            "scope" => header.scope = value,
            "author" => header.author = value,
            "created" => header.created = value,
            _ => {}
        }
    }
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_header_and_stops_at_code() {
        let header = parse(
            "--!strict\n--- name: x\n--- why: because: reasons\n\n--- mode: warn\n--- unknown: y\nlocal z = 1\n--- author: late\n",
        );
        assert_eq!(header.name.as_deref(), Some("x"));
        assert_eq!(header.why.as_deref(), Some("because: reasons"));
        assert_eq!(header.mode.as_deref(), Some("warn"));
        assert_eq!(header.author, None);
    }
}
