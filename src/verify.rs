//! The check before an agent stops: if a test, lint or build run in this
//! session failed and nothing since has covered it, or files changed after
//! the last test run passed with none passing since, the stop hook blocks
//! once and says which. A passing partial test run satisfies this gate; it
//! does not establish that the full suite passed.
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
//! without the session. It is set aside at once when it is its base exactly
//! (HEAD is the merge base with its default branch and nothing differs):
//! the session left no change there to test.

use jouet_rules::{Kind, Record};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// For a failed run whose every failure blames the machine (no D-Bus,
    /// a missing system library, no network): the line that says so. Such
    /// a failure does not hold up finishing; the stop says it in a note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// For a failed run whose every failing test is one the change does not
    /// reach (`jouet test --affected` selects none of them): those tests.
    /// Like a machine failure, it is a note at the stop, not a block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrelated: Option<String>,
    /// For a failed `jouet test` run, the failing tests as its runner named
    /// them, when it named each one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
    /// How the runner's test names hold one another, when jouet knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nesting: Option<Nesting>,
}

/// How a runner's test names hold the ones below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Nesting {
    /// A directory holds its files, a file its `::` tests:
    /// `tests` holds `tests/a.py::t`.
    Pytest,
    /// A dotted label holds the labels below it: `admin_views` holds
    /// `admin_views.tests.T`.
    Django,
}

impl Nesting {
    /// The nesting of the runner `argv` starts, through `python -m`.
    fn of(argv: &[String]) -> Option<Self> {
        let words: Vec<&str> = argv.iter().map(String::as_str).collect();
        let program = words.first()?.rsplit('/').next()?;
        let python = program.starts_with("python");
        match words.as_slice() {
            _ if matches!(program, "pytest" | "py.test") => Some(Self::Pytest),
            [_, "-m", "pytest", ..] if python => Some(Self::Pytest),
            [_, script, ..] if python && script.rsplit('/').next() == Some("runtests.py") => {
                Some(Self::Django)
            }
            [_, script, "test", ..] if python && script.ends_with("manage.py") => {
                Some(Self::Django)
            }
            [_, "test", ..] if program == "django-admin" => Some(Self::Django),
            _ => None,
        }
    }

    /// The nesting of the runner a mise task's last command starts.
    fn of_task(tasks: &[crate::wind::tasks::Existing], name: &str) -> Option<Self> {
        let script = tasks.iter().find(|t| t.name == name)?.run.last()?;
        let parsed = jouet_rules::shell::parse(script);
        Self::of(parsed.commands.last()?.argv())
    }

    /// Whether the name `ran` selects `wanted` and the tests beside it.
    fn holds(self, ran: &str, wanted: &str) -> bool {
        let Some(rest) = wanted.strip_prefix(ran) else {
            return false;
        };
        match self {
            Self::Pytest => {
                rest.starts_with("::")
                    || (rest.starts_with('/') && !rest.split('/').any(|part| part == ".."))
            }
            Self::Django => rest.starts_with('.') && !wanted.contains('/'),
        }
    }
}

impl Run {
    fn new(record: Record, tree: Option<&str>) -> Self {
        let nesting = Nesting::of(&runner_argv(&record.command));
        Self {
            command: record.command,
            exit_code: record.exit_code,
            full: record.full,
            selector: record.selector,
            tree: tree.map(str::to_string),
            environment: None,
            unrelated: None,
            tests: Vec::new(),
            nesting,
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

    fn pnpm_scoped(&self) -> bool {
        self.selection()
            .split_whitespace()
            .any(|word| word.starts_with("@jouet:pnpm:"))
    }

    /// Whether this pass ran what `failed` selected: the same selection,
    /// the same command over more names (`test:e2e a b` covers `test:e2e
    /// a`), or the same selection without the names after it (`-p a`
    /// covers `-p a name`, a filter that narrowed it). An option can change
    /// what the words mean (pytest's `-k not x`), so a selection with one
    /// must otherwise match exactly.
    fn covers(&self, failed: &Run) -> bool {
        let (ran, wanted) = (self.selection(), failed.selection());
        // An affected baseline only established that this tree's selected
        // jobs have pre-existing failures. Another tree may select other jobs.
        if ran.split_whitespace().next() == Some("--affected")
            && self
                .command
                .split_whitespace()
                .any(|word| word == "--baseline")
            && (self.tree.is_none() || self.tree != failed.tree)
        {
            return false;
        }
        // Playwright's unfiltered suite covers its own selected runs, even
        // through different package-manager wrappers. Its namespace also
        // prevents a test name in another runner from matching by accident.
        const PLAYWRIGHT: &str = "@jouet:playwright";
        fn playwright(s: &str) -> Option<&str> {
            s.split_whitespace()
                .next()
                .filter(|first| *first == PLAYWRIGHT || first.starts_with("@jouet:playwright:"))
        }
        match (playwright(ran), playwright(wanted)) {
            (Some(a), Some(b)) => {
                if a != b
                    || playwright_args(&self.command).is_none()
                    || playwright_args(&failed.command).is_none()
                {
                    return false;
                }
                let (Some(ran_args), Some(wanted_args)) = (
                    playwright_selection_args(ran),
                    playwright_selection_args(wanted),
                ) else {
                    return false;
                };
                return playwright_covers(&ran_args, &wanted_args);
            }
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        let pnpm = |selection: &str| {
            selection
                .split_whitespace()
                .any(|word| word.starts_with("@jouet:pnpm:"))
        };
        match (pnpm(ran), pnpm(wanted)) {
            (true, true) => {
                if ran.contains("@jouet:cwd=unknown:") || wanted.contains("@jouet:cwd=unknown:") {
                    return false;
                }
                return ran == wanted
                    || (same_runner(&self.command, &failed.command)
                        && pnpm_vitest_covers(ran, wanted));
            }
            (true, false) | (false, true) => return false,
            (false, false) => {}
        }
        if ran == wanted {
            return true;
        }
        // Selections leave out the runner, which these two read words by.
        let widens = |wanted: &str| {
            without_names(ran, wanted)
                || wider_filter(&failed.command, ran, wanted)
                || reran_failures(&failed.command, ran, wanted, &failed.tests)
                || whole_package(&failed.command, ran, wanted)
                || without_name_filter(ran, wanted)
                || wider_name_filter(ran, wanted)
                || (runner_argv(&self.command).first().map(String::as_str) == Some("go")
                    && go_covers(ran, wanted))
        };
        // A `--workspace` run records its picks without packages and runs
        // them in every package, so it covers them narrowed to some.
        if same_runner(&self.command, &failed.command)
            && (widens(wanted)
                || (cargo_workspace(&self.command)
                    && !ran.starts_with("-p ")
                    && without_packages(wanted).is_some_and(|w| ran == w || widens(&w))))
        {
            return true;
        }
        let words = |s: &str| -> Option<Vec<String>> {
            let words: Vec<String> = s.split_whitespace().map(str::to_string).collect();
            (!words.iter().any(|w| w.starts_with('-') && w != "--")).then_some(words)
        };
        let (Some(ran), Some(wanted)) = (words(ran), words(wanted)) else {
            return false;
        };
        if !same_runner(&self.command, &failed.command) || ran.is_empty() {
            return false;
        }
        if ran.first() == wanted.first() && wanted.iter().all(|w| ran.contains(w)) {
            return true;
        }
        // A name above each failed one, where the runner is known to run
        // the tests below a name.
        let Some(nesting) = self.nesting.filter(|n| failed.nesting == Some(*n)) else {
            return false;
        };
        !wanted.is_empty()
            && wanted
                .iter()
                .all(|w| ran.iter().any(|r| r == w || nesting.holds(r, w)))
    }
}

fn playwright_selection_args(selection: &str) -> Option<Vec<String>> {
    let parsed = jouet_rules::shell::parse(selection);
    let [command] = parsed.commands.as_slice() else {
        return None;
    };
    Some(command.words.iter().skip(1).cloned().collect())
}

/// Match only the explicit Playwright invocations the checks rule recognizes.
/// This keeps even a test literally named `@jouet:playwright` in another
/// runner from being mistaken for a Playwright suite.
fn playwright_args(command: &str) -> Option<Vec<String>> {
    use jouet_rules::shell::{Command, parse};
    parse(command).commands.iter().find_map(|command| {
        let mut argv = command.argv().to_vec();
        for _ in 0..4 {
            let parsed = Command {
                words: argv,
                ..Command::default()
            };
            let words = parsed.argv();
            let program = parsed.program()?;
            let args: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
            if program == "jouet" && args.contains(&"test") && args.contains(&"--cmd") {
                let separator = args.iter().position(|word| *word == "--")?;
                argv = args[separator + 1..]
                    .iter()
                    .map(|word| (*word).to_owned())
                    .collect();
                continue;
            }
            let at = match (program, args.as_slice()) {
                ("playwright", ["test", ..]) => 1,
                ("npx" | "bunx", ["playwright", "test", ..]) => 2,
                ("npx" | "bunx", ["--no-install", "playwright", "test", ..])
                | ("pnpm" | "yarn", ["exec", "playwright", "test", ..]) => 3,
                ("npm", ["exec", "--", "playwright", "test", ..]) => 4,
                ("yarn", ["workspace", _, "exec", "playwright", "test", ..]) => 5,
                _ => return None,
            };
            return Some(args[at..].iter().map(|word| (*word).to_owned()).collect());
        }
        None
    })
}

/// Compare only options whose values we can identify. Unknown flags keep the
/// result conservative; positional arguments retain their shell quoting.
fn playwright_covers(ran: &[String], wanted: &[String]) -> bool {
    if ran == wanted || ran.is_empty() {
        return true;
    }
    struct Selection {
        options: Vec<(String, String)>,
        names: Vec<String>,
    }
    fn parts(args: &[String]) -> Option<Selection> {
        let mut options = Vec::new();
        let mut names = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let word = &args[i];
            i += 1;
            if word.starts_with('-') {
                let (name, inline) = word
                    .split_once('=')
                    .map_or((word.as_str(), None), |(n, v)| (n, Some(v)));
                if !matches!(
                    name,
                    "--project" | "--config" | "-c" | "--grep" | "-g" | "--grep-invert"
                ) {
                    return None;
                }
                let value = if let Some(value) = inline {
                    value
                } else {
                    let value = args.get(i)?;
                    i += 1;
                    value
                };
                options.push((name.to_string(), value.to_string()));
            } else {
                names.push(word.clone());
            }
        }
        Some(Selection { options, names })
    }
    let (Some(ran), Some(wanted)) = (parts(ran), parts(wanted)) else {
        return false;
    };
    ran.options == wanted.options
        && (ran.names.is_empty()
            || (!wanted.names.is_empty()
                && wanted.names.iter().all(|name| ran.names.contains(name))))
}

/// An unfiltered package run covers its own excluded suite, but not a
/// different configuration, root, or unknown filter.
fn pnpm_vitest_covers(ran: &str, wanted: &str) -> bool {
    let ran: Vec<_> = ran.split_whitespace().collect();
    let wanted: Vec<_> = wanted.split_whitespace().collect();
    let prefix_len = if ran
        .first()
        .is_some_and(|word| word.starts_with("@jouet:cwd="))
    {
        2
    } else {
        1
    };
    if ran.len() != prefix_len
        || !ran[prefix_len - 1].starts_with("@jouet:pnpm:")
        || !ran[prefix_len - 1].ends_with(":vitest")
        || !wanted.starts_with(&ran)
    {
        return false;
    }
    let mut rest = &wanted[prefix_len..];
    let mut selected = false;
    while let Some((flag, next)) = rest.split_first() {
        if let Some(value) = flag
            .strip_prefix("--exclude=")
            .or_else(|| flag.strip_prefix("--testNamePattern="))
            .or_else(|| flag.strip_prefix("-t="))
        {
            if value.is_empty() {
                return false;
            }
            selected = true;
            rest = next;
        } else if matches!(*flag, "--exclude" | "--testNamePattern" | "-t")
            && next.first().is_some_and(|value| !value.starts_with('-'))
        {
            selected = true;
            rest = &next[1..];
        } else if !flag.starts_with('-') {
            selected = true;
            rest = next;
        } else {
            return false;
        }
    }
    selected
}

/// Whether two commands ran the same test runner: their program, and for
/// cargo its subcommand (`cargo test` and `cargo nextest` select alike, but
/// not as vitest does).
fn same_runner(a: &str, b: &str) -> bool {
    let runner = |command: &str| -> Vec<String> {
        let words = runner_argv(command);
        match words.as_slice() {
            [cargo, sub, ..] if cargo == "cargo" => vec![cargo.clone(), sub.clone()],
            [program, ..] => vec![program.clone()],
            [] => Vec::new(),
        }
    };
    let (a, b) = (runner(a), runner(b));
    !a.is_empty() && a == b
}

/// The words of the test runner a recorded command ran, through `jouet test
/// --cmd --`, wrappers such as `mise x --` and `timeout 300`, and cargo's
/// `+toolchain`: `jouet test --cmd -- mise x -- cargo +stable test -p a`
/// ran `cargo test -p a`.
fn runner_argv(command: &str) -> Vec<String> {
    use jouet_rules::shell::{Command, parse};
    let Some(first) = parse(command).commands.into_iter().next() else {
        return Vec::new();
    };
    let mut argv = first.argv().to_vec();
    // `jouet --json test --cmd -- …` and a path to jouet are the same call.
    if first.program() == Some("jouet")
        && argv.iter().any(|word| word == "test")
        && argv.iter().any(|word| word == "--cmd")
        && let Some(separator) = argv.iter().position(|word| word == "--")
    {
        argv = Command {
            words: argv[separator + 1..].to_vec(),
            ..Command::default()
        }
        .argv()
        .to_vec();
    }
    if argv.first().map(String::as_str) == Some("cargo")
        && argv.get(1).is_some_and(|word| word.starts_with('+'))
    {
        argv.remove(1);
    }
    argv
}

/// Whether `wanted` is `ran` with plain names after it: words that are
/// neither an option nor `--`, and that do not follow an option, whose
/// value they could be.
fn without_names(ran: &str, wanted: &str) -> bool {
    let ran: Vec<&str> = ran.split_whitespace().collect();
    let wanted: Vec<&str> = wanted.split_whitespace().collect();
    let option = |w: &&str| w.starts_with('-');
    !ran.is_empty()
        && wanted.len() > ran.len()
        && wanted.starts_with(&ran)
        && !ran.last().is_some_and(option)
        && !wanted[ran.len()..].iter().any(option)
}

/// Options that only narrow a run to the tests whose names match: bats'
/// `-f`, jest's and vitest's `-t`, go's `-run`. Not pytest's `-k`, whose
/// `not x` runs more than `x` would.
const NAME_FILTERS: &[&str] = &["-f", "--filter", "-t", "--testNamePattern", "-run"];

/// Whether `wanted` is `ran` with a name filter after it (`bats t.bats -f
/// name`): the whole of `ran` covers what the filter picked from it. The
/// name may be several words, since the recorded command lost its quotes,
/// but none may be another file or package (`./pkg`, or a word with the
/// extension of a file `ran` named), which `ran` did not run.
fn without_name_filter(ran: &str, wanted: &str) -> bool {
    let ran: Vec<&str> = ran.split_whitespace().collect();
    let wanted: Vec<&str> = wanted.split_whitespace().collect();
    let extensions: Vec<&str> = ran
        .iter()
        .filter_map(|w| Path::new(w).extension()?.to_str())
        .collect();
    let a_path = |w: &&str| {
        w.starts_with("./")
            || Path::new(w)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e))
    };
    !ran.is_empty()
        && wanted.len() > ran.len() + 1
        && wanted.starts_with(&ran)
        && NAME_FILTERS.contains(&wanted[ran.len()])
        && !wanted[ran.len() + 1..].iter().any(a_path)
}

/// Whether `ran` is `wanted` with its name filter widened to an alternation
/// that holds part of it (`-run 'TestA|TestPush'` covers `-run
/// TestPusher_Token`). These filters are unanchored regular expressions, so
/// a test whose name holds `wanted`'s plain name holds the branch too.
fn wider_name_filter(ran: &str, wanted: &str) -> bool {
    let ran: Vec<&str> = ran.split_whitespace().collect();
    let wanted: Vec<&str> = wanted.split_whitespace().collect();
    let [prefix @ .., flag, pattern] = ran.as_slice() else {
        return false;
    };
    let [wanted_prefix @ .., wanted_flag, name] = wanted.as_slice() else {
        return false;
    };
    prefix == wanted_prefix
        && flag == wanted_flag
        && NAME_FILTERS.contains(flag)
        && filter_holds(pattern, name)
}

/// Whether every test an unanchored name filter `name` picks is one that
/// `pattern` picks: `name` is a plain name (letters, digits and `_`), and
/// one of `pattern`'s `|` branches, also plain, is part of it.
fn filter_holds(pattern: &str, name: &str) -> bool {
    let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    plain(name) && pattern.split('|').all(plain) && pattern.split('|').any(|b| name.contains(b))
}

/// Whether a `go test` selection `ran` (its packages, then its name
/// filters, as the checks rule records them) ran all that `wanted` did:
/// each of `wanted`'s packages is one of `ran`'s or under a `dir/...` of
/// them, and `ran` filtered no more than `wanted` (no filter, the same, or
/// a `-run` alternation that holds `wanted`'s name). `-bench` runs what a
/// plain run does not, so a selection with it must match the filters.
fn go_covers(ran: &str, wanted: &str) -> bool {
    fn split(selection: &str) -> (Vec<&str>, Vec<&str>) {
        let words: Vec<&str> = selection.split_whitespace().collect();
        let at = words
            .iter()
            .position(|w| w.starts_with('-'))
            .unwrap_or(words.len());
        let mut packages = words[..at].to_vec();
        if packages.is_empty() {
            packages.push(".");
        }
        (packages, words[at..].to_vec())
    }
    fn normal(p: &str) -> &str {
        match p.trim_end_matches('/') {
            "" | "./" => ".",
            p => p,
        }
    }
    let ((ran_packages, ran_filters), (wanted_packages, wanted_filters)) =
        (split(ran), split(wanted));
    let filters = if ran_filters == wanted_filters {
        true
    } else if ran_filters.is_empty() {
        !wanted_filters
            .iter()
            .any(|w| w.starts_with("-bench") || w.starts_with("--bench"))
    } else {
        match (ran_filters.as_slice(), wanted_filters.as_slice()) {
            ([flag, pattern], [wanted_flag, name]) => {
                flag == wanted_flag
                    && matches!(*flag, "-run" | "--run")
                    && filter_holds(pattern, name)
            }
            _ => false,
        }
    };
    filters
        && wanted_packages.iter().all(|want| {
            let want = normal(want);
            ran_packages.iter().any(|ran| {
                let ran = normal(ran);
                ran == want
                    || ran.strip_suffix("/...").is_some_and(|base| {
                        let base = normal(base);
                        want == base
                            || want.strip_prefix(base).is_some_and(|rest| {
                                rest.starts_with('/') && !rest.split('/').any(|c| c == "..")
                            })
                    })
            })
        })
}

/// Whether `ran` is `wanted` with its cargo test-name filters widened:
/// cargo and nextest run every test whose name holds any filter, so
/// `affected::` runs all that `affected::cargo` does, and `a b` all that
/// `ab` does. The checks rule records a selector as its packages and picks
/// without the `--` (`-p mise --lib name other`), so the filters are the
/// plain words at its end, less one that is the value of the option before
/// them (`--bin jouet`, `-p mise`). Not with `--exact`, which matches whole
/// names.
fn wider_filter(command: &str, ran: &str, wanted: &str) -> bool {
    if !cargo_test(command) {
        return false;
    }
    let (Some((ran_head, ran)), Some((wanted_head, wanted))) =
        (cargo_filters(ran), cargo_filters(wanted))
    else {
        return false;
    };
    ran_head == wanted_head && wanted.iter().all(|w| ran.iter().any(|r| w.contains(r)))
}

/// Whether `ran`, a cargo test run of the same targets as `wanted`, has a
/// name filter that picks each of `tests`, the tests that failed in
/// `wanted`. cargo matches a filter against a test's whole path, so
/// `testlog` reruns `testlog::tests::a` that the failed run named `a`.
fn reran_failures(command: &str, ran: &str, wanted: &str, tests: &[String]) -> bool {
    if tests.is_empty() || !cargo_test(command) {
        return false;
    }
    let (Some((ran_head, ran)), Some((wanted_head, _))) =
        (cargo_filters(ran), cargo_filters(wanted))
    else {
        return false;
    };
    ran_head == wanted_head && tests.iter().all(|t| ran.iter().any(|r| t.contains(r)))
}

/// Whether `ran`, a cargo test run of whole packages (`-p jouet`, which a
/// bare `cargo test` in that package records), covers `wanted`: the same
/// packages narrowed to test targets (`--bin jouet`, `--lib`) or names.
/// Ignored tests, examples, benches and other picks run what `ran` did not.
fn whole_package(command: &str, ran: &str, wanted: &str) -> bool {
    if !cargo_test(command) {
        return false;
    }
    let ran: Vec<&str> = ran.split_whitespace().collect();
    let wanted: Vec<&str> = wanted.split_whitespace().collect();
    if ran.is_empty() || !ran.len().is_multiple_of(2) || !ran.chunks(2).all(|p| p[0] == "-p") {
        return false;
    }
    let Some(rest) = wanted.strip_prefix(ran.as_slice()) else {
        return false;
    };
    let mut rest = rest.iter();
    while let Some(word) = rest.next() {
        let target = ["--bin", "--test"].contains(word);
        let ok = ["--lib", "--bins", "--tests", "--doc"].contains(word)
            || word.starts_with("--bin=")
            || word.starts_with("--test=")
            || (target && rest.next().is_some_and(|v| !v.starts_with('-')))
            || !word.starts_with('-');
        if !ok {
            return false;
        }
    }
    true
}

/// Whether `command` is a cargo test run of the whole workspace.
fn cargo_workspace(command: &str) -> bool {
    cargo_test(command)
        && runner_argv(command)
            .iter()
            .take_while(|word| *word != "--")
            .any(|word| word == "--workspace" || word == "--all")
}

/// `selector` without its leading `-p NAME` pairs, or None when it has none.
fn without_packages(selector: &str) -> Option<String> {
    let words: Vec<&str> = selector.split_whitespace().collect();
    let pairs = words
        .chunks(2)
        .take_while(|p| p.len() == 2 && p[0] == "-p")
        .count();
    (pairs > 0).then(|| words[pairs * 2..].join(" "))
}

fn cargo_test(command: &str) -> bool {
    let runner = runner_argv(command);
    runner.first().map(String::as_str) == Some("cargo")
        && matches!(runner.get(1).map(String::as_str), Some("test" | "nextest"))
}

/// A cargo test selection's targets and its name filters, or None when it
/// has no name filter or one that is not a substring match.
fn cargo_filters(selector: &str) -> Option<(Vec<&str>, Vec<&str>)> {
    // The picks the checks rule records that take no value.
    const FLAGS: [&str; 7] = [
        "--lib",
        "--bins",
        "--tests",
        "--examples",
        "--benches",
        "--doc",
        "--ignored",
    ];
    let words: Vec<&str> = selector.split_whitespace().collect();
    // A nextest filter expression is not a name filter, and its words
    // may hold spaces.
    if words.iter().any(|w| {
        *w == "--exact"
            || ["-E", "--filter-expr", "--filterset"]
                .iter()
                .any(|o| w.starts_with(o))
    }) {
        return None;
    }
    let mut at = words.len()
        - words
            .iter()
            .rev()
            .take_while(|w| !w.starts_with('-'))
            .count();
    // `--bin=jouet` holds its own value.
    if at > 0 && !FLAGS.contains(&words[at - 1]) && !words[at - 1].contains('=') {
        at += 1;
    }
    let (head, filters) = words.split_at(at.min(words.len()));
    (!filters.is_empty()).then(|| (head.to_vec(), filters.to_vec()))
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
    /// Files a formatter or a commit's hooks rewrote, as the trees before
    /// and after, oldest first: what passed at the first counts at the
    /// second, so `cargo fmt` after the tests asks for no second run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formatted: Vec<(String, String)>,
    /// The failures blamed on the machine that a stop has noted (see
    /// [`noted_key`]).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub noted: BTreeSet<String>,
    /// Every reason the stop hook has raised here, each with the run it is
    /// about (its tree): a reason is raised once for that run, not again at
    /// each stop after files change, until a new run gives it a new key.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub told: BTreeSet<String>,
    /// The tree when such a call started, until it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatting: Option<String>,
    /// Whether only the session's subagents ran anything here, such as in
    /// a scratch repository of their own: the stop gate does not hold the
    /// lead to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subagents_only: bool,
    /// Whether the newest run here was a subagent's, as when the lead
    /// checked a subagent's work once and the subagent went on working: the
    /// tree is the subagent's to finish, as one only subagents ran in is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub last_by_subagent: bool,
}

/// How many rewrites [`Checks::formatted`] keeps.
const FORMATTED_KEPT: usize = 20;

/// Whether a Bash call only formats files or commits them: every command in
/// it is a formatter (`cargo fmt`, `prettier --write`, `hk fix`, ...), a
/// commit, whose hooks may fix what they check, `git add`, or a command that
/// leaves the files as they are: `git status`, `git diff`, `git log`, `git
/// push`, a new branch at the current commit, `cargo clippy` without `--fix`
/// (also through `jouet test --cmd --`), and `tail` or `head` in a pipe. The
/// script is parsed, so a commit message's newlines, `;` and backticks in
/// single quotes are words. A `cd` makes it something else, since the files
/// it changed are elsewhere.
pub(crate) fn formats(command: &str) -> bool {
    let script = jouet_rules::shell::parse(command);
    // A substitution runs a command of its own: `git status $(sed -i ...)`,
    // or with backticks, which the parser keeps as words.
    if !script.nested.is_empty() || command.contains("<(") || writes_or_substitutes(command) {
        return false;
    }
    !script.commands.is_empty()
        && script.commands.iter().all(|command| {
            // A redirection writes a file (`git status > src/a.rs`), and so
            // does a here-document's `cat`.
            command.heredocs.is_empty()
                && command
                    .stdout_to
                    .as_deref()
                    .is_none_or(|to| to == "/dev/null")
                && leaves_or_formats(command.argv())
        })
}

/// Whether `command` has a backtick the shell would substitute (one outside
/// single quotes: double quotes substitute it too), or a redirection that
/// writes a file other than `/dev/null`, from any descriptor: the parser
/// keeps only standard output's target.
fn writes_or_substitutes(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let (mut single, mut double) = (false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' if !single => i += 1,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '`' if !single => return true,
            // A comment runs to the end of its line, apostrophes and all.
            '#' if !single
                && !double
                && (i == 1
                    || chars[i - 2].is_whitespace()
                    || matches!(chars[i - 2], ';' | '&' | '|' | '(')) =>
            {
                while chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
            }
            '>' if !single && !double => {
                while chars.get(i).is_some_and(|c| matches!(c, '>' | '|')) {
                    i += 1;
                }
                // `>&2` and `>&-` duplicate or close a descriptor.
                if chars.get(i) == Some(&'&')
                    && chars
                        .get(i + 1)
                        .is_some_and(|c| c.is_ascii_digit() || *c == '-')
                {
                    continue;
                }
                if chars.get(i) == Some(&'&') {
                    i += 1;
                }
                while chars.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
                    i += 1;
                }
                let target: String = chars[i..]
                    .iter()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '&' | '|' | ')'))
                    .collect();
                if target != "/dev/null" {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// [`formats`] for one command's words.
fn leaves_or_formats(words: &[String]) -> bool {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let has = |flag: &str| words.contains(&flag);
    match words.as_slice() {
        ["cargo", "fmt", ..] | ["rustfmt", ..] | ["gofmt", ..] | ["go", "fmt", ..] => true,
        ["black", ..] | ["ruff", "format", ..] | ["hk", "fix", ..] => true,
        ["prettier", ..] | ["npx", "prettier", ..] => has("--write") || has("-w"),
        ["git", "commit", ..]
        | ["jouet", "commit", ..]
        | ["git", "add", ..]
        | ["git", "status", ..]
        | ["git", "push", ..] => true,
        ["git", "diff" | "show" | "log", ..] => !words.iter().any(|w| w.starts_with("--output")),
        // A new branch with no start point keeps the files.
        ["git", "checkout", "-q", "-b" | "-B", _]
        | ["git", "checkout", "-b" | "-B", _]
        | ["git", "switch", "-c" | "-C", _] => true,
        ["cargo", "clippy", ..] => !has("--fix"),
        ["jouet", "test", "--cmd", "--", rest @ ..] => {
            matches!(rest, ["cargo", "clippy", ..]) && !rest.contains(&"--fix")
        }
        ["tail", ..] | ["head", ..] | ["true"] => true,
        _ => false,
    }
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
    /// The keys of the reasons raised now, kept in [`Checks::told`].
    #[serde(skip)]
    pub told: Vec<String>,
    /// The unverifiable commands whose failure these reasons raise, which
    /// are then answered (see [`Checks::answered`]).
    #[serde(skip)]
    pub answering: Vec<String>,
}

/// The most reasons one stop raises; the rest wait for the next.
const MAX_RAISED: usize = 5;

/// What [`Checks::told`] keeps a reason by: what it is about and the run,
/// by its command, exit code and tree, and not the message, which names
/// the runs since; so only a newer run of it is a new key.
fn told_key(about: &str, run: &Run) -> String {
    format!(
        "{about}\u{0}{}\u{0}{}\u{0}{}",
        run.command,
        run.exit_code,
        run.tree.as_deref().unwrap_or("")
    )
}

/// A failure as [`Checks::noted`] keeps it: its command and tree.
fn noted_key(failed: &Run) -> String {
    format!(
        "{}\u{0}{}",
        failed.command,
        failed.tree.as_deref().unwrap_or("")
    )
}

/// A command as a message shows it: cut to 80 characters.
fn shown(command: &str) -> String {
    match command.char_indices().nth(80) {
        Some((i, _)) => format!("{}...", &command[..i]),
        None => command.to_string(),
    }
}

/// Why the stop hook blocks, from the checks of every work tree with
/// something to raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    /// The reasons, each tree's named when it is not the session's own.
    pub reason: String,
    /// What to keep in each tree's checks once the stop is blocked, by
    /// root.
    pub raised: Vec<(String, Raised)>,
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

/// Past this size a `--files0-from` list is not read.
const MAX_FILE_LIST: u64 = 1 << 20;

/// Records an hk check of an explicit file list (`files a b`, or
/// `files0-from LIST`) that ran in `dir` as a check of the changed files
/// when the list names every file changed in the work tree, which is what
/// hk without files would have checked. A list it cannot read, or a work
/// tree it cannot read, leaves the run partial.
pub(crate) fn widen_hk_files(record: &mut Record, dir: &Path, timeout: Duration) {
    // With `--cd`, hk reads the list from another directory.
    let moved = record
        .command
        .split_whitespace()
        .any(|w| w == "--cd" || w.starts_with("--cd="));
    if record.kind != Kind::Lint || !through_hk(&record.command) || moved {
        return;
    }
    let Some(selector) = record.selector.as_deref() else {
        return;
    };
    if listed_cover_changes(selector, dir, timeout) == Some(true) {
        record.selector = Some("changed files".into());
    }
}

/// Whether the files `selector` lists from `dir` include every file changed
/// in the work tree there; None when it lists nothing readable.
fn listed_cover_changes(selector: &str, dir: &Path, timeout: Duration) -> Option<bool> {
    let dir = dir.canonicalize().ok()?;
    let root = PathBuf::from(root_of(&dir));
    if root.as_os_str().is_empty() {
        return None;
    }
    let listed: BTreeSet<String> = listed_files(selector, &dir)?
        .iter()
        .filter_map(|file| relative_to(&root, &dir.join(file)))
        .collect();
    let changed = jouet_rules::tree::changed_paths(&root, timeout).ok()?;
    Some(changed.iter().all(|path| listed.contains(path)))
}

/// The files an hk selector names, as given: `files a b` with nothing that
/// narrows it, or the NUL-separated list in `files0-from LIST`, which
/// counts itself so that a list kept in the work tree is not a file left
/// unchecked.
fn listed_files(selector: &str, dir: &Path) -> Option<Vec<String>> {
    if let Some(files) = selector.strip_prefix("files ") {
        let files: Vec<String> = files.split_whitespace().map(str::to_string).collect();
        let narrowed = files
            .iter()
            .any(|f| f.starts_with('-') || f == "files0-from");
        return (!narrowed).then_some(files);
    }
    let list = selector.strip_prefix("files0-from ")?;
    if list.contains(" -") {
        return None;
    }
    let path = dir.join(list);
    if std::fs::metadata(&path).ok()?.len() > MAX_FILE_LIST {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let mut files: Vec<String> = text
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect();
    files.push(list.to_string());
    Some(files)
}

/// `path` relative to `root`, `/`-separated, with `.` and `..` resolved by
/// name; None outside it.
fn relative_to(root: &Path, path: &Path) -> Option<String> {
    use std::path::Component;
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    let parts: Option<Vec<&str>> = clean
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect();
    Some(parts?.join("/"))
}

impl State {
    /// Adds runs that ran in the work tree at `root`, against `tree`.
    pub(crate) fn record(
        &mut self,
        root: &str,
        records: impl IntoIterator<Item = Record>,
        tree: Option<&str>,
    ) {
        // The runner under `mise run TASK`, read from the tasks only for a
        // partial run, the kind a name above can cover.
        let mut tasks = None;
        let mut task_nesting = |command: &str| {
            let [name] = crate::wind::tasks::task_runs(command).try_into().ok()?;
            let tasks = tasks
                .get_or_insert_with(|| crate::mise::tasks(Path::new(root)).unwrap_or_default());
            Nesting::of_task(tasks, &name)
        };
        let records: Vec<(Record, Option<Nesting>)> = records
            .into_iter()
            .map(|record| {
                let nesting = (!record.full)
                    .then(|| task_nesting(&record.command))
                    .flatten();
                (record, nesting)
            })
            .collect();
        self.trees
            .entry(root.to_string())
            .or_default()
            .record_nested(records, tree);
    }

    /// Notes who is running something in the work tree at `root`: the
    /// session's own agent (`lead`), or one of its subagents. A tree only
    /// subagents ran in stays theirs until the lead runs something there.
    pub(crate) fn by(&mut self, root: &str, lead: bool) {
        let new = !self.trees.contains_key(root);
        let checks = self.checks_mut(root);
        checks.last_by_subagent = !lead;
        if lead {
            checks.subagents_only = false;
        } else if new {
            checks.subagents_only = true;
        }
    }

    /// The checks run in the work tree at `root`, made when there are none.
    pub(crate) fn checks_mut(&mut self, root: &str) -> &mut Checks {
        self.trees.entry(root.to_string()).or_default()
    }

    /// The checks run in the work tree at `root`.
    pub(crate) fn checks(&self, root: &str) -> Option<&Checks> {
        self.trees.get(root)
    }

    /// Why stopping should wait, in any work tree the session ran checks
    /// in: `home`, the session's own, first, then the others, each named.
    /// Trees only subagents ran in are left out, except `home`: a subagent's
    /// runs count as evidence, but its scratch repositories are its own.
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
        let others = self.trees.iter().filter(|(root, checks)| {
            root.as_str() != home && !checks.subagents_only && !checks.last_by_subagent
        });
        // Every tree's at once: a tree left for a later stop would be raised
        // at the end of a turn that never touched it.
        let found: Vec<(String, String, Raised)> = own
            .into_iter()
            .chain(others)
            .filter_map(|(root, checks)| {
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
                Some((root.clone(), reason, raised))
            })
            .collect();
        if found.is_empty() {
            return None;
        }
        Some(Block {
            reason: found
                .iter()
                .map(|(_, r, _)| r.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
            raised: found
                .into_iter()
                .map(|(root, _, raised)| (root, raised))
                .collect(),
        })
    }

    /// Dates the newest run in the work tree at `root` to `at`, for runs
    /// recorded after the fact, such as a Codex transcript's.
    pub(crate) fn ran_at(&mut self, root: &str, at: i64) {
        if let Some(checks) = self.trees.get_mut(root) {
            checks.last_run = Some(at);
        }
    }

    /// Drops the checks of work trees that are gone, such as a removed
    /// worktree: nothing is left there to check.
    pub(crate) fn prune_gone(&mut self) {
        self.trees
            .retain(|root, _| root.is_empty() || Path::new(root).is_dir());
    }

    /// Sets aside the checks of work trees the session has left: any but
    /// `home`, the session's own, where `uncommitted` says nothing is
    /// uncommitted (`Some(false)`; None when it cannot tell) and the session
    /// ran nothing in the [`SETTLED_AFTER`] before `now`, or `at_base` says
    /// the tree is its base exactly, so no change of the session's is left.
    pub(crate) fn set_aside_settled(
        &mut self,
        home: &str,
        now: i64,
        uncommitted: impl Fn(&str) -> Option<bool>,
        at_base: impl Fn(&str) -> bool,
    ) {
        self.trees.retain(|root, checks| {
            if root == home || root.is_empty() || uncommitted(root) != Some(false) {
                return true;
            }
            let recent = checks
                .last_run
                .is_some_and(|at| now.saturating_sub(at) < SETTLED_AFTER);
            recent && !at_base(root)
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
        for (root, raised) in &block.raised {
            let checks = self.trees.entry(root.clone()).or_default();
            // The keys go to `told`; the kept copy is what a file reads back.
            checks.raised = Some(Raised {
                told: Vec::new(),
                ..raised.clone()
            });
            checks.answered.extend(raised.answering.iter().cloned());
            checks.told.extend(raised.told.iter().cloned());
        }
    }

    /// Marks the newest failure of `command` in the work tree at `root` as
    /// the machine's, by the line of its output that says so.
    pub(crate) fn mark_environment(&mut self, root: &str, command: &str, cause: &str) {
        let Some(checks) = self.trees.get_mut(root) else {
            return;
        };
        for failures in checks.failures.values_mut() {
            if let Some(failed) = failures.iter_mut().rev().find(|f| f.command == command) {
                failed.environment = Some(cause.to_string());
            }
        }
        for run in checks.runs.values_mut() {
            if run.command == command && !run.passed() {
                run.environment = Some(cause.to_string());
            }
        }
    }

    /// Records `tests` as the failing tests of the newest failure of
    /// `command` in the work tree at `root`.
    pub(crate) fn mark_failed_tests(&mut self, root: &str, command: &str, tests: &[String]) {
        let Some(checks) = self.trees.get_mut(root) else {
            return;
        };
        for failures in checks.failures.values_mut() {
            if let Some(failed) = failures.iter_mut().rev().find(|f| f.command == command) {
                failed.tests = tests.to_vec();
            }
        }
        // The newest run is the failure itself, and the stop says so only
        // while the two are equal.
        for run in checks.runs.values_mut() {
            if run.command == command && !run.passed() {
                run.tests = tests.to_vec();
            }
        }
    }

    /// Marks the newest failure of `command` in the work tree at `root` as
    /// one in tests the change does not reach, which `tests` names.
    pub(crate) fn mark_unrelated(&mut self, root: &str, command: &str, tests: &str) {
        let Some(checks) = self.trees.get_mut(root) else {
            return;
        };
        for failures in checks.failures.values_mut() {
            if let Some(failed) = failures.iter_mut().rev().find(|f| f.command == command) {
                failed.unrelated = Some(tests.to_string());
            }
        }
        for run in checks.runs.values_mut() {
            if run.command == command && !run.passed() {
                run.unrelated = Some(tests.to_string());
            }
        }
    }

    /// One line for each failure the stop let through as the machine's, or
    /// as in tests the change does not reach, in any work tree, for the person to see, each once: those already said
    /// are kept in [`Checks::noted`] by [`State::noted`].
    pub(crate) fn environment_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        for (root, checks) in &self.trees {
            for failed in checks.failures.values().flatten() {
                if checks.noted.contains(&noted_key(failed)) {
                    continue;
                }
                let why = match (&failed.environment, &failed.unrelated) {
                    (Some(cause), _) => {
                        format!("its failures blame the machine, not the change ({cause})")
                    }
                    (None, Some(tests)) => {
                        format!("its failing tests are ones the change does not reach ({tests})")
                    }
                    (None, None) => continue,
                };
                let at = if root.is_empty() {
                    String::new()
                } else {
                    format!(" in {root}")
                };
                let note = format!(
                    "jouet did not hold up finishing for `{}`{at}: {why}.",
                    shown(&failed.command)
                );
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
        notes
    }

    /// Keeps the failures [`State::environment_notes`] gave as
    /// said, so a later stop does not say them again.
    pub(crate) fn noted(&mut self) {
        for checks in self.trees.values_mut() {
            let keys: Vec<String> = checks
                .failures
                .values()
                .flatten()
                .filter(|f| f.environment.is_some() || f.unrelated.is_some())
                .map(noted_key)
                .collect();
            checks.noted.extend(keys);
        }
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
    /// Adds runs that ran against `tree`, through no task.
    #[cfg(test)]
    pub(crate) fn record(&mut self, records: impl IntoIterator<Item = Record>, tree: Option<&str>) {
        self.record_nested(records.into_iter().map(|r| (r, None)), tree);
    }

    /// Adds runs that ran against `tree`, each with the nesting of a runner
    /// its command reaches through a task.
    fn record_nested(
        &mut self,
        records: impl IntoIterator<Item = (Record, Option<Nesting>)>,
        tree: Option<&str>,
    ) {
        for (record, nesting) in records {
            // A run is something new to judge the next stop by.
            self.raised = None;
            self.last_run = Some(jouet_output::time::now());
            let kind = record.kind;
            let mut run = Run::new(record, tree);
            run.nesting = run.nesting.or(nesting);
            let failures = self.failures.entry(kind).or_default();
            // `jouet test` records its own run, and the harness's hook the same
            // call after it: the same failure at the same tree keeps what
            // `jouet test` found about its tests.
            if !run.passed()
                && run.tree.is_some()
                && let Some(earlier) = failures.iter().rev().find(|f| {
                    f.command == run.command && f.tree == run.tree && f.exit_code == run.exit_code
                })
            {
                run.unrelated = earlier.unrelated.clone();
                run.tests = earlier.tests.clone();
            }
            if run.passed() && run.full {
                // A full runner or task can cover a different package than a
                // scoped pnpm run. Keep that failure unless coverage is known.
                failures.retain(|f| f.pnpm_scoped() && !run.covers(f));
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
            run.passed()
                && run.covers_changes()
                && self.same_files(run.tree.as_deref(), now().as_deref())
        })
    }

    /// Whether the files at `now` are those at `ran`, or what formatting or
    /// a commit's hooks made of them since. An unknown tree is never the
    /// same.
    /// Whether `now` is `ran`, or a formatting of it, but for prose: as
    /// [`Self::same_files`], comparing the trees' code parts.
    fn same_code(&self, ran: Option<&str>, now: Option<&str>) -> bool {
        use jouet_rules::tree::code_part;
        let (Some(ran), Some(mut at)) = (ran, now) else {
            return false;
        };
        for _ in 0..=self.formatted.len() {
            if code_part(at) == code_part(ran) {
                return true;
            }
            // A formatting from a tree with the same code, prose aside.
            match self
                .formatted
                .iter()
                .rev()
                .find(|(_, to)| code_part(to) == code_part(at))
            {
                Some((from, _)) => at = from,
                None => return false,
            }
        }
        false
    }

    fn same_files(&self, ran: Option<&str>, now: Option<&str>) -> bool {
        let (Some(ran), Some(mut at)) = (ran, now) else {
            return false;
        };
        for _ in 0..=self.formatted.len() {
            if at == ran {
                return true;
            }
            match self.formatted.iter().rev().find(|(_, to)| to == at) {
                Some((from, _)) => at = from,
                None => return false,
            }
        }
        false
    }

    /// A call that formats or commits ([`formats`]) started, at `tree`.
    pub(crate) fn formatting_started(&mut self, tree: Option<&str>) {
        self.formatting = tree.map(str::to_string);
    }

    /// That call ended, at `tree`: what it changed was formatting.
    pub(crate) fn formatting_ended(&mut self, tree: Option<&str>) {
        if let (Some(from), Some(to)) = (self.formatting.take(), tree) {
            self.reformatted(&from, to);
        }
    }

    /// The files went from `from` to `to` by formatting alone, as a
    /// commit's hooks report it, without ending a formatting call that
    /// may still be in flight around it (`cargo fmt && jouet commit`).
    pub(crate) fn reformatted(&mut self, from: &str, to: &str) {
        if from != to {
            self.formatted.push((from.to_string(), to.to_string()));
            let over = self.formatted.len().saturating_sub(FORMATTED_KEPT);
            self.formatted.drain(..over);
        }
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
        let reasons = self.keyed_reasons(now, hk_repo, affected);
        let tree = now();
        let mut raised = match &self.raised {
            Some(raised) if tree.is_some() && raised.tree == tree => raised.reasons.clone(),
            _ => Vec::new(),
        };
        // Two runs whose commands are cut to the same text read as one
        // reason, so they are raised, and counted, as one.
        // A reason already told for the run it is about is not raised again:
        // not at the next stop after files change, nor at one ending a turn
        // that never touched them. A new run of it is a new key.
        let mut new: Vec<String> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        for (reason, key) in reasons {
            if self.told.contains(&key) {
                continue;
            }
            if !raised.contains(&reason) && !new.contains(&reason) {
                new.push(reason);
                keys.push(key);
            }
        }
        if new.is_empty() {
            return None;
        }
        let more = new.len().saturating_sub(MAX_RAISED);
        let new: Vec<String> = new.into_iter().take(MAX_RAISED).collect();
        keys.truncate(MAX_RAISED);
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
            told: keys,
        })
    }

    /// Every reason stopping should wait, the first to raise first: each
    /// failure nothing has cleared, files changed after the last test run
    /// passed with none passing since, and, in a repository with an hk
    /// config (`hk_repo`), files changed since the last hk check passed.
    /// `tree` gives the work tree now; it is only asked for when one of the
    /// last two could apply. `affected` is whether `jouet test --affected`
    /// can select tests there, for the message.
    #[cfg(test)]
    pub(crate) fn reasons(
        &self,
        tree: impl Fn() -> Option<String>,
        hk_repo: bool,
        affected: bool,
    ) -> Vec<String> {
        self.keyed_reasons(tree, hk_repo, affected)
            .into_iter()
            .map(|(reason, _)| reason)
            .collect()
    }

    /// [`Checks::reasons`], each with its key: the reason and the tree of
    /// the run it is about.
    fn keyed_reasons(
        &self,
        tree: impl Fn() -> Option<String>,
        hk_repo: bool,
        affected: bool,
    ) -> Vec<(String, String)> {
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

    fn hk_reason(
        &self,
        tree: &impl Fn() -> Option<String>,
        hk_repo: bool,
    ) -> Option<(String, String)> {
        let hk = self.hk.as_ref().filter(|_| hk_repo)?;
        if !hk.passed() || hk.tree.is_none() {
            return None;
        }
        if !hk.covers_changes() {
            let keyed = |reason| (reason, told_key("hk-partial", hk));
            return Some(keyed(format!(
                "The last hk check passed for only some files (`{}`). Run `jouet diag check --changed` before you finish, or say why it isn't needed. [jouet: verify]",
                shown(&hk.command)
            )));
        }
        if self.same_files(hk.tree.as_deref(), tree().as_deref()) {
            return None;
        }
        let keyed = |reason| (reason, told_key("hk-changed", hk));
        Some(keyed(format!(
            "Files changed after the last hk check passed (`{}`). Run `jouet diag check --changed` before you finish, or say why it isn't needed. [jouet: verify]",
            shown(&hk.command)
        )))
    }

    /// Files other than prose changed after the newest test run passed, and
    /// no test run has passed at the files now. Any passing run counts, a partial one too:
    /// the tests a change affects are enough here, as the full suite runs
    /// on CI. A failed newest run is a failure reason instead.
    fn untested_reason(
        &self,
        tree: &impl Fn() -> Option<String>,
        affected: bool,
    ) -> Option<(String, String)> {
        let last = self.runs.get(&Kind::Test)?;
        if !last.passed() || last.tree.is_none() {
            return None;
        }
        let now = tree()?;
        // A change to prose alone (docs, a changelog) is one no test run
        // needs to follow, formatting after the pass included.
        let passed_now = self.passes.get(&Kind::Test).is_some_and(|passes| {
            passes
                .iter()
                .any(|p| self.same_code(p.tree.as_deref(), Some(now.as_str())))
        });
        if passed_now {
            return None;
        }
        let run = if affected {
            "`jouet test --affected`, or the tests for the code you changed"
        } else {
            "the tests for the code you changed"
        };
        let reason = format!(
            "Files changed after the last test run passed (`{}`). Run the tests your change affects ({run}) before you finish, or say why none are needed. [jouet: verify]",
            shown(&last.command)
        );
        let key = told_key("untested", last);
        Some((reason, key))
    }

    /// A reason for each failure nothing has cleared, by kind, and within a
    /// kind full failures first, as they say the most, newest first.
    ///
    /// A failure at another tree than now does not count when a pass that
    /// would clear it ran at the tree now: the files are back to ones that
    /// passed, as after checking that a test fails without a fix and putting
    /// the fix back. `tree` is only asked for when there is a failure.
    fn failure_reasons(
        &self,
        tree: &impl Fn() -> Option<String>,
        skip_tests: bool,
    ) -> Vec<(String, String)> {
        let cell = std::cell::OnceCell::new();
        let now = || cell.get_or_init(tree).clone();
        let passed_now = |kind: &Kind, failed: &Run| {
            failed.tree.is_some()
                && now().is_some_and(|now| {
                    failed.tree.as_deref() != Some(now.as_str())
                        && self.passes.get(kind).is_some_and(|passes| {
                            passes.iter().any(|p| {
                                p.tree.as_deref() == Some(now.as_str())
                                    && ((p.full && !failed.pnpm_scoped())
                                        || (!failed.full && p.covers(failed)))
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
                    // The machine's, or in tests the change does not reach,
                    // not the change's: a note, not a block.
                    .filter(|failed| failed.environment.is_none() && failed.unrelated.is_none())
                    .map(|failed| {
                        let reason = self.failure_reason(*kind, failed);
                        let key = told_key(kind.name(), failed);
                        (reason, key)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn failure_reason(&self, kind: Kind, failed: &Run) -> String {
        if self.unverifiable.contains(&same_run(&failed.command)) {
            return format!(
                "The last {} run failed (`{}` exited {}), and the command also failed at the merge base, so jouet cannot tell whether its failures were there before your change. Say which failures, if any, your change caused before you finish; this is asked once. [jouet: verify]",
                kind.name(),
                shown(&failed.command),
                failed.exit_code
            );
        }
        // A run that fails for reasons the change did not touch, such as a
        // tool this machine lacks, is the costly case: point at the check
        // that settles it. Its result is kept, so later runs of the same
        // command compare with it without running the merge base again.
        // Most partial test runs are written to fail, but an affected run
        // often includes existing tests selected by the change.
        let affected = failed.selection().split_whitespace().next() == Some("--affected");
        let baseline = if kind == Kind::Build || (kind == Kind::Test && (failed.full || affected)) {
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
        let clear = if failed.pnpm_scoped() && failed.selection().contains("@jouet:cwd=unknown:") {
            "its original working directory is unknown, so jouet cannot safely match it to another pnpm run".to_string()
        } else if failed.pnpm_scoped() {
            "a passing run of the same supported pnpm selection, or a covering unfiltered Vitest run in the same scope, clears it".to_string()
        } else if failed.full {
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

/// A command a Codex transcript says finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    pub command: String,
    pub exit_code: i32,
    /// The command's own working directory, when recorded by Codex.
    pub cwd: Option<String>,
    /// When, from the line's `timestamp`, in seconds since the Unix epoch.
    pub at: Option<i64>,
    /// A failed command's output, as Codex recorded it, so a failure that
    /// blames the machine can be told apart as it is for other harnesses.
    pub output: Option<String>,
}

/// The finished commands at the end of a Codex session transcript, oldest
/// first, with their exit codes.
///
/// The transcript format is Codex's own and undocumented; lines this does not
/// recognise are skipped, so a format change means no blocking, not false
/// blocking.
pub(crate) fn codex_commands(path: &Path) -> Vec<Ran> {
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
fn normalize_cwd_separators(path: &str, windows: bool) -> String {
    if windows {
        // Git reports worktree roots with forward slashes on Windows.
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}

fn absolute_cwd(path: &str) -> Option<String> {
    let normalized = normalize_cwd_separators(path, cfg!(windows));
    std::path::Path::new(&normalized)
        .is_absolute()
        .then_some(normalized)
}

/// Codex records working directories as absolute paths or local file URIs.
/// Reject remote, malformed, and non-UTF-8 URIs rather than guessing a scope.
fn codex_cwd(cwd: &str) -> Option<String> {
    if !cwd.starts_with("file:") {
        return absolute_cwd(cwd);
    }
    let rest = cwd.strip_prefix("file://")?;
    let path = if rest.starts_with('/') {
        rest
    } else {
        rest.strip_prefix("localhost")?
    };
    if !path.starts_with('/') || path.contains(['?', '#']) {
        return None;
    }
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit()
            {
                return None;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    if decoded.contains('\0') {
        return None;
    }
    let decoded: &str = &decoded;
    #[cfg(windows)]
    let decoded = if decoded
        .as_bytes()
        .get(1)
        .is_some_and(u8::is_ascii_alphabetic)
        && decoded.as_bytes().get(2) == Some(&b':')
        && decoded.as_bytes().get(3) == Some(&b'/')
    {
        &decoded[1..]
    } else {
        return None;
    };
    absolute_cwd(decoded)
}

fn codex_command(line: &str) -> Option<Ran> {
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
    let output = (exit_code != 0)
        .then(|| event.get("aggregated_output").and_then(|o| o.as_str()))
        .flatten()
        .map(str::to_string);
    Some(Ran {
        command: script,
        exit_code,
        output,
        cwd: event
            .get("cwd")
            .and_then(|cwd| cwd.as_str())
            .and_then(codex_cwd),
        at: value
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(jouet_output::time::parse_rfc3339),
    })
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
        let mut affected = Checks::default();
        affected.record(
            [test(
                "jouet test --affected --since main",
                1,
                "--affected --since main",
            )],
            Some("t1"),
        );
        let why = reason(&affected, "t1").unwrap();
        assert!(
            why.contains("`jouet test --baseline --affected --since main`"),
            "{why}"
        );
        affected.record(
            [test(
                "jouet test --baseline --affected --since main",
                0,
                "--affected --since main",
            )],
            Some("t1"),
        );
        assert_eq!(reason(&affected, "t1"), None);
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
    fn scoped_pnpm_exclusions_do_not_clear_other_checks() {
        let mut state = Checks::default();
        state.record(
            [test("pnpm --dir web test", 1, "@jouet:pnpm:dir=web:test")],
            Some("t1"),
        );
        for (command, scope) in [
            (
                "pnpm --dir web exec vitest run --exclude tests/b.test.ts",
                "@jouet:pnpm:dir=web:vitest --exclude tests/b.test.ts",
            ),
            ("pnpm --dir admin test", "@jouet:pnpm:dir=admin:test"),
        ] {
            state.record([test(command, 0, scope)], Some("t1"));
            assert!(
                reason(&state, "t1")
                    .unwrap()
                    .contains("pnpm --dir web test")
            );
        }
        state.record(
            [test("pnpm --dir web test", 0, "@jouet:pnpm:dir=web:test")],
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);

        let mut vitest = Checks::default();
        vitest.record(
            [test(
                "pnpm --dir web exec vitest run tests/b.test.ts",
                1,
                "@jouet:pnpm:dir=web:vitest tests/b.test.ts",
            )],
            Some("t1"),
        );
        vitest.record(
            [test(
                "pnpm --dir web exec vitest run --exclude tests/b.test.ts",
                0,
                "@jouet:pnpm:dir=web:vitest --exclude tests/b.test.ts",
            )],
            Some("t1"),
        );
        assert!(reason(&vitest, "t1").unwrap().contains("tests/b.test.ts"));
        vitest.record(
            [test(
                "pnpm --dir web exec vitest run",
                0,
                "@jouet:pnpm:dir=web:vitest",
            )],
            Some("t1"),
        );
        assert_eq!(reason(&vitest, "t1"), None);

        let mut different_cwd = Checks::default();
        different_cwd.record(
            [test(
                "pnpm --dir web test",
                1,
                "@jouet:cwd=/repo/a:cd=. @jouet:pnpm:dir=web:test",
            )],
            Some("t1"),
        );
        different_cwd.record(
            [test(
                "pnpm --dir web test",
                0,
                "@jouet:cwd=/repo/b:cd=. @jouet:pnpm:dir=web:test",
            )],
            Some("t1"),
        );
        assert!(
            reason(&different_cwd, "t1")
                .unwrap()
                .contains("pnpm --dir web test")
        );
    }

    #[test]
    fn an_unfiltered_pnpm_vitest_run_clears_only_its_excluded_suite() {
        let mut state = Checks::default();
        let prefix = "@jouet:cwd=/repo:cd=. @jouet:pnpm:dir=web:vitest";
        state.record(
            [test(
                "pnpm --dir web exec vitest run --exclude=tests/b",
                1,
                &format!("{prefix} --exclude=tests/b"),
            )],
            Some("t1"),
        );
        state.record(
            [test("pnpm --dir web exec vitest run", 0, prefix)],
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);

        for filter in [
            "--root=other",
            "--config=other",
            "--typecheck.only",
            "--unknown",
        ] {
            let mut state = Checks::default();
            state.record(
                [test(
                    "pnpm --dir web exec vitest run filtered",
                    1,
                    &format!("{prefix} {filter}"),
                )],
                Some("t1"),
            );
            state.record(
                [test("pnpm --dir web exec vitest run", 0, prefix)],
                Some("t1"),
            );
            assert!(reason(&state, "t1").is_some(), "must retain {filter}");
        }
        assert!(!pnpm_vitest_covers(
            "@jouet:cwd=/repo/a:cd=. @jouet:pnpm:dir=web:vitest",
            "@jouet:cwd=/repo/b:cd=. @jouet:pnpm:dir=web:vitest --exclude tests/b"
        ));
    }

    #[test]
    fn selected_pnpm_runs_do_not_clear_unfiltered_failures() {
        for runner in ["vitest", "test"] {
            let prefix = format!("@jouet:cwd=/repo:cd=. @jouet:pnpm:dir=web:{runner}");
            let mut checks = Checks::default();
            checks.record([test("pnpm test", 1, &prefix)], Some("t1"));
            checks.record(
                [test(
                    "pnpm test a.test.ts",
                    0,
                    &format!("{prefix} a.test.ts"),
                )],
                Some("t1"),
            );
            assert!(reason(&checks, "t1").is_some(), "{runner}");
        }
        let mut checks = Checks::default();
        let unknown = "@jouet:cwd=unknown:cd=. @jouet:pnpm:dir=web:test";
        checks.record([test("pnpm test", 1, unknown)], Some("t1"));
        checks.record([test("pnpm test", 0, unknown)], Some("t1"));
        assert!(reason(&checks, "t1").is_some());
    }

    #[test]
    fn unfiltered_pnpm_vitest_clears_name_filtered_failures() {
        let prefix = "@jouet:cwd=/repo:cd=. @jouet:pnpm:dir=web:vitest";
        for filter in [
            "-t case",
            "--testNamePattern case",
            "--testNamePattern=case",
        ] {
            let mut checks = Checks::default();
            checks.record(
                [test(
                    "pnpm exec vitest run -t case",
                    1,
                    &format!("{prefix} {filter}"),
                )],
                Some("t1"),
            );
            checks.record([test("pnpm exec vitest run", 0, prefix)], Some("t1"));
            assert_eq!(reason(&checks, "t1"), None, "{filter}");
        }
    }

    #[test]
    fn a_full_unrelated_run_cannot_clear_a_scoped_pnpm_failure() {
        let selection = "@jouet:cwd=/repo:cd=. @jouet:pnpm:dir=web:test";
        for (failure_tree, pass_tree) in [("same", "same"), ("old", "new")] {
            let mut checks = Checks::default();
            checks.record(
                [test("pnpm --dir web test", 1, selection)],
                Some(failure_tree),
            );
            checks.record([test("vitest run", 0, "full")], Some(pass_tree));
            let why = reason(&checks, pass_tree).expect("scoped failure must remain");
            assert!(why.contains("same supported pnpm selection"), "{why}");
            checks.record([test("pnpm --dir web test", 0, selection)], Some(pass_tree));
            assert_eq!(reason(&checks, pass_tree), None);
        }
        let mut checks = Checks::default();
        checks.record([test("cargo test -p app", 1, "-p app")], Some("t1"));
        checks.record([test("cargo test", 0, "full")], Some("t1"));
        assert_eq!(reason(&checks, "t1"), None);
    }

    #[test]
    fn an_affected_baseline_does_not_clear_a_different_tree() {
        let mut checks = Checks::default();
        checks.record(
            [test("jouet test --affected", 1, "--affected")],
            Some("old"),
        );
        checks.record(
            [test("jouet test --baseline --affected", 0, "--affected")],
            Some("new"),
        );
        assert_eq!(checks.failures[&Kind::Test].len(), 1);
        checks.record(
            [test("jouet test --baseline --affected", 0, "--affected")],
            Some("old"),
        );
        assert!(!checks.failures.contains_key(&Kind::Test));
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
    fn a_wider_go_run_clears_a_narrower_failure() {
        let cleared = |failed: (&str, &str), passed: (&str, &str)| {
            let mut state = Checks::default();
            state.record([test(failed.0, 1, failed.1)], Some("t1"));
            state.record([test(passed.0, 0, passed.1)], Some("t1"));
            reason(&state, "t1").is_none()
        };
        let narrow = (
            "go test ./forge/entire ./internaldomain",
            "./forge/entire ./internaldomain",
        );
        // A `dir/...` pattern runs every package under it.
        assert!(cleared(
            narrow,
            (
                "go test ./forge/... ./internaldomain",
                "./forge/... ./internaldomain"
            )
        ));
        assert!(cleared(narrow, ("go test -count=1 ./...", "./...")));
        // `./...` holds only the packages under `.`, not import paths,
        // `all` or `std`, or a package above it.
        for outside in ["..", "./../x", "all", "std", "github.com/a/b"] {
            assert!(!cleared(
                (
                    &format!("go test -run TestA {outside}"),
                    &format!("{outside} -run TestA")
                ),
                ("go test -run TestA ./...", "./... -run TestA"),
            ));
        }
        // But not a sibling of it, or a package left out.
        assert!(!cleared(
            narrow,
            (
                "go test ./forge/entirex/... ./internaldomain",
                "./forge/entirex/... ./internaldomain"
            )
        ));
        assert!(!cleared(narrow, ("go test ./forge/...", "./forge/...")));
        // No filter covers a `-run`; a `-run` alternation that holds the name does.
        let one = (
            "go test --run TestPusher_ForwardsToken ./forge",
            "./forge --run TestPusher_ForwardsToken",
        );
        assert!(cleared(one, ("go test ./forge/...", "./forge/...")));
        assert!(cleared(
            one,
            (
                "go test --run TestPolicy|TestPusher ./forge",
                "./forge --run TestPolicy|TestPusher"
            )
        ));
        assert!(!cleared(
            one,
            (
                "go test --run TestPolicy|TestPull ./forge",
                "./forge --run TestPolicy|TestPull"
            )
        ));
        // A regular expression in the name is not read as a plain one.
        let anchored = (
            "go test --run ^TestPusher$ ./forge",
            "./forge --run ^TestPusher$",
        );
        assert!(!cleared(
            anchored,
            (
                "go test --run TestPolicy|TestPusher ./forge",
                "./forge --run TestPolicy|TestPusher"
            )
        ));
        // The same holds for other runners' unanchored name filters.
        assert!(cleared(
            ("bats t.bats -f install_a", "t.bats -f install_a"),
            ("bats t.bats -f install|remove", "t.bats -f install|remove"),
        ));
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
    fn files_a_formatter_rewrote_after_a_pass_count_as_tested() {
        let mut state = Checks::default();
        state.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        state.formatting_started(Some("t1"));
        state.formatting_ended(Some("t2"));
        assert_eq!(reason(&state, "t2"), None);
        // An edit after that is not formatting.
        assert!(reason(&state, "t3").is_some());
        // Nor are files a formatter rewrote that had not passed.
        state.formatting_started(Some("t3"));
        state.formatting_ended(Some("t4"));
        assert!(reason(&state, "t4").is_some());
        // A formatter that changed nothing keeps nothing.
        state.formatting_started(Some("t4"));
        state.formatting_ended(Some("t4"));
        assert_eq!(state.formatted.len(), 2);
    }

    #[test]
    fn a_commits_own_reformat_keeps_the_formatting_call_around_it() {
        // `cargo fmt && jouet commit`: the formatter's call spans t1 to t3,
        // and the commit's hooks rewrote t2 to t3 inside it.
        let mut state = Checks::default();
        state.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        state.formatting_started(Some("t1"));
        state.reformatted("t2", "t3");
        state.formatting_ended(Some("t3"));
        assert_eq!(reason(&state, "t3"), None);
    }

    #[test]
    fn only_formatting_and_committing_calls_count_as_formatting() {
        for yes in [
            "cargo fmt",
            "cargo fmt --all 2>&1 | tail -3",
            "mise x -- cargo fmt -p a && jouet commit src/a.rs --message m",
            "prettier --write src && git add src/a.ts && git commit -qm m",
            "hk fix; git status --short",
            // A commit message's lines, `;` and backticks are its words.
            "cargo fmt && jouet commit src/a.rs --message \"fix(a): keep b; and c\n\nCo-Authored-By: X <x@y>\"",
            "jouet commit src/a.rs --message 'fix: `b` stays'",
            "cargo fmt && git diff --stat && jouet commit src/a.rs --message m",
            "cargo fmt --all && cargo fmt --all -- --check | head -2; git diff --stat | tail -3; (jouet commit a --message m 2>&1 | tail -4)",
            "cargo fmt -p a 2>&1 | tail -3; git status --short; jouet test --cmd -- cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -4",
            "git checkout -q -b fix/a 2>&1; jouet commit src/a.rs --message m | tail -2; git push -u origin HEAD 2>&1 | tail -2",
            "git commit -q -m m > /dev/null 2>&1",
            "cargo fmt # keep a#b and 'c'\ngit status",
            "git log --oneline -3 2>/dev/null; cargo fmt >&2",
            "jouet commit a --message \"keep > and 'b' as words\"",
        ] {
            assert!(formats(yes), "{yes}");
        }
        for no in [
            "cargo fmt && cargo test",
            "cargo clippy --fix --allow-dirty",
            "jouet test --cmd -- cargo test",
            "git checkout -b fix/a origin/main",
            "git checkout src/a.rs",
            "git diff --output=src/a.rs",
            "cat > src/a.rs <<'EOF'\nfn main() {}\nEOF\ncargo fmt",
            "jouet commit a --message \"$(sed -i s/a/b/ src/main.rs)\"",
            "prettier --check src",
            "cd sub && cargo fmt",
            "sed -i s/a/b/ src/a.rs && cargo fmt",
            "git status > src/main.rs",
            "git status $(sed -i s/a/b/ src/main.rs)",
            "git commit -m `sed -i s/a/b/ src/main.rs`",
            "cargo fmt &> out.txt",
            "git log -p --output=src/a.rs",
            "git status 2> src/a.rs",
            "cargo fmt 2>>src/a.rs",
            "cargo fmt # it's formatting\ngit status 2> src/a.rs",
            "jouet commit a --message \"it's `sed -i s/a/b/ src/main.rs`\"",
            "",
        ] {
            assert!(!formats(no), "{no}");
        }
    }

    #[test]
    fn a_failure_blamed_on_the_machine_holds_nothing_up_and_is_noted_once() {
        let mut state = State::default();
        state.record(
            "/home",
            [test("bats test/edit.bats", 1, "test/edit.bats")],
            Some("t1"),
        );
        state.mark_environment("/home", "bats test/edit.bats", "DBus error: no session bus");
        assert_eq!(
            state.block("/home", |_| Some("t1".into()), |_| false, |_| true),
            None
        );
        let notes = state.environment_notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("DBus error"), "{notes:?}");
        state.noted();
        assert!(state.environment_notes().is_empty());
        // A failure that does not blame the machine still blocks.
        state.record("/home", [test("cargo test -p a", 101, "-p a")], Some("t1"));
        assert!(
            state
                .block("/home", |_| Some("t1".into()), |_| false, |_| true)
                .is_some()
        );
    }

    #[test]
    fn a_failure_in_tests_the_change_does_not_reach_holds_nothing_up() {
        let mut state = State::default();
        state.record("/home", [test("go test ./...", 1, "")], Some("t1"));
        state.mark_unrelated(
            "/home",
            "go test ./...",
            "TestBroken in other/other_test.go",
        );
        assert_eq!(
            state.block("/home", |_| Some("t1".into()), |_| false, |_| true),
            None
        );
        let notes = state.environment_notes();
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("failing tests are ones the change does not reach (TestBroken in other/other_test.go)"),
            "{notes:?}"
        );
        state.noted();
        assert!(state.environment_notes().is_empty());
        // The harness's hook records the same call after `jouet test` did.
        state.record("/home", [test("go test ./...", 1, "")], Some("t1"));
        assert_eq!(
            state.block("/home", |_| Some("t1".into()), |_| false, |_| true),
            None
        );
        // At another tree, it is a new failure.
        state.record("/home", [test("go test ./...", 1, "")], Some("t2"));
        assert!(
            state
                .block("/home", |_| Some("t2".into()), |_| false, |_| true)
                .is_some()
        );
    }

    #[test]
    fn playwright_pass_does_not_clear_an_unrelated_full_test_failure() {
        let mut state = Checks::default();
        state.record([test("npm test", 1, "full")], Some("t1"));
        let cwd = std::env::current_dir().unwrap();
        let records = crate::cmd::hook::checks(&cwd, "npx playwright test");
        assert_eq!(records.len(), 1);
        assert!(!records[0].full);
        state.record(records, Some("t1"));
        assert!(reason(&state, "t1").is_some());
    }

    #[test]
    fn playwright_coverage_stays_with_its_runner_workspace_and_config() {
        let cwd = std::env::current_dir().unwrap();
        let run = |command: &str, code| {
            let mut records = crate::cmd::hook::checks(&cwd, command);
            assert_eq!(records.len(), 1, "{command}");
            records[0].exit_code = code;
            records
        };
        let mut state = Checks::default();
        state.record(run("cargo test playwright", 1), Some("t1"));
        state.record(run("npx playwright test", 0), Some("t1"));
        assert!(reason(&state, "t1").is_some());

        let mut state = Checks::default();
        state.record(run("pytest @jouet:playwright", 1), Some("t1"));
        state.record(run("npx playwright test", 0), Some("t1"));
        assert!(reason(&state, "t1").is_some());

        let mut state = Checks::default();
        state.record(
            run("pnpm exec playwright test --grep balance", 1),
            Some("t1"),
        );
        state.record(
            run("jouet test --cmd -- npx playwright test", 0),
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);

        let mut state = Checks::default();
        state.record(
            run("yarn workspace web exec playwright test", 1),
            Some("t1"),
        );
        state.record(run("npx playwright test", 0), Some("t1"));
        assert!(reason(&state, "t1").is_some());

        let mut state = Checks::default();
        state.record(
            run("npx playwright test --config=other.config.ts", 1),
            Some("t1"),
        );
        state.record(run("npx playwright test", 0), Some("t1"));
        assert!(reason(&state, "t1").is_some());

        let mut state = Checks::default();
        state.record(
            run("npx playwright test --config=other.config.ts a.spec.ts", 1),
            Some("t1"),
        );
        state.record(
            run("pnpm exec playwright test --config=other.config.ts", 0),
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);

        let mut state = Checks::default();
        state.record(
            run("npx playwright test --project chromium a.spec.ts", 1),
            Some("t1"),
        );
        state.record(
            run(
                "npx playwright test --project firefox a.spec.ts b.spec.ts",
                0,
            ),
            Some("t1"),
        );
        assert!(reason(&state, "t1").is_some());
        state.record(
            run(
                "npx playwright test --project chromium a.spec.ts b.spec.ts",
                0,
            ),
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);

        let mut state = Checks::default();
        state.record(run("npx playwright test a b", 1), Some("t1"));
        state.record(run("npx playwright test 'a b'", 0), Some("t1"));
        assert!(reason(&state, "t1").is_some());
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
    fn a_pass_of_a_label_or_path_above_a_name_clears_its_failure() {
        let django = |names: &str, code| {
            test(
                &format!("python runtests.py {names}"),
                code,
                &format!("runtests.py {names}"),
            )
        };
        let mut state = Checks::default();
        state.record([test("mise run test", 0, "full")], Some("t1"));
        state.record(
            [django(
                "delete admin_views.tests.AdminViewDeletedObjectsTest",
                1,
            )],
            Some("t1"),
        );
        // A sibling label, or one that only starts the same, does not.
        state.record(
            [django("delete admin_views.forms admin_view", 0)],
            Some("t1"),
        );
        assert!(reason(&state, "t1").is_some());
        state.record([django("delete admin_views", 0)], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);

        let pytest = |name: &str, code| test(&format!("python3 -m pytest {name}"), code, name);
        state.record([pytest("tests/a.py::t", 1)], Some("t1"));
        state.record([pytest("tests/a.py", 0)], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
        state.record([pytest("tests/b.py", 1)], Some("t1"));
        state.record([pytest("tests", 0)], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
        // A directory beside the file, a path out of it, or another runner
        // that only shares the program, does not.
        state.record([pytest("a.py::t", 1)], Some("t1"));
        state.record([pytest("a", 0)], Some("t1"));
        state.record([test("python3 -m unittest a.py", 0, "a.py")], Some("t1"));
        assert!(reason(&state, "t1").is_some());
        assert!(!Nesting::Pytest.holds("tests", "tests/../other/a.py"));
        assert!(!Nesting::Django.holds("a", "a/b"));

        // Nor does a runner jouet does not know nests, such as Go's packages.
        let mut state = Checks::default();
        state.record([test("go test ./a/b", 1, "./a/b")], Some("t1"));
        state.record([test("go test ./a", 0, "./a")], Some("t1"));
        assert!(reason(&state, "t1").is_some());
    }

    #[test]
    fn a_mise_task_nests_as_the_runner_it_starts() {
        use crate::wind::tasks::Existing;
        let task = |name: &str, run: &str| Existing {
            name: name.into(),
            run: vec![run.into()],
        };
        let tasks = [
            task("test:unit", "cd tests && python runtests.py"),
            task("test:py", "mise x -- pytest -q"),
            task("test:go", "go test"),
        ];
        assert_eq!(Nesting::of_task(&tasks, "test:unit"), Some(Nesting::Django));
        assert_eq!(Nesting::of_task(&tasks, "test:py"), Some(Nesting::Pytest));
        assert_eq!(Nesting::of_task(&tasks, "test:go"), None);
        assert_eq!(Nesting::of_task(&tasks, "missing"), None);
        let argv = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(Nesting::of(&argv("py.test tests")), Some(Nesting::Pytest));
        assert_eq!(
            Nesting::of(&argv("python manage.py test a")),
            Some(Nesting::Django)
        );
        assert_eq!(Nesting::of(&argv("python -m unittest a")), None);
    }

    #[test]
    fn a_pass_without_the_name_filter_clears_a_failure() {
        let mut state = Checks::default();
        state.record([test("mise run test", 0, "full")], Some("t1"));
        let cargo = |args: &str, code| test(&format!("cargo test {args}"), code, args);
        state.record([cargo("-p a name", 101)], Some("t1"));
        // Another package, or a value for an option, does not cover it.
        state.record([cargo("-p b", 0)], Some("t1"));
        state.record([cargo("-p", 0)], Some("t1"));
        assert!(reason(&state, "t1").is_some());
        state.record([cargo("-p a", 0)], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
        // Words after the pass that hold an option are not only names.
        state.record([cargo("-p a -- --exact name", 101)], Some("t1"));
        state.record([cargo("-p a --", 0)], Some("t1"));
        assert!(reason(&state, "t1").is_some());
    }

    #[test]
    fn a_pass_of_another_runner_never_widens_to_cover_a_failure() {
        let mut state = Checks::default();
        state.record([test("mise run test", 0, "full")], Some("t1"));
        state.record(
            [test(
                "cargo test --bin jouet affected::cargo",
                101,
                "--bin jouet affected::cargo",
            )],
            Some("t1"),
        );
        // A vitest run whose selection reads like a shorter filter.
        state.record(
            [test(
                "vitest --bin jouet affected::",
                0,
                "--bin jouet affected::",
            )],
            Some("t1"),
        );
        assert!(reason(&state, "t1").is_some());
        state.record(
            [test(
                "cargo test --bin jouet affected::",
                0,
                "--bin jouet affected::",
            )],
            Some("t1"),
        );
        assert_eq!(reason(&state, "t1"), None);
        // Nor does dropping the names cross runners.
        state.record([test("cargo test -p a name", 101, "-p a name")], Some("t1"));
        state.record([test("pytest -p a", 0, "-p a")], Some("t1"));
        assert!(reason(&state, "t1").is_some());
        assert!(same_runner("cargo test -p a", "cargo test --workspace"));
        assert!(!same_runner("cargo test -p a", "cargo nextest run -p a"));
        // Through `jouet test --cmd --` and its wrappers, the runner is the
        // command it ran.
        assert!(same_runner(
            "jouet test --cmd -- mise x -- cargo +stable test -p a",
            "cargo test -p a"
        ));
        assert!(!same_runner(
            "jouet test --cmd -- cargo test -p a",
            "jouet test --cmd -- pytest -p a"
        ));
        assert!(same_runner(
            "/home/u/.local/bin/jouet --json test --cmd -- cargo test -p a",
            "cargo test -p a"
        ));
    }

    #[test]
    fn a_wider_filter_through_jouet_test_covers_a_failed_check_of_the_fix() {
        // A test passes, fails with the fix taken out, and the fix goes back:
        // the pass at the files now covers the failure, through `jouet test`
        // and a toolchain as directly.
        let mut state = Checks::default();
        state.record(
            [test(
                "jouet test --cmd -- cargo +stable test -p mbx --lib tests::",
                0,
                "-p mbx --lib tests::",
            )],
            Some("t1"),
        );
        state.record(
            [test(
                "jouet test --cmd -- cargo +stable test -p mbx --lib tests::a_program_run",
                1,
                "-p mbx --lib tests::a_program_run",
            )],
            Some("t2"),
        );
        assert!(reason(&state, "t2").is_some());
        assert_eq!(reason(&state, "t1"), None);
    }

    #[test]
    fn a_whole_package_run_covers_its_targets_and_filters() {
        let run = |state: &mut State, args: &str, code, selector: &str, tree| {
            state.record(
                "/home",
                [test(
                    &format!("jouet test --cmd -- cargo test{args}"),
                    code,
                    selector,
                )],
                Some(tree),
            );
        };
        let block =
            |state: &State| state.block("/home", |_| Some("t2".into()), |_| false, |_| true);
        let covered = |failed: &str, ran: &str| {
            let mut state = State::default();
            run(&mut state, " --bin jouet x", 101, failed, "t1");
            run(&mut state, "", 0, ran, "t2");
            block(&state).is_none()
        };
        assert!(covered(
            "-p jouet --bin jouet a_cargo codex_replay",
            "-p jouet"
        ));
        assert!(covered("-p jouet --lib --test cli a", "-p jouet"));
        // Another package, ignored tests and examples are not in the run.
        assert!(!covered("-p other --bin other a", "-p jouet"));
        assert!(!covered("-p jouet --ignored a", "-p jouet"));
        assert!(!covered("-p jouet --example demo", "-p jouet"));
        assert!(!covered("--bin jouet a", "-p jouet"));
    }

    #[test]
    fn a_workspace_run_covers_its_picks_in_one_package() {
        let run = |state: &mut State, args: &str, code, selector: &str, tree| {
            state.record(
                "/home",
                [test(
                    &format!("jouet test --cmd -- cargo test{args}"),
                    code,
                    selector,
                )],
                Some(tree),
            );
        };
        let covered = |args: &str, ran: &str| {
            let mut state = State::default();
            run(
                &mut state,
                " --bin jouet a",
                101,
                "-p jouet --bin jouet a",
                "t1",
            );
            run(&mut state, args, 0, ran, "t2");
            state
                .block("/home", |_| Some("t2".into()), |_| false, |_| true)
                .is_none()
        };
        assert!(covered(" --workspace --bin jouet a", "--bin jouet a"));
        assert!(covered(" --all --bin jouet a", "--bin jouet a"));
        // Without `--workspace` the same picks run in the current package only.
        assert!(!covered(" --bin jouet a", "--bin jouet a"));
        assert!(!covered(" --workspace --bin jouet b", "--bin jouet b"));
    }

    #[test]
    fn a_cargo_filter_on_the_failed_tests_paths_covers_their_failure() {
        // cargo matches a filter against the whole path, so `testlog` reran
        // the test the failed run named `a_summary`, which only the failing
        // test's name as the runner printed it shows.
        let run = |state: &mut State, filter: &str, code, tree| {
            state.record(
                "/home",
                [test(
                    &format!("jouet test --cmd -- cargo test --bin jouet {filter}"),
                    code,
                    &format!("--bin jouet {filter}"),
                )],
                Some(tree),
            );
        };
        let block =
            |state: &State| state.block("/home", |_| Some("t2".into()), |_| false, |_| true);
        let at_failure = |state: &State| {
            state
                .block("/home", |_| Some("t1".into()), |_| false, |_| true)
                .map(|b| b.reason)
        };
        let failed = |tests: &[&str]| {
            let mut state = State::default();
            run(&mut state, "a_summary", 101, "t1");
            let tests: Vec<String> = tests.iter().map(|t| t.to_string()).collect();
            state.mark_failed_tests(
                "/home",
                "jouet test --cmd -- cargo test --bin jouet a_summary",
                &tests,
            );
            state
        };
        // Until another run, the failure is still the last run.
        let last = at_failure(&failed(&["testlog::tests::a_summary"]));
        assert!(
            last.as_deref()
                .is_some_and(|r| r.starts_with("The last test run failed")),
            "{last:?}"
        );
        let mut state = failed(&["testlog::tests::a_summary"]);
        run(&mut state, "testlog", 0, "t2");
        assert_eq!(block(&state), None);
        // A filter that misses one failing test, or a run whose failures were
        // not named, still holds it up.
        let mut state = failed(&["testlog::tests::a_summary", "verify::tests::a_summary"]);
        run(&mut state, "testlog", 0, "t2");
        assert!(block(&state).is_some());
        let mut state = failed(&[]);
        run(&mut state, "testlog", 0, "t2");
        assert!(block(&state).is_some());
    }

    #[test]
    fn a_shorter_cargo_name_filter_covers_a_longer_one() {
        let c = "cargo test --bin jouet affected::cargo";
        assert!(wider_filter(
            c,
            "--bin jouet affected::",
            "--bin jouet affected::cargo"
        ));
        // Another filter, another target, a whole-name match, another runner.
        assert!(!wider_filter(
            c,
            "--bin jouet verify::",
            "--bin jouet affected::cargo"
        ));
        assert!(!wider_filter(
            c,
            "--lib affected::",
            "--bin jouet affected::cargo"
        ));
        assert!(!wider_filter(c, "-- --exact a", "-- --exact ab"));
        // Several filters widen one by one: a run of several, one of them
        // the failed test's name or part of it, covers that test. The
        // selectors are as the checks rule records them, without `--`.
        let c = "cargo test -p mise --lib -- test_ab";
        assert!(wider_filter(
            c,
            "-p mise --lib test_b test_ab cutoff",
            "-p mise --lib test_ab"
        ));
        assert!(wider_filter(
            c,
            "-p mise --lib cutoff latest",
            "-p mise --lib test_cutoff_x"
        ));
        assert!(wider_filter(
            c,
            "-p mise cutoff latest",
            "-p mise test_cutoff_x"
        ));
        assert!(!wider_filter(
            c,
            "-p mise --lib latest",
            "-p mise --lib test_cutoff_x test_other"
        ));
        assert!(!wider_filter(
            c,
            "-p mise --lib cutoff",
            "-p mise --bins test_cutoff_x"
        ));
        assert!(!wider_filter(
            "cargo nextest run -E x",
            "-p mise -E test(a) | b",
            "-p mise -E test(a) | bc"
        ));
        assert!(wider_filter(c, "--bin=jouet a", "--bin=jouet ab"));
        assert!(!wider_filter(c, "--filter-expr=x a", "--filter-expr=x ab"));
        // A package name is not a filter.
        assert!(!wider_filter(c, "-p mi", "-p mise"));
        assert!(!wider_filter(
            c,
            "-p mise --lib cutoff --exact",
            "-p mise --lib cutoff"
        ));
        assert!(!wider_filter(
            "mise run test:e2e test_ab",
            "test:e2e test_a",
            "test:e2e test_ab"
        ));
    }

    #[test]
    fn a_run_without_a_name_filter_covers_the_filtered_one() {
        assert!(without_name_filter(
            "bats test/hook.bats",
            "bats test/hook.bats -f jouet commit"
        ));
        assert!(without_name_filter(
            "go test ./pkg",
            "go test ./pkg -run TestA"
        ));
        assert!(without_name_filter(
            "npx vitest run a.test.ts",
            "npx vitest run a.test.ts -t adds"
        ));
        assert!(!without_name_filter(
            "pytest t.py",
            "pytest t.py -k not slow"
        ));
        assert!(!without_name_filter("bats a.bats", "bats b.bats -f x"));
        assert!(!without_name_filter("bats a.bats", "bats a.bats -f"));
        // A file or package after the name is one `ran` did not run.
        assert!(!without_name_filter(
            "bats a.bats",
            "bats a.bats -f x b.bats"
        ));
        assert!(!without_name_filter(
            "go test ./pkg",
            "go test ./pkg -run TestA ./other"
        ));
    }

    #[test]
    fn without_names_reads_only_trailing_plain_words() {
        assert!(without_names("-p a", "-p a name"));
        assert!(without_names("test:e2e", "test:e2e test_a"));
        assert!(!without_names("-p a", "-p a"));
        assert!(!without_names("-k", "-k x"));
        assert!(!without_names("-p a", "-p a --exact name"));
        assert!(!without_names("", "name"));
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
        // A partial pass satisfies the stop hook at its tree.
        state.record([test("cargo test -p a", 0, "-p a")], Some("t1"));
        assert_eq!(reason(&state, "t1"), None);
        let why = reason(&state, "t2").unwrap();
        assert!(
            why.starts_with("Files changed after the last test run passed (`cargo test -p a`)"),
            "{why}"
        );
        assert!(why.contains("`jouet test --affected`"), "{why}");
        assert!(!why.contains("The full suite runs on CI."), "{why}");
        // Where --affected cannot select, it is not offered.
        let why = state.reasons(|| Some("t2".into()), false, false).remove(0);
        assert!(!why.contains("--affected"), "{why}");
        // Any passing run at the files now clears it.
        state.record([test("cargo test -p b", 0, "-p b")], Some("t2"));
        assert_eq!(reason(&state, "t2"), None);
        // A change to prose alone keeps the code part, and asks for no run.
        let mut docs = Checks::default();
        docs.record([test("cargo test -p a", 0, "-p a")], Some("all1.code1"));
        assert_eq!(reason(&docs, "all2.code1"), None);
        assert!(reason(&docs, "all3.code2").is_some());
        // Prose changed after a formatter rewrote the tested code.
        docs.reformatted("all1.code1", "all4.code4");
        assert_eq!(reason(&docs, "all5.code4"), None);
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
        soon.set_aside_settled("/home", ran + SETTLED_AFTER - 1, |_| Some(false), |_| false);
        assert!(ask(&soon).is_some());
        // Unless it is its base exactly: nothing of the session's is left there.
        let mut reset = State::load_from(&state);
        reset.set_aside_settled("/home", ran, |_| Some(false), |root| root == "/old");
        assert_eq!(ask(&reset), None);
        // Something uncommitted keeps it, whatever HEAD is.
        let mut dirty = State::load_from(&state);
        dirty.set_aside_settled("/home", ran, |_| Some(true), |_| true);
        assert!(ask(&dirty).is_some());
        // Later, with something uncommitted there, or no telling, it is too.
        let later = ran + SETTLED_AFTER;
        for uncommitted in [Some(true), None] {
            let mut left = State::load_from(&state);
            left.set_aside_settled("/home", later, |_| uncommitted, |_| false);
            assert!(ask(&left).is_some());
        }
        // Later and clean, it is set aside; the session's own tree never is.
        let mut left = State::load_from(&state);
        left.set_aside_settled("/home", later, |_| Some(false), |_| false);
        assert_eq!(ask(&left), None);
        assert!(left.trees.contains_key("/home"));
        left.record("/home", [test("cargo test -p b", 1, "-p b")], Some("h1"));
        left.set_aside_settled("/home", later + SETTLED_AFTER, |_| Some(false), |_| true);
        assert!(ask(&left).unwrap().starts_with("The last test run failed"));

        // A state written before jouet kept the time reads as long ago.
        let mut old = State::load_from(&state);
        old.trees.get_mut("/old").unwrap().last_run = None;
        old.set_aside_settled("/home", ran, |_| Some(false), |_| false);
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
    fn a_tree_only_subagents_ran_in_does_not_hold_up_the_lead() {
        let mut state = State::default();
        let tree = |root: &str| Some(format!("{root}-now"));
        let block = |state: &State| {
            state
                .block("/home", tree, |_| false, |_| true)
                .map(|b| b.reason)
        };
        // A subagent's scratch repository, changed after its pass.
        state.by("/scratch", false);
        state.record(
            "/scratch",
            [test("pytest", 0, "full")],
            Some("/scratch-then"),
        );
        assert_eq!(block(&state), None);
        // In the session's own tree, a subagent's run counts like the lead's.
        state.by("/home", false);
        state.record(
            "/home",
            [test("cargo test", 101, "full")],
            Some("/home-now"),
        );
        assert!(
            block(&state)
                .unwrap()
                .starts_with("The last test run failed")
        );
        // Once the lead runs something in the scratch tree, it is the lead's.
        state.record("/home", [test("cargo test", 0, "full")], Some("/home-now"));
        state.by("/scratch", true);
        let why = block(&state).unwrap();
        assert!(why.starts_with("In /scratch: Files changed"), "{why}");
        // A subagent's later run there hands it back, as when the lead
        // checked a subagent's work once and the subagent went on working.
        state.by("/scratch", false);
        assert!(!state.checks("/scratch").unwrap().subagents_only);
        assert_eq!(block(&state), None);
        // Until the lead runs something there again.
        state.by("/scratch", true);
        assert!(
            block(&state)
                .unwrap()
                .starts_with("In /scratch: Files changed")
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
        // jouet test marks a run whose merge base also failed at the merge base.
        state.mark_unverifiable("/w", "jouet test --budget 1500");
        let block = state
            .block("/w", |_| Some("t1".into()), |_| false, |_| true)
            .unwrap();
        assert!(
            block
                .reason
                .contains("the command also failed at the merge base"),
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
            !again.reason.contains("also failed at the merge base"),
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
        assert_eq!(block.raised[0].1.reasons.len(), 5);
        assert!(
            block
                .reason
                .ends_with("(2 more after these, at the next stop.)"),
            "{}",
            block.reason
        );
        state.raise(&block);
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
        assert_eq!(block.raised[0].1.reasons.len(), 2, "{}", block.reason);
        assert_eq!(block.raised[0].1.more, 0);
        state.raise(&block);
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
    fn every_tree_with_a_reason_is_raised_at_one_stop() {
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
        // Both at once, the session's own first: one left for a later stop
        // would end a turn that never touched it.
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert_eq!(
            block
                .raised
                .iter()
                .map(|(root, _)| root.as_str())
                .collect::<Vec<_>>(),
            ["/home", "/other"]
        );
        assert!(
            block.reason.starts_with("The last test run failed"),
            "{block:?}"
        );
        assert!(block.reason.contains("\n\nIn /other: "), "{block:?}");
        // The raised reason is kept without the tree's name.
        assert!(block.raised[1].1.reasons[0].starts_with("The last test run failed"));
        state.raise(&block);
        assert_eq!(state.block("/home", tree, |_| false, |_| true), None);
    }

    #[test]
    fn a_reason_is_raised_once_for_its_run_not_at_each_tree_after() {
        let mut state = State::default();
        let at = std::cell::Cell::new("t1");
        let tree = |_: &str| Some(at.get().to_string());
        state.record(
            "/home",
            [test("cargo test -p a name", 101, "-p a name")],
            Some("t1"),
        );
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert!(block.reason.contains("exited 101"), "{}", block.reason);
        state.raise(&block);
        // Files change, and other runs pass: the failure was said once.
        at.set("t2");
        assert_eq!(state.block("/home", tree, |_| false, |_| true), None);
        state.record("/home", [test("cargo test -p b", 0, "-p b")], Some("t2"));
        assert_eq!(state.block("/home", tree, |_| false, |_| true), None);
        // The same command failing again, at other files, is new.
        at.set("t3");
        state.record(
            "/home",
            [test("cargo test -p a name", 101, "-p a name")],
            Some("t3"),
        );
        assert!(state.block("/home", tree, |_| false, |_| true).is_some());

        // Files changed after a pass: said once for that pass.
        let mut state = State::default();
        at.set("p1");
        state.record("/home", [test("cargo test -p a", 0, "-p a")], Some("p1"));
        at.set("p2");
        let block = state.block("/home", tree, |_| false, |_| true).unwrap();
        assert!(
            block.reason.starts_with("Files changed after"),
            "{}",
            block.reason
        );
        state.raise(&block);
        at.set("p3");
        assert_eq!(state.block("/home", tree, |_| false, |_| true), None);
        // A newer pass, then changes again: said again.
        state.record("/home", [test("cargo test -p a", 0, "-p a")], Some("p3"));
        at.set("p4");
        assert!(state.block("/home", tree, |_| false, |_| true).is_some());
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
            serde_json::json!({"timestamp": "2026-11-01T10:00:00.123Z",
                "type": "event_msg", "payload": {"type": "item_completed",
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
        let ran = |command: &str, exit_code, at| Ran {
            command: command.into(),
            exit_code,
            at,
            output: None,
            cwd: None,
        };
        assert_eq!(
            codex_commands(&path),
            [
                ran(
                    "cargo test",
                    101,
                    jouet_output::time::parse_utc("2026-11-01T10:00:00Z")
                ),
                ran("ls", 0, None)
            ]
        );
        assert!(codex_commands(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn codex_command_uses_only_an_absolute_recorded_cwd() {
        let line = |cwd: &str| {
            serde_json::json!({"payload": {"type": "exec_command_end", "command": ["pnpm", "test"], "exit_code": 1, "cwd": cwd}}).to_string()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let expected = normalize_cwd_separators(path, cfg!(windows));
        assert_eq!(
            codex_command(&line(path)).unwrap().cwd.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(codex_command(&line("relative")).unwrap().cwd, None);
    }

    #[test]
    fn codex_replay_decodes_local_file_uris_conservatively() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work space#%é");
        let path_str = path.to_str().unwrap();
        let encoded = path_str
            .replace('\\', "/")
            .replace('%', "%25")
            .replace(' ', "%20")
            .replace('#', "%23")
            .replace('é', "%C3%A9");
        let prefix = if cfg!(windows) { "file:///" } else { "file://" };
        let uri = format!("{prefix}{encoded}");
        let line = serde_json::json!({"payload": {"type": "item_completed", "item": {
            "type": "CommandExecution", "command": ["pnpm", "test"], "exit_code": 1, "cwd": uri
        }}})
        .to_string();
        let expected = normalize_cwd_separators(path_str, cfg!(windows));
        assert_eq!(
            codex_command(&line).unwrap().cwd.as_deref(),
            Some(expected.as_str())
        );
        let localhost = format!("file://localhost/{}", encoded.trim_start_matches('/'));
        assert_eq!(codex_cwd(&localhost).as_deref(), Some(expected.as_str()));
        for bad in [
            "file://remote/path",
            "file:///tmp/%ZZ",
            "file:///tmp/%2",
            "file:///tmp/%00",
            "file:///tmp/a?query",
            "file:///tmp/a#fragment",
            "file:///tmp/%FF",
        ] {
            assert_eq!(codex_cwd(bad), None, "{bad}");
        }
    }

    #[test]
    fn windows_native_and_uri_separators_share_a_scope() {
        let git_root = "C:/work/project name";
        assert_eq!(
            normalize_cwd_separators("C:/work/project name", true),
            git_root
        );
        assert_eq!(
            normalize_cwd_separators(r"C:\work\project name", true),
            git_root
        );
        assert_eq!(
            normalize_cwd_separators("/work/project", false),
            "/work/project"
        );
    }

    #[cfg(windows)]
    #[test]
    fn codex_windows_uri_and_native_paths_match() {
        assert_eq!(
            codex_cwd("file:///C:/work/project%20name"),
            codex_cwd(r"C:\work\project name")
        );
        assert_eq!(codex_cwd("file:////server/share"), None);
    }
}

#[cfg(test)]
mod hk_file_list_tests {
    use super::*;

    /// A repository with `a.rs`, `b.rs` and `gone.rs` committed, then
    /// `a.rs` changed, `gone.rs` deleted, `new.rs` added and an untracked
    /// repository put in `nested`.
    fn repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::create_dir(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("src/a.rs"), "a\n").unwrap();
        std::fs::write(repo.path().join("src/b.rs"), "b\n").unwrap();
        std::fs::write(repo.path().join("gone.rs"), "g\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(repo.path().join("src/a.rs"), "a2\n").unwrap();
        std::fs::write(repo.path().join("new.rs"), "n\n").unwrap();
        std::fs::remove_file(repo.path().join("gone.rs")).unwrap();
        let nested = repo.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        git(&["init", "-q", "nested"]);
        std::fs::write(nested.join("x.rs"), "x\n").unwrap();
        repo
    }

    /// The selector `command`, which selected `selector`, is recorded with
    /// after running in `dir`.
    fn widened(command: &str, selector: &str, dir: &Path) -> Option<String> {
        let mut record = Record {
            rule: "checks".into(),
            kind: Kind::Lint,
            command: command.into(),
            exit_code: 0,
            full: false,
            dir: None,
            selector: Some(selector.into()),
        };
        widen_hk_files(&mut record, dir, Duration::from_secs(10));
        record.selector
    }

    #[test]
    fn a_file_list_naming_every_change_covers_the_changes() {
        let repo = repo();
        let changed = Some("changed files".to_string());
        assert_eq!(
            widened(
                "hk check src/a.rs new.rs",
                "files src/a.rs new.rs",
                repo.path()
            ),
            changed
        );
        // From a subdirectory, by paths relative to it.
        let sub = repo.path().join("src");
        assert_eq!(
            widened(
                "hk check a.rs ../new.rs b.rs",
                "files a.rs ../new.rs b.rs",
                &sub
            ),
            changed
        );
        // A NUL-separated list outside the work tree.
        let scratch = tempfile::tempdir().unwrap();
        let list = scratch.path().join("files");
        std::fs::write(&list, "src/a.rs\0new.rs\0").unwrap();
        let selector = format!("files0-from {}", list.display());
        let command = format!("hk run check --files0-from {}", list.display());
        assert_eq!(widened(&command, &selector, repo.path()), changed);
        // One inside it counts itself.
        std::fs::write(repo.path().join("list"), "src/a.rs\0new.rs\0").unwrap();
        assert_eq!(
            widened(
                "hk run check --files0-from list",
                "files0-from list",
                repo.path()
            ),
            changed
        );
        // `jouet diag check` runs hk too.
        std::fs::remove_file(repo.path().join("list")).unwrap();
        assert_eq!(
            widened(
                "jouet diag check src/a.rs new.rs",
                "files src/a.rs new.rs",
                repo.path()
            ),
            changed
        );
    }

    #[test]
    fn a_file_list_missing_a_change_stays_partial() {
        let repo = repo();
        let selector = "files src/a.rs".to_string();
        assert_eq!(
            widened("hk check src/a.rs", &selector, repo.path()),
            Some(selector)
        );
        std::fs::write(repo.path().join("list"), "src/a.rs\0list\0").unwrap();
        let selector = "files0-from list".to_string();
        assert_eq!(
            widened("hk check --files0-from list", &selector, repo.path()),
            Some(selector)
        );
        // Every change, but narrowed to one step.
        let selector = "files src/a.rs new.rs --step fmt".to_string();
        assert_eq!(
            widened(
                "hk check src/a.rs new.rs --step fmt",
                &selector,
                repo.path()
            ),
            Some(selector)
        );
        // From another directory.
        let selector = "files src/a.rs new.rs".to_string();
        assert_eq!(
            widened("hk check --cd src src/a.rs new.rs", &selector, repo.path()),
            Some(selector)
        );
    }

    #[test]
    fn an_unreadable_file_list_stays_partial() {
        let repo = repo();
        let selector = "files0-from missing".to_string();
        assert_eq!(
            widened("hk check --files0-from missing", &selector, repo.path()),
            Some(selector)
        );
        // Outside a repository there are no changes to compare.
        let elsewhere = tempfile::tempdir().unwrap();
        let selector = "files a.rs".to_string();
        assert_eq!(
            widened("hk check a.rs", &selector, elsewhere.path()),
            Some(selector)
        );
    }
}
