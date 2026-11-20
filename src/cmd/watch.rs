//! `jouet watch HARNESS`: the PR watcher jouet starts itself after a push.
//!
//! `jouet setup claude` installs it as a PostToolUse hook of its own, with
//! Claude Code's `asyncRewake`: the harness runs it in the background and,
//! when it exits 2, wakes the agent with what it printed on stderr. So this
//! command, unlike `jouet hook`, exits 2 on purpose: once, when the PR it
//! watched needs the agent, with the digest `jouet wait pr --until
//! needs-agent` prints, the Auto-fix events jouet held for it included.
//!
//! Everything else exits 0 at once and prints nothing: a call that is not a
//! push that worked, a session outside the settle arm (`crate::experiment`),
//! a PR a live watcher of the session already has, a branch with no PR, a
//! harness that would run the hook in the foreground, any error, and a
//! panic. A watcher that cannot go on gives its lease back, so no event is
//! held with nothing left to report it.

use super::wait::{PushWatch, Tuning};
use crate::cli::WatchArgs;
use crate::ledger::{self, Entry, Settle};
use crate::settings::Settings;
use crate::setup::Harness;
use jouet_output::time::parse_duration;
use jouet_rules::Call;
use serde_json::Value;
use std::io::Read;
use std::process::ExitCode;

/// The oldest Claude Code an `asyncRewake` hook was seen woken by. An older
/// one may not know the option and run the hook in the foreground, where the
/// wait would hold the agent's turn.
const MIN_CLAUDE_CODE: [u64; 3] = [2, 1, 318];

pub(crate) fn run(args: &WatchArgs) -> ExitCode {
    match std::panic::catch_unwind(|| watch(args)) {
        Ok(Some(digest)) => {
            eprint!("{digest}");
            if !digest.ends_with('\n') {
                eprintln!();
            }
            ExitCode::from(2)
        }
        _ => ExitCode::SUCCESS,
    }
}

/// What to wake the agent with, if anything.
fn watch(args: &WatchArgs) -> Option<String> {
    if args.harness.as_deref().and_then(Harness::parse) != Some(Harness::Claude) {
        return None;
    }
    let duration = |value: &Option<String>| value.as_deref().map(parse_duration).transpose().ok();
    let tuning = Tuning {
        quiet: duration(&args.quiet_for)?,
        timeout: duration(&args.timeout)?,
        interval: duration(&args.interval)?.filter(|i| !i.is_zero()),
    };
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok()?;
    let payload: Value = serde_json::from_str(&input).ok()?;
    let call = Call::from_hook(&payload, Harness::Claude.name());
    let session = call.session.as_deref()?;
    if !crate::settle::pushed(&payload, &call) {
        return None;
    }
    let settings = Settings::load();
    if !settings.hooks.enabled || !settings.arm(Some(session)).settle {
        return None;
    }
    // One watcher for a call, however many hook entries it started: one per
    // filter the command matches, and one per scope jouet is installed in.
    if !super::hook::first_run(Harness::Claude, "watch", &payload) {
        return None;
    }
    // The PR is the one of the branch checked out where the agent pushed.
    if let Some(cwd) = call.cwd.as_deref().filter(|cwd| !cwd.is_empty())
        && std::env::set_current_dir(cwd).is_err()
    {
        record(&settings, session, "error");
        return None;
    }
    if !backgrounded() {
        record(&settings, session, "foreground");
        return None;
    }
    jouet_github::entire::configure(
        settings.entire.enabled,
        std::time::Duration::from_millis(settings.entire.timeout_ms),
    );
    match PushWatch::claim(session) {
        Ok(watcher) => {
            record(&settings, session, "started");
            watcher.run(&tuning)
        }
        Err(outcome) => {
            record(&settings, session, outcome);
            None
        }
    }
}

/// Records what became of a push in the settle arm, unless the ledger is
/// turned off.
fn record(settings: &Settings, session: &str, outcome: &str) {
    let Some(dir) = ledger::dir().filter(|_| settings.hooks.ledger) else {
        return;
    };
    let mut entry = Entry::new(Harness::Claude.name(), Some(session), "watch");
    entry.repo = std::env::current_dir()
        .ok()
        .and_then(|cwd| ledger::repo_key(&cwd));
    entry.settle = Some(Settle {
        result: outcome.into(),
        watcher: Some(std::process::id()),
        ..Settle::default()
    });
    ledger::append(&dir, &entry);
}

/// Whether Claude Code runs this hook in the background, where a long wait
/// holds nothing up and exit 2 wakes the agent.
///
/// It runs an `asyncRewake` hook in the foreground in a one-shot run
/// (`claude -p PROMPT`), where the wait would hold the run itself, and
/// nothing in the hook's environment says which it is. So this reads the
/// command line of the Claude Code process, which `CLAUDE_PID` names: a run
/// that prints and takes no streamed input is one-shot. Anything it cannot
/// read counts as the foreground.
fn backgrounded() -> bool {
    let agent = jouet_git::agent::Agent::from_env();
    let recent = agent.id.as_deref() == Some("claude-code")
        && agent
            .version
            .as_deref()
            .and_then(version)
            .is_some_and(|v| v >= MIN_CLAUDE_CODE);
    recent
        && std::env::var("CLAUDE_PID")
            .ok()
            .and_then(|pid| pid.trim().parse().ok())
            .and_then(command_line)
            .is_some_and(|argv| !one_shot(&argv))
}

/// `2.1.318` as numbers.
fn version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|p| p.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// The arguments process `pid` was started with.
fn command_line(pid: u32) -> Option<Vec<String>> {
    if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let argv: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        return (!argv.is_empty()).then_some(argv);
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let argv: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (!argv.is_empty()).then_some(argv)
}

/// Whether a Claude Code command line is a one-shot run: it prints (`-p`,
/// `--print`) and is not fed its input as a stream, as the SDK and the
/// desktop app feed it.
fn one_shot(argv: &[String]) -> bool {
    let prints = argv.iter().skip(1).any(|arg| {
        arg == "--print"
            || arg.strip_prefix('-').is_some_and(|flags| {
                flags.contains('p') && flags.chars().all(|c| c.is_ascii_alphabetic())
            })
    });
    let streams = argv
        .windows(2)
        .any(|pair| pair[0] == "--input-format" && pair[1] == "stream-json")
        || argv.iter().any(|arg| arg == "--input-format=stream-json");
    prints && !streams
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn only_a_printing_run_without_streamed_input_is_one_shot() {
        assert!(one_shot(&argv("claude -p hello")));
        assert!(one_shot(&argv("node cli.js --print hello")));
        assert!(one_shot(&argv("claude -pc")));
        assert!(one_shot(&argv(
            "claude -p hello --output-format stream-json"
        )));
        // An interactive session, the SDK and the desktop app.
        assert!(!one_shot(&argv("claude")));
        assert!(!one_shot(&argv("claude --resume abc --model opus")));
        assert!(!one_shot(&argv(
            "claude -p --input-format stream-json --output-format stream-json"
        )));
        assert!(!one_shot(&argv(
            "ccd-cli --output-format stream-json --verbose --input-format=stream-json"
        )));
        // The program's own name is not a flag.
        assert!(!one_shot(&argv("-p")));
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(version("2.1.318"), Some([2, 1, 318]));
        assert!(version("2.1.99").unwrap() < MIN_CLAUDE_CODE);
        assert!(version("2.2.0").unwrap() >= MIN_CLAUDE_CODE);
        assert_eq!(version("2.1"), None);
        assert_eq!(version("2.1.318.4"), None);
        assert_eq!(version("two"), None);
    }

    #[test]
    fn a_process_s_command_line_is_read() {
        let argv = command_line(std::process::id()).unwrap();
        assert!(!argv.is_empty());
        assert_eq!(command_line(u32::MAX - 1), None);
    }
}
