//! `jouet job`: one-off background work, such as a build, a test suite, a
//! long script or a server started for one check, run without a daemon or
//! a supervisor.
//!
//! `start` spawns the command detached, in a session and process group of
//! its own (`setsid`), with stdin closed and stdout and stderr appended to
//! one log. The command runs under `sh`, which writes its exit status to a
//! file when it ends, so a job's exit code is known after jouet has gone.
//! Each job is a JSON file under `$XDG_STATE_HOME/jouet/jobs/<work tree>/`,
//! beside its log and its exit file, so jobs belong to the work tree they
//! were started in.
//!
//! A PID can be reused once its process has exited, so a job is trusted to
//! still be running only while its PID leads the process group it started
//! (`setsid` makes them the same number) and has the start time recorded
//! when it was spawned. Only then is it signalled.
//!
//! Jobs run on Unix. Elsewhere `start` and `stop` refuse with exit 4.

use super::{Detail, Observation, Outcome, Reporter, Stop, Subject, Timing};
use jouet_output::exit::Exit;
use jouet_output::time::elapsed;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long `stop` waits after SIGTERM before SIGKILL, unless told.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long a finished job's files are kept.
const KEEP_FINISHED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What makes a started job ready, recorded so `wait job --until ready` can
/// ask again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ready {
    /// localhost's port accepts connections.
    Port(u16),
    /// The URL answers with a 2xx status.
    Http(String),
    /// A line of the job's log matches the regex.
    Output(String),
}

impl Ready {
    /// The subject that asks once whether the job is ready.
    fn subject(&self, log: &Path) -> Result<Box<dyn Subject>, String> {
        Ok(match self {
            Self::Port(port) => Box::new(crate::local::PortWait::parse(&port.to_string(), false)?),
            Self::Http(url) => Box::new(crate::local::HttpWait::new(url.clone(), None, None)),
            Self::Output(pattern) => {
                let re = regex::Regex::new(pattern)
                    .map_err(|e| format!("--output is not a valid regex: {e}"))?;
                Box::new(crate::local::LogWait::new(log.to_path_buf(), re, true))
            }
        })
    }
}

/// One job, as its file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// The `sh` the command runs under; it leads the job's process group.
    pub pid: u32,
    pub pgid: u32,
    pub command: Vec<String>,
    pub cwd: String,
    pub log: String,
    /// RFC 3339, UTC.
    pub started_at: String,
    /// The process's start time as the OS reports it, to tell it from a
    /// later process that got the same PID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<String>,
    /// The agent session that started it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<Ready>,
    /// When `jouet job stop` stopped it, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
}

/// Where a job is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    /// It ended with this exit code (128 + N when signal N ended it).
    Exited(i32),
    /// `jouet job stop` stopped it.
    Stopped,
    /// It is no longer running, and nothing recorded how it ended.
    Gone,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited(_) => "exited",
            Self::Stopped => "stopped",
            Self::Gone => "gone",
        }
    }

    pub fn exit_code(self) -> Option<i32> {
        match self {
            Self::Exited(code) => Some(code),
            _ => None,
        }
    }

    /// `running`, `exited 1`, `stopped` or `gone`.
    pub fn describe(self) -> String {
        match self {
            Self::Exited(code) => format!("exited {code}"),
            other => other.name().to_string(),
        }
    }
}

/// The jobs of one work tree (or, outside one, one directory).
#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The jobs of the work tree `cwd` is in; None without a state
    /// directory.
    pub fn here(cwd: &Path) -> Option<Self> {
        let key =
            jouet_output::paths::repo_key(cwd).or_else(|| jouet_output::paths::dir_key(cwd))?;
        let dir = jouet_output::paths::state_dir()?.join("jobs").join(key);
        Some(Self { dir })
    }

    fn path(&self, id: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{id}.{ext}"))
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        let bytes = std::fs::read(self.path(id, "json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save(&self, job: &Job) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(&mut file, job)?;
        file.persist(self.path(&job.id, "json"))
            .map(drop)
            .map_err(|e| e.error)
    }

    /// Every job, oldest first. A job that finished more than a week ago is
    /// removed instead.
    pub fn list(&self) -> Vec<Job> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut jobs: Vec<Job> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let id = e.path().file_stem()?.to_string_lossy().into_owned();
                self.get(&id)
            })
            .filter(|job| !self.expired(job))
            .collect();
        jobs.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        jobs
    }

    fn expired(&self, job: &Job) -> bool {
        if self.state(job) == State::Running {
            return false;
        }
        let old = std::fs::metadata(self.path(&job.id, "json"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > KEEP_FINISHED);
        if old {
            for ext in ["json", "log", "exit"] {
                let _ = std::fs::remove_file(self.path(&job.id, ext));
            }
        }
        old
    }

    /// Where the job is now: its exit file first, since a process that has
    /// exited may still be waiting to be reaped.
    pub fn state(&self, job: &Job) -> State {
        if let Some(code) = std::fs::read_to_string(self.path(&job.id, "exit"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return State::Exited(code);
        }
        if sys::is_job(job) {
            State::Running
        } else if job.stopped_at.is_some() {
            State::Stopped
        } else {
            State::Gone
        }
    }

    /// The job's log, whole.
    pub fn read_log(&self, job: &Job) -> String {
        std::fs::read(&job.log)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }
}

/// Whether an ID can name a job: letters, digits, `_`, `.` and `-`, not
/// starting with `.` or `-`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with(['.', '-'])
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// What `start` needs to know.
pub struct Spec {
    pub id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub ready: Option<Ready>,
    pub session: Option<String>,
    /// Stop a running job of the same ID first.
    pub replace: bool,
}

/// Starts a job and records it. Refuses (exit 4) while a job of the same ID
/// is running, unless `replace`.
pub fn spawn(store: &Store, spec: Spec) -> Result<Job, Stop> {
    let refuse = |message: String| Stop {
        exit: Exit::Precondition,
        message,
    };
    if let Some(old) = store.get(&spec.id)
        && store.state(&old) == State::Running
    {
        if !spec.replace {
            return Err(refuse(format!(
                "job `{}` is already running (pid {}); `jouet job stop {}` stops it, or pass --replace",
                old.id, old.pid, old.id
            )));
        }
        stop(store, &old, STOP_GRACE)?;
    }
    let failed = |what: &str, e: std::io::Error| Stop {
        exit: Exit::Failed,
        message: format!("{what}: {e}"),
    };
    std::fs::create_dir_all(&store.dir).map_err(|e| failed("creating the jobs directory", e))?;
    let log = store.path(&spec.id, "log");
    let exit = store.path(&spec.id, "exit");
    let _ = std::fs::remove_file(&exit);
    let (pid, proc_start) = sys::spawn(&spec.command, &spec.cwd, &log, &exit)?;
    let job = Job {
        id: spec.id,
        pid,
        pgid: pid,
        command: spec.command,
        cwd: spec.cwd.display().to_string(),
        log: log.display().to_string(),
        started_at: jouet_output::time::format_utc(jouet_output::time::now()),
        proc_start,
        session: spec.session,
        ready: spec.ready,
        stopped_at: None,
    };
    store
        .save(&job)
        .map_err(|e| failed("recording the job", e))?;
    Ok(job)
}

/// Lines of a job's log a failure or a timeout shows.
pub const LOG_LINES: usize = 60;

#[derive(Serialize)]
struct Started<'a> {
    #[serde(flatten)]
    outcome: &'a Outcome,
    job: &'a Job,
}

/// `job start`: starts the job, then waits up to `timeout` for its ready
/// check (or, without one, makes sure it did not fail at once), and prints
/// the result. A job that is still not ready at the timeout keeps running.
pub fn start(store: Store, spec: Spec, timeout: Duration, reporter: &mut Reporter) -> Exit {
    let job = match spawn(&store, spec) {
        Ok(job) => job,
        Err(stop) => return reporter.fail(&stop),
    };
    if job.ready.is_none() {
        // Long enough for a command that cannot start to say so.
        std::thread::sleep(Duration::from_millis(200));
    }
    let mut wait = match JobWait::new(store, job.clone(), Until::Ready, None, Some(LOG_LINES)) {
        Ok(wait) => wait,
        Err(message) => {
            return reporter.fail(&Stop {
                exit: Exit::Usage,
                message,
            });
        }
    };
    let timing = Timing {
        timeout: if job.ready.is_some() {
            timeout
        } else {
            Duration::ZERO
        },
        interval: Duration::from_millis(250),
    };
    let outcome = match crate::run(&mut wait, &timing, reporter) {
        Ok(outcome) => outcome,
        Err(stop) => return reporter.fail(&stop),
    };
    let text = started_text(&outcome, &job, reporter);
    reporter.finish_doc(
        &Started {
            outcome: &outcome,
            job: &job,
        },
        &text,
    );
    outcome.exit()
}

fn started_text(o: &Outcome, job: &Job, reporter: &Reporter) -> String {
    let id = &job.id;
    let took = elapsed(o.elapsed_s);
    match o.result {
        "met" => format!(
            "{} job {id} (pid {}): {} ({took})\n  log: {}\n  next: `jouet job logs {id}`, `jouet wait job {id}`, `jouet job stop {id}`\n",
            reporter.word("STARTED", true),
            job.pid,
            o.state,
            job.log
        ),
        "failed" => format!(
            "{} job {id}: {} after {took}\n{}",
            reporter.word("FAILED", false),
            o.state,
            crate::report::details(&o.detail)
        ),
        _ => format!(
            "{} job {id}: {} after {took}; it is still running (`jouet job stop {id}` stops it, `jouet wait job {id} --until ready` keeps waiting)\n{}",
            reporter.word("TIMEOUT", false),
            o.state,
            crate::report::details(&o.detail)
        ),
    }
}

/// How `stop` ended a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// It had already ended.
    Already(State),
    /// It exited after SIGTERM.
    Term,
    /// It was still running after the grace period, so it got SIGKILL.
    Kill,
}

/// Stops a running job's whole process group: SIGTERM, then SIGKILL after
/// `grace`. A job that has already ended is left as it is.
pub fn stop(store: &Store, job: &Job, grace: Duration) -> Result<Stopped, Stop> {
    let state = store.state(job);
    if state != State::Running {
        return Ok(Stopped::Already(state));
    }
    let how = sys::stop(job, grace)?;
    let mut job = job.clone();
    job.stopped_at = Some(jouet_output::time::format_utc(jouet_output::time::now()));
    store.save(&job).map_err(|e| Stop {
        exit: Exit::Failed,
        message: format!("recording that job `{}` stopped: {e}", job.id),
    })?;
    Ok(how)
}

/// The end of a job's log for a failure or a timeout, and where all of it
/// is.
pub fn excerpt(store: &Store, job: &Job, tail: Option<usize>) -> Detail {
    let Some(tail) = tail else {
        return Detail {
            log_path: Some(job.log.clone()),
            ..Detail::default()
        };
    };
    let text = store.read_log(job);
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let cut = crate::trim_excerpt(lines, tail);
    let mut output = cut.lines;
    if output.is_empty() {
        output.push("(its log is empty)".into());
    }
    Detail {
        output,
        output_truncated: cut.truncated,
        log_path: Some(job.log.clone()),
        ..Detail::default()
    }
}

/// What a wait on a job is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    /// It ends: met when it exits 0 or is stopped, failed otherwise.
    Exited,
    /// It is running and its ready check passes.
    Ready,
}

impl Until {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "exited" => Some(Self::Exited),
            "ready" => Some(Self::Ready),
            _ => None,
        }
    }
}

/// `wait job ID`, and the wait `job start` does for its ready check.
pub struct JobWait {
    store: Store,
    job: Job,
    until: Until,
    check: Option<Box<dyn Subject>>,
    log_lines: Option<usize>,
}

impl JobWait {
    /// A wait on `job`. Ready is `ready` when given, else the job's own
    /// ready check, else simply running.
    pub fn new(
        store: Store,
        job: Job,
        until: Until,
        ready: Option<&Ready>,
        log_lines: Option<usize>,
    ) -> Result<Self, String> {
        let check = match (until, ready.or(job.ready.as_ref())) {
            (Until::Ready, Some(ready)) => Some(ready.subject(Path::new(&job.log))?),
            _ => None,
        };
        Ok(Self {
            store,
            job,
            until,
            check,
            log_lines,
        })
    }

    pub fn job(&self) -> &Job {
        &self.job
    }
}

impl Subject for JobWait {
    fn name(&self) -> &'static str {
        "job"
    }

    fn target(&self) -> String {
        self.job.id.clone()
    }

    fn check(&mut self, deadline: Instant) -> Result<Observation, Stop> {
        let state = self.store.state(&self.job);
        let ended = |met: bool, state: State, detail: Detail| Observation {
            met,
            failed: !met,
            state: state.describe(),
            detail,
        };
        Ok(match (state, self.until) {
            (State::Running, Until::Exited) => Observation::pending("running"),
            (State::Running, Until::Ready) => match &mut self.check {
                None => Observation {
                    met: true,
                    ..Observation::pending("running")
                },
                Some(check) => {
                    let seen = check.check(deadline)?;
                    Observation {
                        met: seen.met,
                        state: format!(
                            "running, {} {} {}",
                            check.name(),
                            check.target(),
                            seen.state
                        ),
                        detail: seen.detail,
                        failed: false,
                    }
                }
            },
            (State::Exited(0) | State::Stopped, Until::Exited) => {
                ended(true, state, Detail::default())
            }
            // A job with nothing to be ready for that has already done its
            // work.
            (State::Exited(0), Until::Ready) if self.check.is_none() => {
                ended(true, state, Detail::default())
            }
            // Ended before it was ready, or ended badly.
            (_, _) => ended(
                false,
                state,
                excerpt(&self.store, &self.job, self.log_lines),
            ),
        })
    }

    fn timed_out(&mut self, detail: &mut Detail) {
        *detail = excerpt(&self.store, &self.job, self.log_lines);
    }
}

#[cfg(unix)]
mod sys {
    use super::{Job, Stop, Stopped};
    use jouet_output::exit::Exit;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// The script the command runs under: it runs the command, then writes
    /// its exit status beside the log. `$1` is the exit file; `$0`, which
    /// names the shell in its own errors, is `jouet-job`.
    const WRAPPER: &str = r#"exit_file=$1; shift; "$@"; status=$?; printf '%s\n' "$status" >"$exit_file.tmp" && mv "$exit_file.tmp" "$exit_file"; exit "$status""#;

    pub(super) fn spawn(
        command: &[String],
        cwd: &Path,
        log: &Path,
        exit: &Path,
    ) -> Result<(u32, Option<String>), Stop> {
        let failed = |what: String| Stop {
            exit: Exit::Failed,
            message: what,
        };
        let out = std::fs::File::create(log)
            .map_err(|e| failed(format!("creating {}: {e}", log.display())))?;
        let err = out
            .try_clone()
            .map_err(|e| failed(format!("opening {}: {e}", log.display())))?;
        let mut sh = Command::new("/bin/sh");
        sh.arg("-c")
            .arg(WRAPPER)
            .arg("jouet-job")
            .arg(exit)
            .args(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        // SAFETY: `setsid` is async-signal-safe, and nothing else runs
        // between fork and exec.
        unsafe {
            sh.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = sh
            .spawn()
            .map_err(|e| failed(format!("could not start /bin/sh: {e}")))?;
        let pid = child.id();
        // Not waited for: the job outlives jouet, and init reaps it.
        drop(child);
        Ok((pid, start_time(pid)))
    }

    /// Whether the job's process is still the one jouet started: alive,
    /// leading its process group, and started when it was recorded.
    pub(super) fn is_job(job: &Job) -> bool {
        let Ok(pid) = libc::pid_t::try_from(job.pid) else {
            return false;
        };
        if pid <= 0 || !crate::os::process_running(job.pid) {
            return false;
        }
        // SAFETY: getpgid only reads.
        let pgid = unsafe { libc::getpgid(pid) };
        if u32::try_from(pgid).ok() != Some(job.pgid) {
            return false;
        }
        match (&job.proc_start, start_time(job.pid)) {
            (Some(then), Some(now)) => *then == now,
            // Without a start time to compare, the group is the evidence.
            _ => true,
        }
    }

    pub(super) fn stop(job: &Job, grace: Duration) -> Result<Stopped, Stop> {
        let pgid = libc::pid_t::try_from(job.pgid).map_err(|_| Stop {
            exit: Exit::Failed,
            message: format!("job `{}` has no valid process group", job.id),
        })?;
        signal(pgid, libc::SIGTERM, job)?;
        let until = Instant::now() + grace;
        while Instant::now() < until {
            if !group_alive(pgid) {
                return Ok(Stopped::Term);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !group_alive(pgid) {
            return Ok(Stopped::Term);
        }
        signal(pgid, libc::SIGKILL, job)?;
        Ok(Stopped::Kill)
    }

    fn signal(pgid: libc::pid_t, sig: libc::c_int, job: &Job) -> Result<(), Stop> {
        // SAFETY: killpg sends a signal to a group we verified is the job's.
        if unsafe { libc::killpg(pgid, sig) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(Stop {
            exit: Exit::Failed,
            message: format!("signalling job `{}` (process group {pgid}): {e}", job.id),
        })
    }

    /// Whether any process of the group is left. The leader may be gone
    /// while a child it started still runs.
    fn group_alive(pgid: libc::pid_t) -> bool {
        // SAFETY: signal 0 sends nothing.
        let alive = unsafe { libc::killpg(pgid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        // A member that exited but was not yet reaped still answers.
        alive && !only_zombies(pgid)
    }

    #[cfg(target_os = "linux")]
    fn only_zombies(pgid: libc::pid_t) -> bool {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        !entries.flatten().any(|e| {
            let Some(stat) = std::fs::read_to_string(e.path().join("stat")).ok() else {
                return false;
            };
            let Some((_, rest)) = stat.rsplit_once(')') else {
                return false;
            };
            let fields: Vec<&str> = rest.split_whitespace().collect();
            // State, ppid, pgrp.
            fields.get(2).and_then(|g| g.parse::<libc::pid_t>().ok()) == Some(pgid)
                && fields.first().is_some_and(|s| *s != "Z")
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn only_zombies(_pgid: libc::pid_t) -> bool {
        false
    }

    /// When the process started, in the OS's own terms: clock ticks since
    /// boot on Linux, `ps`'s start time elsewhere.
    #[cfg(target_os = "linux")]
    fn start_time(pid: u32) -> Option<String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, rest) = stat.rsplit_once(')')?;
        // Field 22 of stat; `rest` starts at field 3.
        rest.split_whitespace().nth(19).map(str::to_string)
    }

    #[cfg(not(target_os = "linux"))]
    fn start_time(pid: u32) -> Option<String> {
        let out = Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }
}

#[cfg(not(unix))]
mod sys {
    use super::{Job, Stop, Stopped};
    use jouet_output::exit::Exit;
    use std::path::Path;
    use std::time::Duration;

    fn unsupported() -> Stop {
        Stop {
            exit: Exit::Precondition,
            message: "jouet job runs on Linux and macOS only, for now; on Windows, use the harness's own background tasks".into(),
        }
    }

    pub(super) fn spawn(
        _command: &[String],
        _cwd: &Path,
        _log: &Path,
        _exit: &Path,
    ) -> Result<(u32, Option<String>), Stop> {
        Err(unsupported())
    }

    pub(super) fn is_job(_job: &Job) -> bool {
        false
    }

    pub(super) fn stop(_job: &Job, _grace: Duration) -> Result<Stopped, Stop> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_file_names() {
        for good in ["dev", "web-1", "api_v2", "a.b"] {
            assert!(valid_id(good), "{good}");
        }
        for bad in ["", "-x", ".x", "a/b", "a b", "..", &"x".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn states_describe_themselves() {
        assert_eq!(State::Exited(3).describe(), "exited 3");
        assert_eq!(State::Running.describe(), "running");
        assert_eq!(State::Exited(0).exit_code(), Some(0));
        assert_eq!(State::Gone.exit_code(), None);
        assert_eq!(Until::parse("exited"), Some(Until::Exited));
        assert_eq!(Until::parse("stopped"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_job_runs_detached_records_its_exit_and_stops_by_group() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store {
            dir: tmp.path().join("jobs"),
        };
        let spec = |id: &str, script: &str| Spec {
            id: id.into(),
            command: vec!["sh".into(), "-c".into(), script.into()],
            cwd: tmp.path().to_path_buf(),
            ready: None,
            session: Some("s1".into()),
            replace: false,
        };
        let quick = spawn(&store, spec("quick", "echo hi; exit 3"))
            .ok()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while store.state(&quick) == State::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(store.state(&quick), State::Exited(3));
        assert_eq!(store.read_log(&quick), "hi\n");

        let slow = spawn(&store, spec("slow", "sleep 30 & wait")).ok().unwrap();
        assert_eq!(store.state(&slow), State::Running);
        let again = spawn(&store, spec("slow", "true")).err().unwrap();
        assert_eq!(again.exit, Exit::Precondition);
        assert_eq!(stop(&store, &slow, STOP_GRACE).ok(), Some(Stopped::Term));
        let slow = store.get("slow").unwrap();
        assert!(matches!(
            store.state(&slow),
            State::Stopped | State::Exited(_)
        ));
        assert_eq!(store.list().len(), 2);
    }
}
