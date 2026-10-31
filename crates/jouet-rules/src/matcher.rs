//! Running a TOML rule's matchers against a tool call. This is plain Rust
//! over the shell tokenizer's commands: no Luau VM starts for a rule that
//! only matches.

use crate::decl::{Flag, Matcher};
use crate::{Call, glob, shell};
use std::path::{Component, Path, PathBuf};

/// Whether any of the matchers matches the call, whose commands are
/// `commands` (every command it runs, nested ones included).
pub(crate) fn any(matchers: &[Matcher], call: &Call, commands: &[shell::Command]) -> bool {
    matchers.iter().any(|m| matches(m, call, commands))
}

/// Which of `commands` the matchers matched: each command a matcher's
/// `command` names, when the matcher's other conditions hold.
pub(crate) fn matched(matchers: &[Matcher], call: &Call, commands: &[shell::Command]) -> Vec<bool> {
    let mut out = vec![false; commands.len()];
    for m in matchers
        .iter()
        .filter(|m| !m.command.is_empty() && conditions(m, call))
    {
        for (i, hit) in out.iter_mut().enumerate() {
            *hit = *hit || matches_at(m, i, call, commands);
        }
    }
    out
}

fn matches(m: &Matcher, call: &Call, commands: &[shell::Command]) -> bool {
    if !conditions(m, call) {
        return false;
    }
    if m.command.is_empty() {
        return true;
    }
    (0..commands.len()).any(|i| matches_at(m, i, call, commands))
}

/// Whether the command at `i` is one the matcher's `command` names, with its
/// `with` and `unless_with` met by the call's other commands.
fn matches_at(m: &Matcher, i: usize, call: &Call, commands: &[shell::Command]) -> bool {
    let other = |with: &Matcher| {
        commands
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && command(with, other, call))
    };
    command(m, &commands[i], call)
        && m.with.as_deref().is_none_or(other)
        && !m.unless_with.as_deref().is_some_and(other)
}

/// Whether the call meets everything the matcher asks besides its `command`.
fn conditions(m: &Matcher, call: &Call) -> bool {
    if !m.tool.is_empty() && !m.tool.iter().any(|g| glob::matches(&g.0, &call.tool)) {
        return false;
    }
    if !m.paths.is_empty() {
        let Some(path) = call.path.as_deref() else {
            return false;
        };
        let path = relative(path, call.cwd.as_deref());
        if !m.paths.iter().any(|g| glob::matches_path(&g.0, path)) {
            return false;
        }
    }
    if let Some(path) = call.path.as_deref() {
        let path = relative(path, call.cwd.as_deref());
        if m.unless_paths
            .iter()
            .any(|g| glob::matches_path(&g.0, path))
        {
            return false;
        }
    }
    if m.fnox.is_some_and(|want| call.fnox.is_some() != want) {
        return false;
    }
    // Without a command, `secret_files` is about a file tool's path.
    if m.command.is_empty()
        && let Some(want) = m.secret_files
        && call.path.as_deref().is_some_and(|p| secret_file(call, p)) != want
    {
        return false;
    }
    if !input(m, call) {
        return false;
    }
    if m.seen
        .is_some_and(|want| call.seen.unwrap_or(false) != want)
    {
        return false;
    }
    if m.min_read_lines
        .is_some_and(|min| call.read_lines.is_none_or(|n| n < min))
    {
        return false;
    }
    if m.min_code_lines
        .is_some_and(|min| call.code_lines.is_none_or(|n| n < min))
    {
        return false;
    }
    // An unknown count holds: outside a repository, a search can be large.
    if m.min_tracked_files
        .is_some_and(|min| call.tracked_files.is_some_and(|n| n < min))
    {
        return false;
    }
    true
}

/// Whether the tool's input has the fields `input` names, with matching
/// values, and none of those `unless_input` names.
fn input(m: &Matcher, call: &Call) -> bool {
    let field = |name: &str| {
        call.input
            .as_ref()
            .and_then(|i| i.get(name))
            .filter(|v| !v.is_null())
    };
    m.input.iter().all(|(name, want)| {
        field(name).is_some_and(|v| match v {
            serde_json::Value::String(text) => glob::matches(&want.0, text),
            other => glob::matches(&want.0, &other.to_string()),
        })
    }) && !m.unless_input.iter().any(|name| field(name).is_some())
}

/// Whether the harness runs the whole call in the background, as Claude
/// Code's Bash tool does with `run_in_background`.
fn in_background(call: &Call) -> bool {
    call.input
        .as_ref()
        .and_then(|i| i.get("run_in_background"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

/// The path from the working directory, when it is inside it.
fn relative<'a>(path: &'a str, cwd: Option<&str>) -> &'a str {
    cwd.map(|cwd| cwd.trim_end_matches(['/', '\\']))
        .filter(|cwd| !cwd.is_empty())
        .and_then(|cwd| path.strip_prefix(cwd))
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
        .unwrap_or(path)
}

/// Whether `path` is one of the files the project's fnox config reads
/// secrets from, both read from the working directory.
fn secret_file(call: &Call, path: &str) -> bool {
    let Some(fnox) = &call.fnox else {
        return false;
    };
    let cwd = call.cwd.as_deref().map(Path::new);
    let full = |p: &str| {
        let p = Path::new(p);
        normalize(&match cwd {
            Some(cwd) if p.is_relative() => cwd.join(p),
            _ => p.to_path_buf(),
        })
    };
    let path = full(path);
    fnox.files.iter().any(|f| full(f) == path)
}

/// Folds `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `name` is a secret the project's fnox config defines.
fn secret(call: &Call, name: &str) -> bool {
    call.fnox
        .as_ref()
        .is_some_and(|f| f.secrets.iter().any(|s| s == name))
}

/// Whether one command matches the matcher's command keys.
fn command(m: &Matcher, c: &shell::Command, call: &Call) -> bool {
    let argv = c.argv();
    let Some((first, args)) = argv.split_first() else {
        return false;
    };
    let program = shell::basename(first);
    let Some(rest) = m.command.iter().find_map(|w| after(&w.0, program, args)) else {
        return false;
    };
    if !m.via.is_empty() {
        let via = c.via();
        if !m.via.iter().any(|v| via.contains(&v.as_str())) {
            return false;
        }
    }
    let words = Words::read(rest, &m.value_flags);
    if !m.flags.is_empty() && !m.flags.iter().any(|f| words.has(f)) {
        return false;
    }
    if m.unless_flags.iter().any(|f| words.has(f)) {
        return false;
    }
    if let Some(exact) = &m.exact_flags {
        let mut want: Vec<&str> = exact.iter().map(|f| f.0.as_str()).collect();
        want.sort_unstable();
        want.dedup();
        let mut have: Vec<&str> = words.flags.iter().map(String::as_str).collect();
        have.sort_unstable();
        have.dedup();
        if want != have {
            return false;
        }
    }
    let arg_matches = |globs: &[crate::decl::Glob]| {
        words
            .args
            .iter()
            .any(|a| globs.iter().any(|g| glob::matches(&g.0, a)))
    };
    if !m.args.is_empty() && !arg_matches(&m.args) {
        return false;
    }
    if arg_matches(&m.unless_args) {
        return false;
    }
    if m.max_args.is_some_and(|max| words.args.len() > max) {
        return false;
    }
    if m.background.is_some_and(|want| {
        let backgrounded = in_background(call) || c.op == Some(shell::Op::Background);
        backgrounded != want
    }) {
        return false;
    }
    if m.secret_files
        .is_some_and(|want| words.args.iter().any(|a| secret_file(call, a)) != want)
    {
        return false;
    }
    if m.secret_vars
        .is_some_and(|want| c.vars.iter().any(|v| secret(call, v)) != want)
    {
        return false;
    }
    if m.secret_args
        .is_some_and(|want| words.args.iter().any(|a| secret(call, a)) != want)
    {
        return false;
    }
    let prints = c.stdout_to.is_none() && !c.captured && c.op != Some(shell::Op::Pipe);
    if m.prints.is_some_and(|want| prints != want) {
        return false;
    }
    m.text.iter().all(|pattern| {
        args.iter()
            .chain(&c.heredocs)
            .any(|text| pattern.0.is_match(text))
    })
}

/// The words after the program and subcommands `words` names, when the
/// command is that program and those subcommands.
fn after<'a>(words: &[String], program: &str, args: &'a [String]) -> Option<&'a [String]> {
    let (name, subcommands) = words.split_first()?;
    if !glob::matches(name, program) {
        return None;
    }
    let mut rest = args;
    if !subcommands.is_empty() {
        rest = skip_global_options(program, rest);
    }
    for word in subcommands {
        let (first, tail) = rest.split_first()?;
        if !glob::matches(word, first) {
            return None;
        }
        rest = tail;
    }
    Some(rest)
}

/// Skips the options before a program's first subcommand, and their
/// values where jouet knows them: `git -C dir add`, `cargo +nightly test`.
fn skip_global_options<'a>(program: &str, args: &'a [String]) -> &'a [String] {
    let takes_value: &[&str] = match program {
        "git" => &[
            "-C",
            "-c",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--exec-path",
        ],
        _ => &[],
    };
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let toolchain = program == "cargo" && i == 0 && arg.starts_with('+');
        if !toolchain && (arg == "--" || arg == "-" || !arg.starts_with('-')) {
            break;
        }
        i += if takes_value.contains(&arg.as_str()) {
            2
        } else {
            1
        };
    }
    args.get(i..).unwrap_or_default()
}

/// A command's words after its subcommands, read as options and
/// arguments.
struct Words<'a> {
    /// Each option given, as `-x` or `--long`, without its value.
    flags: Vec<String>,
    /// The words that are neither options nor their values.
    args: Vec<&'a str>,
}

impl<'a> Words<'a> {
    fn read(words: &'a [String], value_flags: &[Flag]) -> Self {
        let takes_value = |flag: &str| value_flags.iter().any(|f| f.0 == flag);
        let mut out = Self {
            flags: Vec::new(),
            args: Vec::new(),
        };
        let mut i = 0;
        let mut options_done = false;
        while let Some(word) = words.get(i) {
            i += 1;
            if options_done || word == "-" || !word.starts_with('-') {
                out.args.push(word);
            } else if word == "--" {
                options_done = true;
            } else if let Some(long) = word.strip_prefix("--") {
                let (name, value) = match long.split_once('=') {
                    Some((name, _)) => (name, true),
                    None => (long, false),
                };
                let flag = format!("--{name}");
                if !value && takes_value(&flag) {
                    i += 1;
                }
                out.flags.push(flag);
            } else {
                // A cluster such as `-fu`; an option that takes a value
                // takes the rest of the word, or the next word.
                let cluster = &word[1..];
                for (at, c) in cluster.char_indices() {
                    let flag = format!("-{c}");
                    let value = takes_value(&flag);
                    out.flags.push(flag);
                    if value {
                        if at + c.len_utf8() == cluster.len() {
                            i += 1;
                        }
                        break;
                    }
                }
            }
        }
        out
    }

    fn has(&self, flag: &Flag) -> bool {
        self.flags.contains(&flag.0)
    }
}

#[cfg(test)]
mod tests;
