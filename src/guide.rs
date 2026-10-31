//! What agents are told about jouet: `jouet agent-help`, the skill and the
//! session-start primer. All of it is generated from the CLI definition, so
//! it describes the installed binary and cannot go stale.

use crate::Jouet;
use jouet_output::exit::Exit;
use std::fmt::Write as _;
use usage_rs::spec::CommandMeta;

/// When an agent should reach for each command. Every visible command has a
/// line (a test checks), so a new command lands with one; `None` marks a
/// command that is for people setting jouet up, not for agents.
const WHEN: &[(&str, Option<&str>)] = &[
    (
        "search",
        Some(
            "searching code: like rg, but grouped by the function each match is in, cut to a budget with markers for what it left out, and collapsing matches this session already saw",
        ),
    ),
    (
        "read",
        Some(
            "reading code: `jouet read FILE --outline`, then `--symbol NAME` or `--range A:B`, instead of reading a whole file to find one thing; `jouet read --map` prints a repository's most important definitions to fit a token budget",
        ),
    ),
    (
        "edit",
        Some(
            "changing files: exact replacements (`jouet edit replace <PATH> --old TEXT --new TEXT`, or a spec on stdin), whole definitions by name (`=== PATH#SYMBOL`), or every match of a code pattern (`--pattern --rewrite --lang`), that all land or none do, instead of python/sed rewrites or a string of separate edits",
        ),
    ),
    (
        "diag",
        Some(
            "after an edit: the errors and warnings in just the files you changed (`diag check --changed`), or only the ones your edit introduced (`diag delta`, same arguments), from the project's hk checks, instead of a full `mise run lint` or `cargo clippy | head`; `diag fix` formats them",
        ),
    ),
    (
        "wait",
        Some(
            "waiting for CI, a PR or a workflow run, or for a file, log line, port, URL, process, command or `jouet job`, instead of a sleep loop",
        ),
    ),
    (
        "job",
        Some(
            "running a build, a test suite, a long script or a server for one check in the background: `jouet job start ID --port N -- CMD` returns once it is ready, `jouet wait job ID` blocks until it ends, instead of `&`, nohup, sleep-and-curl loops and pkill",
        ),
    ),
    (
        "ci",
        Some(
            "reproducing a GitHub Actions job on this machine before pushing, or one `wait ci` reported failing (`ci run --check NAME`), instead of pushing again to see; needs jactionlint",
        ),
    ),
    (
        "pr",
        Some(
            "checking many PRs at once (`pr status`), finding the one thing a PR needs next (`pr next`) and merging it once ready (`pr land`), or reading, replying to and resolving review threads (`pr threads`), instead of gh loops and raw GraphQL",
        ),
    ),
    (
        "commit",
        Some(
            "committing: only the paths you name, with the configured trailers and hooks run, instead of `git add -A && git commit`",
        ),
    ),
    (
        "review-package",
        Some(
            "handing a range of commits to a reviewer: one file with the commits, a diffstat and the full diff, refusing a range that would show the wrong diff",
        ),
    ),
    (
        "handoff",
        Some(
            "ending a session with work in progress: records the branch, uncommitted files, the PR, the last test runs, background processes and jobs for the next session",
        ),
    ),
    (
        "pickup",
        Some(
            "starting a session that continues earlier work: prints the last handoff and checks that it still holds",
        ),
    ),
    (
        "test",
        Some(
            "running tests while you iterate: `jouet test --affected` runs only the tests your changes affect (vitest, jest, pytest, and the code graph for Go, Ruby and Java) and lists why each was picked; `jouet test` runs the full suite, which still has to pass before you finish or push",
        ),
    ),
    (
        "rule",
        Some(
            "writing or checking a jouet rule, the TOML matchers behind jouet's hooks (`jouet rule schema`): write its `fires` and `quiet` examples first, and `jouet rule test` runs them before it is enabled",
        ),
    ),
    (
        "run",
        Some(
            "a job you would otherwise do step by step, written as a routine (code, where rules are TOML configuration): `joue land [PR]` (pr-next, wait for CI, then pr land), `joue sweep` (unresolved threads as a work list), `joue ship-check` (the tests, then the PR); `joue --list` lists them, `joue NAME --help` gives the args, and a routine that writes needs --yes (without it: a preview and exit 4)",
        ),
    ),
    (
        "wind",
        Some(
            "a winding note at session start names a drafted rule or a proposal: `jouet wind show ID` reads it; or a session is worth reviewing: `jouet wind review SESSION` prints a prompt and a digest for a subagent",
        ),
    ),
    (
        "unwind",
        Some("a rule of yours keeps getting overridden or is noisy: it proposes retiring it"),
    ),
    // Not in the overview: it is at its 40-line budget. `jouet agent-help
    // graph` describes it, and `jouet read` and `jouet edit` use it.
    ("graph", None),
    ("share", None),
    ("agent-help", None),
    ("audit", None),
    ("skill", None),
    ("setup", None),
    ("stats", None),
    ("completion", None),
];

/// Subcommands an agent is told about where it needs them, in a line jouet
/// prints (a proposal's note, the review prompt), so the overview leaves
/// them out. `jouet agent-help COMMAND` still lists them.
const PROMPTED: &[(&str, &str)] = &[
    ("wind", "adopt"),
    ("wind", "propose"),
    ("wind", "dismiss"),
    // `diag`'s when line names them, and they take check's arguments.
    ("diag", "delta"),
    ("diag", "fix"),
];

/// Subcommands for a person's decision, not an agent's, which the overview
/// leaves out as it leaves out `jouet setup`: trusting a repository's rules.
/// `jouet agent-help COMMAND` still lists them.
const PEOPLE: &[(&str, &str)] = &[("rule", "trust"), ("rule", "untrust")];

/// Tools jouet works alongside rather than reimplementing, mentioned when
/// they are installed.
const COMPANIONS: &[(&str, &str)] = &[
    (
        "mbx",
        "mbx: Rust builds. Run cargo as usual (or prefix it with `mbx`); compiled output is shared across checkouts and builds are scheduled machine-wide. `mbx analyze` explains a slow rebuild",
    ),
    (
        "jactionlint",
        "jactionlint: lints GitHub workflows; `jouet ci run` runs their jobs here through it",
    ),
    (
        "rtk",
        "rtk: shrinks the output of common commands; see `rtk --help`",
    ),
    (
        "bd",
        "bd (beads): issue tracking for agents; `bd ready` lists unblocked work",
    ),
    (
        "fnox",
        "fnox: a project's secrets, encrypted or in a vault. `fnox exec -- CMD` runs a command with them in its environment and `fnox list` shows their names; never print a value (`fnox get`, `env`) into the transcript",
    ),
];

fn root() -> &'static CommandMeta<'static> {
    Jouet::spec().root
}

fn find<'a>(meta: &'a CommandMeta<'a>, name: &str) -> Option<&'a CommandMeta<'a>> {
    meta.subcommands
        .iter()
        .copied()
        .find(|c| c.cmd.name == name || c.cmd.aliases.contains(&name))
}

fn lookup<S: AsRef<str>>(path: &[S]) -> Option<&'static CommandMeta<'static>> {
    let mut meta = root();
    for name in path {
        meta = find(meta, name.as_ref())?;
    }
    Some(meta)
}

/// The commands an agent should know about, with when to use each.
pub(crate) fn agent_commands() -> Vec<(&'static CommandMeta<'static>, &'static str)> {
    root()
        .subcommands
        .iter()
        .filter(|c| !c.hide)
        .filter_map(|c| when(c.cmd.name).map(|w| (*c, w)))
        .collect()
}

fn when(name: &str) -> Option<&'static str> {
    WHEN.iter().find(|(n, _)| *n == name).and_then(|(_, w)| *w)
}

/// The guide for the whole tool: one entry per command family, with its
/// subcommands by name and when to reach for it. Flags and synopses live in
/// `jouet agent-help COMMAND`, so a new subcommand costs the overview a word,
/// not a line.
pub(crate) fn overview() -> String {
    let mut out = format!(
        "jouet {}: a toolkit CLI for coding agents. Reach for it before hand-rolling a script.\n\n",
        env!("CARGO_PKG_VERSION")
    );
    for (meta, when) in agent_commands() {
        let name = meta.cmd.name;
        let subs: Vec<&str> = meta
            .subcommands
            .iter()
            .filter(|s| {
                let at = (name, s.cmd.name);
                !s.hide && !PROMPTED.contains(&at) && !PEOPLE.contains(&at)
            })
            .map(|s| s.cmd.name)
            .collect();
        let head = if subs.is_empty() {
            format!("jouet {name}")
        } else if meta.subcommand_required() {
            format!("jouet {name} <{}>", subs.join("|"))
        } else {
            // The family also runs on its own.
            format!("jouet {name} [{}]", subs.join("|"))
        };
        let _ = writeln!(out, "{head}: {}", meta.about.unwrap_or(""));
        let _ = writeln!(out, "  when: {when}");
    }
    out.push('\n');
    out.push_str(&conventions());
    for line in companions() {
        let _ = writeln!(out, "Also installed: {line}.");
    }
    out.push_str(
        "Details: `jouet agent-help COMMAND` for its subcommands and flags, `jouet COMMAND --help`.\n",
    );
    out
}

/// The guide for one command and its subcommands.
pub(crate) fn command<S: AsRef<str>>(path: &[S]) -> Option<String> {
    let meta = lookup(path)?;
    let words: Vec<&str> = path.iter().map(AsRef::as_ref).collect();
    let name = format!("jouet {}", words.join(" "));
    let mut out = format!("{name}: {}\n", meta.about.unwrap_or(""));
    if let Some(when) = words.first().and_then(|w| when(w)) {
        let _ = writeln!(out, "when: {when}");
    }
    describe(&mut out, &name, meta);
    for sub in meta.subcommands.iter().filter(|s| !s.hide) {
        out.push('\n');
        let _ = writeln!(out, "{name} {}: {}", sub.cmd.name, sub.about.unwrap_or(""));
        describe(&mut out, &format!("{name} {}", sub.cmd.name), sub);
    }
    out.push('\n');
    out.push_str(&conventions());
    Some(out)
}

fn describe(out: &mut String, name: &str, meta: &CommandMeta<'_>) {
    if let Some(long) = meta.long_about {
        for line in long.lines().skip_while(|l| !l.is_empty()).skip(1) {
            let _ = writeln!(out, "  {line}");
        }
    }
    let _ = writeln!(out, "  {}", synopsis(name, meta));
    for flag in visible_flags(meta) {
        let _ = writeln!(out, "    {}  {}", flag_usage(flag), flag.help.unwrap_or(""));
    }
}

/// The short primer printed when a session starts.
pub(crate) fn summary(rules: &[&str], verify: bool) -> String {
    let commands: Vec<String> = agent_commands()
        .iter()
        .map(|(meta, _)| {
            format!(
                "`jouet {}` ({})",
                meta.cmd.name,
                lowercase_first(meta.about.unwrap_or(""))
            )
        })
        .collect();
    let mut out = format!(
        "jouet is installed: {}. Run `jouet agent-help` for when to use each and its flags, before hand-rolling a script.\n",
        commands.join(", ")
    );
    if !rules.is_empty() {
        let _ = writeln!(
            out,
            "jouet's hooks deny some commands and name a better one (rules: {}).",
            rules.join(", ")
        );
    }
    if verify {
        out.push_str(
            "They also stop you finishing while a test, lint or build run failed, or after only part of the test suite ran on changed code.\n",
        );
    }
    out
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn conventions() -> String {
    let codes: Vec<String> = Exit::ALL
        .iter()
        .map(|e| format!("{} {} ({})", e.code(), e.name(), e.meaning()))
        .collect();
    format!(
        "Output: the result on stdout, diagnostics on stderr. --json prints one JSON document, errors included. Lists are never cut short without saying so.\nExit codes: {}.\n",
        codes.join(", ")
    )
}

fn companions() -> Vec<&'static str> {
    COMPANIONS
        .iter()
        .filter(|(bin, _)| on_path(bin))
        .map(|(_, line)| *line)
        .collect()
}

pub(crate) fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{bin}.exe"), format!("{bin}.cmd")]
    } else {
        vec![bin.to_string()]
    };
    std::env::split_paths(&path).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
}

/// The root's own flags, which every command accepts and the guide states
/// once, in the conventions.
fn is_root_flag(flag: &usage_rs::spec::FlagMeta<'_>) -> bool {
    root().flags.iter().any(|r| r.flag.longs == flag.flag.longs)
}

fn visible_flags<'a>(
    meta: &'a CommandMeta<'a>,
) -> impl Iterator<Item = &'a usage_rs::spec::FlagMeta<'a>> {
    meta.flags
        .iter()
        .filter(|f| !f.builtin && !f.hide && !is_root_flag(f))
}

fn flag_usage(flag: &usage_rs::spec::FlagMeta<'_>) -> String {
    let name = flag.flag.longs.first().map_or_else(
        || {
            flag.flag
                .shorts
                .first()
                .map(|s| format!("-{}", *s as char))
                .unwrap_or_default()
        },
        |l| format!("--{l}"),
    );
    match flag.value_name {
        Some(value) if flag.flag.takes_value => format!("{name} {value}"),
        _ => name,
    }
}

fn synopsis(name: &str, meta: &CommandMeta<'_>) -> String {
    let mut parts = vec![name.to_string()];
    for arg in meta.args {
        let value = arg.value_names.first().copied().unwrap_or(arg.arg.name);
        let mut text = format!("<{value}>");
        if arg.arg.var {
            text.push_str("...");
        }
        if !arg.arg.required {
            text = format!("[{text}]");
        }
        parts.push(text);
    }
    for flag in visible_flags(meta) {
        let usage = flag_usage(flag);
        parts.push(if flag.required {
            usage
        } else {
            format!("[{usage}]")
        });
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_visible_command_says_when_to_use_it() {
        for meta in root().subcommands.iter().filter(|c| !c.hide) {
            assert!(
                WHEN.iter().any(|(n, _)| *n == meta.cmd.name),
                "add `{}` to WHEN in src/guide.rs",
                meta.cmd.name
            );
        }
        for (name, _) in WHEN {
            assert!(
                crate::spec::has_command(&[name]),
                "WHEN names `{name}`, which jouet does not have"
            );
        }
    }

    #[test]
    fn subcommands_left_out_of_the_overview_exist() {
        for (name, sub) in PROMPTED {
            assert!(crate::spec::has_command(&[name, sub]), "{name} {sub}");
            let guide = command(&[name]).unwrap();
            assert!(guide.contains(&format!("jouet {name} {sub}")), "{guide}");
            assert!(!overview().contains(&format!("jouet {name} {sub}")));
        }
    }

    #[test]
    fn the_overview_stays_short() {
        let text = overview();
        // The cap is a budget to defend, not a ceiling to fill. It counts
        // jouet's own lines: the "Also installed" lines depend on which
        // companion tools are on this machine's PATH, so counting them made
        // the test pass or fail by machine.
        let own = text
            .lines()
            .filter(|line| !line.starts_with("Also installed:"))
            .count();
        assert!(own <= 40, "{own} lines:\n{text}");
        assert!(
            text.contains("jouet edit replace <PATH> --old TEXT --new TEXT"),
            "{text}"
        );
        // Families list their subcommands by name, so a new one costs a word.
        assert!(text.contains("jouet pr <"), "{text}");
        assert!(text.contains("|land"), "{text}");
        assert!(text.contains("jouet edit [replace|hash]"), "{text}");
        assert!(text.contains("jouet wait <ci|"), "{text}");
        assert!(text.contains("4 precondition"), "{text}");
        assert!(!text.contains("jouet setup"), "{text}");
    }

    #[test]
    fn a_command_guide_lists_its_flags() {
        let text = command(&["edit"]).unwrap();
        assert!(text.contains("--expect-hash PATH=SHA256"), "{text}");
        assert!(text.contains("jouet edit hash"), "{text}");
        assert!(!text.contains("--json  "), "{text}");
        assert_eq!(command(&["nope"]), None);
    }
}
