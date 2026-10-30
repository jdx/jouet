//! `jouet pr next`: the one thing a PR needs next, after Kent C. Dodds'
//! `ship-pr.mjs`.
//!
//! The order is fixed and the first state that applies wins: merged, closed,
//! draft, conflict, mergeability still unknown, behind its base, failing
//! checks, pending checks, changes requested, unresolved threads, waiting on
//! approval, blocked by another branch protection rule, auto-merge, and
//! finally ready to merge. The states come from what `pr status` fetched,
//! and its attention codes and messages, so the two commands never disagree.

use super::status::{PrStatus, State};
use serde::Serialize;

/// Whose move it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Nothing is left for the agent to do: merged, closed, or ready for a
    /// person to land.
    Done,
    /// The agent must do something.
    Act,
    /// Waiting on someone or something else: CI, a reviewer, GitHub.
    Wait,
}

/// The next action, as a fixed code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Merged,
    Closed,
    Draft,
    Conflict,
    MergeableUnknown,
    BehindBase,
    ChecksFailing,
    ChecksPending,
    ChangesRequested,
    UnresolvedThreads,
    AwaitingApproval,
    Blocked,
    AutoMerge,
    ReadyToMerge,
}

impl Action {
    pub fn kind(self) -> Kind {
        match self {
            Self::Merged | Self::Closed | Self::ReadyToMerge => Kind::Done,
            Self::Draft
            | Self::Conflict
            | Self::BehindBase
            | Self::ChecksFailing
            | Self::ChangesRequested
            | Self::UnresolvedThreads => Kind::Act,
            Self::MergeableUnknown
            | Self::ChecksPending
            | Self::AwaitingApproval
            | Self::Blocked
            | Self::AutoMerge => Kind::Wait,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Draft => "draft",
            Self::Conflict => "conflict",
            Self::MergeableUnknown => "mergeable_unknown",
            Self::BehindBase => "behind_base",
            Self::ChecksFailing => "checks_failing",
            Self::ChecksPending => "checks_pending",
            Self::ChangesRequested => "changes_requested",
            Self::UnresolvedThreads => "unresolved_threads",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Blocked => "blocked",
            Self::AutoMerge => "auto_merge",
            Self::ReadyToMerge => "ready_to_merge",
        }
    }
}

/// What a PR needs next, why, and the command that does it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Next {
    pub action: Action,
    pub kind: Kind,
    /// One line on why this is the next action.
    pub reason: String,
    /// The command that addresses it; absent when there is nothing to run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// GitHub's merge states that let a PR merge. `unstable` means a check that
/// branch protection does not require is failing, and `has_hooks` that the
/// server has pre-receive hooks; neither stops a merge.
const MERGEABLE_STATES: [&str; 3] = ["clean", "unstable", "has_hooks"];

/// Works through the states in order and returns the first that applies.
/// `pr` must have been finished by `Fetched::finish`, which fills in its
/// attention codes.
pub fn next(pr: &PrStatus) -> Next {
    let name = format!("{}#{}", pr.repo, pr.number);
    let attention = |code: &str| {
        pr.attention
            .iter()
            .find(|a| a.code == code)
            .map(|a| a.message.clone())
    };
    let step = |action: Action, reason: String, command: Option<String>| Next {
        action,
        kind: action.kind(),
        reason,
        command,
    };
    let update = format!("git fetch origin {0} && git merge origin/{0}", pr.base);
    match pr.state {
        State::Merged => return step(Action::Merged, "Merged".into(), None),
        State::Closed => {
            return step(
                Action::Closed,
                "Closed without merging; reopen it to continue".into(),
                None,
            );
        }
        State::Open | State::Draft => {}
    }
    if let Some(reason) = attention("draft") {
        return step(
            Action::Draft,
            reason,
            Some(format!("gh pr ready {} --repo {}", pr.number, pr.repo)),
        );
    }
    if let Some(reason) = attention("conflict") {
        return step(Action::Conflict, reason, Some(update));
    }
    if let Some(reason) = attention("mergeable_unknown") {
        return step(
            Action::MergeableUnknown,
            reason,
            Some(format!("jouet wait pr {name} --until mergeable")),
        );
    }
    if let Some(reason) = attention("behind_base") {
        return step(Action::BehindBase, reason, Some(update));
    }
    if let Some(reason) = attention("checks_failing") {
        return step(
            Action::ChecksFailing,
            reason,
            Some(format!("jouet wait ci {name}")),
        );
    }
    let checks = &pr.checks;
    // No checks yet, but not for long enough that `pr status` calls them
    // missing: they are still being registered.
    let registering = checks.total() == 0 && attention("checks_missing").is_none();
    if checks.pending > 0 || registering {
        let reason = if checks.pending > 0 {
            format!("{} of {} checks pending", checks.pending, checks.total())
        } else {
            "Waiting for checks to register on the new head".into()
        };
        return step(
            Action::ChecksPending,
            reason,
            Some(format!("jouet wait ci {name}")),
        );
    }
    if let Some(reason) = attention("changes_requested") {
        return step(
            Action::ChangesRequested,
            reason,
            Some(format!("jouet pr threads {name} --include-reviews")),
        );
    }
    if let Some(reason) = attention("unresolved_threads") {
        return step(
            Action::UnresolvedThreads,
            reason,
            Some(format!("jouet pr threads {name}")),
        );
    }
    if pr.review.as_deref() == Some("review_required") {
        return step(
            Action::AwaitingApproval,
            "Waiting for an approving review".into(),
            Some(format!("jouet wait pr {name} --until approved")),
        );
    }
    if pr.merge_state == "unknown" {
        return step(
            Action::MergeableUnknown,
            "GitHub has not finished computing the merge state; run again shortly".into(),
            Some(format!("jouet wait pr {name} --until mergeable")),
        );
    }
    if !MERGEABLE_STATES.contains(&pr.merge_state.as_str()) {
        return step(
            Action::Blocked,
            format!(
                "GitHub reports the merge as {} by branch protection or a ruleset",
                pr.merge_state
            ),
            Some(format!("jouet wait pr {name} --until any-change")),
        );
    }
    if pr.auto_merge {
        return step(
            Action::AutoMerge,
            "Auto-merge is enabled; GitHub will merge it".into(),
            Some(format!("jouet wait pr {name} --until merged")),
        );
    }
    step(
        Action::ReadyToMerge,
        ready_reason(pr),
        Some(format!("jouet pr land {name}")),
    )
}

/// `Checks pass (46/46), approved, no unresolved threads`.
fn ready_reason(pr: &PrStatus) -> String {
    let checks = &pr.checks;
    let checks = if checks.total() == 0 {
        "No checks ran".to_string()
    } else {
        format!("Checks pass ({}/{})", checks.pass, checks.total())
    };
    let review = if pr.review.as_deref() == Some("approved") {
        "approved"
    } else {
        "no review required"
    };
    format!("{checks}, {review}, no unresolved threads")
}

#[cfg(test)]
#[path = "next_tests.rs"]
mod tests;
