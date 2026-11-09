//! The context budget setup offers for Claude Code: asked at a terminal,
//! noted everywhere else, and written only on a yes or an explicit flag.

use crate::budget::{self, Change, History, Kind, Unused};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};

/// What setup could change, for `--json` and the note.
#[derive(Debug, Serialize)]
pub(super) struct Offer {
    /// Recent sessions read.
    pub(super) sessions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) compact_window: Option<u64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) past_window: usize,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub(super) peak_context: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) unused: Vec<Unused>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

impl Offer {
    pub(super) fn new(history: &History, window: Option<u64>, unused: Vec<Unused>) -> Self {
        Self {
            sessions: history.sessions,
            compact_window: window,
            past_window: history.past_window,
            peak_context: history.peak,
            unused,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.compact_window.is_none() && self.unused.is_empty()
    }

    fn unused_tokens(&self) -> u64 {
        self.unused.iter().map(|u| u.tokens).sum()
    }

    fn window_reason(&self) -> String {
        format!(
            "{} of your last {} sessions went past {} tokens of context, up to {}",
            self.past_window,
            self.sessions,
            budget::tokens(budget::WINDOW),
            budget::tokens(self.peak_context)
        )
    }

    fn unused_reason(&self) -> String {
        format!(
            "{} definitions none of your last {} sessions used, ~{} tokens on every request",
            self.unused.len(),
            self.sessions,
            budget::tokens(self.unused_tokens())
        )
    }

    /// The note for a run that cannot ask: what each flag would do.
    pub(super) fn note(&self, setup: &str) -> String {
        let mut parts = Vec::new();
        if self.compact_window.is_some() {
            parts.push(format!(
                "`{setup} --compact-window 200k` ({})",
                self.window_reason()
            ));
        }
        if !self.unused.is_empty() {
            let named = |kind: Kind| {
                self.unused
                    .iter()
                    .filter(|u| u.kind == kind)
                    .map(|u| u.name.as_str())
                    .collect::<Vec<_>>()
            };
            let mut what = Vec::new();
            for (kind, label) in [(Kind::Tool, "tools"), (Kind::Mcp, "MCP servers")] {
                let names = named(kind);
                if !names.is_empty() {
                    what.push(format!("{label} {}", names.join(", ")));
                }
            }
            let skills = named(Kind::Skill).len();
            if skills > 0 {
                what.push(format!("{skills} skills"));
            }
            parts.push(format!(
                "`{setup} --deny-unused` ({}: {}; `--dry-run --json` lists each with its tokens)",
                self.unused_reason(),
                what.join("; ")
            ));
        }
        format!(
            "setup can also cut what each request carries, changing nothing unless asked: {}",
            parts.join("; and ")
        )
    }

    /// Asks the person at the terminal about each choice, and returns the
    /// ones accepted.
    pub(super) fn ask(self, setup: &str) -> Change {
        let mut change = Change::default();
        if let Some(window) = self.compact_window {
            eprintln!("{}.", capitalized(&self.window_reason()));
            if confirm(&format!(
                "Compact conversations at {} tokens of context (autoCompactWindow: {window})? \
                 `{setup} --undo window` takes it back. [y/N] ",
                budget::tokens(window)
            )) {
                change.window = Some(window);
            }
        }
        if !self.unused.is_empty() {
            let mut list = String::new();
            for item in &self.unused {
                let _ = writeln!(list, "  {}", item.line());
            }
            eprint!(
                "These are {} (tokens per request, estimated):\n{list}",
                self.unused_reason()
            );
            if confirm(&format!(
                "Deny them? Skills stay available as /name. `{setup} --undo unused` takes them back. [y/N] "
            )) {
                change.deny = self.unused;
            }
        }
        change
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}

/// Whether stdin has ended: every later question is then a no, rather than
/// a read that may never return.
static ENDED: AtomicBool = AtomicBool::new(false);

/// Asks a yes-or-no question on stderr; anything but a yes is a no.
pub(super) fn confirm(question: &str) -> bool {
    eprint!("{question}");
    let _ = io::stderr().flush();
    if ENDED.load(Ordering::Relaxed) {
        eprintln!();
        return false;
    }
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            ENDED.store(true, Ordering::Relaxed);
            eprintln!();
            false
        }
        Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
    }
}
