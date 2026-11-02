//! A repository's `.jouet/project.toml`: the commands its checks run, as the
//! jouet-init skill found and verified them, committed so every session
//! starts knowing them rather than looking for them.
//!
//! ```toml
//! [checks]
//! full = "mise run test"
//! lint = "jouet diag check --changed"
//! before_push = ["cargo clippy --workspace --all-targets -- -D warnings"]
//!
//! [checks.focused]
//! unit = "jouet test --affected"
//! bats = "jouet test --cmd -- mise run test:bats -- test/NAME.bats"
//! ```
//!
//! It is text the agent reads, so it counts only once the repository's jouet
//! files are trusted, as its rules and `test.toml` do.

use crate::sources::{self, Kind};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Project {
    pub(crate) checks: Checks,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Checks {
    /// The whole suite.
    pub(crate) full: Option<String>,
    /// A focused check by what it covers: `unit`, `e2e`, `bats` and so on.
    pub(crate) focused: BTreeMap<String, String>,
    pub(crate) lint: Option<String>,
    /// What to run before pushing that the focused checks leave out.
    pub(crate) before_push: Vec<String>,
}

/// The longest line the primer gives the project's checks, in characters.
const LINE_MAX: usize = 600;

/// The project file at `root`, when there is one that parses.
pub(crate) fn read(root: &Path) -> Option<Project> {
    let text = std::fs::read_to_string(root.join(".jouet/project.toml")).ok()?;
    toml::from_str(&text).ok()
}

/// One line for the session-start primer naming the project's checks, when
/// its `.jouet/project.toml` is trusted and names any.
pub(crate) fn primer_line(cwd: &Path) -> Option<String> {
    let root = crate::verify::root_of(cwd);
    let root = Path::new(&root);
    let project = read(root)?;
    let mut errors = Vec::new();
    let trusted = sources::sets(root, None, &mut sources::digests(), &mut errors)
        .iter()
        .any(|set| set.kind == Kind::Repo && set.status.runs());
    if !trusted {
        return None;
    }
    line(&project.checks)
}

fn line(checks: &Checks) -> Option<String> {
    let mut parts = Vec::new();
    for (kind, command) in &checks.focused {
        parts.push(format!("{kind} `{command}`"));
    }
    if let Some(full) = &checks.full {
        parts.push(format!("full suite `{full}`"));
    }
    if let Some(lint) = &checks.lint {
        parts.push(format!("lint `{lint}`"));
    }
    if !checks.before_push.is_empty() {
        let each: Vec<String> = checks
            .before_push
            .iter()
            .map(|c| format!("`{c}`"))
            .collect();
        parts.push(format!("before pushing {}", each.join(" and ")));
    }
    if parts.is_empty() {
        return None;
    }
    let text = format!(
        "jouet: this project's checks (.jouet/project.toml): {}.",
        parts.join("; ")
    );
    Some(if text.chars().count() > LINE_MAX {
        let cut: String = text.chars().take(LINE_MAX).collect();
        format!("{cut}… (more in .jouet/project.toml)")
    } else {
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checks_make_one_line_in_a_fixed_order() {
        let project: Project = toml::from_str(
            "[checks]\nfull = \"mise run test\"\nbefore_push = [\"cargo clippy -- -D warnings\"]\n[checks.focused]\nbats = \"jouet test --cmd -- mise run test:bats -- test/NAME.bats\"\nunit = \"jouet test --affected\"\n",
        )
        .unwrap();
        assert_eq!(
            line(&project.checks).unwrap(),
            "jouet: this project's checks (.jouet/project.toml): bats `jouet test --cmd -- mise run test:bats -- test/NAME.bats`; unit `jouet test --affected`; full suite `mise run test`; before pushing `cargo clippy -- -D warnings`."
        );
        assert_eq!(line(&Checks::default()), None);
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(toml::from_str::<Project>("[checks]\nfast = \"x\"\n").is_err());
    }
}
