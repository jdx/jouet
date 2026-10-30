//! How a wait reports its progress and its result.
//!
//! - `--jsonl` prints one event per line on stdout, then the result document
//!   tagged `"event":"result"`.
//! - In human mode, with stderr on a terminal, one status line on stderr is
//!   redrawn every second.
//! - Otherwise each change of state is one line on stderr, so a backgrounded
//!   wait's log grows only when something happens.
//!
//! `--quiet` drops the progress and the text result; JSON is still printed,
//! because it was asked for.

use super::{Outcome, Stop};
use crate::display::Mode;
use crate::exit::Exit;
use crate::time::elapsed;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// The live line is a status display, not output, so it is cut to fit a
/// typical terminal rather than wrapping (which breaks the redraw).
const LIVE_WIDTH: usize = 100;

pub struct Reporter {
    mode: Mode,
    json: bool,
    jsonl: bool,
    quiet: bool,
    live: bool,
    /// `port 127.0.0.1:8080`, the prefix of progress lines.
    label: String,
    drawn: bool,
}

#[derive(Serialize)]
struct Tagged<'a, T> {
    event: &'static str,
    #[serde(flatten)]
    body: &'a T,
}

#[derive(Serialize)]
struct StateEvent<'a> {
    elapsed_s: u64,
    state: &'a str,
}

impl Reporter {
    pub fn new(mode: Mode, json: bool, jsonl: bool, quiet: bool, label: String) -> Self {
        let live = mode == Mode::Human && !quiet && !jsonl && std::io::stderr().is_terminal();
        Self {
            mode,
            json,
            jsonl,
            quiet,
            live,
            label,
            drawn: false,
        }
    }

    /// The state changed (or was seen for the first time).
    pub fn state(&mut self, since_start: Duration, state: &str) {
        if self.jsonl {
            let event = StateEvent {
                elapsed_s: since_start.as_secs(),
                state,
            };
            self.line(&Tagged {
                event: "state",
                body: &event,
            });
        } else if self.live {
            self.draw(since_start, state);
        } else if !self.quiet {
            eprintln!(
                "[{}] {}: {state}",
                elapsed(since_start.as_secs()),
                self.label
            );
        }
    }

    /// Sleeps between checks, redrawing the live line each second.
    pub fn pause(&mut self, length: Duration, start: Instant, state: &str) {
        if !self.live {
            std::thread::sleep(length);
            return;
        }
        let until = Instant::now() + length;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(Duration::from_secs(1)));
            self.draw(start.elapsed(), state);
        }
    }

    fn draw(&mut self, since_start: Duration, state: &str) {
        let mut line = format!(
            "waiting on {}: {state}  {}",
            self.label,
            elapsed(since_start.as_secs())
        );
        if line.chars().count() > LIVE_WIDTH {
            line = line.chars().take(LIVE_WIDTH - 3).collect::<String>() + "...";
        }
        eprint!("\r\x1b[2K{line}");
        let _ = std::io::stderr().flush();
        self.drawn = true;
    }

    fn clear(&mut self) {
        if self.drawn {
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
            self.drawn = false;
        }
    }

    fn line<T: Serialize>(&self, value: &T) {
        let mut stdout = std::io::stdout().lock();
        let text = serde_json::to_string(value).expect("jouet's output types always serialize");
        let _ = writeln!(stdout, "{text}");
        let _ = stdout.flush();
    }

    pub fn finish(&mut self, outcome: &Outcome) -> Exit {
        self.clear();
        if self.jsonl {
            self.line(&Tagged {
                event: "result",
                body: outcome,
            });
        } else if self.json {
            println!("{}", self.mode.json(outcome));
        } else if !self.quiet {
            print!("{}", text(outcome, self.mode));
        }
        outcome.exit()
    }

    pub fn fail(&mut self, stop: &Stop) -> Exit {
        self.clear();
        let doc = serde_json::json!({
            "error": { "code": stop.exit.name(), "message": stop.message }
        });
        if self.jsonl {
            self.line(&Tagged {
                event: "result",
                body: &doc,
            });
        } else if self.json {
            println!("{}", self.mode.json(&doc));
        } else {
            eprintln!("jouet: {}", stop.message);
        }
        stop.exit
    }
}

/// The text result: a first line that starts with `MET` or `TIMEOUT`, then
/// whatever the subject found, indented.
fn text(o: &Outcome, mode: Mode) -> String {
    let met = o.result == "met";
    let word = if met { "MET" } else { "TIMEOUT" };
    let word = if mode == Mode::Human && std::io::stdout().is_terminal() {
        let colour = if met { "32" } else { "33" };
        format!("\x1b[1;{colour}m{word}\x1b[0m")
    } else {
        word.to_string()
    };
    let took = elapsed(o.elapsed_s);
    let mut out = if met {
        format!("{word} {} {}: {} ({took})\n", o.subject, o.target, o.state)
    } else {
        let checks = if o.checks == 1 { "check" } else { "checks" };
        format!(
            "{word} {} {}: {} after {took} ({} {checks}); run it again to keep waiting\n",
            o.subject, o.target, o.state, o.checks
        )
    };
    let d = &o.detail;
    if let Some(line) = &d.line {
        let _ = writeln!(out, "  {line}");
    }
    if d.output_truncated {
        let path = d.log_path.as_deref().unwrap_or("not saved");
        let _ = writeln!(
            out,
            "  ... {} more lines (full log: {path})",
            d.output_omitted
        );
    }
    for line in &d.output {
        let _ = writeln!(out, "  {line}");
    }
    if let (Some(path), false) = (&d.log_path, d.output_truncated) {
        let _ = writeln!(out, "  full log: {path}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wait::Detail;

    fn outcome(result: &'static str, detail: Detail) -> Outcome {
        Outcome {
            subject: "cmd",
            target: "make check".into(),
            result,
            state: "exit 1".into(),
            elapsed_s: 372,
            checks: 9,
            detail,
        }
    }

    #[test]
    fn a_timeout_says_how_to_keep_waiting() {
        let text = text(&outcome("timeout", Detail::default()), Mode::Plain);
        assert_eq!(
            text,
            "TIMEOUT cmd make check: exit 1 after 6m12s (9 checks); run it again to keep waiting\n"
        );
    }

    #[test]
    fn a_cut_output_tail_says_what_was_left_out() {
        let detail = Detail {
            exit_code: Some(0),
            output: vec!["ok".into()],
            output_truncated: true,
            output_omitted: 214,
            log_path: Some("/logs/x.log".into()),
            ..Detail::default()
        };
        let text = text(&outcome("met", detail), Mode::Agent);
        assert_eq!(
            text,
            "MET cmd make check: exit 1 (6m12s)\n  ... 214 more lines (full log: /logs/x.log)\n  ok\n"
        );
    }

    #[test]
    fn json_leaves_out_empty_fields() {
        let doc = Mode::Agent.json(&outcome("met", Detail::default()));
        assert_eq!(
            doc,
            r#"{"subject":"cmd","target":"make check","result":"met","state":"exit 1","elapsed_s":372,"checks":9}"#
        );
    }
}
