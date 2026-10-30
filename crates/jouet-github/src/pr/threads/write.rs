//! Replying to, resolving and unresolving threads.
//!
//! Every request is checked against the PR before anything is posted, so a
//! bad ID or a thread someone already resolved fails the whole call with
//! nothing written. Writes then go one at a time and stop at the first
//! failure. A retry is safe: a thread already resolved is not resolved again,
//! and a reply whose exact text is already the thread's last comment from
//! the same user is not posted again.

use super::{PrThreads, Thread, same_login};
use crate::Client;
use jouet_output::exit::Exit;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

/// What to do to one thread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    #[serde(deserialize_with = "thread_id")]
    pub id: u64,
    #[serde(default)]
    pub reply: Option<String>,
    #[serde(default)]
    pub resolve: bool,
    #[serde(skip)]
    pub unresolve: bool,
}

/// A thread ID as a JSON number or a string of digits.
fn thread_id<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u64),
        Text(String),
    }
    match Id::deserialize(d)? {
        Id::Number(n) => Ok(n),
        Id::Text(s) => parse_id(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("`{s}` is not a thread ID"))),
    }
}

/// A thread ID: the database ID of its first comment.
pub fn parse_id(s: &str) -> Option<u64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
        .filter(|&n| n > 0)
}

/// Reads an `apply` file: a JSON list of `{id, reply?, resolve?}`.
pub fn parse_apply(text: &str) -> Result<Vec<Entry>, String> {
    let entries: Vec<Entry> = serde_json::from_str(text).map_err(|e| format!("apply file: {e}"))?;
    if entries.is_empty() {
        return Err("apply file: the list is empty".into());
    }
    for (i, e) in entries.iter().enumerate() {
        if e.reply.is_none() && !e.resolve {
            return Err(format!("apply file: {} neither replies nor resolves", e.id));
        }
        if e.reply.as_deref().is_some_and(|r| r.trim().is_empty()) {
            return Err(format!("apply file: the reply to {} is empty", e.id));
        }
        if entries[..i].iter().any(|o| o.id == e.id) {
            return Err(format!("apply file: {} is listed twice", e.id));
        }
    }
    Ok(entries)
}

pub struct Options {
    /// Appended to every reply after a blank line.
    pub footer: Option<String>,
    /// The short HEAD SHA that `{sha}` expands to, when there is one.
    pub sha: Option<String>,
    /// Skip threads already resolved instead of failing.
    pub ignore_resolved: bool,
    /// Treat a resolve of an already-resolved thread as done rather than as
    /// a mistake: `apply` is meant to be re-run after a partial failure.
    pub retry_safe: bool,
}

/// What happened to one thread, or in a dry run what would.
#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub id: u64,
    #[serde(skip_serializing_if = "is_false")]
    pub replied: bool,
    /// The reply was already the thread's last comment, so it was not
    /// posted again.
    #[serde(skip_serializing_if = "is_false")]
    pub already_replied: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub resolved: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub unresolved: bool,
    /// The reply's URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// In a dry run, the reply exactly as it would be posted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    #[serde(skip)]
    node_id: String,
    #[serde(skip)]
    post: Option<String>,
    #[serde(skip)]
    resolve: bool,
    #[serde(skip)]
    unresolve: bool,
    #[serde(skip)]
    uses_sha: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Outcome {
    /// Whether this will post a reply that names `{sha}`.
    pub fn posts_sha(&self) -> bool {
        self.uses_sha && self.post.is_some()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlanError {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

/// Checks every entry against the PR and works out what is left to do.
pub fn plan(entries: &[Entry], pr: &PrThreads, o: &Options) -> Result<Vec<Outcome>, PlanError> {
    let find = |id: u64| pr.threads.iter().find(|t| t.id == id);
    let unknown: Vec<String> = entries
        .iter()
        .filter(|e| find(e.id).is_none())
        .map(|e| {
            match pr
                .threads
                .iter()
                .find(|t| t.comments.iter().any(|c| c.id == e.id))
            {
                Some(t) => format!("{} (a reply in thread {})", e.id, t.id),
                None => e.id.to_string(),
            }
        })
        .collect();
    if !unknown.is_empty() {
        let noun = if unknown.len() == 1 {
            "thread"
        } else {
            "threads"
        };
        return Err(PlanError {
            exit: Exit::NotFound,
            message: format!(
                "no review {noun} {} on {}#{}; nothing was posted",
                unknown.join(", "),
                pr.repo,
                pr.number
            ),
            hint: Some(
                "a thread's ID is its first comment's ID, as `jouet pr threads --all` lists".into(),
            ),
        });
    }
    let mut conflicts = Vec::new();
    let mut outcomes = Vec::new();
    for e in entries {
        let t = find(e.id).expect("unknown IDs were rejected");
        let mut out = Outcome {
            id: e.id,
            node_id: t.node_id.clone(),
            ..Outcome::default()
        };
        if let Some(reply) = &e.reply {
            out.uses_sha = reply.contains("{sha}");
            let text = reply_text(reply, o)?;
            match last_reply(t, &pr.viewer, &text) {
                Some(url) => {
                    out.already_replied = true;
                    out.url = Some(url.to_string());
                }
                None => out.post = Some(text),
            }
        }
        if e.unresolve {
            if t.resolved {
                out.unresolve = true;
            } else {
                out.skipped = Some("not resolved".into());
            }
        } else if t.resolved && (out.post.is_some() || e.resolve) {
            if (out.post.is_none() && o.retry_safe) || o.ignore_resolved {
                out.skipped = Some("already resolved".into());
                out.post = None;
            } else {
                conflicts.push(e.id.to_string());
            }
        } else if out.post.is_none() && !e.resolve {
            out.skipped = Some("reply already posted".into());
        } else {
            out.resolve = e.resolve;
        }
        outcomes.push(out);
    }
    if !conflicts.is_empty() {
        let (noun, verb) = if conflicts.len() == 1 {
            ("thread", "is")
        } else {
            ("threads", "are")
        };
        return Err(PlanError {
            exit: Exit::Precondition,
            message: format!(
                "{noun} {} {verb} already resolved; nothing was posted",
                conflicts.join(", ")
            ),
            hint: Some("--ignore-resolved skips resolved threads".into()),
        });
    }
    Ok(outcomes)
}

/// The reply as it will be posted: `{sha}` expanded, then the footer.
fn reply_text(reply: &str, o: &Options) -> Result<String, PlanError> {
    let mut text = reply.trim_end().to_string();
    if text.contains("{sha}") {
        let Some(sha) = &o.sha else {
            return Err(PlanError {
                exit: Exit::Precondition,
                message: "a reply uses {sha}, but there is no git commit here for it to name; nothing was posted".into(),
                hint: Some("run jouet in the PR's checkout".into()),
            });
        };
        text = text.replace("{sha}", sha);
    }
    if let Some(footer) = &o.footer {
        text = format!("{text}\n\n{footer}");
    }
    Ok(text)
}

/// The URL of the thread's last comment when the viewer wrote it and it
/// reads exactly `text`.
fn last_reply<'a>(t: &'a Thread, viewer: &str, text: &str) -> Option<&'a str> {
    let norm = |s: &str| s.replace("\r\n", "\n").trim().to_string();
    let last = t.comments.last()?;
    (!viewer.is_empty() && same_login(viewer, &last.author) && norm(&last.body) == norm(text))
        .then_some(last.url.as_str())
}

/// Fills in a dry run's outcomes with what would happen.
pub fn dry_run(outcomes: &mut [Outcome]) {
    for o in outcomes {
        o.replied = o.post.is_some();
        o.body = o.post.take();
        o.resolved = o.resolve;
        o.unresolved = o.unresolve;
    }
}

const REPLY: &str = "mutation($id: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $id, body: $body}) { comment { url } }
}";
const RESOLVE: &str =
    "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }";
const UNRESOLVE: &str = "mutation($id: ID!) { unresolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }";

/// Carries out the plan, stopping at the first failure. Returns whether
/// every write succeeded.
pub fn execute(client: &mut Client, outcomes: &mut [Outcome]) -> bool {
    let mut ok = true;
    for o in outcomes {
        if o.skipped.is_some() {
            continue;
        }
        if !ok {
            o.failed = Some("not attempted after an earlier failure".into());
            continue;
        }
        if let Err(e) = apply(client, o) {
            o.failed = Some(e);
            ok = false;
        }
    }
    ok
}

fn apply(client: &mut Client, o: &mut Outcome) -> Result<(), String> {
    if let Some(body) = o.post.take() {
        let data = client.mutate(REPLY, &json!({ "id": o.node_id, "body": body }))?;
        let url = data["addPullRequestReviewThreadReply"]["comment"]["url"]
            .as_str()
            .ok_or("GitHub did not post the reply")?;
        o.replied = true;
        o.url = Some(url.to_string());
    }
    for (wanted, query, resolved) in [(o.resolve, RESOLVE, true), (o.unresolve, UNRESOLVE, false)] {
        if !wanted {
            continue;
        }
        let data = client.mutate(query, &json!({ "id": o.node_id }))?;
        let field = if resolved {
            "resolveReviewThread"
        } else {
            "unresolveReviewThread"
        };
        if data[field]["thread"]["isResolved"].as_bool() != Some(resolved) {
            return Err(format!(
                "GitHub did not {} the thread",
                &field[..field.len() - "ReviewThread".len()]
            ));
        }
        if resolved {
            o.resolved = true;
        } else {
            o.unresolved = true;
        }
    }
    Ok(())
}
