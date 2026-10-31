//! Session review: the second layer of winding, beside the ledger's.
//!
//! The ledger sees command shapes, cheaply and all the time. A review sees
//! a whole session, through a [digest](jouet_audit::digest), and only now
//! and then. jouet calls no model itself, so the review is done by an
//! agent: `jouet wind review` prints a prompt and the digest for a fresh
//! subagent or a headless run (`claude -p`, `codex exec`), which records
//! what it proposes with `jouet wind propose`.
//!
//! With `[wind] review` on, the stop hook digests the session it ends and,
//! when the user corrected the agent or a lot was wasted, queues it; the
//! next session start in the same repository shows one line suggesting the
//! review, at most once every `review_every` sessions. Proposals surface
//! the same way, beside the ledger's notes and under the same limits, and
//! nothing is applied without the agent asking the user first.

use super::{MAX_SHOWINGS, Paths, State, Status};
use crate::settings::WindSettings;
use jouet_audit::digest::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Proposals recorded per review, and per day, at most.
pub(crate) const MAX_PER_REVIEW: usize = 3;
pub(crate) const MAX_PROPOSALS_PER_DAY: usize = 3;
/// The stop hook digests a session again only after this many seconds:
/// it fires at the end of every turn.
const CHECK_INTERVAL: u64 = 10 * 60;
/// Sessions the stop hook remembers having seen, for `review_every`.
const SEEN: usize = 200;
/// Waste that queues a review even without corrections: failed attempts
/// of commands that were retried, or time spent polling CI or a process
/// (as `jouet audit` counts it; a long build is work, not waste).
const FAILED_RETRIES: u64 = 6;
const POLLING_MS: u64 = 10 * 60 * 1000;

/// What a reviewer may propose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Task,
    Routine,
    Rule,
    AgentsMd,
    Memory,
    /// A change to a memory the agent already has, named by `target`.
    MemoryUpdate,
    /// Deleting a memory the agent has, named by `target`.
    MemoryDelete,
    /// A change to jouet itself, for its maintainers: recorded as an idea
    /// for `jouet share`, never as a pending proposal.
    Jouet,
    Nothing,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Routine => "routine",
            Self::Rule => "rule",
            Self::AgentsMd => "agents_md",
            Self::Memory => "memory",
            Self::MemoryUpdate => "memory_update",
            Self::MemoryDelete => "memory_delete",
            Self::Jouet => "jouet",
            Self::Nothing => "nothing",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Task => "a mise task",
            Self::Routine => "a routine",
            Self::Rule => "a rule",
            Self::AgentsMd => "an AGENTS.md note",
            Self::Memory => "a memory note",
            Self::MemoryUpdate => "an update to a memory",
            Self::MemoryDelete => "deleting a memory",
            Self::Jouet => "a change to jouet",
            Self::Nothing => "nothing",
        }
    }

    /// How an agent applies one, asking the user where it must.
    fn how(self) -> &'static str {
        match self {
            Self::Task => {
                "ask the user whether it goes in mise.local.toml (the default) or mise.toml, and add it with `mise tasks add`"
            }
            Self::Routine => {
                "ask the user before writing it as a Luau routine (or a mise task, if it needs no logic)"
            }
            Self::Rule => {
                "write it with `jouet rule new NAME` (warn mode) and check it with `jouet rule test NAME`"
            }
            Self::AgentsMd => "ask the user before adding it to the project's AGENTS.md",
            Self::Memory => "save it to your memory if the user agrees",
            Self::MemoryUpdate => "ask the user, then rewrite the memory with the draft",
            Self::MemoryDelete => {
                "ask the user, then delete the memory and its line in the index (MEMORY.md)"
            }
            Self::Jouet | Self::Nothing => "",
        }
    }

    /// The scopes a proposal of this kind may have, the default first.
    fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Task => &["local", "project", "global"],
            Self::Routine | Self::Rule | Self::Memory | Self::MemoryUpdate | Self::MemoryDelete => {
                &["user", "project"]
            }
            Self::AgentsMd => &["project"],
            Self::Jouet => &["maintainers"],
            Self::Nothing => &[""],
        }
    }
}

/// One improvement a review proposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub id: String,
    pub created: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session: String,
    pub kind: Kind,
    pub title: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    pub scope: String,
    /// The memory a `memory_update` or `memory_delete` changes: its path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub draft: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

/// A session the stop hook found worth a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Queued {
    pub id: String,
    pub created: u64,
    pub session: String,
    pub harness: String,
    /// The work tree, as the ledger keys it: a hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub corrections: u64,
    pub failed_retries: u64,
    pub polling_ms: u64,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

/// The review layer's part of the winding state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Reviews {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    #[serde(default)]
    pub queue: Vec<Queued>,
    /// When the stop hook last digested each session.
    #[serde(default)]
    pub checked: BTreeMap<String, u64>,
    /// Sessions the stop hook has seen, oldest first, and how many of
    /// them it had seen when it last queued one.
    #[serde(default)]
    pub seen: Vec<String>,
    #[serde(default)]
    pub seen_at_last_queue: Option<usize>,
}

impl Reviews {
    fn id(&mut self, prefix: char) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    pub(crate) fn proposal(&self, id: &str) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }

    pub(crate) fn queued(&self, id: &str) -> Option<&Queued> {
        self.queue.iter().find(|q| q.id == id)
    }

    /// Sets a proposal's or a queued review's status.
    pub(crate) fn set_status(&mut self, id: &str, status: Status) -> bool {
        if let Some(p) = self.proposals.iter_mut().find(|p| p.id == id) {
            p.status = status;
            return true;
        }
        if let Some(q) = self.queue.iter_mut().find(|q| q.id == id) {
            q.status = status;
            return true;
        }
        false
    }
}

/// What the stop hook measures in a digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Signals {
    /// Corrections, an interruption and the prompt after it counting once.
    pub corrections: u64,
    pub failed_retries: u64,
    pub polling_ms: u64,
}

impl Signals {
    pub(crate) fn of(d: &Digest) -> Self {
        Self {
            corrections: d.corrections.iter().filter(|c| !c.after_interrupt).count() as u64,
            failed_retries: d
                .failures
                .iter()
                .filter(|f| f.attempts > 1)
                .map(|f| f.failed)
                .sum(),
            polling_ms: d
                .sinks
                .iter()
                .filter(|s| matches!(s.category.as_str(), "ci_polling" | "process_polling"))
                .map(|s| s.duration_ms)
                .sum(),
        }
    }

    fn worth_reviewing(self, settings: &WindSettings) -> bool {
        self.corrections >= settings.review_corrections.max(1)
            || self.failed_retries >= FAILED_RETRIES
            || self.polling_ms >= POLLING_MS
    }
}

/// What the stop hook decided about a session.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// Digested too recently, or already queued.
    Skipped,
    /// Digested, and not worth a review (or not yet: `review_every`).
    Quiet,
    Queued(String),
}

/// Whether the stop hook should digest `session` now, remembering that it
/// did. Called before reading the transcript, which is the costly part.
pub(crate) fn due(paths: &Paths, session: &str, now: u64) -> bool {
    super::update(&paths.state, |state| {
        let r = &mut state.reviews;
        if r.queue.iter().any(|q| q.session == session) {
            return false;
        }
        if r.checked
            .get(session)
            .is_some_and(|t| now.saturating_sub(*t) < CHECK_INTERVAL)
        {
            return false;
        }
        r.checked.insert(session.to_string(), now);
        // Forget sessions not checked for a week.
        r.checked
            .retain(|_, t| now.saturating_sub(*t) < 7 * 24 * 60 * 60);
        if !r.seen.iter().any(|s| s == session) {
            r.seen.push(session.to_string());
            if r.seen.len() > SEEN {
                let drop = r.seen.len() - SEEN;
                r.seen.drain(..drop);
                r.seen_at_last_queue = r.seen_at_last_queue.map(|n| n.saturating_sub(drop));
            }
        }
        true
    })
    .unwrap_or(false)
}

/// Queues `session` for review when its digest crosses a threshold and
/// `review_every` sessions have passed since the last one queued.
pub(crate) fn consider(
    paths: &Paths,
    settings: &WindSettings,
    harness: &str,
    session: &str,
    repo: Option<String>,
    signals: Signals,
    now: u64,
) -> Check {
    if !signals.worth_reviewing(settings) {
        return Check::Quiet;
    }
    super::update(&paths.state, |state| {
        let r = &mut state.reviews;
        if r.queue.iter().any(|q| q.session == session) {
            return Check::Skipped;
        }
        let every = usize::try_from(settings.review_every.max(1)).unwrap_or(usize::MAX);
        if let Some(at) = r.seen_at_last_queue
            && r.seen.len().saturating_sub(at) < every
        {
            return Check::Quiet;
        }
        let id = r.id('r');
        r.queue.push(Queued {
            id: id.clone(),
            created: now,
            session: session.to_string(),
            harness: harness.to_string(),
            repo,
            corrections: signals.corrections,
            failed_retries: signals.failed_retries,
            polling_ms: signals.polling_ms,
            status: Status::Pending,
            shown: Vec::new(),
        });
        r.seen_at_last_queue = Some(r.seen.len());
        Check::Queued(id)
    })
    .unwrap_or(Check::Skipped)
}

/// The lines to show a starting session, at most `room`: queued reviews
/// of other sessions in its repository first, then proposals.
pub(crate) fn lines_for_session(
    paths: &Paths,
    session: &str,
    repo: Option<&str>,
    room: usize,
    now: u64,
) -> Vec<String> {
    if room == 0 {
        return Vec::new();
    }
    super::update(&paths.state, |state| {
        let r = &mut state.reviews;
        let mut lines = Vec::new();
        for q in r.queue.iter_mut() {
            if lines.len() == room {
                break;
            }
            if q.status != Status::Pending || q.session == session || q.repo.as_deref() != repo {
                continue;
            }
            mark_shown(&mut q.shown, &mut q.status, session);
            lines.push(queued_line(q, now));
        }
        for p in r.proposals.iter_mut() {
            if lines.len() == room {
                break;
            }
            if p.status != Status::Pending {
                continue;
            }
            mark_shown(&mut p.shown, &mut p.status, session);
            lines.push(proposal_line(p));
        }
        lines
    })
    .unwrap_or_default()
}

pub(super) fn mark_shown(shown: &mut Vec<String>, status: &mut Status, session: &str) {
    if !shown.iter().any(|s| s == session) {
        shown.push(session.to_string());
    }
    if shown.len() >= MAX_SHOWINGS {
        *status = Status::Lapsed;
    }
}

fn queued_line(q: &Queued, now: u64) -> String {
    let mut found = Vec::new();
    if q.corrections > 0 {
        found.push(format!("{} user corrections", q.corrections));
    }
    if q.failed_retries > 0 {
        found.push(format!("{} failed retries", q.failed_retries));
    }
    if q.polling_ms > 0 {
        found.push(format!(
            "{} polling",
            jouet_audit::digest::duration(q.polling_ms)
        ));
    }
    let short: String = q.session.chars().take(8).collect();
    let ago = jouet_output::time::age(i64::try_from(now.saturating_sub(q.created)).unwrap_or(0));
    format!(
        "jouet wind: session {short} ({ago} ago, this repository) had {}. At a natural pause, review it in a subagent: give it the output of `jouet wind review {}`, and it records what it proposes with `jouet wind propose`. `jouet wind dismiss {}` to skip it.",
        found.join(", "),
        q.session,
        q.id
    )
}

fn proposal_line(p: &Proposal) -> String {
    let target = if p.target.is_empty() {
        String::new()
    } else {
        format!(" `{}`", p.target)
    };
    format!(
        "jouet wind review proposed {}{target} ({}): {} `jouet wind show {}` for why and the draft; {}; then `jouet wind adopt {}`, or `jouet wind dismiss {}` to drop it.",
        p.kind.label(),
        p.id,
        sentence(&p.title),
        p.id,
        p.kind.how(),
        p.id,
        p.id
    )
}

fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// A review's reply, as `jouet wind propose` reads it.
#[derive(Debug, Deserialize)]
pub(crate) struct Reply {
    #[serde(default)]
    pub session: String,
    pub proposals: Vec<Draft>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Draft {
    pub kind: Kind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub draft: String,
}

/// What `jouet wind propose` did with each proposal.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Recorded {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recorded: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
    /// The review found nothing worth changing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub nothing: bool,
    /// Changes to jouet itself, recorded as ideas for `jouet share --idea`
    /// by these keys.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ideas: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Skipped {
    pub title: String,
    pub reason: String,
}

/// The memory a draft changes, as one of `targets` (their paths, with the
/// home directory as `~`), when it names one.
fn target_of(d: &Draft, targets: &[String]) -> Option<String> {
    let t = d.target.as_deref()?.trim();
    let t = super::memories::tilde(std::path::Path::new(t));
    targets.iter().find(|known| **known == t).cloned()
}

/// Why a draft cannot be recorded, if it cannot.
fn problem(d: &Draft, targets: &[String]) -> Option<String> {
    if d.title.trim().is_empty() || d.why.trim().is_empty() || d.draft.trim().is_empty() {
        return Some("a proposal needs a title, a why and a draft".into());
    }
    if d.title.chars().count() > 120 {
        return Some("the title is over 120 characters".into());
    }
    if let Some(scope) = &d.scope
        && !d.kind.scopes().contains(&scope.as_str())
    {
        return Some(format!(
            "{} takes the scope {}, not `{scope}`",
            d.kind.label(),
            d.kind.scopes().join(", ")
        ));
    }
    if matches!(d.kind, Kind::MemoryUpdate | Kind::MemoryDelete) && target_of(d, targets).is_none()
    {
        return Some(match d.target.as_deref() {
            None => format!("{} needs a target: the memory's path", d.kind.label()),
            Some(t) => format!("`{t}` is not one of the memories the review listed"),
        });
    }
    let span = |e: &str| {
        let e = e.strip_prefix('L').unwrap_or("");
        let mut parts = e.splitn(2, '-');
        parts.all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
    };
    if let Some(bad) = d.evidence.iter().find(|e| !span(e)) {
        return Some(format!("evidence `{bad}` is not a span such as L120-188"));
    }
    let text = format!("{}\n{}\n{}", d.title, d.why, d.draft);
    if crate::redact::find_secret(&text).is_some() {
        return Some("it holds something that looks like a secret".into());
    }
    None
}

/// Records a review's proposals as pending, within the limits, and its
/// changes to jouet itself as ideas for `jouet share`.
/// `targets` are the memories a `memory_update` or `memory_delete` may
/// name, by path.
pub(crate) fn record(
    paths: &Paths,
    reply: Reply,
    now: u64,
    targets: &[String],
) -> std::io::Result<Recorded> {
    let (mut out, ideas) =
        super::update(&paths.state, |state| record_in(state, reply, now, targets))?;
    if let Some(gaps) = crate::share::gaps_path() {
        for (title, body) in ideas {
            out.ideas
                .push(crate::share::record_review(&gaps, &title, &body, now)?);
        }
    }
    Ok(out)
}

fn record_in(
    state: &mut State,
    reply: Reply,
    now: u64,
    targets: &[String],
) -> (Recorded, Vec<(String, String)>) {
    let r = &mut state.reviews;
    let today = jouet_output::time::date(now);
    let mut budget = MAX_PROPOSALS_PER_DAY.saturating_sub(
        r.proposals
            .iter()
            .filter(|p| jouet_output::time::date(p.created) == today)
            .count(),
    );
    let mut out = Recorded::default();
    let mut ideas = Vec::new();
    let mut this_review = 0;
    for d in reply.proposals {
        if d.kind == Kind::Nothing {
            out.nothing = true;
            continue;
        }
        let skip = |reason: String| Skipped {
            title: d.title.clone(),
            reason,
        };
        if let Some(reason) = problem(&d, targets) {
            out.skipped.push(skip(reason));
            continue;
        }
        if d.kind == Kind::Jouet {
            let body = format!("{}\n\n{}", d.why.trim(), d.draft.trim());
            ideas.push((d.title.trim().to_string(), body));
            continue;
        }
        let target = target_of(&d, targets)
            .filter(|_| matches!(d.kind, Kind::MemoryUpdate | Kind::MemoryDelete))
            .unwrap_or_default();
        let duplicate = r.proposals.iter().any(|p| {
            p.kind == d.kind
                && (p.title.trim().eq_ignore_ascii_case(d.title.trim())
                    || (!target.is_empty() && p.target == target))
        });
        if duplicate {
            out.skipped.push(skip("already proposed".into()));
            continue;
        }
        if this_review == MAX_PER_REVIEW {
            out.skipped.push(skip(format!(
                "a review records {MAX_PER_REVIEW} proposals at most"
            )));
            continue;
        }
        if budget == 0 {
            out.skipped.push(skip(format!(
                "{MAX_PROPOSALS_PER_DAY} proposals a day at most; propose it again tomorrow"
            )));
            continue;
        }
        let id = r.id('p');
        r.proposals.push(Proposal {
            id: id.clone(),
            created: now,
            session: reply.session.clone(),
            kind: d.kind,
            title: d.title.trim().to_string(),
            why: d.why.trim().to_string(),
            evidence: d.evidence,
            scope: d.scope.unwrap_or_else(|| d.kind.scopes()[0].to_string()),
            target,
            draft: d.draft,
            status: Status::Pending,
            shown: Vec::new(),
        });
        out.recorded.push(id);
        this_review += 1;
        budget -= 1;
    }
    // A reviewed session is no longer waiting for its review.
    if !reply.session.is_empty() {
        for q in r.queue.iter_mut() {
            let same =
                reply.session.starts_with(&q.session) || q.session.starts_with(&reply.session);
            if q.status == Status::Pending && same {
                q.status = Status::Adopted;
            }
        }
    }
    (out, ideas)
}

/// The prompt `jouet wind review` prints before the digests; with
/// `contribute`, it may also propose changes to jouet itself.
pub(crate) fn prompt(contribute: bool) -> String {
    if !contribute {
        return PROMPT.to_string();
    }
    PROMPT.replace(
        "\n- \"nothing\":",
        "\n- \"jouet\": a change to jouet itself, for its maintainers: it crashed, exited with an odd code, printed malformed JSON, its flags confused the agent, or its output was abandoned for raw `gh` or `git` (see \"jouet itself\" in the digest). The draft is the report, with no private values. It is recorded as an idea for `jouet share`, which posts only after the user agrees. A memory that works around jouet is such a report too, beside a \"memory_delete\" for the memory.\n- \"nothing\":",
    )
}

const PROMPT: &str = r#"You are reviewing a coding-agent session to find changes to its environment that would have made it go better: fewer corrections from the user, fewer failed retries, less waiting, less exploring the same ground. You are not reviewing the code it wrote.

Below is a digest of the session from `jouet wind digest`: the user's corrections, failures, long calls, where tokens went, files read repeatedly, commands typed out again, and a timeline of phases, one per prompt. Each item has a span (L120-188) into the transcript named in its header, a JSONL file; when an item is unclear, read those lines (`sed -n '120,188p' FILE | cut -c1-600`), and only those.

Look for causes, not symptoms:
- A correction the user had to make: what did the agent not know, or not follow? Would a note or a rule have prevented it?
- An approach tried and abandoned, or a failure retried unchanged: what would have pointed the agent the right way first?
- Exploration repeated, within the session or across sessions: what project knowledge is missing?
- A long command typed out more than once: it may want to be a mise task.
- Waiting: sleeping or polling where `jouet wait` blocks on the condition itself.

For each improvement, choose one kind:
- "task": a mise task. Fixed commands, no decisions, specific to the project, useful to humans too. The default for anything retyped. Scope "local" (mise.local.toml, the default: absolute or home paths, personal flags, one specific test or PR), "project" (mise.toml: only paths inside the repo and tools already in mise.toml, named like the existing tasks, generally useful) or "global".
- "routine": a Luau program over jouet's primitives, when it needs logic over structured state (conditionals, parsing, git or GitHub state). Start with a task; escalate when conditionals creep in.
- "rule": a jouet rule that steers the agent's own tool calls, such as warning about a command and naming the task or command to use instead. Always warn mode.
- "agents_md": a line or two for the project's AGENTS.md: knowledge every agent in this repository needs.
- "memory": a note for the harness's memory: this user's preference, across projects.
- "memory_update": a memory listed under "Memories" that the session contradicts: the bug it works around did not happen, the user corrected a step it prescribes, or what it says is out of date. "target" is its path as listed; the draft is its new text.
- "memory_delete": a listed memory that is wrong or no longer needed. "target" is its path; the draft says why. A memory that works around jouet itself (a rule that misfires, a jouet command that fails) is a jouet bug, not a memory: propose deleting it.
- "nothing": nothing is worth changing. Most sessions need nothing; say so rather than invent a change. A one-off slip is not a pattern.

Propose at most three, each grounded in spans from the digest. Never put a secret, a token or a private value in a proposal.

Record them by piping this JSON to `jouet wind propose`; if you cannot run commands, reply with only the JSON:

{"session": "SESSION_ID", "proposals": [
  {"kind": "task", "title": "Add a `release:check` task", "why": "The agent tried `cargo xtask release-check` twice before the user named the task (L8-18).", "evidence": ["L8-14", "L18"], "scope": "local", "draft": "[tasks.\"release:check\"]\nrun = \"...\""}
]}

- title: at most 80 characters, in the imperative.
- why: one or two sentences: what went wrong, and how the change prevents it.
- draft: the change itself: the task's TOML, the rule, a sketch of the routine, the AGENTS.md lines, the memory note.
- scope: as above for a task; "user" or "project" for a rule, routine or memory note; "project" for AGENTS.md.
- target: for memory_update and memory_delete only, the memory's path exactly as listed.

Recording changes nothing: jouet shows each proposal at a later session start, and the agent there asks the user before applying it.
"#;

#[cfg(test)]
mod tests;
