//! The check before an agent stops: if a test, lint or build run in this
//! session failed and nothing since has covered it, or files changed after
//! the last test run passed with none passing since, the stop hook blocks
//! once and says which. The tests a change affects are enough: the full
//! suite runs on CI.
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
//! the reasons raised and the tree, and a later stop at the same tree, with
//! no run recorded since, raises only a reason not raised yet, or is let
//! through. The agent has already answered the others.
//!
//! A work tree other than the session's own is set aside once the session
//! has left it: nothing there is uncommitted, and the session has run no
//! check there for [`SETTLED_AFTER`]. Its runs judged work that has been
//! committed and left, such as a branch since merged, whose tree moved on
//! without the session.

use jouet_rules::{Kind, Record};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How many unresolved failures of a kind are kept; older ones are dropped.
const MAX_FAILURES: usize = 16;

/// How long after the session's last run in another work tree with nothing
/// uncommitted that tree stops being asked about.
pub(crate) const SETTLED_AFTER: i64 = 60 * 60;

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

    /// Whether this pass ran what `failed` selected: the same selection, or
    /// the same command over more names (`test:e2e a b` covers `test:e2e
    /// a`). An option can change what the words mean (pytest's `-k not x`),
    /// so a selection with one must match exactly.
    fn covers(&self, failed: &Run) -> bool {
        let (ran, wanted) = (self.selection(), failed.selection());
        if ran == wanted {
            return true;
        }
        let words = |s: &str| -> Option<Vec<String>> {
            let words: Vec<String> = s.split_whitespace().map(str::to_string).collect();
            (!words.iter().any(|w| w.starts_with('-') && w != "--")).then_some(words)
        };
        let (Some(ran), Some(wanted)) = (words(ran), words(wanted)) else {
            return false;
        };
        !ran.is_empty() && ran.first() == wanted.first() && wanted.iter().all(|w| ran.contains(w))
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
    /// The newest passes of each kind, one per selection, oldest first: what
    /// a failure at another tree is judged by when the tree goes back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub passes: BTreeMap<Kind, Vec<Run>>,
    /// The newest lint run through hk (`hk check`, `jouet diag check`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hk: Option<Run>,
    /// What the stop hook last blocked for, until a run is recorded here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised: Option<Raised>,
    /// The `jouet test` runs still going here, as each marked itself when
    /// it started, until it records its result: runs can overlap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<Running>,
    /// Commands (as [`same_run`] keys them) whose failures `jouet test`
    /// could not compare with the merge base, which could not run them
    /// either, until one of them passes.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unverifiable: BTreeSet<String>,
    /// Those whose failure the stop hook has raised: the agent has said
    /// which failures are its own, and nothing here can tell more, so they
    /// are not raised again whatever changes, until the command passes.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub answered: BTreeSet<String>,
    /// When the session last recorded or started a run here, in seconds
    /// since the Unix epoch. State files written before jouet kept it lack
    /// it, and read as long ago.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<i64>,
}

/// A command as the unverifiable failures key it: `jouet test` without
/// `--budget`, which only changes how much of a failure is shown.
fn same_run(command: &str) -> String {
    if !command.starts_with("jouet test") {
        return command.to_string();
    }
    let mut words = Vec::new();
    let mut skip = false;
    for word in command.split_whitespace() {
        if skip {
            skip = false;
        } else if word == "--budget" {
            skip = true;
        } else if !word.starts_with("--budget=") {
            words.push(word);
        }
    }
    words.join(" ")
}

/// A `jouet test` run in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Running {
    pub pid: u32,
    pub command: String,
}

impl Running {
    /// Whether its process has not exited: one killed before it recorded a
    /// result holds nothing up.
    fn alive(&self) -> bool {
        jouet_wait::process_running(self.pid)
    }
}

/// The reasons the stop hook blocked with, and the tree it saw then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Raised {
    /// Every reason raised at `tree`, in the order they were.
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
    /// How many reasons were left for a later stop, past [`MAX_RAISED`].
    #[serde(skip)]
    pub more: usize,
    /// The unverifiable commands whose failure these reasons raise, which
    /// are then answered (see [`Checks::answered`]).
    #[serde(skip)]
    pub answering: Vec<String>,
}

/// The most reasons one stop raises; the rest wait for the next.
const MAX_RAISED: usize = 5;

/// A command as a message shows it: cut to 80 characters.
fn shown(command: &str) -> String {
    match command.char_indices().nth(80) {
        Some((i, _)) => format!("{}...", &command[..i]),
        None => command.to_string(),
    }
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

/// The `jouet test --baseline` run that compares the failed full run's own
/// command: a `jouet test` run with the flag added, any other command
/// through `--cmd`.
fn baseline_command(command: &str) -> String {
    match command.strip_prefix("jouet test") {
        Some(rest) if rest.split_whitespace().any(|w| w == "--baseline") => command.to_string(),
        Some(rest) if rest.is_empty() || rest.starts_with(' ') => {
            format!("jouet test --baseline{rest}")
        }
        _ => format!("jouet test --baseline --cmd -- {command}"),
    }
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
    /// A tree whose reasons were all raised, with nothing changed since,
    /// is passed over (see [`Checks::block`]). `tree` gives a root's work
    /// tree now, `hk_repo` whether it has an hk config, and `affected`
    /// whether `jouet test --affected` can select tests there.
    pub(crate) fn block(
        &self,
        home: &str,
        tree: impl Fn(&str) -> Option<String>,
        hk_repo: impl Fn(&str) -> bool,
        affected: impl Fn(&str) -> bool,
    ) -> Option<Block> {
        let own = self.trees.get_key_value(home);
        let others = self.trees.iter().filter(|(root, _)| root.as_str() != home);
        own.into_iter().chain(others).find_map(|(root, checks)| {
            let raised = checks.block(|| tree(root), hk_repo(root), affected(root))?;
            // Those raised now: the ones before were raised at this tree.
            let before = match &checks.raised {
                Some(r) if r.tree.is_some() && r.tree == raised.tree => r.reasons.len(),
                _ => 0,
            };
            let mut reason = raised.reasons[before.min(raised.reasons.len())..].join("\n\n");
            if raised.more > 0 {
                reason.push_str(&format!(
                    "\n\n({} more after these, at the next stop.)",
                    raised.more
                ));
            }
            let reason = if root == home || root.is_empty() {
                reason
            } else {
                format!("In {root}: {reason}")
            };
            Some(Block {
                root: root.clone(),
                reason,
                raised,
            })
        })
    }

    /// Drops the checks of work trees that are gone, such as a removed
    /// worktree: nothing is left there to check.
    pub(crate) fn prune_gone(&mut self) {
        self.trees
            .retain(|root, _| root.is_empty() || Path::new(root).is_dir());
    }

    /// Sets aside the checks of work trees the session has left: any but
    /// `home`, the session's own, where it ran nothing in the
    /// [`SETTLED_AFTER`] before `now` and `uncommitted` says nothing is
    /// uncommitted (`Some(false)`; None when it cannot tell).
    pub(crate) fn set_aside_settled(
        &mut self,
        home: &str,
        now: i64,
        uncommitted: impl Fn(&str) -> Option<bool>,
    ) {
        self.trees.retain(|root, checks| {
            let recent = checks
                .last_run
                .is_some_and(|at| now.saturating_sub(at) < SETTLED_AFTER);
            root == home || root.is_empty() || recent || uncommitted(root) != Some(false)
        });
    }

    /// Marks a `jouet test` run as going in the work tree at `root`.
    pub(crate) fn start_run(&mut self, root: &str, running: Running) {
        let checks = self.trees.entry(root.to_string()).or_default();
        checks.last_run = Some(jouet_output::time::now());
        let runs = &mut checks.running;
        // Marks whose process is gone are dropped as new ones come.
        runs.retain(|r| r.pid != running.pid && r.alive());
        runs.push(running);
    }

    /// Clears the mark the run with process `pid` left in `root`.
    pub(crate) fn end_run(&mut self, root: &str, pid: u32) {
        if let Some(checks) = self.trees.get_mut(root) {
            checks.running.retain(|r| r.pid != pid);
        }
    }

    /// Keeps what a stop was blocked for, so the next stop at the same tree
    /// with no run since does not raise it again.
    pub(crate) fn raise(&mut self, block: &Block) {
        let checks = self.trees.entry(block.root.clone()).or_default();
        checks.raised = Some(block.raised.clone());
        checks
            .answered
            .extend(block.raised.answering.iter().cloned());
    }

    /// Marks `command`'s failures in the work tree at `root` as ones the
    /// merge base could not run either.
    pub(crate) fn mark_unverifiable(&mut self, root: &str, command: &str) {
        self.trees
            .entry(root.to_string())
            .or_default()
            .unverifiable
            .insert(same_run(command));
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
            self.last_run = Some(jouet_output::time::now());
            let kind = record.kind;
            let run = Run::new(record, tree);
            let failures = self.failures.entry(kind).or_default();
            if run.passed() && run.full {
                failures.clear();
                self.full_passes.insert(kind, run.clone());
            } else if run.passed() {
                failures.retain(|f| f.full || !run.covers(f));
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
            if run.passed() {
                let key = same_run(&run.command);
                self.unverifiable.remove(&key);
                self.answered.remove(&key);
                let passes = self.passes.entry(kind).or_default();
                passes.retain(|p| p.full != run.full || p.selection() != run.selection());
                passes.push(run.clone());
                if passes.len() > MAX_FAILURES {
                    passes.remove(0);
                }
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

    /// The reasons to stop waiting, as [`Checks::reasons`] lists them, that
    /// the stop hook has not already blocked for at the tree now (up to
    /// [`MAX_RAISED`] of them), added to those it has; None when every reason
    /// has been raised at this tree with no run recorded since, so the agent
    /// has answered each once. An unknown tree is never the same tree.
    pub(crate) fn block(
        &self,
        tree: impl Fn() -> Option<String>,
        hk_repo: bool,
        affected: bool,
    ) -> Option<Raised> {
        let cell = std::cell::OnceCell::new();
        let now = || cell.get_or_init(&tree).clone();
        let reasons = self.reasons(now, hk_repo, affected);
        let tree = now();
        let mut raised = match &self.raised {
            Some(raised) if tree.is_some() && raised.tree == tree => raised.reasons.clone(),
            _ => Vec::new(),
        };
        // Two runs whose commands are cut to the same text read as one
        // reason, so they are raised, and counted, as one.
        let mut new: Vec<String> = Vec::new();
        for reason in reasons {
            if !raised.contains(&reason) && !new.contains(&reason) {
                new.push(reason);
            }
        }
        if new.is_empty() {
            return None;
        }
        let more = new.len().saturating_sub(MAX_RAISED);
        let new: Vec<String> = new.into_iter().take(MAX_RAISED).collect();
        let answering = self
            .failures
            .iter()
            .flat_map(|(kind, failures)| failures.iter().map(move |f| (*kind, f)))
            .filter(|(_, f)| self.unverifiable.contains(&same_run(&f.command)))
            .filter(|(kind, f)| new.contains(&self.failure_reason(*kind, f)))
            .map(|(_, f)| same_run(&f.command))
            .collect();
        raised.extend(new);
        Some(Raised {
            reasons: raised,
            tree,
            more,
            answering,
        })
    }

    /// Every reason stopping should wait, the first to raise first: each
    /// failure nothing has cleared, files changed after the last test run
    /// passed with none passing since, and, in a repository with an hk
    /// config (`hk_repo`), files changed since the last hk check passed.
    /// `tree` gives the work tree now; it is only asked for when one of the
    /// last two could apply. `affected` is whether `jouet test --affected`
    /// can select tests there, for the message.
    pub(crate) fn reasons(
        &self,
        tree: impl Fn() -> Option<String>,
        hk_repo: bool,
        affected: bool,
    ) -> Vec<String> {
        // While a test run is still going, its result decides; the agent
        // waiting for it is not finishing on a stale one.
        let testing = self.running.iter().any(Running::alive);
        let mut reasons = self.failure_reasons(&tree, testing);
        if !testing {
            reasons.extend(self.untested_reason(&tree, affected));
        }
        reasons.extend(self.hk_reason(&tree, hk_repo));
        reasons
    }

    fn hk_reason(&self, tree: &impl Fn() -> Option<String>, hk_repo: bool) -> Option<String> {
        let hk = self.hk.as_ref().filter(|_| hk_repo)?;
        if !hk.passed() || hk.tree.is_none() {
            return None;
        }
        if !hk.covers_changes() {
            return Some(format!(
                "The last hk check passed for only some files (`{}`). Run `jouet diag check --changed` before you finish, or say why it isn't needed. [jouet: verify]",
                shown(&hk.command)
            ));
        }
        if hk.tree == tree() {
            return None;
        }
        Some(format!(
            "Files changed after the last hk check passed (`{}`). Run `jouet diag check --changed` before you finish, or say why it isn't needed. [jouet: verify]",
            shown(&hk.command)
        ))
    }

    /// Files changed after the newest test run passed, and no test run has
    /// passed at the files now. Any passing run counts, a partial one too:
    /// the tests a change affects are enough here, as the full suite runs
    /// on CI. A failed newest run is a failure reason instead.
    fn untested_reason(
        &self,
        tree: &impl Fn() -> Option<String>,
        affected: bool,
    ) -> Option<String> {
        let last = self.runs.get(&Kind::Test)?;
        if !last.passed() || last.tree.is_none() {
            return None;
        }
        let now = tree()?;
        let passed_now = self.passes.get(&Kind::Test).is_some_and(|passes| {
            passes
                .iter()
                .any(|p| p.tree.as_deref() == Some(now.as_str()))
        });
        if passed_now {
            return None;
        }
        let run = if affected {
            "`jouet test --affected`, or the tests for the code you changed"
        } else {
            "the tests for the code you changed"
        };
        Some(format!(
            "Files changed after the last test run passed (`{}`). Run the tests your change affects ({run}) before you finish, or say why none are needed. The full suite runs on CI. [jouet: verify]",
            shown(&last.command)
        ))
    }

    /// A reason for each failure nothing has cleared, by kind, and within a
    /// kind full failures first, as they say the most, newest first.
    ///
    /// A failure at another tree than now does not count when a pass that
    /// would clear it ran at the tree now: the files are back to ones that
    /// passed, as after checking that a test fails without a fix and putting
    /// the fix back. `tree` is only asked for when there is a failure.
    fn failure_reasons(&self, tree: &impl Fn() -> Option<String>, skip_tests: bool) -> Vec<String> {
        let cell = std::cell::OnceCell::new();
        let now = || cell.get_or_init(tree).clone();
        let passed_now = |kind: &Kind, failed: &Run| {
            failed.tree.is_some()
                && now().is_some_and(|now| {
                    failed.tree.as_deref() != Some(now.as_str())
                        && self.passes.get(kind).is_some_and(|passes| {
                            passes.iter().any(|p| {
                                p.tree.as_deref() == Some(now.as_str())
                                    && (p.full || (!failed.full && p.covers(failed)))
                            })
                        })
                })
        };
        self.failures
            .iter()
            .filter(|(kind, _)| !(skip_tests && **kind == Kind::Test))
            .flat_map(|(kind, failures)| {
                let full = failures.iter().rev().filter(|f| f.full);
                let partial = failures.iter().rev().filter(|f| !f.full);
                full.chain(partial)
                    .filter(|failed| !passed_now(kind, failed))
                    .filter(|failed| !self.answered.contains(&same_run(&failed.command)))
                    .map(|failed| self.failure_reason(*kind, failed))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn failure_reason(&self, kind: Kind, failed: &Run) -> String {
        if self.unverifiable.contains(&same_run(&failed.command)) {
            return format!(
                "The last {} run failed (`{}` exited {}), and the merge base could not run it either, so jouet cannot tell whether its failures were there before your change. Say which failures, if any, your change caused before you finish; this is asked once. [jouet: verify]",
                kind.name(),
                shown(&failed.command),
                failed.exit_code
            );
        }
        // A run that fails for reasons the change did not touch, such as a
        // tool this machine lacks, is the costly case: point at the check
        // that settles it. Its result is kept, so later runs of the same
        // command compare with it without running the merge base again.
        // Not a partial test run: that is most often a test written to fail.
        let baseline = if kind == Kind::Build || (kind == Kind::Test && failed.full) {
            format!(
                " If the failures are not from your change, `{}` runs it at the merge base too, and passes when each failure fails there as well.",
                baseline_command(&failed.command)
            )
        } else {
            String::new()
        };
        let last = self.runs.get(&kind);
        if last == Some(failed) {
            return format!(
                "The last {} run failed (`{}` exited {}). Fix it and run it again before you finish, or say plainly why it cannot pass.{baseline} [jouet: verify]",
                kind.name(),
                shown(&failed.command),
                failed.exit_code
            );
        }
        let since = last.map_or(String::new(), |l| format!(" (`{}`)", shown(&l.command)));
        let clear = if failed.full {
            format!("only a full {} run clears it", kind.name())
        } else {
            format!(
                "a run of the same selection, alone or with more names, or a full {} run clears it",
                kind.name()
            )
        };
        format!(
            "A {} run failed (`{}` exited {}), and the runs since{since} do not cover it: {clear}. Fix it and run it again before you finish, or say plainly why it cannot pass.{baseline} [jouet: verify]",
            kind.name(),
            shown(&failed.command),
            failed.exit_code
        )
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

    impl Checks {
        /// The first reason to block with, raised or not.
        fn block_reason(&self, tree: impl Fn() -> Option<String>, hk_repo: bool) -> Option<String> {
            self.reasons(tree, hk_repo, true).into_iter().next()
        }
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
    fn the_baseline_hint_compares_the_failed_command() {
        let mut state = Checks::default();
        state.record([test("python -m unittest", 1, "full")], Some("t1"));
        let why = reason(&state, "t1").unwrap();
        assert!(
            why.contains("`jouet test --baseline --cmd -- python -m unittest`"),
            "{why}"
        );
        for (command, hint) in [
            ("jouet test", "jouet test --baseline"),
            (
                "jouet test --cmd -- go test ./...",
                "jouet test --baseline --cmd -- go test ./...",
            ),
            ("jouet test --baseline", "jouet test --baseline"),
        ] {
            assert_eq!(baseline_command(command), hint);
        }
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
    fn a_test_run_still_going_holds_back_test_reasons_only() {
        let mut state = State::default();
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        state.record(
            &root,
            [test("cargo test --workspace", 101, "full")],
            Some("t1"),
        );
        state.record(
            &root,
            [record(Kind::Lint, "cargo clippy", 1, "full")],
            Some("t1"),
        );
        let block = |state: &State| {
            state
                .block(&root, |_| Some("t1".into()), |_| false, |_| true)
                .map(|b| b.reason)
        };
        assert!(block(&state).unwrap().contains("test run"));
        // This process is alive: the run it marks decides the tests.
        let command = "jouet test".to_string();
        let pid = std::process::id();
        state.start_run(&root, Running { pid, command });
        let why = block(&state).unwrap();
        assert!(why.contains("lint run"), "{why}");
        state.end_run(&root, pid);
        // A run whose process is gone holds nothing back.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let gone = child.id();
        child.wait().unwrap();
        state.start_run(
            &root,
            Running {
                pid: gone,
                command: "jouet test".into(),
            },
        );
        assert!(block(&state).unwrap().contains("test run"));
        state.end_run(&root, gone);
        assert!(state.trees[&root].running.is_empty());
        // Of two overlapping runs, the one still going holds back tests
        // after the other has ended.
        state.start_run(
            &root,
            Running {
                pid,
                command: "jouet test".into(),
            },
        );
        state.start_run(
            &root,
            Running {
                pid: gone,
                command: "jouet test --cmd".into(),
            },
        );
        state.end_run(&root, gone);
        assert!(block(&state).unwrap().contains("lint run"));
    }

    #[test]
    fn a_work_tree_that_is_gone_is_not_judged() {
        let mut state = State::default();
        let gone = std::env::temp_dir().join("jouet-verify-gone-tree-that-does-not-exist");
        let gone = gone.to_string_lossy().into_owned();
        state.record(
            &gone,
            [test("cargo test --workspace", 101, "full")],
            Some("t1"),
        );
        assert!(
            state
                .block("/home", |_| None, |_| false, |_| true)
                .is_some()
        );
        state.prune_gone();
        assert!(
            state
                .block("/home", |_| None, |_| false, |_| true)
                .is_none()
        );
    }

    #[test]
    fn a_failure_at_another_tree_does_not_count_once_the_tree_is_back_to_a_pass() {
        // A test passes with the fix, fails with it taken out to prove the
        // test, and the fix goes back: the tree is the one that passed.
        let mut state = Checks::default();
        let run = |code| test("go test -run TestImages .", code, "-run TestImages");
        state.record([run(0)], Some("fixed"));
        state.record([run(1)], Some("unfixed"));
        assert_eq!(reason(&state, "fixed"), None);
        // At the failing tree, or a third one, it still counts.
        let why = reason(&state, "unfixed").unwrap();
        assert!(why.starts_with("The last test run failed"), "{why}");
        assert!(reason(&state, "other").is_some());
        // A pass of another selection at this tree does not clear it.
        let mut other = Checks::default();
        other.record(
            [test("go test -run TestOther .", 0, "-run TestOther")],
            Some("fixed"),
        );
        other.record([run(1)], Some("unfixed"));
        assert!(reason(&other, "fixed").is_some());
        // A full failure needs a full pass at this tree; a partial one is not enough.
        let mut full = Checks::default();
        full.record([run(0)], Some("fixed"));
        full.record([test("go test ./...", 1, "full")], Some("unfixed"));
        assert!(reason(&full, "fixed").is_some());
        let mut both = Checks::default();
        both.record([test("go test ./...", 0, "full")], Some("fixed"));
        both.record([test("go test ./...", 1, "full")], Some("unfixed"));
        assert_eq!(reason(&both, "fixed"), None);
    }

    #[test]
    fn a_pass_over_more_names_clears_a_failure() {
        let mut state = Checks::default();
        state.record([test("mise run test", 0, "full")], Some("t1"));
        let e2e = |names: &str, code| {
            test(
                &format!("mise run test:e2e {names}"),
                code,
                &format!("test:e2e {names}"),
            )
        };
        state.record([e2e("test_a", 1)], Some("t1"));
        // Other names, or the same one run with an option, do not.
        state.record([e2e("test_b test_c", 0)], Some("t1"));
        state.record([test("pytest -k test_a", 0, "-k test_a")], Some("t1"));
        assert!(reason(&state, "t1").is_some());
        state.record([e2e("test_b test_a", 0)], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
        // A selection with an option has to match exactly.
        state.record([test("pytest -k x", 1, "-k x")], Some("t1"));
        state.record([test("pytest -k not x", 0, "-k not x")], Some("t1"));
        assert!(reason(&state, "t1").is_some());
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
    fn a_change_after_the_last_passing_test_run_blocks_until_one_passes_again() {
        let mut state = Checks::default();
        // A partial pass is enough at its tree: the full suite runs on CI.
        state.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
        let why = reason(&state, "t2").unwrap();
        assert!(
            why.starts_with("Files changed after the last test run passed (`cargo test -p a`)"),
            "{why}"
        );
        assert!(why.contains("`jouet test --affected`"), "{why}");
        assert!(why.contains("The full suite runs on CI."), "{why}");
        // Where --affected cannot select, it is not offered.
        let why = state.reasons(|| Some("t2".into()), false, false).remove(0);
        assert!(!why.contains("--affected"), "{why}");
        // Any passing run at the files now clears it.
        state.record([test("cargo test -p b", 0, "-p b")], Some("t2"));
        assert_eq!(reason(&state, "t2"), None);
        // An unknown tree (a Codex session) cannot be compared.
        assert_eq!(state.block_reason(|| None, false), None);
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
    fn another_work_tree_left_committed_and_idle_is_set_aside() {
        // Hours ago the session passed its tests in /old, whose branch has
        // since merged and moved on: its tree is not the one that passed.
        let tree = |root: &str| Some(if root == "/old" { "o2" } else { "h1" }.to_string());
        let ask = |state: &State| {
            state
                .block("/home", tree, |_| false, |_| true)
                .map(|b| b.reason)
        };
        let mut state = State::default();
        state.record("/old", [test("cargo test -p a", 0, "-p a")], Some("o1"));
        state.record("/home", [test("cargo test -p b", 0, "-p b")], Some("h1"));
        let ran = state.trees["/old"].last_run.unwrap();
        let why = ask(&state).unwrap();
        assert!(
            why.starts_with("In /old: Files changed after the last test run passed"),
            "{why}"
        );

        // Within the hour, it is still asked about, clean or not.
        let mut soon = State::load_from(&state);
        soon.set_aside_settled("/home", ran + SETTLED_AFTER - 1, |_| Some(false));
        assert!(ask(&soon).is_some());
        // Later, with something uncommitted there, or no telling, it is too.
        let later = ran + SETTLED_AFTER;
        for uncommitted in [Some(true), None] {
            let mut left = State::load_from(&state);
            left.set_aside_settled("/home", later, |_| uncommitted);
            assert!(ask(&left).is_some());
        }
        // Later and clean, it is set aside; the session's own tree never is.
        let mut left = State::load_from(&state);
        left.set_aside_settled("/home", later, |_| Some(false));
        assert_eq!(ask(&left), None);
        assert!(left.trees.contains_key("/home"));
        left.record("/home", [test("cargo test -p b", 1, "-p b")], Some("h1"));
        left.set_aside_settled("/home", later + SETTLED_AFTER, |_| Some(false));
        assert!(ask(&left).unwrap().starts_with("The last test run failed"));

        // A state written before jouet kept the time reads as long ago.
        let mut old = State::load_from(&state);
        old.trees.get_mut("/old").unwrap().last_run = None;
        old.set_aside_settled("/home", ran, |_| Some(false));
        assert_eq!(ask(&old), None);
    }

    impl State {
        /// A copy, through the state file's form.
        fn load_from(state: &State) -> State {
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap()
        }
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
            state
                .block("/home", tree, |_| false, |_| true)
                .map(|b| b.reason),
            None
        );
        // A change in the session's own tree after its last pass blocks,
        // unnamed.
        state.record("/home", [test("cargo test -p a", 0, "-p a")], Some("h0"));
        let why = state
            .block("/home", tree, |_| false, |_| true)
            .map(|b| b.reason)
            .unwrap();
        assert!(
            why.starts_with("Files changed after the last test run passed"),
            "{why}"
        );
        // A failure in another tree blocks, and names it.
        state.record(
            "/home",
            [test("cargo test --workspace", 0, "full")],
            Some("h1"),
        );
        assert_eq!(
            state
                .block("/home", tree, |_| false, |_| true)
                .map(|b| b.reason),
            None
        );
        state.record(
            "/other",
            [test("cargo test --workspace", 101, "full")],
            Some("o1"),
        );
        let why = state
            .block("/home", tree, |_| false, |_| true)
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
        let raised = state.block(|| Some("t1".into()), false, true).unwrap();
        assert!(raised.reasons[0].starts_with("The last test run failed"));
        assert_eq!(raised.tree.as_deref(), Some("t1"));
        state.raised = Some(raised.clone());
        // The same failure at the same tree: the agent has answered it.
        assert_eq!(state.block(|| Some("t1".into()), false, true), None);
        // The tree changed.
        assert_eq!(
            state.block(|| Some("t2".into()), false, true),
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
        assert!(state.block(|| None, false, true).is_some());
        // A new run, even the same failure again at the same tree.
        state.raised = Some(raised.clone());
        state.record([test("cargo test --workspace", 101, "full")], Some("t1"));
        assert_eq!(state.raised, None);
        assert_eq!(state.block(|| Some("t1".into()), false, true), Some(raised));
        // An untested change is kept the same way.
        let mut untested = Checks::default();
        untested.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        untested.raised = untested.block(|| Some("t2".into()), false, true);
        assert!(untested.raised.is_some());
        assert_eq!(untested.block(|| Some("t2".into()), false, true), None);
    }

    #[test]
    fn budget_does_not_make_another_run() {
        assert_eq!(same_run("jouet test --budget 300"), "jouet test");
        assert_eq!(
            same_run("jouet test --budget=40 --cmd -- cargo test"),
            "jouet test --cmd -- cargo test"
        );
        assert_eq!(same_run("cargo test --budget 3"), "cargo test --budget 3");
    }

    #[test]
    fn an_unverifiable_failure_is_raised_once_whatever_changes() {
        let mut state = State::default();
        let run = |budget: u32| test(&format!("jouet test --budget {budget}"), 1, "full");
        state.record("/w", [run(1500)], Some("t1"));
        // Without the mark, the usual reason, with --baseline offered.
        let first = state
            .block("/w", |_| Some("t1".into()), |_| false, |_| true)
            .unwrap();
        assert!(first.reason.contains("--baseline"), "{}", first.reason);
        // jouet test marks a run whose merge base could not run it either.
        state.mark_unverifiable("/w", "jouet test --budget 1500");
        let block = state
            .block("/w", |_| Some("t1".into()), |_| false, |_| true)
            .unwrap();
        assert!(
            block
                .reason
                .contains("the merge base could not run it either"),
            "{}",
            block.reason
        );
        assert!(
            block.reason.contains("this is asked once"),
            "{}",
            block.reason
        );
        state.raise(&block);
        // Answered: not again at another tree, nor after the same failure
        // with another --budget.
        assert!(
            state
                .block("/w", |_| Some("t2".into()), |_| false, |_| true)
                .is_none()
        );
        state.record("/w", [run(300)], Some("t3"));
        assert!(
            state
                .block("/w", |_| Some("t3".into()), |_| false, |_| true)
                .is_none()
        );
        // Until it passes: then a later failure is judged afresh.
        state.record("/w", [test("jouet test", 0, "full")], Some("t4"));
        let checks = &state.trees["/w"];
        assert!(checks.unverifiable.is_empty() && checks.answered.is_empty());
        state.record("/w", [run(400)], Some("t5"));
        let again = state
            .block("/w", |_| Some("t5".into()), |_| false, |_| true)
            .unwrap();
        assert!(
            again.reason.starts_with("The last test run failed"),
            "{}",
            again.reason
        );
        assert!(
            !again.reason.contains("could not run it either"),
            "{}",
            again.reason
        );
    }

    #[test]
    fn every_unresolved_reason_in_a_tree_is_raised_at_once() {
        let mut state = Checks::default();
        let lint = |command: &str, code: i32, scope: &str| record(Kind::Lint, command, code, scope);
        state.record(
            [
                test("cargo test --workspace", 101, "full"),
                test("cargo test -p a", 101, "-p a"),
                lint("cargo clippy --workspace", 1, "full"),
            ],
            Some("t1"),
        );
        let now = || Some("t1".to_string());
        let raised = state.block(now, false, true).unwrap();
        let seen = raised.reasons.clone();
        assert_eq!(raised.more, 0);
        state.raised = Some(raised);
        assert_eq!(state.block(now, false, true), None);
        // Each failure once, the full test failure before the partial one.
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(
            seen[0].contains("`cargo test --workspace` exited 101"),
            "{seen:?}"
        );
        assert!(seen[1].contains("`cargo test -p a` exited 101"), "{seen:?}");
        assert!(
            seen[2].contains("`cargo clippy --workspace` exited 1"),
            "{seen:?}"
        );
        // At another tree, they are raised again.
        let raised = state.block(|| Some("t2".into()), false, true).unwrap();
        assert_eq!(raised.reasons, seen);
    }

    #[test]
    fn past_five_reasons_the_rest_wait_for_the_next_stop() {
        let mut state = State::default();
        let failures: Vec<Record> = (0..7)
            .map(|i| test(&format!("cargo test -p p{i}"), 101, &format!("-p p{i}")))
            .collect();
        state.record("/home", failures, Some("t1"));
        let tree = |_: &str| Some("t1".to_string());
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert_eq!(block.raised.reasons.len(), 5);
        assert!(
            block
                .reason
                .ends_with("(2 more after these, at the next stop.)"),
            "{}",
            block.reason
        );
        state.trees.get_mut("/home").unwrap().raised = Some(block.raised);
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert_eq!(
            block.reason.matches("[jouet: verify]").count(),
            2,
            "{}",
            block.reason
        );
        assert!(
            !block.reason.contains("more after these"),
            "{}",
            block.reason
        );
    }

    #[test]
    fn reasons_that_read_the_same_are_raised_and_counted_once() {
        let mut state = State::default();
        let stem = format!("cargo test -p a {}", "x".repeat(80));
        let mut failures: Vec<Record> = (0..6)
            .map(|i| test(&format!("{stem} case{i}"), 101, &format!("{stem} case{i}")))
            .collect();
        failures.push(test("cargo test -p b", 101, "-p b"));
        state.record("/home", failures, Some("t1"));
        let tree = |_: &str| Some("t1".to_string());
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        // Six cut to one text, and the other.
        assert_eq!(block.raised.reasons.len(), 2, "{}", block.reason);
        assert_eq!(block.raised.more, 0);
        state.trees.get_mut("/home").unwrap().raised = Some(block.raised);
        assert_eq!(state.block("/home", tree, |_| false, |_| true), None);
    }

    #[test]
    fn a_long_command_is_cut_in_the_message_but_whole_in_the_hint() {
        let long = format!(
            "python -m unittest {}",
            "tests.test_a ".repeat(10).trim_end()
        );
        let mut state = Checks::default();
        state.record([test(&long, 1, "full")], Some("t1"));
        let why = reason(&state, "t1").unwrap();
        assert!(
            why.contains(&format!("(`{}` exited 1)", shown(&long))),
            "{why}"
        );
        assert!(shown(&long).ends_with("..."));
        assert!(
            why.contains(&format!("`jouet test --baseline --cmd -- {long}`")),
            "{why}"
        );
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
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert_eq!(block.root, "/home");
        assert!(block.reason.contains("exited 101"), "{block:?}");
        state.raise(&block);
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert_eq!(block.root, "/other");
        assert!(block.reason.starts_with("In /other: "), "{block:?}");
        // The raised reason is kept without the tree's name.
        assert!(block.raised.reasons[0].starts_with("The last test run failed"));
        state.raise(&block);
        assert_eq!(state.block("/home", tree, |_| false, |_| true), None);
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
        let block = state
            .block("/r", |_| Some("t1".into()), |_| false, |_| true)
            .unwrap();
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
