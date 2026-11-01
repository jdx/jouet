//! The `cargo` adapter: a Rust workspace's tests by crate. A changed file
//! belongs to the workspace member whose directory holds it, and the crates
//! to test are those members and every member that depends on one of them,
//! directly or through others, dev-dependencies included, as `cargo
//! metadata` lists them. They run as `cargo test -p A -p B`.

use super::{Adapter, Base, Change, Ecosystem, Error, Project, Selection, Test};
use jouet_output::exit::Exit;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// A workspace member: its name, its directory from the repository root
/// (empty at the root) and the members it depends on, by index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    name: String,
    dir: String,
    deps: Vec<usize>,
}

/// The workspace as `cargo metadata` describes it, or why it could not.
pub(crate) struct Cargo {
    members: Result<Vec<Member>, String>,
    /// Whether the project's manifest declares a workspace, so the whole
    /// suite is `cargo test --workspace`.
    workspace: bool,
}

impl Cargo {
    pub(crate) fn load(project: &Project) -> Cargo {
        let workspace = std::fs::read_to_string(project.dir.join("Cargo.toml"))
            .is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]"));
        Cargo {
            members: metadata(project).and_then(|m| members(&m, &project.root)),
            workspace,
        }
    }

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
                dir: rel(dir)
                    .ok_or_else(|| format!("{} is outside the repository", dir.display()))?,
                deps,
            })
        })
        .collect()
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
        self.owner(path).is_some()
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
        let changed: BTreeSet<usize> = sources.iter().filter_map(|c| self.owner(&c.path)).collect();
        let chains = dependents(members, &changed);
        let mut picked: Vec<(&str, Vec<String>)> = chains
            .iter()
            .map(|(&i, chain)| {
                let name = members[i].name.as_str();
                (
                    name,
                    chain.iter().map(|&k| members[k].name.clone()).collect(),
                )
            })
            .collect();
        picked.sort_by(|a, b| a.0.cmp(b.0));
        let mut command = vec!["cargo".to_string(), "test".to_string()];
        for (name, _) in &picked {
            command.push("-p".into());
            command.push((*name).to_string());
        }
        command.extend(extra.iter().cloned());
        Ok(Selection {
            tests: picked
                .into_iter()
                .map(|(name, via)| Test {
                    path: name.to_string(),
                    via,
                })
                .collect(),
            command,
            note: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, dir: &str, deps: &[usize]) -> Member {
        Member {
            name: name.into(),
            dir: dir.into(),
            deps: deps.to_vec(),
        }
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
        };
        // The deepest directory owns a file; build output is no crate's.
        assert_eq!(cargo.owner("crates/core/src/lib.rs"), Some(1));
        assert_eq!(cargo.owner("src/main.rs"), Some(2));
        assert_eq!(cargo.owner("crates/util/target/debug/x"), None);
        assert_eq!(cargo.owner("target/debug/x"), None);
    }
}
