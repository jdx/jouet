//! What jouet itself cost, priced the way [`super::estimate`] prices what it
//! saved: each request jouet added at the harness's average request before
//! setup, and the primer's tokens on every request after it.

use super::{
    Baseline, Estimate, Kind, MIN_BASELINE_RUNS, Measured, count_text, ratio, tokens_text,
};
use serde::Serialize;
use std::collections::BTreeSet;

/// A kind of cost jouet adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostKind {
    /// Finishing blocked after a failed check: the agent makes at least one
    /// more request.
    StopBlocks,
    /// A denied call: the agent makes at least one more request.
    Denials,
    /// A failed jouet run that was not a check's answer.
    FailedRuns,
    /// `jouet edit` failures beyond the rate edits failed at before setup.
    EditFailures,
    /// The session-start primer, read again by every request.
    Primer,
    /// Rule warnings, counted but not priced.
    Warnings,
}

impl CostKind {
    /// What was counted, for text.
    pub fn label(self) -> &'static str {
        match self {
            Self::StopBlocks => "finishing blocked after a failed check",
            Self::Denials => "calls a rule denied",
            Self::FailedRuns => "failed jouet runs",
            Self::EditFailures => "jouet edit failures beyond the rate before setup",
            Self::Primer => "requests carrying the session-start primer",
            Self::Warnings => "rule warnings",
        }
    }

    /// Whether the net needs this cost priced. An edit cost is refused
    /// exactly when the edit saving is, and a warning is never priced.
    fn needed(self) -> bool {
        !matches!(self, Self::EditFailures | Self::Warnings)
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

/// Prices what jouet added: the requests the ledger shows it caused (stop
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
                .filter(|c| Kind::of(&c.command) == Some(Kind::Edit))
                .map(|c| c.harness.as_str()),
        )
        .collect();
    let mut out = Vec::new();
    for harness in harnesses {
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
                },
                Err(reason) => Cost {
                    harness: harness.to_string(),
                    kind,
                    count: rounded,
                    input_tokens: None,
                    basis: String::new(),
                    refused: Some(reason.clone()),
                },
            }
        };
        if let Some(o) = measured.overhead.iter().find(|o| o.harness == harness) {
            if o.stop_blocks > 0 {
                out.push(priced(
                    CostKind::StopBlocks,
                    o.stop_blocks as f64,
                    "each is at least one more request",
                ));
            }
            if o.denials > 0 {
                out.push(priced(
                    CostKind::Denials,
                    o.denials as f64,
                    "each is at least one more request, for the call again or another",
                ));
            }
            if o.failed_runs > 0 {
                out.push(priced(
                    CostKind::FailedRuns,
                    o.failed_runs as f64,
                    "each is at least one more request; a check's answer, such as a failing test, is not counted",
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
                });
            }
        }
        if let Some(edit) = edit_cost(measured, baseline, harness, &priced) {
            out.push(edit);
        }
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
            }),
            None => out.push(Cost {
                    harness: harness.to_string(),
                    kind: CostKind::Primer,
                    count: 0,
                    input_tokens: None,
                    basis: String::new(),
                    refused: Some(format!("no {harness} history after setup was read")),
            }),
        }
    }
    out
}

/// `jouet edit` failures beyond the rate edits failed at before setup, or
/// `None` when it failed less often (the saving side credits that).
fn edit_cost(
    measured: &Measured,
    baseline: Option<&Baseline>,
    harness: &str,
    priced: &dyn Fn(CostKind, f64, &str) -> Cost,
) -> Option<Cost> {
    let (runs, failed) = measured
        .commands
        .iter()
        .filter(|c| c.harness == harness && Kind::of(&c.command) == Some(Kind::Edit))
        .fold((0, 0), |(r, f), c| (r + c.runs, f + c.failed));
    if failed == 0 {
        return None;
    }
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
                count_text(runs)
            )),
        });
    };
    let rate = k.failure_rate();
    let extra = failed as f64 - runs as f64 * rate;
    (extra > 0.0).then(|| {
        priced(
            CostKind::EditFailures,
            extra,
            &format!(
                "jouet edit failed {:.1}% ({failed} of {}), edits before setup {:.1}%; each extra failure is one more request",
                ratio(failed, runs) * 100.0,
                count_text(runs),
                rate * 100.0
            ),
        )
    })
}

/// What the estimates saved less what the costs cost, in input tokens, or
/// `None` when a cost the net needs could not be priced. A saving that was
/// refused counts as nothing.
pub fn net(estimates: &[Estimate], costs: &[Cost]) -> Option<i64> {
    if costs
        .iter()
        .any(|c| c.kind.needed() && c.input_tokens.is_none())
    {
        return None;
    }
    let saved: u64 = estimates.iter().filter_map(|e| e.input_tokens).sum();
    let cost: u64 = costs.iter().filter_map(|c| c.input_tokens).sum();
    Some(i64::try_from(saved).unwrap_or(i64::MAX) - i64::try_from(cost).unwrap_or(i64::MAX))
}
