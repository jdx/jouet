//! Commands recorded one at a time, such as jouet's hook ledger holds,
//! rather than read from a harness's transcript.

use crate::analyze::{Analyzer, PatternReport};
use crate::event::{Call, Request, Transcript};
use std::collections::HashMap;

/// One shell command and how it went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandRecord {
    pub session: String,
    /// When it ran, in milliseconds since the Unix epoch.
    pub at: Option<i64>,
    /// The command, or its shape.
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub output_bytes: u64,
}

/// Groups records into one transcript per session, in the order sessions
/// first appear, with each record as one request that has no token count.
/// Records keep their order within a session, which
/// [`Pattern::RepeatedFailure`](crate::Pattern::RepeatedFailure) depends on.
pub fn transcripts(records: &[CommandRecord]) -> Vec<Transcript> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut out: Vec<Transcript> = Vec::new();
    for record in records {
        let i = *index.entry(&record.session).or_insert_with(|| {
            out.push(Transcript {
                session: record.session.clone(),
                ..Default::default()
            });
            out.len() - 1
        });
        let t = &mut out[i];
        let mut call = Call::shell(record.command.clone());
        call.at = record.at;
        call.duration_ms = record.duration_ms;
        call.output_bytes = record.output_bytes;
        call.failed = record.exit_code.is_some_and(|c| c != 0);
        call.exit_code = record.exit_code;
        let n = t.calls.len();
        t.calls.push(call);
        t.requests.push(Request {
            at: record.at,
            calls: n..n + 1,
            ..Default::default()
        });
    }
    out
}

/// The patterns a set of records shows, most frequent first, each with its
/// `top_shapes` most frequent shapes. Without token counts, a finding's cost
/// is its `calls` and `duration_ms`.
pub fn findings(records: &[CommandRecord], top_shapes: usize) -> Vec<PatternReport> {
    let mut analyzer = Analyzer::default();
    for t in transcripts(records) {
        analyzer.add("records", &t);
    }
    analyzer
        .report(top_shapes)
        .harnesses
        .into_iter()
        .next()
        .map(|h| h.patterns)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pattern;

    fn record(session: &str, command: &str, exit_code: i32) -> CommandRecord {
        CommandRecord {
            session: session.into(),
            command: command.into(),
            exit_code: Some(exit_code),
            duration_ms: Some(1000),
            ..Default::default()
        }
    }

    #[test]
    fn groups_records_by_session_in_order() {
        let ts = transcripts(&[
            record("a", "git status", 0),
            record("b", "ls", 0),
            record("a", "cargo test", 101),
        ]);
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].session, "a");
        assert_eq!(ts[0].requests.len(), 2);
        assert_eq!(ts[0].requests[1].calls, 1..2);
        assert!(ts[0].calls[1].failed);
    }

    #[test]
    fn shapes_say_whether_they_ever_succeeded_and_how_they_last_ended() {
        let at = |mut r: CommandRecord, ms| {
            r.at = Some(ms);
            r
        };
        let found = findings(
            &[
                at(record("a", "cargo test -p x | tail -5", 101), 1),
                at(record("a", "cargo test -p x | tail -5", 101), 2),
                at(record("b", "cargo test -p y | tail -5", 0), 3),
                record("c", "never-works --flag", 2),
                record("c", "never-works --flag", 2),
            ],
            5,
        );
        let cut = found
            .iter()
            .find(|f| f.pattern == Pattern::HeadOnTestOutput)
            .expect("cut test output");
        let s = &cut.top_shapes[0];
        assert_eq!((s.calls, s.failures, s.successes), (3, 2, 1));
        assert_eq!((s.last_failed, s.last_exit_code), (false, Some(0)));
        let retry = found
            .iter()
            .find(|f| f.pattern == Pattern::RepeatedFailure)
            .expect("a retry");
        let shapes: Vec<_> = retry
            .top_shapes
            .iter()
            .map(|s| (s.shape.as_str(), s.successes, s.last_exit_code))
            .collect();
        assert!(
            shapes.contains(&("never-works --flag", 0, Some(2))),
            "{shapes:?}"
        );
    }

    #[test]
    fn finds_patterns_in_records_retries_included() {
        let found = findings(
            &[
                record("a", "cargo test -p x 2>&1 | tail -5", 101),
                record("a", "cargo test -p y 2>&1 | tail -5", 101),
                record("a", "cargo test -p z 2>&1 | tail -5", 0),
                record("b", "for n in 1 2; do gh pr view $n; done", 0),
                record("b", "sleep 30; gh pr checks 3", 1),
            ],
            2,
        );
        let get = |p: Pattern| found.iter().find(|f| f.pattern == p);
        let cut = get(Pattern::HeadOnTestOutput).expect("cut test output");
        assert_eq!((cut.calls, cut.sessions, cut.duration_ms), (3, 1, 3000));
        assert_eq!(
            cut.top_shapes[0].shape,
            "cargo test -p <v> 2>&1 | tail -<n>"
        );
        // Only the second failure of the same shape is a retry.
        assert_eq!(get(Pattern::RepeatedFailure).map(|f| f.calls), Some(1));
        assert_eq!(get(Pattern::GhLoop).map(|f| f.calls), Some(1));
        assert_eq!(get(Pattern::SleepPoll).map(|f| f.calls), Some(1));
        assert_eq!(found[0].pattern, Pattern::HeadOnTestOutput);
    }
}
