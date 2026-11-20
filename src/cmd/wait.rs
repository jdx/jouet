//! `jouet wait` and its subjects.

use super::Output;
use crate::cli::{
    WaitArgs, WaitCiArgs, WaitCmd, WaitCmdArgs, WaitDaemonArgs, WaitFileArgs, WaitHttpArgs,
    WaitJobArgs, WaitLogArgs, WaitPidArgs, WaitPrArgs, WaitRunArgs,
};
use jouet_github::host::Feature;
use jouet_github::pr::target::{self, Arg, Repo, Target};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use jouet_output::time::parse_duration;
use jouet_wait::checks::Selection;
use jouet_wait::ci::{self, CiWait};
use jouet_wait::daemon::{self, Answer, DaemonWait};
use jouet_wait::host::Remote;
use jouet_wait::job;
use jouet_wait::lease::{self, Lease};
use jouet_wait::local::{CmdWait, FileWait, HttpWait, LogWait, PidWait, PortWait};
use jouet_wait::pr::{PrWait, Until};
use jouet_wait::remote::{self, Halt, Watch};
use jouet_wait::run::{self as run_wait, RunWait};
use jouet_wait::settle::{self, SettleWait};
use jouet_wait::triage::Acks;
use jouet_wait::{self as wait, Reporter, Stop, Subject, Timing};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_APPEAR_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const DEFAULT_LOG_LINES: usize = 60;
/// `--until needs-agent`'s quiet window and timeout: a PR's reviewers and CI
/// take longer than one check run to settle.
const DEFAULT_QUIET: Duration = Duration::from_secs(20 * 60);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// The quiet window once every check has finished and no review bot says
/// it is still reviewing, unless `--quiet-for` is given.
const SHORT_QUIET: Duration = Duration::from_secs(5 * 60);
/// A watcher in an agent's session holds Auto-fix events, so it waits up
/// to a day, polling every 5 minutes once the PR has settled.
pub(crate) const LEASE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const SETTLED_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long a watcher waits, after removing its lease, for a hook that
/// was holding an event as it did.
const HOLD_GRACE: Duration = Duration::from_secs(1);
/// How often a parked watcher looks for its session.
const PARK_STEP: Duration = Duration::from_secs(60);

pub(crate) fn run(args: WaitArgs, out: &Output) -> Exit {
    if out.json && args.jsonl {
        return out.error(Exit::Usage, "give --json or --jsonl, not both");
    }
    let timing = match timing(&args) {
        Ok(timing) => timing,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    let confirm_conflict = match args.confirm_conflict.as_deref().map(parse_duration) {
        None => jouet_wait::CONFLICT_CONFIRM,
        Some(Ok(d)) => d,
        Some(Err(e)) => return out.error(Exit::Usage, &format!("--confirm-conflict: {e}")),
    };
    let reporter = |label: String| Reporter::new(out.mode, out.json, args.jsonl, args.quiet, label);
    match args.cmd {
        WaitCmd::Ci(a) => match ci_wait(&a, confirm_conflict) {
            Ok((t, sha, options)) => watch(reporter, &timing, &t.repo().clone(), |r| {
                if options.selection.required
                    && let Some(message) = r.host().lacks(Feature::RequiredChecks)
                {
                    return Err(Halt::Stop(Stop {
                        exit: Exit::Precondition,
                        message,
                    }));
                }
                let (repo, number) = r.resolve(t)?;
                Ok(CiWait::new(repo, number, sha, options))
            }),
            Err(message) => out.error(Exit::Usage, &message),
        },
        WaitCmd::Run(a) => {
            match run_args(&a) {
                Ok((repo, id, lines)) => watch(reporter, &timing, &repo.clone(), |r| {
                    if let Some(message) = r.host().lacks(Feature::Runs) {
                        return Err(Halt::Stop(Stop {
                            exit: Exit::Precondition,
                            message,
                        }));
                    }
                    Ok(RunWait::new(repo, id, lines)
                        .local_runner(crate::guide::on_path("jactionlint")))
                }),
                Err(message) => out.error(Exit::Usage, &message),
            }
        }
        WaitCmd::Pr(a) if a.until == "needs-agent" => {
            let session = jouet_git::session::id_from_env();
            let timing = Timing {
                timeout: match (&args.timeout, &session) {
                    (Some(_), _) => timing.timeout,
                    (None, Some(_)) => LEASE_TIMEOUT,
                    (None, None) => SETTLE_TIMEOUT,
                },
                ..timing
            };
            match settle_args(&a, confirm_conflict) {
                Ok((t, options)) => match session {
                    Some(session) => settle_watch(reporter, &timing, t, options, &session),
                    None => watch(reporter, &timing, &t.repo().clone(), |_| {
                        Ok(SettleWait::new(t, options))
                    }),
                },
                Err(message) => out.error(Exit::Usage, &message),
            }
        }
        WaitCmd::Pr(a) => match pr_args(&a) {
            Ok((t, until)) => watch(reporter, &timing, &t.repo().clone(), |_| {
                Ok(PrWait::new(t, until, confirm_conflict))
            }),
            Err(message) => out.error(Exit::Usage, &message),
        },
        WaitCmd::File(a) => local(file(a), &timing, reporter),
        WaitCmd::Log(a) => local(log(a), &timing, reporter),
        WaitCmd::Port(a) => local(
            PortWait::parse(&a.addr, a.closed).map(|p| Box::new(p) as Box<dyn Subject>),
            &timing,
            reporter,
        ),
        WaitCmd::Http(a) => local(http(a), &timing, reporter),
        WaitCmd::Pid(a) => local(pid(&a), &timing, reporter),
        WaitCmd::Cmd(a) => local(command(a, &timing), &timing, reporter),
        WaitCmd::Daemon(a) => local(daemon_wait(a), &timing, reporter),
        WaitCmd::Job(a) => match job_wait(a) {
            Ok(subject) => local(Ok(subject), &timing, reporter),
            Err((exit, message)) => reporter(String::new()).fail(&Stop { exit, message }),
        },
    }
}

/// `wait daemon ID`.
fn daemon_wait(a: WaitDaemonArgs) -> Result<Box<dyn Subject>, String> {
    let until = match a.until.as_deref() {
        None => daemon::Until::Ready,
        Some(name) => daemon::Until::parse(name)
            .ok_or_else(|| format!("--until must be ready or stopped, not `{name}`"))?,
    };
    let given: Option<Box<dyn Subject>> = match (a.port, a.http) {
        (Some(_), Some(_)) => return Err("give --port or --http, not both".into()),
        (Some(port), None) => Some(Box::new(PortWait::parse(&port.to_string(), false)?)),
        (None, Some(url)) => Some(http(WaitHttpArgs {
            url,
            status: None,
            contains: None,
            expect: Vec::new(),
        })?),
        (None, None) => None,
    };
    if given.is_some() && until == daemon::Until::Stopped {
        return Err(
            "--port and --http are ready checks: leave them out with --until stopped".into(),
        );
    }
    let answer = given.map_or(Answer::Inferred, Answer::Given);
    Ok(Box::new(DaemonWait::new(
        a.id,
        until,
        answer,
        log_lines(a.log_lines, a.no_logs)?,
    )))
}

/// `wait job ID`: exit 3 when this work tree has no such job.
fn job_wait(a: WaitJobArgs) -> Result<Box<dyn Subject>, (Exit, String)> {
    let usage = |message: String| (Exit::Usage, message);
    let until = match a.until.as_deref() {
        None => job::Until::Exited,
        Some(name) => job::Until::parse(name)
            .ok_or_else(|| usage(format!("--until must be exited or ready, not `{name}`")))?,
    };
    let ready = super::job::ready(a.port, a.http, None).map_err(usage)?;
    if ready.is_some() && until == job::Until::Exited {
        return Err(usage(
            "--port and --http are ready checks: give them with --until ready".into(),
        ));
    }
    let store = super::job::here().map_err(|m| (Exit::Precondition, m))?;
    let found = store.get(&a.id).ok_or_else(|| {
        (
            Exit::NotFound,
            format!(
                "no job `{}` in this work tree; `jouet job ls` lists them",
                a.id
            ),
        )
    })?;
    let lines = log_lines(a.log_lines, a.no_logs).map_err(usage)?;
    let wait = job::JobWait::new(store, found, until, ready.as_ref(), lines).map_err(usage)?;
    Ok(Box::new(wait))
}

/// Runs a local subject, or reports why it could not be built.
fn local(
    subject: Result<Box<dyn Subject>, String>,
    timing: &Timing,
    reporter: impl Fn(String) -> Reporter,
) -> Exit {
    let mut subject = match subject {
        Ok(subject) => subject,
        Err(message) => {
            return reporter(String::new()).fail(&Stop {
                exit: Exit::Usage,
                message,
            });
        }
    };
    let mut reporter = reporter(format!("{} {}", subject.name(), subject.target()));
    match wait::run(subject.as_mut(), timing, &mut reporter) {
        Ok(outcome) => reporter.finish(&outcome),
        Err(stop) => reporter.fail(&stop),
    }
}

/// Builds a wait on the forge `repo` is on (which can need a request, such
/// as finding a branch's PR) and runs it.
fn watch<W: Watch>(
    reporter: impl Fn(String) -> Reporter,
    timing: &Timing,
    repo: &Repo,
    build: impl FnOnce(&mut Remote) -> Result<W, Halt>,
) -> Exit {
    let mut remote = match Remote::connect(repo) {
        Ok(remote) => remote,
        Err(message) => {
            return reporter(String::new()).fail(&Stop {
                exit: Exit::Precondition,
                message,
            });
        }
    };
    match build(&mut remote) {
        Ok(mut w) => {
            let mut reporter = reporter(w.label());
            remote::run(&mut w, &mut remote, timing, &mut reporter)
        }
        Err(halt) => reporter(String::new()).fail(&halt_stop(halt)),
    }
}

fn halt_stop(halt: Halt) -> Stop {
    match halt {
        Halt::Stop(stop) => stop,
        Halt::RateLimited { reset } => Stop {
            exit: Exit::Remote,
            message: github_failure(reset),
        },
    }
}

/// `wait pr --until needs-agent` in an agent's session: the watcher holds a
/// lease, so the hooks can hold the Auto-fix events about its PR for its
/// digest, and records in the ledger what it did with them. Anything that
/// keeps it from writing the lease leaves a plain wait.
fn settle_watch(
    reporter: impl Fn(String) -> Reporter,
    timing: &Timing,
    target: Target,
    mut options: settle::Options,
    session: &str,
) -> Exit {
    let started = Instant::now();
    let mut remote = match Remote::connect(target.repo()) {
        Ok(remote) => remote,
        Err(message) => {
            return reporter(String::new()).fail(&Stop {
                exit: Exit::Precondition,
                message,
            });
        }
    };
    // The lease names its PR, so the target is resolved first.
    let (repo, number) = match remote.resolve(target) {
        Ok(resolved) => resolved,
        Err(halt) => return reporter(String::new()).fail(&halt_stop(halt)),
    };
    let now = jouet_output::time::now();
    let lease = Lease {
        pid: std::process::id(),
        session: session.to_string(),
        repo: repo.to_string(),
        pr: number,
        branch: None,
        started: now,
        since: options.since,
        heartbeat: now,
    };
    let path = lease::dir()
        .and_then(|dir| Lease::path(&dir, session, lease.pid))
        .filter(|path| lease.write(path).is_ok());
    if path.is_some() {
        options.lease.clone_from(&path);
        options.settled_interval = Some(SETTLED_INTERVAL);
    }
    let beat = path.clone().map(|path| Heartbeat::start(path, lease));
    let mut w = SettleWait::new(Target::Number(repo, number), options);
    let mut reporter = reporter(w.label());
    let (exit, doc) = remote::run_doc(&mut w, &mut remote, timing, &mut reporter);
    if let Some(beat) = beat {
        beat.stop();
    }
    let Some(path) = path else {
        return exit;
    };
    let held = give_back(&path);
    let reported = doc.as_ref().map_or(&[][..], |d| &d.held[..]);
    if let Some(late) = late_note(held.len().saturating_sub(reported.len()), number) {
        eprintln!("{late}");
    }
    let result = doc.as_ref().map_or("error", settle::Doc::result);
    let parked = result == "closed" && held.is_empty();
    record_settle(
        session,
        crate::ledger::Settle {
            result: if parked { "parked" } else { result }.into(),
            held: held.len() as u64,
            accounted: reported.iter().filter(|h| h.accounted).count() as u64,
            wake_after_first_held_s: doc.as_ref().and_then(|d| d.wake_after_first_held_s),
            watcher: Some(std::process::id()),
            woke: !parked,
        },
    );
    if parked {
        // Nothing for the agent: a wake would only start a turn to read
        // that the PR closed. The wait stays until its session ends.
        let parent = parent();
        while started.elapsed() < timing.timeout && !orphaned(parent) {
            std::thread::sleep(PARK_STEP.min(timing.timeout.saturating_sub(started.elapsed())));
        }
    }
    exit
}

/// Gives a watcher's lease back and returns the events held for it. No hook
/// holds an event once the lease is gone; one that was holding as it went
/// has the grace to finish.
fn give_back(path: &Path) -> Vec<lease::Held> {
    let _ = std::fs::remove_file(path);
    std::thread::sleep(HOLD_GRACE);
    let held = lease::held(path);
    Lease::remove(path);
    held
}

/// What to say about events held after the digest was put together.
fn late_note(late: usize, number: u64) -> Option<String> {
    (late > 0).then(|| {
        format!(
            "jouet: {late} Auto-fix event{} held as this wait ended; look at the PR: `jouet wait pr {number} --until needs-agent` or `jouet pr threads {number}`",
            if late == 1 { "" } else { "s" }
        )
    })
}

/// How long the watcher jouet starts after a push waits, when the defaults
/// are not wanted.
#[derive(Default)]
pub(crate) struct Tuning {
    pub quiet: Option<Duration>,
    pub timeout: Option<Duration>,
    pub interval: Option<Duration>,
}

/// The watcher jouet starts after a push (`jouet watch`), holding its lease.
pub(crate) struct PushWatch {
    remote: Remote,
    repo: Repo,
    lease: Lease,
    path: PathBuf,
}

impl PushWatch {
    /// Takes the lease for the PR of the branch checked out here, pushed at
    /// `since`: comments from then on are new to the watcher, also one
    /// posted while it was finding the PR. The error is why no watcher
    /// starts: `no-pr` (nothing to watch: the branch has no PR, or no branch
    /// of a repository jouet can read is checked out), `watching` (a live
    /// watcher of the session has it) or `error`.
    pub(crate) fn claim(session: &str, since: i64) -> Result<Self, &'static str> {
        let target = target(None, None).map_err(|_| "no-pr")?;
        let mut remote = Remote::connect(target.repo()).map_err(|_| "error")?;
        let (repo, number) = remote.resolve(target).map_err(|halt| match halt {
            Halt::Stop(stop) if stop.exit == Exit::NotFound => "no-pr",
            _ => "error",
        })?;
        let now = jouet_output::time::now();
        let lease = Lease {
            pid: std::process::id(),
            session: session.to_string(),
            repo: repo.to_string(),
            pr: number,
            branch: None,
            started: now,
            since,
            heartbeat: now,
        };
        let dir = lease::dir().ok_or("error")?;
        match lease.claim(&dir, now, crate::git::head().as_deref()) {
            Ok(Some(path)) => Ok(Self {
                remote,
                repo,
                lease,
                path,
            }),
            Ok(None) => Err("watching"),
            Err(_) => Err("error"),
        }
    }

    /// Waits as `wait pr --until needs-agent` does, gives the lease back,
    /// and returns what to wake the agent with: the digest, held events
    /// included. `None` when the PR closed or the wait ran out with nothing
    /// held, which is not worth a turn. A wait that failed still reports
    /// the events held for it.
    pub(crate) fn run(mut self, tuning: &Tuning) -> Option<String> {
        let number = self.lease.pr;
        let timeout = tuning.timeout.unwrap_or(LEASE_TIMEOUT);
        let started = Instant::now();
        let mut beat = Some(Heartbeat::start(self.path.clone(), self.lease.clone()));
        let (w, reporter, doc) = loop {
            let options = settle::Options {
                quiet: tuning.quiet.unwrap_or(DEFAULT_QUIET),
                quiet_short: tuning.quiet.is_none().then_some(SHORT_QUIET),
                acks: Acks::Loose,
                since: self.lease.since,
                log_lines: Some(DEFAULT_LOG_LINES),
                confirm_conflict: jouet_wait::CONFLICT_CONFIRM,
                lease: Some(self.path.clone()),
                settled_interval: Some(SETTLED_INTERVAL),
                restarts: true,
            };
            let timing = Timing {
                timeout: timeout.saturating_sub(started.elapsed()),
                interval: tuning.interval.unwrap_or(DEFAULT_INTERVAL),
            };
            let mut w = SettleWait::new(Target::Number(self.repo.clone(), number), options);
            // Quiet: what this process prints is what the agent is woken with.
            let mut reporter = Reporter::new(Mode::Agent, false, false, true, w.label());
            let doc = remote::wait(&mut w, &mut self.remote, &timing, &mut reporter).ok();
            // A push that came as this wait was ending found the PR watched
            // and started no watcher, so its commit is waited on here.
            let head = doc.as_ref().map(settle::Doc::head_sha);
            let stop = || beat.take().into_iter().for_each(Heartbeat::stop);
            if Lease::retire(&self.path, head, stop) {
                break (w, reporter, doc);
            }
        };
        let held = give_back(&self.path);
        let reported = doc.as_ref().map_or(&[][..], |d| &d.held[..]);
        let result = doc.as_ref().map_or("error", settle::Doc::result);
        let wake = match &doc {
            Some(doc) if result == "needs_agent" || !held.is_empty() => {
                let mut text = w.text(doc, &reporter);
                if let Some(late) = late_note(held.len().saturating_sub(reported.len()), number) {
                    text.push_str(&late);
                    text.push('\n');
                }
                Some(text)
            }
            Some(_) => None,
            None => crate::settle::lost_note(&self.lease, &held),
        };
        let accounted = match &doc {
            Some(_) => reported.iter().filter(|h| h.accounted).count(),
            // The note that stands in for the digest names each one.
            None => held.len(),
        };
        record_settle(
            &self.lease.session,
            crate::ledger::Settle {
                result: result.into(),
                held: held.len() as u64,
                accounted: accounted as u64,
                wake_after_first_held_s: doc.as_ref().and_then(|d| d.wake_after_first_held_s),
                watcher: Some(self.lease.pid),
                woke: wake.is_some(),
            },
        );
        wake
    }
}

/// Records a watcher's `settle` entry, unless the ledger is turned off.
fn record_settle(session: &str, settle: crate::ledger::Settle) {
    let settings = crate::settings::Settings::load();
    let Some(dir) = crate::ledger::dir().filter(|_| settings.hooks.ledger) else {
        return;
    };
    let mut entry = crate::ledger::Entry::new("jouet", Some(session), "settle");
    entry.repo = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::ledger::repo_key(&cwd));
    entry.settle = Some(settle);
    crate::ledger::append(&dir, &entry);
}

/// The process that started this one: the agent's shell.
fn parent() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Whether the process that started this one has gone: it was reparented.
fn orphaned(parent: u32) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id() != parent
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        false
    }
}

/// The thread that keeps a lease's heartbeat fresh. When the session that
/// started the watcher is gone, or the watcher is told to stop (SIGTERM,
/// which a harness sends a hook's watcher when its session closes), it ends
/// the process: with nothing held, it removes the lease first; with events
/// held, it leaves the lease to go stale, so the session's next hook reports
/// them.
struct Heartbeat {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Heartbeat {
    fn start(path: PathBuf, mut lease: Lease) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let told = told_to_stop(&stop);
        let parent = parent();
        let thread = std::thread::spawn(move || {
            let mut beat = Instant::now();
            while !stopped.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                if told.load(Ordering::Relaxed) {
                    leave(&path, &lease);
                    die();
                }
                if beat.elapsed() < lease::HEARTBEAT || stopped.load(Ordering::Relaxed) {
                    continue;
                }
                beat = Instant::now();
                if orphaned(parent) {
                    leave(&path, &lease);
                    std::process::exit(i32::from(Exit::Failed.code()));
                }
                lease.heartbeat = jouet_output::time::now();
                let _ = lease.write(&path);
            }
        });
        Self { stop, thread }
    }

    /// Stops it, so it never writes the lease again.
    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

/// A flag SIGTERM sets while a watcher holds its lease, in place of ending
/// the process, so the heartbeat thread can give the lease back first. Once
/// `stopped` is set, SIGTERM ends the process as it always did.
fn told_to_stop(stopped: &Arc<AtomicBool>) -> Arc<AtomicBool> {
    let told = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        let term = signal_hook::consts::SIGTERM;
        let _ = signal_hook::flag::register_conditional_default(term, Arc::clone(stopped));
        let _ = signal_hook::flag::register(term, Arc::clone(&told));
    }
    #[cfg(not(unix))]
    {
        let _ = stopped;
    }
    told
}

/// Ends the process the way the SIGTERM it was sent would have.
fn die() -> ! {
    #[cfg(unix)]
    {
        let _ = signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGTERM);
    }
    std::process::exit(i32::from(Exit::Failed.code()))
}

/// A watcher that stops before its digest: with nothing held it takes its
/// lease along; a lease with held events stays, stale, for the hook that
/// reports them (`lease-lost`).
fn leave(path: &Path, lease: &Lease) {
    // A lease that is not live holds no new event, and it is still there
    // for a hook that was holding one as it went stale, which has the grace
    // to finish. Removing it first would leave that event with no lease if
    // the process were killed meanwhile. No hook takes it for lost while
    // this process runs (`Lease::leaving`).
    let _ = lease.leaving(jouet_output::time::now()).write(path);
    std::thread::sleep(HOLD_GRACE);
    if lease::held(path).is_empty() {
        Lease::remove(path);
    }
    record_settle(
        &lease.session,
        crate::ledger::Settle {
            result: "stopped".into(),
            watcher: Some(lease.pid),
            ..crate::ledger::Settle::default()
        },
    );
}

fn github_failure(reset: i64) -> String {
    jouet_github::Failure::RateLimited { reset }.message()
}

/// The PR an argument names, in `--repo` or this checkout's repository,
/// defaulting to the current branch's PR.
pub(crate) fn target(pr: Option<&str>, repo: Option<&str>) -> Result<Target, String> {
    let repo = || match repo {
        Some(r) => Repo::parse(r)
            .ok_or_else(|| format!("--repo takes OWNER/NAME or HOST/OWNER/NAME, not `{r}`")),
        None => target::repo_from_git()
            .ok_or_else(|| "no --repo given and no GitHub remote here".to_string()),
    };
    Ok(match pr.map(Arg::parse) {
        Some(Arg::Number(n)) => Target::Number(repo()?, n),
        Some(Arg::Branch(branch)) => Target::Branch(repo()?, branch),
        Some(Arg::InRepo(repo, n)) => Target::Number(repo, n),
        None => {
            let branch = target::current_branch().ok_or("not on a branch: name a PR")?;
            Target::Branch(repo()?, branch)
        }
    })
}

fn log_lines(lines: Option<usize>, no_logs: bool) -> Result<Option<usize>, String> {
    match (lines, no_logs) {
        (Some(_), true) => Err("give --log-lines or --no-logs, not both".into()),
        (_, true) => Ok(None),
        (lines, false) => Ok(Some(lines.unwrap_or(DEFAULT_LOG_LINES))),
    }
}

fn ci_wait(
    a: &WaitCiArgs,
    confirm_conflict: Duration,
) -> Result<(Target, Option<String>, ci::Options), String> {
    let appear_timeout = match &a.appear_timeout {
        Some(d) => parse_duration(d).map_err(|e| format!("--appear-timeout: {e}"))?,
        None => DEFAULT_APPEAR_TIMEOUT,
    };
    if let Some(sha) = &a.sha
        && !(sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(format!("--sha takes a commit SHA, not `{sha}`"));
    }
    let options = ci::Options {
        selection: Selection {
            required: a.required,
            include: a.check.clone(),
            exclude: a.ignore.clone(),
        },
        all: a.all,
        log_lines: log_lines(a.log_lines, a.no_logs)?,
        appear_timeout,
        confirm_conflict,
        local_runner: crate::guide::on_path("jactionlint"),
    };
    Ok((
        target(a.pr.as_deref(), a.repo.as_deref())?,
        a.sha.clone(),
        options,
    ))
}

fn run_args(a: &WaitRunArgs) -> Result<(Repo, u64, Option<usize>), String> {
    let (in_url, id) = run_wait::parse_run(&a.run).ok_or_else(|| {
        format!(
            "`{}` is not a run ID, an Actions run URL or a GitLab pipeline URL",
            a.run
        )
    })?;
    let repo = match (in_url, a.repo.as_deref()) {
        (Some(repo), _) => repo,
        (None, Some(r)) => Repo::parse(r)
            .ok_or_else(|| format!("--repo takes OWNER/NAME or HOST/OWNER/NAME, not `{r}`"))?,
        (None, None) => {
            target::repo_from_git().ok_or("no --repo given and no GitHub remote here")?
        }
    };
    Ok((repo, id, log_lines(a.log_lines, a.no_logs)?))
}

fn pr_args(a: &WaitPrArgs) -> Result<(Target, Until), String> {
    if a.quiet_for.is_some()
        || a.acks.is_some()
        || a.since.is_some()
        || a.log_lines.is_some()
        || a.no_logs
    {
        return Err(
            "--quiet-for, --acks, --since, --log-lines and --no-logs go with --until needs-agent"
                .into(),
        );
    }
    let until = Until::parse(&a.until).ok_or_else(|| {
        format!(
            "--until must be mergeable, merged, closed, approved, checks-registered, ready, any-change or needs-agent, not `{}`",
            a.until
        )
    })?;
    Ok((target(a.pr.as_deref(), a.repo.as_deref())?, until))
}

/// `wait pr --until needs-agent`.
fn settle_args(
    a: &WaitPrArgs,
    confirm_conflict: Duration,
) -> Result<(Target, settle::Options), String> {
    let quiet = match a.quiet_for.as_deref() {
        Some(d) => parse_duration(d).map_err(|e| format!("--quiet-for: {e}"))?,
        None => DEFAULT_QUIET,
    };
    let acks = match a.acks.as_deref() {
        Some(s) => {
            Acks::parse(s).ok_or_else(|| format!("--acks must be loose or strict, not `{s}`"))?
        }
        None => Acks::Loose,
    };
    let now = jouet_output::time::now();
    let since = match a.since.as_deref() {
        Some(s) => jouet_output::time::parse_time(s, now).map_err(|e| format!("--since: {e}"))?,
        None => now,
    };
    let options = settle::Options {
        quiet,
        // Given a window, the wait keeps to it.
        quiet_short: a.quiet_for.is_none().then_some(SHORT_QUIET),
        acks,
        since,
        log_lines: log_lines(a.log_lines, a.no_logs)?,
        confirm_conflict,
        lease: None,
        settled_interval: None,
        restarts: false,
    };
    Ok((target(a.pr.as_deref(), a.repo.as_deref())?, options))
}

fn timing(args: &WaitArgs) -> Result<Timing, String> {
    let read = |flag: &str, value: &Option<String>, default| match value {
        Some(v) => parse_duration(v).map_err(|e| format!("{flag}: {e}")),
        None => Ok(default),
    };
    let interval = read("--interval", &args.interval, DEFAULT_INTERVAL)?;
    if interval.is_zero() {
        return Err("--interval must be longer than 0".into());
    }
    Ok(Timing {
        timeout: read("--timeout", &args.timeout, DEFAULT_TIMEOUT)?,
        interval,
    })
}

fn regex(flag: &str, pattern: Option<&str>) -> Result<Option<Regex>, String> {
    pattern
        .map(|p| Regex::new(p).map_err(|e| format!("{flag} is not a valid regex: {e}")))
        .transpose()
}

fn file(a: WaitFileArgs) -> Result<Box<dyn Subject>, String> {
    if a.gone && a.contains.is_some() {
        return Err("give --contains or --gone, not both".into());
    }
    Ok(Box::new(FileWait {
        path: PathBuf::from(a.path),
        contains: regex("--contains", a.contains.as_deref())?,
        gone: a.gone,
    }))
}

fn log(a: WaitLogArgs) -> Result<Box<dyn Subject>, String> {
    let pattern = regex("--match", Some(&a.r#match))?.expect("a pattern was given");
    Ok(Box::new(LogWait::new(
        PathBuf::from(a.path),
        pattern,
        a.from_start,
    )))
}

fn http(a: WaitHttpArgs) -> Result<Box<dyn Subject>, String> {
    jouet_wait::http::check_url(&a.url)?;
    let contains = regex("--contains", a.contains.as_deref())?;
    let expect = a
        .expect
        .iter()
        .map(|e| jouet_wait::http::Assertion::parse(e))
        .collect::<Result<_, _>>()?;
    Ok(Box::new(HttpWait::new(a.url, a.status, contains, expect)))
}

fn pid(a: &WaitPidArgs) -> Result<Box<dyn Subject>, String> {
    if a.pid == 0 {
        return Err("PID must be a process ID above 0".into());
    }
    Ok(Box::new(PidWait { pid: a.pid }))
}

fn command(a: WaitCmdArgs, timing: &Timing) -> Result<Box<dyn Subject>, String> {
    if a.command.is_empty() {
        return Err("no command: write it after `--`, as in `jouet wait cmd -- make check`".into());
    }
    Ok(Box::new(CmdWait {
        argv: a.command,
        expect_exit: a.expect_exit.unwrap_or(0),
        log_path: cmd_log_path(),
        min_run: timing.interval,
    }))
}

fn cmd_log_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    jouet_output::paths::log_dir().join(format!("wait-cmd-{stamp}-{}.log", std::process::id()))
}
