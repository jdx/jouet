//! What builds, tests and lints cost here: how long each run took, kept as
//! git notes on the commit it ran at, under `refs/notes/jouet-cost`.
//!
//! Timings from different machines do not compare, so each sample says
//! which machine took it. CI's samples give how costly each command is
//! against the others; this machine's own samples turn those ratios into
//! time here. The scale is the median, over the commands timed both here
//! and elsewhere, of this machine's time over theirs, and a command timed
//! only elsewhere is estimated at their time times that scale.

use jouet_git::Git;
use jouet_git::notes::Notes;
use jouet_rules::Record;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The notes ref, under `refs/notes/`.
pub(crate) const NOTES: &str = "jouet-cost";
/// How many first-parent commits back estimates read.
const DEPTH: usize = 200;
/// The most recent samples of a command that an estimate uses.
const RECENT: usize = 20;

/// One timed run, as a line of a note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sample {
    pub v: u8,
    /// The check's kind: `test`, `build`, `lint`, ...
    pub kind: String,
    /// The command as the `checks` rule recorded it.
    pub cmd: String,
    /// The command with the last word of what it selected (a test name, a
    /// package) as `…`, so one e2e test's cost speaks for the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    pub ms: u64,
    pub exit: i32,
    /// The machine, as a hash: samples from one machine compare.
    pub host: String,
    /// Whether it ran on CI.
    #[serde(default)]
    pub ci: bool,
    pub cpus: u32,
    pub os: String,
    /// Seconds since the Unix epoch.
    pub ts: u64,
}

impl Sample {
    /// A sample of `record`'s command, run on this machine.
    pub(crate) fn of(record: &Record, ms: u64, exit: i32) -> Self {
        Self {
            v: 1,
            kind: record.kind.name().to_string(),
            cmd: crate::redact::redact(&record.command),
            family: family(
                &crate::redact::redact(&record.command),
                record.selector.as_deref(),
            ),
            ms,
            exit,
            host: host(),
            ci: std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "false"),
            cpus: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            os: std::env::consts::OS.to_string(),
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        }
    }

    /// Whether its command reads as one: the primer and `jouet cost` print
    /// it, and notes fetched from a remote are anyone's writing, so a line
    /// break or a terminal escape, or a key past 200 characters, drops it.
    fn plain(&self) -> bool {
        let fits = |s: &str| s.chars().count() <= 200 && !s.chars().any(char::is_control);
        fits(&self.cmd) && self.family.as_deref().is_none_or(fits)
    }

    /// The key estimates group it under: its family, else its command.
    fn key(&self) -> &str {
        self.family.as_deref().unwrap_or(&self.cmd)
    }
}

/// `cmd` with the last word of `selector` as `…`, when the command ends
/// with it: `mise run test:e2e test_use` selects `test:e2e test_use`,
/// and its family is `mise run test:e2e …`, not `mise run …`.
fn family(cmd: &str, selector: Option<&str>) -> Option<String> {
    let last = selector?.split_whitespace().last()?;
    let rest = cmd.strip_suffix(last)?;
    rest.ends_with(' ').then(|| format!("{rest}…"))
}

/// This machine, as the first 12 hex characters of the SHA-256 of its
/// machine id, or of its host name where there is none.
fn host() -> String {
    let id = ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        // macOS has neither file, and a hook seldom has HOSTNAME.
        .or_else(hostname)
        .unwrap_or_default();
    let digest = Sha256::digest(id.trim().as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// The host name, from the system.
#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `buf.len()` bytes into `buf`.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// The host name, from the system.
#[cfg(not(unix))]
fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Keeps `samples` in the note on HEAD, unless `[cost] record` is off.
/// Best effort: a run never fails for want of its timing.
pub(crate) fn record(cwd: &Path, samples: &[Sample]) {
    if samples.is_empty() || !crate::settings::Settings::load().cost.record {
        return;
    }
    let lines: Vec<String> = samples
        .iter()
        .filter_map(|s| serde_json::to_string(s).ok())
        .collect();
    let _ = Notes::new(Git::new(cwd), NOTES).append("HEAD", &lines);
}

/// What one command is estimated to take here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Estimate {
    /// The command, or its family.
    pub key: String,
    pub kind: String,
    pub ms: u64,
    /// Samples taken on this machine.
    pub here: usize,
    /// Samples taken elsewhere.
    pub elsewhere: usize,
    /// The scale applied to the samples from elsewhere; None when the
    /// estimate is this machine's own median.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
}

/// The samples along HEAD, with this machine's scale.
pub(crate) struct Costs {
    /// Newest first.
    samples: Vec<Sample>,
    host: String,
    scale: f64,
    /// How many commands the scale was measured on; 0 means it is 1 by
    /// default.
    pub scaled_by: usize,
}

impl Costs {
    pub(crate) fn load(cwd: &Path) -> Self {
        let notes = Notes::new(Git::new(cwd), NOTES);
        let samples = notes
            .along("HEAD", DEPTH)
            .into_iter()
            .flat_map(|(_, lines)| lines)
            .filter_map(|l| serde_json::from_str::<Sample>(&l).ok())
            .filter(Sample::plain)
            .collect();
        Self::new(samples, host())
    }

    pub(crate) fn new(mut samples: Vec<Sample>, host: String) -> Self {
        samples.sort_by_key(|s| std::cmp::Reverse(s.ts));
        let mut costs = Self {
            samples,
            host,
            scale: 1.0,
            scaled_by: 0,
        };
        let ratios: Vec<f64> = costs
            .by(|s| s.cmd.as_str())
            .values()
            .filter_map(|runs| {
                let (here, there) = costs.split(runs);
                Some(median(&here)? as f64 / median(&there)?.max(1) as f64)
            })
            .collect();
        if let Some(scale) = median_f(&ratios) {
            costs.scale = scale;
            costs.scaled_by = ratios.len();
        }
        costs
    }

    /// This machine's time over the others'.
    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The samples grouped by `key`: the ones that passed, or all of them
    /// when none did, since a failing run can stop early.
    fn by<'a>(&'a self, key: impl Fn(&'a Sample) -> &'a str) -> BTreeMap<&'a str, Vec<&'a Sample>> {
        let mut groups: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
        for s in &self.samples {
            groups.entry(key(s)).or_default().push(s);
        }
        for runs in groups.values_mut() {
            if runs.iter().any(|s| s.exit == 0) {
                runs.retain(|s| s.exit == 0);
            }
            runs.truncate(RECENT);
        }
        groups
    }

    /// The durations taken here and elsewhere.
    fn split(&self, runs: &[&Sample]) -> (Vec<u64>, Vec<u64>) {
        let (here, there): (Vec<&&Sample>, Vec<&&Sample>) =
            runs.iter().partition(|s| s.host == self.host);
        (
            here.iter().map(|s| s.ms).collect(),
            there.iter().map(|s| s.ms).collect(),
        )
    }

    fn estimate_of(&self, key: &str, runs: &[&Sample]) -> Option<Estimate> {
        let (here, there) = self.split(runs);
        let (ms, scale) = match median(&here) {
            Some(ms) => (ms, None),
            None => (
                (median(&there)? as f64 * self.scale) as u64,
                Some(self.scale),
            ),
        };
        Some(Estimate {
            key: key.to_string(),
            kind: runs[0].kind.clone(),
            ms,
            here: here.len(),
            elsewhere: there.len(),
            scale,
        })
    }

    /// Every command, costliest first: a family of them as one, by the
    /// name it gives what they select, once it holds two commands.
    pub(crate) fn table(&self) -> Vec<Estimate> {
        let mut all: Vec<Estimate> = self
            .by(Sample::key)
            .iter()
            .filter_map(|(key, runs)| {
                let one = runs.iter().all(|s| s.cmd == runs[0].cmd);
                self.estimate_of(if one { &runs[0].cmd } else { key }, runs)
            })
            .collect();
        all.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.key.cmp(&b.key)));
        all
    }
}

/// The fewest seconds a check takes to be named at session start.
const PRIMED_MS: u64 = 20_000;
/// How many checks session start names.
const PRIMED: usize = 4;

/// One line for the session-start primer: the costliest checks here, so
/// the agent can plan its runs around them. None without a check that
/// takes [`PRIMED_MS`] or more.
pub(crate) fn primer_line(costs: &Costs) -> Option<String> {
    let slow: Vec<String> = costs
        .table()
        .into_iter()
        .filter(|e| e.ms >= PRIMED_MS)
        .take(PRIMED)
        .map(|e| format!("`{}` ~{}", e.key, jouet_audit::digest::duration(e.ms)))
        .collect();
    if slow.is_empty() {
        return None;
    }
    Some(format!(
        "jouet: slow checks here: {} (`jouet cost` lists all). Finish the edits a slow run checks before starting it, so one run covers them.",
        slow.join(", ")
    ))
}

fn median(values: &[u64]) -> Option<u64> {
    let mut v = values.to_vec();
    v.sort_unstable();
    match v.len() {
        0 => None,
        n if n % 2 == 1 => Some(v[n / 2]),
        // Halved first: two values near u64::MAX would overflow.
        n => {
            let (a, b) = (v[n / 2 - 1], v[n / 2]);
            Some(a / 2 + b / 2 + (a % 2 + b % 2) / 2)
        }
    }
}

fn median_f(values: &[f64]) -> Option<f64> {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => None,
        n if n % 2 == 1 => Some(v[n / 2]),
        n => Some((v[n / 2 - 1] + v[n / 2]) / 2.0),
    }
}

#[cfg(test)]
#[path = "cost_tests.rs"]
mod tests;
