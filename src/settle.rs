//! The hooks' side of a PR watcher's lease (see `jouet_wait::lease`).
//!
//! In the `settle` arm (`crate::experiment`), jouet starts a watcher itself
//! after a push (`jouet watch`, see `crate::cmd::watch`), and an Auto-fix
//! event about the PR one live watcher of the session covers is held for
//! that watcher's digest instead of starting a turn. Every session records,
//! as `would_drop = "covered"`, the events it would hold. A watcher that
//! ends without its digest leaves its lease behind; the next hook of its
//! session reports the events held for it, records a `lease-lost` entry,
//! and removes it.
//!
//! A lease is for one PR, and a session may hold several: an event is held
//! for the watcher of the PR it names, whichever directory the session runs
//! in. So is the PR a push is watched for the one of the checkout the push
//! ran in ([`push_dir`]), not of the session's own directory.

use crate::prompt::{Coverable, Shape};
use jouet_rules::shell::{self, Op};
use jouet_wait::lease::{self, Held, Lease};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

/// Why a covered event did not start a turn.
pub(crate) const HOLD_REASON: &str = "jouet held this Auto-fix event for the PR watcher running in this session (the one jouet started after a push, or a `jouet wait pr --until needs-agent`), which reports it in its digest when the PR settles; set `[experiment] settle_share = 0` in jouet's config to stop.";

fn now() -> i64 {
    jouet_output::time::now()
}

/// The live lease of `session` that covers every PR the event names, with
/// its watcher's pid.
pub(crate) fn covering(session: &str, event: &Coverable) -> Option<(PathBuf, u32)> {
    let dir = lease::dir()?;
    covering_in(&dir, session, event, now())
}

fn covering_in(dir: &Path, session: &str, event: &Coverable, now: i64) -> Option<(PathBuf, u32)> {
    Lease::of_session(dir, session)
        .into_iter()
        .find(|(_, l)| l.live(now) && event.prs.iter().all(|(r, n)| l.covers(r, *n)))
        .map(|(path, l)| (path, l.pid))
}

/// Holds the event for the watcher of `lease`. False when it could not be
/// written, or the watcher ended meanwhile, so the event starts a turn.
pub(crate) fn hold(lease_path: &Path, event: &Coverable, shape: &Shape) -> bool {
    let held = Held {
        at: now(),
        prs: event.prs.iter().map(|(r, n)| format!("{r}#{n}")).collect(),
        kinds: shape.kinds.clone(),
        comments: shape.comments,
        checks: event.checks.clone(),
        comment_ids: event.comments.clone(),
    };
    // The watcher removes its lease before it reads the held file a last
    // time, so a lease still there means the watcher will see this one, or
    // has left the lease for the next hook to report it.
    lease::hold(lease_path, &held).is_ok() && lease_path.exists()
}

/// A watcher that ended without its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lost {
    /// The watcher's pid.
    pub pid: u32,
    pub held: usize,
    /// What to tell the agent, when events were held for it.
    pub note: Option<String>,
}

/// The leases of `session` whose watchers are gone, each removed.
pub(crate) fn lost(session: &str) -> Vec<Lost> {
    lease::dir().map_or_else(Vec::new, |dir| lost_in(&dir, session, now()))
}

fn lost_in(dir: &Path, session: &str, now: i64) -> Vec<Lost> {
    Lease::of_session(dir, session)
        .into_iter()
        .filter(|(_, l)| l.lost(now))
        .map(|(path, l)| {
            // The lease goes first: a hook that held an event while it was
            // there has written it by now, and one that writes later finds
            // no lease and lets its event through (`hold`).
            let _ = std::fs::remove_file(&path);
            let held = lease::held(&path);
            Lease::remove(&path);
            Lost {
                pid: l.pid,
                held: held.len(),
                note: lost_note(&l, &held),
            }
        })
        .collect()
}

/// What to tell the agent about the events held for a watcher that ended
/// without its digest; nothing when none were.
pub(crate) fn lost_note(lease: &Lease, held: &[Held]) -> Option<String> {
    let first = held.iter().map(|h| h.at).min()?;
    // The watcher's own --since, so a restart sees the comments it did:
    // they were posted before their events were held.
    let since = Some(lease.since)
        .filter(|s| *s > 0)
        .unwrap_or_else(|| lease.started.min(first));
    let n = held.len();
    let mut note = format!(
        "jouet: the `jouet wait pr` watcher for {}#{} stopped before its digest, with {n} Auto-fix event{} jouet held for it:",
        lease.repo,
        lease.pr,
        if n == 1 { "" } else { "s" }
    );
    for h in held {
        let checks = if h.checks.is_empty() {
            String::new()
        } else {
            format!(" ({})", h.checks.join(", "))
        };
        let _ = write!(
            note,
            " {} {}{checks};",
            jouet_output::time::format_utc(h.at),
            h.kinds.join(", ")
        );
    }
    let _ = write!(
        note,
        " look at the PR, or start the watcher again in the background: `jouet wait pr {} --repo {} --until needs-agent --since {}`",
        lease.pr,
        lease.repo,
        jouet_output::time::format_utc(since)
    );
    Some(note)
}

/// The word pairs of a command that pushes a branch or opens a PR.
const PUSH_WORDS: [[&str; 2]; 3] = [["git", "push"], ["pr", "create"], ["stack", "submit"]];

/// git's options before its subcommand that take the next word as their
/// value, as `git_subcommand` in the rules' prelude has them.
const GIT_VALUE_OPTIONS: [&str; 6] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--exec-path",
];

/// What a Bash command that may push looks like, as the patterns the
/// watcher's hook entries are installed with: each pair of [`PUSH_WORDS`],
/// and a push git is given its directory for, whose `-C DIR` stands between
/// the two words.
pub(crate) fn push_patterns() -> Vec<String> {
    PUSH_WORDS
        .iter()
        .map(|[program, verb]| format!("{program} {verb}"))
        .chain(["git -C * push".to_string()])
        .collect()
}

/// A git command's options before its subcommand, each with the value it
/// takes as its next word, and its words from the subcommand on.
fn git_options(argv: &[String]) -> (Vec<(&str, Option<&str>)>, &[String]) {
    let mut options = Vec::new();
    let mut rest = argv.get(1..).unwrap_or_default();
    while let Some(option) = rest.first().filter(|word| word.starts_with('-')) {
        let valued = GIT_VALUE_OPTIONS.contains(&option.as_str());
        let value = rest.get(1).filter(|_| valued).map(String::as_str);
        options.push((option.as_str(), value));
        rest = rest.get(1 + usize::from(valued)..).unwrap_or_default();
    }
    (options, rest)
}

/// Whether a parsed command is itself a push or the opening of a PR: it
/// runs git with `push` after git's own options (`git -C dir push`), or
/// another program with one of the other pairs as its subcommand (`gh pr
/// create`, `gh --repo o/r pr create`). A command that only has the words
/// among its arguments (`echo git push done`) is not one.
fn is_push(command: &shell::Command) -> bool {
    let argv = command.argv();
    match command.program() {
        Some("git") => git_options(argv)
            .1
            .first()
            .is_some_and(|verb| verb == "push"),
        Some("echo" | "printf") | None => false,
        Some(_) => PUSH_WORDS
            .iter()
            .any(|pair| pair[0] != "git" && subcommand(argv, pair)),
    }
}

/// Whether `pair` is the subcommand `argv` runs: its first words after the
/// program and the program's own options, each of which may take the word
/// after it as its value.
fn subcommand(argv: &[String], pair: &[&str; 2]) -> bool {
    let mut rest = argv.get(1..).unwrap_or_default();
    let mut value = false;
    loop {
        if rest.get(..2).is_some_and(|words| words == pair) {
            return true;
        }
        match rest.first() {
            Some(word) if word.starts_with('-') => value = true,
            Some(_) if value => value = false,
            _ => return false,
        }
        rest = &rest[1..];
    }
}

/// Whether the command at `at`, or the pipeline it is in, runs only when
/// what is before it in its list failed (`a || b`), so a script that worked
/// may not have run it. The shell groups a list from the left, so the push
/// of `a || b && git push` is not one: it runs when either worked.
fn alternative(commands: &[shell::Command], at: usize) -> bool {
    commands[..at]
        .iter()
        .rev()
        .find(|c| c.op != Some(Op::Pipe))
        .is_some_and(|c| c.op == Some(Op::Or))
}

/// Whether a shell command pushes a branch or opens a PR.
pub(crate) fn pushes(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    PUSH_WORDS
        .iter()
        .any(|pair| words.windows(2).any(|w| w == pair))
        // Only its parsed words tell `git -C dir push` from `git -C dir
        // status; echo push`.
        || (command.contains("push") && shell::parse(command).commands.iter().any(is_push))
}

/// The directory the last push of `command` ran in, when it is not `cwd`,
/// the directory the call started in, and the command's text says so for
/// sure: where the `cd`s before the push and git's own `-C` lead.
///
/// None for a push in `cwd`, and whenever the text leaves it open: a `cd`
/// to a variable's value or a substitution's output, a subshell, a group,
/// a loop, a function, a `cd` that runs only when something before it
/// failed, `pushd`, a script that is sourced. The caller then looks in
/// `cwd`, as it did before it asked.
///
/// Nothing is run to find out, neither the command nor git. The text is
/// the agent's, so a directory is taken only when it is there and inside a
/// git work tree.
pub(crate) fn push_dir(command: &str, cwd: &Path) -> Option<PathBuf> {
    let dir = followed(&stated_dir(command, cwd)?)?;
    let moved = cwd.canonicalize().map_or(true, |cwd| cwd != dir);
    (moved && dir.is_dir() && jouet_output::paths::work_tree(&dir).is_some()).then_some(dir)
}

/// `dir` as the file system has it, with its links and each `..` followed,
/// when the two readings of a `..` agree. The shell's `cd` drops the name
/// before it, while git's `-C` and the file system take the parent of what
/// that name links to. None when they differ (`cd link && cd ..`), since
/// the text does not say which shell option is set, and for a path that is
/// not there.
fn followed(dir: &Path) -> Option<PathBuf> {
    let mut at = PathBuf::new();
    for part in dir.components() {
        match part {
            Component::ParentDir => {
                let named = at.parent()?.canonicalize().ok()?;
                if at.canonicalize().ok()?.parent() != Some(&named) {
                    return None;
                }
                at.pop();
            }
            Component::CurDir => {}
            part => at.push(part),
        }
    }
    at.canonicalize().ok()
}

/// Where the text of `command` says its last push ran, started in `cwd`;
/// None when it leaves that open. Whether there is such a directory is
/// [`push_dir`]'s to check.
fn stated_dir(command: &str, cwd: &Path) -> Option<PathBuf> {
    let script = shell::parse(command);
    // The last push is the one whose PR is new. One after `||` ran only if
    // something failed, and a script that worked does not say whether it
    // did, or an earlier push instead.
    let at = script.commands.iter().rposition(is_push)?;
    if alternative(&script.commands, at) {
        return None;
    }
    let (before, push) = (&script.commands[..at], &script.commands[at]);
    if !plain(command, &script.commands[..=at]) {
        return None;
    }
    let mut dir = cwd.to_path_buf();
    // `GIT_DIR=dir git push`, `env GIT_DIR=dir git push` and `export
    // GIT_DIR=dir` before it name the repository another way, and `export
    // HOME=dir` changes what `~` and `$HOME` stand for.
    let elsewhere = |a: &String| {
        ["GIT_DIR=", "GIT_WORK_TREE=", "HOME="]
            .iter()
            .any(|name| a.starts_with(name))
    };
    for (i, step) in before.iter().enumerate() {
        if step.words.iter().any(elsewhere) {
            return None;
        }
        match step.program() {
            Some("cd") => {
                let runs = match i.checked_sub(1).and_then(|p| before[p].op) {
                    // The first of its list: it runs whatever came before.
                    None | Some(Op::Seq | Op::Background) => true,
                    // After `&&` it ran if the push did, as long as nothing
                    // from here to the push lets the shell carry on without
                    // it.
                    Some(Op::And) => before[i..]
                        .iter()
                        .all(|c| matches!(c.op, Some(Op::And | Op::Pipe))),
                    Some(Op::Or | Op::Pipe) => false,
                };
                // In a pipeline or the background it moves a subshell, or
                // the shell itself: zsh runs a pipeline's last command there.
                if !runs || !matches!(step.op, Some(Op::And | Op::Or | Op::Seq)) {
                    return None;
                }
                dir = literal_dir(&dir, cd_target(step)?, step, command)?;
            }
            // These move the shell too, to where the text does not say.
            Some("pushd" | "popd" | "eval" | "source" | "." | "builtin") => return None,
            _ => {}
        }
    }
    // So does `env -C dir git push`.
    let wrappers = &push.words[..push.words.len() - push.argv().len()];
    if wrappers
        .iter()
        .any(|w| w == "-C" || w.starts_with("--chdir"))
        || wrappers.iter().chain(&push.env).any(elsewhere)
    {
        return None;
    }
    if push.program() == Some("git") {
        for (option, value) in git_options(push.argv()).0 {
            match option {
                "-C" => dir = literal_dir(&dir, value?, push, command)?,
                _ if option.starts_with("--git-dir") || option.starts_with("--work-tree") => {
                    return None;
                }
                _ => {}
            }
        }
    }
    Some(dir)
}

/// Whether `commands`, a script's first, run one after another in the shell
/// that runs the script: nothing but operators, blank space and comments is
/// before or between them, so none is inside a subshell, a group, a loop, a
/// condition or a function, and no assignment or here-document stands
/// between two.
fn plain(script: &str, commands: &[shell::Command]) -> bool {
    let mut from = 0;
    commands.iter().all(|command| {
        let gap = command
            .span
            .as_ref()
            .and_then(|span| Some((script.get(from..span.start)?, span.end)));
        gap.is_some_and(|(gap, end)| {
            from = end;
            gap.lines().all(|line| {
                let code = line.split('#').next().unwrap_or_default();
                code.chars()
                    .all(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '\\'))
            })
        })
    })
}

/// The one directory a `cd` names: `cd DIR` or `cd -- DIR`. None for `cd`
/// alone, `cd -` and a `cd` with options.
fn cd_target(cd: &shell::Command) -> Option<&str> {
    let args = cd.argv().get(1..)?;
    let args = match args {
        [first, rest @ ..] if first == "--" => rest,
        _ => args,
    };
    match args {
        [dir] if !dir.starts_with('-') => Some(dir),
        _ => None,
    }
}

/// `word`, one of `command`'s in `script`, as a directory from `base`, when
/// the shell takes it as written: no pattern, no brace and no substitution
/// is in it, the only parameter is a `$HOME` it starts with, and a `~` it
/// starts with stands alone before any `/`, in a command with no quoting.
fn literal_dir(base: &Path, word: &str, command: &shell::Command, script: &str) -> Option<PathBuf> {
    let (home, rest) = ["$HOME", "${HOME}", "~"]
        .iter()
        .find_map(|prefix| Some((*prefix, word.strip_prefix(prefix)?)))
        .filter(|(_, rest)| rest.is_empty() || rest.starts_with('/'))
        .unwrap_or(("", word));
    let as_written = match home {
        "" => !word.starts_with('~'),
        "~" => command
            .span
            .clone()
            .and_then(|span| script.get(span))
            .is_some_and(|text| !text.contains(['\'', '"', '\\'])),
        _ => command.vars.iter().any(|var| var == "HOME"),
    };
    (as_written && !rest.contains(['$', '*', '?', '[', '{', '`']))
        .then(|| jouet_rules::resolve_dir(base, word))
}

/// Whether a hook's payload reports a push that worked: a foreground Bash
/// call that exited 0 and pushed a branch or opened a PR.
pub(crate) fn pushed(payload: &serde_json::Value, call: &jouet_rules::Call) -> bool {
    matches!(call.event, Some(jouet_rules::Event::PostToolUse))
        && call.tool == "Bash"
        && call.exit_code == Some(0)
        && !crate::cmd::hook::in_background(payload)
        && call.command.as_deref().is_some_and(pushes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease_at(session: &str, pid: u32, heartbeat: i64) -> Lease {
        Lease {
            pid,
            session: session.into(),
            repo: "jdx/jouet".into(),
            pr: 12,
            branch: None,
            started: heartbeat,
            since: heartbeat - 60,
            heartbeat,
        }
    }

    fn event(prs: &[(&str, u64)]) -> Coverable {
        Coverable {
            prs: prs.iter().map(|(r, n)| ((*r).to_string(), *n)).collect(),
            checks: vec!["trusted / test".into()],
            comments: Vec::new(),
        }
    }

    #[test]
    fn only_a_live_lease_of_the_session_covering_every_pr_covers_an_event() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let path = Lease::path(dir.path(), "s1", me).unwrap();
        lease_at("s1", me, 1_000).write(&path).unwrap();
        let now = 1_010;
        assert_eq!(
            covering_in(dir.path(), "s1", &event(&[("jdx/jouet", 12)]), now),
            Some((path.clone(), me))
        );
        assert_eq!(
            covering_in(dir.path(), "s2", &event(&[("jdx/jouet", 12)]), now),
            None
        );
        assert_eq!(
            covering_in(
                dir.path(),
                "s1",
                &event(&[("jdx/jouet", 12), ("jdx/jouet", 13)]),
                now
            ),
            None
        );
        // Its heartbeat went stale.
        assert_eq!(
            covering_in(
                dir.path(),
                "s1",
                &event(&[("jdx/jouet", 12)]),
                1_000 + lease::STALE
            ),
            None
        );
    }

    #[test]
    fn a_gone_watcher_reports_what_was_held_for_it_once() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let live = Lease::path(dir.path(), "s1", me).unwrap();
        lease_at("s1", me, 1_000).write(&live).unwrap();
        let gone = Lease::path(dir.path(), "s1", u32::MAX - 1).unwrap();
        lease_at("s1", u32::MAX - 1, 1_000).write(&gone).unwrap();
        let empty = Lease::path(dir.path(), "s1", u32::MAX - 2).unwrap();
        lease_at("s1", u32::MAX - 2, 1_000).write(&empty).unwrap();
        let shape = Shape {
            kinds: vec!["ci-failure".into()],
            ..Shape::default()
        };
        assert!(hold(&gone, &event(&[("jdx/jouet", 12)]), &shape));
        let lost = lost_in(dir.path(), "s1", 1_010);
        assert_eq!(lost.len(), 2);
        let with_held = lost.iter().find(|l| l.held == 1).unwrap();
        let note = with_held.note.as_deref().unwrap();
        assert!(
            note.contains("jdx/jouet#12 stopped before its digest"),
            "{note}"
        );
        assert!(note.contains("ci-failure (trusted / test)"), "{note}");
        assert!(
            note.contains(&format!(
                "jouet wait pr 12 --repo jdx/jouet --until needs-agent --since {}",
                jouet_output::time::format_utc(940)
            )),
            "{note}"
        );
        assert!(lost.iter().any(|l| l.held == 0 && l.note.is_none()));
        // Removed, so the next hook says nothing; the live one stays.
        assert!(lost_in(dir.path(), "s1", 1_010).is_empty());
        assert!(live.exists());
        // So does the lease of a watcher that was told to stop and is still
        // running: it covers nothing, and is not taken away under a hook
        // that is holding an event for it.
        lease_at("s1", me, 1_000)
            .leaving(1_010)
            .write(&live)
            .unwrap();
        assert!(covering_in(dir.path(), "s1", &event(&[("jdx/jouet", 12)]), 1_010).is_none());
        assert!(lost_in(dir.path(), "s1", 1_011).is_empty());
        assert!(live.exists());
        assert_eq!(lost_in(dir.path(), "s1", 1_100).len(), 1);
    }

    #[test]
    fn a_hold_after_the_watcher_removed_its_lease_lets_the_event_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = Lease::path(dir.path(), "s1", 1).unwrap();
        assert!(!hold(
            &path,
            &event(&[("jdx/jouet", 12)]),
            &Shape::default()
        ));
    }

    #[test]
    fn pushes_and_pr_creation_are_recognized() {
        assert!(pushes("git push -u origin feat/x"));
        assert!(pushes("gh pr create --fill"));
        assert!(pushes("gh stack submit --open"));
        assert!(!pushes("git status"));
        assert!(!pushes("jouet commit a.rs --message m"));
        assert!(!pushes("echo pushing"));
        // git's own options stand between the two words.
        assert!(pushes("git -C ../other push -u origin feat/x"));
        assert!(pushes(
            "cd /src && git -c push.default=current --no-pager push"
        ));
        assert!(!pushes("git -C ../other status; echo push"));
        assert!(!pushes("git -C ../other log --grep push"));
        assert!(!pushes("git stash push"));
        assert_eq!(
            push_patterns(),
            ["git push", "pr create", "stack submit", "git -C * push"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn each_watcher_of_a_session_holds_the_events_of_its_own_pr() {
        let dir = tempfile::tempdir().unwrap();
        // Two processes that run: the test and the one that started it.
        let (me, parent) = (std::process::id(), std::os::unix::process::parent_id());
        let here = Lease::path(dir.path(), "s1", me).unwrap();
        lease_at("s1", me, 1_000).write(&here).unwrap();
        let elsewhere = Lease::path(dir.path(), "s1", parent).unwrap();
        Lease {
            repo: "jdx/mise".into(),
            pr: 9,
            ..lease_at("s1", parent, 1_000)
        }
        .write(&elsewhere)
        .unwrap();
        let now = 1_010;
        let (first, second) = (event(&[("jdx/jouet", 12)]), event(&[("jdx/mise", 9)]));
        assert_eq!(
            covering_in(dir.path(), "s1", &first, now),
            Some((here.clone(), me))
        );
        assert_eq!(
            covering_in(dir.path(), "s1", &second, now),
            Some((elsewhere.clone(), parent))
        );
        // Neither covers the other's PR number in its own repository, nor an
        // event about both.
        for prs in [
            &[("jdx/jouet", 9)][..],
            &[("jdx/mise", 12)],
            &[("jdx/jouet", 12), ("jdx/mise", 9)],
        ] {
            assert_eq!(covering_in(dir.path(), "s1", &event(prs), now), None);
        }
        assert!(hold(&elsewhere, &second, &Shape::default()));
        assert!(lease::held(&here).is_empty());
        let [held] = lease::held(&elsewhere).try_into().unwrap();
        assert_eq!(held.prs, ["jdx/mise#9"]);
    }

    /// Where `command`'s push ran by its text, started in `/work/session`.
    fn stated(command: &str) -> Option<PathBuf> {
        stated_dir(command, Path::new("/work/session"))
    }

    /// `dir` as the shell takes it in `/work/session`.
    fn at(dir: &str) -> Option<PathBuf> {
        Some(jouet_rules::resolve_dir(Path::new("/work/session"), dir))
    }

    #[test]
    fn a_push_is_followed_through_the_cds_before_it() {
        // Two pushes of the watcher's first day, which it looked for in the
        // session's directory.
        assert_eq!(
            stated(
                "cd ~/src/mise-fu-lint-runs-clippy && git add tasks.md && git commit -q -m \"docs(tasks): say what lint runs\" && git push 2>&1 | tail -2 && jouet pr threads reply PRRT_1 --body 'Done.' --resolve"
            ),
            at("~/src/mise-fu-lint-runs-clippy")
        );
        assert_eq!(
            stated("cd ~/src/mise-silent-otlp && git push -u origin claude/fix-silent-otlp-export"),
            at("~/src/mise-silent-otlp")
        );
        assert_eq!(
            stated(
                "cd ~/src/mise-silent-otlp && gh pr create --base main --head claude/fix-silent-otlp-export --title 'fix(otlp): report a failed export' --body-file /tmp/body.md"
            ),
            at("~/src/mise-silent-otlp")
        );
        for (command, dir) in [
            ("git push", "/work/session"),
            (
                "cd /src/other; git add a.rs; git commit -q -m m; git push",
                "/src/other",
            ),
            ("cd $HOME/src/other && gh stack submit", "$HOME/src/other"),
            ("cd \"${HOME}/src/other\" && git push", "${HOME}/src/other"),
            ("cd ~ && git push", "~"),
            ("cd ../other && git push", "/work/session/../other"),
            (
                "cd crates && cargo fmt && cd jouet-wait && git push",
                "/work/session/crates/jouet-wait",
            ),
            ("cd 'a b' && git push", "/work/session/a b"),
            ("cd -- /src/other && git push", "/src/other"),
            ("command cd /src/other && rtk git push", "/src/other"),
            // A `cd` that ends the script when it fails.
            ("cd /src/other || exit 1\ngit push", "/src/other"),
            ("git fetch -q && cd /src/other && git push", "/src/other"),
            (
                "# the other checkout\ncd /src/other && \\\n  git push # now",
                "/src/other",
            ),
            // What comes after the push moved nothing it ran in, and the
            // last push is the one watched.
            (
                "cd /src/other && git push && cd /src/third && cargo test",
                "/src/other",
            ),
            ("cd /src/a && git push; cd /src/b && git push", "/src/b"),
            // Words a later command only prints are not the push.
            (
                "git -C /src/other push && echo git push complete",
                "/src/other",
            ),
            ("cd /src/a && git push; cd /src/b; echo pr create", "/src/a"),
            // The program's own options stand before its subcommand.
            (
                "cd /src/other && gh --repo jdx/other pr create --fill",
                "/src/other",
            ),
            (
                "cd /src/other && gh -R jdx/other stack submit",
                "/src/other",
            ),
            // What follows `||` is not a push, so the push is the one that ran.
            ("git -C /src/a push || echo failed", "/src/a"),
            ("git fetch || true && git -C /src/other push", "/src/other"),
            (
                "cd /src/a && git push -u origin x && gh pr create --fill",
                "/src/a",
            ),
        ] {
            assert_eq!(stated(command), at(dir), "{command}");
        }
    }

    #[test]
    fn git_s_own_directory_option_is_followed() {
        for (command, dir) in [
            ("git -C /src/other push", "/src/other"),
            ("git -C ~/src/other push -u origin feat", "~/src/other"),
            ("git -C ../other push", "/work/session/../other"),
            (
                "git --no-pager -C /src/other -c push.default=current push",
                "/src/other",
            ),
            ("cd /src && git -C other push", "/src/other"),
            ("git -C /src -C other push", "/src/other"),
            // Another command's `-C` says nothing about the push.
            ("git -C /src/other status && git push", "/work/session"),
            (
                "git -C /src/other add a.rs && gh pr create --fill",
                "/work/session",
            ),
        ] {
            assert_eq!(stated(command), at(dir), "{command}");
        }
    }

    #[test]
    fn a_directory_the_text_leaves_open_is_not_guessed() {
        for command in [
            // A near miss of the shapes above: only the shell knows `$DIR`.
            "cd \"$DIR\" && git push",
            "cd $DIR && git push",
            "DIR=/src/other; cd $DIR && git push",
            "cd $HOME/src/$NAME && git push",
            "cd $(git rev-parse --show-toplevel) && git push",
            "cd `pwd`/other && git push",
            "cd ~bob/src && git push",
            "cd /src/oth* && git push",
            // Quoted, these are not the home directory.
            "cd '~/src/other' && git push",
            "cd \"~/src/other\" && git push",
            "cd '$HOME/src/other' && git push",
            "cd - && git push",
            "cd && git push",
            "cd -P /src/other && git push",
            // Not the shell the push ran in, or not for sure.
            "(cd /src/other && git push)",
            "{ cd /src/other; git push; }",
            "if cd /src/other; then git push; fi",
            "for d in /src/a /src/b; do cd $d; done; git push",
            "f() { cd /src/other; }; f; git push",
            "! cd /src/other; git push",
            "cd /src/other | cat; git push",
            "cd /src/other & git push",
            "test -d .git || cd /src/other; git push",
            "test -d /src/other && cd /src/other; git push",
            "test -d x && cd /src/other && true || true && git push",
            "pushd /src/other && git push",
            "source ./env.sh && git push",
            "eval \"$go\"; git push",
            "cat <<EOF\ncd /src/x\nEOF\ncd /src/other && git push",
            // Said another way than `-C`.
            "git -C \"$DIR\" push",
            "git --git-dir=/src/other/.git push",
            "git --work-tree /src/other push",
            "GIT_DIR=/src/other/.git git push",
            "env GIT_DIR=/src/a/.git git -C /src/other push",
            "export GIT_DIR=/src/a/.git; cd /src/other && git push",
            // `$HOME` is not the hook's once the command sets it.
            "export HOME=/srv/alt; cd \"$HOME/repo\" && git push",
            "HOME=/srv/alt; cd $HOME/repo && git push",
            "HOME=/srv/alt git -C ~/repo push",
            // The only push runs when something before it failed.
            "git fetch || git -C /src/other push",
            // Either push may be the one that worked.
            "git push || git -C /src/other push",
            "cd /src/a && git push || git -C /src/b push",
            "git -C /src/a push && echo ok || git -C /src/b push",
            "git fetch || cat log | git -C /src/other push",
            "cd /src/other && gh api pr create",
            "env -C /src/other git push",
            // No command of its own is the push.
            "cd /src/other && sh -c 'git push'",
            "cd /src/other && echo \"$(git push)\"",
            "cd /src/other && git commit -m 'then git push'",
            "cd /src/other && echo git push",
            "cd /src/other && echo pr create",
        ] {
            assert_eq!(stated(command), None, "{command}");
        }
    }

    #[test]
    fn only_a_directory_inside_a_git_work_tree_is_taken() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let session = root.join("session");
        let other = root.join("other");
        for dir in [
            session.join(".git"),
            other.join(".git"),
            other.join("sub"),
            root.join("plain"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(other.join("file"), "").unwrap();
        assert_eq!(
            push_dir("cd ../other && git add file && git push", &session),
            Some(other.clone())
        );
        assert_eq!(
            push_dir(
                &format!("git -C {} push", other.join("sub").display()),
                &session
            ),
            Some(other.join("sub"))
        );
        for command in [
            // The session's own directory needs no saying.
            "git push",
            "cd . && git push",
            "cd ../session && git push",
            // Not there, not a directory, not in a work tree.
            "cd ../gone && git push",
            "cd ../other/file && git push",
            "cd ../plain && git push",
            "cd \"$DIR\" && git push",
        ] {
            assert_eq!(push_dir(command, &session), None, "{command}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_parent_directory_reached_through_a_link_is_not_guessed() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let session = root.join("session");
        let other = root.join("repos/other");
        for dir in [session.join(".git"), other.join(".git"), other.join("sub")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // `repos` is a work tree too, so a wrong `..` would find a PR there.
        std::fs::create_dir_all(root.join("repos/.git")).unwrap();
        std::os::unix::fs::symlink(&other, session.join("link")).unwrap();
        // A link is followed, and a `..` that stays under it.
        for (command, dir) in [
            ("cd link && git push", other.clone()),
            ("cd link/sub && git push", other.join("sub")),
            ("cd link/sub && cd .. && git push", other.clone()),
            ("git -C link/sub/.. push", other.clone()),
        ] {
            assert_eq!(push_dir(command, &session), Some(dir), "{command}");
        }
        // `cd ..` after it is the session's directory to the shell and
        // `repos` to the file system.
        for command in [
            "cd link && cd .. && git push",
            "cd link/.. && git push",
            "git -C link/.. push",
            "cd link && git -C .. push",
        ] {
            assert_eq!(push_dir(command, &session), None, "{command}");
        }
    }
}
