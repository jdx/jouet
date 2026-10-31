//! Finding routines and reading their headers.
//!
//! A routine is a `.luau` file that starts with a header of `--- key: value`
//! lines and returns a table with a `run(args)` function:
//!
//! ```luau
//! --- name: triage
//! --- about: List my PRs that need me, most urgent first
//! --- capabilities: github.read
//! --- usage: flag "--repo <OWNER/NAME>" help="The repository (default: the git remote)"
//! return { run = function(args) ... end }
//! ```
//!
//! - `name` is optional, and must match the file's name when given.
//! - `about` is required: one line, shown by `joue --list` and `--help`.
//! - `help` lines are the longer description `--help` shows.
//! - `capabilities` lists what it may reach ([`Capability`]).
//! - `usage` lines are a [usage](https://usage.jdx.dev) spec in KDL: its
//!   args and flags, which `--help` and argument parsing use. `--json`, and
//!   for a routine that writes `--yes` and `--dry-run`, are added.
//!
//! Nothing in the header needs the Luau VM, so listing routines, their help
//! and their arguments costs no script run. The usage spec is parsed only for
//! the routine that runs, or when [`Routine::check`] asks.

use crate::capability::Capability;
use serde::Serialize;
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use usage::Spec;

/// The built-in routines, embedded in the binary.
const BUILTIN: &[(&str, &str)] = &[
    ("land", include_str!("../routines/land.luau")),
    ("pr-next", include_str!("../routines/pr-next.luau")),
    ("ship-check", include_str!("../routines/ship-check.luau")),
    ("sweep", include_str!("../routines/sweep.luau")),
];

/// Names `joue` keeps for itself.
pub const RESERVED: &[&str] = &["help", "test", "types"];

/// Where a routine came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    User(PathBuf),
    Repo(PathBuf),
    /// A file in an installed plugin.
    Plugin(PathBuf),
    /// A file in the system directory, or a plugin it pins.
    Enforced(PathBuf),
}

impl Origin {
    /// The file it came from; none for a built-in routine.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Builtin => None,
            Self::User(path) | Self::Repo(path) | Self::Plugin(path) | Self::Enforced(path) => {
                Some(path)
            }
        }
    }

    pub fn describe(&self) -> String {
        self.path()
            .map_or_else(|| "builtin".into(), |path| path.display().to_string())
    }

    /// The directory a routine's fixtures are read from; None for a
    /// built-in, whose fixtures are embedded.
    pub fn dir(&self) -> Option<&Path> {
        self.path().and_then(Path::parent)
    }
}

/// A routine's source, before its header is read.
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub origin: Origin,
    pub code: String,
    /// False for a repository routine whose directory is not trusted: it is
    /// listed, never run.
    pub trusted: bool,
}

/// Whether a routine can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    /// A repository routine in a directory that is not trusted.
    Untrusted,
    Broken(String),
}

#[derive(Debug, Clone)]
pub struct Routine {
    pub name: String,
    pub origin: Origin,
    pub about: String,
    pub help: Option<String>,
    pub capabilities: Vec<Capability>,
    pub state: State,
    /// The usage spec's args and flags, as written in the header.
    pub usage: Vec<String>,
    pub(crate) code: String,
    /// The parsed usage spec, once something needed it.
    spec: OnceCell<Result<Spec, String>>,
}

/// What parsing a routine's command line came to, short of its arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    /// `--help` or `-h`: the page to print.
    Help(String),
    /// The words do not fit the routine's usage spec.
    Usage(String),
}

/// A routine's arguments, and the flags `joue` adds to every routine.
#[derive(Debug, Default)]
pub struct Args {
    /// By name, `-` written as `_`: a string, a boolean, a list of strings
    /// for a repeatable one, or a count for a repeatable switch. Absent when
    /// not given.
    pub values: serde_json::Map<String, serde_json::Value>,
    pub json: bool,
    pub yes: bool,
    pub dry_run: bool,
}

impl Routine {
    /// Reads the header and checks it. A routine whose header is wrong is
    /// kept, as [`State::Broken`], so it can be listed.
    pub fn load(source: Source) -> Self {
        let mut routine = Self {
            name: source.name,
            origin: source.origin,
            about: String::new(),
            help: None,
            capabilities: Vec::new(),
            state: State::Ready,
            usage: Vec::new(),
            code: source.code,
            spec: OnceCell::new(),
        };
        if let Err(e) = routine.read_header() {
            routine.state = State::Broken(e);
            return routine;
        }
        if !source.trusted {
            routine.state = State::Untrusted;
        }
        routine
    }

    /// The parsed usage spec, parsed on first use.
    fn spec(&self) -> Result<&Spec, String> {
        self.spec
            .get_or_init(|| {
                self.spec_kdl().parse::<Spec>().map_err(|e| {
                    format!(
                        "its usage spec does not parse: {}",
                        first_line(&e.to_string())
                    )
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Parses the usage spec, and marks the routine broken if it does not
    /// parse: for listing, where a broken spec should show before a run.
    pub fn check(&mut self) {
        if self.state == State::Ready
            && let Err(e) = self.spec()
        {
            self.state = State::Broken(e);
        }
    }

    fn read_header(&mut self) -> Result<(), String> {
        if RESERVED.contains(&self.name.as_str()) {
            return Err(format!(
                "`{}` is one of joue's own commands; rename the file",
                self.name
            ));
        }
        // A plugin's routine is named `plugin/file`; the plugin's name is
        // checked when it is added.
        let file = self.name.rsplit('/').next().unwrap_or_default();
        let valid_name = !file.is_empty()
            && file
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid_name {
            return Err(
                "name the file in lowercase words joined by dashes, such as `pr-next.luau`".into(),
            );
        }
        let mut help = Vec::new();
        for line in self.code.lines() {
            let Some(rest) = line.strip_prefix("---") else {
                break;
            };
            let Some((key, value)) = rest.trim().split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "name" if value != self.name => {
                    return Err(format!(
                        "the header names `{value}`, but the file is `{}.luau`",
                        self.name
                    ));
                }
                "name" | "author" => {}
                "about" => self.about = value.to_string(),
                "help" => help.push(value.to_string()),
                "usage" => self.usage.push(value.to_string()),
                "capabilities" => {
                    for word in value.split([',', ' ']).filter(|w| !w.is_empty()) {
                        let capability = Capability::parse(word).ok_or_else(|| {
                            let names: Vec<&str> =
                                Capability::ALL.iter().map(|c| c.name()).collect();
                            format!("no capability `{word}`; they are {}", names.join(", "))
                        })?;
                        if !self.capabilities.contains(&capability) {
                            self.capabilities.push(capability);
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "unknown header key `{other}`; the keys are name, about, help, capabilities, usage and author"
                    ));
                }
            }
        }
        if self.about.is_empty() {
            return Err("start the file with a `--- about: ...` header line".into());
        }
        if !help.is_empty() {
            self.help = Some(help.join("\n"));
        }
        Ok(())
    }

    /// Whether it declares a capability that writes.
    pub fn writes(&self) -> bool {
        self.capabilities.iter().any(|c| c.writes())
    }

    /// The routine's usage spec in KDL, with the flags `joue` adds.
    pub fn spec_kdl(&self) -> String {
        // Every flag is the routine's own, so a misspelled one is an error
        // rather than an argument.
        let mut kdl = format!(
            "name {}\nbin {}\nabout {}\nunknown_flags \"error\"\n",
            quote(&self.name),
            quote(&format!("joue {}", self.name)),
            quote(&self.about),
        );
        if let Some(help) = &self.help {
            let long = format!("{}\n\n{help}", self.about);
            kdl.push_str(&format!("long_about {}\n", quote(&long)));
        }
        for line in &self.usage {
            kdl.push_str(line);
            kdl.push('\n');
        }
        kdl.push_str("flag \"--json\" help=\"Print one JSON document instead of text\"\n");
        if self.writes() {
            kdl.push_str(
                "flag \"--yes\" help=\"Make the changes. Without it they are only shown, and joue exits 4\"\n\
                 flag \"--dry-run\" help=\"Show the changes and make none\"\n",
            );
        }
        kdl
    }

    /// Parses the words after the routine's name.
    pub fn parse_args(&self, words: &[String]) -> Result<Args, ArgError> {
        if let State::Broken(e) = &self.state {
            return Err(ArgError::Usage(e.clone()));
        }
        let spec = self.spec().map_err(ArgError::Usage)?;
        let mut input = vec![self.name.clone()];
        input.extend(words.iter().cloned());
        let output = usage::Parser::new(spec)
            .without_running_commands()
            .explain(&input)
            .map_err(|e| ArgError::Usage(first_line(&e.to_string())))?;
        let mut problems = Vec::new();
        for error in &output.errors {
            match error {
                usage::error::UsageErr::Help(_) => {
                    return Err(ArgError::Help(crate::help::render(self, spec)));
                }
                other => problems.push(first_line(&other.to_string())),
            }
        }
        if !problems.is_empty() {
            return Err(ArgError::Usage(problems.join("; ")));
        }
        let mut args = Args::default();
        let named = output
            .args
            .iter()
            .map(|(arg, value)| (arg.name.clone(), value))
            .chain(
                output
                    .flags
                    .iter()
                    .map(|(flag, value)| (flag.name.clone(), value)),
            );
        for (name, value) in named {
            let key = name.replace('-', "_");
            let value = match value {
                usage::parse::ParseValue::Bool(b) => serde_json::Value::Bool(*b),
                usage::parse::ParseValue::String(s) => s.as_str().into(),
                usage::parse::ParseValue::MultiString(v) => v.clone().into(),
                usage::parse::ParseValue::MultiBool(v) => v.iter().filter(|b| **b).count().into(),
            };
            match key.as_str() {
                "json" => args.json = value == serde_json::Value::Bool(true),
                "yes" if self.writes() => args.yes = value == serde_json::Value::Bool(true),
                "dry_run" if self.writes() => args.dry_run = value == serde_json::Value::Bool(true),
                _ => {
                    args.values.insert(key, value);
                }
            }
        }
        Ok(args)
    }
}

/// The routines `joue` can run: built in, the user's, then a repository's.
#[derive(Debug, Clone)]
pub struct Catalog {
    routines: Vec<Routine>,
}

impl Catalog {
    /// Reads every routine's header. As with rules, a routine of
    /// yours or a repository's never takes a built-in one's name, and two
    /// with one name are an error on the second.
    pub fn new(sources: Vec<Source>) -> Self {
        let mut routines: Vec<Routine> = Vec::new();
        for source in sources {
            let mut routine = Routine::load(source);
            if routine.state != State::Untrusted
                && let Some(other) = routines
                    .iter()
                    .find(|r| r.name == routine.name && r.state != State::Untrusted)
            {
                routine.state = State::Broken(match other.origin {
                    Origin::Builtin => format!(
                        "`{}` is a built-in routine, which cannot be replaced; rename this file",
                        routine.name
                    ),
                    _ => format!(
                        "{} already defines `{}`",
                        other.origin.describe(),
                        routine.name
                    ),
                });
            }
            routines.push(routine);
        }
        Self { routines }
    }

    pub fn routines(&self) -> &[Routine] {
        &self.routines
    }

    /// Parses every routine's usage spec, marking those that do not parse
    /// broken, as `joue --list` shows them.
    pub fn check(&mut self) {
        for routine in &mut self.routines {
            routine.check();
        }
    }

    /// The routine that runs under `name`: a ready one first.
    pub fn get(&self, name: &str) -> Option<&Routine> {
        let named = || self.routines.iter().filter(move |r| r.name == name);
        named()
            .find(|r| r.state == State::Ready)
            .or_else(|| named().next())
    }

    /// The names closest to `word`, for "did you mean".
    pub fn similar(&self, word: &str) -> Vec<&str> {
        let mut close: Vec<(usize, &str)> = self
            .routines
            .iter()
            .map(|r| (distance(word, &r.name), r.name.as_str()))
            .filter(|(d, name)| *d <= 2.max(word.len() / 3) || name.starts_with(word))
            .collect();
        close.sort();
        close.dedup_by(|a, b| a.1 == b.1);
        close.into_iter().map(|(_, name)| name).take(3).collect()
    }
}

/// The built-in routines' sources.
pub fn builtin() -> Vec<Source> {
    BUILTIN
        .iter()
        .map(|(name, code)| Source {
            name: (*name).into(),
            origin: Origin::Builtin,
            code: (*code).into(),
            trusted: true,
        })
        .collect()
}

/// A directory of routines to load, after the built-in ones.
pub struct RoutineDir {
    pub dir: PathBuf,
    /// What its files' routines are: `Origin::User`, `Origin::Repo`, ...
    pub origin: fn(PathBuf) -> Origin,
    /// False to list its routines without running them.
    pub trusted: bool,
    /// A plugin's name, which its routines' names start with:
    /// `joue acme/deploy`, so a plugin never takes another routine's name.
    pub namespace: Option<String>,
}

/// Every routine source: built in, then the `*.luau` files of each
/// directory in order. Files that cannot be read are reported, not fatal.
pub fn discover(dirs: &[RoutineDir]) -> (Vec<Source>, Vec<String>) {
    let mut sources = builtin();
    let mut errors = Vec::new();
    for dir in dirs {
        let files = match luau_files(&dir.dir) {
            Ok(files) => files,
            Err(e) => {
                errors.push(format!("reading {}: {e}", dir.dir.display()));
                continue;
            }
        };
        for path in files {
            let code = match std::fs::read_to_string(&path) {
                Ok(code) => code,
                Err(e) => {
                    errors.push(format!("reading {}: {e}", path.display()));
                    continue;
                }
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            sources.push(Source {
                name: match &dir.namespace {
                    Some(namespace) => format!("{namespace}/{stem}"),
                    None => stem,
                },
                origin: (dir.origin)(path),
                code,
                trusted: dir.trusted,
            });
        }
    }
    (sources, errors)
}

/// The `*.luau` files in a directory, sorted by name.
pub fn luau_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "luau") && p.is_file())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    files.sort();
    Ok(files)
}

/// A KDL string.
fn quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim().to_string()
}

/// The edit distance between two words.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + cost);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests;
