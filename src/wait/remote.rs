//! The loop for the subjects that poll GitHub: `wait ci`, `wait run` and
//! `wait pr`.
//!
//! It times checks like the local loop, and adds what talking to GitHub
//! needs: events between states, a document of the subject's own shape as
//! the result, and the primary rate limit. A wait that hits the rate limit
//! sleeps until it resets when that is before `--timeout`, and otherwise
//! exits 5.

use super::{Event, Reporter, Stop, Timing};
use crate::exit::Exit;
use crate::github::{Client, Failure};
use crate::time::{age, now};
use serde::Serialize;
use serde_json::json;
use std::time::{Duration, Instant};

pub enum Poll<D> {
    Pending(String),
    Done(D),
}

/// Why a poll did not answer.
pub enum Halt {
    Stop(Stop),
    /// The primary rate limit is used up until `reset` (Unix seconds).
    RateLimited {
        reset: i64,
    },
}

impl From<Failure> for Halt {
    fn from(f: Failure) -> Self {
        match f {
            Failure::RateLimited { reset } => Self::RateLimited { reset },
            Failure::Other(message) => Self::Stop(Stop {
                exit: Exit::Remote,
                message,
            }),
        }
    }
}

impl Halt {
    pub fn not_found(message: String) -> Self {
        Self::Stop(Stop {
            exit: Exit::NotFound,
            message,
        })
    }
}

pub trait Watch {
    type Doc: Serialize;
    /// The prefix of progress lines, such as `ci jdx/mise#15002`.
    fn label(&self) -> String;
    /// Looks once, adding anything that happened to `events`.
    fn poll(
        &mut self,
        client: &mut Client,
        interval: Duration,
        events: &mut Vec<Event>,
    ) -> Result<Poll<Self::Doc>, Halt>;
    /// The result when `--timeout` expires.
    fn timed_out(&mut self, client: &mut Client) -> Self::Doc;
    fn text(&self, doc: &Self::Doc, reporter: &Reporter) -> String;
    fn exit(&self, doc: &Self::Doc) -> Exit;
}

pub fn run<W: Watch>(
    w: &mut W,
    client: &mut Client,
    timing: &Timing,
    reporter: &mut Reporter,
) -> Exit {
    let start = Instant::now();
    let deadline = start + timing.timeout;
    let mut interval = timing.interval;
    let mut last_state: Option<String> = None;
    loop {
        let mut events = Vec::new();
        let polled = w.poll(client, interval, &mut events);
        for event in &events {
            reporter.event(start.elapsed(), event);
        }
        if !events.is_empty() && last_state.is_some() {
            interval = timing.interval;
        }
        let doc = match polled {
            Ok(Poll::Done(doc)) => Some(doc),
            Ok(Poll::Pending(state)) => {
                if last_state.as_deref() != Some(&state) {
                    reporter.state(start.elapsed(), &state);
                    last_state = Some(state);
                }
                None
            }
            Err(Halt::Stop(stop)) => return reporter.fail(&stop),
            Err(Halt::RateLimited { reset }) => {
                // A second past the reset, so the next request is allowed.
                let wait = Duration::from_secs(u64::try_from(reset - now() + 1).unwrap_or(1));
                if Instant::now() + wait > deadline {
                    return reporter.fail(&Stop {
                        exit: Exit::Remote,
                        message: format!(
                            "GitHub rate limit exhausted; it resets in {}, after --timeout",
                            age(reset - now())
                        ),
                    });
                }
                let event = Event {
                    name: "rate_limited",
                    fields: json!({ "reset_in_s": wait.as_secs() }),
                    text: format!(
                        "rate limited; sleeping {} until it resets",
                        age(reset - now())
                    ),
                };
                reporter.event(start.elapsed(), &event);
                std::thread::sleep(wait);
                continue;
            }
        };
        if let Some(doc) = doc {
            let text = w.text(&doc, reporter);
            reporter.finish_doc(&doc, &text);
            return w.exit(&doc);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let doc = w.timed_out(client);
            let text = w.text(&doc, reporter);
            reporter.finish_doc(&doc, &text);
            return w.exit(&doc);
        }
        let state = last_state.clone().unwrap_or_default();
        reporter.pause(interval.min(left), start, &state);
        interval = timing.back_off(interval);
    }
}
