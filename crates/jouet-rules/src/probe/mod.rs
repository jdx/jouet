//! Probes: read-only views of jouet's state that rules and recipes can ask
//! for. `jouet.pr.status()` and `jouet.pr.next()` (GitHub),
//! `jouet.ci.last_run()` (the hooks' ledger), `jouet.git.state()` and
//! `jouet.repo()` (plain `git`), and `jouet.env()` (an allowlist).
//!
//! Every probe:
//! - only reads;
//! - is cached for the rest of the hook call, or of the recipe step, that
//!   first asked for it;
//! - runs inside the caller's budget: it gets whatever time the rule (or
//!   step) has left, and a probe that cannot answer in that time returns
//!   nil with the reason, as does any other failure;
//! - returns `(value, nil)`, `(nil, reason)` or `(nil, nil)` when there is
//!   simply nothing to report, such as no ledger yet.
//!
//! The GitHub probes are off in hooks unless the `rules.network` setting
//! turns them on, so a hook never waits on the network by default; recipes
//! always have them.

mod git;
mod github;
mod ledger;

use crate::Shared;
use crate::json::to_lua;
use mlua::{Lua, Table, Value};
use std::path::PathBuf;
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

/// Where the probes look, and whether they may use the network.
#[derive(Clone)]
pub struct Probes {
    network: bool,
    dir: Option<PathBuf>,
    ledger: Option<PathBuf>,
    /// The key the ledger names this work tree by, if it has one.
    repo_key: Option<String>,
    github: Arc<PrFetch>,
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
        }
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

    fn last_run(&self, kind: Option<&str>) -> Option<serde_json::Value> {
        ledger::last_run(self.ledger.as_deref()?, kind, self.repo_key.as_deref())
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

/// Looks a probe up in the cache, or runs it with the time the caller has
/// left and caches what it says.
fn cached(
    shared: &Shared,
    key: &str,
    probe: impl FnOnce(&Probes, Duration) -> Answer,
) -> mlua::Result<Answer> {
    let probes = shared.probes.borrow().clone();
    let Some(probes) = probes else {
        return Ok(Err("probes are not available here".into()));
    };
    if let Some(hit) = shared.cache.borrow().get(key) {
        return Ok(hit.clone());
    }
    let left = shared.remaining();
    if left.is_zero() {
        return Err(mlua::Error::runtime("exceeded its time budget"));
    }
    let answer = probe(&probes, left);
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
        lua.create_function(move |lua, kind: Option<String>| {
            if let Some(kind) = kind.as_deref()
                && !["test", "lint", "build"].contains(&kind)
            {
                return Err(mlua::Error::runtime(format!(
                    "jouet.ci.last_run: kind must be \"test\", \"lint\" or \"build\", not `{kind}`"
                )));
            }
            let key = format!("ci:{}", kind.as_deref().unwrap_or(""));
            let found = cached(&s, &key, |p, _| Ok(p.last_run(kind.as_deref())))?;
            answer(lua, found)
        })?,
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
