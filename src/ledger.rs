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
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The current file is rotated once it reaches this size.
const ROTATE_AT: u64 = 4 * 1024 * 1024;
/// Rotated files are removed once they are this old.
const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How far back a retry counts as an override.
pub(crate) const OVERRIDE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How much of the current file the override check reads, from the end.
const TAIL: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Decision {
    pub rule: String,
    /// `deny`, `warn`, `record:<kind>` or `block`.
    pub action: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub ts: u64,
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The work tree the session runs in, as [`repo_key`]: a hash, never the
    /// path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// `session-start`, `pre-tool-use`, `post-tool-use`, `stop` or `override`.
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// `rtk` when rtk wrapped the command, whose shape is then the wrapped
    /// command's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
}

impl Entry {
    pub(crate) fn new(harness: &str, session: Option<&str>, event: &str) -> Self {
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

/// The work tree containing `cwd`, as the first 16 hex characters of the
/// SHA-256 of its root: the nearest directory with a `.git`, with symlinks
/// resolved, which is what `git rev-parse --show-toplevel` prints. Read from
/// the filesystem, because a hook cannot afford to start git.
pub(crate) fn repo_key(cwd: &Path) -> Option<String> {
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let root = root.canonicalize().ok()?;
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// `$XDG_STATE_HOME/jouet/ledger`, when there is a home directory.
pub(crate) fn dir() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("ledger"))
}

/// Appends an entry, rotating the file first if it has grown too large.
/// Best effort: an error loses the entry, nothing else.
pub(crate) fn append(dir: &Path, entry: &Entry) {
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
pub(crate) fn recent(dir: &Path, session: &str) -> Vec<Entry> {
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
pub(crate) fn overrides(recent: &[Entry], shape: &str, now: u64) -> Vec<String> {
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

/// `Some("rtk")` when rtk wraps any of the script's top-level commands.
pub(crate) fn via(script: &str) -> Option<String> {
    shell::parse(script)
        .commands
        .iter()
        .find_map(shell::Command::via)
        .map(str::to_string)
}

/// A command's shape: each top-level command's program, subcommand words and
/// flag names, joined by its operators, with everything else a placeholder.
pub(crate) fn shape(script: &str) -> String {
    let parsed = shell::parse(script);
    let mut out = String::new();
    for command in &parsed.commands {
        let argv = command.argv();
        let Some(program) = argv.first() else {
            continue;
        };
        let program = shell::basename(program);
        out.push_str(&redact(program));
        let mut state = ArgState {
            program,
            ..ArgState::default()
        };
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
struct ArgState<'a> {
    program: &'a str,
    /// The first subcommand kept, which decides the second.
    first: Option<&'static str>,
    /// Subcommand words kept so far.
    subcommands: usize,
    /// Positionals have started, so no more words are kept.
    done: bool,
    /// The previous argument was a flag, which may take this one as its
    /// value.
    after_flag: bool,
}

/// One argument's shape. Up to two known subcommands of the program (`stash
/// push`, `pr checks`) stay; a flag's value and every other positional,
/// including a task, target or script name, is `<arg>`.
fn shape_arg(arg: &str, state: &mut ArgState<'_>) -> String {
    let after_flag = std::mem::take(&mut state.after_flag);
    // Flags after a subcommand end the subcommand words; global options
    // before it (`git -C dir stash push`) do not.
    if arg.starts_with('-') && state.subcommands > 0 {
        state.done = true;
    }
    if let Some(long) = arg.strip_prefix("--") {
        let (name, value) = match long.split_once('=') {
            Some((name, _)) => (name, true),
            None => (long, false),
        };
        // Only the form of a real flag name (`--all-targets`) is kept; a
        // value that happens to start with `--` is a value.
        let flag_name = name.len() <= 32
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if long.is_empty() {
            return "--".into();
        }
        if !flag_name {
            return "<arg>".into();
        }
        state.after_flag = !value;
        return if value {
            format!("--{}=<v>", redact(name))
        } else {
            format!("--{}", redact(name))
        };
    }
    if let Some(short) = arg.strip_prefix('-') {
        // A short cluster such as `-fu`; longer, it may be `-mmessage`.
        let flags =
            !short.is_empty() && short.len() <= 3 && short.chars().all(|c| c.is_ascii_alphabetic());
        state.after_flag = flags;
        return if flags {
            arg.to_string()
        } else {
            "<arg>".into()
        };
    }
    if after_flag {
        return "<arg>".into();
    }
    let known = match (state.done, state.first) {
        (true, _) => None,
        (false, None) => subcommand(FIRST, state.program, arg),
        (false, Some(first)) => SECOND
            .iter()
            .find(|(p, f, _)| programs(p).contains(&state.program) && *f == first)
            .and_then(|(_, _, words)| words.iter().copied().find(|w| *w == arg)),
    };
    match known {
        Some(word) => {
            state.subcommands += 1;
            state.first = state.first.or(Some(word));
            if state.subcommands == 2 {
                state.done = true;
            }
            redact(word)
        }
        None => {
            state.done = true;
            "<arg>".into()
        }
    }
}

/// The subcommand `arg` names, if it is one of the program's known ones.
fn subcommand(table: &[(&str, &[&'static str])], program: &str, arg: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(p, _)| programs(p).contains(&program))
        .and_then(|(_, words)| words.iter().copied().find(|w| *w == arg))
}

/// A table key: one program, or several separated by `|`.
fn programs(key: &str) -> Vec<&str> {
    key.split('|').collect()
}

/// Subcommands a shape keeps, per program. Any other positional, and every
/// positional of a program not listed, is a placeholder: task, target and
/// script names are the user's own words.
const FIRST: &[(&str, &[&str])] = &[
    (
        "git",
        &[
            "add",
            "am",
            "apply",
            "bisect",
            "blame",
            "branch",
            "checkout",
            "cherry-pick",
            "clean",
            "clone",
            "commit",
            "config",
            "describe",
            "diff",
            "fetch",
            "format-patch",
            "grep",
            "init",
            "log",
            "ls-files",
            "merge",
            "mv",
            "notes",
            "pull",
            "push",
            "rebase",
            "reflog",
            "remote",
            "reset",
            "restore",
            "rev-parse",
            "revert",
            "rm",
            "show",
            "stash",
            "status",
            "submodule",
            "switch",
            "tag",
            "worktree",
        ],
    ),
    (
        "gh",
        &[
            "api", "auth", "browse", "cache", "gist", "issue", "label", "pr", "release", "repo",
            "run", "search", "secret", "status", "variable", "workflow",
        ],
    ),
    (
        "cargo|mbx|cross",
        &[
            "add",
            "audit",
            "b",
            "bench",
            "build",
            "c",
            "check",
            "clean",
            "clippy",
            "deny",
            "doc",
            "fetch",
            "fix",
            "fmt",
            "insta",
            "install",
            "metadata",
            "nextest",
            "package",
            "publish",
            "r",
            "remove",
            "run",
            "t",
            "test",
            "tree",
            "uninstall",
            "update",
        ],
    ),
    (
        "mise",
        &[
            "activate",
            "cache",
            "config",
            "doctor",
            "env",
            "exec",
            "i",
            "install",
            "latest",
            "lock",
            "ls",
            "ls-remote",
            "outdated",
            "plugins",
            "prune",
            "r",
            "registry",
            "run",
            "self-update",
            "set",
            "settings",
            "tasks",
            "trust",
            "uninstall",
            "unset",
            "upgrade",
            "use",
            "where",
            "which",
            "x",
        ],
    ),
    (
        "npm|pnpm|yarn|bun|aube",
        &[
            "add", "audit", "ci", "create", "dlx", "exec", "i", "init", "install", "link", "list",
            "ls", "outdated", "pack", "publish", "remove", "rm", "run", "t", "test", "update", "x",
        ],
    ),
    (
        "go",
        &[
            "build", "clean", "doc", "env", "fmt", "generate", "get", "install", "list", "mod",
            "run", "test", "tool", "vet", "work",
        ],
    ),
    (
        "docker|podman",
        &[
            "build", "buildx", "compose", "cp", "exec", "images", "inspect", "login", "logs",
            "network", "ps", "pull", "push", "restart", "rm", "rmi", "run", "start", "stop",
            "system", "tag", "volume",
        ],
    ),
    (
        "kubectl",
        &[
            "apply",
            "config",
            "create",
            "delete",
            "describe",
            "diff",
            "edit",
            "exec",
            "explain",
            "get",
            "logs",
            "port-forward",
            "rollout",
            "scale",
            "top",
        ],
    ),
    (
        "jouet",
        &[
            "agent-help",
            "audit",
            "edit",
            "hook",
            "pr",
            "rule",
            "setup",
            "skill",
            "stats",
            "wait",
        ],
    ),
    (
        "hk",
        &[
            "check", "config", "fix", "init", "install", "run", "validate",
        ],
    ),
    (
        "rustup",
        &[
            "component",
            "default",
            "install",
            "override",
            "run",
            "show",
            "target",
            "toolchain",
            "update",
            "which",
        ],
    ),
    (
        "uv",
        &[
            "add", "build", "init", "lock", "pip", "publish", "python", "remove", "run", "sync",
            "tool", "venv",
        ],
    ),
    (
        "pip|pip3",
        &["download", "freeze", "install", "list", "show", "uninstall"],
    ),
    (
        "brew",
        &[
            "cleanup",
            "doctor",
            "info",
            "install",
            "list",
            "outdated",
            "reinstall",
            "search",
            "services",
            "tap",
            "uninstall",
            "update",
            "upgrade",
        ],
    ),
];

/// Second-level subcommands, per program and first subcommand.
const SECOND: &[(&str, &str, &[&str])] = &[
    (
        "git",
        "stash",
        &[
            "apply", "branch", "clear", "create", "drop", "list", "pop", "push", "save", "show",
            "store",
        ],
    ),
    (
        "git",
        "worktree",
        &["add", "list", "lock", "move", "prune", "remove", "unlock"],
    ),
    (
        "git",
        "remote",
        &[
            "add", "get-url", "prune", "remove", "rename", "set-url", "show",
        ],
    ),
    (
        "git",
        "submodule",
        &[
            "add", "deinit", "foreach", "init", "status", "sync", "update",
        ],
    ),
    (
        "git",
        "bisect",
        &["bad", "good", "reset", "run", "skip", "start"],
    ),
    (
        "gh",
        "pr",
        &[
            "checkout", "checks", "close", "comment", "create", "diff", "edit", "list", "merge",
            "ready", "reopen", "review", "status", "view",
        ],
    ),
    (
        "gh",
        "issue",
        &[
            "close", "comment", "create", "edit", "list", "reopen", "view",
        ],
    ),
    (
        "gh",
        "run",
        &["cancel", "download", "list", "rerun", "view", "watch"],
    ),
    (
        "gh",
        "workflow",
        &["disable", "enable", "list", "run", "view"],
    ),
    ("gh", "api", &["graphql"]),
    (
        "gh",
        "repo",
        &["clone", "create", "fork", "list", "sync", "view"],
    ),
    (
        "gh",
        "release",
        &[
            "create", "delete", "download", "edit", "list", "upload", "view",
        ],
    ),
    (
        "gh",
        "auth",
        &["login", "logout", "refresh", "setup-git", "status", "token"],
    ),
    ("cargo|mbx|cross", "nextest", &["list", "run"]),
    ("cargo|mbx|cross", "insta", &["accept", "review", "test"]),
    (
        "go",
        "mod",
        &[
            "download", "edit", "graph", "init", "tidy", "vendor", "verify", "why",
        ],
    ),
    (
        "docker|podman",
        "compose",
        &[
            "build", "down", "exec", "logs", "ps", "pull", "restart", "run", "stop", "up",
        ],
    ),
    (
        "kubectl",
        "rollout",
        &["history", "restart", "status", "undo"],
    ),
    ("jouet", "edit", &["hash", "replace"]),
    (
        "jouet",
        "wait",
        &[
            "ci", "cmd", "file", "http", "log", "pid", "port", "pr", "run",
        ],
    ),
    ("jouet", "pr", &["status", "threads"]),
    (
        "jouet",
        "rule",
        &["list", "test", "trust", "types", "untrust"],
    ),
    (
        "uv|pip|pip3",
        "pip",
        &["freeze", "install", "list", "show", "uninstall"],
    ),
];

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
            ("git commit -mwip", "git commit <arg>"),
            ("echo \"--password hunter2\"", "echo <arg>"),
            (
                "git commit -m '--wip rotate key abc'",
                "git commit -m <arg>",
            ),
            ("cargo test --Secret_Thing", "cargo test <arg>"),
            ("git push -f origin main", "git push -f <arg> <arg>"),
            // Programs without subcommands keep no positional.
            ("echo confidential", "echo <arg>"),
            ("cat credentials secrets.txt", "cat <arg> <arg>"),
            ("rm -rf build", "rm -rf <arg>"),
            // A task, target or script name is the user's own word.
            ("make deploy-prod", "make <arg>"),
            ("mise run release-secret", "mise run <arg>"),
            ("npm run deploy", "npm run <arg>"),
            // Only known subcommands, at either level.
            ("git stash push -m wip", "git stash push -m <arg>"),
            ("git checkout main", "git checkout <arg>"),
            ("jouet rule test git-stash", "jouet rule test <arg>"),
            ("jouet audit --since 7d", "jouet audit --since <arg>"),
            ("jouet stats --session s1", "jouet stats --session <arg>"),
            (
                "curl -H 'Authorization: token ghp_abcdefghijklmnopqrstuvwxyz0123' https://x",
                "curl -H <arg> <arg>",
            ),
            ("GH_TOKEN=ghp_x gh api --token=sk-123", "gh api --token=<v>"),
            ("python3 - <<'EOF'\nprint('secret')\nEOF", "python3 <arg>"),
            ("cargo test 2>&1 | tail -5", "cargo test | tail <arg>"),
            ("rtk git add -A", "git add -A"),
            ("rtk gain --history", "rtk <arg> --history"),
        ] {
            assert_eq!(shape(script), expected, "{script}");
        }
    }

    #[test]
    fn the_repo_key_hashes_the_work_tree_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        let key = repo_key(&root.join("src/deep")).unwrap();
        assert_eq!(key.len(), 16);
        assert_eq!(repo_key(&root), Some(key.clone()));
        let expected = Sha256::digest(root.canonicalize().unwrap().to_string_lossy().as_bytes());
        assert!(
            expected
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                .starts_with(&key)
        );
        assert_eq!(repo_key(dir.path()), None);
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
