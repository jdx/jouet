//! Replying to, resolving and unresolving threads.
//!
//! Every request is checked against the PR before anything is posted, so a
//! bad ID fails the whole call with nothing written. A thread someone already
//! resolved, often a review bot after a push, still takes a reply and stays
//! resolved. Writes then go one at a time and stop at the first failure. A
//! retry is safe: a thread already resolved is not resolved again, and a reply
//! whose exact text is already the thread's last comment from the same user
//! is not posted again.

use super::{PrThreads, Thread, same_login};
use crate::host::Host;
use jouet_output::exit::Exit;
use serde::{Deserialize, Deserializer, Serialize};

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
    /// Named among `resolve`'s or `unresolve`'s arguments, where a PR written
    /// after the IDs (`resolve 4435734278 1098`) arrives as one: then an ID
    /// that is the PR's own number and no thread's is the PR. An `apply`
    /// file names threads only, so there it is an unknown thread.
    #[serde(skip)]
    pub among_args: bool,
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
    /// Leave threads already resolved alone, posting nothing to them.
    pub ignore_resolved: bool,
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
    /// The thread was resolved before: the reply went in, and it stays
    /// resolved.
    #[serde(skip_serializing_if = "is_false")]
    pub already_resolved: bool,
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

    /// Whether this will resolve or unresolve the thread.
    pub fn changes_resolution(&self) -> bool {
        self.skipped.is_none() && (self.resolve || self.unresolve)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlanError {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

/// The entries with each ID as the thread it means. A reply's comment ID
/// stands for the thread it is in. A number among the arguments that is the
/// PR's own and no thread's was the PR, written after the IDs (`resolve
/// 4435734278 1098`), so it is dropped (see [`Entry::among_args`]).
fn addressed(entries: &[Entry], pr: &PrThreads) -> Result<Vec<Entry>, PlanError> {
    let mut out: Vec<Entry> = Vec::new();
    for e in entries {
        let thread = pr
            .threads
            .iter()
            .find(|t| t.id == e.id || t.comments.iter().any(|c| c.id == e.id));
        let id = match thread {
            Some(t) => t.id,
            None if e.among_args && e.id == pr.number => continue,
            None => e.id,
        };
        if out.iter().any(|o| o.id == id) {
            return Err(PlanError {
                exit: Exit::Usage,
                message: format!("thread {id} is named twice; nothing was posted"),
                hint: Some("a reply's ID stands for the thread it is in".into()),
            });
        }
        out.push(Entry { id, ..e.clone() });
    }
    if out.is_empty() {
        return Err(PlanError {
            exit: Exit::Usage,
            message: format!(
                "{} is the PR, not a thread; name at least one thread ID",
                pr.number
            ),
            hint: Some(
                "a thread's ID is its first comment's ID, as `jouet pr threads --all` lists".into(),
            ),
        });
    }
    Ok(out)
}

/// Checks every entry against the PR and works out what is left to do.
pub fn plan(entries: &[Entry], pr: &PrThreads, o: &Options) -> Result<Vec<Outcome>, PlanError> {
    let find = |id: u64| pr.threads.iter().find(|t| t.id == id);
    let entries = &addressed(entries, pr)?;
    let unknown: Vec<String> = entries
        .iter()
        .filter(|e| find(e.id).is_none())
        .map(|e| e.id.to_string())
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
    let mut outcomes = Vec::new();
    for e in entries {
        let t = find(e.id).expect("unknown IDs were rejected");
        let mut out = Outcome {
            id: e.id,
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
            // A reviewer, often a bot after a push, resolved it already: a
            // reply still goes in, and the thread stays resolved.
            if out.post.is_some() && !o.ignore_resolved {
                out.already_resolved = true;
            } else {
                out.skipped = Some("already resolved".into());
                out.post = None;
            }
        } else if out.post.is_none() && !e.resolve {
            out.skipped = Some("reply already posted".into());
        } else {
            out.resolve = e.resolve;
        }
        outcomes.push(out);
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

/// Carries out the plan through the PR's host, stopping at the first
/// failure. Returns whether every write succeeded.
pub fn execute(host: &mut dyn Host, pr: &PrThreads, outcomes: &mut [Outcome]) -> bool {
    let mut ok = true;
    for o in outcomes {
        if o.skipped.is_some() {
            continue;
        }
        if !ok {
            o.failed = Some("not attempted after an earlier failure".into());
            continue;
        }
        if let Err(e) = apply(host, pr, o) {
            o.failed = Some(e);
            ok = false;
        }
    }
    ok
}

fn apply(host: &mut dyn Host, pr: &PrThreads, o: &mut Outcome) -> Result<(), String> {
    let thread = pr
        .threads
        .iter()
        .find(|t| t.id == o.id)
        .ok_or("the thread is gone")?;
    if let Some(body) = o.post.take() {
        o.url = Some(host.reply(pr, thread, &body)?);
        o.replied = true;
    }
    if o.resolve {
        host.set_resolved(pr, thread, true)?;
        o.resolved = true;
    }
    if o.unresolve {
        host.set_resolved(pr, thread, false)?;
        o.unresolved = true;
    }
    Ok(())
}
