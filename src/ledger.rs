//! An append-only record of what jouet's hooks saw and decided: one compact
//! JSON line per event under `$XDG_STATE_HOME/jouet/ledger/`.
//!
//! It is what rule quality will be measured from, and what agents will learn
//! new rules from, so it records the *shape* of each command, never its
//! values: the program, its subcommand words and its flag names, with paths,
//! values and message text replaced by placeholders, and anything that looks
//! like a token redacted. Writing it is best effort and never blocks a hook.

use jouet_rules::shell::{self, Op};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The current file is rotated once it reaches this size.
const ROTATE_AT: u64 = 4 * 1024 * 1024;
/// Rotated files are removed once they are this old.
const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How far back a retry counts as an override.
pub const OVERRIDE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How much of the current file the override check reads, from the end.
const TAIL: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub rule: String,
    /// `deny`, `warn`, `record:<kind>` or `block`.
    pub action: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub ts: u64,
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// `session-start`, `pre-tool-use`, `post-tool-use`, `stop` or `override`.
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
}

impl Entry {
    pub fn new(harness: &str, session: Option<&str>, event: &str) -> Self {
        Self {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            harness: harness.into(),
            session: session.map(redact_id),
            event: event.into(),
            ..Self::default()
        }
    }
}

/// `$XDG_STATE_HOME/jouet/ledger`, when there is a home directory.
pub fn dir() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("ledger"))
}

/// Appends an entry, rotating the file first if it has grown too large.
/// Best effort: an error loses the entry, nothing else.
pub fn append(dir: &Path, entry: &Entry) {
    let _ = try_append(dir, entry);
}

fn try_append(dir: &Path, entry: &Entry) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let current = dir.join("ledger.jsonl");
    if std::fs::metadata(&current).is_ok_and(|m| m.len() >= ROTATE_AT) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let _ = std::fs::rename(&current, dir.join(format!("ledger-{stamp}.jsonl")));
        prune(dir);
    }
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    // One write of a short line with O_APPEND: concurrent hooks interleave
    // whole lines.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(current)?
        .write_all(&line)
}

/// Removes rotated files older than [`KEEP`].
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let rotated = entry.file_name().to_string_lossy().starts_with("ledger-");
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > KEEP);
        if rotated && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The recent entries of one session, oldest first, from the end of the
/// current file.
pub fn recent(dir: &Path, session: &str) -> Vec<Entry> {
    let Ok(mut file) = std::fs::File::open(dir.join("ledger.jsonl")) else {
        return Vec::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(TAIL);
    let mut text = String::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    let session = redact_id(session);
    let needle = format!("\"session\":{}", serde_json::Value::from(session.as_str()));
    let mut lines = text.lines();
    if start > 0 {
        // Starts mid-line.
        lines.next();
    }
    lines
        .filter(|line| line.contains(&needle))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The rules whose denial or warning this command repeats: the session
/// denied or warned about a command of the same shape within
/// [`OVERRIDE_WINDOW`], and this one is going ahead anyway.
///
/// A heuristic. Same shape means the same program, subcommands and flags,
/// whatever the values, so `git add -A` retried as `git add -A` counts while
/// `git add src/` does not. Each earlier decision is counted once.
pub fn overrides(recent: &[Entry], shape: &str, now: u64) -> Vec<String> {
    let since = now.saturating_sub(OVERRIDE_WINDOW.as_secs());
    let mut rules = Vec::new();
    for entry in recent.iter().rev() {
        if entry.ts < since {
            break;
        }
        if entry.event == "override" && entry.shape.as_deref() == Some(shape) {
            // Already counted from here back.
            break;
        }
        if entry.event != "pre-tool-use" || entry.shape.as_deref() != Some(shape) {
            continue;
        }
        for d in &entry.decisions {
            if (d.action == "deny" || d.action == "warn") && !rules.contains(&d.rule) {
                rules.push(d.rule.clone());
            }
        }
    }
    rules
}

/// A command's shape: each top-level command's program, subcommand words and
/// flag names, joined by its operators, with everything else a placeholder.
pub fn shape(script: &str) -> String {
    let parsed = shell::parse(script);
    let mut out = String::new();
    for command in &parsed.commands {
        let argv = command.argv();
        let Some(program) = argv.first() else {
            continue;
        };
        out.push_str(&redact(shell::basename(program)));
        let mut state = ArgState::default();
        for arg in &argv[1..] {
            out.push(' ');
            out.push_str(&shape_arg(arg, &mut state));
        }
        out.push_str(match command.op {
            Some(Op::Seq) => "; ",
            Some(Op::And) => " && ",
            Some(Op::Or) => " || ",
            Some(Op::Pipe) => " | ",
            Some(Op::Background) => " &",
            None => "",
        });
    }
    out.trim_end_matches("; ").trim_end().to_string()
}

#[derive(Default)]
struct ArgState {
    /// Subcommand words kept so far.
    subcommands: usize,
    /// Positionals have started, so no more words are kept.
    done: bool,
    /// The previous argument was a flag, which may take this one as its
    /// value.
    after_flag: bool,
}

/// One argument's shape. Up to two words that read like subcommands (`stash
/// push`, `pr checks`) stay; a flag's value and every other positional is
/// `<arg>`.
fn shape_arg(arg: &str, state: &mut ArgState) -> String {
    let after_flag = std::mem::take(&mut state.after_flag);
    // Flags after a subcommand end the subcommand words; global options
    // before it (`git -C dir stash push`) do not.
    if arg.starts_with('-') && state.subcommands > 0 {
        state.done = true;
    }
    if let Some(long) = arg.strip_prefix("--") {
        return match long.split_once('=') {
            Some((name, _)) => format!("--{}=<v>", redact(name)),
            None => {
                state.after_flag = !long.is_empty();
                format!("--{}", redact(long))
            }
        };
    }
    if let Some(short) = arg.strip_prefix('-') {
        let flags =
            !short.is_empty() && short.len() <= 5 && short.chars().all(|c| c.is_ascii_alphabetic());
        state.after_flag = flags;
        return if flags {
            arg.to_string()
        } else {
            "<arg>".into()
        };
    }
    let word = arg.len() <= 24
        && arg.starts_with(|c: char| c.is_ascii_lowercase())
        && arg
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if after_flag {
        return "<arg>".into();
    }
    if word && !state.done && state.subcommands < 2 {
        state.subcommands += 1;
        return arg.to_string();
    }
    state.done = true;
    "<arg>".into()
}

/// Replaces anything that looks like a credential.
fn redact(word: &str) -> String {
    const PREFIXES: &[&str] = &[
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "github_pat_",
        "sk-",
        "xox",
        "AKIA",
        "glpat-",
    ];
    let long_secret = word.len() >= 24
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-+/=".contains(c))
        && word.chars().any(|c| c.is_ascii_digit());
    if PREFIXES.iter().any(|p| word.starts_with(p)) || long_secret {
        "<redacted>".into()
    } else {
        word.to_string()
    }
}

/// Session ids are opaque, but keep them short and printable.
fn redact_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_keeps_programs_subcommands_and_flags_only() {
        for (script, expected) in [
            ("git add -A", "git add -A"),
            (
                "git -C /repo stash push -m 'wip secret'",
                "git -C <arg> stash push -m <arg>",
            ),
            (
                "cd crates/x && cargo test --all-targets -- --nocapture",
                "cd <arg> && cargo test --all-targets -- --nocapture",
            ),
            (
                "gh pr checks 123 --repo jdx/jouet",
                "gh pr checks <arg> --repo <arg>",
            ),
            ("git commit -m fix", "git commit -m <arg>"),
            ("git push -f origin main", "git push -f <arg> <arg>"),
            (
                "curl -H 'Authorization: token ghp_abcdefghijklmnopqrstuvwxyz0123' https://x",
                "curl -H <arg> <arg>",
            ),
            ("GH_TOKEN=ghp_x gh api --token=sk-123", "gh api --token=<v>"),
            ("python3 - <<'EOF'\nprint('secret')\nEOF", "python3 <arg>"),
            ("cargo test 2>&1 | tail -5", "cargo test | tail <arg>"),
        ] {
            assert_eq!(shape(script), expected, "{script}");
        }
    }

    #[test]
    fn tokens_are_redacted_even_where_words_are_kept() {
        assert_eq!(redact("ghp_abc"), "<redacted>");
        assert_eq!(redact("AbCdEf0123456789AbCdEf0123"), "<redacted>");
        assert_eq!(redact("checkout"), "checkout");
    }

    #[test]
    fn entries_round_trip_and_rotate() {
        let dir = tempfile::tempdir().unwrap();
        let mut entry = Entry::new("claude", Some("s-1"), "pre-tool-use");
        entry.shape = Some("git add -A".into());
        entry.decisions = vec![Decision {
            rule: "git-add-all".into(),
            action: "deny".into(),
        }];
        append(dir.path(), &entry);
        append(dir.path(), &Entry::new("claude", Some("other"), "stop"));
        assert_eq!(recent(dir.path(), "s-1"), [entry.clone()]);

        std::fs::write(
            dir.path().join("ledger.jsonl"),
            vec![b'x'; usize::try_from(ROTATE_AT).unwrap()],
        )
        .unwrap();
        append(dir.path(), &entry);
        let files = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(files, 2);
        assert_eq!(recent(dir.path(), "s-1"), [entry]);
    }

    #[test]
    fn a_retry_of_the_same_shape_soon_after_is_an_override() {
        let denied = |ts: u64, shape: &str| Entry {
            ts,
            event: "pre-tool-use".into(),
            shape: Some(shape.into()),
            decisions: vec![Decision {
                rule: "git-stash".into(),
                action: "deny".into(),
            }],
            ..Entry::default()
        };
        let now = 10_000;
        let recent = [denied(now - 60, "git stash")];
        assert_eq!(overrides(&recent, "git stash", now), ["git-stash"]);
        assert!(overrides(&recent, "git stash list", now).is_empty());
        let stale = [denied(now - 3600, "git stash")];
        assert!(overrides(&stale, "git stash", now).is_empty());
        // Counted once: an override already logged ends the search.
        let counted = [
            denied(now - 60, "git stash"),
            Entry {
                ts: now - 30,
                event: "override".into(),
                shape: Some("git stash".into()),
                ..Entry::default()
            },
        ];
        assert!(overrides(&counted, "git stash", now).is_empty());
    }
}
