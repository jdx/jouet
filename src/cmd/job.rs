//! `jouet job`: one-off background work. The runner is jouet-wait's.

use super::Output;
use crate::{JobArgs, JobCmd, JobLogsArgs, JobStartArgs, JobStopArgs};
use jouet_output::exit::Exit;
use jouet_output::time::{self, parse_duration};
use jouet_wait::Reporter;
use jouet_wait::job::{self, Job, Ready, Spec, State, Stopped, Store};
use serde::Serialize;
use std::fmt::Write as _;
use std::time::Duration;

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_LOG_LINES: usize = 50;

pub(crate) fn run(args: JobArgs, out: &Output) -> Exit {
    let store = match here() {
        Ok(store) => store,
        Err(message) => return out.error(Exit::Precondition, &message),
    };
    match args.cmd {
        JobCmd::Start(a) => start(a, store, out),
        JobCmd::Ls(_) => ls(&store, out),
        JobCmd::Logs(a) => logs(&a, &store, out),
        JobCmd::Stop(a) => stop(&a, &store, out),
    }
}

/// This work tree's jobs.
pub(crate) fn here() -> Result<Store, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("reading the current directory: {e}"))?;
    Store::here(&cwd).ok_or_else(|| "no state directory (set HOME or XDG_STATE_HOME)".into())
}

/// The ready check the flags give, at most one.
pub(crate) fn ready(
    port: Option<u16>,
    http: Option<String>,
    output: Option<String>,
) -> Result<Option<Ready>, String> {
    let mut given = Vec::new();
    if let Some(port) = port {
        if port == 0 {
            return Err("--port takes a port from 1 to 65535".into());
        }
        given.push(Ready::Port(port));
    }
    if let Some(url) = http {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!("`{url}` is not an http:// or https:// URL"));
        }
        given.push(Ready::Http(url));
    }
    if let Some(pattern) = output {
        regex::Regex::new(&pattern).map_err(|e| format!("--output is not a valid regex: {e}"))?;
        given.push(Ready::Output(pattern));
    }
    if given.len() > 1 {
        return Err("give one ready check: --port, --http or --output".into());
    }
    Ok(given.pop())
}

fn start(a: JobStartArgs, store: Store, out: &Output) -> Exit {
    if !job::valid_id(&a.id) {
        return out.error(
            Exit::Usage,
            &format!(
                "`{}` is not a job ID: use letters, digits, `_`, `.` and `-`",
                a.id
            ),
        );
    }
    if a.command.is_empty() {
        return out.error(
            Exit::Usage,
            "no command: write it after `--`, as in `jouet job start dev --port 3000 -- npm run dev`",
        );
    }
    let ready = match ready(a.port, a.http, a.output) {
        Ok(ready) => ready,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    let timeout = match a.timeout.as_deref().map(parse_duration) {
        None => DEFAULT_READY_TIMEOUT,
        Some(Ok(d)) => d,
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--timeout: {e}")),
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => return out.error(Exit::Failed, &format!("reading the current directory: {e}")),
    };
    let spec = Spec {
        id: a.id.clone(),
        command: a.command,
        cwd,
        ready,
        session: session_from_env(),
        replace: a.replace,
    };
    let mut reporter = Reporter::new(out.mode, out.json, false, false, format!("job {}", a.id));
    job::start(store, spec, timeout, &mut reporter)
}

/// The agent session a job is started in, so the stop hook can tell which
/// jobs a session left running.
fn session_from_env() -> Option<String> {
    [
        "JOUET_SESSION_ID",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
    ]
    .iter()
    .filter_map(|name| std::env::var(name).ok())
    .find(|v| !v.trim().is_empty())
}

/// One row of `job ls`.
#[derive(Serialize)]
pub(crate) struct Row {
    pub id: String,
    /// `running`, `exited`, `stopped` or `gone`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub pid: u32,
    pub command: Vec<String>,
    pub cwd: String,
    pub log: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<Ready>,
}

pub(crate) fn row(store: &Store, job: Job) -> Row {
    let state = store.state(&job);
    Row {
        id: job.id,
        state: state.name(),
        exit_code: state.exit_code(),
        pid: job.pid,
        command: job.command,
        cwd: job.cwd,
        log: job.log,
        started_at: job.started_at,
        ready: job.ready,
    }
}

fn ls(store: &Store, out: &Output) -> Exit {
    let rows: Vec<Row> = store.list().into_iter().map(|j| row(store, j)).collect();
    out.result(&rows, || {
        if rows.is_empty() {
            return "no jobs in this work tree; `jouet job start ID -- CMD` starts one\n".into();
        }
        let now = time::now();
        let width = rows.iter().map(|r| r.id.len()).max().unwrap_or(0);
        let mut s = String::new();
        for r in &rows {
            let state = match r.exit_code {
                Some(code) => format!("exited {code}"),
                None => r.state.to_string(),
            };
            let age = time::parse_utc(&r.started_at)
                .map(|t| format!("{} ago", time::age(now - t)))
                .unwrap_or_default();
            let _ = writeln!(
                s,
                "{:width$}  {state:9}  pid {:<7}  {age:>8}  {}  (log: {})",
                r.id,
                r.pid,
                shell_words(&r.command),
                r.log
            );
        }
        s
    });
    Exit::Ok
}

fn find(store: &Store, id: &str, out: &Output) -> Result<Job, Exit> {
    store.get(id).ok_or_else(|| {
        out.error(
            Exit::NotFound,
            &format!("no job `{id}` in this work tree; `jouet job ls` lists them"),
        )
    })
}

#[derive(Serialize)]
struct Logs {
    id: String,
    log: String,
    lines: Vec<String>,
    /// Lines before the ones shown.
    #[serde(skip_serializing_if = "is_zero")]
    omitted: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn logs(a: &JobLogsArgs, store: &Store, out: &Output) -> Exit {
    let job = match find(store, &a.id, out) {
        Ok(job) => job,
        Err(exit) => return exit,
    };
    let text = store.read_log(&job);
    let all: Vec<&str> = text.lines().collect();
    let want = a.lines.unwrap_or(DEFAULT_LOG_LINES);
    let omitted = if want == 0 {
        0
    } else {
        all.len().saturating_sub(want)
    };
    let doc = Logs {
        id: job.id.clone(),
        log: job.log.clone(),
        lines: all[omitted..].iter().map(|l| (*l).to_string()).collect(),
        omitted,
    };
    out.result(&doc, || {
        let mut s = String::new();
        if omitted > 0 {
            let _ = writeln!(s, "... {omitted} earlier lines (full log: {})", doc.log);
        }
        for line in &doc.lines {
            let _ = writeln!(s, "{line}");
        }
        s
    });
    Exit::Ok
}

#[derive(Serialize)]
struct StopRow {
    id: String,
    /// `term`, `kill`, or the state it had already ended in.
    result: String,
}

fn stop(a: &JobStopArgs, store: &Store, out: &Output) -> Exit {
    if a.ids.is_empty() {
        return out.error(
            Exit::Usage,
            "name the jobs to stop; `jouet job ls` lists them",
        );
    }
    let grace = match a.grace.as_deref().map(parse_duration) {
        None => job::STOP_GRACE,
        Some(Ok(d)) => d,
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--grace: {e}")),
    };
    // Every ID is checked before anything is signalled.
    let mut jobs = Vec::new();
    for id in &a.ids {
        match find(store, id, out) {
            Ok(job) => jobs.push(job),
            Err(exit) => return exit,
        }
    }
    let mut rows = Vec::new();
    for job in &jobs {
        let result = match job::stop(store, job, grace) {
            Ok(Stopped::Term) => "term".to_string(),
            Ok(Stopped::Kill) => "kill".to_string(),
            Ok(Stopped::Already(state)) => format!("already {}", describe(state)),
            Err(stop) => return out.error(stop.exit, &stop.message),
        };
        rows.push(StopRow {
            id: job.id.clone(),
            result,
        });
    }
    out.result(&rows, || {
        let mut s = String::new();
        for r in &rows {
            let line = match r.result.as_str() {
                "term" => format!("stopped {}", r.id),
                "kill" => format!("stopped {} (it ignored SIGTERM, so it got SIGKILL)", r.id),
                already => format!("{} had {already}", r.id),
            };
            let _ = writeln!(s, "{line}");
        }
        s
    });
    Exit::Ok
}

fn describe(state: State) -> String {
    match state {
        State::Exited(code) => format!("exited {code}"),
        State::Stopped => "been stopped".into(),
        other => other.name().into(),
    }
}

/// A command as a POSIX shell would need it written, for display.
pub(crate) fn shell_words(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            let plain = |c: char| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c);
            if !w.is_empty() && w.chars().all(plain) {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_ready_check_at_most() {
        assert_eq!(ready(None, None, None).unwrap(), None);
        assert_eq!(
            ready(Some(3000), None, None).unwrap(),
            Some(Ready::Port(3000))
        );
        assert!(ready(Some(1), Some("http://x".into()), None).is_err());
        assert!(ready(None, Some("localhost:3000".into()), None).is_err());
        assert!(ready(None, None, Some("(".into())).is_err());
        assert!(ready(Some(0), None, None).is_err());
    }

    #[test]
    fn commands_are_shown_quoted() {
        let words = ["sh".to_string(), "-c".into(), "echo 'hi'".into()];
        assert_eq!(shell_words(&words), r"sh -c 'echo '\''hi'\'''");
    }
}
