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
        "edit",
        Some(
            "changing files: exact replacements that all land or none do, instead of python/sed rewrites or a string of separate edits",
        ),
    ),
    (
        "wait",
        Some(
            "waiting for CI, a PR or a workflow run, or for a file, log line, port, URL, process or command, instead of a sleep loop",
        ),
    ),
    (
        "pr",
        Some(
            "checking many PRs at once (`pr status`), finding the one thing a PR needs next (`pr next`) and merging it once ready (`pr land`), or reading, replying to and resolving review threads (`pr threads`), instead of gh loops and raw GraphQL",
        ),
    ),
    (
        "rule",
        Some(
            "writing or checking a jouet rule, the Luau scripts behind jouet's hooks: `jouet rule test` runs one before it is enabled",
        ),
    ),
    (
        "run",
        Some(
            "a routine you would otherwise type out step by step: `jouet run land` (pr next, wait ci, then pr land; yes=true merges), `jouet run sweep` (unresolved threads as a work list) or `jouet run ship-check` (the test task, then pr status); `jouet run --list` shows them all, `--dry-run` the steps",
        ),
    ),
    (
        "wind",
        Some(
            "a winding note at session start names a drafted rule: `jouet wind show ID` reads it, `jouet rule new NAME --from-note ID` adopts it",
        ),
    ),
    (
        "unwind",
        Some("a rule of yours keeps getting overridden or is noisy: it proposes retiring it"),
    ),
    ("share", None),
    ("agent-help", None),
    ("skill", None),
    ("setup", None),
];

/// Tools jouet works alongside rather than reimplementing, mentioned when
/// they are installed.
const COMPANIONS: &[(&str, &str)] = &[
    (
        "mbx",
        "mbx: Rust builds. Run cargo as usual (or prefix it with `mbx`); compiled output is shared across checkouts and builds are scheduled machine-wide. `mbx analyze` explains a slow rebuild",
    ),
    (
        "rtk",
        "rtk: shrinks the output of common commands; see `rtk --help`",
    ),
    (
        "bd",
        "bd (beads): issue tracking for agents; `bd ready` lists unblocked work",
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

/// The guide for the whole tool, kept under about 80 lines.
pub(crate) fn overview() -> String {
    let mut out = format!(
        "jouet {}: a toolkit CLI for coding agents. Reach for it before hand-rolling a script.\n",
        env!("CARGO_PKG_VERSION")
    );
    for (meta, when) in agent_commands() {
        let name = meta.cmd.name;
        let _ = writeln!(out, "\njouet {name}: {}", meta.about.unwrap_or(""));
        let _ = writeln!(out, "  when: {when}");
        let _ = writeln!(out, "  {}", synopsis(&format!("jouet {name}"), meta));
        for sub in meta.subcommands.iter().filter(|s| !s.hide) {
            let path = format!("jouet {name} {}", sub.cmd.name);
            let _ = writeln!(
                out,
                "  {}: {}",
                synopsis(&path, sub),
                sub.about.unwrap_or("")
            );
        }
    }
    out.push('\n');
    out.push_str(&conventions());
    for line in companions() {
        let _ = writeln!(out, "Also installed: {line}.");
    }
    out.push_str(
        "Details: `jouet agent-help COMMAND` for one command's flags, `jouet COMMAND --help`.\n",
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

fn on_path(bin: &str) -> bool {
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
    fn the_overview_stays_short() {
        let text = overview();
        // The cap is a budget to defend, not a ceiling to fill.
        assert!(text.lines().count() <= 80, "{text}");
        assert!(
            text.contains("jouet edit replace <PATH> --old TEXT --new TEXT"),
            "{text}"
        );
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
