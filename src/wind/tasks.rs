//! Winding into mise tasks: command sequences an agent keeps running in a
//! project, drafted as tasks for the project's own task runner.
//!
//! Two layers, as for rules. The ledger's shapes show, cheaply and all the
//! time, a sequence run again and again in one work tree; the session-start
//! hook names it. The ledger never keeps the command itself, so `jouet task
//! propose` reads the project's session transcripts for it, and drafts each
//! command run at least [`RUNS`] times in [`SESSIONS`] sessions as a task:
//! a name, its `run`, a description, `depends` when the sequence starts
//! with an existing task, `sources` when it names files in the repository,
//! and the file it should go in, with why. `jouet task apply` writes it
//! through `mise tasks add`; jouet never edits a mise config file itself.
//! The tasks jouet added are remembered with how often a session ran them,
//! so `jouet unwind` can propose removing one nobody runs.

use super::{MAX_NEW_PER_DAY, Paths, Status};
use crate::ledger::Entry;
use jouet_rules::shell;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Runs, and sessions, before a command is worth a task.
pub(crate) const RUNS: u64 = 3;
pub(crate) const SESSIONS: usize = 2;
/// New proposals one `jouet task propose` records, at most.
pub(crate) const MAX_DRAFTS: usize = 3;
/// A single command this long is worth a task on its own; a shorter one
/// only as part of a sequence.
const LONG_COMMAND: usize = 80;
/// Days a task jouet added may go unrun before `jouet unwind` proposes
/// removing it.
pub(crate) const UNUSED_DAYS: u64 = 21;
/// Sequences the ledger layer remembers, and sessions per sequence.
const MAX_SEEN: usize = 500;
const MAX_SESSIONS: usize = 50;
/// How much of a command a session-start line shows.
const LINE_COMMAND: usize = 100;

/// Programs that look around, move or record rather than do the project's
/// work: a sequence of only these is not a task.
const NOT_TASKS: &[&str] = &[
    "awk", "cat", "cd", "diff", "echo", "export", "false", "find", "gh", "git", "grep", "head",
    "jouet", "joue", "jq", "ls", "mise", "printf", "pwd", "rg", "sed", "sleep", "sort", "source",
    "stat", "tail", "test", "touch", "tree", "true", "uniq", "wc", "which",
];

/// Which mise config file a task goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum File {
    /// `mise.local.toml`: this user's, kept out of git.
    Local,
    /// The project's `mise.toml`, committed and shared.
    Shared,
}

impl File {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "local" => Some(Self::Local),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Shared => "shared",
        }
    }

    pub(crate) fn config(self) -> &'static str {
        match self {
            Self::Local => "mise.local.toml",
            Self::Shared => "mise.toml",
        }
    }

    pub(crate) fn other(self) -> Self {
        match self {
            Self::Local => Self::Shared,
            Self::Shared => Self::Local,
        }
    }
}

/// The file jouet suggests for a task, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Suggestion {
    pub file: File,
    pub reasons: Vec<String>,
}

/// What the sessions showed of a command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub runs: u64,
    pub sessions: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed: u64,
    /// Time the runs took, summed.
    #[serde(default)]
    pub duration_ms: u64,
    /// Their output read into the agent's context, about (bytes / 4).
    #[serde(default)]
    pub output_tokens: u64,
    /// Runs of its shape the hook ledger holds for this work tree, which
    /// counts sessions whose transcripts are gone or were not read.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ledger_runs: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A drafted mise task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Draft {
    pub name: String,
    /// The command as the sessions ran it, and as the task runs it.
    pub run: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    pub evidence: Evidence,
    pub suggest: Suggestion,
    /// Its shape as the ledger keys it, to match what the ledger noticed.
    pub shape: String,
}

/// A task jouet noticed, drafted, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub id: String,
    pub created: u64,
    /// The work tree, as the ledger keys it: a hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The work tree's root, once a task is drafted there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// The sequence's ledger shape, values left out.
    pub shape: String,
    /// The task, once `jouet task propose` read the command from a
    /// transcript; until then only the ledger noticed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Draft>,
    /// Runs and sessions the ledger saw when it noticed the sequence.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seen_runs: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seen_sessions: u64,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

/// A task jouet added, and how often sessions ran it since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Applied {
    pub name: String,
    pub root: String,
    /// The config file mise wrote.
    pub path: String,
    pub file: File,
    /// The task's `run` as mise wrote it.
    pub run: String,
    pub proposal: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub uses: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

/// A sequence the ledger has seen succeed in one work tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    repo: String,
    shape: String,
    runs: u64,
    sessions: BTreeSet<String>,
    last: u64,
}

/// The tasks part of the winding state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Tasks {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    #[serde(default)]
    seen: BTreeMap<String, Seen>,
    #[serde(default)]
    pub applied: Vec<Applied>,
}

impl Tasks {
    pub(crate) fn proposal(&self, id: &str) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }

    pub(crate) fn set_status(&mut self, id: &str, status: Status) -> bool {
        match self.proposals.iter_mut().find(|p| p.id == id) {
            Some(p) => {
                p.status = status;
                true
            }
            None => false,
        }
    }

    /// Takes in one ledger entry: a sequence that succeeded in a work tree.
    pub(crate) fn observe(&mut self, entry: &Entry) {
        if entry.tool.as_deref() != Some("Bash")
            || entry.event != "post-tool-use"
            || entry.exit_code != Some(0)
        {
            return;
        }
        let (Some(repo), Some(shape)) = (entry.repo.as_deref(), entry.shape.as_deref()) else {
            return;
        };
        let shape = shape_key(shape);
        if !worth_a_task(&super::reader::script(&shape)) {
            return;
        }
        let key = format!("{repo} {shape}");
        let seen = self.seen.entry(key).or_insert_with(|| Seen {
            repo: repo.to_string(),
            shape,
            runs: 0,
            sessions: BTreeSet::new(),
            last: entry.ts,
        });
        seen.runs += 1;
        seen.last = entry.ts;
        if seen.sessions.len() < MAX_SESSIONS {
            seen.sessions
                .insert(entry.session.clone().unwrap_or_default());
        }
        if self.seen.len() > MAX_SEEN
            && let Some(oldest) = self
                .seen
                .iter()
                .min_by_key(|(_, s)| s.last)
                .map(|(k, _)| k.clone())
        {
            self.seen.remove(&oldest);
        }
    }

    /// Turns sequences that crossed the threshold into proposals the
    /// session start names, within the daily limit; returns their ids.
    pub(crate) fn notice(&mut self, now: u64) -> Vec<String> {
        let today = jouet_output::time::date(now);
        let mut budget = MAX_NEW_PER_DAY.saturating_sub(
            self.proposals
                .iter()
                .filter(|p| jouet_output::time::date(p.created) == today)
                .count(),
        );
        let ready: Vec<Seen> = self
            .seen
            .values()
            .filter(|s| s.runs >= RUNS && s.sessions.len() >= SESSIONS)
            .filter(|s| {
                !self.proposals.iter().any(|p| {
                    p.repo.as_deref() == Some(s.repo.as_str())
                        && (p.shape == s.shape
                            || p.task.as_ref().is_some_and(|t| t.shape == s.shape))
                })
            })
            .cloned()
            .collect();
        let mut ids = Vec::new();
        for s in ready {
            if budget == 0 {
                break;
            }
            let id = self.id();
            self.proposals.push(Proposal {
                id: id.clone(),
                created: now,
                repo: Some(s.repo),
                root: None,
                shape: s.shape,
                task: None,
                seen_runs: s.runs,
                seen_sessions: s.sessions.len() as u64,
                status: Status::Pending,
                shown: Vec::new(),
            });
            ids.push(id);
            budget -= 1;
        }
        ids
    }

    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("t{}", self.next_id)
    }

    /// Records what `jouet task propose` drafted in the work tree `repo`
    /// at `root`: a draft fills in the proposal the ledger noticed for its
    /// shape, refreshes a pending one for the same command, or becomes a
    /// new proposal, [`MAX_DRAFTS`] at most. A proposal the ledger noticed
    /// that no draft matches lapses: no transcript ran one command often
    /// enough, so it varies from run to run.
    pub(crate) fn record(
        &mut self,
        repo: Option<&str>,
        root: &str,
        drafts: Vec<Draft>,
        now: u64,
    ) -> Recorded {
        let mut out = Recorded::default();
        let mut drafts: Vec<Option<Draft>> = drafts.into_iter().map(Some).collect();
        for p in self.proposals.iter_mut() {
            if p.repo.as_deref() != repo || p.status != Status::Pending {
                continue;
            }
            let same = drafts.iter().position(|d| {
                d.as_ref().is_some_and(|d| match &p.task {
                    Some(t) => t.run == d.run,
                    None => d.shape == p.shape,
                })
            });
            match same.and_then(|i| drafts[i].take()) {
                Some(mut d) => {
                    if let Some(t) = &p.task {
                        // Its name may have been shown already.
                        d.name.clone_from(&t.name);
                        out.updated.push(p.id.clone());
                    } else {
                        out.drafted.push(p.id.clone());
                    }
                    p.root = Some(root.to_string());
                    p.task = Some(d);
                }
                None if p.task.is_none() => {
                    p.status = Status::Lapsed;
                    out.lapsed.push(p.id.clone());
                }
                None => {}
            }
        }
        for d in drafts.into_iter().flatten() {
            let known = self.proposals.iter().any(|p| {
                p.repo.as_deref() == repo && p.task.as_ref().is_some_and(|t| t.run == d.run)
            });
            if known || out.drafted.len() + out.new.len() >= MAX_DRAFTS {
                continue;
            }
            let id = self.id();
            self.proposals.push(Proposal {
                id: id.clone(),
                created: now,
                repo: repo.map(str::to_string),
                root: Some(root.to_string()),
                shape: d.shape.clone(),
                task: Some(d),
                seen_runs: 0,
                seen_sessions: 0,
                status: Status::Pending,
                shown: Vec::new(),
            });
            out.new.push(id);
        }
        out
    }

    /// Notes that a session ran `command` in `root`, for each task jouet
    /// added there that it runs.
    pub(crate) fn record_use(&mut self, root: &str, command: &str, now: u64) -> bool {
        let names = task_runs(command);
        let mut used = false;
        for a in self.applied.iter_mut() {
            if a.root == root && names.contains(&a.name) {
                a.uses += 1;
                a.last_used = Some(now);
                used = true;
            }
        }
        used
    }

    /// Tasks jouet added at least [`UNUSED_DAYS`] ago that no session has
    /// run since.
    pub(crate) fn unused(&self, now: u64) -> Vec<&Applied> {
        self.applied
            .iter()
            .filter(|a| a.uses == 0 && now.saturating_sub(a.at) >= UNUSED_DAYS * 24 * 60 * 60)
            .collect()
    }
}

/// What [`Tasks::record`] did.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Recorded {
    /// New proposals.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new: Vec<String>,
    /// Proposals the ledger noticed, now drafted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafted: Vec<String>,
    /// Pending proposals with fresh evidence.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub updated: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lapsed: Vec<String>,
}

/// The task names a command runs through mise: `mise run NAME`, `mise r
/// NAME`, `mise run A ::: B`.
fn task_runs(command: &str) -> Vec<String> {
    let mut names = Vec::new();
    for c in shell::parse(command).all() {
        if c.program() != Some("mise") {
            continue;
        }
        let argv = c.argv();
        let Some(i) = argv.iter().position(|w| w == "run" || w == "r") else {
            continue;
        };
        let mut next = true;
        for word in &argv[i + 1..] {
            if word == "--" {
                break;
            }
            if word == ":::" {
                next = true;
            } else if next && !word.starts_with('-') {
                names.push(word.clone());
                next = false;
            }
        }
    }
    names
}

/// A script's top-level parts, each with the operator after it, split
/// outside quotes, parentheses and substitutions.
fn split(text: &str) -> Vec<(String, Option<&'static str>)> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            cur.push(c);
            if c == '\\' && q == '"' && i + 1 < chars.len() {
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        let prev = cur.chars().last();
        match c {
            '\\' => {
                cur.push(c);
                if let Some(n) = next {
                    cur.push(n);
                }
                i += 2;
                continue;
            }
            '\'' | '"' | '`' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            _ if depth > 0 => cur.push(c),
            // `2>&1`, `&>` and `>|` are redirections, not operators.
            '&' if matches!(prev, Some('>' | '<')) || next == Some('>') => cur.push(c),
            '|' if prev == Some('>') => cur.push(c),
            '&' | '|' | ';' | '\n' => {
                let op = match (c, next) {
                    ('&', Some('&')) => {
                        i += 1;
                        "&&"
                    }
                    ('|', Some('|')) => {
                        i += 1;
                        "||"
                    }
                    ('|', _) => "|",
                    ('&', _) => "&",
                    _ => ";",
                };
                parts.push((cur.trim().to_string(), Some(op)));
                cur.clear();
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    if !cur.trim().is_empty() {
        parts.push((cur.trim().to_string(), None));
    }
    parts
}

fn join(parts: &[(String, Option<&str>)]) -> String {
    let mut out = String::new();
    for (i, (text, op)) in parts.iter().enumerate() {
        out.push_str(text);
        match op {
            Some(";") if i + 1 < parts.len() => out.push_str("; "),
            Some(op) if i + 1 < parts.len() => {
                out.push(' ');
                out.push_str(op);
                out.push(' ');
            }
            _ => {}
        }
    }
    out
}

/// `| tail -n 20`, `| head -5` and the like, which only trim what the
/// agent reads.
fn limiter(part: &str) -> bool {
    let mut words = part.split_whitespace();
    matches!(words.next(), Some("tail" | "head"))
        && words.all(|w| {
            w.starts_with('-')
                || w.bytes().all(|b| b.is_ascii_digit())
                || w == "<n>"
                || w == "<arg>"
        })
}

/// Where a leading `cd` goes; `None` when the part is not one.
fn cd_target(part: &str) -> Option<&str> {
    let mut words = part.split_whitespace();
    (words.next() == Some("cd"))
        .then(|| words.next())
        .flatten()
        .filter(|_| words.next().is_none())
}

/// A command as a task would run it, in `root`: without a trailing `|
/// tail` or `| head` that only trims what the agent reads (and the `2>&1`
/// before it), without a leading `cd` to the root, and with paths under
/// the root made relative to it. With no root (a ledger shape, whose
/// values are gone), any leading `cd` goes.
pub(crate) fn normalize(command: &str, root: Option<&str>) -> String {
    let mut text = command.trim().to_string();
    if let Some(root) = root.filter(|r| r.len() > 1) {
        let root = root.trim_end_matches('/');
        for (from, to) in [
            (format!("cd {root} &&"), "cd . &&".to_string()),
            (format!("{root}/"), String::new()),
        ] {
            text = text.replace(&from, &to);
        }
    }
    let mut parts = split(&text);
    while parts.len() >= 2
        && parts[parts.len() - 2].1 == Some("|")
        && limiter(&parts[parts.len() - 1].0)
    {
        parts.pop();
        if let Some(last) = parts.last_mut() {
            last.1 = None;
            last.0 = last.0.trim_end_matches("2>&1").trim_end().to_string();
        }
    }
    if parts.len() >= 2
        && parts[0].1 == Some("&&")
        && cd_target(&parts[0].0).is_some_and(|dir| root.is_none() || dir == "." || dir == "./")
    {
        parts.remove(0);
    }
    join(&parts)
}

/// The key the ledger layer and drafts share: a command's shape,
/// normalized.
pub(crate) fn shape_key(shape: &str) -> String {
    normalize(shape, None)
}

/// Whether a command is the kind a task replaces: a sequence of commands,
/// or one long command, doing the project's work (not only looking
/// around), in one line, with no secret in it.
pub(crate) fn worth_a_task(command: &str) -> bool {
    if command.contains('\n') || command.contains("<redacted>") || command.contains("REDACTED") {
        return false;
    }
    let parts = split(command);
    if parts.iter().any(|(_, op)| *op == Some("&")) {
        return false;
    }
    let parsed = shell::parse(command);
    let programs: Vec<&str> = parsed.commands.iter().filter_map(|c| c.program()).collect();
    if programs.is_empty() || programs.iter().all(|p| NOT_TASKS.contains(p)) {
        return false;
    }
    let sequence = parts
        .iter()
        .any(|(_, op)| matches!(op, Some("&&" | ";" | "||")));
    sequence || command.chars().count() >= LONG_COMMAND
}

/// A task that already exists, from `mise tasks ls --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Existing {
    pub name: String,
    pub run: Vec<String>,
}

/// One shell call from a transcript.
#[derive(Debug, Clone)]
pub(crate) struct Run {
    pub session: String,
    pub command: String,
    pub failed: bool,
    pub duration_ms: u64,
    pub output_bytes: u64,
}

/// What jouet knows of the project a task would go in.
pub(crate) struct Project<'a> {
    pub root: &'a str,
    /// Whether the repository has a mise config file in git.
    pub shared_config: bool,
    /// Programs the project's own config and manifests name: its mise
    /// tools, cargo for a Cargo.toml, npm for a package.json.
    pub tools: BTreeSet<String>,
    /// Whether the repository's CI, docs or build files hold this text.
    pub documented: &'a dyn Fn(&str) -> bool,
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drafts a task for each command in `runs` that sessions repeated
/// enough, that a task would replace, and that no existing task already
/// runs, most sessions first. `ledger` holds, by [`shape_key`], the runs
/// the hook ledger saw in this work tree. `taken` names that are in use.
pub(crate) fn drafts(
    runs: &[Run],
    existing: &[Existing],
    project: &Project<'_>,
    ledger: &BTreeMap<String, u64>,
    taken: &BTreeSet<String>,
) -> Vec<Draft> {
    let mut by_command: BTreeMap<String, (Evidence, BTreeSet<&str>, String)> = BTreeMap::new();
    for run in runs {
        let command = normalize(&run.command, Some(project.root));
        if !worth_a_task(&command) {
            continue;
        }
        let (evidence, sessions, _) = by_command.entry(command.clone()).or_insert_with(|| {
            (
                Evidence::default(),
                BTreeSet::new(),
                shape_key(&crate::ledger::shape(&command)),
            )
        });
        evidence.runs += 1;
        evidence.failed += u64::from(run.failed);
        evidence.duration_ms += run.duration_ms;
        evidence.output_tokens += run.output_bytes / 4;
        sessions.insert(&run.session);
    }
    let runs_of: BTreeSet<String> = existing
        .iter()
        .flat_map(|t| {
            let mut all: Vec<String> = t.run.iter().map(|r| squash(r)).collect();
            all.push(squash(&t.run.join(" && ")));
            all
        })
        .collect();
    let mut ready: Vec<(String, Evidence, String)> = by_command
        .into_iter()
        .filter(|(_, (e, sessions, _))| e.runs >= RUNS && sessions.len() >= SESSIONS)
        .filter(|(command, _)| !runs_of.contains(&squash(command)))
        .map(|(command, (mut e, sessions, shape))| {
            e.sessions = sessions.len() as u64;
            e.ledger_runs = ledger.get(&shape).copied().unwrap_or(0);
            (command, e, shape)
        })
        .collect();
    ready.sort_by(|a, b| {
        (b.1.sessions, b.1.runs)
            .cmp(&(a.1.sessions, a.1.runs))
            .then(a.0.cmp(&b.0))
    });
    let mut taken = taken.clone();
    taken.extend(existing.iter().map(|t| t.name.clone()));
    let mut out = Vec::new();
    for (command, evidence, shape) in ready {
        let (depends, run) = depends(&command, existing);
        let name = free_name(&name_for(&run), &taken);
        taken.insert(name.clone());
        let sources = sources(&run, Path::new(project.root));
        let suggest = suggest(&command, project);
        out.push(Draft {
            description: describe(&run, &depends, &evidence),
            name,
            run,
            depends,
            sources,
            evidence,
            suggest,
            shape,
        });
    }
    out
}

/// A sequence that starts with an existing task (by `mise run NAME` or by
/// running exactly its command) depends on that task and runs the rest.
/// Only the first part: mise runs several dependencies at once, and the
/// sequence ran them in order.
fn depends(command: &str, existing: &[Existing]) -> (Vec<String>, String) {
    let parts = split(command);
    if parts.len() < 2 || parts[0].1 != Some("&&") {
        return (Vec::new(), command.to_string());
    }
    let first = squash(&parts[0].0);
    let mut words = first.split_whitespace();
    let named = match (words.next(), words.next(), words.next(), words.next()) {
        (Some("mise"), Some("run" | "r"), Some(name), None) if !name.starts_with('-') => {
            Some(name.to_string())
        }
        _ => existing
            .iter()
            .find(|t| t.run.len() == 1 && squash(&t.run[0]) == first)
            .map(|t| t.name.clone()),
    };
    match named {
        Some(name) => (vec![name], join(&parts[1..])),
        None => (Vec::new(), command.to_string()),
    }
}

/// Programs that run the project's own commands, so their subcommand
/// names what a call does: `cargo build`, `npm run lint`.
const RUNNERS: &[&str] = &[
    "bun", "bundle", "cargo", "deno", "dotnet", "go", "gradle", "gradlew", "hk", "just", "make",
    "mvn", "npm", "pnpm", "poetry", "rake", "uv", "yarn",
];

/// A task's name from what its commands do: `cargo build && cargo test`
/// is `build-test`, `./scripts/release.sh` is `release`.
fn name_for(run: &str) -> String {
    let mut verbs: Vec<String> = Vec::new();
    for command in &shell::parse(run).commands {
        let Some(program) = command.program().filter(|p| !NOT_TASKS.contains(p)) else {
            continue;
        };
        let signature = super::detect::Signature::of(command);
        let word = if RUNNERS.contains(&program) {
            signature.words.last()
        } else {
            signature.words.first()
        };
        let Some(word) = word else {
            continue;
        };
        let word = word.split('.').next().unwrap_or(word);
        let verb: String = word
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !verb.is_empty() && !verbs.contains(&verb) && verbs.len() < 3 {
            verbs.push(verb);
        }
    }
    if verbs.is_empty() {
        "task".into()
    } else {
        verbs.join("-")
    }
}

fn free_name(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken.contains(name))
        .unwrap_or_else(|| base.to_string())
}

/// Files in the repository the commands name, so mise can skip the task
/// while they are unchanged.
fn sources(run: &str, root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for command in &shell::parse(run).commands {
        for word in command.argv().iter().skip(1) {
            let word = word.split_once('=').map_or(word.as_str(), |(_, v)| v);
            let pathy = word.contains('/') || word.contains('*') || word.contains('.');
            if !pathy
                || word.starts_with(['-', '/', '~', '$'])
                || word.contains("..")
                || word.contains("://")
            {
                continue;
            }
            let base = match word.find(['*', '?', '[', '{']) {
                Some(i) => word[..i].rsplit_once('/').map_or("", |(dir, _)| dir),
                None => word,
            };
            let exists = if base.is_empty() {
                word.contains('*')
            } else {
                root.join(base).exists()
            };
            if exists && !out.iter().any(|s| s == word) && out.len() < 5 {
                out.push(word.to_string());
            }
        }
    }
    out
}

fn describe(run: &str, depends: &[String], e: &Evidence) -> String {
    let steps: Vec<String> = shell::parse(run)
        .commands
        .iter()
        .filter(|c| c.program().is_some_and(|p| !NOT_TASKS.contains(&p)))
        .map(|c| super::detect::Signature::of(c).words.join(" "))
        .collect();
    let mut what = steps.join(", then ");
    if let Some(first) = depends.first() {
        what = format!("{first}, then {what}");
    }
    format!(
        "{what} (added by jouet: agents ran it {} times in {} sessions)",
        e.runs, e.sessions
    )
}

/// A path that only means something on this machine: absolute outside
/// the project, or under the home directory.
fn machine_path(command: &str) -> Option<String> {
    let parsed = shell::parse(command);
    for c in parsed.all() {
        if c.vars.iter().any(|v| v == "HOME") {
            return Some("$HOME".into());
        }
        for word in &c.words {
            let word = word.split_once('=').map_or(word.as_str(), |(_, v)| v);
            if word.starts_with('~') || (word.starts_with('/') && !word.starts_with("/dev/")) {
                return Some(word.to_string());
            }
        }
    }
    None
}

/// Which file a task goes in, and why. `mise.local.toml` when it is
/// personal: a path on this machine, a repository with no mise config to
/// share it through, or a command nothing in the repository runs.
/// `mise.toml` when it is the project's workflow: its CI or docs run it,
/// or it runs only the project's own tools.
pub(crate) fn suggest(command: &str, project: &Project<'_>) -> Suggestion {
    let mut local = Vec::new();
    if let Some(path) = machine_path(command) {
        local.push(format!(
            "it uses `{path}`, a path on this machine that others won't have"
        ));
    }
    if !project.shared_config {
        local.push(
            "the repository has no mise.toml in git, so the task would be the first one shared"
                .into(),
        );
    }
    if !local.is_empty() {
        return Suggestion {
            file: File::Local,
            reasons: local,
        };
    }
    let mut shared = Vec::new();
    let parts: Vec<String> = split(command).into_iter().map(|(t, _)| t).collect();
    if let Some(part) = parts.iter().find(|p| (project.documented)(p)) {
        shared.push(format!("the repository's CI or docs run `{part}` too"));
    }
    let parsed = shell::parse(command);
    let programs: BTreeSet<&str> = parsed
        .commands
        .iter()
        .filter_map(|c| c.argv().first().map(String::as_str))
        .filter(|p| !NOT_TASKS.contains(&shell::basename(p)))
        .collect();
    // A program is the project's when it names it, or is a file in it,
    // such as `./scripts/release.sh`.
    let ours = |p: &str| {
        project.tools.contains(shell::basename(p))
            || (p.contains('/') && Path::new(project.root).join(p).is_file())
    };
    if !programs.is_empty() && programs.iter().all(|p| ours(p)) {
        let names: Vec<&str> = programs.into_iter().collect();
        shared.push(format!(
            "it runs only tools the project already uses ({})",
            names.join(", ")
        ));
    }
    if shared.is_empty() {
        return Suggestion {
            file: File::Local,
            reasons: vec![
                "only your sessions run it: nothing in the repository's CI, docs or tools does"
                    .into(),
            ],
        };
    }
    shared.push("it uses only paths inside the repository".into());
    Suggestion {
        file: File::Shared,
        reasons: shared,
    }
}

/// What the agent asks the user before applying a draft.
pub(crate) fn question(d: &Draft) -> String {
    format!(
        "Add the mise task `{}` (`{}`) to mise.local.toml, just for you and not committed, or to mise.toml, shared with everyone who works on this repository? I suggest {}: {}.",
        d.name,
        d.run,
        d.suggest.file.config(),
        d.suggest.reasons.join("; ")
    )
}

/// The proposals to show a session starting in the work tree `repo`, at
/// most `room`, one line each, marked as shown to it.
pub(crate) fn lines_for_session(
    paths: &Paths,
    session: &str,
    repo: Option<&str>,
    room: usize,
) -> Vec<String> {
    if room == 0 || repo.is_none() {
        return Vec::new();
    }
    super::update(&paths.state, |state| {
        let mut lines = Vec::new();
        for p in state.tasks.proposals.iter_mut() {
            if lines.len() == room {
                break;
            }
            if p.status != Status::Pending || p.repo.as_deref() != repo {
                continue;
            }
            super::review::mark_shown(&mut p.shown, &mut p.status, session);
            lines.push(line(p));
        }
        lines
    })
    .unwrap_or_default()
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

fn line(p: &Proposal) -> String {
    match &p.task {
        None => format!(
            "jouet wind noticed `{}` run {} times in {} sessions in this repository; a mise task may replace it. At a natural pause, `jouet task propose` drafts one from the session transcripts (`jouet wind dismiss {}` drops it).",
            clip(&p.shape, LINE_COMMAND),
            p.seen_runs,
            p.seen_sessions,
            p.id
        ),
        Some(d) => format!(
            "jouet task {id} is ready: a mise task `{}` for `{}`, which agents ran {} times in {} sessions here (`jouet task propose` shows it in full). At a natural pause, ask the user whether it goes in mise.local.toml (just theirs) or mise.toml (shared), suggesting {} because {}; then `jouet task apply {id} --file local|shared`, or `jouet wind dismiss {id}`.",
            d.name,
            clip(&d.run, LINE_COMMAND),
            d.evidence.runs,
            d.evidence.sessions,
            d.suggest.file.config(),
            d.suggest.reasons.first().map_or("", String::as_str),
            id = p.id
        ),
    }
}

/// Notes a run of a task jouet added, from the post-tool-use hook. Reads
/// the state only when the command runs a mise task, and writes it only
/// when the task is one of jouet's.
pub(crate) fn record_use(paths: &Paths, cwd: &Path, command: &str, now: u64) {
    if !command.contains("mise") || task_runs(command).is_empty() {
        return;
    }
    let Some(root) = jouet_output::paths::work_tree(cwd) else {
        return;
    };
    let root = root.to_string_lossy().into_owned();
    let state = super::State::load(&paths.state);
    let names = task_runs(command);
    let ours = state
        .tasks
        .applied
        .iter()
        .any(|a| a.root == root && names.contains(&a.name));
    if ours {
        let _ = super::update(&paths.state, |state| {
            state.tasks.record_use(&root, command, now)
        });
    }
}

#[cfg(test)]
mod tests;
