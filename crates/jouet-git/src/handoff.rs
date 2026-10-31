//! `jouet handoff` and `jouet pickup`: facts about a checkout, written for
//! the next session, and a check that they still hold.
//!
//! A handoff records facts only: the branch and its upstream, HEAD, an
//! operation in progress, uncommitted and untracked files, the PR and its
//! `pr status` attention codes, the session's last test, lint and build
//! runs, background processes running in the worktree, and its `jouet job`s.
//! What the work is and what to do next is the agent's to write; this is
//! what it can stand on.
//!
//! The markdown file carries the same facts as JSON in a trailing HTML
//! comment, so `pickup` reads exactly what was written, and a reader sees
//! only the markdown.

use crate::procs::{self, Process};
use crate::session::{self, CheckRun};
use crate::{Error, Git, nul_fields};
use jouet_output::time;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const DATA_OPEN: &str = "<!-- jouet-handoff";
const DATA_CLOSE: &str = "-->";
/// Entries a markdown list shows before saying how many more there are.
const LIST_LIMIT: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub created_at: String,
    pub worktree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The full SHA; absent on an unborn branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_subject: Option<String>,
    /// `merge`, `rebase`, `am`, `cherry-pick`, `revert` or `bisect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    /// Tracked files with uncommitted changes, staged or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untracked: Vec<String>,
    /// SHA-256 of the staged and the unstaged changes, so `pickup` notices
    /// edits to files that were already changed, staged or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<Pr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<Process>,
    /// The worktree's `jouet job`s; the caller adds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<Job>,
    /// Facts that could not be gathered, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Change {
    /// Two letters as in `git status --short`: the index, then the working
    /// tree.
    pub status: String,
    pub path: String,
    /// The old path of a rename or copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<Attention>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    pub code: String,
    pub message: String,
}

/// A `jouet job`, as `jouet job ls --json` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// `running`, `exited`, `stopped` or `gone`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub pid: u32,
    /// The command, as a shell would need it written.
    pub command: String,
    pub log: String,
}

impl Job {
    fn describe(&self) -> String {
        match self.exit_code {
            Some(code) => format!("exited {code}"),
            None => self.state.clone(),
        }
    }
}

/// A job a handoff recorded, as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobNow {
    pub id: String,
    /// Its state when the handoff was written, such as `running` or
    /// `exited 1`.
    pub then: String,
    /// Its state now, or `gone` when this worktree no longer has it.
    pub now: String,
}

/// How the jobs a handoff recorded are doing, given `now`, this worktree's
/// jobs today; and the drift: a job that was running and has since ended,
/// or gone.
pub fn jobs_now(then: &[Job], now: &[Job]) -> (Vec<JobNow>, Vec<Drift>) {
    let mut states = Vec::new();
    let mut drift = Vec::new();
    for j in then {
        let current = now.iter().find(|n| n.id == j.id && n.pid == j.pid);
        states.push(JobNow {
            id: j.id.clone(),
            then: j.describe(),
            now: current.map_or_else(|| "gone".into(), Job::describe),
        });
        if j.state != "running" {
            continue;
        }
        let message = match current {
            Some(n) if n.state == "running" => continue,
            Some(n) => format!(
                "job `{}` has {} since; `jouet job logs {}` shows how it ended",
                j.id,
                match n.exit_code {
                    Some(code) => format!("exited {code}"),
                    None => format!("ended ({})", n.state),
                },
                j.id
            ),
            None => format!(
                "job `{}` is no longer in this worktree, or was started again",
                j.id
            ),
        };
        drift.push(Drift {
            code: "job_ended",
            message,
        });
    }
    (states, drift)
}

/// Where the facts beyond git come from.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The sessions directory and the session ID, for the last check runs.
    pub session: Option<(PathBuf, String)>,
    /// Whether to list background processes.
    pub processes: bool,
}

/// The handoff file for this worktree when none is named:
/// `<git dir>/jouet/handoff.md`, private to the worktree and never part of
/// a commit.
pub fn default_path(git: &Git) -> Result<PathBuf, Error> {
    Ok(git.git_dir()?.join("jouet").join("handoff.md"))
}

/// Gathers the git facts, the session's checks and the processes. The PR is
/// the caller's to add, since it takes a network call.
pub fn collect(git: &Git, opts: &Options) -> Result<Handoff, Error> {
    let top = git.toplevel()?;
    let mut h = git_facts(git, &top)?;
    h.created_at = time::format_utc(time::now());
    match &opts.session {
        Some((dir, id)) => match session::last_checks(dir, id) {
            Some(runs) => h.checks = runs,
            None => h.notes.push(format!(
                "checks: none recorded for session {id} (jouet setup's hooks record them)"
            )),
        },
        None => h
            .notes
            .push("checks: no session ID, so no recorded test or lint runs".into()),
    }
    if opts.processes {
        match procs::in_worktree(&top) {
            Some(found) => h.processes = found,
            None => h
                .notes
                .push("processes: not listed on this platform (Linux only)".into()),
        }
    }
    Ok(h)
}

/// What git says about the checkout.
fn git_facts(git: &Git, top: &Path) -> Result<Handoff, Error> {
    let status = git.text(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=all",
    ])?;
    let mut h = Handoff {
        worktree: top.display().to_string(),
        ..Handoff::default()
    };
    let mut fields = nul_fields(&status);
    while let Some(field) = fields.next() {
        if let Some(header) = field.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => h.head = Some(value.into()),
                "branch.head" if value != "(detached)" => h.branch = Some(value.into()),
                "branch.upstream" => h.upstream = Some(value.into()),
                "branch.ab" => {
                    let mut counts = value
                        .split(' ')
                        .map(|n| n.trim_start_matches(['+', '-']).parse().ok());
                    h.ahead = counts.next().flatten();
                    h.behind = counts.next().flatten();
                }
                _ => {}
            }
            continue;
        }
        let (kind, rest) = field.split_at(1);
        let rest = rest.trim_start();
        match kind {
            "1" => h.changes.push(entry(rest, 6, None)),
            "2" => {
                let from = fields.next().map(str::to_string);
                h.changes.push(entry(rest, 7, from));
            }
            "u" => h.changes.push(entry(rest, 8, None)),
            "?" => h.untracked.push(rest.to_string()),
            _ => {}
        }
    }
    if h.head.is_some() {
        h.head_subject = git.line(&["log", "-1", "--format=%s", "HEAD"]);
    }
    h.diff_sha256 = fingerprint(git)?;
    h.operation = operation(&git.git_dir()?);
    Ok(h)
}

/// SHA-256 of the staged changes (the index against HEAD, or against
/// nothing on an unborn branch) and, separately, the unstaged ones (the
/// working tree against the index); `None` when there are neither. Hashing
/// the two apart means restaging a file changes the fingerprint even when
/// its working-tree contents and its status letters stay the same.
fn fingerprint(git: &Git) -> Result<Option<String>, Error> {
    let diff = ["diff", "--binary", "--no-ext-diff", "--no-color"];
    let staged = git.text(&[&diff[..], &["--cached"]].concat())?;
    let unstaged = git.text(&diff)?;
    if staged.is_empty() && unstaged.is_empty() {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    for (name, text) in [("staged", &staged), ("unstaged", &unstaged)] {
        hash.update(format!("{name} {}\n", text.len()));
        hash.update(text.as_bytes());
    }
    Ok(Some(hex(&hash.finalize())))
}

/// One `git status --porcelain=v2` entry: XY, then `fields` more fields
/// before the path, which may hold spaces.
fn entry(rest: &str, fields: usize, from: Option<String>) -> Change {
    let mut parts = rest.splitn(fields + 2, ' ');
    let xy = parts.next().unwrap_or("..").replace('.', " ");
    let path = parts.nth(fields).unwrap_or_default().to_string();
    Change {
        status: xy,
        path,
        from,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The operation git is in the middle of, from the marker files it leaves
/// in the git directory.
fn operation(git_dir: &Path) -> Option<String> {
    let has = |name: &str| git_dir.join(name).exists();
    let op = if has("rebase-merge") || (has("rebase-apply") && !has("rebase-apply/applying")) {
        "rebase"
    } else if has("rebase-apply") {
        "am"
    } else if has("MERGE_HEAD") {
        "merge"
    } else if has("CHERRY_PICK_HEAD") {
        "cherry-pick"
    } else if has("REVERT_HEAD") {
        "revert"
    } else if has("BISECT_LOG") {
        "bisect"
    } else {
        return None;
    };
    Some(op.into())
}

impl Handoff {
    /// The handoff as markdown, followed by its data unless `data` is false.
    pub fn markdown(&self, data: bool) -> String {
        let mut s = String::new();
        let title = self.branch.as_deref().unwrap_or("detached HEAD");
        let _ = writeln!(s, "# Handoff: {title}\n");
        let _ = writeln!(s, "Written {} in `{}`.\n", self.created_at, self.worktree);
        let mut branch = format!("- **Branch:** {title}");
        if let Some(up) = &self.upstream {
            let _ = write!(branch, ", tracking {up}");
            if let (Some(a), Some(b)) = (self.ahead, self.behind) {
                let _ = write!(branch, " ({a} ahead, {b} behind)");
            }
        } else {
            branch.push_str(", no upstream");
        }
        let _ = writeln!(s, "{branch}");
        match (&self.head, &self.head_subject) {
            (Some(head), subject) => {
                let _ = writeln!(
                    s,
                    "- **HEAD:** {} {}",
                    head.get(..12).unwrap_or(head),
                    subject.as_deref().unwrap_or("")
                );
            }
            (None, _) => s.push_str("- **HEAD:** none yet (unborn branch)\n"),
        }
        if let Some(op) = &self.operation {
            let _ = writeln!(s, "- **In progress:** a {op}");
        }
        match &self.pr {
            Some(pr) => {
                let _ = writeln!(
                    s,
                    "- **PR:** {}#{} ({}) {}: {}",
                    pr.repo, pr.number, pr.state, pr.url, pr.title
                );
                if pr.attention.is_empty() {
                    s.push_str("  - no attention codes\n");
                }
                for a in &pr.attention {
                    let _ = writeln!(s, "  - `{}`: {}", a.code, a.message);
                }
            }
            None => s.push_str("- **PR:** none found\n"),
        }
        if !self.checks.is_empty() {
            s.push_str("- **Last checks:**\n");
            for c in &self.checks {
                let verdict = if c.exit_code == 0 { "passed" } else { "failed" };
                let _ = writeln!(
                    s,
                    "  - {} {verdict} (exit {}): `{}`",
                    c.kind, c.exit_code, c.command
                );
            }
        }
        let changes: Vec<String> = self
            .changes
            .iter()
            .map(|c| match &c.from {
                Some(from) => format!("`{}` {} (from {from})", c.status, c.path),
                None => format!("`{}` {}", c.status, c.path),
            })
            .collect();
        list(&mut s, "Uncommitted", &changes, data);
        let untracked: Vec<String> = self.untracked.clone();
        list(&mut s, "Untracked", &untracked, data);
        let processes: Vec<String> = self
            .processes
            .iter()
            .map(|p| format!("{} `{}`: attach with `{}`", p.pid, p.command, p.attach()))
            .collect();
        list(&mut s, "Background processes", &processes, data);
        let jobs: Vec<String> = self
            .jobs
            .iter()
            .map(|j| {
                let follow = if j.state == "running" {
                    format!(", `jouet wait job {}`", j.id)
                } else {
                    String::new()
                };
                format!(
                    "`{}` {}, pid {}: `{}`; `jouet job logs {}`{follow}",
                    j.id,
                    j.describe(),
                    j.pid,
                    j.command,
                    j.id
                )
            })
            .collect();
        list(&mut s, "Jobs", &jobs, data);
        if !self.notes.is_empty() {
            s.push_str("- **Not gathered:**\n");
            for n in &self.notes {
                let _ = writeln!(s, "  - {n}");
            }
        }
        if data {
            let json = serde_json::to_string_pretty(self)
                .expect("a handoff always serializes")
                // `-->` can only occur inside a JSON string, where `>`
                // means the same and does not end the comment.
                .replace("-->", "--\\u003e");
            let _ = write!(s, "\n{DATA_OPEN}\n{json}\n{DATA_CLOSE}\n");
        }
        s
    }
}

/// A bulleted list under a heading, with at most [`LIST_LIMIT`] entries and
/// a marker naming the rest.
fn list(s: &mut String, heading: &str, items: &[String], data: bool) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(s, "- **{heading} ({}):**", items.len());
    for item in items.iter().take(LIST_LIMIT) {
        let _ = writeln!(s, "  - {item}");
    }
    if items.len() > LIST_LIMIT {
        let rest = items.len() - LIST_LIMIT;
        let whole = if data {
            "the data at the end of this file lists all of them"
        } else {
            "`jouet pickup --json` lists all of them"
        };
        let _ = writeln!(s, "  - ... {rest} more ({whole})");
    }
}

/// Reads a handoff written as markdown (with its data) or as JSON.
pub fn parse(text: &str) -> Result<Handoff, String> {
    let json = if text.trim_start().starts_with('{') {
        text
    } else {
        let start = text
            .rfind(DATA_OPEN)
            .ok_or("this is not a jouet handoff: it has no `<!-- jouet-handoff` data")?;
        let body = &text[start + DATA_OPEN.len()..];
        let end = body
            .find(DATA_CLOSE)
            .ok_or("the handoff's data is cut short: no closing `-->`")?;
        &body[..end]
    };
    serde_json::from_str(json).map_err(|e| format!("reading the handoff's data: {e}"))
}

/// Writes the handoff: JSON for a `.json` path, markdown otherwise.
pub fn write(path: &Path, handoff: &Handoff) -> std::io::Result<()> {
    let text = if path.extension().is_some_and(|e| e == "json") {
        let mut json = serde_json::to_string_pretty(handoff)?;
        json.push('\n');
        json
    } else {
        handoff.markdown(true)
    };
    crate::write_atomic(path, &text)
}

/// One way the checkout no longer matches a handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drift {
    pub code: &'static str,
    pub message: String,
}

/// How the checkout has moved on since `then` was written; empty when the
/// handoff still holds. The PR is not fetched again.
pub fn drift(git: &Git, then: &Handoff) -> Result<Vec<Drift>, Error> {
    let top = git.toplevel()?;
    let now = git_facts(git, &top)?;
    let mut out = Vec::new();
    let mut add = |code, message: String| out.push(Drift { code, message });
    if now.worktree != then.worktree {
        add(
            "worktree",
            format!("written in {}, read in {}", then.worktree, now.worktree),
        );
    }
    if now.branch != then.branch {
        add(
            "branch",
            format!(
                "the branch was {}, now {}",
                then.branch.as_deref().unwrap_or("detached"),
                now.branch.as_deref().unwrap_or("detached")
            ),
        );
    }
    if now.head != then.head {
        let (code, message) = head_drift(git, then.head.as_deref(), now.head.as_deref());
        add(code, message);
    }
    if now.operation != then.operation {
        add(
            "operation",
            format!(
                "the operation in progress was {}, now {}",
                then.operation.as_deref().unwrap_or("none"),
                now.operation.as_deref().unwrap_or("none")
            ),
        );
    }
    if now.upstream != then.upstream {
        add(
            "upstream",
            format!(
                "the upstream was {}, now {}",
                then.upstream.as_deref().unwrap_or("none"),
                now.upstream.as_deref().unwrap_or("none")
            ),
        );
    } else if now.behind != then.behind {
        add(
            "behind",
            format!(
                "{} behind {}, was {}",
                now.behind.unwrap_or(0),
                now.upstream.as_deref().unwrap_or("the upstream"),
                then.behind.unwrap_or(0)
            ),
        );
    }
    if now.changes != then.changes {
        add("uncommitted", changed_files(&then.changes, &now.changes));
    } else if now.diff_sha256 != then.diff_sha256 {
        add(
            "uncommitted",
            "the uncommitted files are the same, but their staged or unstaged changes differ"
                .into(),
        );
    }
    let (before, after): (BTreeSet<_>, BTreeSet<_>) = (
        then.untracked.iter().collect(),
        now.untracked.iter().collect(),
    );
    if before != after {
        let gone: Vec<String> = before.difference(&after).map(|p| (*p).clone()).collect();
        let new: Vec<String> = after.difference(&before).map(|p| (*p).clone()).collect();
        add("untracked", changed_sets("untracked", &new, &gone));
    }
    for p in &then.processes {
        if procs::alive(p.pid) == Some(false) {
            add(
                "process_exited",
                format!("{} `{}` has exited", p.pid, p.command),
            );
        }
    }
    Ok(out)
}

fn head_drift(git: &Git, then: Option<&str>, now: Option<&str>) -> (&'static str, String) {
    let (Some(then), Some(now)) = (then, now) else {
        return (
            "head",
            format!(
                "HEAD was {}, now {}",
                then.unwrap_or("unborn"),
                now.unwrap_or("unborn")
            ),
        );
    };
    let short = |s: &str| s.get(..12).unwrap_or(s).to_string();
    if git.commit_sha(then).is_none() {
        return (
            "head",
            format!(
                "HEAD was {}, which is not in this repository; now {}",
                short(then),
                short(now)
            ),
        );
    }
    if git.is_ancestor(then, now) {
        let n = git.count(&format!("{then}..{now}")).unwrap_or(0);
        return (
            "head",
            format!(
                "{n} new {} since the handoff ({}..{})",
                if n == 1 { "commit" } else { "commits" },
                short(then),
                short(now)
            ),
        );
    }
    if git.is_ancestor(now, then) {
        let n = git.count(&format!("{now}..{then}")).unwrap_or(0);
        return (
            "head",
            format!(
                "HEAD moved back {n} {} to {} (it was {})",
                if n == 1 { "commit" } else { "commits" },
                short(now),
                short(then)
            ),
        );
    }
    (
        "head",
        format!(
            "HEAD was rewritten: it was {}, now {}, and neither contains the other",
            short(then),
            short(now)
        ),
    )
}

/// How the uncommitted files moved: per path, newly changed, no longer
/// changed, or changed in another way.
fn changed_files(then: &[Change], now: &[Change]) -> String {
    let status = |list: &[Change], path: &str| {
        list.iter()
            .find(|c| c.path == path)
            .map(|c| c.status.clone())
    };
    let paths: BTreeSet<&str> = then.iter().chain(now).map(|c| c.path.as_str()).collect();
    let mut parts = Vec::new();
    for path in paths {
        match (status(then, path), status(now, path)) {
            (None, Some(s)) => parts.push(format!("{path} is now changed (`{s}`)")),
            (Some(_), None) => parts.push(format!("{path} no longer has changes")),
            (Some(a), Some(b)) if a != b => {
                parts.push(format!("{path} was `{a}`, now `{b}`"));
            }
            _ => {}
        }
    }
    parts.join("; ")
}

fn changed_sets(what: &str, new: &[String], gone: &[String]) -> String {
    let mut parts = Vec::new();
    if !new.is_empty() {
        parts.push(format!("now {what}: {}", new.join(", ")));
    }
    if !gone.is_empty() {
        parts.push(format!("no longer {what}: {}", gone.join(", ")));
    }
    parts.join("; ")
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
