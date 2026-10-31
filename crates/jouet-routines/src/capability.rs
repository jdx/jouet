//! What a routine may reach. Its header declares them, and a primitive
//! outside them is refused, so reading the header says what running it can
//! do.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Capability {
    /// GitHub queries and REST reads, `github.prs`, and waits on CI, runs
    /// and PRs.
    #[serde(rename = "github.read")]
    GithubRead,
    /// GitHub mutations, REST writes, and jouet's commands that post or
    /// merge.
    #[serde(rename = "github.write")]
    GithubWrite,
    /// The work tree's state, diffs, history and refs.
    #[serde(rename = "git.read")]
    GitRead,
    /// Planning edits, and waits on files and logs.
    #[serde(rename = "fs.read")]
    FsRead,
    /// Writing edits.
    #[serde(rename = "fs.write")]
    FsWrite,
    /// Waits on ports and URLs, HTTP requests of any method, and package
    /// lookups.
    #[serde(rename = "net")]
    Net,
    /// The hooks' ledger.
    #[serde(rename = "ledger.read")]
    LedgerRead,
    /// mise's resolved config, tools, environment and task list.
    #[serde(rename = "mise.read")]
    MiseRead,
    /// Running the project's tasks: a mise task, or its test command.
    #[serde(rename = "mise.run")]
    MiseRun,
}

impl Capability {
    pub const ALL: [Self; 9] = [
        Self::GithubRead,
        Self::GithubWrite,
        Self::GitRead,
        Self::FsRead,
        Self::FsWrite,
        Self::Net,
        Self::LedgerRead,
        Self::MiseRead,
        Self::MiseRun,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::GithubRead => "github.read",
            Self::GithubWrite => "github.write",
            Self::GitRead => "git.read",
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
            Self::Net => "net",
            Self::LedgerRead => "ledger.read",
            Self::MiseRead => "mise.read",
            Self::MiseRun => "mise.run",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// Whether it changes something outside the routine: a routine with
    /// one needs `--yes` to act, and examples to be run.
    pub fn writes(self) -> bool {
        matches!(self, Self::GithubWrite | Self::FsWrite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_serialize_as_written() {
        for c in Capability::ALL {
            assert_eq!(Capability::parse(c.name()), Some(c));
            assert_eq!(
                serde_json::to_value(c).unwrap(),
                serde_json::json!(c.name())
            );
        }
        assert_eq!(Capability::parse("github"), None);
    }
}
