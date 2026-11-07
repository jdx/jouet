//! The `cargo` adapter: a Rust workspace's tests by crate. A changed file
//! belongs to the workspace member whose directory holds it, and the crates
//! to test are those members and every member that depends on one of them,
//! directly or through others, dev-dependencies included, as `cargo
//! metadata` lists them. They run as `cargo test -p A -p B`. A file outside
//! every member, such as a shared fixture directory, belongs to the members
//! whose Rust sources name it or a directory holding it.

use super::{Adapter, Base, Change, Ecosystem, Error, Project, Selection, Test};
use jouet_output::exit::Exit;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex, OnceLock};

/// Whether `dir` has a Cargo manifest with a package or a workspace in it.
pub(crate) fn detect(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|t| {
        t.lines()
            .any(|l| matches!(l.trim(), "[package]" | "[workspace]"))
    })
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    id: String,
    manifest_path: PathBuf,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    /// Set for a path dependency: the directory of its manifest.
    #[serde(default)]
    path: Option<PathBuf>,
}

/// A workspace member: its name, how `cargo test -p` names it, its directory
/// from the repository root (empty at the root) and the members it depends
/// on, by index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    name: String,
    spec: String,
    dir: String,
    deps: Vec<usize>,
}

/// The workspace as `cargo metadata` describes it, or why it could not.
pub(crate) struct Cargo {
    members: Result<Vec<Member>, String>,
    /// Whether the project's manifest declares a workspace, so the whole
    /// suite is `cargo test --workspace`.
    workspace: bool,
    /// The repository root, where members' sources are read from.
    root: PathBuf,
    /// [`Cargo::readers`] by path, as each is looked up more than once.
    readers: Mutex<HashMap<String, Vec<usize>>>,
    /// Each member's Rust files, read once for every path
    /// [`Cargo::readers`] looks up.
    sources: OnceLock<Vec<(usize, Vec<Name>)>>,
}

impl Cargo {
    pub(crate) fn load(project: &Project) -> Cargo {
        let workspace = std::fs::read_to_string(project.dir.join("Cargo.toml"))
            .is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]"));
        Cargo {
            members: metadata(project).and_then(|m| members(&m, &project.root)),
            workspace,
            root: project.root.clone(),
            readers: Mutex::default(),
            sources: OnceLock::new(),
        }
    }

    /// The members a changed file reaches: the one holding it or, for a file
    /// outside every member, those that read it ([`Cargo::readers`]).
    fn reached(&self, path: &str) -> Vec<usize> {
        match self.owner(path) {
            Some(i) => vec![i],
            None => self.readers(path),
        }
    }

    /// The members whose Rust sources name `path`, or a directory holding it,
    /// in a string ([`named`]). Only names with a `/` count, as a bare word
    /// such as `"corpus"` is too common to mean the file.
    fn readers(&self, path: &str) -> Vec<usize> {
        let Ok(members) = self.members.as_ref() else {
            return Vec::new();
        };
        if members.iter().any(|m| inside(&m.dir, path)) {
            return Vec::new();
        }
        let mut cache = self.readers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = cache.get(path) {
            return found.clone();
        }
        let found: Vec<usize> = self
            .sources()
            .iter()
            .filter(|(_, names)| names.iter().any(|name| name.reaches(path)))
            .map(|(i, _)| *i)
            .collect::<BTreeSet<usize>>()
            .into_iter()
            .collect();
        cache.insert(path.to_string(), found.clone());
        found
    }

    /// What each Rust file that git tracks or would add names ([`named`]),
    /// with the member that holds it, read on the first lookup.
    fn sources(&self) -> &[(usize, Vec<Name>)] {
        self.sources.get_or_init(|| {
            let Ok(members) = self.members.as_ref() else {
                return Vec::new();
            };
            self.rust_files()
                .into_iter()
                .filter_map(|file| {
                    let i = self.owner(&file)?;
                    let text = std::fs::read_to_string(self.root.join(&file)).ok()?;
                    Some((i, named(&text, &members[i].dir, &file)))
                })
                .collect()
        })
    }

    /// The repository's Rust files that git tracks or would add, from the
    /// root.
    fn rust_files(&self) -> Vec<String> {
        super::changes::git(
            &self.root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                "*.rs",
            ],
        )
        .unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
    }
}

/// A path a Rust file names, from the root: a file or directory, or with a
/// placeholder (`"cases/{name}.json"`) the start of the paths it can be.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Name {
    path: String,
    placeholder: bool,
}

impl Name {
    /// Whether `path` is this one, inside it, or one it can be.
    fn reaches(&self, path: &str) -> bool {
        if self.placeholder {
            path.starts_with(&self.path)
        } else {
            path == self.path
                || path
                    .strip_prefix(&self.path)
                    .is_some_and(|rest| rest.starts_with('/'))
        }
    }
}

/// The paths the strings in `text`, a Rust file at `file` in a member at
/// `dir`, name: each string with a `/`, read from the member's directory
/// (`Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus")`, or after it
/// in a `concat!` or a leading `{}` in `format!`), from the root, or, in `include_str!`, `include_bytes!`
/// or `include!`, from the file's own directory. A string that leads out of
/// the repository names nothing.
fn named(text: &str, dir: &str, file: &str) -> Vec<Name> {
    static INCLUDE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"include(?:_str|_bytes)?!\s*\(\s*$").expect("include macro")
    });
    let here = file.rsplit_once('/').map_or("", |(d, _)| d);
    let mut names = Vec::new();
    for (start, literal) in strings(text) {
        if !literal.contains('/') {
            continue;
        }
        // A leading placeholder is the directory the rest is read from, as
        // in `format!("{}/../corpus", env!("CARGO_MANIFEST_DIR"))`.
        let literal = match literal.strip_prefix('{').and_then(|l| l.split_once('}')) {
            Some((_, rest)) if rest.starts_with('/') => rest,
            _ => literal,
        };
        let literal = literal.trim_start_matches('/');
        let (fixed, placeholder) = match literal.split_once('{') {
            Some((fixed, _)) => (fixed, true),
            None => (literal, false),
        };
        let bases: &[&str] = if INCLUDE.is_match(&text[..start]) {
            &[here]
        } else {
            &[dir, ""]
        };
        for base in bases {
            if let Some(path) = resolve(base, fixed, placeholder)
                && !path.is_empty()
            {
                let name = Name { path, placeholder };
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// The string literals in Rust source `text`, each with where it starts
/// (its `r`, `b` or quote) and what it holds: plain strings with their
/// escapes skipped over, byte strings and raw strings (`r#"..."#`). Char
/// literals and comments are passed over, so a `'"'` or a quote in a
/// comment does not open a string. A plain string with an escape holds a
/// path only by accident and is left out.
fn strings(text: &str) -> Vec<(usize, &str)> {
    let bytes = text.as_bytes();
    let ident = |i: usize| i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
            }
            b'\'' => {
                // A char literal ('x', '\n', '\u{..}'), or else a lifetime.
                let close = if bytes.get(i + 1) == Some(&b'\\') {
                    text[i + 2..].find('\'').map(|n| i + 2 + n)
                } else {
                    text[i + 1..]
                        .char_indices()
                        .nth(1)
                        .filter(|&(_, c)| c == '\'')
                        .map(|(n, _)| i + 1 + n)
                };
                i = close.map_or(i + 1, |c| c + 1);
            }
            b'r' | b'b' if !ident(i) => {
                let mut j = i + 1;
                if bytes[i] == b'b' && bytes.get(j) == Some(&b'r') {
                    j += 1;
                }
                let raw = j > i + 1 || bytes[i] == b'r';
                let hashes = bytes[j..].iter().take_while(|&&b| b == b'#').count();
                if raw && bytes.get(j + hashes) == Some(&b'"') {
                    let open = j + hashes + 1;
                    let close = format!("\"{}", "#".repeat(hashes));
                    let end = text[open..].find(&close).map_or(bytes.len(), |n| open + n);
                    found.push((i, &text[open..end]));
                    i = end + close.len();
                } else {
                    // A byte string's quote is read next.
                    i += 1;
                }
            }
            b'"' => {
                let start = if i > 0 && bytes[i - 1] == b'b' && !ident(i - 1) {
                    i - 1
                } else {
                    i
                };
                let mut j = i + 1;
                let mut escaped = false;
                while j < bytes.len() && bytes[j] != b'"' {
                    if bytes[j] == b'\\' {
                        escaped = true;
                        j += 1;
                    }
                    j += 1;
                }
                let end = j.min(bytes.len());
                if !escaped {
                    found.push((start, &text[i + 1..end]));
                }
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    found
}

/// `rel` from `base`, both from the root, with `.` and `..` taken out, or
/// `None` when it leads out of the root. With a placeholder after `rel`, its
/// last part is kept as the start of a name.
fn resolve(base: &str, rel: &str, placeholder: bool) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|c| !c.is_empty()).collect();
    let (dirs, last) = if placeholder {
        rel.rsplit_once('/').unwrap_or(("", rel))
    } else {
        (rel, "")
    };
    for part in dirs.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    let mut path = parts.join("/");
    if placeholder {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(last);
    }
    Some(path)
}

impl Cargo {
    /// The member whose directory holds `path`, a path from the root: the
    /// deepest one, as a crate can sit inside another's directory.
    fn owner(&self, path: &str) -> Option<usize> {
        let members = self.members.as_ref().ok()?;
        if members
            .iter()
            .any(|m| inside(&m.dir, path) && in_target(&m.dir, path))
        {
            return None;
        }
        members
            .iter()
            .enumerate()
            .filter(|(_, m)| inside(&m.dir, path))
            .max_by_key(|(_, m)| m.dir.len())
            .map(|(i, _)| i)
    }
}

fn inside(dir: &str, path: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `path` is in `dir`'s build output, which is no crate's source.
fn in_target(dir: &str, path: &str) -> bool {
    let rest = if dir.is_empty() {
        path
    } else {
        path.strip_prefix(dir)
            .and_then(|r| r.strip_prefix('/'))
            .unwrap_or(path)
    };
    rest.starts_with("target/")
}

fn metadata(project: &Project) -> Result<Metadata, String> {
    let out = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .current_dir(&project.dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        return Err(format!("cargo metadata failed: {line}"));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("reading cargo metadata: {e}"))
}

/// The workspace's members, with paths from the repository root and their
/// path dependencies on each other.
fn members(meta: &Metadata, root: &Path) -> Result<Vec<Member>, String> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(root);
    let rel = |dir: &Path| -> Option<String> {
        let dir = canon(dir);
        dir.strip_prefix(&root)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    };
    let packages: Vec<&Package> = meta
        .packages
        .iter()
        .filter(|p| meta.workspace_members.contains(&p.id))
        .collect();
    let lock = std::fs::read_to_string(meta.workspace_root.join("Cargo.lock")).unwrap_or_default();
    let dirs: Vec<PathBuf> = packages
        .iter()
        .map(|p| canon(p.manifest_path.parent().unwrap_or(&meta.workspace_root)))
        .collect();
    packages
        .iter()
        .zip(&dirs)
        .map(|(p, dir)| {
            let deps = p
                .dependencies
                .iter()
                .filter_map(|d| d.path.as_deref())
                .filter_map(|path| dirs.iter().position(|m| *m == canon(path)))
                .collect();
            Ok(Member {
                name: p.name.clone(),
                spec: spec(&p.name, &p.version, &lock),
                dir: rel(dir)
                    .ok_or_else(|| format!("{} is outside the repository", dir.display()))?,
                deps,
            })
        })
        .collect()
}

/// How `cargo test -p` names a member: by name, or with its version when
/// the lockfile has another package of that name (tokio's workspace locks a
/// published tokio too), where the name alone is ambiguous.
fn spec(name: &str, version: &str, lock: &str) -> String {
    let entry = format!("name = \"{name}\"");
    if lock.lines().filter(|l| l.trim() == entry).count() > 1 {
        format!("{name}@{version}")
    } else {
        name.to_string()
    }
}

/// The members to test for changes to `changed`: those members and every
/// member that depends on one, each with the chain from a changed member
/// to it, by name.
fn dependents(members: &[Member], changed: &BTreeSet<usize>) -> BTreeMap<usize, Vec<usize>> {
    let mut chains: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    for &i in changed {
        chains.insert(i, vec![i]);
        queue.push_back(i);
    }
    while let Some(i) = queue.pop_front() {
        for (j, m) in members.iter().enumerate() {
            if m.deps.contains(&i) && !chains.contains_key(&j) {
                let mut chain = chains[&i].clone();
                chain.push(j);
                chains.insert(j, chain);
                queue.push_back(j);
            }
        }
    }
    chains
}

impl Adapter for Cargo {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn precision(&self) -> String {
        match &self.members {
            Ok(_) => {
                "the workspace's crates: each changed crate and every crate that depends on it"
                    .into()
            }
            // Every change then runs the whole suite; say why.
            Err(e) => format!("{e}, so it cannot tell which crates a change is in"),
        }
    }

    fn covers(&self, ecosystem: Ecosystem) -> bool {
        ecosystem == Ecosystem::Rust
    }

    fn traces(&self, path: &str) -> bool {
        !self.reached(path).is_empty()
    }

    fn traces_deletions(&self) -> bool {
        true
    }

    fn trigger(&self, path: &str) -> Option<&'static str> {
        match path {
            "Cargo.toml" if self.workspace => Some("the workspace manifest changed"),
            ".cargo/config.toml" | ".cargo/config" => Some("cargo's configuration changed"),
            "rust-toolchain" | "rust-toolchain.toml" => Some("the Rust toolchain changed"),
            _ => None,
        }
    }

    fn full(&self, _project: &Project, extra: &[String]) -> Result<Vec<String>, Error> {
        let mut argv = vec!["cargo".to_string(), "test".to_string()];
        if self.workspace {
            argv.push("--workspace".into());
        }
        argv.extend(extra.iter().cloned());
        Ok(argv)
    }

    fn select(
        &self,
        _project: &Project,
        _base: &Base,
        sources: &[Change],
        extra: &[String],
    ) -> Result<Selection, Error> {
        let members = self
            .members
            .as_ref()
            .map_err(|e| Error::new(Exit::Precondition, e.clone()))?;
        let changed: BTreeSet<usize> = sources.iter().flat_map(|c| self.reached(&c.path)).collect();
        let chains = dependents(members, &changed);
        let mut picked: Vec<(&Member, Vec<String>)> = chains
            .iter()
            .map(|(&i, chain)| {
                (
                    &members[i],
                    chain.iter().map(|&k| members[k].name.clone()).collect(),
                )
            })
            .collect();
        picked.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        let mut command = vec!["cargo".to_string(), "test".to_string()];
        for (member, _) in &picked {
            command.push("-p".into());
            command.push(member.spec.clone());
        }
        command.extend(extra.iter().cloned());
        let note = others(sources);
        Ok(Selection {
            tests: picked
                .into_iter()
                .map(|(member, via)| Test {
                    path: member.name.clone(),
                    via,
                })
                .collect(),
            command,
            full_reason: None,
            note,
        })
    }
}

/// Languages whose files cargo builds no tests from: a crate's directory
/// can hold its bats tests or a Python harness, which `cargo test` leaves
/// alone. Other files (`.luau`, `.toml`, fixtures) can reach a test
/// through `include_str!`, so they stay the crate's.
const NOT_CARGOS: &[&str] = &[
    "py", "bats", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go", "rb",
];

/// How many of those files the note names.
const OTHERS_SHOWN: usize = 4;

/// A note naming the changed files of another language, whose own tests
/// this run does not include.
fn others(sources: &[Change]) -> Option<String> {
    let paths: Vec<&str> = sources
        .iter()
        .map(|c| c.path.as_str())
        .filter(|p| {
            Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| NOT_CARGOS.contains(&e))
        })
        .collect();
    if paths.is_empty() {
        return None;
    }
    let more = paths.len().saturating_sub(OTHERS_SHOWN);
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Some(format!(
        "cargo runs no tests of {}{more}: run their own tests too",
        paths[..paths.len().min(OTHERS_SHOWN)].join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, dir: &str, deps: &[usize]) -> Member {
        Member {
            name: name.into(),
            spec: name.into(),
            dir: dir.into(),
            deps: deps.to_vec(),
        }
    }

    #[test]
    fn files_of_another_language_are_named_as_not_run() {
        let change = |path: &str| Change {
            path: path.into(),
            status: crate::affected::Status::Modified,
        };
        assert_eq!(others(&[change("src/a.rs"), change("rules/x.luau")]), None);
        assert_eq!(
            others(&[
                change("src/a.rs"),
                change("test/x.bats"),
                change("bench/a.py")
            ])
            .as_deref(),
            Some("cargo runs no tests of test/x.bats, bench/a.py: run their own tests too")
        );
    }

    #[test]
    fn a_member_whose_name_the_lockfile_shares_is_named_with_its_version() {
        let lock = "[[package]]\nname = \"tokio\"\nversion = \"1.21.2\"\n\n[[package]]\nname = \"tokio\"\nversion = \"1.54.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"tokio-util\"\nversion = \"0.7.4\"\n";
        assert_eq!(spec("tokio", "1.21.2", lock), "tokio@1.21.2");
        assert_eq!(spec("tokio-util", "0.7.4", lock), "tokio-util");
        assert_eq!(spec("tokio", "1.21.2", ""), "tokio");
    }

    #[test]
    fn a_change_reaches_every_crate_that_depends_on_it() {
        // app -> core -> util; cli -> util; docs stands alone.
        let members = [
            member("util", "crates/util", &[]),
            member("core", "crates/core", &[0]),
            member("app", "", &[1]),
            member("cli", "crates/cli", &[0]),
            member("docs", "crates/docs", &[]),
        ];
        let chains = dependents(&members, &BTreeSet::from([0]));
        assert_eq!(
            chains,
            BTreeMap::from([
                (0, vec![0]),
                (1, vec![0, 1]),
                (2, vec![0, 1, 2]),
                (3, vec![0, 3])
            ])
        );
        let cargo = Cargo {
            members: Ok(members.to_vec()),
            workspace: true,
            root: PathBuf::new(),
            readers: Mutex::default(),
            sources: OnceLock::new(),
        };
        // The deepest directory owns a file; build output is no crate's.
        assert_eq!(cargo.owner("crates/core/src/lib.rs"), Some(1));
        assert_eq!(cargo.owner("src/main.rs"), Some(2));
        assert_eq!(cargo.owner("crates/util/target/debug/x"), None);
        assert_eq!(cargo.owner("target/debug/x"), None);
    }

    #[test]
    fn a_shared_fixture_reaches_the_crates_that_name_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(init.success());
        let write = |path: &str, text: &str| {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "conformance/src/lib.rs",
            r#"fn dir() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/complete") }"#,
        );
        write(
            "crates/fmt/tests/it.rs",
            r#"let case = format!("../../corpus/fmt/{name}.json"); let y = dir.join("../shared/y.json");"#,
        );
        // A bare word names nothing, a longer path is another directory, and
        // a path from the crate's directory ends where it leads.
        write(
            "crates/lib/src/lib.rs",
            r#"const A: &str = "corpus"; const B: &str = "../corpus/completed"; const C: &str = "../../corpus/x.json";"#,
        );
        write(
            "crates/cli/src/main.rs",
            r#"const X: &str = include_str!("../../shared/x.json");"#,
        );
        let cargo = Cargo {
            members: Ok(vec![
                member("conformance", "conformance", &[]),
                member("fmt", "crates/fmt", &[]),
                member("lib", "crates/lib", &[]),
                member("cli", "crates/cli", &[2]),
            ]),
            workspace: true,
            root: root.path().to_path_buf(),
            readers: Mutex::default(),
            sources: OnceLock::new(),
        };
        assert_eq!(cargo.reached("corpus/complete/01-positions.json"), vec![0]);
        assert_eq!(cargo.reached("corpus/fmt/a.json"), vec![1]);
        assert!(cargo.traces("corpus/fmt/a.json"));
        assert!(!cargo.traces("corpus/README.md"));
        assert_eq!(cargo.reached("crates/lib/data/x.json"), vec![2]);
        // A sibling directory, named through their parent; `include_str!`
        // names it from the source file, which ends in the crate's name.
        assert_eq!(cargo.reached("crates/shared/x.json"), vec![3]);
        assert_eq!(cargo.reached("crates/shared/y.json"), vec![1]);
        // `"../../corpus/x.json"` from `crates/lib` is `corpus/x.json`.
        assert_eq!(cargo.reached("corpus/x.json"), vec![2]);
        assert!(!cargo.traces("crates/corpus/x.json"));
    }

    #[test]
    fn strings_keep_track_of_their_quotes() {
        let text = r####"let label = "\n"; let p = "../corpus/x.json";
let q = '"'; // a "quote" in a comment
let r = include_str!(r#"../../../corpus/y.json"#); let b = b"a/b"; let c = br"c/d";
/* "e/f" */ let g = "g/h";"####;
        let found: Vec<&str> = strings(text).into_iter().map(|(_, s)| s).collect();
        assert_eq!(
            found,
            [
                "../corpus/x.json",
                "../../../corpus/y.json",
                "a/b",
                "c/d",
                "g/h"
            ]
        );
        let names = named(text, "crates/a", "crates/a/src/lib.rs");
        let name = |path: &str| Name {
            path: path.into(),
            placeholder: false,
        };
        assert!(names.contains(&name("crates/corpus/x.json")), "{names:?}");
        assert!(names.contains(&name("corpus/y.json")), "{names:?}");
    }

    #[test]
    fn a_name_resolves_from_its_directory() {
        assert_eq!(
            resolve("crates/a", "../corpus/x.json", false).as_deref(),
            Some("crates/corpus/x.json")
        );
        assert_eq!(
            resolve("crates/a", "../../corpus/./x", false).as_deref(),
            Some("corpus/x")
        );
        assert_eq!(resolve("a", "../../x", false), None);
        assert_eq!(
            resolve("a", "../cases/case_", true).as_deref(),
            Some("cases/case_")
        );
        assert_eq!(
            named(
                r#"concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus")"#,
                "cli",
                "cli/src/main.rs"
            ),
            [Name {
                path: "corpus".into(),
                placeholder: false
            }]
        );
        // A leading placeholder is the directory; a later one ends the name.
        assert_eq!(
            named(
                r#"format!("{}/../corpus/complete", env!("CARGO_MANIFEST_DIR")); format!("{dir}/../corpus/{name}.json")"#,
                "cli",
                "cli/src/main.rs"
            ),
            [
                Name {
                    path: "corpus/complete".into(),
                    placeholder: false
                },
                Name {
                    path: "corpus/".into(),
                    placeholder: true
                }
            ]
        );
    }
}
