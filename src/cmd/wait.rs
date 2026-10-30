//! `jouet wait` and its subjects.

use super::Output;
use crate::exit::Exit;
use crate::time::parse_duration;
use crate::wait::local::{CmdWait, FileWait, HttpWait, LogWait, PidWait, PortWait};
use crate::wait::{self, Reporter, Subject, Timing};
use crate::{WaitArgs, WaitCmd};
use regex::Regex;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);

pub fn run(args: WaitArgs, out: &Output) -> Exit {
    if out.json && args.jsonl {
        return out.error(Exit::Usage, "give --json or --jsonl, not both");
    }
    let timing = match timing(&args) {
        Ok(timing) => timing,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    let mut subject = match subject(args.cmd, &timing) {
        Ok(subject) => subject,
        Err(message) => return out.error(Exit::Usage, &message),
    };
    let label = format!("{} {}", subject.name(), subject.target());
    let mut reporter = Reporter::new(out.mode, out.json, args.jsonl, args.quiet, label);
    match wait::run(subject.as_mut(), &timing, &mut reporter) {
        Ok(outcome) => reporter.finish(&outcome),
        Err(stop) => reporter.fail(&stop),
    }
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

fn subject(cmd: WaitCmd, timing: &Timing) -> Result<Box<dyn Subject>, String> {
    Ok(match cmd {
        WaitCmd::File(a) => {
            if a.gone && a.contains.is_some() {
                return Err("give --contains or --gone, not both".into());
            }
            Box::new(FileWait {
                path: PathBuf::from(a.path),
                contains: regex("--contains", a.contains.as_deref())?,
                gone: a.gone,
            })
        }
        WaitCmd::Log(a) => {
            let pattern = regex("--match", Some(&a.r#match))?.expect("a pattern was given");
            Box::new(LogWait::new(PathBuf::from(a.path), pattern, a.from_start))
        }
        WaitCmd::Port(a) => Box::new(PortWait::parse(&a.addr, a.closed)?),
        WaitCmd::Http(a) => {
            if !(a.url.starts_with("http://") || a.url.starts_with("https://")) {
                return Err(format!("`{}` is not an http:// or https:// URL", a.url));
            }
            Box::new(HttpWait::new(
                a.url,
                a.status,
                regex("--contains", a.contains.as_deref())?,
            ))
        }
        WaitCmd::Pid(a) => {
            if a.pid == 0 {
                return Err("PID must be a process ID above 0".into());
            }
            Box::new(PidWait { pid: a.pid })
        }
        WaitCmd::Cmd(a) => {
            if a.command.is_empty() {
                return Err(
                    "no command: write it after `--`, as in `jouet wait cmd -- make check`".into(),
                );
            }
            Box::new(CmdWait {
                argv: a.command,
                expect_exit: a.expect_exit.unwrap_or(0),
                log_path: cmd_log_path(),
                min_run: timing.interval,
            })
        }
    })
}

fn cmd_log_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    crate::paths::log_dir().join(format!("wait-cmd-{stamp}-{}.log", std::process::id()))
}
