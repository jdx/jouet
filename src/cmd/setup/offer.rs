//! The context budget a plain `jouet setup claude` applies from your recent
//! sessions, and the note that says why and how to take it back.

use crate::budget::{self, History, Kind, Unused};
use serde::Serialize;
use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};

/// What setup changes, for `--json` and the note.
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

    /// Why setup changes what it does, and how to install without it. The
    /// notes from [`super::budget_notes`] say what changed and how to undo
    /// each part.
    pub(super) fn note(&self, setup: &str, dry_run: bool) -> String {
        let mut parts = Vec::new();
        if let Some(window) = self.compact_window {
            parts.push(format!(
                "compact at {} tokens ({})",
                budget::tokens(window),
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
                "deny {} ({}; `--dry-run --json` lists each with its tokens)",
                self.unused_reason(),
                what.join("; ")
            ));
        }
        format!(
            "{} what each request carries, from your recent sessions: {}. \
             `{setup} --no-context-budget` installs without it",
            if dry_run {
                "setup would cut"
            } else {
                "setup cut"
            },
            parts.join("; and ")
        )
    }
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
