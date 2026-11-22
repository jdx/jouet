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
//!
//! Claude Code wakes the agent with everything the hook wrote to stderr, so
//! what the wait prints on the way (a rejected token, a retry) goes to the
//! log of a watcher that fails ([`Log`]), with the error that stopped it, and
//! nowhere otherwise. The ledger has that error's class.

use super::wait::{PushWatch, Tuning};
use crate::cli::WatchArgs;
use crate::ledger::{self, Entry, Settle};
use crate::settings::Settings;
use crate::setup::Harness;
use jouet_output::exit::Exit;
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
    // The hook runs as the push returns: what is posted from here on is new.
    let pushed_at = jouet_output::time::now();
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
        && let Err(e) = std::env::set_current_dir(cwd)
    {
        let class = Exit::Precondition.name();
        Log::open().error(
            "started no watcher",
            class,
            &format!("cannot enter {cwd}, the directory the call ran in: {e}"),
        );
        record_why(&settings, session, "error", Some(class));
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
    // From here to the digest, what is printed to stderr is kept for the
    // log: stderr is put back when the log is dropped, as this returns.
    let mut log = Log::open();
    match PushWatch::claim(session, pushed_at) {
        Ok(watcher) => {
            record(&settings, session, "started");
            log.say(&format!("watching {}", watcher.pr()));
            let ended = watcher.run(&tuning);
            if let Some(stop) = &ended.error {
                log.error("stopped on an error", stop.exit.name(), &stop.message);
            }
            ended.wake
        }
        Err(no) => {
            if let Some(stop) = &no.error {
                log.error("started no watcher", stop.exit.name(), &stop.message);
            }
            record_why(
                &settings,
                session,
                no.outcome,
                no.error.as_ref().map(|stop| stop.exit.name()),
            );
            None
        }
    }
}

/// Records what became of a push in the settle arm, unless the ledger is
/// turned off.
fn record(settings: &Settings, session: &str, outcome: &str) {
    record_why(settings, session, outcome, None);
}

/// The same, with the class of the error that kept a watcher from starting.
fn record_why(settings: &Settings, session: &str, outcome: &str, error: Option<&str>) {
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
        error: error.map(String::from),
        ..Settle::default()
    });
    ledger::append(&dir, &entry);
}

/// What a watcher that failed leaves behind, `watch-STAMP-PID.log` in
/// jouet's log directory: the PR it watched, what the wait printed on the way
/// (on Unix) and the error that stopped it or kept it from starting. On Unix
/// it stands in for stderr until it is dropped, so the agent is woken with
/// the digest alone. The file is written only for an error: a watcher that
/// ends any other way, also one the session's end stops, leaves none.
struct Log {
    path: std::path::PathBuf,
    said: String,
    #[cfg(unix)]
    diverted: Option<Diverted>,
}

impl Log {
    fn open() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = jouet_output::paths::log_dir()
            .join(format!("watch-{stamp}-{}.log", std::process::id()));
        Self {
            #[cfg(unix)]
            diverted: Diverted::start(&path),
            path,
            said: String::new(),
        }
    }

    fn say(&mut self, line: &str) {
        use std::fmt::Write;
        let now = jouet_output::time::format_utc(jouet_output::time::now());
        let _ = writeln!(self.said, "{now} jouet watch: {line}");
    }

    /// Writes the log, ending with what went wrong; `class` is the exit
    /// code's name, which the ledger records.
    fn error(&mut self, what: &str, class: &str, message: &str) {
        #[cfg(unix)]
        if let Some(diverted) = self.diverted.as_mut() {
            self.said.push_str(&diverted.printed());
        }
        self.say(&format!("{what} ({class}): {message}"));
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, &self.said);
    }
}

/// This process's stderr sent to a file with no name, which is gone with the
/// process, until this is dropped. Nothing is diverted when that file cannot
/// be made; the watcher goes on as it did.
#[cfg(unix)]
struct Diverted {
    printed: std::fs::File,
    /// The hook's own stderr, to put back.
    stderr: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl Diverted {
    /// `path` names the file only until it is open.
    fn start(path: &std::path::Path) -> Option<Self> {
        use std::os::fd::{AsFd, AsRawFd};
        std::fs::create_dir_all(path.parent()?).ok()?;
        let printed = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(path)
            .ok()?;
        std::fs::remove_file(path).ok()?;
        let stderr = std::io::stderr().as_fd().try_clone_to_owned().ok()?;
        // SAFETY: dup2(2) on two descriptors this process has open; it makes
        // descriptor 2 name the file and touches no memory.
        let done = unsafe { libc::dup2(printed.as_raw_fd(), libc::STDERR_FILENO) };
        (done != -1).then_some(Self { printed, stderr })
    }

    /// What was printed so far.
    fn printed(&mut self) -> String {
        use std::io::Seek;
        let mut text = Vec::new();
        if self.printed.rewind().is_ok() {
            let _ = self.printed.read_to_end(&mut text);
        }
        String::from_utf8_lossy(&text).into_owned()
    }
}

#[cfg(unix)]
impl Drop for Diverted {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: as in `start`: both descriptors are open and owned here.
        unsafe {
            libc::dup2(self.stderr.as_raw_fd(), libc::STDERR_FILENO);
        }
    }
}

/// Whether Claude Code runs this hook in the background, where a long wait
/// holds nothing up and exit 2 wakes the agent.
///
/// It runs an `asyncRewake` hook in the foreground in a one-shot run
/// (`claude -p PROMPT`), where the wait would hold the run itself, and
/// nothing in the hook's environment says which it is. So this reads the
/// command line of the Claude Code process, which `CLAUDE_PID` names: a run
/// that prints and takes no streamed input is one-shot. Anything it cannot
/// read, or cannot tell for sure, counts as the foreground.
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
            .is_some_and(|(argv, exact)| !one_shot(&argv, exact))
}

/// `2.1.318` as numbers.
fn version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|p| p.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// The arguments process `pid` was started with, and whether they are told
/// apart exactly. Where there is no `/proc` (macOS), `ps` prints them joined
/// by spaces, so a word of the prompt cannot be told from a flag.
fn command_line(pid: u32) -> Option<(Vec<String>, bool)> {
    if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let argv: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        return (!argv.is_empty()).then_some((argv, true));
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
    (!argv.is_empty()).then_some((argv, false))
}

/// Whether a Claude Code command line is a one-shot run: it prints (`-p`,
/// `--print`) and is not fed its input as a stream, as the SDK and the
/// desktop app feed it. When the arguments are not `exact`, what looks like
/// the stream flag may be words of the prompt (`claude -p "fix
/// --input-format stream-json"`), so a run that prints counts as one-shot.
fn one_shot(argv: &[String], exact: bool) -> bool {
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
    prints && !(exact && streams)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn only_a_printing_run_without_streamed_input_is_one_shot() {
        assert!(one_shot(&argv("claude -p hello"), true));
        assert!(one_shot(&argv("node cli.js --print hello"), true));
        assert!(one_shot(&argv("claude -pc"), true));
        assert!(one_shot(
            &argv("claude -p hello --output-format stream-json"),
            true
        ));
        // An interactive session, the SDK and the desktop app.
        let sdk = argv("claude -p --input-format stream-json --output-format stream-json");
        let desktop =
            argv("ccd-cli --output-format stream-json --verbose --input-format=stream-json");
        for exact in [true, false] {
            assert!(!one_shot(&argv("claude"), exact));
            assert!(!one_shot(&argv("claude --resume abc --model opus"), exact));
            assert!(!one_shot(&desktop, exact));
            // The program's own name is not a flag.
            assert!(!one_shot(&argv("-p"), exact));
        }
        assert!(!one_shot(&sdk, true));
        // The stream flag is one argument of the prompt here, which only
        // exact arguments show.
        let prompt = ["claude", "-p", "fix --input-format stream-json handling"].map(String::from);
        assert!(one_shot(&prompt, true));
        // Joined by spaces, that run and the SDK's look alike, so both count
        // as one-shot.
        assert!(one_shot(
            &argv("claude -p fix --input-format stream-json handling"),
            false
        ));
        assert!(one_shot(&sdk, false));
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
        let (argv, exact) = command_line(std::process::id()).unwrap();
        assert!(!argv.is_empty());
        assert_eq!(exact, std::path::Path::new("/proc/self/cmdline").exists());
        assert_eq!(command_line(u32::MAX - 1), None);
    }
}
