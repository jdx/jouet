//! Subjects that need nothing but this machine: files, logs, ports, URLs,
//! processes and commands.
//!
//! They stay deliberately small. Starting and supervising long-running
//! services, and their ready checks, is pitchfork's job.

use super::{Detail, Observation, Stop, Subject, os};
use crate::exit::Exit;
use regex::Regex;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How much of the last `wait cmd` run's output is printed.
const OUTPUT_TAIL: usize = 20;
/// How long a TCP connection attempt may take. Windows reports a refused
/// connection only after about two seconds of retries, so this leaves room
/// for that to be seen as a refusal rather than a timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one HTTP request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The most of an HTTP body `--contains` searches.
const BODY_LIMIT: u64 = 16 * 1024 * 1024;
/// The most of an unfinished log line kept between checks.
const PARTIAL_LIMIT: usize = 64 * 1024;

/// The first line of `text` that matches.
fn first_match(re: &Regex, text: &str) -> Option<String> {
    text.lines()
        .find(|line| re.is_match(line))
        .map(str::to_string)
}

/// `wait file PATH [--contains REGEX] [--gone]`
pub struct FileWait {
    pub path: PathBuf,
    pub contains: Option<Regex>,
    pub gone: bool,
}

impl Subject for FileWait {
    fn name(&self) -> &'static str {
        "file"
    }

    fn target(&self) -> String {
        self.path.display().to_string()
    }

    fn check(&mut self, _deadline: Instant) -> Result<Observation, Stop> {
        let exists = match std::fs::symlink_metadata(&self.path) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Ok(Observation::pending(format!("unreadable: {e}"))),
        };
        if self.gone || !exists {
            let state = if exists { "exists" } else { "missing" };
            return Ok(Observation {
                met: exists != self.gone,
                ..Observation::pending(state)
            });
        }
        let Some(re) = &self.contains else {
            return Ok(Observation {
                met: true,
                ..Observation::pending("exists")
            });
        };
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) => return Ok(Observation::pending(format!("exists, unreadable: {e}"))),
        };
        Ok(match first_match(re, &String::from_utf8_lossy(&bytes)) {
            Some(line) => Observation {
                met: true,
                state: "matched".into(),
                detail: Detail {
                    line: Some(line),
                    ..Detail::default()
                },
            },
            None => Observation::pending("exists, no match"),
        })
    }
}

/// `wait log PATH --match REGEX [--from-start]`, which follows the path the
/// way `tail -F` does: a file that is replaced (rotated) or truncated is read
/// again from its start, and a file that does not exist yet is waited for.
pub struct LogWait {
    path: PathBuf,
    pattern: Regex,
    /// The file being followed, and how far into it has been read.
    id: Option<(u64, u64)>,
    offset: u64,
    /// The end of the file when it does not end in a newline yet.
    partial: Vec<u8>,
    rotations: u32,
}

impl LogWait {
    /// Starts at the current end of the file unless `from_start`. A file
    /// that does not exist yet is read from its start once it appears.
    pub fn new(path: PathBuf, pattern: Regex, from_start: bool) -> Self {
        let (id, offset) = match File::open(&path) {
            Ok(file) if !from_start => {
                let len = file.metadata().map_or(0, |m| m.len());
                (os::file_id(&file), len)
            }
            Ok(file) => (os::file_id(&file), 0),
            Err(_) => (None, 0),
        };
        Self {
            path,
            pattern,
            id,
            offset,
            partial: Vec::new(),
            rotations: 0,
        }
    }

    fn following(&self) -> String {
        match self.rotations {
            0 => "following".into(),
            1 => "following, reopened after 1 rotation".into(),
            n => format!("following, reopened after {n} rotations"),
        }
    }

    /// Reads what was appended since the last check and returns the first
    /// line that matches. An unfinished last line is matched too, and kept
    /// so it is matched again in full once its newline arrives.
    fn read_new(&mut self, file: &mut File) -> io::Result<Option<String>> {
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = std::mem::take(&mut self.partial);
        let read = file.read_to_end(&mut buf)?;
        self.offset += read as u64;
        let complete = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let mut rest = buf.split_off(complete);
        // `buf` is empty or ends with a newline. Splitting it as it is would
        // also yield an empty "line" after that newline (or for an empty
        // read), which a pattern such as `^$` would match; a blank line that
        // was really written is kept.
        let lines = buf.strip_suffix(b"\n").map(|b| b.split(|&c| c == b'\n'));
        for line in lines.into_iter().flatten() {
            let line = String::from_utf8_lossy(line);
            let line = line.trim_end_matches('\r');
            if self.pattern.is_match(line) {
                return Ok(Some(line.to_string()));
            }
        }
        let line = String::from_utf8_lossy(&rest);
        if !rest.is_empty() && self.pattern.is_match(&line) {
            return Ok(Some(line.trim_end_matches('\r').to_string()));
        }
        if rest.len() > PARTIAL_LIMIT {
            rest.drain(..rest.len() - PARTIAL_LIMIT);
        }
        self.partial = rest;
        Ok(None)
    }
}

impl Subject for LogWait {
    fn name(&self) -> &'static str {
        "log"
    }

    fn target(&self) -> String {
        self.path.display().to_string()
    }

    fn check(&mut self, _deadline: Instant) -> Result<Observation, Stop> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Observation::pending("missing"));
            }
            Err(e) => return Ok(Observation::pending(format!("unreadable: {e}"))),
        };
        let id = os::file_id(&file);
        let len = file.metadata().map_or(0, |m| m.len());
        if self.id.is_some() && id != self.id || len < self.offset {
            self.rotations += 1;
            self.offset = 0;
            self.partial.clear();
        }
        self.id = id;
        Ok(match self.read_new(&mut file) {
            Ok(Some(line)) => Observation {
                met: true,
                state: "matched".into(),
                detail: Detail {
                    line: Some(line),
                    ..Detail::default()
                },
            },
            Ok(None) => Observation::pending(self.following()),
            Err(e) => Observation::pending(format!("unreadable: {e}")),
        })
    }
}

/// `wait port ADDR [--closed]`
pub struct PortWait {
    /// `host:port`, resolved again at every check.
    pub addr: String,
    pub closed: bool,
}

impl PortWait {
    /// Reads `ADDR`: `host:port`, `[v6]:port`, or a bare port on localhost.
    pub fn parse(addr: &str, closed: bool) -> Result<Self, String> {
        let bad = || format!("`{addr}` is not an address; write it as HOST:PORT or PORT");
        let addr = if addr.bytes().all(|b| b.is_ascii_digit()) {
            format!("localhost:{addr}")
        } else {
            addr.to_string()
        };
        let (host, port) = addr.rsplit_once(':').ok_or_else(bad)?;
        match port.parse::<u16>() {
            Ok(p) if p > 0 && !host.is_empty() => Ok(Self { addr, closed }),
            _ => Err(bad()),
        }
    }
}

/// What connecting to a port found.
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    Open,
    /// Every address refused the connection.
    Refused,
    /// No address accepted, and at least one failed some other way (a
    /// timeout, an unreachable network, a permission error), so whether the
    /// port is closed is not known.
    Unknown(String),
}

impl Probe {
    /// Combines the attempts on each resolved address, stopping at the
    /// first that connects.
    fn of(attempts: impl IntoIterator<Item = io::Result<()>>) -> Self {
        let (mut any, mut unknown) = (false, None);
        for attempt in attempts {
            any = true;
            match attempt {
                Ok(()) => return Self::Open,
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {}
                Err(e) => {
                    unknown.get_or_insert_with(|| e.to_string());
                }
            }
        }
        match unknown {
            Some(e) => Self::Unknown(e),
            None if any => Self::Refused,
            None => Self::Unknown("the address resolved to nothing".into()),
        }
    }
}

impl Subject for PortWait {
    fn name(&self) -> &'static str {
        "port"
    }

    fn target(&self) -> String {
        self.addr.clone()
    }

    fn check(&mut self, _deadline: Instant) -> Result<Observation, Stop> {
        let addrs: Vec<SocketAddr> = match self.addr.to_socket_addrs() {
            Ok(addrs) => addrs.collect(),
            Err(e) => return Ok(Observation::pending(format!("unresolved: {e}"))),
        };
        let probe = Probe::of(
            addrs
                .iter()
                .map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).map(drop)),
        );
        let (met, state) = match probe {
            Probe::Open => (!self.closed, "open".to_string()),
            Probe::Refused => (self.closed, "closed".to_string()),
            Probe::Unknown(e) => (false, format!("unreachable: {e}")),
        };
        Ok(Observation {
            met,
            ..Observation::pending(state)
        })
    }
}

/// `wait http URL [--status CODE] [--contains REGEX]`
pub struct HttpWait {
    url: String,
    status: Option<u16>,
    contains: Option<Regex>,
    agent: ureq::Agent,
}

impl HttpWait {
    pub fn new(url: String, status: Option<u16>, contains: Option<Regex>) -> Self {
        // A 3xx is only seen when redirects are not followed.
        let redirects = if status.is_some_and(|s| (300..400).contains(&s)) {
            0
        } else {
            10
        };
        let agent = ureq::Agent::config_builder()
            .tls_config(crate::tls::config())
            .http_status_as_error(false)
            .max_redirects(redirects)
            .user_agent(concat!("jouet/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            url,
            status,
            contains,
            agent,
        }
    }

    fn status_ok(&self, status: u16) -> bool {
        match self.status {
            Some(want) => status == want,
            None => (200..300).contains(&status),
        }
    }
}

impl Subject for HttpWait {
    fn name(&self) -> &'static str {
        "http"
    }

    fn target(&self) -> String {
        self.url.clone()
    }

    fn check(&mut self, deadline: Instant) -> Result<Observation, Stop> {
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = left.clamp(Duration::from_secs(1), REQUEST_TIMEOUT);
        let sent = self
            .agent
            .get(&self.url)
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call();
        let mut resp = match sent {
            Ok(resp) => resp,
            Err(e) => return Ok(Observation::pending(format!("unreachable: {e}"))),
        };
        let status = resp.status().as_u16();
        let detail = Detail {
            status: Some(status),
            ..Detail::default()
        };
        if !self.status_ok(status) {
            return Ok(Observation {
                met: false,
                state: format!("status {status}"),
                detail,
            });
        }
        let Some(re) = &self.contains else {
            return Ok(Observation {
                met: true,
                state: format!("status {status}"),
                detail,
            });
        };
        let body = resp
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec();
        let body = match body {
            Ok(body) => body,
            Err(e) => {
                return Ok(Observation {
                    met: false,
                    state: format!("status {status}, unreadable body: {e}"),
                    detail,
                });
            }
        };
        Ok(match first_match(re, &String::from_utf8_lossy(&body)) {
            Some(line) => Observation {
                met: true,
                state: format!("status {status}, matched"),
                detail: Detail {
                    line: Some(line),
                    ..detail
                },
            },
            None => Observation {
                met: false,
                state: format!("status {status}, no match"),
                detail,
            },
        })
    }
}

/// `wait pid PID`
pub struct PidWait {
    pub pid: u32,
}

impl Subject for PidWait {
    fn name(&self) -> &'static str {
        "pid"
    }

    fn target(&self) -> String {
        self.pid.to_string()
    }

    fn check(&mut self, _deadline: Instant) -> Result<Observation, Stop> {
        let running = os::process_running(self.pid);
        Ok(Observation {
            met: !running,
            ..Observation::pending(if running { "running" } else { "exited" })
        })
    }
}

/// `wait cmd -- CMD... [--expect-exit N]`: runs the command (not through a
/// shell) until it exits with the expected code. Each run's output replaces
/// the last one's in the saved log.
pub struct CmdWait {
    pub argv: Vec<String>,
    pub expect_exit: i32,
    pub log_path: PathBuf,
    /// The least time a run is given, even past the deadline, so that the
    /// last check (and `--timeout 0`'s only one) can finish.
    pub min_run: Duration,
}

impl CmdWait {
    fn spawn_error(&self, e: &io::Error) -> Stop {
        Stop {
            exit: if e.kind() == io::ErrorKind::NotFound {
                Exit::NotFound
            } else {
                Exit::Failed
            },
            message: format!("could not run `{}`: {e}", self.argv[0]),
        }
    }

    fn log_error(&self, e: &io::Error) -> Stop {
        Stop {
            exit: Exit::Failed,
            message: format!("writing {}: {e}", self.log_path.display()),
        }
    }
}

impl Subject for CmdWait {
    fn name(&self) -> &'static str {
        "cmd"
    }

    fn target(&self) -> String {
        self.argv
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn check(&mut self, deadline: Instant) -> Result<Observation, Stop> {
        if let Some(dir) = self.log_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.log_error(&e))?;
        }
        let log = File::create(&self.log_path).map_err(|e| self.log_error(&e))?;
        let err_log = log.try_clone().map_err(|e| self.log_error(&e))?;
        let stop_at = deadline.max(Instant::now() + self.min_run);
        let mut child = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err_log)
            .spawn()
            .map_err(|e| self.spawn_error(&e))?;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= stop_at => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(self.spawn_error(&e)),
            }
        };
        let text = std::fs::read(&self.log_path).map_err(|e| self.log_error(&e))?;
        let text = String::from_utf8_lossy(&text);
        let lines: Vec<&str> = text.lines().collect();
        let omitted = lines.len().saturating_sub(OUTPUT_TAIL);
        let code = status.and_then(|s| s.code());
        let state = match (status, code) {
            (None, _) => "still running at the timeout, so stopped".to_string(),
            (Some(_), Some(code)) => format!("exit {code}"),
            (Some(status), None) => format!("ended by a signal ({status})"),
        };
        Ok(Observation {
            met: code == Some(self.expect_exit),
            state,
            detail: Detail {
                exit_code: code,
                output: lines[omitted..].iter().map(|l| (*l).to_string()).collect(),
                output_truncated: omitted > 0,
                output_omitted: omitted,
                log_path: Some(self.log_path.display().to_string()),
                ..Detail::default()
            },
        })
    }
}

/// An argument as a POSIX shell would need it written, for display.
fn quote(arg: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c);
    if !arg.is_empty() && arg.chars().all(plain) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_shown_quoted() {
        let cmd = CmdWait {
            argv: vec![
                "sh".into(),
                "-c".into(),
                "echo 'hi'; exit 3".into(),
                String::new(),
            ],
            expect_exit: 0,
            log_path: PathBuf::new(),
            min_run: Duration::ZERO,
        };
        assert_eq!(cmd.target(), r"sh -c 'echo '\''hi'\''; exit 3' ''");
    }

    #[test]
    fn a_bare_port_is_localhost() {
        assert_eq!(
            PortWait::parse("8080", false).unwrap().addr,
            "localhost:8080"
        );
        assert_eq!(PortWait::parse("[::1]:80", false).unwrap().addr, "[::1]:80");
        for bad in ["", "host", ":80", "host:0", "host:99999", "8080x"] {
            assert!(PortWait::parse(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_log_line_is_matched_once_it_is_whole_or_already_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "old ready line\n").unwrap();
        let mut log = LogWait::new(path.clone(), Regex::new("^ready$").unwrap(), false);
        let far = Instant::now() + Duration::from_secs(60);
        assert_eq!(log.check(far).ok().unwrap().state, "following");
        std::fs::write(&path, "old ready line\nrea").unwrap();
        assert!(!log.check(far).ok().unwrap().met);
        std::fs::write(&path, "old ready line\nready\r\n").unwrap();
        let obs = log.check(far).ok().unwrap();
        assert!(obs.met);
        assert_eq!(obs.detail.line.as_deref(), Some("ready"));
    }

    #[test]
    fn only_lines_that_were_written_are_matched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "").unwrap();
        let mut log = LogWait::new(path.clone(), Regex::new("^$").unwrap(), true);
        let far = Instant::now() + Duration::from_secs(60);
        // An empty read, then a single non-blank line.
        assert!(!log.check(far).ok().unwrap().met);
        std::fs::write(&path, "hello\n").unwrap();
        assert!(!log.check(far).ok().unwrap().met);
        // A blank line that was really written does match.
        std::fs::write(&path, "hello\n\nworld\n").unwrap();
        let obs = log.check(far).ok().unwrap();
        assert!(obs.met);
        assert_eq!(obs.detail.line.as_deref(), Some(""));
    }

    #[test]
    fn only_refusal_on_every_address_counts_as_closed() {
        let refused = || Err(io::Error::from(io::ErrorKind::ConnectionRefused));
        let timeout = || Err(io::Error::from(io::ErrorKind::TimedOut));
        assert_eq!(Probe::of([refused(), refused()]), Probe::Refused);
        assert_eq!(Probe::of([refused(), Ok(())]), Probe::Open);
        assert!(matches!(
            Probe::of([refused(), timeout()]),
            Probe::Unknown(_)
        ));
        assert!(matches!(Probe::of([timeout()]), Probe::Unknown(_)));
        assert!(matches!(Probe::of([]), Probe::Unknown(_)));
    }

    #[test]
    fn a_truncated_log_is_read_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "a long first line that is longer than the next\n").unwrap();
        let mut log = LogWait::new(path.clone(), Regex::new("done").unwrap(), false);
        let far = Instant::now() + Duration::from_secs(60);
        std::fs::write(&path, "done\n").unwrap();
        let obs = log.check(far).ok().unwrap();
        assert!(obs.met);
        assert_eq!(log.rotations, 1);
    }
}
