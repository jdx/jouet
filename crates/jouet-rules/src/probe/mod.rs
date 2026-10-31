//! Probes: read-only views of jouet's state that rules can ask for.
//! `jouet.pr.status()` and `jouet.pr.next()` (GitHub), `jouet.ci.last_run()`
//! (the hooks' ledger), `jouet.git.state()`, `jouet.git.tree()` and
//! `jouet.repo()` (plain `git`), `jouet.cargo.workspace()` (`Cargo.toml`
//! files), `jouet.test.adapters()` (the test runners `jouet test --affected`
//! can select for, as the binary detects them), `jouet.pitchfork.daemons()`
//! (the daemons a project's pitchfork config declares), and `jouet.env()` (an
//! allowlist).
//!
//! Every probe:
//! - only reads;
//! - is cached for the rest of the hook call that first asked for it;
//! - runs inside the probes' budget, apart from the rule's own time: it
//!   gets whatever is left of it, and a probe that cannot answer in that
//!   time returns nil with the reason, as does any other failure;
//! - returns `(value, nil)`, `(nil, reason)` or `(nil, nil)` when there is
//!   simply nothing to report, such as no ledger yet.
//!
//! The GitHub probes are off in hooks unless the `rules.network` setting
//! turns them on, so a hook never waits on the network by default.

mod cargo;
mod claims;
pub(crate) mod git;
mod github;
mod ledger;
mod pitchfork;
mod stack;

use crate::Shared;
use crate::json::to_lua;
use mlua::{Lua, Table, Value};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// The variables `jouet.env` reads. Anything with TOKEN, SECRET, PASSWORD
/// or KEY in its name is refused even under `JOUET_`.
const ENV_ALLOWED: &[&str] = &[
    "AI_AGENT",
    "CARGO_TARGET_DIR",
    "CI",
    "CLAUDECODE",
    "CODEX_SANDBOX",
    "GITHUB_ACTIONS",
    "GITHUB_BASE_REF",
    "GITHUB_HEAD_REF",
    "GITHUB_REF_NAME",
    "GITHUB_REPOSITORY",
    "LANG",
    "MISE_ENV",
    "NODE_ENV",
    "SHELL",
    "TERM",
    "USER",
    "VIRTUAL_ENV",
];

/// What the GitHub probes return for one PR: `pr status`'s entry and
/// `pr next`'s.
#[derive(Debug, Clone)]
pub(crate) struct PrView {
    pub(crate) status: serde_json::Value,
    pub(crate) next: serde_json::Value,
}

type PrFetch = dyn Fn(Option<&str>) -> Result<Option<PrView>, String> + Send + Sync;

/// The names of the test adapters for a directory.
type Adapters = dyn Fn(&std::path::Path) -> Vec<String> + Send + Sync;

/// Where the probes look, and whether they may use the network.
#[derive(Clone)]
pub struct Probes {
    network: bool,
    dir: Option<PathBuf>,
    ledger: Option<PathBuf>,
    /// The key the ledger names this work tree by, if it has one.
    repo_key: Option<String>,
    github: Arc<PrFetch>,
    adapters: Option<Arc<Adapters>>,
    /// Where `jouet claim` keeps its claims.
    claims: Option<PathBuf>,
}

impl Probes {
    /// Probes of the work tree at `dir` (this process's directory when
    /// None) and of the ledger in `ledger`. `network` lets the GitHub
    /// probes run; GitHub is reached through jouet-github, in the
    /// process's directory.
    pub fn new(dir: Option<PathBuf>, ledger: Option<PathBuf>, network: bool) -> Self {
        Self {
            network,
            dir,
            ledger,
            repo_key: None,
            github: Arc::new(github::fetch),
            adapters: None,
            claims: None,
        }
    }

    /// The same probes, reading `jouet claim`'s claims from `dir` (`<state
    /// dir>/claims`), for `jouet.claims.held()`. Without it, that probe
    /// answers nil and a reason.
    pub fn with_claims(mut self, dir: Option<PathBuf>) -> Self {
        self.claims = dir;
        self
    }

    /// The same probes, with how to find the test adapters for a
    /// directory, for `jouet.test.adapters()`. Without it, that probe
    /// answers nil and a reason.
    pub fn with_test_adapters(
        mut self,
        find: impl Fn(&std::path::Path) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        self.adapters = Some(Arc::new(find));
        self
    }

    /// The same probes, with the key the ledger's writer gives this work
    /// tree (`repo` on each entry), so `jouet.ci.last_run` passes over runs
    /// recorded in other repositories. Without one, every run matches.
    pub fn with_repo_key(mut self, key: Option<String>) -> Self {
        self.repo_key = key;
        self
    }

    fn pr(
        &self,
        arg: Option<String>,
        timeout: Duration,
    ) -> Result<Option<serde_json::Value>, String> {
        if !self.network {
            return Err(
                "network probes are off in hooks; set `rules.network = true` to turn them on"
                    .into(),
            );
        }
        let fetch = self.github.clone();
        let view = bounded(timeout, move || fetch(arg.as_deref()))
            .ok_or("GitHub did not answer within the budget")??;
        Ok(view.map(|v| serde_json::json!({ "status": v.status, "next": v.next })))
    }

    fn last_run(&self, kind: Option<&str>, scope: Option<&str>) -> Option<serde_json::Value> {
        ledger::last_run(
            self.ledger.as_deref()?,
            kind,
            scope,
            self.repo_key.as_deref(),
        )
    }
}

/// Runs `f` on a thread of its own and waits at most `timeout` for it. A
/// call that runs over is left to finish on its own; its answer is dropped.
fn bounded<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(timeout).ok()
}

type Answer = Result<Option<serde_json::Value>, String>;

/// Looks a probe up in the cache, or runs it with what is left of the
/// probes' time and caches what it says. The wait is not charged to the
/// rule's own time. A rule's example may put an answer in the cache when no
/// probes are set up.
fn cached(
    shared: &Shared,
    key: &str,
    probe: impl FnOnce(&Probes, Duration) -> Answer,
) -> mlua::Result<Answer> {
    if let Some(hit) = shared.cache.borrow().get(key) {
        return Ok(hit.clone());
    }
    let probes = shared.probes.borrow().clone();
    let Some(probes) = probes else {
        return Ok(Err("probes are not available here".into()));
    };
    if shared.remaining().is_zero() {
        return Err(mlua::Error::runtime("exceeded its time budget"));
    }
    let left = shared.probe_time_left();
    if left.is_zero() {
        return Ok(Err("the probes have used their time budget".into()));
    }
    let started = std::time::Instant::now();
    let answer = probe(&probes, left);
    shared.spent_on_probe(started.elapsed());
    shared
        .cache
        .borrow_mut()
        .insert(key.to_string(), answer.clone());
    Ok(answer)
}

/// A probe's answer as Luau's two return values.
fn answer(lua: &Lua, answer: Answer) -> mlua::Result<(Value, Option<String>)> {
    match answer {
        Ok(Some(value)) => Ok((to_lua(lua, &value)?, None)),
        Ok(None) => Ok((Value::Nil, None)),
        Err(reason) => Ok((Value::Nil, Some(reason))),
    }
}

fn env_allowed(name: &str) -> bool {
    let secret = ["TOKEN", "SECRET", "PASSWORD", "KEY"]
        .iter()
        .any(|s| name.contains(s));
    !secret && (ENV_ALLOWED.contains(&name) || name.starts_with("JOUET_"))
}

/// Adds the probes to the `jouet` module. `jouet.git` is already a table,
/// which the prelude makes callable.
pub(crate) fn install(
    lua: &Lua,
    jouet: &Table,
    git_table: &Table,
    shared: &Rc<Shared>,
) -> mlua::Result<()> {
    let pr = lua.create_table()?;
    for (name, field) in [("status", "status"), ("next", "next")] {
        let shared = shared.clone();
        pr.set(
            name,
            lua.create_function(move |lua, arg: Option<String>| {
                let key = format!("pr:{}", arg.as_deref().unwrap_or(""));
                let found = cached(&shared, &key, |p, left| p.pr(arg, left))?;
                answer(lua, found.map(|v| v.map(|v| v[field].clone())))
            })?,
        )?;
    }
    jouet.set("pr", pr)?;

    let ci = lua.create_table()?;
    let s = shared.clone();
    ci.set(
        "last_run",
        lua.create_function(
            move |lua, (kind, scope): (Option<String>, Option<String>)| {
                if let Some(kind) = kind.as_deref()
                    && !["test", "lint", "build"].contains(&kind)
                {
                    return Err(mlua::Error::runtime(format!(
                        "jouet.ci.last_run: kind must be \"test\", \"lint\" or \"build\", not `{kind}`"
                    )));
                }
                if let Some(scope) = scope.as_deref()
                    && !["full", "partial"].contains(&scope)
                {
                    return Err(mlua::Error::runtime(format!(
                        "jouet.ci.last_run: scope must be \"full\" or \"partial\", not `{scope}`"
                    )));
                }
                let key = format!(
                    "ci:{}:{}",
                    kind.as_deref().unwrap_or(""),
                    scope.as_deref().unwrap_or("")
                );
                let found = cached(&s, &key, |p, _| {
                    Ok(p.last_run(kind.as_deref(), scope.as_deref()))
                })?;
                answer(lua, found)
            },
        )?,
    )?;
    jouet.set("ci", ci)?;

    let s = shared.clone();
    git_table.set(
        "state",
        lua.create_function(move |lua, ()| {
            let found = cached(&s, "git", |p, left| {
                git::state(p.dir.as_deref(), left).map(Some)
            })?;
            answer(lua, found)
        })?,
    )?;

    let s = shared.clone();
    git_table.set(
        "tree",
        lua.create_function(move |lua, ()| {
            let found = cached(&s, "tree", |p, left| {
                crate::tree::work_tree(p.dir.as_deref(), left).map(|t| Some(t.into()))
            })?;
            answer(lua, found)
        })?,
    )?;

    let cargo_table = lua.create_table()?;
    let s = shared.clone();
    cargo_table.set(
        "workspace",
        lua.create_function(move |lua, dir: Option<String>| {
            let key = format!("cargo:{}", dir.as_deref().unwrap_or(""));
            let found = cached(&s, &key, |p, _| {
                let base = match &p.dir {
                    Some(dir) => dir.clone(),
                    None => std::env::current_dir().map_err(|e| e.to_string())?,
                };
                cargo::workspace(&crate::resolve_dir(&base, dir.as_deref().unwrap_or(".")))
            })?;
            answer(lua, found)
        })?,
    )?;
    jouet.set("cargo", cargo_table)?;

    let test_table = lua.create_table()?;
    let s = shared.clone();
    test_table.set(
        "adapters",
        lua.create_function(move |lua, ()| {
            let found = cached(&s, "test:adapters", |p, _| {
                let find = p
                    .adapters
                    .clone()
                    .ok_or("test adapters cannot be detected here")?;
                let dir = match &p.dir {
                    Some(dir) => dir.clone(),
                    None => std::env::current_dir().map_err(|e| e.to_string())?,
                };
                Ok(Some(serde_json::json!(find(&dir))))
            })?;
            answer(lua, found)
        })?,
    )?;
    jouet.set("test", test_table)?;

    let pitchfork_table = lua.create_table()?;
    let s = shared.clone();
    pitchfork_table.set(
        "daemons",
        lua.create_function(move |lua, ()| {
            let found = cached(&s, "pitchfork:daemons", |p, _| {
                let dir = match &p.dir {
                    Some(dir) => dir.clone(),
                    None => std::env::current_dir().map_err(|e| e.to_string())?,
                };
                pitchfork::daemons(&dir)
            })?;
            answer(lua, found)
        })?,
    )?;
    jouet.set("pitchfork", pitchfork_table)?;

    let claims_table = lua.create_table()?;
    let s = shared.clone();
    claims_table.set(
        "held",
        lua.create_function(move |lua, paths: Vec<String>| {
            let found = cached(&s, "claims", |p, _| {
                let state = p.claims.as_deref().ok_or("claims are not available here")?;
                let dir = match &p.dir {
                    Some(dir) => dir.clone(),
                    None => std::env::current_dir().map_err(|e| e.to_string())?,
                };
                claims::list(state, &dir)
            })?;
            // An example's claims come without probes, and so without a
            // work tree to place its paths in.
            let dir = s
                .probes
                .borrow()
                .as_ref()
                .map(|p| p.dir.clone().or_else(|| std::env::current_dir().ok()));
            let held = found
                .map(|list| list.map(|list| claims::held(&list, &paths, dir.flatten().as_deref())));
            answer(lua, held)
        })?,
    )?;
    jouet.set("claims", claims_table)?;

    let s = shared.clone();
    git_table.set(
        "changed",
        lua.create_function(move |lua, scope: Option<String>| {
            let scope = scope.unwrap_or_else(|| "staged".into());
            if !["staged", "tracked"].contains(&scope.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "jouet.git.changed: scope must be \"staged\" or \"tracked\", not `{scope}`"
                )));
            }
            let key = format!("changed:{scope}");
            let found = cached(&s, &key, |p, left| {
                claims::changed(p.dir.as_deref(), &scope, left).map(Some)
            })?;
            answer(lua, found)
        })?,
    )?;

    let stack_table = lua.create_table()?;
    let s = shared.clone();
    stack_table.set(
        "layer",
        lua.create_function(move |lua, ()| {
            let found = cached(&s, "stack:layer", |p, left| {
                stack::layer(p.dir.as_deref(), left)
            })?;
            answer(lua, found)
        })?,
    )?;
    jouet.set("stack", stack_table)?;

    let s = shared.clone();
    git_table.set(
        "ahead",
        lua.create_function(move |lua, (branch, of): (String, String)| {
            // An example answers for any branch.
            let stubbed = s.cache.borrow().get("ahead:*").cloned();
            if let Some(answer_) = stubbed {
                return answer(lua, answer_);
            }
            let key = format!("ahead:{branch}:{of}");
            let found = cached(&s, &key, |p, left| {
                stack::ahead(p.dir.as_deref(), &branch, &of, left)
            })?;
            answer(lua, found)
        })?,
    )?;

    let s = shared.clone();
    jouet.set(
        "repo",
        lua.create_function(move |lua, ()| {
            let found = cached(&s, "repo", |p, left| {
                git::repo(p.dir.as_deref(), left).map(Some)
            })?;
            answer(lua, found)
        })?,
    )?;

    jouet.set(
        "env",
        lua.create_function(|_, name: String| {
            if !env_allowed(&name) {
                return Err(mlua::Error::runtime(format!(
                    "jouet.env: `{name}` is not on the allowlist (see `jouet rule types`)"
                )));
            }
            Ok(std::env::var(&name).ok())
        })?,
    )?;
    Ok(())
}

impl Probes {
    /// The directory the probes look in, when they were given one.
    pub(crate) fn dir(&self) -> Option<PathBuf> {
        self.dir.clone()
    }
}

/// Whether one of `files` is in `dir` or a directory above it.
pub(crate) fn has_file(files: &[String], dir: &Path) -> bool {
    dir.ancestors()
        .any(|d| files.iter().any(|f| d.join(f).is_file()))
}

#[cfg(test)]
impl Probes {
    /// Probes whose GitHub answers come from `fetch`.
    pub(crate) fn with_github(
        mut self,
        fetch: impl Fn(Option<&str>) -> Result<Option<PrView>, String> + Send + Sync + 'static,
    ) -> Self {
        self.github = Arc::new(fetch);
        self
    }
}

#[cfg(test)]
mod tests;
