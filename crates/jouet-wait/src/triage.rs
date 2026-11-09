//! Which review comments need no agent: bot notices that carry no finding.
//!
//! These are the drop rules replayed against five weeks of Claude Code
//! Auto-fix events: rate-limit notices, "review in progress" or "skipped"
//! notices, review summaries with no findings, perf reports with no
//! regression, and bot acknowledgements on review threads. Only a bot's
//! comment is ever dropped; a person's always reaches the agent.

use regex::Regex;
use std::sync::LazyLock;

/// How readily a bot's reply on a review thread counts as an
/// acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acks {
    /// Any reply that opens with thanks or agreement and has no severity
    /// marker.
    Loose,
    /// As loose, but not when its opening also hedges or asks ("…but X
    /// remains", "could you add a test?").
    Strict,
}

impl Acks {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "loose" => Some(Self::Loose),
            "strict" => Some(Self::Strict),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Loose => "loose",
            Self::Strict => "strict",
        }
    }
}

/// Where a comment was posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// On a review thread.
    Thread,
    /// A review summary or a conversation comment.
    TopLevel,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid built-in pattern")
}

static RATE_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)rate limited by coderabbit|review limit reached|rate limit exceeded|usage limits? (reached|exceeded)|rate[- ]limit(ed)? (exceeded|reached)|you have reached your",
    )
});
static IN_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)review in progress by coderabbit|currently processing new changes|review (is )?in progress|is reviewing|reviewing your (changes|pr)|review skipped|reviews paused|review paused|bugbot is reviewing",
    )
});
static NO_FINDINGS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)no actionable comments were generated|found no (new )?(potential )?(issues|bugs)|no issues found|no reviewable files|does not match any of the \d+ configured review trigger|has reviewed your changes and found no|didn.t find any (major )?issues",
    )
});
static ZERO_ACTIONABLE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)actionable comments posted: 0\b"));
static ANY_ACTIONABLE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)actionable comments posted: [1-9]"));
static SIDE_FINDINGS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)outside diff|nitpick|additional comments|duplicate comments"));
static GREPTILE_FINDINGS: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)comments outside diff|alt="P[0-3]""#));
static CURSOR_FOUND: LazyLock<Regex> =
    LazyLock::new(|| re(r"has reviewed your changes and found \d+ potential"));
static PERF: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)instruction[- ]count"));
static PERF_WORSE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)regress(ed|ion)|increase[sd]? by|\+\d+(\.\d+)?%"));
static SEVERITY: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"Major\*\*|Minor\*\*|Critical\*\*|🟠|🟡|🔴|Potential issue|Medium Severity|High Severity|Low Severity|alt="P[0-3]""#,
    )
});
static THANKS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^[`＠@\w\[\]-]*`?\s*(thanks|thank you|understood|got it|confirmed|you.re right|perfect|great|acknowledged|noted|sounds good|fair point|makes sense|looks good|lgtm)",
    )
});
static HEDGE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(still|could you|can you|please|remains?|however|but|though|not yet|missing|consider)\b|\?",
    )
});
static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)<!--.*?-->"));

/// How much of an acknowledgement's opening the strict rule reads.
const OPENING: usize = 240;

fn is_bot(author: &str) -> bool {
    let a = author.to_ascii_lowercase();
    a.ends_with("[bot]")
        || [
            "coderabbit",
            "cursor",
            "greptile",
            "github-actions",
            "codex",
            "chatgpt",
            "gemini",
            "copilot",
            "sourcery",
            "socket",
            "renovate",
        ]
        .iter()
        .any(|b| a.starts_with(b))
}

/// The class of a comment that needs no agent, or `None` when it should
/// reach one.
pub(crate) fn noise(author: &str, body: &str, place: Place, acks: Acks) -> Option<&'static str> {
    if !is_bot(author) {
        return None;
    }
    let lower = author.to_ascii_lowercase();
    if RATE_LIMIT.is_match(body) {
        return Some("rate_limit");
    }
    if IN_PROGRESS.is_match(body) {
        return Some("in_progress");
    }
    if lower.starts_with("github-actions") && PERF.is_match(body) {
        let rest = body
            .replace("No instruction-count regression", "")
            .replace("regression beyond the gate", "");
        return (!PERF_WORSE.is_match(&rest)).then_some("perf_no_regression");
    }
    let zero = ZERO_ACTIONABLE.is_match(body) && !SIDE_FINDINGS.is_match(body);
    let greptile_clean = lower.starts_with("greptile")
        && body.contains("Confidence Score: 5/5")
        && !GREPTILE_FINDINGS.is_match(body);
    let walkthrough = body.contains("summarize by coderabbit")
        && body.contains("Walkthrough")
        && !ANY_ACTIONABLE.is_match(body);
    if NO_FINDINGS.is_match(body) || zero || greptile_clean || walkthrough {
        return Some("no_findings");
    }
    if body.contains("Stale Bugbot comment from a previous run") {
        return Some("stale");
    }
    // A summary that only counts findings it also posts as review threads,
    // which reach the agent on their own.
    let cursor_count = lower.starts_with("cursor") && CURSOR_FOUND.is_match(body);
    let rabbit_count = lower.starts_with("coderabbit")
        && ANY_ACTIONABLE.is_match(body)
        && !SIDE_FINDINGS.is_match(body);
    if place == Place::TopLevel && (cursor_count || rabbit_count) {
        return Some("summary");
    }
    if place == Place::Thread && !SEVERITY.is_match(body) {
        let text = HTML_COMMENT.replace_all(body, "");
        let opening = text.trim_matches(|c: char| c.is_whitespace() || c == '>' || c == '`');
        let head: String = opening.chars().take(OPENING).collect();
        let thanks =
            body.contains("auto-generated reply by CodeRabbit") || THANKS.is_match(opening);
        if thanks && !(acks == Acks::Strict && HEDGE.is_match(&head)) {
            return Some("ack");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(author: &str, body: &str, place: Place, acks: Acks) -> Option<&'static str> {
        noise(author, body, place, acks)
    }

    #[test]
    fn notices_with_no_finding_are_noise() {
        let t = Place::TopLevel;
        let l = Acks::Loose;
        assert_eq!(
            class(
                "coderabbitai[bot]",
                "> [!WARNING]\n> ## Review limit reached",
                t,
                l
            ),
            Some("rate_limit")
        );
        assert_eq!(
            class(
                "coderabbitai[bot]",
                "Currently processing new changes in this PR.",
                t,
                l
            ),
            Some("in_progress")
        );
        assert_eq!(
            class(
                "coderabbitai[bot]",
                "**Actionable comments posted: 0**",
                t,
                l
            ),
            Some("no_findings")
        );
        assert_eq!(
            class(
                "greptile-apps[bot]",
                "<h3>Confidence Score: 5/5</h3> safe to merge",
                t,
                l
            ),
            Some("no_findings")
        );
        assert_eq!(
            class(
                "github-actions[bot]",
                "No instruction-count regression above 1%. -0.3%",
                t,
                l
            ),
            Some("perf_no_regression")
        );
    }

    #[test]
    fn summaries_of_inline_findings_and_stale_notices_are_noise() {
        let t = Place::TopLevel;
        let l = Acks::Loose;
        let stale = "Stale Bugbot comment from a previous run.";
        assert_eq!(class("cursor[bot]", stale, t, l), Some("stale"));
        let found = "Cursor Bugbot has reviewed your changes and found 1 potential issue.";
        assert_eq!(class("cursor[bot]", found, t, l), Some("summary"));
        let counted = "**Actionable comments posted: 2**";
        assert_eq!(class("coderabbitai[bot]", counted, t, l), Some("summary"));
        let nitpicks =
            "**Actionable comments posted: 2**\n<summary>🧹 Nitpick comments (1)</summary>";
        assert_eq!(class("coderabbitai[bot]", nitpicks, t, l), None);
    }

    #[test]
    fn findings_and_people_reach_the_agent() {
        let t = Place::TopLevel;
        let l = Acks::Loose;
        assert_eq!(
            class(
                "coderabbitai[bot]",
                "**Actionable comments posted: 0**\n<summary>🧹 Nitpick comments (2)</summary>",
                t,
                l
            ),
            None
        );
        assert_eq!(
            class(
                "greptile-apps[bot]",
                "Confidence Score: 5/5 <img alt=\"P1\">",
                t,
                l
            ),
            None
        );
        assert_eq!(
            class(
                "github-actions[bot]",
                "instruction-count: `startup` +2.4%",
                t,
                l
            ),
            None
        );
        assert_eq!(class("jdx", "Review limit reached? thanks", t, l), None);
        assert_eq!(class("jdx", "Thanks!", Place::Thread, l), None);
    }

    #[test]
    fn acks_are_only_thread_replies_without_a_severity() {
        let ack = "Thanks for the fix! The change looks right.";
        assert_eq!(
            class("coderabbitai[bot]", ack, Place::Thread, Acks::Strict),
            Some("ack")
        );
        assert_eq!(
            class("coderabbitai[bot]", ack, Place::TopLevel, Acks::Loose),
            None
        );
        let severe = "Thanks. _⚠️ Potential issue_ | _🟠 Major_";
        assert_eq!(
            class("coderabbitai[bot]", severe, Place::Thread, Acks::Loose),
            None
        );
    }

    #[test]
    fn strict_acks_refuse_a_reply_that_hedges_or_asks() {
        for body in [
            "Thanks, but the `0 selected` case still is not handled.",
            "Thanks! Could you add a test for the exponent form?",
        ] {
            assert_eq!(
                class("coderabbitai[bot]", body, Place::Thread, Acks::Loose),
                Some("ack")
            );
            assert_eq!(
                class("coderabbitai[bot]", body, Place::Thread, Acks::Strict),
                None
            );
        }
    }
}
