//! The experiment arms compared: for each toggle under test, the sessions
//! that kept its old behaviour (`control`) against those on the new one
//! (`new`), per session, on requests, input tokens, active time and CI
//! failures. The arms are small (tens of sessions a week), so each shows its
//! n, median and interquartile range, and the difference of medians a
//! bootstrap 95% interval.

use super::{LedgerEntry, StatsFilter};
use crate::Transcript;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// What one session cost, from its transcripts, its subagents' included.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SessionMetrics {
    pub session: String,
    /// Model requests, each counted once even where a forked or resumed
    /// session repeats them.
    pub requests: u64,
    /// Input tokens over those requests, cached or not.
    pub input_tokens: u64,
    /// Model time plus tool time, in milliseconds, as wall time: every
    /// request of every transcript of the session is put on one timeline,
    /// each spanning from the model's wait before it to the end of its
    /// calls, and the time any of them covers is counted once. The wait is
    /// from the end of whatever came before it, at most 15 minutes, so a
    /// person away from the keyboard is not counted. A resumed or forked
    /// copy, or a subagent working alongside the lead, adds no time twice,
    /// and the order the transcripts are read in does not matter.
    pub active_ms: u64,
    /// CI-monitor events reporting failing checks: the most any one of its
    /// transcripts has, since a resumed or forked copy repeats them and a
    /// subagent gets none.
    pub ci_failures: u64,
    /// The directory the session ran in, from its first transcript that
    /// names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The repository it ran in, as the caller names it from `cwd` (the
    /// same for every worktree of one repository); `None` when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

/// Longest a request's model time counts for.
const MODEL_CAP_MS: i64 = 15 * 60 * 1000;

impl SessionMetrics {
    /// The metrics of `session` from its transcripts.
    pub fn of(session: &str, transcripts: &[Transcript]) -> Self {
        let mut m = Self {
            session: session.to_string(),
            ..Self::default()
        };
        let mut seen = HashSet::new();
        // Each request's span: from it, or its first call where a harness
        // dates the request after its calls (Codex), to the end of the
        // last one.
        let mut spans: Vec<(i64, i64)> = Vec::new();
        for t in transcripts {
            m.ci_failures = m.ci_failures.max(t.ci_failures);
            if m.cwd.is_none() {
                m.cwd.clone_from(&t.cwd);
            }
            for request in &t.requests {
                if let Some(at) = request.at {
                    let calls = &t.calls[request.calls.clone()];
                    let ends = calls.iter().filter_map(|c| {
                        let ms = i64::try_from(c.duration_ms.unwrap_or(0)).unwrap_or(0);
                        Some(c.at? + ms)
                    });
                    let starts = calls.iter().filter_map(|c| c.at);
                    spans.push((starts.fold(at, i64::min), ends.fold(at, i64::max)));
                }
                // A resumed or forked copy repeats its parent's requests:
                // their tokens count once.
                if request.id.is_some_and(|id| !seen.insert(id)) {
                    continue;
                }
                m.requests += 1;
                m.input_tokens += request.input_tokens.unwrap_or(0);
            }
        }
        m.active_ms = covered(spans);
        m
    }

    fn value(&self, metric: Metric) -> f64 {
        let n = match metric {
            Metric::Requests => self.requests,
            Metric::InputTokens => self.input_tokens,
            Metric::ActiveMinutes => return self.active_ms as f64 / 60_000.0,
            Metric::CiFailures => self.ci_failures,
        };
        n as f64
    }
}

/// The time `spans` cover, each `(request, end of its calls)` widened back
/// by the model's wait since the end of whatever came before it, at most
/// [`MODEL_CAP_MS`]; the first has no wait to count.
fn covered(mut spans: Vec<(i64, i64)>) -> u64 {
    spans.sort_unstable();
    let mut total: i64 = 0;
    let mut reached: Option<i64> = None;
    for (at, end) in spans {
        let start = match reached {
            Some(before) if before < at => before.max(at - MODEL_CAP_MS),
            Some(_) => at,
            None => at,
        };
        let from = reached.map_or(start, |before| start.max(before));
        total += (end - from).max(0);
        reached = Some(reached.map_or(end, |before| before.max(end)));
    }
    u64::try_from(total).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    Requests,
    InputTokens,
    ActiveMinutes,
    CiFailures,
}

impl Metric {
    const ALL: [Self; 4] = [
        Self::Requests,
        Self::InputTokens,
        Self::ActiveMinutes,
        Self::CiFailures,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::InputTokens => "input_tokens",
            Self::ActiveMinutes => "active_minutes",
            Self::CiFailures => "ci_failures",
        }
    }
}

/// One toggle's arms, compared within one repository.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArmComparison {
    /// The repository the sessions ran in; `None` for those whose
    /// repository is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// `gate` or `python-edit`.
    pub toggle: String,
    /// The old behaviour.
    pub control: ArmSpread,
    /// The new default.
    pub new: ArmSpread,
    /// Control less new, by metric; none while an arm has no sessions.
    pub differences: Vec<Difference>,
}

/// One arm's sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ArmSpread {
    /// Sessions in the arm whose transcripts were read.
    pub sessions: u64,
    /// By metric: the median and the interquartile range; none without
    /// sessions.
    pub metrics: Vec<Spread>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Spread {
    pub metric: String,
    pub median: f64,
    pub p25: f64,
    pub p75: f64,
}

/// The control arm's median less the new arm's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Difference {
    pub metric: String,
    pub median: f64,
    /// A 95% bootstrap interval of it, when each arm has at least two
    /// sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_high: Option<f64>,
    /// Sessions each arm needs before a difference of each of
    /// [`NEEDED_FOR`] would show, with an interval: none without one, or
    /// when the new arm's median is zero.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub needed: Vec<Needed>,
}

/// The differences, as percent of the new arm's median, that
/// [`Difference::needed`] sizes the arms for.
pub const NEEDED_FOR: [u64; 2] = [30, 10];

/// How many sessions a difference of `percent` of the new arm's median
/// needs to show: the 95% interval scaled from the arms' own spread to two
/// equal arms, until its half-width is that difference. A true difference
/// that size then has about even odds of the interval leaving out zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Needed {
    pub percent: u64,
    /// Sessions in each arm.
    pub per_arm: u64,
    /// Sessions still to come, over both arms: what each arm lacks of
    /// `per_arm`.
    pub more: u64,
}

/// The sessions per arm for a 95% interval of half-width `target`, from
/// one of `half_width` with `control` and `new` sessions: the half-width
/// goes as the square root of `1/control + 1/new`, which is `2/n` for two
/// arms of `n`.
fn per_arm(half_width: f64, target: f64, control: u64, new: u64) -> Option<u64> {
    if target <= 0.0 || control == 0 || new == 0 {
        return None;
    }
    let spread = 1.0 / control as f64 + 1.0 / new as f64;
    let n = 2.0 * (half_width / target).powi(2) / spread;
    Some((n.ceil() as u64).max(2))
}

/// Each session's arm of each toggle, from its session-start entries that
/// `filter` keeps, by session.
pub fn session_arms(
    entries: &[LedgerEntry],
    filter: &StatsFilter,
) -> BTreeMap<String, BTreeMap<String, String>> {
    entries
        .iter()
        .filter(|e| e.event == "session-start" && !e.arm.is_empty() && filter.keeps(e))
        .filter_map(|e| Some((e.session.clone()?, e.arm.clone())))
        .collect()
}

/// Each toggle's arms compared within each repository, over the sessions
/// `metrics` has. Sessions in different repositories do different work, so
/// they are never pooled. The repository with the most sessions comes
/// first, those of an unknown repository last.
pub fn compare_arms(
    arms: &BTreeMap<String, BTreeMap<String, String>>,
    metrics: &[SessionMetrics],
) -> Vec<ArmComparison> {
    let mut by_repository: BTreeMap<Option<&str>, Vec<&SessionMetrics>> = BTreeMap::new();
    for m in metrics {
        by_repository
            .entry(m.repository.as_deref())
            .or_default()
            .push(m);
    }
    let mut repositories: Vec<_> = by_repository.into_iter().collect();
    repositories.sort_by_key(|(repository, sessions)| {
        (repository.is_none(), std::cmp::Reverse(sessions.len()))
    });
    repositories
        .into_iter()
        .flat_map(|(repository, sessions)| compare_within(repository, arms, &sessions))
        .collect()
}

/// Each toggle's arms compared over `metrics`, the sessions of one
/// repository.
fn compare_within(
    repository: Option<&str>,
    arms: &BTreeMap<String, BTreeMap<String, String>>,
    metrics: &[&SessionMetrics],
) -> Vec<ArmComparison> {
    let toggles: std::collections::BTreeSet<&String> = metrics
        .iter()
        .filter_map(|m| arms.get(&m.session))
        .flat_map(BTreeMap::keys)
        .collect();
    toggles
        .into_iter()
        .map(|toggle| {
            let group = |arm: &str| -> Vec<&SessionMetrics> {
                metrics
                    .iter()
                    .copied()
                    .filter(|m| {
                        arms.get(&m.session)
                            .and_then(|a| a.get(toggle))
                            .is_some_and(|a| a == arm)
                    })
                    .collect()
            };
            let (control, new) = (group("control"), group("new"));
            // An arm with no sessions has no median to compare.
            let both = !control.is_empty() && !new.is_empty();
            let differences = Metric::ALL
                .iter()
                .filter(|_| both)
                .map(|&metric| {
                    let values = |group: &[&SessionMetrics]| -> Vec<f64> {
                        group.iter().map(|m| m.value(metric)).collect()
                    };
                    let (c, n) = (values(&control), values(&new));
                    let ci = bootstrap(&c, &n);
                    let base = quantile(&n, 0.5);
                    let (sessions_c, sessions_n) = (c.len() as u64, n.len() as u64);
                    let needed = NEEDED_FOR
                        .iter()
                        .filter_map(|&percent| {
                            let (low, high) = ci?;
                            let target = base * percent as f64 / 100.0;
                            let per_arm =
                                per_arm((high - low) / 2.0, target, sessions_c, sessions_n)?;
                            Some(Needed {
                                percent,
                                per_arm,
                                more: per_arm.saturating_sub(sessions_c)
                                    + per_arm.saturating_sub(sessions_n),
                            })
                        })
                        .collect();
                    Difference {
                        metric: metric.name().to_string(),
                        median: quantile(&c, 0.5) - base,
                        ci_low: ci.map(|(low, _)| low),
                        ci_high: ci.map(|(_, high)| high),
                        needed,
                    }
                })
                .collect();
            ArmComparison {
                repository: repository.map(String::from),
                toggle: toggle.clone(),
                control: spread(&control),
                new: spread(&new),
                differences,
            }
        })
        .collect()
}

fn spread(group: &[&SessionMetrics]) -> ArmSpread {
    ArmSpread {
        sessions: group.len() as u64,
        metrics: Metric::ALL
            .iter()
            .filter(|_| !group.is_empty())
            .map(|&metric| {
                let values: Vec<f64> = group.iter().map(|m| m.value(metric)).collect();
                Spread {
                    metric: metric.name().to_string(),
                    median: quantile(&values, 0.5),
                    p25: quantile(&values, 0.25),
                    p75: quantile(&values, 0.75),
                }
            })
            .collect(),
    }
}

/// The `q` quantile of `values`, interpolated between the two nearest; 0
/// for none.
fn quantile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = q * (sorted.len() - 1) as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    sorted[low] + (sorted[high] - sorted[low]) * (at - low as f64)
}

/// Resamples drawn for an interval.
const RESAMPLES: usize = 2000;

/// The 2.5th and 97.5th percentiles of the difference of medians, `a`'s
/// less `b`'s, over resamples of each with replacement. A fixed seed keeps
/// the report the same from one run to the next.
fn bootstrap(a: &[f64], b: &[f64]) -> Option<(f64, f64)> {
    if a.len() < 2 || b.len() < 2 {
        return None;
    }
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |n: usize| {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_f491_4f6c_dd1d) % n as u64) as usize
    };
    let mut differences: Vec<f64> = (0..RESAMPLES)
        .map(|_| {
            let ra: Vec<f64> = (0..a.len()).map(|_| a[next(a.len())]).collect();
            let rb: Vec<f64> = (0..b.len()).map(|_| b[next(b.len())]).collect();
            quantile(&ra, 0.5) - quantile(&rb, 0.5)
        })
        .collect();
    differences.sort_by(f64::total_cmp);
    Some((quantile(&differences, 0.025), quantile(&differences, 0.975)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Call, Request, Tool};

    #[test]
    fn a_session_counts_its_requests_once_and_its_active_time_capped() {
        let request = |at: i64, id: u64, calls: std::ops::Range<usize>| Request {
            at: Some(at),
            input_tokens: Some(1000),
            id: Some(id),
            calls,
            ..Request::default()
        };
        let call = |at: i64, ms: u64| Call {
            at: Some(at),
            duration_ms: Some(ms),
            ..Call::new(Tool::Shell)
        };
        let main = Transcript {
            session: "s".into(),
            // A request, a 30 s call, a request 10 s after it ends, then one
            // an hour later: 15 minutes of that counts.
            requests: vec![
                request(0, 1, 0..1),
                request(40_000, 2, 1..1),
                request(3_640_000, 3, 1..1),
            ],
            calls: vec![call(0, 30_000)],
            ci_failures: 1,
            ..Transcript::default()
        };
        // A resumed copy repeats requests 1 and 3, with the call, and the
        // CI event: none of it counts twice.
        let resumed = Transcript {
            session: "s".into(),
            requests: vec![request(0, 1, 0..1), request(3_640_000, 3, 1..1)],
            calls: vec![call(0, 30_000)],
            ci_failures: 1,
            ..Transcript::default()
        };
        let m = SessionMetrics::of("s", &[main, resumed]);
        assert_eq!((m.requests, m.input_tokens, m.ci_failures), (3, 3000, 1));
        assert_eq!(m.active_ms, 30_000 + 10_000 + 15 * 60 * 1000);
    }

    #[test]
    fn active_time_is_the_same_whatever_order_transcripts_come_in() {
        let request = |at: i64, id: u64| Request {
            at: Some(at),
            id: Some(id),
            calls: 0..0,
            ..Request::default()
        };
        let transcript = |requests: Vec<Request>| Transcript {
            session: "s".into(),
            requests,
            ..Transcript::default()
        };
        // The full copy, and one that left out request 2 and went on with
        // a request of its own: its wait runs from request 2, not 1.
        let full = transcript(vec![request(0, 1), request(60_000, 2)]);
        let partial = transcript(vec![request(0, 1), request(90_000, 3)]);
        let a = SessionMetrics::of("s", &[full.clone(), partial.clone()]);
        let b = SessionMetrics::of("s", &[partial, full]);
        assert_eq!(a, b);
        assert_eq!((a.requests, a.active_ms), (3, 90_000));
    }

    #[test]
    fn calls_dated_before_their_request_count_as_tool_time() {
        // Codex dates a request by the token count after its calls: a
        // 20-minute run before it is tool time, not a capped wait.
        let call = Call {
            at: Some(60_000),
            duration_ms: Some(20 * 60 * 1000),
            ..Call::new(Tool::Shell)
        };
        let t = Transcript {
            session: "s".into(),
            requests: vec![
                Request {
                    at: Some(0),
                    calls: 0..0,
                    ..Request::default()
                },
                Request {
                    at: Some(60_000 + 20 * 60 * 1000),
                    calls: 0..1,
                    ..Request::default()
                },
            ],
            calls: vec![call],
            ..Transcript::default()
        };
        assert_eq!(SessionMetrics::of("s", &[t]).active_ms, 21 * 60 * 1000);
    }

    #[test]
    fn arms_are_compared_within_each_repository_and_sized_for_a_difference() {
        // The gate over two days: 7 control sessions against 28, and an
        // interval of -40 to +105 requests on a median of 35.
        assert_eq!(per_arm(72.5, 0.3 * 35.0, 7, 28), Some(534));
        assert_eq!(per_arm(72.5, 0.1 * 35.0, 7, 28), Some(4806));
        assert_eq!(per_arm(72.5, 0.0, 7, 28), None);
        assert_eq!(per_arm(0.0, 10.0, 7, 28), Some(2));
        let session = |n: u64, repository: &str, requests: u64| SessionMetrics {
            session: format!("s{n}"),
            repository: Some(repository.to_string()),
            requests,
            ..SessionMetrics::default()
        };
        // Repository a: control takes about 10 more requests than new.
        let mut metrics: Vec<SessionMetrics> = (0..8)
            .map(|n| session(n, "a", if n < 4 { 50 + n } else { 40 + 2 * n }))
            .collect();
        // Repository b has one session in each arm; and one session's
        // repository is unknown.
        metrics.push(session(8, "b", 500));
        metrics.push(session(9, "b", 100));
        metrics.push(SessionMetrics {
            repository: None,
            ..session(10, "", 7)
        });
        let arms: BTreeMap<String, BTreeMap<String, String>> = (0..11)
            .map(|n| {
                let arm = if n < 4 || n == 8 || n == 10 {
                    "control"
                } else {
                    "new"
                };
                (
                    format!("s{n}"),
                    BTreeMap::from([("gate".to_string(), arm.to_string())]),
                )
            })
            .collect();
        let [a, b, unknown] = compare_arms(&arms, &metrics).try_into().unwrap();
        assert_eq!(a.repository.as_deref(), Some("a"));
        assert_eq!((a.control.sessions, a.new.sessions), (4, 4));
        // Pooled with b, the new median would be 52 rather than 51.
        assert_eq!(a.new.metrics[0].median, 51.0);
        assert_eq!(b.repository.as_deref(), Some("b"));
        assert_eq!((b.control.sessions, b.new.sessions), (1, 1));
        assert_eq!(b.differences[0].median, 400.0);
        assert_eq!((unknown.repository, unknown.new.sessions), (None, 0));
        let requests = &a.differences[0];
        let (low, high) = (requests.ci_low.unwrap(), requests.ci_high.unwrap());
        let base = a.new.metrics[0].median;
        let percents: Vec<u64> = requests.needed.iter().map(|n| n.percent).collect();
        assert_eq!(percents, [30, 10]);
        for needed in &requests.needed {
            let target = base * needed.percent as f64 / 100.0;
            assert_eq!(
                Some(needed.per_arm),
                per_arm((high - low) / 2.0, target, 4, 4)
            );
            assert_eq!(needed.more, 2 * needed.per_arm.saturating_sub(4));
        }
        // No interval, no estimate.
        assert!(b.differences[0].needed.is_empty());
    }

    #[test]
    fn arms_show_n_spread_and_an_interval_for_the_difference() {
        let arms: BTreeMap<String, BTreeMap<String, String>> = (0..12)
            .map(|n| {
                let arm = if n < 4 { "control" } else { "new" };
                (
                    format!("s{n}"),
                    BTreeMap::from([("gate".to_string(), arm.to_string())]),
                )
            })
            .collect();
        let metrics: Vec<SessionMetrics> = (0..12)
            .map(|n| SessionMetrics {
                session: format!("s{n}"),
                // The control arm takes 10 more requests.
                requests: if n < 4 { 50 + n } else { 40 + n % 4 },
                ..SessionMetrics::default()
            })
            .collect();
        let [gate] = compare_arms(&arms, &metrics).try_into().unwrap();
        assert_eq!((gate.control.sessions, gate.new.sessions), (4, 8));
        let requests = &gate.differences[0];
        assert_eq!(requests.metric, "requests");
        assert!((requests.median - 10.0).abs() < 1e-9, "{requests:?}");
        let (low, high) = (requests.ci_low.unwrap(), requests.ci_high.unwrap());
        assert!(
            low > 5.0 && high < 15.0 && low <= 10.0 && high >= 10.0,
            "{low} {high}"
        );
        assert_eq!(gate.new.metrics[0].median, 41.5);
        assert_eq!(
            (gate.new.metrics[0].p25, gate.new.metrics[0].p75),
            (40.75, 42.25)
        );
        // One session in an arm gives no interval.
        let one = compare_arms(&arms, &metrics[3..]);
        assert_eq!(one[0].control.sessions, 1);
        assert_eq!(one[0].differences[0].ci_low, None);
        // None gives no median at all, rather than a made-up zero.
        let none = compare_arms(&arms, &metrics[4..]);
        assert_eq!(none[0].control, ArmSpread::default());
        assert!(none[0].differences.is_empty());
        assert_eq!(none[0].new.sessions, 8);
    }
}
