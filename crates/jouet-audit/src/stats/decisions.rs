//! What each rule decision cost, one decision at a time: the model requests
//! from a deny, warning or rewrite until the session next ran a check or
//! edited a file. The ledger says when each happened; the session's
//! transcripts say how many requests came between.

use super::quantile;
use crate::event::Transcript;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The actions judged one decision at a time.
pub(crate) const JUDGED: [&str; 3] = ["deny", "warn", "rewrite"];

/// One decision, and when the session next ran a check or edited a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionSpan {
    pub rule: String,
    /// `deny`, `warn` or `rewrite`.
    pub action: String,
    pub session: String,
    /// The decision's ledger time, in seconds since the Unix epoch.
    pub at: u64,
    /// When the next check or edit was issued, if the session had one, in
    /// milliseconds: the end of its ledger second less how long it ran, so
    /// requests its output led to are not counted.
    pub until_ms: Option<u64>,
}

/// One rule's decisions of one action, and the requests each took until
/// the next check or edit.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DecisionsAfter {
    pub rule: String,
    pub action: String,
    pub decisions: u64,
    /// Those followed by a check or an edit, whose session's transcripts
    /// were found: what the requests are counted over.
    pub measured: u64,
    /// The median requests until then, and the interquartile range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p25: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p75: Option<f64>,
    /// Those the session never followed with a check or edit.
    pub open: u64,
}

/// When each of a session's requests began, in milliseconds, each counted
/// once where a resumed or forked copy repeats it. A request begins at its
/// time or its first call, whichever is earlier, since Codex dates a
/// request after its calls.
pub fn request_times(transcripts: &[Transcript]) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut times: Vec<i64> = transcripts
        .iter()
        .flat_map(|t| t.requests.iter().map(move |r| (t, r)))
        .filter(|(_, r)| r.id.is_none_or(|id| seen.insert(id)))
        .filter_map(|(t, r)| {
            let calls = t.calls.get(r.calls.clone()).unwrap_or_default();
            let first = calls.iter().filter_map(|c| c.at).min();
            match (r.at, first) {
                (Some(at), Some(first)) => Some(at.min(first)),
                (at, first) => at.or(first),
            }
        })
        .collect();
    times.sort_unstable();
    times
}

/// The requests from a decision until the next check or edit: those that
/// began after the decision's second and before the check or edit was
/// issued, so the request that issued it is counted and the one that issued
/// the decided call is not. The ledger keeps whole seconds, so a request
/// within a second of the decision counts as the decision's.
fn requests_between(times: &[i64], at: u64, until_ms: u64) -> u64 {
    let from = i64::try_from(at.saturating_add(1).saturating_mul(1000)).unwrap_or(i64::MAX);
    let to = i64::try_from(until_ms).unwrap_or(i64::MAX);
    times.iter().filter(|&&t| t >= from && t < to).count() as u64
}

/// Each rule's decisions by action, with the requests until the next check
/// or edit, given each session's [`request_times`]. Most decisions first.
pub fn after_decisions(
    spans: &[DecisionSpan],
    times: &HashMap<String, Vec<i64>>,
) -> Vec<DecisionsAfter> {
    let mut groups: BTreeMap<(&str, &str), (DecisionsAfter, Vec<f64>)> = BTreeMap::new();
    for span in spans {
        let (after, requests) = groups
            .entry((span.rule.as_str(), span.action.as_str()))
            .or_insert_with(|| {
                let after = DecisionsAfter {
                    rule: span.rule.clone(),
                    action: span.action.clone(),
                    ..DecisionsAfter::default()
                };
                (after, Vec::new())
            });
        after.decisions += 1;
        match (span.until_ms, times.get(&span.session)) {
            (None, _) => after.open += 1,
            (Some(until_ms), Some(times)) => {
                requests.push(requests_between(times, span.at, until_ms) as f64);
            }
            (Some(_), None) => {}
        }
    }
    let mut out: Vec<DecisionsAfter> = groups
        .into_values()
        .map(|(mut after, requests)| {
            after.measured = requests.len() as u64;
            if !requests.is_empty() {
                after.median = Some(quantile(&requests, 0.5));
                after.p25 = Some(quantile(&requests, 0.25));
                after.p75 = Some(quantile(&requests, 0.75));
            }
            after
        })
        .collect();
    out.sort_by(|a, b| {
        b.decisions
            .cmp(&a.decisions)
            .then_with(|| a.rule.cmp(&b.rule))
            .then_with(|| a.action.cmp(&b.action))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Request;

    fn transcript(requests: &[(Option<u64>, i64)]) -> Transcript {
        Transcript {
            requests: requests
                .iter()
                .map(|&(id, at)| Request {
                    id,
                    at: Some(at),
                    ..Request::default()
                })
                .collect(),
            ..Transcript::default()
        }
    }

    #[test]
    fn requests_are_counted_once_and_in_order() {
        let resumed = [
            transcript(&[(Some(1), 3_000), (Some(2), 1_000)]),
            transcript(&[(Some(1), 3_000), (None, 2_000), (None, 2_000)]),
        ];
        assert_eq!(request_times(&resumed), [1_000, 2_000, 2_000, 3_000]);
    }

    #[test]
    fn a_decision_costs_the_requests_until_the_next_check_or_edit() {
        // The denied call's request at 100.4s; the next ones at 103, 107
        // and 110.5, the last issuing a check by 110.9s.
        let times = HashMap::from([(
            "s".to_string(),
            vec![100_400, 103_000, 107_000, 110_500, 120_000],
        )]);
        let span = |rule: &str, action: &str, session: &str, at, until_ms| DecisionSpan {
            rule: rule.into(),
            action: action.into(),
            session: session.into(),
            at,
            until_ms,
        };
        let spans = [
            span("piped-test-output", "deny", "s", 100, Some(110_900)),
            span("piped-test-output", "deny", "s", 103, Some(107_500)),
            span("piped-test-output", "deny", "s", 120, None),
            // A warned check runs at once: no request in between, though
            // the next one came while it ran.
            span("piped-test-output", "warn", "s", 110, Some(110_400)),
            // A session whose transcripts were not found.
            span("piped-test-output", "rewrite", "gone", 100, Some(110_900)),
        ];
        let after = after_decisions(&spans, &times);
        let summary: Vec<_> = after
            .iter()
            .map(|a| {
                (
                    a.action.as_str(),
                    a.decisions,
                    a.measured,
                    a.median,
                    a.p75,
                    a.open,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("deny", 3, 2, Some(2.0), Some(2.5), 1),
                ("rewrite", 1, 0, None, None, 0),
                ("warn", 1, 1, Some(0.0), Some(0.0), 0),
            ]
        );
    }
}
