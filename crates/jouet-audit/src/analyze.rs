//! Accumulating transcripts into a report.
//!
//! Each request's input tokens are attributed to what it did: split evenly
//! between the calls it issued, by their category, or to
//! [`Category::Reply`] when it issued none. A request re-reads the whole
//! context, so a request avoided saves its whole input, which is why savings
//! are counted in requests that would not have been made.

use crate::category::{Category, categorize};
use crate::event::{Tool, Transcript};
use crate::pattern::{Pattern, detect_call};
use crate::redact::{example, redact};
use crate::shape::shape;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// The longest example shown, in characters.
const EXAMPLE_CHARS: usize = 240;

/// Accumulates transcripts, per harness.
#[derive(Debug, Clone, Default)]
pub struct Analyzer {
    harnesses: HashMap<String, Acc>,
    examples: bool,
    home: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Acc {
    sessions: HashSet<String>,
    transcripts: u64,
    requests: u64,
    requests_with_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    calls: u64,
    commands: u64,
    command_ms: u64,
    tool_ms: u64,
    output_bytes: u64,
    failed: u64,
    categories: [CatAcc; Category::ALL.len()],
    patterns: [PatAcc; Pattern::ALL.len()],
}

#[derive(Debug, Clone, Default)]
struct CatAcc {
    requests: f64,
    tokens: f64,
    calls: u64,
    duration_ms: u64,
    output_bytes: u64,
    failed: u64,
    /// Tokens of requests that did the same kind of thing as the request
    /// before them.
    repeat_tokens: f64,
    /// Tokens of requests whose call in this category failed.
    failed_tokens: f64,
    shapes: HashMap<String, ShapeAcc>,
}

#[derive(Debug, Clone, Default)]
struct ShapeAcc {
    calls: u64,
    tokens: f64,
    duration_ms: u64,
    failures: u64,
    /// The latest call: when, and how it ended (failed, exit code).
    last: Option<Last>,
    example: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Last {
    at: Option<i64>,
    failed: bool,
    exit_code: Option<i32>,
}

impl ShapeAcc {
    fn count(&mut self, call: &crate::event::Call, tokens: f64, duration: u64) {
        self.calls += 1;
        self.tokens += tokens;
        self.duration_ms += duration;
        self.failures += u64::from(call.failed);
        let last = Last {
            at: call.at,
            failed: call.failed,
            exit_code: call.exit_code,
        };
        // Calls arrive in order, so a call without a time is the latest.
        if self
            .last
            .is_none_or(|l| last.at.is_none() || last.at >= l.at)
        {
            self.last = Some(last);
        }
    }

    fn merge(&mut self, theirs: Self) {
        self.calls += theirs.calls;
        self.tokens += theirs.tokens;
        self.duration_ms += theirs.duration_ms;
        self.failures += theirs.failures;
        if let Some(t) = theirs.last
            && self.last.is_none_or(|o| t.at > o.at)
        {
            self.last = Some(t);
        }
        merge_example(&mut self.example, theirs.example);
    }
}

#[derive(Debug, Clone, Default)]
struct PatAcc {
    calls: u64,
    sessions: HashSet<String>,
    tokens: f64,
    duration_ms: u64,
    example: Option<String>,
    shapes: HashMap<String, ShapeAcc>,
}

impl Analyzer {
    /// `examples` keeps one redacted example command per shape and pattern;
    /// `home` is written as `~` in them.
    pub fn new(examples: bool, home: Option<String>) -> Self {
        Self {
            harnesses: HashMap::new(),
            examples,
            home,
        }
    }

    /// An analyzer with the same settings and nothing counted.
    pub(crate) fn empty_like(&self) -> Self {
        Self::new(self.examples, self.home.clone())
    }

    /// Adds a transcript. Calls count only through the requests that issued
    /// them, so dropping requests (for `--since`) drops their calls too.
    pub fn add(&mut self, harness: &str, transcript: &Transcript) {
        if transcript.requests.is_empty() {
            return;
        }
        let keep_examples = self.examples;
        let home = self.home.clone();
        let acc = self.harnesses.entry(harness.to_string()).or_default();
        if !acc.sessions.contains(&transcript.session) {
            acc.sessions.insert(transcript.session.clone());
        }
        acc.transcripts += 1;
        let mut previous = 0u32;
        // The shape of the last command, when it failed.
        let mut last_failure: Option<String> = None;
        for request in &transcript.requests {
            acc.requests += 1;
            let tokens = request.input_tokens.unwrap_or(0);
            if let Some(input) = request.input_tokens {
                acc.requests_with_tokens += 1;
                acc.input_tokens += input;
            }
            acc.output_tokens += request.output_tokens.unwrap_or(0);
            let calls = transcript
                .calls
                .get(request.calls.clone())
                .unwrap_or_default();
            if calls.is_empty() {
                let reply = &mut acc.categories[Category::Reply.index()];
                reply.requests += 1.0;
                reply.tokens += tokens as f64;
                if previous & bit(Category::Reply) != 0 {
                    reply.repeat_tokens += tokens as f64;
                }
                previous = bit(Category::Reply);
                continue;
            }
            let share = 1.0 / calls.len() as f64;
            let tokens = tokens as f64 * share;
            let mut mask = 0;
            for call in calls {
                let category = categorize(call);
                mask |= bit(category);
                let duration = call.duration_ms.unwrap_or(0);
                acc.calls += 1;
                acc.tool_ms += duration;
                acc.output_bytes += call.output_bytes;
                acc.failed += u64::from(call.failed);
                if call.tool == Tool::Shell {
                    acc.commands += 1;
                    acc.command_ms += duration;
                }
                let c = &mut acc.categories[category.index()];
                c.requests += share;
                c.tokens += tokens;
                c.calls += 1;
                c.duration_ms += duration;
                c.output_bytes += call.output_bytes;
                if call.failed {
                    c.failed += 1;
                    c.failed_tokens += tokens;
                }
                if previous & bit(category) != 0 {
                    c.repeat_tokens += tokens;
                }
                let call_shape = call.command.as_deref().map(shape);
                if let (Some(command), Some(call_shape)) = (&call.command, &call_shape) {
                    let s = c.shapes.entry(call_shape.clone()).or_default();
                    s.count(call, tokens, duration);
                    if keep_examples {
                        keep_shortest(&mut s.example, command, home.as_deref());
                    }
                }
                let mut patterns = detect_call(call);
                if call_shape.is_some() {
                    if call.failed && call_shape == last_failure {
                        patterns.insert(Pattern::RepeatedFailure);
                    }
                    last_failure = if call.failed {
                        call_shape.clone()
                    } else {
                        None
                    };
                }
                for pattern in patterns.iter() {
                    // Rate-limit messages count only in GitHub output, not in
                    // a file that happens to mention one.
                    if pattern == Pattern::RateLimited && !is_github(category) {
                        continue;
                    }
                    let p = &mut acc.patterns[pattern as usize];
                    p.calls += 1;
                    p.tokens += tokens;
                    p.duration_ms += duration;
                    if let Some(call_shape) = &call_shape {
                        let s = p.shapes.entry(call_shape.clone()).or_default();
                        s.count(call, tokens, duration);
                    }
                    if !p.sessions.contains(&transcript.session) {
                        p.sessions.insert(transcript.session.clone());
                    }
                    if keep_examples && let Some(command) = &call.command {
                        keep_shortest(&mut p.example, command, home.as_deref());
                    }
                }
            }
            previous = mask;
        }
    }

    /// Folds another analyzer's counts into this one.
    pub fn merge(&mut self, other: Self) {
        for (harness, theirs) in other.harnesses {
            let ours = self.harnesses.entry(harness).or_default();
            ours.sessions.extend(theirs.sessions);
            ours.transcripts += theirs.transcripts;
            ours.requests += theirs.requests;
            ours.requests_with_tokens += theirs.requests_with_tokens;
            ours.input_tokens += theirs.input_tokens;
            ours.output_tokens += theirs.output_tokens;
            ours.calls += theirs.calls;
            ours.commands += theirs.commands;
            ours.command_ms += theirs.command_ms;
            ours.tool_ms += theirs.tool_ms;
            ours.output_bytes += theirs.output_bytes;
            ours.failed += theirs.failed;
            for (o, t) in ours.categories.iter_mut().zip(theirs.categories) {
                o.requests += t.requests;
                o.tokens += t.tokens;
                o.calls += t.calls;
                o.duration_ms += t.duration_ms;
                o.output_bytes += t.output_bytes;
                o.failed += t.failed;
                o.repeat_tokens += t.repeat_tokens;
                o.failed_tokens += t.failed_tokens;
                merge_shapes(&mut o.shapes, t.shapes);
            }
            for (o, t) in ours.patterns.iter_mut().zip(theirs.patterns) {
                o.calls += t.calls;
                o.tokens += t.tokens;
                o.duration_ms += t.duration_ms;
                merge_shapes(&mut o.shapes, t.shapes);
                o.sessions.extend(t.sessions);
                merge_example(&mut o.example, t.example);
            }
        }
    }

    /// The report: harnesses in name order, each with its categories ranked
    /// by input tokens (or by time, when the harness recorded no tokens), and
    /// `top_shapes` shapes per category.
    pub fn report(&self, top_shapes: usize) -> Report {
        let mut names: Vec<&String> = self.harnesses.keys().collect();
        names.sort();
        Report {
            harnesses: names
                .into_iter()
                .map(|name| harness_report(name, &self.harnesses[name], top_shapes))
                .collect(),
        }
    }
}

fn merge_shapes(ours: &mut HashMap<String, ShapeAcc>, theirs: HashMap<String, ShapeAcc>) {
    for (shape, ts) in theirs {
        ours.entry(shape).or_default().merge(ts);
    }
}

fn bit(category: Category) -> u32 {
    1 << category.index()
}

fn is_github(category: Category) -> bool {
    matches!(
        category,
        Category::CiPolling | Category::PrThreads | Category::PrStatus | Category::Github
    )
}

/// Keeps the shortest example, so the one kept does not depend on the order
/// files were read in.
fn keep_shortest(slot: &mut Option<String>, command: &str, home: Option<&str>) {
    let candidate = example(command, home, EXAMPLE_CHARS);
    merge_example(slot, Some(candidate));
}

fn merge_example(slot: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    let better = match slot {
        None => true,
        Some(current) => (candidate.len(), &candidate) < (current.len(), current),
    };
    if better {
        *slot = Some(candidate);
    }
}

fn harness_report(name: &str, acc: &Acc, top_shapes: usize) -> HarnessReport {
    let by_tokens = acc.input_tokens > 0;
    let total = if by_tokens {
        acc.input_tokens as f64
    } else {
        acc.tool_ms as f64
    };
    let mut categories: Vec<CategoryReport> = Category::ALL
        .into_iter()
        .filter_map(|category| {
            let c = &acc.categories[category.index()];
            if c.requests == 0.0 && c.calls == 0 {
                return None;
            }
            let measure = if by_tokens {
                c.tokens
            } else {
                c.duration_ms as f64
            };
            Some(CategoryReport {
                category,
                share: ratio(measure, total),
                input_tokens: c.tokens.round() as u64,
                requests: c.requests.round() as u64,
                calls: c.calls,
                duration_ms: c.duration_ms,
                output_bytes: c.output_bytes,
                failed: c.failed,
                shape_count: c.shapes.len() as u64,
                top_shapes: top(&c.shapes, top_shapes),
            })
        })
        .collect();
    categories.sort_by(|a, b| {
        b.share
            .total_cmp(&a.share)
            .then(b.duration_ms.cmp(&a.duration_ms))
            .then(a.category.cmp(&b.category))
    });
    let suggestions = categories
        .iter()
        .filter_map(|row| suggest(row.category, &acc.categories[row.category.index()], acc))
        .collect();
    let mut patterns: Vec<PatternReport> = Pattern::ALL
        .into_iter()
        .filter_map(|pattern| {
            let p = &acc.patterns[pattern as usize];
            (p.calls > 0).then(|| PatternReport {
                pattern,
                calls: p.calls,
                sessions: p.sessions.len() as u64,
                input_tokens: p.tokens.round() as u64,
                duration_ms: p.duration_ms,
                advice: pattern.advice(),
                top_shapes: top(&p.shapes, top_shapes),
                example: p.example.clone(),
            })
        })
        .collect();
    patterns.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.pattern.cmp(&b.pattern)));
    HarnessReport {
        harness: name.to_string(),
        ranked_by: if by_tokens {
            "input_tokens"
        } else {
            "duration"
        },
        totals: Totals {
            sessions: acc.sessions.len() as u64,
            transcripts: acc.transcripts,
            requests: acc.requests,
            requests_with_tokens: acc.requests_with_tokens,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            calls: acc.calls,
            commands: acc.commands,
            command_ms: acc.command_ms,
            tool_ms: acc.tool_ms,
            output_bytes: acc.output_bytes,
            failed_calls: acc.failed,
        },
        categories,
        suggestions,
        patterns,
    }
}

fn ratio(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        (part / whole * 10_000.0).round() / 10_000.0
    } else {
        0.0
    }
}

fn top(shapes: &HashMap<String, ShapeAcc>, n: usize) -> Vec<ShapeCount> {
    let mut shapes: Vec<(&String, &ShapeAcc)> = shapes.iter().collect();
    shapes.sort_by(|a, b| {
        b.1.calls
            .cmp(&a.1.calls)
            .then(b.1.tokens.total_cmp(&a.1.tokens))
            .then(b.1.duration_ms.cmp(&a.1.duration_ms))
            .then(a.0.cmp(b.0))
    });
    shapes
        .into_iter()
        .take(n)
        .map(|(shape, s)| ShapeCount {
            // Shapes hold no values, but a program's own name could still
            // look like a key.
            shape: redact(shape),
            calls: s.calls,
            input_tokens: s.tokens.round() as u64,
            duration_ms: s.duration_ms,
            failures: s.failures,
            successes: s.calls - s.failures,
            last_failed: s.last.is_some_and(|l| l.failed),
            last_exit_code: s.last.and_then(|l| l.exit_code),
            example: s.example.clone(),
        })
        .collect()
}

/// The jouet command or rule for a category, with an estimate of what it
/// would have saved where there is a defensible way to count one.
fn suggest(category: Category, c: &CatAcc, acc: &Acc) -> Option<Suggestion> {
    let repeat = |basis| Saving::new(c.repeat_tokens, acc.input_tokens, basis);
    let (fix, why, saving) = match category {
        Category::CiPolling => (
            "jouet wait ci",
            "one call blocks until the checks settle and shows why any failed, instead of a model turn per check (built-in rule `sleep-poll`)",
            repeat(
                "requests that polled CI right after a request that did; a wait would have made each run of polls one call",
            ),
        ),
        Category::ProcessPolling => (
            "jouet wait pid|log|port|cmd",
            "one call blocks until the process exits, the log matches or the port opens",
            repeat(
                "requests that checked on a process right after a request that did; a wait would have made each run one call",
            ),
        ),
        Category::PrThreads => (
            "jouet pr threads",
            "lists unresolved threads condensed, and replies to and resolves many in one `pr threads apply` (built-in rule `review-threads`)",
            repeat(
                "requests that worked on review threads right after a request that did; most runs become one list and one apply",
            ),
        ),
        Category::PrStatus => (
            "jouet pr status",
            "reads many PRs in one batched query and names what each needs",
            repeat(
                "requests that read PR state right after a request that did; one `pr status` reads them all",
            ),
        ),
        Category::Edits => (
            "jouet edit",
            "checks every edit before writing any file, so a failed edit changes nothing and says which match was wrong (built-in rule `python-edit`)",
            Saving::new(
                c.failed_tokens,
                acc.input_tokens,
                "requests whose edit failed and had to be redone",
            ),
        ),
        Category::BuildTestLint => (
            "mbx",
            "builds and tests cost time more than tokens; mbx shares one build cache and scheduler between worktrees. jouet has no command for this",
            None,
        ),
        Category::Github => (
            "jouet pr status, pr threads and wait",
            "cover the common GitHub reads with compact output",
            None,
        ),
        _ => return None,
    };
    Some(Suggestion {
        category,
        fix,
        why,
        saving,
    })
}

/// Every analysis result, one entry per harness.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub harnesses: Vec<HarnessReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessReport {
    pub harness: String,
    /// What the categories' `share` is a share of: `input_tokens`, or
    /// `duration` (tool time) for a harness that recorded no tokens.
    pub ranked_by: &'static str,
    pub totals: Totals,
    pub categories: Vec<CategoryReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<PatternReport>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    #[serde(skip_serializing_if = "is_zero")]
    pub sessions: u64,
    /// Sessions plus their subagents' transcripts.
    #[serde(skip_serializing_if = "is_zero")]
    pub transcripts: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub requests: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub requests_with_tokens: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub calls: u64,
    /// Shell commands.
    #[serde(skip_serializing_if = "is_zero")]
    pub commands: u64,
    /// Time shell commands ran.
    #[serde(skip_serializing_if = "is_zero")]
    pub command_ms: u64,
    /// Time every tool call took, shell commands included.
    #[serde(skip_serializing_if = "is_zero")]
    pub tool_ms: u64,
    /// Tool output returned to the model.
    #[serde(skip_serializing_if = "is_zero")]
    pub output_bytes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub failed_calls: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryReport {
    pub category: Category,
    /// The category's part of the harness's input tokens (or tool time), 0–1.
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub share: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: u64,
    /// Requests attributed to the category, rounded: a request that issued
    /// calls in two categories counts half in each.
    #[serde(skip_serializing_if = "is_zero")]
    pub requests: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub calls: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub output_bytes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub failed: u64,
    /// How many distinct command shapes the category holds.
    #[serde(skip_serializing_if = "is_zero")]
    pub shape_count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_shapes: Vec<ShapeCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShapeCount {
    pub shape: String,
    pub calls: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
    /// Calls that failed: an error result or a non-zero exit.
    #[serde(skip_serializing_if = "is_zero")]
    pub failures: u64,
    /// Calls that did not fail.
    #[serde(skip_serializing_if = "is_zero")]
    pub successes: u64,
    /// The latest call failed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub last_failed: bool,
    /// The latest call's exit code, when the source recorded one. Claude Code
    /// records only whether a call failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub category: Category,
    /// The command or tool that addresses the category.
    pub fix: &'static str,
    pub why: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saving: Option<Saving>,
}

/// A rough, deliberately simple estimate of the input tokens a suggestion
/// would have saved.
#[derive(Debug, Clone, Serialize)]
pub struct Saving {
    pub input_tokens: u64,
    /// Of the harness's input tokens, 0–1.
    pub share: f64,
    /// What was counted.
    pub basis: &'static str,
}

impl Saving {
    fn new(tokens: f64, total: u64, basis: &'static str) -> Option<Self> {
        let input_tokens = tokens.round() as u64;
        (input_tokens > 0).then(|| Self {
            input_tokens,
            share: ratio(tokens, total as f64),
            basis,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PatternReport {
    pub pattern: Pattern,
    pub calls: u64,
    pub sessions: u64,
    /// Input tokens of the requests that issued these calls.
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: u64,
    /// Time these calls ran.
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
    pub advice: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    /// The most frequent shapes showing the pattern.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_shapes: Vec<ShapeCount>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

/// A call made outside any transcript, such as a test's or a ledger
/// entry's, as a one-request transcript.
#[cfg(test)]
pub(crate) fn single(
    session: &str,
    input_tokens: Option<u64>,
    calls: Vec<crate::event::Call>,
) -> Transcript {
    let n = calls.len();
    Transcript {
        session: session.into(),
        requests: vec![crate::event::Request {
            input_tokens,
            calls: 0..n,
            ..Default::default()
        }],
        calls,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Call, Request};

    fn transcript(steps: &[(u64, &[&str])]) -> Transcript {
        let mut t = Transcript {
            session: "s".into(),
            ..Default::default()
        };
        for (tokens, commands) in steps {
            let start = t.calls.len();
            for command in *commands {
                t.calls.push(Call::shell(*command));
            }
            t.requests.push(Request {
                input_tokens: Some(*tokens),
                calls: start..t.calls.len(),
                ..Default::default()
            });
        }
        t
    }

    #[test]
    fn splits_a_requests_tokens_between_its_calls() {
        let mut a = Analyzer::default();
        a.add(
            "codex",
            &transcript(&[(100, &["git status", "cargo test"]), (50, &[])]),
        );
        let r = &a.report(3).harnesses[0];
        assert_eq!(r.totals.input_tokens, 150);
        let tokens = |c: Category| {
            r.categories
                .iter()
                .find(|row| row.category == c)
                .map_or(0, |row| row.input_tokens)
        };
        assert_eq!(tokens(Category::Git), 50);
        assert_eq!(tokens(Category::BuildTestLint), 50);
        assert_eq!(tokens(Category::Reply), 50);
        assert_eq!(r.categories.len(), 3);
    }

    #[test]
    fn counts_runs_of_polls_as_the_saving() {
        let mut a = Analyzer::default();
        a.add(
            "codex",
            &transcript(&[
                (100, &["gh pr checks 1"]),
                (110, &["gh pr checks 1"]),
                (120, &["gh pr checks 1"]),
                (130, &["git push"]),
                (140, &["gh pr checks 1"]),
            ]),
        );
        let r = &a.report(3).harnesses[0];
        assert_eq!(r.categories[0].category, Category::CiPolling);
        assert_eq!(r.categories[0].input_tokens, 470);
        assert_eq!(r.categories[0].top_shapes[0].shape, "gh pr checks <n>");
        let ci = &r.suggestions[0];
        assert_eq!(ci.fix, "jouet wait ci");
        // The second and third polls repeat the one before them.
        assert_eq!(ci.saving.as_ref().map(|s| s.input_tokens), Some(230));
    }

    #[test]
    fn ranks_by_time_without_tokens_and_merges() {
        let mut call = Call::shell("cargo build");
        call.duration_ms = Some(9_000);
        let mut fast = Call::shell("ls");
        fast.duration_ms = Some(1_000);
        let mut a = Analyzer::default();
        a.add("ledger", &single("one", None, vec![fast]));
        let mut b = Analyzer::default();
        b.add("ledger", &single("two", None, vec![call]));
        a.merge(b);
        let r = &a.report(1).harnesses[0];
        assert_eq!(r.ranked_by, "duration");
        assert_eq!(r.totals.sessions, 2);
        assert_eq!(r.categories[0].category, Category::BuildTestLint);
        assert_eq!(r.categories[0].share, 0.9);
    }

    #[test]
    fn finds_patterns_and_keeps_redacted_examples() {
        let mut a = Analyzer::new(true, Some("/home/me".into()));
        let mut limited = Call::shell("gh api user");
        limited.rate_limited = true;
        let mut grep = Call::shell("rg 'rate limit exceeded'");
        grep.rate_limited = true;
        a.add(
            "claude",
            &single(
                "s",
                Some(10),
                vec![
                    Call::shell("GH_TOKEN= gh pr view 1 --json state"),
                    Call::shell("cd /home/me/x && cargo test 2>&1 | head -50"),
                    limited,
                    grep,
                ],
            ),
        );
        let r = &a.report(3).harnesses[0];
        let names: Vec<Pattern> = r.patterns.iter().map(|p| p.pattern).collect();
        assert_eq!(
            names,
            [
                Pattern::HeadOnTestOutput,
                Pattern::StaleTokenPrefix,
                Pattern::RateLimited
            ]
        );
        assert_eq!(
            r.patterns[0].example.as_deref(),
            Some("cd ~/x && cargo test 2>&1 | head -50")
        );
    }
}
