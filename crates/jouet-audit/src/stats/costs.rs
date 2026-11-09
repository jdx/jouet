//! What jouet itself cost, priced the way [`super::estimate`] prices what it
//! saved: each request jouet added at the harness's average request before
//! setup, and the primer's tokens on every request after it.

use super::{
    BYTES_PER_TOKEN, Baseline, Estimate, Kind, MIN_BASELINE_RUNS, Measured, count_text, ratio,
    tokens_text,
};
use serde::Serialize;
use std::collections::BTreeSet;

/// A kind of cost jouet adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostKind {
    /// Finishing blocked after a failed check, and finished with no check
    /// passing after it: priced assuming one more request for it.
    StopBlocks,
    /// Finishing blocked until a check passed: counted, not priced, since a
    /// correct change needs that check to pass anyway.
    HeldBlocks,
    /// A denied call: priced assuming one more request.
    Denials,
    /// A failed jouet run that was not a check's answer.
    FailedRuns,
    /// `jouet edit` failures beyond the rate edits failed at before setup.
    EditFailures,
    /// Extra output from `jouet test`, against the baseline average.
    TestOutput,
    /// The session-start primer, read again by every request.
    Primer,
    /// Rule warnings, counted but not priced.
    Warnings,
}

impl CostKind {
    /// What was counted, for text.
    pub fn label(self) -> &'static str {
        match self {
            Self::StopBlocks => "finishing blocked, then finished with no check passing",
            Self::HeldBlocks => "finishing held until a check passed",
            Self::Denials => "calls a rule denied",
            Self::FailedRuns => "failed jouet runs",
            Self::EditFailures => "jouet edit failures beyond the rate before setup",
            Self::TestOutput => "extra jouet test output",
            Self::Primer => "requests carrying the session-start primer",
            Self::Warnings => "rule warnings",
        }
    }

    /// Whether the net needs this cost priced. An edit cost is refused
    /// exactly when the edit saving is, and a warning is never priced.
    fn needed(self) -> bool {
        !matches!(self, Self::EditFailures | Self::Warnings | Self::HeldBlocks)
    }
}

/// An estimate of one kind of cost in one harness.
#[derive(Debug, Clone, Serialize)]
pub struct Cost {
    pub harness: String,
    pub kind: CostKind,
    /// What was counted: blocks, denials, runs, failures or requests.
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// What the estimate rests on.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub basis: String,
    /// Why there is no estimate, when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// Its harness has no history before setup, so neither its costs nor its
    /// savings can be priced, and the net leaves it out.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub outside_net: bool,
}

/// One harness's history after setup, as far as the costs need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AfterSetup {
    pub harness: String,
    /// Model requests, subagents' included.
    pub requests: u64,
    /// About how many tokens its primer is; 0 when the primer is off.
    pub primer_tokens: u64,
}

/// Estimates what jouet added: one assumed request per event (stop
/// blocks, denials, failed runs, and `jouet edit` failures beyond the old
/// rate) at each harness's average request before setup, and the primer
/// on every request in `after`. A harness missing from `after` has its
/// primer refused; one whose primer is off has no primer line.
pub fn costs(measured: &Measured, baselines: &[Baseline], after: &[AfterSetup]) -> Vec<Cost> {
    let harnesses: BTreeSet<&str> = measured
        .overhead
        .iter()
        .map(|o| o.harness.as_str())
        .chain(after.iter().map(|a| a.harness.as_str()))
        .chain(
            measured
                .commands
                .iter()
                .filter(|c| matches!(Kind::of(&c.command), Some(Kind::Edit | Kind::Test)))
                .map(|c| c.harness.as_str()),
        )
        .collect();
    let mut out = Vec::new();
    for harness in harnesses {
        let start = out.len();
        let baseline = baselines.iter().find(|b| b.harness == harness);
        let price = match baseline {
            None => Err(format!("no {harness} history from before setup")),
            Some(b) if b.requests_with_tokens == 0 => {
                Err(format!("the {harness} history records no token counts"))
            }
            // The same floor as the savings, so neither side rests on less.
            Some(b) if b.requests_with_tokens < MIN_BASELINE_RUNS => Err(format!(
                "only {} requests with token counts before setup, and a price needs {MIN_BASELINE_RUNS}",
                b.requests_with_tokens
            )),
            // Saved before baselines kept their input tokens.
            Some(b) if b.input_tokens == 0 => Err(format!(
                "the saved {harness} baseline has no average request; run `jouet stats --rescan`"
            )),
            Some(b) => Ok(b.tokens_per_request()),
        };
        let priced = |kind: CostKind, count: f64, basis: &str| -> Cost {
            let rounded = count.round() as u64;
            match &price {
                Ok(p) => Cost {
                    harness: harness.to_string(),
                    kind,
                    count: rounded,
                    input_tokens: Some((count * p).round() as u64),
                    basis: format!(
                        "{basis}, at {} tokens per request (the average before setup)",
                        tokens_text(*p)
                    ),
                    refused: None,
                    outside_net: false,
                },
                Err(reason) => Cost {
                    harness: harness.to_string(),
                    kind,
                    count: rounded,
                    input_tokens: None,
                    basis: String::new(),
                    refused: Some(reason.clone()),
                    outside_net: false,
                },
            }
        };
        if let Some(o) = measured.overhead.iter().find(|o| o.harness == harness) {
            let wasted = o.stop_blocks.saturating_sub(o.stop_blocks_resolved);
            if wasted > 0 {
                out.push(priced(
                    CostKind::StopBlocks,
                    wasted as f64,
                    "assuming one more request per block; no test, lint or build passed after it before the session next stopped",
                ));
            }
            if o.stop_blocks_resolved > 0 {
                out.push(Cost {
                    harness: harness.to_string(),
                    kind: CostKind::HeldBlocks,
                    count: o.stop_blocks_resolved,
                    input_tokens: None,
                    basis: String::new(),
                    refused: Some("a check passed after each before the session next stopped; a correct change needs that anyway, so not priced".into()),
                    outside_net: false,
                });
            }
            if o.denials > 0 {
                out.push(priced(
                    CostKind::Denials,
                    o.denials as f64,
                    "assuming one more request per denial, for the call again or another",
                ));
            }
            if o.failed_runs > 0 {
                out.push(priced(
                    CostKind::FailedRuns,
                    o.failed_runs as f64,
                    "jouet commands that failed by their own exit code (before jouet recorded it, calls that ran only jouet and failed), assuming one more request per failure; a check's answer, such as a failing test, is not counted",
                ));
            }
            if o.warnings > 0 {
                out.push(Cost {
                    harness: harness.to_string(),
                    kind: CostKind::Warnings,
                    count: o.warnings,
                    input_tokens: None,
                    basis: String::new(),
                    refused: Some("a warning adds no request, but its text stays in the context that later requests read; not priced".into()),
                    outside_net: false,
                });
            }
        }
        if let Some(edit) = edit_cost(measured, baseline, harness, &priced) {
            out.push(edit);
        }
        out.extend(test_output_cost(measured, baseline, harness));
        match after.iter().find(|a| a.harness == harness) {
            Some(a) if a.primer_tokens == 0 => {}
            Some(a) => out.push(Cost {
                    harness: harness.to_string(),
                    kind: CostKind::Primer,
                    count: a.requests,
                    input_tokens: Some(a.requests.saturating_mul(a.primer_tokens)),
                    basis: format!(
                        "{} requests in the history after setup, subagents' included, each reading the primer's ~{} tokens",
                        count_text(a.requests),
                        count_text(a.primer_tokens)
                    ),
                    refused: None,
                    outside_net: false,
            }),
            None => out.push(Cost {
                    harness: harness.to_string(),
                    kind: CostKind::Primer,
                    count: 0,
                    input_tokens: None,
                    basis: String::new(),
                    refused: Some(format!("no {harness} history after setup was read")),
                    outside_net: false,
            }),
        }
        // Nothing of a harness without a history before setup can be
        // priced, savings included, so the net is of the others.
        if baseline.is_none() {
            for c in &mut out[start..] {
                c.outside_net = true;
            }
        }
    }
    out
}

/// Extra test output compared with the pre-setup average, counted once.
fn test_output_cost(measured: &Measured, baseline: Option<&Baseline>, harness: &str) -> Vec<Cost> {
    let (runs, calls, bytes) = measured
        .commands
        .iter()
        .filter(|command| {
            command.harness == harness && Kind::of(&command.command) == Some(Kind::Test)
        })
        .fold((0, 0, 0), |(runs, calls, bytes), command| {
            (
                runs + command.runs,
                calls + command.output_calls,
                bytes + command.output_bytes,
            )
        });
    if runs == 0 {
        return Vec::new();
    }
    let refuse = |count, reason: String| Cost {
        harness: harness.to_string(),
        kind: CostKind::TestOutput,
        count,
        input_tokens: None,
        basis: String::new(),
        refused: Some(reason),
        outside_net: false,
    };
    let Some(before) = baseline.and_then(|b| b.kinds.iter().find(|kind| kind.kind == Kind::Test))
    else {
        return vec![refuse(runs, "no test output baseline before setup".into())];
    };
    if before.runs < MIN_BASELINE_RUNS || before.calls == 0 || before.output_bytes == 0 {
        return vec![refuse(
            runs,
            "too little pre-setup test output evidence; rescan or collect a baseline".into(),
        )];
    }
    let mut result = Vec::new();
    if calls < runs {
        result.push(refuse(
            runs - calls,
            "test output size was not recorded for these runs".into(),
        ));
    }
    if calls == 0 {
        return result;
    }
    let old = ratio(before.output_bytes, before.calls);
    let new = ratio(bytes, calls);
    let extra = (new - old).max(0.0);
    if extra > 0.0 {
        result.push(Cost {
        harness: harness.to_string(),
        kind: CostKind::TestOutput,
        count: calls,
        input_tokens: Some((calls as f64 * extra / BYTES_PER_TOKEN).round() as u64),
        basis: format!("{} calls before setup gave the agent {} bytes of output on average; jouet test gave {} over the {} calls that recorded it, at {BYTES_PER_TOKEN} bytes a token, counted once", count_text(before.calls), count_text(old.round() as u64), count_text(new.round() as u64), count_text(calls)),
        refused: None,
        outside_net: false,
        });
    }
    result
}

/// `jouet edit` failures beyond the rate edits failed at before setup, or
/// `None` when it failed less often (the saving side credits that).
fn edit_cost(
    measured: &Measured,
    baseline: Option<&Baseline>,
    harness: &str,
    priced: &dyn Fn(CostKind, f64, &str) -> Cost,
) -> Option<Cost> {
    // Its own failure rate, from the runs that recorded their exit code,
    // applied to every run.
    let (runs, own_runs, failed) = measured
        .commands
        .iter()
        .filter(|c| c.harness == harness && Kind::of(&c.command) == Some(Kind::Edit))
        .fold((0, 0, 0), |(r, o, f), c| {
            (r + c.runs.max(c.own_runs), o + c.own_runs, f + c.own_failed)
        });
    if failed == 0 {
        return None;
    }
    let own = ratio(failed, own_runs);
    let before = baseline.and_then(|b| b.kinds.iter().find(|k| k.kind == Kind::Edit));
    let Some(k) = before.filter(|k| k.runs >= MIN_BASELINE_RUNS) else {
        return Some(Cost {
            harness: harness.to_string(),
            kind: CostKind::EditFailures,
            count: failed,
            input_tokens: None,
            basis: String::new(),
            refused: Some(format!(
                "too few edits before setup to compare its {failed} failures in {} runs with",
                count_text(own_runs)
            )),
            outside_net: false,
        });
    };
    let rate = k.failure_rate();
    let extra = runs as f64 * (own - rate);
    (extra > 0.0).then(|| {
        priced(
            CostKind::EditFailures,
            extra,
            &format!(
                "jouet edit failed {:.1}% of the {} runs that recorded their own exit code ({failed}), edits before setup {:.1}%, over its {} runs; assuming one more request per extra failure",
                own * 100.0,
                count_text(own_runs),
                rate * 100.0,
                count_text(runs)
            ),
        )
    })
}

/// What the estimates saved less what the costs cost, in input tokens, or
/// `None` when a cost the net needs could not be priced, or CI wait savings
/// have no linked evidence. Other refused savings count as nothing. A harness
/// with no history before setup is left out (see [`Cost::outside_net`]).
pub fn net(estimates: &[Estimate], costs: &[Cost]) -> Option<i64> {
    if estimates
        .iter()
        .any(|e| !e.outside_net && e.kind == Kind::CiWait && e.input_tokens.is_none())
        || costs
            .iter()
            .any(|c| !c.outside_net && c.kind.needed() && c.input_tokens.is_none())
    {
        return None;
    }
    let saved: u64 = estimates.iter().filter_map(|e| e.input_tokens).sum();
    let cost: u64 = costs.iter().filter_map(|c| c.input_tokens).sum();
    Some(i64::try_from(saved).unwrap_or(i64::MAX) - i64::try_from(cost).unwrap_or(i64::MAX))
}
