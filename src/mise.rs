//! The `mise` on PATH, which jouet writes a project's tasks through: its
//! version, the tasks a project has, and `mise tasks add`, `set` and `rm`,
//! whose `--json` reports what changed. jouet never edits a mise config
//! file itself.

use crate::wind::tasks::Existing;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

/// The first mise release whose `tasks add` chooses the file and prints
/// JSON, and that has `tasks set` and `tasks rm`.
pub(crate) const MIN_VERSION: &str = "2026.10.25";

/// Why mise could not do what jouet asked.
#[derive(Debug)]
pub(crate) enum Problem {
    /// No `mise` on PATH.
    Missing,
    /// Older than [`MIN_VERSION`]; the version it reported.
    TooOld(String),
    /// It ran and failed, or printed something jouet could not read.
    Failed(String),
}

impl Problem {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Missing => format!(
                "mise is not installed; jouet writes tasks through mise {MIN_VERSION} or later"
            ),
            Self::TooOld(version) => format!(
                "this needs mise {MIN_VERSION} or later, for `mise tasks set` and `rm` and their JSON; the mise on PATH is {version}"
            ),
            Self::Failed(message) => message.clone(),
        }
    }
}

fn run(root: &Path, args: &[String]) -> Result<std::process::Output, Problem> {
    Command::new("mise")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Problem::Missing,
            _ => Problem::Failed(format!("running mise: {e}")),
        })
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

/// `YYYY.M.P` from the start of `mise --version`'s output.
fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().next()?;
    let word = word.split('-').next()?;
    let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Whether the mise on PATH can write tasks as jouet needs, by its
/// version; a build older by version number but with `tasks set` (one
/// from mise's main branch) passes too. Returns the version.
pub(crate) fn check(root: &Path) -> Result<String, Problem> {
    let out = run(root, &strings(&["--version"]))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let version = text
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_string();
    let min = parse_version(MIN_VERSION);
    if parse_version(&text).is_some_and(|v| Some(v) >= min) {
        return Ok(version);
    }
    // Before `set` existed, `mise tasks set --help` is the help of `mise
    // tasks`, which has no --unset.
    let help = run(root, &strings(&["tasks", "set", "--help"]))?;
    if help.status.success() && String::from_utf8_lossy(&help.stdout).contains("--unset") {
        return Ok(version);
    }
    Err(Problem::TooOld(version))
}

/// The tasks mise sees in `root`, by `mise tasks ls --json`.
pub(crate) fn tasks(root: &Path) -> Result<Vec<Existing>, Problem> {
    let value = json(root, &strings(&["tasks", "ls", "--json"]))?;
    let Value::Array(items) = value else {
        return Err(Problem::Failed(
            "`mise tasks ls --json` printed no list".into(),
        ));
    };
    Ok(items.iter().filter_map(existing).collect())
}

fn existing(item: &Value) -> Option<Existing> {
    let name = item.get("name")?.as_str()?.to_string();
    let run = match item.get("run") {
        Some(Value::Array(runs)) => runs
            .iter()
            .map(|r| match r {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    Some(Existing { name, run })
}

/// Runs `mise ARGS` in `root` and reads the JSON it prints.
pub(crate) fn json(root: &Path, args: &[String]) -> Result<Value, Problem> {
    let out = run(root, args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Problem::Failed(format!(
            "`mise {}` failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| Problem::Failed(format!("`mise {}` printed no JSON: {e}", args.join(" "))))
}

/// The words to give `mise tasks add` after `--` so that the task runs
/// `command`. mise quotes each word it is given, so a command of plain
/// words goes as its words, and anything with shell syntax (operators,
/// quotes, redirections, an assignment) runs through `sh -c`.
pub(crate) fn run_words(command: &str) -> Vec<String> {
    let plain = |w: &str| {
        let value = w.starts_with('-') && w.contains('=');
        w.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:@%+,-".contains(c) || (value && c == '='))
    };
    let words: Vec<&str> = command.split_whitespace().collect();
    if !words.is_empty() && words.iter().all(|w| plain(w)) {
        words.into_iter().map(str::to_string).collect()
    } else {
        vec!["sh".into(), "-c".into(), command.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_their_numbers() {
        assert_eq!(
            parse_version("2026.10.21 linux-x64 (2026-10-27)"),
            Some((2026, 10, 21))
        );
        assert_eq!(
            parse_version("2026.10.24-DEBUG linux-x64"),
            Some((2026, 10, 24))
        );
        assert!(parse_version("2026.11.0").unwrap() > parse_version(MIN_VERSION).unwrap());
        assert_eq!(parse_version("mise"), None);
    }

    #[test]
    fn plain_commands_go_as_words_and_the_rest_through_sh() {
        assert_eq!(
            run_words("cargo test -p jouet-edit"),
            ["cargo", "test", "-p", "jouet-edit"]
        );
        assert_eq!(
            run_words("cargo test --features=x"),
            ["cargo", "test", "--features=x"]
        );
        assert_eq!(
            run_words("cargo build && cargo test"),
            ["sh", "-c", "cargo build && cargo test"]
        );
        assert_eq!(
            run_words("RUST_LOG=1 cargo test"),
            ["sh", "-c", "RUST_LOG=1 cargo test"]
        );
        assert_eq!(run_words("echo 'a b'"), ["sh", "-c", "echo 'a b'"]);
    }

    #[test]
    fn tasks_ls_runs_are_strings_or_lists() {
        let items: Value = serde_json::json!([
            { "name": "build", "run": ["cargo build"] },
            { "name": "lint", "run": "cargo clippy" },
            { "name": "all", "run": [{ "task": "build" }] },
            { "name": "file" },
        ]);
        let tasks: Vec<Existing> = items
            .as_array()
            .unwrap()
            .iter()
            .filter_map(existing)
            .collect();
        assert_eq!(tasks[0].run, ["cargo build"]);
        assert_eq!(tasks[1].run, ["cargo clippy"]);
        assert_eq!(tasks[2].run, [r#"{"task":"build"}"#]);
        assert!(tasks[3].run.is_empty());
    }
}
