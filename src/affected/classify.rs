//! What a changed file means for a test suite: nothing (docs, or another
//! package's code), a file the adapter can trace to tests, or a reason to
//! run the whole suite.

use super::Ecosystem;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What one changed file means for one suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// It cannot change a test's outcome; why.
    Ignored(String),
    /// The adapter traces it to the tests that depend on it.
    Source,
    /// The whole suite has to run; why.
    Full(String),
}

/// The last part of a path.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The part of a file name after its last dot, lowercased.
pub(crate) fn extension(path: &str) -> String {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn is_requirements(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("requirements") || lower.starts_with("constraints"))
        && (lower.ends_with(".txt") || lower.ends_with(".in"))
}

/// Documentation, and repository files no test reads: changing only these
/// runs nothing.
pub(crate) fn is_docs(path: &str) -> bool {
    let name = file_name(path);
    if is_requirements(name) {
        return false;
    }
    let ext = extension(path);
    if ["md", "mdx", "markdown", "rst", "adoc", "asciidoc", "txt"].contains(&ext.as_str()) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if [
        "LICENSE",
        "LICENCE",
        "COPYING",
        "AUTHORS",
        "CHANGELOG",
        "CHANGES",
        "NOTICE",
        "CODEOWNERS",
        "CONTRIBUTORS",
    ]
    .contains(&stem.as_str())
    {
        return true;
    }
    [
        ".gitignore",
        ".gitattributes",
        ".editorconfig",
        ".mailmap",
        ".git-blame-ignore-revs",
    ]
    .contains(&name)
        || path.starts_with(".github/ISSUE_TEMPLATE/")
        || path.starts_with(".vscode/")
        || path.starts_with(".idea/")
}

/// Why a change to this file, wherever it is, means every suite runs:
/// lockfiles, tool versions and CI configuration.
pub(crate) fn global_trigger(path: &str) -> Option<String> {
    let name = file_name(path);
    const LOCKFILES: &[&str] = &[
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "bun.lock",
        "bun.lockb",
        "aube-lock.yaml",
        "poetry.lock",
        "uv.lock",
        "Pipfile",
        "Pipfile.lock",
        "pdm.lock",
        "Cargo.lock",
        "go.sum",
        "Gemfile.lock",
    ];
    if LOCKFILES.contains(&name) || is_requirements(name) {
        return Some(format!("a lockfile changed ({path})"));
    }
    const VERSIONS: &[&str] = &[
        ".tool-versions",
        ".nvmrc",
        ".node-version",
        ".python-version",
        ".npmrc",
        ".yarnrc",
        ".yarnrc.yml",
        ".pnpmfile.cjs",
    ];
    let mise = (name.starts_with("mise.") || name.starts_with(".mise."))
        && (name.ends_with(".toml") || name.ends_with(".lock"))
        || path.ends_with("mise/config.toml")
        || path == ".config/mise.toml";
    if VERSIONS.contains(&name) || mise {
        return Some(format!("tool versions or settings changed ({path})"));
    }
    const CI_DIRS: &[&str] = &[
        ".github/workflows/",
        ".github/actions/",
        ".gitlab/",
        ".circleci/",
        ".buildkite/",
        ".woodpecker/",
        ".forgejo/workflows/",
        ".gitea/workflows/",
    ];
    const CI_FILES: &[&str] = &[
        ".gitlab-ci.yml",
        "azure-pipelines.yml",
        "Jenkinsfile",
        ".travis.yml",
        "bitbucket-pipelines.yml",
        ".woodpecker.yml",
    ];
    if CI_DIRS.iter().any(|d| path.starts_with(d)) || CI_FILES.contains(&path) {
        return Some(format!("CI configuration changed ({path})"));
    }
    None
}

/// Which ecosystems' manifests are in a directory.
fn manifests(dir: &Path) -> Vec<Ecosystem> {
    let has = |name: &str| dir.join(name).is_file();
    let mut out = Vec::new();
    if has("package.json") {
        out.push(Ecosystem::Js);
    }
    if has("pyproject.toml") || has("setup.py") || has("setup.cfg") {
        out.push(Ecosystem::Python);
    }
    for (names, eco) in [
        (&["Cargo.toml"][..], Ecosystem::Rust),
        (&["go.mod"], Ecosystem::Go),
        (&["Gemfile"], Ecosystem::Ruby),
        (
            &["pom.xml", "build.gradle", "build.gradle.kts"],
            Ecosystem::Jvm,
        ),
        (&["Package.swift"], Ecosystem::Swift),
    ] {
        if names.iter().any(|n| has(n)) {
            out.push(eco);
        }
    }
    out
}

/// The package a file outside the suite's directory belongs to: the
/// nearest directory above it with a manifest, by path from the root, and
/// the ecosystems that manifest is for.
pub(crate) struct Packages {
    root: PathBuf,
    cache: HashMap<String, Vec<Ecosystem>>,
}

impl Packages {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            cache: HashMap::new(),
        }
    }

    pub(crate) fn nearest(&mut self, path: &str) -> Option<(String, Vec<Ecosystem>)> {
        let mut dir = path;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            dir = parent;
            if let Some(found) = self.at(dir) {
                return Some((dir.to_string(), found));
            }
        }
        self.at("").map(|found| (String::new(), found))
    }

    fn at(&mut self, dir: &str) -> Option<Vec<Ecosystem>> {
        let root = &self.root;
        let found = self
            .cache
            .entry(dir.to_string())
            .or_insert_with(|| manifests(&root.join(dir)));
        (!found.is_empty()).then(|| found.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_are_docs_and_requirements_are_not() {
        for path in [
            "README.md",
            "docs/guide.mdx",
            "LICENSE",
            "CHANGELOG",
            "notes.txt",
            ".github/ISSUE_TEMPLATE/bug.yml",
            ".gitignore",
        ] {
            assert!(is_docs(path), "{path}");
        }
        for path in [
            "requirements.txt",
            "requirements-dev.txt",
            "src/app.py",
            "docs/conf.py",
            "fixture.json",
        ] {
            assert!(!is_docs(path), "{path}");
        }
    }

    #[test]
    fn lockfiles_tool_versions_and_ci_mean_everything() {
        for path in [
            "package-lock.json",
            "web/pnpm-lock.yaml",
            "uv.lock",
            "requirements-dev.txt",
            "mise.toml",
            "mise.lock",
            ".nvmrc",
            ".github/workflows/ci.yml",
            ".gitlab-ci.yml",
        ] {
            assert!(global_trigger(path).is_some(), "{path}");
        }
        for path in [
            "src/lock.ts",
            "docs/mise.md",
            ".github/dependabot.yml",
            "app/ci.py",
        ] {
            assert_eq!(global_trigger(path), None, "{path}");
        }
    }

    #[test]
    fn finds_the_nearest_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("web/src")).unwrap();
        std::fs::create_dir_all(root.join("tools/x")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("web/package.json"), "{}").unwrap();
        let mut packages = Packages::new(root);
        assert_eq!(
            packages.nearest("web/src/a.ts"),
            Some(("web".into(), vec![Ecosystem::Js]))
        );
        assert_eq!(
            packages.nearest("tools/x/y.rs"),
            Some((String::new(), vec![Ecosystem::Rust]))
        );
        std::fs::remove_file(root.join("Cargo.toml")).unwrap();
        let mut packages = Packages::new(root);
        assert_eq!(packages.nearest("tools/x/y.rs"), None);
    }
}
