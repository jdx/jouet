//! jouet as its own sessions show it: where it crashed, exited with a code
//! outside its table, printed `--json` output that is not JSON, met usage
//! errors, or was dropped for raw `gh` or `git` right after it answered.
//!
//! The same scan runs over a transcript's calls and over the hook ledger's
//! shapes, so a finding names a command by its subject (`jouet pr status`)
//! and never by its values.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// It panicked (exit 101, or `panicked at` in its output).
    Crash,
    /// It exited with a code outside the shared table.
    ExitCode,
    /// `--json` printed something that does not start a JSON document.
    BadJson,
    /// It exited 2, a usage error.
    Usage,
    /// It answered, and the agent ran the raw command it replaces next.
    Abandoned,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::ExitCode => "exit_code",
            Self::BadJson => "bad_json",
            Self::Usage => "usage",
            Self::Abandoned => "abandoned",
        }
    }
}

/// One command an agent ran, and how it ended.
#[derive(Debug, Clone, Copy)]
pub struct Run<'a> {
    /// The command, or its shape.
    pub command: &'a str,
    pub exit_code: Option<i32>,
    /// The start of what it printed, when known.
    pub output: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    pub kind: Kind,
    /// The jouet command, such as `jouet pr status`.
    pub subject: String,
    /// The run it was found at, as an index into the runs scanned.
    #[serde(skip)]
    pub index: usize,
    /// What was seen, without values: `exit 7`, `panicked at
    /// src/cmd/pr.rs:41:9`, `then gh pr view`.
    pub detail: String,
}

/// The exit codes every jouet command may use; `pr next` adds 10 and 20.
const TABLE: &[i32] = &[0, 1, 2, 3, 4, 5, 124, 130];

/// What an agent runs instead of a jouet command, when its answer fell
/// short.
const REPLACES: &[(&str, &[&str])] = &[
    ("jouet pr", &["gh pr", "gh api"]),
    ("jouet wait", &["gh run", "gh pr checks", "gh api", "sleep"]),
    ("jouet commit", &["git commit", "git add"]),
    ("jouet review-package", &["git diff", "git log", "git show"]),
    ("jouet handoff", &["git status", "git log"]),
    ("jouet pickup", &["git status", "git log"]),
];

/// The jouet command a script runs: `jouet` and up to two subcommand words,
/// such as `jouet pr status`; `joue` and its routine's name.
pub fn subject(command: &str) -> Option<String> {
    crate::structure::simple_words(command)
        .into_iter()
        .find_map(|(words, _)| {
            let argv = super::unwrapped(&words);
            let program = argv.first()?.rsplit('/').next()?;
            if program != "jouet" && program != "joue" {
                return None;
            }
            let mut subject = program.to_string();
            let depth = if program == "jouet" { 2 } else { 1 };
            for word in argv[1..].iter().take(depth) {
                if !word.starts_with(|c: char| c.is_ascii_lowercase())
                    || !word
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    break;
                }
                subject.push(' ');
                subject.push_str(word);
            }
            Some(subject)
        })
}

/// Whether `code` is one jouet itself could have meant. Codes from 126 up
/// belong to the shell (not executable, not found, a signal), not to jouet.
fn expected(subject: &str, code: i32) -> bool {
    TABLE.contains(&code)
        || !(0..126).contains(&code)
        || subject.starts_with("jouet pr next") && matches!(code, 10 | 20)
}

/// `panicked at src/x.rs:41:9`, without the message, which may hold a value.
fn panic_site(output: &str) -> Option<String> {
    let at = output.find("panicked at ")?;
    let rest = &output[at + "panicked at ".len()..];
    let site: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect::<String>()
        .trim_end_matches(':')
        .to_string();
    Some(format!("panicked at {site}"))
}

/// A run's whole script is one jouet command with `--json`, its stdout not
/// piped or merged with stderr: what it printed is jouet's document.
fn json_alone(command: &str) -> bool {
    let words = crate::structure::simple_words(command);
    words.len() == 1 && words[0].0.iter().any(|w| w == "--json") && !command.contains("2>&1")
}

pub fn scan(runs: &[Run<'_>]) -> Vec<Found> {
    let mut out = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let Some(subject) = subject(run.command) else {
            continue;
        };
        let found = |kind, detail: String| Found {
            kind,
            subject: subject.clone(),
            index,
            detail,
        };
        let output = run.output.unwrap_or_default();
        if run.exit_code == Some(101) || output.contains("panicked at ") {
            let site = panic_site(output).unwrap_or_else(|| "exit 101".into());
            out.push(found(Kind::Crash, site));
            continue;
        }
        if let Some(code) = run.exit_code
            && !expected(&subject, code)
        {
            out.push(found(Kind::ExitCode, format!("exit {code}")));
            continue;
        }
        let printed = output.trim_start();
        if json_alone(run.command)
            && !printed.is_empty()
            && !printed.starts_with(['{', '['])
            && run.output.is_some()
        {
            out.push(found(
                Kind::BadJson,
                "`--json` output did not start a JSON document".into(),
            ));
        }
        let next = runs
            .get(index + 1..(index + 4).min(runs.len()))
            .unwrap_or_default();
        if run.exit_code == Some(2) {
            let retried = next.iter().any(|n| {
                subject_of(n).as_deref() == Some(subject.as_str()) && n.command != run.command
            });
            out.push(found(
                Kind::Usage,
                if retried {
                    "exit 2, then retried with other flags".into()
                } else {
                    "exit 2".into()
                },
            ));
            continue;
        }
        if run.exit_code == Some(0)
            && let Some(raw) = abandoned_for(&subject, &next[..next.len().min(2)])
        {
            out.push(found(Kind::Abandoned, format!("then `{raw}`")));
        }
    }
    out
}

fn subject_of(run: &Run<'_>) -> Option<String> {
    subject(run.command)
}

/// The raw command, by its key, that one of `next` runs in place of
/// `subject`.
fn abandoned_for(subject: &str, next: &[Run<'_>]) -> Option<String> {
    let (_, raw) = REPLACES.iter().find(|(s, _)| subject.starts_with(s))?;
    next.iter().find_map(|n| {
        if subject_of(n).is_some() {
            return None;
        }
        let key = super::command_key(n.command);
        raw.iter()
            .any(|r| {
                key.split(" + ")
                    .any(|k| k == *r || k.starts_with(&format!("{r} ")))
            })
            .then_some(key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &str, exit_code: i32) -> Run<'_> {
        Run {
            command,
            exit_code: Some(exit_code),
            output: None,
        }
    }

    fn kinds(runs: &[Run<'_>]) -> Vec<(Kind, String, String)> {
        scan(runs)
            .into_iter()
            .map(|f| (f.kind, f.subject, f.detail))
            .collect()
    }

    #[test]
    fn subjects_name_the_jouet_command_without_values() {
        assert_eq!(
            subject("jouet pr status 12 --json").as_deref(),
            Some("jouet pr status")
        );
        assert_eq!(
            subject("cd x && env -u GH_TOKEN jouet wait ci 15002").as_deref(),
            Some("jouet wait ci")
        );
        assert_eq!(
            subject("joue triage --dry-run").as_deref(),
            Some("joue triage")
        );
        assert_eq!(subject("jouet --version").as_deref(), Some("jouet"));
        assert_eq!(subject("gh pr view 12"), None);
        assert_eq!(subject("echo jouet pr status"), None);
    }

    #[test]
    fn crashes_and_codes_outside_the_table_are_bugs() {
        let mut crash = run("jouet pr threads 7", 101);
        crash.output = Some(
            "thread 'main' panicked at src/cmd/threads.rs:41:9:\nindex out of range for secret-repo",
        );
        assert_eq!(
            kinds(&[
                crash,
                run("jouet wait ci", 7),
                run("jouet pr next", 20),
                run("jouet x", 127)
            ]),
            [
                (
                    Kind::Crash,
                    "jouet pr threads".into(),
                    "panicked at src/cmd/threads.rs:41:9".into()
                ),
                (Kind::ExitCode, "jouet wait ci".into(), "exit 7".into()),
            ]
        );
    }

    #[test]
    fn json_that_is_not_json_is_a_bug_when_nothing_else_printed_it() {
        let bad = Run {
            command: "jouet pr status --json",
            exit_code: Some(0),
            output: Some("warning: something\n{\"prs\":[]}"),
        };
        let piped = Run {
            command: "jouet pr status --json | jq .prs",
            ..bad
        };
        let good = Run {
            output: Some("  {\"prs\":[]}"),
            ..bad
        };
        assert_eq!(kinds(&[bad]).len(), 1);
        assert!(kinds(&[piped, good]).is_empty());
    }

    #[test]
    fn usage_errors_and_retries_with_other_flags() {
        assert_eq!(
            kinds(&[
                run("jouet wind digest --session x", 2),
                run("jouet wind digest x", 0),
                run("jouet edit --nope", 2),
                run("ls", 0),
            ]),
            [
                (
                    Kind::Usage,
                    "jouet wind digest".into(),
                    "exit 2, then retried with other flags".into()
                ),
                (Kind::Usage, "jouet edit".into(), "exit 2".into()),
            ]
        );
    }

    #[test]
    fn a_raw_command_right_after_an_answer_is_an_abandonment() {
        assert_eq!(
            kinds(&[
                run("jouet pr status 12", 0),
                run("env -u GH_TOKEN gh pr view 12 --json mergeable", 0),
                run("jouet commit src/a.rs -m x", 0),
                run("cargo test", 0),
                run("ls", 0),
                run("git commit -m y", 0),
            ]),
            [(
                Kind::Abandoned,
                "jouet pr status".into(),
                "then `gh pr view`".into()
            )]
        );
    }
}
