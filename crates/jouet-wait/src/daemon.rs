//! `wait daemon ID`: a pitchfork daemon, until it is ready or until it
//! stops.
//!
//! pitchfork's JSON does not say whether a daemon's own ready check has
//! passed: its status is `running` from the moment the process starts. So
//! ready here is `running`, and then, when there is a port to try (`--port`,
//! or the one pitchfork says the daemon binds) or a URL (`--http`), that it
//! answers there. A daemon that errors or fails ends the wait at once, with
//! the end of its log.

use super::{Detail, Observation, Stop, Subject};
use crate::local::PortWait;
use crate::pitchfork::{self, Daemon, Error, Phase};
use jouet_output::exit::Exit;
use std::path::PathBuf;
use std::time::Instant;

/// What the wait is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    /// Running, and answering where it can be asked.
    Ready,
    /// No longer running.
    Stopped,
}

impl Until {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "ready" => Some(Self::Ready),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// How a ready daemon is asked whether it answers.
pub enum Answer {
    /// The port pitchfork says it binds, if any.
    Inferred,
    /// `--port` or `--http`.
    Given(Box<dyn Subject>),
}

pub struct DaemonWait {
    id: String,
    until: Until,
    answer: Answer,
    /// The port subject for the inferred port, built once it is known.
    inferred: Option<(u16, PortWait)>,
    /// Lines of the log to show on failure; 0 shows all of it.
    log_lines: Option<usize>,
}

impl DaemonWait {
    pub fn new(id: String, until: Until, answer: Answer, log_lines: Option<usize>) -> Self {
        Self {
            id,
            until,
            answer,
            inferred: None,
            log_lines,
        }
    }

    /// Whether the running daemon answers: met, and what was seen.
    fn answers(&mut self, daemon: &Daemon, deadline: Instant) -> Result<(bool, String), Stop> {
        let subject: &mut dyn Subject = match &mut self.answer {
            Answer::Given(subject) => subject.as_mut(),
            Answer::Inferred => {
                let Some(port) = daemon.ready_port() else {
                    return Ok((true, "running".into()));
                };
                if self.inferred.as_ref().is_none_or(|(p, _)| *p != port) {
                    let probe =
                        PortWait::parse(&port.to_string(), false).map_err(|message| Stop {
                            exit: Exit::Failed,
                            message,
                        })?;
                    self.inferred = Some((port, probe));
                }
                let (_, probe) = self.inferred.as_mut().expect("just set");
                probe
            }
        };
        let seen = subject.check(deadline)?;
        let state = format!(
            "running, {} {} {}",
            subject.name(),
            subject.target(),
            seen.state
        );
        Ok((seen.met, state))
    }

    /// The end of the daemon's log and where the whole of it is saved.
    fn log(&self, daemon: &Daemon) -> Detail {
        let Some(tail) = self.log_lines else {
            return Detail::default();
        };
        let id = if daemon.id.is_empty() {
            self.id.as_str()
        } else {
            daemon.id.as_str()
        };
        let text = match pitchfork::logs(id, None) {
            Ok(text) => text,
            Err(e) => {
                return Detail {
                    output: vec![format!("(its log could not be read: {})", e.message())],
                    ..Detail::default()
                };
            }
        };
        let path = log_path(id);
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, &text));
        let excerpt = crate::trim_excerpt(text.lines().map(str::to_string).collect(), tail);
        let mut output = excerpt.lines;
        if output.is_empty() {
            output.push("(its log is empty)".into());
        }
        Detail {
            output,
            output_truncated: excerpt.truncated,
            log_path: match saved {
                Ok(()) => Some(path.display().to_string()),
                Err(e) => Some(format!("not saved ({e}); `pitchfork logs {id}` prints it")),
            },
            ..Detail::default()
        }
    }
}

/// A new file for a copy of a daemon's log: pitchfork keeps logs in a
/// database, not in files of their own.
fn log_path(id: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let name: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    jouet_output::paths::log_dir().join(format!(
        "wait-daemon-{name}-{stamp}-{}.log",
        std::process::id()
    ))
}

impl Subject for DaemonWait {
    fn name(&self) -> &'static str {
        "daemon"
    }

    fn target(&self) -> String {
        self.id.clone()
    }

    fn check(&mut self, deadline: Instant) -> Result<Observation, Stop> {
        let daemon = match pitchfork::status(&self.id, None) {
            Ok(daemon) => daemon,
            Err(Error::Missing) => {
                return Err(Stop {
                    exit: Exit::Precondition,
                    message: format!(
                        "{}; `wait daemon` reads its status",
                        Error::Missing.message()
                    ),
                });
            }
            Err(Error::NotFound(said)) => {
                return Err(Stop {
                    exit: Exit::NotFound,
                    message: format!(
                        "pitchfork has no daemon `{}` here ({said}); `pitchfork list --project` lists them",
                        self.id
                    ),
                });
            }
            Err(Error::Failed(said)) => return Ok(Observation::pending(said)),
        };
        let (met, state) = match (daemon.phase(), self.until) {
            (Phase::Failed, _) => {
                // `status` leaves out why; `list` says.
                let daemon = pitchfork::list(None)
                    .ok()
                    .and_then(|all| all.into_iter().find(|d| d.id == daemon.id))
                    .unwrap_or(daemon);
                return Ok(Observation {
                    met: false,
                    failed: true,
                    state: daemon.describe(),
                    detail: self.log(&daemon),
                });
            }
            (Phase::Done, _) => (true, daemon.status.clone()),
            (Phase::Down, Until::Stopped) => (true, daemon.status.clone()),
            (Phase::Up, Until::Ready) => self.answers(&daemon, deadline)?,
            _ => (false, daemon.status.clone()),
        };
        Ok(Observation {
            met,
            ..Observation::pending(state)
        })
    }

    fn timed_out(&mut self, detail: &mut Detail) {
        if let Ok(daemon) = pitchfork::status(&self.id, None) {
            *detail = self.log(&daemon);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_names_two_conditions() {
        assert_eq!(Until::parse("ready"), Some(Until::Ready));
        assert_eq!(Until::parse("stopped"), Some(Until::Stopped));
        assert_eq!(Until::parse("running"), None);
    }

    #[test]
    fn a_log_copy_is_named_for_its_daemon() {
        let path = log_path("myproj/web");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("wait-daemon-myproj-web-"), "{name}");
        assert!(name.ends_with(".log"), "{name}");
    }
}
