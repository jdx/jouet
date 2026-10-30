//! Conventional-commit subjects: whether a repository asks for them, and
//! what is wrong with a subject that is not one.
//!
//! A repository asks for them when a workflow checks them (a file under
//! `.github/workflows/` that mentions conventional commits, commitlint or
//! `action-semantic-pull-request`) or when it carries a commitlint,
//! cocogitto or commitizen config. The rules are commitlint's
//! `config-conventional` ones that matter for a subject: a known type, an
//! optional `(scope)` and `!`, a description that does not start with a
//! capitalised word (an acronym such as `CLI` is fine) or end with a period,
//! and at most 100 characters.

use std::path::Path;

/// commitlint's `config-conventional` types, plus `security`.
pub const DEFAULT_TYPES: &[&str] = &[
    "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor", "revert", "security",
    "style", "test",
];

const CONFIG_FILES: &[&str] = &[
    ".commitlintrc",
    ".commitlintrc.cjs",
    ".commitlintrc.js",
    ".commitlintrc.json",
    ".commitlintrc.mjs",
    ".commitlintrc.ts",
    ".commitlintrc.yaml",
    ".commitlintrc.yml",
    "commitlint.config.cjs",
    "commitlint.config.cts",
    "commitlint.config.js",
    "commitlint.config.mjs",
    "commitlint.config.mts",
    "commitlint.config.ts",
    "cog.toml",
    ".cz.json",
    ".cz.toml",
    ".czrc",
    "cz.json",
];

const WORKFLOW_MARKERS: &[&str] = &[
    "conventional-commit",
    "conventional commit",
    "conventionalcommits",
    "action-semantic-pull-request",
    "commitlint",
];

const MAX_HEADER: usize = 100;

/// What `jouet commit` does with a subject that breaks the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    Off,
    /// Commit anyway, with a warning.
    #[default]
    Warn,
    /// Refuse to commit.
    Refuse,
}

impl Policy {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "off" => Some(Self::Off),
            "warn" => Some(Self::Warn),
            "refuse" => Some(Self::Refuse),
            _ => None,
        }
    }
}

/// The file that shows a repository wants conventional commits, relative to
/// its top, or `None` when nothing does.
pub fn detect(top: &Path) -> Option<String> {
    if let Some(file) = CONFIG_FILES.iter().find(|f| top.join(f).is_file()) {
        return Some((*file).to_string());
    }
    let workflows = top.join(".github").join("workflows");
    let mut names: Vec<_> = std::fs::read_dir(&workflows)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        .collect();
    names.sort();
    names.into_iter().find_map(|name| {
        let lower_name = name.to_ascii_lowercase();
        let text = std::fs::read_to_string(workflows.join(&name))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let marked = lower_name.contains("conventional")
            || WORKFLOW_MARKERS.iter().any(|m| text.contains(m));
        marked.then(|| format!(".github/workflows/{name}"))
    })
}

/// What is wrong with a commit message's subject (and the line after it),
/// in the order a reader would fix them; empty when nothing is.
pub fn lint(message: &str, types: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut lines = message.lines();
    let subject = lines.next().unwrap_or_default();
    if lines.next().is_some_and(|l| !l.trim().is_empty()) {
        problems.push("leave a blank line between the subject and the body".into());
    }
    let Some(caps) = header().captures(subject) else {
        problems.insert(
            0,
            "the subject is not `type(scope): description` (the scope and a `!` are optional)"
                .into(),
        );
        return problems;
    };
    let kind = &caps["type"];
    if !types.iter().any(|t| t == kind) {
        problems.push(format!(
            "unknown type `{kind}`; use one of {}",
            types.join(", ")
        ));
    }
    let description = &caps["description"];
    if starts_capitalised(description) {
        problems
            .push("start the description lowercase, or with an acronym such as CLI or PGO".into());
    }
    if description.ends_with('.') {
        problems.push("do not end the subject with a period".into());
    }
    let length = subject.chars().count();
    if length > MAX_HEADER {
        problems.push(format!(
            "the subject is {length} characters; keep it to {MAX_HEADER}"
        ));
    }
    problems
}

fn header() -> &'static regex::Regex {
    static HEADER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    HEADER.get_or_init(|| {
        regex::Regex::new(r"^(?<type>[a-z]+)(\([^()\s][^()]*\))?!?: (?<description>\S.*)$")
            .expect("the header pattern compiles")
    })
}

/// Whether the description starts with a capitalised word that is not an
/// acronym: `Add` is, `CLI` and `add` are not.
fn starts_capitalised(description: &str) -> bool {
    let word: String = description
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect();
    let mut chars = word.chars();
    match chars.next() {
        Some(first) if first.is_uppercase() => {
            let rest: Vec<char> = chars.collect();
            rest.is_empty() || !rest.iter().all(|c| c.is_uppercase() || c.is_ascii_digit())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types() -> Vec<String> {
        DEFAULT_TYPES.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn good_subjects_pass() {
        for subject in [
            "feat: add jouet commit",
            "feat(git): add jouet commit, review-package and handoff",
            "fix(wait)!: stop following a closed PR",
            "docs: describe the CLI",
            "perf: cut PGO build time",
            "chore: release 0.1.0\n\nThe body is fine.",
        ] {
            assert_eq!(lint(subject, &types()), Vec::<String>::new(), "{subject}");
        }
    }

    #[test]
    fn bad_subjects_say_why() {
        let problems = |s: &str| lint(s, &types());
        assert!(problems("Add a thing")[0].contains("not `type(scope): description`"));
        assert!(problems("feature: add")[0].contains("unknown type `feature`"));
        assert!(problems("feat: Add commit")[0].contains("lowercase"));
        assert!(problems("feat: A thing")[0].contains("lowercase"));
        assert!(problems("feat: add commit.")[0].contains("period"));
        assert!(problems(&format!("feat: {}", "x".repeat(100)))[0].contains("characters"));
        assert!(problems("feat: add\nbody right away")[0].contains("blank line"));
        assert!(problems("feat:add")[0].contains("not `type"));
        assert!(problems("feat(): add")[0].contains("not `type"));
    }

    #[test]
    fn configured_types_replace_the_defaults() {
        let types = vec!["registry".to_string()];
        assert!(lint("registry: add jouet", &types).is_empty());
        assert!(!lint("feat: add jouet", &types).is_empty());
    }

    #[test]
    fn detection_reads_configs_and_workflows() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect(dir.path()), None);
        let workflows = dir.path().join(".github/workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("ci.yml"), "on: push\n").unwrap();
        assert_eq!(detect(dir.path()), None);
        std::fs::write(
            workflows.join("title.yaml"),
            "steps:\n  - uses: amannn/action-semantic-pull-request@v5\n",
        )
        .unwrap();
        assert_eq!(
            detect(dir.path()).as_deref(),
            Some(".github/workflows/title.yaml")
        );
        std::fs::write(dir.path().join("cog.toml"), "").unwrap();
        assert_eq!(detect(dir.path()).as_deref(), Some("cog.toml"));
    }
}
