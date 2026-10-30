//! `jouet wait`: poll a condition until it holds or `--timeout` expires, then
//! print one summary.
//!
//! Each subject only knows how to look once ([`Subject::check`]). The loop
//! here owns the timing: the first check is immediate, the interval backs off
//! by half again after each unchanged check up to [`MAX_INTERVAL`], and a
//! change of state resets it. A sleep never runs past the deadline, and one
//! last check happens at it, so `--timeout 0` checks exactly once.

pub mod checks;
pub mod ci;
mod joblog;
pub mod local;
mod os;
pub mod pr;
pub mod remote;
mod report;
pub mod run;
mod watch;

pub use report::Reporter;
/// How long a conflict GitHub reports must persist before a wait believes
/// it: right after a push, GitHub can still answer for the old head.
pub const CONFLICT_CONFIRM: Duration = watch::CONFLICT_CONFIRM;

use jouet_output::exit::Exit;
use serde::Serialize;
use std::time::{Duration, Instant};

/// Where the interval stops backing off, unless `--interval` is longer.
pub(crate) const MAX_INTERVAL: Duration = Duration::from_secs(60);

/// One look at a subject.
pub struct Observation {
    /// Whether the condition holds.
    pub(crate) met: bool,
    /// A short description of what was seen, such as `refused` or `exit 1`.
    /// A change in it is an event.
    pub(crate) state: String,
    pub(crate) detail: Detail,
}

impl Observation {
    pub(crate) fn pending(state: impl Into<String>) -> Self {
        Self {
            met: false,
            state: state.into(),
            detail: Detail::default(),
        }
    }
}

/// Something a GitHub wait saw happen, such as a check finishing.
#[derive(Debug, Clone)]
pub struct Event {
    /// The `event` name under `--jsonl`, such as `check_completed`.
    pub(crate) name: &'static str,
    /// The rest of the `--jsonl` line.
    pub(crate) fields: serde_json::Value,
    /// The progress line in text modes.
    pub(crate) text: String,
}

/// An error that ends the wait without an answer, such as a command that
/// cannot be started.
pub struct Stop {
    pub exit: Exit,
    pub message: String,
}

pub trait Subject {
    /// The subcommand: `file`, `log`, `port`, `http`, `pid` or `cmd`.
    fn name(&self) -> &'static str;
    /// What is being waited on, such as a path or an address.
    fn target(&self) -> String;
    /// Looks once. A check that can block (running a command, an HTTP
    /// request) gives up at `deadline`.
    fn check(&mut self, deadline: Instant) -> Result<Observation, Stop>;
}

/// What a subject found, beyond its state. Every field is optional, so each
/// subject fills in only its own.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Detail {
    /// The line that matched `--match` or `--contains`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<String>,
    /// The HTTP status code of the last response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<u16>,
    /// The last run's exit code; absent when a signal ended it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) exit_code: Option<i32>,
    /// The tail of the last run's output.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) output: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) output_truncated: bool,
    /// How many lines `output` leaves out, for the text marker.
    #[serde(skip)]
    pub(crate) output_omitted: usize,
    /// Where the last run's full output is saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) log_path: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// The result document, for `--json` and the last `--jsonl` line.
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub(crate) subject: &'static str,
    pub(crate) target: String,
    /// `met` or `timeout`.
    pub(crate) result: &'static str,
    /// The state at the last check.
    pub(crate) state: String,
    pub(crate) elapsed_s: u64,
    pub(crate) checks: u32,
    #[serde(flatten)]
    pub(crate) detail: Detail,
}

impl Outcome {
    pub(crate) fn exit(&self) -> Exit {
        if self.result == "met" {
            Exit::Ok
        } else {
            Exit::Timeout
        }
    }
}

pub struct Timing {
    pub timeout: Duration,
    pub interval: Duration,
}

impl Timing {
    /// The wait after `current`, half again as long, capped at
    /// [`MAX_INTERVAL`] or `--interval` when that is longer.
    fn back_off(&self, current: Duration) -> Duration {
        (current * 3 / 2).min(MAX_INTERVAL.max(self.interval))
    }
}

/// Checks `subject` until it is met or the timeout expires.
pub fn run(
    subject: &mut dyn Subject,
    timing: &Timing,
    reporter: &mut Reporter,
) -> Result<Outcome, Stop> {
    let start = Instant::now();
    let deadline = start + timing.timeout;
    let mut interval = timing.interval;
    let mut last_state: Option<String> = None;
    let mut checks = 0;
    loop {
        let obs = subject.check(deadline)?;
        checks += 1;
        if last_state.as_deref() != Some(&obs.state) {
            reporter.state(start.elapsed(), &obs.state);
            if last_state.is_some() {
                interval = timing.interval;
            }
            last_state = Some(obs.state.clone());
        }
        let now = Instant::now();
        if obs.met || now >= deadline {
            return Ok(Outcome {
                subject: subject.name(),
                target: subject.target(),
                result: if obs.met { "met" } else { "timeout" },
                state: obs.state,
                elapsed_s: start.elapsed().as_secs(),
                checks,
                detail: obs.detail,
            });
        }
        reporter.pause(interval.min(deadline - now), start, &obs.state);
        interval = timing.back_off(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_backs_off_to_a_minute() {
        let timing = Timing {
            timeout: Duration::from_secs(600),
            interval: Duration::from_secs(10),
        };
        let mut d = timing.interval;
        let mut seen = Vec::new();
        for _ in 0..6 {
            seen.push(d.as_secs());
            d = timing.back_off(d);
        }
        assert_eq!(seen, [10, 15, 22, 33, 50, 60]);
    }

    #[test]
    fn a_long_interval_is_not_shortened() {
        let timing = Timing {
            timeout: Duration::from_secs(600),
            interval: Duration::from_secs(90),
        };
        assert_eq!(timing.back_off(timing.interval), Duration::from_secs(90));
    }
}
