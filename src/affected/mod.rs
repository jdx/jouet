//! `jouet test --affected`: the tests a change affects, chosen by each
//! ecosystem's own tool and explained the same way everywhere.
//!
//! The changed set is everything that differs from the merge base with the
//! base branch, committed or not, plus untracked files. Each changed file
//! is classified for each suite ([`classify`]): docs and other packages'
//! code are ignored, lockfiles, tool versions, CI and the suite's own
//! configuration mean the whole suite runs, and the rest goes to the
//! suite's [`Adapter`], which delegates to the test runner's own selection
//! (jest's and vitest's module graphs, pytest-testmon's coverage data) or,
//! failing that, to a static import graph. An adapter that cannot trace a
//! file says so, and the whole suite runs.
//!
//! Affected runs are for iteration. The hooks record them as partial runs,
//! so the full suite still has to pass before an agent claims done or
//! pushes (`crate::verify`).

mod cargo;
mod changes;
mod classify;
mod codegraph;
mod graph;
mod js;
mod project_config;
mod pyimports;
mod python;

pub(crate) use changes::{Base, Change, Status, base, unchanged_since};
pub(crate) use classify::Effect;
use classify::Packages;
use jouet_output::exit::Exit;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The language a package is written in, by its manifest: which packages a
/// suite's adapter can see into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ecosystem {
    /// `package.json`.
    Js,
    /// `pyproject.toml`, `setup.py` or `setup.cfg`.
    Python,
    /// `Cargo.toml`.
    Rust,
    /// `go.mod`.
    Go,
    /// `Gemfile`.
    Ruby,
    /// `pom.xml` or `build.gradle(.kts)`.
    Jvm,
    /// `Package.swift`.
    Swift,
}

/// Why `jouet test` stopped before running anything: the exit code to leave
/// with, what went wrong, and what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Error {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

impl Error {
    pub(crate) fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Where a suite runs: the repository, and the directory the adapter was
/// found in.
#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub root: PathBuf,
    pub dir: PathBuf,
    /// `dir` from the root, with `/`; empty at the root.
    pub rel: String,
}

impl Project {
    /// A path from the root, as a path from the suite's directory, when it
    /// is inside it.
    pub(crate) fn inside<'a>(&self, path: &'a str) -> Option<&'a str> {
        if self.rel.is_empty() {
            return Some(path);
        }
        path.strip_prefix(&self.rel)?.strip_prefix('/')
    }

    /// A path from the root, from the suite's directory instead: `../`
    /// for each level up.
    pub(crate) fn relative_to_dir(&self, path: &str) -> String {
        match self.inside(path) {
            Some(inside) => inside.to_string(),
            None => {
                let up = self.rel.split('/').count();
                format!("{}{path}", "../".repeat(up))
            }
        }
    }

    /// An absolute path, as a path from the root, when it is inside the
    /// repository.
    pub(crate) fn relative_to_root(&self, path: &Path) -> Option<String> {
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let rel = path.strip_prefix(&root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }
}

/// A test the selection picked, by its path from the root, with the chain
/// of imports from a changed file to it when one is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Test {
    pub path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<String>,
}

/// What an adapter selected, and the command that runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub tests: Vec<Test>,
    pub command: Vec<String>,
    /// Something the reader should know about how the tests were chosen.
    pub note: Option<String>,
}

/// One test runner in one directory: how it finds the tests a change
/// affects, and how it runs them. Adding one means implementing this and
/// adding a line to [`ADAPTERS`].
pub(crate) trait Adapter {
    /// Its name, as `--adapter` takes it.
    fn name(&self) -> &'static str;
    /// How it selects, in a few words, such as "vitest's module graph".
    fn precision(&self) -> String;
    /// Whether it can see into packages of this ecosystem: a change in a
    /// package of an ecosystem it does not cover cannot affect its tests.
    /// A native adapter covers one; a fallback built on a cross-language
    /// import graph covers several.
    fn covers(&self, ecosystem: Ecosystem) -> bool;
    /// Whether it can trace a change to this file to the tests that depend
    /// on it.
    fn traces(&self, path: &str) -> bool;
    /// Whether it can trace a deleted file.
    fn traces_deletions(&self) -> bool;
    /// Why a change to this file (a path from its package's directory)
    /// means the whole suite runs, if it does: its configuration and setup
    /// files, such as "a conftest changed".
    fn trigger(&self, path: &str) -> Option<&'static str>;
    /// The command that runs the whole suite, with `extra` passed on.
    fn full(&self, project: &Project, extra: &[String]) -> Result<Vec<String>, Error>;
    /// The tests that depend on `sources` (files it traces), and the command
    /// that runs them.
    fn select(
        &self,
        project: &Project,
        base: &Base,
        sources: &[Change],
        extra: &[String],
    ) -> Result<Selection, Error>;
    /// Environment variables its commands need.
    fn env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The shared exit code for the runner's own.
    fn exit(&self, code: i32) -> Exit {
        if code == 0 { Exit::Ok } else { Exit::Failed }
    }
}

/// An adapter jouet knows: its name, as `--adapter` takes it, how its
/// projects are recognised from their files alone (so the hooks can ask
/// too), and how it is made ready to select.
pub(crate) struct Known {
    pub name: &'static str,
    detect: fn(&Path) -> bool,
    load: fn(&Project) -> Box<dyn Adapter>,
}

/// Every adapter, in the order they are tried. Native adapters, which hand
/// the selection to the runner's own tool, come first. The fallback serving
/// several languages from jouet's code graph (`graph`) goes last, covers
/// only the ecosystems no native adapter covers (Go, Ruby, Java), and
/// reports a changed file it cannot speak for through `traces`, so the
/// whole suite runs.
pub(crate) const ADAPTERS: &[Known] = &[
    Known {
        name: "vitest",
        detect: js::detect_vitest,
        load: |p| Box::new(js::Js::vitest(p)),
    },
    Known {
        name: "jest",
        detect: js::detect_jest,
        load: |p| Box::new(js::Js::jest(p)),
    },
    Known {
        name: "pytest",
        detect: python::detect,
        load: |p| Box::new(python::Pytest::load(p)),
    },
    Known {
        name: "unittest",
        detect: python::detect_unittest,
        load: |p| Box::new(python::Unittest::load(p)),
    },
    Known {
        name: "cargo",
        detect: cargo::detect,
        load: |p| Box::new(cargo::Cargo::load(p)),
    },
    Known {
        name: "graph",
        detect: codegraph::detect,
        load: |p| Box::new(codegraph::Graph::load(p)),
    },
];

/// Every adapter's name, for messages.
pub(crate) fn names() -> String {
    ADAPTERS
        .iter()
        .map(|k| k.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The adapters whose projects are in `dir`, by name.
pub(crate) fn detect(dir: &Path) -> Vec<&'static str> {
    ADAPTERS
        .iter()
        .filter(|k| (k.detect)(dir))
        .map(|k| k.name)
        .collect()
}

/// The nearest directory from `cwd` up to `root` with an adapter's project
/// in it, and the adapters found there.
pub(crate) fn find(cwd: &Path, root: &Path) -> Option<(PathBuf, Vec<&'static str>)> {
    for dir in cwd.ancestors() {
        let found = detect(dir);
        if !found.is_empty() {
            return Some((dir.to_path_buf(), found));
        }
        if dir == root {
            break;
        }
    }
    None
}

/// The adapters in the nearest directory from `dir` up to its work tree's
/// root that has any. Only files are read, for the hooks'
/// `jouet.test.adapters()` probe.
pub(crate) fn near(dir: &Path) -> Vec<&'static str> {
    let root = dir
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(dir);
    find(dir, root).map(|(_, names)| names).unwrap_or_default()
}

/// An adapter, ready to select: its runner found, its configuration read.
fn load(name: &str, project: &Project) -> Option<Box<dyn Adapter>> {
    ADAPTERS
        .iter()
        .find(|k| k.name == name)
        .map(|k| (k.load)(project))
}

/// What a suite will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Run the tests the changes affect.
    Affected,
    /// Run the whole suite; why.
    Full(String),
    /// Run nothing; why.
    Nothing(String),
}

/// A changed file and what it means for a suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileEffect {
    pub path: String,
    pub status: Status,
    pub effect: Effect,
}

/// One adapter's plan.
pub(crate) struct Suite {
    pub adapter: Box<dyn Adapter>,
    pub project: Project,
    pub files: Vec<FileEffect>,
    pub decision: Decision,
    pub tests: Vec<Test>,
    /// What runs; None when nothing does.
    pub command: Option<Vec<String>>,
    pub note: Option<String>,
}

/// Everything `jouet test --affected` will do.
pub(crate) struct Plan {
    pub base: Base,
    pub changes: Vec<Change>,
    pub suites: Vec<Suite>,
}

/// The repository root, the project directory and the adapters to use,
/// from `cwd`; `only` limits them to one adapter.
pub(crate) fn locate(
    cwd: &Path,
    only: Option<&str>,
) -> Result<(PathBuf, Project, Vec<&'static str>), Error> {
    if let Some(name) = only
        && !ADAPTERS.iter().any(|k| k.name == name)
    {
        return Err(Error::new(
            Exit::Usage,
            format!("no adapter `{name}`: there are {}", names()),
        ));
    }
    let root = changes::git(cwd, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .map_err(|e| {
            Error::new(Exit::Precondition, format!("not in a git repository: {e}"))
                .hint("jouet test --affected compares the work tree with a branch, so it needs git")
        })?;
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let root = root.canonicalize().unwrap_or(root);
    let Some((dir, mut names)) = find(&cwd, &root) else {
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "no test runner jouet can select for, from here up to {}: it knows {}",
                root.display(),
                names()
            ),
        )
        .hint("run `jouet test` for the full suite, or run from a package's directory"));
    };
    if let Some(name) = only {
        if !names.contains(&name) {
            return Err(Error::new(
                Exit::Precondition,
                format!(
                    "no {name} project in {} (found {})",
                    dir.display(),
                    names.join(", ")
                ),
            ));
        }
        names.retain(|n| *n == name);
    }
    let rel = dir
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let project = Project {
        root: root.clone(),
        dir,
        rel,
    };
    Ok((root, project, names))
}

/// Plans an affected run from `cwd`.
pub(crate) fn plan(
    cwd: &Path,
    since: Option<&str>,
    only: Option<&str>,
    extra: &[String],
) -> Result<Plan, Error> {
    let (root, project, names) = locate(cwd, only)?;
    let pytest_extra = if names.contains(&"pytest") {
        project_config::pytest_args(&root)?
    } else {
        Vec::new()
    };
    let base = changes::base(&root, since)?;
    let changes = changes::changed(&root, &base.merge_base)?;
    let adapters: Vec<Box<dyn Adapter>> = names.iter().filter_map(|n| load(n, &project)).collect();
    let mut packages = Packages::new(&root);
    let effects: Vec<Vec<FileEffect>> = (0..adapters.len())
        .map(|i| {
            let others: Vec<&dyn Adapter> = adapters
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, a)| a.as_ref())
                .collect();
            changes
                .iter()
                .map(|c| FileEffect {
                    path: c.path.clone(),
                    status: c.status,
                    effect: effect(adapters[i].as_ref(), &others, &project, &mut packages, c),
                })
                .collect()
        })
        .collect();
    let suites = adapters
        .into_iter()
        .zip(effects)
        .map(|(adapter, files)| {
            let configured = adapter.name() == "pytest" && !pytest_extra.is_empty();
            let args = if configured {
                pytest_extra.iter().chain(extra).cloned().collect::<Vec<_>>()
            } else {
                extra.to_vec()
            };
            let mut suite = decide(adapter, project.clone(), files, &base, &changes, extra, &args)?;
            if configured && matches!(suite.decision, Decision::Affected) && suite.command.is_some() {
                let note = "pytest arguments from .jouet/test.toml restrict this affected run; they do not apply to the full suite";
                suite.note = Some(match suite.note {
                    Some(previous) => format!("{previous}; {note}"),
                    None => note.into(),
                });
            }
            Ok(suite)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Plan {
        base,
        changes,
        suites,
    })
}

/// `name`'s whole suite in `project`, for `jouet test` without a test task.
pub(crate) fn full_suite(name: &str, project: &Project, extra: &[String]) -> Result<Suite, Error> {
    let adapter = load(name, project).ok_or_else(|| {
        Error::new(
            Exit::Usage,
            format!("no adapter `{name}`: there are {}", names()),
        )
    })?;
    let command = adapter.full(project, extra)?;
    Ok(Suite {
        adapter,
        project: project.clone(),
        files: Vec::new(),
        decision: Decision::Full("the runner's own, as there is no test task".into()),
        tests: Vec::new(),
        command: Some(command),
        note: None,
    })
}

/// What one changed file means for `adapter`'s suite.
fn effect(
    adapter: &dyn Adapter,
    others: &[&dyn Adapter],
    project: &Project,
    packages: &mut Packages,
    change: &Change,
) -> Effect {
    let path = change.path.as_str();
    if classify::is_docs(path) {
        return Effect::Ignored("documentation".into());
    }
    if let Some(why) = classify::global_trigger(path) {
        return Effect::Full(why);
    }
    let traced = || {
        if change.status == Status::Deleted && !adapter.traces_deletions() {
            Effect::Full(format!(
                "{path} was deleted, and {} cannot trace a file that is gone",
                adapter.name()
            ))
        } else {
            Effect::Source
        }
    };
    if let Some(inside) = project.inside(path) {
        if let Some(why) = adapter.trigger(inside) {
            return Effect::Full(format!("{why} ({path})"));
        }
        if adapter.traces(path) {
            return traced();
        }
        if let Some(other) = others
            .iter()
            .find(|o| o.traces(path) || o.trigger(inside).is_some())
        {
            return Effect::Ignored(format!("{}'s, not {}'s", other.name(), adapter.name()));
        }
        return Effect::Full(format!(
            "{} cannot trace {path} to the tests that use it",
            adapter.name()
        ));
    }
    match packages.nearest(path) {
        Some((dir, ecosystems)) if ecosystems.iter().any(|e| adapter.covers(*e)) => {
            let inside = if dir.is_empty() {
                path
            } else {
                path.strip_prefix(&dir)
                    .and_then(|p| p.strip_prefix('/'))
                    .unwrap_or(path)
            };
            if let Some(why) = adapter.trigger(inside) {
                Effect::Full(format!("{why} in another package ({path})"))
            } else if adapter.traces(path) {
                traced()
            } else {
                Effect::Full(format!(
                    "{} cannot trace {path} to the tests that use it",
                    adapter.name()
                ))
            }
        }
        Some((dir, _)) => Effect::Ignored(format!(
            "in another package ({}), which {} tests cannot import",
            if dir.is_empty() { "." } else { &dir },
            adapter.name()
        )),
        None => Effect::Full(format!(
            "{path} is outside every package, so jouet cannot tell what it affects"
        )),
    }
}

/// Decides what one suite runs.
fn decide(
    adapter: Box<dyn Adapter>,
    project: Project,
    files: Vec<FileEffect>,
    base: &Base,
    changes: &[Change],
    extra: &[String],
    selection_extra: &[String],
) -> Result<Suite, Error> {
    let mut suite = Suite {
        adapter,
        project,
        files,
        decision: Decision::Affected,
        tests: Vec::new(),
        command: None,
        note: None,
    };
    if changes.is_empty() {
        suite.decision = Decision::Nothing(format!("nothing changed since {}", base.name));
        return Ok(suite);
    }
    if let Some(why) = suite.files.iter().find_map(|f| match &f.effect {
        Effect::Full(why) => Some(why.clone()),
        _ => None,
    }) {
        suite.command = Some(suite.adapter.full(&suite.project, extra)?);
        suite.decision = Decision::Full(why);
        return Ok(suite);
    }
    let sources: Vec<Change> = suite
        .files
        .iter()
        .filter(|f| f.effect == Effect::Source)
        .map(|f| Change {
            path: f.path.clone(),
            status: f.status,
        })
        .collect();
    if sources.is_empty() {
        let only_docs = suite
            .files
            .iter()
            .all(|f| f.effect == Effect::Ignored("documentation".into()));
        suite.decision = Decision::Nothing(if only_docs {
            "only documentation changed".into()
        } else {
            format!(
                "no change {} tests can see: only docs and other packages' files",
                suite.adapter.name()
            )
        });
        return Ok(suite);
    }
    let selection = suite
        .adapter
        .select(&suite.project, base, &sources, selection_extra)?;
    suite.note = selection.note;
    if selection.tests.is_empty() {
        suite.decision = Decision::Nothing("no tests selected for the changed files".into());
        return Ok(suite);
    }
    suite.tests = selection.tests;
    suite.command = Some(selection.command);
    Ok(suite)
}

/// The executable `name` in `dir`: `name`, or on Windows the first of
/// `name.exe` and `name.cmd` there.
pub(crate) fn executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ""]
    } else {
        &[""]
    };
    candidates
        .iter()
        .map(|ext| dir.join(format!("{name}{ext}")))
        .find(|p| p.is_file())
}

/// A word as a shell would need it, for showing a command line.
pub(crate) fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=./:@#,+%".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// A command line for showing.
pub(crate) fn show(argv: &[String]) -> String {
    argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
}

// The fixtures install stub runners as shell scripts.
#[cfg(all(test, unix))]
mod tests;
