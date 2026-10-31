//! The "Entire Gates" check: one check run that passes only when every
//! blocking gate on the PR's trail does. Its summary lists each gate on a
//! line of its own, readable with the forge's token alone:
//!
//! ```text
//! - ❌ **Checks**: failed — CI checks have not passed
//! - ❌ **Findings**: failed — Unresolved findings: 2 medium severity
//! - ✅ **Up to date**: passed — 2 commits behind · Mergeable
//!
//! [Trail #424 on Entire](https://entire.io/gh/jdx/mise/trails/424)
//! ```
//!
//! Two of the gates only repeat what is already known elsewhere: `checks`
//! fails because another check failed, and `up_to_date` because the branch
//! is behind its base or conflicts with it. Only `findings` (and any gate
//! jouet does not know) is a failure of its own.

use serde::{Deserialize, Serialize};

/// The check run's name.
pub const CHECK: &str = "Entire Gates";

/// Gates whose failure repeats something already reported elsewhere.
const DERIVED: [&str; 2] = ["checks", "up_to_date"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    /// `checks`, `findings`, `up_to_date`, `approvals`, or another gate's
    /// label in snake_case.
    pub gate: String,
    /// As Entire words it: `passed`, `failed`, `pending`, `skipped`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

impl Gate {
    pub fn failed(&self) -> bool {
        self.status == "failed"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gates {
    /// The check's own state: `passed`, `failed`, `pending` or `neutral`
    /// (the gates do not apply, as when no trail tracks the branch).
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub gates: Vec<Gate>,
    /// The trail number the summary links to. Trail numbers are not PR
    /// numbers.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub trail: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
}

impl Gates {
    /// Reads the check from its status and conclusion (in either REST's
    /// lowercase or GraphQL's uppercase), title, summary and details URL.
    pub fn parse(
        status: &str,
        conclusion: Option<&str>,
        title: Option<&str>,
        summary: Option<&str>,
        url: Option<&str>,
    ) -> Self {
        let state = if !status.eq_ignore_ascii_case("completed") {
            "pending"
        } else {
            match conclusion.map(str::to_ascii_lowercase).as_deref() {
                Some("success") => "passed",
                Some("neutral" | "skipped" | "stale") => "neutral",
                _ => "failed",
            }
        };
        let summary = summary.unwrap_or_default();
        Self {
            state: state.into(),
            title: title.filter(|t| !t.is_empty()).map(str::to_string),
            gates: summary.lines().filter_map(gate_line).collect(),
            trail: trail_number(summary),
            url: url.filter(|u| !u.is_empty()).map(str::to_string),
        }
    }

    pub fn failed(&self) -> bool {
        self.state == "failed"
    }

    pub fn failing(&self) -> impl Iterator<Item = &Gate> {
        self.gates.iter().filter(|g| g.failed())
    }

    /// Whether the gate named `gate` failed.
    pub fn fails(&self, gate: &str) -> bool {
        self.failing().any(|g| g.gate == gate)
    }

    /// Whether the check failed only for reasons reported elsewhere: other
    /// checks failing, or the branch behind or conflicting. A failed check
    /// whose summary names no failing gate is not derived: there is nothing
    /// to point at instead.
    pub fn derived(&self) -> bool {
        self.failed()
            && self.failing().next().is_some()
            && self.failing().all(|g| DERIVED.contains(&g.gate.as_str()))
    }

    /// The failing gates, as `findings: Unresolved findings: 2 medium
    /// severity; checks: CI checks have not passed`, or the check's title
    /// when its summary names none.
    pub fn failing_text(&self) -> String {
        if self.failing().next().is_none() {
            return self.title.clone().unwrap_or_else(|| self.state.clone());
        }
        self.failing()
            .map(|g| match &g.reason {
                Some(r) => format!("{}: {r}", label(&g.gate)),
                None => format!("{} failed", label(&g.gate)),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// A gate's name as people read it.
pub fn label(gate: &str) -> String {
    match gate {
        "up_to_date" => "up to date".into(),
        other => other.replace('_', " "),
    }
}

/// `- ❌ **Checks**: failed — CI checks have not passed`.
fn gate_line(line: &str) -> Option<Gate> {
    let line = line.trim();
    let rest = line.strip_prefix("- ")?;
    let (_, rest) = rest.split_once("**")?;
    let (name, rest) = rest.split_once("**")?;
    let rest = rest.strip_prefix(':')?.trim();
    let (status, reason) = match rest.split_once('—') {
        Some((status, reason)) => (status.trim(), Some(reason.trim())),
        None => (rest, None),
    };
    if name.is_empty() || status.is_empty() {
        return None;
    }
    Some(Gate {
        gate: key(name),
        status: status.to_ascii_lowercase(),
        reason: reason.filter(|r| !r.is_empty()).map(str::to_string),
    })
}

/// `Up to date` → `up_to_date`.
fn key(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// The number in `[Trail #424 on Entire](...)`.
fn trail_number(summary: &str) -> Option<u64> {
    let (_, rest) = summary.split_once("Trail #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded from jdx/mise#13958, jdx/mise#15220 and jdx/jouet#104.
    const CHECKS_AND_FINDINGS: &str = "- ❌ **Checks**: failed — CI checks have not passed\n- ❌ **Findings**: failed — Unresolved findings: 2 medium severity\n\n[Trail #424 on Entire](https://entire.io/gh/jdx/mise/trails/424)";
    const CHECKS: &str = "- ❌ **Checks**: failed — CI checks have not passed\n- ✅ **Findings**: passed — Agent review found no blocking issues\n\n[Trail #1176 on Entire](https://entire.com/gh/jdx/-/trails/3261/changes/mise/1176)";
    const UP_TO_DATE: &str = "- ✅ **Checks**: passed — CI checks have passed\n- ✅ **Findings**: passed — No blocking findings\n- ❌ **Up to date**: failed — 3 commits behind · Merge conflicts\n\n[Trail #88 on Entire](https://entire.com/gh/jdx/-/trails/3284/changes/jouet/88)";

    #[test]
    fn the_summary_names_each_gate_and_the_trail() {
        let g = Gates::parse(
            "completed",
            Some("failure"),
            Some("2 blocking gates not passing"),
            Some(CHECKS_AND_FINDINGS),
            Some("https://entire.io/gh/jdx/mise/trails/424"),
        );
        assert_eq!(g.state, "failed");
        assert_eq!(g.trail, Some(424));
        assert_eq!(
            g.gates[1],
            Gate {
                gate: "findings".into(),
                status: "failed".into(),
                reason: Some("Unresolved findings: 2 medium severity".into()),
            }
        );
        assert!(!g.derived(), "findings are a failure of their own");
        assert_eq!(
            g.failing_text(),
            "checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity"
        );
    }

    #[test]
    fn failed_checks_and_a_stale_branch_are_derived() {
        let checks = Gates::parse("COMPLETED", Some("FAILURE"), None, Some(CHECKS), None);
        assert_eq!(checks.trail, Some(1176));
        assert!(checks.derived());
        let behind = Gates::parse("completed", Some("failure"), None, Some(UP_TO_DATE), None);
        assert!(behind.fails("up_to_date"));
        assert!(behind.derived());
        assert_eq!(
            behind.failing_text(),
            "up to date: 3 commits behind · Merge conflicts"
        );
    }

    #[test]
    fn a_failure_with_no_gates_named_is_not_derived() {
        let g = Gates::parse("completed", Some("failure"), None, None, None);
        assert!(g.failed());
        assert!(!g.derived());
        let evaluating = Gates::parse(
            "in_progress",
            None,
            Some("Gates are evaluating"),
            Some("- ⏳ **Checks**: pending — CI checks are still running"),
            None,
        );
        assert_eq!(evaluating.state, "pending");
        assert_eq!(evaluating.gates[0].status, "pending");
        let none = Gates::parse(
            "completed",
            Some("neutral"),
            Some("No trail tracks this branch"),
            Some(
                "Entire gates do not apply to this pull request: no open trail tracks its head branch.",
            ),
            None,
        );
        assert_eq!((none.state.as_str(), none.gates.len()), ("neutral", 0));
    }
}
