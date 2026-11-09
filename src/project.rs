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
//!
//! [checks.ci]
//! e2e = "mise run test:e2e"
//!
//! [checks.broken]
//! "mise run lint" = "golangci-lint is built with Go 1.26; go.mod needs 1.27"
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
    /// A verified project-specific invocation of the affected selector.
    pub(crate) affected: Option<String>,
    /// A focused check by what it covers: `unit`, `e2e`, `bats` and so on.
    pub(crate) focused: BTreeMap<String, String>,
    pub(crate) lint: Option<String>,
    /// What to run before pushing that the focused checks leave out.
    pub(crate) before_push: Vec<String>,
    /// Checks to leave to CI: slow, or needing what a workstation lacks
    /// (services, credentials, tools CI installs).
    pub(crate) ci: BTreeMap<String, String>,
    /// Commands that fail here before any change, each with why. A failed
    /// run of one does not hold up finishing.
    pub(crate) broken: BTreeMap<String, String>,
}

impl Checks {
    /// Why `command` fails here before any change, when it is a broken
    /// check: as written, or run through `jouet test --cmd --`. A `NAME` in
    /// the check stands for the one or more tests a run names there.
    pub(crate) fn broken(&self, command: &str) -> Option<&str> {
        let ran: Vec<&str> = through_jouet_test(command).split_whitespace().collect();
        self.broken
            .iter()
            .find(|(check, _)| {
                let check: Vec<&str> = through_jouet_test(check).split_whitespace().collect();
                match check.iter().position(|word| *word == "NAME") {
                    Some(at) => {
                        let after = &check[at + 1..];
                        ran.len() > check.len() - 1
                            && ran.starts_with(&check[..at])
                            && ran.ends_with(after)
                            && !ran[at..ran.len() - after.len()]
                                .iter()
                                .any(|word| word.starts_with('-'))
                    }
                    None => check == ran,
                }
            })
            .map(|(_, why)| why.as_str())
    }
}

/// The command `jouet test --cmd -- CMD` runs, or `command` itself.
fn through_jouet_test(command: &str) -> &str {
    let command = command.trim();
    if command.starts_with("jouet test ")
        && let Some(at) = command.find(" -- ")
    {
        return command[at + 4..].trim();
    }
    command
}

/// The longest line the primer gives the project's checks, in characters.
const LINE_MAX: usize = 600;

/// The project file at `root`, when there is one: its checks, or why it
/// does not parse.
pub(crate) fn read(root: &Path) -> Option<Result<Project, String>> {
    let text = std::fs::read_to_string(root.join(".jouet/project.toml")).ok()?;
    Some(parse(&text))
}

pub(crate) fn parse(text: &str) -> Result<Project, String> {
    let project: Project =
        toml::from_str(text).map_err(|e: toml::de::Error| e.message().to_string())?;
    if project
        .checks
        .affected
        .as_deref()
        .is_some_and(|command| inline_command(command).is_none())
    {
        return Err("checks.affected must be a nonempty single-line command of at most 200 characters without backticks".to_string());
    }
    Ok(project)
}

/// One line for the session-start primer naming the project's checks, when
/// its `.jouet/project.toml` is trusted and names any.
pub(crate) fn primer_line(cwd: &Path) -> Option<String> {
    let root = crate::verify::root_of(cwd);
    let project = trusted(Path::new(&root))?;
    match project {
        Ok(project) => line(&project.checks),
        // Said, so a typo does not look like no file at all.
        Err(error) => Some(format!(
            "jouet: .jouet/project.toml does not parse, so its checks are not shown: {error}"
        )),
    }
}

/// The project file at `root` once the repository's jouet files are
/// trusted: it is text an agent acts on.
pub(crate) fn trusted(root: &Path) -> Option<Result<Project, String>> {
    let project = read(root)?;
    repo_trusted(root).then_some(project)
}

/// The affected check to recommend in the session primer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) enum AffectedCommand {
    #[default]
    Default,
    Configured(String),
    Broken,
}

pub(crate) fn affected_command(cwd: &Path) -> AffectedCommand {
    let root = crate::verify::root_of(cwd);
    trusted(Path::new(&root))
        .and_then(Result::ok)
        .map_or(AffectedCommand::Default, |project| {
            affected_for(&project.checks)
        })
}

fn affected_for(checks: &Checks) -> AffectedCommand {
    let configured = checks.affected.as_deref().and_then(inline_command);
    let selected = configured.unwrap_or("jouet test --affected");
    if checks.broken(selected).is_some() {
        AffectedCommand::Broken
    } else if let Some(command) = configured {
        AffectedCommand::Configured(command.to_string())
    } else {
        AffectedCommand::Default
    }
}

fn inline_command(command: &str) -> Option<&str> {
    let command = command.trim();
    if command.is_empty()
        || command.chars().count() > 200
        || command.chars().any(|c| c.is_control() || c == '`')
    {
        return None;
    }
    Some(command)
}

/// Whether the repository's jouet files at `root` run.
fn repo_trusted(root: &Path) -> bool {
    let mut errors = Vec::new();
    sources::sets(root, None, &mut sources::digests(), &mut errors)
        .iter()
        .any(|set| set.kind == Kind::Repo && set.status.runs())
}

/// One line for the session-start primer pointing at the repository's
/// `.jouet/map.md`, once its jouet files are trusted. The map is read once,
/// when the agent needs it, rather than carried by every request.
pub(crate) fn map_line(cwd: &Path) -> Option<String> {
    let root = crate::verify::root_of(cwd);
    let root = Path::new(&root);
    let text = std::fs::read_to_string(root.join(".jouet/map.md")).ok()?;
    repo_trusted(root).then(|| map_note(&text))
}

fn map_note(text: &str) -> String {
    format!(
        "jouet: .jouet/map.md maps where this project's code, tests and generated files live ({} lines): read it before searching for where something is.",
        text.lines().count()
    )
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
    if !checks.ci.is_empty() {
        let each: Vec<String> = checks
            .ci
            .iter()
            .map(|(kind, command)| format!("{kind} `{command}`"))
            .collect();
        parts.push(format!(
            "left to CI, so push and `jouet wait ci` rather than run them here: {}",
            each.join(", ")
        ));
    }
    if !checks.broken.is_empty() {
        let each: Vec<String> = checks
            .broken
            .iter()
            .map(|(command, why)| format!("`{command}` ({why})"))
            .collect();
        parts.push(format!(
            "failing here before any change, so not yours to fix unless asked: {}",
            each.join(", ")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    let prefix = "jouet: prefer these project-specific checks over generic examples when applicable (.jouet/project.toml):";
    let full = format!("{prefix} {}.", parts.join("; "));
    if full.chars().count() <= LINE_MAX {
        return Some(full);
    }
    let mut text = prefix.to_string();
    let mut shown = 0;
    for part in &parts {
        let omitted = parts.len() - shown - 1;
        let separator = if shown == 0 { " " } else { "; " };
        let ending = if omitted == 0 {
            ".".to_string()
        } else {
            format!("; … {omitted} more check entries in .jouet/project.toml")
        };
        if text.chars().count() + separator.len() + part.chars().count() + ending.chars().count()
            > LINE_MAX
        {
            break;
        }
        text.push_str(separator);
        text.push_str(part);
        shown += 1;
    }
    let omitted = parts.len() - shown;
    if omitted == 0 {
        text.push('.');
    } else {
        let separator = if shown == 0 { " " } else { "; " };
        text.push_str(&format!(
            "{separator}… {omitted} more check entries in .jouet/project.toml"
        ));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affected_command_is_a_bounded_single_line() {
        let project =
            parse("[checks]\naffected = 'env -u NO_COLOR jouet test --affected'\n").unwrap();
        assert_eq!(
            inline_command(project.checks.affected.as_deref().unwrap()),
            Some("env -u NO_COLOR jouet test --affected")
        );
        assert_eq!(inline_command(" "), None);
        assert_eq!(inline_command("first\nsecond"), None);
        assert_eq!(inline_command("echo `bad`"), None);
        assert_eq!(inline_command(&"x".repeat(201)), None);
        assert!(
            parse("[checks]\naffected = ''\n")
                .unwrap_err()
                .contains("checks.affected")
        );
        assert!(
            parse("[checks]\naffected = 'echo `bad`'\n")
                .unwrap_err()
                .contains("checks.affected")
        );
    }

    #[test]
    fn known_broken_affected_checks_are_not_recommended() {
        let override_command = "env -u NO_COLOR jouet test --affected";
        let mut checks = Checks {
            affected: Some(override_command.into()),
            ..Checks::default()
        };
        assert_eq!(
            affected_for(&checks),
            AffectedCommand::Configured(override_command.into())
        );
        checks
            .broken
            .insert(override_command.into(), "broken".into());
        assert_eq!(affected_for(&checks), AffectedCommand::Broken);

        checks.affected = None;
        assert_eq!(affected_for(&checks), AffectedCommand::Default);
        checks
            .broken
            .insert("jouet test --affected".into(), "broken".into());
        assert_eq!(affected_for(&checks), AffectedCommand::Broken);
    }

    #[test]
    fn the_map_is_named_with_its_length_not_inlined() {
        assert_eq!(
            map_note("# Map\n\n- `src/`: the code\n"),
            "jouet: .jouet/map.md maps where this project's code, tests and generated files live (3 lines): read it before searching for where something is."
        );
    }

    #[test]
    fn the_checks_make_one_line_in_a_fixed_order() {
        let project: Project = toml::from_str(
            "[checks]\nfull = \"mise run test\"\nbefore_push = [\"cargo clippy -- -D warnings\"]\n[checks.focused]\nbats = \"jouet test --cmd -- mise run test:bats -- test/NAME.bats\"\nunit = \"jouet test --affected\"\n",
        )
        .unwrap();
        assert_eq!(
            line(&project.checks).unwrap(),
            "jouet: prefer these project-specific checks over generic examples when applicable (.jouet/project.toml): bats `jouet test --cmd -- mise run test:bats -- test/NAME.bats`; unit `jouet test --affected`; full suite `mise run test`; before pushing `cargo clippy -- -D warnings`."
        );
        assert_eq!(line(&Checks::default()), None);
    }

    #[test]
    fn long_check_notes_omit_whole_entries_and_report_the_count() {
        let mut checks = Checks::default();
        checks.focused.insert("a".into(), "cargo test".into());
        checks
            .focused
            .insert("b".into(), format!("command-{}", "é".repeat(700)));
        checks.focused.insert("c".into(), "cargo test --all".into());
        let note = line(&checks).unwrap();
        assert!(note.chars().count() <= LINE_MAX);
        assert!(note.contains("a `cargo test`"));
        assert!(!note.contains("command-"));
        assert!(!note.contains("cargo test --all"));
        assert!(note.ends_with("… 2 more check entries in .jouet/project.toml"));

        checks.focused.remove("a");
        let note = line(&checks).unwrap();
        assert!(note.chars().count() <= LINE_MAX);
        assert!(!note.contains("command-"));
        assert!(note.ends_with("… 2 more check entries in .jouet/project.toml"));

        checks.focused.clear();
        checks.focused.insert("a".into(), "x".repeat(460));
        checks.focused.insert("b".into(), "ok".into());
        let note = line(&checks).unwrap();
        assert!(note.chars().count() <= LINE_MAX);
        assert!(note.contains("b `ok`"));
        assert!(!note.contains("more check entries"));
    }

    #[test]
    fn ci_and_broken_checks_say_to_leave_them() {
        let project: Project = toml::from_str(
            "[checks]\nlint = \"mise run lint\"\n[checks.focused]\nunit = \"jouet test --affected\"\n[checks.ci]\ne2e = \"mise run test:e2e\"\n[checks.broken]\n\"mise run lint\" = \"golangci-lint is built with Go 1.26\"\n",
        )
        .unwrap();
        assert_eq!(
            line(&project.checks).unwrap(),
            "jouet: prefer these project-specific checks over generic examples when applicable (.jouet/project.toml): unit `jouet test --affected`; lint `mise run lint`; left to CI, so push and `jouet wait ci` rather than run them here: e2e `mise run test:e2e`; failing here before any change, so not yours to fix unless asked: `mise run lint` (golangci-lint is built with Go 1.26)."
        );
        let checks = &project.checks;
        let why = Some("golangci-lint is built with Go 1.26");
        assert_eq!(checks.broken("mise run lint"), why);
        assert_eq!(checks.broken("jouet test --cmd -- mise run lint"), why);
        assert_eq!(
            checks.broken("jouet test --budget 900 --cmd -- mise run lint"),
            why
        );
        assert_eq!(checks.broken("mise run lint:go"), None);
        assert_eq!(checks.broken("mise run test"), None);
    }

    #[test]
    fn a_broken_check_with_name_matches_the_tests_a_run_names() {
        let project: Project = toml::from_str(
            "[checks.broken]\n\"jouet test --cmd -- mise run test:unit NAME\" = \"asgiref is not installed\"\n",
        )
        .unwrap();
        let checks = &project.checks;
        let why = Some("asgiref is not installed");
        assert_eq!(checks.broken("mise run test:unit forms_tests"), why);
        assert_eq!(
            checks.broken("jouet test --cmd -- mise run test:unit forms_tests admin_views.tests"),
            why
        );
        // NAME is at least one test, and never an option that changes the run.
        assert_eq!(checks.broken("mise run test:unit"), None);
        assert_eq!(checks.broken("mise run test:unit --parallel 1 a"), None);
        assert_eq!(checks.broken("mise run test:e2e a"), None);
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(toml::from_str::<Project>("[checks]\nfast = \"x\"\n").is_err());
    }
}
