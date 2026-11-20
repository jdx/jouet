//! `wait pr --until needs-agent`: wait for a PR to settle, then wake once
//! with everything the agent has to act on.
//!
//! The PR has settled when no check is pending and nothing about it (the
//! head, a check, mergeability, a review comment or thread) has changed for
//! the quiet window. It needs the agent when it has settled and has a failed
//! check, a conflict, or a review thread or top-level review comment posted
//! since the wait began that is not bot noise ([`crate::triage`]). Then the
//! wait prints one digest of all of them. A PR that settles with none of
//! these keeps being watched; one that is merged or closed ends the wait.
//!
//! A watcher an agent's session started holds a lease ([`crate::lease`]),
//! and the Auto-fix events jouet held back for it are part of its digest:
//! each one, with what became of it. A PR that settles with nothing to act
//! on but with held events still wakes the agent once, so a held event
//! always ends up in a digest.

use super::checks::{self, Check};
use super::host::Remote;
use super::lease::{self, Held};
use super::remote::{Halt, Poll, Watch};
use super::triage::{self, Acks, Place};
use super::watch::{ConflictCheck, PollError, Snapshot, now_ms, short};
use super::{Event, Reporter, Stop};
use jouet_github::Failure;
use jouet_github::pr::target::{Repo, Target};
use jouet_github::pr::threads::{self, Filter, Kind, PrThreads, Status, condense::condense};
use jouet_output::exit::Exit;
use jouet_output::time::{elapsed, format_utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Lines of each comment the digest shows.
const BODY_LINES: usize = 20;

#[derive(Clone)]
pub struct Options {
    /// How long nothing may change before the PR counts as settled.
    pub quiet: Duration,
    /// The shorter window used instead once every check has finished and
    /// no review bot says it is still reviewing.
    pub quiet_short: Option<Duration>,
    pub acks: Acks,
    /// Comments posted from this time on (Unix seconds) are new.
    pub since: i64,
    /// Lines of each failure log to show; `None` fetches no logs.
    pub log_lines: Option<usize>,
    pub confirm_conflict: Duration,
    /// The lease whose held events this wait reports.
    pub lease: Option<PathBuf>,
    /// The wait between polls once the PR has settled with nothing for the
    /// agent, instead of the usual back-off.
    pub settled_interval: Option<Duration>,
    /// jouet starts the next watcher itself after a push, so the digest
    /// does not tell the agent to.
    pub restarts: bool,
}

/// A held event, and what became of it.
#[derive(Debug, Clone, Serialize)]
pub struct HeldReport {
    /// When jouet held it.
    pub at: String,
    pub kinds: Vec<String>,
    pub prs: Vec<String>,
    /// Whether this digest shows what it reported, or that it is over.
    pub accounted: bool,
    pub notes: Vec<String>,
}

/// What a digest shows, and what the PR shows now, for [`account`].
struct Shown<'a> {
    open: bool,
    /// Every check on the head.
    checks: &'a [Check],
    /// The failed checks the digest shows.
    failed: &'a [Check],
    conflict: bool,
    items: usize,
    dropped: &'a BTreeMap<&'static str, usize>,
    /// What triage made of each thread or top-level comment, by its ID.
    judged: &'a BTreeMap<u64, Judged>,
}

/// What triage made of one thread or top-level comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Judged {
    Shown,
    Noise,
    /// The agent spoke last.
    Answered,
    /// Its newest comment is older than the wait's `--since`.
    Old,
}

/// What became of each held event, by what the digest shows.
fn account(held: &[Held], now: &Shown<'_>) -> Vec<HeldReport> {
    held.iter()
        .map(|h| {
            let mut notes = Vec::new();
            let mut accounted = true;
            let mut note = |ok: bool, text: String| {
                accounted &= ok;
                notes.push(text);
            };
            if !now.open {
                note(true, "the PR is closed".to_string());
            } else {
                for kind in &h.kinds {
                    match kind.as_str() {
                        "ci-failure" => {
                            for (ok, text) in check_notes(&h.checks, now.checks, now.failed) {
                                note(ok, text);
                            }
                        }
                        "conflict" if now.conflict => {
                            note(true, "the conflict is shown above".into());
                        }
                        "conflict" => note(true, "the conflict has cleared".into()),
                        "review-comment" if !h.comment_ids.is_empty() => {
                            for id in &h.comment_ids {
                                let (ok, text) = comment_note(*id, now);
                                note(ok, text);
                            }
                        }
                        "review-comment" => {
                            let (ok, text) = comments_note(now);
                            note(ok, text.to_string());
                        }
                        _ => note(false, format!("{kind}: not found on the PR now")),
                    }
                }
            }
            HeldReport {
                at: format_utc(h.at),
                kinds: h.kinds.clone(),
                prs: h.prs.clone(),
                accounted,
                notes,
            }
        })
        .collect()
}

/// What became of one comment a held event relayed, and whether that is
/// accounted for. A reply on a thread is not known by its own ID, so one
/// triage did not judge falls back to [`comments_note`].
fn comment_note(id: u64, now: &Shown<'_>) -> (bool, String) {
    match now.judged.get(&id) {
        Some(Judged::Shown) => (true, format!("comment {id}: shown above")),
        Some(Judged::Noise) => (true, format!("comment {id}: left out as bot noise")),
        Some(Judged::Answered) => (true, format!("comment {id}: answered since")),
        Some(Judged::Old) => (
            false,
            format!("comment {id}: older than this wait's --since, so not shown"),
        ),
        None => {
            let (ok, text) = comments_note(now);
            (ok, format!("comment {id}: {text}"))
        }
    }
}

/// What became of a held event's comments when they cannot be told apart.
fn comments_note(now: &Shown<'_>) -> (bool, &'static str) {
    if now.items > 0 {
        (true, "the review comments are shown above")
    } else if !now.dropped.is_empty() {
        (true, "its comments were left out as bot noise")
    } else {
        (false, "review-comment: not found on the PR now")
    }
}

/// What became of each check a held CI failure named, and whether that is
/// accounted for: a failure the digest does not show is not.
fn check_notes(named: &[String], checks: &[Check], failed: &[Check]) -> Vec<(bool, String)> {
    if named.is_empty() {
        let unshown = checks
            .iter()
            .any(|c| c.state.failed() && !c.derived() && !failed.iter().any(|f| f.name == c.name));
        return vec![if unshown {
            (false, "a failed check is not shown here".to_string())
        } else if failed.is_empty() {
            (true, "no check fails now".to_string())
        } else {
            (true, "the failed checks are shown above".to_string())
        }];
    }
    named
        .iter()
        .map(|name| match checks.iter().find(|c| &c.name == name) {
            Some(_) if failed.iter().any(|f| &f.name == name) => {
                (true, format!("{name}: shown above"))
            }
            Some(c) if c.state.failed() && c.derived() => (
                true,
                format!("{name}: fails only with other checks, or on a base update or an approval"),
            ),
            Some(c) if c.state.failed() => (false, format!("{name}: fails, not shown here")),
            Some(c) if c.state.finished() => (true, format!("{name}: failed, and passes now")),
            Some(_) => (true, format!("{name}: running again")),
            None => (true, format!("{name}: not on the head now")),
        })
        .collect()
}

/// A review thread or top-level comment the agent has to see.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Item {
    /// The thread's handle for `jouet pr threads reply`, or the review's or
    /// comment's ID.
    pub(crate) id: u64,
    /// `thread`, `review` or `comment`.
    pub(crate) kind: &'static str,
    pub(crate) author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) outdated: bool,
    /// The first comment: the finding.
    pub(crate) body: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) body_truncated: bool,
    /// The latest reply on a thread, when it is not the first comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) latest: Option<Reply>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Reply {
    pub(crate) author: String,
    pub(crate) body: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) body_truncated: bool,
}

/// One comment, as triage reads it.
#[derive(Debug, Clone)]
pub(crate) struct Said {
    pub(crate) author: String,
    pub(crate) created: i64,
    pub(crate) body: String,
}

/// What the review side of a PR asks of the agent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Activity {
    comments: usize,
    newest: i64,
    unresolved: usize,
}

/// Whether a thread or top-level comment needs the agent: `Ok(true)` when
/// it does, `Err(class)` when its newest comment is bot noise, `Ok(false)`
/// when it has nothing new or the agent spoke last.
pub(crate) fn needs_agent(
    place: Place,
    comments: &[Said],
    viewer: &str,
    since: i64,
    acks: Acks,
) -> Result<bool, &'static str> {
    let Some(last) = comments.last() else {
        return Ok(false);
    };
    if last.created < since || threads::same_login(viewer, &last.author) {
        return Ok(false);
    }
    match triage::noise(&last.author, &last.body, place, acks) {
        Some(class) => Err(class),
        None => Ok(true),
    }
}

fn said(t: &threads::Thread) -> Vec<Said> {
    t.comments
        .iter()
        .map(|c| Said {
            author: c.author.clone(),
            created: c.created,
            body: c.body.clone(),
        })
        .collect()
}

/// The threads and top-level comments that need the agent, how many of
/// each noise class were left out, and what triage made of each by its ID.
fn review(
    pr: &PrThreads,
    since: i64,
    acks: Acks,
) -> (
    Vec<Item>,
    BTreeMap<&'static str, usize>,
    Activity,
    BTreeMap<u64, Judged>,
) {
    let filter = Filter {
        status: Status::Unresolved,
        outdated: None,
        authors: Vec::new(),
        bots: None,
        since: None,
        paths: Vec::new(),
    };
    let mut items = Vec::new();
    let mut dropped = BTreeMap::new();
    let mut judged = BTreeMap::new();
    let mut activity = Activity::default();
    for t in threads::select(pr, &filter) {
        let comments = said(t);
        activity.comments += comments.len();
        activity.newest = comments
            .iter()
            .map(|c| c.created)
            .fold(activity.newest, i64::max);
        let (place, kind) = match t.kind {
            Kind::Thread => {
                activity.unresolved += 1;
                (Place::Thread, "thread")
            }
            Kind::Review => (Place::TopLevel, "review"),
            Kind::Comment => (Place::TopLevel, "comment"),
        };
        match needs_agent(place, &comments, pr.viewer(), since, acks) {
            Ok(false) => {
                let old = comments.last().is_some_and(|c| c.created < since);
                judged.insert(t.id, if old { Judged::Old } else { Judged::Answered });
            }
            Err(class) => {
                *dropped.entry(class).or_insert(0) += 1;
                judged.insert(t.id, Judged::Noise);
            }
            Ok(true) => {
                judged.insert(t.id, Judged::Shown);
                let first = condense(&comments[0].body, BODY_LINES);
                let latest = (comments.len() > 1).then(|| {
                    let last = &comments[comments.len() - 1];
                    let c = condense(&last.body, BODY_LINES);
                    Reply {
                        author: last.author.clone(),
                        body: c.body,
                        body_truncated: c.truncated,
                    }
                });
                items.push(Item {
                    id: t.id,
                    kind,
                    author: t.author.clone(),
                    path: t.path.clone(),
                    line: t.line,
                    outdated: t.outdated,
                    body: first.body,
                    body_truncated: first.truncated,
                    latest,
                });
            }
        }
    }
    (items, dropped, activity, judged)
}

/// What a change to any of resets the quiet window.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    head: String,
    checks: Vec<(String, &'static str)>,
    mergeable: (Option<bool>, String),
    activity: Activity,
}

pub struct SettleWait {
    target: Target,
    resolved: Option<(Repo, u64)>,
    options: Options,
    start: Instant,
    snap: Snapshot,
    last: Option<Fingerprint>,
    quiet_since: Instant,
    conflict: ConflictCheck,
    items: Vec<Item>,
    dropped: BTreeMap<&'static str, usize>,
    judged: BTreeMap<u64, Judged>,
    /// The conflict the latest poll confirmed.
    conflicted: Option<String>,
    /// When the PR last settled with nothing for the agent (Unix seconds).
    settled_at: Option<i64>,
    /// Whether the latest poll found it so.
    last_poll_settled: bool,
    /// The events jouet held for this wait, as of the latest poll.
    held: Vec<Held>,
}

#[derive(Debug, Serialize)]
pub struct Doc {
    pub(crate) repo: String,
    pub(crate) pr: u64,
    pub(crate) until: &'static str,
    /// `needs_agent`, `closed` or `timeout`.
    pub(crate) result: &'static str,
    pub(crate) state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) head_sha: String,
    pub(crate) elapsed_s: u64,
    pub(crate) quiet_s: u64,
    pub(crate) acks: &'static str,
    /// Comments from this time on counted as new.
    pub(crate) since: String,
    /// The `--since` for the next wait, so nothing posted in between is
    /// missed.
    pub(crate) next_since: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conflict: Option<String>,
    /// The failed checks on the head, with their log excerpts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) checks: Vec<Check>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) threads: Vec<Item>,
    /// Bot comments left out as noise, by class.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) dropped: BTreeMap<&'static str, usize>,
    /// The Auto-fix events jouet held back for this wait, and what became
    /// of each.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<HeldReport>,
    /// How long after the first held event this wait woke the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_after_first_held_s: Option<u64>,
}

impl Doc {
    /// `needs_agent`, `closed` or `timeout`.
    pub fn result(&self) -> &'static str {
        self.result
    }

    /// The commit the digest is about; empty when the PR could not be read.
    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }

    /// `owner/name#N`.
    pub fn pr(&self) -> String {
        format!("{}#{}", self.repo, self.pr)
    }

    pub fn quiet_s(&self) -> u64 {
        self.quiet_s
    }
}

impl SettleWait {
    pub fn new(target: Target, options: Options) -> Self {
        let conflict = ConflictCheck::new(options.confirm_conflict);
        Self {
            target,
            resolved: None,
            options,
            start: Instant::now(),
            snap: Snapshot::default(),
            last: None,
            quiet_since: Instant::now(),
            conflict,
            items: Vec::new(),
            dropped: BTreeMap::new(),
            judged: BTreeMap::new(),
            conflicted: None,
            settled_at: None,
            last_poll_settled: false,
            held: Vec::new(),
        }
    }

    fn doc(
        &self,
        result: &'static str,
        state: String,
        conflict: Option<String>,
        checks: Vec<Check>,
    ) -> Doc {
        let (repo, pr) = match &self.resolved {
            Some((repo, n)) => (repo.to_string(), *n),
            None => (self.target.to_string(), 0),
        };
        let held = account(
            &self.held,
            &Shown {
                open: result != "closed",
                checks: &self.snap.checks,
                failed: &checks,
                conflict: conflict.is_some(),
                items: self.items.len(),
                dropped: &self.dropped,
                judged: &self.judged,
            },
        );
        let wake_after_first_held_s = self
            .held
            .iter()
            .map(|h| h.at)
            .min()
            .map(|first| u64::try_from(jouet_output::time::now() - first).unwrap_or(0));
        Doc {
            repo,
            pr,
            until: "needs-agent",
            result,
            state,
            head_sha: self
                .snap
                .pr
                .as_ref()
                .map(|p| p.head_sha.clone())
                .unwrap_or_default(),
            elapsed_s: self.start.elapsed().as_secs(),
            quiet_s: self.options.quiet.as_secs(),
            acks: self.options.acks.as_str(),
            since: format_utc(self.options.since),
            next_since: format_utc(jouet_output::time::now()),
            conflict,
            checks,
            threads: self.items.clone(),
            dropped: self.dropped.clone(),
            held,
            wake_after_first_held_s,
        }
    }

    /// Rereads the events held for this wait.
    fn read_held(&mut self) {
        if let Some(path) = &self.options.lease {
            self.held = lease::held(path);
        }
    }

    fn remote_error(e: PollError) -> Halt {
        match e {
            PollError::Remote(Failure::RateLimited { reset }) => Halt::RateLimited { reset },
            PollError::Remote(f) => f.into(),
            PollError::NotFound(message) => Halt::Stop(Stop {
                exit: Exit::NotFound,
                message,
            }),
        }
    }
}

impl Watch for SettleWait {
    type Doc = Doc;

    fn label(&self) -> String {
        format!("pr {} until needs-agent", self.target)
    }

    fn poll(
        &mut self,
        remote: &mut Remote,
        _interval: Duration,
        _events: &mut Vec<Event>,
    ) -> Result<Poll<Doc>, Halt> {
        if !matches!(remote, Remote::GitHub(_)) {
            return Err(Halt::Stop(Stop {
                exit: Exit::Precondition,
                message:
                    "--until needs-agent reads review threads, which jouet reads on GitHub only"
                        .into(),
            }));
        }
        let (repo, number) = match &self.resolved {
            Some(r) => r.clone(),
            None => {
                let r = remote.resolve(self.target.clone())?;
                self.resolved = Some(r.clone());
                r
            }
        };
        self.read_held();
        remote
            .ci()
            .poll(&repo, number, None, &mut self.snap)
            .map_err(Self::remote_error)?;
        let Some(pr) = self.snap.pr.clone() else {
            return Err(Halt::Stop(Stop {
                exit: Exit::NotFound,
                message: format!("no PR for {}", self.target),
            }));
        };
        if pr.state != "open" {
            let state = pr.state.clone();
            return Ok(Poll::Done(self.doc("closed", state, None, Vec::new())));
        }
        let fetched = remote
            .host()
            .threads(&Target::Number(repo.clone(), number), true, &[])
            .map_err(|message| {
                Halt::Stop(Stop {
                    exit: Exit::Remote,
                    message,
                })
            })?
            .ok_or_else(|| {
                Halt::Stop(Stop {
                    exit: Exit::NotFound,
                    message: format!("{repo}#{number} does not exist"),
                })
            })?;
        self.last_poll_settled = false;
        let (items, dropped, activity, judged) =
            review(&fetched, self.options.since, self.options.acks);
        self.items = items;
        self.dropped = dropped;
        self.judged = judged;
        let now = Fingerprint {
            head: pr.head_sha.clone(),
            checks: self
                .snap
                .checks
                .iter()
                .map(|c| (c.name.clone(), c.state.as_str()))
                .collect(),
            mergeable: (pr.mergeable, pr.mergeable_state.clone()),
            activity,
        };
        if self.last.as_ref() != Some(&now) {
            self.last = Some(now);
            self.quiet_since = Instant::now();
        }
        let conflict = self
            .conflict
            .confirmed(pr.conflicted(), now_ms())
            .then(|| format!("{repo}#{number} conflicts with {}", pr.base_ref));
        self.conflicted.clone_from(&conflict);
        let pending = self
            .snap
            .checks
            .iter()
            .filter(|c| !c.state.finished())
            .count();
        // A conflicted PR gets no pull_request CI, so its checks never settle.
        if pending > 0 && conflict.is_none() {
            return Ok(Poll::Pending(format!(
                "{pending} check{} pending on {}",
                if pending == 1 { "" } else { "s" },
                short(&pr.head_sha)
            )));
        }
        let quiet = self.quiet_since.elapsed();
        // Once every check has finished and no bot says it is still
        // reviewing, nothing is known to be on its way.
        let window = match self.options.quiet_short {
            Some(short) if pending == 0 && !self.dropped.contains_key("in_progress") => {
                short.min(self.options.quiet)
            }
            _ => self.options.quiet,
        };
        if quiet < window {
            return Ok(Poll::Pending(format!(
                "settling: quiet for {} of {}",
                elapsed(quiet.as_secs()),
                elapsed(window.as_secs())
            )));
        }
        let mut failed: Vec<Check> = self
            .snap
            .checks
            .iter()
            // An Entire Gates failure that only repeats other checks, or
            // waits on a base update or an approval, is not one to fix.
            .filter(|c| c.state.failed() && !c.derived())
            .cloned()
            .collect();
        if failed.is_empty() && conflict.is_none() && self.items.is_empty() {
            self.settled_at = Some(jouet_output::time::now());
            // Held events wake the agent once, to say what became of them.
            if !self.held.is_empty() {
                let n = self.held.len();
                let state = format!(
                    "nothing needs the agent; {n} held Auto-fix event{} settled",
                    if n == 1 { "" } else { "s" }
                );
                return Ok(Poll::Done(self.doc("needs_agent", state, None, failed)));
            }
            self.last_poll_settled = true;
            return Ok(Poll::Pending("settled; nothing needs the agent".into()));
        }
        if let Some(lines) = self.options.log_lines {
            remote.ci().attach_logs(&repo, &mut failed, lines);
        }
        let state = summary(failed.len(), conflict.is_some(), self.items.len());
        Ok(Poll::Done(self.doc("needs_agent", state, conflict, failed)))
    }

    fn timed_out(&mut self, _remote: &mut Remote) -> Doc {
        self.read_held();
        // Comments seen before the PR last settled with nothing to do were
        // judged; any later ones were not, so the next wait starts there.
        let (state, next) = match self.settled_at {
            Some(at) if self.last_poll_settled => ("nothing needed the agent".to_string(), at),
            _ => (
                "the PR had not settled".to_string(),
                self.settled_at.unwrap_or(self.options.since),
            ),
        };
        // What the latest poll found failing is shown, so a held event is
        // not said to be shown when it is not.
        let failed = self
            .snap
            .checks
            .iter()
            .filter(|c| c.state.failed() && !c.derived())
            .cloned()
            .collect();
        let mut doc = self.doc("timeout", state, self.conflicted.clone(), failed);
        doc.next_since = format_utc(next);
        doc
    }

    fn text(&self, doc: &Doc, reporter: &Reporter) -> String {
        let (word, good) = match doc.result {
            "needs_agent" => ("NEEDS_AGENT", false),
            "closed" => ("CLOSED", true),
            _ => ("TIMEOUT", true),
        };
        let mut out = format!(
            "{} {}#{} @ {}: {} ({})\n",
            reporter.word(word, good),
            doc.repo,
            doc.pr,
            short(&doc.head_sha),
            doc.state,
            elapsed(doc.elapsed_s)
        );
        if let Some(conflict) = &doc.conflict {
            let _ = writeln!(out, "  conflict: {conflict}; merge the base branch in");
        }
        out.push_str(&checks::failure_blocks(&doc.checks));
        for item in &doc.threads {
            let place = match (&item.path, item.line) {
                (Some(p), Some(l)) => format!(" on {p}:{l}"),
                (Some(p), None) => format!(" on {p}"),
                _ => String::new(),
            };
            let outdated = if item.outdated { " (outdated)" } else { "" };
            let _ = writeln!(
                out,
                "  {} {} by {}{place}{outdated}:",
                item.kind, item.id, item.author
            );
            push_body(&mut out, &item.body, item.body_truncated);
            if let Some(r) = &item.latest {
                let _ = writeln!(out, "    latest, by {}:", r.author);
                push_body(&mut out, &r.body, r.body_truncated);
            }
        }
        if !doc.dropped.is_empty() {
            let classes: Vec<String> = doc
                .dropped
                .iter()
                .map(|(k, v)| format!("{v} {k}"))
                .collect();
            let _ = writeln!(out, "  left out as bot noise: {}", classes.join(", "));
        }
        out.push_str(&held_text(&doc.held));
        if doc.result != "closed" && self.options.restarts {
            out.push_str("  next: jouet watches this PR again after your next push\n");
        } else if doc.result != "closed" {
            let _ = writeln!(
                out,
                "  next: jouet wait pr {} --until needs-agent --since {}",
                doc.pr, doc.next_since
            );
        }
        out
    }

    fn exit(&self, doc: &Doc) -> Exit {
        match doc.result {
            "needs_agent" => Exit::Ok,
            "closed" => Exit::Failed,
            // The agent has held events to hear about.
            _ if !doc.held.is_empty() => Exit::Ok,
            _ => Exit::Timeout,
        }
    }

    fn interval(&self) -> Option<Duration> {
        self.options
            .settled_interval
            .filter(|_| self.last_poll_settled)
    }
}

/// The held events, for the text digest.
pub fn held_text(held: &[HeldReport]) -> String {
    let mut out = String::new();
    if held.is_empty() {
        return out;
    }
    out.push_str("  held for this wait (jouet kept these Auto-fix events from starting a turn):\n");
    for h in held {
        let _ = writeln!(
            out,
            "    {} {} on {}: {}",
            h.at,
            h.kinds.join(", "),
            h.prs.join(", "),
            h.notes.join("; ")
        );
    }
    out
}

fn summary(failed: usize, conflict: bool, items: usize) -> String {
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!(
            "{failed} failed check{}",
            if failed == 1 { "" } else { "s" }
        ));
    }
    if conflict {
        parts.push("a conflict".into());
    }
    if items > 0 {
        parts.push(format!(
            "{items} new review comment{}",
            if items == 1 { "" } else { "s" }
        ));
    }
    parts.join(", ")
}

fn push_body(out: &mut String, body: &str, truncated: bool) {
    for line in body.lines() {
        let _ = writeln!(out, "    {line}");
    }
    if truncated {
        out.push_str("    ... (cut; full text: jouet pr threads)\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(author: &str, created: i64, body: &str) -> Said {
        Said {
            author: author.into(),
            created,
            body: body.into(),
        }
    }

    #[test]
    fn only_new_comments_from_others_that_are_not_noise_wake_the_agent() {
        let finding = c(
            "coderabbitai[bot]",
            100,
            "_⚠️ Potential issue_ | _🟠 Major_ off by one",
        );
        let l = Acks::Loose;
        assert_eq!(
            needs_agent(Place::Thread, std::slice::from_ref(&finding), "me", 50, l),
            Ok(true)
        );
        // Posted before the wait began.
        assert_eq!(
            needs_agent(Place::Thread, std::slice::from_ref(&finding), "me", 150, l),
            Ok(false)
        );
        // The agent replied last, so the thread waits on the reviewer.
        let reply = c("me", 120, "Fixed in abc123.");
        assert_eq!(
            needs_agent(
                Place::Thread,
                &[finding.clone(), reply.clone()],
                "me",
                50,
                l
            ),
            Ok(false)
        );
        let ack = c("coderabbitai[bot]", 130, "Thanks for the fix!");
        assert_eq!(
            needs_agent(
                Place::Thread,
                &[finding.clone(), reply.clone(), ack],
                "me",
                50,
                l
            ),
            Err("ack")
        );
        let more = c(
            "coderabbitai[bot]",
            130,
            "Thanks, but the empty case is still not handled.",
        );
        assert_eq!(
            needs_agent(
                Place::Thread,
                &[finding, reply, more],
                "me",
                50,
                Acks::Strict
            ),
            Ok(true)
        );
        let limit = c("coderabbitai[bot]", 100, "## Review limit reached");
        assert_eq!(
            needs_agent(Place::TopLevel, &[limit], "me", 50, l),
            Err("rate_limit")
        );
    }

    #[test]
    fn the_summary_names_what_needs_the_agent() {
        assert_eq!(
            summary(2, true, 1),
            "2 failed checks, a conflict, 1 new review comment"
        );
        assert_eq!(summary(0, false, 3), "3 new review comments");
    }

    fn held(kinds: &[&str], checks: &[&str]) -> Held {
        Held {
            at: 1_000,
            prs: vec!["jdx/jouet#12".into()],
            kinds: kinds.iter().map(|k| (*k).to_string()).collect(),
            comments: 0,
            checks: checks.iter().map(|c| (*c).to_string()).collect(),
            comment_ids: Vec::new(),
        }
    }

    fn shown<'a>(
        checks: &'a [Check],
        failed: &'a [Check],
        dropped: &'a BTreeMap<&'static str, usize>,
        judged: &'a BTreeMap<u64, Judged>,
    ) -> Shown<'a> {
        Shown {
            open: true,
            checks,
            failed,
            conflict: false,
            items: 0,
            dropped,
            judged,
        }
    }

    #[test]
    fn each_held_event_says_what_became_of_it() {
        let checks = [
            Check::new("lint".into(), checks::State::Fail),
            Check::new("test".into(), checks::State::Pass),
            Check::new("build".into(), checks::State::Pending),
        ];
        let none = BTreeMap::new();
        let unjudged = BTreeMap::new();
        let ci = held(&["ci-failure"], &["lint", "test", "build", "gone"]);
        let all = shown(&checks, &checks[..1], &none, &unjudged);
        let [r] = account(std::slice::from_ref(&ci), &all).try_into().unwrap();
        assert!(r.accounted);
        assert_eq!(
            r.notes,
            [
                "lint: shown above",
                "test: failed, and passes now",
                "build: running again",
                "gone: not on the head now"
            ]
        );
        // A timeout digest that does not show a failure does not say it does.
        let unshown = account(
            std::slice::from_ref(&ci),
            &shown(&checks, &[], &none, &unjudged),
        );
        assert!(!unshown[0].accounted);
        assert_eq!(unshown[0].notes[0], "lint: fails, not shown here");
        let both = held(&["review-comment", "conflict"], &[]);
        let found = account(
            std::slice::from_ref(&both),
            &Shown {
                conflict: true,
                items: 1,
                ..shown(&checks, &checks[..1], &none, &unjudged)
            },
        );
        assert_eq!(
            found[0].notes,
            [
                "the review comments are shown above",
                "the conflict is shown above"
            ]
        );
        // A comment the digest neither shows nor left out as noise went
        // missing: a silent drop.
        let lost = account(
            std::slice::from_ref(&both),
            &shown(&[], &[], &none, &unjudged),
        );
        assert!(!lost[0].accounted);
        assert_eq!(
            lost[0].notes,
            [
                "review-comment: not found on the PR now",
                "the conflict has cleared"
            ]
        );
        let noise = BTreeMap::from([("rate_limit", 1)]);
        assert!(
            account(
                std::slice::from_ref(&both),
                &shown(&[], &[], &noise, &unjudged)
            )[0]
            .accounted
        );
        let closed = account(
            &[ci],
            &Shown {
                open: false,
                ..shown(&[], &[], &none, &unjudged)
            },
        );
        assert_eq!(
            (closed[0].accounted, &closed[0].notes[..]),
            (true, &["the PR is closed".to_string()][..])
        );
        assert_eq!(
            held_text(&closed),
            format!(
                "  held for this wait (jouet kept these Auto-fix events from starting a turn):\n    {} ci-failure on jdx/jouet#12: the PR is closed\n",
                format_utc(1_000)
            )
        );
        assert_eq!(
            check_notes(&[], &checks[..1], &checks[..1]),
            [(true, "the failed checks are shown above".to_string())]
        );
        assert_eq!(
            check_notes(&[], &checks[1..], &[]),
            [(true, "no check fails now".to_string())]
        );
    }

    #[test]
    fn each_held_comment_is_matched_by_its_id() {
        let judged = BTreeMap::from([
            (1, Judged::Shown),
            (2, Judged::Noise),
            (3, Judged::Answered),
            (4, Judged::Old),
        ]);
        let none = BTreeMap::new();
        let now = Shown {
            items: 1,
            ..shown(&[], &[], &none, &judged)
        };
        let mut comments = held(&["review-comment"], &[]);
        comments.comment_ids = vec![1, 2, 3];
        let [r] = account(std::slice::from_ref(&comments), &now)
            .try_into()
            .unwrap();
        assert!(r.accounted);
        assert_eq!(
            r.notes,
            [
                "comment 1: shown above",
                "comment 2: left out as bot noise",
                "comment 3: answered since"
            ]
        );
        // Another comment being shown does not account for one older than
        // the wait's --since.
        comments.comment_ids = vec![1, 4];
        let old = account(std::slice::from_ref(&comments), &now);
        assert!(!old[0].accounted);
        assert_eq!(
            old[0].notes[1],
            "comment 4: older than this wait's --since, so not shown"
        );
        // A reply on a thread is not known by its own ID.
        comments.comment_ids = vec![9];
        assert_eq!(
            account(std::slice::from_ref(&comments), &now)[0].notes,
            ["comment 9: the review comments are shown above"]
        );
    }
}
