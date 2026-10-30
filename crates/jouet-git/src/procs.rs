//! Background processes running in a worktree, for `jouet handoff`.
//!
//! A heuristic, and Linux only (it reads `/proc`): processes of this user
//! whose working directory is inside the worktree, other than jouet's own
//! ancestors and siblings (the agent, its shell, and the rest of the
//! pipeline jouet runs in), and other than what a person is using in a
//! terminal: an interactive shell (a session leader with a terminal) or
//! whatever runs in a terminal's foreground, such as an editor. A child
//! whose parent is also listed is left out, so `npm run dev` is one entry
//! rather than three. A process whose stdout is a file names it as its log.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub command: String,
    pub cwd: String,
    /// The file its stdout goes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

impl Process {
    /// How the next session follows it: its log if it has one, else its
    /// exit.
    pub fn attach(&self) -> String {
        match &self.log {
            Some(log) => format!("tail -f {}", shell_quote(log)),
            None => format!("jouet wait pid {}", self.pid),
        }
    }
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The background processes in `top`, or `None` where they cannot be
/// listed.
#[cfg(target_os = "linux")]
pub(crate) fn in_worktree(top: &Path) -> Option<Vec<Process>> {
    linux::in_worktree(top)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn in_worktree(_top: &Path) -> Option<Vec<Process>> {
    None
}

/// Whether a process is still running, or `None` where that cannot be told.
#[cfg(target_os = "linux")]
pub(crate) fn alive(pid: u32) -> Option<bool> {
    Some(linux::stat(pid).is_some_and(|s| s.state != 'Z'))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn alive(_pid: u32) -> Option<bool> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Process;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    pub(super) struct Stat {
        pub state: char,
        pub ppid: u32,
        pgrp: i64,
        session: i64,
        tty: i64,
        tpgid: i64,
    }

    impl Stat {
        /// Whether a person is using it from a terminal.
        fn interactive(&self, pid: u32) -> bool {
            self.tty != 0 && (self.session == i64::from(pid) || self.pgrp == self.tpgid)
        }
    }

    /// The fields of `/proc/PID/stat` this needs. The command name is in
    /// parentheses and may hold spaces or parentheses itself, so fields are
    /// counted from the last `)`.
    pub(super) fn stat(pid: u32) -> Option<Stat> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &text[text.rfind(')')? + 1..];
        let mut fields = rest.split_whitespace();
        let state = fields.next()?.chars().next()?;
        let mut next = || fields.next()?.parse::<i64>().ok();
        let ppid = u32::try_from(next()?).ok()?;
        let (pgrp, session, tty, tpgid) = (next()?, next()?, next()?, next()?);
        Some(Stat {
            state,
            ppid,
            pgrp,
            session,
            tty,
            tpgid,
        })
    }

    fn uid(pid: u32) -> Option<u32> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let line = text.lines().find(|l| l.starts_with("Uid:"))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }

    fn command(pid: u32) -> Option<String> {
        let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let words: Vec<String> = bytes
            .split(|b| *b == 0)
            .filter(|w| !w.is_empty())
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
        (!words.is_empty()).then(|| words.join(" "))
    }

    /// The file a process writes its stdout to, if that is a regular file.
    fn log(pid: u32) -> Option<String> {
        let target = std::fs::read_link(format!("/proc/{pid}/fd/1")).ok()?;
        let meta = std::fs::metadata(&target).ok()?;
        (target.is_absolute() && meta.is_file()).then(|| target.display().to_string())
    }

    /// jouet itself and every process above it: the shell and the agent.
    fn ancestors() -> HashSet<u32> {
        let mut out = HashSet::new();
        let mut pid = std::process::id();
        while pid > 1 && out.insert(pid) {
            match stat(pid) {
                Some(s) => pid = s.ppid,
                None => break,
            }
        }
        out
    }

    pub(super) fn in_worktree(top: &Path) -> Option<Vec<Process>> {
        let me = uid(std::process::id())?;
        let top = top.canonicalize().unwrap_or_else(|_| top.to_path_buf());
        let skip = ancestors();
        let parent = stat(std::process::id()).map(|s| s.ppid);
        let mut found: Vec<(Process, u32)> = Vec::new();
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if skip.contains(&pid) || uid(pid) != Some(me) {
                continue;
            }
            let Some(st) = stat(pid) else { continue };
            if st.state == 'Z' || st.interactive(pid) || Some(st.ppid) == parent {
                continue;
            }
            let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
                continue;
            };
            if !PathBuf::from(&cwd).starts_with(&top) {
                continue;
            }
            let Some(command) = command(pid) else {
                continue;
            };
            found.push((
                Process {
                    pid,
                    command,
                    cwd: cwd.display().to_string(),
                    log: log(pid),
                },
                st.ppid,
            ));
        }
        let pids: HashSet<u32> = found.iter().map(|(p, _)| p.pid).collect();
        let mut out: Vec<Process> = found
            .into_iter()
            .filter(|(_, ppid)| !pids.contains(ppid))
            .map(|(p, _)| p)
            .collect();
        out.sort_by_key(|p| p.pid);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_commands() {
        let mut p = Process {
            pid: 42,
            command: "npm run dev".into(),
            cwd: "/w".into(),
            log: None,
        };
        assert_eq!(p.attach(), "jouet wait pid 42");
        p.log = Some("/tmp/dev server.log".into());
        assert_eq!(p.attach(), "tail -f '/tmp/dev server.log'");
        p.log = Some("/tmp/dev.log".into());
        assert_eq!(p.attach(), "tail -f /tmp/dev.log");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_a_background_process_and_its_log() {
        use std::process::{Command, Stdio};
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("server.log");
        let mut child = Command::new("sleep")
            .arg("30")
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        // Until the child execs, its command line is still this test
        // binary's.
        let cmdline = format!("/proc/{}/cmdline", child.id());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !std::fs::read(&cmdline).unwrap().starts_with(b"sleep\0") {
            assert!(std::time::Instant::now() < deadline, "sleep never started");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let found = in_worktree(dir.path()).unwrap();
        let alive_before = alive(child.id());
        child.kill().unwrap();
        child.wait().unwrap();

        // The child of this test is the test's own descendant, not an
        // ancestor, so it is listed.
        let p = found.iter().find(|p| p.pid == child.id()).expect("listed");
        assert_eq!(p.command, "sleep 30");
        assert_eq!(
            p.log.as_deref(),
            Some(log.canonicalize().unwrap().to_str().unwrap())
        );
        assert_eq!(alive_before, Some(true));
        assert_eq!(alive(child.id()), Some(false));
    }
}
