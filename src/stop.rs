//! Stopping the command jouet runs when jouet itself is told to stop:
//! SIGTERM, which is what `timeout` and a harness send at a time limit.
//!
//! While a command runs, the first SIGTERM sets a flag in place of ending
//! jouet ([`Stop`]), so the command can be stopped and what it had said by
//! then reported. The command is stopped with the processes it started
//! ([`Tree`]): a signal sent to jouet alone reaches none of them, and a
//! shell or a task runner that ends at SIGTERM leaves its own running.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Whether a run holds SIGTERM, for the handlers other code installs.
static HELD: AtomicBool = AtomicBool::new(false);

/// Whether a run holds SIGTERM now, and so acts on it itself: a handler that
/// would end jouet at SIGTERM leaves it alone then.
#[cfg(unix)]
pub(crate) fn held() -> bool {
    HELD.load(Ordering::SeqCst)
}

/// In a [`Stop`]'s state: a run holds SIGTERM.
const HOLDS: u8 = 1;
/// In a [`Stop`]'s state: a SIGTERM came.
const TOLD: u8 = 2;

/// SIGTERM while a command runs. Outside a run, and from the first one a run
/// held, SIGTERM ends jouet as it always did, so a second one is never held
/// up by the report.
pub(crate) struct Stop {
    /// [`HOLDS`] and [`TOLD`], in one value: a SIGTERM and a run that lets
    /// go of it each change it in one step, so whichever is second sees the
    /// first, on whatever thread the signal is handled.
    state: Arc<AtomicU8>,
}

/// What a SIGTERM does to `state`: whether it is recorded for the run that
/// holds it, being the first, in place of ending jouet.
#[cfg(unix)]
fn recorded(state: &AtomicU8) -> bool {
    state.fetch_or(TOLD, Ordering::SeqCst) == HOLDS
}

impl Stop {
    pub(crate) fn new() -> Self {
        let state = Arc::new(AtomicU8::new(0));
        // At each SIGTERM: it ends jouet when no run holds it, or when one
        // was held before; else it is recorded, which is what makes the next
        // one end jouet, so none that follows waits for the state to be read.
        #[cfg(unix)]
        {
            let term = signal_hook::consts::SIGTERM;
            let state = Arc::clone(&state);
            // SAFETY: the handler changes one atomic and calls the
            // signal-safe default handler.
            let _ = unsafe {
                signal_hook::low_level::register(term, move || {
                    if !recorded(&state) {
                        let _ = signal_hook::low_level::emulate_default_handler(term);
                    }
                })
            };
        }
        Self { state }
    }

    /// One that was told to stop already.
    #[cfg(all(test, unix))]
    pub(crate) fn told_already() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(TOLD)),
        }
    }

    /// Holds SIGTERM while a command runs, and lets go of it after.
    pub(crate) fn hold(&self, held: bool) {
        if held {
            self.state.fetch_or(HOLDS, Ordering::SeqCst);
        } else {
            self.state.fetch_and(!HOLDS, Ordering::SeqCst);
        }
        HELD.store(held, Ordering::SeqCst);
    }

    /// Whether jouet was told to stop.
    pub(crate) fn told(&self) -> bool {
        self.state.load(Ordering::SeqCst) & TOLD != 0
    }
}

/// Sends a process SIGTERM, or SIGKILL when `kill`.
#[cfg(unix)]
pub(crate) fn signal(pid: u32, kill: bool) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        let signal = if kill { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: kill(2) with a pid and a constant signal has no memory
        // effects.
        unsafe {
            libc::kill(pid, signal);
        }
    }
}

/// Nothing sends jouet a SIGTERM to pass on.
#[cfg(not(unix))]
pub(crate) fn signal(_pid: u32, _kill: bool) {}

/// A process as the process table lists it.
#[cfg(unix)]
#[derive(Clone)]
struct Proc {
    pid: u32,
    ppid: u32,
    /// When it started, in the table's own terms, which tells it from a
    /// later process given the same id.
    born: String,
}

/// The running processes, without those that exited and wait to be reaped;
/// none where the table cannot be read.
#[cfg(target_os = "linux")]
fn table() -> Vec<Proc> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            // Not as UTF-8: the command's name is cut at 15 bytes, which
            // may be inside a character, and the fields after it are ASCII.
            let stat = std::fs::read(entry.path().join("stat")).ok()?;
            let stat = String::from_utf8_lossy(&stat);
            // The command's name is in parentheses and may hold spaces or
            // parentheses itself: the fields after it start at the state.
            let (_, rest) = stat.rsplit_once(')')?;
            let mut fields = rest.split_whitespace();
            if fields.next()? == "Z" {
                return None;
            }
            let ppid = fields.next()?.parse().ok()?;
            // Field 22 of stat, the start time; the parent was field 4.
            let born = fields.nth(17)?.to_string();
            Some(Proc { pid, ppid, born })
        })
        .collect()
}

/// Without `/proc`, `ps` lists them.
#[cfg(all(unix, not(target_os = "linux")))]
fn table() -> Vec<Proc> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat=,lstart="])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            if fields.next()?.starts_with('Z') {
                return None;
            }
            let born = fields.collect::<Vec<_>>().join(" ");
            Some(Proc { pid, ppid, born })
        })
        .collect()
}

/// A command jouet was told to stop and the processes below it, as the
/// process table had them then and has had them since. A process whose
/// parent had exited by then is not found: the table no longer says whose
/// it was.
#[cfg(unix)]
pub(crate) struct Tree {
    root: u32,
    procs: Vec<Proc>,
}

#[cfg(unix)]
impl Tree {
    /// The tree of the command `root`, which must not have been reaped: its
    /// id may be another process's after that.
    pub(crate) fn of(root: u32) -> Self {
        let mut tree = Self {
            root,
            procs: table().into_iter().filter(|p| p.pid == root).collect(),
        };
        tree.refresh();
        tree
    }

    /// One with nothing in it, for a command that has ended.
    pub(crate) fn none() -> Self {
        Self {
            root: 0,
            procs: Vec::new(),
        }
    }

    /// Drops the processes that have ended, and adds those the rest have
    /// started.
    fn refresh(&mut self) {
        let table = table();
        // By id and start, not parent: one whose parent ended has another.
        self.procs
            .retain(|p| table.iter().any(|t| t.pid == p.pid && t.born == p.born));
        loop {
            let started: Vec<Proc> = table
                .iter()
                .filter(|p| self.has(p.ppid) && !self.has(p.pid))
                .cloned()
                .collect();
            if started.is_empty() {
                break;
            }
            self.procs.extend(started);
        }
    }

    fn has(&self, pid: u32) -> bool {
        self.procs.iter().any(|p| p.pid == pid)
    }

    /// Whether a process below the command is still running.
    pub(crate) fn running(&mut self) -> bool {
        self.refresh();
        self.procs.iter().any(|p| p.pid != self.root)
    }

    /// Sends each process below the command SIGTERM, or SIGKILL when
    /// `kill`. The command itself is its caller's, which knows whether it
    /// has been reaped.
    pub(crate) fn signal(&mut self, kill: bool) {
        self.refresh();
        for p in self.procs.iter().filter(|p| p.pid != self.root) {
            signal(p.pid, kill);
        }
    }
}

/// Nothing sends jouet a SIGTERM, so no tree is ever stopped.
#[cfg(not(unix))]
pub(crate) struct Tree;

#[cfg(not(unix))]
impl Tree {
    pub(crate) fn of(_root: u32) -> Self {
        Self
    }

    pub(crate) fn none() -> Self {
        Self
    }

    pub(crate) fn running(&mut self) -> bool {
        false
    }

    pub(crate) fn signal(&mut self, _kill: bool) {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Waits until `done` says so, for at most ten seconds.
    fn until(mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_sigterm_is_recorded_once_for_the_run_that_holds_it() {
        let stop = Stop {
            state: Arc::new(AtomicU8::new(0)),
        };
        // The first one a run holds is the run's, and the next ends jouet.
        stop.hold(true);
        assert!(!stop.told());
        assert!(recorded(&stop.state));
        assert!(stop.told());
        assert!(!recorded(&stop.state));
        // The run that lets go of it still sees it.
        stop.hold(false);
        assert!(stop.told());

        // One that comes once the run has let go ends jouet: the run did
        // not see it, and will not look again.
        let stop = Stop {
            state: Arc::new(AtomicU8::new(0)),
        };
        stop.hold(true);
        stop.hold(false);
        assert!(!stop.told());
        assert!(!recorded(&stop.state));
    }

    #[test]
    fn a_tree_holds_what_the_command_started_and_stops_it() {
        // A shell that starts another process and waits for it, as a task
        // runner does: SIGTERM to the shell alone leaves the other running.
        let mut child = Command::new("sh")
            .args(["-c", "sleep 300 & wait"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let mut tree = Tree::none();
        assert!(
            until(|| {
                tree = Tree::of(child.id());
                tree.procs.len() == 2
            }),
            "the shell and its sleep are listed"
        );
        let shell = child.id();
        let sleep = tree.procs.iter().find(|p| p.pid != shell).cloned().unwrap();
        assert!(tree.running());
        signal(child.id(), true);
        child.wait().unwrap();
        // The shell is gone, and the sleep is still the tree's.
        assert!(tree.running(), "the sleep outlives the shell");
        assert!(!tree.has(child.id()));
        tree.signal(false);
        // By the process table itself, not by what the tree makes of it.
        let runs = || {
            table()
                .iter()
                .any(|p| p.pid == sleep.pid && p.born == sleep.born)
        };
        assert!(until(|| !runs()), "SIGTERM ends the sleep");
        assert!(!tree.running());
    }
}
