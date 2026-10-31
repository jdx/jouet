//! `jouet wait` and its subjects.

use super::Output;
use crate::{
    WaitArgs, WaitCiArgs, WaitCmd, WaitCmdArgs, WaitDaemonArgs, WaitFileArgs, WaitHttpArgs,
    WaitJobArgs, WaitLogArgs, WaitPidArgs, WaitPrArgs, WaitRunArgs,
};
use jouet_github::host::Feature;
use jouet_github::pr::target::{self, Arg, Repo, Target};
use jouet_output::exit::Exit;
use jouet_output::time::parse_duration;
use jouet_wait::checks::Selection;
use jouet_wait::ci::{self, CiWait};
use jouet_wait::daemon::{self, Answer, DaemonWait};
use jouet_wait::host::Remote;
use jouet_wait::job;
use jouet_wait::local::{CmdWait, FileWait, HttpWait, LogWait, PidWait, PortWait};
use jouet_wait::pr::{PrWait, Until};
use jouet_wait::remote::{self, Halt, Watch};
use jouet_wait::run::{self as run_wait, RunWait};
use jouet_wait::{self as wait, Reporter, Stop, Subject, Timing};
use regex::Regex;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_APPEAR_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const DEFAULT_LOG_LINES: usize = 60;

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
        Err(halt) => {
            let stop = match halt {
                Halt::Stop(stop) => stop,
                Halt::RateLimited { reset } => Stop {
                    exit: Exit::Remote,
                    message: github_failure(reset),
                },
            };
            reporter(String::new()).fail(&stop)
        }
    }
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
    let until = Until::parse(&a.until).ok_or_else(|| {
        format!(
            "--until must be mergeable, merged, closed, approved, checks-registered, ready or any-change, not `{}`",
            a.until
        )
    })?;
    Ok((target(a.pr.as_deref(), a.repo.as_deref())?, until))
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
