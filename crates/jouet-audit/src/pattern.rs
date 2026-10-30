//! Recurring habits worth a rule, found in single commands.

use crate::event::Call;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// A recurring habit that a jouet command or rule replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// `sleep N` before checking something, alone or in a loop: each check
    /// costs a model turn.
    SleepPoll,
    /// Test, build or lint output cut with `| head` or `| tail`: the failure
    /// is often in the part cut off, so the run is repeated.
    HeadOnTestOutput,
    /// `GH_TOKEN= ` or `env -u GITHUB_TOKEN` before a command, working around
    /// a stale token in the environment.
    StaleTokenPrefix,
    /// A script in a heredoc or `-c` that rewrites a file, or `cat > file
    /// <<EOF`: no check that the old text matched.
    HeredocEdit,
    /// Output that reported a GitHub rate limit.
    RateLimited,
}

impl Pattern {
    pub const ALL: [Self; 5] = [
        Self::SleepPoll,
        Self::HeadOnTestOutput,
        Self::StaleTokenPrefix,
        Self::HeredocEdit,
        Self::RateLimited,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::SleepPoll => "sleep_poll",
            Self::HeadOnTestOutput => "head_on_test_output",
            Self::StaleTokenPrefix => "stale_token_prefix",
            Self::HeredocEdit => "heredoc_edit",
            Self::RateLimited => "rate_limited",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SleepPoll => "sleep before a check",
            Self::HeadOnTestOutput => "test output cut with head/tail",
            Self::StaleTokenPrefix => "stale-token prefix",
            Self::HeredocEdit => "heredoc or script edit",
            Self::RateLimited => "GitHub rate limit hit",
        }
    }

    /// What replaces the habit.
    pub fn advice(self) -> &'static str {
        match self {
            Self::SleepPoll => {
                "`jouet wait` blocks until the condition holds, in one call (built-in rule `sleep-poll`)"
            }
            Self::HeadOnTestOutput => {
                "run tests once with the full log saved, and read the failures from it; mbx's agent display prints only failures"
            }
            Self::StaleTokenPrefix => {
                "fix the token in the environment once, or have the session start without it"
            }
            Self::HeredocEdit => {
                "`jouet edit` checks every match before writing anything (built-in rule `python-edit`)"
            }
            Self::RateLimited => {
                "`jouet pr status` reads many PRs in one query, and `jouet wait` shares one poll between sessions"
            }
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of patterns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Patterns(u8);

impl Patterns {
    pub fn contains(self, p: Pattern) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn insert(&mut self, p: Pattern) {
        self.0 |= p.bit();
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Pattern> {
        Pattern::ALL.into_iter().filter(move |p| self.contains(*p))
    }
}

static SLEEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s;&|(])sleep\s+(?:\d|<n>)").expect("the sleep pattern compiles")
});
static TEST_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?s)(?:\bcargo\s+(?:\+\S+\s+)?(?:test|nextest|build|check|clippy)|\bmise\s+(?:run|r)\s+(?:test|lint|build|ci|e2e|t|b)",
        r"|\b(?:npm|pnpm|yarn|bun)\s+(?:run\s+)?test|\bpytest\b|\bgo\s+test\b|\bbats\b|\bmake\s+(?:test|check)\b|\bhk\s+(?:check|run)\b)",
        r"[^\n;]*?\|\s*(?:head|tail)\b",
    ))
    .expect("the test-output pattern compiles")
});
static STALE_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:^|[\s;&|(])(?:GH_TOKEN|GITHUB_TOKEN|MISE_GITHUB_TOKEN|GH_ENTERPRISE_TOKEN)=(?:\s|$)",
        r"|\benv\s+(?:-\S+\s+)*-u\s*(?:GH_TOKEN|GITHUB_TOKEN)\b|\bunset\s+(?:GH_TOKEN|GITHUB_TOKEN)\b",
    ))
    .expect("the stale-token pattern compiles")
});
static SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:python[\d.]*|perl|ruby|node|deno|bun)\s+(?:-\S*\s+)*(?:-|-c|-e|-E)?\s*(?:<<|['\x22])")
        .expect("the script pattern compiles")
});
static WRITES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\.write_text\(|\.write\(|open\([^)]*['\x22][wa]\+?['\x22]|writeFileSync|File\.write|\bspurt\b",
        r"|\$\^I|-[a-z]*p[a-z]*i\b",
    ))
    .expect("the writes pattern compiles")
});
static CAT_HEREDOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:cat|tee)\s+(?:-a\s+)?>?>?\s*[^\s|;&<>]+\s*<<|\bcat\s+<<-?\s*\S+\s*>")
        .expect("the cat-heredoc pattern compiles")
});

/// The patterns a command shows. Works on shapes too, except
/// [`Pattern::HeredocEdit`], which needs the script's body.
pub fn detect(command: &str) -> Patterns {
    let mut found = Patterns::default();
    if SLEEP.is_match(command) {
        found.insert(Pattern::SleepPoll);
    }
    if TEST_HEAD.is_match(command) {
        found.insert(Pattern::HeadOnTestOutput);
    }
    if STALE_TOKEN.is_match(command) {
        found.insert(Pattern::StaleTokenPrefix);
    }
    if (SCRIPT.is_match(command) && WRITES.is_match(command)) || CAT_HEREDOC.is_match(command) {
        found.insert(Pattern::HeredocEdit);
    }
    found
}

/// The patterns a call shows, its output included.
pub(crate) fn detect_call(call: &Call) -> Patterns {
    let mut found = call.command.as_deref().map(detect).unwrap_or_default();
    if call.rate_limited {
        found.insert(Pattern::RateLimited);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(command: &str, p: Pattern) -> bool {
        detect(command).contains(p)
    }

    #[test]
    fn finds_sleep_polls() {
        assert!(has("sleep 30; gh pr checks 1", Pattern::SleepPoll));
        assert!(has("while true; do sleep 5; done", Pattern::SleepPoll));
        assert!(has("sleep <n> && tail -<n> <path>", Pattern::SleepPoll));
        assert!(!has("echo sleepy 5", Pattern::SleepPoll));
    }

    #[test]
    fn finds_cut_test_output() {
        assert!(has(
            "cargo test --workspace 2>&1 | tail -40",
            Pattern::HeadOnTestOutput
        ));
        assert!(has(
            "mise run test:e2e x.bats | head -n 100",
            Pattern::HeadOnTestOutput
        ));
        assert!(has(
            "cargo +1.91 check 2>&1 | head",
            Pattern::HeadOnTestOutput
        ));
        assert!(!has(
            "cargo test > log; head log",
            Pattern::HeadOnTestOutput
        ));
        assert!(!has("git log | head", Pattern::HeadOnTestOutput));
    }

    #[test]
    fn finds_stale_token_prefixes() {
        assert!(has(
            "GH_TOKEN= GITHUB_TOKEN= gh pr view 1",
            Pattern::StaleTokenPrefix
        ));
        assert!(has(
            "env -u GH_TOKEN gh api user",
            Pattern::StaleTokenPrefix
        ));
        assert!(has(
            "unset GITHUB_TOKEN; gh auth status",
            Pattern::StaleTokenPrefix
        ));
        assert!(!has(
            "GH_TOKEN=$(cat f) gh api user",
            Pattern::StaleTokenPrefix
        ));
    }

    #[test]
    fn finds_heredoc_edits() {
        let py = "python3 - <<'EOF'\nfrom pathlib import Path\np = Path('a')\np.write_text(p.read_text().replace('x', 'y'))\nEOF";
        assert!(has(py, Pattern::HeredocEdit));
        assert!(has("perl -pi -e 's/a/b/' f", Pattern::HeredocEdit));
        assert!(has(
            "cat > src/a.rs <<'EOF'\nfn main() {}\nEOF",
            Pattern::HeredocEdit
        ));
        assert!(has("cat <<EOF > notes.md\nx\nEOF", Pattern::HeredocEdit));
        assert!(!has(
            "python3 - <<'EOF'\nprint(1)\nEOF",
            Pattern::HeredocEdit
        ));
        assert!(!has("cat <<EOF\nhello\nEOF", Pattern::HeredocEdit));
    }
}
