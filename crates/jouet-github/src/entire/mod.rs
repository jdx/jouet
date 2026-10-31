//! Entire (entire.io) on top of a PR: its "Entire Gates" check, and the
//! review findings on the PR's trail.
//!
//! Entire sits beside the forge, not inside it. The gates are read from the
//! check run's summary with the forge's own token; the findings, and how far
//! behind the branch is, come from the `entire` CLI. That CLI is used only
//! when it is on PATH and logged in (`entire auth status`); otherwise
//! commands carry on without it and say why only in `--json`. Every call has
//! a timeout, and a read is made once per process however many PRs or
//! commands ask for it, so one command never asks twice about the same head.
//!
//! The CLI's help is the reference for what is called here: `entire
//! agent-help trail` and `entire agent-help trail finding`. Credentials
//! never pass through jouet: it runs `entire`, which reads its own.

pub mod findings;
pub mod gates;

pub use findings::{Brief, Finding, Trail};
pub use gates::{CHECK, Gate, Gates};

use crate::forge::Forge;
use crate::pr::target::Repo;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long one `entire` call may take when jouet's settings were not
/// passed in: `[entire] timeout_ms`'s default.
const TIMEOUT: Duration = Duration::from_secs(5);

/// jouet's `[entire]` settings, passed in by [`configure`].
static SETTINGS: OnceLock<(bool, Duration)> = OnceLock::new();

/// Whether jouet may run `entire` at all, and how long one call may take:
/// jouet's `[entire] enabled` and `timeout_ms` settings (`JOUET_ENTIRE`,
/// `JOUET_ENTIRE_TIMEOUT_MS`). Set once, before anything probes.
pub fn configure(enabled: bool, timeout: Duration) {
    let _ = SETTINGS.set((enabled, timeout));
}
/// Findings per page.
const PAGE: &str = "100";
/// Trails listed when looking for a branch's trail by name.
const TRAILS: &str = "100";

/// Whether Entire could be asked, for `--json`: `{available: true}`, or
/// `{available: false, reason}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Availability {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Availability {
    pub fn of(probed: &Result<Entire, String>) -> Self {
        Self {
            available: probed.is_ok(),
            reason: probed.as_ref().err().cloned(),
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
        }
    }
}

/// What to do with a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Resolve,
    Dismiss,
    Reopen,
}

impl Change {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Dismiss => "dismiss",
            Self::Reopen => "reopen",
        }
    }

    pub fn done(self) -> &'static str {
        match self {
            Self::Resolve => "resolved",
            Self::Dismiss => "dismissed",
            Self::Reopen => "reopened",
        }
    }
}

/// Findings read from a trail, and whether that is all of them.
#[derive(Debug, Clone, Default)]
pub struct Findings {
    pub findings: Vec<Finding>,
    /// Entire said more pages follow but gave no way to fetch them.
    pub incomplete: bool,
}

/// The `entire` CLI, found and logged in.
pub struct Entire {
    timeout: Duration,
    reads: Mutex<HashMap<Vec<String>, Result<Value, String>>>,
}

/// Entire's selector for a repository: `gh/OWNER/NAME` on GitHub,
/// `et/OWNER/NAME` on Entire's own forge (which takes no clone URL), else
/// its clone URL, which Entire takes for other forges.
pub fn selector(repo: &Repo) -> String {
    match repo.host.as_deref() {
        None => format!("gh/{repo}"),
        Some(_) if repo.forge() == Some(Forge::Entire) => format!("et/{repo}"),
        Some(host) => format!("https://{host}/{repo}.git"),
    }
}

impl Entire {
    /// `entire`, if it is on PATH and logged in; otherwise why not, in words
    /// fit for `entire: {available: false, reason}`.
    pub fn probe() -> Result<Self, String> {
        let (enabled, timeout) = SETTINGS.get().copied().unwrap_or((true, TIMEOUT));
        if !enabled {
            return Err("turned off by [entire] enabled = false (JOUET_ENTIRE)".into());
        }
        let entire = Self {
            timeout,
            reads: Mutex::new(HashMap::new()),
        };
        let status = match entire.run(&["auth", "status", "--json"]) {
            Ok(status) => status,
            Err(Run::Missing) => return Err("entire is not on PATH".into()),
            Err(e) => return Err(format!("entire auth status: {}", e.message())),
        };
        let doc: Value = serde_json::from_str(&status)
            .map_err(|_| "entire auth status --json printed no JSON".to_string())?;
        if doc["logged_in"].as_bool() == Some(true) {
            Ok(entire)
        } else {
            Err("entire is not logged in (see `entire auth status`)".into())
        }
    }

    /// The trail for a PR: the one the gates check links to when there is
    /// one, else the one tracking the head branch (or, once merged, the one
    /// that tracked it). `None` when no trail does.
    pub fn trail(
        &self,
        repo: &Repo,
        linked: Option<u64>,
        branch: &str,
    ) -> Result<Option<Trail>, String> {
        let repo = selector(repo);
        if let Some(n) = linked {
            let doc = self.read(&["trail", "show", &n.to_string(), "--json", "--repo", &repo])?;
            return Ok(Trail::parse(&doc));
        }
        match self.read(&[
            "trail", "show", "--branch", branch, "--json", "--repo", &repo,
        ]) {
            Ok(doc) => return Ok(Trail::parse(&doc)),
            Err(e) if e.contains("no trail") => {}
            // A repository Entire does not know has no trails.
            Err(e) if e.contains("not onboarded") => return Ok(None),
            Err(e) => return Err(e),
        }
        // A merged trail keeps its branch only as `original_branch`.
        let list = self.read(&[
            "trail", "list", "--status", "any", "--json", "-n", TRAILS, "--repo", &repo,
        ])?;
        let mut matching: Vec<&Value> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["branch"] == branch || t["original_branch"] == branch)
            .collect();
        matching.sort_by_key(|t| t["updated_at"].as_str().unwrap_or_default().to_string());
        Ok(matching.last().and_then(|t| Trail::parse(t)))
    }

    /// A trail's findings: open and current ones, or with `all` every one,
    /// resolved, dismissed and stale included.
    pub fn findings(&self, repo: &Repo, trail: u64, all: bool) -> Result<Findings, String> {
        let repo = selector(repo);
        let trail = trail.to_string();
        let mut args = vec![
            "trail", "finding", "--trail", &trail, "--json", "-n", PAGE, "--repo", &repo,
        ];
        if all {
            args.extend([
                "--status",
                "any",
                "--freshness",
                "any",
                "--include-dismissed",
            ]);
        }
        let mut out = Findings::default();
        let mut cursor: Option<String> = None;
        loop {
            let mut page_args = args.clone();
            if let Some(c) = &cursor {
                page_args.extend(["--cursor", c.as_str()]);
            }
            let doc = self.read(&page_args)?;
            let (page, more) = findings::parse(&doc)?;
            out.findings.extend(page);
            if !more {
                return Ok(out);
            }
            let next = ["next_cursor", "nextCursor", "cursor"]
                .iter()
                .find_map(|k| doc[k].as_str().filter(|c| !c.is_empty()))
                .map(str::to_string);
            match next {
                Some(next) if cursor.as_ref() != Some(&next) => cursor = Some(next),
                _ => {
                    out.incomplete = true;
                    return Ok(out);
                }
            }
        }
    }

    /// Resolves, dismisses or reopens a finding, recording `message` as the
    /// reason.
    pub fn change(
        &self,
        repo: &Repo,
        trail: u64,
        id: &str,
        change: Change,
        message: Option<&str>,
    ) -> Result<(), String> {
        let args = change_args(repo, trail, id, change, message);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&args).map(|_| ()).map_err(|e| e.message())
    }

    /// `entire ARGS`, uncached, returning what it printed: a write, or a
    /// read that must see changes, as a wait's polls must.
    pub fn output(&self, args: &[&str]) -> Result<String, String> {
        self.run(args).map_err(|e| e.message())
    }

    /// A read, made once per process: `entire ARGS`, which prints one JSON
    /// document.
    pub fn read(&self, args: &[&str]) -> Result<Value, String> {
        let key: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
        if let Some(done) = self.reads.lock().ok().and_then(|r| r.get(&key).cloned()) {
            return done;
        }
        let result = self.run(args).map_err(|e| e.message()).and_then(|out| {
            serde_json::from_str(&out).map_err(|e| {
                format!(
                    "entire {} printed JSON jouet could not read: {e}",
                    args[..2].join(" ")
                )
            })
        });
        if let Ok(mut reads) = self.reads.lock() {
            reads.insert(key, result.clone());
        }
        result
    }

    /// Runs `entire ARGS` with no stdin and a deadline, returning stdout.
    fn run(&self, args: &[&str]) -> Result<String, Run> {
        let mut child = Command::new("entire")
            .args(args)
            // Plain prompts and no colour, should anything ask or paint.
            .env("ACCESSIBLE", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Run::Missing,
                _ => Run::Failed(e.to_string()),
            })?;
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");
        let out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = out.read_to_string(&mut s);
            s
        });
        let err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Run::TimedOut(self.timeout));
                }
                Err(e) => return Err(Run::Failed(e.to_string())),
            }
        };
        let (out, err) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        if status.success() {
            Ok(out)
        } else {
            let line = err
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .trim_start_matches("Error: ")
                .to_string();
            Err(Run::Failed(if line.is_empty() {
                format!("exited with {status}")
            } else {
                line
            }))
        }
    }
}

/// The arguments of `entire trail finding resolve|dismiss|reopen`.
pub fn change_args(
    repo: &Repo,
    trail: u64,
    id: &str,
    change: Change,
    message: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = ["trail", "finding", change.verb()].map(String::from).into();
    args.extend([trail.to_string(), id.to_string()]);
    if let Some(m) = message {
        args.extend(["-m".into(), m.to_string()]);
    }
    args.extend(["--repo".into(), selector(repo)]);
    args
}

/// How a run of `entire` went wrong.
enum Run {
    Missing,
    TimedOut(Duration),
    Failed(String),
}

impl Run {
    fn message(&self) -> String {
        match self {
            Self::Missing => "entire is not on PATH".into(),
            Self::TimedOut(t) if t.as_secs() == 0 => {
                format!("entire took longer than {}ms", t.as_millis())
            }
            Self::TimedOut(t) => format!("entire took longer than {}s", t.as_secs()),
            Self::Failed(m) => m.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_is_named_for_entire_by_its_forge() {
        assert_eq!(selector(&Repo::parse("jdx/mise").unwrap()), "gh/jdx/mise");
        assert_eq!(
            selector(&Repo::parse("gitlab.com/group/sub/app").unwrap()),
            "https://gitlab.com/group/sub/app.git"
        );
        assert_eq!(
            selector(&Repo::parse("entire.com/jdx/notes").unwrap()),
            "et/jdx/notes"
        );
    }
}
