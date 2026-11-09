//! What jouet itself cost, as the transcripts show it: where it met a usage
//! error, crashed or was dropped for the raw command right after it answered
//! ([`digest::jouet`](crate::digest::jouet)), and where an agent read a test
//! log right after `jouet test` had summarized it. The hook ledger's blocks
//! and denials are joined to this by whoever reads the ledger.
//!
//! Only calls of the requests a transcript kept count, so `--since` applies.
//! Commands are kept only as shapes.

use crate::digest::jouet::Kind;
use crate::event::{Call, Tool, Transcript};
use crate::shape::shape;
use crate::stats::jouet_commands;
use serde::Serialize;
use std::collections::BTreeMap;

/// A test log read in this many requests after a `jouet test` run counts.
const LOG_READ_REQUESTS: usize = 3;

#[derive(Debug, Clone, Default)]
pub(crate) struct OwnAcc {
    sessions: BTreeMap<String, OwnSession>,
}

/// One session's own jouet costs.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OwnSession {
    pub session: String,
    pub harness: String,
    /// The directory it ran in, the home directory written `~`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// `jouet test` runs.
    #[serde(skip_serializing_if = "is_zero")]
    pub test_runs: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<OwnFinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log_reads: Vec<LogRead>,
}

/// Something jouet did wrong or fell short in.
#[derive(Debug, Clone, Serialize)]
pub struct OwnFinding {
    pub kind: Kind,
    /// The jouet command, such as `jouet pr status`.
    pub subject: String,
    /// What was seen, without values.
    pub detail: String,
    /// When, in milliseconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// A test log read soon after `jouet test` summarized the run.
#[derive(Debug, Clone, Serialize)]
pub struct LogRead {
    /// When the log was read, in milliseconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    /// The `jouet test` run's shape.
    pub test: String,
    /// The read's shape, or the tool's name for a read that ran no command.
    pub read: String,
    /// Requests after the run's.
    pub after_requests: usize,
}

impl OwnAcc {
    pub(crate) fn add(&mut self, harness: &str, t: &Transcript, home: Option<&str>) {
        let calls: Vec<(usize, &Call)> = t
            .requests
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                t.calls
                    .get(r.calls.clone())
                    .unwrap_or_default()
                    .iter()
                    .map(move |c| (i, c))
            })
            .collect();
        let only: Vec<&Call> = calls.iter().map(|(_, c)| *c).collect();
        let findings: Vec<OwnFinding> = crate::digest::jouet_findings(&only)
            .into_iter()
            .map(|f| OwnFinding {
                kind: f.found.kind,
                subject: f.found.subject,
                detail: f.found.detail,
                at: f.at,
            })
            .collect();
        let (test_runs, log_reads) = log_reads(&calls);
        let s = self
            .sessions
            .entry(t.session.clone())
            .or_insert_with(|| OwnSession {
                session: t.session.clone(),
                harness: harness.to_string(),
                ..OwnSession::default()
            });
        if s.project.is_none() {
            s.project = t.cwd.as_deref().map(|cwd| match home {
                Some(home) if home.len() > 1 && cwd.starts_with(home) => {
                    format!("~{}", &cwd[home.len()..])
                }
                _ => cwd.to_string(),
            });
        }
        s.test_runs += test_runs;
        s.findings.extend(findings);
        s.log_reads.extend(log_reads);
    }

    pub(crate) fn merge(&mut self, theirs: Self) {
        for (id, t) in theirs.sessions {
            match self.sessions.get_mut(&id) {
                None => {
                    self.sessions.insert(id, t);
                }
                Some(o) => {
                    if o.project.is_none() {
                        o.project = t.project;
                    }
                    o.test_runs += t.test_runs;
                    o.findings.extend(t.findings);
                    o.log_reads.extend(t.log_reads);
                }
            }
        }
    }

    /// Every session, by ID, its findings and log reads in time order.
    pub(crate) fn sessions(&self) -> Vec<OwnSession> {
        self.sessions
            .values()
            .cloned()
            .map(|mut s| {
                s.findings.sort_by_key(|f| f.at);
                s.log_reads.sort_by_key(|r| r.at);
                s
            })
            .collect()
    }
}

/// Whether a shell command runs `jouet test`.
fn runs_jouet_test(command: &str) -> bool {
    jouet_commands(&shape(command))
        .iter()
        .any(|c| c == "jouet test" || c.starts_with("jouet test "))
}

/// Whether a call reads one of `jouet test`'s logs:
/// `<log dir>/test-<ms>-<pid>-<n>.log`.
fn reads_test_log(call: &Call) -> bool {
    let text = match call.tool {
        Tool::Shell => call.command.as_deref(),
        _ => call.target.as_deref(),
    };
    text.is_some_and(|t| {
        t.match_indices("test-").any(|(i, _)| {
            let rest = &t[i + "test-".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            name.split('-').count() == 3 && rest[name.len()..].starts_with(".log")
        })
    })
}

/// `jouet test` runs, and the test logs read in the requests just after
/// one: each run's first read, each read counted once.
fn log_reads(calls: &[(usize, &Call)]) -> (u64, Vec<LogRead>) {
    let mut runs = 0;
    let mut out = Vec::new();
    let mut used = vec![false; calls.len()];
    for &(request, call) in calls {
        if call.tool != Tool::Shell || !call.command.as_deref().is_some_and(runs_jouet_test) {
            continue;
        }
        runs += 1;
        let test = shape(call.command.as_deref().unwrap_or_default());
        let read = calls.iter().enumerate().find(|(j, (r, c))| {
            *r > request && *r <= request + LOG_READ_REQUESTS && !used[*j] && reads_test_log(c)
        });
        if let Some((j, &(r, c))) = read {
            used[j] = true;
            out.push(LogRead {
                at: c.at,
                test,
                read: c
                    .command
                    .as_deref()
                    .map(shape)
                    .or_else(|| c.name.clone())
                    .unwrap_or_else(|| "a read".into()),
                after_requests: r - request,
            });
        }
    }
    (runs, out)
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Request;

    fn transcript(requests: Vec<Vec<Call>>) -> Transcript {
        let mut t = Transcript {
            session: "s".into(),
            cwd: Some("/home/me/src/app".into()),
            ..Default::default()
        };
        for calls in requests {
            let start = t.calls.len();
            t.calls.extend(calls);
            t.requests.push(Request {
                calls: start..t.calls.len(),
                ..Default::default()
            });
        }
        t
    }

    fn read(path: &str) -> Call {
        let mut c = Call::new(Tool::Read);
        c.name = Some("Read".into());
        c.target = Some(path.into());
        c
    }

    const LOG: &str = "/home/me/.local/state/jouet/logs/test-1793404927123-4242-0.log";

    #[test]
    fn a_test_log_read_soon_after_jouet_test_counts_once() {
        let t = transcript(vec![
            vec![Call::shell("jouet test --cmd -- cargo test 2>&1")],
            vec![Call::shell(format!("tail -50 {LOG}"))],
            vec![read(LOG)],
            vec![Call::shell("cd x && jouet test --affected")],
            vec![Call::shell("ls")],
            vec![Call::shell("ls")],
            vec![Call::shell("ls")],
            vec![Call::shell(format!("grep FAIL {LOG}"))],
        ]);
        let mut acc = OwnAcc::default();
        acc.add("claude", &t, Some("/home/me"));
        let s = &acc.sessions()[0];
        assert_eq!(s.project.as_deref(), Some("~/src/app"));
        assert_eq!(s.test_runs, 2);
        assert_eq!(s.log_reads.len(), 1, "{:?}", s.log_reads);
        assert_eq!(s.log_reads[0].read, "tail -<n> <path>");
        assert_eq!(s.log_reads[0].test, "jouet test --cmd -- <v>… 2>&1");
        assert_eq!(s.log_reads[0].after_requests, 1);
    }

    #[test]
    fn other_logs_and_reads_before_the_run_do_not_count() {
        let t = transcript(vec![
            vec![read(LOG)],
            vec![Call::shell("jouet test --cmd -- cargo test")],
            vec![Call::shell("cat ~/.local/state/jouet/logs/ci-run-1-2.log")],
            vec![read("/src/test-data.log")],
        ]);
        let mut acc = OwnAcc::default();
        acc.add("claude", &t, None);
        assert!(acc.sessions()[0].log_reads.is_empty());
    }

    #[test]
    fn usage_errors_count_only_in_the_requests_kept() {
        let mut usage = Call::shell("jouet read src/a.rs --range 1,5");
        usage.exit_code = Some(2);
        usage.failed = true;
        let mut t = transcript(vec![vec![usage.clone()], vec![usage]]);
        // `--since` left out the first request.
        t.requests.remove(0);
        let mut a = OwnAcc::default();
        a.add("claude", &t, None);
        let mut b = OwnAcc::default();
        b.add("claude", &t, None);
        a.merge(b);
        let s = &a.sessions()[0];
        assert_eq!(s.findings.len(), 2);
        assert_eq!(s.findings[0].kind, Kind::Usage);
        assert_eq!(s.findings[0].subject, "jouet read");
    }
}
