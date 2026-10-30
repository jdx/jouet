//! `wait pr --until ...`: wait for a PR to reach a state, through the same
//! batched query and attention codes as `pr status`.

use super::remote::{Halt, Poll, Watch};
use super::watch::short;
use super::{Event, Reporter};
use crate::exit::Exit;
use crate::github::Client;
use crate::pr::status::{self, Attention, Lookup, Mergeability, PrStatus, State};
use crate::pr::target::Target;
use crate::time::elapsed;
use serde::Serialize;
use serde_json::json;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Attention codes only the agent can clear, so `--until ready` stops on
/// them rather than waiting.
const BLOCKING: [&str; 4] = [
    "conflict",
    "checks_failing",
    "changes_requested",
    "unresolved_threads",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    Mergeable,
    Merged,
    Closed,
    Approved,
    ChecksRegistered,
    Ready,
    AnyChange,
}

impl Until {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "mergeable" => Self::Mergeable,
            "merged" => Self::Merged,
            "closed" => Self::Closed,
            "approved" => Self::Approved,
            "checks-registered" => Self::ChecksRegistered,
            "ready" => Self::Ready,
            "any-change" => Self::AnyChange,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Mergeable => "mergeable",
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Approved => "approved",
            Self::ChecksRegistered => "checks-registered",
            Self::Ready => "ready",
            Self::AnyChange => "any-change",
        }
    }
}

/// Where a PR stands against the condition.
#[derive(Debug, PartialEq, Eq)]
enum Judgement {
    Met(String),
    /// The condition can no longer be met.
    Failed(String),
    Pending(String),
}

fn judge(until: Until, pr: &PrStatus) -> Judgement {
    use Judgement::{Failed, Met, Pending};
    let open = matches!(pr.state, State::Open | State::Draft);
    match until {
        Until::Closed if open => Pending(pr.state.as_str().into()),
        Until::Closed => Met(pr.state.as_str().into()),
        _ if pr.state == State::Merged => Met("merged".into()),
        _ if pr.state == State::Closed => Failed("closed without merging".into()),
        Until::Merged => Pending(pr.state.as_str().into()),
        Until::Mergeable => match pr.mergeable {
            Mergeability::Mergeable => Met("mergeable".into()),
            Mergeability::Conflicting => Failed(format!("conflicts with {}", pr.base)),
            Mergeability::Unknown => Pending("mergeability unknown".into()),
        },
        Until::Approved => match pr.review.as_deref() {
            Some("approved") => Met("approved".into()),
            Some("changes_requested") => Failed("changes requested".into()),
            Some(review) => Pending(review.replace('_', " ")),
            None => Pending("no review decision".into()),
        },
        Until::ChecksRegistered => match pr.checks.total() {
            0 => Pending(format!("no checks on {}", short(&pr.head_sha))),
            1 => Met("1 check registered".into()),
            n => Met(format!("{n} checks registered")),
        },
        Until::Ready => {
            let codes =
                |list: &[&Attention]| list.iter().map(|a| a.code).collect::<Vec<_>>().join(", ");
            let blocking: Vec<&Attention> = pr
                .attention
                .iter()
                .filter(|a| BLOCKING.contains(&a.code))
                .collect();
            if !blocking.is_empty() {
                return Failed(codes(&blocking));
            }
            let waiting: Vec<&Attention> = pr.attention.iter().collect();
            if !waiting.is_empty() {
                Pending(codes(&waiting))
            } else if pr.checks.pending > 0 {
                Pending(format!("{} checks pending", pr.checks.pending))
            } else {
                Met("ready".into())
            }
        }
        Until::AnyChange => Pending("watching".into()),
    }
}

/// What `any-change` compares between polls.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    head_sha: String,
    state: State,
    mergeable: Mergeability,
    checks: (u32, u32, u32, u32),
    review: Option<String>,
    unresolved_threads: u32,
}

impl Key {
    fn of(pr: &PrStatus) -> Self {
        let c = &pr.checks;
        Self {
            head_sha: pr.head_sha.clone(),
            state: pr.state,
            mergeable: pr.mergeable,
            checks: (c.pass, c.fail, c.pending, c.skipped),
            review: pr.review.clone(),
            unresolved_threads: pr.unresolved_threads,
        }
    }

    /// The first difference, as a `change` description.
    fn change(&self, now: &Self, pr: &PrStatus) -> Option<Change> {
        let change = |what: &'static str, text: String| Some(Change { what, text });
        if self.head_sha != now.head_sha {
            return change(
                "push",
                format!(
                    "head moved from {} to {}",
                    short(&self.head_sha),
                    short(&now.head_sha)
                ),
            );
        }
        if self.state != now.state {
            return change("state", format!("now {}", now.state.as_str()));
        }
        if self.checks != now.checks {
            let c = &pr.checks;
            return change(
                "checks",
                format!(
                    "checks now {} pass, {} fail, {} skipped, {} pending",
                    c.pass, c.fail, c.skipped, c.pending
                ),
            );
        }
        if self.review != now.review {
            let review = now.review.as_deref().unwrap_or("none").replace('_', " ");
            return change("review", format!("review decision now {review}"));
        }
        if self.unresolved_threads != now.unresolved_threads {
            return change(
                "threads",
                format!("{} unresolved threads", now.unresolved_threads),
            );
        }
        if self.mergeable != now.mergeable {
            let m = match now.mergeable {
                Mergeability::Mergeable => "mergeable",
                Mergeability::Conflicting => "conflicting",
                Mergeability::Unknown => "unknown",
            };
            return change("mergeable", format!("mergeability now {m}"));
        }
        None
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub what: &'static str,
    pub text: String,
}

pub struct PrWait {
    target: Target,
    until: Until,
    start: Instant,
    baseline: Option<Key>,
    last: Option<PrStatus>,
}

#[derive(Debug, Serialize)]
pub struct Doc {
    pub repo: String,
    pub pr: u64,
    pub until: &'static str,
    /// `met`, `failed` or `timeout`.
    pub result: &'static str,
    pub state: String,
    pub elapsed_s: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<Attention>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}

impl PrWait {
    pub fn new(target: Target, until: Until) -> Self {
        Self {
            target,
            until,
            start: Instant::now(),
            baseline: None,
            last: None,
        }
    }

    fn doc(&self, result: &'static str, state: String, change: Option<Change>) -> Doc {
        let (repo, pr, attention) = match &self.last {
            Some(p) => (p.repo.clone(), p.number, p.attention.clone()),
            None => (self.target.to_string(), 0, Vec::new()),
        };
        Doc {
            repo,
            pr,
            until: self.until.as_str(),
            result,
            state,
            elapsed_s: self.start.elapsed().as_secs(),
            attention,
            change,
        }
    }
}

impl Watch for PrWait {
    type Doc = Doc;

    fn label(&self) -> String {
        format!("pr {} until {}", self.target, self.until.as_str())
    }

    fn poll(
        &mut self,
        client: &mut Client,
        _interval: Duration,
        events: &mut Vec<Event>,
    ) -> Result<Poll<Doc>, Halt> {
        let fetched =
            status::fetch(client, vec![Lookup::Pr(self.target.clone())]).map_err(|message| {
                Halt::Stop(super::Stop {
                    exit: Exit::Remote,
                    message,
                })
            })?;
        let Some(pr) = fetched.prs.into_iter().next() else {
            return Err(Halt::not_found(format!("no PR for {}", self.target)));
        };
        let pr = pr.finish(crate::time::now());
        // Once the PR is known by number, a branch that gets a newer PR
        // does not switch the wait to it.
        if let Some(target) =
            crate::pr::target::Repo::parse(&pr.repo).map(|repo| Target::Number(repo, pr.number))
        {
            self.target = target;
        }
        let key = Key::of(&pr);
        self.last = Some(pr.clone());
        if self.until == Until::AnyChange {
            let Some(baseline) = &self.baseline else {
                self.baseline = Some(key);
                return Ok(Poll::Pending("watching".into()));
            };
            return Ok(match baseline.change(&key, &pr) {
                Some(change) => {
                    events.push(Event {
                        name: "change",
                        fields: json!({ "what": change.what, "text": change.text }),
                        text: String::new(),
                    });
                    Poll::Done(self.doc("met", change.text.clone(), Some(change)))
                }
                None => Poll::Pending("watching".into()),
            });
        }
        Ok(match judge(self.until, &pr) {
            Judgement::Met(state) => Poll::Done(self.doc("met", state, None)),
            Judgement::Failed(state) => Poll::Done(self.doc("failed", state, None)),
            Judgement::Pending(state) => Poll::Pending(state),
        })
    }

    fn timed_out(&mut self, _client: &mut Client) -> Doc {
        let state = match &self.last {
            Some(pr) if self.until != Until::AnyChange => match judge(self.until, pr) {
                Judgement::Met(s) | Judgement::Failed(s) | Judgement::Pending(s) => s,
            },
            _ => "no change".into(),
        };
        self.doc("timeout", state, None)
    }

    fn text(&self, doc: &Doc, reporter: &Reporter) -> String {
        let (word, good) = match doc.result {
            "met" => ("MET", true),
            "failed" => ("FAILED", false),
            _ => ("TIMEOUT", false),
        };
        let mut out = format!(
            "{} {}#{} {}: {} ({})\n",
            reporter.word(word, good),
            doc.repo,
            doc.pr,
            doc.until,
            doc.state,
            elapsed(doc.elapsed_s)
        );
        if doc.until == "ready" || doc.result == "failed" {
            for a in &doc.attention {
                let _ = writeln!(out, "  {}: {}", a.code, a.message);
            }
        }
        if doc.result == "timeout" {
            out.push_str("  run it again to keep waiting\n");
        }
        out
    }

    fn exit(&self, doc: &Doc) -> Exit {
        match doc.result {
            "met" => Exit::Ok,
            "timeout" => Exit::Timeout,
            _ => Exit::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr::status::Checks;

    fn pr() -> PrStatus {
        PrStatus {
            repo: "jdx/mise".into(),
            number: 1,
            title: "t".into(),
            url: "u".into(),
            state: State::Open,
            base: "main".into(),
            head: "feat".into(),
            head_sha: "3f2a1bc9e0".into(),
            pushed_at: None,
            mergeable: Mergeability::Unknown,
            merge_state: "unknown".into(),
            checks: Checks::default(),
            review: None,
            unresolved_threads: 0,
            auto_merge: false,
            attention: Vec::new(),
        }
    }

    #[test]
    fn unknown_mergeability_keeps_waiting() {
        let mut p = pr();
        assert!(matches!(judge(Until::Mergeable, &p), Judgement::Pending(_)));
        p.mergeable = Mergeability::Conflicting;
        assert_eq!(
            judge(Until::Mergeable, &p),
            Judgement::Failed("conflicts with main".into())
        );
        p.state = State::Merged;
        assert_eq!(judge(Until::Mergeable, &p), Judgement::Met("merged".into()));
    }

    #[test]
    fn ready_stops_on_codes_only_the_agent_can_clear() {
        let mut p = pr();
        let code = |code| Attention {
            code,
            message: String::new(),
        };
        p.attention = vec![code("mergeable_unknown")];
        assert!(matches!(judge(Until::Ready, &p), Judgement::Pending(_)));
        p.attention = vec![code("draft"), code("checks_failing")];
        assert_eq!(
            judge(Until::Ready, &p),
            Judgement::Failed("checks_failing".into())
        );
        p.attention.clear();
        p.checks.pending = 2;
        assert_eq!(
            judge(Until::Ready, &p),
            Judgement::Pending("2 checks pending".into())
        );
        p.checks.pending = 0;
        assert_eq!(judge(Until::Ready, &p), Judgement::Met("ready".into()));
    }

    #[test]
    fn merged_ends_merged_and_closed_waits() {
        let mut p = pr();
        p.state = State::Closed;
        assert!(matches!(judge(Until::Merged, &p), Judgement::Failed(_)));
        assert_eq!(judge(Until::Closed, &p), Judgement::Met("closed".into()));
    }

    #[test]
    fn any_change_names_the_first_difference() {
        let mut p = pr();
        let before = Key::of(&p);
        p.checks.pass = 1;
        let change = before.change(&Key::of(&p), &p).unwrap();
        assert_eq!(change.what, "checks");
        p.head_sha = "abcdef12".into();
        assert_eq!(before.change(&Key::of(&p), &p).unwrap().what, "push");
    }
}
