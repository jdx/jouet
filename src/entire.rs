//! Entire (entire.io), read through its CLI: `entire status --json`, `entire
//! auth status --json`, `entire session list --json`, `entire checkpoint list
//! --json` and `entire why FILE --json`. jouet runs only these read-only
//! subcommands, never one that writes (`session stop`, `checkpoint explain
//! --generate`, `enable`, ...), and never `entire auth token`: of `auth
//! status` it reads only whether the login works.
//!
//! Everything here is best effort. Without `entire` on PATH, in a repository
//! Entire is not enabled in, or without a working login, [`Entire::here`] is
//! None and the command says nothing about Entire. A call that fails or runs
//! past its timeout after that becomes a note, never an error. Each answer is
//! kept for the rest of the command, so a command asks each question once.
//! Hooks never call this: the network is off there.

use crate::diag::process;
use crate::redact::redact;
use crate::settings::Settings;
use jouet_output::time;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROGRAM: &str = "entire";

/// How much of a prompt or checkpoint message is shown.
const MESSAGE_CHARS: usize = 100;

/// A repository where Entire is enabled and its login works.
pub(crate) struct Entire {
    dir: PathBuf,
    timeout: Duration,
    why_timeout: Duration,
    answers: RefCell<HashMap<Vec<String>, Result<String, String>>>,
    /// A call ran past its timeout.
    stalled: Cell<bool>,
}

#[derive(serde::Deserialize)]
struct Status {
    #[serde(default)]
    enabled: bool,
}

#[derive(serde::Deserialize)]
struct Auth {
    #[serde(default)]
    logged_in: bool,
}

/// A session, as `entire session list --json` describes it. Fields Entire
/// adds later are ignored.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct Session {
    pub session_id: String,
    #[serde(default)]
    pub agent: String,
    /// `active`, `idle` or `ended`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub last_active: Option<String>,
    #[serde(default)]
    pub checkpoints: u64,
    #[serde(default)]
    pub files_touched: Vec<String>,
}

/// A checkpoint on the current branch, as `entire checkpoint list --json`
/// describes it: a committed one (its message is the commit's subject) or
/// one not yet condensed (its message is the prompt).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct Checkpoint {
    pub checkpoint_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Why {
    #[serde(default)]
    lines: Vec<WhyLine>,
}

/// One line of `entire why FILE --json`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct WhyLine {
    pub line_number: usize,
    /// `ai`, `human` or `mixed`; anything else is not committed yet.
    #[serde(default)]
    pub authorship: String,
    #[serde(default)]
    pub short_commit_sha: Option<String>,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
}

impl WhyLine {
    /// An agent wrote it, alone (`ai`) or with a person (`mixed`).
    fn by_agent(&self) -> bool {
        matches!(self.authorship.as_str(), "ai" | "mixed")
    }
}

impl Entire {
    /// Entire for the repository at `dir`, when the settings allow it,
    /// `entire` runs, the repository is enabled and the login works.
    pub(crate) fn here(dir: &Path) -> Option<Self> {
        let settings = Settings::load().entire;
        if !settings.enabled {
            return None;
        }
        let entire = Self {
            dir: dir.to_path_buf(),
            timeout: Duration::from_millis(settings.timeout_ms),
            why_timeout: Duration::from_millis(settings.why_timeout_ms),
            answers: RefCell::default(),
            stalled: Cell::default(),
        };
        let status: Status = entire.json(&["status", "--json"], entire.timeout).ok()?;
        if !status.enabled {
            return None;
        }
        let auth: Auth = entire
            .json(&["auth", "status", "--json"], entire.timeout)
            .ok()?;
        auth.logged_in.then_some(entire)
    }

    /// Every session Entire tracks for the repository, across its worktrees.
    pub(crate) fn sessions(&self) -> Result<Vec<Session>, String> {
        self.json(&["session", "list", "--json"], self.timeout)
    }

    /// The checkpoints on the current branch, newest first.
    pub(crate) fn checkpoints(&self) -> Result<Vec<Checkpoint>, String> {
        self.json(
            &["checkpoint", "list", "--json", "--no-pager"],
            self.timeout,
        )
    }

    /// Where each line of `file` came from.
    pub(crate) fn why(&self, file: &str) -> Result<Vec<WhyLine>, String> {
        self.json::<Why>(&["why", file, "--json"], self.why_timeout)
            .map(|w| w.lines)
    }

    fn json<T: DeserializeOwned>(&self, args: &[&str], timeout: Duration) -> Result<T, String> {
        let text = self.ask(args, timeout)?;
        serde_json::from_str(&text)
            .map_err(|e| format!("`entire {}` printed JSON jouet cannot read: {e}", args[0]))
    }

    fn ask(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let key: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
        if let Some(answer) = self.answers.borrow().get(&key) {
            return answer.clone();
        }
        let answer = run(&self.dir, args, timeout).map_err(|(message, timed_out)| {
            if timed_out {
                self.stalled.set(true);
            }
            message
        });
        self.answers.borrow_mut().insert(key, answer.clone());
        answer
    }

    /// Whether a call ran past its timeout, so the next is likely to as well.
    pub(crate) fn stalled(&self) -> bool {
        self.stalled.get()
    }
}

/// Runs `entire ARGS` in `dir` with stdin closed, for at most `timeout`.
/// An error says what went wrong and whether it was the timeout.
fn run(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, (String, bool)> {
    let shown = format!("entire {}", args.join(" "));
    let argv: Vec<String> = std::iter::once(PROGRAM)
        .chain(args.iter().copied())
        .map(String::from)
        .collect();
    let ran =
        process::run(&argv, dir, b"", timeout).map_err(|e| (format!("`{shown}`: {e}"), false))?;
    if ran.timed_out {
        let waited = if timeout < Duration::from_secs(1) {
            format!("{}ms", timeout.as_millis())
        } else {
            time::elapsed(timeout.as_secs())
        };
        return Err((
            format!("`{shown}` took longer than {waited}, so jouet stopped waiting"),
            true,
        ));
    }
    if ran.code != Some(0) {
        let stderr = String::from_utf8_lossy(&ran.stderr);
        let why = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let code = ran.code.map_or("on a signal".into(), |c| c.to_string());
        return Err((
            format!("`{shown}` exited {code}: {}", redact(why.trim())),
            false,
        ));
    }
    String::from_utf8(ran.stdout)
        .map_err(|_| (format!("`{shown}` printed text that is not UTF-8"), false))
}

/// The start of an ID, as text shows one.
pub(crate) fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// The start of a checkpoint ID, which Entire takes in its place.
pub(crate) fn short_checkpoint(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// The first line of a prompt or message, redacted and cut to a length. A
/// tag a harness wraps a message in, such as `<ci-monitor-event>` or
/// `<cross-session-message from="...">`, is left out.
fn excerpt(text: &str) -> String {
    let line = text
        .lines()
        .map(|l| without_tag(l.trim()))
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = redact(line);
    if line.chars().count() <= MESSAGE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(MESSAGE_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// A line without the tag it starts with: `<name ...>`, where the name is
/// letters, digits and dashes.
fn without_tag(line: &str) -> &str {
    let Some((tag, after)) = line.strip_prefix('<').and_then(|r| r.split_once('>')) else {
        return line;
    };
    let named = tag
        .split_whitespace()
        .next()
        .is_some_and(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if named { after.trim() } else { line }
}

/// What `jouet pickup` shows of Entire: the sessions with checkpoints on
/// this branch, newest first.
#[derive(Debug, Serialize)]
pub(crate) struct Recap {
    /// Checkpoints Entire lists on the branch.
    pub checkpoints: usize,
    /// Of them, those made after the handoff was written.
    #[serde(skip_serializing_if = "is_zero")]
    pub since_handoff: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<RecapSession>,
    /// Sessions with checkpoints on the branch that are not listed.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_sessions: usize,
    /// Why part of it could not be read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RecapSession {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// `active`, `idle` or `ended`, when Entire still lists the session.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active: Option<String>,
    /// Its checkpoints on this branch.
    pub checkpoints: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub since_handoff: usize,
    pub last_checkpoint: RecapCheckpoint,
    /// The files the session touched, the first few of `file_count`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub file_count: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct RecapCheckpoint {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The commit subject, or for a checkpoint not yet committed the prompt,
    /// cut to its first line and 100 characters.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How many sessions a recap lists, and how many of each one's files.
const RECAP_SESSIONS: usize = 3;
const RECAP_FILES: usize = 5;

impl Entire {
    /// The sessions with checkpoints on this branch, leaving out `current`
    /// (the session asking), with how many checkpoints came after `since`.
    pub(crate) fn recap(&self, current: Option<&str>, since: Option<i64>) -> Recap {
        let mut notes = Vec::new();
        let checkpoints = self.checkpoints().unwrap_or_else(|e| {
            notes.push(e);
            Vec::new()
        });
        let sessions = if checkpoints.is_empty() {
            Vec::new()
        } else {
            self.sessions().unwrap_or_else(|e| {
                notes.push(e);
                Vec::new()
            })
        };
        recap(&checkpoints, &sessions, current, since, notes)
    }
}

fn recap(
    checkpoints: &[Checkpoint],
    sessions: &[Session],
    current: Option<&str>,
    since: Option<i64>,
    notes: Vec<String>,
) -> Recap {
    let now = time::now();
    let after = |c: &Checkpoint| {
        since.is_some_and(|since| {
            c.date
                .as_deref()
                .and_then(|d| time::parse_time(d, now).ok())
                .is_some_and(|t| t > since)
        })
    };
    // In the order of each session's newest checkpoint; the list is newest
    // first, but not every Entire version promises it.
    let mut order: Vec<&str> = Vec::new();
    let mut by_session: BTreeMap<&str, Vec<&Checkpoint>> = BTreeMap::new();
    let mut sorted: Vec<&Checkpoint> = checkpoints.iter().collect();
    sorted.sort_by_key(|c| {
        std::cmp::Reverse(
            c.date
                .as_deref()
                .and_then(|d| time::parse_time(d, now).ok())
                .unwrap_or(0),
        )
    });
    for c in sorted {
        let Some(id) = c.session_id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        if Some(id) == current {
            continue;
        }
        if !by_session.contains_key(id) {
            order.push(id);
        }
        by_session.entry(id).or_default().push(c);
    }
    let listed: Vec<RecapSession> = order
        .iter()
        .take(RECAP_SESSIONS)
        .map(|id| {
            let mine = &by_session[id];
            let last = mine[0];
            let known = sessions.iter().find(|s| s.session_id == *id);
            let files = known.map(|s| s.files_touched.as_slice()).unwrap_or(&[]);
            RecapSession {
                id: crate::ledger::redact_id(id),
                agent: known
                    .map(|s| s.agent.clone())
                    .filter(|a| !a.is_empty())
                    .or_else(|| last.agent.clone())
                    .unwrap_or_default(),
                status: known.map(|s| s.status.clone()).unwrap_or_default(),
                last_active: known.and_then(|s| s.last_active.clone()),
                checkpoints: mine.len(),
                since_handoff: mine.iter().filter(|c| after(c)).count(),
                last_checkpoint: RecapCheckpoint {
                    id: crate::ledger::redact_id(&last.checkpoint_id),
                    date: last.date.clone(),
                    message: excerpt(&last.message),
                },
                files: files.iter().take(RECAP_FILES).map(|f| redact(f)).collect(),
                file_count: files.len(),
            }
        })
        .collect();
    Recap {
        checkpoints: checkpoints.len(),
        since_handoff: checkpoints.iter().filter(|c| after(c)).count(),
        more_sessions: order.len() - listed.len(),
        sessions: listed,
        notes,
    }
}

impl Recap {
    /// The recap as a markdown section, or nothing when there is nothing to
    /// say.
    pub(crate) fn markdown(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        if self.sessions.is_empty() && self.notes.is_empty() {
            return s;
        }
        let _ = writeln!(s, "\n## Entire: earlier sessions on this branch\n");
        let now = time::now();
        for r in &self.sessions {
            let agent = if r.agent.is_empty() {
                "session"
            } else {
                &r.agent
            };
            let mut state = Vec::new();
            if !r.status.is_empty() {
                state.push(r.status.clone());
            }
            if let Some(t) = r
                .last_active
                .as_deref()
                .and_then(|t| time::parse_time(t, now).ok())
            {
                state.push(format!("active {} ago", time::age(now - t)));
            }
            let state = if state.is_empty() {
                String::new()
            } else {
                format!(" ({})", state.join(", "))
            };
            let mut count = format!(
                "{} checkpoint{}",
                r.checkpoints,
                if r.checkpoints == 1 { "" } else { "s" }
            );
            if r.since_handoff > 0 {
                let _ = write!(count, ", {} since the handoff", r.since_handoff);
            }
            let _ = writeln!(s, "- {agent} `{}`{state}: {count}", short(&r.id));
            let c = &r.last_checkpoint;
            let _ = write!(s, "  - last checkpoint `{}`", short_checkpoint(&c.id));
            if let Some(date) = &c.date {
                let _ = write!(s, " at {date}");
            }
            if !c.message.is_empty() {
                let _ = write!(s, ": {}", c.message);
            }
            s.push('\n');
            if !r.files.is_empty() {
                let _ = write!(s, "  - files: {}", r.files.join(", "));
                if r.file_count > r.files.len() {
                    let _ = write!(s, ", ... {} more", r.file_count - r.files.len());
                }
                s.push('\n');
            }
        }
        if self.more_sessions > 0 {
            let _ = writeln!(
                s,
                "- ... {} more session{} (`entire checkpoint list`)",
                self.more_sessions,
                if self.more_sessions == 1 { "" } else { "s" }
            );
        }
        for note in &self.notes {
            let _ = writeln!(s, "- note: {note}");
        }
        if let Some(first) = self.sessions.first() {
            let _ = writeln!(
                s,
                "\nMore: `entire session info {id}`, `entire checkpoint list --session {id}`, `entire checkpoint explain --checkpoint {cp} --short`, `entire recap --static --week`.",
                id = first.id,
                cp = short_checkpoint(&first.last_checkpoint.id)
            );
        }
        s
    }
}

/// Where a run of lines came from, by `entire why`.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Origin {
    /// The lines agents wrote, by checkpoint, most lines first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agent: Vec<AgentLines>,
    #[serde(skip_serializing_if = "is_zero")]
    pub human_lines: usize,
    /// The commits the human lines came from.
    #[serde(skip_serializing_if = "is_zero")]
    pub human_commits: usize,
    /// Lines not committed yet, which Entire cannot place.
    #[serde(skip_serializing_if = "is_zero")]
    pub uncommitted_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentLines {
    /// Absent when Entire knows an agent wrote the lines but not in which
    /// checkpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The lines in the run, 1-based.
    pub lines: Vec<usize>,
    /// The prompt behind them: its first line, redacted, up to 100
    /// characters.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
}

/// Where the lines `wanted` (1-based, in order) of a file came from.
pub(crate) fn origin(why: &[WhyLine], wanted: impl Fn(usize) -> bool) -> Origin {
    let mut agent: Vec<AgentLines> = Vec::new();
    let mut origin = Origin::default();
    let mut human_commits: Vec<&str> = Vec::new();
    for l in why.iter().filter(|l| wanted(l.line_number)) {
        if l.by_agent() {
            let id = l.checkpoint_id.clone();
            match agent.iter_mut().find(|a| a.checkpoint_id == id) {
                Some(a) => a.lines.push(l.line_number),
                None => agent.push(AgentLines {
                    checkpoint_id: id,
                    session_id: l.session_id.as_deref().map(crate::ledger::redact_id),
                    agent: l.agent.clone(),
                    commit: l.short_commit_sha.clone(),
                    lines: vec![l.line_number],
                    prompt: l.prompt.as_deref().map(excerpt).unwrap_or_default(),
                }),
            }
        } else if l.authorship == "human" {
            origin.human_lines += 1;
            let commit = l.short_commit_sha.as_deref().unwrap_or("");
            if !human_commits.contains(&commit) {
                human_commits.push(commit);
            }
        } else {
            origin.uncommitted_lines += 1;
        }
    }
    agent.sort_by_key(|a| std::cmp::Reverse(a.lines.len()));
    origin.agent = agent;
    origin.human_commits = human_commits.len();
    origin
}

impl Origin {
    pub(crate) fn is_empty(&self) -> bool {
        self.agent.is_empty() && self.human_lines == 0 && self.uncommitted_lines == 0
    }

    /// One line: `origin: 12 lines by Claude Code ..., 3 human (1 commit)`.
    pub(crate) fn line(&self) -> String {
        let mut parts: Vec<String> = self.agent.iter().map(AgentLines::describe).collect();
        if self.human_lines > 0 {
            parts.push(format!(
                "{} by people ({} commit{})",
                lines(self.human_lines),
                self.human_commits,
                if self.human_commits == 1 { "" } else { "s" }
            ));
        }
        if self.uncommitted_lines > 0 {
            parts.push(format!("{} not committed", lines(self.uncommitted_lines)));
        }
        format!("origin (entire why): {}", parts.join("; "))
    }
}

impl AgentLines {
    /// `4 lines (12-15) by Claude Code session 8d2ac41d, checkpoint
    /// 01M66H17P11G (commit 12fa1abf): "the prompt"`.
    pub(crate) fn describe(&self) -> String {
        let mut s = format!(
            "{} ({}) by {}",
            lines(self.lines.len()),
            spans(&self.lines),
            self.agent.as_deref().unwrap_or("an agent")
        );
        if let Some(session) = &self.session_id {
            s.push_str(&format!(" session {}", short(session)));
        }
        if let Some(checkpoint) = &self.checkpoint_id {
            s.push_str(&format!(", checkpoint {}", short_checkpoint(checkpoint)));
        }
        if let Some(commit) = &self.commit {
            s.push_str(&format!(" (commit {commit})"));
        }
        if !self.prompt.is_empty() {
            s.push_str(&format!(": \"{}\"", self.prompt));
        }
        s
    }
}

fn lines(n: usize) -> String {
    format!("{n} line{}", if n == 1 { "" } else { "s" })
}

/// `3-5, 9`: sorted line numbers as runs.
pub(crate) fn spans(lines: &[usize]) -> String {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &n in lines {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == n => *end = n,
            _ => runs.push((n, n)),
        }
    }
    runs.iter()
        .map(|&(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(id: &str, session: &str, date: &str, message: &str) -> Checkpoint {
        Checkpoint {
            checkpoint_id: id.into(),
            session_id: Some(session.into()),
            agent: None,
            date: Some(date.into()),
            message: message.into(),
        }
    }

    #[test]
    fn a_recap_groups_checkpoints_by_session_newest_first() {
        let checkpoints = [
            checkpoint("c1", "aaaaaaaa-1", "2026-10-30T10:00:00Z", "first"),
            checkpoint("c3", "bbbbbbbb-2", "2026-10-30T12:00:00+02:00", "b's"),
            checkpoint(
                "c2",
                "aaaaaaaa-1",
                "2026-10-30T11:00:00Z",
                "fix: the thing\nmore",
            ),
            checkpoint("c4", "cccccccc-3", "2026-10-30T13:00:00Z", "mine"),
            checkpoint(
                "c5",
                "dddddddd-4",
                "2026-10-30T09:00:00Z",
                "<ci-monitor-event>\"Auto-fix\" is watching",
            ),
        ];
        let sessions = [Session {
            session_id: "aaaaaaaa-1".into(),
            agent: "Claude Code".into(),
            status: "ended".into(),
            files_touched: (0..7).map(|i| format!("f{i}")).collect(),
            ..Session::default()
        }];
        let since = time::parse_utc("2026-10-30T10:30:00Z");
        let r = recap(
            &checkpoints,
            &sessions,
            Some("cccccccc-3"),
            since,
            Vec::new(),
        );
        assert_eq!(r.checkpoints, 5);
        assert_eq!(r.since_handoff, 2);
        let ids: Vec<&str> = r.sessions.iter().map(|s| s.id.as_str()).collect();
        // b's checkpoint is 10:00 UTC, before a's newest.
        assert_eq!(ids, ["aaaaaaaa-1", "bbbbbbbb-2", "dddddddd-4"]);
        assert_eq!(
            r.sessions[2].last_checkpoint.message,
            "\"Auto-fix\" is watching"
        );
        assert_eq!(
            excerpt("<cross-session-message from=\"local_1\">\n\nplease rebase"),
            "please rebase"
        );
        assert_eq!(excerpt("a < b > c"), "a < b > c");
        let a = &r.sessions[0];
        assert_eq!((a.checkpoints, a.since_handoff), (2, 1));
        assert_eq!(a.last_checkpoint.message, "fix: the thing");
        assert_eq!((a.files.len(), a.file_count), (5, 7));
        let text = r.markdown();
        assert!(
            text.contains("- Claude Code `aaaaaaaa` (ended): 2 checkpoints, 1 since the handoff")
        );
        assert!(text.contains("  - files: f0, f1, f2, f3, f4, ... 2 more"));
        assert!(!text.contains("mine"));
    }

    #[test]
    fn an_origin_groups_agent_lines_by_checkpoint() {
        let line = |n, authorship: &str, cp: Option<&str>, commit: &str| WhyLine {
            line_number: n,
            authorship: authorship.into(),
            short_commit_sha: Some(commit.into()),
            checkpoint_id: cp.map(String::from),
            session_id: cp.map(|_| "8d2ac41d-2f33".into()),
            agent: cp.map(|_| "Claude Code".into()),
            prompt: cp.map(|_| "fix the flaky test GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz".into()),
        };
        let why = [
            line(1, "human", None, "aaaa"),
            line(2, "ai", Some("01M6"), "bbbb"),
            line(3, "ai", Some("01M6"), "bbbb"),
            line(4, "human", None, "cccc"),
            line(5, "ai", Some("01M6"), "bbbb"),
            line(6, "uncommitted", None, ""),
            line(9, "ai", Some("01M7"), "dddd"),
        ];
        let o = origin(&why, |n| (1..=6).contains(&n));
        assert_eq!(o.agent.len(), 1);
        assert_eq!(o.agent[0].lines, [2, 3, 5]);
        assert_eq!(
            (o.human_lines, o.human_commits, o.uncommitted_lines),
            (2, 2, 1)
        );
        assert_eq!(
            o.line(),
            "origin (entire why): 3 lines (2-3, 5) by Claude Code session 8d2ac41d, checkpoint 01M6 (commit bbbb): \"fix the flaky test GH_TOKEN=[redacted]\"; 2 lines by people (2 commits); 1 line not committed"
        );
    }
}
