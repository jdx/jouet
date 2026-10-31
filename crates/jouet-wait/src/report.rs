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

use super::{Event, Outcome, Stop};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use jouet_output::time::elapsed;
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
    pub(crate) fn state(&mut self, since_start: Duration, state: &str) {
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

    /// Something happened, such as a check finishing: an `event` line under
    /// `--jsonl`, else a progress line, unless the event has no text (it
    /// shows in the state line's counts instead).
    pub(crate) fn event(&mut self, since_start: Duration, event: &Event) {
        if self.jsonl {
            self.line(&Tagged {
                event: event.name,
                body: &event.fields,
            });
        } else if !self.quiet && !event.text.is_empty() {
            // Above the live line, which is redrawn at the next tick.
            self.clear();
            eprintln!(
                "[{}] {}: {}",
                elapsed(since_start.as_secs()),
                self.label,
                event.text
            );
        }
    }

    /// Sleeps between checks, redrawing the live line each second.
    pub(crate) fn pause(&mut self, length: Duration, start: Instant, state: &str) {
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
        let text = text(outcome, self.mode);
        self.finish_doc(outcome, &text);
        outcome.exit()
    }

    /// Prints a result document, or its text form.
    pub(crate) fn finish_doc<T: Serialize>(&mut self, doc: &T, text: &str) {
        self.clear();
        if self.jsonl {
            self.line(&Tagged {
                event: "result",
                body: doc,
            });
        } else if self.json {
            println!("{}", self.mode.json(doc));
        } else if !self.quiet {
            print!("{text}");
        }
    }

    /// The first word of a text result, in colour for a person at a
    /// terminal: green when `good`, else yellow.
    pub(crate) fn word(&self, word: &str, good: bool) -> String {
        headline(word, good, self.mode)
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

fn headline(word: &str, good: bool, mode: Mode) -> String {
    if mode == Mode::Human && std::io::stdout().is_terminal() {
        let colour = if good { "32" } else { "33" };
        format!("\x1b[1;{colour}m{word}\x1b[0m")
    } else {
        word.to_string()
    }
}

/// The text result: a first line that starts with `MET`, `FAILED` or
/// `TIMEOUT`, then whatever the subject found, indented.
fn text(o: &Outcome, mode: Mode) -> String {
    let met = o.result == "met";
    let took = elapsed(o.elapsed_s);
    let mut out = if met {
        let word = headline("MET", true, mode);
        format!("{word} {} {}: {} ({took})\n", o.subject, o.target, o.state)
    } else if o.result == "failed" {
        let word = headline("FAILED", false, mode);
        format!(
            "{word} {} {}: {} after {took}\n",
            o.subject, o.target, o.state
        )
    } else {
        let word = headline("TIMEOUT", false, mode);
        let checks = if o.checks == 1 { "check" } else { "checks" };
        format!(
            "{word} {} {}: {} after {took} ({} {checks}); run it again to keep waiting\n",
            o.subject, o.target, o.state, o.checks
        )
    };
    out.push_str(&details(&o.detail));
    out
}

/// What a subject found, indented, under a result's first line.
pub(crate) fn details(d: &crate::Detail) -> String {
    let mut out = String::new();
    if let Some(line) = &d.line {
        let _ = writeln!(out, "  {line}");
    }
    // A tail cut from the top says how much is above it; an excerpt marks
    // its own gaps.
    if d.output_omitted > 0 {
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
    if let (Some(path), 0) = (&d.log_path, d.output_omitted) {
        let _ = writeln!(out, "  full log: {path}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Detail;

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
    fn a_failure_shows_its_excerpt_and_the_full_log() {
        let detail = Detail {
            output: vec!["... 40 lines skipped".into(), "Error: boom".into()],
            output_truncated: true,
            log_path: Some("/logs/d.log".into()),
            ..Detail::default()
        };
        let o = Outcome {
            subject: "daemon",
            target: "api".into(),
            state: "errored (exit code 1)".into(),
            ..outcome("failed", detail)
        };
        assert_eq!(o.exit(), Exit::Failed);
        assert_eq!(
            text(&o, Mode::Agent),
            "FAILED daemon api: errored (exit code 1) after 6m12s\n  ... 40 lines skipped\n  Error: boom\n  full log: /logs/d.log\n"
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
