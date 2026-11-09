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

use super::checks::{self, Check};
use super::host::Remote;
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
use std::time::{Duration, Instant};

/// Lines of each comment the digest shows.
const BODY_LINES: usize = 20;

pub struct Options {
    /// How long nothing may change before the PR counts as settled.
    pub quiet: Duration,
    pub acks: Acks,
    /// Comments posted from this time on (Unix seconds) are new.
    pub since: i64,
    /// Lines of each failure log to show; `None` fetches no logs.
    pub log_lines: Option<usize>,
    pub confirm_conflict: Duration,
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

/// The threads and top-level comments that need the agent, and how many
/// of each noise class were left out.
fn review(
    pr: &PrThreads,
    since: i64,
    acks: Acks,
) -> (Vec<Item>, BTreeMap<&'static str, usize>, Activity) {
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
            Ok(false) => {}
            Err(class) => *dropped.entry(class).or_insert(0) += 1,
            Ok(true) => {
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
    (items, dropped, activity)
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
        let (items, dropped, activity) = review(&fetched, self.options.since, self.options.acks);
        self.items = items;
        self.dropped = dropped;
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
        if quiet < self.options.quiet {
            return Ok(Poll::Pending(format!(
                "settling: quiet for {} of {}",
                elapsed(quiet.as_secs()),
                elapsed(self.options.quiet.as_secs())
            )));
        }
        let mut failed: Vec<Check> = self
            .snap
            .checks
            .iter()
            .filter(|c| c.state.failed())
            .cloned()
            .collect();
        if failed.is_empty() && conflict.is_none() && self.items.is_empty() {
            return Ok(Poll::Pending("settled; nothing needs the agent".into()));
        }
        if let Some(lines) = self.options.log_lines {
            remote.ci().attach_logs(&repo, &mut failed, lines);
        }
        let state = summary(failed.len(), conflict.is_some(), self.items.len());
        Ok(Poll::Done(self.doc("needs_agent", state, conflict, failed)))
    }

    fn timed_out(&mut self, _remote: &mut Remote) -> Doc {
        self.doc(
            "timeout",
            "nothing needed the agent".into(),
            None,
            Vec::new(),
        )
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
        if doc.result != "closed" {
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
            _ => Exit::Timeout,
        }
    }
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
}
