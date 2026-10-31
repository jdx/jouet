//! pitchfork, the daemon manager jouet leaves long-running services to,
//! read through its CLI: `pitchfork status ID --json`, `pitchfork list
//! --json` and `pitchfork logs ID --raw`. jouet never talks to pitchfork's
//! supervisor or its MCP server.
//!
//! jouet only reads, so it never starts the supervisor. `pitchfork list`
//! otherwise starts one when none is running, and it keeps running after
//! `jouet handoff` exits, in every repository where handoff runs. With no
//! supervisor, no daemon is running, and the lists are empty.
//!
//! pitchfork runs in the directory it is given, which decides the namespace
//! a short ID such as `api` resolves in: the nearest `pitchfork.toml`'s
//! project, or `global` outside one.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// The program jouet runs.
const PROGRAM: &str = "pitchfork";

/// One daemon, as `pitchfork list --json` and `pitchfork status --json`
/// describe it. Fields pitchfork adds later are ignored, and ones it leaves
/// out take their defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Daemon {
    /// The qualified ID, `namespace/name`.
    pub id: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// `running`, `waiting`, `stopping`, `restarting`, `stopped`,
    /// `errored`, `failed`, `completed`, `available` or `disabled`.
    #[serde(default)]
    pub status: String,
    /// Why it errored or failed, such as `exit code 7`; only `list` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The ports it is expected to bind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port: Vec<u16>,
    /// The port it bound, when pitchfork knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_port: Option<u16>,
    /// Its proxy URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Where a daemon is in its life, from its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// `running`.
    Up,
    /// `waiting`, `stopping` or `restarting`: on its way somewhere.
    Moving,
    /// `stopped`, `available` or `disabled`: not running, and not because
    /// it failed.
    Down,
    /// `completed`: a oneshot that exited 0.
    Done,
    /// `errored` or `failed`.
    Failed,
}

impl Daemon {
    pub fn phase(&self) -> Phase {
        match self.status.as_str() {
            "running" => Phase::Up,
            "stopped" | "available" | "disabled" => Phase::Down,
            "completed" => Phase::Done,
            "errored" | "failed" => Phase::Failed,
            _ => Phase::Moving,
        }
    }

    /// Whether it has a process: running, or on its way up or down.
    pub fn alive(&self) -> bool {
        matches!(self.phase(), Phase::Up | Phase::Moving)
    }

    /// Its status, with the error when there is one: `errored (exit code 7)`.
    pub fn describe(&self) -> String {
        match &self.error {
            Some(error) => format!("{} ({error})", self.status),
            None => self.status.clone(),
        }
    }

    /// The port to connect to for a ready check: the one it bound, else
    /// the first it is expected to bind.
    pub fn ready_port(&self) -> Option<u16> {
        self.active_port.or_else(|| self.port.first().copied())
    }
}

/// Why pitchfork could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `pitchfork` is not on PATH.
    Missing,
    /// pitchfork knows no daemon by that ID.
    NotFound(String),
    /// The command needs pitchfork's supervisor, and none is running.
    NoSupervisor,
    /// Anything else, with what pitchfork said.
    Failed(String),
}

impl Error {
    pub fn message(&self) -> String {
        match self {
            Self::Missing => {
                "pitchfork is not installed (`mise use -g pitchfork` installs it)".into()
            }
            Self::NoSupervisor => "pitchfork's supervisor is not running".into(),
            Self::NotFound(message) | Self::Failed(message) => message.clone(),
        }
    }
}

/// Runs pitchfork with `args` in `dir` (this process's directory when
/// None). Its stdin is closed, so it never shows a picker, and it does not
/// start the supervisor.
fn run(dir: Option<&Path>, args: &[&str]) -> Result<Output, Error> {
    let mut command = Command::new(PROGRAM);
    command
        .args(args)
        .stdin(Stdio::null())
        .env("NO_COLOR", "1")
        .env("PITCHFORK_SUPERVISOR_AUTO_START", "false");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let output = command.output().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => Error::Missing,
        _ => Error::Failed(format!("could not run pitchfork: {e}")),
    })?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // A mise shim with no version selected here: as good as not installed.
    if stderr.contains("No version is set for shim") {
        return Err(Error::Missing);
    }
    if stderr.contains("pitchfork::ipc::connection_failed") {
        return Err(Error::NoSupervisor);
    }
    let said = said(&stderr);
    // pitchfork exits 1 for every error, so a missing daemon is told apart
    // by what it says.
    if said.contains("not found") {
        return Err(Error::NotFound(said));
    }
    Err(Error::Failed(format!(
        "`pitchfork {}` failed: {said}",
        args.join(" ")
    )))
}

/// pitchfork's error, without miette's decoration: the lines that say
/// something, joined.
fn said(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['×', '│', '╰', '─', '▶'])
                .trim()
        })
        .filter(|l| !l.is_empty() && !l.starts_with("Error: pitchfork::"))
        .collect();
    if lines.is_empty() {
        "it exited 1 and said nothing".into()
    } else {
        lines.join("; ")
    }
}

fn parse<T: for<'de> Deserialize<'de>>(output: &Output, what: &str) -> Result<T, Error> {
    serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::Failed(format!("reading `pitchfork {what} --json`: {e}")))
}

/// One daemon, by a short or qualified ID.
pub fn status(id: &str, dir: Option<&Path>) -> Result<Daemon, Error> {
    let output = run(dir, &["status", id, "--json"])?;
    parse(&output, "status")
}

/// Every daemon pitchfork tracks or has configured, in every namespace;
/// none when its supervisor is not running.
pub fn list(dir: Option<&Path>) -> Result<Vec<Daemon>, Error> {
    listing(dir, &["list", "--json"], "list")
}

/// The daemons in `dir`'s namespace: its project's, or `global`'s outside
/// a project; none when its supervisor is not running.
pub fn list_project(dir: Option<&Path>) -> Result<Vec<Daemon>, Error> {
    listing(dir, &["list", "--project", "--json"], "list --project")
}

fn listing(dir: Option<&Path>, args: &[&str], what: &str) -> Result<Vec<Daemon>, Error> {
    match run(dir, args) {
        Err(Error::NoSupervisor) => Ok(Vec::new()),
        output => parse(&output?, what),
    }
}

/// A daemon's whole log, as plain lines.
pub fn logs(id: &str, dir: Option<&Path>) -> Result<String, Error> {
    let output = run(dir, &["logs", id, "--raw", "--no-pager"])?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_list_json_pitchfork_prints() {
        let json = r#"[{"id":"myproj/web","namespace":"myproj","name":"web","pid":null,
            "status":"errored","oneshot":false,"disabled":false,"available":false,
            "proxy_url":null,"url":null,"error":"exit code 7","port":[3000]}]"#;
        let daemons: Vec<Daemon> = serde_json::from_str(json).unwrap();
        let d = &daemons[0];
        assert_eq!(d.id, "myproj/web");
        assert_eq!(d.phase(), Phase::Failed);
        assert_eq!(d.describe(), "errored (exit code 7)");
        assert_eq!(d.ready_port(), Some(3000));
        assert!(!d.alive());
    }

    #[test]
    fn every_status_has_a_phase() {
        let phase = |status: &str| {
            Daemon {
                status: status.into(),
                ..Daemon::default()
            }
            .phase()
        };
        assert_eq!(phase("running"), Phase::Up);
        for s in ["waiting", "stopping", "restarting", "something-new"] {
            assert_eq!(phase(s), Phase::Moving, "{s}");
        }
        for s in ["stopped", "available", "disabled"] {
            assert_eq!(phase(s), Phase::Down, "{s}");
        }
        assert_eq!(phase("completed"), Phase::Done);
        assert_eq!(phase("failed"), Phase::Failed);
    }

    #[test]
    fn miettes_decoration_is_dropped() {
        let stderr = "Error: pitchfork::deps::not_found (https://pitchfork.jdx.dev/configuration#depends)\n\n  × daemon 'nope' not found in configuration\n\n";
        assert_eq!(said(stderr), "daemon 'nope' not found in configuration");
        assert_eq!(said(""), "it exited 1 and said nothing");
    }
}
