//! The check before an agent stops: if a test, lint or build run in this
//! session failed and nothing since has covered it, or only part of the
//! test suite ran since the tree last changed, the stop hook blocks once and
//! says which.
//!
//! What counts as a check is decided by post-tool-use rules (the built-in
//! `checks` rule, or a user's replacement), which `jouet.record` the runs
//! they recognise, each `full` (it covered the whole project) or `partial`
//! (with what it selected, such as `-p jouet-edit`). Claude Code reports
//! each command's exit code to a hook as it happens (PostToolUse on success,
//! PostToolUseFailure with `Exit code N`), and those records are kept, with
//! the tree each run saw ([`jouet_rules::tree`]), in a small per-session
//! state file. They are kept by the work tree each run ran in, after any `cd`
//! before it, so a session that works in another repository is judged by the
//! runs it made there. Codex sends no exit code to hooks, so its commands are read
//! from the end of the session transcript when it stops, and run through the
//! same rules then; their trees are unknown.
//!
//! A failed run is cleared by a later full pass of its kind, or by a partial
//! pass that selected the same thing. A partial pass never clears a full
//! failure: `cargo test -p x` passing says nothing about the rest of
//! `cargo test --workspace`.
//!
//! A reason is raised once per state: after a block, the tree's checks keep
//! the reason and the tree, and a later stop with the same reason at the
//! same tree, and no run recorded since, is let through. The agent has
//! already answered it.

use jouet_rules::{Kind, Record};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How many unresolved failures of a kind are kept; older ones are dropped.
const MAX_FAILURES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Run {
    pub command: String,
    pub exit_code: i32,
    /// Whether the run covered the whole project. State files written
    /// before jouet kept a scope lack it, and read as partial.
    #[serde(default)]
    pub full: bool,
    /// What a partial run selected, when its rule said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// The work tree the run saw, when it could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
}

impl Run {
    fn new(record: Record, tree: Option<&str>) -> Self {
        Self {
            command: record.command,
            exit_code: record.exit_code,
            full: record.full,
            selector: record.selector,
            tree: tree.map(str::to_string),
        }
    }

    /// What a pass must have selected to clear this run's failure: its
    /// selector, or without one its command.
    fn selection(&self) -> &str {
        self.selector.as_deref().unwrap_or(&self.command)
    }

    fn passed(&self) -> bool {
        self.exit_code == 0
    }
}

/// A session's checks, kept by work tree: a run counts for the repository
/// it ran in, which need not be the one the session started in.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    /// By the root of the work tree the runs ran in (see [`root_of`]); ""
    /// for runs outside a repository.
    #[serde(default)]
    pub trees: BTreeMap<String, Checks>,
}

/// The checks run in one work tree.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checks {
    /// The newest run of each kind.
    #[serde(default)]
    pub runs: BTreeMap<Kind, Run>,
    /// The newest full pass of each kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub full_passes: BTreeMap<Kind, Run>,
    /// Failed runs no later run has cleared, oldest first.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub failures: BTreeMap<Kind, Vec<Run>>,
    /// The newest lint run through hk (`hk check`, `jouet diag check`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hk: Option<Run>,
    /// What the stop hook last blocked for, until a run is recorded here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised: Option<Raised>,
}

/// A reason the stop hook blocked with, and the tree it saw then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Raised {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
}

/// Why the stop hook blocks, from the checks of one work tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    /// The root of that work tree.
    pub root: String,
    /// The reason, naming the tree when it is not the session's own.
    pub reason: String,
    /// What to keep in that tree's checks once the stop is blocked.
    pub raised: Raised,
}

/// Whether a lint run went through hk: `hk check`, `hk run check`, or
/// `jouet diag`, which runs it.
fn through_hk(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    let hk = words
        .windows(2)
        .any(|w| (w[0] == "hk" || w[0].ends_with("/hk")) && matches!(w[1], "check" | "run"));
    // `jouet diag`, also past jouet's output options (`jouet --json diag`).
    let diag = words
        .iter()
        .position(|w| *w == "jouet" || w.ends_with("/jouet"))
        .is_some_and(|at| {
            words[at + 1..]
                .iter()
                .find(|w| !w.starts_with('-') && **w != "agent" && **w != "human" && **w != "plain")
                == Some(&"diag")
        });
    hk || diag
}

impl Run {
    /// Whether it covered what hk's stop hook checks: the whole project, or
    /// every changed file.
    fn covers_changes(&self) -> bool {
        self.full || self.selector.as_deref() == Some("changed files")
    }
}

impl State {
    /// Adds runs that ran in the work tree at `root`, against `tree`.
    pub(crate) fn record(
        &mut self,
        root: &str,
        records: impl IntoIterator<Item = Record>,
        tree: Option<&str>,
    ) {
        self.trees
            .entry(root.to_string())
            .or_default()
            .record(records, tree);
    }

    /// The checks run in the work tree at `root`.
    pub(crate) fn checks(&self, root: &str) -> Option<&Checks> {
        self.trees.get(root)
    }

    /// Why stopping should wait, in any work tree the session ran checks
    /// in: `home`, the session's own, first, then the others, each named.
    /// A tree whose reason was already raised, with nothing changed since,
    /// is passed over (see [`Checks::block`]). `tree` gives a root's work
    /// tree now, and `hk_repo` whether it has an hk config.
    pub(crate) fn block(
        &self,
        home: &str,
        tree: impl Fn(&str) -> Option<String>,
        hk_repo: impl Fn(&str) -> bool,
    ) -> Option<Block> {
        let own = self.trees.get_key_value(home);
        let others = self.trees.iter().filter(|(root, _)| root.as_str() != home);
        own.into_iter().chain(others).find_map(|(root, checks)| {
            let raised = checks.block(|| tree(root), hk_repo(root))?;
            let reason = if root == home || root.is_empty() {
                raised.reason.clone()
            } else {
                format!("In {root}: {}", raised.reason)
            };
            Some(Block {
                root: root.clone(),
                reason,
                raised,
            })
        })
    }

    /// Keeps what a stop was blocked for, so the next stop at the same tree
    /// with no run since does not raise it again.
    pub(crate) fn raise(&mut self, block: &Block) {
        self.trees.entry(block.root.clone()).or_default().raised = Some(block.raised.clone());
    }

    pub(crate) fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut file, self)?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

/// The root of the work tree `dir` is in, as [`State`] keys it: the nearest
/// directory with a `.git`, from `dir` with symlinks and `..` resolved; ""
/// outside a repository.
pub(crate) fn root_of(dir: &Path) -> String {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    jouet_output::paths::work_tree(&dir)
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Checks {
    /// Adds runs that ran against `tree`.
    pub(crate) fn record(&mut self, records: impl IntoIterator<Item = Record>, tree: Option<&str>) {
        for record in records {
            // A run is something new to judge the next stop by.
            self.raised = None;
            let kind = record.kind;
            let run = Run::new(record, tree);
            let failures = self.failures.entry(kind).or_default();
            if run.passed() && run.full {
                failures.clear();
                self.full_passes.insert(kind, run.clone());
            } else if run.passed() {
                failures.retain(|f| f.full || f.selection() != run.selection());
            } else {
                failures.retain(|f| f.full != run.full || f.selection() != run.selection());
                failures.push(run.clone());
                if failures.len() > MAX_FAILURES {
                    failures.remove(0);
                }
            }
            if failures.is_empty() {
                self.failures.remove(&kind);
            }
            if kind == Kind::Lint && through_hk(&run.command) {
                self.hk = Some(run.clone());
            }
            self.runs.insert(kind, run);
        }
    }

    /// Whether the newest hk check passed, covered every changed file, and
    /// ran at the tree `now` gives.
    pub(crate) fn hk_passed_at(&self, now: impl Fn() -> Option<String>) -> bool {
        self.hk.as_ref().is_some_and(|run| {
            run.passed() && run.covers_changes() && run.tree.is_some() && run.tree == now()
        })
    }

    /// Why stopping should wait, as [`Checks::block_reason`] says, with the
    /// tree now; or None when the stop hook already blocked for that reason
    /// at that tree and no run has been recorded since, so the agent has
    /// answered it once. An unknown tree is never the same tree.
    pub(crate) fn block(&self, tree: impl Fn() -> Option<String>, hk_repo: bool) -> Option<Raised> {
        let cell = std::cell::OnceCell::new();
        let now = || cell.get_or_init(&tree).clone();
        let raised = Raised {
            reason: self.block_reason(now, hk_repo)?,
            tree: now(),
        };
        if raised.tree.is_some() && self.raised.as_ref() == Some(&raised) {
            return None;
        }
        Some(raised)
    }

    /// Why stopping should wait: a failure nothing has cleared, a test
    /// suite that has only partly run since the tree changed, or, in a
    /// repository with an hk config (`hk_repo`), files changed since the
    /// last hk check passed. `tree` gives the work tree now; it is only
    /// asked for when one of the last two could apply.
    pub(crate) fn block_reason(
        &self,
        tree: impl Fn() -> Option<String>,
        hk_repo: bool,
    ) -> Option<String> {
        if let Some(reason) = self.failure_reason() {
            return Some(reason);
        }
        if let Some(reason) = self.partial_test_reason(&tree) {
            return Some(reason);
        }
        let hk = self.hk.as_ref().filter(|_| hk_repo)?;
        if !hk.passed() || hk.tree.is_none() {
            return None;
        }
        if !hk.covers_changes() {
            return Some(format!(
                "The last hk check passed for only some files (`{}`). Run `jouet diag check --changed` before you finish, or say why it isn't needed. [jouet: verify]",
                hk.command
            ));
        }
        if hk.tree == tree() {
            return None;
        }
        Some(format!(
            "Files changed after the last hk check passed (`{}`). Run `jouet diag check --changed` before you finish, or say why it isn't needed. [jouet: verify]",
            hk.command
        ))
    }

    fn partial_test_reason(&self, tree: &impl Fn() -> Option<String>) -> Option<String> {
        let last = self.runs.get(&Kind::Test)?;
        if last.full || !last.passed() {
            return None;
        }
        let since = match &last.selector {
            Some(selector) => format!("`{}`, selecting {selector}", last.command),
            None => format!("`{}`", last.command),
        };
        let Some(full) = self.full_passes.get(&Kind::Test) else {
            return Some(format!(
                "Only part of the test suite has run in this session (last {since}). Run the full suite before you finish, or say why it isn't needed. [jouet: verify]"
            ));
        };
        if full.tree.is_some() && full.tree == tree() {
            return None;
        }
        Some(format!(
            "The tree has changed since the last full test run (`{}`), and only part of the suite has run since (last {since}). Run the full suite before you finish, or say why it isn't needed. [jouet: verify]",
            full.command
        ))
    }

    fn failure_reason(&self) -> Option<String> {
        let (kind, failures) = self.failures.iter().find(|(_, f)| !f.is_empty())?;
        // A full failure first: it says the most.
        let failed = failures.iter().rev().find(|f| f.full).or(failures.last())?;
        let last = self.runs.get(kind);
        if last == Some(failed) {
            return Some(format!(
                "The last {} run failed (`{}` exited {}). Fix it and run it again before you finish, or say plainly why it cannot pass. [jouet: verify]",
                kind.name(),
                failed.command,
                failed.exit_code
            ));
        }
        let since = last.map_or(String::new(), |l| format!(" (`{}`)", l.command));
        let clear = if failed.full {
            format!("only a full {} run clears it", kind.name())
        } else {
            format!(
                "a run of the same selection or a full {} run clears it",
                kind.name()
            )
        };
        Some(format!(
            "A {} run failed (`{}` exited {}), and the runs since{since} do not cover it: {clear}. Fix it and run it again before you finish, or say plainly why it cannot pass. [jouet: verify]",
            kind.name(),
            failed.command,
            failed.exit_code
        ))
    }
}

/// Loads, changes and saves a session's state under an exclusive lock, so
/// two hooks finishing at once cannot each save a copy without the other's
/// run.
pub(crate) fn update(path: &Path, change: impl FnOnce(&mut State)) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    // Released when `lock` is dropped.
    lock.lock()?;
    let mut state = State::load(path);
    change(&mut state);
    state.save(path)
}

/// The state file for a session, under `$XDG_STATE_HOME/jouet/sessions/`.
pub(crate) fn state_path(session_id: &str) -> Option<PathBuf> {
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(128)
        .collect();
    if safe.is_empty() {
        return None;
    }
    Some(sessions_dir()?.join(format!("{safe}.json")))
}

pub(crate) fn sessions_dir() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("sessions"))
}

/// Removes session files untouched for a week. Best effort.
pub(crate) fn prune_sessions() {
    let Some(Ok(entries)) = sessions_dir().map(std::fs::read_dir) else {
        return;
    };
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// How much of a Codex transcript is read, from the end.
const TRANSCRIPT_TAIL: u64 = 8 * 1024 * 1024;

/// The finished commands at the end of a Codex session transcript, oldest
/// first, with their exit codes.
///
/// The transcript format is Codex's own and undocumented; lines this does not
/// recognise are skipped, so a format change means no blocking, not false
/// blocking.
pub(crate) fn codex_commands(path: &Path) -> Vec<(String, i32)> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        // Starts mid-line.
        lines.next();
    }
    lines
        .filter(|line| line.contains("CommandExecution") || line.contains("exec_command_end"))
        .filter_map(codex_command)
        .collect()
}

/// A finished command from one transcript line: `item_completed` events for
/// a `CommandExecution` item, or older `exec_command_end` events.
fn codex_command(line: &str) -> Option<(String, i32)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = value.get("payload")?;
    let event = match payload.get("type")?.as_str()? {
        "item_completed" => payload
            .get("item")
            .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("CommandExecution"))?,
        "exec_command_end" => payload,
        _ => return None,
    };
    let exit_code = i32::try_from(event.get("exit_code")?.as_i64()?).ok()?;
    let command: Vec<&str> = event
        .get("command")?
        .as_array()?
        .iter()
        .filter_map(|w| w.as_str())
        .collect();
    // `["/usr/bin/zsh", "-lc", "cargo test"]`: the script is the last word.
    let script = match command.as_slice() {
        [_, flag, script] if flag.starts_with('-') && flag.contains('c') => script.to_string(),
        words => words.join(" "),
    };
    Some((script, exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run of `kind`: full when `scope` is "full", else partial and
    /// selecting `scope`.
    fn record(kind: Kind, command: &str, exit_code: i32, scope: &str) -> Record {
        Record {
            rule: "checks".into(),
            kind,
            command: command.into(),
            exit_code,
            full: scope == "full",
            dir: None,
            selector: (scope != "full").then(|| scope.to_string()),
        }
    }

    fn test(command: &str, exit_code: i32, scope: &str) -> Record {
        record(Kind::Test, command, exit_code, scope)
    }

    /// The reason to block with the work tree at `tree`.
    fn reason(state: &Checks, tree: &str) -> Option<String> {
        state.block_reason(|| Some(tree.to_string()), false)
    }

    #[test]
    fn blocks_while_the_last_run_failed() {
        let mut state = Checks::default();
        state.record([test("cargo test", 101, "full")], Some("t1"));
        let why = reason(&state, "t1").unwrap();
        assert!(why.starts_with("The last test run failed"), "{why}");
        assert!(why.contains("`cargo test` exited 101"), "{why}");
        state.record([test("cargo test", 0, "full")], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
    }

    #[test]
    fn a_partial_pass_does_not_clear_a_failed_full_run() {
        let mut state = Checks::default();
        state.record([test("cargo test --workspace", 101, "full")], Some("t1"));
        state.record(
            [test("cargo test -p jouet-edit", 0, "-p jouet-edit")],
            Some("t1"),
        );
        let why = reason(&state, "t1").unwrap();
        assert!(why.contains("`cargo test --workspace` exited 101"), "{why}");
        assert!(why.contains("only a full test run clears it"), "{why}");
        state.record([test("cargo test --workspace", 0, "full")], Some("t2"));
        assert_eq!(reason(&state, "t2"), None);
    }

    #[test]
    fn a_partial_pass_clears_only_a_failure_of_the_same_selection() {
        let mut state = Checks::default();
        state.record([test("cargo test --workspace", 0, "full")], Some("t1"));
        state.record([test("cargo test -p a", 101, "-p a")], Some("t1"));
        state.record([test("cargo test -p b", 0, "-p b")], Some("t1"));
        let why = reason(&state, "t1").unwrap();
        assert!(why.contains("`cargo test -p a` exited 101"), "{why}");
        assert!(why.contains("same selection"), "{why}");
        // The same selection, run with other options, clears it.
        state.record(
            [test("cargo test -p a -- --nocapture", 0, "-p a")],
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);
        // A lint failure is its own.
        state.record(
            [record(Kind::Lint, "cargo clippy --workspace", 101, "full")],
            Some("t1"),
        );
        state.record(
            [record(Kind::Lint, "cargo clippy -p a", 0, "-p a")],
            Some("t1"),
        );
        assert!(reason(&state, "t1").unwrap().contains("clippy --workspace"));
    }

    #[test]
    fn only_partial_test_runs_since_the_tree_changed_block() {
        let mut state = Checks::default();
        state.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        let why = reason(&state, "t1").unwrap();
        assert!(why.starts_with("Only part of the test suite"), "{why}");
        assert!(why.contains("selecting -p a"), "{why}");

        state.record([test("cargo test --workspace", 0, "full")], Some("t1"));
        state.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        // The tree the full suite passed at: nothing to add.
        assert_eq!(reason(&state, "t1"), None);
        let why = reason(&state, "t2").unwrap();
        assert!(why.starts_with("The tree has changed"), "{why}");
        assert!(why.contains("`cargo test --workspace`"), "{why}");
        // An unknown tree (a Codex session) is not the same tree.
        assert!(state.block_reason(|| None, false).is_some());
        // A partial lint or build run is not asked about.
        let mut lint = Checks::default();
        lint.record([record(Kind::Lint, "cargo clippy -p a", 0, "-p a")], None);
        assert_eq!(lint.block_reason(|| None, false), None);
    }

    #[test]
    fn files_changed_after_a_passing_hk_check_block_in_an_hk_repo() {
        let mut state = Checks::default();
        let lint = |command: &str, code: i32, scope: &str| record(Kind::Lint, command, code, scope);
        state.record(
            [lint("jouet diag check --changed", 0, "changed files")],
            Some("t1"),
        );
        assert!(state.hk_passed_at(|| Some("t1".into())));
        assert!(!state.hk_passed_at(|| Some("t2".into())));
        assert_eq!(state.block_reason(|| Some("t1".into()), true), None);
        let why = state.block_reason(|| Some("t2".into()), true).unwrap();
        assert!(
            why.starts_with(
                "Files changed after the last hk check passed (`jouet diag check --changed`)"
            ),
            "{why}"
        );
        // Not without an hk config, and not for a check of a few files.
        assert_eq!(state.block_reason(|| Some("t2".into()), false), None);
        state.record([lint("hk check src/a.rs", 0, "files src/a.rs")], Some("t1"));
        assert!(!state.hk_passed_at(|| Some("t1".into())));
        // At the same tree, a check of some files still asks for the changed ones.
        let why = state.block_reason(|| Some("t1".into()), true).unwrap();
        assert!(
            why.starts_with("The last hk check passed for only some files (`hk check src/a.rs`)"),
            "{why}"
        );
        // A failing hk check is a lint failure like any other.
        state.record([lint("hk check --all", 1, "full")], Some("t3"));
        let why = state.block_reason(|| Some("t3".into()), true).unwrap();
        assert!(why.contains("`hk check --all` exited 1"), "{why}");
        // Other linters are not hk.
        assert!(!through_hk("cargo clippy --workspace"));
        assert!(through_hk("mise x hk -- hk run check --files0-from -"));
        assert!(through_hk("jouet --json diag check --changed"));
        assert!(through_hk("jouet --display agent diag check"));
        assert!(!through_hk("jouet edit replace diag --old a --new b"));
    }

    #[test]
    fn each_work_tree_is_judged_by_its_own_runs() {
        let mut state = State::default();
        let tree = |root: &str| Some(if root == "/other" { "o1" } else { "h1" }.to_string());
        // A full pass in another repository, then part of its suite.
        state.record(
            "/other",
            [test("cargo test --workspace", 0, "full")],
            Some("o1"),
        );
        state.record(
            "/other",
            [test("bats test/edit.bats", 0, "test/edit.bats")],
            Some("o1"),
        );
        assert_eq!(
            state.block("/home", tree, |_| false).map(|b| b.reason),
            None
        );
        // Only part of the suite in the session's own tree blocks, unnamed.
        state.record("/home", [test("cargo test -p a", 0, "-p a")], Some("h1"));
        let why = state
            .block("/home", tree, |_| false)
            .map(|b| b.reason)
            .unwrap();
        assert!(why.starts_with("Only part of the test suite"), "{why}");
        // A failure in another tree blocks, and names it.
        state.record(
            "/home",
            [test("cargo test --workspace", 0, "full")],
            Some("h1"),
        );
        assert_eq!(
            state.block("/home", tree, |_| false).map(|b| b.reason),
            None
        );
        state.record(
            "/other",
            [test("cargo test --workspace", 101, "full")],
            Some("o1"),
        );
        let why = state
            .block("/home", tree, |_| false)
            .map(|b| b.reason)
            .unwrap();
        assert!(
            why.starts_with("In /other: The last test run failed"),
            "{why}"
        );
    }

    #[test]
    fn a_raised_reason_is_not_raised_again_until_something_changes() {
        let mut state = Checks::default();
        state.record([test("cargo test --workspace", 101, "full")], Some("t1"));
        let raised = state.block(|| Some("t1".into()), false).unwrap();
        assert!(raised.reason.starts_with("The last test run failed"));
        assert_eq!(raised.tree.as_deref(), Some("t1"));
        state.raised = Some(raised.clone());
        // The same failure at the same tree: the agent has answered it.
        assert_eq!(state.block(|| Some("t1".into()), false), None);
        // The tree changed.
        assert_eq!(
            state.block(|| Some("t2".into()), false),
            Some(Raised {
                tree: Some("t2".into()),
                ..raised.clone()
            })
        );
        // An unknown tree is never the same one.
        state.raised = Some(Raised {
            tree: None,
            ..raised.clone()
        });
        assert!(state.block(|| None, false).is_some());
        // A new run, even the same failure again at the same tree.
        state.raised = Some(raised.clone());
        state.record([test("cargo test --workspace", 101, "full")], Some("t1"));
        assert_eq!(state.raised, None);
        assert_eq!(state.block(|| Some("t1".into()), false), Some(raised));
        // A partial-run reason is kept the same way.
        let mut partial = Checks::default();
        partial.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        partial.raised = partial.block(|| Some("t1".into()), false);
        assert!(partial.raised.is_some());
        assert_eq!(partial.block(|| Some("t1".into()), false), None);
    }

    #[test]
    fn a_tree_whose_reason_was_raised_gives_way_to_the_next() {
        let mut state = State::default();
        let tree = |root: &str| Some(if root == "/other" { "o1" } else { "h1" }.to_string());
        state.record(
            "/home",
            [test("cargo test --workspace", 101, "full")],
            Some("h1"),
        );
        state.record(
            "/other",
            [test("cargo test --workspace", 1, "full")],
            Some("o1"),
        );
        let block = state.block("/home", tree, |_| false).unwrap();
        assert_eq!(block.root, "/home");
        assert!(block.reason.contains("exited 101"), "{block:?}");
        state.raise(&block);
        let block = state.block("/home", tree, |_| false).unwrap();
        assert_eq!(block.root, "/other");
        assert!(block.reason.starts_with("In /other: "), "{block:?}");
        // The raised reason is kept without the tree's name.
        assert!(block.raised.reason.starts_with("The last test run failed"));
        state.raise(&block);
        assert_eq!(state.block("/home", tree, |_| false), None);
    }

    #[test]
    fn state_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let mut state = State::default();
        state.record(
            "/r",
            [record(Kind::Lint, "cargo clippy --workspace", 101, "full")],
            Some("t1"),
        );
        state.record("", [test("cargo test -p a", 0, "-p a")], None);
        let block = state.block("/r", |_| Some("t1".into()), |_| false).unwrap();
        state.raise(&block);
        state.save(&path).unwrap();
        assert_eq!(State::load(&path), state);
        assert_eq!(
            State::load(&dir.path().join("missing.json")),
            State::default()
        );
    }

    #[test]
    fn concurrent_updates_keep_every_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let threads: Vec<_> = [(Kind::Test, 101), (Kind::Lint, 0), (Kind::Build, 1)]
            .into_iter()
            .map(|(kind, code)| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..40 {
                        let run = record(kind, &format!("run {i}"), code, "full");
                        update(&path, |s| s.record("/r", [run], None)).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let state = State::load(&path);
        let runs = &state.trees["/r"].runs;
        assert_eq!(runs.len(), 3, "{state:?}");
        assert_eq!(runs[&Kind::Test].exit_code, 101);
    }

    #[test]
    fn session_ids_cannot_escape_the_state_directory() {
        let path = state_path("../../etc/passwd").unwrap();
        assert_eq!(path.file_name().unwrap(), "etcpasswd.json");
        assert_eq!(state_path("../"), None);
    }

    #[test]
    fn reads_commands_from_a_codex_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let item = |cmd: &str, code: i32| {
            serde_json::json!({"type": "event_msg", "payload": {"type": "item_completed",
                "item": {"type": "CommandExecution", "command": ["/usr/bin/zsh", "-lc", cmd],
                    "status": "completed", "exit_code": code}}})
            .to_string()
        };
        let lines = [
            item("cargo test", 101),
            r#"{"type":"response_item","payload":{"type":"message"}}"#.to_string(),
            "not json, but mentions CommandExecution".to_string(),
            r#"{"type":"event_msg","payload":{"type":"exec_command_end","command":["ls"],"exit_code":0}}"#.to_string(),
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        assert_eq!(
            codex_commands(&path),
            [("cargo test".to_string(), 101), ("ls".to_string(), 0)]
        );
        assert!(codex_commands(&dir.path().join("missing")).is_empty());
    }
}
